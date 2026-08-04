//! `auris` — the command line frontend.
//!
//! Every subcommand here drives the same [`Session`] the desktop application does, with no
//! window and no audio device. That is the point: if this compiles and works, the backend
//! really is independent of the UI rather than merely arranged to look that way.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;
use auris_session::{Session, SessionOptions};

/// The language every message is printed in.
///
/// Read from the same settings file the desktop application writes, so choosing Japanese in one
/// place answers in Japanese in both. With no file yet, the environment decides.
fn language() -> Language {
    Settings::load().language()
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Before the first `Settings::load`: the configuration moved to `~/.config/auris-studio`, and
    // whichever frontend runs first is the one that carries an older installation's across.
    auris_session::migrate_legacy_config();

    let language = language();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_usage(language);
        return ExitCode::FAILURE;
    };

    let result = match command {
        "plugins" => list_plugins(language),
        "progressions" => list_progressions(language),
        "compose" => compose(&args, language),
        "info" => with_path(&args, language, info),
        "render" => render(&args, language),
        "new" => new_project(&args, language),
        "collect" => with_path(&args, language, collect),
        "help" | "-h" | "--help" => {
            print_usage(language);
            Ok(())
        }
        other => Err(messages::unknown_command(language, other)),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("auris: {message}");
            ExitCode::FAILURE
        }
    }
}

/// How many terminal columns `text` occupies.
///
/// `{:<12}` pads by counting *characters*, which lines a table up in English and ruins it in
/// Japanese, where one character is two columns wide. The ranges below are the East Asian Wide
/// and Fullwidth blocks — an approximation, but an exact one for every script this ships in.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| match c as u32 {
            0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x3FFFD => 2,
            _ => 1,
        })
        .sum()
}

/// `text` padded with spaces to `columns` terminal columns.
fn pad(text: &str, columns: usize) -> String {
    let width = display_width(text);
    format!("{text}{}", " ".repeat(columns.saturating_sub(width)))
}

fn print_usage(language: Language) {
    println!("{}", Key::CliUsage.get(language));
}

fn with_path(
    args: &[String],
    language: Language,
    run: impl Fn(&Path, Language) -> Result<(), String>,
) -> Result<(), String> {
    let path = args
        .get(1)
        .ok_or_else(|| Key::CliExpectedProjectPath.get(language).to_string())?;
    run(Path::new(path), language)
}

/// A session with no audio device and no GPU, which is all a batch tool needs.
fn headless() -> Result<Session, String> {
    Session::new(SessionOptions::headless()).map_err(|error| error.to_string())
}

/// Lists the chord progressions the composer knows by name.
fn list_progressions(language: Language) -> Result<(), String> {
    println!("{}", Key::CliProgressions.get(language));
    for entry in auris_session::prelude::progression_catalog() {
        println!("  @{:<14} {}", entry.name, entry.description);
        println!("  {:<15} {}", "", entry.chart);
    }
    Ok(())
}

/// Writes a piece from a specification and saves it as a project.
fn compose(args: &[String], language: Language) -> Result<(), String> {
    let source = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| Key::CliExpectedSpecPath.get(language).to_string())?;
    let source = PathBuf::from(source);

    let mut output = source.with_extension(auris_session::PROJECT_EXTENSION);
    let mut overrides: Vec<String> = Vec::new();
    let mut print_only = false;

    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                output = PathBuf::from(args.get(index).ok_or_else(|| {
                    messages::option_needs_value(
                        language,
                        "--output",
                        Key::CliNeedsPath.get(language),
                    )
                })?);
            }
            // Every override is just another line of the format, appended after the document —
            // so the command line needs no vocabulary of its own and can never drift from it.
            "--set" => {
                index += 1;
                overrides.push(
                    args.get(index)
                        .ok_or_else(|| {
                            messages::option_needs_value(language, "--set", "field: value")
                        })?
                        .clone(),
                );
            }
            "--seed" | "--key" | "--tempo" | "--mood" | "--groove" | "--scale" | "--swing" => {
                let field = args[index].trim_start_matches('-').to_string();
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| messages::option_needs_value(language, &field, "a value"))?;
                overrides.push(format!("{field}: {value}"));
            }
            "--print" => print_only = true,
            other => return Err(messages::unknown_option(language, other)),
        }
        index += 1;
    }

    let text = std::fs::read_to_string(&source)
        .map_err(|error| format!("{}: {error}", source.display()))?;
    // `[song]` first, so an override lands on a header field even when the document ends inside
    // a section or a part block.
    let combined = if overrides.is_empty() {
        text
    } else {
        format!("{text}\n[song]\n{}", overrides.join("\n"))
    };
    let spec = auris_session::prelude::SongSpec::parse(&combined).map_err(|errors| {
        let mut message = messages::spec_rejected(language, &source.display().to_string());
        for error in errors {
            message.push_str(&format!("\n  {error}"));
        }
        message
    })?;

    if print_only {
        print!("{}", spec.to_text());
        return Ok(());
    }

    let piece = auris_session::prelude::compose(&spec);
    let mut session = headless()?;
    let report = session.compose(&piece).map_err(|error| error.to_string())?;
    for missing in &report.substituted {
        eprintln!("{}", messages::instrument_substituted(language, missing));
    }
    session.save(&output).map_err(|error| error.to_string())?;

    println!(
        "{}",
        messages::composed(
            language,
            &output.display().to_string(),
            report.tracks,
            report.notes,
            piece.seed,
        )
    );
    print!("{}", piece.summary());
    Ok(())
}

fn list_plugins(language: Language) -> Result<(), String> {
    let session = headless()?;
    let registry = session.registry();

    // The registry id stays as it is: it is what a project file stores and what a script types.
    let describe = |descriptor: &PluginDescriptor| {
        println!(
            "  {:<26} {} {}",
            descriptor.id,
            pad(
                auris_i18n::audio::category(descriptor.category.label(), language),
                14
            ),
            auris_i18n::audio::plugin_name(&descriptor.name, language)
        );
        println!(
            "  {:<26} {:<14} {}",
            "",
            "",
            auris_i18n::audio::plugin_description(&descriptor.description, language)
        );
    };

    println!("{}", Key::CliInstruments.get(language));
    for descriptor in registry.instruments() {
        describe(descriptor);
    }
    println!("\n{}", Key::CliEffects.get(language));
    for descriptor in registry.effects() {
        describe(descriptor);
    }
    Ok(())
}

/// Copies every file a project refers to into its folder, and saves it.
///
/// The command for archiving a project or handing it to someone else, from a script. Audio is
/// already collected as a matter of course; what this adds is the SoundFonts, which are left in
/// place on an ordinary save because one font is shared by every project that uses it.
fn collect(path: &Path, language: Language) -> Result<(), String> {
    let mut session = headless()?;
    for missing in session.open(path).map_err(|error| error.to_string())? {
        eprintln!(
            "{}",
            messages::warning_missing_audio(language, &missing.display().to_string())
        );
    }
    let collected = session
        .collect_assets()
        .map_err(|error| error.to_string())?;
    session.save_in_place().map_err(|error| error.to_string())?;
    println!("{}", messages::assets_collected(language, collected));
    Ok(())
}

fn info(path: &Path, language: Language) -> Result<(), String> {
    let mut session = headless()?;
    let missing = session.open(path).map_err(|error| error.to_string())?;
    let project = session.project();
    let field = |key: Key| Key::get(key, language);

    println!("{}", project.name);
    let label = |key: Key| pad(field(key), 14);
    println!("  {} {}", label(Key::CliFieldPath), path.display());
    println!("  {} {:.2} BPM", label(Key::CliFieldTempo), project.bpm());
    println!(
        "  {} {}/{}",
        label(Key::CliFieldSignature),
        project.time_signature.numerator,
        project.time_signature.denominator
    );
    println!(
        "  {} {:.0} Hz",
        label(Key::CliFieldSampleRate),
        project.sample_rate
    );
    println!(
        "  {} {}",
        label(Key::CliFieldDuration),
        Seconds(project.duration_seconds()).format_clock()
    );
    println!("  {} {}", label(Key::CliFieldTracks), project.tracks.len());

    for track in &project.tracks {
        let clips = field(Key::CliClipCount);
        let detail = match &track.kind {
            TrackKind::Instrument(inner) => format!(
                "{} {:<24} {} {clips}",
                pad(field(Key::CliKindInstrument), 12),
                inner.instrument_id,
                inner.clips.len()
            ),
            TrackKind::Audio(inner) => format!(
                "{} {:<24} {} {clips}",
                pad(field(Key::CliKindAudio), 12),
                "",
                inner.clips.len()
            ),
        };
        println!("    {} {detail}", pad(&track.name, 18));
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
        println!(
            "    {} {:<10} {}",
            pad(field(Key::CliMaster), 18),
            "fx",
            chain.join(" -> ")
        );
    }

    for path in &missing {
        eprintln!(
            "{}",
            messages::warning_missing_audio(language, &path.display().to_string())
        );
    }
    Ok(())
}

fn render(args: &[String], language: Language) -> Result<(), String> {
    let source = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| Key::CliExpectedProjectPath.get(language).to_string())?;
    let source = PathBuf::from(source);

    let mut output = source.with_extension("wav");
    let mut settings = WavExportSettings::default();
    let mut options = OfflineOptions::whole_project();

    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                output = PathBuf::from(args.get(index).ok_or_else(|| {
                    messages::option_needs_value(
                        language,
                        "--output",
                        Key::CliNeedsPath.get(language),
                    )
                })?);
            }
            "--bit-depth" => {
                index += 1;
                settings.bit_depth = match args.get(index).map(String::as_str) {
                    Some("16") => WavBitDepth::Int16,
                    Some("24") => WavBitDepth::Int24,
                    Some("32") => WavBitDepth::Float32,
                    other => {
                        return Err(messages::bad_bit_depth(language, other.unwrap_or_default()));
                    }
                };
            }
            "--dither" => settings.dither = true,
            "--no-tail" => options.include_tail = false,
            other => return Err(messages::unknown_option(language, other)),
        }
        index += 1;
    }

    let mut session = headless()?;
    for path in session.open(&source).map_err(|error| error.to_string())? {
        eprintln!(
            "{}",
            messages::warning_missing_audio(language, &path.display().to_string())
        );
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
                eprint!("\r{}", messages::render_progress(language, percent));
            }
        })
        .map_err(|error| error.to_string())?;
    eprintln!();

    println!(
        "{}",
        messages::wrote_file(
            language,
            &output.display().to_string(),
            &Seconds(summary.seconds).format_clock(),
            summary.channels,
            settings.bit_depth.bits(),
            summary.peak_db,
        )
    );
    Ok(())
}

fn new_project(args: &[String], language: Language) -> Result<(), String> {
    let target = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| Key::CliExpectedNewPath.get(language).to_string())?;
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
                    .ok_or_else(|| {
                        messages::option_needs_value(
                            language,
                            "--bpm",
                            Key::CliNeedsNumber.get(language),
                        )
                    })?;
            }
            "--sample-rate" => {
                index += 1;
                sample_rate = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    // `parse::<f64>` happily accepts `inf`, `NaN` and overflowing literals
                    // like `1e999`, and a project stored with one serialises its rate as JSON
                    // null — a file this very tool can never read back. A rate that is not a
                    // positive finite number is not a rate.
                    .filter(|rate: &f64| rate.is_finite() && *rate > 0.0)
                    .ok_or_else(|| {
                        messages::option_needs_value(
                            language,
                            "--sample-rate",
                            Key::CliNeedsNumber.get(language),
                        )
                    })?;
            }
            other => return Err(messages::unknown_option(language, other)),
        }
        index += 1;
    }

    let mut session = Session::new(SessionOptions::headless().with_sample_rate(sample_rate))
        .map_err(|error| error.to_string())?;
    session.set_bpm(bpm);
    session
        .add_default_instrument_track(messages::new_track_name(language, 1))
        .map_err(|error| error.to_string())?;
    session.save(&target).map_err(|error| error.to_string())?;

    println!(
        "{}",
        messages::created_project(
            language,
            &target.display().to_string(),
            session.project().bpm(),
            session.project().sample_rate,
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_characters_count_as_two_columns() {
        assert_eq!(display_width("path"), 4);
        assert_eq!(display_width("パス"), 4);
        assert_eq!(display_width("サンプルレート"), 14);
        assert_eq!(display_width("128 BPM"), 7);
    }

    #[test]
    fn padding_lines_both_languages_up_to_the_same_column() {
        assert_eq!(display_width(&pad("path", 14)), 14);
        assert_eq!(display_width(&pad("パス", 14)), 14);
        // Something already too wide is never truncated: a clipped label is worse than a
        // ragged column.
        assert_eq!(pad("サンプルレート", 4), "サンプルレート");
    }
}
