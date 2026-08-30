//! The session, offered as tools a language model can call.
//!
//! `auris-i18n` holds every word the interface says to a *person*; this crate holds every word
//! it says to a *model* — tool names, descriptions, argument schemas and the answers the tools
//! give — together with the work behind them. It exists because two frontends speak to models:
//! `auris-mcp`, where a model's harness dials in over the Model Context Protocol, and
//! `auris-agent`, where Auris dials out to a model API and runs the loop itself. A tool that
//! existed twice would drift twice; here `compose` at one door and `compose` at the other are
//! the same text, the same schema and the same code by construction.
//!
//! Three decisions, inherited by both doors:
//!
//! * **English.** The reader is a model, every one of which reads English, and neither protocol
//!   has a language field.
//! * **A session per call.** Every tool that touches a project opens a fresh headless
//!   [`Session`], uses it and drops it. The tools all speak in *files*, and a project on disk
//!   is the only state worth keeping between calls; it also means no [`Session`] ever has to
//!   cross a thread. The caller is expected to run these functions off its async runtime —
//!   they block, honestly: opening a session parses SoundFont files, and a render is minutes
//!   of DSP.
//! * **Errors are answers.** `Err` here is text a model can read and act on — a wrong path, a
//!   rejected spec, a track that does not exist — never a panic and never a protocol failure,
//!   which both frontends reserve for their own machinery breaking.
//!
//! # Shape
//!
//! One public module per tool, each with the same four items: [`compose::NAME`],
//! [`compose::DESCRIPTION`], [`compose::Args`] and [`compose::run`] — so a frontend can bind
//! the whole set with one pattern and a new tool added here appears at every door.

#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_session::prelude::*;
use auris_session::{Session, SessionError, SessionOptions};

/// What a model is told before it has called anything.
///
/// The one piece of text a model keeps in context for the whole conversation, so it carries
/// the workflow and nothing else — the format itself is behind `spec_reference`, fetched when
/// a spec is actually being written rather than sitting in every exchange.
pub const INSTRUCTIONS: &str = "Auris Studio is a digital audio workstation; these tools drive \
    its headless session. A song is written as a `.asong` specification — TOML in which every \
    field has a default, so two lines are already a valid song. The flow: `spec_reference` once \
    to learn the format, `check_spec` to validate a draft (errors name lines and fields, and a \
    valid spec comes back with every default filled in), `compose` to write the piece and save \
    it as a project, `render` to hear it as a WAV file. `describe` inspects an existing \
    project; `list_presets` and `list_progressions` are the vocabulary a spec can quote. To \
    improve a piece, iterate: `analyze` listens for you — loudness and peaks for the mix, per \
    section and (on request) per track — then either edit the spec and `compose` again with \
    force, or aim `another_take` / `write_again` at one clip the way `describe` numbers them. \
    Give every path as an absolute path — the working directory is wherever the host process \
    happened to be launched.";

/// A song specification, however the three optional fields spell it.
///
/// The same triangle as `auris compose`: a document, or a named preset, and overrides that land
/// on either. Shared between `check_spec` and `compose` so that validating a spec and composing
/// from it can never read the same text two different ways.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SpecArgs {
    /// The `.asong` document itself, as TOML text. Pass this or `preset`, not both.
    pub spec: Option<String>,
    /// The name of a shipped style to start from instead — `list_presets` names them all.
    pub preset: Option<String>,
    /// Field overrides applied on top, e.g. `{"key": "D minor", "tempo": "96"}`. Every name is
    /// a field of the format itself — run `check_spec` on an empty spec to see them all.
    pub overrides: Option<BTreeMap<String, String>>,
}

/// The address of one project change: which clip, and which take of it.
///
/// Shared by `another_take` and `write_again`, which aim the same way and differ only in the
/// seed they keep.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RegenerateArgs {
    /// The project to change — an absolute path to a `.auris` file.
    pub project: String,
    /// The track whose clip to write again, by name as `describe` lists it.
    pub track: String,
    /// Which clip on that track, by the 1-based number `describe` shows. Every generated
    /// clip on the track when left out.
    pub clip: Option<usize>,
    /// `another_take` only: the exact seed to take, instead of the next one — how a take
    /// that measured better earlier is got back, since every result names its seed.
    pub seed: Option<u64>,
}

/// The `.asong` format, taught by example.
pub mod spec_reference {
    /// The tool's name at every door.
    pub const NAME: &str = "spec_reference";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "The `.asong` format, taught by example: a two-line song, \
        then a specification using most of the vocabulary with a comment on every field. Read \
        this before writing a spec.";

    /// The reference text itself.
    pub fn run() -> String {
        // `include_str!` reaches outside the crate, which would break `cargo package` — and
        // these crates are built from the repository, never published, so the examples the
        // *repository's* documentation points at stay the ones this tool serves.
        const HELLO: &str = include_str!("../../../examples/hello.asong");
        const NEON_DRIVE: &str = include_str!("../../../examples/neon-drive.asong");
        format!(
            "A specification is TOML; every field has a default, so start small and only say \
             what should differ. The smallest useful song:\n\n{HELLO}\n\nAnd most of the \
             vocabulary, each field explained where it is used:\n\n{NEON_DRIVE}"
        )
    }
}

/// Validation without composition.
pub mod check_spec {
    /// The tool's name at every door.
    pub const NAME: &str = "check_spec";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Validates a specification without composing anything. A \
        rejected spec answers with every complaint at once, line numbers where they exist; a \
        valid one answers with the full document, every default filled in — the cheap way to \
        see what a draft actually means.";

    /// The three-field spec triangle, unchanged.
    pub use crate::SpecArgs as Args;

    /// Parses the spec and answers with the fully defaulted document, or the complaints.
    pub fn run(args: &Args) -> Result<String, String> {
        crate::resolve_spec(args).map(|spec| {
            format!(
                "The specification is valid. In full, with every default filled in:\n\n{}",
                spec.to_toml()
            )
        })
    }
}

/// Composition, and the save that makes it a project.
pub mod compose {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "compose";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Composes a song from a specification and saves it as a \
        project. The answer reports what was written — tracks, notes, seed, where the mix was \
        measured to — and the seed is what to pin in the spec to ask for this exact take again.";

    /// Arguments to `compose`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The specification to compose from.
        #[serde(flatten)]
        pub spec: SpecArgs,
        /// Where to save the project, as an absolute `.auris` path. The project becomes a
        /// folder: choosing `MySong.auris` writes `MySong/MySong.auris`.
        pub output: String,
        /// Replace the project already at `output`, instead of refusing to.
        #[serde(default)]
        pub force: bool,
    }

    /// What `auris compose` does, answering in one string.
    pub fn run(args: &Args) -> Result<String, String> {
        let spec = resolve_spec(&args.spec)?;
        let piece = auris_session::prelude::compose(&spec);
        let mut session = headless()?;
        let report = session.compose(&piece).map_err(|error| error.to_string())?;

        // `save_as`, never `save`: the project must land in a folder of its own, and a folder
        // already holding a different project is a refusal the caller answers deliberately.
        let chosen = Path::new(&args.output);
        let saved = match args.force {
            true => session.save_as_replacing(chosen),
            false => session.save_as(chosen),
        };
        let written = match saved {
            Ok(save) => save.document,
            Err(SessionError::WouldReplace(path)) => {
                return Err(format!(
                    "the folder at {} already holds a project; pass force: true to replace it",
                    path.display()
                ));
            }
            Err(error) => return Err(error.to_string()),
        };

        let mut text = format!(
            "Wrote {} — {} tracks, {} notes, {}, seed {}.",
            written.display(),
            report.tracks,
            report.notes,
            Seconds(session.project().duration_seconds()).format_clock(),
            piece.seed,
        );
        for missing in &report.substituted {
            text.push_str(&format!(
                "\nNote: no instrument in this build answers to '{missing}'; a stand-in plays \
                 that part."
            ));
        }
        if let Some(lufs) = report.balance.as_ref().and_then(|balance| balance.now_lufs) {
            text.push_str(&format!(
                "\nThe mix was measured and set to {lufs:.1} LUFS."
            ));
        }
        text.push_str(&format!("\n\n{}", piece.summary()));
        Ok(text)
    }
}

/// Rendering, to one mix or to stems.
pub mod render {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "render";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Renders a project to a WAV file — or, with `stems`, to one \
        file per track — and reports each file's length, channels and peak level.";

    /// Arguments to `render`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to render — an absolute path to a `.auris` file.
        pub project: String,
        /// Where to write the WAV file. Beside the project, `.wav` for `.auris`, when left out.
        pub output: Option<String>,
        /// Bits per sample: 16, 24 or 32 (float). 24 when left out.
        pub bit_depth: Option<u16>,
        /// Render each track to its own file in this folder instead of writing one mix.
        pub stems: Option<String>,
    }

    /// One mix, or one file per track.
    pub fn run(args: &Args) -> Result<String, String> {
        let source = Path::new(&args.project);
        let settings = WavExportSettings {
            bit_depth: match args.bit_depth {
                None | Some(24) => WavBitDepth::Int24,
                Some(16) => WavBitDepth::Int16,
                Some(32) => WavBitDepth::Float32,
                Some(other) => return Err(format!("bit_depth is 16, 24 or 32, not {other}")),
            },
            ..WavExportSettings::default()
        };
        let options = OfflineOptions::whole_project();

        let mut session = headless()?;
        let missing = session.open(source).map_err(|error| error.to_string())?;
        let mut text = String::new();
        for path in &missing {
            text.push_str(&format!(
                "Note: the audio file {} is missing; its track rendered silent.\n",
                path.display()
            ));
        }

        // Nobody is watching a progress bar here — the call simply takes as long as it takes —
        // so the default progress: unreported, uncancellable.
        let mut job = session.render_job();
        if let Some(folder) = &args.stems {
            let folder = PathBuf::from(folder);
            std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
            let written = job
                .render_stems(&folder, &settings, &options, &mut RenderProgress::default())
                .map_err(|error| error.to_string())?;
            for stem in &written {
                text.push_str(&wrote_line(&stem.path, &stem.summary, &settings));
            }
        } else {
            let output = args
                .output
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| source.with_extension("wav"));
            let summary = job
                .render_to_wav(&output, &settings, &options, &mut RenderProgress::default())
                .map_err(|error| error.to_string())?;
            text.push_str(&wrote_line(&output, &summary, &settings));
        }
        Ok(text.trim_end().to_string())
    }
}

/// Inspection of a project on disk.
pub mod describe {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "describe";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Describes a project on disk: tempo, meter, duration, and \
        every track with its instrument, clip count, effects and routing.";

    /// Arguments to `describe`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to describe — an absolute path to a `.auris` file.
        pub project: String,
    }

    /// What `auris info` prints, answering in one string.
    pub fn run(args: &Args) -> Result<String, String> {
        let path = Path::new(&args.project);
        let mut session = headless()?;
        let missing = session.open(path).map_err(|error| error.to_string())?;
        let project = session.project();

        let mut text = format!("{}\n", project.name);
        text.push_str(&format!("  tempo       {:.2} BPM\n", project.bpm()));
        // The meter, and where it changes if it does — a bare `4/4` over a piece that spends
        // its second half in 7/8 would be the line lying about the document.
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
        text.push_str(&format!("  meter       {meters}\n"));
        text.push_str(&format!("  sample rate {:.0} Hz\n", project.sample_rate));
        text.push_str(&format!(
            "  duration    {}\n",
            Seconds(project.duration_seconds()).format_clock()
        ));
        text.push_str(&format!("  tracks      {}\n", project.tracks.len()));

        let name_of = |id: TrackId| {
            project
                .track(id)
                .map(|bus| bus.name.clone())
                .unwrap_or_else(|| format!("#{}", id.0))
        };
        for track in &project.tracks {
            let detail = match &track.kind {
                TrackKind::Instrument(inner) => {
                    format!(
                        "instrument {} — {} clips",
                        inner.instrument_id,
                        inner.clips.len()
                    )
                }
                TrackKind::Singer(inner) => {
                    format!(
                        "singer {} — {} clips",
                        inner.instrument_id,
                        inner.clips.len()
                    )
                }
                TrackKind::Audio(inner) => format!("audio — {} clips", inner.clips.len()),
                // A bus holds no clips at all, so a count would be a nought that means nothing.
                TrackKind::Bus => "bus".to_string(),
            };
            text.push_str(&format!("    {:<20} {detail}\n", track.name));
            if !track.mixer.effects.is_empty() {
                let chain: Vec<&str> = track
                    .mixer
                    .effects
                    .iter()
                    .map(|slot| slot.effect_id.as_str())
                    .collect();
                text.push_str(&format!("    {:<20} fx: {}\n", "", chain.join(" -> ")));
            }
            // Where the track goes, said only when it is somewhere other than the obvious place.
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
                text.push_str(&format!("    {:<20} {}\n", "", routing.join(", ")));
            }
            // The clips, numbered — this numbering is the address `another_take` and
            // `write_again` take, so it is printed rather than implied.
            if let Some(clips) = track.kind.note_clips() {
                for (index, clip) in clips.iter().enumerate() {
                    let last = (clip.start + clip.length - Ticks(1)).max_zero();
                    let origin = match &clip.recipe {
                        Some(recipe) => {
                            let edited = match session.clip_hand_edited(clip.id) {
                                true => ", edited by hand",
                                false => "",
                            };
                            format!(
                                "generated ({}, seed {}{edited})",
                                recipe.preset.name(),
                                recipe.seed
                            )
                        }
                        None => "written by hand".to_string(),
                    };
                    text.push_str(&format!(
                        "      [{}] '{}' bars {}-{} — {origin}\n",
                        index + 1,
                        clip.name,
                        project.signatures.bar_of(clip.start),
                        project.signatures.bar_of(last),
                    ));
                }
            }
        }
        if !project.master.effects.is_empty() {
            let chain: Vec<&str> = project
                .master
                .effects
                .iter()
                .map(|slot| slot.effect_id.as_str())
                .collect();
            text.push_str(&format!(
                "    {:<20} fx: {}\n",
                "master",
                chain.join(" -> ")
            ));
        }

        for path in &missing {
            text.push_str(&format!(
                "Note: the audio file {} is missing; its track plays silent.\n",
                path.display()
            ));
        }
        match session.saved_by_another_build() {
            Some("") => text.push_str("Note: this project was saved by an older build.\n"),
            Some(version) => {
                text.push_str(&format!(
                    "Note: this project was saved by build {version}.\n"
                ));
            }
            None => {}
        }
        Ok(text.trim_end().to_string())
    }
}

/// Listening, as numbers.
pub mod analyze {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "analyze";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Listens to a project and reports what it measured, changing \
        nothing: length, integrated loudness and peaks for the whole mix, the same per named \
        section — the piece's dynamic arc as numbers — and, with `per_track`, each track alone. \
        This is the ears of the improve loop: render, analyze, edit the spec or rewrite one \
        clip, and ask again.";

    /// Arguments to `analyze`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to listen to — an absolute path to a `.auris` file.
        pub project: String,
        /// Also measure every track alone, through the buses it feeds. Costs one render per
        /// track, so ask only when the question is about the balance.
        #[serde(default)]
        pub per_track: bool,
    }

    /// Open, listen, report.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = headless()?;
        let missing = session
            .open(Path::new(&args.project))
            .map_err(|error| error.to_string())?;
        let report = session
            .analyze(args.per_track)
            .map_err(|error| error.to_string())?;

        let mut text = String::new();
        for path in &missing {
            text.push_str(&format!(
                "Note: the audio file {} is missing; its track rendered silent.\n",
                path.display()
            ));
        }
        text.push_str(&analysis_text(&report));
        Ok(text.trim_end().to_string())
    }
}

/// A different take of a generated clip.
pub mod another_take {
    /// The tool's name at every door.
    pub const NAME: &str = "another_take";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Writes another take of a generated clip: the same ask, the \
        next seed, different notes. The change is saved into the project — render again to hear \
        it. Aim it with `track` and the clip number `describe` shows; without a number, every \
        generated clip on the track gets a new take. Every answer names its seed, and passing \
        `seed` takes that exact take again — how a rewrite that measured worse is rolled back.";

    /// The shared rewrite address.
    pub use crate::RegenerateArgs as Args;

    /// The next seed — or a named one.
    pub fn run(args: &Args) -> Result<String, String> {
        crate::regenerate(args, crate::Take::Another)
    }
}

/// The same take, following the harmony as it stands now.
pub mod write_again {
    /// The tool's name at every door.
    pub const NAME: &str = "write_again";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Writes a generated clip again with its own seed, following \
        the key and chords as they stand now — the tool to reach for after changing the harmony \
        under an existing piece. The change is saved into the project. Addressed exactly like \
        `another_take`.";

    /// The shared rewrite address.
    pub use crate::RegenerateArgs as Args;

    /// The same seed, the current harmony.
    pub fn run(args: &Args) -> Result<String, String> {
        crate::regenerate(args, crate::Take::Same)
    }
}

/// Keeping a progression under a name.
pub mod teach_progression {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "teach_progression";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Keeps a chord progression under a name on this machine. It \
        then shows up in `list_progressions` and the desktop picker; a specification still \
        writes the chords out in full — only the built-in catalogue is quotable as `@name`, so \
        a document stays portable.";

    /// Arguments to `teach_progression`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The name to keep the progression under.
        pub name: String,
        /// The chords, as bars of roman numerals — e.g. `| i | bVII | IVmaj7 | v7 |`.
        pub chords: String,
        /// Which mode the numerals are written against: "major" or "minor". Left out, the
        /// progression is read against whatever key a song is in.
        pub mode: Option<String>,
    }

    /// Parses the chart and keeps it in the book.
    pub fn run(args: &Args) -> Result<String, String> {
        let chart = Chart::parse(&args.chords).ok_or_else(|| {
            format!(
                "could not read '{}' as chords — write bars of roman numerals, like \
                 \"| i | bVII | IVmaj7 | v7 |\"",
                args.chords
            )
        })?;
        let mode = match args.mode.as_deref() {
            None => None,
            Some("major") => Some(ChartMode::Major),
            Some("minor") => Some(ChartMode::Minor),
            Some(other) => {
                return Err(format!("mode is \"major\" or \"minor\", not \"{other}\""));
            }
        };
        let mut book = auris_session::progressions::ProgressionBook::load();
        if !book.keep(&args.name, &chart, mode) {
            return Err(format!(
                "'{}' cannot be kept under that name — it is empty, or the built-in catalogue \
                 already uses it",
                args.name
            ));
        }
        book.save().map_err(|error| error.to_string())?;
        Ok(format!(
            "Kept '{}' — {chart}. It now shows in `list_progressions` on this machine; a \
             specification still writes the chords out in full, which is what keeps a document \
             portable.",
            args.name
        ))
    }
}

/// Forgetting a kept progression.
pub mod forget_progression {
    /// The tool's name at every door.
    pub const NAME: &str = "forget_progression";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Forgets a progression kept with `teach_progression`, by name.";

    /// Arguments to `forget_progression`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The kept progression to forget, by name.
        pub name: String,
    }

    /// Drops the entry and saves the book.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut book = auris_session::progressions::ProgressionBook::load();
        if !book.forget(&args.name) {
            return Err(format!("nothing is kept under '{}'", args.name));
        }
        book.save().map_err(|error| error.to_string())?;
        Ok(format!("Forgot '{}'.", args.name))
    }
}

/// The quotable progressions.
pub mod list_progressions {
    /// The tool's name at every door.
    pub const NAME: &str = "list_progressions";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Lists the chord progressions a specification can quote by \
        name, with the chords each one plays.";

    /// The catalogue, and what this machine has been taught.
    pub fn run() -> String {
        let mut text =
            String::from("Progressions a spec quotes by name, as `chords = \"@name\"`:\n");
        for entry in auris_session::prelude::progression_catalog() {
            text.push_str(&format!("  @{:<14} {}\n", entry.name, entry.description));
            text.push_str(&format!("  {:<15} {}\n", "", entry.chart));
        }
        // The ones this installation has been taught, listed apart because the difference
        // matters: a document saying `@axis` is portable, one quoting a kept name needs the
        // same catalogue.
        let book = auris_session::progressions::ProgressionBook::load();
        if !book.entries().is_empty() {
            text.push_str("Kept on this machine only — quote the chords, not the name:\n");
            for entry in book.entries() {
                text.push_str(&format!("  {:<15} {}\n", entry.name, entry.chart));
            }
        }
        text.trim_end().to_string()
    }
}

/// The whole songs a spec can start from.
pub mod list_presets {
    /// The tool's name at every door.
    pub const NAME: &str = "list_presets";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Lists the whole songs a specification can start from, with \
        each one's key, tempo and groove.";

    /// Every shipped preset, one line of character each.
    pub fn run() -> String {
        let mut text = String::from("Styles `compose` and `check_spec` accept as `preset`:\n");
        for preset in auris_session::prelude::PRESETS {
            let spec = preset.spec();
            text.push_str(&format!("  {:<13} {}\n", preset.name, preset.description));
            text.push_str(&format!(
                "  {:<13} {} · {:.0} BPM · {}\n",
                "",
                spec.key.to_text(),
                spec.tempo,
                spec.groove
            ));
        }
        text.trim_end().to_string()
    }
}

/// A session with no audio device and no GPU, with the shipped SoundFonts.
///
/// The fonts for the same reason `auris compose` loads them: `compose` here and **Compose a
/// Song…** in the window have to write the same piece, and half the instruments a piece asks
/// for are in that library.
fn headless() -> Result<Session, String> {
    Session::new(SessionOptions::headless().with_shipped_fonts(true))
        .map_err(|error| error.to_string())
}

/// Reads [`SpecArgs`] into the one [`SongSpec`] both spec tools mean by it.
fn resolve_spec(args: &SpecArgs) -> Result<SongSpec, String> {
    let text = match (&args.spec, &args.preset) {
        (Some(_), Some(_)) => {
            return Err("pass either `spec` or `preset`, not both".to_string());
        }
        (Some(text), None) => text.clone(),
        (None, Some(name)) => auris_session::prelude::preset(name)
            .ok_or_else(|| format!("no preset is named '{name}' — `list_presets` names them all"))?
            .source
            .to_string(),
        (None, None) => {
            return Err(
                "pass a specification: `spec` with the .asong text, or `preset` with a name \
                 from `list_presets`"
                    .to_string(),
            );
        }
    };
    let overrides: Vec<(String, String)> = args
        .overrides
        .iter()
        .flatten()
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect();
    SongSpec::parse_with_overrides(&text, &overrides).map_err(|errors| {
        let mut message = String::from("the specification was rejected:");
        for error in errors {
            message.push_str(&format!("\n  {error}"));
        }
        message
    })
}

/// One rendered file, reported: where it is and what a listener will find in it.
fn wrote_line(path: &Path, summary: &ExportSummary, settings: &WavExportSettings) -> String {
    format!(
        "Wrote {} — {}, {} ch, {}-bit, peak {:.1} dBFS.\n",
        path.display(),
        Seconds(summary.seconds).format_clock(),
        summary.channels,
        settings.bit_depth.bits(),
        summary.peak_db,
    )
}

/// One loudness, spelt for a reader — a part that made no sound says so.
fn lufs_text(lufs: Option<f32>) -> String {
    lufs.map_or_else(|| "silent".to_string(), |lufs| format!("{lufs:.1} LUFS"))
}

/// [`auris_session::MixAnalysis`], laid out as the tables a reader scans.
fn analysis_text(report: &auris_session::MixAnalysis) -> String {
    let mut text = format!(
        "The mix — {}, {}, peak {:.1} dBFS (true peak {:.1}).\n",
        Seconds(report.seconds).format_clock(),
        lufs_text(report.lufs),
        report.peak_db,
        report.true_peak_db,
    );
    if !report.sections.is_empty() {
        text.push_str("By section:\n");
        for section in &report.sections {
            // The second chorus is named as the second, because the two differing is usually
            // the very question being asked.
            let label = match section.instance {
                1 => section.label.clone(),
                instance => format!("{} ({instance})", section.label),
            };
            text.push_str(&format!(
                "  {:<14} bars {:>3}-{:<3}  {:>11}  peak {:.1} dBFS\n",
                label,
                section.start_bar,
                section.end_bar,
                lufs_text(section.lufs),
                section.peak_db,
            ));
        }
    }
    if !report.tracks.is_empty() {
        text.push_str("Each track alone, through its buses:\n");
        for track in &report.tracks {
            text.push_str(&format!(
                "  {:<14} {:>11}\n",
                track.name,
                lufs_text(track.lufs)
            ));
        }
    }
    text
}

/// Which seed a rewritten clip keeps — the difference between the two rewrite tools.
#[derive(Clone, Copy)]
enum Take {
    /// The next seed: different notes for the same ask.
    Another,
    /// The same seed: the same take, following the harmony as it stands now.
    Same,
}

/// The work behind `another_take` and `write_again`.
fn regenerate(args: &RegenerateArgs, take: Take) -> Result<String, String> {
    let mut session = headless()?;
    session
        .open(Path::new(&args.project))
        .map_err(|error| error.to_string())?;

    let track = session
        .project()
        .tracks
        .iter()
        .find(|track| track.name.eq_ignore_ascii_case(&args.track))
        .ok_or_else(|| {
            let names: Vec<&str> = session
                .project()
                .tracks
                .iter()
                .map(|track| track.name.as_str())
                .collect();
            format!(
                "no track is named '{}' — this project has: {}",
                args.track,
                names.join(", ")
            )
        })?;
    let clips: Vec<(usize, ClipId, String, bool)> = track
        .kind
        .note_clips()
        .ok_or_else(|| format!("'{}' holds no note clips", track.name))?
        .iter()
        .enumerate()
        .map(|(index, clip)| (index + 1, clip.id, clip.name.clone(), clip.recipe.is_some()))
        .collect();

    let chosen: Vec<(usize, ClipId, String)> = match args.clip {
        Some(wanted) => {
            let (index, id, name, generated) = clips
                .iter()
                .find(|(index, ..)| *index == wanted)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "'{}' has {} clips, numbered as `describe` shows them — there is no [{wanted}]",
                        args.track,
                        clips.len()
                    )
                })?;
            if !generated {
                return Err(format!(
                    "clip [{index}] '{name}' was not generated — it carries no recipe, so \
                     there is nothing to write again"
                ));
            }
            vec![(index, id, name)]
        }
        None => {
            let generated: Vec<_> = clips
                .iter()
                .filter(|(.., generated)| *generated)
                .map(|(index, id, name, _)| (*index, *id, name.clone()))
                .collect();
            if generated.is_empty() {
                return Err(format!("'{}' has no generated clips", args.track));
            }
            generated
        }
    };

    if matches!(take, Take::Same) && args.seed.is_some() {
        return Err(
            "write_again keeps the clip's own seed — use another_take with `seed` to choose one"
                .to_string(),
        );
    }

    let mut text = String::new();
    for (index, id, name) in &chosen {
        // Asked before the rewrite, because writing the clip again is exactly what resets
        // the measurement that knows.
        let edited = session.clip_hand_edited(*id);
        let notes = match (take, args.seed) {
            // The named seed, so a take that measured better two rewrites ago is not lost
            // behind a counter that only advances.
            (Take::Another, Some(seed)) => {
                let recipe = session
                    .clip_recipe(*id)
                    .expect("only generated clips were chosen above")
                    .with_seed(seed);
                session.set_clip_recipe(*id, recipe)
            }
            (Take::Another, None) => session.reroll_clip(*id),
            (Take::Same, _) => session.regenerate_clip(*id),
        }
        .map_err(|error| error.to_string())?;
        let seed = session
            .clip_recipe(*id)
            .map_or_else(String::new, |recipe| format!(", seed {}", recipe.seed));
        text.push_str(&format!("[{index}] '{name}' — {notes} notes{seed}.\n"));
        if edited {
            text.push_str(&format!(
                "Note: [{index}] had been edited by hand; those edits are gone.\n"
            ));
        }
    }
    session.save_in_place().map_err(|error| error.to_string())?;
    text.push_str("Saved. Render again to hear it.");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_args(spec: Option<&str>, preset: Option<&str>) -> SpecArgs {
        SpecArgs {
            spec: spec.map(String::from),
            preset: preset.map(String::from),
            overrides: None,
        }
    }

    #[test]
    fn a_spec_needs_exactly_one_source() {
        let both = resolve_spec(&spec_args(Some("title = \"A\""), Some("city-pop")));
        assert!(both.unwrap_err().contains("not both"));
        let neither = resolve_spec(&spec_args(None, None));
        assert!(neither.unwrap_err().contains("spec"));
    }

    #[test]
    fn an_unknown_preset_points_at_the_list() {
        let error = resolve_spec(&spec_args(None, Some("polka"))).unwrap_err();
        assert!(error.contains("list_presets"), "{error}");
    }

    #[test]
    fn overrides_land_on_a_preset() {
        let name = auris_session::prelude::PRESETS[0].name;
        let mut args = spec_args(None, Some(name));
        args.overrides = Some(BTreeMap::from([("tempo".to_string(), "96".to_string())]));
        let spec = resolve_spec(&args).unwrap();
        assert!((spec.tempo - 96.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_rejected_spec_reports_its_lines() {
        // An unknown field is refused rather than ignored, and the syntax reader knows lines.
        let error = resolve_spec(&spec_args(Some("colour = 3"), None)).unwrap_err();
        assert!(error.contains("rejected"), "{error}");
        assert!(error.contains("line"), "{error}");
    }

    #[test]
    fn the_vocabulary_lists_answer_with_real_entries() {
        assert!(list_progressions::run().contains("@axis"));
        assert!(list_presets::run().contains(auris_session::prelude::PRESETS[0].name));
    }

    #[test]
    fn bad_chords_and_bad_modes_are_refused_with_directions() {
        // Neither reaches the book, so nothing on the machine changes under a test.
        let unreadable = teach_progression::run(&teach_progression::Args {
            name: "test".to_string(),
            chords: "definitely not chords !!".to_string(),
            mode: None,
        })
        .unwrap_err();
        assert!(unreadable.contains("roman numerals"), "{unreadable}");
        let sideways = teach_progression::run(&teach_progression::Args {
            name: "test".to_string(),
            chords: "| i | iv |".to_string(),
            mode: Some("sideways".to_string()),
        })
        .unwrap_err();
        assert!(sideways.contains("major"), "{sideways}");
    }

    /// The whole loop a model would run, against one four-bar piece: compose it, read the
    /// project back, render it. One test rather than three because the compose is the slow
    /// part — it listens to what it wrote — and each stage here feeds the next its file.
    #[test]
    fn the_whole_loop_composes_describes_and_renders() {
        let root = std::env::temp_dir().join(format!("auris-toolbox-loop-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        let spec = r#"
            title = "Loop"
            form = "verse"
            chords = "@axis"
            [section.verse]
            bars = 4
        "#;
        let report = compose::run(&compose::Args {
            spec: spec_args(Some(spec), None),
            output: root.join("Loop.auris").display().to_string(),
            force: false,
        })
        .unwrap();
        assert!(report.contains("Wrote"), "{report}");
        assert!(report.contains("seed"), "{report}");

        // `save_as` nests the document in a folder of its own.
        let document = root.join("Loop").join("Loop.auris");
        let described = describe::run(&describe::Args {
            project: document.display().to_string(),
        })
        .unwrap();
        assert!(described.contains("Loop"), "{described}");
        assert!(described.contains("tempo"), "{described}");
        assert!(described.contains("instrument"), "{described}");
        // The clips are numbered and marked, because that numbering is the rewrite address.
        assert!(described.contains("[1]"), "{described}");
        assert!(described.contains("generated"), "{described}");

        // The ears: the mix and its one section, measured off a render that changes nothing.
        let heard = analyze::run(&analyze::Args {
            project: document.display().to_string(),
            per_track: false,
        })
        .unwrap();
        assert!(heard.contains("LUFS"), "{heard}");
        assert!(heard.contains("By section"), "{heard}");

        // A new take of one part, addressed the way describe numbers it, lands in the file.
        let before = std::fs::read_to_string(&document).unwrap();
        let take_of_lead = |clip: Option<usize>, seed: Option<u64>| {
            another_take::run(&RegenerateArgs {
                project: document.display().to_string(),
                track: "lead".to_string(),
                clip,
                seed,
            })
        };
        let took = take_of_lead(Some(1), None).unwrap();
        assert!(took.contains("seed"), "{took}");
        assert!(took.contains("Saved"), "{took}");
        assert_ne!(
            before,
            std::fs::read_to_string(&document).unwrap(),
            "another take was saved into the project"
        );

        // A take the numbers liked is not lost behind the advancing counter: every answer
        // names its seed, and naming it back restores the take byte for byte.
        let at = took.find("seed ").unwrap() + "seed ".len();
        let liked: u64 = took[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap();
        let kept = std::fs::read_to_string(&document).unwrap();
        take_of_lead(Some(1), None).unwrap();
        assert_ne!(kept, std::fs::read_to_string(&document).unwrap());
        let back = take_of_lead(Some(1), Some(liked)).unwrap();
        assert!(back.contains(&format!("seed {liked}")), "{back}");
        assert_eq!(
            kept,
            std::fs::read_to_string(&document).unwrap(),
            "naming the seed brings the earlier take back exactly"
        );

        let missing = another_take::run(&RegenerateArgs {
            project: document.display().to_string(),
            track: "nobody".to_string(),
            clip: None,
            seed: None,
        })
        .unwrap_err();
        assert!(
            missing.contains("lead"),
            "the refusal lists the real tracks: {missing}"
        );

        // Composing over the same folder again is a refusal until `force` says otherwise.
        let refused = compose::run(&compose::Args {
            spec: spec_args(Some(spec), None),
            output: root.join("Loop.auris").display().to_string(),
            force: false,
        })
        .unwrap_err();
        assert!(refused.contains("force"), "{refused}");

        let wav = root.join("loop.wav");
        let rendered = render::run(&render::Args {
            project: document.display().to_string(),
            output: Some(wav.display().to_string()),
            bit_depth: Some(16),
            stems: None,
        })
        .unwrap();
        assert!(rendered.contains("16-bit"), "{rendered}");
        assert!(wav.exists());

        std::fs::remove_dir_all(&root).unwrap();
    }
}
