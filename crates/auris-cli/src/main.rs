//! `auris` — the command line frontend.
//!
//! Every subcommand here drives the same [`Session`] the desktop application does, with no
//! window and no audio device. That is the point: if this compiles and works, the backend
//! really is independent of the UI rather than merely arranged to look that way.

#![warn(missing_docs)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;
use auris_session::{Session, SessionOptions};

/// The language every message is printed in.
///
/// English, whatever the system says and whatever the desktop application has been set to. This
/// used to follow [`Settings::language`](auris_session::Settings::language) so that choosing
/// Japanese in one place answered in Japanese in both, and a terminal is not a place that holds
/// up its end of that bargain: a Windows console on a code page other than UTF-8 turns Japanese
/// into mojibake, and a pipe into a tool that assumes ASCII does worse. A window draws its own
/// glyphs and can promise to render what it is given; here the last word belongs to whatever the
/// output happens to land in.
///
/// A constant rather than a parameter, so there is no per-command language to accidentally take
/// from somewhere else. Text that came out of a *document* — a project's name, a track's — is
/// still whatever the user typed, because that is their data and not this program's voice.
const LANGUAGE: Language = Language::English;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Nothing here reads the configuration, but this is still the frontend that may run first on
    // a machine, and an installation predating the move to `~/.config/auris-studio` only has its
    // settings carried across by whichever one does.
    auris_session::migrate_legacy_config();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return ExitCode::FAILURE;
    };

    let result = match command {
        "plugins" => list_plugins(),
        "progressions" => list_progressions(),
        "soundfonts" => list_soundfonts(&args),
        "compose" => compose(&args),
        "info" => with_path(&args, info),
        "render" => render(&args),
        "new" => new_project(&args),
        "collect" => with_path(&args, collect),
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(messages::unknown_command(LANGUAGE, other)),
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
/// `{:<12}` pads by counting *characters*, which lines a table up in English and ruins it the
/// moment a column holds anything wider. [`LANGUAGE`] took the interface's own text out of that
/// question and not the rest of it: `auris info` prints a project's name and its tracks' names,
/// and those are whatever the person who wrote the music called them.
///
/// The ranges below are the East Asian Wide and Fullwidth blocks — an approximation, and an exact
/// one for the scripts a name is most likely to be in.
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

fn print_usage() {
    // Ignored on purpose: usage goes to whoever is still listening, and a reader that has
    // gone away is not a failure worth reporting over the top of the one being explained.
    let _ = writeln!(std::io::stdout(), "{}", Key::CliUsage.get(LANGUAGE));
}

/// Turns a listing's write errors into the command's verdict.
///
/// Every listing used to print through `println!`, which panics when the write fails — and
/// Rust ignores SIGPIPE on Unix, so `auris plugins | head` ended in a panic message and exit
/// code 101 the moment `head` closed its end. A closed pipe is the reader having heard
/// enough, which is a clean exit; any other write failure is a real error.
fn printed(result: std::io::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn with_path(args: &[String], run: impl Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    let path = args
        .get(1)
        .ok_or_else(|| Key::CliExpectedProjectPath.get(LANGUAGE).to_string())?;
    run(Path::new(path))
}

/// A session with no audio device and no GPU, which is all a batch tool needs.
///
/// With the shipped SoundFonts, which [`SessionOptions::headless`] leaves out because a *test*
/// wants a session that is the same on every machine. This is not a test: `auris compose` here
/// and **Compose a Song…** in the window have to write the same piece, and half the instruments
/// a piece asks for are in that library.
fn headless() -> Result<Session, String> {
    Session::new(SessionOptions::headless().with_shipped_fonts(true))
        .map_err(|error| error.to_string())
}

/// Lists the chord progressions the composer knows by name.
fn list_progressions() -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let print = (|| {
        writeln!(out, "{}", Key::CliProgressions.get(LANGUAGE))?;
        for entry in auris_session::prelude::progression_catalog() {
            // Through the same lookup the desktop's menu uses: the descriptions all have
            // Japanese translations, pinned complete by a test, and this listing printed the
            // raw English under a translated heading.
            writeln!(
                out,
                "  @{:<14} {}",
                entry.name,
                auris_i18n::audio::theory_description(entry.description, LANGUAGE)
            )?;
            writeln!(out, "  {:<15} {}", "", entry.chart)?;
        }
        // Then the ones this installation has been taught. Listed under a heading of their own
        // rather than mixed in, because the difference matters: an `.asong` quotes a built-in by
        // name and carries a kept one as *chords*, so a document referring to one of these is
        // portable and a document saying `@axis` needs the reader to have the same catalogue.
        let book = auris_session::progressions::ProgressionBook::load();
        if !book.entries().is_empty() {
            writeln!(out)?;
            writeln!(out, "{}", Key::CliKeptProgressions.get(LANGUAGE))?;
            for entry in book.entries() {
                writeln!(out, "  {:<15} {}", entry.name, entry.chart)?;
            }
        }
        Ok(())
    })();
    printed(print)
}

/// Lists the SoundFonts this build ships with, and whether they are installed.
///
/// With `--manifest`, prints the same list as tab-separated fields instead:
/// `id`, `file`, `bytes`, `sha256`, `url`, `license_url`. That form is what `tools/fetch-soundfonts.sh`
/// downloads from, which is the point of it — the manifest lives in
/// [`auris_session::library`] and nowhere else, so a font that is added or a digest that
/// changes cannot leave the script quietly fetching last month's file.
fn list_soundfonts(args: &[String]) -> Result<(), String> {
    let manifest = args.iter().any(|arg| arg == "--manifest");
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let print = (|| {
        if manifest {
            for font in auris_session::library::SHIPPED {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    font.id, font.file, font.bytes, font.sha256, font.url, font.license_url
                )?;
            }
            return Ok(());
        }
        writeln!(out, "{}", Key::CliSoundFonts.get(LANGUAGE))?;
        for font in auris_session::library::SHIPPED {
            let state = match auris_session::library::installed(font) {
                Some(path) => path.display().to_string(),
                None => Key::CliSoundFontMissing.get(LANGUAGE).to_string(),
            };
            writeln!(out, "  {} {}", pad(font.name, 22), font.license)?;
            writeln!(out, "  {} {state}", pad("", 22))?;
        }
        Ok(())
    })();
    printed(print)
}

/// Writes a piece from a specification and saves it as a project.
fn compose(args: &[String]) -> Result<(), String> {
    let source = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| Key::CliExpectedSpecPath.get(LANGUAGE).to_string())?;
    let source = PathBuf::from(source);

    let mut output = source.with_extension(auris_session::PROJECT_EXTENSION);
    let mut overrides: Vec<(String, String)> = Vec::new();
    let mut print_only = false;

    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                output = PathBuf::from(args.get(index).ok_or_else(|| {
                    messages::option_needs_value(
                        LANGUAGE,
                        "--output",
                        Key::CliNeedsPath.get(LANGUAGE),
                    )
                })?);
            }
            // Every override names a field of the format, so the command line needs no
            // vocabulary of its own and can never drift from it.
            "--set" => {
                index += 1;
                let pair = args.get(index).ok_or_else(|| {
                    messages::option_needs_value(LANGUAGE, "--set", "field: value")
                })?;
                overrides.push(split_override(pair)?);
            }
            "--seed" | "--key" | "--tempo" | "--mood" | "--groove" | "--scale" | "--swing" => {
                let field = args[index].trim_start_matches('-').to_string();
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    // Through the table, like every sibling arm: a literal here was English
                    // interpolated verbatim into the middle of the Japanese message.
                    messages::option_needs_value(LANGUAGE, &field, Key::CliNeedsValue.get(LANGUAGE))
                })?;
                overrides.push((field, value.clone()));
            }
            "--print" => print_only = true,
            other => return Err(messages::unknown_option(LANGUAGE, other)),
        }
        index += 1;
    }

    let text = std::fs::read_to_string(&source)
        .map_err(|error| format!("{}: {error}", source.display()))?;
    let spec = auris_session::prelude::SongSpec::parse_with_overrides(&text, &overrides).map_err(
        |errors| {
            let mut message = messages::spec_rejected(LANGUAGE, &source.display().to_string());
            for error in errors {
                message.push_str(&format!("\n  {error}"));
            }
            message
        },
    )?;

    if print_only {
        return printed(write!(std::io::stdout(), "{}", spec.to_toml()));
    }

    let piece = auris_session::prelude::compose(&spec);
    let mut session = headless()?;
    let report = session.compose(&piece).map_err(|error| error.to_string())?;
    for missing in &report.substituted {
        eprintln!("{}", messages::instrument_substituted(LANGUAGE, missing));
    }
    session.save(&output).map_err(|error| error.to_string())?;

    printed(writeln!(
        std::io::stdout(),
        "{}",
        messages::composed(
            LANGUAGE,
            &output.display().to_string(),
            report.tracks,
            report.notes,
            piece.seed,
        )
    ))?;
    printed(write!(std::io::stdout(), "{}", piece.summary()))?;
    Ok(())
}

/// Splits `--set "field: value"` into its two halves.
///
/// Either separator is accepted: the document itself writes `field = value`, and the option was
/// documented with a colon long before it did. Neither is ambiguous, because no field name of
/// the format contains one. The value is left exactly as it was typed — `D minor` rather than
/// `"D minor"` — and quoted for TOML on the other side, where a number can still be a number.
fn split_override(pair: &str) -> Result<(String, String), String> {
    let (field, value) = pair
        .split_once([':', '='])
        .ok_or_else(|| messages::option_needs_value(LANGUAGE, "--set", "field: value"))?;
    Ok((field.trim().to_string(), value.trim().to_string()))
}

fn list_plugins() -> Result<(), String> {
    let session = headless()?;
    let registry = session.registry();

    // The registry id stays as it is: it is what a project file stores and what a script types.
    fn describe(out: &mut impl Write, descriptor: &PluginDescriptor) -> std::io::Result<()> {
        writeln!(
            out,
            "  {:<26} {} {}",
            descriptor.id,
            pad(
                auris_i18n::audio::category(descriptor.category.label(), LANGUAGE),
                14
            ),
            auris_i18n::audio::plugin_name(&descriptor.name, LANGUAGE)
        )?;
        writeln!(
            out,
            "  {:<26} {:<14} {}",
            "",
            "",
            auris_i18n::audio::plugin_description(&descriptor.description, LANGUAGE)
        )
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let print = (|| {
        writeln!(out, "{}", Key::CliInstruments.get(LANGUAGE))?;
        for descriptor in registry.instruments() {
            describe(&mut out, descriptor)?;
        }
        writeln!(out, "\n{}", Key::CliEffects.get(LANGUAGE))?;
        for descriptor in registry.effects() {
            describe(&mut out, descriptor)?;
        }
        Ok(())
    })();
    printed(print)
}

/// Copies every file a project refers to into its folder, and saves it.
///
/// The command for archiving a project or handing it to someone else, from a script. Audio is
/// already collected as a matter of course; what this adds is the SoundFonts, which are left in
/// place on an ordinary save because one font is shared by every project that uses it.
fn collect(path: &Path) -> Result<(), String> {
    let mut session = headless()?;
    for missing in session.open(path).map_err(|error| error.to_string())? {
        eprintln!(
            "{}",
            messages::warning_missing_audio(LANGUAGE, &missing.display().to_string())
        );
    }
    let collected = session
        .collect_assets()
        .map_err(|error| error.to_string())?;
    session.save_in_place().map_err(|error| error.to_string())?;
    printed(writeln!(
        std::io::stdout(),
        "{}",
        messages::assets_collected(LANGUAGE, collected)
    ))?;
    Ok(())
}

fn info(path: &Path) -> Result<(), String> {
    let mut session = headless()?;
    let missing = session.open(path).map_err(|error| error.to_string())?;
    let project = session.project();
    let field = |key: Key| Key::get(key, LANGUAGE);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let print = (|| {
        writeln!(out, "{}", project.name)?;
        let label = |key: Key| pad(field(key), 14);
        writeln!(out, "  {} {}", label(Key::CliFieldPath), path.display())?;
        writeln!(
            out,
            "  {} {:.2} BPM",
            label(Key::CliFieldTempo),
            project.bpm()
        )?;
        // The meter, and where it changes if it does. A song in one meter reads as it always
        // did; one that changes says so, because a bare `4/4` over a piece that spends its
        // second half in 7/8 is the field lying about the document.
        let meters = project
            .signatures
            .points()
            .iter()
            .map(|point| match point.tick {
                Ticks::ZERO => point.signature.to_string(),
                tick => format!("{} @ {}", point.signature, project.signatures.bar_of(tick)),
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "  {} {meters}", label(Key::CliFieldSignature))?;
        writeln!(
            out,
            "  {} {:.0} Hz",
            label(Key::CliFieldSampleRate),
            project.sample_rate
        )?;
        writeln!(
            out,
            "  {} {}",
            label(Key::CliFieldDuration),
            Seconds(project.duration_seconds()).format_clock()
        )?;
        writeln!(
            out,
            "  {} {}",
            label(Key::CliFieldTracks),
            project.tracks.len()
        )?;

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
                // A bus holds no clips at all, so the count would be a nought that means nothing.
                TrackKind::Bus => format!("{} {:<24}", pad(field(Key::CliKindBus), 12), ""),
            };
            writeln!(out, "    {} {detail}", pad(&track.name, 18))?;
            if !track.mixer.effects.is_empty() {
                let chain: Vec<&str> = track
                    .mixer
                    .effects
                    .iter()
                    .map(|slot| slot.effect_id.as_str())
                    .collect();
                writeln!(out, "    {:<18} {:<10} {}", "", "fx", chain.join(" -> "))?;
            }
            // Where the track goes, said only when it is somewhere other than the obvious place.
            // A line reading "-> master" against every track in a project with no buses at all
            // would be noise on the one command whose whole job is a short answer.
            let name_of = |id: TrackId| {
                project
                    .track(id)
                    .map(|bus| bus.name.clone())
                    .unwrap_or_else(|| format!("#{}", id.0))
            };
            let mut routing: Vec<String> = track
                .output
                .bus()
                .map(|bus| format!("-> {}", name_of(bus)))
                .into_iter()
                .collect();
            routing.extend(track.sends.iter().map(|send| {
                let tap = if send.pre_fader { " pre" } else { "" };
                format!("=> {} {:+.1} dB{tap}", name_of(send.target), send.level_db)
            }));
            if !routing.is_empty() {
                writeln!(
                    out,
                    "    {:<18} {:<10} {}",
                    "",
                    field(Key::CliFieldRouting),
                    routing.join(", ")
                )?;
            }
        }

        if !project.master.effects.is_empty() {
            let chain: Vec<&str> = project
                .master
                .effects
                .iter()
                .map(|slot| slot.effect_id.as_str())
                .collect();
            writeln!(
                out,
                "    {} {:<10} {}",
                pad(field(Key::CliMaster), 18),
                "fx",
                chain.join(" -> ")
            )?;
        }
        Ok(())
    })();

    for path in &missing {
        eprintln!(
            "{}",
            messages::warning_missing_audio(LANGUAGE, &path.display().to_string())
        );
    }
    printed(print)
}

fn render(args: &[String]) -> Result<(), String> {
    let source = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| Key::CliExpectedProjectPath.get(LANGUAGE).to_string())?;
    let source = PathBuf::from(source);

    let mut output = source.with_extension("wav");
    let mut settings = WavExportSettings::default();
    let mut options = OfflineOptions::whole_project();
    let mut loop_only = false;

    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                output = PathBuf::from(args.get(index).ok_or_else(|| {
                    messages::option_needs_value(
                        LANGUAGE,
                        "--output",
                        Key::CliNeedsPath.get(LANGUAGE),
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
                        return Err(messages::bad_bit_depth(LANGUAGE, other.unwrap_or_default()));
                    }
                };
            }
            "--dither" => settings.dither = true,
            "--no-tail" => options.include_tail = false,
            "--loop" => loop_only = true,
            other => return Err(messages::unknown_option(LANGUAGE, other)),
        }
        index += 1;
    }

    let mut session = headless()?;
    for path in session.open(&source).map_err(|error| error.to_string())? {
        eprintln!(
            "{}",
            messages::warning_missing_audio(LANGUAGE, &path.display().to_string())
        );
    }

    let job = session.render_job();
    if loop_only {
        // Resolved through the job so the region is converted exactly as the GUI converts
        // it; a project without one is an error, not a silent whole-project render.
        options = job
            .loop_options(options)
            .ok_or_else(|| Key::CliNoCycle.get(LANGUAGE).to_string())?;
    }
    let mut last_percent = -1i32;
    let summary = job
        .render_to_wav(&output, &settings, &options, &mut |fraction| {
            // Only redraw when the number actually changes; repainting per block spends more
            // time on the terminal than on the audio.
            let percent = (fraction * 100.0) as i32;
            if percent != last_percent {
                last_percent = percent;
                eprint!("\r{}", messages::render_progress(LANGUAGE, percent));
            }
        })
        .map_err(|error| error.to_string())?;
    eprintln!();

    printed(writeln!(
        std::io::stdout(),
        "{}",
        messages::wrote_file(
            LANGUAGE,
            &output.display().to_string(),
            &Seconds(summary.seconds).format_clock(),
            summary.channels,
            settings.bit_depth.bits(),
            summary.peak_db,
        )
    ))?;
    Ok(())
}

fn new_project(args: &[String]) -> Result<(), String> {
    let target = args
        .get(1)
        .filter(|arg| !arg.starts_with('-'))
        .ok_or_else(|| Key::CliExpectedNewPath.get(LANGUAGE).to_string())?;
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
                            LANGUAGE,
                            "--bpm",
                            Key::CliNeedsNumber.get(LANGUAGE),
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
                            LANGUAGE,
                            "--sample-rate",
                            Key::CliNeedsNumber.get(LANGUAGE),
                        )
                    })?;
            }
            other => return Err(messages::unknown_option(LANGUAGE, other)),
        }
        index += 1;
    }

    let mut session = Session::new(
        SessionOptions::headless()
            .with_sample_rate(sample_rate)
            .with_shipped_fonts(true),
    )
    .map_err(|error| error.to_string())?;
    session.set_bpm(bpm);
    session
        .add_default_instrument_track(messages::new_track_name(LANGUAGE, 1))
        .map_err(|error| error.to_string())?;
    session.save(&target).map_err(|error| error.to_string())?;

    printed(writeln!(
        std::io::stdout(),
        "{}",
        messages::created_project(
            LANGUAGE,
            &target.display().to_string(),
            session.project().bpm(),
            session.project().sample_rate,
        )
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_is_split_on_either_separator() {
        // The document writes `field = value`; this option was documented with a colon long
        // before it did, and both are unambiguous because no field name of the format has one.
        assert_eq!(
            split_override("tempo: 128").unwrap(),
            ("tempo".to_string(), "128".to_string())
        );
        assert_eq!(
            split_override("tempo = 128").unwrap(),
            ("tempo".to_string(), "128".to_string())
        );
        // The value keeps its spaces and its quotes stay off: `key: D minor`, not `key: "D
        // minor"`. Quoting it for TOML is the format's job, not the shell's.
        assert_eq!(
            split_override("key: D minor").unwrap(),
            ("key".to_string(), "D minor".to_string())
        );
        assert!(split_override("tempo").is_err());
    }

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
