//! Render the effects modelled as moving things against their dry originals, as editable
//! projects.
//!
//! `cargo run -p auris-session --example resonance_demo -- <directory>` writes, for each one, a
//! project and a WAV: a bowed melody over a I–V–vi–IV in D with and without sympathetic strings
//! following the chords; an electric guitar's pickup signal and a chiptune lead put through a
//! speaker cabinet, a guitar's box and a cello's; piano chords on a plate; the guitar in a spring
//! tank and through a valve; the chiptune lead on a tape echo; and pad chords on a rotary
//! speaker, slow and fast. The directory must be empty.

use std::{error::Error, path::Path, path::PathBuf};

use auris_core::{Note, ParamTarget, Ticks, TrackId};
use auris_session::prelude::MusicalKey;
use auris_session::{Session, SessionOptions};

/// A melody that dwells on the chord tones of D, A, Bm and G, a bar each, twice.
const MELODY: [(u8, f64, f64); 16] = [
    (66, 0.0, 1.0),
    (69, 1.0, 1.0),
    (74, 2.0, 2.0),
    (76, 4.0, 1.0),
    (73, 5.0, 1.0),
    (69, 6.0, 2.0),
    (74, 8.0, 1.0),
    (71, 9.0, 1.0),
    (66, 10.0, 2.0),
    (67, 12.0, 1.0),
    (71, 13.0, 1.0),
    (74, 14.0, 2.0),
    (78, 16.0, 2.0),
    (76, 18.0, 2.0),
    (74, 20.0, 4.0),
    (74, 24.0, 6.0),
];

fn set(
    session: &mut Session,
    track: TrackId,
    slot: auris_core::EffectSlotId,
    effect: &str,
    values: &[(&str, f32)],
) -> Result<(), Box<dyn Error>> {
    for (key, value) in values {
        let param = session
            .param_descriptors(effect)
            .iter()
            .find(|param| param.key == *key)
            .ok_or_else(|| format!("{effect} has no {key}"))?
            .id;
        session.set_param(
            ParamTarget::Effect {
                track: Some(track),
                slot,
                param,
            },
            *value,
        );
    }
    Ok(())
}

fn render(session: &mut Session, directory: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    session.save_as(&directory.join(format!("{name}.auris")))?;
    session.render_job().render_to_wav(
        &directory.join(format!("{name}.wav")),
        &Default::default(),
        &Default::default(),
        &mut Default::default(),
    )?;
    println!("{name}");
    Ok(())
}

fn strings(directory: &Path, wet: bool) -> Result<(), Box<dyn Error>> {
    let mut session = Session::new(SessionOptions::headless())?;
    session.set_bpm(84.0);
    session.set_key(Ticks::ZERO, MusicalKey::parse("D major").ok_or("key")?);
    session.stamp_named_progression("axis", Ticks::ZERO, 8)?;
    let track = session.add_instrument_track("Violin", "auris.physical.violin")?;
    let clip = session.add_midi_clip(track, "Melody", Ticks::ZERO, Ticks::from_beats(32.0))?;
    for (pitch, beat, length) in MELODY {
        let mut note = Note::new(
            pitch,
            Ticks::from_beats(beat),
            Ticks::from_beats(length * 0.95),
        );
        note.velocity = 0.7;
        session.add_note(clip, note)?;
    }
    if wet {
        let slot = session.add_effect(Some(track), "auris.fx.sympathetic")?;
        set(
            &mut session,
            track,
            slot,
            "auris.fx.sympathetic",
            &[("mix", 0.8), ("decay", 6.0), ("strings", 12.0)],
        )?;
    }
    session.add_effect(Some(track), "auris.fx.reverb")?;
    render(
        &mut session,
        directory,
        if wet {
            "strings-with-sympathetic"
        } else {
            "strings-dry"
        },
    )
}

/// An effect and the settings it is rendered with.
type Insert<'a> = Option<(&'a str, &'a [(&'a str, f32)])>;

fn insert(session: &mut Session, track: TrackId, effect: Insert) -> Result<(), Box<dyn Error>> {
    if let Some((effect, values)) = effect {
        let slot = session.add_effect(Some(track), effect)?;
        set(session, track, slot, effect, values)?;
    }
    Ok(())
}

fn phrase(
    directory: &Path,
    name: &str,
    instrument: &str,
    effect: Insert,
) -> Result<(), Box<dyn Error>> {
    let mut session = Session::new(SessionOptions::headless())?;
    session.set_bpm(100.0);
    let track = session.add_instrument_track("Lead", instrument)?;
    let clip = session.add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(16.0))?;
    let phrase = [
        (52, 0.0, 0.5),
        (55, 0.5, 0.5),
        (57, 1.0, 0.5),
        (59, 1.5, 1.5),
        (62, 3.0, 0.5),
        (59, 3.5, 0.5),
        (57, 4.0, 2.0),
        (55, 6.0, 0.5),
        (57, 6.5, 0.5),
        (59, 7.0, 1.0),
        (64, 8.0, 3.0),
        (62, 11.0, 0.5),
        (59, 11.5, 0.5),
        (57, 12.0, 4.0),
    ];
    for (pitch, beat, length) in phrase {
        let mut note = Note::new(
            pitch,
            Ticks::from_beats(beat),
            Ticks::from_beats(length * 0.9),
        );
        note.velocity = 0.75;
        session.add_note(clip, note)?;
    }
    insert(&mut session, track, effect)?;
    render(&mut session, directory, name)
}

/// Block chords of the I–V–vi–IV in D, a bar each with a beat of rest, twice.
fn chords(
    directory: &Path,
    name: &str,
    instrument: &str,
    velocity: f32,
    effect: Insert,
) -> Result<(), Box<dyn Error>> {
    let mut session = Session::new(SessionOptions::headless())?;
    session.set_bpm(90.0);
    let track = session.add_instrument_track("Chords", instrument)?;
    let clip = session.add_midi_clip(track, "Chords", Ticks::ZERO, Ticks::from_beats(32.0))?;
    let voicings: [&[u8]; 4] = [
        &[50, 62, 66, 69],
        &[45, 61, 64, 69],
        &[47, 62, 66, 71],
        &[43, 62, 67, 71],
    ];
    for bar in 0..8 {
        for pitch in voicings[bar % 4] {
            let mut note = Note::new(
                *pitch,
                Ticks::from_beats(bar as f64 * 4.0),
                Ticks::from_beats(if bar == 7 { 4.0 } else { 3.0 }),
            );
            note.velocity = velocity;
            session.add_note(clip, note)?;
        }
    }
    insert(&mut session, track, effect)?;
    render(&mut session, directory, name)
}

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("expected output directory")?,
    );
    std::fs::create_dir_all(&directory)?;
    strings(&directory, false)?;
    strings(&directory, true)?;
    let electric = "auris.physical.electric_guitar";
    let chiptune = "auris.synth.chiptune";
    let body = |kind: f32| -> [(&str, f32); 3] { [("body", kind), ("mix", 1.0), ("width", 0.8)] };
    phrase(&directory, "electric-dry", electric, None)?;
    phrase(
        &directory,
        "electric-cabinet",
        electric,
        Some(("auris.fx.body", &body(5.0))),
    )?;
    phrase(
        &directory,
        "electric-guitar-body",
        electric,
        Some(("auris.fx.body", &body(0.0))),
    )?;
    phrase(&directory, "chiptune-dry", chiptune, None)?;
    phrase(
        &directory,
        "chiptune-cello-body",
        chiptune,
        Some(("auris.fx.body", &body(2.0))),
    )?;
    phrase(
        &directory,
        "electric-spring",
        electric,
        Some(("auris.fx.spring", &[("mix", 0.4), ("decay", 2.5)])),
    )?;
    phrase(
        &directory,
        "electric-tube",
        electric,
        Some(("auris.fx.tube", &[("drive_db", 34.0), ("iron", 0.5)])),
    )?;
    phrase(
        &directory,
        "chiptune-tape-echo",
        chiptune,
        Some((
            "auris.fx.tape",
            &[
                ("time", 300.0),
                ("heads", 1.0),
                ("feedback", 0.5),
                ("mix", 0.4),
            ],
        )),
    )?;
    let piano = "auris.physical.piano";
    chords(&directory, "piano-dry", piano, 0.7, None)?;
    chords(
        &directory,
        "piano-plate",
        piano,
        0.7,
        Some(("auris.fx.plate", &[("mix", 0.35), ("decay", 3.0)])),
    )?;
    let pad = "auris.synth.pad";
    chords(&directory, "pad-dry", pad, 0.4, None)?;
    chords(
        &directory,
        "pad-rotary-chorale",
        pad,
        0.4,
        Some(("auris.fx.rotary", &[("speed", 1.0)])),
    )?;
    chords(
        &directory,
        "pad-rotary-tremolo",
        pad,
        0.4,
        Some(("auris.fx.rotary", &[("speed", 2.0)])),
    )?;
    Ok(())
}
