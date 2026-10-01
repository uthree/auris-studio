//! Writes a deterministic listening probe: piano, guitar, bass, bell, mallet, then violin.

use std::{error::Error, fs::File, io::Write, path::PathBuf};

use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
};
use auris_synth::{Model, Physical};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| "physical-demo.wav".into());
    let rate: u32 = 48_000;
    let mut all = Vec::new();
    for model in Model::ALL {
        let mut instrument = Physical::new(model);
        instrument.prepare(&PrepareContext::new(f64::from(rate), 256, 2));
        instrument.set_param_by_key("level", -6.0);
        let mut buffer = AudioBuffer::stereo(256, f64::from(rate));
        let mut samples = Vec::new();
        let pitch = if model == Model::Bass { 40 } else { 60 };
        let events = [
            NoteEvent::NoteOn {
                frame: 0,
                pitch,
                velocity: 0.4,
            },
            NoteEvent::NoteOff {
                frame: 24_000,
                pitch,
            },
            NoteEvent::NoteOn {
                frame: 36_000,
                pitch: pitch + 4,
                velocity: 0.65,
            },
            NoteEvent::NoteOff {
                frame: 60_000,
                pitch: pitch + 4,
            },
            NoteEvent::NoteOn {
                frame: 72_000,
                pitch: pitch + 7,
                velocity: 0.9,
            },
            NoteEvent::NoteOff {
                frame: 108_000,
                pitch: pitch + 7,
            },
        ];
        for start in (0..192_000).step_by(256) {
            let block_events: Vec<_> = events
                .iter()
                .filter(|event| (start..start + 256).contains(&event.frame()))
                .map(|event| event.with_frame(event.frame() - start))
                .collect();
            let ctx = ProcessContext::realtime(f64::from(rate), 256, u64::from(start), 120.0, true);
            instrument.process(&block_events, &mut buffer, &ctx);
            samples.extend_from_slice(buffer.channel(0));
        }
        let peak = samples.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        println!("{model:?}: peak {peak:.5}, RMS {rms:.5}");
        all.extend(samples);
    }
    let mut file = File::create(&path)?;
    let bytes = (all.len() * 2) as u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?; // PCM
    file.write_all(&1_u16.to_le_bytes())?; // mono
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * 2).to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for sample in all {
        file.write_all(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    println!("{}", path.display());
    Ok(())
}
