//! The phrase every built-in sound's default level is set against.
//!
//! A user who swaps one sound for another should not have to reach for the fader to hear the
//! new one at the same level, so the instruments' default `level`s (and the physical models'
//! normalisation) are trimmed against each other: each plays the same four bars and the
//! integrated loudness of what comes out is held to a narrow band around [`REFERENCE_LUFS`].
//! This crate's tests hold every instrument in the pack to it, and the session's tests hold the
//! factory patches built on them.
//!
//! The phrase is what a part *played on* the instrument would be, one line at a time: a melody
//! in the instrument's own register for the melodic ones, a kick–snare–hat beat for the kits.
//! Chords are left out on purpose; a chord is louder than a single note on every instrument,
//! which is the player's doing, not the instrument's.
//!
//! ```
//! use auris_core::Instrument;
//! use auris_synth::level_reference::{REFERENCE_LUFS, phrase, phrase_loudness};
//! use auris_synth::{Analog, AnalogPatch};
//!
//! let mut pad = Analog::new(AnalogPatch::Pad);
//! let id = pad.descriptor().id.to_string();
//! let lufs = phrase_loudness(&mut pad, &phrase(&id)).unwrap();
//! assert!((lufs - REFERENCE_LUFS).abs() < 1.0);
//! ```

use auris_core::{AudioBuffer, Instrument, NoteEvent, PrepareContext, ProcessContext};
use auris_dsp::loudness::integrated_lufs;

use crate::{authored_drum_map, is_drum_kit};

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK: usize = 256;
/// A quarter note at 120 BPM, in frames.
const BEAT: usize = 24_000;
const BARS: usize = 4;
/// What the phrase is played at: a firm *mezzo forte*, not the top of the range.
pub const VELOCITY: f32 = 0.8;

/// The integrated loudness every built-in sound's default patch reads on its [`phrase`].
///
/// EBU R128's programme level, for one part on its own: four or five parts at this level sum
/// to about -16 LUFS, which the master limiter and `normalize_mix` take the rest of the way, and
/// the most transient instrument (the acoustic kit) still peaks under -1 dBFS.
pub const REFERENCE_LUFS: f32 = -23.0;

/// Where an instrument's melody sits: the bass instruments two octaves down, the whistle up
/// where it speaks, everything else around middle C.
pub fn register(id: &str) -> u8 {
    match id {
        "auris.physical.bass"
        | "auris.synth.bass"
        | "auris.synth.boom_bass"
        | "auris.synth.kick_bass"
        | "auris.synth.sub_bass" => 36,
        "auris.physical.tin_whistle" => 74,
        _ => 60,
    }
}

/// Four bars of quarter notes from `root`, each held for most of its beat.
pub fn melody(root: u8) -> Vec<NoteEvent> {
    const DEGREES: [u8; 16] = [0, 2, 4, 5, 7, 5, 4, 2, 0, 4, 7, 12, 7, 4, 2, 0];
    events(
        DEGREES
            .iter()
            .enumerate()
            .map(|(beat, degree)| (beat * BEAT, BEAT * 7 / 8, root.saturating_add(*degree))),
    )
}

/// Four bars of eighth-note hats, kick on one and three, snare on two and four, on the keys
/// `id`'s authored drum map plays them from (General MIDI's where it has none).
pub fn beat(id: &str) -> Vec<NoteEvent> {
    use auris_core::DrumRole;
    let map = authored_drum_map(id);
    let key = |role: DrumRole, general: u8| {
        map.as_ref()
            .and_then(|map| map.voices.get(&role).copied())
            .unwrap_or(general)
    };
    let (kick, snare, hat) = (
        key(DrumRole::Kick, 36),
        key(DrumRole::Snare, 38),
        key(DrumRole::ClosedHat, 42),
    );
    let mut hits = Vec::new();
    for eighth in 0..BARS * 8 {
        let at = eighth * BEAT / 2;
        hits.push((at, BEAT / 4, hat));
        match eighth % 8 {
            0 | 4 => hits.push((at, BEAT / 4, kick)),
            2 | 6 => hits.push((at, BEAT / 4, snare)),
            _ => {}
        }
    }
    events(hits)
}

/// The phrase the instrument `id` is measured on: [`beat`] for a kit, otherwise [`melody`] in
/// its [`register`].
pub fn phrase(id: &str) -> Vec<NoteEvent> {
    if is_drum_kit(id) {
        beat(id)
    } else {
        melody(register(id))
    }
}

/// Note-ons and note-offs for `(start, length, key)` notes, in frame order.
fn events(notes: impl IntoIterator<Item = (usize, usize, u8)>) -> Vec<NoteEvent> {
    let mut events: Vec<NoteEvent> = notes
        .into_iter()
        .flat_map(|(at, length, key)| {
            [
                NoteEvent::note_on(at as u32, key, VELOCITY),
                NoteEvent::NoteOff {
                    frame: (at + length) as u32,
                    key,
                },
            ]
        })
        .collect();
    events.sort_by_key(NoteEvent::frame);
    events
}

/// Prepares `instrument` and renders `events` through it offline, in stereo at 48 kHz, with a
/// bar of tail after the four.
pub fn render(instrument: &mut dyn Instrument, events: &[NoteEvent]) -> AudioBuffer {
    let frames = (BARS + 1) * 4 * BEAT;
    instrument.prepare(&PrepareContext::new(SAMPLE_RATE, BLOCK, 2));
    let mut block = AudioBuffer::new(2, BLOCK, SAMPLE_RATE);
    let mut out = AudioBuffer::new(2, frames, SAMPLE_RATE);
    let mut done = 0usize;
    while done < frames {
        let this = BLOCK.min(frames - done);
        block.set_frame_count(this);
        let (start, end) = (done as u32, (done + this) as u32);
        let block_events: Vec<NoteEvent> = events
            .iter()
            .filter(|event| (start..end).contains(&event.frame()))
            .map(|event| event.with_frame(event.frame() - start))
            .collect();
        let mut ctx = ProcessContext::realtime(SAMPLE_RATE, this, done as u64, 120.0, true);
        ctx.is_offline = true;
        instrument.process(&block_events, &mut block, &ctx);
        for channel in 0..2 {
            out.channel_mut(channel)[done..done + this].copy_from_slice(block.channel(channel));
        }
        done += this;
    }
    out
}

/// The integrated loudness of `events` [`render`]ed through `instrument`, in LUFS; `None` for
/// silence.
pub fn phrase_loudness(instrument: &mut dyn Instrument, events: &[NoteEvent]) -> Option<f32> {
    integrated_lufs(&render(instrument, events))
}
