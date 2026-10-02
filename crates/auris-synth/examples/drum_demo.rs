//! Writes twelve isolated physical-kit sounds followed by an eight-bar listening probe.

use std::{
    error::Error,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

use auris_core::{AudioBuffer, Instrument, NoteEvent, PrepareContext, ProcessContext};
use auris_synth::DrumKit;

const RATE: u32 = 48_000;
const BLOCK: u32 = 256;

fn render(kit: &mut DrumKit, frames: u32, events: &[NoteEvent], samples: &mut Vec<f32>) {
    let mut block = AudioBuffer::stereo(BLOCK as usize, f64::from(RATE));
    for start in (0..frames).step_by(BLOCK as usize) {
        let count = (frames - start).min(BLOCK);
        block.set_frame_count(count as usize);
        let events: Vec<_> = events
            .iter()
            .filter(|event| (start..start + count).contains(&event.frame()))
            .map(|event| event.with_frame(event.frame() - start))
            .collect();
        kit.process(
            &events,
            &mut block,
            &ProcessContext::realtime(
                f64::from(RATE),
                count as usize,
                u64::from(start),
                120.0,
                true,
            ),
        );
        samples.extend_from_slice(block.channel(0));
    }
}

fn on(frame: u32, pitch: u8, velocity: f32) -> NoteEvent {
    NoteEvent::NoteOn {
        frame,
        pitch,
        velocity,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| "drum-demo.wav".into());
    let mut kit = DrumKit::new();
    kit.prepare(&PrepareContext::new(f64::from(RATE), BLOCK as usize, 2));
    let mut samples = Vec::new();
    for key in [36, 38, 42, 46, 49, 51, 41, 43, 45, 47, 48, 50] {
        kit.reset();
        let start = samples.len();
        render(&mut kit, RATE * 2, &[on(0, key, 0.9)], &mut samples);
        let peak = samples[start..].iter().fold(0.0_f32, |a, s| a.max(s.abs()));
        println!("key {key}: peak {peak:.5}");
    }
    kit.reset();
    let mut groove = Vec::new();
    for beat in 0..32 {
        let start = beat * (RATE / 2);
        groove.push(on(start, if beat % 2 == 0 { 36 } else { 38 }, 0.85));
        groove.push(on(start, 42, 0.55));
        groove.push(on(
            start + RATE / 4,
            if beat % 8 == 7 { 46 } else { 42 },
            0.4,
        ));
        if beat % 8 == 0 {
            groove.push(on(start, 49, 0.65));
        }
        if beat >= 28 {
            groove.push(on(
                start + RATE / 8,
                [50, 48, 45, 41][(beat - 28) as usize],
                0.75,
            ));
        }
    }
    groove.sort_by_key(NoteEvent::frame);
    render(&mut kit, RATE * 18, &groove, &mut samples);
    // Worst-case callback cost: all 24 voices active, with and without motion sampling.
    for watched in [false, true] {
        kit.reset();
        kit.motion_monitor()
            .ok_or("missing drum motion monitor")?
            .watch(watched);
        let events: [NoteEvent; 24] = std::array::from_fn(|_| on(0, 49, 0.8));
        let mut out = AudioBuffer::stereo(BLOCK as usize, f64::from(RATE));
        let context = ProcessContext::realtime(f64::from(RATE), BLOCK as usize, 0, 120.0, true);
        let mut timings = [0.0_f64; 256];
        for batch in 0..32 {
            kit.reset();
            kit.process(&events, &mut out, &context);
            for block in 0..8 {
                let start = Instant::now();
                kit.process(&[], &mut out, &context);
                timings[batch * 8 + block] = start.elapsed().as_secs_f64();
            }
        }
        timings.sort_by(f64::total_cmp);
        println!(
            "24 crashes, watched={watched}: median {:.3}, p99 {:.3}, worst {:.3} ms / {:.3} ms budget",
            timings[128] * 1_000.0,
            timings[253] * 1_000.0,
            timings[255] * 1_000.0,
            f64::from(BLOCK) / f64::from(RATE) * 1_000.0
        );
    }
    let peak = samples.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
    // One common listening gain preserves the kit balance; no per-hit normalization or limiter.
    let gain = 0.9 / peak.max(0.9);
    let mut file = BufWriter::new(File::create(&path)?);
    let bytes = (samples.len() * 2) as u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * 2).to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for sample in samples {
        file.write_all(&((sample * gain * 32767.0) as i16).to_le_bytes())?;
    }
    file.flush()?;
    println!(
        "{} (common gain {gain:.3}, raw peak {peak:.3})",
        path.display()
    );
    Ok(())
}
