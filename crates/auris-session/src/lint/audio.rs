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

/// A note whose sound sits further than this from its written pitch, once octaves are set aside,
/// is out of tune: a quarter tone either way.
pub const PITCH_TOLERANCE_CENTS: f32 = 50.0;
/// A note with a pitch in fewer than this share of its frames has none to speak of.
pub const VOICED_SHARE: f32 = 0.3;
/// Below this, a stretch of the mix loses this much level summed to mono that the left and
/// right channels are cancelling each other: a phone speaker plays it as a hole.
pub const MONO_LOSS_DB: f32 = -12.0;

/// The written notes measured: C2 to C6, where a period fits a short frame at the lint's rate.
const PITCH_NOTES: (u8, u8) = (36, 84);
/// The shortest note measured: Harvest needs a few periods past the attack.
const PITCH_SHORTEST: f64 = 0.15;
/// The most notes, and the most seconds of notes, measured per track.
const PITCH_MAX_NOTES: usize = 160;
const PITCH_MAX_SECONDS: f64 = 60.0;
/// Mono-loss windows are longer than peak windows: cancellation is a property of a sound, not a
/// click.
const PHASE_WINDOW_SECONDS: f64 = 0.4;
/// Mono loss is judged only where the mix is loud enough to be heard.
const PHASE_LEVEL_DB: f32 = -40.0;

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

/// A note played alone on its track, whose pitch the render can be held to.
#[derive(Clone, Debug)]
struct PitchNote {
    note: super::NoteRef,
    pitch: u8,
    start: Ticks,
    end: Ticks,
    /// Where it sounds, in frames of the render.
    frames: (usize, usize),
    /// Whether the track is a singer, whose every note must have a pitch.
    singer: bool,
}

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
    /// For each checked track, the notes it plays alone, by start.
    pitched: Vec<Vec<PitchNote>>,
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
        let events = super::score::events(project);
        let has_notes = !events.is_empty();
        let pitched = tracks
            .iter()
            .map(|(track, _, _)| {
                if self.unpitched_instrument(*track) {
                    Vec::new()
                } else {
                    pitch_notes(project, &events, *track, AUDIO_LINT_RATE)
                }
            })
            .collect();
        AudioLintJob {
            revision: self.revision(),
            job: self.render_job_at(Some(AUDIO_LINT_RATE)),
            rate: AUDIO_LINT_RATE,
            tracks,
            has_notes,
            pitched,
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

impl Session {
    /// Whether a track's instrument is percussion played from an instrument track, whose notes
    /// choose sounds rather than pitches. The same exclusions the chord reader makes.
    fn unpitched_instrument(&self, track: TrackId) -> bool {
        self.track_preset(track)
            .is_some_and(|preset| preset.bank == 128)
            || self
                .project()
                .track(track)
                .and_then(|t| t.kind.as_instrument())
                .is_some_and(|i| i.instrument_id == "auris.synth.noisedrum")
    }
}

/// The notes `track` plays with nothing else of its own sounding, long enough and in a range
/// where a pitch can be measured.
fn pitch_notes(
    project: &Project,
    events: &[super::score::Event],
    track: TrackId,
    rate: f64,
) -> Vec<PitchNote> {
    let Some(kind) = project.track(track).map(|t| &t.kind) else {
        return Vec::new();
    };
    let singer = matches!(kind, TrackKind::Singer(_));
    if !singer && !matches!(kind, TrackKind::Instrument(_)) {
        return Vec::new();
    }
    let own: Vec<_> = events
        .iter()
        .filter(|event| event.track == track && !event.drum)
        .collect();
    let tempo = &project.tempo_map;
    let mut notes = Vec::new();
    let mut seconds = 0.0;
    for (index, event) in own.iter().enumerate() {
        let alone = own
            .iter()
            .enumerate()
            .all(|(other, e)| other == index || e.end <= event.start || e.start >= event.end);
        if !alone || !(PITCH_NOTES.0..=PITCH_NOTES.1).contains(&event.pitch) {
            continue;
        }
        let from = tempo.ticks_to_seconds(event.start).0;
        let to = tempo.ticks_to_seconds(event.end).0;
        if to - from < PITCH_SHORTEST {
            continue;
        }
        seconds += to - from;
        if notes.len() >= PITCH_MAX_NOTES || seconds > PITCH_MAX_SECONDS {
            break;
        }
        notes.push(PitchNote {
            note: event.note,
            pitch: event.pitch,
            start: event.start,
            end: event.end,
            frames: ((from * rate) as usize, (to * rate) as usize),
            singer,
        });
    }
    notes
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
        // Each pitched note's samples, mono, captured as its track's pass goes by.
        let mut captured: Vec<Vec<Vec<f32>>> = self
            .pitched
            .iter()
            .map(|notes| notes.iter().map(|_| Vec::new()).collect())
            .collect();
        let mut position = vec![0usize; passes.len()];
        let pitched = &self.pitched;
        let mut progress = RenderProgress::default().cancelled_by(cancelled);
        let rate = self
            .job
            .render_passes(&passes, &options, &mut progress, |pass, block| {
                windows[pass].feed(block);
                let at = position[pass];
                position[pass] += block.frame_count();
                if pass > 0 {
                    let channels = block.channel_count().max(1);
                    for (note, out) in pitched[pass - 1].iter().zip(&mut captured[pass - 1]) {
                        let from = note.frames.0.max(at);
                        let to = note.frames.1.min(at + block.frame_count());
                        for frame in from..to {
                            let sum: f32 = (0..block.channel_count())
                                .map(|channel| block.channel(channel)[frame - at])
                                .sum();
                            out.push(sum / channels as f32);
                        }
                    }
                }
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
        let silent = silent_gain();
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
        diagnostics.extend(mono_loss(&mix, rate, &to_ticks));
        for ((track, _, _), (notes, samples)) in
            self.tracks.iter().zip(self.pitched.iter().zip(&captured))
        {
            diagnostics.extend(pitch_problems(*track, notes, samples, rate, silent_gain()));
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

fn silent_gain() -> f32 {
    auris_core::param::db_to_gain(SILENCE_DB)
}

/// Stretches where the mix, summed to mono, loses [`MONO_LOSS_DB`] or more against its stereo
/// level: the left and right channels are out of phase.
fn mono_loss(mix: &AudioBuffer, rate: f64, to_ticks: &dyn Fn(f64) -> Ticks) -> Vec<Diagnostic> {
    if mix.channel_count() < 2 {
        return Vec::new();
    }
    let (left, right) = (mix.channel(0), mix.channel(1));
    let size = ((rate * PHASE_WINDOW_SECONDS) as usize).max(1);
    let audible = auris_core::param::db_to_gain(PHASE_LEVEL_DB);
    let mut spans: Vec<(usize, usize, f32)> = Vec::new();
    for (index, (l, r)) in left.chunks(size).zip(right.chunks(size)).enumerate() {
        let stereo = l
            .iter()
            .zip(r)
            .map(|(a, b)| (a * a + b * b) / 2.0)
            .sum::<f32>()
            / l.len() as f32;
        let mono = l
            .iter()
            .zip(r)
            .map(|(a, b)| ((a + b) / 2.0).powi(2))
            .sum::<f32>()
            / l.len() as f32;
        if stereo.sqrt() < audible {
            continue;
        }
        let loss = 10.0 * (mono.max(1e-12) / stereo).log10();
        if loss > MONO_LOSS_DB {
            continue;
        }
        match spans.last_mut() {
            Some(last) if last.1 + 1 == index => {
                last.1 = index;
                last.2 = last.2.min(loss);
            }
            _ => spans.push((index, index, loss)),
        }
    }
    spans
        .into_iter()
        .take(MAX_SPANS)
        .map(|(first, last, loss)| {
            let mut diagnostic = Diagnostic::new(
                Rule::PhaseCancellation,
                Severity::Warning,
                to_ticks(first as f64 * PHASE_WINDOW_SECONDS),
                to_ticks((last + 1) as f64 * PHASE_WINDOW_SECONDS),
            );
            diagnostic.value = Some(loss);
            diagnostic.limit = Some(MONO_LOSS_DB);
            diagnostic
        })
        .collect()
}

/// The written pitch of MIDI note `pitch`, in hertz.
fn hertz(pitch: u8) -> f32 {
    440.0 * 2f32.powf((f32::from(pitch) - 69.0) / 12.0)
}

/// Notes that sound off their written pitch, or with no pitch at all.
///
/// Octaves are set aside: an instrument an octave up or down is a register choice, and Harvest
/// can be an octave out on a bright sound. A pitched instrument with most of its notes unvoiced
/// is one finding for the track; a singer's unvoiced note is a finding of its own, because a
/// sung note is nothing but its pitch.
fn pitch_problems(
    track: TrackId,
    notes: &[PitchNote],
    samples: &[Vec<f32>],
    rate: f64,
    silent: f32,
) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    let mut unvoiced = Vec::new();
    let mut measured = 0usize;
    for (note, samples) in notes.iter().zip(samples) {
        let peak = samples.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        if peak < silent {
            continue;
        }
        measured += 1;
        let heard = auris_analysis::pitch::note_pitch(samples, rate, hertz(note.pitch));
        let Some(hz) = heard.hz.filter(|_| heard.voiced >= VOICED_SHARE) else {
            unvoiced.push(note);
            continue;
        };
        let cents = 1200.0 * (hz / hertz(note.pitch)).log2();
        let off = cents - 1200.0 * (cents / 1200.0).round();
        if off.abs() <= PITCH_TOLERANCE_CENTS || found.len() >= MAX_SPANS * 2 {
            continue;
        }
        let mut diagnostic =
            Diagnostic::new(Rule::PitchMismatch, Severity::Warning, note.start, note.end);
        diagnostic.track = Some(track);
        diagnostic.notes = vec![note.note];
        diagnostic.pitches = vec![note.pitch];
        diagnostic.value = Some(off);
        diagnostic.limit = Some(PITCH_TOLERANCE_CENTS);
        found.push(diagnostic);
    }
    let singer = notes.first().is_some_and(|note| note.singer);
    if singer {
        for note in unvoiced.iter().take(MAX_SPANS * 2) {
            let mut diagnostic =
                Diagnostic::new(Rule::Unpitched, Severity::Warning, note.start, note.end);
            diagnostic.track = Some(track);
            diagnostic.notes = vec![note.note];
            diagnostic.pitches = vec![note.pitch];
            found.push(diagnostic);
        }
    } else if measured >= 4 && unvoiced.len() * 2 >= measured {
        let mut diagnostic = Diagnostic::new(
            Rule::Unpitched,
            Severity::Info,
            unvoiced[0].start,
            unvoiced[unvoiced.len() - 1].end,
        );
        diagnostic.track = Some(track);
        diagnostic.value = Some(unvoiced.len() as f32 / measured as f32 * 100.0);
        found.push(diagnostic);
    }
    found
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
        // The built-in voice plays in tune, and the measurement agrees.
        assert!(
            !result.diagnostics.iter().any(|d| matches!(
                d.rule,
                Rule::SilentTrack | Rule::PitchMismatch | Rule::Unpitched | Rule::PhaseCancellation
            )),
            "{:?}",
            result.diagnostics
        );
    }

    fn sawtooth(hz: f32, seconds: f64) -> Vec<f32> {
        (0..(AUDIO_LINT_RATE * seconds) as usize)
            .map(|i| ((i as f32 * hz / AUDIO_LINT_RATE as f32).fract() * 2.0 - 1.0) * 0.3)
            .collect()
    }

    fn note(pitch: u8, index: usize) -> PitchNote {
        PitchNote {
            note: super::super::NoteRef {
                clip: auris_core::project::ClipId(1),
                index,
            },
            pitch,
            start: Ticks(index as i64 * 960),
            end: Ticks(index as i64 * 960 + 960),
            frames: (0, 0),
            singer: false,
        }
    }

    #[test]
    fn a_note_a_semitone_sharp_is_off_pitch_and_one_an_octave_up_is_not() {
        let a3 = hertz(57);
        let notes = [note(57, 0), note(57, 1), note(57, 2)];
        let samples = [
            sawtooth(a3, 0.5),
            sawtooth(a3 * 2f32.powf(1.0 / 12.0), 0.5),
            sawtooth(a3 * 2.0, 0.5),
        ];
        let found = pitch_problems(TrackId(1), &notes, &samples, AUDIO_LINT_RATE, silent_gain());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::PitchMismatch);
        assert_eq!(found[0].notes[0].index, 1);
        assert!((found[0].value.unwrap() - 100.0).abs() < 15.0, "{found:?}");
    }

    #[test]
    fn a_mix_whose_channels_are_opposite_cancels_in_mono() {
        let left = sawtooth(220.0, 2.0);
        let right: Vec<f32> = left.iter().map(|s| -s).collect();
        let mix = AudioBuffer::from_planar(vec![left.clone(), right], AUDIO_LINT_RATE).unwrap();
        let tempo = auris_core::time::TempoMap::default();
        let to_ticks = |seconds: f64| tempo.seconds_to_ticks(Seconds(seconds));
        let found = mono_loss(&mix, AUDIO_LINT_RATE, &to_ticks);
        assert_eq!(found.len(), 1);
        assert!(found[0].value.unwrap() < -40.0);

        let mix = AudioBuffer::from_planar(vec![left.clone(), left], AUDIO_LINT_RATE).unwrap();
        assert!(mono_loss(&mix, AUDIO_LINT_RATE, &to_ticks).is_empty());
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
