//! Writes rule-only bands as saved projects, for comparing two builds of the phrase writers.
//!
//! ```text
//! cargo run -p auris-session --example band_takes -- <output folder>
//! ```
//!
//! Each project is a lead over a named progression, and chords, a bass, an arpeggio and a kit
//! written round it by `Session::accompany`, balanced the way a composed song is. No model is
//! involved, so two builds given the same seeds differ only by what their writers write.

use std::path::PathBuf;

use auris_core::theory::chart::Chart;
use auris_core::time::Ticks;
use auris_core::{ClipPreset, ClipRecipe};
use auris_session::{Session, SessionOptions};

const BARS: usize = 32;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).ok_or("an output folder")?);
    std::fs::create_dir_all(&out)?;
    let charts = [
        ("axis", "@axis"),
        ("royal-road", "@royal-road"),
        ("marusa", "@marusa"),
        ("koakuma", "@koakuma"),
        ("canon", "@canon"),
        ("komuro", "@komuro"),
    ];
    for (name, chart) in charts {
        for seed in [1u64, 2] {
            let mut session = Session::new(
                SessionOptions::headless()
                    .with_shipped_fonts(true)
                    .with_master_limiter(true),
            )?;
            let track = session.add_default_instrument_track("Lead")?;
            let chart = Chart::parse(chart).ok_or("a chart")?;
            session.stamp_progression(&chart, Ticks::ZERO, BARS);
            let length = Ticks(3840 * BARS as i64);
            let lead = session.generate_clip(
                track,
                Ticks::ZERO,
                length,
                ClipRecipe::new(ClipPreset::Lead, seed),
            )?;
            let report = session.accompany(
                lead,
                &[
                    ClipPreset::Chords,
                    ClipPreset::Bass,
                    ClipPreset::Arp,
                    ClipPreset::Drums,
                ],
                seed + 100,
            )?;
            session.balance_levels()?;
            let path = out.join(format!("{name}-{seed}.auris"));
            session.save_as(&path)?;
            let mut rules = std::collections::BTreeMap::<String, usize>::new();
            for diagnostic in session.lint_score().diagnostics {
                *rules.entry(format!("{:?}", diagnostic.rule)).or_default() += 1;
            }
            println!("{name}-{seed} lint: {rules:?}");
            println!(
                "{name}-{seed}: {} bars, {} chords, {} notes in {} parts{}",
                report.bars,
                report.chords,
                report.notes,
                report.parts.len(),
                if report.substituted {
                    " (a sound was substituted)"
                } else {
                    ""
                }
            );
        }
    }
    Ok(())
}
