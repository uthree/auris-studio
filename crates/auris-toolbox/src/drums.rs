//! Measuring a configured drum instrument and applying its computed note mapping.

use super::*;

/// Manual musical drum assignments, separate from acoustic measurements.
pub mod set_drum_assignment {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "set_drum_assignment";
    /// The model-facing description, shared by both tool frontends.
    pub const DESCRIPTION: &str = "Adds or changes one drum track role's MIDI note assignment, or removes it with remove: true. Roles are kick, snare, closed_hat, open_hat, crash and tom; note is 0-127. Requires an explicit drum track. Saves the assignment for the drum editor and future generation without rewriting existing clips or their saved recipes. Other assignments and instrument state are preserved; removing the final role leaves an explicitly empty map.";

    /// Arguments for adding, replacing or removing one musical assignment.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute path to the project.
        pub project: String,
        /// Drum track name or an `id:<number>` selector from describe.
        pub track: String,
        /// One of kick, snare, closed_hat, open_hat, crash or tom.
        pub role: String,
        /// MIDI address from 0 through 127. Required unless remove is true.
        pub note: Option<u8>,
        /// Remove the role's assignment; do not supply note together with this flag.
        #[serde(default)]
        pub remove: bool,
    }

    /// Applies one manual assignment and saves a checkpoint when it changed.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.note.is_some() == args.remove {
            return Err("pass note to assign a role, or remove: true to remove it".into());
        }
        if args.note.is_some_and(|note| note > 127) {
            return Err("note must be between 0 and 127".into());
        }
        let role: DrumRole = serde_json::from_value(serde_json::Value::String(args.role.clone()))
            .map_err(|_| {
            "role must be kick, snare, closed_hat, open_hat, crash or tom".to_string()
        })?;
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let changed = session
            .set_drum_assignment(track, role, args.note)
            .map_err(|error| error.to_string())?;
        if changed {
            session
                .save_with_checkpoint()
                .map_err(|error| error.to_string())?;
        }
        serde_json::to_string_pretty(&serde_json::json!({
            "track": track.0,
            "changed": changed,
            "assignments": session.drum_assignments(track),
        }))
        .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn malformed_manual_edits_are_refused_before_opening_any_project() {
            let mut args = Args {
                project: "/does/not/exist.auris".into(),
                track: "Kit".into(),
                role: "snare".into(),
                note: None,
                remove: false,
            };
            assert!(run(&args).unwrap_err().starts_with("pass note"));
            args.note = Some(38);
            args.remove = true;
            assert!(run(&args).unwrap_err().starts_with("pass note"));
            args.remove = false;
            args.note = Some(128);
            assert!(run(&args).unwrap_err().starts_with("note must"));
            args.note = Some(38);
            args.role = "melody".into();
            assert!(run(&args).unwrap_err().starts_with("role must"));
        }
    }
}

/// Acoustic drum analysis without name or General MIDI priors.
pub mod analyze_drum_kit {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "analyze_drum_kit";
    /// The model-facing description, shared by both tool frontends.
    pub const DESCRIPTION: &str = "Renders a drum track's instrument at several velocities and reports spectral and envelope measurements, role-fit scores and proposed drum mappings. Requires kind drum; melodic tracks are refused. Classification uses audio only, never note names or GM numbers; fit scores are not probabilities and missing roles are allowed. With apply, saves the computed map; remap_clips also retargets generated drum notes and their recipes. Existing notes are unchanged by analysis alone.";

    /// A project instrument to measure.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute path to the project.
        pub project: String,
        /// Drum track name or an `id:<number>` selector from describe.
        pub track: String,
        /// First MIDI key to probe, 0-127; defaults to 0. Not a classification hint.
        pub first_note: Option<u8>,
        /// Last MIDI key to probe, inclusive, 0-127; defaults to 127.
        pub last_note: Option<u8>,
        /// Save the computed mapping for future generation.
        #[serde(default)]
        pub apply: bool,
        /// Also retarget existing generated drum notes. Requires apply.
        #[serde(default)]
        pub remap_clips: bool,
        /// Include every trigger's measurements. Default summarizes audible notes and ranges.
        #[serde(default)]
        pub include_triggers: bool,
    }

    /// Probes a private instrument instance and optionally saves the chosen mapping.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.remap_clips && !args.apply {
            return Err("remap_clips requires apply".into());
        }
        let first = args.first_note.unwrap_or(0);
        let last = args.last_note.unwrap_or(127);
        if first > last || last > 127 {
            return Err("note range must be within 0-127, with first_note <= last_note".into());
        }
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let options = auris_session::DrumScanOptions {
            notes: (first..=last).collect(),
            ..Default::default()
        };
        let request = session
            .drum_probe_request(track, &options)
            .map_err(|error| error.to_string())?;
        let report = auris_session::run_drum_probe_isolated(
            &request,
            &std::sync::atomic::AtomicBool::new(false),
            std::time::Duration::from_secs(600),
        )
        .map_err(|error| error.to_string())?;
        if args.apply {
            session
                .apply_drum_map(&report, args.remap_clips)
                .map_err(|error| error.to_string())?;
            session
                .save_with_checkpoint()
                .map_err(|error| error.to_string())?;
        }
        if args.include_triggers {
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
        } else {
            serde_json::to_string_pretty(&summary(&report)).map_err(|error| error.to_string())
        }
    }

    fn summary(report: &auris_session::DrumKitAnalysis) -> serde_json::Value {
        let voices: Vec<_> = report
            .voices
            .iter()
            .filter(|voice| !voice.silent)
            .map(|voice| {
                let range = |feature: fn(&auris_session::DrumProbeSample) -> f64| {
                    voice
                        .samples
                        .iter()
                        .map(feature)
                        .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], value| {
                            [lo.min(value), hi.max(value)]
                        })
                };
                serde_json::json!({
                    "note": voice.note,
                    "fitness": voice.fitness,
                    "fitness_variation": voice.fitness_variation,
                    "measured_ranges": {
                        "centroid_hz": range(|s| s.acoustics.spectrum.centroid_hz),
                        "energy_duration_seconds": range(|s| s.acoustics.energy_duration_seconds),
                        "low_energy": range(|s| s.acoustics.spectrum.low),
                        "body_energy": range(|s| s.acoustics.spectrum.body),
                        "high_energy": range(|s| s.acoustics.spectrum.high),
                        "rms": range(|s| s.acoustics.rms),
                    }
                })
            })
            .collect();
        let silent: Vec<_> = report
            .voices
            .iter()
            .filter(|voice| voice.silent)
            .map(|voice| voice.note)
            .collect();
        serde_json::json!({
            "track": report.track,
            "source_fingerprint": report.source_fingerprint,
            "sample_rate": report.sample_rate,
            "bpm": report.bpm,
            "options": report.options,
            "proposed_map": report.proposed_map,
            "voices": voices,
            "silent_notes": silent,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn compact_report_keeps_evidence_ranges_and_explicit_silence() {
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let track = session
                .add_drum_track("Unknown", "auris.synth.drumkit")
                .unwrap();
            let report = session
                .analyze_drum_kit(
                    track,
                    &auris_session::DrumScanOptions {
                        notes: vec![0, 36],
                        velocities: vec![1.0],
                        repetitions: 1,
                        seconds_per_note: 0.5,
                        ..Default::default()
                    },
                )
                .unwrap();
            let compact = summary(&report);
            assert_eq!(compact["silent_notes"], serde_json::json!([0]));
            assert_eq!(compact["voices"].as_array().unwrap().len(), 1);
            assert_eq!(compact["voices"][0]["note"], 36);
            assert!(compact["voices"][0]["samples"].is_null());
            assert_eq!(
                compact["proposed_map"],
                serde_json::json!(report.proposed_map)
            );
            assert!(
                compact["voices"][0]["measured_ranges"]["rms"][0]
                    .as_f64()
                    .unwrap()
                    > 0.0
            );
        }
    }
}
