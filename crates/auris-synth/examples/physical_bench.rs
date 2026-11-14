//! How much of a processor each physical instrument takes.
//!
//! ```sh
//! cargo run --release -p auris-synth --example physical_bench
//! cargo run --release -p auris-synth --example physical_bench -- --quality 2 --rate 96000
//! ```
//!
//! For every model it holds one note, then a handful, then as many as the model has voices,
//! renders ten seconds in host-sized blocks and prints how long that took as a share of the
//! ten seconds: the part of one core the instrument would use live. `--quality` picks the
//! live quality (0 draft, 1 standard, 2 high), `--rate` the sample rate and `--block` the
//! host's block size.
//!
//! Development tooling: build it with `--release`, or the numbers mean nothing.

use std::time::Instant;

use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
};
use auris_synth::{Model, Physical};

const SECONDS: f64 = 10.0;

/// Notes spread over the middle of each model's range.
fn chord(model: Model, count: usize) -> Vec<u8> {
    let (low, high): (u8, u8) = match model {
        Model::Piano => (33, 96),
        Model::HammeredDulcimer => (50, 86),
        Model::Guitar | Model::ElectricGuitar => (40, 84),
        Model::Bass => (28, 55),
        Model::Violin => (36, 93),
        Model::Mallet => (45, 93),
        Model::Bell => (55, 88),
        Model::Clarinet => (50, 86),
        Model::TinWhistle => (72, 96),
    };
    (0..count)
        .map(|n| {
            let span = f32::from(high - low);
            (f32::from(low) + span * (n as f32 + 0.5) / count as f32).round() as u8
        })
        .collect()
}

fn measure(model: Model, voices: usize, quality: f32, rate: f64, block: usize) -> (f64, usize) {
    let mut instrument = Physical::new(model);
    instrument.set_param_by_key("quality", quality);
    instrument.prepare(&PrepareContext::new(rate, block, 2));
    let mut buffer = AudioBuffer::stereo(block, rate);
    let frames = (SECONDS * rate) as usize;
    // Struck and plucked notes die away, so they are struck again every second.
    let again = rate as usize;
    let notes = chord(model, voices);
    let mut sounding = 0;
    let started = Instant::now();
    for start in (0..frames).step_by(block) {
        let mut events = Vec::new();
        if start % again < block {
            for (index, pitch) in notes.iter().enumerate() {
                if start > 0 && !model.is_sustained() {
                    events.push(NoteEvent::NoteOff {
                        frame: 0,
                        key: *pitch,
                    });
                }
                if start == 0 || !model.is_sustained() {
                    events.push(NoteEvent::note_on(
                        0,
                        *pitch,
                        0.6 + 0.3 * (index % 3) as f32 / 2.0,
                    ));
                }
            }
        }
        let context = ProcessContext::realtime(rate, block, start as u64, 120.0, true);
        instrument.process(&events, &mut buffer, &context);
        sounding = sounding.max(instrument.active_voices());
    }
    (started.elapsed().as_secs_f64() / SECONDS, sounding)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let option = |name: &str, default: f64| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let quality = option("--quality", 1.0) as f32;
    let rate = option("--rate", 48_000.0);
    let block = option("--block", 256.0) as usize;
    println!("quality {quality}, {rate} Hz, blocks of {block}: share of one core");
    println!(
        "{:<20} {:>10} {:>10} {:>14}",
        "model", "1 voice", "6 voices", "every voice"
    );
    for model in Model::ALL {
        let most = model.voices();
        let (one, _) = measure(model, 1, quality, rate, block);
        let (six, _) = measure(model, 6.min(most), quality, rate, block);
        let (all, sounding) = measure(model, most, quality, rate, block);
        println!(
            "{:<20} {:>9.2}% {:>9.2}% {:>9.2}% ({sounding})",
            model.name(),
            one * 100.0,
            six * 100.0,
            all * 100.0,
        );
    }
}
