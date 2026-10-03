//! An editable four-chord phrase and matched wordless choir vowel auditions.

use std::{error::Error, fs, path::PathBuf};

use auris_session::prelude::*;
use auris_session::{Session, SessionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("expected output directory")?,
    );
    fs::create_dir_all(&directory)?;
    let mut session = Session::new(SessionOptions::headless().with_balance(false))?;
    let track = session.add_instrument_track("Choir", "auris.physical.choir")?;
    let clip = session.add_midi_clip(
        track,
        "Wordless choir",
        Ticks::ZERO,
        Ticks::from_beats(20.0),
    )?;
    for (index, pitches) in [
        [48, 55, 60, 64],
        [45, 52, 57, 60],
        [41, 48, 57, 60],
        [43, 50, 55, 62],
    ]
    .into_iter()
    .enumerate()
    {
        for pitch in pitches {
            let mut note = Note::new(
                pitch,
                Ticks::from_beats(index as f64 * 4.0),
                Ticks::from_beats(3.8),
            );
            note.velocity = 0.7;
            session.add_note(clip, note)?;
        }
    }
    let saved = session.save_as(&directory.join("choir-preview.auris"))?;
    println!("project: {}", saved.document.display());
    let vowel = session
        .instrument_descriptors(track)
        .iter()
        .find(|p| p.key == "vowel")
        .ok_or("missing vowel parameter")?
        .id;
    for (name, value) in [("oo", 0.0), ("ah", 1.0), ("ee", 2.0)] {
        session.set_param(
            ParamTarget::Instrument {
                track,
                param: vowel,
            },
            value,
        );
        let path = directory.join(format!("choir-{name}.wav"));
        let summary = session.render_job().render_to_wav(
            &path,
            &Default::default(),
            &Default::default(),
            &mut Default::default(),
        )?;
        println!("audio: {} ({summary:?})", path.display());
    }
    Ok(())
}
