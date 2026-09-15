//! `auris-mcp` — the Model Context Protocol frontend.
//!
//! The third frontend, and the first with no person at it: an MCP client — a language model's
//! harness — connects over stdio and drives the same session the desktop application and the
//! command line do. Everything the tools *are* — names, descriptions, argument schemas and the
//! work behind them — lives in [`auris_toolbox`], shared with `auris-agent` so the two doors a
//! model comes through can never drift apart; this crate is the stdio door and nothing else.
//!
//! One seam shows: the doc comment on each method below is its full registered description, and
//! the SDK's macro only reads it from a literal — it cannot be pointed at the toolbox constant.
//! So the text exists twice, and the test at the bottom holds the two copies equal, which turns
//! silent drift into a red build.
//! `tools/list` applies the shared compact presentation and startup task-group filter;
//! `tool_help` retains the full description and schema.
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

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};

tokio::task_local! {
    /// Cancellation state for the request currently executing on this async task.
    static REQUEST_CANCELLATION: std::sync::Arc<toolbox::Cancellation>;
}

/// Saved projects and rendered audio stay on disk.
#[derive(Clone, Debug, Default)]
struct AurisMcp {
    groups: toolbox::ToolGroups,
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
    /// Open a saved project and return project_id for subsequent project arguments. Handles expire on server restart or after 64 distinct projects. Does not change files.
    #[tool(input_schema = tool_schema("open_project"))]
    async fn open_project(
        &self,
        Parameters(args): Parameters<toolbox::open_project::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::open_project::run(&args)).await
    }
    /// Add 1..16 tracks with optional sound_id and empty clip in one atomic save. Use a unique request_id; identical retries return the same result while the document is unchanged (last 32 receipts, this server lifetime). Existing track names are rejected. On conflict, inspect the project before a new request.
    #[tool(input_schema = tool_schema("setup_tracks"))]
    async fn setup_tracks(
        &self,
        Parameters(args): Parameters<toolbox::setup_tracks::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::setup_tracks::run(&args)).await
    }
    /// Read scan and acoustic measurement failures after search_instruments or similar_instruments. Does not rescan. Messages are bounded; follow next_offset.
    #[tool(input_schema = tool_schema("instrument_diagnostics"))]
    async fn instrument_diagnostics(
        &self,
        Parameters(args): Parameters<toolbox::instrument_diagnostics::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::instrument_diagnostics::run(&args)).await
    }
    /// Find tool names by text and optional task group, without full schemas. Fetch tool_help for one result. Startup AURIS_MCP_TOOL_GROUPS controls which groups are callable.
    #[tool(input_schema = tool_schema("discover_tools"))]
    async fn discover_tools(
        &self,
        Parameters(args): Parameters<toolbox::discover_tools::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::discover_tools::run(&args)).await
    }
    /// Renders a short project excerpt and sends its actual WAV to the configured audio critic. Use start_bar and bars (default first four bars), or section and optional instance. Keep focus to a short question about the sound. compare_to accepts an earlier audio_path from this project's listen/preview. Review a supported edit by listening to the same range again; leave the mix unchanged when no correction is supported. Returns audio delivery status, fallible observations and separate measurements. Configure AURIS_AUDIO_MODEL and AURIS_AUDIO_URL for a music-capable server. Does not edit the project.
    #[tool(input_schema = tool_schema("listen"))]
    async fn listen(
        &self,
        Parameters(args): Parameters<toolbox::listen::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::listen::run(&args)).await
    }

    /// Reads a cached immutable report snapshot without repeating analysis or edits. Copy report_id and report_path from a large report response. path is a JSON pointer (empty for root); offset follows next_offset. Arrays/objects page entries; strings page Unicode characters. Snapshots expire on server restart or eviction and may predate project edits.
    #[tool(input_schema = tool_schema("read_report"))]
    async fn read_report(
        &self,
        Parameters(args): Parameters<toolbox::read_report::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::read_report::run(&args)).await
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

    /// Reads or edits parameter automation. target and operation are objects: target {"kind":"mixer"}, operation {"action":"read"} discovers keys, units and ranges. Other targets: instrument, effect with slot, send with destination. Set example: {"action":"set","param":"gain","points":[{"beat":0,"value":0.5}]}. Beats are absolute quarter notes from zero; values use parameter units. Set merges points; replace true replaces the lane. Curve: linear or hold. Read pages default to 32 (max 128): omit param for parameter summaries, or supply param for points and choices. Follow top-level next_offset for parameters, lane.next_offset for points, and next_choice_offset with choice_offset for choices. Set/clear returns only saved status and point count. Changes are validated, checkpointed and saved.
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

    /// Renders a short WAV audition, at most 120 seconds without effect tails. Supply start_bar and bars at the top level, for example start_bar:1,bars:4; or section and optional instance. Omit both to preview the whole song within the limit. Returns the absolute path of the local WAV file and measurements. Open that file with a local audio player. Does not change the project. Use render for unrestricted exports.
    #[tool(input_schema = tool_schema("preview"))]
    async fn preview(
        &self,
        Parameters(args): Parameters<toolbox::preview::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::preview::run(&args)).await
    }
    /// Measures each note clip's pitch range, note density, pitch-class count and exact bar-pattern repetition. Reads stored notes without rendering. These describe musical choices, not aesthetic quality; use analyze for loudness and audio input for listening. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("analyze_music"))]
    async fn analyze_music(
        &self,
        Parameters(args): Parameters<toolbox::analyze_music::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_music::run(&args)).await
    }

    /// Recognizes chords from written notes on the CPU without rendering or models. Reports absolute-tick intervals, alternate chord symbols and unknown/silent regions. Known percussion is excluded. Apply explicitly replaces recognized harmony and clears silence while preserving unknown intervals and outside harmony; saves a checkpoint. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("analyze_chords"))]
    async fn analyze_chords(
        &self,
        Parameters(args): Parameters<toolbox::analyze_chords::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_chords::run(&args)).await
    }
    /// Analyzes an audio file on the CPU without models or GPU: constant BPM alternatives, beat timestamps and half-second major/minor chord windows. Scores are template/periodicity agreement, not calibrated probabilities. No project is changed. Does not identify instruments. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("analyze_audio"))]
    async fn analyze_audio(
        &self,
        Parameters(args): Parameters<toolbox::analyze_audio::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_audio::run(&args)).await
    }
    /// Estimates instrument and singing presence with an explicitly supplied local YAMNet ONNX export on CPU. Returns overlapping source-second windows, multiple candidate labels, raw event scores and model hash. Empty candidates mean unknown. Scores are not calibrated probabilities. No downloads, GPU, source separation, note assignment or project edits. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("analyze_instruments"))]
    async fn analyze_instruments(
        &self,
        Parameters(args): Parameters<toolbox::analyze_instruments::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze_instruments::run(&args)).await
    }
    /// Transcribes an isolated monophonic audio file using CPU YIN, without models or GPU. Supports approximately 65-1000 Hz; does not separate mixed instruments or produce engraved staff notation. Returns source-second note estimates. Optional MIDI output creates a new file; apply adds an editable note track to a project and saves a checkpoint. Existing notes and tempo are preserved. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("transcribe_audio"))]
    async fn transcribe_audio(
        &self,
        Parameters(args): Parameters<toolbox::transcribe_audio::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::transcribe_audio::run(&args)).await
    }
    /// Uses user-converted MuScriptor Small ONNX on CPU for instrument-labeled note drafts. Its model is CC BY-NC 4.0, noncommercial only; present this restriction and obtain explicit user acknowledgement for this invocation before setting acknowledge_noncommercial=true. Acknowledgement does not grant commercial rights. Auris itself remains Apache-2.0. Select decoder.onnx beside audio.onnx and muscriptor.json, prepared with export_muscriptor.py. Runtime requires no Python or downloads. Defaults to read-only JSON. Optional MIDI creates a new file; apply adds instrument tracks and saves a checkpoint. Notes and playback patches need review. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("transcribe_mixture"))]
    async fn transcribe_mixture(
        &self,
        Parameters(args): Parameters<toolbox::transcribe_mixture::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::transcribe_mixture::run(&args)).await
    }
    /// Reads the original song specification and the current key, chords, tempo, meter, sections and clip recipes. The specification is provenance; later manual edits are represented by the current state, not by that original text. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
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
    /// For repeating game BGM, use preset game-loop or set ending: loop; the complete form becomes the enabled cycle region.
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

    /// Describes a project on disk: tempo, meter, duration, and every track with its instrument, clip count, effects and routing. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
    #[tool(input_schema = tool_schema("describe"))]
    async fn describe(
        &self,
        Parameters(args): Parameters<toolbox::describe::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::describe::run(&args)).await
    }

    /// Listens to a project and reports what it measured, changing nothing: length, integrated loudness and peaks for the whole mix, the same per named section — the piece's dynamic arc as numbers — and, with `per_track`, each track alone. This is the ears of the improve loop: render, analyze, edit the spec or rewrite one clip, and ask again. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.
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

    /// Reads the mixer as it stands: every track's fader, pan, mute and solo, its sends, and each effect's parameters with key, value and range — the vocabulary `set_level`, `routing` and `set_effect` move. A control marked `[automated]` is driven by its lane, not its stored value. Strip pages default to 16 (max 32). Gain points, section midpoints, effect parameters and choices are previews of at most 16 each; use automation for paged parameter, point and choice details. Follow next_offset for more strips.
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

    /// Legacy compact built-in summary. Use search_instruments with a focused query to discover selectable sounds and plugin presets; use similar_instruments for acoustic alternatives.
    #[tool(input_schema = tool_schema("list_instruments"))]
    async fn list_instruments(&self) -> Result<CallToolResult, ErrorData> {
        blocking(move || Ok(toolbox::list_instruments::run())).await
    }

    /// Search sounds by name, library, vendor or tags; all query words must match. Filter by source and library before paging. Returns at most 50 sound IDs for sound_id in add_track/set_instrument/setup_tracks. IDs expire on explicit refresh, a library identity change, bounded handle eviction, or server restart. Each sound.library indexes the response libraries array. Read instrument_diagnostics for scan failures.
    #[tool(input_schema = tool_schema("search_instruments"))]
    async fn search_instruments(
        &self,
        Parameters(args): Parameters<toolbox::search_instruments::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::search_instruments::run(&args)).await
    }

    /// Find acoustic alternatives to a searched sound ID using full standardized timbre vectors. Filter source/library before selecting at most 50 neighbors; excludes the reference and drum kits. First use starts indexing: continue other work and retry the same id later. Lower distance is closer, not better. Each sound.library indexes libraries. Read instrument_diagnostics for failed measurements.
    #[tool(input_schema = tool_schema("similar_instruments"))]
    async fn similar_instruments(
        &self,
        Parameters(args): Parameters<toolbox::similar_instruments::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::similar_instruments::run(&args)).await
    }

    /// Add a named track and save. Required kind selects instrument, drum, singer, audio or bus. Use sound_id from search_instruments/similar_instruments for an exact sound on instrument/drum tracks; omit for the default. New note tracks have no clips. Prefer setup_tracks to create multiple tracks with sounds and empty clips atomically.
    #[tool(input_schema = tool_schema("add_track"))]
    async fn add_track(
        &self,
        Parameters(args): Parameters<toolbox::add_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_track::run(&args)).await
    }

    /// Writes a generated part onto an existing instrument track, from the key and chords already under the song — lead, chords, pad, arp, bass, drums, kick, snare or hat. Covers the whole song unless `start_bar` and `bars` aim it. The clip keeps its recipe, so `regenerate_clips` chooses a new take or follows a harmony change; the answer numbers it the way `describe` does.
    #[tool(input_schema = tool_schema("add_part"))]
    async fn add_part(
        &self,
        Parameters(args): Parameters<toolbox::add_part::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_part::run(&args)).await
    }

    /// Replace an instrument/drum track sound using sound_id from search_instruments/similar_instruments for this project. Keeps notes and mixer settings but clears previous instrument parameters and their automation. Saves the change.
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

    /// Reads one clip's notes, numbered in time order — pitch, bar, beat, length in beats, velocity and, where a note carries one, its lyric. The numbers are the address `edit_notes` removes and `write_lyrics` starts by; aim with `track` and the clip number `describe` shows. Returns at most 128 notes; follow next_offset without editing between pages. Note numbers remain global within the clip.
    #[tool(input_schema = tool_schema("notes"))]
    async fn notes(
        &self,
        Parameters(args): Parameters<toolbox::notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::notes::run(&args)).await
    }

    /// Adds and removes notes in one clip, in one call: `remove` takes the numbers `notes` lists, `add` takes notes as pitch (a name like "F#4" or a MIDI number), 1-based bar and beat in the song, length in beats, and velocity 0-1 (0.75 when left out). Removals happen first. The change is saved. On a generated clip the edit sticks until `regenerate_clips` rewrites the clip whole. Inline add and remove each allow at most 256 entries. For a complete larger score, use replace_notes with source pointing to a JSON file.
    #[tool(input_schema = tool_schema("edit_notes"))]
    async fn edit_notes(
        &self,
        Parameters(args): Parameters<toolbox::edit_notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::edit_notes::run(&args)).await
    }

    /// Replaces all authored notes in one clip. Supply exactly one of notes (an array) or source (an absolute path to a UTF-8 JSON array on the MCP server). Use source for script-generated scores instead of printing and copying large arrays into tool calls. Notes use pitch (60, "60", or "C4"), song-relative 1-based bar and beat, beats for duration, and optional velocity 0-1 (default 0.75). All notes are validated before changing the clip. Identical retries do not duplicate notes or create checkpoints; an empty array clears notes. Preserves clip length, curves, transforms and recipe; regeneration can overwrite authored notes. Inline notes allow at most 256 entries; larger scores require source (maximum 65536 notes and 16 MiB per file). Saves with a checkpoint.
    #[tool(input_schema = tool_schema("replace_notes"))]
    async fn replace_notes(
        &self,
        Parameters(args): Parameters<toolbox::replace_notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::replace_notes::run(&args)).await
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
    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        let tools = toolbox::tool_catalog()
            .into_iter()
            .filter(|t| self.groups.allows(t.name))
            .map(|mut t| {
                toolbox::compact_parameters(&mut t.parameters);
                rmcp::model::Tool::new(
                    t.name,
                    toolbox::concise_description(t.description),
                    t.parameters.as_object().expect("object schema").clone(),
                )
            })
            .collect();
        Ok(rmcp::model::ListToolsResult {
            tools,
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        if !self.groups.allows(&name) {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!("Tool {name} is disabled. Enable group '{}' in AURIS_MCP_TOOL_GROUPS and restart the server.", toolbox::tool_group(&name)))]).into());
        }
        let router = Self::tool_router();
        let known = router.has_route(&name);
        let arguments = serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        let protocol_probe = context.ct.clone();
        let cancellation = std::sync::Arc::new(toolbox::Cancellation::with_probe(move || {
            protocol_probe.is_cancelled()
        }));
        let cancellation_request = std::sync::Arc::clone(&cancellation);
        let protocol_token = context.ct.clone();
        // A dropped JoinHandle leaves its task running. That matters when rmcp drops this handler
        // future on cancellation: the blocking worker still receives the signal and stops before
        // it can cross the toolbox's commit gate.
        let cancellation_watcher = tokio::spawn(async move {
            protocol_token.cancelled().await;
            cancellation_request.cancel();
        });
        let call = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let result = REQUEST_CANCELLATION
            .scope(cancellation, router.call(call))
            .await;
        cancellation_watcher.abort();
        recover_argument_error(result, &name, known, Some(&arguments))
    }
    fn get_info(&self) -> ServerInfo {
        // Field by field because the type is `non_exhaustive`, which rules the literal out.
        // Named explicitly rather than via `Implementation::from_build_env`, whose `env!` was
        // expanded when *rmcp* was compiled — a server introducing itself as "rmcp 3.1.4".
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.instructions = Some(self.groups.instructions());
        info
    }
}

/// Invalid arguments are recoverable model mistakes; unknown tools and server errors retain
/// their protocol error codes.
fn recover_argument_error(
    result: Result<rmcp::model::CallToolResponse, ErrorData>,
    name: &str,
    known: bool,
    arguments: Option<&serde_json::Value>,
) -> Result<rmcp::model::CallToolResponse, ErrorData> {
    let guidance = || {
        let hint = arguments
            .and_then(|args| toolbox::argument_error_hint(name, args))
            .map(|hint| format!("{hint}. "))
            .unwrap_or_default();
        format!(
            "{hint}Call tool_help with name '{name}' for exact fields and examples, then correct the arguments."
        )
    };
    match result {
        Err(error) if known && error.code == rmcp::model::ErrorCode::INVALID_PARAMS => {
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Invalid arguments for {name}: {}. {}",
                error.message,
                guidance()
            ))])
            .into())
        }
        Ok(rmcp::model::CallToolResponse::Complete(mut result))
            if known
                && result.is_error == Some(true)
                && result
                    .content
                    .iter()
                    .filter_map(|content| content.as_text())
                    .any(|content| {
                        content.text.starts_with("failed to deserialize parameters")
                    }) =>
        {
            // The SDK converts Parameters failures into tool content before they reach us.
            // Preserve its detail and add the same recovery advice as protocol-level errors.
            result.content.push(ContentBlock::text(guidance()));
            Ok(result.into())
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
    finished(run_blocking(work).await?)
}

/// Runs one synchronous toolbox call with the current MCP cancellation state installed on its
/// pooled worker thread.
async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<Result<T, String>, ErrorData> {
    let cancellation = REQUEST_CANCELLATION
        .try_with(std::sync::Arc::clone)
        .unwrap_or_else(|_| std::sync::Arc::new(toolbox::Cancellation::new()));
    let observed = std::sync::Arc::clone(&cancellation);
    tokio::task::spawn_blocking(move || {
        toolbox::with_cancellation(cancellation, || {
            if observed.is_cancelled() {
                return Err("the tool request was cancelled before starting".to_string());
            }
            let outcome = work();
            if observed.is_cancelled() {
                return Err("the tool request was cancelled before making durable changes".into());
            }
            outcome
        })
    })
    .await
    .map_err(|error| ErrorData::internal_error(error.to_string(), None))
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
    if let Some(code) = auris_session::handle_plugin_discovery_worker() {
        std::process::exit(code);
    }
    // Stderr, and only stderr: stdout is the protocol channel, and one stray line on it is a
    // broken connection.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Like the CLI: nothing here reads the configuration, but this may still be the frontend
    // that runs first on a machine, and an installation predating the move to
    // `~/.config/auris-studio` only has its settings carried across by whichever one does.

    tokio::runtime::Runtime::new()?.block_on(async {
        let groups = match std::env::var("AURIS_MCP_TOOL_GROUPS") {
            Ok(v) => toolbox::ToolGroups::parse(&v)?,
            Err(std::env::VarError::NotPresent) => toolbox::ToolGroups::default(),
            Err(e) => return Err(e.into()),
        };
        let service = AurisMcp { groups }.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_catalog_is_compact_and_disabled_tools_are_not_callable() {
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp {
                groups: toolbox::ToolGroups::parse("manual").unwrap(),
            }
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let listed = client.list_tools(None).await.unwrap();
        assert!(listed.tools.iter().any(|t| t.name == "setup_tracks"));
        assert!(!listed.tools.iter().any(|t| t.name == "transcribe_audio"
            || t.name == "compose"
            || t.name == "automation"));
        let full = toolbox::tool_catalog();
        for actual in &listed.tools {
            let original = full.iter().find(|t| t.name == actual.name).unwrap();
            let mut compact = original.parameters.clone();
            toolbox::compact_parameters(&mut compact);
            assert_eq!(actual.input_schema.as_ref(), compact.as_object().unwrap());
        }
        let denied = client
            .call_tool(rmcp::model::CallToolRequestParams::new("compose"))
            .await
            .unwrap();
        assert_eq!(denied.is_error, Some(true));
        let help = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("tool_help").with_arguments(
                    serde_json::json!({"name":"replace_notes"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(help.is_error, Some(true));
        let payload: serde_json::Value =
            serde_json::from_str(&help.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(
            payload["parameters"],
            full.iter()
                .find(|t| t.name == "replace_notes")
                .unwrap()
                .parameters
        );
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn preview_returns_a_local_wav_path_without_binary_content() {
        use auris_session::prelude::{Note, Ticks};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Preview.auris");
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.save(&path).unwrap();
        drop(session);
        assert!(
            AurisMcp::default()
                .get_info()
                .capabilities
                .resources
                .is_none()
        );
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp::default()
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let result = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("preview").with_arguments(
                    serde_json::json!({"project":path,"start_bar":1,"bars":1})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert_eq!(result.content.len(), 1);
        let text = &result.content[0].as_text().unwrap().text;
        let wav = std::fs::read_dir(root.path().join(".auris-previews"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(wav.is_absolute());
        assert!(text.contains(&wav.display().to_string()), "{text}");
        assert!(text.len() < 2048);
        let bytes = std::fs::read(wav).unwrap();
        assert!(bytes.starts_with(b"RIFF"));
        assert_eq!(&bytes[8..12], b"WAVE");
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn routed_parameter_failures_include_field_and_recovery_guidance() {
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp::default()
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let result = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("add_track").with_arguments(
                    serde_json::json!({
                        "project":"unused.auris", "name":"Lead", "kind":["instrument"]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|content| content.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("arguments.kind"), "{text}");
        assert!(text.contains("string"), "{text}");
        assert!(text.contains("tool_help"), "{text}");
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_cancellation_reaches_the_blocking_worker() {
        let cancellation = std::sync::Arc::new(toolbox::Cancellation::new());
        let observed = std::sync::Arc::clone(&cancellation);
        let worker_observed = std::sync::Arc::clone(&cancellation);
        let (started, running) = std::sync::mpsc::sync_channel(0);

        let canceller = std::thread::spawn(move || {
            running.recv().expect("worker start");
            assert!(observed.cancel());
        });

        let result = REQUEST_CANCELLATION
            .scope(
                cancellation,
                run_blocking(move || {
                    started.send(()).expect("test receiver");
                    while !worker_observed.is_cancelled() {
                        std::thread::yield_now();
                    }
                    Ok::<_, String>("work completed after cancellation")
                }),
            )
            .await
            .expect("worker task");
        canceller.join().expect("canceller thread");

        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[test]
    fn bad_arguments_are_tool_feedback_but_protocol_errors_keep_their_codes() {
        let result = recover_argument_error(
            Err(ErrorData::invalid_params("missing end_bar", None)),
            "edit_clip",
            true,
            None,
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
            None,
        )
        .unwrap_err();
        assert_eq!(unknown.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        let internal = recover_argument_error(
            Err(ErrorData::internal_error("worker failed", None)),
            "edit_clip",
            true,
            None,
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
