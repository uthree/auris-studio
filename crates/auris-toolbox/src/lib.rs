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
    The mix itself has its own smaller loop: `mixer` reads every fader, send and effect \
    parameter, `set_level` / `set_send` / `set_effect` move one, and `section_gain` holds one \
    section's gain at a level — often the better instrument than rewriting notes when `analyze` \
    says a section is too loud, a part is buried, or the master limiter is pinned. \
    The arrangement itself can be edited in place: `add_track` puts a new track in an existing \
    project (`list_instruments` names the built-in instruments, and any General MIDI sound can \
    be asked for by name), `add_part` writes a generated part onto a track from the harmony \
    underneath, `set_instrument` re-voices a track, and `rename_track` / `remove_track` do what \
    they say — so one more part is an edit, not a recomposition. Notes can be placed one by \
    one: `add_clip` opens an empty clip, `edit_notes` adds and removes notes in it by name and \
    bar, `notes` reads a clip back numbered — and `accompany` reads a melody clip and writes \
    the key, the chords and a backing band under it, which is the melody-first way around: \
    write the tune by hand, derive the accompaniment. \
    A song can also sing: `add_track` with kind \"singer\" holds notes that carry lyrics, \
    `write_lyrics` lays a phrase across a clip's notes one syllable each, and `sing` renders \
    the track through a voice model into a take that playback and `render` play — pass \
    `voice` the first time to choose the model. `compose_lyrics` is the words-first way \
    around: give it Japanese lyrics and it writes the melody under them — following the \
    lyric's pitch accent where a Japanese dictionary is configured — with chords and a band, \
    saved as a new project ready to `sing`. \
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

/// The tools that write the project file they are aimed at, by wire name.
///
/// For a host watching the conversation from beside an open document — the desktop's agent
/// panel — this is how it knows the file under it may have moved after a call succeeds.
/// `render` is absent because it writes WAV files beside the project, and the progression
/// tools because they write the machine's own book; neither touches a document.
pub const WRITES_PROJECTS: &[&str] = &[
    compose::NAME,
    compose_lyrics::NAME,
    another_take::NAME,
    write_again::NAME,
    set_level::NAME,
    set_send::NAME,
    set_effect::NAME,
    section_gain::NAME,
    add_track::NAME,
    add_part::NAME,
    set_instrument::NAME,
    rename_track::NAME,
    remove_track::NAME,
    add_clip::NAME,
    edit_notes::NAME,
    accompany::NAME,
    write_lyrics::NAME,
    sing::NAME,
];

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
        // Absolute in the answer even when the ask was relative: this line is what every
        // later tool call gets copied from, wherever the host process happens to sit.
        let written = std::path::absolute(&written).unwrap_or(written);

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
        if report.sung > 0 {
            text.push_str(&format!(
                "\nThe lyrics became {} sung notes on a Vocal track — `sing` gives it a voice.",
                report.sung
            ));
        }
        for section in &report.unsung {
            text.push_str(&format!(
                "\nNote: section '{section}' has lyrics no dictionary here can read; it plays \
                 instrumentally."
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
        let source = resolve_project(&args.project)?;
        let source = source.as_path();
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
            let folder = std::path::absolute(&folder).unwrap_or(folder);
            protect_project_assets(&session, &folder, true)?;
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
            // Absolute in the answer for the same reason `compose` answers absolute: the
            // caller reads this line to find the file.
            let output = std::path::absolute(&output).unwrap_or(output);
            protect_project_assets(&session, &output, false)?;
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
        let path = resolve_project(&args.project)?;
        let mut session = headless()?;
        let missing = session.open(&path).map_err(|error| error.to_string())?;
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
            .open(&resolve_project(&args.project)?)
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

/// The mixer, read out loud.
pub mod mixer {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "mixer";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Reads the mixer as it stands: every track's fader, pan, \
        mute and solo, its sends, and each effect's parameters with key, value and range — the \
        vocabulary `set_level`, `set_send` and `set_effect` move. A control marked `[automated]` \
        is driven by its lane, not its stored value.";

    /// Arguments to `mixer`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to read — an absolute path to a `.auris` file.
        pub project: String,
    }

    /// One strip's worth of structure, copied out so the parameter pass can borrow the
    /// session mutably.
    struct Row {
        track: Option<TrackId>,
        name: String,
        gain_db: f32,
        pan: f32,
        mute: bool,
        solo: bool,
        sends: Vec<(String, f32, bool)>,
        effects: Vec<(EffectSlotId, String)>,
    }

    /// Faders, pans, sends and every effect parameter, as one table.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;

        let rows: Vec<Row> = {
            let project = session.project();
            let name_of = |id: TrackId| {
                project
                    .track(id)
                    .map_or_else(|| format!("#{}", id.0), |track| track.name.clone())
            };
            let mut rows: Vec<Row> = project
                .tracks
                .iter()
                .map(|track| Row {
                    track: Some(track.id),
                    name: track.name.clone(),
                    gain_db: track.mixer.gain_db,
                    pan: track.mixer.pan,
                    mute: track.mixer.mute,
                    solo: track.mixer.solo,
                    sends: track
                        .sends
                        .iter()
                        .map(|send| (name_of(send.target), send.level_db, send.pre_fader))
                        .collect(),
                    effects: track
                        .mixer
                        .effects
                        .iter()
                        .map(|slot| (slot.id, slot.effect_id.clone()))
                        .collect(),
                })
                .collect();
            rows.push(Row {
                track: None,
                name: "master".to_string(),
                gain_db: project.master.gain_db,
                pan: project.master.pan,
                mute: false,
                solo: false,
                sends: Vec::new(),
                effects: project
                    .master
                    .effects
                    .iter()
                    .map(|slot| (slot.id, slot.effect_id.clone()))
                    .collect(),
            });
            rows
        };

        let mut text = String::new();
        for row in rows {
            let (gain_target, pan_target) = match row.track {
                Some(id) => (ParamTarget::TrackGain(id), ParamTarget::TrackPan(id)),
                None => (ParamTarget::MasterGain, ParamTarget::MasterPan),
            };
            let mut flags = Vec::new();
            if row.mute {
                flags.push("muted");
            }
            if row.solo {
                flags.push("solo");
            }
            if session.is_automated(gain_target) {
                flags.push("[gain automated]");
            }
            if session.is_automated(pan_target) {
                flags.push("[pan automated]");
            }
            let flags = match flags.is_empty() {
                true => String::new(),
                false => format!("  {}", flags.join(", ")),
            };
            text.push_str(&format!(
                "{:<16} {:+.1} dB, pan {:+.2}{flags}\n",
                row.name, row.gain_db, row.pan
            ));
            for (target, level, pre) in &row.sends {
                let tap = if *pre { " (pre-fader)" } else { "" };
                text.push_str(&format!("  => {target} {level:+.1} dB{tap}\n"));
            }
            for (position, (slot, effect_id)) in row.effects.iter().enumerate() {
                text.push_str(&format!("  fx [{}] {effect_id}:\n", position + 1));
                let mut index = 0u32;
                loop {
                    let target = ParamTarget::Effect {
                        track: row.track,
                        slot: *slot,
                        param: ParamId(index),
                    };
                    let Some(descriptor) = session.descriptor_for(target) else {
                        break;
                    };
                    let value = session.param_value(target, &descriptor);
                    let automated = match session.is_automated(target) {
                        true => "  [automated]",
                        false => "",
                    };
                    text.push_str(&format!(
                        "    {:<14} {}  ({} to {}{}){automated}\n",
                        descriptor.key,
                        number(value, descriptor.unit),
                        trimmed(descriptor.min),
                        trimmed(descriptor.max),
                        unit_suffix(descriptor.unit),
                    ));
                    if !descriptor.choices.is_empty() {
                        let listed: Vec<String> = descriptor
                            .choices
                            .iter()
                            .enumerate()
                            .map(|(index, label)| format!("{index}={label}"))
                            .collect();
                        text.push_str(&format!("    {:<14} {}\n", "", listed.join(" ")));
                    }
                    index += 1;
                }
            }
        }
        Ok(text.trim_end().to_string())
    }
}

/// A fader or a pan, moved.
pub mod set_level {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "set_level";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Sets a track's fader and/or pan; `track` may be \"master\". \
        Gain runs -60 to +12 dB, pan -1 (left) to +1 (right). The change is saved — `analyze` \
        again to hear what it did to the numbers. A fader that `mixer` marks `[automated]` is \
        ruled by its lane, not this value; `section_gain` with clear: true removes the lane.";

    /// Arguments to `set_level`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track whose strip to move, by name — or "master" for the master bus.
        pub track: String,
        /// Where to put the fader, in decibels (-60 to +12). Left out, the fader stays.
        pub gain_db: Option<f32>,
        /// Where to put the pan, -1 to +1. Left out, the pan stays.
        pub pan: Option<f32>,
    }

    /// Moves the fader and/or the pan, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.gain_db.is_none() && args.pan.is_none() {
            return Err("pass gain_db, pan, or both — there is nothing else here to set".into());
        }
        let mut session = opened(&args.project)?;
        let strip = strip_by_name(session.project(), &args.track)?;
        let (gain_target, pan_target) = match strip {
            Some(id) => (ParamTarget::TrackGain(id), ParamTarget::TrackPan(id)),
            None => (ParamTarget::MasterGain, ParamTarget::MasterPan),
        };

        let mut notes = String::new();
        if let Some(gain) = args.gain_db {
            if !(-60.0..=12.0).contains(&gain) {
                return Err(format!(
                    "gain_db runs -60 to +12 dB; {gain} is outside that"
                ));
            }
            if session.is_automated(gain_target) {
                notes.push_str(
                    "\nNote: a lane is driving this fader, so the stored position is not what \
                     plays — `section_gain` with clear: true removes the lane.",
                );
            }
            if strip.is_none() && gain > 0.0 {
                notes.push_str(
                    "\nNote: the master fader sits after the master chain, so a boost here is \
                     not limited — watch the peak in `analyze`.",
                );
            }
            session.set_param(gain_target, gain);
        }
        if let Some(pan) = args.pan {
            if !(-1.0..=1.0).contains(&pan) {
                return Err(format!("pan runs -1 to +1; {pan} is outside that"));
            }
            session.set_param(pan_target, pan);
        }
        session.save_in_place().map_err(|error| error.to_string())?;

        let (gain, pan) = match strip {
            Some(id) => {
                let track = session
                    .project()
                    .track(id)
                    .expect("the track was just moved");
                (track.mixer.gain_db, track.mixer.pan)
            }
            None => (
                session.project().master.gain_db,
                session.project().master.pan,
            ),
        };
        Ok(format!(
            "{} — fader {gain:+.1} dB, pan {pan:+.2}. Saved.{notes}",
            args.track
        ))
    }
}

/// A send level, moved.
pub mod set_send {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "set_send";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Sets how much of a track one of its sends carries, \
        addressed by the bus it feeds — the routing `mixer` and `describe` show. Send levels \
        run -60 to 0 dB; there is no headroom above unity on a send. The change is saved.";

    /// Arguments to `set_send`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track the send is taken from, by name.
        pub track: String,
        /// The bus the send feeds, by name — `mixer` lists each track's sends.
        pub to: String,
        /// How much to send, in decibels (-60 to 0).
        pub level_db: f32,
    }

    /// Finds the send by the bus it feeds and moves it.
    pub fn run(args: &Args) -> Result<String, String> {
        if !(-60.0..=0.0).contains(&args.level_db) {
            return Err(format!(
                "send levels run -60 to 0 dB; {} is outside that",
                args.level_db
            ));
        }
        let mut session = opened(&args.project)?;
        let (track_id, send_id) = {
            let project = session.project();
            let track = track_by_name(project, &args.track)?;
            let named: Vec<(SendId, String)> = track
                .sends
                .iter()
                .map(|send| {
                    let name = project
                        .track(send.target)
                        .map_or_else(|| format!("#{}", send.target.0), |bus| bus.name.clone());
                    (send.id, name)
                })
                .collect();
            let found = named
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(&args.to))
                .map(|(id, _)| *id)
                .ok_or_else(|| match named.is_empty() {
                    true => format!("'{}' has no sends", track.name),
                    false => format!(
                        "'{}' sends to: {} — not '{}'",
                        track.name,
                        named
                            .iter()
                            .map(|(_, name)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        args.to
                    ),
                })?;
            (track.id, found)
        };
        session
            .set_send_level(track_id, send_id, args.level_db)
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;
        Ok(format!(
            "{} => {} at {:+.1} dB. Saved.",
            args.track, args.to, args.level_db
        ))
    }
}

/// One effect parameter, moved.
pub mod set_effect {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "set_effect";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Sets one parameter of one effect, addressed the way \
        `mixer` lists them: `track` (or \"master\"), the effect by its id — or by `slot`, its \
        1-based position, when a chain holds the same effect twice — and the parameter by key \
        or name, in the parameter's own units. Values outside the range `mixer` shows are \
        refused. The change is saved. The master limiter's `input_db` is the dial to back off \
        when `analyze` says the loud sections are pinned against the ceiling.";

    /// Arguments to `set_effect`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The strip the effect sits on: a track by name, or "master".
        pub track: String,
        /// The effect's id as `mixer` lists it — the full `auris.fx.limiter` or just
        /// `limiter`. Leave out when addressing by `slot`.
        pub effect: Option<String>,
        /// The effect's 1-based position in the chain, as `mixer` numbers it — for when a
        /// chain holds the same effect twice.
        pub slot: Option<usize>,
        /// The parameter, by the key or the name `mixer` lists.
        pub param: String,
        /// The value, in the parameter's own units — decibels for a gain, milliseconds for a
        /// release.
        pub value: f32,
    }

    /// Finds the slot, finds the parameter, refuses nonsense, writes the rest.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let strip = strip_by_name(session.project(), &args.track)?;
        let (slot, effect_id) = {
            let project = session.project();
            let chain = match strip {
                Some(id) => &project.track(id).expect("the strip was just found").mixer,
                None => &project.master,
            };
            let slots: Vec<(EffectSlotId, String)> = chain
                .effects
                .iter()
                .map(|slot| (slot.id, slot.effect_id.clone()))
                .collect();
            if slots.is_empty() {
                return Err(format!("'{}' has no effects", args.track));
            }
            let listed = || {
                slots
                    .iter()
                    .enumerate()
                    .map(|(index, (_, id))| format!("[{}] {id}", index + 1))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            match (args.slot, &args.effect) {
                (Some(number), _) => slots
                    .get(number.wrapping_sub(1))
                    .cloned()
                    .ok_or_else(|| format!("'{}' has {}: no [{number}]", args.track, listed()))?,
                (None, Some(name)) => {
                    let matches: Vec<&(EffectSlotId, String)> = slots
                        .iter()
                        .filter(|(_, id)| {
                            id.eq_ignore_ascii_case(name)
                                || id
                                    .rsplit('.')
                                    .next()
                                    .is_some_and(|last| last.eq_ignore_ascii_case(name))
                        })
                        .collect();
                    match matches.as_slice() {
                        [one] => (*one).clone(),
                        [] => {
                            return Err(format!(
                                "no effect on '{}' answers to '{name}' — the chain is: {}",
                                args.track,
                                listed()
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "'{name}' sits on '{}' more than once — address it as slot: N \
                                 the way `mixer` numbers the chain: {}",
                                args.track,
                                listed()
                            ));
                        }
                    }
                }
                (None, None) => {
                    return Err("pass `effect` (its id) or `slot` (its position)".into());
                }
            }
        };

        // The parameter, by key or by name, with the refusal listing what is really there.
        let mut keys = Vec::new();
        let mut found = None;
        let mut index = 0u32;
        loop {
            let target = ParamTarget::Effect {
                track: strip,
                slot,
                param: ParamId(index),
            };
            let Some(descriptor) = session.descriptor_for(target) else {
                break;
            };
            keys.push(descriptor.key.to_string());
            if descriptor.key.eq_ignore_ascii_case(&args.param)
                || descriptor.name.eq_ignore_ascii_case(&args.param)
            {
                found = Some((target, descriptor));
                break;
            }
            index += 1;
        }
        let Some((target, descriptor)) = found else {
            return Err(format!(
                "{effect_id} has no parameter '{}' — it has: {}",
                args.param,
                keys.join(", ")
            ));
        };
        if !(descriptor.min..=descriptor.max).contains(&args.value) {
            return Err(format!(
                "{} runs {} to {}{}; {} is outside that",
                descriptor.key,
                trimmed(descriptor.min),
                trimmed(descriptor.max),
                unit_suffix(descriptor.unit),
                args.value
            ));
        }

        let before = session.param_value(target, &descriptor);
        let automated = session.is_automated(target);
        session.set_param(target, args.value);
        session.save_in_place().map_err(|error| error.to_string())?;

        let mut text = format!(
            "{effect_id} {}: {} -> {}. Saved.",
            descriptor.key,
            number(before, descriptor.unit),
            number(args.value, descriptor.unit),
        );
        if automated {
            text.push_str(
                "\nNote: a lane is driving this parameter, so the stored value is not what \
                 plays until the lane is cleared.",
            );
        }
        Ok(text)
    }
}

/// One section, held at a level.
pub mod section_gain {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "section_gain";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Holds a track's gain at a level across one named section — \
        dynamics without rewriting a note. `track` may be \"master\"; the section is addressed \
        by the label `analyze` shows, every occurrence unless `instance` picks one. Writes gain \
        automation with short ramps at the edges: the fader keeps ruling outside the stretch, \
        and holds on different sections compose. `clear: true` removes the track's whole gain \
        lane instead, giving the fader back everywhere. The change is saved. The master fader \
        sits after the master chain, so a boost there is not limited and can clip — widen \
        contrast by holding the louder sections down instead.";

    /// Arguments to `section_gain`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track whose gain to hold, by name — or "master".
        pub track: String,
        /// The section to hold, by label as `analyze` shows it.
        pub section: Option<String>,
        /// Which occurrence of the label, counting from 1 — every one when left out.
        pub instance: Option<usize>,
        /// The level to hold through the section, in decibels (-60 to +12).
        pub gain_db: Option<f32>,
        /// Remove the track's gain lane instead, giving the fader back everywhere.
        #[serde(default)]
        pub clear: bool,
    }

    /// Holds the span — or takes the whole lane back.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let strip = strip_by_name(session.project(), &args.track)?;
        let target = match strip {
            Some(id) => ParamTarget::TrackGain(id),
            None => ParamTarget::MasterGain,
        };

        if args.clear {
            if !session.clear_automation(target) {
                return Err(format!("nothing is automated on '{}'", args.track));
            }
            session.save_in_place().map_err(|error| error.to_string())?;
            return Ok(format!(
                "The gain lane on '{}' is gone; the fader rules everywhere again. Saved.",
                args.track
            ));
        }
        let Some(label) = &args.section else {
            return Err("pass `section` and `gain_db`, or clear: true".into());
        };
        let Some(gain) = args.gain_db else {
            return Err("pass `gain_db` — the level to hold through the section".into());
        };
        if !(-60.0..=12.0).contains(&gain) {
            return Err(format!(
                "gain_db runs -60 to +12 dB; {gain} is outside that"
            ));
        }

        let spans: Vec<(Ticks, Ticks, String, usize, u32, u32)> = {
            let project = session.project();
            let end = project.end_tick();
            let all = project.sections.spans_in(Ticks::ZERO, end);
            if all.is_empty() {
                return Err("this project has no named sections to hold".into());
            }
            let chosen: Vec<_> = all
                .iter()
                .filter(|span| span.label.eq_ignore_ascii_case(label))
                .filter(|span| args.instance.is_none_or(|wanted| span.instance == wanted))
                .map(|span| {
                    (
                        span.start,
                        span.end,
                        span.label.clone(),
                        span.instance,
                        project.signatures.bar_of(span.start),
                        project.signatures.bar_of((span.end - Ticks(1)).max_zero()),
                    )
                })
                .collect();
            if chosen.is_empty() {
                let known: Vec<String> = all
                    .iter()
                    .map(|span| match span.instance {
                        1 => span.label.clone(),
                        instance => format!("{} ({instance})", span.label),
                    })
                    .collect();
                return Err(format!(
                    "no section answers to '{label}' — this song has: {}",
                    known.join(", ")
                ));
            }
            chosen
        };

        let mut text = String::new();
        for (start, end, label, instance, first_bar, last_bar) in &spans {
            session.hold_automation(target, *start, *end, gain);
            let which = match instance {
                1 => label.clone(),
                instance => format!("{label} ({instance})"),
            };
            text.push_str(&format!(
                "{which} bars {first_bar}-{last_bar}: '{}' held at {gain:+.1} dB.\n",
                args.track
            ));
        }
        session.save_in_place().map_err(|error| error.to_string())?;
        text.push_str(
            "The fader keeps ruling outside the stretch. Saved — `analyze` will show the arc.",
        );
        // The trap the first model to use this tool walked straight into: the master fader
        // sits after the limiter, so a boost there is a boost nothing catches.
        if strip.is_none() && gain > 0.0 {
            text.push_str(
                "\nNote: the master fader sits after the master chain, so this boost is not \
                 limited — if `analyze` now shows peaks above 0 dBFS, hold the louder sections \
                 down instead.",
            );
        }
        Ok(text)
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

/// The instruments a track can be voiced with.
pub mod list_instruments {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "list_instruments";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Lists the built-in instruments a track can play, by the id \
        `add_track` and `set_instrument` take. Any General MIDI sound is also available — name \
        it in those tools' `sound` field instead, as a GM name or program number.";

    /// Every registered instrument, one line each.
    pub fn run() -> String {
        let mut text = String::from("Instruments `add_track` and `set_instrument` accept:\n");
        match headless() {
            Ok(session) => {
                for descriptor in session.registry().instruments() {
                    text.push_str(&format!("  {:<24} {}\n", descriptor.id, descriptor.name));
                }
            }
            Err(error) => text.push_str(&format!("  (unlisted: {error})\n")),
        }
        text.push_str(
            "\nOr pass `sound` instead of `instrument`: any General MIDI sound by name \
             (\"Electric Piano 1\", \"Fretless Bass\") or program number 0-127, with \
             `drums: true` to read the number as a drum kit.",
        );
        text.trim_end().to_string()
    }
}

/// A track, added to a project that already exists.
pub mod add_track {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "add_track";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Adds a track to an existing project and saves. An instrument \
        track by default — voiced by `instrument` (an id from `list_instruments`) or by `sound` \
        (a General MIDI name or program number, `drums: true` for a kit) — or, with `kind`, a \
        singer track (notes that carry lyrics, sung by a voice model), an audio track or a \
        bus. A new instrument track has no clips: `add_part` writes one.";

    /// Arguments to `add_track`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// What to call the track.
        pub name: String,
        /// A built-in instrument id from `list_instruments`. Pass this or `sound`, not both;
        /// with neither, the default instrument plays.
        pub instrument: Option<String>,
        /// A General MIDI sound instead — a name like "Electric Piano 1" or a program number
        /// 0-127, out of the shipped library.
        pub sound: Option<String>,
        /// Read `sound`'s number as a drum kit rather than a melodic program.
        #[serde(default)]
        pub drums: bool,
        /// "instrument" (the default), "singer", "audio", or "bus".
        pub kind: Option<String>,
    }

    /// Adds the track, voices it, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let kind = args.kind.as_deref().unwrap_or("instrument");
        let voiced = match kind {
            "instrument" => {
                let id = match &args.instrument {
                    Some(id) => session
                        .add_instrument_track(&args.name, id)
                        .map_err(|error| {
                            format!("{error} — `list_instruments` names the real ones")
                        })?,
                    None => session
                        .add_default_instrument_track(&args.name)
                        .map_err(|error| error.to_string())?,
                };
                voice(&mut session, id, &args.sound, args.drums, &args.instrument)?
            }
            "singer" | "audio" | "bus" => {
                if args.instrument.is_some() || args.sound.is_some() {
                    return Err(format!("a {kind} track plays no instrument — drop it"));
                }
                match kind {
                    "singer" => {
                        session.add_singer_track(&args.name);
                    }
                    "audio" => {
                        session.add_audio_track(&args.name);
                    }
                    _ => {
                        session.add_bus_track(&args.name);
                    }
                };
                kind.to_string()
            }
            other => {
                return Err(format!(
                    "`kind` is \"instrument\", \"singer\", \"audio\" or \"bus\", not \"{other}\""
                ));
            }
        };
        session.save_in_place().map_err(|error| error.to_string())?;
        let mut text = format!("Added track '{}' — {voiced}. Saved.", args.name);
        if kind == "instrument" {
            text.push_str(" The track holds no clips yet; `add_part` writes one.");
        }
        if kind == "singer" {
            text.push_str(
                " The track holds no clips yet; `add_clip` opens one, `edit_notes` places the \
                 tune, `write_lyrics` gives it words and `sing` renders the voice.",
            );
        }
        Ok(text)
    }

    /// Voices an instrument track by `sound` when one is named, answering with what plays.
    ///
    /// Shared with `set_instrument`, where the same three fields mean the same things.
    pub(super) fn voice(
        session: &mut Session,
        id: auris_session::prelude::TrackId,
        sound: &Option<String>,
        drums: bool,
        instrument: &Option<String>,
    ) -> Result<String, String> {
        if let Some(wanted) = sound {
            if instrument.is_some() {
                return Err(
                    "pass `instrument` or `sound`, not both — a sound implies the sampler".into(),
                );
            }
            let program = gm::Program::parse(wanted).ok_or_else(|| {
                format!(
                    "no General MIDI sound answers to '{wanted}' — give a name like \
                     \"Electric Piano 1\" or a program number 0-127"
                )
            })?;
            let chosen = program.sound(drums);
            session
                .set_track_general_midi(id, i32::from(chosen.bank), i32::from(chosen.patch))
                .map_err(|error| error.to_string())?;
            return Ok(format!(
                "playing {} (General MIDI {})",
                program.label(drums),
                chosen.patch
            ));
        }
        let playing = session
            .project()
            .track(id)
            .and_then(|track| track.kind.as_instrument())
            .map(|inner| inner.instrument_id.clone())
            .unwrap_or_default();
        Ok(format!("instrument {playing}"))
    }
}

/// A generated part, written onto a track that is already there.
pub mod add_part {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "add_part";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Writes a generated part onto an existing instrument track, \
        from the key and chords already under the song — lead, chords, pad, arp, bass, stab, \
        drums, kick, snare or hat. Covers the whole song unless `start_bar` and `bars` aim it. \
        The clip keeps its recipe, so `another_take` rerolls it and `write_again` follows a \
        harmony change; the answer numbers it the way `describe` does.";

    /// Arguments to `add_part`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track to write on, by name as `describe` lists it.
        pub track: String,
        /// What the part plays: lead, chords, pad, arp, bass, stab, drums, kick, snare or hat.
        pub part: String,
        /// The 1-based bar the part starts at. Bar 1 when left out.
        pub start_bar: Option<u32>,
        /// How many bars it covers. To the end of the song when left out.
        pub bars: Option<u32>,
        /// The seed of the take. 0 when left out; every answer names it, and `another_take`
        /// moves to the next.
        pub seed: Option<u64>,
    }

    /// Writes the part, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let preset = ClipPreset::parse(&args.part).ok_or_else(|| {
            let names: Vec<&str> = ClipPreset::ALL.iter().map(|preset| preset.name()).collect();
            format!(
                "no part is called '{}' — the parts are: {}",
                args.part,
                names.join(", ")
            )
        })?;
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;

        let start_bar = args.start_bar.unwrap_or(1).max(1);
        let start = session.project().signatures.bar_start(start_bar);
        let bars = match args.bars {
            Some(bars) => bounded_bars(bars, "part")?,
            None => {
                let end = session.project().end_tick();
                if end <= start {
                    return Err(format!(
                        "the song ends before bar {start_bar}, so there is nothing to cover — \
                         pass `bars` to write into silence deliberately"
                    ));
                }
                let last = session.project().signatures.bar_of(end - Ticks(1));
                bounded_bars(last - start_bar + 1, "part")?
            }
        };
        let after = bar_after(start_bar, bars)?;
        let length = session.project().signatures.bar_start(after) - start;
        let seed = args.seed.unwrap_or(0);

        let clip = session
            .generate_clip(track, start, length, ClipRecipe::new(preset, seed))
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;

        let notes = session
            .project()
            .midi_clip(clip)
            .map(|(_, midi)| midi.notes.len())
            .unwrap_or(0);
        let number = session
            .project()
            .track(track)
            .and_then(|entry| entry.kind.note_clips())
            .and_then(|clips| clips.iter().position(|entry| entry.id == clip))
            .map(|index| index + 1)
            .unwrap_or(0);
        let mut text = format!(
            "Wrote clip [{number}] '{}' on {} — bars {start_bar}-{}, {notes} notes, seed \
             {seed}. Saved.",
            preset.name(),
            args.track,
            after - 1,
        );
        if notes == 0 {
            text.push_str(
                "\nNote: no chords lie under those bars, so the clip is empty — the harmony \
                 covers the song `compose` wrote; aim the part there, or compose again longer.",
            );
        }
        Ok(text)
    }
}

/// A track's voice, changed.
pub mod set_instrument {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "set_instrument";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Re-voices an instrument track: `instrument` names a built-in \
        from `list_instruments`, or `sound` names a General MIDI sound (a name or a program \
        number, `drums: true` for a kit). The previous instrument's dial positions and the \
        automation that drove them go with it. The change is saved.";

    /// Arguments to `set_instrument`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track to re-voice, by name as `describe` lists it.
        pub track: String,
        /// A built-in instrument id from `list_instruments`. Pass this or `sound`.
        pub instrument: Option<String>,
        /// A General MIDI sound instead — a name like "Electric Piano 1" or a program number
        /// 0-127, out of the shipped library.
        pub sound: Option<String>,
        /// Read `sound`'s number as a drum kit rather than a melodic program.
        #[serde(default)]
        pub drums: bool,
    }

    /// Changes the voice, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.instrument.is_none() && args.sound.is_none() {
            return Err("pass `instrument` or `sound` — there is nothing else here to set".into());
        }
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        if let Some(id) = &args.instrument {
            if args.sound.is_some() {
                return Err(
                    "pass `instrument` or `sound`, not both — a sound implies the sampler".into(),
                );
            }
            session
                .set_track_instrument(track, id)
                .map_err(|error| format!("{error} — `list_instruments` names the real ones"))?;
        }
        let voiced = add_track::voice(&mut session, track, &args.sound, args.drums, &None)?;
        session.save_in_place().map_err(|error| error.to_string())?;
        Ok(format!("{} — now {voiced}. Saved.", args.track))
    }
}

/// A track, renamed.
pub mod rename_track {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "rename_track";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Renames a track. Every other tool addresses tracks by name, \
        so the new name is the address from here on. The change is saved.";

    /// Arguments to `rename_track`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track to rename, by name as `describe` lists it.
        pub track: String,
        /// The new name.
        pub name: String,
    }

    /// Renames, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.name.trim().is_empty() {
            return Err("the new name is empty — a track no tool can address again".into());
        }
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        session
            .rename_track(track, args.name.trim())
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;
        Ok(format!(
            "'{}' is now '{}'. Saved.",
            args.track,
            args.name.trim()
        ))
    }
}

/// A track, removed.
pub mod remove_track {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "remove_track";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Removes a track and everything on it — its clips, its effect \
        chain, its sends and its automation. The change is saved.";

    /// Arguments to `remove_track`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track to remove, by name as `describe` lists it.
        pub track: String,
    }

    /// Removes, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        session
            .remove_track(track)
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;
        Ok(format!(
            "Removed '{}' — {} tracks remain. Saved.",
            args.track,
            session.project().tracks.len()
        ))
    }
}

/// An empty clip, opened to be written into.
pub mod add_clip {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "add_clip";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Opens an empty clip on an instrument or singer track, for \
        `edit_notes` to write into — the way a melody is placed note by note. Aim it with \
        `start_bar` and `bars`; the answer numbers the clip the way `describe` does.";

    /// Arguments to `add_clip`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track to put the clip on, by name as `describe` lists it.
        pub track: String,
        /// What to call the clip. "melody" when left out.
        pub name: Option<String>,
        /// The 1-based bar the clip starts at. Bar 1 when left out.
        pub start_bar: Option<u32>,
        /// How many bars it covers.
        pub bars: u32,
    }

    /// Opens the clip, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let bars = bounded_bars(args.bars, "clip")?;
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let start_bar = args.start_bar.unwrap_or(1).max(1);
        let after = bar_after(start_bar, bars)?;
        let start = session.project().signatures.bar_start(start_bar);
        let length = session.project().signatures.bar_start(after) - start;
        let name = args.name.as_deref().unwrap_or("melody");
        let clip = session
            .add_midi_clip(track, name, start, length)
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;
        let number = clip_number(session.project(), track, clip).unwrap_or(0);
        Ok(format!(
            "Opened clip [{number}] '{name}' on {} — bars {start_bar}-{}, empty. Saved. \
             `edit_notes` writes into it.",
            args.track,
            after - 1,
        ))
    }
}

/// A clip's notes, read back numbered.
pub mod notes {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "notes";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Reads one clip's notes, numbered in time order — pitch, bar, \
        beat, length in beats, velocity and, where a note carries one, its lyric. The numbers \
        are the address `edit_notes` removes and `write_lyrics` starts by; aim with `track` \
        and the clip number `describe` shows.";

    /// Arguments to `notes`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to read — an absolute path to a `.auris` file.
        pub project: String,
        /// The track the clip is on, by name as `describe` lists it.
        pub track: String,
        /// Which clip, by the 1-based number `describe` shows.
        pub clip: usize,
    }

    /// Answers with the numbered listing.
    pub fn run(args: &Args) -> Result<String, String> {
        let session = opened(&args.project)?;
        let project = session.project();
        let track = track_by_name(project, &args.track)?.id;
        let (id, clip) = clip_by_number(project, track, args.clip)?;
        let origin = match &clip.recipe {
            Some(recipe) => format!("generated ({}, seed {})", recipe.preset.name(), recipe.seed),
            None => "written by hand".to_string(),
        };
        let mut text = format!(
            "Clip [{}] '{}' on {} — {}, {} notes.\n",
            args.clip,
            clip.name,
            args.track,
            origin,
            clip.notes.len()
        );
        for (number, (_, note)) in time_ordered(clip).into_iter().enumerate() {
            let tick = clip.start + note.start;
            let bar = project.signatures.bar_of(tick);
            let within = tick - project.signatures.bar_start(bar);
            let per_beat = project.signatures.signature_at(tick).ticks_per_beat();
            let beat = within.raw() as f64 / per_beat.raw().max(1) as f64 + 1.0;
            let beats = note.length.raw() as f64 / per_beat.raw().max(1) as f64;
            let lyric = match note.lyric.is_empty() {
                true => String::new(),
                false => format!(", lyric '{}'", note.lyric),
            };
            text.push_str(&format!(
                "  [{}] bar {bar} beat {} — {}, {} beats, vel {}{lyric}\n",
                number + 1,
                trimmed(beat as f32),
                auris_session::prelude::midi_name(i32::from(note.pitch)),
                trimmed(beats as f32),
                trimmed(note.velocity),
            ));
        }
        let _ = id;
        Ok(text.trim_end().to_string())
    }
}

/// Notes, placed and removed by hand.
pub mod edit_notes {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "edit_notes";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Adds and removes notes in one clip, in one call: `remove` \
        takes the numbers `notes` lists, `add` takes notes as pitch (a name like \"F#4\" or a \
        MIDI number), 1-based bar and beat in the song, length in beats, and velocity 0-1 \
        (0.75 when left out). Removals happen first. The change is saved. On a generated clip \
        the edit sticks until `another_take` or `write_again` rewrites the clip whole.";

    /// One note to place.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct NoteSpec {
        /// The pitch: a name in scientific notation ("C4", "F#3", "Bb2") or a MIDI number
        /// 0-127. C4 is middle C.
        pub pitch: String,
        /// The 1-based bar the note starts in.
        pub bar: u32,
        /// The 1-based beat within that bar; fractions land between beats (1.5 is the "and"
        /// of one).
        pub beat: f64,
        /// How long the note is held, in beats.
        pub beats: f64,
        /// Attack strength 0-1. 0.75 when left out.
        pub velocity: Option<f32>,
    }

    /// Arguments to `edit_notes`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track the clip is on, by name as `describe` lists it.
        pub track: String,
        /// Which clip, by the 1-based number `describe` shows.
        pub clip: usize,
        /// Note numbers to remove, as the `notes` listing counts them.
        pub remove: Option<Vec<usize>>,
        /// Notes to place.
        pub add: Option<Vec<NoteSpec>>,
    }

    /// Removes, places, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let removals = args.remove.as_deref().unwrap_or_default();
        let additions = args.add.as_deref().unwrap_or_default();
        if removals.is_empty() && additions.is_empty() {
            return Err("pass `remove`, `add`, or both — there is nothing else here to do".into());
        }
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let (id, clip) = clip_by_number(session.project(), track, args.clip)?;
        let generated = clip.recipe.is_some();
        let clip_start = clip.start;
        let clip_end = clip.start + clip.length;

        // The listing's numbers, translated back to storage order before anything moves.
        let ordered = time_ordered(clip);
        let mut doomed = Vec::with_capacity(removals.len());
        for number in removals {
            let (index, _) = ordered.get(number.wrapping_sub(1)).ok_or_else(|| {
                format!(
                    "the clip has notes [1]-[{}]; there is no [{number}] — `notes` lists them",
                    ordered.len()
                )
            })?;
            doomed.push(*index);
        }

        let mut placed = Vec::with_capacity(additions.len());
        for spec in additions {
            let pitch = pitch_named(&spec.pitch)?;
            let tick = placed_at(session.project(), spec.bar, spec.beat)?;
            if tick < clip_start || tick >= clip_end {
                let first = session.project().signatures.bar_of(clip_start);
                let last = session
                    .project()
                    .signatures
                    .bar_of((clip_end - Ticks(1)).max_zero());
                return Err(format!(
                    "bar {} beat {} is outside the clip, which covers bars {first}-{last}",
                    spec.bar, spec.beat
                ));
            }
            if !spec.beats.is_finite() || spec.beats <= 0.0 || spec.beats > MAX_TOOL_BEATS {
                return Err(format!(
                    "`beats` is how long the note is held; give more than 0 and at most {MAX_TOOL_BEATS}"
                ));
            }
            // Refused rather than clamped, like every other bounded number at this door: the
            // session would quietly pull it into range, and a success that placed a different
            // velocity than the one asked for is a lie of omission.
            if let Some(velocity) = spec.velocity
                && !(0.0..=1.0).contains(&velocity)
            {
                return Err(format!("velocity runs 0-1; {velocity} is outside that"));
            }
            let per_beat = session
                .project()
                .signatures
                .signature_at(tick)
                .ticks_per_beat();
            let length = Ticks((per_beat.raw() as f64 * spec.beats).round() as i64);
            if length > clip_end - tick {
                let first = session.project().signatures.bar_of(clip_start);
                let last = session
                    .project()
                    .signatures
                    .bar_of((clip_end - Ticks(1)).max_zero());
                return Err(format!(
                    "a note at bar {} beat {} held for {} beats runs past the clip, which covers bars {first}-{last}",
                    spec.bar, spec.beat, spec.beats
                ));
            }
            let mut note = Note::new(pitch, tick - clip_start, length);
            note.velocity = spec.velocity.unwrap_or(auris_session::DEFAULT_VELOCITY);
            placed.push(note);
        }

        session
            .remove_notes(id, &doomed)
            .map_err(|error| error.to_string())?;
        for note in placed {
            session
                .add_note(id, note)
                .map_err(|error| error.to_string())?;
        }
        session.save_in_place().map_err(|error| error.to_string())?;

        let now = session
            .project()
            .midi_clip(id)
            .map(|(_, clip)| clip.notes.len())
            .unwrap_or(0);
        let mut text = format!(
            "Removed {}, placed {} — the clip holds {now} notes. Saved.",
            doomed.len(),
            additions.len()
        );
        if generated {
            text.push_str(
                "\nNote: this clip is generated; `another_take` or `write_again` would rewrite \
                 it whole, these edits included.",
            );
        }
        Ok(text)
    }
}

/// A band, written under a melody.
pub mod accompany {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "accompany";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Reads a melody clip and writes a key, a chord progression \
        and backing tracks under it — the melody-first way around: place the tune with \
        `edit_notes`, then derive the band. The melody itself is not touched. `parts` picks \
        the band (bass, chords and drums when left out); the harmony it writes is a first \
        draft to argue with — `write_again` re-derives any part after a correction. The \
        change is saved.";

    /// Arguments to `accompany`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The track the melody is on, by name as `describe` lists it.
        pub track: String,
        /// Which clip the melody is, by the 1-based number `describe` shows.
        pub clip: usize,
        /// The parts to write: lead, chords, pad, arp, bass, stab, drums, kick, snare or hat.
        /// Bass, chords and drums when left out.
        pub parts: Option<Vec<String>>,
        /// The first part's seed; the rest count up from it. 0 when left out.
        pub seed: Option<u64>,
    }

    /// Writes the band, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let parts: Vec<ClipPreset> = match &args.parts {
            None => auris_session::DEFAULT_PARTS.to_vec(),
            Some(named) => named
                .iter()
                .map(|name| {
                    ClipPreset::parse(name).ok_or_else(|| {
                        let all: Vec<&str> =
                            ClipPreset::ALL.iter().map(|preset| preset.name()).collect();
                        format!(
                            "no part is called '{name}' — the parts are: {}",
                            all.join(", ")
                        )
                    })
                })
                .collect::<Result<_, _>>()?,
        };
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let (id, _) = clip_by_number(session.project(), track, args.clip)?;
        let report = session
            .accompany(id, &parts, args.seed.unwrap_or(0))
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;

        let band: Vec<String> = report
            .parts
            .iter()
            .filter_map(|part| {
                session
                    .project()
                    .track(*part)
                    .map(|track| track.name.clone())
            })
            .collect();
        let mut text = format!(
            "Read the melody in {} — {} bars — and wrote {} chords under it. Key: {}. \
             Band: {} ({} notes between them). Saved.",
            args.track,
            report.bars,
            report.chords,
            report.key.to_text(),
            band.join(", "),
            report.notes,
        );
        if report.substituted {
            text.push_str(
                "\nNote: the General MIDI library is not installed, so the band plays the \
                 built-in oscillators.",
            );
        }
        text.push_str(
            "\nThe key and chords are written into the song — a wrong guess is corrected by \
             ear: `analyze`, then `write_again` on any part after fixing what it follows.",
        );
        Ok(text)
    }
}

/// A phrase, laid across a clip's notes.
pub mod write_lyrics {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "write_lyrics";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Lays a phrase across a singer clip's notes, one syllable to \
        each, and derives the phonemes it will be sung as — kana through the built-in table, \
        other text through the Japanese dictionary where one is installed. `from` starts \
        partway in, at a number the way `notes` counts them, so a verse is filled one line at \
        a time; notes past the end of the phrase keep their words. The change is saved.";

    /// Arguments to `write_lyrics`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// The singer track the clip is on, by name as `describe` lists it.
        pub track: String,
        /// Which clip, by the 1-based number `describe` shows.
        pub clip: usize,
        /// The phrase to sing, e.g. "こんにちは" — one mora lands on each note.
        pub text: String,
        /// The 1-based note number to start at, as `notes` counts them. The first note when
        /// left out.
        pub from: Option<usize>,
    }

    /// Lays the phrase, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.text.trim().is_empty() {
            return Err("`text` is the phrase to lay across the notes — give it words".into());
        }
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?;
        if !track.kind.is_singer() {
            return Err(format!(
                "{} plays an instrument — lyrics go on a singer track",
                args.track
            ));
        }
        let track = track.id;
        let (id, clip) = clip_by_number(session.project(), track, args.clip)?;
        let ordered = time_ordered(clip);
        let from = args.from.unwrap_or(1);
        if from == 0 || from > ordered.len() {
            return Err(format!(
                "the clip has notes [1]-[{}]; there is no [{from}] to start from — `notes` \
                 lists them",
                ordered.len()
            ));
        }
        let indices: Vec<usize> = ordered[from - 1..]
            .iter()
            .map(|(index, _)| *index)
            .collect();
        let filled = session
            .write_lyrics(id, &indices, &args.text)
            .map_err(|error| error.to_string())?;
        session.save_in_place().map_err(|error| error.to_string())?;
        let mut text = format!(
            "Laid '{}' across {filled} notes starting at [{from}]. Saved.",
            args.text.trim()
        );
        if filled == indices.len() {
            text.push_str(
                " Every note from there is filled — a longer phrase would have run out of \
                 notes.",
            );
        }
        Ok(text)
    }
}

/// Composing a song from its words.
pub mod compose_lyrics {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "compose_lyrics";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Writes a song from Japanese lyrics and saves it as a new \
        project: a melody searched under the words the Orpheus way, sung notes carrying each \
        syllable, chords in the harmony lane, and a backing band unless `melody_only`. Where \
        a Japanese dictionary is configured the melody follows the lyric's pitch accent; kana \
        lyrics work without one, free of the accent. Phrases break at line breaks and \
        punctuation. The same lyrics and `seed` write the same song; `sing` then gives the \
        vocal its voice.";

    /// Arguments to `compose_lyrics`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The lyrics to compose from. Line breaks and 、。！？ cut the musical phrases.
        pub lyrics: String,
        /// Where to save the project, as an absolute `.auris` path. The project becomes a
        /// folder: choosing `MySong.auris` writes `MySong/MySong.auris`.
        pub output: String,
        /// The take's seed; 0 when left out. Another seed is another melody.
        pub seed: Option<u64>,
        /// Write only the sung melody and its chords, leaving the band off.
        #[serde(default)]
        pub melody_only: bool,
        /// Replace the project already at `output`, instead of refusing to.
        #[serde(default)]
        pub force: bool,
    }

    /// Writes the song, and saves it where asked.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = headless()?;
        let seed = args.seed.unwrap_or(0);
        let parts: &[ClipPreset] = match args.melody_only {
            true => &[],
            false => &auris_session::DEFAULT_PARTS,
        };
        let report = session
            .compose_from_lyrics(&args.lyrics, parts, seed)
            .map_err(|error| error.to_string())?;

        // `save_as`, never `save` — the compose tool's reasoning, verbatim.
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
        let written = std::path::absolute(&written).unwrap_or(written);

        let mut text = format!(
            "Wrote {} — {} phrases, {} sung notes over {} bars, {} backing parts, seed {seed}.",
            written.display(),
            report.phrases,
            report.notes,
            report.bars,
            report.parts.len(),
        );
        if !report.accented {
            text.push_str(
                "\nNote: no Japanese dictionary is configured, so the melody is free of the \
                 lyric's pitch accent; naming one in the settings makes the tune follow the \
                 words.",
            );
        }
        if report.substituted {
            text.push_str(
                "\nNote: the General MIDI font is not installed, so the band plays through \
                 stand-in oscillators.",
            );
        }
        text.push_str("\nThe vocal track has no voice yet — `sing` with `voice` gives it one.");
        Ok(text)
    }
}

/// A singer track, rendered through its voice model.
pub mod sing {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "sing";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Renders a singer track through its voice model and keeps \
        the audio as the track's take, which is what playback and `render` then play. Aims at \
        the project's only singer track when `track` is left out. `voice` chooses a model the \
        first time — an absolute path to an exported `.onnx` voice, which the track keeps. A \
        take is deterministic: the same notes, lyrics, voice and `seed` render the same audio, \
        and another seed is another take. The change is saved.";

    /// Arguments to `sing`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// The project to sing in — an absolute path to a `.auris` file.
        pub project: String,
        /// The singer track, by name as `describe` lists it. The project's only singer track
        /// when left out.
        pub track: Option<String>,
        /// An absolute path to a voice model (`.onnx`) to choose before singing. The track
        /// keeps the voice, so later takes need not name it again.
        pub voice: Option<String>,
        /// Which of the voice's speakers sings, by name, for a model trained on several. The
        /// track keeps the choice. Left out, the track's current speaker sings — the model's
        /// first, until one is chosen. An unknown name is refused, naming the ones the voice
        /// has.
        pub speaker: Option<String>,
        /// The take's seed. The previous take's seed — or 0 — when left out, so singing again
        /// after an edit keeps the same take.
        pub seed: Option<u64>,
    }

    /// Renders the take, and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let target = match &args.track {
            Some(name) => {
                let track = track_by_name(session.project(), name)?;
                if !track.kind.is_singer() {
                    return Err(format!("{name} is not a singer track — `sing` needs one"));
                }
                track.id
            }
            None => {
                let mut singers = session
                    .project()
                    .tracks
                    .iter()
                    .filter(|track| track.kind.is_singer());
                let only = singers.next().map(|track| track.id);
                match (only, singers.next()) {
                    (Some(track), None) => track,
                    (None, _) => {
                        return Err("this project has no singer track — `add_track` with kind \
                             \"singer\" makes one"
                            .into());
                    }
                    _ => {
                        return Err(
                            "this project has more than one singer track — name one with \
                             `track`"
                                .into(),
                        );
                    }
                }
            }
        };
        if let Some(voice) = &args.voice {
            session
                .set_singer_voice(target, Some(Path::new(voice)))
                .map_err(|error| error.to_string())?;
        }
        if args.speaker.is_some() {
            session
                .set_singer_speaker(target, args.speaker.as_deref())
                .map_err(|error| error.to_string())?;
        }
        let name = session
            .singer_voice(target)
            .map_err(|error| error.to_string())?
            .map(|voice| match &voice.speaker {
                Some(speaker) => format!("{} · {speaker}", voice.name),
                None => voice.name.clone(),
            })
            .unwrap_or_default();
        let name = bounded_label(&name);
        let seconds = session
            .sing(target, args.seed)
            .map_err(|error| error.to_string())?;
        // A take names its audio by a pointer in the document; a pointer that only lived in
        // memory would leave the rendered file orphaned on disk.
        session.save_in_place().map_err(|error| error.to_string())?;
        let seed = session
            .project()
            .track(target)
            .and_then(|track| track.kind.as_singer())
            .and_then(|singer| singer.take.as_ref())
            .map(|take| take.seed)
            .unwrap_or_default();
        Ok(format!(
            "Voice {name} sang {seconds:.1} s into the project — seed {seed} names this take, and \
             playback and `render` now sing it. Saved."
        ))
    }
}

/// A session with no audio device and no GPU, with the shipped SoundFonts.
///
/// The fonts for the same reason `auris compose` loads them: `compose` here and **Compose a
/// Song…** in the window have to write the same piece, and half the instruments a piece asks
/// for are in that library.
fn headless() -> Result<Session, String> {
    Session::new(
        SessionOptions::headless()
            .with_shipped_fonts(true)
            .with_shipped_dictionary(true),
    )
    .map_err(|error| error.to_string())
}

/// The project file a path means: absolute, and reaching inside the folder a project becomes.
///
/// Models hand back the path they asked `compose` for — `GemmaTake.auris` — where the file
/// really went to `GemmaTake/GemmaTake.auris`, because saving nests a project in a folder of
/// its own. The convention is one-to-one, so rather than teach it as an error, every tool
/// that opens a project walks it: absolutise (a relative path is resolved against wherever
/// the host process happens to be), and when the file is not there, look one folder down
/// under its own name. A path found neither way is refused with the absolute form, so the
/// caller learns what its relative path actually meant.
pub fn resolve_project(path: &str) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(Path::new(path)).map_err(|error| error.to_string())?;
    if absolute.exists() {
        return Ok(absolute);
    }
    if let (Some(parent), Some(stem), Some(name)) = (
        absolute.parent(),
        absolute.file_stem(),
        absolute.file_name(),
    ) {
        let nested = parent.join(stem).join(name);
        if nested.exists() {
            return Ok(nested);
        }
    }
    Err(format!("file not found: {}", absolute.display()))
}

/// A headless session with `path`'s project already open.
///
/// For the tools that read and adjust rather than render: they have no use for the list of
/// missing audio files, because nothing here plays.
fn opened(path: &str) -> Result<Session, String> {
    let mut session = headless()?;
    session
        .open(&resolve_project(path)?)
        .map_err(|error| error.to_string())?;
    Ok(session)
}

/// The track called `name`, or a refusal that lists the real ones.
fn track_by_name<'p>(project: &'p Project, name: &str) -> Result<&'p Track, String> {
    let matches: Vec<(usize, &Track)> = project
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.name.eq_ignore_ascii_case(name))
        .collect();
    match matches.as_slice() {
        [(_, track)] => Ok(*track),
        [] => {
            let names: Vec<&str> = project
                .tracks
                .iter()
                .map(|track| track.name.as_str())
                .collect();
            Err(format!(
                "no track is named '{name}' — this project has: {}",
                names.join(", ")
            ))
        }
        ambiguous => {
            let names: Vec<String> = ambiguous
                .iter()
                .map(|(index, track)| format!("[{}] '{}'", index + 1, track.name))
                .collect();
            Err(format!(
                "track name '{name}' is ambiguous — it matches {}; rename one before using a by-name tool",
                names.join(", ")
            ))
        }
    }
}

/// The clip a track's 1-based number means, in the numbering `describe` prints.
fn clip_by_number(
    project: &Project,
    track: TrackId,
    number: usize,
) -> Result<(ClipId, &MidiClip), String> {
    let clips = project
        .track(track)
        .and_then(|entry| entry.kind.note_clips())
        .ok_or("that track holds no note clips")?;
    clips
        .get(number.wrapping_sub(1))
        .map(|clip| (clip.id, clip))
        .ok_or_else(|| {
            format!(
                "the track has clips [1]-[{}]; there is no [{number}] — `describe` numbers them",
                clips.len()
            )
        })
}

/// The 1-based number `describe` would print for this clip.
fn clip_number(project: &Project, track: TrackId, clip: ClipId) -> Option<usize> {
    project
        .track(track)?
        .kind
        .note_clips()?
        .iter()
        .position(|entry| entry.id == clip)
        .map(|index| index + 1)
}

/// A clip's notes in time order, each with its storage index — the listing `notes` prints
/// and the numbering `edit_notes` removes by, computed one way in one place.
fn time_ordered(clip: &MidiClip) -> Vec<(usize, &Note)> {
    let mut ordered: Vec<(usize, &Note)> = clip.notes.iter().enumerate().collect();
    ordered.sort_by_key(|(index, note)| (note.start, note.pitch, *index));
    ordered
}

/// A pitch, read as a MIDI number or a scientific name — "C4" is middle C.
fn pitch_named(text: &str) -> Result<u8, String> {
    let text = text.trim();
    if let Ok(number) = text.parse::<i32>() {
        if (0..=127).contains(&number) {
            return Ok(number as u8);
        }
        return Err(format!("MIDI numbers run 0-127; {number} is outside that"));
    }
    let split = text
        .find(|mark: char| mark.is_ascii_digit() || mark == '-')
        .ok_or_else(|| format!("'{text}' is not a pitch — a name like \"F#4\", or 0-127"))?;
    let class = auris_session::prelude::PitchClass::parse(&text[..split])
        .ok_or_else(|| format!("'{text}' is not a pitch — a name like \"F#4\", or 0-127"))?;
    let octave: i32 = text[split..]
        .parse()
        .map_err(|_| format!("'{text}' is not a pitch — a name like \"F#4\", or 0-127"))?;
    // `midi` is plain i32 arithmetic, and an octave in the hundreds of millions would overflow
    // it before the 0-127 check below could answer. MIDI lives in octaves -1 to 9; a couple
    // either side still falls through to the friendlier answer that names the number.
    if !(-4..=12).contains(&octave) {
        return Err(format!("{text} is far outside the MIDI range 0-127"));
    }
    let midi = class.midi(octave);
    u8::try_from(midi)
        .ok()
        .filter(|midi| *midi <= 127)
        .ok_or_else(|| format!("{text} is MIDI {midi}, outside 0-127"))
}

/// The bar one past the last of a run `bars` long starting at 1-based `start_bar` — refused,
/// rather than overflowed, when the numbers are absurd.
fn bar_after(start_bar: u32, bars: u32) -> Result<u32, String> {
    start_bar.checked_add(bars).ok_or_else(|| {
        format!("bar {start_bar} plus {bars} bars is past any timeline this can count")
    })
}

/// Most bars a model-facing command may create or generate in one call.
const MAX_TOOL_BARS: u32 = 4_096;

/// A requested span checked before it can drive timeline-sized allocation or generation.
fn bounded_bars(bars: u32, subject: &str) -> Result<u32, String> {
    if bars == 0 {
        return Err(format!(
            "`bars` is how many bars the {subject} covers; give at least 1"
        ));
    }
    if bars > MAX_TOOL_BARS {
        return Err(format!(
            "`bars` is how many bars the {subject} covers; give at most {MAX_TOOL_BARS}"
        ));
    }
    Ok(bars)
}

/// Where 1-based `bar` and `beat` land on the timeline.
fn placed_at(project: &Project, bar: u32, beat: f64) -> Result<Ticks, String> {
    if !beat.is_finite() || !(1.0..=MAX_TOOL_BEATS).contains(&beat) {
        return Err(format!(
            "beats count from 1 and stop at {MAX_TOOL_BEATS}; {beat} is outside that range"
        ));
    }
    let start = project.signatures.bar_start(bar.max(1));
    let per_beat = project.signatures.signature_at(start).ticks_per_beat();
    Ok(start + Ticks((per_beat.raw() as f64 * (beat - 1.0)).round() as i64))
}

/// Largest beat position or note duration accepted at the model-facing door.
///
/// This is 4,096 bars of common time: far beyond an ordinary clip, while still keeping every
/// conversion and subsequent timeline calculation comfortably inside `Ticks`.
const MAX_TOOL_BEATS: f64 = 16_384.0;

/// A mixer strip address: a track by name, or `None` for the master bus as "master".
fn strip_by_name(project: &Project, name: &str) -> Result<Option<TrackId>, String> {
    if project
        .tracks
        .iter()
        .any(|track| track.name.eq_ignore_ascii_case(name))
    {
        return track_by_name(project, name).map(|track| Some(track.id));
    }
    match name.eq_ignore_ascii_case("master") {
        true => Ok(None),
        false => track_by_name(project, name).map(|track| Some(track.id)),
    }
}

/// Marks a file-supplied display label as data before it reaches a model-facing answer.
fn bounded_label(value: &str) -> String {
    const MAX_CHARS: usize = 80;
    let mut chars = value.chars().filter(|character| !character.is_control());
    let mut label: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        label.push('…');
    }
    format!("{label:?}")
}

/// Refuses render destinations that can replace a file the open document still names.
fn protect_project_assets(
    session: &Session,
    destination: &Path,
    stems: bool,
) -> Result<(), String> {
    let folder = session
        .project_folder()
        .ok_or_else(|| "the open project has no folder".to_string())?;
    let destination = resolved_path(destination);
    let audio = resolved_path(&folder.join("Audio"));
    if destination == audio || destination.starts_with(&audio) {
        return Err(format!(
            "refusing to render into the project's asset folder: {}",
            destination.display()
        ));
    }

    let conflicts = session
        .project()
        .audio_sources
        .values()
        .map(|source| &source.path)
        .chain(session.project().soundfonts.values().map(|font| &font.path))
        .filter_map(|asset| asset.resolve(Some(folder)))
        .map(|asset| resolved_path(&asset))
        .any(|asset| match stems {
            true => asset.starts_with(&destination),
            false => asset == destination,
        });
    if conflicts {
        return Err(format!(
            "refusing to overwrite an asset used by this project at {}",
            destination.display()
        ));
    }
    Ok(())
}

/// Resolves symlinks through the nearest existing ancestor, including for a new output file.
fn resolved_path(path: &Path) -> PathBuf {
    let mut cursor = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut missing = Vec::new();
    while !cursor.exists() {
        let Some(name) = cursor.file_name().map(ToOwned::to_owned) else {
            break;
        };
        missing.push(name);
        if !cursor.pop() {
            break;
        }
    }
    let mut resolved = cursor.canonicalize().unwrap_or(cursor);
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// A number without the trailing zeros a fixed format would carry.
fn trimmed(value: f32) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The suffix a unit reads with, or nothing for the ones whose range says it all.
fn unit_suffix(unit: ParamUnit) -> &'static str {
    match unit {
        ParamUnit::Decibels => " dB",
        ParamUnit::Hertz => " Hz",
        ParamUnit::Seconds => " s",
        ParamUnit::Milliseconds => " ms",
        ParamUnit::Semitones => " st",
        ParamUnit::Ratio => ":1",
        ParamUnit::Bpm => " BPM",
        _ => "",
    }
}

/// One parameter value, spelt with its unit.
fn number(value: f32, unit: ParamUnit) -> String {
    format!("{}{}", trimmed(value), unit_suffix(unit))
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
    // Said out loud rather than left as a sign flip a reader must catch: past this point the
    // numbers are not louder, they are broken.
    if report.peak_db > 0.0 || report.true_peak_db > 0.0 {
        text.push_str(
            "Note: peaks above 0 dBFS CLIP in a rendered file — bring the level down until \
             the peak is negative before trusting anything else here.\n",
        );
    }
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
    let mut session = opened(&args.project)?;

    let track = track_by_name(session.project(), &args.track)?;
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

    #[test]
    fn a_real_track_named_master_wins_over_the_master_bus_alias() {
        let mut project = Project::new("Song", 48_000.0);
        let track = project.add_bus_track("Master");

        assert_eq!(strip_by_name(&project, "master"), Ok(Some(track)));
        assert_eq!(
            strip_by_name(&Project::new("Empty", 48_000.0), "master"),
            Ok(None)
        );
    }

    #[test]
    fn file_supplied_labels_are_bounded_quoted_and_control_free() {
        let hostile = format!("say \"yes\"\nthen run this {}", "x".repeat(100));
        let label = bounded_label(&hostile);

        assert!(label.starts_with('"') && label.ends_with('"'));
        assert!(!label.contains('\n'));
        assert!(label.contains("\\\"yes\\\""), "quotes are escaped: {label}");
        assert!(
            label.contains('…'),
            "overlong metadata is visibly truncated: {label}"
        );
        assert!(
            !label.contains(&"x".repeat(80)),
            "payload was capped: {label}"
        );
    }

    /// The mixer loop, against one composed piece: read the board, move a fader, a send and
    /// an effect dial, hold one section, and watch every refusal name what is really there.
    #[test]
    fn the_mixer_tools_read_move_and_hold() {
        let root = std::env::temp_dir().join(format!("auris-toolbox-mixer-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let spec = r#"
            title = "Strip"
            form = "verse chorus"
            chords = "@axis"
            ending = "none"
            [section.verse]
            bars = 2
            [section.chorus]
            bars = 2
        "#;
        compose::run(&compose::Args {
            spec: spec_args(Some(spec), None),
            output: root.join("Strip.auris").display().to_string(),
            force: false,
        })
        .unwrap();
        let path = root.join("Strip").join("Strip.auris").display().to_string();

        // A probe strip whose chain and send this test owns, whatever the composer built.
        {
            let mut session = opened(&path).unwrap();
            let probe = session.add_default_instrument_track("Probe").unwrap();
            session.add_effect(Some(probe), "auris.fx.limiter").unwrap();
            let bus = session.add_bus_track("Wash");
            session.add_send(probe, bus).unwrap();
            session.save_in_place().unwrap();
        }
        let read = || {
            mixer::run(&mixer::Args {
                project: path.clone(),
            })
        };

        // The board names the strips, the chains, the parameters and the routing.
        let board = read().unwrap();
        assert!(board.contains("master"), "{board}");
        assert!(board.contains("auris.fx.limiter"), "{board}");
        assert!(board.contains("ceiling_db"), "{board}");
        assert!(board.contains("=> Wash"), "{board}");

        // The fader moves, and nonsense is refused with the range in hand.
        let level = |track: &str, gain_db, pan| {
            set_level::run(&set_level::Args {
                project: path.clone(),
                track: track.to_string(),
                gain_db,
                pan,
            })
        };
        let moved = level("Probe", Some(-6.0), Some(0.25)).unwrap();
        assert!(moved.contains("-6.0"), "{moved}");
        let refused = level("Probe", Some(40.0), None).unwrap_err();
        assert!(refused.contains("-60"), "{refused}");
        let missing = level("Nobody", Some(-1.0), None).unwrap_err();
        assert!(
            missing.contains("Probe"),
            "the refusal lists the real tracks: {missing}"
        );

        // The send is addressed by the bus it feeds.
        let send = |to: &str, level_db| {
            set_send::run(&set_send::Args {
                project: path.clone(),
                track: "Probe".to_string(),
                to: to.to_string(),
                level_db,
            })
        };
        let sent = send("Wash", -12.0).unwrap();
        assert!(sent.contains("-12.0"), "{sent}");
        let nowhere = send("Elsewhere", -12.0).unwrap_err();
        assert!(
            nowhere.contains("Wash"),
            "the refusal lists the real sends: {nowhere}"
        );

        // The dial turns, and both wrong names and wrong values answer with the truth.
        let dial = |param: &str, value| {
            set_effect::run(&set_effect::Args {
                project: path.clone(),
                track: "Probe".to_string(),
                effect: Some("limiter".to_string()),
                slot: None,
                param: param.to_string(),
                value,
            })
        };
        let dialed = dial("ceiling_db", -3.0).unwrap();
        assert!(dialed.contains("ceiling_db"), "{dialed}");
        assert!(dialed.contains("-3"), "{dialed}");
        let wrong = dial("colour", 1.0).unwrap_err();
        assert!(
            wrong.contains("ceiling_db"),
            "the refusal lists the real parameters: {wrong}"
        );
        let too_far = dial("ceiling_db", 20.0).unwrap_err();
        assert!(too_far.contains("-24"), "{too_far}");

        // One section held; the board says a lane took the fader; clear gives it back.
        let hold = |section: Option<&str>, gain_db, clear| {
            section_gain::run(&section_gain::Args {
                project: path.clone(),
                track: "Probe".to_string(),
                section: section.map(String::from),
                instance: None,
                gain_db,
                clear,
            })
        };
        let held = hold(Some("chorus"), Some(-9.0), false).unwrap();
        assert!(held.contains("chorus"), "{held}");
        assert!(held.contains("-9.0"), "{held}");
        assert!(read().unwrap().contains("[gain automated]"));
        let unknown = hold(Some("coda"), Some(-9.0), false).unwrap_err();
        assert!(
            unknown.contains("chorus"),
            "the refusal lists the sections: {unknown}"
        );
        let cleared = hold(None, None, true).unwrap();
        assert!(cleared.contains("fader"), "{cleared}");
        assert!(!read().unwrap().contains("[gain automated]"));

        // A boost on the master carries the warning the first model to use this tool needed:
        // that fader sits after the limiter, so nothing catches what it adds.
        let risky = section_gain::run(&section_gain::Args {
            project: path.clone(),
            track: "master".to_string(),
            section: Some("verse".to_string()),
            instance: None,
            gain_db: Some(3.0),
            clear: false,
        })
        .unwrap();
        assert!(risky.contains("not limited"), "{risky}");
        let safe = section_gain::run(&section_gain::Args {
            project: path.clone(),
            track: "master".to_string(),
            section: Some("verse".to_string()),
            instance: None,
            gain_db: Some(-3.0),
            clear: false,
        })
        .unwrap();
        assert!(!safe.contains("not limited"), "{safe}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The arrangement tools, on one composed piece: a track added, a part written from the
    /// harmony already under the song, a re-voice, a rename that becomes the address, and a
    /// removal — with every refusal naming the real vocabulary. The General MIDI leg runs
    /// only where the fetched library is installed; elsewhere the honest refusal is asserted
    /// instead, because that is what a user without the fonts gets too.
    #[test]
    fn the_arrangement_tools_add_voice_write_and_remove() {
        let root =
            std::env::temp_dir().join(format!("auris-toolbox-arrange-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let spec = r#"
            title = "Arrange"
            form = "verse"
            chords = "@axis"
            ending = "none"
            [section.verse]
            bars = 4
        "#;
        compose::run(&compose::Args {
            spec: spec_args(Some(spec), None),
            output: root.join("Arrange.auris").display().to_string(),
            force: false,
        })
        .unwrap();
        let path = root
            .join("Arrange")
            .join("Arrange.auris")
            .display()
            .to_string();

        // The vocabulary teaches both doors: the registry ids and the General MIDI field.
        let listed = list_instruments::run();
        assert!(listed.contains("General MIDI"), "{listed}");
        let default_id = headless()
            .unwrap()
            .registry()
            .default_instrument_id()
            .unwrap()
            .to_string();
        assert!(listed.contains(&default_id), "{listed}");

        // A plain instrument track lands on the default voice and says what to do next.
        let added = add_track::run(&add_track::Args {
            project: path.clone(),
            name: "Keys".to_string(),
            instrument: None,
            sound: None,
            drums: false,
            kind: None,
        })
        .unwrap();
        assert!(added.contains(&default_id), "{added}");
        assert!(added.contains("add_part"), "{added}");

        // The General MIDI door: the sound where the library is installed, the honest
        // refusal where it is not — never a silent substitution.
        let voiced = add_track::run(&add_track::Args {
            project: path.clone(),
            name: "EP".to_string(),
            instrument: None,
            sound: Some("Electric Piano 1".to_string()),
            drums: false,
            kind: None,
        });
        match voiced {
            Ok(text) => assert!(text.contains("Electric Piano 1"), "{text}"),
            Err(text) => assert!(text.contains("library"), "{text}"),
        }
        let nonsense = add_track::run(&add_track::Args {
            project: path.clone(),
            name: "X".to_string(),
            instrument: None,
            sound: Some("Theremin Choir 9".to_string()),
            drums: false,
            kind: None,
        })
        .unwrap_err();
        assert!(nonsense.contains("0-127"), "{nonsense}");

        // A part covers the song by default, numbers itself the way `describe` does, and
        // really wrote notes — the harmony was under those bars.
        let wrote = add_part::run(&add_part::Args {
            project: path.clone(),
            track: "Keys".to_string(),
            part: "chords".to_string(),
            start_bar: None,
            bars: None,
            seed: None,
        })
        .unwrap();
        assert!(wrote.contains("bars 1-4"), "{wrote}");
        assert!(wrote.contains("seed 0"), "{wrote}");
        assert!(!wrote.contains("empty"), "{wrote}");
        let described = describe::run(&describe::Args {
            project: path.clone(),
        })
        .unwrap();
        assert!(
            described.contains("generated (chords, seed 0)"),
            "the part is addressable by `another_take`: {described}"
        );

        // A wrong part name answers with the whole vocabulary.
        let unknown = add_part::run(&add_part::Args {
            project: path.clone(),
            track: "Keys".to_string(),
            part: "sitar solo".to_string(),
            start_bar: None,
            bars: None,
            seed: None,
        })
        .unwrap_err();
        assert!(unknown.contains("bass"), "{unknown}");

        // Re-voicing refuses an id nothing answers to, pointing at the list.
        let wrong = set_instrument::run(&set_instrument::Args {
            project: path.clone(),
            track: "Keys".to_string(),
            instrument: Some("auris.not.a.thing".to_string()),
            sound: None,
            drums: false,
        })
        .unwrap_err();
        assert!(wrong.contains("list_instruments"), "{wrong}");
        let revoiced = set_instrument::run(&set_instrument::Args {
            project: path.clone(),
            track: "Keys".to_string(),
            instrument: Some(default_id.clone()),
            sound: None,
            drums: false,
        })
        .unwrap();
        assert!(revoiced.contains(&default_id), "{revoiced}");

        // The rename is the address from then on: the old name refuses, naming the new.
        rename_track::run(&rename_track::Args {
            project: path.clone(),
            track: "Keys".to_string(),
            name: "Piano".to_string(),
        })
        .unwrap();
        let stale = add_part::run(&add_part::Args {
            project: path.clone(),
            track: "Keys".to_string(),
            part: "chords".to_string(),
            start_bar: None,
            bars: None,
            seed: None,
        })
        .unwrap_err();
        assert!(stale.contains("Piano"), "{stale}");

        // Removal takes the clips with it and counts what remains.
        let removed = remove_track::run(&remove_track::Args {
            project: path.clone(),
            track: "Piano".to_string(),
        })
        .unwrap();
        assert!(removed.contains("remain"), "{removed}");
        let after = describe::run(&describe::Args {
            project: path.clone(),
        })
        .unwrap();
        assert!(!after.contains("Piano"), "{after}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A pitch is read the way a person writes one, and refused with directions otherwise.
    #[test]
    fn pitches_are_read_by_name_and_by_number() {
        assert_eq!(pitch_named("C4").unwrap(), 60, "middle C");
        assert_eq!(pitch_named("c4").unwrap(), 60, "case does not matter");
        assert_eq!(pitch_named("F#3").unwrap(), 54);
        assert_eq!(pitch_named("Bb2").unwrap(), 46);
        assert_eq!(pitch_named("A-1").unwrap(), 9, "the octave below the floor");
        assert_eq!(pitch_named("69").unwrap(), 69, "a number is taken as MIDI");
        assert!(pitch_named("H4").unwrap_err().contains("F#4"));
        assert!(pitch_named("300").unwrap_err().contains("0-127"));
        assert!(pitch_named("C99").unwrap_err().contains("0-127"));
        assert!(
            pitch_named("C200000000").unwrap_err().contains("0-127"),
            "an absurd octave is refused, not overflowed"
        );
    }

    #[test]
    fn the_documented_default_velocity_is_the_real_one() {
        // The description is the model's contract, and it names the default in prose; this
        // pins that prose to the constant so the two cannot drift apart again.
        let told = auris_session::DEFAULT_VELOCITY.to_string();
        assert!(
            edit_notes::DESCRIPTION.contains(&told),
            "the description says a default the code does not use"
        );
    }

    #[test]
    fn absurd_bar_arithmetic_is_refused_rather_than_overflowed() {
        assert_eq!(bar_after(1, 8).unwrap(), 9);
        assert!(bar_after(u32::MAX, 1).unwrap_err().contains("timeline"));
    }

    #[test]
    fn one_tool_call_cannot_create_an_unbounded_number_of_bars() {
        assert_eq!(bounded_bars(MAX_TOOL_BARS, "part"), Ok(MAX_TOOL_BARS));
        let error = bounded_bars(MAX_TOOL_BARS + 1, "part").unwrap_err();
        assert!(error.contains("at most 4096"), "{error}");
        assert!(bounded_bars(0, "clip").unwrap_err().contains("at least 1"));
    }

    #[test]
    fn a_duplicate_track_name_is_ambiguous_instead_of_picking_the_first() {
        let mut project = Project::new("Song", 48_000.0);
        project.add_bus_track("Drums");
        project.add_bus_track("drums");

        let error = track_by_name(&project, "DRUMS").unwrap_err();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains("[1] 'Drums'"), "{error}");
        assert!(error.contains("[2] 'drums'"), "{error}");
        assert!(
            strip_by_name(&project, "Drums")
                .unwrap_err()
                .contains("ambiguous"),
            "mixer-strip tools use the same safe lookup"
        );
    }

    /// The melody-first workflow, whole: an empty project, a track, an empty clip, a tune
    /// placed note by note, read back numbered, corrected — and then a band derived from it.
    #[test]
    fn a_melody_is_placed_read_corrected_and_accompanied() {
        let root =
            std::env::temp_dir().join(format!("auris-toolbox-melody-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut session = headless().unwrap();
            session.save_as(&root.join("Tune.auris")).unwrap();
        }
        let path = root.join("Tune").join("Tune.auris").display().to_string();

        add_track::run(&add_track::Args {
            project: path.clone(),
            name: "Lead".to_string(),
            instrument: None,
            sound: None,
            drums: false,
            kind: None,
        })
        .unwrap();
        let opened_clip = add_clip::run(&add_clip::Args {
            project: path.clone(),
            track: "Lead".to_string(),
            name: None,
            start_bar: None,
            bars: 2,
        })
        .unwrap();
        assert!(opened_clip.contains("[1] 'melody'"), "{opened_clip}");
        assert!(opened_clip.contains("bars 1-2"), "{opened_clip}");

        // The tune, placed out of time order on purpose: the listing sorts.
        let place = |notes: Vec<edit_notes::NoteSpec>, remove: Option<Vec<usize>>| {
            edit_notes::run(&edit_notes::Args {
                project: path.clone(),
                track: "Lead".to_string(),
                clip: 1,
                remove,
                add: Some(notes),
            })
        };
        let note = |pitch: &str, bar: u32, beat: f64| edit_notes::NoteSpec {
            pitch: pitch.to_string(),
            bar,
            beat,
            beats: 1.0,
            velocity: None,
        };
        let placed = place(
            vec![
                note("E4", 1, 3.0),
                note("C4", 1, 1.0),
                note("G4", 2, 1.0),
                note("D4", 2, 3.0),
            ],
            None,
        )
        .unwrap();
        assert!(placed.contains("placed 4"), "{placed}");

        let listing = notes::run(&notes::Args {
            project: path.clone(),
            track: "Lead".to_string(),
            clip: 1,
        })
        .unwrap();
        assert!(listing.contains("written by hand"), "{listing}");
        assert!(
            listing.contains("[1] bar 1 beat 1 — C4"),
            "time order, not placement order: {listing}"
        );
        assert!(listing.contains("[3] bar 2 beat 1 — G4"), "{listing}");

        // The correction: the D was wrong, an E belongs there. Numbers are the listing's.
        let corrected = place(vec![note("E4", 2, 3.0)], Some(vec![4])).unwrap();
        assert!(corrected.contains("Removed 1, placed 1"), "{corrected}");
        let after = notes::run(&notes::Args {
            project: path.clone(),
            track: "Lead".to_string(),
            clip: 1,
        })
        .unwrap();
        assert!(after.contains("[4] bar 2 beat 3 — E4"), "{after}");
        assert!(!after.contains("D4"), "{after}");
        let missing = place(vec![], Some(vec![9])).unwrap_err();
        assert!(missing.contains("[1]-[4]"), "{missing}");
        let outside = place(vec![note("C4", 7, 1.0)], None).unwrap_err();
        assert!(outside.contains("bars 1-2"), "{outside}");
        let past_end = place(
            vec![edit_notes::NoteSpec {
                beats: 2.0,
                ..note("C4", 2, 4.0)
            }],
            None,
        )
        .unwrap_err();
        assert!(past_end.contains("runs past the clip"), "{past_end}");
        let loud = place(
            vec![edit_notes::NoteSpec {
                velocity: Some(5.0),
                ..note("C4", 1, 1.0)
            }],
            None,
        )
        .unwrap_err();
        assert!(loud.contains("0-1"), "refused, not clamped: {loud}");
        let too_long = place(
            vec![edit_notes::NoteSpec {
                beats: 1e16,
                ..note("C4", 1, 1.0)
            }],
            None,
        )
        .unwrap_err();
        assert!(too_long.contains("at most"), "refused safely: {too_long}");
        let too_late = place(vec![note("C4", 1, 1e18)], None).unwrap_err();
        assert!(
            too_late.contains("outside that range"),
            "refused safely: {too_late}"
        );

        // The band, derived from the tune. The melody itself is untouched.
        let band = accompany::run(&accompany::Args {
            project: path.clone(),
            track: "Lead".to_string(),
            clip: 1,
            parts: None,
            seed: None,
        })
        .unwrap();
        assert!(band.contains("Key:"), "{band}");
        assert!(band.contains("Bass"), "{band}");
        let unchanged = notes::run(&notes::Args {
            project: path.clone(),
            track: "Lead".to_string(),
            clip: 1,
        })
        .unwrap();
        assert!(unchanged.contains("[1] bar 1 beat 1 — C4"), "{unchanged}");
        let described = describe::run(&describe::Args {
            project: path.clone(),
        })
        .unwrap();
        assert!(described.contains("Bass"), "{described}");
        assert!(described.contains("Drums"), "{described}");

        std::fs::remove_dir_all(&root).unwrap();
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

        // `save_as` nests the document in a folder of its own — and the answer says where,
        // absolutely, because that line is what a model copies its next call from.
        let document = root.join("Loop").join("Loop.auris");
        assert!(
            report.contains(&document.display().to_string()),
            "the compose answer names the nested, absolute path: {report}"
        );

        // A caller holding the path it *asked* with — the unnested `Loop.auris` — is met at
        // the place the file really went, rather than taught the convention as an error.
        let shorthand = describe::run(&describe::Args {
            project: root.join("Loop.auris").display().to_string(),
        })
        .unwrap();
        assert!(shorthand.contains("tempo"), "{shorthand}");

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

        let asset_folder = document.parent().unwrap().join("Audio");
        let protected_mix = render::run(&render::Args {
            project: document.display().to_string(),
            output: Some(asset_folder.join("source.wav").display().to_string()),
            bit_depth: Some(16),
            stems: None,
        })
        .unwrap_err();
        assert!(protected_mix.contains("asset folder"), "{protected_mix}");
        let protected_stems = render::run(&render::Args {
            project: document.display().to_string(),
            output: None,
            bit_depth: Some(16),
            stems: Some(asset_folder.display().to_string()),
        })
        .unwrap_err();
        assert!(
            protected_stems.contains("asset folder"),
            "{protected_stems}"
        );

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

    /// The words-first door: kana lyrics in, a saved song out, and the answer says both what
    /// was written and what was *not* analysed — no dictionary means no accent, and the tool
    /// says so instead of quietly free-composing under a false flag.
    #[test]
    fn lyrics_become_a_saved_song_that_names_its_next_step() {
        let root =
            std::env::temp_dir().join(format!("auris-toolbox-lyrics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let args = compose_lyrics::Args {
            lyrics: "さくら さいた\nはるが きた".into(),
            output: root.join("Sakura.auris").display().to_string(),
            seed: Some(1),
            melody_only: false,
            force: false,
        };

        let answer = compose_lyrics::run(&args).expect("kana needs no dictionary");
        assert!(answer.contains("2 phrases"), "{answer}");
        assert!(answer.contains("11 sung notes"), "{answer}");
        assert!(answer.contains("3 backing parts"), "{answer}");
        // The tool sessions load the shipped dictionary, so what the answer says about the
        // accent depends on whether this machine has fetched it — a fetched checkout hears
        // the accent, a bare CI runner is told plainly that nothing did.
        assert_eq!(
            answer.contains("pitch accent"),
            auris_session::library::installed_dictionary().is_none(),
            "honest about the accent either way: {answer}"
        );
        assert!(
            answer.contains("`sing`"),
            "the next step is named: {answer}"
        );
        assert!(root.join("Sakura").join("Sakura.auris").exists());

        // Writing again without force refuses and names the flag.
        let refused = compose_lyrics::run(&args).unwrap_err();
        assert!(refused.contains("force"), "{refused}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The singer tools at the model door: a track that sings, words laid across its notes,
    /// and a refusal naming the cure at every missing piece. With
    /// `AURIS_SINGER_TEST_MODEL` set, the voice actually sings at the end.
    #[test]
    fn the_singer_tools_write_words_and_name_their_cures() {
        let root = std::env::temp_dir().join(format!("auris-toolbox-sing-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let spec = r#"
            title = "Verse"
            form = "verse"
            chords = "@axis"
            ending = "none"
            [section.verse]
            bars = 2
        "#;
        compose::run(&compose::Args {
            spec: spec_args(Some(spec), None),
            output: root.join("Verse.auris").display().to_string(),
            force: false,
        })
        .unwrap();
        let path = root.join("Verse").join("Verse.auris").display().to_string();
        let sing_the = |track: Option<&str>, voice: Option<String>| {
            sing::run(&sing::Args {
                project: path.clone(),
                track: track.map(String::from),
                voice,
                speaker: None,
                seed: None,
            })
        };

        // Before a singer exists, the refusal says how to get one.
        let unsung = sing_the(None, None).unwrap_err();
        assert!(unsung.contains("add_track"), "{unsung}");

        // The track arrives through the same door as every other kind, with directions on.
        let added = add_track::run(&add_track::Args {
            project: path.clone(),
            name: "Vocal".to_string(),
            instrument: None,
            sound: None,
            drums: false,
            kind: Some("singer".to_string()),
        })
        .unwrap();
        assert!(added.contains("write_lyrics"), "{added}");

        // A tune note by note, then its words, one mora to each.
        add_clip::run(&add_clip::Args {
            project: path.clone(),
            track: "Vocal".to_string(),
            name: None,
            start_bar: Some(1),
            bars: 2,
        })
        .unwrap();
        let note = |beat: f64| edit_notes::NoteSpec {
            pitch: "C4".to_string(),
            bar: 1,
            beat,
            beats: 1.0,
            velocity: None,
        };
        edit_notes::run(&edit_notes::Args {
            project: path.clone(),
            track: "Vocal".to_string(),
            clip: 1,
            remove: None,
            add: Some(vec![note(1.0), note(2.0), note(3.0)]),
        })
        .unwrap();
        let laid = write_lyrics::run(&write_lyrics::Args {
            project: path.clone(),
            track: "Vocal".to_string(),
            clip: 1,
            text: "かえる".to_string(),
            from: None,
        })
        .unwrap();
        assert!(laid.contains("3 notes"), "{laid}");
        let listing = notes::run(&notes::Args {
            project: path.clone(),
            track: "Vocal".to_string(),
            clip: 1,
        })
        .unwrap();
        assert!(listing.contains("lyric 'か'"), "{listing}");

        // Words on an instrument track are met with directions, not silence...
        let sideways = write_lyrics::run(&write_lyrics::Args {
            project: path.clone(),
            track: "lead".to_string(),
            clip: 1,
            text: "か".to_string(),
            from: None,
        })
        .unwrap_err();
        assert!(sideways.contains("singer"), "{sideways}");
        // ...and so is singing a track that plays rather than sings.
        let wrong = sing_the(Some("lead"), None).unwrap_err();
        assert!(wrong.contains("singer"), "{wrong}");

        // With a singer but no voice, the refusal names the missing piece.
        let unvoiced = sing_the(None, None).unwrap_err();
        assert!(unvoiced.contains("voice"), "{unvoiced}");

        // Where a real voice model is around, the door sings for real.
        if let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") {
            let sung = sing_the(None, Some(model.to_string_lossy().into_owned())).unwrap();
            assert!(sung.contains("sang"), "{sung}");
            assert!(sung.contains("seed 0"), "{sung}");
        }

        std::fs::remove_dir_all(&root).unwrap();
    }
}
