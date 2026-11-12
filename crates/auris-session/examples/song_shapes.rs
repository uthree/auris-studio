//! Prints the melody and arrangement checks an agent reads, for saved songs, as one JSON line
//! each — for comparing trial songs without opening them.
//!
//! ```text
//! cargo run --release -p auris-session --example song_shapes -- <song.auris>...
//! ```
//!
//! The accent of the lyrics against the melody needs the Japanese dictionary a headless session
//! loads. The rhythm of the sung lyrics is drawn as `lyric_rhythm` draws it for the agent.
//!
//! The melody is measured on the first singer track, or the first track with notes when the song
//! has no singer.

use std::path::Path;

use auris_session::session::{arrangement_shape, melody_shape};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let project = auris_io::load_project(Path::new(&path))?;
        let lead = project
            .tracks
            .iter()
            .find(|track| track.kind.as_singer().is_some())
            .or_else(|| {
                project.tracks.iter().find(|track| {
                    track
                        .kind
                        .note_clips()
                        .is_some_and(|clips| !clips.is_empty())
                })
            })
            .map(|track| track.id);
        let melody = lead.map(|track| melody_shape::melody_shape(&project, track));
        // The accent needs the Japanese dictionary, which a session loads and a bare project
        // does not.
        let session = lead.and_then(|_| {
            let mut session = auris_session::Session::new(
                auris_session::SessionOptions::headless().with_shipped_dictionary(true),
            )
            .ok()?;
            session.open(Path::new(&path)).ok()?;
            Some(session)
        });
        let accent = lead.zip(session.as_ref()).and_then(|(track, session)| {
            serde_json::to_value(session.lyric_accent(track, &[]).ok()?).ok()
        });
        let rhythm = lead.zip(session.as_ref()).and_then(|(track, session)| {
            serde_json::to_value(session.lyric_rhythm(track).ok()?).ok()
        });
        let report = serde_json::json!({
            "path": path,
            "song": project.name,
            "melody": melody.map(|result| result.unwrap_or_else(|error| serde_json::json!({"error": error}))),
            "accent": accent,
            "rhythm": rhythm,
            "arrangement": arrangement_shape::arrangement_shape(&project),
        });
        println!("{report}");
    }
    Ok(())
}
