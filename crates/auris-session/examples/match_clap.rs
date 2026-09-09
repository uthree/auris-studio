//! Evaluate actual renders against a CLAP text prompt or reference and retain the winning PCM.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use auris_io::{WavBitDepth, WavExportSettings, write_wav};
use auris_session::audio_evaluation::AudioEvaluator;
use auris_session::clap_evaluation::{ClapAudioEvaluator, ClapTarget};
use auris_session::{
    ReferenceMatchSettings, ReferenceMatchStep, Session, SessionOptions, decode_audio,
};

const USAGE: &str = "usage: match_clap <project.auris> <model-directory> <text|audio> <prompt|reference.wav> <new-output-directory> [attempts] [seconds]";

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if !(5..=7).contains(&args.len()) {
        return Err(USAGE.into());
    }
    let output = PathBuf::from(&args[4]);
    if output.exists() {
        return Err("output directory already exists; choose a new directory".into());
    }
    let settings = ReferenceMatchSettings {
        attempts: args.get(5).map_or(Ok(8), |text| text.parse())?,
        duration_seconds: args.get(6).map_or(Ok(10.0), |text| text.parse())?,
        ..Default::default()
    };
    if !settings.duration_seconds.is_finite() || !(1.0..=30.0).contains(&settings.duration_seconds)
    {
        return Err("duration must be between 1 and 30 seconds".into());
    }
    let target = match args[2].as_str() {
        "text" => ClapTarget::Text(args[3].clone()),
        "audio" => {
            let audio = decode_audio(Path::new(&args[3]), 44_100.0)?;
            let end = ((settings.duration_seconds * audio.sample_rate()).round() as usize)
                .min(audio.frame_count());
            ClapTarget::Audio(Arc::new(auris_core::AudioBuffer::from_planar(
                audio
                    .iter_channels()
                    .map(|channel| channel[..end].to_vec())
                    .collect(),
                audio.sample_rate(),
            )?))
        }
        _ => return Err(USAGE.into()),
    };
    let reference = match &target {
        ClapTarget::Audio(audio) => Some(Arc::clone(audio)),
        ClapTarget::Text(_) => None,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    eprintln!("Loading CLAP and embedding the fixed target...");
    let evaluator = Arc::new(ClapAudioEvaluator::load(
        Path::new(&args[1]),
        target,
        Arc::clone(&cancel),
    )?);
    let objective = evaluator.description();
    let mut session = Session::new(SessionOptions::headless())?;
    let missing = session.open(Path::new(&args[0]))?;
    if !missing.is_empty() {
        return Err(format!("project assets are missing: {missing:?}").into());
    }
    let mut job = session.begin_reference_match(settings.clone(), evaluator.clone())?;
    let report = loop {
        let progress = job.progress();
        eprintln!(
            "render/CLAP {} / {}",
            progress.completed + 1,
            progress.total
        );
        let result = job.run(&cancel, &mut |_| {})?;
        match session.continue_reference_match(result)? {
            ReferenceMatchStep::Pending(next) => job = next,
            ReferenceMatchStep::Complete(report) => break report,
        }
    };
    // Re-evaluate the retained PCM, not a new composition/render, to verify result provenance.
    if evaluator.evaluate(&report.best_audio)? != report.best {
        return Err("retained PCM did not reproduce its recorded CLAP evaluation".into());
    }
    std::fs::create_dir(&output)?;
    let wav = WavExportSettings {
        sample_rate: 44_100,
        bit_depth: WavBitDepth::Float32,
        dither: false,
    };
    write_wav(&output.join("before.wav"), &report.baseline_audio, &wav)?;
    write_wav(&output.join("best.wav"), &report.best_audio, &wav)?;
    if let Some(reference) = &reference {
        write_wav(&output.join("reference.wav"), reference, &wav)?;
    }
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "package_version": env!("CARGO_PKG_VERSION"),
            "objective": objective,
            "target_mode": args[2],
            "reference_duration_seconds": reference.as_ref().map(|audio| audio.duration_seconds()),
            "attempts": report.attempts,
            "duration_seconds": settings.duration_seconds,
            "search_seed": settings.seed,
            "before_cosine": report.baseline.metrics[0].value,
            "best_cosine": report.best.metrics[0].value,
            "before_fitness": report.baseline.fitness,
            "best_fitness": report.best.fitness,
            "changes": report.changes,
        }))?,
    )?;
    let changed = session.apply_reference_match(&report)?;
    let saved = session.save_as(&output.join("Matched.auris"))?;
    println!(
        "cosine {:.6} -> {:.6}; applied: {changed}; saved: {saved:?}",
        report.baseline.fitness, report.best.fitness
    );
    Ok(())
}
