//! Plays single notes on every physical instrument and prints what each one measured.
//!
//! ```sh
//! cargo run -p auris-synth --example physical_probe
//! cargo run -p auris-synth --example physical_probe -- --model piano --wav target/probe
//! ```
//!
//! One row per note: its peak and level, how far the sounding pitch is from the key, where the
//! spectrum's weight lies early and late, and how fast it falls. `--wav` also writes each
//! model's notes end to end as a stereo 32-bit float file, for listening.
//!
//! Development tooling: nothing here runs on an audio thread.

use std::{error::Error, fs::File, io::Write, path::PathBuf};

use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
};
use auris_synth::{Model, Physical};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

/// Renders `events` and returns the two channels.
fn render(instrument: &mut dyn Instrument, frames: usize, events: &[NoteEvent]) -> [Vec<f32>; 2] {
    let mut buffer = AudioBuffer::stereo(BLOCK, RATE);
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for start in (0..frames).step_by(BLOCK) {
        let block: Vec<NoteEvent> = events
            .iter()
            .filter(|event| (start..start + BLOCK).contains(&(event.frame() as usize)))
            .map(|event| event.with_frame(event.frame() - start as u32))
            .collect();
        let ctx = ProcessContext::realtime(RATE, BLOCK, start as u64, 120.0, true);
        instrument.process(&block, &mut buffer, &ctx);
        out[0].extend_from_slice(buffer.channel(0));
        out[1].extend_from_slice(buffer.channel(1));
    }
    out
}

fn goertzel(samples: &[f32], hz: f64) -> f64 {
    let coefficient = 2.0 * (std::f64::consts::TAU * hz / RATE).cos();
    let (mut previous, mut older) = (0.0_f64, 0.0_f64);
    for sample in samples {
        let current = f64::from(*sample) + coefficient * previous - older;
        older = previous;
        previous = current;
    }
    2.0 * (previous * previous + older * older - coefficient * previous * older)
        .max(0.0)
        .sqrt()
        / samples.len() as f64
}

fn db(value: f64) -> f64 {
    20.0 * value.max(1.0e-12).log10()
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len().max(1) as f64)
        .sqrt()
}

/// Cents from `hz` at which the spectrum of `samples` peaks, within a semitone.
fn cents_off(samples: &[f32], hz: f64) -> f64 {
    (-200..=200)
        .map(|half| f64::from(half) * 0.5)
        .max_by(|a, b| {
            let at = |cents: f64| goertzel(samples, hz * (cents / 1200.0).exp2());
            at(*a).total_cmp(&at(*b))
        })
        .unwrap_or(0.0)
}

/// The frequency below which half the power of the first forty harmonics of `hz` lies.
fn centroid(samples: &[f32], hz: f64) -> f64 {
    let mut weighted = 0.0;
    let mut total = 0.0;
    for harmonic in 1..=40 {
        let at = hz * f64::from(harmonic);
        if at > RATE * 0.45 {
            break;
        }
        // A stiff string's partials run sharp; look a little either side.
        let power = (-3..=6)
            .map(|step| goertzel(samples, at * (1.0 + f64::from(step) * 0.004)).powi(2))
            .fold(0.0, f64::max);
        weighted += power * at;
        total += power;
    }
    if total > 0.0 { weighted / total } else { 0.0 }
}

fn name(model: Model) -> &'static str {
    model.id().trim_start_matches("auris.physical.")
}

fn pitches(model: Model) -> &'static [u8] {
    match model {
        Model::Piano => &[24, 36, 48, 60, 72, 84, 96],
        Model::HammeredDulcimer => &[50, 62, 74, 86],
        Model::Guitar | Model::ElectricGuitar => &[40, 52, 64, 76],
        Model::Bass => &[28, 40, 52],
        Model::Violin => &[36, 48, 60, 72, 84],
        Model::Mallet => &[48, 60, 72, 84],
        Model::Bell => &[60, 72, 84],
        Model::Clarinet => &[50, 62, 74, 86],
        Model::TinWhistle => &[74, 86, 98],
    }
}

fn write_wav(path: &PathBuf, channels: &[Vec<f32>; 2]) -> Result<(), Box<dyn Error>> {
    let mut file = File::create(path)?;
    let frames = channels[0].len() as u32;
    let bytes = frames * 8;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&3_u16.to_le_bytes())?; // IEEE float
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&(RATE as u32).to_le_bytes())?;
    file.write_all(&(RATE as u32 * 8).to_le_bytes())?;
    file.write_all(&8_u16.to_le_bytes())?;
    file.write_all(&32_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for (left, right) in channels[0].iter().zip(&channels[1]) {
        file.write_all(&left.to_le_bytes())?;
        file.write_all(&right.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let option = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let only = option("--model");
    let wav = option("--wav").map(PathBuf::from);
    let partials = args.iter().any(|arg| arg == "--partials");
    let params: Vec<(String, f32)> = args
        .iter()
        .filter_map(|arg| arg.split_once('='))
        .filter_map(|(key, value)| Some((key.to_owned(), value.parse().ok()?)))
        .collect();
    if let Some(directory) = &wav {
        std::fs::create_dir_all(directory)?;
    }
    println!(
        "{:<18} {:>4} {:>4}  {:>7} {:>7}  {:>6}  {:>7} {:>7}  {:>6} {:>6}  {:>6}",
        "model",
        "key",
        "vel",
        "peak dB",
        "rms dB",
        "cents",
        "early",
        "late",
        "dB/s a",
        "dB/s b",
        "L-R dB"
    );
    for model in Model::ALL {
        if only.as_deref().is_some_and(|wanted| wanted != name(model)) {
            continue;
        }
        let mut all = [Vec::new(), Vec::new()];
        for pitch in pitches(model) {
            for velocity in [0.35_f32, 0.8, 1.0] {
                let mut instrument = Physical::new(model);
                for (key, value) in &params {
                    instrument.set_param_by_key(key, *value);
                }
                instrument.prepare(&PrepareContext::new(RATE, BLOCK, 2));
                let hold = if model.is_sustained() { 72_000 } else { 96_000 };
                let events = [
                    NoteEvent::note_on(0, *pitch, velocity),
                    NoteEvent::NoteOff {
                        frame: hold,
                        key: *pitch,
                    },
                ];
                let channels = render(&mut instrument, 144_000, &events);
                let mono: Vec<f32> = channels[0]
                    .iter()
                    .zip(&channels[1])
                    .map(|(l, r)| (l + r) * 0.5)
                    .collect();
                let hz = 440.0 * ((f64::from(*pitch) - 69.0) / 12.0).exp2();
                let peak = mono.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
                let early = &mono[2_400..14_400];
                let late = &mono[48_000..60_000];
                // Two stretches of decay: the tenth of a second to the half, and one to two.
                let level = |from: usize| db(rms(&mono[from..from + 4_800]));
                let first = (level(4_800) - level(24_000)) / 0.4;
                let second = (level(48_000) - level(91_200)) / 0.9;
                println!(
                    "{:<18} {:>4} {:>4.2}  {:>7.1} {:>7.1}  {:>+6.1}  {:>7.0} {:>7.0}  {:>6.1} {:>6.1}  {:>+6.1}",
                    name(model),
                    pitch,
                    velocity,
                    db(f64::from(peak)),
                    db(rms(&mono[..hold as usize])),
                    cents_off(&mono[9_600..48_000], hz),
                    centroid(early, hz),
                    centroid(late, hz),
                    first,
                    second,
                    db(rms(&channels[0])) - db(rms(&channels[1])),
                );
                if partials {
                    // Levels of the first sixteen partials over the first half second, in
                    // decibels under the strongest.
                    let window = &mono[1_200..25_200];
                    let levels: Vec<f64> = (1..=16)
                        .map(|harmonic| {
                            let at = hz * f64::from(harmonic);
                            (-4..=24)
                                .map(|step| goertzel(window, at * (1.0 + f64::from(step) * 0.002)))
                                .fold(0.0, f64::max)
                        })
                        .collect();
                    let top = levels.iter().copied().fold(0.0, f64::max);
                    let row: Vec<String> = levels
                        .iter()
                        .map(|level| format!("{:>4.0}", db(level / top.max(1.0e-12))))
                        .collect();
                    println!("    partials {}", row.join(""));
                }
                if velocity == 0.8 {
                    all[0].extend_from_slice(&channels[0]);
                    all[1].extend_from_slice(&channels[1]);
                }
            }
        }
        if let Some(directory) = &wav {
            let path = directory.join(format!("{}.wav", name(model)));
            write_wav(&path, &all)?;
        }
    }
    Ok(())
}
