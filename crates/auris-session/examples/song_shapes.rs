//! Prints the melody and arrangement checks an agent reads, for saved songs, as one JSON line
//! each — for comparing trial songs without opening them.
//!
//! ```text
//! cargo run --release -p auris-session --example song_shapes -- <song.auris>...
//! ```
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
        let report = serde_json::json!({
            "path": path,
            "song": project.name,
            "melody": melody.map(|result| result.unwrap_or_else(|error| serde_json::json!({"error": error}))),
            "arrangement": arrangement_shape::arrangement_shape(&project),
        });
        println!("{report}");
    }
    Ok(())
}
