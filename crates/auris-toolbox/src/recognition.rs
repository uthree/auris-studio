//! CPU music recognition commands shared by both model frontends.

use super::*;
use auris_session::{AnalysisControl, ChordOptions};

fn analysis_control() -> AnalysisControl {
    AnalysisControl::cancelled_by(cancellation::current().flag_handle())
}

/// Written notes to chromatic chord candidates and optional explicit harmony edits.
pub mod analyze_chords {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "analyze_chords";
    /// Model-facing command description.
    pub const DESCRIPTION: &str = "Recognizes chords from written notes on the CPU without rendering or models. Reports absolute-tick intervals, alternate chord symbols and unknown/silent regions. Known percussion is excluded. Apply explicitly replaces recognized harmony and clears silence while preserving unknown intervals and outside harmony; saves a checkpoint. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.";
    /// Track selection, time range and optional acceptance.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Stable numeric track IDs; empty selects all pitched tracks.
        #[serde(default)]
        pub tracks: Vec<u64>,
        /// Nonnegative absolute start tick; defaults to zero.
        pub from_tick: Option<i64>,
        /// Exclusive absolute end tick; defaults to the project end.
        pub to_tick: Option<i64>,
        /// Positive analysis window in ticks; defaults to one quarter note (960).
        pub window_ticks: Option<i64>,
        /// Explicitly accept recognized candidates and save; false only reports.
        #[serde(default)]
        pub apply: bool,
    }
    /// Analyzes one immutable selection and optionally accepts its recognized harmony.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let tracks: Vec<_> = args.tracks.iter().copied().map(TrackId).collect();
        let from = Ticks(args.from_tick.unwrap_or(0));
        let to = args
            .to_tick
            .map(Ticks)
            .unwrap_or_else(|| session.project().end_tick());
        let window = Ticks(args.window_ticks.unwrap_or(Ticks::QUARTER.0));
        if from < Ticks::ZERO || to <= from || window <= Ticks::ZERO {
            return Err(
                "Use nonnegative from_tick, to_tick > from_tick and positive window_ticks".into(),
            );
        }
        let options = ChordOptions { window };
        let report = session
            .chord_analysis_job(&tracks, from, to, options)
            .and_then(|j| j.run(&analysis_control()))
            .map_err(|e| e.to_string())?;
        let response = reports::publish(&report)?;
        if args.apply {
            session
                .apply_chord_analysis(&report)
                .map_err(|e| e.to_string())?;
            save_checkpointed(&mut session)?;
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chord_report_writes_nothing_then_external_notes_are_accepted() {
        let root = std::env::temp_dir().join(format!(
            "auris-recognition-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let project = root.join("Song.auris");
        let mut s = Session::new(SessionOptions::headless()).unwrap();
        let track = s.add_default_instrument_track("Existing").unwrap();
        let clip = s
            .add_midi_clip(track, "Triad", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        for pitch in [60, 64, 67] {
            s.add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }
        s.save(&project).unwrap();
        let original = std::fs::read(&project).unwrap();
        let chord_args = analyze_chords::Args {
            project: project.display().to_string(),
            tracks: vec![track.0],
            from_tick: Some(13),
            to_tick: None,
            window_ticks: None,
            apply: false,
        };
        let chord_json: serde_json::Value =
            serde_json::from_str(&analyze_chords::run(&chord_args).unwrap()).unwrap();
        assert_eq!(
            chord_json["segments"][0]["reading"]["candidates"][0]["symbol"],
            "C"
        );
        assert_eq!(std::fs::read(&project).unwrap(), original);
        let accepted = crate::runtime::call_saved(
            &crate::ToolGroups::default(),
            "import_transcription",
            serde_json::json!({"project":project,"transcription":{"source":{"method":"external-fixture"},"seconds":1.0,"notes":[{"pitch":69,"start":0.2,"end":0.8,"strength":0.7}]},"at_tick":7680}),
        )
        .unwrap();
        let accepted: serde_json::Value = serde_json::from_str(&accepted).unwrap();
        assert_eq!(accepted["notes"], 1);
        let saved = opened(project.to_str().unwrap()).unwrap();
        assert_eq!(saved.project().tracks.len(), 2);
        assert_eq!(saved.project().midi_clip(clip).unwrap().1.notes.len(), 3);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
