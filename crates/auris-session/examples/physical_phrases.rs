//! Saves fixed piano chords, guitar picking/strumming and violin legato listening projects.

use std::{error::Error, fs, path::PathBuf};

use auris_core::{Note, ParamTarget, PresetRef, Ticks};
use auris_session::{Session, SessionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = PathBuf::from(args.next().ok_or("expected output directory")?);
    let font_path = args.next().map(PathBuf::from);
    fs::create_dir_all(&directory)?;
    for (name, patch) in [("piano", 0), ("guitar", 25), ("violin", 40)] {
        let mut session = Session::new(SessionOptions::headless().with_balance(false))?;
        let track = session.add_default_instrument_track(name)?;
        if let Some(path) = &font_path {
            let font = session.import_soundfont(path)?;
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
            if name == "violin" {
                let param = session
                    .instrument_descriptors(track)
                    .iter()
                    .find(|parameter| parameter.key == "legato")
                    .ok_or("missing legato")?
                    .id;
                session.set_param(ParamTarget::Instrument { track, param }, 1.0);
            }
        }
        let clip = session.add_midi_clip(track, name, Ticks::ZERO, Ticks::from_beats(12.0))?;
        let mut notes = Vec::new();
        let mut add = |pitch, beat, duration, velocity| {
            let mut note = Note::new(pitch, Ticks::from_beats(beat), Ticks::from_beats(duration));
            note.velocity = velocity;
            notes.push(note);
        };
        match name {
            "piano" => {
                for pitch in [60, 64, 67] {
                    add(pitch, 0.0, 2.0, 0.75);
                }
                for (index, pitch) in [72, 71, 69, 67, 64, 60].into_iter().enumerate() {
                    add(
                        pitch,
                        2.0 + index as f64 * 0.5,
                        0.75,
                        0.45 + index as f32 * 0.05,
                    );
                }
                for pitch in [57, 60, 64] {
                    add(pitch, 6.0, 3.0, 0.8);
                }
            }
            "guitar" => {
                for (index, pitch) in [40, 52, 55, 59, 64, 67, 64, 59].into_iter().enumerate() {
                    add(pitch, index as f64 * 0.5, 1.25, 0.65);
                }
                for (index, pitch) in [45, 52, 57, 60, 64].into_iter().enumerate() {
                    add(pitch, 5.0 + index as f64 * 0.03, 4.0, 0.8);
                }
            }
            _ => {
                for (index, pitch) in [64, 67, 69, 72, 71, 69, 67, 64].into_iter().enumerate() {
                    add(pitch, index as f64 * 0.75, 0.85, 0.7);
                }
                for (index, pitch) in [67, 69, 71, 72].into_iter().enumerate() {
                    add(pitch, 8.0 + index as f64 * 0.5, 0.6, 0.8);
                }
            }
        }
        for note in notes {
            session.add_note(clip, note)?;
        }
        let report = session.save_as(&directory.join(format!("{name}.auris")))?;
        println!("{}", report.document.display());
    }
    Ok(())
}
