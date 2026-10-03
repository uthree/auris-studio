//! Save three editable, sample-free guitar projects playing the same audition phrase.

use std::{error::Error, path::PathBuf};

use auris_core::{Note, ParamTarget, Ticks};
use auris_session::{Session, SessionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("expected output directory")?,
    );
    std::fs::create_dir_all(&directory)?;
    for (name, drive, output, cabinet) in [
        ("clean", 0.0, 0.0, 1.0),
        ("crunch", 18.0, -4.0, 1.0),
        ("lead", 34.0, -9.0, 2.0),
    ] {
        let mut session = Session::new(SessionOptions::headless().with_balance(false))?;
        let track =
            session.add_instrument_track("Electric Guitar", "auris.physical.electric_guitar")?;
        let pickup = session
            .instrument_descriptors(track)
            .iter()
            .find(|param| param.key == "pickup_position")
            .ok_or("missing pickup position")?
            .id;
        session.set_param(
            ParamTarget::Instrument {
                track,
                param: pickup,
            },
            0.064146,
        );
        let slot = session.add_effect(Some(track), "auris.fx.guitar_amp")?;
        for (key, value) in [
            ("drive_db", drive),
            ("output_db", output),
            ("cabinet", cabinet),
        ] {
            let param = session
                .param_descriptors("auris.fx.guitar_amp")
                .iter()
                .find(|param| param.key == key)
                .ok_or("missing amplifier parameter")?
                .id;
            session.set_param(
                ParamTarget::Effect {
                    track: Some(track),
                    slot,
                    param,
                },
                value,
            );
        }
        let clip = session.add_midi_clip(track, name, Ticks::ZERO, Ticks::from_beats(16.0))?;
        let mut add = |pitch, beat, duration, velocity| -> Result<(), Box<dyn Error>> {
            let mut note = Note::new(pitch, Ticks::from_beats(beat), Ticks::from_beats(duration));
            note.velocity = velocity;
            session.add_note(clip, note)?;
            Ok(())
        };
        for (index, pitch) in [40, 52, 55, 59, 64, 67, 64, 59].into_iter().enumerate() {
            add(pitch, index as f64 * 0.5, 1.1, 0.65)?;
        }
        for (beat, root) in [(4.0, 40), (6.0, 43)] {
            for (index, interval) in [0, 7, 12].into_iter().enumerate() {
                add(root + interval, beat + index as f64 * 0.025, 1.3, 0.8)?;
            }
        }
        for (index, pitch) in [64, 67, 69, 71, 74, 71, 69, 67].into_iter().enumerate() {
            add(
                pitch,
                8.0 + index as f64 * 0.5,
                0.45,
                0.55 + (index % 3) as f32 * 0.1,
            )?;
        }
        for (index, interval) in [0, 7, 12, 16].into_iter().enumerate() {
            add(40 + interval, 12.0 + index as f64 * 0.025, 3.0, 0.7)?;
        }
        let report = session.save_as(&directory.join(format!("{name}.auris")))?;
        session.render_job().render_to_wav(
            &directory.join(format!("{name}.wav")),
            &Default::default(),
            &Default::default(),
            &mut Default::default(),
        )?;
        println!("{}", report.document.display());
    }
    Ok(())
}
