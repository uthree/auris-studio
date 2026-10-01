//! Offline copy-synthesis worker: JSON lines in, length-prefixed float32 mono PCM out.
//!
//! Every request uses the actual instrument and resets between notes. This example is
//! development tooling; its I/O and allocations never run on the audio callback thread.

use std::{
    collections::BTreeMap,
    error::Error,
    io::{self, BufRead, Write},
};

use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
};
use auris_synth::{Model, Physical};
use serde::Deserialize;

const RATE: u32 = 24_000;
const BLOCK: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Note {
    pitch: u8,
    velocity: f32,
    #[serde(default)]
    tuning_cents: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    model: String,
    notes: Vec<Note>,
    seconds: f32,
    hold: f32,
    params: BTreeMap<String, f32>,
    #[serde(default = "default_rate")]
    sample_rate: u32,
}

fn default_rate() -> u32 {
    RATE
}

fn render(request: &Request) -> Result<Vec<f32>, Box<dyn Error>> {
    let model = match request.model.as_str() {
        "piano" => Model::Piano,
        "guitar" => Model::Guitar,
        "violin" => Model::Violin,
        _ => return Err("unknown model".into()),
    };
    if !request.seconds.is_finite()
        || !(0.05..=10.0).contains(&request.seconds)
        || !request.hold.is_finite()
        || !(0.0..=request.seconds).contains(&request.hold)
        || request.notes.is_empty()
        || request.notes.len() > 128
        || !(8_000..=192_000).contains(&request.sample_rate)
    {
        return Err("invalid note duration or count".into());
    }
    let rate = request.sample_rate;
    let frames = (request.seconds * rate as f32).round() as usize;
    let off = (request.hold * rate as f32).round() as usize;
    let mut instrument = Physical::new(model);
    instrument.prepare(&PrepareContext::new(f64::from(rate), BLOCK, 2));
    for (key, value) in &request.params {
        let Some(descriptor) = instrument.parameters().iter().find(|p| p.key == *key) else {
            return Err(format!("unknown parameter: {key}").into());
        };
        if !value.is_finite() || *value < descriptor.min || *value > descriptor.max {
            return Err(format!("out of range parameter: {key}").into());
        }
        instrument.set_param_by_key(key, *value);
    }
    let mut samples = Vec::with_capacity(frames * request.notes.len());
    let mut buffer = AudioBuffer::stereo(BLOCK, f64::from(rate));
    for note in &request.notes {
        if note.pitch > 127
            || !note.velocity.is_finite()
            || !(0.0..=1.0).contains(&note.velocity)
            || !note.tuning_cents.is_finite()
            || note.tuning_cents.abs() > 75.0
        {
            return Err("invalid pitch or velocity".into());
        }
        instrument.reset();
        for start in (0..frames).step_by(BLOCK) {
            let mut events = Vec::with_capacity(2);
            if start == 0 {
                events.push(NoteEvent::PitchBend {
                    frame: 0,
                    semitones: note.tuning_cents / 100.0,
                });
                events.push(NoteEvent::NoteOn {
                    frame: 0,
                    pitch: note.pitch,
                    velocity: note.velocity,
                });
            }
            if (start..start + BLOCK).contains(&off) {
                events.push(NoteEvent::NoteOff {
                    frame: (off - start) as u32,
                    pitch: note.pitch,
                });
            }
            let context =
                ProcessContext::realtime(f64::from(rate), BLOCK, start as u64, 120.0, true);
            instrument.process(&events, &mut buffer, &context);
            samples.extend_from_slice(&buffer.channel(0)[..BLOCK.min(frames - start)]);
        }
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err("non-finite renderer output".into());
    }
    Ok(samples)
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("--describe") {
        let models: BTreeMap<_, _> = [
            ("piano", Model::Piano),
            ("guitar", Model::Guitar),
            ("violin", Model::Violin),
        ]
        .into_iter()
        .map(|(name, model)| {
            let instrument = Physical::new(model);
            let parameters: BTreeMap<_, _> = instrument
                .parameters()
                .iter()
                .map(|p| (p.key.clone(), instrument.param(p.id)))
                .collect();
            (name, parameters)
        })
        .collect();
        println!("{}", serde_json::to_string(&models)?);
        return Ok(());
    }
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let request: Request = serde_json::from_str(&line?)?;
        let samples = render(&request)?;
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
    fn worker_resets_between_notes_and_preserves_exact_length() {
        let request = Request {
            model: "piano".into(),
            notes: vec![
                Note {
                    pitch: 60,
                    velocity: 0.7,
                    tuning_cents: 0.0,
                },
                Note {
                    pitch: 60,
                    velocity: 0.7,
                    tuning_cents: 0.0,
                },
            ],
            seconds: 0.101,
            hold: 0.05,
            params: BTreeMap::new(),
            sample_rate: RATE,
        };
        let samples = render(&request).unwrap();
        assert_eq!(samples.len(), 4848);
        assert_eq!(&samples[..2424], &samples[2424..]);
        assert!(samples.iter().any(|sample| sample.abs() > 0.01));
    }

    #[test]
    fn worker_rejects_unknown_or_non_finite_parameters() {
        let mut request = Request {
            model: "guitar".into(),
            notes: vec![Note {
                pitch: 48,
                velocity: 0.6,
                tuning_cents: 0.0,
            }],
            seconds: 0.1,
            hold: 0.1,
            params: BTreeMap::from([("missing".into(), 0.5)]),
            sample_rate: RATE,
        };
        assert!(
            render(&request)
                .unwrap_err()
                .to_string()
                .contains("unknown parameter")
        );
        request.params = BTreeMap::from([("damping".into(), f32::NAN)]);
        assert!(
            render(&request)
                .unwrap_err()
                .to_string()
                .contains("out of range")
        );
    }

    #[test]
    fn worker_supports_production_sample_rates_without_changing_note_duration() {
        let mut request = Request {
            model: "violin".into(),
            notes: vec![Note {
                pitch: 67,
                velocity: 0.65,
                tuning_cents: 20.0,
            }],
            seconds: 0.125,
            hold: 0.125,
            params: BTreeMap::new(),
            sample_rate: 48_000,
        };
        assert_eq!(render(&request).unwrap().len(), 6000);
        request.sample_rate = 1;
        assert!(render(&request).is_err());
    }
}
