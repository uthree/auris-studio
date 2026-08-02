//! `auris` — the command line frontend.
//!
//! Every subcommand here drives the same [`Session`] the desktop application does, with no
//! window and no audio device. That is the point: if this compiles and works, the backend
//! really is independent of the UI rather than merely arranged to look that way.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use auris_session::prelude::*;
use auris_session::{Session, SessionOptions};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return ExitCode::FAILURE;
    };

    let result = match command {
        "plugins" => list_plugins(),
        "info" => with_path(&args, info),
        "render" => render(&args),
        "new" => new_project(&args),
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("unknown command `{other}`; try `auris help`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("auris: {message}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!(
        "\
auris — Auris Studio from the command line

USAGE
    auris <command> [options]

COMMANDS
    plugins                       List every registered instrument and effect
    info <project.auris>          Print a project's tracks, clips and duration
    render <project.auris> [opts] Render a project to a WAV file
    new <project.auris> [opts]    Create a project with one instrument track
    help                          Show this message

RENDER OPTIONS
    -o, --output <file.wav>       Where to write (default: alongside the project)
        --bit-depth <16|24|32>    Sample format; 32 means 32-bit float (default: 24)
        --dither                  Add TPDF dither, for 16-bit masters
        --no-tail                 Stop at the last clip instead of letting effect tails ring

NEW OPTIONS
        --bpm <tempo>             Tempo of the new project (default: 120)
        --sample-rate <hz>        Rate of the new project (default: 48000)"
    );
}

fn with_path(args: &[String], run: impl Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    let path = args
        .get(1)
        .ok_or_else(|| "expected a project path".to_string())?;
    run(Path::new(path))
}

/// A session with no audio device and no GPU, which is all a batch tool needs.
fn headless() -> Result<Session, String> {
    Session::new(SessionOptions::headless()).map_err(|error| error.to_string())
}

fn list_plugins() -> Result<(), String> {
    let session = headless()?;
    let registry = session.registry();

    println!("INSTRUMENTS");
    for descriptor in registry.instruments() {
        println!(
            "  {:<26} {:<12} {}",
            descriptor.id,
            descriptor.category.label(),
            descriptor.name
        );
        println!("  {:<26} {:<12} {}", "", "", descriptor.description);
    }
    println!("\nEFFECTS");
    for descriptor in registry.effects() {
        println!(
            "  {:<26} {:<12} {}",
            descriptor.id,
            descriptor.category.label(),
            descriptor.name
        );
        println!("  {:<26} {:<12} {}", "", "", descriptor.description);
    }
    Ok(())
}

fn info(path: &Path) -> Result<(), String> {
    let mut session = headless()?;
    let missing = session.open(path).map_err(|error| error.to_string())?;
    let project = session.project();

    println!("{}", project.name);
    println!("  path         {}", path.display());
    println!("  tempo        {:.2} BPM", project.bpm());
    println!(
        "  signature    {}/{}",
        project.time_signature.numerator, project.time_signature.denominator
    );
    println!("  sample rate  {:.0} Hz", project.sample_rate);
    println!(
        "  duration     {}",
        Seconds(project.duration_seconds()).format_clock()
    );
    println!("  tracks       {}", project.tracks.len());

    for track in &project.tracks {
        let detail = match &track.kind {
            TrackKind::Instrument(inner) => format!(
                "{:<10} {:<24} {} clip(s)",
                "instrument",
                inner.instrument_id,
                inner.clips.len()
            ),
            TrackKind::Audio(inner) => {
                format!("{:<10} {:<24} {} clip(s)", "audio", "", inner.clips.len())
            }
        };
        println!("    {:<18} {detail}", track.name);
        if !track.mixer.effects.is_empty() {
            let chain: Vec<&str> = track
                .mixer
                .effects
                .iter()
                .map(|slot| slot.effect_id.as_str())
                .collect();
            println!("    {:<18} {:<10} {}", "", "fx", chain.join(" -> "));
        }
    }

    if !project.master.effects.is_empty() {
        let chain: Vec<&str> = project
            .master
            .effects
            .iter()
            .map(|slot| slot.effect_id.as_str())
            .collect();
        println!("    {:<18} {:<10} {}", "master", "fx", chain.join(" -> "));
    }

    for path in &missing {
        eprintln!("warning: missing audio file {}", path.display());
    }
    Ok(())
}

fn render(args: &[String]) -> Result<(), String> {
    let source = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| "expected a project path".to_string())?;
    let source = PathBuf::from(source);

    let mut output = source.with_extension("wav");
    let mut settings = WavExportSettings::default();
    let mut options = OfflineOptions::whole_project();

    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                output = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--output needs a path".to_string())?,
                );
            }
            "--bit-depth" => {
                index += 1;
                settings.bit_depth = match args.get(index).map(String::as_str) {
                    Some("16") => WavBitDepth::Int16,
                    Some("24") => WavBitDepth::Int24,
                    Some("32") => WavBitDepth::Float32,
                    other => {
                        return Err(format!(
                            "--bit-depth wants 16, 24 or 32, not `{}`",
                            other.unwrap_or("nothing")
                        ));
                    }
                };
            }
            "--dither" => settings.dither = true,
            "--no-tail" => options.include_tail = false,
            other => return Err(format!("unknown option `{other}`")),
        }
        index += 1;
    }

    let mut session = headless()?;
    for path in session.open(&source).map_err(|error| error.to_string())? {
        eprintln!("warning: missing audio file {}", path.display());
    }

    let job = session.render_job();
    let mut last_percent = -1i32;
    let summary = job
        .render_to_wav(&output, &settings, &options, &mut |fraction| {
            // Only redraw when the number actually changes; repainting per block spends more
            // time on the terminal than on the audio.
            let percent = (fraction * 100.0) as i32;
            if percent != last_percent {
                last_percent = percent;
                eprint!("\rrendering {percent:>3}%");
            }
        })
        .map_err(|error| error.to_string())?;
    eprintln!();

    println!(
        "wrote {} · {} · {} ch · {}-bit · peak {:.1} dBFS",
        output.display(),
        Seconds(summary.seconds).format_clock(),
        summary.channels,
        settings.bit_depth.bits(),
        summary.peak_db
    );
    Ok(())
}

fn new_project(args: &[String]) -> Result<(), String> {
    let target = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| "expected a path for the new project".to_string())?;
    let target = PathBuf::from(target);

    let mut bpm = 120.0;
    let mut sample_rate = 48_000.0;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--bpm" => {
                index += 1;
                bpm = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--bpm needs a number".to_string())?;
            }
            "--sample-rate" => {
                index += 1;
                sample_rate = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--sample-rate needs a number".to_string())?;
            }
            other => return Err(format!("unknown option `{other}`")),
        }
        index += 1;
    }

    let mut session = Session::new(SessionOptions::headless().with_sample_rate(sample_rate))
        .map_err(|error| error.to_string())?;
    session.set_bpm(bpm);
    session
        .add_default_instrument_track("Track 1")
        .map_err(|error| error.to_string())?;
    session.save(&target).map_err(|error| error.to_string())?;

    println!(
        "created {} · {:.2} BPM · {:.0} Hz",
        target.display(),
        session.project().bpm(),
        session.project().sample_rate
    );
    Ok(())
}
