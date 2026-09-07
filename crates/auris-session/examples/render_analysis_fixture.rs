//! Render an original piano/bass excerpt for model smoke tests, with note references.
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PluginState, PrepareContext, PresetRef,
    ProcessContext, SoundFontId,
};
use auris_sampler::{Sampler, SoundFontBank, store_preset};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("expected SoundFont path and new output folder".into());
    }
    let out = PathBuf::from(&args[1]);
    if out.exists() {
        return Err("output folder already exists".into());
    }
    let bank = SoundFontBank::shared();
    bank.insert(
        SoundFontId(1),
        auris_io::load_soundfont(std::path::Path::new(&args[0]))?,
    );
    let rate = 16000.0;
    let frames = 12 * 16000;
    let mut channels = vec![vec![0.0_f32; frames]; 2];
    let mut reference = Vec::new();
    let mut stems = Vec::new();
    for (name, patch, bass) in [("piano", 0, false), ("bass", 32, true)] {
        let mut synth = Sampler::new(bank.clone());
        let mut state = PluginState::default();
        store_preset(
            &mut state,
            PresetRef {
                font: SoundFontId(1),
                bank: 0,
                patch,
            },
        );
        synth.load_state(&state);
        synth.prepare(&PrepareContext::new(rate, 256, 2));
        let mut events = Vec::new();
        for index in 0..16 {
            if bass && index % 2 == 1 {
                continue;
            }
            let start = 0.5 + index as f64 * 0.5;
            let end = start + if bass { 0.85 } else { 0.42 };
            let pitches: Vec<u8> = if bass {
                vec![[36, 41, 43, 36][index / 4]]
            } else {
                let root = [60, 65, 67, 60][index / 4];
                vec![root, root + 4, root + 7]
            };
            for pitch in pitches {
                events.push(((start * rate) as usize, pitch, true));
                events.push(((end * rate) as usize, pitch, false));
                reference.push(
                    serde_json::json!({"instrument":name,"pitch":pitch,"start":start,"end":end}),
                );
            }
        }
        events.sort_by_key(|e| (e.0, e.2));
        let mut stem = vec![vec![0.0_f32; frames]; 2];
        for from in (0..frames).step_by(256) {
            let count = (frames - from).min(256);
            let block_events: Vec<_> = events
                .iter()
                .filter(|e| e.0 >= from && e.0 < from + count)
                .map(|&(frame, pitch, on)| {
                    if on {
                        NoteEvent::NoteOn {
                            frame: (frame - from) as u32,
                            pitch,
                            velocity: 0.7,
                        }
                    } else {
                        NoteEvent::NoteOff {
                            frame: (frame - from) as u32,
                            pitch,
                        }
                    }
                })
                .collect();
            let mut block = AudioBuffer::new(2, count, rate);
            synth.process(
                &block_events,
                &mut block,
                &ProcessContext {
                    sample_rate: rate,
                    block_frames: count,
                    playhead_samples: from as u64,
                    bpm: 120.0,
                    is_playing: true,
                    is_offline: true,
                },
            );
            for (channel, rendered) in stem.iter_mut().zip(block.iter_channels()) {
                channel[from..from + count].copy_from_slice(rendered);
            }
        }
        for (mixed, source) in channels.iter_mut().zip(&stem) {
            for (target, sample) in mixed.iter_mut().zip(source) {
                *target += sample;
            }
        }
        stems.push((name, AudioBuffer::from_planar(stem, rate)?));
    }
    std::fs::create_dir_all(&out)?;
    let settings = auris_io::WavExportSettings {
        sample_rate: 16000,
        bit_depth: auris_io::WavBitDepth::Float32,
        dither: false,
    };
    for (name, stem) in stems {
        auris_io::write_wav(&out.join(format!("{name}.wav")), &stem, &settings)?;
    }
    auris_io::write_wav(
        &out.join("mixture.wav"),
        &AudioBuffer::from_planar(channels, rate)?,
        &settings,
    )?;
    std::fs::write(
        out.join("reference.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"notes":reference}))?,
    )?;
    Ok(())
}
