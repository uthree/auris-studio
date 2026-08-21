//! Faster-than-realtime rendering of a whole project into one buffer.
//!
//! Export runs the *same* [`render_block`] the audio callback does, block by block, with the
//! transport rolling and looping switched off. That is what makes an exported file match what
//! was heard: there is no second code path that could drift from the first.

use auris_core::AudioBuffer;
use auris_core::project::{AudioSourceBank, Project};
use auris_core::registry::PluginRegistry;

use crate::error::EngineError;
use crate::graph::{RENDER_CHANNELS, RenderGraph};
use crate::renderer::render_block;
use crate::transport::Transport;

/// Longest span an offline render will attempt, in frames: twenty-four hours at 192 kHz.
///
/// This is a sanity bound, not a quota. Nothing a user can arrange comes anywhere near it, so
/// anything that does is a corrupt figure — and turning that into an error is what keeps the
/// export path from panicking in the allocator on a number it was handed rather than chose.
const MAX_RENDER_FRAMES: u64 = 24 * 60 * 60 * 192_000;

/// How much of a project to render, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflineOptions {
    /// First frame to render.
    pub start_frames: u64,
    /// One past the last frame to render. `None` means the end of the arrangement.
    pub end_frames: Option<u64>,
    /// Frames per processing block. Larger blocks export faster; the output is identical.
    pub block_frames: usize,
    /// Whether to keep rendering past the end for the longest effect tail in the graph.
    pub include_tail: bool,
    /// Rate to render at. `None` uses the project's own rate.
    pub sample_rate: Option<f64>,
}

impl Default for OfflineOptions {
    fn default() -> Self {
        Self {
            start_frames: 0,
            end_frames: None,
            block_frames: 1_024,
            include_tail: true,
            sample_rate: None,
        }
    }
}

impl OfflineOptions {
    /// Options rendering the whole arrangement plus its tails.
    pub fn whole_project() -> Self {
        Self::default()
    }

    /// Restricts the render to a frame range.
    pub fn with_range(mut self, start_frames: u64, end_frames: u64) -> Self {
        self.start_frames = start_frames;
        self.end_frames = Some(end_frames);
        self
    }

    /// Sets the processing block size.
    pub fn with_block_frames(mut self, block_frames: usize) -> Self {
        self.block_frames = block_frames.max(1);
        self
    }
}

/// A render's two-way channel with whoever asked for it: how far it has got, and whether to stop.
///
/// The two travel together because they are read in the same place — once per block, at the
/// bottom of the loop — and because a caller that wants either almost always wants both. An
/// export with a progress bar and no way out is a window somebody has to wait in front of, and
/// there is no other moment at which a render can be interrupted: a block is short, and stopping
/// between two of them costs nothing.
///
/// Both halves are optional. [`Default`] is a render nobody is watching and nobody can stop,
/// which is what [`render_project`] and every test wants.
#[derive(Default)]
pub struct RenderProgress<'a> {
    report: Option<&'a mut dyn FnMut(f32)>,
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
}

impl<'a> RenderProgress<'a> {
    /// A render whose progress is reported to `report`.
    pub fn reporting(report: &'a mut dyn FnMut(f32)) -> Self {
        Self {
            report: Some(report),
            cancel: None,
        }
    }

    /// The same render, stopped when `flag` becomes true.
    ///
    /// A flag rather than a channel or a handle: the render checks it between blocks and the
    /// caller sets it from wherever the button was pressed, and neither has to know anything
    /// about the other's thread.
    pub fn cancelled_by(mut self, flag: &'a std::sync::atomic::AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }

    /// Reports how far along the render is, from 0.0 to 1.0.
    fn report(&mut self, fraction: f32) {
        if let Some(report) = self.report.as_mut() {
            report(fraction);
        }
    }

    /// Whether the render has been asked to stop.
    fn cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// Renders a project into a single buffer.
///
/// See [`render_project_with_progress`] when the caller wants to drive a progress bar.
pub fn render_project(
    project: &Project,
    bank: &AudioSourceBank,
    registry: &PluginRegistry,
    options: &OfflineOptions,
) -> Result<AudioBuffer, EngineError> {
    render_project_with_progress(
        project,
        bank,
        registry,
        options,
        &mut RenderProgress::default(),
    )
}

/// Renders a project into a single buffer, reporting progress from 0.0 to 1.0.
///
/// `progress` is reported once before any work starts and once per block afterwards, finishing
/// at exactly 1.0. It runs on the calling thread, so a UI must marshal it as usual, and it is
/// also where a cancellation is noticed -- see [`RenderProgress`].
pub fn render_project_with_progress(
    project: &Project,
    bank: &AudioSourceBank,
    registry: &PluginRegistry,
    options: &OfflineOptions,
    progress: &mut RenderProgress<'_>,
) -> Result<AudioBuffer, EngineError> {
    render_project_using(
        project,
        bank,
        registry,
        &mut crate::graph::PlacedEffects::new(),
        &mut crate::graph::PlacedInstruments::new(),
        options,
        progress,
    )
}

/// Renders a project, taking some of its plugins from the caller.
///
/// See [`PlacedEffects`](crate::graph::PlacedEffects) and
/// [`PlacedInstruments`](crate::graph::PlacedInstruments) for why a caller would have any. An
/// export that went through [`render_project`] instead would silently leave every hosted plugin
/// out — the bounce would be the mix minus whatever somebody loaded, which is the worst way for a
/// feature to be missing.
pub fn render_project_using(
    project: &Project,
    bank: &AudioSourceBank,
    registry: &PluginRegistry,
    placed: &mut crate::graph::PlacedEffects,
    instruments: &mut crate::graph::PlacedInstruments,
    options: &OfflineOptions,
    progress: &mut RenderProgress<'_>,
) -> Result<AudioBuffer, EngineError> {
    let sample_rate = options.sample_rate.unwrap_or(project.sample_rate);
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(EngineError::InvalidSampleRate(sample_rate));
    }
    let block_frames = options.block_frames.max(1);

    let mut graph = RenderGraph::build_with(
        project,
        bank,
        registry,
        placed,
        instruments,
        block_frames,
        sample_rate,
    );

    let end_frames = options.end_frames.unwrap_or_else(|| {
        // Both figures measure the same thing, but `Project::end_tick` rounds an audio clip's
        // length to whole ticks on the way through while the graph converts it to frames
        // directly, so they can land a frame or two apart. Taking whichever reaches further can
        // only ever add silence; using the tick figure alone would chop the end off a clip.
        let from_ticks = project
            .tempo_map
            .ticks_to_samples(project.end_tick(), sample_rate)
            .raw();
        from_ticks.max(graph.end_frame())
    });
    if end_frames < options.start_frames {
        return Err(EngineError::InvalidRange {
            start: options.start_frames,
            end: end_frames,
        });
    }

    let tail = if options.include_tail {
        graph.tail_frames()
    } else {
        0
    };
    // A corrupt tempo map or a bad `end_frames` can make this span astronomically large, and the
    // buffer that follows would then panic inside the allocator rather than returning an error.
    let span = end_frames - options.start_frames;
    let too_long = || EngineError::RenderTooLong {
        frames: span,
        limit: MAX_RENDER_FRAMES,
    };
    if span > MAX_RENDER_FRAMES {
        return Err(too_long());
    }
    let total = usize::try_from(span)
        .ok()
        .and_then(|span| span.checked_add(tail))
        .ok_or_else(too_long)?;

    let mut out = AudioBuffer::new(RENDER_CHANNELS, total, sample_rate);
    progress.report(0.0);
    if total == 0 {
        progress.report(1.0);
        return Ok(out);
    }

    // Plugin delay compensation holds the whole mix back, so the first `latency` frames out of
    // the graph are the lead-in of empty delay lines rather than anything on the timeline. The
    // render runs that much longer and the file starts where the lead-in ends, which is what
    // keeps an export lined up with what the arrangement shows.
    let latency = graph.latency_frames();
    let end = total.checked_add(latency).ok_or_else(too_long)?;

    // Only an explicit range can have material lying beyond its end; a whole-project render
    // ends where the material does, and stopping it there would cut the natural releases out
    // of its own tail.
    let ranged = options.end_frames.is_some();
    let performed = total - tail;

    let mut transport = Transport::playing_from(options.start_frames);
    let mut scratch = AudioBuffer::new(RENDER_CHANNELS, block_frames, sample_rate);
    let mut rendered = 0;
    while rendered < end {
        // The range's end is a Stop, exactly as realtime playback stops there: the voices are
        // released and what runs on into the tail is the effects' ring-out — not the material
        // that lies beyond the range, which used to keep performing straight through it.
        if ranged && transport.playing && rendered >= performed {
            transport.playing = false;
            graph.reset_voices();
        }
        let mut frames = block_frames.min(end - rendered);
        if ranged && transport.playing {
            // Never render across the boundary; the block after this one starts stopped.
            frames = frames.min(performed - rendered);
        }
        scratch.set_frame_count(frames);
        render_block(&mut graph, &mut transport, &mut scratch, true);
        // How much of this block is still lead-in, and where the rest lands in the file.
        let skip = latency.saturating_sub(rendered).min(frames);
        if skip < frames {
            let at = rendered + skip - latency;
            let count = (frames - skip).min(total - at);
            for channel in 0..RENDER_CHANNELS {
                out.channel_mut(channel)[at..at + count]
                    .copy_from_slice(&scratch.channel(channel)[skip..skip + count]);
            }
        }
        rendered += frames;
        progress.report(rendered as f32 / end as f32);
        // Between blocks, which is the only place a render can be interrupted — and cheap
        // enough that the check costs nothing next to the block that just ran.
        if progress.cancelled() {
            return Err(EngineError::RenderCancelled);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{self, TAIL_FRAMES, TONE_AMPLITUDE};
    use auris_core::AudioBuffer;
    use auris_core::project::Note;
    use auris_core::time::Ticks;
    use std::sync::Arc;

    const SAMPLE_RATE: f64 = 48_000.0;

    /// Four beats of held note at 120 BPM: 96 000 frames.
    fn four_beat_project() -> Project {
        let mut project = Project::new("Export", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(4.0),
        ));
        project
    }

    #[test]
    fn the_output_is_the_arrangement_length_when_nothing_has_a_tail() {
        let project = four_beat_project();
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000);
        assert_eq!(rendered.sample_rate(), SAMPLE_RATE);
        assert!((rendered.channel(0)[95_999] - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn the_longest_effect_tail_is_appended() {
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000 + TAIL_FRAMES);
        // The tail must actually contain the ring-out, not silence.
        assert!(rendered.slice(96_000, TAIL_FRAMES).peak() > 0.0);

        let without = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions {
                include_tail: false,
                ..OfflineOptions::default()
            },
        )
        .expect("render");
        assert_eq!(without.frame_count(), 96_000);
    }

    #[test]
    fn a_range_export_does_not_perform_what_lies_beyond_the_range() {
        // A note that starts after the exported range, in a graph with a tail. The tail must
        // hold the ring-out of what was *in* the range — realtime playback of the same span
        // followed by Stop releases the voices and lets only the effects ring — and it used to
        // hold a full-level performance of material past the range instead, because nothing
        // ever told the transport the range had ended.
        let mut project = Project::new("Range", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(
                track,
                "Late",
                Ticks::from_beats(4.0),
                Ticks::from_beats(4.0),
            )
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(4.0),
        ));
        project.add_effect(None, testkit::TAIL_ID);

        // Bars one to four hold nothing; the note begins at bar five, past the range.
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, 96_000),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000 + TAIL_FRAMES);
        assert!(
            rendered.slice(96_000, TAIL_FRAMES).peak() < TONE_AMPLITUDE * 0.01,
            "the tail performed material that lies beyond the range"
        );
    }

    #[test]
    fn a_tail_behind_another_tail_lengthens_the_export_by_both() {
        // The master chain is fed for the whole of the track's ring-out and only then starts its
        // own, so the export needs room for the two end to end.
        let mut project = four_beat_project();
        let track = project.tracks[0].id;
        project.add_effect(Some(track), testkit::TAIL_ID);
        project.add_effect(None, testkit::TAIL_ID);

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000 + 2 * TAIL_FRAMES);
    }

    #[test]
    fn a_bypassed_effect_does_not_lengthen_the_export() {
        // A bypassed slot is never handed a block, so it cannot ring out; counting its declared
        // tail would pad every export with silence. This is also what makes the placeholder for
        // a missing plugin harmless, since that one is bypassed by construction.
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        project.master.effects[0].enabled = false;

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000);
    }

    #[test]
    fn the_export_matches_block_by_block_realtime_rendering() {
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        project.master.gain_db = -3.0;
        project.tracks[0].mixer.pan = -0.4;
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();

        let options = OfflineOptions::default().with_block_frames(512);
        let exported =
            render_project(&project, &bank, &registry, &options).expect("offline render");

        // The same project driven the way the audio callback drives it.
        let mut graph = RenderGraph::build_at(&project, &bank, &registry, 512, SAMPLE_RATE);
        let mut transport = Transport::playing_from(0);
        let mut block = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        let mut realtime = AudioBuffer::new(RENDER_CHANNELS, exported.frame_count(), SAMPLE_RATE);
        let mut written = 0;
        while written < realtime.frame_count() {
            let frames = 512.min(realtime.frame_count() - written);
            block.set_frame_count(frames);
            render_block(&mut graph, &mut transport, &mut block, false);
            for channel in 0..RENDER_CHANNELS {
                realtime.channel_mut(channel)[written..written + frames]
                    .copy_from_slice(&block.channel(channel)[..frames]);
            }
            written += frames;
        }

        assert_eq!(exported.channel(0), realtime.channel(0));
        assert_eq!(exported.channel(1), realtime.channel(1));
    }

    #[test]
    fn an_export_still_starts_on_the_timeline_when_something_looks_ahead() {
        // Compensation holds the whole mix back, so the graph's first frames are the empty delay
        // lines. Those are a lead-in, not part of the piece, and the file must not begin with
        // them — an export that did would be shifted against every other export of the project.
        let mut project = four_beat_project();
        project.add_effect(None, testkit::LOOKAHEAD_ID);

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");

        assert_eq!(rendered.frame_count(), 96_000);
        assert!(
            (rendered.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5,
            "the file starts with the compensation lead-in: {}",
            rendered.channel(0)[0]
        );
        assert!((rendered.channel(0)[95_999] - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn a_look_ahead_effect_does_not_change_what_an_export_contains() {
        // The same project with and without an effect that only delays: once compensated, the
        // exported samples have to match.
        let project = four_beat_project();
        let plain = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");

        let mut delayed_project = project.clone();
        delayed_project.add_effect(None, testkit::LOOKAHEAD_ID);
        let delayed = render_project(
            &delayed_project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");

        assert_eq!(plain.channel(0), delayed.channel(0));
    }

    #[test]
    fn the_export_block_size_does_not_change_the_result() {
        let project = four_beat_project();
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();
        let a = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions::default().with_block_frames(64),
        )
        .expect("render");
        let b = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions::default().with_block_frames(3_000),
        )
        .expect("render");
        assert_eq!(a.channel(0), b.channel(0));
    }

    #[test]
    fn an_explicit_range_renders_only_that_span() {
        let project = four_beat_project();
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(24_000, 36_000),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 12_000);
        assert!((rendered.peak() - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn an_explicit_range_matches_realtime_seeking_to_the_same_place() {
        // Two overlapping notes, so the range starts inside one of them and the second joins
        // partway through: the render is only right if the note chase is.
        let mut project = Project::new("Range", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes
            .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(4.0)));
        midi.notes.push(Note::new(
            64,
            Ticks::from_beats(2.0),
            Ticks::from_beats(4.0),
        ));
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();

        let exported = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions::default()
                .with_range(30_000, 70_000)
                .with_block_frames(512),
        )
        .expect("render");

        let mut graph = RenderGraph::build(&project, &bank, &registry, 512);
        let mut transport = Transport::playing_from(30_000);
        let mut block = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        let mut realtime = AudioBuffer::new(RENDER_CHANNELS, 40_000, SAMPLE_RATE);
        let mut written = 0;
        while written < 40_000 {
            let frames = 512.min(40_000 - written);
            block.set_frame_count(frames);
            render_block(&mut graph, &mut transport, &mut block, false);
            for channel in 0..RENDER_CHANNELS {
                realtime.channel_mut(channel)[written..written + frames]
                    .copy_from_slice(&block.channel(channel)[..frames]);
            }
            written += frames;
        }
        assert_eq!(exported.channel(0), realtime.channel(0));
        // Pitch 60 alone at the range start; pitch 64 joins at frame 48 000.
        assert!((exported.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5);
        assert!((exported.channel(0)[20_000] - 2.0 * TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn an_overridden_sample_rate_scales_every_position() {
        let mut project = Project::new("Rate", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(0.5),
        ));

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions {
                sample_rate: Some(96_000.0),
                ..OfflineOptions::default()
            },
        )
        .expect("render");

        // Eight beats at 120 BPM is 4 s, which is 384 000 frames at 96 kHz.
        assert_eq!(rendered.frame_count(), 384_000);
        assert_eq!(rendered.sample_rate(), 96_000.0);
        // The note lasts 0.25 s, so it ends at frame 24 000 rather than 12 000.
        assert!((rendered.channel(0)[23_999] - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(rendered.channel(0)[24_000], 0.0);
    }

    #[test]
    fn an_overridden_sample_rate_converts_a_clips_frame_counts() {
        // The clip's trim is counted in the frames of the file as it was imported — 48 000 of
        // them, one second — while this export runs at half that rate, where one second is
        // 24 000 frames. The caller hands the bank over already converted, so the clip covers
        // the whole export rather than half of it or twice it.
        let mut project = Project::new("Clip", SAMPLE_RATE);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            48_000,
            SAMPLE_RATE,
            1,
        );
        project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let mut bank = AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(AudioBuffer::from_planar(vec![vec![0.5; 24_000]], 24_000.0).expect("planar")),
        );

        let rendered = render_project(
            &project,
            &bank,
            &testkit::registry(),
            &OfflineOptions {
                sample_rate: Some(24_000.0),
                ..OfflineOptions::default()
            },
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 24_000);
        assert_eq!(rendered.sample_rate(), 24_000.0);
        assert!((rendered.channel(0)[0] - 0.5).abs() < 1e-5);
        assert!(
            (rendered.channel(0)[23_999] - 0.5).abs() < 1e-5,
            "the clip stopped short of the end of the export"
        );
    }

    #[test]
    fn a_render_asked_to_stop_stops_and_says_which_it_was() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // Raised before the first block, so the very first check fires. The render still
        // reports its progress up to that point — the bar shows where it got to, which is what
        // a stopped export has to be able to draw.
        let flag = AtomicBool::new(true);
        let mut reports = Vec::new();
        let ended = render_project_with_progress(
            &four_beat_project(),
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_block_frames(8_192),
            &mut RenderProgress::reporting(&mut |fraction| reports.push(fraction))
                .cancelled_by(&flag),
        );
        assert!(matches!(ended, Err(EngineError::RenderCancelled)));
        // 0.0 before the loop and one block's worth inside it, then out.
        assert_eq!(reports.len(), 2);
        assert!(reports[1] < 1.0, "it stopped part way, not at the end");

        // And a flag that is never raised changes nothing.
        flag.store(false, Ordering::Relaxed);
        assert!(
            render_project_with_progress(
                &four_beat_project(),
                &AudioSourceBank::new(),
                &testkit::registry(),
                &OfflineOptions::default().with_block_frames(8_192),
                &mut RenderProgress::default().cancelled_by(&flag),
            )
            .is_ok()
        );
    }

    #[test]
    fn progress_runs_from_zero_to_one_and_is_monotonic() {
        let project = four_beat_project();
        let mut reports = Vec::new();
        render_project_with_progress(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_block_frames(8_192),
            &mut RenderProgress::reporting(&mut |fraction| reports.push(fraction)),
        )
        .expect("render");

        assert_eq!(reports.first().copied(), Some(0.0));
        assert_eq!(reports.last().copied(), Some(1.0));
        assert!(reports.windows(2).all(|pair| pair[0] <= pair[1]));
        // 96 000 frames in 8 192-frame blocks is 12 blocks, plus the initial 0.0.
        assert_eq!(reports.len(), 13);
    }

    #[test]
    fn a_backwards_range_is_rejected() {
        let project = four_beat_project();
        let error = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(9_000, 1_000),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EngineError::InvalidRange {
                start: 9_000,
                end: 1_000
            }
        ));
    }

    #[test]
    fn an_absurd_frame_count_is_an_error_rather_than_an_allocator_panic() {
        let project = four_beat_project();
        let error = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, u64::MAX),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                EngineError::RenderTooLong {
                    frames: u64::MAX,
                    limit: MAX_RENDER_FRAMES
                }
            ),
            "got {error}"
        );
    }

    #[test]
    fn a_long_but_plausible_range_is_still_accepted() {
        // A minute at 48 kHz: well inside the bound, and cheap enough to actually render.
        let project = four_beat_project();
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default()
                .with_range(0, 2_880_000)
                .with_block_frames(65_536),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 2_880_000);
    }

    #[test]
    fn an_empty_project_renders_an_empty_buffer() {
        let project = Project::new("Empty", SAMPLE_RATE);
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 0);
    }

    #[test]
    fn rendering_twice_produces_identical_samples() {
        let project = four_beat_project();
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();
        let first =
            render_project(&project, &bank, &registry, &OfflineOptions::default()).expect("render");
        let second =
            render_project(&project, &bank, &registry, &OfflineOptions::default()).expect("render");
        assert_eq!(first, second);
    }
}
