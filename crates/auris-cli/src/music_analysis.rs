//! Chord recognition from written notes: command-line parsing and presentation.

use super::*;
use auris_session::{AnalysisControl, ChordOptions};
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
    let usage = Key::CliAnalyzeChordsUsage.get(LANGUAGE);
    let path = args.get(1).ok_or(usage)?;
    let mut options = BTreeMap::new();
    let mut at = 2;
    while at < args.len() {
        let key = args[at].as_str();
        let allowed = [
            "--track",
            "--from-beat",
            "--to-beat",
            "--window-beats",
            "--apply",
        ]
        .contains(&key);
        if !allowed || options.contains_key(key) {
            return Err(usage.into());
        }
        let value = if key == "--apply" {
            ""
        } else {
            at += 1;
            args.get(at).ok_or(usage)?.as_str()
        };
        options.insert(key, value);
        at += 1;
    }
    let control = AnalysisControl::default();
    let value = {
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
    };
    let json = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    printed(writeln!(std::io::stdout(), "{json}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_options_fail_before_opening_audio() {
        for args in [
            vec!["analyze-chords", "absent.auris", "--model", "x"],
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
