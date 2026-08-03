//! A SoundFont assembled in memory, so the playback path can be tested at all.
//!
//! There is no fixture file. A real `.sf2` is somebody's copyrighted two hundred megabytes, and a
//! stripped-down one checked into the repository would still be a binary blob nobody could read
//! or amend. So the bytes are written here instead: a RIFF container holding one sine tone per
//! preset, which is the smallest thing the format will accept and the largest thing that can be
//! understood by reading it.
//!
//! What this buys is the only test that can fail for a real reason — whether a note event turns
//! into audio. Everything else about the sampler is arithmetic on state; this is the part that
//! goes through the file format, the preset lookup and the synthesiser.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f32::consts::TAU;
use std::io::Cursor;
use std::sync::Arc;

use rustysynth::SoundFont;

thread_local! {
    /// Whether this thread is currently inside [`count_allocations`].
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    /// Allocations this thread has made since the count started.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Counts heap allocations made by `body` **on the calling thread**.
///
/// The same device `auris-engine` uses, and here for a sharper reason: this crate's `process`
/// hands the work to somebody else's synthesiser. That it allocates nothing was established by
/// reading their source, which is exactly the kind of fact that stops being true in a version
/// bump without anyone noticing.
///
/// Per-thread rather than global because `cargo test` runs cases in parallel.
pub(crate) fn count_allocations(body: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|slot| slot.set(0));
    WATCHING.with(|slot| slot.set(true));
    body();
    WATCHING.with(|slot| slot.set(false));
    ALLOCATIONS.with(Cell::get)
}

fn note_allocation() {
    // `try_with` because the allocator can run while thread-locals are being torn down.
    if WATCHING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|slot| slot.set(slot.get() + 1));
    }
}

/// A pass-through allocator that reports to [`count_allocations`].
struct CountingAllocator;

// SAFETY: every method forwards to `System` unchanged; the counter only reads and writes
// thread-local `Cell`s, which allocate nothing (both are `const`-initialised and have no
// destructor, so no lazy allocation or TLS destructor registration happens).
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Generator that names which instrument a preset zone plays.
const GENERATOR_INSTRUMENT: u16 = 41;
/// Generator that names which sample an instrument zone plays.
const GENERATOR_SAMPLE_ID: u16 = 53;

/// Frames of sample data per voice.
///
/// Long enough that a note goes on sounding through several blocks: the sample does not loop, so
/// when it runs out the voice stops however long the key is held.
pub(crate) const SAMPLE_FRAMES: usize = 32_768;

/// Silence after each sample, as the format expects between one sample and the next.
const SAMPLE_PADDING: usize = 46;

/// MIDI key the samples are recorded at, so playing it back reproduces them unresampled.
pub(crate) const ROOT_KEY: u8 = 69;

/// Pitch of the recorded tone, at [`ROOT_KEY`].
const TONE_HZ: f32 = 440.0;

/// One sound in the font under construction.
pub(crate) struct Voice {
    /// What the preset is called.
    pub(crate) name: &'static str,
    /// MIDI bank.
    pub(crate) bank: u16,
    /// MIDI program.
    pub(crate) patch: u16,
    /// Peak of the recorded tone, which is what a test can measure back.
    pub(crate) amplitude: f32,
}

/// A font holding one sine tone per voice, ready to hand to a synthesiser.
pub(crate) fn soundfont(voices: &[Voice], sample_rate: i32) -> Arc<SoundFont> {
    let bytes = soundfont_bytes(voices, sample_rate);
    let font = SoundFont::new(&mut Cursor::new(bytes)).expect("the font this module writes");
    Arc::new(font)
}

/// The default fixture: two tones an even four to one apart, at two different patches.
///
/// The ratio is the point. A test that only asked "is there sound" would pass just as happily
/// when every preset played the same thing, which is exactly the bug worth catching.
pub(crate) fn two_tone_font(sample_rate: i32) -> Arc<SoundFont> {
    soundfont(
        &[
            Voice {
                name: "Loud Tone",
                bank: 0,
                patch: 0,
                amplitude: 0.5,
            },
            Voice {
                name: "Quiet Tone",
                bank: 0,
                patch: 42,
                amplitude: 0.125,
            },
        ],
        sample_rate,
    )
}

/// Writes the file.
fn soundfont_bytes(voices: &[Voice], sample_rate: i32) -> Vec<u8> {
    assert!(!voices.is_empty(), "a font needs at least one sound");

    let mut info = Vec::new();
    info.extend(chunk(b"ifil", {
        let mut version = Vec::new();
        push_u16(&mut version, 2);
        push_u16(&mut version, 1);
        version
    }));
    info.extend(chunk(b"isng", padded(b"EMU8000")));
    info.extend(chunk(b"INAM", padded(b"Auris Test Font")));

    let sdta = chunk(b"smpl", sample_data(voices, sample_rate));

    let mut pdta = Vec::new();
    pdta.extend(chunk(b"phdr", preset_headers(voices)));
    pdta.extend(chunk(b"pbag", zones(voices.len())));
    pdta.extend(chunk(b"pmod", modulator_terminator()));
    pdta.extend(chunk(
        b"pgen",
        generators(GENERATOR_INSTRUMENT, voices.len()),
    ));
    pdta.extend(chunk(b"inst", instrument_headers(voices)));
    pdta.extend(chunk(b"ibag", zones(voices.len())));
    pdta.extend(chunk(b"imod", modulator_terminator()));
    pdta.extend(chunk(
        b"igen",
        generators(GENERATOR_SAMPLE_ID, voices.len()),
    ));
    pdta.extend(chunk(b"shdr", sample_headers(voices, sample_rate)));

    let mut body = Vec::new();
    body.extend(list(b"INFO", info));
    body.extend(list(b"sdta", sdta));
    body.extend(list(b"pdta", pdta));

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    push_u32(&mut file, (body.len() + 4) as u32);
    file.extend_from_slice(b"sfbk");
    file.extend(body);
    file
}

/// Every voice's tone, one after another with silence between them.
fn sample_data(voices: &[Voice], sample_rate: i32) -> Vec<u8> {
    let mut data = Vec::new();
    for voice in voices {
        for frame in 0..SAMPLE_FRAMES {
            let phase = TAU * TONE_HZ * frame as f32 / sample_rate as f32;
            let value = (voice.amplitude * phase.sin() * i16::MAX as f32) as i16;
            push_u16(&mut data, value as u16);
        }
        for _ in 0..SAMPLE_PADDING {
            push_u16(&mut data, 0);
        }
    }
    data
}

/// Where in the sample data one voice's tone begins.
fn voice_start(index: usize) -> i32 {
    (index * (SAMPLE_FRAMES + SAMPLE_PADDING)) as i32
}

fn preset_headers(voices: &[Voice]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, voice) in voices.iter().enumerate() {
        push_name(&mut out, voice.name);
        push_u16(&mut out, voice.patch);
        push_u16(&mut out, voice.bank);
        push_u16(&mut out, index as u16);
        push_i32(&mut out, 0); // library
        push_i32(&mut out, 0); // genre
        push_i32(&mut out, 0); // morphology
    }
    // The terminal record, whose bag index is what closes the last real one's zone span.
    push_name(&mut out, "EOP");
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, voices.len() as u16);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0);
    out
}

fn instrument_headers(voices: &[Voice]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, voice) in voices.iter().enumerate() {
        push_name(&mut out, voice.name);
        push_u16(&mut out, index as u16);
    }
    push_name(&mut out, "EOI");
    push_u16(&mut out, voices.len() as u16);
    out
}

/// One zone per voice, plus the terminal record that gives the last one its length.
fn zones(count: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for index in 0..=count {
        push_u16(&mut out, index as u16); // generator index
        push_u16(&mut out, 0); // modulator index
    }
    out
}

/// One generator per zone, naming what that zone plays, plus the terminator.
fn generators(generator: u16, count: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for index in 0..count {
        push_u16(&mut out, generator);
        push_u16(&mut out, index as u16);
    }
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    out
}

/// No modulators, but the chunk still has to be there and still ends in a terminator.
fn modulator_terminator() -> Vec<u8> {
    vec![0; 10]
}

fn sample_headers(voices: &[Voice], sample_rate: i32) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, voice) in voices.iter().enumerate() {
        let start = voice_start(index);
        let end = start + SAMPLE_FRAMES as i32;
        push_name(&mut out, voice.name);
        push_i32(&mut out, start);
        push_i32(&mut out, end);
        push_i32(&mut out, start + 1); // loop start, unused: these samples do not loop
        push_i32(&mut out, end - 1); // loop end
        push_i32(&mut out, sample_rate);
        out.push(ROOT_KEY);
        out.push(0); // pitch correction, in cents
        push_u16(&mut out, 0); // link to the other half of a stereo pair
        push_u16(&mut out, 1); // mono
    }
    push_name(&mut out, "EOS");
    out.extend_from_slice(&[0; 26]);
    out
}

// ---------------------------------------------------------------- bytes

/// A chunk: four-character id, length, body. Bodies here are always an even length, which is
/// what lets the reader run straight from one chunk to the next without a pad byte.
fn chunk(id: &[u8; 4], body: Vec<u8>) -> Vec<u8> {
    assert!(
        body.len().is_multiple_of(2),
        "chunk bodies must be an even length"
    );
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(id);
    push_u32(&mut out, body.len() as u32);
    out.extend(body);
    out
}

/// A list: a chunk whose body starts with a four-character type.
fn list(kind: &[u8; 4], body: Vec<u8>) -> Vec<u8> {
    let mut inner = Vec::with_capacity(body.len() + 4);
    inner.extend_from_slice(kind);
    inner.extend(body);
    chunk(b"LIST", inner)
}

/// A string padded to an even length with the terminator the format expects.
fn padded(text: &[u8]) -> Vec<u8> {
    let mut out = text.to_vec();
    out.push(0);
    if !out.len().is_multiple_of(2) {
        out.push(0);
    }
    out
}

/// A twenty-byte name field, truncated or zero-filled to fit.
fn push_name(out: &mut Vec<u8>, name: &str) {
    let mut field = [0u8; 20];
    for (slot, byte) in field.iter_mut().zip(name.bytes()).take(19) {
        *slot = byte;
    }
    out.extend_from_slice(&field);
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_this_module_writes_is_one_the_reader_accepts() {
        // If this fails, every playback test below it is testing the builder rather than the
        // sampler, so it is worth saying separately.
        let font = two_tone_font(48_000);
        let presets = font.get_presets();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].get_name(), "Loud Tone");
        assert_eq!(presets[0].get_bank_number(), 0);
        assert_eq!(presets[0].get_patch_number(), 0);
        assert_eq!(presets[1].get_patch_number(), 42);
        assert_eq!(
            font.get_wave_data().len(),
            2 * (SAMPLE_FRAMES + SAMPLE_PADDING)
        );
    }
}
