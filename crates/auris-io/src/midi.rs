//! Reading Standard MIDI Files.
//!
//! A `.mid` file is the one format every other piece of music software can hand over, so this is
//! the door material comes in through. What comes out is not a [`Project`] —
//! that would mean this crate deciding which instrument each track plays, which is the session's
//! business — but everything a project needs: the tempo and meter along the timeline, and a set of
//! named tracks holding notes at absolute positions.
//!
//! # What a tick means
//!
//! An SMF counts time in its own division, declared in the header as ticks per quarter note. Ours
//! is [`TICKS_PER_QUARTER`], so every position is scaled on the way in. The division is usually
//! 480 or 960 and ours is a multiple of neither by luck, so the scaling is done in `i64` with the
//! multiply first: dividing first would quantise every note in the file to the file's own grid
//! before it ever reached ours.
//!
//! A file may instead count in **SMPTE timecode** — frames of real time rather than beats. That is
//! a different kind of thing: it has no beats, so it has no bars, and laying it on a musical
//! timeline would mean choosing a tempo on the file's behalf and writing it down as though the
//! file had said so. Those are refused by name rather than guessed at.
//!
//! # What survives a round trip, and what does not
//!
//! Writing happens at *our* division, because [`TICKS_PER_QUARTER`] is 960 and an SMF header holds
//! up to 32767. So every note position and length written here reads back exactly.
//!
//! A tempo does not, quite. The file stores whole microseconds per quarter note, so 144 bpm is
//! 416 666.67 written as 416 667 and read back as 143.999 88 — a thousandth of a beat per minute,
//! inaudible over any length of piece, and a property of the format rather than of this code. A
//! tempo whose period *is* a whole number of microseconds, 96 or 120, comes back exact.
//!
//! What has nowhere to go in a `.mid` at all: audio tracks, every mixer setting including mute and
//! solo, which instrument a track plays, and the automation. A MIDI file is the notes and the
//! clock.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use auris_core::plugin::CONTROLLER_MAX;
use auris_core::project::{
    CURVE_STEP, ClipCurve, CurvePoint, MidiClip, Note, NoteTransform, Project,
    validated_loop_pass_count,
};
use auris_core::time::{SignatureMap, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignature};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use tempfile::NamedTempFile;

use crate::error::{IoError, MidiExportResource, MidiImportResource, Result};

/// Largest Standard MIDI File the in-memory parser will accept.
///
/// Sixty-four MiB already represents many millions of ordinary MIDI events. Bounding the source
/// prevents a selected or replaced file from turning one import into an unbounded allocation.
const MAX_MIDI_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Decode ceilings shared with the render graph's practical scheduling bounds.
const MAX_MIDI_TRACK_EVENTS: usize = 1_000_000;
const MAX_MIDI_FILE_EVENTS: usize = 4_000_000;
const MAX_MIDI_NOTES: usize = 500_000;
const MAX_MIDI_AUTOMATION_POINTS: usize = 4_000_000;
const MAX_MIDI_OUTPUT_EVENTS: usize = 4_000_000;

/// Export ceilings shared with the render graph's practical scheduling bounds.
///
/// Note-off events double the note count, and the eight-event allowance covers a track name,
/// end marker and the six-message pitch-bend-range handshake. Curves and notes still share the
/// same overall wire-event ceiling, so neither resource can hide behind the other's limit.
const MAX_MIDI_EXPORT_TRACK_NOTES: usize = 500_000;
const MAX_MIDI_EXPORT_FILE_NOTES: usize = 500_000;
const MAX_MIDI_EXPORT_TRACK_CURVE_EVENTS: usize = 1_000_000;
const MAX_MIDI_EXPORT_FILE_CURVE_EVENTS: usize = 4_000_000;
const MAX_MIDI_EXPORT_TRACK_EVENTS: usize = 1_000_008;
const MAX_MIDI_EXPORT_FILE_EVENTS: usize = 4_000_000;

#[derive(Clone, Copy)]
struct ExportLimits {
    track_notes: usize,
    file_notes: usize,
    track_curve_events: usize,
    file_curve_events: usize,
    track_events: usize,
    file_events: usize,
}

const EXPORT_LIMITS: ExportLimits = ExportLimits {
    track_notes: MAX_MIDI_EXPORT_TRACK_NOTES,
    file_notes: MAX_MIDI_EXPORT_FILE_NOTES,
    track_curve_events: MAX_MIDI_EXPORT_TRACK_CURVE_EVENTS,
    file_curve_events: MAX_MIDI_EXPORT_FILE_CURVE_EVENTS,
    track_events: MAX_MIDI_EXPORT_TRACK_EVENTS,
    file_events: MAX_MIDI_EXPORT_FILE_EVENTS,
};

#[derive(Clone, Copy)]
struct ImportLimits {
    track_events: usize,
    file_events: usize,
    notes: usize,
    automation_points: usize,
    output_events: usize,
}

const IMPORT_LIMITS: ImportLimits = ImportLimits {
    track_events: MAX_MIDI_TRACK_EVENTS,
    file_events: MAX_MIDI_FILE_EVENTS,
    notes: MAX_MIDI_NOTES,
    automation_points: MAX_MIDI_AUTOMATION_POINTS,
    output_events: MAX_MIDI_OUTPUT_EVENTS,
};

#[derive(Default)]
struct ImportBudget {
    file_events: usize,
    notes: usize,
    automation_points: usize,
    output_events: usize,
}

impl ImportBudget {
    fn source_event(&mut self, track_events: &mut usize, limits: ImportLimits) -> Result<()> {
        let next_track = bounded_increment(
            *track_events,
            limits.track_events,
            MidiImportResource::TrackEvents,
        )?;
        let next_file = bounded_increment(
            self.file_events,
            limits.file_events,
            MidiImportResource::FileEvents,
        )?;
        *track_events = next_track;
        self.file_events = next_file;
        Ok(())
    }

    fn note(&mut self, limits: ImportLimits) -> Result<()> {
        let notes = bounded_increment(self.notes, limits.notes, MidiImportResource::Notes)?;
        let output_events = bounded_increment(
            self.output_events,
            limits.output_events,
            MidiImportResource::OutputEvents,
        )?;
        self.notes = notes;
        self.output_events = output_events;
        Ok(())
    }

    fn automation_point(&mut self, limits: ImportLimits) -> Result<()> {
        let automation_points = bounded_increment(
            self.automation_points,
            limits.automation_points,
            MidiImportResource::AutomationPoints,
        )?;
        let output_events = bounded_increment(
            self.output_events,
            limits.output_events,
            MidiImportResource::OutputEvents,
        )?;
        self.automation_points = automation_points;
        self.output_events = output_events;
        Ok(())
    }

    fn timeline_point(&mut self, limits: ImportLimits) -> Result<()> {
        self.output_events = bounded_increment(
            self.output_events,
            limits.output_events,
            MidiImportResource::OutputEvents,
        )?;
        Ok(())
    }
}

fn bounded_increment(current: usize, limit: usize, resource: MidiImportResource) -> Result<usize> {
    let observed = current.saturating_add(1);
    if observed > limit {
        return Err(IoError::MidiImportTooLarge {
            resource,
            observed: u64::try_from(observed).unwrap_or(u64::MAX),
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
        });
    }
    Ok(observed)
}

/// File extensions the MIDI importer accepts, for a file-dialog filter.
pub fn midi_extensions() -> &'static [&'static str] {
    &["mid", "midi"]
}

/// One track's worth of what a MIDI file held.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiTrack {
    /// The track's name, from its name meta event or from the channel it played on.
    pub name: String,
    /// Which MIDI channel the notes came from, 0-based. Channel 9 is the drum channel by
    /// convention, which is the only hint a file gives about what a track is *for*.
    pub channel: u8,
    /// Every note, positioned from the start of the song rather than from a clip.
    pub notes: Vec<Note>,
    /// The pitch bend, positioned the same way.
    ///
    /// A file that never bends brings none, which is what keeps this off the overwhelming
    /// majority of imports.
    pub bend: Vec<CurvePoint>,
    /// The controllers the file wrote, by MIDI controller number, positioned the same way.
    ///
    /// Only the ones that shape a performance: see [`is_performance_controller`]. A file's bank
    /// selects and RPN handshakes are how it addresses an instrument, not something anyone drew,
    /// and importing them as lanes would put a staircase on screen for every General MIDI file
    /// ever written — and send it back out on the next export.
    pub controllers: BTreeMap<u8, Vec<CurvePoint>>,
}

/// Everything a Standard MIDI File said, in this application's units.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiImport {
    /// Tempo along the timeline, from the file's tempo meta events.
    pub tempo_map: TempoMap,
    /// Meter along the timeline, from the file's time signature meta events.
    pub signatures: SignatureMap,
    /// The tracks, in the order they appeared.
    pub tracks: Vec<MidiTrack>,
}

impl MidiImport {
    /// Position just past the last note in the file.
    pub fn end(&self) -> Ticks {
        self.tracks
            .iter()
            .flat_map(|track| track.notes.iter())
            .map(|note| note.end())
            .max()
            .unwrap_or(Ticks::ZERO)
    }

    /// How many notes the file held, across every track.
    pub fn note_count(&self) -> usize {
        self.tracks.iter().map(|track| track.notes.len()).sum()
    }
}

/// Reads a Standard MIDI File.
pub fn read_midi_file(path: &Path) -> Result<MidiImport> {
    let bytes = read_midi_source(path, MAX_MIDI_FILE_BYTES)?;
    let smf = Smf::parse(&bytes).map_err(|error| IoError::MidiParse(error.to_string()))?;
    read_smf(&smf)
}

/// Reads the MIDI source under an explicit bound.
fn read_midi_source(path: &Path, limit: usize) -> Result<Vec<u8>> {
    crate::bounded::read_with_limit(path, limit, |observed| {
        midi_too_large(path, limit, observed)
    })
}

/// The same read with a test seam after metadata, for reproducing a growing file.
#[cfg(test)]
fn read_midi_source_after_metadata(
    path: &Path,
    limit: usize,
    after_metadata: impl FnOnce(),
) -> Result<Vec<u8>> {
    crate::bounded::read_with_limit_after_metadata(path, limit, after_metadata, |observed| {
        midi_too_large(path, limit, observed)
    })
}

fn midi_too_large(path: &Path, limit: usize, observed: u64) -> IoError {
    IoError::MidiFileTooLarge {
        path: path.to_path_buf(),
        observed,
        limit: limit as u64,
    }
}

/// The half of [`read_midi_file`] that touches no filesystem, so it can be tested on bytes.
pub fn read_midi_bytes(bytes: &[u8]) -> Result<MidiImport> {
    ensure_midi_data_size(bytes, MAX_MIDI_FILE_BYTES)?;
    let smf = Smf::parse(bytes).map_err(|error| IoError::MidiParse(error.to_string()))?;
    read_smf(&smf)
}

fn ensure_midi_data_size(bytes: &[u8], limit: usize) -> Result<()> {
    if bytes.len() > limit {
        return Err(IoError::MidiDataTooLarge {
            observed: bytes.len() as u64,
            limit: limit as u64,
        });
    }
    Ok(())
}

fn read_smf(smf: &Smf) -> Result<MidiImport> {
    read_smf_with_limits(smf, IMPORT_LIMITS)
}

fn read_smf_with_limits(smf: &Smf, limits: ImportLimits) -> Result<MidiImport> {
    let per_quarter = match smf.header.timing {
        Timing::Metrical(per_quarter) => u32::from(per_quarter.as_int()).max(1),
        Timing::Timecode(fps, subframe) => {
            return Err(IoError::MidiTimecode {
                fps: fps.as_f32(),
                subframe,
            });
        }
    };

    let mut tempo_map = TempoMap::constant(DEFAULT_BPM);
    let mut signatures = SignatureMap::constant(TimeSignature::default());
    let mut saw_tempo = false;
    let mut saw_signature = false;
    // Notes are gathered per source track *and* channel. A format 0 file is one track carrying
    // every channel, and a format 1 track can still carry more than one — either way two channels
    // are two instruments, and merging them would put a bass line inside the drum part.
    let mut parts: HashMap<(usize, u8), Part> = HashMap::new();
    let mut order: Vec<(usize, u8)> = Vec::new();
    let mut budget = ImportBudget::default();

    for (index, track) in smf.tracks.iter().enumerate() {
        let mut at: u64 = 0;
        let mut track_events = 0usize;
        let mut track_name: Option<String> = None;
        // Sounding notes, keyed by channel and pitch. A stack per key rather than one slot: the
        // same pitch struck twice before either release is legal, and the engine already keeps
        // such notes independent.
        let mut sounding: HashMap<(u8, u8), Vec<(u64, u8)>> = HashMap::new();

        for event in track.iter() {
            // Count before interpreting or retaining the event. In particular, splitting an
            // attack over another source track cannot reset the file-wide budget.
            budget.source_event(&mut track_events, limits)?;
            at += u64::from(event.delta.as_int());
            let tick = scale(at, per_quarter);
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(micros)) => {
                    budget.timeline_point(limits)?;
                    let micros = micros.as_int().max(1);
                    let bpm = 60_000_000.0 / f64::from(micros);
                    // A tempo at the very start *is* the song's tempo rather than a change
                    // written on top of a default that was never in the file. One that arrives
                    // later is a change, and the head of the song ran at the default until it —
                    // so the question is where the event sits and not merely whether it is the
                    // first one seen. Everything after that goes through `set_point`, which
                    // replaces the anchor in place when it lands at zero: a later track
                    // restating the opening tempo, which a format 1 file often does, must not
                    // throw away the changes already read.
                    match saw_tempo || tick != Ticks::ZERO {
                        false => tempo_map = TempoMap::constant(bpm),
                        true => tempo_map.set_point(tick, bpm),
                    }
                    saw_tempo = true;
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(numerator, denominator, ..)) => {
                    // The file stores the denominator as a power of two: 3 means an eighth. The
                    // power is judged *before* it is shifted rather than clamped into range and
                    // judged afterwards — clamping first defeated the whole check below, because a
                    // 7/32 file arrived at it already reading 7/16 and passed. Which is precisely
                    // the outcome that check exists to rule out: nothing distinguished such a file
                    // from one that really did say 7/16, and no warning fired either.
                    let power = u32::from(denominator);
                    let numerator = u32::from(numerator);
                    let denominator = match power <= MAX_DENOMINATOR_POWER {
                        true => 1u32 << power,
                        false => {
                            log::warn!(
                                "MIDI file gives a meter the denominator two to the {power}, finer \
                                 than this build can hold; ignoring it"
                            );
                            continue;
                        }
                    };
                    // The range is checked here rather than left to `TimeSignature::new`, which
                    // answers 4/4 for anything outside it. That is the right answer for a control
                    // with bounds and the wrong one for a file: it would write down a meter the
                    // file never claimed, and the reader could not tell it apart from one it did.
                    if !TimeSignature::NUMERATORS.contains(&numerator)
                        || !TimeSignature::DENOMINATORS.contains(&denominator)
                    {
                        log::warn!(
                            "MIDI file names a {numerator}/{denominator} meter; ignoring it"
                        );
                        continue;
                    }
                    budget.timeline_point(limits)?;
                    let signature = TimeSignature::new(numerator, denominator);
                    match saw_signature || tick != Ticks::ZERO {
                        false => signatures = SignatureMap::constant(signature),
                        true => signatures.set_point(tick, signature),
                    }
                    saw_signature = true;
                }
                TrackEventKind::Meta(MetaMessage::TrackName(name)) => {
                    track_name = Some(String::from_utf8_lossy(name).trim().to_string())
                        .filter(|name| !name.is_empty());
                }
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    let key = (index, channel);
                    // The order is kept separately so tracks come out in the order the file put
                    // them in rather than in a hash map's order, which would shuffle a song's
                    // parts differently on every run.
                    parts.entry(key).or_insert_with(|| {
                        order.push(key);
                        Part::default()
                    });
                    match message {
                        // A note-on at zero velocity is a note-off. Every sequencer that ever
                        // used running status emits them, so this is not an edge case.
                        MidiMessage::NoteOn { key: pitch, vel } if vel.as_int() > 0 => {
                            // Every accepted attack becomes a note, even when the source forgot
                            // its release and we close it at end-of-track. Reserve that object
                            // before growing the sounding-note stack.
                            budget.note(limits)?;
                            sounding
                                .entry((channel, pitch.as_int()))
                                .or_default()
                                .push((at, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key: pitch, vel: _ }
                        | MidiMessage::NoteOff { key: pitch, vel: _ } => {
                            let pitch = pitch.as_int();
                            if let Some(started) = sounding
                                .get_mut(&(channel, pitch))
                                .and_then(|stack| stack.pop())
                                && let Some(part) = parts.get_mut(&key)
                            {
                                part.notes.push(note(started, at, pitch, per_quarter));
                            }
                        }
                        MidiMessage::PitchBend { bend } => {
                            budget.automation_point(limits)?;
                            if let Some(part) = parts.get_mut(&key) {
                                part.bend.push(CurvePoint {
                                    at: scale(at, per_quarter),
                                    value: bend.as_f32() * part.bend_state.range(),
                                });
                            }
                        }
                        MidiMessage::Controller { controller, value }
                            if is_performance_controller(controller.as_int()) =>
                        {
                            budget.automation_point(limits)?;
                            if let Some(part) = parts.get_mut(&key) {
                                part.controllers
                                    .entry(controller.as_int())
                                    .or_default()
                                    .push(CurvePoint {
                                        at: scale(at, per_quarter),
                                        value: f32::from(value.as_int()) / 127.0,
                                    });
                            }
                        }
                        MidiMessage::Controller { controller, value } => {
                            if let Some(part) = parts.get_mut(&key) {
                                part.bend_state.receive(controller.as_int(), value.as_int());
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if let Some(name) = &track_name
                && let Some(part) = parts.get_mut(&(index, channel_of(&event.kind)))
            {
                part.name.get_or_insert_with(|| name.clone());
            }
        }

        // Anything still held when the track runs out is closed there rather than dropped: a file
        // that forgot a note-off still meant the note to sound, and a note with no length would be
        // silence the user cannot see the cause of.
        for ((channel, pitch), stack) in sounding {
            for started in stack {
                if let Some(part) = parts.get_mut(&(index, channel)) {
                    part.notes.push(note(started, at, pitch, per_quarter));
                }
            }
        }
        if let Some(name) = track_name {
            for key in order.iter().copied().filter(|(track, _)| *track == index) {
                if let Some(part) = parts.get_mut(&key) {
                    part.name.get_or_insert_with(|| name.clone());
                }
            }
        }
    }

    let tracks = order
        .into_iter()
        .filter_map(|key| {
            let part = parts.remove(&key)?;
            (!part.notes.is_empty()).then(|| {
                let mut notes = part.notes;
                notes.sort_by_key(|note| (note.start, note.pitch));
                let mut bend = part.bend;
                bend.sort_by_key(|point| point.at);
                let mut controllers = part.controllers;
                for points in controllers.values_mut() {
                    points.sort_by_key(|point| point.at);
                }
                MidiTrack {
                    name: part.name.unwrap_or_else(|| default_name(key.1)),
                    channel: key.1,
                    notes,
                    bend,
                    controllers,
                }
            })
        })
        .collect();

    if smf.header.format == Format::Sequential {
        // Format 2 is a bag of separate songs in one file, not one song in several tracks. There
        // is no right way to lay them on a single timeline, and they are rare enough that guessing
        // is worse than saying so — they come in stacked, and the log is where that is admitted.
        log::warn!(
            "MIDI file is format 2: its {} tracks are separate pieces, imported stacked together",
            smf.tracks.len()
        );
    }
    Ok(MidiImport {
        tempo_map,
        signatures,
        tracks,
    })
}

struct InstrumentSeed {
    track_index: usize,
    channel: u4,
}

struct ExportTrackPlan {
    track_index: usize,
    channel: u4,
    bend_range: f32,
    event_capacity: usize,
}

struct ExportPlan {
    conductor_capacity: usize,
    instrument_tracks: Vec<ExportTrackPlan>,
}

fn export_too_large(resource: MidiExportResource, observed: u128, limit: usize) -> IoError {
    IoError::MidiExportTooLarge {
        resource,
        observed: u64::try_from(observed).unwrap_or(u64::MAX),
        limit: u64::try_from(limit).unwrap_or(u64::MAX),
    }
}

fn add_export_count(
    current: &mut u128,
    additional: u128,
    limit: usize,
    resource: MidiExportResource,
) -> Result<()> {
    let observed = current.saturating_add(additional);
    if observed > limit as u128 {
        return Err(export_too_large(resource, observed, limit));
    }
    *current = observed;
    Ok(())
}

fn reserve_export<T>(values: &mut Vec<T>, additional: usize, purpose: &str) -> Result<()> {
    values.try_reserve_exact(additional).map_err(|_| {
        IoError::MidiWrite(format!(
            "could not allocate {additional} entries for {purpose}"
        ))
    })
}

fn pitch_contour_points(transforms: &[NoteTransform]) -> u128 {
    transforms
        .iter()
        .map(|transform| match transform {
            NoteTransform::Pitch { settings } => settings.volume_contour.points().len() as u128,
            NoteTransform::ForDrumVoice { transforms, .. } => pitch_contour_points(transforms),
            _ => 0,
        })
        .fold(0, u128::saturating_add)
}

/// Conservative ceiling for the vector built by `sounding_performance_curve_events`.
///
/// Keep this in step with the render scheduler's corresponding bound. The regular sampling grid,
/// authored corners, per-pass resets, generated note edges/contours and the five-millisecond
/// generated-performance grid are all charged before the curve builder is allowed to allocate.
fn curve_event_upper_bound(
    clip: &MidiClip,
    which: ClipCurve,
    tempo_map: &TempoMap,
    passes: u128,
    note_instances: u128,
) -> u128 {
    let total_ticks = clip.sounding_length().raw().max(0) as u128;
    let step = CURVE_STEP.raw().max(1) as u128;
    let mut events = total_ticks
        .div_ceil(step)
        .saturating_add(passes.saturating_mul(5))
        .saturating_add((clip.curve(which).len() as u128).saturating_mul(passes));

    if clip.has_generated_curve(which) {
        events = events
            .saturating_add(note_instances.saturating_mul(4))
            .saturating_add(note_instances.saturating_mul(pitch_contour_points(&clip.transforms)));
        let start = tempo_map.ticks_to_seconds(clip.start).0;
        let end = tempo_map
            .ticks_to_seconds(clip.start + clip.sounding_length())
            .0;
        let seconds = (end - start).max(0.0);
        let five_ms_samples = if seconds.is_finite() {
            (seconds / 0.005).ceil() as u128
        } else {
            u128::MAX
        };
        events = events.saturating_add(five_ms_samples);
    }
    events
}

/// Proves the complete expansion fits before any performed-note or performance-curve vector is
/// created. A later track therefore cannot evade the file-wide budget after earlier tracks have
/// already consumed memory.
fn preflight_export(project: &Project, limits: ExportLimits) -> Result<ExportPlan> {
    let mut seeds = Vec::new();
    reserve_export(
        &mut seeds,
        project.tracks.len(),
        "the MIDI export track plan",
    )?;
    let mut melodic_index = 0_u8;
    for (track_index, track) in project.tracks.iter().enumerate() {
        if track.kind.as_instrument().is_none() {
            continue;
        }
        let channel = u4::new(if track.kind.is_drum() {
            9
        } else {
            let channel = melodic_index + u8::from(melodic_index >= 9);
            melodic_index = (melodic_index + 1) % 15;
            channel
        });
        seeds.push(InstrumentSeed {
            track_index,
            channel,
        });
    }

    // Pitch-bend sensitivity is channel state, so a wide bend on any reused channel adds the RPN
    // handshake to every track assigned that channel.
    let mut bend_ranges = [BEND_RANGE; 16];
    for seed in &seeds {
        let instrument = project.tracks[seed.track_index]
            .kind
            .as_instrument()
            .ok_or_else(|| IoError::MidiWrite("instrument export plan changed".to_string()))?;
        if instrument
            .clips
            .iter()
            .filter(|clip| !clip.muted)
            .any(|clip| {
                clip.has_pitch_performance()
                    || clip.bend.iter().any(|point| point.value.abs() > BEND_RANGE)
            })
        {
            bend_ranges[usize::from(seed.channel.as_int())] = 12.0;
        }
    }

    let conductor_capacity = (project.tempo_map.points().len() as u128)
        .saturating_add(project.signatures.points().len() as u128)
        .saturating_add(2);
    let mut conductor_events = 0;
    add_export_count(
        &mut conductor_events,
        conductor_capacity,
        limits.track_events,
        MidiExportResource::TrackEvents,
    )?;
    let mut file_events = 0;
    add_export_count(
        &mut file_events,
        conductor_capacity,
        limits.file_events,
        MidiExportResource::FileEvents,
    )?;

    let mut file_notes = 0;
    let mut file_curve_events = 0;
    let mut instrument_tracks = Vec::new();
    reserve_export(
        &mut instrument_tracks,
        seeds.len(),
        "the MIDI export track plan",
    )?;
    for seed in seeds {
        let instrument = project.tracks[seed.track_index]
            .kind
            .as_instrument()
            .ok_or_else(|| IoError::MidiWrite("instrument export plan changed".to_string()))?;
        let bend_range = bend_ranges[usize::from(seed.channel.as_int())];
        let mut track_notes = 0;
        let mut track_curve_events = 0;
        let mut track_events = 0;
        let fixed_events = 2 + u128::from(bend_range > BEND_RANGE) * 6;
        add_export_count(
            &mut track_events,
            fixed_events,
            limits.track_events,
            MidiExportResource::TrackEvents,
        )?;
        add_export_count(
            &mut file_events,
            fixed_events,
            limits.file_events,
            MidiExportResource::FileEvents,
        )?;

        for clip in instrument.clips.iter().filter(|clip| !clip.muted) {
            let passes = validated_loop_pass_count(clip.id, clip.length, clip.loop_end)? as u128;
            let notes = clip.looped_note_instances()? as u128;
            add_export_count(
                &mut track_notes,
                notes,
                limits.track_notes,
                MidiExportResource::TrackNotes,
            )?;
            add_export_count(
                &mut file_notes,
                notes,
                limits.file_notes,
                MidiExportResource::FileNotes,
            )?;
            let note_events = notes.saturating_mul(2);
            add_export_count(
                &mut track_events,
                note_events,
                limits.track_events,
                MidiExportResource::TrackEvents,
            )?;
            add_export_count(
                &mut file_events,
                note_events,
                limits.file_events,
                MidiExportResource::FileEvents,
            )?;

            for which in clip.performance_curves() {
                let curve_events =
                    curve_event_upper_bound(clip, which, &project.tempo_map, passes, notes);
                add_export_count(
                    &mut track_curve_events,
                    curve_events,
                    limits.track_curve_events,
                    MidiExportResource::TrackCurveEvents,
                )?;
                add_export_count(
                    &mut file_curve_events,
                    curve_events,
                    limits.file_curve_events,
                    MidiExportResource::FileCurveEvents,
                )?;
                add_export_count(
                    &mut track_events,
                    curve_events,
                    limits.track_events,
                    MidiExportResource::TrackEvents,
                )?;
                add_export_count(
                    &mut file_events,
                    curve_events,
                    limits.file_events,
                    MidiExportResource::FileEvents,
                )?;
            }
        }
        instrument_tracks.push(ExportTrackPlan {
            track_index: seed.track_index,
            channel: seed.channel,
            bend_range,
            event_capacity: usize::try_from(track_events).map_err(|_| {
                export_too_large(
                    MidiExportResource::TrackEvents,
                    track_events,
                    limits.track_events,
                )
            })?,
        });
    }
    Ok(ExportPlan {
        conductor_capacity: usize::try_from(conductor_capacity).map_err(|_| {
            export_too_large(
                MidiExportResource::TrackEvents,
                conductor_capacity,
                limits.track_events,
            )
        })?,
        instrument_tracks,
    })
}

/// A complete, synchronised MIDI file awaiting atomic publication.
///
/// Dropping it removes only the private sibling. Its publication methods are the only operations
/// that make its bytes visible at the requested destination.
pub struct StagedMidi {
    file: NamedTempFile,
    path: PathBuf,
    notes: usize,
}

impl StagedMidi {
    /// Number of performed notes encoded in the staged file.
    pub fn notes(&self) -> usize {
        self.notes
    }

    /// Atomically replaces the destination with the already-synchronised MIDI file.
    pub fn publish(self) -> Result<usize> {
        crate::project_file::publish_ready_staged_file(self.file, &self.path)?;
        Ok(self.notes)
    }

    /// Atomically claims the still-absent destination without replacing another writer's file.
    pub fn publish_noclobber(self) -> Result<usize> {
        crate::project_file::publish_ready_staged_file_noclobber(self.file, &self.path)?;
        Ok(self.notes)
    }
}

/// Encodes and synchronises a project into a private sibling of `path` without publishing it.
///
/// This is the worker half of a two-phase export. Encoding, allocation, flushing and file sync
/// happen before this function returns; publication is one short rename after the caller has
/// revalidated cancellation and live document ownership.
pub fn stage_midi_file(path: &Path, project: &Project) -> Result<StagedMidi> {
    let (smf_tracks, notes) = build_tracks(project)?;
    let smf = Smf {
        header: Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(TICKS_PER_QUARTER as u16)),
        ),
        tracks: smf_tracks,
    };
    let mut file = crate::project_file::new_staged_file(path)?;
    smf.write_std(file.as_file_mut())
        .and_then(|()| file.flush())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|source| IoError::from_fs(path, source))?;
    Ok(StagedMidi {
        file,
        path: path.to_path_buf(),
        notes,
    })
}

/// Writes the project's instrument tracks as a Standard MIDI File.
///
/// Format 1, at this application's own division — [`TICKS_PER_QUARTER`] is 960 and an SMF header
/// holds up to 32767, so nothing is scaled on the way out and a file written here reads back as
/// exactly the notes that went into it.
///
/// What does not travel, because a MIDI file has nowhere to put it: audio tracks, which have no
/// notes; every mixer setting, including mute and solo; the instrument each track plays; and the
/// automation. A `.mid` is the notes and the clock, and saying so here is better than a reader
/// discovering it by comparing two files.
pub fn write_midi_file(path: &Path, project: &Project) -> Result<usize> {
    stage_midi_file(path, project)?.publish()
}

/// [`write_midi_file`] into memory, so a round trip can be tested without a filesystem.
pub fn write_midi_bytes(project: &Project) -> Result<Vec<u8>> {
    let (tracks, _) = build_tracks(project)?;
    let smf = Smf {
        header: Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(TICKS_PER_QUARTER as u16)),
        ),
        tracks,
    };
    let mut bytes = Vec::new();
    smf.write(&mut bytes)
        .map_err(|error| IoError::MidiWrite(error.to_string()))?;
    Ok(bytes)
}

/// The file's tracks, and how many notes went into them.
fn build_tracks(project: &Project) -> Result<(Vec<Vec<TrackEvent<'static>>>, usize)> {
    let plan = preflight_export(project, EXPORT_LIMITS)?;
    let mut tracks = Vec::new();
    reserve_export(
        &mut tracks,
        plan.instrument_tracks.len().saturating_add(1),
        "the Standard MIDI File track list",
    )?;
    tracks.push(conductor_track(project, plan.conductor_capacity)?);
    let mut count = 0usize;
    for track_plan in plan.instrument_tracks {
        let track = project.tracks.get(track_plan.track_index).ok_or_else(|| {
            IoError::MidiWrite("instrument export plan no longer names a track".to_string())
        })?;
        let instrument = track.kind.as_instrument().ok_or_else(|| {
            IoError::MidiWrite("instrument export plan no longer names an instrument".to_string())
        })?;
        let channel = track_plan.channel;
        let mut events: Vec<(Ticks, TrackEventKind<'static>)> = Vec::new();
        reserve_export(
            &mut events,
            track_plan.event_capacity.saturating_sub(2),
            "an exported MIDI track",
        )?;
        let bend_range = track_plan.bend_range;
        if bend_range > BEND_RANGE {
            // RPN 0 is pitch-bend sensitivity, followed by a null RPN selection.
            for (number, value) in [
                (101_u8, 0_u8),
                (100, 0),
                (6, 12),
                (38, 0),
                (101, 127),
                (100, 127),
            ] {
                events.push((
                    Ticks::ZERO,
                    controller_message(channel, number, f32::from(value) / 127.0),
                ));
            }
        }
        for clip in &instrument.clips {
            if clip.muted {
                continue;
            }
            // The *sounding* notes, repeats and all. A MIDI file has no notion of a region that
            // repeats, so a loop is written out as the notes it plays — which is also the only
            // reading that matches what the renderer does with the same clip. The tempo handed
            // over is the one the renderer reads for the same clip, so a humanised wobble lands
            // on the same ticks in the file as in the mix.
            for note in clip.sounding_notes_with_meter(
                project.tempo_map.bpm_at(clip.start),
                project.signatures.clone(),
            ) {
                count = count.checked_add(1).ok_or_else(|| {
                    export_too_large(
                        MidiExportResource::FileNotes,
                        u128::MAX,
                        MAX_MIDI_EXPORT_FILE_NOTES,
                    )
                })?;
                ensure_planned_events(&events, 2, track_plan.event_capacity)?;
                let start = clip.start + note.start;
                events.push((start, message(channel, note.pitch, velocity(note.velocity))));
                events.push((
                    start + note.length,
                    message(channel, note.pitch, u7::new(0)),
                ));
            }
            // The curves, sampled by the clip's own rule rather than by one of this file's. What
            // the wire carries — fourteen bits of bend, seven of controller — is this file's
            // business and stops here; the document works in semitones and in a fraction.
            for which in clip.performance_curves() {
                let curve_events = clip.sounding_performance_curve_events(
                    which,
                    CURVE_STEP,
                    &project.tempo_map,
                    &project.signatures,
                );
                ensure_planned_events(&events, curve_events.len(), track_plan.event_capacity)?;
                for (at, value) in curve_events {
                    let message = match which {
                        ClipCurve::Bend => bend_message(channel, value, bend_range),
                        ClipCurve::Controller(number) => controller_message(channel, number, value),
                    };
                    events.push((clip.start + at, message));
                }
            }
        }
        // Sorted by position, and at one position the releases go first: a note struck again at
        // the instant the last one ended must not have its release land on the new one.
        events.sort_by_key(|(at, kind)| {
            (
                *at,
                if is_release(kind) {
                    0
                } else if matches!(
                    kind,
                    TrackEventKind::Midi {
                        message: MidiMessage::NoteOn { .. },
                        ..
                    }
                ) {
                    2
                } else {
                    1
                },
            )
        });
        tracks.push(delta_encode(track.name.clone(), events)?);
    }
    Ok((tracks, count))
}

fn ensure_planned_events<T>(events: &[T], additional: usize, event_capacity: usize) -> Result<()> {
    if events.len().saturating_add(additional) > event_capacity.saturating_sub(2) {
        return Err(IoError::MidiWrite(
            "internal MIDI export event bound was exceeded".to_string(),
        ));
    }
    Ok(())
}

/// The first track of a format 1 file: the clock, and nothing that makes a sound.
fn conductor_track(project: &Project, event_capacity: usize) -> Result<Vec<TrackEvent<'static>>> {
    let mut events: Vec<(Ticks, TrackEventKind<'static>)> = Vec::new();
    reserve_export(
        &mut events,
        event_capacity.saturating_sub(2),
        "the MIDI conductor track",
    )?;
    for point in project.tempo_map.points() {
        let micros = (60_000_000.0 / point.bpm).round().clamp(1.0, MAX_MICROS) as u32;
        events.push((
            point.tick,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(micros))),
        ));
    }
    for point in project.signatures.points() {
        // Back to the power of two the file wants. Every denominator this application holds is
        // one, so the count of trailing zeros is exact rather than rounded.
        let power = point.signature.denominator.trailing_zeros().min(255) as u8;
        events.push((
            point.tick,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                point.signature.numerator.min(255) as u8,
                power,
                24,
                8,
            )),
        ));
    }
    events.sort_by_key(|(at, _)| *at);
    delta_encode(project.name.clone(), events)
}

/// Turns absolute positions into the deltas a file stores, with a name and an end marker.
fn delta_encode(
    name: String,
    events: Vec<(Ticks, TrackEventKind<'static>)>,
) -> Result<Vec<TrackEvent<'static>>> {
    let capacity = events
        .len()
        .checked_add(2)
        .ok_or_else(|| IoError::MidiWrite("MIDI event count overflowed".to_string()))?;
    let mut out = Vec::new();
    reserve_export(&mut out, capacity, "a delta-encoded MIDI track")?;
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::TrackName(
            // Leaked on purpose and once per track: `midly` borrows the bytes of a meta event for
            // the file's lifetime, and a name is a handful of bytes written once per export.
            Box::leak(name.into_bytes().into_boxed_slice()),
        )),
    });
    let mut previous = Ticks::ZERO;
    for (at, kind) in events {
        let delta = (at - previous).raw().max(0) as u32;
        if delta > 0x0fff_ffff {
            return Err(IoError::MidiWrite(format!(
                "event delta of {delta} ticks exceeds the Standard MIDI File limit"
            )));
        }
        previous = at;
        out.push(TrackEvent {
            delta: u28::new(delta),
            kind,
        });
    }
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    Ok(out)
}

/// A note-on, or a note-off when the velocity is zero.
fn message(channel: u4, pitch: u8, vel: u7) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel,
        message: MidiMessage::NoteOn {
            key: u7::new(pitch.min(127)),
            vel,
        },
    }
}

/// Default pitch-bend sensitivity when no RPN range was supplied.
///
/// Two, which is MIDI's default and what a receiver assumes when nothing has told it otherwise.
/// Larger exported bends explicitly select an octave through RPN 0.
const BEND_RANGE: f32 = 2.0;

/// Whether a controller shapes a performance, rather than addressing an instrument.
///
/// What comes back as a lane the user can see and edit. The ones left out are the file's own
/// plumbing: bank select and its fine half, the data entry and increment pair, the RPN and NRPN
/// selectors that those write into, and the channel mode messages from 120 up — "all notes off"
/// is a thing that happens to a performance, not a thing anybody performed.
///
/// Free rather than a match inside the reader so it can be tested, and so the reverse question —
/// which numbers a lane may be *drawn* on — has one answer to point at.
pub fn is_performance_controller(number: u8) -> bool {
    !matches!(number, 0 | 32 | 6 | 38 | 96..=101 | 120..=127)
}

/// A pitch bend message carrying `semitones`.
fn bend_message(channel: u4, semitones: f32, range: f32) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel,
        message: MidiMessage::PitchBend {
            bend: midly::PitchBend::from_f32((semitones / range).clamp(-1.0, 1.0)),
        },
    }
}

/// A controller message carrying `value`, from 0 to 1.
///
/// The seven bits are the wire's business and stop here: the document works in a fraction, the
/// way [`NoteEvent::Controller`](auris_core::NoteEvent::Controller) does.
fn controller_message(channel: u4, number: u8, value: f32) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel,
        message: MidiMessage::Controller {
            controller: u7::new(number.min(CONTROLLER_MAX)),
            value: u7::new((value.clamp(0.0, 1.0) * 127.0).round() as u8),
        },
    }
}

/// Whether an event releases a note rather than starting one.
fn is_release(kind: &TrackEventKind<'static>) -> bool {
    matches!(
        kind,
        TrackEventKind::Midi {
            message: MidiMessage::NoteOn { vel, .. },
            ..
        } if vel.as_int() == 0
    )
}

/// Our 0.0..=1.0 as MIDI's 1..=127.
///
/// Never zero: a note-on at zero velocity *is* a note-off, so a note played that softly would
/// release itself the instant it started.
fn velocity(value: f32) -> u7 {
    let scaled = (value.clamp(0.0, 1.0) * 127.0).round() as u8;
    u7::new(scaled.clamp(1, 127))
}

/// Largest tempo a file can name, in microseconds per quarter: `u24`'s ceiling.
const MAX_MICROS: f64 = 16_777_215.0;

/// What a file says when it says nothing: the tempo every sequencer assumes.
const DEFAULT_BPM: f64 = 120.0;

/// Largest power of two a denominator byte is allowed to name.
///
/// The byte is a shift, so an absurd one would shift a `u32` off its own end. Sixteenth notes are
/// the finest denominator [`TimeSignature`] holds, and a file naming a 1/1024 meter is a file that
/// has gone wrong rather than one making a point.
///
/// A limit to *refuse* by, never to clamp to: a 7/32 meter forced into range becomes a 7/16 one
/// nothing can tell from a meter the file really wrote.
const MAX_DENOMINATOR_POWER: u32 = 4;

/// One track-and-channel's notes as they are gathered.
#[derive(Default)]
struct Part {
    bend_state: BendState,
    name: Option<String>,
    notes: Vec<Note>,
    /// The bend, at absolute ticks. Rebased onto the clip once the clip's start is known.
    bend: Vec<CurvePoint>,
    /// The controllers, on the same terms.
    controllers: BTreeMap<u8, Vec<CurvePoint>>,
}

struct BendState {
    msb: u8,
    lsb: u8,
    semitones: u8,
    cents: u8,
}
impl Default for BendState {
    fn default() -> Self {
        Self {
            msb: 127,
            lsb: 127,
            semitones: 2,
            cents: 0,
        }
    }
}
impl BendState {
    fn range(&self) -> f32 {
        f32::from(self.semitones) + f32::from(self.cents.min(99)) / 100.0
    }
    fn receive(&mut self, number: u8, value: u8) {
        match number {
            101 => self.msb = value,
            100 => self.lsb = value,
            98 | 99 => {
                self.msb = 127;
                self.lsb = 127;
            }
            6 if self.msb == 0 && self.lsb == 0 => self.semitones = value,
            38 if self.msb == 0 && self.lsb == 0 => self.cents = value,
            _ => {}
        }
    }
}

/// The channel an event belongs to, or 0 for the ones that belong to the track as a whole.
fn channel_of(kind: &TrackEventKind) -> u8 {
    match kind {
        TrackEventKind::Midi { channel, .. } => channel.as_int(),
        _ => 0,
    }
}

/// A track with no name of its own, called after the channel it played on.
fn default_name(channel: u8) -> String {
    match channel {
        // Channel 10, counting from one. Every General MIDI file puts its drums there, and it is
        // the only thing a bare SMF says about what a track is for.
        9 => "Drums".to_string(),
        other => format!("Channel {}", u16::from(other) + 1),
    }
}

/// Turns a position in the file's own division into one of ours.
///
/// The multiply comes first on purpose. Dividing first would round every position onto the file's
/// grid before it reached ours, which for a file written at a coarse division is audible as every
/// note landing a little early or late.
fn scale(at: u64, per_quarter: u32) -> Ticks {
    let scaled = (at as i128 * i128::from(TICKS_PER_QUARTER)) / i128::from(per_quarter);
    Ticks(scaled.clamp(0, i128::from(i64::MAX)) as i64)
}

/// A note from its onset and release, in the file's division.
fn note(started: (u64, u8), ended: u64, pitch: u8, per_quarter: u32) -> Note {
    let start = scale(started.0, per_quarter);
    // At least one tick: a note-on and note-off at the same instant is something a file can
    // contain, and a zero-length note is one nothing would ever play or let you grab hold of.
    let length = Ticks((scale(ended, per_quarter) - start).raw().max(1));
    Note {
        // MIDI velocity is 1..=127 and ours is 0.0..=1.0. Divided by 127 rather than 128 so a
        // full-strength note comes out at exactly 1.0 rather than a hair under it.
        velocity: f32::from(started.1) / 127.0,
        ..Note::new(pitch, start, length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempFile;
    use auris_core::plugin::CC_MODULATION;
    use midly::num::{u4, u7, u15, u24, u28};
    use midly::{Header, Track, TrackEvent};
    use std::fs::OpenOptions;
    use std::io::Write;

    /// Ticks per quarter used by the fixtures, deliberately not ours.
    const PPQ: u16 = 480;

    #[test]
    fn midi_file_size_limit_accepts_its_edges_and_rejects_the_next_byte() {
        const LIMIT: usize = 32;
        for length in [LIMIT - 1, LIMIT] {
            let file = TempFile::new(&format!("midi-{length}.mid"));
            std::fs::write(file.path(), vec![0x5a; length]).unwrap();
            assert_eq!(read_midi_source(file.path(), LIMIT).unwrap().len(), length);
        }

        let file = TempFile::new("midi-too-large.mid");
        std::fs::write(file.path(), vec![0x5a; LIMIT + 1]).unwrap();
        assert!(matches!(
            read_midi_source(file.path(), LIMIT),
            Err(IoError::MidiFileTooLarge {
                observed,
                limit,
                ..
            }) if observed == (LIMIT + 1) as u64 && limit == LIMIT as u64
        ));
    }

    #[test]
    fn in_memory_midi_data_uses_the_same_inclusive_size_boundary() {
        const LIMIT: usize = 32;
        assert!(ensure_midi_data_size(&[0; LIMIT - 1], LIMIT).is_ok());
        assert!(ensure_midi_data_size(&[0; LIMIT], LIMIT).is_ok());
        assert!(matches!(
            ensure_midi_data_size(&[0; LIMIT + 1], LIMIT),
            Err(IoError::MidiDataTooLarge {
                observed,
                limit,
            }) if observed == (LIMIT + 1) as u64 && limit == LIMIT as u64
        ));
    }

    #[test]
    fn a_midi_file_that_grows_after_metadata_is_still_bounded() {
        const LIMIT: usize = 32;
        let file = TempFile::new("growing.mid");
        std::fs::write(file.path(), vec![0x5a; LIMIT]).unwrap();

        let result = read_midi_source_after_metadata(file.path(), LIMIT, || {
            let mut append = OpenOptions::new().append(true).open(file.path()).unwrap();
            append.write_all(&[0x7f]).unwrap();
            append.flush().unwrap();
        });

        assert!(matches!(
            result,
            Err(IoError::MidiFileTooLarge { observed, .. })
                if observed == (LIMIT + 1) as u64
        ));
    }

    fn event(delta: u32, kind: TrackEventKind<'_>) -> TrackEvent<'_> {
        TrackEvent {
            delta: u28::new(delta),
            kind,
        }
    }

    fn note_on(delta: u32, pitch: u8, velocity: u8) -> TrackEvent<'static> {
        event(
            delta,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(pitch),
                    vel: u7::new(velocity),
                },
            },
        )
    }

    fn note_off(delta: u32, pitch: u8) -> TrackEvent<'static> {
        event(
            delta,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOff {
                    key: u7::new(pitch),
                    vel: u7::new(64),
                },
            },
        )
    }

    fn parse(header: Header, tracks: Vec<Track<'_>>) -> MidiImport {
        let smf = Smf { header, tracks };
        let mut bytes = Vec::new();
        smf.write(&mut bytes).expect("the fixture is writable");
        read_midi_bytes(&bytes).expect("the fixture is readable")
    }

    fn metrical(tracks: Vec<Track<'_>>) -> MidiImport {
        parse(
            Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))),
            tracks,
        )
    }

    fn import_with_limits(tracks: Vec<Track<'_>>, limits: ImportLimits) -> Result<MidiImport> {
        read_smf_with_limits(
            &Smf {
                header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))),
                tracks,
            },
            limits,
        )
    }

    fn generous_limits() -> ImportLimits {
        ImportLimits {
            track_events: usize::MAX,
            file_events: usize::MAX,
            notes: usize::MAX,
            automation_points: usize::MAX,
            output_events: usize::MAX,
        }
    }

    fn track_name() -> TrackEvent<'static> {
        event(0, TrackEventKind::Meta(MetaMessage::TrackName(b"track")))
    }

    fn pitch_bend() -> TrackEvent<'static> {
        event(
            0,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::PitchBend {
                    bend: midly::PitchBend::from_f32(0.0),
                },
            },
        )
    }

    #[test]
    fn production_midi_budgets_accept_the_boundary_and_reject_the_next_item() {
        let mut budget = ImportBudget {
            notes: MAX_MIDI_NOTES - 1,
            ..ImportBudget::default()
        };
        budget.note(IMPORT_LIMITS).expect("the last note fits");
        assert!(matches!(
            budget.note(IMPORT_LIMITS),
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::Notes,
                observed,
                limit,
            }) if observed == (MAX_MIDI_NOTES + 1) as u64
                && limit == MAX_MIDI_NOTES as u64
        ));

        let mut budget = ImportBudget {
            automation_points: MAX_MIDI_AUTOMATION_POINTS - 1,
            ..ImportBudget::default()
        };
        budget
            .automation_point(IMPORT_LIMITS)
            .expect("the last automation point fits");
        assert!(matches!(
            budget.automation_point(IMPORT_LIMITS),
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::AutomationPoints,
                observed,
                limit,
            }) if observed == (MAX_MIDI_AUTOMATION_POINTS + 1) as u64
                && limit == MAX_MIDI_AUTOMATION_POINTS as u64
        ));

        let mut budget = ImportBudget {
            file_events: MAX_MIDI_FILE_EVENTS - 1,
            ..ImportBudget::default()
        };
        let mut track_events = 0;
        budget
            .source_event(&mut track_events, IMPORT_LIMITS)
            .expect("the last file event fits");
        assert!(matches!(
            budget.source_event(&mut track_events, IMPORT_LIMITS),
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::FileEvents,
                observed,
                limit,
            }) if observed == (MAX_MIDI_FILE_EVENTS + 1) as u64
                && limit == MAX_MIDI_FILE_EVENTS as u64
        ));

        let mut budget = ImportBudget::default();
        let mut track_events = MAX_MIDI_TRACK_EVENTS - 1;
        budget
            .source_event(&mut track_events, IMPORT_LIMITS)
            .expect("the last event in one track fits");
        assert!(matches!(
            budget.source_event(&mut track_events, IMPORT_LIMITS),
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::TrackEvents,
                observed,
                limit,
            }) if observed == (MAX_MIDI_TRACK_EVENTS + 1) as u64
                && limit == MAX_MIDI_TRACK_EVENTS as u64
        ));

        let mut budget = ImportBudget {
            output_events: MAX_MIDI_OUTPUT_EVENTS - 1,
            ..ImportBudget::default()
        };
        budget
            .timeline_point(IMPORT_LIMITS)
            .expect("the last retained event fits");
        assert!(matches!(
            budget.timeline_point(IMPORT_LIMITS),
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::OutputEvents,
                observed,
                limit,
            }) if observed == (MAX_MIDI_OUTPUT_EVENTS + 1) as u64
                && limit == MAX_MIDI_OUTPUT_EVENTS as u64
        ));
    }

    #[test]
    fn event_budget_is_aggregated_across_source_tracks() {
        let mut limits = generous_limits();
        limits.track_events = 2;
        let one_track =
            import_with_limits(vec![vec![track_name(), track_name(), track_name()]], limits);
        assert!(matches!(
            one_track,
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::TrackEvents,
                observed: 3,
                limit: 2,
            })
        ));

        let mut limits = generous_limits();
        limits.track_events = 3;
        limits.file_events = 4;
        let result = import_with_limits(
            vec![
                vec![track_name(), track_name(), track_name()],
                vec![track_name(), track_name(), track_name()],
            ],
            limits,
        );
        assert!(matches!(
            result,
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::FileEvents,
                observed: 5,
                limit: 4,
            })
        ));
    }

    #[test]
    fn note_and_automation_budgets_cannot_be_reset_by_splitting_tracks() {
        let mut limits = generous_limits();
        limits.notes = 2;
        let notes = import_with_limits(
            vec![
                vec![note_on(0, 60, 100)],
                vec![note_on(0, 61, 100)],
                vec![note_on(0, 62, 100)],
            ],
            limits,
        );
        assert!(matches!(
            notes,
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::Notes,
                observed: 3,
                limit: 2,
            })
        ));

        let mut limits = generous_limits();
        limits.automation_points = 2;
        let automation = import_with_limits(
            vec![
                vec![note_on(0, 60, 100), pitch_bend(), pitch_bend()],
                vec![note_on(0, 61, 100), pitch_bend(), pitch_bend()],
            ],
            limits,
        );
        assert!(matches!(
            automation,
            Err(IoError::MidiImportTooLarge {
                resource: MidiImportResource::AutomationPoints,
                observed: 3,
                limit: 2,
            })
        ));
    }

    #[test]
    fn a_quarter_note_in_the_files_division_is_a_quarter_note_in_ours() {
        // The whole point of the scaling: a file counts in its own ticks and we count in ours,
        // and a quarter note has to survive the trip as a quarter note.
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_off(u32::from(PPQ), 60),
        ]]);
        let note = &imported.tracks[0].notes[0];
        assert_eq!(note.start, Ticks::ZERO);
        assert_eq!(note.length, Ticks::QUARTER);
    }

    #[test]
    fn a_position_is_not_quantised_onto_the_files_own_grid() {
        // Dividing before multiplying would land this on a whole tick of the file's division and
        // move the note. At 480 ppq one file tick is a 1920th note; ours is finer.
        let imported = metrical(vec![vec![note_on(1, 60, 100), note_off(1, 60)]]);
        let note = &imported.tracks[0].notes[0];
        assert_eq!(note.start, Ticks(TICKS_PER_QUARTER / 480));
        assert!(note.start > Ticks::ZERO, "a tick of the file is not zero");
    }

    #[test]
    fn a_note_on_at_zero_velocity_is_a_note_off() {
        // Every sequencer that used running status emits these, so it is not an edge case.
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_on(u32::from(PPQ), 60, 0),
        ]]);
        assert_eq!(imported.note_count(), 1);
        assert_eq!(imported.tracks[0].notes[0].length, Ticks::QUARTER);
    }

    #[test]
    fn the_same_pitch_struck_twice_before_either_release_stays_two_notes() {
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_on(240, 60, 90),
            note_off(240, 60),
            note_off(240, 60),
        ]]);
        assert_eq!(imported.note_count(), 2);
    }

    #[test]
    fn a_note_nobody_released_is_closed_where_the_track_ends() {
        // A file that forgot a note-off still meant the note to sound, and a note with no length
        // would be silence with no visible cause.
        let imported = metrical(vec![vec![note_on(0, 60, 100), note_on(960, 62, 100)]]);
        assert_eq!(imported.note_count(), 2);
        assert!(
            imported.tracks[0]
                .notes
                .iter()
                .all(|note| note.length > Ticks::ZERO)
        );
    }

    #[test]
    fn velocity_arrives_as_a_fraction_with_full_strength_reaching_one() {
        let imported = metrical(vec![vec![note_on(0, 60, 127), note_off(480, 60)]]);
        assert_eq!(imported.tracks[0].notes[0].velocity, 1.0);
    }

    #[test]
    fn the_tempo_the_file_opens_with_is_the_songs_tempo() {
        // 500 000 microseconds per quarter is 120 bpm; 400 000 is 150.
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(400_000))),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 150.0);
        assert_eq!(
            imported.tempo_map.points().len(),
            1,
            "not a change on a default"
        );
    }

    #[test]
    fn a_tempo_change_further_in_becomes_a_point() {
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            ),
            note_on(0, 60, 100),
            note_off(1_920, 60),
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(250_000))),
            ),
        ]]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)),
            240.0
        );
    }

    #[test]
    fn a_later_track_restating_the_opening_tempo_does_not_erase_the_changes() {
        // A format 1 file often repeats the tempo at the head of every track, because a player
        // that started reading at track two would otherwise have nothing to go on. That is the
        // same tempo said twice, not an instruction to forget the rest of the map.
        let imported = metrical(vec![
            vec![
                event(
                    0,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
                ),
                event(
                    1_920,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(250_000))),
                ),
            ],
            vec![
                event(
                    0,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
                ),
                note_on(0, 60, 100),
                note_off(480, 60),
            ],
        ]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)),
            240.0,
            "the change in the first track survived the second track's restatement"
        );
        assert_eq!(imported.tempo_map.points().len(), 2);
    }

    #[test]
    fn a_tempo_that_first_appears_partway_in_leaves_the_head_at_the_default() {
        // Nothing said what the opening bars run at, so they run at the tempo every sequencer
        // assumes. Taking the first event as the song's tempo wherever it sat would play the
        // head at a speed the file never named for it.
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_off(1_920, 60),
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(250_000))),
            ),
        ]]);
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks::ZERO),
            120.0,
            "the head is the default, not the tempo that arrives four beats in"
        );
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)),
            240.0
        );
        assert_eq!(imported.tempo_map.points().len(), 2);
    }

    #[test]
    fn a_time_signature_arrives_with_its_denominator_read_as_a_power_of_two() {
        // The file stores 3 for an eighth. Reading it as a literal 3 would give 6/3.
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(6, 3, 24, 8)),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::new(6, 8)
        );
    }

    #[test]
    fn a_meter_finer_than_this_build_holds_is_refused_rather_than_rounded() {
        // 7/32. Clamping the power into range before checking it turned this into 7/16 — a meter
        // the file never claimed, indistinguishable afterwards from one that did, and arrived at
        // without a warning because the value was already in range by the time anything looked.
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(7, 5, 24, 8)),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::default(),
            "a meter that cannot be held is left out, not rounded into a different one"
        );

        // A denominator absurd enough to shift a `u32` off its end is the same answer, not a panic.
        let absurd = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(4, 200, 24, 8)),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(
            absurd.signatures.signature_at(Ticks::ZERO),
            TimeSignature::default()
        );

        // And the one either side of the limit still reads: a sixteenth is the finest we hold.
        let finest = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(7, 4, 24, 8)),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(
            finest.signatures.signature_at(Ticks::ZERO),
            TimeSignature::new(7, 16)
        );
    }

    #[test]
    fn a_file_with_no_tempo_or_meter_opens_at_the_one_every_sequencer_assumes() {
        let imported = metrical(vec![vec![note_on(0, 60, 100), note_off(480, 60)]]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::default()
        );
    }

    #[test]
    fn two_channels_in_one_track_come_out_as_two_tracks() {
        // A format 0 file is exactly this: one track carrying every channel. Merging them would
        // put a bass line inside the drum part.
        let on = |channel: u8, pitch: u8| {
            event(
                0,
                TrackEventKind::Midi {
                    channel: u4::new(channel),
                    message: MidiMessage::NoteOn {
                        key: u7::new(pitch),
                        vel: u7::new(100),
                    },
                },
            )
        };
        let imported = parse(
            Header::new(Format::SingleTrack, Timing::Metrical(u15::new(PPQ))),
            vec![vec![on(0, 60), on(9, 36), note_off(480, 60)]],
        );
        assert_eq!(imported.tracks.len(), 2);
        assert_eq!(imported.tracks[1].channel, 9);
        assert_eq!(
            imported.tracks[1].name, "Drums",
            "channel 10 is where drums go"
        );
    }

    #[test]
    fn a_track_keeps_the_name_the_file_gave_it() {
        let imported = metrical(vec![vec![
            event(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Bass"))),
            note_on(0, 40, 100),
            note_off(480, 40),
        ]]);
        assert_eq!(imported.tracks[0].name, "Bass");
    }

    #[test]
    fn a_track_with_no_notes_is_not_a_track() {
        // The conductor track of a format 1 file carries the tempo and nothing else, and an empty
        // track in the arrangement is a row that can only be deleted.
        let imported = metrical(vec![
            vec![event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            )],
            vec![note_on(0, 60, 100), note_off(480, 60)],
        ]);
        assert_eq!(imported.tracks.len(), 1);
    }

    #[test]
    fn a_file_counted_in_smpte_frames_is_refused_by_name() {
        // It has no beats, so it has no bars. Laying it on a musical timeline would mean choosing
        // a tempo on the file's behalf and writing it down as though the file had said so.
        let smf = Smf {
            header: Header::new(Format::Parallel, Timing::Timecode(midly::Fps::Fps25, 40)),
            tracks: vec![vec![note_on(0, 60, 100), note_off(40, 60)]],
        };
        let mut bytes = Vec::new();
        smf.write(&mut bytes).expect("writable");
        assert!(matches!(
            read_midi_bytes(&bytes),
            Err(IoError::MidiTimecode { .. })
        ));
    }

    #[test]
    fn rubbish_is_refused_rather_than_read_as_an_empty_song() {
        assert!(read_midi_bytes(b"this is not a MIDI file").is_err());
    }

    // ------------------------------------------------------------------- writing

    /// A project with one instrument track holding one clip of `notes`.
    fn project_with(notes: Vec<Note>, clip_length: Ticks) -> Project {
        let mut project = Project::new("Round Trip", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.chiptune");
        let clip = project
            .add_midi_clip(track, "Riff", Ticks::ZERO, clip_length)
            .expect("an instrument track takes a clip");
        project.midi_clip_mut(clip).expect("the clip").notes = notes;
        project
    }

    fn roomy_export_limits() -> ExportLimits {
        ExportLimits {
            track_notes: 100,
            file_notes: 100,
            track_curve_events: 100,
            file_curve_events: 100,
            track_events: 1_000,
            file_events: 1_000,
        }
    }

    fn add_looped_note_track(project: &mut Project, name: &str) {
        let track = project.add_instrument_track(name, "synth");
        let clip = project
            .add_midi_clip(track, name, Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let clip = project.midi_clip_mut(clip).unwrap();
        clip.notes = vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)];
        clip.loop_end = Ticks::QUARTER * 2;
    }

    #[test]
    fn export_note_budget_is_checked_per_track_and_across_all_tracks_before_expansion() {
        let mut one_track = Project::new("Track limit", 48_000.0);
        add_looped_note_track(&mut one_track, "Lead");
        let mut limits = roomy_export_limits();
        limits.track_notes = 1;
        assert!(matches!(
            preflight_export(&one_track, limits),
            Err(IoError::MidiExportTooLarge {
                resource: MidiExportResource::TrackNotes,
                observed: 2,
                limit: 1,
            })
        ));

        let mut project = Project::new("File limit", 48_000.0);
        add_looped_note_track(&mut project, "Lead");
        add_looped_note_track(&mut project, "Bass");
        let mut limits = roomy_export_limits();
        limits.track_notes = 2;
        limits.file_notes = 3;
        assert!(matches!(
            preflight_export(&project, limits),
            Err(IoError::MidiExportTooLarge {
                resource: MidiExportResource::FileNotes,
                observed: 4,
                limit: 3,
            })
        ));
    }

    #[test]
    fn export_curve_and_wire_event_budgets_cannot_be_reset_by_splitting_tracks() {
        let mut project = Project::new("Curve limit", 48_000.0);
        for name in ["Lead", "Bass"] {
            let track = project.add_instrument_track(name, "synth");
            let clip = project
                .add_midi_clip(track, name, Ticks::ZERO, Ticks(1))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().bend = vec![CurvePoint {
                at: Ticks::ZERO,
                value: 1.0,
            }];
        }
        let mut limits = roomy_export_limits();
        limits.track_curve_events = 6;
        assert!(matches!(
            preflight_export(&project, limits),
            Err(IoError::MidiExportTooLarge {
                resource: MidiExportResource::TrackCurveEvents,
                observed: 7,
                limit: 6,
            })
        ));

        let mut limits = roomy_export_limits();
        limits.track_curve_events = 7;
        limits.file_curve_events = 13;
        assert!(matches!(
            preflight_export(&project, limits),
            Err(IoError::MidiExportTooLarge {
                resource: MidiExportResource::FileCurveEvents,
                observed: 14,
                limit: 13,
            })
        ));

        let mut note_events = Project::new("Track wire limit", 48_000.0);
        add_looped_note_track(&mut note_events, "Lead");
        let mut limits = roomy_export_limits();
        limits.track_events = 5;
        assert!(matches!(
            preflight_export(&note_events, limits),
            Err(IoError::MidiExportTooLarge {
                resource: MidiExportResource::TrackEvents,
                observed: 6,
                limit: 5,
            })
        ));

        let mut empty_tracks = Project::new("Wire limit", 48_000.0);
        empty_tracks.add_instrument_track("Lead", "synth");
        empty_tracks.add_instrument_track("Bass", "synth");
        let mut limits = roomy_export_limits();
        limits.file_events = 7;
        assert!(matches!(
            preflight_export(&empty_tracks, limits),
            Err(IoError::MidiExportTooLarge {
                resource: MidiExportResource::FileEvents,
                observed: 8,
                limit: 7,
            })
        ));
    }

    #[test]
    fn staged_and_failed_midi_exports_never_damage_the_previous_destination() {
        let folder = TempFile::new("midi-export-folder");
        std::fs::create_dir(folder.path()).unwrap();
        let destination = folder.path().join("song.mid");
        std::fs::write(&destination, b"previous MIDI bytes").unwrap();
        let project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        );

        let staged = stage_midi_file(&destination, &project).unwrap();
        assert_eq!(staged.notes(), 1);
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous MIDI bytes");
        drop(staged);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);

        let mut invalid = project.clone();
        invalid.tracks[0].kind.as_instrument_mut().unwrap().clips[0].start = Ticks(0x1000_0000);
        assert!(write_midi_file(&destination, &invalid).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous MIDI bytes");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);

        assert_eq!(write_midi_file(&destination, &project).unwrap(), 1);
        assert_eq!(&std::fs::read(&destination).unwrap()[..4], b"MThd");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn staged_midi_noclobber_publish_is_atomic_and_loses_a_race_safely() {
        // Model-facing commands stage directly beside the requested destination, put their
        // cancellation boundary here, and then atomically claim a name that must remain absent.
        let folder = TempFile::new("midi-noclobber-folder");
        std::fs::create_dir(folder.path()).unwrap();
        let project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        );
        let destination = folder.path().join("tool-output.mid");
        assert_eq!(
            stage_midi_file(&destination, &project)
                .unwrap()
                .publish_noclobber()
                .unwrap(),
            1
        );

        assert_eq!(read_midi_file(&destination).unwrap().note_count(), 1);

        let raced = folder.path().join("raced.mid");
        let staged = stage_midi_file(&raced, &project).unwrap();
        std::fs::write(&raced, b"winning writer").unwrap();
        assert!(matches!(
            staged.publish_noclobber(),
            Err(IoError::ExportDestinationExists(path)) if path == raced
        ));
        assert_eq!(std::fs::read(&raced).unwrap(), b"winning writer");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    }

    fn round_trip(project: &Project) -> MidiImport {
        let bytes = write_midi_bytes(project).expect("writable");
        read_midi_bytes(&bytes).expect("readable")
    }

    #[test]
    fn articulated_midi_exports_the_same_notes_as_playback_in_compound_time() {
        let mut project = project_with(
            vec![
                Note::new(60, Ticks::ZERO, Ticks(480)),
                Note::new(64, Ticks::ZERO, Ticks(480)),
                Note::new(67, Ticks(1920), Ticks(480)),
            ],
            Ticks(2880),
        );
        project.signatures = auris_core::SignatureMap::constant(TimeSignature::new(6, 8));
        let clip = project.tracks[0]
            .kind
            .as_instrument_mut()
            .unwrap()
            .clips
            .first_mut()
            .unwrap();
        clip.start = Ticks(240);
        clip.loop_end = Ticks(4500);
        clip.transforms = vec![
            auris_core::NoteTransform::Brush { amount: 0.5 },
            auris_core::NoteTransform::Slide { amount: 0.5 },
            auris_core::NoteTransform::Mute { amount: 0.5 },
            auris_core::NoteTransform::Stroke {
                spread_ms: 40.0,
                direction: auris_core::StrokeDirection::LowToHigh,
            },
            auris_core::NoteTransform::Humanize {
                amount: 0.5,
                seed: 23,
            },
        ];
        let mut expected: Vec<_> = clip
            .sounding_notes_with_meter(120.0, project.signatures.clone())
            .map(|note| {
                (
                    note.start + clip.start,
                    note.pitch,
                    note.length,
                    velocity(note.velocity).as_int(),
                )
            })
            .collect();
        expected.sort();
        let imported = round_trip(&project);
        let mut actual: Vec<_> = imported.tracks[0]
            .notes
            .iter()
            .map(|note| {
                (
                    note.start,
                    note.pitch,
                    note.length,
                    velocity(note.velocity).as_int(),
                )
            })
            .collect();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn drum_tracks_export_on_channel_ten_and_melodic_tracks_skip_it() {
        let mut project = Project::new("Channels", 48_000.0);
        for index in 0..17 {
            let track = if index == 2 {
                project.add_drum_track("Kit", "kit")
            } else {
                project.add_instrument_track(format!("Melodic {index}"), "synth")
            };
            let clip = project
                .add_midi_clip(track, "Note", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                38,
                Ticks::ZERO,
                Ticks::QUARTER,
            ));
        }
        let imported = round_trip(&project);
        assert_eq!(imported.tracks.len(), 17);
        for track in imported.tracks {
            assert_eq!(track.channel == 9, track.name == "Kit");
        }
    }

    #[test]
    fn interleaved_drum_tracks_do_not_consume_melodic_midi_channels() {
        let mut project = Project::new("Channels", 48_000.0);
        project.add_audio_track("Audio");
        project.add_bus_track("Bus");
        for index in 0..18 {
            let track = if matches!(index, 1 | 7 | 13) {
                project.add_drum_track(format!("Kit {index}"), "kit")
            } else {
                project.add_instrument_track(format!("Melodic {index}"), "synth")
            };
            let clip = project
                .add_midi_clip(track, "Note", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                38,
                Ticks::ZERO,
                Ticks::QUARTER,
            ));
        }
        let imported = round_trip(&project);
        assert_eq!(imported.tracks.len(), 18);
        let drums: Vec<_> = imported
            .tracks
            .iter()
            .filter(|track| track.name.starts_with("Kit "))
            .collect();
        assert_eq!(drums.len(), 3);
        assert!(drums.iter().all(|track| track.channel == 9));
        let melodic: Vec<_> = imported
            .tracks
            .iter()
            .filter(|track| track.name.starts_with("Melodic "))
            .collect();
        assert_eq!(melodic.len(), 15);
        let channels: std::collections::BTreeSet<_> =
            melodic.iter().map(|track| track.channel).collect();
        assert_eq!(
            channels,
            (0_u8..16).filter(|channel| *channel != 9).collect()
        );
    }

    #[test]
    fn a_bend_goes_out_and_comes_back_as_the_same_slide() {
        // The one thing here that is *not* exact: a file carries fourteen bits across the range
        // a receiver assumes, so a semitone is quantised on the way through. The tolerance below
        // is a hundredth of one, which is a fiftieth of a cent and inaudible by a wide margin.
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4))],
            Ticks(TICKS_PER_QUARTER * 4),
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        project.midi_clip_mut(clip).expect("the clip").bend = vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 0.0,
            },
            CurvePoint {
                at: Ticks(TICKS_PER_QUARTER * 2),
                value: 2.0,
            },
        ];

        let imported = round_trip(&project);
        let bend = &imported.tracks[0].bend;
        assert!(!bend.is_empty(), "the bend did not survive the file");
        for pair in bend.windows(2) {
            assert!(pair[0].at <= pair[1].at, "out of order: {bend:?}");
        }
        let at = |tick: i64| {
            bend.iter()
                .rfind(|point| point.at <= Ticks(tick))
                .map(|point| point.value)
                .unwrap_or(0.0)
        };
        assert!(at(0).abs() < 0.01, "it started bent: {}", at(0));
        assert!(
            (at(TICKS_PER_QUARTER) - 1.0).abs() < 0.01,
            "halfway up is {}",
            at(TICKS_PER_QUARTER)
        );
        assert!(
            (at(TICKS_PER_QUARTER * 2) - 2.0).abs() < 0.01,
            "the top is {}",
            at(TICKS_PER_QUARTER * 2)
        );
        // And it is let go before the clip ends, or everything after it would play sharp.
        assert!(
            at(TICKS_PER_QUARTER * 4).abs() < 0.01,
            "the bend was left hanging at {}",
            at(TICKS_PER_QUARTER * 4)
        );

        // A file that never bends brings none, which keeps this off almost every import.
        assert!(
            round_trip(&project_with(
                vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
                Ticks::QUARTER
            ))
            .tracks[0]
                .bend
                .is_empty()
        );
    }

    #[test]
    fn generated_pitch_round_trips_with_explicit_range_and_precedes_the_attack() {
        let mut project = project_with(vec![Note::new(60, Ticks::ZERO, Ticks(1920))], Ticks(1920));
        let clip = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        project.midi_clip_mut(clip).unwrap().transforms = vec![auris_core::NoteTransform::Pitch {
            settings: auris_core::PitchPerformance {
                scoop: 3.0,
                fall: 6.0,
                ..auris_core::PitchPerformance::default()
            },
        }];
        let bytes = write_midi_bytes(&project).unwrap();
        let smf = Smf::parse(&bytes).unwrap();
        let events = &smf.tracks[1];
        let first_on = events.iter().position(|e| matches!(e.kind, TrackEventKind::Midi { message: MidiMessage::NoteOn { vel, .. }, .. } if vel.as_int() > 0)).unwrap();
        assert!(events[..first_on].iter().any(|e| matches!(
            e.kind,
            TrackEventKind::Midi {
                message: MidiMessage::PitchBend { .. },
                ..
            }
        )));
        let imported = round_trip(&project);
        assert_eq!(imported.tracks[0].notes.len(), 1);
        assert!((imported.tracks[0].bend[0].value + 3.0).abs() < 0.002);
        assert!(imported.tracks[0].bend.iter().any(|p| p.value < -5.9));
        assert_eq!(imported.tracks[0].bend.last().unwrap().value, 0.0);
        assert!(
            imported.tracks[0].controllers.is_empty(),
            "RPN setup is not an editable lane"
        );
    }

    #[test]
    fn generated_controllers_and_octave_layers_survive_midi_export() {
        let mut project = project_with(vec![Note::new(60, Ticks::ZERO, Ticks(3840))], Ticks(3840));
        let clip = project.tracks[0].kind.as_instrument().unwrap().clips[0].id;
        project.midi_clip_mut(clip).unwrap().transforms = vec![
            auris_core::NoteTransform::Pitch {
                settings: auris_core::PitchPerformance {
                    modulation: 0.7,
                    volume_swell: 1.0,
                    ..auris_core::PitchPerformance::default()
                },
            },
            auris_core::NoteTransform::Octaves {
                above: 0.5,
                below: 0.5,
            },
        ];
        let imported = round_trip(&project);
        let track = &imported.tracks[0];
        let mut pitches: Vec<_> = track.notes.iter().map(|n| n.pitch).collect();
        pitches.sort();
        assert_eq!(pitches, [48, 60, 72]);
        assert!(track.controllers[&1].iter().any(|p| p.value > 0.6));
        assert_eq!(track.controllers[&1].last().unwrap().value, 0.0);
        assert!(track.controllers[&7].iter().any(|p| p.value < 0.4));
        assert_eq!(track.controllers[&7].last().unwrap().value, 1.0);
        assert_eq!(project.midi_clip(clip).unwrap().1.notes.len(), 1);
    }

    #[test]
    fn tracks_sharing_a_midi_channel_use_the_same_bend_sensitivity() {
        // Channel 0 is reused by the sixteenth melodic track. RPN sensitivity belongs to
        // that channel, even when the two tracks never play at the same time.
        for (wide, narrow) in [(0, 15), (15, 0)] {
            let mut project = Project::new("Shared bend channel", 48_000.0);
            for index in 0..16 {
                let track = project.add_instrument_track(format!("Track {index}"), "synth");
                let clip = project
                    .add_midi_clip(
                        track,
                        "Note",
                        Ticks::QUARTER * (index as i64 * 2),
                        Ticks::QUARTER,
                    )
                    .unwrap();
                let clip = project.midi_clip_mut(clip).unwrap();
                clip.notes = vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)];
                if index == wide {
                    clip.transforms = vec![auris_core::NoteTransform::Pitch {
                        settings: auris_core::PitchPerformance {
                            scoop: 3.0,
                            ..auris_core::PitchPerformance::default()
                        },
                    }];
                } else if index == narrow {
                    clip.bend = vec![CurvePoint {
                        at: Ticks::ZERO,
                        value: 2.0,
                    }];
                }
            }
            let bytes = write_midi_bytes(&project).unwrap();
            let smf = Smf::parse(&bytes).unwrap();
            let bend = smf.tracks[narrow + 1]
                .iter()
                .find_map(|event| match event.kind {
                    TrackEventKind::Midi {
                        channel,
                        message: MidiMessage::PitchBend { bend },
                    } => {
                        assert_eq!(channel.as_int(), 0);
                        Some(bend.as_f32())
                    }
                    _ => None,
                })
                .unwrap();
            assert!(
                (bend * 12.0 - 2.0).abs() < 0.002,
                "a shared-channel +2 bend played at {} semitones",
                bend * 12.0
            );
            for index in [wide, narrow] {
                assert!(smf.tracks[index + 1].iter().any(|event| matches!(
                    event.kind,
                    TrackEventKind::Midi {
                        channel,
                        message: MidiMessage::Controller { controller, value },
                    } if channel.as_int() == 0 && controller.as_int() == 6 && value.as_int() == 12
                )));
            }
            let imported = read_midi_bytes(&bytes).unwrap();
            assert!((imported.tracks[narrow].bend[0].value - 2.0).abs() < 0.002);
            assert!((imported.tracks[wide].bend[0].value + 3.0).abs() < 0.002);
        }
    }

    #[test]
    fn bend_sensitivity_respects_null_and_nrpn_selections() {
        let mut state = BendState::default();
        assert_eq!(state.range(), 2.0);
        state.receive(101, 0);
        state.receive(100, 0);
        state.receive(6, 12);
        state.receive(38, 50);
        assert_eq!(state.range(), 12.5);
        state.receive(101, 127);
        state.receive(100, 127);
        state.receive(6, 1);
        assert_eq!(state.range(), 12.5);
        state.receive(101, 0);
        state.receive(100, 0);
        state.receive(99, 0);
        state.receive(6, 1);
        assert_eq!(state.range(), 12.5);
    }

    #[test]
    fn the_wheel_goes_out_as_controller_one_and_comes_back() {
        // Seven bits rather than fourteen, so the tolerance is a hundred and twenty-eighth of the
        // travel — and that *is* the resolution the wire has, so a receiver reading this file
        // hears exactly what a receiver reading any other one would.
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4))],
            Ticks(TICKS_PER_QUARTER * 4),
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        project
            .midi_clip_mut(clip)
            .expect("the clip")
            .controllers
            .insert(
                CC_MODULATION,
                vec![
                    CurvePoint {
                        at: Ticks::ZERO,
                        value: 0.0,
                    },
                    CurvePoint {
                        at: Ticks(TICKS_PER_QUARTER * 2),
                        value: 1.0,
                    },
                ],
            );

        let imported = round_trip(&project);
        let wheel = imported.tracks[0]
            .controllers
            .get(&CC_MODULATION)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert!(!wheel.is_empty(), "the wheel did not survive the file");
        for pair in wheel.windows(2) {
            assert!(pair[0].at <= pair[1].at, "out of order: {wheel:?}");
        }
        let at = |tick: i64| {
            wheel
                .iter()
                .rfind(|point| point.at <= Ticks(tick))
                .map(|point| point.value)
                .unwrap_or(0.0)
        };
        assert!(at(0).abs() < 0.01, "it started up: {}", at(0));
        assert!(
            (at(TICKS_PER_QUARTER) - 0.5).abs() < 0.01,
            "halfway up is {}",
            at(TICKS_PER_QUARTER)
        );
        assert!(
            (at(TICKS_PER_QUARTER * 2) - 1.0).abs() < 0.01,
            "the top is {}",
            at(TICKS_PER_QUARTER * 2)
        );
        // Let go before the clip ends, for the reason the bend is: a wheel left up is channel
        // state, and everything after it would go on wobbling.
        assert!(
            at(TICKS_PER_QUARTER * 4).abs() < 0.01,
            "the wheel was left up at {}",
            at(TICKS_PER_QUARTER * 4)
        );
        // And a bend written beside it comes back as a bend rather than as a wheel: two curves on
        // one channel, told apart by the message they are carried in.
        assert!(imported.tracks[0].bend.is_empty());
    }

    #[test]
    fn any_controller_goes_out_and_comes_back_under_its_own_number() {
        // The wheel is not a special case: an expression pedal is the same three bytes with a
        // different number in the middle, and a part shaped by one has to survive the file it is
        // handed to somebody in.
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4))],
            Ticks(TICKS_PER_QUARTER * 4),
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        let written = vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 1.0,
            },
            CurvePoint {
                at: Ticks(TICKS_PER_QUARTER * 2),
                value: 0.25,
            },
        ];
        let midi = project.midi_clip_mut(clip).expect("the clip");
        midi.controllers.insert(EXPRESSION, written);
        midi.controllers.insert(SUSTAIN, {
            vec![CurvePoint {
                at: Ticks::QUARTER,
                value: 1.0,
            }]
        });

        let imported = round_trip(&project);
        let lanes = &imported.tracks[0].controllers;
        assert!(
            lanes.contains_key(&EXPRESSION) && lanes.contains_key(&SUSTAIN),
            "the lanes came back as {:?}",
            lanes.keys().collect::<Vec<_>>()
        );
        assert!(
            !lanes.contains_key(&CC_MODULATION),
            "a controller nobody wrote turned up"
        );
        let expression = &lanes[&EXPRESSION];
        let at = |tick: i64| {
            expression
                .iter()
                .rfind(|point| point.at <= Ticks(tick))
                .map(|point| point.value)
                .unwrap_or(0.0)
        };
        assert!((at(0) - 1.0).abs() < 0.01, "it started at {}", at(0));
        assert!(
            (at(TICKS_PER_QUARTER * 2) - 0.25).abs() < 0.01,
            "the pedal ended at {}",
            at(TICKS_PER_QUARTER * 2)
        );
    }

    #[test]
    fn the_files_own_plumbing_does_not_come_back_as_a_lane() {
        // A General MIDI file addresses its instruments with bank selects and RPN handshakes.
        // Those are how a file says which sound to play, not something anybody drew, and a lane
        // per one of them would put a staircase on screen for nearly every file there is.
        assert!(is_performance_controller(CC_MODULATION));
        assert!(is_performance_controller(EXPRESSION));
        assert!(is_performance_controller(SUSTAIN));
        for plumbing in [0, 32, 6, 38, 98, 99, 100, 101, 120, 123, 127] {
            assert!(
                !is_performance_controller(plumbing),
                "controller {plumbing} would be drawn"
            );
        }
    }

    /// Controller 11, the expression pedal.
    const EXPRESSION: u8 = 11;

    /// Controller 64, the sustain pedal.
    const SUSTAIN: u8 = 64;

    #[test]
    fn what_goes_out_comes_back_as_the_same_notes() {
        // Written at our own division, so this is exact rather than approximate: nothing is
        // scaled in either direction.
        let notes = vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            Note::new(64, Ticks::QUARTER, Ticks::QUARTER),
            Note::new(
                67,
                Ticks(TICKS_PER_QUARTER * 2),
                Ticks(TICKS_PER_QUARTER * 2),
            ),
        ];
        let imported = round_trip(&project_with(notes.clone(), Ticks(TICKS_PER_QUARTER * 4)));
        assert_eq!(imported.tracks.len(), 1);
        let back = &imported.tracks[0].notes;
        assert_eq!(back.len(), notes.len());
        for (was, now) in notes.iter().zip(back) {
            assert_eq!(
                (now.pitch, now.start, now.length),
                (was.pitch, was.start, was.length)
            );
        }
    }

    #[test]
    fn the_tempo_and_the_meter_come_back() {
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks(TICKS_PER_QUARTER * 8),
        );
        project.tempo_map.set_point(Ticks::ZERO, 96.0);
        project
            .tempo_map
            .set_point(Ticks(TICKS_PER_QUARTER * 4), 144.0);
        project.signatures = SignatureMap::constant(TimeSignature::new(3, 4));

        let imported = round_trip(&project);
        // 96 bpm is 625 000 microseconds exactly and comes back exact; 144 does not divide, and
        // is why the comparison below has a tolerance at all. See the test after this one.
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 96.0);
        assert!((imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)) - 144.0).abs() < 0.001);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::new(3, 4)
        );
    }

    #[test]
    fn a_tempo_survives_the_trip_to_within_what_the_format_can_say() {
        // A MIDI file stores tempo as whole microseconds per quarter note, so a tempo whose
        // period is not a whole number of them cannot come back exactly — 144 bpm is 416 666.67,
        // written as 416 667, read back as 143.999 88.
        //
        // Pinned rather than papered over: the error is a thousandth of a beat per minute, which
        // is inaudible over any length of piece, and knowing that is better than a round number
        // that quietly is not one.
        for bpm in [60.0, 96.0, 120.0, 128.0, 140.0, 144.0, 174.0, 200.0] {
            let mut project = project_with(
                vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
                Ticks::QUARTER,
            );
            project.tempo_map.set_point(Ticks::ZERO, bpm);
            let back = round_trip(&project).tempo_map.bpm_at(Ticks::ZERO);
            assert!((back - bpm).abs() < 0.001, "{bpm} bpm came back as {back}");
        }
    }

    #[test]
    fn a_track_keeps_its_name_through_the_trip() {
        let imported = round_trip(&project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        ));
        assert_eq!(imported.tracks[0].name, "Lead");
    }

    #[test]
    fn only_the_notes_a_clip_plays_are_written() {
        // A clip is a window onto its notes, and the exporter asks the same question the renderer
        // does — otherwise the file would be a piece nobody can hear by pressing play.
        let notes = vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            // Runs past the clip's end, so it is cut off there.
            Note::new(62, Ticks::QUARTER, Ticks(TICKS_PER_QUARTER * 4)),
            // Starts past the end, so it never sounds at all.
            Note::new(64, Ticks(TICKS_PER_QUARTER * 3), Ticks::QUARTER),
        ];
        let imported = round_trip(&project_with(notes, Ticks(TICKS_PER_QUARTER * 2)));
        let back = &imported.tracks[0].notes;
        assert_eq!(back.len(), 2, "the one past the end is not written");
        assert_eq!(back[1].pitch, 62);
        assert_eq!(
            back[1].end(),
            Ticks(TICKS_PER_QUARTER * 2),
            "cut at the clip's end"
        );
    }

    #[test]
    fn a_muted_clip_is_not_written() {
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        project.midi_clip_mut(clip).expect("the clip").muted = true;
        assert_eq!(round_trip(&project).note_count(), 0);
    }

    #[test]
    fn a_note_played_as_softly_as_possible_does_not_release_itself() {
        // A note-on at zero velocity *is* a note-off, so the floor is 1 rather than 0 — otherwise
        // the quietest note in a piece would vanish on the way out.
        let mut notes = vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)];
        notes[0].velocity = 0.0;
        let imported = round_trip(&project_with(notes, Ticks::QUARTER));
        assert_eq!(imported.note_count(), 1);
        assert!(imported.tracks[0].notes[0].velocity > 0.0);
    }

    #[test]
    fn a_note_struck_again_the_instant_the_last_one_ended_keeps_both() {
        // At one position the releases have to be written first, or the release of the first note
        // lands on the second and cuts it to nothing.
        let notes = vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            Note::new(60, Ticks::QUARTER, Ticks::QUARTER),
        ];
        let imported = round_trip(&project_with(notes, Ticks(TICKS_PER_QUARTER * 2)));
        assert_eq!(imported.note_count(), 2);
        assert!(
            imported.tracks[0]
                .notes
                .iter()
                .all(|note| note.length == Ticks::QUARTER),
            "neither note was cut short by the other's release"
        );
    }

    #[test]
    fn an_audio_track_has_no_notes_to_write_and_makes_no_track() {
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        );
        project.add_audio_track("Vocals");
        assert_eq!(round_trip(&project).tracks.len(), 1);
    }
}
