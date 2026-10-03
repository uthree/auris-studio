//! Save editable solo and combined folk phrases for the tin whistle and hammered dulcimer.

use std::{
    error::Error,
    path::{Path, PathBuf},
};

use auris_core::{Note, Ticks};
use auris_session::{Session, SessionOptions};

const LENGTH: f64 = 24.0;

fn add_note(
    session: &mut Session,
    clip: auris_core::ClipId,
    pitch: u8,
    beat: f64,
    duration: f64,
    velocity: f32,
) -> Result<(), Box<dyn Error>> {
    let mut note = Note::new(pitch, Ticks::from_beats(beat), Ticks::from_beats(duration));
    note.velocity = velocity;
    session.add_note(clip, note)?;
    Ok(())
}

fn add_dulcimer(session: &mut Session) -> Result<(), Box<dyn Error>> {
    let track =
        session.add_instrument_track("Hammered Dulcimer", "auris.physical.hammered_dulcimer")?;
    let clip = session.add_midi_clip(
        track,
        "Dulcimer ostinato",
        Ticks::ZERO,
        Ticks::from_beats(LENGTH),
    )?;
    for (bar, root) in [
        (0.0, 50),
        (4.0, 45),
        (8.0, 48),
        (12.0, 50),
        (16.0, 45),
        (20.0, 43),
    ] {
        for (index, pitch) in [root, root + 7, root + 12, root + 19]
            .into_iter()
            .enumerate()
        {
            add_note(session, clip, pitch, bar + index as f64 * 0.03, 3.7, 0.48)?;
        }
        for index in 0..8 {
            let pitch = root + [0, 7, 12, 7][index % 4];
            add_note(session, clip, pitch, bar + index as f64 * 0.5, 0.38, 0.42)?;
        }
    }
    Ok(())
}

fn add_whistle(session: &mut Session) -> Result<(), Box<dyn Error>> {
    let track = session.add_instrument_track("Tin Whistle", "auris.physical.tin_whistle")?;
    let clip =
        session.add_midi_clip(track, "Folk melody", Ticks::ZERO, Ticks::from_beats(LENGTH))?;
    for (index, pitch) in [
        74, 77, 79, 81, 79, 77, 74, 76, 74, 77, 79, 81, 84, 81, 79, 77,
    ]
    .into_iter()
    .enumerate()
    {
        add_note(
            session,
            clip,
            pitch,
            index as f64 * 0.75,
            0.62,
            0.58 + (index % 3) as f32 * 0.08,
        )?;
    }
    for (index, pitch) in [86, 84, 81, 79, 77, 74, 76, 74].into_iter().enumerate() {
        add_note(session, clip, pitch, 12.0 + index as f64 * 0.75, 0.62, 0.62)?;
    }
    add_note(session, clip, 74, 18.0, 5.5, 0.68)?;
    Ok(())
}

fn write_project(path: &Path, dulcimer: bool, whistle: bool) -> Result<(), Box<dyn Error>> {
    let mut session = Session::new(
        SessionOptions::headless()
            .with_balance(false)
            .with_sample_rate(48_000.0),
    )?;
    if dulcimer {
        add_dulcimer(&mut session)?;
    }
    if whistle {
        add_whistle(&mut session)?;
    }
    let saved = session.save_as(path)?;
    let wav = path.with_extension("wav");
    session.render_job().render_to_wav(
        &wav,
        &Default::default(),
        &Default::default(),
        &mut Default::default(),
    )?;
    println!("{}", saved.document.display());
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("expected output directory")?,
    );
    std::fs::create_dir_all(&directory)?;
    write_project(&directory.join("hammered-dulcimer.auris"), true, false)?;
    write_project(&directory.join("tin-whistle.auris"), false, true)?;
    write_project(&directory.join("folk-combined.auris"), true, true)?;
    Ok(())
}
