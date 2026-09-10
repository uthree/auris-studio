//! Match an existing project's rendered sound to a reference excerpt, retaining A/B audio.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use auris_io::{WavBitDepth, WavExportSettings, write_wav};
use auris_session::audio_evaluation::ReferenceAudioEvaluator;
use auris_session::{
    ReferenceMatchSettings, ReferenceMatchStep, Session, SessionOptions, decode_audio,
};

const USAGE: &str = "usage: match_reference <project.auris> <reference.wav> <new-output-directory> [attempts] [seconds]";

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if !(3..=5).contains(&args.len()) {
        return Err(USAGE.into());
    }
    let output = PathBuf::from(&args[2]);
    if output.exists() {
        return Err("output directory already exists; choose a new directory".into());
    }
    let settings = ReferenceMatchSettings {
        attempts: args.get(3).map_or(Ok(32), |text| text.parse())?,
        duration_seconds: args.get(4).map_or(Ok(12.0), |text| text.parse())?,
        ..Default::default()
    };
    let audio = decode_audio(Path::new(&args[1]), 44_100.0)?;
    let end = ((settings.duration_seconds * audio.sample_rate()).round() as usize)
        .min(audio.frame_count());
    let reference = auris_core::AudioBuffer::from_planar(
        audio
            .iter_channels()
            .map(|channel| channel[..end].to_vec())
            .collect(),
        audio.sample_rate(),
    )?;
    let evaluator = Arc::new(ReferenceAudioEvaluator::new(&reference)?);
    let mut session = Session::new(SessionOptions::headless())?;
    let missing = session.open(Path::new(&args[0]))?;
    if !missing.is_empty() {
        return Err(format!("project assets are missing: {missing:?}").into());
    }
    let mut job = session.begin_reference_match(settings.clone(), evaluator)?;
    let cancel = AtomicBool::new(false);
    let report = loop {
        let progress = job.progress();
        eprintln!(
            "render/evaluate {} / {}",
            progress.completed + 1,
            progress.total
        );
        let result = job.run(&cancel, &mut |_| {})?;
        match session.continue_reference_match(result)? {
            ReferenceMatchStep::Pending(next) => job = next,
            ReferenceMatchStep::Complete(report) => break report,
        }
    };
    std::fs::create_dir(&output)?;
    let wav = WavExportSettings {
        sample_rate: 44_100,
        bit_depth: WavBitDepth::Float32,
        dither: false,
    };
    write_wav(&output.join("reference.wav"), &reference, &wav)?;
    write_wav(&output.join("before.wav"), &report.baseline_audio, &wav)?;
    write_wav(&output.join("best.wav"), &report.best_audio, &wav)?;
    let metrics = |evaluation: &auris_session::audio_evaluation::AudioEvaluation| {
        evaluation
            .metrics
            .iter()
            .map(|metric| serde_json::json!({"name":metric.name,"value":metric.value}))
            .collect::<Vec<_>>()
    };
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "package_version": env!("CARGO_PKG_VERSION"),
            "attempts": report.attempts,
            "failed_attempts": report.failed_attempts,
            "scopes": {
                "mix": settings.mix,
                "performance": settings.performance,
                "generation_seeds": settings.generation_seeds,
                "instruments": settings.instruments,
                "arrangement": settings.arrangement,
            },
            "cancelled": report.cancelled,
            "duration_seconds": settings.duration_seconds,
            "search_seed": settings.seed,
            "before_fitness": report.baseline.fitness,
            "best_fitness": report.best.fitness,
            "before_metrics": metrics(&report.baseline),
            "best_metrics": metrics(&report.best),
            "changes": report.changes,
        }))?,
    )?;
    let changed = session.apply_reference_match(&report)?;
    let saved = session.save_as(&output.join("Matched.auris"))?;
    println!(
        "fitness {:.6} -> {:.6}; applied: {changed}; saved: {saved:?}",
        report.baseline.fitness, report.best.fitness
    );
    Ok(())
}
