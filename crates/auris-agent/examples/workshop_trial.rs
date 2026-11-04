//! Composes a logged workshop candidate's direction again, headless, for comparing the new run
//! with the logged one.
//!
//! ```text
//! cargo run --release -p auris-agent --example workshop_trial -- <thread folder> <candidate id> <output folder>
//! ```
//!
//! The thread folder is one under `<config>/workshop/threads/`. The direction is read from its
//! `conversation.json`, the composer is the one in the user's settings, and the output folder
//! receives `composer-log.jsonl` and `song/song.auris` exactly as a workshop candidate would.
//! A candidate that continued a parent continues the same parent here.
//!
//! A phased round is tried by overriding the logged direction: `AURIS_TRIAL_PHASE=score`
//! composes it as a demo, and `AURIS_TRIAL_PHASE=finish` with `AURIS_TRIAL_PARENT` set to an
//! earlier demo's output folder finishes that demo.
//!
//! Another composer is tried without touching the settings: `AURIS_TRIAL_BACKEND` and
//! `AURIS_TRIAL_MODEL` replace the configured backend and model, and `AURIS_TRIAL_FAST=0` or
//! `1` the faster service tier. The other agent preferences, effort among them, are kept.
//!
//! A finished run writes `lint.json`, what the score linter still finds in the song.
//! `workshop_trial --lint <song.auris>...` prints the same for songs made earlier.

use std::path::PathBuf;

use auris_agent::workshop::compose_trial;
use auris_session::lint::Severity;
use auris_session::workshop::{Candidate, Document, RoundPhase};
use auris_session::{Session, SessionOptions, Settings};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("--lint") {
        for song in args.skip(1) {
            println!("{song}\t{}", score_lint(std::path::Path::new(&song))?);
        }
        return Ok(());
    }
    let usage = "usage: workshop_trial <thread folder> <candidate id> <output folder>";
    let thread = PathBuf::from(args.next().ok_or(usage)?);
    let id: u64 = args.next().ok_or(usage)?.parse()?;
    let output = PathBuf::from(args.next().ok_or(usage)?);
    if output.exists() {
        return Err(format!("{} exists; choose a new folder", output.display()).into());
    }
    let document: Document =
        serde_json::from_slice(&std::fs::read(thread.join("conversation.json"))?)?;
    let (round, candidate): (_, &Candidate) = document
        .rounds
        .iter()
        .find_map(|round| {
            round
                .candidates
                .iter()
                .find(|candidate| candidate.id == id)
                .map(|candidate| (round, candidate))
        })
        .ok_or("no such candidate in the thread")?;
    // The conversation as it stood when the director planned this round.
    let mut planned = document.clone();
    planned.messages.truncate(round.after_message);
    planned
        .rounds
        .retain(|earlier| earlier.after_message < round.after_message);
    let mut direction = candidate.direction.clone();
    let mut parent = candidate
        .parent
        .map(|parent| thread.join(parent.to_string()).join("song/song.auris"));
    match std::env::var("AURIS_TRIAL_PHASE").as_deref() {
        Ok("score") => direction.phase = Some(RoundPhase::Score),
        Ok("finish") => {
            direction.phase = Some(RoundPhase::Finish);
            let demo = std::env::var("AURIS_TRIAL_PARENT")
                .map_err(|_| "AURIS_TRIAL_PHASE=finish needs AURIS_TRIAL_PARENT")?;
            parent = Some(PathBuf::from(demo).join("song/song.auris"));
        }
        Ok(other) => return Err(format!("unknown AURIS_TRIAL_PHASE {other}").into()),
        Err(_) => {}
    }
    let settings = Settings::load();
    let mut prefs = settings.agent.clone();
    if let Ok(backend) = std::env::var("AURIS_TRIAL_BACKEND") {
        prefs.backend = backend;
    }
    if let Ok(model) = std::env::var("AURIS_TRIAL_MODEL") {
        prefs.model = model;
    }
    if let Ok(fast) = std::env::var("AURIS_TRIAL_FAST") {
        prefs.fast_mode = fast == "1";
    }
    eprintln!(
        "composing \"{}\" with {} {}",
        candidate.direction.title, prefs.backend, prefs.model
    );
    let started = std::time::Instant::now();
    let result = compose_trial(
        direction,
        &planned,
        prefs,
        settings,
        output.clone(),
        parent,
        |line| eprintln!("[{:>5.0}s] {line}", started.elapsed().as_secs_f64()),
    );
    match result {
        Ok(_) => {
            eprintln!(
                "finished in {:.0}s: {}",
                started.elapsed().as_secs_f64(),
                output.display()
            );
            let lint = score_lint(&output.join("song/song.auris"))?;
            eprintln!("score lint: {lint}");
            std::fs::write(output.join("lint.json"), lint.to_string())?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// What the score linter still finds in the finished song: counts by severity and by rule,
/// for comparing takes. The audio rules need a render and are left to the agent's own pass.
fn score_lint(song: &std::path::Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut session = Session::new(SessionOptions::headless())?;
    session.open(song)?;
    let report = session.lint_score();
    let mut by_rule = serde_json::Map::new();
    let (mut errors, mut warnings, mut infos) = (0, 0, 0);
    for diagnostic in &report.diagnostics {
        match diagnostic.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
            Severity::Info => infos += 1,
        }
        let count = by_rule
            .entry(diagnostic.rule.name())
            .or_insert(serde_json::json!(0));
        *count = serde_json::json!(count.as_u64().unwrap_or(0) + 1);
    }
    Ok(serde_json::json!({
        "errors": errors,
        "warnings": warnings,
        "infos": infos,
        "suppressed": report.suppressed,
        "by_rule": by_rule,
    }))
}
