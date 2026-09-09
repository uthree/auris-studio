//! Make a self-contained baseline project and an achievable rendered reference for matching.

use std::error::Error;
use std::path::Path;

use auris_core::{ClipPreset, ClipRecipe, Note, NoteTransform, ParamTarget, Ticks};
use auris_engine::{OfflineOptions, RenderProgress};
use auris_io::{WavBitDepth, WavExportSettings};
use auris_session::{Session, SessionOptions};

const USAGE: &str = "usage: reference_match_demo <new-output-directory>";
const SECONDS: f64 = 12.0;
const RATE: f64 = 44_100.0;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.len() != 1 {
        return Err(USAGE.into());
    }
    let output = std::path::absolute(Path::new(&args[0]))?;
    // Refuse an existing directory, including an empty one, so fixtures never overwrite work.
    std::fs::create_dir(&output)?;
    let mut session = Session::new(SessionOptions::headless())?;
    session.set_tempo_at(Ticks::ZERO, 120.0);
    let track = session.add_default_instrument_track("Reference matching demo")?;
    let clip = session.add_midi_clip(
        track,
        "Written phrase",
        Ticks::ZERO,
        Ticks::from_beats(24.0),
    )?;
    for beat in 0..24 {
        let pitch = [60, 67, 64, 69, 65, 72, 67, 64][beat % 8];
        session.add_note(
            clip,
            Note::new(pitch, Ticks::from_beats(beat as f64), Ticks::QUARTER),
        )?;
    }
    session.set_param(ParamTarget::TrackGain(track), -12.0);
    let original_transforms = vec![NoteTransform::Humanize {
        amount: 0.12,
        seed: 981,
    }];
    session.set_clip_transforms(clip, original_transforms.clone())?;

    // Keep a live recipe alongside the authored melody so all five search families can be
    // exercised, including take proposals that must never rewrite the authored phrase.
    let drums = session.add_drum_track("Generated hats", "auris.synth.noisedrum")?;
    let mut recipe = ClipRecipe::new(ClipPreset::Hat, 42);
    recipe.drum_note = Some(42);
    let hats = session.generate_clip(drums, Ticks::ZERO, Ticks::from_beats(24.0), recipe)?;
    session.set_clip_transforms(
        hats,
        vec![NoteTransform::Humanize {
            amount: 0.2,
            seed: 42,
        }],
    )?;
    session.set_param(ParamTarget::TrackGain(drums), -18.0);

    let options = OfflineOptions {
        sample_rate: Some(RATE),
        start_frames: 0,
        end_frames: Some((SECONDS * RATE) as u64),
        include_tail: false,
        block_frames: session.engine().max_block(),
        ..OfflineOptions::default()
    };
    let wav = WavExportSettings {
        sample_rate: RATE as u32,
        bit_depth: WavBitDepth::Float32,
        dither: false,
    };
    let baseline = session.save_as(&output.join("Baseline.auris"))?;
    session.render_job().render_to_wav(
        &output.join("baseline.wav"),
        &wav,
        &options,
        &mut RenderProgress::default(),
    )?;

    // These are within the optimizer's original-relative bounds. The score and private seed
    // stay exact; only stereo placement and how long its notes are held change.
    session.set_param(ParamTarget::TrackPan(track), 0.15);
    let mut target_transforms = original_transforms;
    target_transforms.push(NoteTransform::Gate { amount: 0.95 });
    session.set_clip_transforms(clip, target_transforms)?;
    let target = session.save_as(&output.join("ReferenceTarget.auris"))?;
    let reference_path = output.join("reference.wav");
    session.render_job().render_to_wav(
        &reference_path,
        &wav,
        &options,
        &mut RenderProgress::default(),
    )?;
    std::fs::write(
        output.join("fixture.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "project": baseline.document,
            "reference": reference_path,
            "reference_project": target.document,
            "sample_rate": RATE,
            "duration_seconds": SECONDS,
            "search_seed": 0,
            "recommended_attempts": 32,
            "changes": ["track pan: 0.00 to +0.15", "clip gate: 1.00 to 0.95"],
        }))?,
    )?;
    println!("Project: {}", baseline.document.display());
    println!("Reference audio: {}", reference_path.display());
    println!("Exact reference project: {}", target.document.display());
    println!(
        "Use project/reference start 0 seconds, duration {SECONDS} seconds, 32 attempts, seed 0, and all five adjustment types."
    );
    println!(
        "cargo run -p auris-session --example match_reference -- \"{}\" \"{}\" \"{}\" 32 12",
        baseline.document.display(),
        reference_path.display(),
        output.join("matched").display(),
    );
    Ok(())
}
