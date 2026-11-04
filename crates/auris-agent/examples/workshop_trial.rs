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
//!
//! A phased round is tried by overriding the logged direction: `AURIS_TRIAL_PHASE=score`
//! composes it as a demo, and `AURIS_TRIAL_PHASE=finish` with `AURIS_TRIAL_PARENT` set to an
//! earlier demo's output folder finishes that demo.

use std::path::PathBuf;

use auris_agent::workshop::compose_trial;
use auris_session::Settings;
use auris_session::workshop::{Candidate, Document, RoundPhase};

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
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
