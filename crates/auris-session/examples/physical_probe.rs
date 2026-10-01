//! Matched isolated-note probes for physical-model calibration and listening.

use std::{error::Error, fs, io::Write, path::PathBuf};

use auris_core::{Note, ParamTarget, PresetRef, Ticks};
use auris_session::{Session, SessionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = PathBuf::from(args.next().ok_or("expected output directory")?);
    let font = args.next().filter(|arg| arg != "native").map(PathBuf::from);
    let dry = args.next().is_some_and(|arg| arg == "dry");
    fs::create_dir_all(&directory)?;
    let mut session = Session::new(SessionOptions::headless().with_balance(false))?;
    let font = font
        .map(|path| session.import_soundfont(&path))
        .transpose()?;
    for (name, patch, pitches) in [
        ("piano", 0, [36, 43, 48, 55, 60, 67, 72, 79, 84]),
        ("guitar", 25, [40, 45, 50, 55, 59, 64, 69, 74, 79]),
        ("violin", 40, [55, 57, 60, 62, 64, 67, 69, 72, 76]),
    ] {
        let track = session.add_default_instrument_track(name)?;
        if let Some(font) = font {
            session.set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch,
                },
            )?;
        } else {
            session.set_track_instrument(track, &format!("auris.physical.{name}"))?;
            if dry {
                let param = session
                    .instrument_descriptors(track)
                    .iter()
                    .find(|p| p.key == "body")
                    .ok_or("missing body control")?
                    .id;
                session.set_param(ParamTarget::Instrument { track, param }, 0.0);
            }
        }
        let clip = session.add_midi_clip(track, name, Ticks::ZERO, Ticks::from_beats(5.0))?;
        for pitch in pitches {
            for (label, velocity) in [(50, 0.5), (85, 0.85)] {
                let mut note = Note::new(pitch, Ticks::ZERO, Ticks::from_beats(3.0));
                note.velocity = velocity;
                session.add_note(clip, note)?;
                let audio = session
                    .render_job()
                    .render(&Default::default(), &mut Default::default())?;
                let samples = audio.channel(0);
                let mut file =
                    fs::File::create(directory.join(format!("{name}-{pitch}-{label}.wav")))?;
                let bytes = u32::try_from(samples.len() * 4)?;
                file.write_all(b"RIFF")?;
                file.write_all(&(36 + bytes).to_le_bytes())?;
                file.write_all(b"WAVEfmt ")?;
                file.write_all(&16_u32.to_le_bytes())?;
                file.write_all(&3_u16.to_le_bytes())?; // IEEE float PCM.
                file.write_all(&1_u16.to_le_bytes())?;
                file.write_all(&48_000_u32.to_le_bytes())?;
                file.write_all(&192_000_u32.to_le_bytes())?;
                file.write_all(&4_u16.to_le_bytes())?;
                file.write_all(&32_u16.to_le_bytes())?;
                file.write_all(b"data")?;
                file.write_all(&bytes.to_le_bytes())?;
                for sample in samples {
                    file.write_all(&sample.to_le_bytes())?;
                }
                session.remove_notes(clip, &[0])?;
            }
        }
        session.remove_track(track)?;
        println!("wrote {name} probes");
    }
    Ok(())
}
