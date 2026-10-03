//! Development-only piano pedal experiment: a bounded JSON event score to float32 PCM.
//!
//! The sympathetic bank is a feed-forward approximation, not a solved bridge coupling.
//! No prototype or SMD recording is linked into the application.

use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
};
use auris_synth::{Model, Physical};
use serde::Deserialize;
use std::io::{self, BufRead, Write};

#[derive(Clone, Copy, Default)]
struct Mode {
    real: f32,
    imag: f32,
    cos: f32,
    sin: f32,
}

struct Resonance {
    modes: [Mode; 88],
    radius: f32,
    target: f32,
    step: f32,
    rate: f32,
    pedal: f32,
}

impl Resonance {
    fn new(rate: f32) -> Self {
        let mut bank = Self {
            modes: [Mode::default(); 88],
            radius: 0.0,
            target: 0.0,
            step: 1.0 - (-1.0 / (rate * 0.015)).exp(),
            rate,
            pedal: 0.0,
        };
        for (index, mode) in bank.modes.iter_mut().enumerate() {
            let frequency = 440.0 * ((index as f32 + 21.0 - 69.0) / 12.0).exp2();
            (mode.sin, mode.cos) = (std::f32::consts::TAU * frequency / rate).sin_cos();
        }
        bank.pedal(0.0);
        bank.radius = bank.target;
        bank
    }

    fn pedal(&mut self, amount: f32) {
        self.pedal = amount;
        self.target = (-6.907_755 / (self.rate * (0.03 + 2.0 * amount.powi(2)))).exp();
    }

    fn next(&mut self, input: f32) -> f32 {
        self.radius += self.step * (self.target - self.radius);
        let mut output = 0.0;
        for mode in &mut self.modes {
            let real = self.radius * (mode.cos * mode.real - mode.sin * mode.imag);
            mode.imag = self.radius * (mode.sin * mode.real + mode.cos * mode.imag);
            mode.real = real + input * 0.001;
            output += mode.imag;
        }
        output / 88.0
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    seconds: f32,
    kind: String,
    #[serde(default)]
    pitch: u8,
    #[serde(default)]
    value: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    events: Vec<Event>,
    seconds: f32,
    #[serde(default)]
    resonance: f32,
    #[serde(default)]
    half_pedal: bool,
}

fn render(request: &Request, block: usize) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    const RATE: f32 = 48_000.0;
    if !request.seconds.is_finite()
        || !(0.1..=20.0).contains(&request.seconds)
        || !request.resonance.is_finite()
        || !(0.0..=16.0).contains(&request.resonance)
        || request.events.len() > 10_000
        || request
            .events
            .windows(2)
            .any(|p| p[0].seconds > p[1].seconds)
        || request.events.iter().any(|e| {
            !e.seconds.is_finite()
                || !(0.0..request.seconds).contains(&e.seconds)
                || e.pitch > 127
                || !e.value.is_finite()
                || !(0.0..=1.0).contains(&e.value)
                || !matches!(e.kind.as_str(), "on" | "off" | "pedal")
        })
    {
        return Err("invalid pedal score".into());
    }
    let mut instrument = Physical::new(Model::Piano);
    instrument.prepare(&PrepareContext::new(f64::from(RATE), block, 2));
    let mut bank = Resonance::new(RATE);
    let frames = (request.seconds * RATE).round() as usize;
    let mut result = Vec::with_capacity(frames);
    let mut buffer = AudioBuffer::stereo(1, f64::from(RATE));
    let mut cursor = 0;
    let mut pedal = 0.0_f32;
    for start in (0..frames).step_by(block) {
        // Split at every event, so external release changes and modal damping are sample exact.
        for offset in 0..block.min(frames - start) {
            let frame = start + offset;
            let mut events = Vec::new();
            while let Some(event) = request.events.get(cursor) {
                if (event.seconds * RATE).round() as usize > frame {
                    break;
                }
                match event.kind.as_str() {
                    "on" => events.push(NoteEvent::NoteOn {
                        frame: 0,
                        pitch: event.pitch,
                        velocity: event.value,
                    }),
                    "off" => events.push(NoteEvent::NoteOff {
                        frame: 0,
                        pitch: event.pitch,
                    }),
                    "pedal" => {
                        pedal = event.value;
                        bank.pedal(pedal);
                        events.push(NoteEvent::Controller {
                            frame: 0,
                            number: 64,
                            value: pedal,
                        });
                    }
                    _ => unreachable!(),
                }
                cursor += 1;
            }
            if request.half_pedal {
                instrument.set_param_by_key("release", 0.03 + 0.7 * pedal.powi(2));
            }
            instrument.process(
                &events,
                &mut buffer,
                &ProcessContext::realtime(f64::from(RATE), 1, frame as u64, 120.0, true),
            );
            let dry = buffer.channel(0)[0];
            result.push(dry + request.resonance * bank.next(dry));
        }
    }
    Ok(result)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let request = serde_json::from_str(&line?)?;
        let samples = render(&request, 256)?;
        output.write_all(&u32::try_from(samples.len())?.to_le_bytes())?;
        for sample in samples {
            output.write_all(&sample.to_le_bytes())?;
        }
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifting_the_pedal_damps_existing_sympathetic_energy() {
        let mut bank = Resonance::new(48_000.0);
        bank.pedal(1.0);
        for i in 0..24_000 {
            bank.next((std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin());
        }
        let energy = |bank: &Resonance| {
            bank.modes
                .iter()
                .map(|m| m.real * m.real + m.imag * m.imag)
                .sum::<f32>()
        };
        let before = energy(&bank);
        assert!(before > 0.0001);
        bank.pedal(0.0);
        for _ in 0..24_000 {
            bank.next(0.0);
        }
        assert!(energy(&bank) < before * 1e-6);
    }
}
