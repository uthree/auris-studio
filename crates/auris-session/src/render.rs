//! Offline rendering, detached from the session that produced it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use auris_core::param::gain_to_db;
use auris_core::project::{UNSTRETCHED, stretch_key};
use auris_core::{AudioBuffer, AudioSourceBank, PluginRegistry, Project, SourceId};
use auris_dsp::stretch::time_stretch;
use auris_engine::{OfflineOptions, PlacedEffects, PlacedInstruments, render_project_using};
use auris_io::{WavExportSettings, resample_buffer, write_wav};

use crate::error::SessionError;

/// `buffer` at `rate`, or `None` when it could not be converted.
///
/// The render graph reads a clip's samples one for one against the timeline, so a source has to
/// be at the rate the graph renders at before it is handed over. A buffer already there is shared
/// rather than copied, which is what makes the usual case — a device agreeing with the project —
/// free.
pub(crate) fn source_at_rate(
    id: SourceId,
    buffer: &Arc<AudioBuffer>,
    rate: f64,
) -> Option<Arc<AudioBuffer>> {
    if buffer.sample_rate() == rate {
        return Some(Arc::clone(buffer));
    }
    match resample_buffer(buffer, rate) {
        Ok(resampled) => Some(Arc::new(resampled)),
        // Handing the graph a source at the wrong rate would play it at the wrong speed and
        // finish it in the wrong place, which is worse than not playing it: a clip that is
        // silent is obviously wrong, one that drifts is not. The warning says which it was.
        Err(error) => {
            log::warn!(
                "source {} could not be converted to {rate} Hz ({error}); its clips are silent",
                id.0
            );
            None
        }
    }
}

/// Stretches whatever the project's clips ask for, and drops whatever they no longer do.
///
/// The renderer plays a clip that follows the tempo out of a *stretched copy* of its source, and
/// looks that copy up by the same number the clip works its stretch out to. Making them is this
/// side's job: a time stretcher allocates the whole output and costs tens of operations a sample,
/// which is a thing to do while a graph is being built and never on the audio thread.
///
/// Cached across rebuilds, because a rebuild happens on every structural edit and re-stretching a
/// three-minute source each time somebody moves a clip would be seconds of the window not
/// answering. What is *not* cached is anything the project has stopped asking for — dragging a
/// tempo from 90 to 140 asks for a copy at every value it passes through, and only the last of
/// them is ever played.
pub(crate) fn fill_stretches(project: &Project, bank: &mut AudioSourceBank) {
    let mut wanted: HashSet<(SourceId, u32)> = HashSet::new();
    for track in &project.tracks {
        let Some(audio) = track.kind.as_audio() else {
            continue;
        };
        for clip in &audio.clips {
            let stretch = clip.stretch_in(&project.tempo_map);
            let key = stretch_key(stretch);
            if key == UNSTRETCHED || !wanted.insert((clip.source, key)) {
                continue;
            }
            if bank.stretched(clip.source, key).is_some() {
                continue;
            }
            let Some(source) = bank.get(clip.source) else {
                continue;
            };
            bank.insert_stretched(clip.source, key, Arc::new(time_stretch(source, stretch)));
        }
    }
    bank.retain_stretched(&|source, key| wanted.contains(&(source, key)));
}

/// A copy of `bank` with every source at `rate`.
pub(crate) fn bank_at_rate(bank: &AudioSourceBank, rate: f64) -> AudioSourceBank {
    let mut converted = AudioSourceBank::new();
    for (id, buffer) in bank.iter() {
        if let Some(buffer) = source_at_rate(id, buffer, rate) {
            converted.insert(id, buffer);
        }
    }
    converted
}

/// What an export produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSummary {
    /// Length of the rendered audio in seconds, including the effect tail.
    pub seconds: f64,
    /// Frames written per channel.
    pub frames: u64,
    /// Channels written.
    pub channels: usize,
    /// Loudest sample in the render, in dBFS.
    pub peak_db: f32,
}

/// A self-contained copy of everything a render needs.
///
/// Rendering a long project takes seconds, so a GUI runs it off the main thread — but the
/// session is not `Send` in spirit (it owns an audio device) and must stay editable meanwhile.
/// A job captures the document, the sample bank and the registry, all of which are cheap to
/// clone and `Send`, so the render sees a consistent snapshot and later edits cannot disturb it.
///
/// Hosted plugins are the exception that stops this being cloneable. The half of a CLAP plugin
/// that renders is `Send` and travels here happily; the half that could *make* another one is not
/// and stays in the session. So a job carries the effects themselves, and there is exactly one
/// job per set of them.
pub struct RenderJob {
    project: Project,
    bank: AudioSourceBank,
    registry: Arc<PluginRegistry>,
    /// Effects the session built because the registry could not. Taken by the render.
    placed: PlacedEffects,
    /// Instruments the session built, for the same reason.
    instruments: PlacedInstruments,
}

impl RenderJob {
    pub(crate) fn new(
        project: Project,
        bank: AudioSourceBank,
        registry: Arc<PluginRegistry>,
        placed: PlacedEffects,
        instruments: PlacedInstruments,
    ) -> Self {
        Self {
            project,
            bank,
            registry,
            placed,
            instruments,
        }
    }

    /// The document this job will render.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Restricts `options` to the project's cycle region, converted to frames at the rate the
    /// render will run at.
    ///
    /// `None` when the project has no region, or the region is empty. Whether cycling is
    /// *enabled* is deliberately not consulted: the region is a marked span of the song, and
    /// exporting that span is meaningful whether or not playback happens to be looping over it
    /// right now.
    pub fn loop_options(&self, options: OfflineOptions) -> Option<OfflineOptions> {
        let (start, end) = self.project.loop_region?;
        let (start, end) = (start.max_zero(), end.max_zero());
        if end <= start {
            return None;
        }
        let rate = options.sample_rate.unwrap_or(self.project.sample_rate);
        let start_frames = self.project.tempo_map.ticks_to_samples(start, rate).raw();
        let end_frames = self.project.tempo_map.ticks_to_samples(end, rate).raw();
        Some(options.with_range(start_frames, end_frames))
    }

    /// Renders to a buffer, reporting progress from 0.0 to 1.0.
    ///
    /// An export can be asked for any rate, including one the sources were never decoded at, so
    /// the bank is converted to whatever this render will run at before it is handed over.
    pub fn render(
        &mut self,
        options: &OfflineOptions,
        progress: &mut dyn FnMut(f32),
    ) -> Result<AudioBuffer, SessionError> {
        let rate = options.sample_rate.unwrap_or(self.project.sample_rate);
        let mut bank = bank_at_rate(&self.bank, rate);
        // An export builds its own graph from its own bank, so it stretches its own copies. They
        // are made at the rate the export runs at, which is the rate the clips are measured
        // against — a copy borrowed from the session's bank would be at the *device's*.
        fill_stretches(&self.project, &mut bank);
        Ok(render_project_using(
            &self.project,
            &bank,
            &self.registry,
            &mut self.placed,
            &mut self.instruments,
            options,
            progress,
        )?)
    }

    /// Renders and writes a WAV file.
    ///
    /// The file is written at the rate the project was rendered at, not at whatever the
    /// settings say, because the two disagreeing would resample the audio by accident: the
    /// header would claim a rate the samples were never produced at.
    pub fn render_to_wav(
        &mut self,
        path: &Path,
        settings: &WavExportSettings,
        options: &OfflineOptions,
        progress: &mut dyn FnMut(f32),
    ) -> Result<ExportSummary, SessionError> {
        let buffer = self.render(options, progress)?;
        let rendered_rate = options.sample_rate.unwrap_or(self.project.sample_rate);
        let settings = WavExportSettings {
            sample_rate: rendered_rate.round().max(1.0) as u32,
            ..*settings
        };
        write_wav(path, &buffer, &settings)?;
        Ok(ExportSummary {
            seconds: buffer.duration_seconds(),
            frames: buffer.frame_count() as u64,
            channels: buffer.channel_count(),
            peak_db: gain_to_db(buffer.peak()),
        })
    }
}

impl std::fmt::Debug for RenderJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderJob")
            .field("project", &self.project.name)
            .field("tracks", &self.project.tracks.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::plugin_catalogue;
    use auris_core::time::Ticks;

    const SOURCE: SourceId = SourceId(1);

    /// A bank holding one second of steady 0.5 on two channels, at `rate`.
    fn bank_at(id: SourceId, rate: f64) -> AudioSourceBank {
        let frames = rate as usize;
        let mut bank = AudioSourceBank::new();
        bank.insert(
            id,
            Arc::new(AudioBuffer::from_planar(vec![vec![0.5; frames]; 2], rate).expect("planar")),
        );
        bank
    }

    #[test]
    fn a_clip_that_follows_the_tempo_gets_a_stretched_copy_of_its_source() {
        // The renderer plays a following clip out of a stretched copy and looks it up by the
        // number the clip works its stretch out to. Making it is this side's job, so what is
        // asserted is that the copy exists, under that key, and is the length the clip will be
        // measured as — a copy the renderer cannot find is a clip that plays at the wrong speed.
        let mut project = Project::new("Follow", 48_000.0);
        project.tempo_map = auris_core::time::TempoMap::constant(60.0);
        let track = project.add_audio_track("Audio");
        let source = project.add_audio_source(
            "loop",
            auris_core::AssetPath::inside("Audio/loop.wav"),
            48_000,
            48_000.0,
            2,
        );
        let clip = project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("a clip");
        let audio = project.audio_clip_mut(clip).expect("the clip");
        audio.source_bpm = Some(120.0);
        audio.follows_tempo = true;

        let mut bank = bank_at(source, 48_000.0);
        fill_stretches(&project, &mut bank);
        let key = stretch_key(2.0);
        let stretched = bank.stretched(source, key).expect("a stretched copy");
        assert_eq!(
            stretched.frame_count(),
            96_000,
            "a loop recorded at 120 has to last twice as long to cover its bars at 60"
        );

        // Nothing is stretched twice, and nothing the project has stopped asking for is kept: a
        // tempo dragged across a range asks for a copy at every value it passes through.
        let again = Arc::clone(stretched);
        fill_stretches(&project, &mut bank);
        assert!(
            Arc::ptr_eq(&again, bank.stretched(source, key).expect("still there")),
            "the copy was made a second time"
        );
        project
            .audio_clip_mut(clip)
            .expect("the clip")
            .follows_tempo = false;
        fill_stretches(&project, &mut bank);
        assert_eq!(bank.stretches().count(), 0, "a stale copy was kept");
    }

    #[test]
    fn the_loop_options_cover_the_region_at_the_render_rate() {
        // Four beats at 120 BPM is two seconds: 96 000 frames at the project's 48 kHz.
        let mut project = Project::new("Cycle", 48_000.0);
        project.loop_region = Some((Ticks::ZERO, Ticks::from_beats(4.0)));
        let job = RenderJob::new(
            project,
            AudioSourceBank::new(),
            plugin_catalogue(),
            PlacedEffects::new(),
            PlacedInstruments::new(),
        );

        let options = job
            .loop_options(OfflineOptions::whole_project())
            .expect("a region exists");
        assert_eq!(options.start_frames, 0);
        assert_eq!(options.end_frames, Some(96_000));

        // An overridden rate measures the same two seconds in its own frames.
        let doubled = job
            .loop_options(OfflineOptions {
                sample_rate: Some(96_000.0),
                ..OfflineOptions::whole_project()
            })
            .expect("the region does not depend on the rate");
        assert_eq!(doubled.end_frames, Some(192_000));
    }

    #[test]
    fn a_missing_or_empty_region_offers_no_loop_options() {
        let mut project = Project::new("None", 48_000.0);
        let job = RenderJob::new(
            project.clone(),
            AudioSourceBank::new(),
            plugin_catalogue(),
            PlacedEffects::new(),
            PlacedInstruments::new(),
        );
        assert!(job.loop_options(OfflineOptions::whole_project()).is_none());

        // A region of no width is nothing to export either.
        project.loop_region = Some((Ticks::from_beats(2.0), Ticks::from_beats(2.0)));
        let job = RenderJob::new(
            project,
            AudioSourceBank::new(),
            plugin_catalogue(),
            PlacedEffects::new(),
            PlacedInstruments::new(),
        );
        assert!(job.loop_options(OfflineOptions::whole_project()).is_none());
    }

    #[test]
    fn a_source_already_at_the_rate_is_shared_rather_than_copied() {
        let bank = bank_at(SOURCE, 48_000.0);
        let original = Arc::clone(bank.get(SOURCE).expect("inserted"));
        let same = bank_at_rate(&bank, 48_000.0);
        assert!(
            Arc::ptr_eq(same.get(SOURCE).expect("kept"), &original),
            "a rate that already matches must not copy the samples"
        );
    }

    #[test]
    fn a_source_at_another_rate_is_converted() {
        let converted = bank_at_rate(&bank_at(SOURCE, 48_000.0), 24_000.0);
        let buffer = converted.get(SOURCE).expect("converted");
        assert_eq!(buffer.sample_rate(), 24_000.0);
        assert_eq!(buffer.frame_count(), 24_000);
        // A steady level stays a steady level; measure away from the edges, where the
        // resampler's window is still filling.
        assert!((buffer.channel(0)[12_000] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn an_export_at_another_rate_keeps_a_clips_duration() {
        // One second of audio is one second whatever rate it is written at. Before the bank was
        // converted, its 48 000 frames were played as 48 000 frames of a 96 kHz timeline — half
        // a second, at double speed.
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            48_000,
            48_000.0,
            2,
        );
        project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("clip");

        let mut job = RenderJob::new(
            project,
            bank_at(source, 48_000.0),
            plugin_catalogue(),
            PlacedEffects::new(),
            PlacedInstruments::new(),
        );
        let rendered = job
            .render(
                &OfflineOptions {
                    sample_rate: Some(96_000.0),
                    ..OfflineOptions::default()
                },
                &mut |_| {},
            )
            .expect("render");

        assert_eq!(rendered.frame_count(), 96_000);
        assert_eq!(rendered.sample_rate(), 96_000.0);
        assert!((rendered.channel(0)[48_000] - 0.5).abs() < 1e-3);
        assert!(
            (rendered.channel(0)[95_000] - 0.5).abs() < 1e-3,
            "the clip ran out before the end of the export"
        );
    }
}
