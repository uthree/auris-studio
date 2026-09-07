//! CPU music analysis command-line parsing and presentation.

use super::*;
use auris_session::{AnalysisControl, AudioOptions, ChordOptions};
use std::collections::BTreeMap;

fn beat(value: &str) -> Result<Ticks, String> {
    let n: f64 = value
        .parse()
        .map_err(|_| Key::AnalysisInvalidBeat.get(LANGUAGE))?;
    if !n.is_finite() || !(0.0..=1_000_000.0).contains(&n) {
        return Err(Key::AnalysisInvalidBeat.get(LANGUAGE).into());
    }
    Ok(Ticks::from_beats(n))
}

pub(super) fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("");
    let usage = match command {
        "analyze-chords" => Key::CliAnalyzeChordsUsage,
        "analyze-audio" => Key::CliAnalyzeAudioUsage,
        "analyze-instruments" => Key::CliAnalyzeInstrumentsUsage,
        "transcribe-mixture" => Key::CliTranscribeMixtureUsage,
        _ => Key::CliTranscribeAudioUsage,
    }
    .get(LANGUAGE);
    let path = args.get(1).ok_or(usage)?;
    let mut options = BTreeMap::new();
    let mut at = 2;
    while at < args.len() {
        let key = args[at].as_str();
        let allowed = match command {
            "analyze-chords" => [
                "--track",
                "--from-beat",
                "--to-beat",
                "--window-beats",
                "--apply",
            ]
            .contains(&key),
            "analyze-audio" => false,
            "analyze-instruments" => ["--model", "--threshold"].contains(&key),
            "transcribe-mixture" => [
                "--model",
                "--python",
                "--acknowledge-noncommercial",
                "--midi",
                "--project",
                "--apply",
                "--at-beat",
            ]
            .contains(&key),
            _ => ["--midi", "--project", "--at-beat", "--name", "--apply"].contains(&key),
        };
        if !allowed || options.contains_key(key) {
            return Err(usage.into());
        }
        let value = if matches!(key, "--apply" | "--acknowledge-noncommercial") {
            ""
        } else {
            at += 1;
            args.get(at).ok_or(usage)?.as_str()
        };
        options.insert(key, value);
        at += 1;
    }
    let control = AnalysisControl::default();
    let value = if command == "transcribe-mixture" {
        if !options.contains_key("--acknowledge-noncommercial") {
            return Err(Key::MuscriptorWarning.get(LANGUAGE).into());
        }
        if options.contains_key("--project") != options.contains_key("--apply") {
            return Err(usage.into());
        }
        if options.get("--midi").is_some_and(|p| Path::new(p).exists()) {
            return Err(Key::AnalysisMidiExists.get(LANGUAGE).into());
        }
        let start = beat(options.get("--at-beat").copied().unwrap_or("0"))?;
        let config = auris_session::MixtureOptions {
            python: (*options.get("--python").ok_or(usage)?).into(),
            model: (*options.get("--model").ok_or(usage)?).into(),
            acknowledge_noncommercial: true,
        };
        eprintln!("{}", Key::MuscriptorWarning.get(LANGUAGE));
        let report = auris_session::transcribe_mixture_file(Path::new(path), &config, &control)
            .map_err(|e| e.to_string())?;
        if let Some(path) = options.get("--midi") {
            let mut output = Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
            output
                .create_mixture_tracks(&report, Ticks::ZERO)
                .map_err(|e| e.to_string())?;
            output
                .export_midi(Path::new(path))
                .map_err(|e| e.to_string())?;
        }
        if let Some(path) = options.get("--project") {
            let mut output = Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
            output.open(Path::new(path)).map_err(|e| e.to_string())?;
            output
                .create_mixture_tracks(&report, start)
                .map_err(|e| e.to_string())?;
            output.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        serde_json::to_value(report).map_err(|e| e.to_string())?
    } else if command == "analyze-instruments" {
        let model = options.get("--model").ok_or(usage)?;
        let threshold = options
            .get("--threshold")
            .copied()
            .unwrap_or("0.2")
            .parse::<f32>()
            .map_err(|_| usage)?;
        let report = auris_session::analyze_instrument_file(
            Path::new(path),
            Path::new(model),
            threshold,
            &control,
        )
        .map_err(|e| e.to_string())?;
        serde_json::to_value(report).map_err(|e| e.to_string())?
    } else if command == "analyze-chords" {
        let mut session = Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
        session.open(Path::new(path)).map_err(|e| e.to_string())?;
        let mut tracks = Vec::new();
        if let Some(name) = options.get("--track") {
            let matching: Vec<_> = session
                .project()
                .tracks
                .iter()
                .filter(|t| {
                    name.strip_prefix("id:")
                        .and_then(|id| id.parse::<u64>().ok())
                        .map_or(t.name == *name, |id| t.id.0 == id)
                })
                .map(|t| t.id)
                .collect();
            if matching.len() != 1 {
                return Err(Key::AnalysisSelectTrack.get(LANGUAGE).into());
            }
            tracks = matching;
        }
        let from = beat(options.get("--from-beat").copied().unwrap_or("0"))?;
        let to = options
            .get("--to-beat")
            .map(|s| beat(s))
            .transpose()?
            .unwrap_or_else(|| session.project().end_tick());
        let window = beat(options.get("--window-beats").copied().unwrap_or("1"))?;
        let report = session
            .chord_analysis_job(&tracks, from, to, ChordOptions { window })
            .and_then(|j| j.run(&control))
            .map_err(|e| e.to_string())?;
        if options.contains_key("--apply") {
            session
                .apply_chord_analysis(&report)
                .map_err(|e| e.to_string())?;
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        serde_json::to_value(report).map_err(|e| e.to_string())?
    } else {
        if options.contains_key("--project") != options.contains_key("--apply") {
            return Err(usage.into());
        }
        if options.get("--midi").is_some_and(|p| Path::new(p).exists()) {
            return Err(Key::AnalysisMidiExists.get(LANGUAGE).into());
        }
        let start = beat(options.get("--at-beat").copied().unwrap_or("0"))?;
        let report = auris_session::analyze_audio_file(
            Path::new(path),
            AudioOptions {
                transcribe: command == "transcribe-audio",
            },
            &control,
        )
        .map_err(|e| e.to_string())?;
        let name = options
            .get("--name")
            .copied()
            .unwrap_or(Key::AnalysisDraft.get(LANGUAGE));
        if let Some(path) = options.get("--midi") {
            let mut output = Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
            output
                .create_transcription_track(&report, Ticks::ZERO, name)
                .map_err(|e| e.to_string())?;
            output
                .export_midi(Path::new(path))
                .map_err(|e| e.to_string())?;
        }
        if let Some(path) = options.get("--project") {
            let mut session =
                Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
            session.open(Path::new(path)).map_err(|e| e.to_string())?;
            session
                .create_transcription_track(&report, start, name)
                .map_err(|e| e.to_string())?;
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        serde_json::to_value(report).map_err(|e| e.to_string())?
    };
    let json = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    printed(writeln!(std::io::stdout(), "{json}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixture_requires_explicit_per_invocation_acknowledgement() {
        let args = [
            "transcribe-mixture",
            "absent.wav",
            "--python",
            "absent",
            "--model",
            "absent",
        ];
        assert!(
            run(&args.into_iter().map(str::to_string).collect::<Vec<_>>())
                .unwrap_err()
                .contains("CC BY-NC 4.0")
        );
    }
    #[test]
    fn malformed_options_fail_before_opening_audio() {
        for args in [
            vec!["analyze-audio", "absent.wav", "--apply"],
            vec![
                "transcribe-audio",
                "absent.wav",
                "--project",
                "absent.auris",
            ],
            vec!["analyze-chords", "absent.auris", "--track"],
        ] {
            assert!(run(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
        }
        assert!(beat("NaN").is_err());
        assert!(beat("-1").is_err());
    }
}
