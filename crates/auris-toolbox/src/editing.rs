//! Reading and revising the musical decisions in an existing document.

use super::*;

/// Range, density and repetition without an audio render.
pub mod analyze_music {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "analyze_music";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Measures each note clip's pitch range, note density, pitch-class count and exact bar-pattern repetition. Reads stored notes without rendering. These describe musical choices, not aesthetic quality; use analyze for loudness and audio input for listening. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.";
    /// The project to inspect.
    pub use crate::ProjectArgs as Args;
    /// Returns measurements with the track and clip numbers used by editing tools.
    pub fn run(args: &Args) -> Result<String, String> {
        let session = opened(&args.project)?;
        let reports: Vec<_> = session.analyze_music().into_iter().map(|report| {
            serde_json::json!({"track": session.project().track(report.track).map(|track| &track.name),
                "clip": report.clip.0, "track_id":report.track.0,"measurements": report})
        }).collect();
        reports::publish(&reports)
    }
}

/// The saved specification and the current musical state, read separately.
pub mod inspect_composition {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "inspect_composition";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Read the original song specification, current instrument controls, key/chords/tempo/meter/sections and clip recipes, plus tracked changes since composition. Old projects without a saved baseline report comparison unavailable. The original specification is provenance; the .auris project holds final edits. Comparison scope and untracked state are explicit. Large results return report_id; use read_report without repeating edits.";
    /// The project to inspect.
    pub use crate::ProjectArgs as Args;
    /// Reports musical decisions without rendering audio or changing the document.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let state = session.inspect_composition_state();
        let project = session.project();
        let tracks: Vec<_> = project.tracks.iter().map(|track| {
            let clips: Vec<_> = track.kind.note_clips().map(Vec::as_slice).unwrap_or_default().iter().map(|clip| {
                serde_json::json!({"clip": clip.id.0, "name": clip.name,
                    "start_bar": project.signatures.bar_of(clip.start), "start_tick": clip.start.raw(),
                    "length_ticks": clip.length.raw(), "notes": clip.notes.len(),
                    "recipe": clip.recipe, "hand_edited": session.clip_hand_edited(clip.id)})
            }).collect();
            serde_json::json!({"name": track.name, "track": track.id.0, "clips": clips})
        }).collect();
        reports::publish(serde_json::json!({
            "original_specification": project.song_spec,
            "current_instruments": state["instruments"],
            "changes_since_composition": {"baseline_available":state["baseline_available"],"changes":state["changes"],
                "comparison_scope":state["comparison_scope"],"uncompared_state":state["uncompared_state"],"note":state["baseline_note"]},
            "final_state":state["final_state"],
            "playback": session.playback_readiness(),
            "grooves": groove_catalog().iter().map(|groove| groove.name).collect::<Vec<_>>(),
            "harmony": project.harmony, "tempo": project.tempo_map,
            "meter": project.signatures, "sections": project.sections, "lyrics": project.lyrics, "tracks": tracks,
            "time_units": "Map positions are ticks; 960 ticks are one quarter note. Track and clip IDs are stable numeric document IDs."
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn composition_inspection_exposes_current_controls_without_saving_legacy_projects() {
        let root = tempfile::tempdir().unwrap();
        let mut session = headless().unwrap();
        session.add_default_instrument_track("Lead").unwrap();
        let path = root.path().join("Inspection.auris");
        session.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let response = inspect_composition::run(&args(json!({"project":path}))).unwrap();
        let report: serde_json::Value = serde_json::from_str(&response).unwrap();
        let read = |path: &str| -> serde_json::Value {
            if let Some(id) = report["report_id"].as_str() {
                let page = read_report::run(&read_report::Args {
                    report_id: id.into(),
                    path: path.into(),
                    offset: 0,
                    limit: None,
                })
                .unwrap();
                serde_json::from_str::<serde_json::Value>(&page).unwrap()["data"].clone()
            } else {
                report.pointer(path).unwrap().clone()
            }
        };
        assert_eq!(read("/changes_since_composition/baseline_available"), false);
        assert_eq!(read("/current_instruments/0/name"), "Lead");
        assert!(read("/current_instruments/0/parameters/0/param").is_string());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
