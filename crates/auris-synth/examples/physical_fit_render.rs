//! Offline copy-synthesis worker: JSON lines in, length-prefixed float32 mono PCM out.
//!
//! Every request uses the actual instrument and resets between notes. This example is
//! development tooling; its I/O and allocations never run on the audio callback thread.

use std::{
    collections::BTreeMap,
    error::Error,
    io::{self, BufRead, Write},
};

use auris_core::{AudioBuffer, Effect, Instrument, NoteEvent, PrepareContext, ProcessContext};
use auris_synth::{DrumKit, Model, Physical};
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
    #[serde(default)]
    hold: Option<f32>,
    #[serde(default)]
    bends: Vec<Point>,
    #[serde(default)]
    expression: Vec<Point>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    seconds: f32,
    value: f32,
}

fn automation(points: &[Point], hold: f32, range: std::ops::RangeInclusive<f32>) -> bool {
    points.len() <= 4000
        && points.iter().all(|point| {
            point.seconds.is_finite()
                && (0.0..hold).contains(&point.seconds)
                && point.value.is_finite()
                && range.contains(&point.value)
        })
        && points
            .windows(2)
            .all(|pair| pair[0].seconds < pair[1].seconds)
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
    #[serde(default)]
    effects: Vec<EffectRequest>,
    #[serde(default)]
    inputs: Vec<std::path::PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectRequest {
    id: String,
    params: BTreeMap<String, f32>,
}

fn default_rate() -> u32 {
    RATE
}

fn render(request: &Request) -> Result<Vec<f32>, Box<dyn Error>> {
    render_blocks(request, BLOCK)
}

fn render_blocks(request: &Request, block: usize) -> Result<Vec<f32>, Box<dyn Error>> {
    let mut instrument = instrument(&request.model)?;
    if !request.seconds.is_finite()
        || !(0.05..=20.0).contains(&request.seconds)
        || !request.hold.is_finite()
        || !(0.0..=request.seconds).contains(&request.hold)
        || request.notes.is_empty()
        || request.notes.len() > 128
        || !(8_000..=192_000).contains(&request.sample_rate)
        || request.effects.len() > 4
        || !request.inputs.is_empty() && request.inputs.len() != request.notes.len()
    {
        return Err("invalid note duration or count".into());
    }
    let rate = request.sample_rate;
    let frames = (request.seconds * rate as f32).round() as usize;
    if frames * request.notes.len() > 32_000_000 {
        return Err("PCM request exceeds memory budget".into());
    }
    instrument.prepare(&PrepareContext::new(f64::from(rate), block, 2));
    let mut effects: Vec<Box<dyn Effect>> = Vec::new();
    for spec in &request.effects {
        let mut effect: Box<dyn Effect> = match spec.id.as_str() {
            "amp" => Box::new(auris_dsp::GuitarAmp::new()),
            "distortion" => Box::new(auris_dsp::Distortion::new()),
            _ => return Err("unknown effect".into()),
        };
        for (key, value) in &spec.params {
            let Some(descriptor) = effect.parameters().iter().find(|p| p.key == *key) else {
                return Err(format!("unknown effect parameter: {key}").into());
            };
            if !value.is_finite() || *value < descriptor.min || *value > descriptor.max {
                return Err(format!("out of range effect parameter: {key}").into());
            }
            effect.set_param_by_key(key, *value);
        }
        effect.prepare(&PrepareContext::new(f64::from(rate), block, 2));
        effects.push(effect);
    }
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
    let mut buffer = AudioBuffer::stereo(block, f64::from(rate));
    for (note_index, note) in request.notes.iter().enumerate() {
        let input = if let Some(path) = request.inputs.get(note_index) {
            let raw = std::fs::read(path)?;
            if raw.len() != frames * 4 {
                return Err("input PCM length differs from request".into());
            }
            let data: Vec<f32> = raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect();
            if data.iter().any(|sample| !sample.is_finite()) {
                return Err("non-finite input PCM".into());
            }
            Some(data)
        } else {
            None
        };
        let hold = note.hold.unwrap_or(request.hold);
        if note.pitch > 127
            || !note.velocity.is_finite()
            || !(0.0..=1.0).contains(&note.velocity)
            || !note.tuning_cents.is_finite()
            || note.tuning_cents.abs() > 75.0
            || !hold.is_finite()
            || !(0.0..=request.seconds).contains(&hold)
            || !automation(&note.bends, hold, -12.0..=12.0)
            || !automation(&note.expression, hold, 0.0..=1.0)
        {
            return Err("invalid pitch or velocity".into());
        }
        instrument.reset();
        for effect in &mut effects {
            effect.reset();
        }
        let mut scheduled = vec![NoteEvent::PitchBend {
            frame: 0,
            semitones: note.tuning_cents / 100.0,
        }];
        scheduled.extend(note.bends.iter().map(|point| NoteEvent::PitchBend {
            frame: (point.seconds * rate as f32).round() as u32,
            semitones: point.value,
        }));
        scheduled.extend(note.expression.iter().map(|point| NoteEvent::Controller {
            frame: (point.seconds * rate as f32).round() as u32,
            number: 11,
            value: point.value,
        }));
        scheduled.push(NoteEvent::NoteOn {
            frame: 0,
            pitch: note.pitch,
            velocity: note.velocity,
        });
        scheduled.push(NoteEvent::NoteOff {
            frame: (hold * rate as f32).round() as u32,
            pitch: note.pitch,
        });
        // Stable sorting applies time-zero controls before the excitation and keeps
        // every later control at its actual sample, independently of host blocks.
        scheduled.sort_by_key(NoteEvent::frame);
        let mut cursor = 0;
        for start in (0..frames).step_by(block) {
            let mut events = Vec::new();
            while let Some(event) = scheduled.get(cursor) {
                if event.frame() as usize >= start + block {
                    break;
                }
                events.push((*event).with_frame(event.frame() - start as u32));
                cursor += 1;
            }
            let context =
                ProcessContext::realtime(f64::from(rate), block, start as u64, 120.0, true);
            if let Some(input) = &input {
                buffer.clear();
                let count = block.min(frames - start);
                for channel in buffer.channels_mut() {
                    channel[..count].copy_from_slice(&input[start..start + count]);
                }
            } else {
                instrument.process(&events, &mut buffer, &context);
            }
            for effect in &mut effects {
                effect.process(&mut buffer, &context);
            }
            samples.extend_from_slice(&buffer.channel(0)[..block.min(frames - start)]);
        }
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err("non-finite renderer output".into());
    }
    Ok(samples)
}

fn instrument(name: &str) -> Result<Box<dyn Instrument>, Box<dyn Error>> {
    let model = match name {
        "piano" => Model::Piano,
        "guitar" => Model::Guitar,
        "electric_guitar" => Model::ElectricGuitar,
        "violin" => Model::Violin,
        "bass" => Model::Bass,
        "bell" => Model::Bell,
        "mallet" => Model::Mallet,
        "drums" => return Ok(Box::new(DrumKit::new())),
        _ => return Err("unknown model".into()),
    };
    Ok(Box::new(Physical::new(model)))
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("--describe") {
        let models: BTreeMap<_, _> = [
            "piano",
            "guitar",
            "electric_guitar",
            "violin",
            "bass",
            "bell",
            "mallet",
            "drums",
        ]
        .into_iter()
        .map(|name| -> Result<_, Box<dyn Error>> {
            let instrument = instrument(name)?;
            let parameters: BTreeMap<_, _> = instrument
                .parameters()
                .iter()
                .map(|p| (p.key.clone(), instrument.param(p.id)))
                .collect();
            Ok((name, parameters))
        })
        .collect::<Result<_, _>>()?;
        let mut description = serde_json::to_value(&models)?;
        let response_fraction = if description["violin"].get("bow_response").is_some() {
            0.5
        } else {
            0.0
        };
        description["_performance"] = serde_json::json!({
            "expression_response_fraction": response_fraction,
            "expression_exponent": Model::Violin.expression_exponent()
        });
        description["_radiation"] = serde_json::json!({
            "piano": Model::Piano.radiation_modes(),
            "guitar": Model::Guitar.radiation_modes(),
            "violin": Model::Violin.radiation_modes()
        });
        println!("{}", serde_json::to_string(&description)?);
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
    fn trajectories_are_sample_exact_across_block_sizes_and_reach_the_target_pitch() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "model": "violin", "notes": [{"pitch": 69, "velocity": 0.65,
                "bends": [{"seconds": 0.233, "value": 5.0}],
                "expression": [{"seconds": 0.417, "value": 0.7}]}],
            "seconds": 1.5, "hold": 1.25,
            "params": {"release": 0.05, "bow_response": 0.012}, "sample_rate": 24000
        }))
        .unwrap();
        let reference = render_blocks(&request, 64).unwrap();
        assert_eq!(reference, render_blocks(&request, 257).unwrap());
        assert_eq!(reference, render_blocks(&request, 1024).unwrap());
        let steady = &reference[16_800..24_000];
        let energy = |hz: f64| {
            let (mut real, mut imaginary) = (0.0, 0.0);
            for (index, sample) in steady.iter().enumerate() {
                let phase = std::f64::consts::TAU * hz * index as f64 / 24_000.0;
                real += f64::from(*sample) * phase.cos();
                imaginary += f64::from(*sample) * phase.sin();
            }
            real * real + imaginary * imaginary
        };
        assert!(energy(587.33) > energy(440.0) * 10.0);
        assert!(reference[34_000..].iter().all(|sample| sample.abs() < 0.01));
    }

    #[test]
    fn trajectories_reject_non_finite_unsorted_and_out_of_range_points() {
        for points in [
            vec![Point {
                seconds: 0.2,
                value: f32::NAN,
            }],
            vec![Point {
                seconds: 0.1,
                value: 13.0,
            }],
            vec![Point {
                seconds: 0.7,
                value: 0.0,
            }],
            vec![
                Point {
                    seconds: 0.2,
                    value: 0.0,
                },
                Point {
                    seconds: 0.1,
                    value: 0.0,
                },
            ],
        ] {
            assert!(!automation(&points, 0.5, -12.0..=12.0));
        }
    }

    #[test]
    fn worker_resets_between_notes_and_preserves_exact_length() {
        let mut request = Request {
            model: "piano".into(),
            notes: vec![
                Note {
                    pitch: 60,
                    velocity: 0.7,
                    tuning_cents: 0.0,
                    hold: None,
                    bends: Vec::new(),
                    expression: Vec::new(),
                },
                Note {
                    pitch: 60,
                    velocity: 0.7,
                    tuning_cents: 0.0,
                    hold: None,
                    bends: Vec::new(),
                    expression: Vec::new(),
                },
            ],
            seconds: 0.101,
            hold: 0.05,
            params: BTreeMap::new(),
            sample_rate: RATE,
            effects: Vec::new(),
            inputs: Vec::new(),
        };
        for model in [
            "piano",
            "guitar",
            "electric_guitar",
            "violin",
            "bass",
            "bell",
            "mallet",
            "drums",
        ] {
            request.model = model.into();
            let pitch = if model == "drums" { 36 } else { 60 };
            for note in &mut request.notes {
                note.pitch = pitch;
            }
            let samples = render(&request).unwrap();
            assert_eq!(samples.len(), 4848);
            assert_eq!(
                &samples[..2424],
                &samples[2424..],
                "{model} retained state between notes"
            );
            assert!(
                samples.iter().any(|sample| sample.abs() > 0.01),
                "silent {model}"
            );
        }
    }

    #[test]
    fn worker_rejects_unknown_or_non_finite_parameters() {
        let mut request = Request {
            model: "guitar".into(),
            notes: vec![Note {
                pitch: 48,
                velocity: 0.6,
                tuning_cents: 0.0,
                hold: None,
                bends: Vec::new(),
                expression: Vec::new(),
            }],
            seconds: 0.1,
            hold: 0.1,
            params: BTreeMap::from([("missing".into(), 0.5)]),
            sample_rate: RATE,
            effects: Vec::new(),
            inputs: Vec::new(),
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
                hold: None,
                bends: Vec::new(),
                expression: Vec::new(),
            }],
            seconds: 0.125,
            hold: 0.125,
            params: BTreeMap::new(),
            sample_rate: 48_000,
            effects: Vec::new(),
            inputs: Vec::new(),
        };
        assert_eq!(render(&request).unwrap().len(), 6000);
        request.sample_rate = 1;
        assert!(render(&request).is_err());
    }

    #[test]
    fn recorded_di_and_amp_are_block_independent_reset_and_reject_invalid_pcm() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("di.f32");
        let raw: Vec<_> = (0..2424)
            .flat_map(|index| ((index as f32 * 0.057).sin() * 0.2).to_le_bytes())
            .collect();
        std::fs::write(&path, &raw).unwrap();
        let request: Request = serde_json::from_value(serde_json::json!({
            "model": "electric_guitar", "notes": [{"pitch": 64, "velocity": 0.8},
                {"pitch": 64, "velocity": 0.8}], "seconds": 0.101, "hold": 0.101,
            "params": {}, "effects": [{"id": "amp", "params": {"drive_db": 24}}],
            "inputs": [path, path]
        }))
        .unwrap();
        let samples = render_blocks(&request, 64).unwrap();
        assert_eq!(samples.len(), 4848);
        assert_eq!(&samples[..2424], &samples[2424..]);
        assert_eq!(samples, render_blocks(&request, 257).unwrap());
        assert!(samples.iter().any(|sample| sample.abs() > 0.05));
        std::fs::write(&path, &raw[..100]).unwrap();
        assert!(
            render(&request)
                .unwrap_err()
                .to_string()
                .contains("length differs")
        );
        let mut invalid = raw;
        invalid[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        std::fs::write(&path, invalid).unwrap();
        assert!(
            render(&request)
                .unwrap_err()
                .to_string()
                .contains("non-finite input")
        );
    }
}
