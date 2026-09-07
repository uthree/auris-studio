//! `auris-mcp` — the Model Context Protocol frontend.
//!
//! The third frontend, and the first with no person at it: an MCP client — a language model's
//! harness — connects over stdio and drives the same session the desktop application and the
//! command line do. Everything the tools *are* — names, descriptions, argument schemas and the
//! work behind them — lives in [`auris_toolbox`], shared with `auris-agent` so the two doors a
//! model comes through can never drift apart; this crate is the stdio door and nothing else.
//!
//! One seam shows: the doc comment on each method below *is* that tool's wire description, and
//! the SDK's macro only reads it from a literal — it cannot be pointed at the toolbox constant.
//! So the text exists twice, and the test at the bottom holds the two copies equal, which turns
//! silent drift into a red build.
//!
//! What remains here is the protocol binding, and its two decisions:
//!
//! * **Errors are tool answers.** A mistake a model can read and fix (a wrong path, a rejected
//!   spec) comes back as a result with `is_error` set, not as a protocol error — MCP reserves
//!   those for the server itself breaking.
//! * **Blocking work leaves the runtime.** Every tool that opens a session runs inside
//!   `spawn_blocking`, both because the work is honest blocking DSP and because a session
//!   created and dropped inside one closure never has to be `Send`.

#![warn(missing_docs)]

use auris_toolbox as toolbox;
mod previews;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};

/// Project state lives on disk; only the connection's bounded audio resources live here.
#[derive(Clone, Debug, Default)]
struct AurisMcp {
    previews: std::sync::Arc<std::sync::Mutex<previews::Previews>>,
}

/// Both transports publish the shared, self-contained schemas.
fn tool_schema(name: &str) -> std::sync::Arc<rmcp::model::JsonObject> {
    static CATALOG: std::sync::OnceLock<Vec<toolbox::ToolDefinition>> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(toolbox::tool_catalog)
        .iter()
        .find(|tool| tool.name == name)
        .expect("registered toolbox tool")
        .parameters
        .as_object()
        .expect("object argument schema")
        .clone()
        .into()
}

// Every method is one tool: the method name is the tool's wire name, the doc comment its
// description (held equal to the toolbox constant by test), and argument type and work both
// come from the toolbox — this list is the door, not the furniture.
#[tool_router]
impl AurisMcp {
    /// Renders a short project excerpt and sends its actual WAV to the configured audio critic. Use start_bar and bars (default first four bars), or section and optional instance. Keep focus to a short question about the sound. compare_to accepts an earlier audio_path from this project's listen/preview. Review a supported edit by listening to the same range again; leave the mix unchanged when no correction is supported. Returns audio delivery status, fallible observations and separate measurements. Configure AURIS_AUDIO_MODEL and AURIS_AUDIO_URL for a music-capable server. Does not edit the project.
    #[tool(input_schema = tool_schema("listen"))]
    async fn listen(
        &self,
        Parameters(args): Parameters<toolbox::listen::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::listen::run(&args)).await
    }

    /// Creates an empty project with one default instrument track and no clips. Optional tempo and meter set its clock. Output must be a new absolute .auris path; choosing Song.auris writes Song/Song.auris. Returns the actual project path to use in later calls. Use add_clip and edit_notes for manual notes, import_audio for recordings, or add_track for more parts.
    #[tool(input_schema = tool_schema("create_project"))]
    async fn create_project(
        &self,
        Parameters(args): Parameters<toolbox::create_project::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::create_project::run(&args)).await
    }

    /// Imports an audio file into an existing project as a new audio track, starting at start_bar (1-based, default 1). Source must be an absolute path. The session copies the audio into the project Audio folder when possible; the result reports whether it was copied or remains external. Saves with a checkpoint and returns the actual track ID, clip ID and duration.
    #[tool(input_schema = tool_schema("import_audio"))]
    async fn import_audio(
        &self,
        Parameters(args): Parameters<toolbox::import_audio::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::import_audio::run(&args)).await
    }

    /// Imports a .mid or .midi file into a new project, preserving its note timing, tempo and meter. Supply absolute source and new .auris output paths. The MIDI becomes a separate document, with built-in instruments that can be changed using set_instrument. Returns the actual saved project path, track count and note count. Existing projects are never replaced.
    #[tool(input_schema = tool_schema("import_midi"))]
    async fn import_midi(
        &self,
        Parameters(args): Parameters<toolbox::import_midi::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::import_midi::run(&args)).await
    }

    /// Exports instrument-track notes, tempo, meter, pitch bends and MIDI controllers to a new .mid or .midi file. Supply absolute project and output paths. Existing files are never overwritten and the project is unchanged. MIDI does not preserve audio, singer tracks, instruments or the mix; use render for an audio export.
    #[tool(input_schema = tool_schema("export_midi"))]
    async fn export_midi(
        &self,
        Parameters(args): Parameters<toolbox::export_midi::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::export_midi::run(&args)).await
    }

    /// Reads or changes a track's output and sends. Start with operation list to discover available buses and send IDs. Output requires destination (a bus name, id:N, or master). Add_send requires a bus destination and optionally level_db (-60 to 0) and pre_fader. Remove_send, send_mode and send_level select an existing send by destination or send_id; send_mode requires pre_fader and send_level requires level_db (-60 to 0). Send levels report whether automation overrides the static value. A bus can be created with add_track kind bus. Returns the actual routing; changes are checkpointed and saved.
    #[tool(input_schema = tool_schema("routing"))]
    async fn routing(
        &self,
        Parameters(args): Parameters<toolbox::routing::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::routing::run(&args)).await
    }

    /// Sets a track's mute and/or solo state and saves with a checkpoint. Supply mute, solo, or both; omitted switches stay unchanged. Solo is additive: other soloed tracks remain soloed. Returns the actual switches and all currently soloed tracks. Use mixer to inspect the whole mix.
    #[tool(input_schema = tool_schema("set_track_state"))]
    async fn set_track_state(
        &self,
        Parameters(args): Parameters<toolbox::set_track_state::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_track_state::run(&args)).await
    }

    /// Renders an instrument, drum or singer track and replaces it with an audio track at the same position, preserving its ID, mixer, effects and routing. Instrument automation is baked; mixer automation stays editable. A singer uses its current take or generates a fresh one through its chosen voice. Saves with a checkpoint so the original score can be restored. Refuses audio tracks, buses, empty tracks and unavailable sounds.
    #[tool(input_schema = tool_schema("convert_track_to_audio"))]
    async fn convert_track_to_audio(
        &self,
        Parameters(args): Parameters<toolbox::convert_track_to_audio::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::convert_track_to_audio::run(&args)).await
    }

    /// Sets one instrument parameter's static value and saves with a checkpoint. Discover exact parameter keys, units and ranges using automation with target {kind:instrument} and operation {action:read}. Values use those units; invalid ranges and fractional discrete choices are refused. Existing automation is preserved and reported because it overrides the static value during playback.
    #[tool(input_schema = tool_schema("set_instrument_param"))]
    async fn set_instrument_param(
        &self,
        Parameters(args): Parameters<toolbox::set_instrument_param::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_instrument_param::run(&args)).await
    }

    /// Returns the exact argument schema and examples for one tool. Use before an unfamiliar edit or after an argument error; copy the field names and nesting, replacing example paths and selectors with the current project's values. Does not change files.
    #[tool(input_schema = tool_schema("tool_help"))]
    async fn tool_help(
        &self,
        Parameters(args): Parameters<toolbox::tool_help::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        finished(toolbox::tool_help::run(&args))
    }

    /// Reports General MIDI availability, installed voice paths from the desktop's library settings, and optional project playback readiness and selected voice metadata. Voice discovery does not load or validate models. Guide vocals are temporary synthesis; stale takes need sing again. Audio preview creates a playable file; it does not imply that the connected language model can hear audio.
    #[tool(input_schema = tool_schema("capabilities"))]
    async fn capabilities(
        &self,
        Parameters(args): Parameters<toolbox::capabilities::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::capabilities::run(&args)).await
    }

    /// Reads or edits parameter automation. target and operation are objects: target {"kind":"mixer"}, operation {"action":"read"} discovers keys, units and ranges. Other targets: instrument, effect with slot, send with destination. Set example: {"action":"set","param":"gain","points":[{"beat":0,"value":0.5}]}. Beats are absolute quarter notes from zero; values use parameter units. Set merges points; replace true replaces the lane. Curve: linear or hold. Changes are validated, checkpointed and saved.
    #[tool(input_schema = tool_schema("automation"))]
    async fn automation(
        &self,
        Parameters(args): Parameters<toolbox::automation::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::automation::run(&args)).await
    }

    /// Reads or edits a strip's effect chain. operation is an object: {"action":"list"} reads; {"action":"add","effect":"auris.fx.compressor"} inserts. Use track master for the master bus. Slots/positions are 1-based; re-read after reordering. Sidechain source null disconnects. Use set_effect for static parameters and automation for curves. Changes are checkpointed and saved.
    #[tool(input_schema = tool_schema("effects"))]
    async fn effects(
        &self,
        Parameters(args): Parameters<toolbox::effects::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::effects::run(&args)).await
    }

    /// Renders a short WAV audition, at most 120 seconds without effect tails. Supply start_bar and bars at the top level, for example start_bar:1,bars:4; or section and optional instance. Omit both to preview the whole song within the limit. Returns a local audio file; MCP also returns an audio/wav resource link readable through resources/read. Does not change the project. Use render for unrestricted exports.
    #[tool(input_schema = tool_schema("preview"))]
    async fn preview(
        &self,
        Parameters(args): Parameters<toolbox::preview::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        let rendered = tokio::task::spawn_blocking(move || {
            let preview = toolbox::preview::create(&args)?;
            let bytes = std::fs::read(&preview.path).map_err(|e| e.to_string())?;
            Ok::<_, String>((preview.text, bytes))
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match rendered {
            Ok((text, bytes)) => {
                let resource = self
                    .previews
                    .lock()
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                    .insert(bytes)?;
                Ok(CallToolResult::success(vec![
                    ContentBlock::text(text),
                    ContentBlock::resource_link(resource),
                ]))
            }
            Err(error) => finished(Err(error)),
        }
    }
    /// Measures each note clip's pitch range, note density, pitch-class count and exact bar-pattern repetition. Reads stored notes without rendering. These describe musical choices, not aesthetic quality; use analyze for loudness and audio input for listening.
    #[tool(input_schema = tool_schema("analyze_music"))]
    async fn analyze_music(
        &self,
        Parameters(args): Parameters<toolbox::analyze_music::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_music::run(&args)).await
    }

    /// Recognizes chords from written notes on the CPU without rendering or models. Reports absolute-tick intervals, alternate chord symbols and unknown/silent regions. Known percussion is excluded. Apply explicitly replaces recognized harmony and clears silence while preserving unknown intervals and outside harmony; saves a checkpoint.
    #[tool(input_schema = tool_schema("analyze_chords"))]
    async fn analyze_chords(
        &self,
        Parameters(args): Parameters<toolbox::analyze_chords::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_chords::run(&args)).await
    }
    /// Analyzes an audio file on the CPU without models or GPU: constant BPM alternatives, beat timestamps and half-second major/minor chord windows. Scores are template/periodicity agreement, not calibrated probabilities. No project is changed. Does not identify instruments.
    #[tool(input_schema = tool_schema("analyze_audio"))]
    async fn analyze_audio(
        &self,
        Parameters(args): Parameters<toolbox::analyze_audio::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_audio::run(&args)).await
    }
    /// Estimates instrument and singing presence with an explicitly supplied local YAMNet ONNX export on CPU. Returns overlapping source-second windows, multiple candidate labels, raw event scores and model hash. Empty candidates mean unknown. Scores are not calibrated probabilities. No downloads, GPU, source separation, note assignment or project edits.
    #[tool(input_schema = tool_schema("analyze_instruments"))]
    async fn analyze_instruments(
        &self,
        Parameters(args): Parameters<toolbox::analyze_instruments::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_instruments::run(&args)).await
    }
    /// Transcribes an isolated monophonic audio file using CPU YIN, without models or GPU. Supports approximately 65-1000 Hz; does not separate mixed instruments or produce engraved staff notation. Returns source-second note estimates. Optional MIDI output creates a new file; apply adds an editable note track to a project and saves a checkpoint. Existing notes and tempo are preserved.
    #[tool(input_schema = tool_schema("transcribe_audio"))]
    async fn transcribe_audio(
        &self,
        Parameters(args): Parameters<toolbox::transcribe_audio::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::transcribe_audio::run(&args)).await
    }
    /// Uses user-converted MuScriptor Small ONNX on CPU for instrument-labeled note drafts. Its model is CC BY-NC 4.0, noncommercial only; present this restriction and obtain explicit user acknowledgement for this invocation before setting acknowledge_noncommercial=true. Acknowledgement does not grant commercial rights. Auris itself remains Apache-2.0. Select decoder.onnx beside audio.onnx and muscriptor.json, prepared with export_muscriptor.py. Runtime requires no Python or downloads. Defaults to read-only JSON. Optional MIDI creates a new file; apply adds instrument tracks and saves a checkpoint. Notes and playback patches need review.
    #[tool(input_schema = tool_schema("transcribe_mixture"))]
    async fn transcribe_mixture(
        &self,
        Parameters(args): Parameters<toolbox::transcribe_mixture::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::transcribe_mixture::run(&args)).await
    }
    /// Reads the original song specification and the current key, chords, tempo, meter, sections and clip recipes. The specification is provenance; later manual edits are represented by the current state, not by that original text.
    #[tool(input_schema = tool_schema("inspect_composition"))]
    async fn inspect_composition(
        &self,
        Parameters(args): Parameters<toolbox::inspect_composition::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::inspect_composition::run(&args)).await
    }

    /// Changes key, chord progression, tempo or section label at a bar in an existing project. Optional bars bounds the progression and restores the previous key and tempo at the end. Existing notes stay unchanged; explicitly call regenerate_clips on the parts that should follow the new harmony.
    #[tool(input_schema = tool_schema("edit_harmony"))]
    async fn edit_harmony(
        &self,
        Parameters(args): Parameters<toolbox::edit_harmony::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::edit_harmony::run(&args)).await
    }

    /// Changes a generated clip's recipe and regenerates that clip only. With drum_voice, edits only the named writer within a drum kit. Unspecified controls and the seed are kept. Read inspect_composition first. Hand-edited notes require replace_hand_edits; a checkpoint preserves the previous document. Use edit_clip with freeze to keep a take without its recipe.
    #[tool(input_schema = tool_schema("edit_recipe"))]
    async fn edit_recipe(
        &self,
        Parameters(args): Parameters<toolbox::edit_recipe::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::edit_recipe::run(&args)).await
    }

    /// Moves, duplicates, copies, splits, resizes, removes, mutes or freezes one note clip. Pass action as an object: resize uses {kind:resize,end_bar:9} to end before bar 9 (eight bars from bar 1); move uses {kind:move,bar:5}. Copy can transpose stored notes and freezes a transposed recipe. Track and 1-based clip numbers come from describe. Positions use absolute song bars and meter beats. Resize can regenerate; freeze first to preserve written notes. Changes are checkpointed and saved.
    #[tool(input_schema = tool_schema("edit_clip"))]
    async fn edit_clip(
        &self,
        Parameters(args): Parameters<toolbox::edit_clip::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::edit_clip::run(&args)).await
    }

    /// Lists, creates or restores document checkpoints in the project folder. Editing tools automatically keep the previous document. Create a named checkpoint for an A/B comparison; restore brings its notes, harmony and mix back and saves, keeping the current version in another automatic checkpoint. Assets are referenced, not copied.
    #[tool(input_schema = tool_schema("checkpoints"))]
    async fn checkpoints(
        &self,
        Parameters(args): Parameters<toolbox::checkpoints::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::checkpoints::run(&args)).await
    }

    /// Searches the Auris Studio documentation embedded in this build. Use it for questions about features, workflows, composition, development, evaluation, and singing-voice training. Returns the most relevant passages with their document paths and section headings.
    #[tool(input_schema = tool_schema("search_documentation"))]
    async fn search_documentation(
        &self,
        Parameters(args): Parameters<toolbox::search_documentation::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        finished(toolbox::search_documentation::run(&args))
    }

    /// The `.asong` format, taught by example: a two-line song, then a specification using most of the vocabulary with a comment on every field. Read this before writing a spec.
    #[tool(input_schema = tool_schema("spec_reference"))]
    async fn spec_reference(&self) -> Result<CallToolResult, ErrorData> {
        finished(Ok(toolbox::spec_reference::run()))
    }

    /// Validates a specification without composing anything. A rejected spec answers with every complaint at once, line numbers where they exist; a valid one answers with the full document, every default filled in — the cheap way to see what a draft actually means.
    #[tool(input_schema = tool_schema("check_spec"))]
    async fn check_spec(
        &self,
        Parameters(args): Parameters<toolbox::check_spec::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        // No session and no blocking work — parsing a spec is pure text.
        finished(toolbox::check_spec::run(&args))
    }

    /// Composes a song from a specification and saves it as a project. The answer reports what was written — tracks, notes, seed, where the mix was measured to — and the seed is what to pin in the spec to ask for this exact take again.
    #[tool(input_schema = tool_schema("compose"))]
    async fn compose(
        &self,
        Parameters(args): Parameters<toolbox::compose::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::compose::run(&args)).await
    }

    /// Renders a project to a WAV file — or, with `stems`, to one file per track — and reports each file's length, channels and peak level. Optionally select start_bar + bars or one section occurrence; ranges omit tails by default.
    #[tool(input_schema = tool_schema("render"))]
    async fn render(
        &self,
        Parameters(args): Parameters<toolbox::render::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::render::run(&args)).await
    }

    /// Describes a project on disk: tempo, meter, duration, and every track with its instrument, clip count, effects and routing.
    #[tool(input_schema = tool_schema("describe"))]
    async fn describe(
        &self,
        Parameters(args): Parameters<toolbox::describe::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::describe::run(&args)).await
    }

    /// Listens to a project and reports what it measured, changing nothing: length, integrated loudness and peaks for the whole mix, the same per named section — the piece's dynamic arc as numbers — and, with `per_track`, each track alone. This is the ears of the improve loop: render, analyze, edit the spec or rewrite one clip, and ask again.
    #[tool(input_schema = tool_schema("analyze"))]
    async fn analyze(
        &self,
        Parameters(args): Parameters<toolbox::analyze::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze::run(&args)).await
    }

    /// Renders a drum track's instrument at several velocities and reports spectral and envelope measurements, role-fit scores and proposed drum mappings. Requires kind drum; melodic tracks are refused. Classification uses audio only, never note names or GM numbers; fit scores are not probabilities and missing roles are allowed. With apply, saves the computed map; remap_clips also retargets generated drum notes and their recipes. Existing notes are unchanged by analysis alone.
    #[tool(input_schema = tool_schema("analyze_drum_kit"))]
    async fn analyze_drum_kit(
        &self,
        Parameters(args): Parameters<toolbox::analyze_drum_kit::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_drum_kit::run(&args)).await
    }

    /// Adds or changes one drum track role's MIDI note assignment, or removes it with remove: true. Roles are kick, snare, closed_hat, open_hat, crash and tom; note is 0-127. Requires an explicit drum track. Saves the assignment for the drum editor and future generation without rewriting existing clips or their saved recipes. Other assignments and instrument state are preserved; removing the final role leaves an explicitly empty map.
    #[tool(input_schema = tool_schema("set_drum_assignment"))]
    async fn set_drum_assignment(
        &self,
        Parameters(args): Parameters<toolbox::set_drum_assignment::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_drum_assignment::run(&args)).await
    }

    /// Reads the mixer as it stands: every track's fader, pan, mute and solo, its sends, and each effect's parameters with key, value and range — the vocabulary `set_level`, `routing` and `set_effect` move. A control marked `[automated]` is driven by its lane, not its stored value. Gain envelopes include every point and section midpoint values.
    #[tool(input_schema = tool_schema("mixer"))]
    async fn mixer(
        &self,
        Parameters(args): Parameters<toolbox::mixer::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::mixer::run(&args)).await
    }

    /// Sets a track's fader and/or pan; `track` may be "master". Gain runs -60 to +12 dB, pan -1 (left) to +1 (right). The change is saved — `analyze` again to hear what it did to the numbers. A fader that `mixer` marks `[automated]` is ruled by its lane, not this value; `section_gain` with clear: true removes the lane.
    #[tool(input_schema = tool_schema("set_level"))]
    async fn set_level(
        &self,
        Parameters(args): Parameters<toolbox::set_level::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_level::run(&args)).await
    }

    /// Sets one static effect parameter. Read mixer, then provide track (or master), slot (1-based chain position), param (key/name) and value in its listed units. Example: slot 1, param threshold_db, value -18. Optional effect checks that the slot contains the expected effect id. Out-of-range values are refused. Changes are saved. Lower the master limiter's input_db when loud sections hit its ceiling.
    #[tool(input_schema = tool_schema("set_effect"))]
    async fn set_effect(
        &self,
        Parameters(args): Parameters<toolbox::set_effect::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_effect::run(&args)).await
    }

    /// Holds a track's gain at a level across one named section — dynamics without rewriting a note. `track` may be "master"; the section is addressed by the label `analyze` shows, every occurrence unless `instance` picks one. Writes gain automation with short ramps at the edges: the fader keeps ruling outside the stretch, and holds on different sections compose. `clear: true` removes the track's whole gain lane instead, giving the fader back everywhere. The change is saved. The master fader sits after the master chain, so a boost there is not limited and can clip — widen contrast by holding the louder sections down instead. Use gain_delta_db instead of gain_db to offset the existing envelope; mixer reads it back.
    #[tool(input_schema = tool_schema("section_gain"))]
    async fn section_gain(
        &self,
        Parameters(args): Parameters<toolbox::section_gain::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::section_gain::run(&args)).await
    }

    /// Regenerates selected clips against the current harmony. Required take is an object: {kind:same} keeps each seed, {kind:next} writes a new take, or {kind:seed,seed:42} selects an exact seed for one clip. Track and optional 1-based clip come from describe; omitting clip selects all generated clips on the track. Every target is checked before changes; hand-edited clips require replace_hand_edits:true. Saves a checkpoint and reports each seed.
    #[tool(input_schema = tool_schema("regenerate_clips"))]
    async fn regenerate_clips(
        &self,
        Parameters(args): Parameters<toolbox::regenerate_clips::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::regenerate_clips::run(&args)).await
    }

    /// Keeps a chord progression under a name on this machine. It then shows up in `list_progressions` and the desktop picker; a specification still writes the chords out in full — only the built-in catalogue is quotable as `@name`, so a document stays portable.
    #[tool(input_schema = tool_schema("teach_progression"))]
    async fn teach_progression(
        &self,
        Parameters(args): Parameters<toolbox::teach_progression::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::teach_progression::run(&args)).await
    }

    /// Forgets a progression kept with `teach_progression`, by name.
    #[tool(input_schema = tool_schema("forget_progression"))]
    async fn forget_progression(
        &self,
        Parameters(args): Parameters<toolbox::forget_progression::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::forget_progression::run(&args)).await
    }

    /// Lists the chord progressions a specification can quote by name, with the chords each one plays.
    #[tool(input_schema = tool_schema("list_progressions"))]
    async fn list_progressions(&self) -> Result<CallToolResult, ErrorData> {
        blocking(move || Ok(toolbox::list_progressions::run())).await
    }

    /// Lists the whole songs a specification can start from, with each one's key, tempo and groove.
    #[tool(input_schema = tool_schema("list_presets"))]
    async fn list_presets(&self) -> Result<CallToolResult, ErrorData> {
        finished(Ok(toolbox::list_presets::run()))
    }

    /// Lists the built-in instruments a track can play, by the id `add_track` and `set_instrument` take. Reports whether the General MIDI library is loaded; when available, select a GM name or program number using sound.
    #[tool(input_schema = tool_schema("list_instruments"))]
    async fn list_instruments(&self) -> Result<CallToolResult, ErrorData> {
        blocking(move || Ok(toolbox::list_instruments::run())).await
    }

    /// Adds a named track and saves. Required kind selects instrument, drum, singer, audio or bus; a bus name alone does not create a bus. For instrument or drum tracks, choose instrument from list_instruments or sound by General MIDI name/program; omitting both uses the default instrument. Kind drum uses the drum editor and treats sound as a GM kit; New note tracks have no clips: add_clip creates an empty named clip; add_part generates notes.
    #[tool(input_schema = tool_schema("add_track"))]
    async fn add_track(
        &self,
        Parameters(args): Parameters<toolbox::add_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_track::run(&args)).await
    }

    /// Writes a generated part onto an existing instrument track, from the key and chords already under the song — lead, chords, pad, arp, bass, stab, drums, kick, snare or hat. Covers the whole song unless `start_bar` and `bars` aim it. The clip keeps its recipe, so `regenerate_clips` chooses a new take or follows a harmony change; the answer numbers it the way `describe` does.
    #[tool(input_schema = tool_schema("add_part"))]
    async fn add_part(
        &self,
        Parameters(args): Parameters<toolbox::add_part::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_part::run(&args)).await
    }

    /// Re-voices an instrument track: `instrument` names a built-in from `list_instruments`, or `sound` names a General MIDI sound (a name or a program number, `drums: true` for a kit). The previous instrument's dial positions and the automation that drove them go with it. The change is saved.
    #[tool(input_schema = tool_schema("set_instrument"))]
    async fn set_instrument(
        &self,
        Parameters(args): Parameters<toolbox::set_instrument::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_instrument::run(&args)).await
    }

    /// Renames a track. Every other tool addresses tracks by name, or `id:<number>`. The ID survives a rename; a new name must be unique. The change is saved.
    #[tool(input_schema = tool_schema("rename_track"))]
    async fn rename_track(
        &self,
        Parameters(args): Parameters<toolbox::rename_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::rename_track::run(&args)).await
    }

    /// Removes a track and everything on it — its clips, its effect chain, its sends and its automation. The change is saved.
    #[tool(input_schema = tool_schema("remove_track"))]
    async fn remove_track(
        &self,
        Parameters(args): Parameters<toolbox::remove_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::remove_track::run(&args)).await
    }

    /// Opens an empty named clip on an instrument or singer track for edit_notes. Required name is the intended clip name; preserve the user's exact name. Aim it with start_bar and bars; the answer numbers the clip the way describe does.
    #[tool(input_schema = tool_schema("add_clip"))]
    async fn add_clip(
        &self,
        Parameters(args): Parameters<toolbox::add_clip::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_clip::run(&args)).await
    }

    /// Reads one clip's notes, numbered in time order — pitch, bar, beat, length in beats, velocity and, where a note carries one, its lyric. The numbers are the address `edit_notes` removes and `write_lyrics` starts by; aim with `track` and the clip number `describe` shows.
    #[tool(input_schema = tool_schema("notes"))]
    async fn notes(
        &self,
        Parameters(args): Parameters<toolbox::notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::notes::run(&args)).await
    }

    /// Adds and removes notes in one clip, in one call: `remove` takes the numbers `notes` lists, `add` takes notes as pitch (a name like "F#4" or a MIDI number), 1-based bar and beat in the song, length in beats, and velocity 0-1 (0.75 when left out). Removals happen first. The change is saved. On a generated clip the edit sticks until `regenerate_clips` rewrites the clip whole.
    #[tool(input_schema = tool_schema("edit_notes"))]
    async fn edit_notes(
        &self,
        Parameters(args): Parameters<toolbox::edit_notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::edit_notes::run(&args)).await
    }

    /// Reads a melody clip and writes a key, a chord progression and backing tracks under it — the melody-first way around: place the tune with `edit_notes`, then derive the band. The melody itself is not touched. `parts` picks the band (bass, chords and drums when left out); the harmony it writes is a first draft to argue with — `regenerate_clips` re-derives any part after a correction. The change is saved.
    #[tool(input_schema = tool_schema("accompany"))]
    async fn accompany(
        &self,
        Parameters(args): Parameters<toolbox::accompany::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::accompany::run(&args)).await
    }

    /// Lays a phrase across a singer clip's notes, one syllable to each, and derives the phonemes it will be sung as — kana through the built-in table, other text through the Japanese dictionary where one is installed. `from` starts partway in, at a number the way `notes` counts them, so a verse is filled one line at a time; notes past the end of the phrase keep their words. The change is saved.
    #[tool(input_schema = tool_schema("write_lyrics"))]
    async fn write_lyrics(
        &self,
        Parameters(args): Parameters<toolbox::write_lyrics::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::write_lyrics::run(&args)).await
    }

    /// Renders a singer track through its voice model and keeps the audio as the track's take, which is what playback and `render` then play. Aims at the project's only singer track when `track` is left out. `voice` chooses a model the first time — an absolute path to an Auris `.onnx` voice, DiffSinger `dsconfig.yaml`, VOICEVOX `.voicevox.json` connection, or LeapSinger `.leapsinger.json` manifest, which the track keeps. Native Auris voices use `seed` to reproduce a take. LeapSinger generates noise internally, so repeated renders can differ even with the same seed. The rendered audio and the change are saved.
    #[tool(input_schema = tool_schema("sing"))]
    async fn sing(
        &self,
        Parameters(args): Parameters<toolbox::sing::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::sing::run(&args)).await
    }

    /// Writes a song from Japanese lyrics and saves it as a new project: a melody searched under the words the Orpheus way, sung notes carrying each syllable, chords in the harmony lane, and a backing band unless `melody_only`. Where a Japanese dictionary is configured the melody follows the lyric's pitch accent; kana lyrics work without one, free of the accent. Phrases break at line breaks and punctuation. The same lyrics and `seed` write the same song; `sing` then gives the vocal its voice.
    #[tool(input_schema = tool_schema("compose_lyrics"))]
    async fn compose_lyrics(
        &self,
        Parameters(args): Parameters<toolbox::compose_lyrics::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::compose_lyrics::run(&args)).await
    }
}

#[tool_handler]
impl ServerHandler for AurisMcp {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        let router = Self::tool_router();
        let known = router.has_route(&name);
        let call = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        recover_argument_error(router.call(call).await, &name, known)
    }

    async fn list_resources(
        &self,
        _: Option<rmcp::model::PaginatedRequestParams>,
        _: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourcesResult, ErrorData> {
        let resources = self
            .previews
            .lock()
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .list();
        Ok(rmcp::model::ListResourcesResult {
            resources,
            ..Default::default()
        })
    }
    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        _: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let bytes = self
            .previews
            .lock()
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .bytes(&request.uri)?;
        let content = previews::Previews::content(request.uri, &bytes);
        Ok(rmcp::model::ReadResourceResult::new(vec![content]).into())
    }
    fn get_info(&self) -> ServerInfo {
        // Field by field because the type is `non_exhaustive`, which rules the literal out.
        // Named explicitly rather than via `Implementation::from_build_env`, whose `env!` was
        // expanded when *rmcp* was compiled — a server introducing itself as "rmcp 3.1.4".
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.instructions = Some(toolbox::INSTRUCTIONS.into());
        info
    }
}

/// Invalid arguments are recoverable model mistakes; unknown tools and server errors retain
/// their protocol error codes.
fn recover_argument_error(
    result: Result<rmcp::model::CallToolResponse, ErrorData>,
    name: &str,
    known: bool,
) -> Result<rmcp::model::CallToolResponse, ErrorData> {
    match result {
        Err(error) if known && error.code == rmcp::model::ErrorCode::INVALID_PARAMS => {
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Invalid arguments for {name}: {}. Call tool_help with name '{name}' for exact fields and examples, then correct the arguments.", error.message
            ))]).into())
        }
        other => other,
    }
}

/// Wraps work that touches a session or the filesystem, off the async runtime.
///
/// Everything behind these tools is honest blocking work — opening a session parses SoundFont
/// files, a render is minutes of DSP — and tokio's worker threads are for neither. `Ok` and
/// `Err` both become *results* here: an error a model can read and fix is a tool answer, not a
/// protocol failure, which MCP reserves for the server itself breaking — the one thing left
/// for the outer `Result`.
async fn blocking(
    work: impl FnOnce() -> Result<String, String> + Send + 'static,
) -> Result<CallToolResult, ErrorData> {
    let outcome = tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    finished(outcome)
}

/// Turns a tool's verdict into the result the protocol carries.
fn finished(outcome: Result<String, String>) -> Result<CallToolResult, ErrorData> {
    Ok(match outcome {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(code) = auris_session::handle_drum_probe_worker() {
        std::process::exit(code);
    }
    // Stderr, and only stderr: stdout is the protocol channel, and one stray line on it is a
    // broken connection.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Like the CLI: nothing here reads the configuration, but this may still be the frontend
    // that runs first on a machine, and an installation predating the move to
    // `~/.config/auris-studio` only has its settings carried across by whichever one does.

    tokio::runtime::Runtime::new()?.block_on(async {
        let service = AurisMcp::default().serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_arguments_are_tool_feedback_but_protocol_errors_keep_their_codes() {
        let result = recover_argument_error(
            Err(ErrorData::invalid_params("missing end_bar", None)),
            "edit_clip",
            true,
        )
        .unwrap();
        let rmcp::model::CallToolResponse::Complete(result) = result else {
            panic!("expected tool result")
        };
        assert_eq!(result.is_error, Some(true));
        assert!(format!("{:?}", result.content).contains("tool_help"));
        let unknown = recover_argument_error(
            Err(ErrorData::invalid_params("unknown tool", None)),
            "invented",
            false,
        )
        .unwrap_err();
        assert_eq!(unknown.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        let internal = recover_argument_error(
            Err(ErrorData::internal_error("worker failed", None)),
            "edit_clip",
            true,
        )
        .unwrap_err();
        assert_eq!(internal.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    }

    #[test]
    fn the_server_introduces_itself_and_carries_the_shared_instructions() {
        let info = AurisMcp::default().get_info();
        // The name is this crate's, not the SDK's — the `from_build_env` trap in `get_info`.
        assert_eq!(info.server_info.name, "auris-mcp");
        assert_eq!(
            info.instructions.as_deref(),
            Some(toolbox::INSTRUCTIONS),
            "both doors hand a model the same standing instructions"
        );
    }

    /// The doc comments above are wire descriptions the SDK's macro will only read from
    /// literals, so the toolbox text is copied rather than named. This is where the copies
    /// are held together: every tool this door serves must carry, word for word, the
    /// description the toolbox declares for that name — the same text `auris-agent` sends.
    #[test]
    fn every_wire_description_is_the_toolbox_text_word_for_word() {
        let catalog = toolbox::tool_catalog();
        let expected: std::collections::BTreeMap<_, _> = catalog
            .iter()
            .map(|tool| (tool.name, tool.description))
            .collect();
        let served = AurisMcp::tool_router().list_all();
        assert_eq!(
            served.len(),
            expected.len(),
            "every shared tool is registered at this door"
        );
        for tool in served {
            let shared = catalog
                .iter()
                .find(|entry| entry.name == tool.name.as_ref())
                .unwrap();
            assert_eq!(
                tool.input_schema.as_ref(),
                shared.parameters.as_object().unwrap(),
                "{} schema differs from the shared catalog",
                tool.name
            );
            let description = tool.description.as_deref().unwrap_or_default();
            let toolbox_text = expected
                .get(tool.name.as_ref())
                .unwrap_or_else(|| panic!("'{}' is not a toolbox tool", tool.name));
            // Doc comments arrive one line per `///` with the indentation trimmed; the
            // constant is one wrapped string. Compare word sequences, which is what a model
            // reads either way.
            let words = |text: &str| {
                text.split_whitespace()
                    .map(String::from)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                words(description),
                words(toolbox_text),
                "'{}' says something different at this door",
                tool.name
            );
        }
    }
}
