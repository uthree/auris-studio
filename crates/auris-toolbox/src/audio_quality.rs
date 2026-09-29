//! Shared saved-project controls for measured loudness and encoded-output checks.

use super::*;

/// Adjust the whole mix toward a requested measured loudness.
pub mod normalize_mix {
    use super::*;

    /// Tool name.
    pub const NAME: &str = "normalize_mix";
    /// Model-facing instructions.
    pub const DESCRIPTION: &str = "Measures the whole mix, moves all source faders by one common dB offset toward target_lufs, measures again, and saves. Preserves their relative balance and master gain. A true-peak ceiling and fader limits can leave the mix short of the target; inspect the reported result. Refuses automated source gain.";

    /// Input to the measured adjustment.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Saved project path or project_id.
        pub project: String,
        /// Desired integrated loudness, -60 to -6 LUFS.
        #[schemars(range(min = -60, max = -6))]
        pub target_lufs: f32,
        /// Maximum true peak, -12 to 0 dBTP; defaults to -1.
        #[schemars(range(min = -12, max = 0))]
        pub ceiling_db: Option<f32>,
    }

    /// Apply and save one reversible adjustment.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let report = session
            .normalize_mix(args.target_lufs, args.ceiling_db.unwrap_or(-1.0))
            .map_err(|error| error.to_string())?;
        save_checkpointed(&mut session)?;
        Ok(format!(
            "Moved all source faders {:+.2} dB; mix {:.1} -> {:.1} LUFS, true peak {:.1} -> {:.1} dBTP. Target {:.1} LUFS, ceiling {:.1} dBTP. {} Saved. Render and verify the encoded WAV next.",
            report.offset_db,
            report.before.lufs.unwrap_or(f32::NEG_INFINITY),
            report.after.lufs.unwrap_or(f32::NEG_INFINITY),
            report.before.true_peak_db,
            report.after.true_peak_db,
            report.target_lufs,
            report.ceiling_db,
            if report
                .after
                .lufs
                .is_some_and(|value| (value - report.target_lufs).abs() <= 1.0)
                && report.after.true_peak_db <= report.ceiling_db + 0.1
            {
                "Measured target reached."
            } else {
                "Target not reached; inspect fader/peak limits."
            },
        ))
    }
}

/// Verify the audio stored in a WAV file after export.
pub mod verify_render {
    use super::*;

    /// Tool name.
    pub const NAME: &str = "verify_render";
    /// Model-facing instructions.
    pub const DESCRIPTION: &str = "Decodes the saved WAV file itself and reports sample rate, channels, bit depth, duration, integrated LUFS, true peak, full-scale samples, final 0.5 s RMS and SHA-256. Pass target_lufs and optional ending_rms_max_db to check the delivery target; omit ending limit for intentional loops or hard cuts. A failed check keeps the file for repair.";

    /// Requested checks on one exported WAV.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute path of the WAV file to check.
        pub output: String,
        /// Desired integrated loudness; omit to skip the loudness target check.
        #[schemars(range(min = -60, max = -6))]
        pub target_lufs: Option<f32>,
        /// Allowed absolute difference from target, in LU; defaults to 1.
        #[schemars(range(min = 0, max = 6))]
        pub lufs_tolerance: Option<f32>,
        /// Maximum true peak, in dBTP; defaults to -1.
        #[schemars(range(min = -12, max = 0))]
        pub ceiling_db: Option<f32>,
        /// Maximum final half-second RMS in dBFS for a fade ending; omit for loops.
        #[schemars(range(min = -120, max = 0))]
        pub ending_rms_max_db: Option<f32>,
    }

    /// Inspect a completed file and return its checks.
    pub fn run(args: &Args) -> Result<String, String> {
        let path = PathBuf::from(&args.output);
        if !path.is_absolute() {
            return Err("output must be an absolute WAV path".into());
        }
        let tolerance = args.lufs_tolerance.unwrap_or(1.0);
        let ceiling = args.ceiling_db.unwrap_or(-1.0);
        if !tolerance.is_finite() || !(0.0..=6.0).contains(&tolerance) {
            return Err("lufs_tolerance must be finite and 0..6 LU".into());
        }
        if !ceiling.is_finite() || !(-12.0..=0.0).contains(&ceiling) {
            return Err("ceiling_db must be finite and -12..0 dBTP".into());
        }
        if args
            .target_lufs
            .is_some_and(|value| !value.is_finite() || !(-60.0..=-6.0).contains(&value))
        {
            return Err("target_lufs must be finite and -60..-6 LUFS".into());
        }
        if args
            .ending_rms_max_db
            .is_some_and(|value| !value.is_finite() || !(-120.0..=0.0).contains(&value))
        {
            return Err("ending_rms_max_db must be finite and -120..0 dBFS".into());
        }
        let inspection = auris_session::inspect_export(&path).map_err(|e| e.to_string())?;
        Ok(report(
            &path,
            &inspection,
            args.target_lufs,
            tolerance,
            Some(ceiling),
            args.ending_rms_max_db,
        ))
    }
}

pub(super) fn report(
    path: &Path,
    inspection: &auris_session::ExportInspection,
    target_lufs: Option<f32>,
    tolerance: f32,
    ceiling_db: Option<f32>,
    ending_rms_max_db: Option<f32>,
) -> String {
    let mut failures = Vec::new();
    if inspection.full_scale_samples > 0 {
        failures.push(format!(
            "{} full-scale samples",
            inspection.full_scale_samples
        ));
    }
    if let Some(ceiling_db) = ceiling_db
        && inspection.true_peak_db > ceiling_db + 0.1
    {
        failures.push(format!("true peak exceeds {ceiling_db:.1} dBTP"));
    }
    if let Some(target) = target_lufs
        && !inspection
            .lufs
            .is_some_and(|actual| (actual - target).abs() <= tolerance)
    {
        failures.push(format!(
            "loudness differs from {target:.1} LUFS by more than {tolerance:.1} LU"
        ));
    }
    if let Some(limit) = ending_rms_max_db
        && inspection.ending_rms_db > limit
    {
        failures.push(format!("ending RMS exceeds {limit:.1} dBFS"));
    }
    let status = if failures.is_empty() {
        "PASS".into()
    } else {
        format!("FAIL: {}", failures.join("; "))
    };
    format!(
        "Encoded WAV {}: {status}. {} Hz, {} ch, {}-bit, {:.2} s, {}, peak {:.1} dBFS, true peak {:.1} dBTP, full-scale samples {}, final 0.5 s RMS {:.1} dBFS. SHA-256 {}.\n",
        path.display(),
        inspection.sample_rate,
        inspection.channels,
        inspection.bit_depth,
        inspection.seconds,
        lufs_text(inspection.lufs),
        inspection.peak_db,
        inspection.true_peak_db,
        inspection.full_scale_samples,
        inspection.ending_rms_db,
        inspection.sha256,
    )
}

pub(super) fn check_layout(
    inspection: &auris_session::ExportInspection,
    summary: &ExportSummary,
    settings: &WavExportSettings,
    sample_rate: f64,
) -> Result<(), String> {
    if inspection.frames != summary.frames
        || inspection.channels != summary.channels
        || inspection.bit_depth != settings.bit_depth.bits()
        || inspection.sample_rate != sample_rate.round() as u32
    {
        return Err(format!(
            "encoded WAV layout differs from export: {} frames/{} channels/{} bits/{} Hz on disk, expected {} frames/{} channels/{} bits/{:.0} Hz",
            inspection.frames,
            inspection.channels,
            inspection.bit_depth,
            inspection.sample_rate,
            summary.frames,
            summary.channels,
            settings.bit_depth.bits(),
            sample_rate,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_delivery_checks_distinguish_fade_from_loop() {
        let measured = auris_session::ExportInspection {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 24,
            seconds: 12.0,
            frames: 576_000,
            lufs: Some(-23.2),
            peak_db: -1.4,
            true_peak_db: -1.2,
            full_scale_samples: 0,
            ending_rms_db: -20.0,
            sha256: "abc".into(),
        };
        let path = Path::new("song.wav");
        assert!(report(path, &measured, Some(-23.0), 1.0, Some(-1.0), None).contains("PASS"));
        assert!(
            report(path, &measured, Some(-23.0), 1.0, Some(-1.0), Some(-60.0))
                .contains("FAIL: ending RMS exceeds")
        );
        let clipped = auris_session::ExportInspection {
            full_scale_samples: 4997,
            ..measured
        };
        assert!(
            report(path, &clipped, Some(-23.0), 1.0, Some(-1.0), None)
                .contains("FAIL: 4997 full-scale samples")
        );
    }

    #[test]
    fn encoded_layout_must_match_the_render_summary() {
        let measured = auris_session::ExportInspection {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 24,
            seconds: 1.0,
            frames: 48_000,
            lufs: Some(-23.0),
            peak_db: -1.0,
            true_peak_db: -0.9,
            full_scale_samples: 0,
            ending_rms_db: -60.0,
            sha256: "abc".into(),
        };
        let summary = ExportSummary {
            seconds: 1.0,
            frames: 48_000,
            channels: 2,
            peak_db: -1.0,
        };
        let settings = WavExportSettings::default();
        assert!(check_layout(&measured, &summary, &settings, 48_000.0).is_ok());
        let mismatch = auris_session::ExportInspection {
            frames: 47_999,
            ..measured
        };
        assert!(
            check_layout(&mismatch, &summary, &settings, 48_000.0)
                .unwrap_err()
                .contains("encoded WAV layout differs")
        );
    }
}
