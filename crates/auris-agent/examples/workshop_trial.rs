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
//! A candidate that continued a parent continues the same parent here. Set
//! `AURIS_COMPOSITION_WORKFLOWS=1` to try the phased profile.

use std::path::PathBuf;

use auris_agent::workshop::compose_trial;
use auris_session::Settings;
use auris_session::workshop::{Candidate, Document};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: workshop_trial <thread folder> <candidate id> <output folder>";
    let thread = PathBuf::from(args.next().ok_or(usage)?);
    let id: u64 = args.next().ok_or(usage)?.parse()?;
    let output = PathBuf::from(args.next().ok_or(usage)?);
    if output.exists() {
        return Err(format!("{} exists; choose a new folder", output.display()).into());
    }
    let document: Document =
        serde_json::from_slice(&std::fs::read(thread.join("conversation.json"))?)?;
    let candidate: &Candidate = document
        .rounds
        .iter()
        .flat_map(|round| &round.candidates)
        .find(|candidate| candidate.id == id)
        .ok_or("no such candidate in the thread")?;
    let parent = candidate
        .parent
        .map(|parent| thread.join(parent.to_string()).join("song/song.auris"));
    let settings = Settings::load();
    let prefs = settings.agent.clone();
    eprintln!(
        "composing \"{}\" with {} {} ({} profile)",
        candidate.direction.title,
        prefs.backend,
        prefs.model,
        if auris_toolbox::composition_workflows::enabled() {
            "phased"
        } else {
            "legacy"
        }
    );
    let started = std::time::Instant::now();
    let result = compose_trial(
        candidate.direction.clone(),
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
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
