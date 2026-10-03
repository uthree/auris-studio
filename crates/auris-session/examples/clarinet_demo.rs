//! Save an editable clarinet audition spanning chalumeau and clarion registers.

use std::{error::Error, path::PathBuf};

use auris_core::{Note, Ticks};
use auris_session::{Session, SessionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("expected output directory")?,
    );
    std::fs::create_dir_all(&directory)?;
    let mut session = Session::new(SessionOptions::headless().with_balance(false))?;
    let track = session.add_instrument_track("Clarinet", "auris.physical.clarinet")?;
    let clip = session.add_midi_clip(
        track,
        "Register and articulation audition",
        Ticks::ZERO,
        Ticks::from_beats(24.0),
    )?;
    for (beat, pitch, duration, velocity) in [
        (0.0, 50, 1.8, 0.55),
        (2.0, 53, 0.85, 0.65),
        (3.0, 57, 0.85, 0.75),
        (4.0, 60, 1.8, 0.80),
        (6.0, 57, 0.85, 0.70),
        (7.0, 53, 0.65, 0.60),
        (8.0, 62, 1.8, 0.70),
        (10.0, 65, 0.85, 0.75),
        (11.0, 69, 0.85, 0.80),
        (12.0, 72, 1.8, 0.85),
        (14.0, 69, 0.85, 0.75),
        (15.0, 65, 0.65, 0.70),
        (16.0, 74, 0.42, 0.80),
        (16.5, 72, 0.42, 0.75),
        (17.0, 69, 0.42, 0.70),
        (17.5, 65, 0.42, 0.65),
        (18.0, 62, 1.65, 0.60),
        (20.0, 50, 3.5, 0.75),
    ] {
        let mut note = Note::new(pitch, Ticks::from_beats(beat), Ticks::from_beats(duration));
        note.velocity = velocity;
        session.add_note(clip, note)?;
    }
    let saved = session.save_as(&directory.join("clarinet.auris"))?;
    session.render_job().render_to_wav(
        &directory.join("clarinet.wav"),
        &Default::default(),
        &Default::default(),
        &mut Default::default(),
    )?;
    println!("{}", saved.document.display());
    Ok(())
}
