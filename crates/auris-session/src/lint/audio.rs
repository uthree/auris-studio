//! The audio rules: render the song, then measure what actually came out.

use std::sync::atomic::AtomicBool;

use auris_core::project::{Project, TrackId, TrackKind};
use auris_core::time::{Seconds, Ticks};
use auris_core::{AudioBuffer, param::gain_to_db};
use auris_engine::{OfflineOptions, RenderProgress};

use super::{Diagnostic, Fix, Rule, Severity, sort};
use crate::{RenderJob, Session, SessionError};

/// The rate the lint renders at. Every rule here reads levels, and none of them needs the top
/// octave of hearing, so half the work buys the same answer.
pub const AUDIO_LINT_RATE: f64 = 24_000.0;
/// A track whose peak stays below this while its notes play is producing nothing.
pub const SILENCE_DB: f32 = -70.0;
/// Integrated loudness above which a mix is louder than any platform plays it.
pub const LOUD_LUFS: f32 = -8.0;
/// Integrated loudness below which a mix is far too quiet to deliver.
pub const QUIET_LUFS: f32 = -28.0;
/// A sample at or above this is full scale: clipped once it is encoded.
pub const CLIP_LEVEL: f32 = 0.999;
/// A fader at or below this is as good as off.
pub const FADER_OFF_DB: f32 = -60.0;

/// Measurement windows: short enough to place a click, long enough to stay cheap.
const WINDOW_SECONDS: f64 = 0.05;
/// No more tracks are rendered alone than this; each is a whole extra pass.
const MAX_TRACK_PASSES: usize = 24;
/// The most separate clipped or silent stretches reported, per track or for the mix.
const MAX_SPANS: usize = 8;

/// What the audio pass measured, beyond the problems it found.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AudioSummary {
    /// The mix's integrated loudness; `None` for a silent mix.
    pub integrated_lufs: Option<f32>,
    /// The mix's highest sample, in dBFS.
    pub peak_db: f32,
    /// How long the render was.
    pub seconds: f64,
    /// How many tracks were rendered alone to check that they sound.
    pub tracks_checked: usize,
}

/// A checked track, its fader in dB, and the merged stretches where its notes play.
type CheckedTrack = (TrackId, f32, Vec<(Ticks, Ticks)>);

/// A render of the mix and every sounding track, captured on the session thread.
///
/// `Send`: run it on a worker, then hand the result to the frontend, which keeps it only if
/// [`AudioLintResult::revision`] is still the document's revision.
pub struct AudioLintJob {
    revision: u64,
    job: RenderJob,
    rate: f64,
    /// Each checked track, its fader, and the merged stretches where its notes play.
    tracks: Vec<CheckedTrack>,
    /// Whether any track holds notes, which is what makes a silent mix a problem.
    has_notes: bool,
}

/// The audio pass's findings, tagged with the revision they describe.
#[derive(Clone, Debug)]
pub struct AudioLintResult {
    /// The revision the render was captured at.
    pub revision: u64,
    /// Problems found in the audio, with fixes where they are obvious.
    pub diagnostics: Vec<Diagnostic>,
    /// The measurements behind them.
    pub summary: AudioSummary,
}

impl Session {
    /// Captures the render the audio rules measure. Cheap here; the work is in
    /// [`AudioLintJob::run`].
    pub fn audio_lint_job(&mut self) -> AudioLintJob {
        let project = self.project();
        let tracks = checked_tracks(project);
        let has_notes = !super::score::events(project).is_empty();
        AudioLintJob {
            revision: self.revision(),
            job: self.render_job_at(Some(AUDIO_LINT_RATE)),
            rate: AUDIO_LINT_RATE,
            tracks,
            has_notes,
        }
    }

    /// Whether rendering the document instantiates third-party plugins, which is slow enough
    /// that a background pass should wait to be asked.
    pub fn has_hosted_plugins(&self) -> bool {
        let project = self.project();
        project.tracks.iter().any(|track| {
            track.kind.as_instrument().is_some_and(|i| i.is_hosted())
                || track.mixer.effects.iter().any(|slot| slot.is_hosted())
        }) || project.master.effects.iter().any(|slot| slot.is_hosted())
    }
}

/// Unmuted note tracks that can make a sound at all, with where their notes play.
fn checked_tracks(project: &Project) -> Vec<CheckedTrack> {
    let events = super::score::events(project);
    let mut tracks = Vec::new();
    for track in &project.tracks {
        if track.mixer.mute {
            continue;
        }
        // A singer with no take is the singer panel's business: it says so, and offers to sing.
        if matches!(&track.kind, TrackKind::Singer(singer) if singer.take.is_none()) {
            continue;
        }
        let mut spans: Vec<(Ticks, Ticks)> = Vec::new();
        for event in events.iter().filter(|event| event.track == track.id) {
            match spans.last_mut() {
                Some(last) if event.start <= last.1 => last.1 = last.1.max(event.end),
                _ => spans.push((event.start, event.end)),
            }
        }
        if !spans.is_empty() {
            tracks.push((track.id, track.mixer.gain_db, spans));
        }
        if tracks.len() >= MAX_TRACK_PASSES {
            break;
        }
    }
    tracks
}

/// Peak per window for one pass.
struct Windows {
    size: usize,
    filled: usize,
    peaks: Vec<f32>,
}

impl Windows {
    fn new(rate: f64) -> Self {
        Self {
            size: ((rate * WINDOW_SECONDS) as usize).max(1),
            filled: 0,
            peaks: Vec::new(),
        }
    }

    fn feed(&mut self, block: &AudioBuffer) {
        for frame in 0..block.frame_count() {
            if self.filled == 0 {
                self.peaks.push(0.0);
            }
            let peak = self.peaks.last_mut().expect("pushed above");
            for channel in 0..block.channel_count() {
                *peak = peak.max(block.channel(channel)[frame].abs());
            }
            self.filled = (self.filled + 1) % self.size;
        }
    }

    /// The loudest window touching `from..to` seconds; `None` when the range lies past the end.
    fn peak_between(&self, from: f64, to: f64) -> Option<f32> {
        let first = (from / WINDOW_SECONDS).floor().max(0.0) as usize;
        let last = ((to / WINDOW_SECONDS).ceil() as usize).min(self.peaks.len());
        (first < last).then(|| {
            self.peaks[first..last]
                .iter()
                .fold(0.0f32, |a, b| a.max(*b))
        })
    }
}

impl AudioLintJob {
    /// The revision the job was captured at.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Renders and measures. Seconds of work; never call it on a UI thread.
    pub fn run(mut self, cancelled: &AtomicBool) -> Result<AudioLintResult, SessionError> {
        let project = self.job.project().clone();
        let mut passes = vec![None];
        passes.extend(self.tracks.iter().map(|(track, _, _)| Some(*track)));
        let options = OfflineOptions {
            sample_rate: Some(self.rate),
            include_tail: false,
            block_frames: 2_048,
            ..OfflineOptions::whole_project()
        };
        let mut windows: Vec<Windows> = passes.iter().map(|_| Windows::new(self.rate)).collect();
        // The mix is kept whole for its integrated loudness, which is gated over the song.
        let mut mix: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
        let mut progress = RenderProgress::default().cancelled_by(cancelled);
        let rate = self
            .job
            .render_passes(&passes, &options, &mut progress, |pass, block| {
                windows[pass].feed(block);
                if pass == 0 {
                    for (channel, out) in mix.iter_mut().enumerate() {
                        let source = channel.min(block.channel_count().saturating_sub(1));
                        out.extend_from_slice(block.channel(source));
                    }
                }
            })?;
        if progress.is_cancelled() {
            return Err(SessionError::Engine(
                auris_engine::EngineError::RenderCancelled,
            ));
        }
        let mix = AudioBuffer::from_planar(mix, rate).map_err(auris_engine::EngineError::from)?;
        let seconds = mix.frame_count() as f64 / rate;
        let peak = windows[0].peaks.iter().fold(0.0f32, |a, b| a.max(*b));
        let integrated_lufs = auris_dsp::integrated_lufs(&mix);
        let tempo = &project.tempo_map;
        let to_ticks = |seconds: f64| tempo.seconds_to_ticks(Seconds(seconds));

        let mut diagnostics = Vec::new();

        // Clipping: merge clipped windows closer than half a second into one stretch.
        let mut clipped: Vec<(usize, usize)> = Vec::new();
        for (index, window) in windows[0].peaks.iter().enumerate() {
            if *window < CLIP_LEVEL {
                continue;
            }
            match clipped.last_mut() {
                Some(last) if index - last.1 <= (0.5 / WINDOW_SECONDS) as usize => last.1 = index,
                _ => clipped.push((index, index)),
            }
        }
        for (first, last) in clipped.into_iter().take(MAX_SPANS) {
            let local = windows[0].peaks[first..=last]
                .iter()
                .fold(0.0f32, |a, b| a.max(*b));
            let mut diagnostic = Diagnostic::new(
                Rule::Clipping,
                Severity::Error,
                to_ticks(first as f64 * WINDOW_SECONDS),
                to_ticks((last + 1) as f64 * WINDOW_SECONDS),
            );
            diagnostic.value = Some(gain_to_db(local));
            diagnostic.limit = Some(gain_to_db(CLIP_LEVEL));
            diagnostic.fix = Some(Fix::NormalizeMix);
            diagnostics.push(diagnostic);
        }

        // Loudness of the whole song.
        let song_end = to_ticks(seconds).max(Ticks(1));
        match integrated_lufs {
            Some(lufs) if lufs > LOUD_LUFS => {
                let mut diagnostic =
                    Diagnostic::new(Rule::TooLoud, Severity::Warning, Ticks::ZERO, song_end);
                diagnostic.value = Some(lufs);
                diagnostic.limit = Some(LOUD_LUFS);
                diagnostic.fix = Some(Fix::NormalizeMix);
                diagnostics.push(diagnostic);
            }
            Some(lufs) if lufs < QUIET_LUFS && self.has_notes => {
                let mut diagnostic =
                    Diagnostic::new(Rule::TooQuiet, Severity::Warning, Ticks::ZERO, song_end);
                diagnostic.value = Some(lufs);
                diagnostic.limit = Some(QUIET_LUFS);
                diagnostic.fix = Some(Fix::NormalizeMix);
                diagnostics.push(diagnostic);
            }
            _ => {}
        }

        // Tracks that play but make no sound.
        let silent = auris_core::param::db_to_gain(SILENCE_DB);
        for (pass, (track, fader, spans)) in self.tracks.iter().enumerate() {
            let windows = &windows[pass + 1];
            let mut quiet = Vec::new();
            let mut heard = false;
            for (start, end) in spans {
                let from = tempo.ticks_to_seconds(*start).0;
                // A little past the end, for an instrument that speaks late.
                let to = tempo.ticks_to_seconds(*end).0 + 0.1;
                match windows.peak_between(from, to) {
                    Some(level) if level >= silent => heard = true,
                    Some(_) => quiet.push((*start, *end)),
                    None => {}
                }
            }
            if quiet.is_empty() {
                continue;
            }
            let fix = (*fader <= FADER_OFF_DB).then_some(Fix::ResetFader { track: *track });
            if !heard {
                // Nothing at all: one problem, across everything the track plays.
                let mut diagnostic = Diagnostic::new(
                    Rule::SilentTrack,
                    Severity::Error,
                    quiet[0].0,
                    quiet[quiet.len() - 1].1,
                );
                diagnostic.track = Some(*track);
                diagnostic.limit = Some(SILENCE_DB);
                diagnostic.fix = fix;
                diagnostics.push(diagnostic);
                continue;
            }
            for (start, end) in quiet.into_iter().take(MAX_SPANS) {
                let mut diagnostic =
                    Diagnostic::new(Rule::SilentTrack, Severity::Warning, start, end);
                diagnostic.track = Some(*track);
                diagnostic.limit = Some(SILENCE_DB);
                diagnostics.push(diagnostic);
            }
        }
        sort(&mut diagnostics);
        Ok(AudioLintResult {
            revision: self.revision,
            diagnostics,
            summary: AudioSummary {
                integrated_lufs,
                peak_db: gain_to_db(peak),
                seconds,
                tracks_checked: self.tracks.len(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::param::ParamTarget;
    use auris_core::project::Note;
    use std::sync::atomic::AtomicBool;

    fn session_with_a_phrase() -> (Session, TrackId) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(3840))
            .unwrap();
        session
            .replace_notes(
                clip,
                (0..4)
                    .map(|i| Note::new(60 + i * 2, Ticks(i as i64 * 960), Ticks(900)))
                    .collect(),
            )
            .unwrap();
        (session, track)
    }

    fn run(session: &mut Session) -> AudioLintResult {
        session
            .audio_lint_job()
            .run(&AtomicBool::new(false))
            .unwrap()
    }

    #[test]
    fn a_sounding_phrase_is_measured_and_reports_no_silence() {
        let (mut session, _) = session_with_a_phrase();
        let result = run(&mut session);
        assert!(result.summary.tracks_checked == 1);
        assert!(result.summary.integrated_lufs.is_some());
        assert!(result.summary.seconds > 1.5);
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.rule == Rule::SilentTrack),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn a_track_with_its_fader_off_is_silent_and_offers_the_fader_back() {
        let (mut session, track) = session_with_a_phrase();
        session.set_param(ParamTarget::TrackGain(track), -120.0);
        let result = run(&mut session);
        let silent: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.rule == Rule::SilentTrack)
            .collect();
        assert_eq!(silent.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(silent[0].severity, Severity::Error);
        assert_eq!(silent[0].fix, Some(Fix::ResetFader { track }));
        // The mix is silent too, but an empty-sounding mix is not "too quiet": it has no level.
        assert!(result.summary.integrated_lufs.is_none());

        let revision = session.revision();
        session.apply_lint_fix(silent[0], revision).unwrap();
        assert_eq!(session.project().track(track).unwrap().mixer.gain_db, 0.0);
        let after = run(&mut session);
        assert!(
            !after
                .diagnostics
                .iter()
                .any(|d| d.rule == Rule::SilentTrack)
        );
    }

    #[test]
    fn a_hot_mix_is_clipping_where_it_clips() {
        let (mut session, track) = session_with_a_phrase();
        // The headless session has no master limiter, so this much gain reaches full scale.
        session.set_param(ParamTarget::TrackGain(track), 24.0);
        session.set_param(ParamTarget::MasterGain, 12.0);
        let result = run(&mut session);
        let clipping: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.rule == Rule::Clipping)
            .collect();
        assert!(!clipping.is_empty(), "{:?}", result.summary);
        assert!(clipping[0].start < Ticks(3840));
        assert!(result.summary.peak_db >= -0.01);
    }
}
