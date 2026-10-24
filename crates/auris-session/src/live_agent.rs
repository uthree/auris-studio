//! Canonical session commands shared by saved-file and live agents.

use crate::{Session, prelude::*};
pub mod automation;
pub mod document_editing;
pub mod file_jobs;
pub mod harmony_suggestions;
mod jobs;
pub mod recipes;
pub mod vocal_editing;
pub use jobs::{AgentJob, AgentResult, AgentStep};
mod harmony;
pub mod note_editing;
pub mod performance;

/// Read or reverse one complete live agent request using the session's ordinary history.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryOperation {
    /// Inspect bounded entries and the current numeric revision.
    Read,
    /// Reverse the latest uninterrupted agent request.
    Undo {
        /// Exact available_group from history.undo.
        group: u64,
        /// Numeric revision returned by history, not the note content hash.
        expected_revision: u64,
    },
    /// Reapply the latest uninterrupted agent request.
    Redo {
        /// Exact available_group from history.redo.
        group: u64,
        /// Numeric revision returned by history.
        expected_revision: u64,
    },
}
mod vocals;
pub use vocals::{VocalJob, VocalResult, VocalStep};
fn sound_search_limit() -> usize {
    10
}

fn mixer_summary(mixer: &auris_core::project::MixerStrip) -> serde_json::Value {
    serde_json::json!({
        "gain_db":mixer.gain_db, "pan":mixer.pan,
        "mute":mixer.mute, "solo":mixer.solo,
        "effects":mixer.effects.iter().map(|slot| serde_json::json!({
            "id":slot.id, "effect_id":slot.effect_id, "enabled":slot.enabled
        })).collect::<Vec<_>>()
    })
}

/// A validated session operation; the frontend owns project selection and persistence.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Suggest up to three melody-aware chord progressions without changing notes or harmony. Pass request with melody_clips, start_bar, bars and optional key/mood/ending. Keeps a plain alternative; 7th, sus4 and diminished colour needs melodic evidence and appropriate resolution, never a complexity quota. Returns explanations, tensions, a content revision and harmony objects usable by accompaniment. Read tool_help for bounds. These are heuristic proposals; compare and audition before claiming musical improvement.
    SuggestHarmony {
        /// Explicit foreground clips and musical intent for one short phrase.
        request: harmony_suggestions::Request,
    },
    /// Generate backing parts on existing tracks, or change harmony and regenerate only explicit backing clips, while preserving protected melody clips and every other clip. Uses Auris writers, not model-authored note arrays. Read inspect_project for expected_revision. Operations run on a worker and apply atomically as one Undo; stale/cancelled work and hand-edited regeneration targets are refused. Returns compact recipe summaries, not notes. Split targets at meter/section boundaries first; read tool_help for limits.
    Accompaniment {
        /// Content revision from inspect_project or read_notes.
        expected_revision: String,
        /// 1..16 distinct melody clip IDs to protect; never inferred from names.
        #[schemars(length(min = 1, max = 16))]
        protected_clips: Vec<u64>,
        /// Generate configured backing clips or explicitly regenerate selected recipes.
        operation: crate::session::accompaniment::Operation,
    },
    /// Inspect a passage's verified instrument capabilities and phrase conditions, validate a bounded arrangement proposal on an immutable snapshot, or apply its exact plan_id as one Undo edit. Requires explicit AURIS_ARRANGEMENT_HARNESS=1. Read first; validation returns changes, warnings and a fifteen-minute process-local plan bound to this document/revision. Only an explicit breath edit may shorten notes or omit named pickups; lyrics, harmony and timeline maps remain intact. Independently read back and compare the same rendered bars; structural readiness is not musical improvement.
    Arrangement {
        /// Inspect, validate or apply. Copy stable clip IDs and native parameter addresses from inspection.
        operation: crate::arrangement::Operation,
    },
    /// Analyze Japanese lyrics into source-located phrases, readings and moras without editing. Explicit kana readings are diagnostic-only; use repair_lyrics to apply a pronunciation to existing note slots.
    AnalyzeLyrics {
        /// Original Japanese lyric, up to 65536 UTF-8 bytes.
        text: String,
        /// Optional request-local readings, addressed by one-based musical phrase.
        #[serde(default)]
        readings: Vec<crate::lyric_harness::PhraseReading>,
    },
    /// Diagnose lyrics before composition or compare explicit singer-clip phrases. Pass exactly one of spec (an .asong TOML document) or comparisons. Returns all independent complaints and the live document revision. No audio synthesis or document edits.
    DiagnoseSong {
        /// Full .asong text; shared relationships are declared with melody_from.
        spec: Option<String>,
        /// Existing one-mora-per-note singer phrases to compare, at most sixteen.
        #[serde(default)]
        comparisons: Vec<crate::lyric_harness::ClipComparison>,
    },
    /// Preview or atomically apply a local lyric repair using 1..3 candidates. Keep the same target, template and required keywords throughout the repair round. Only a passing candidate may change target lyrics/phonemes; pitches, timing, ornaments and all other notes are preserved. Stop after three alternatives and report unresolved diagnostics. Read notes first; stale revisions are rejected.
    RepairLyrics {
        /// Bounded alternatives, immutable musical template, keywords and revision.
        #[serde(flatten)]
        request: crate::lyric_harness::RepairRequest,
    },
    /// Read or undo/redo one complete user request in the live session. First use operation {"kind":"read"}; then supply its available_group and numeric revision. Human edits and open gestures are boundaries. Use this for "undo that change" instead of rewriting old notes. Does not save. Session-local; unavailable for saved-project calls.
    History {
        /// Read, undo or redo; reversal requires the exact group and revision.
        operation: HistoryOperation,
    },
    /// List/create named snapshots, or explicitly restore one as a single Undo edit. Read inspect_project and pass expected_revision for restoration; concurrent edits invalidate worker results. Keeps the current project path. Checkpoints retain asset references, not independent audio copies.
    Checkpoints {
        /// Bounded listing, new snapshot or restoration.
        operation: crate::CheckpointOperation,
        /// Current revision required for restore; omitted for list/create.
        expected_revision: Option<String>,
    },
    /// Import a local audio file as a new track at an absolute tick position. Existing music and tempo are preserved. Copy and decode run off the UI thread; cancellation or document changes discard the staged import. One Undo removes it.
    ImportAudio {
        /// Absolute local source path, up to 512 MiB; no network paths.
        path: String,
        /// Nonnegative absolute timeline position in ticks.
        at_tick: i64,
        /// Optional track and clip name, up to 256 bytes; otherwise the source stem.
        name: Option<String>,
    },
    /// Append a Standard MIDI File as new editable instrument/drum tracks. Keep the current tempo/meter maps and existing tracks. Notes, bends and controllers retain exact source ticks; playback instruments are defaults and need review. One Undo removes the whole import.
    ImportMidi {
        /// Absolute local .mid path, up to 32 MiB.
        path: String,
        /// Nonnegative absolute insertion tick, added to all source positions.
        at_tick: i64,
    },
    /// Correct singer-note lyrics, supported IPA/duration pins and pitch ornaments in one atomic edit. Read inspect_singer capabilities and read_notes first. Unsupported controls are refused; stale revisions and failed batches leave all notes unchanged. Rerender stale takes with sing.
    EditVocalNotes {
        /// Stable singer clip ID.
        clip: u64,
        /// Exact revision returned by read_notes.
        expected_revision: String,
        /// 1..256 ordered note corrections.
        #[schemars(length(min = 1, max = 256))]
        edits: Vec<vocal_editing::VocalEdit>,
    },
    /// Read or edit native-unit parameter automation through stable track/effect/send IDs. Read first; writes require the returned document revision. Static values and authored notes are preserved.
    Automation {
        /// Exact parameter address.
        target: automation::ControlTarget,
        /// Paged read, atomic set or clear.
        operation: automation::Operation,
        /// Required for writes; copied from the latest read.
        expected_revision: Option<String>,
    },
    /// Inspect, partially edit or explicitly regenerate 1..16 generated clips. Writes preserve other clips and mix state; hand-edited generated notes require explicit replacement permission. Recipe controls change stored notes; performance controls are separate. Subdivision is a grid, not note duration: eighth, sixteenth, eighth-triplet or sixteenth-triplet. Rhythm must use repeating grid steps: x hit, X accent, o ghost, dot/tilde/hyphen rest (example x...x...); empty clears it. Adjust note count and held duration with density and gate, not subdivision.
    EditRecipe {
        /// Unique stable generated clip IDs.
        #[schemars(length(min = 1, max = 16))]
        clips: Vec<u64>,
        /// Optional exact writer name within a composite drum kit.
        drum_voice: Option<String>,
        /// Read, patch controls or explicitly regenerate a take.
        operation: recipes::Operation,
    },
    /// Change one drum-role assignment for the editor and future generation. Existing notes and retained recipes stay unless remap_clips moves every generated drum voice onto the saved assignments in the same undo step; hand-placed hits stay. Read current assignments with inspect_project. Removing the last role preserves an explicitly empty map.
    SetDrumAssignment {
        /// Stable drum track ID.
        track: u64,
        /// kick, snare, closed_hat, open_hat, crash or tom.
        #[schemars(extend("enum"=["kick","snare","closed_hat","open_hat","crash","tom"]))]
        role: String,
        /// MIDI key 0..127; required unless remove is true.
        note: Option<u8>,
        /// Explicitly remove this role; omit note when true.
        #[serde(default)]
        remove: bool,
        /// Also move generated drum notes and their recipes onto the saved assignments, so a
        /// regenerated take plays the new note. Refused whole if a generated voice plays a role
        /// left unassigned. Requires note.
        #[serde(default)]
        remap_clips: bool,
    },
    /// Render an immutable snapshot to a new WAV mix or stems, then measure the encoded files. The exact requested waveform is attenuated only when needed for ceiling_db (default -1 dBTP), before integer encoding; this is not loudness normalization or limiting. Stems share one gain to preserve relative levels. Read attenuation_db, source_true_peak_db and encoded measurements; peak protection may leave loudness below a requested LUFS target. Existing outputs are never replaced. No project save or document edit; cancellation/stale documents discard private stages.
    Render {
        /// Explicit mix file or stem folder destination.
        destination: file_jobs::AudioDestination,
        /// Omit for the whole arrangement; bar ranges are one-based.
        range: Option<file_jobs::BarRange>,
        /// 16, 24 (default), or 32 float bits per sample.
        bit_depth: Option<u16>,
        /// Include effect tails; defaults to false.
        #[serde(default)]
        include_tail: bool,
        /// Maximum encoded true peak, -12..-0.1 dBTP; default -1. Only attenuation is applied.
        #[schemars(range(min = -12, max = -0.1))]
        ceiling_db: Option<f32>,
    },
    /// Create a protected 24 kHz WAV audition of 1..120 seconds from the current snapshot. Returns a playable file under the document's preview folder. Does not imply listening.
    Preview {
        /// Explicit one-based bar range within the arrangement.
        range: file_jobs::BarRange,
    },
    /// Export the current unsaved instrumental score to a new MIDI file without saving the project. Existing files are refused.
    ExportMidi {
        /// Absolute new local MIDI file path.
        output: String,
    },
    /// Measure an existing encoded WAV and return PASS/FAIL against supplied delivery limits. A failed verdict is an unfinished delivery, not a successful listening check.
    VerifyRender {
        /// Absolute local WAV path to inspect.
        path: String,
        /// Optional target loudness, -60..-6 LUFS.
        target_lufs: Option<f32>,
        /// True-peak ceiling, -12..0 dBTP; default -1.
        ceiling_db: Option<f32>,
        /// Allowed loudness error, 0..12 LU; default 1.
        tolerance_lu: Option<f32>,
        /// Optional final half-second RMS limit, -120..0 dBFS.
        ending_rms_max_db: Option<f32>,
    },
    /// Read or edit positioned tempo/meter changes without moving existing notes. Read returns native bounds and the current document revision.
    EditTimeline {
        /// Explicit read, set or removal operation with absolute ticks.
        operation: document_editing::TimelineOperation,
    },
    /// Move/copy/split/resize/remove MIDI or audio clips by stable ID. Preserves contents, recipes and performance unless explicitly frozen or baked. One atomic Undo step.
    EditClip {
        /// Stable clip ID from inspect_project.
        clip: u64,
        /// Requested operation in absolute timeline ticks.
        operation: document_editing::ClipOperation,
    },
    /// Read or edit authored pitch bend or normalized MIDI controllers, including sustain CC64. Derived performance curves are preserved. Read the lane first; mutations require its revision.
    EditClipCurve {
        /// Stable MIDI clip ID.
        clip: u64,
        /// Bend or numbered controller lane.
        curve: document_editing::CurveTarget,
        /// Paged read, merge, replacement or clear.
        operation: document_editing::CurveOperation,
        /// Required for writes, returned by the latest lane read.
        expected_revision: Option<String>,
    },
    /// Discover instrument parameter keys, native units, choices, values and automation overrides. Follow next_offset.
    InspectInstrument {
        /// Stable instrument or drum track ID.
        track: u64,
        /// Zero-based parameter offset.
        #[serde(default)]
        offset: usize,
    },
    /// Set 1..256 static instrument controls atomically in native units. Discover exact keys with inspect_instrument. Omitted controls and existing automation are preserved. Identical retries are no-ops.
    SetInstrumentParams {
        /// One or more parameter targets; duplicate targets are refused.
        #[schemars(length(min = 1, max = 256))]
        edits: Vec<InstrumentControl>,
    },
    /// Read a MIDI clip's non-destructive settings and measured note/pitch effects, including whether slide/glide actually contributes. Optional drum_voice addresses one named writer in a drum kit; omit it for the whole clip. Read completeness and warnings: stored settings alone are not verification. Notes, curves and generation recipes are unchanged. Use this before set_performance and independently read back afterward.
    InspectPerformance {
        /// Stable MIDI clip ID from inspect_project or read_notes.
        clip: u64,
        /// Exact drum writer name from the clip's notes or recipe; omit for the whole clip.
        drum_voice: Option<String>,
    },
    /// Adjust only supplied non-destructive playing controls and return measured note/pitch effects. Read inspect_performance first and independently afterward. Ratios use 0..1, swing 50..75 percent, times milliseconds and pitch semitones. slide inserts a discrete intermediate note; pitch.glide_ms creates a continuous pitch connection. Strum spreads simultaneous pitches, not single-note arpeggios. Check note_effects, pitch_effects, completeness and warnings before claiming an effective change. Omitted fields, stored notes, curves and recipes are preserved. settings.reset removes selected stages. Optional drum_voice scopes edits to one writer. More specific expression/ghost fields override humanize/brush. Edits validate atomically; identical retries are no-ops.
    SetPerformance {
        /// Stable MIDI clip ID.
        clip: u64,
        /// Exact drum writer name; omit to edit the whole clip's performance.
        drum_voice: Option<String>,
        /// Partial settings; omit every field that should stay unchanged.
        settings: Box<performance::PerformancePatch>,
    },
    /// Write the document's key, Roman-numeral chord progression and/or section label across a bounded bar range. For a new song, write its harmony and named form (intro, verse, chorus, bridge, outro as appropriate) before adding notes. Changes metadata only; existing notes and mix are preserved. One Undo reverts the edit; persistence follows the host.
    EditHarmony {
        /// First bar, 1-based.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Duration in bars, 1..4096. Charts repeat to fill this range.
        #[schemars(range(min = 1, max = 4096))]
        bars: u32,
        /// Optional key such as D major or A minor; restored to the prior key after the range.
        key: Option<String>,
        /// Roman chart, e.g. | I | vi | IV | V |; interpreted in the supplied or current key.
        chords: Option<String>,
        /// Optional section name for this range, e.g. Intro or Chorus. Replaces internal section changes and restores the previous section after the range. Supply section alone to write structure without changing harmony.
        section: Option<String>,
        /// Clear this range instead of supplying chords.
        #[serde(default)]
        clear_chords: bool,
    },
    /// Discover installed singing voices in pages of 16 without loading their models. Copy a returned voice path into set_singer_voice. Discovery is not validation; no models are downloaded.
    ListVoices {
        /// Zero-based page offset; follow next_offset.
        #[serde(default)]
        offset: usize,
    },
    /// Read one singer's selected voice, available speakers, and whether its take is missing, current or stale. Does not load a model. Call after voice selection, lyrics, and singing to verify the result.
    InspectSinger {
        /// Stable singer track ID.
        track: u64,
        /// Zero-based speaker offset; follow next_offset for voices with more than 128 speakers.
        #[serde(default)]
        offset: usize,
    },
    /// Choose a local installed singing voice and optional speaker for a singer track. Use list_voices for paths, then inspect_singer for speaker names. To change speaker, repeat the selected voice path with a speaker. Validates before editing; failure leaves the track unchanged. One transaction. Remote network/device model paths and remote VOICEVOX servers are refused; local loopback VOICEVOX is supported.
    SetSingerVoice {
        /// Stable singer track ID.
        track: u64,
        /// Absolute local voice entry path returned by list_voices or explicitly supplied by the user.
        voice: String,
        /// Exact speaker name; omit for the voice's default. Unknown names are refused with available names.
        speaker: Option<String>,
    },
    /// Write lyrics across a singer clip's notes in time order. Optional from is a zero-based storage index from read_notes; omit to start at the earliest note. Kana is split into moras; other text requires the installed Japanese dictionary. Later unfilled notes keep their words. Returns filled count and available notes; one transaction. Use read_notes to verify before sing.
    WriteLyrics {
        /// Stable singer clip ID.
        clip: u64,
        /// Nonempty phrase, e.g. さくらさいた.
        #[schemars(length(min = 1, max = 16384))]
        text: String,
        /// Zero-based storage index from read_notes; notes from this position in time order receive lyrics.
        from: Option<usize>,
    },
    /// Synthesize the selected singer's notes and lyrics into an actual playback take on a cancellable worker. Select a voice with set_singer_voice and write_lyrics first. Publishes only if the document is unchanged; one Undo restores the prior take. Does not save the project. Inspect_singer confirms the resulting take; a guide voice is not a completed vocal render.
    Sing {
        /// Stable singer track ID.
        track: u64,
        /// Optional take seed; preserves the previous seed, or uses 0 for a first take.
        seed: Option<u64>,
    },
    /// Explicitly accept an immutable recognized draft as new editable tracks, preserving existing music and timeline maps. Review extra/missing notes, timing and playback instruments; one Undo removes the complete import.
    ImportTranscription {
        /// Validated recognizer-independent draft; resolved by the tool host before authorization.
        draft: Box<crate::TranscriptionDraft>,
        /// New track/clip name for a piano draft; mixture labels retain their instrument names.
        name: String,
        /// Nonnegative absolute insertion tick.
        at_tick: i64,
    },
    /// Read or edit the live mixer DAG through stable node identities.
    MixerGraph {
        /// The validated graph operation; persistence belongs to the frontend.
        operation: crate::mixer_graph::commands::GraphAction,
    },
    /// Read the current arrangement and stable track/clip IDs. Use read_notes for note data.
    Inspect {},
    /// Read bounded scan/measurement failures after searching. Does not rescan.
    InstrumentDiagnostics {
        /// Zero-based first diagnostic.
        #[serde(default)]
        offset: usize,
        /// Page size, 1..16.
        #[serde(default = "sound_search_limit")]
        #[schemars(range(min = 1, max = 16))]
        limit: usize,
    },
    /// Inspect actual rendered audio and score without saving: mel image, levels and piano roll. Start within the arrangement and request 1..8 bars fitting in 30 seconds. Visual interpretation is not listening. Use the same range before and after editing.
    InspectAudio {
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Number of bars, 1..8; the range must fit in 30 seconds.
        #[schemars(range(min = 1, max = 8))]
        bars: u32,
        /// Optional track ID for solo routing; omit for the mix.
        track: Option<u64>,
    },
    /// Render the current unsaved mix and send actual WAV audio to the configured audio critic (AURIS_AUDIO_URL/MODEL). Returns fallible observations, exact WAV measurements, document revision and audition_id. Start inside the arrangement; 1..8 bars, at most 30 seconds. For composition, review a completed phrase, make a supported local edit, read back its state, then listen to the same bars with compare_to set to the earlier audition_id. No edit is required when the critic cannot judge. The latest four IDs are retained in this open document. No save or Undo; network permission applies. This is a separate model's review, not proof that you heard the music.
    Listen {
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Number of bars, 1..8, fitting at most thirty seconds.
        #[schemars(range(min = 1, max = 8))]
        bars: u32,
        /// Short question about audible instrumentation, balance or phrasing, up to 4000 bytes.
        focus: Option<String>,
        /// Opaque audition_id from this open document, with the same bars and timing.
        compare_to: Option<String>,
    },

    /// Search instruments by text, acoustic reference, or bounded browsing. Copy returned sound IDs into set_instrument. Paging, filters and acoustic-index status are shared across backends.
    SearchInstruments {
        /// Explicit text, similar or browse request.
        request: crate::SoundSearch,
        /// Rescan only on the first page; invalidates existing acoustic measurements.
        #[serde(default)]
        refresh: bool,
    },
    /// Edit authored notes atomically across 1..16 clips in exact clip-relative ticks (960 per quarter). Velocity is normalized 0..1, never MIDI 0..127; for example {"pitch":60,"start_tick":0,"length_ticks":960,"velocity":0.75}. Use input {"kind":"inline","edits":[...]} or {"kind":"file","source_path":"/absolute/notes.json"}, never both. Edits are typed objects, not JSON strings; expected_revision belongs at the top level. Read read_notes first and supply its revision. Patch indices address original storage order. Preserves recipes, curves, metadata on updated notes and performance. Identical replacements are no-ops.
    EditNotes {
        /// Exact content revision from the latest read_notes or inspect_project.
        expected_revision: String,
        /// One patch or replacement per clip, at most 65536 resulting notes total.
        #[schemars(length(min = 1, max = 16))]
        edits: Vec<note_editing::NoteEdit>,
    },
    /// Read 1..8 MIDI clip pages in one call. Copy each clip from inspect_project tracks[].clips[].clip or add_clip's clip result, never tracks[].track or add_track's track result. Track IDs and clip IDs are different addresses. Each request is `{clip:<returned clip ID>,offset:0}`; offset defaults to zero. Returns one shared revision and clips containing up to 128 notes each with exact clip-relative ticks, normalized velocity 0..1 and original zero-based storage indices. Follow each clip's next_offset; truncated is explicit. Do not edit between pages. Pass the top-level revision to edit_notes; stale indices are rejected.
    ReadNotes {
        /// Up to eight distinct clip pages; omitted offset starts at zero.
        #[schemars(length(min = 1, max = 8))]
        clips: Vec<note_editing::NoteRead>,
    },
    /// Compose into the current document. Replacing existing tracks requires replace=true.
    Compose {
        /// Inline .asong TOML, mutually exclusive with preset.
        spec: Option<String>,
        /// A name returned by list_presets, mutually exclusive with spec.
        preset: Option<String>,
        /// Explicit permission to replace a nonempty arrangement.
        #[serde(default)]
        replace: bool,
    },
    /// Add one empty track and return its numeric track ID. Pass kind as one string, for example {"name":"Drums","kind":"drum"}. Instrument and drum tracks start with a default sound; then use set_instrument with an ID from search_instruments. Use add_clip followed by edit_notes to write music. The host owns persistence.
    AddTrack {
        /// The exact desired name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
        /// One string: instrument, drum, singer, audio or bus. Never an array or instrument ID.
        kind: TrackKind,
    },
    /// Rename a track using its stable numeric ID from inspect_project or add_track.
    RenameTrack {
        /// Stable track ID.
        track: u64,
        /// New name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
    },
    /// Remove one track and its clips.
    RemoveTrack {
        /// Stable track ID.
        track: u64,
    },
    /// Replace the sound of an instrument or drum track using an exact id from search_instruments. Keeps notes, clips, mixer and effects. Remeasure with inspect_audio and normalize_mix because the new sound can be much louder or quieter. Replacing the instrument clears its old parameter automation. SoundFonts must already be loaded; search IDs expire on refresh, library changes, bounded handle eviction, or restart.
    SetInstrument {
        /// Stable track ID.
        track: u64,
        /// Exact id returned by search_instruments.
        #[serde(rename = "sound_id")]
        instrument: String,
    },

    /// Measure the whole mix, move every source fader by the same amount toward target_lufs under a true-peak ceiling, and measure again. Keeps relative balance and master gain; refuses automated source gain. The measured result may fall short at fader or peak limits. Persistence follows the host.
    NormalizeMix {
        /// Desired integrated loudness, -60 to -6 LUFS.
        #[schemars(range(min = -60, max = -6))]
        target_lufs: f32,
        /// Maximum reconstructed peak, -12 to 0 dBTP; defaults to -1.
        #[schemars(range(min = -12, max = 0))]
        ceiling_db: Option<f32>,
    },

    /// Add an empty MIDI clip to an instrument, drum or singer track and return its numeric clip ID. start_bar is 1-based in the project's meter; bars is a duration. Audio and bus tracks cannot hold MIDI clips.
    AddClip {
        /// Stable track ID.
        track: u64,
        /// Exact clip name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Duration in bars, 1 through 1024.
        #[schemars(range(min = 1, max = 1024))]
        bars: u32,
    },
    /// Generate a new MIDI phrase on an existing instrument or drum track using the project's key, chords and meter. Write melodic harmony with edit_harmony first; drum parts use the track's sound assignments. part is lead, chords, pad, arp, bass, drums, kick, snare or hat. Returns clip ID, note count and seed; zero notes is an empty clip, not a finished phrase. Existing clips are preserved. One Undo removes the new clip; persistence follows the host.
    GeneratePhrase {
        /// Stable instrument or drum track ID from inspect_project or add_track.
        track: u64,
        /// Melodic parts require an instrument track; drum parts require a drum track.
        #[schemars(extend("enum" = ["lead", "chords", "pad", "arp", "bass", "drums", "kick", "snare", "hat"]))]
        part: String,
        /// First bar, starting at 1, in the project's meter.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Duration in bars, 1 through 1024.
        #[schemars(range(min = 1, max = 1024))]
        bars: u32,
        /// Deterministic take seed; defaults to zero. A different seed selects a different take.
        #[serde(default)]
        seed: u64,
    },

    /// Set and enable a playback loop using a 1-based start bar and duration. Undo restores both the region and its enabled state.
    SetLoop {
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Loop duration in bars, 1 through 1024.
        #[schemars(range(min = 1, max = 1024))]
        bars: u32,
    },
}

impl Command {
    /// Whether this operation can change the document; file output alone does not change it.
    pub fn is_mutating(&self) -> bool {
        match self {
            Self::Arrangement { operation } => operation.is_mutating(),
            Self::RepairLyrics { request } => request.apply,
            Self::History { operation } => !matches!(operation, HistoryOperation::Read),
            Self::Checkpoints { operation, .. } => operation.is_mutating(),
            Self::Automation { operation, .. } => !operation.is_read_only(),
            Self::EditRecipe { operation, .. } => !operation.is_read_only(),
            Self::MixerGraph { operation } => !operation.is_read_only(),
            Self::EditClipCurve { operation, .. } => !operation.is_read_only(),
            Self::EditTimeline { operation } => !operation.is_read_only(),
            Self::Inspect {}
            | Self::SuggestHarmony { .. }
            | Self::AnalyzeLyrics { .. }
            | Self::DiagnoseSong { .. }
            | Self::InspectInstrument { .. }
            | Self::InspectPerformance { .. }
            | Self::ListVoices { .. }
            | Self::InspectSinger { .. }
            | Self::InspectAudio { .. }
            | Self::Listen { .. }
            | Self::ReadNotes { .. }
            | Self::SearchInstruments { .. }
            | Self::InstrumentDiagnostics { .. }
            | Self::Render { .. }
            | Self::Preview { .. }
            | Self::ExportMidi { .. }
            | Self::VerifyRender { .. } => false,
            _ => true,
        }
    }
    /// Whether execution requires detached work and an owner-side continuation.
    pub fn needs_worker(&self) -> bool {
        matches!(
            self,
            Self::Arrangement {
                operation: crate::arrangement::Operation::Validate { .. }
            } | Self::Accompaniment { .. }
                | Self::ListVoices { .. }
                | Self::SetSingerVoice { .. }
                | Self::Sing { .. }
                | Self::Render { .. }
                | Self::Preview { .. }
                | Self::ExportMidi { .. }
                | Self::VerifyRender { .. }
                | Self::ImportAudio { .. }
                | Self::ImportMidi { .. }
                | Self::Checkpoints { .. }
        )
    }
}

/// Types of empty tracks the agent can add.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// A melodic instrument.
    Instrument,
    /// A percussion instrument.
    Drum,
    /// A singing track.
    Singer,
    /// An audio track.
    Audio,
    /// A mixer bus.
    Bus,
}

/// One static instrument parameter edit in native descriptor units.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstrumentControl {
    /// Stable instrument or drum track ID.
    pub track: u64,
    /// Exact parameter key returned by inspect_instrument.
    pub param: String,
    /// Value in native descriptor units.
    pub value: f32,
}

impl Session {
    /// Execute a file-free command against this session, preserving ordinary undo semantics.
    pub fn agent_command(&mut self, command: Command) -> Result<String, String> {
        self.agent_command_with_plugin_paths(command, &[])
    }

    /// Run a live command using the frontend's additional plugin search folders.
    pub fn agent_command_with_plugin_paths(
        &mut self,
        command: Command,
        plugin_paths: &[std::path::PathBuf],
    ) -> Result<String, String> {
        self.with_agent_history(|session| session.execute_agent_command(command, plugin_paths))
    }

    fn execute_agent_command(
        &mut self,
        command: Command,
        plugin_paths: &[std::path::PathBuf],
    ) -> Result<String, String> {
        let error = |error: crate::SessionError| error.to_string();
        if matches!(&command, Command::Arrangement { .. }) && !crate::arrangement::enabled() {
            return Err(
                "Arrangement harness is disabled; opt in with AURIS_ARRANGEMENT_HARNESS=1".into(),
            );
        }
        match command {
            Command::SuggestHarmony { request } => self.agent_suggest_harmony(request),
            Command::Arrangement {
                operation:
                    operation @ (crate::arrangement::Operation::Inspect { .. }
                    | crate::arrangement::Operation::InspectBreathing { .. }
                    | crate::arrangement::Operation::Apply { .. }),
            } => self.agent_arrangement(operation),
            Command::AnalyzeLyrics { text, readings } => {
                serde_json::to_string(&self.analyze_lyrics(&text, &readings)?)
                    .map_err(|e| e.to_string())
            }
            Command::DiagnoseSong { spec, comparisons } => {
                if spec.is_some() == !comparisons.is_empty() {
                    return Err("Pass exactly one of spec or nonempty comparisons".into());
                }
                let report = if let Some(source) = spec {
                    if source.len() > 262144 {
                        return Err("Song specifications are limited to 262144 bytes".into());
                    }
                    let spec = SongSpec::parse(&source).map_err(|e| format!("{e:?}"))?;
                    self.diagnose_song_lyrics(&spec)?
                } else {
                    self.diagnose_clip_lyrics(&comparisons)?
                };
                let mut value = serde_json::to_value(report).map_err(|e| e.to_string())?;
                value["revision"] = self.agent_document_revision()?.into();
                Ok(value.to_string())
            }
            Command::RepairLyrics { request } => {
                self.repair_lyrics(&request).map(|v| v.to_string())
            }
            Command::History { operation } => self.agent_history(operation),
            Command::EditVocalNotes {
                clip,
                expected_revision,
                edits,
            } => self.agent_edit_vocal_notes(clip, expected_revision, edits),
            Command::Automation {
                target,
                operation,
                expected_revision,
            } => self.agent_automation(target, operation, expected_revision),
            Command::EditRecipe {
                clips,
                drum_voice,
                operation,
            } => self.agent_edit_recipe(clips, drum_voice, operation),
            Command::SetDrumAssignment {
                track,
                role,
                note,
                remove,
                remap_clips,
            } => {
                if note.is_some() == remove || note.is_some_and(|note| note > 127) {
                    return Err("Supply note within 0..127, or remove:true without note".into());
                }
                if remove && remap_clips {
                    return Err(
                        "remap_clips needs a note to move generated hits to; remove does not take it"
                            .into(),
                    );
                }
                let role = serde_json::from_value::<auris_core::project::DrumRole>(role.into())
                    .map_err(|_| "Unknown drum role")?;
                let track = TrackId(track);
                let changed = match note {
                    Some(note) if remap_clips => {
                        self.set_drum_assignment_remapping(track, role, note)
                    }
                    note => self.set_drum_assignment(track, role, note),
                }
                .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({"track":track.0,"changed":changed,"assignments":self.drum_assignments(track)}).to_string())
            }
            Command::EditTimeline { operation } => self.agent_edit_timeline(operation),
            Command::EditClip { clip, operation } => self.agent_edit_clip(clip, operation),
            Command::EditClipCurve {
                clip,
                curve,
                operation,
                expected_revision,
            } => self.agent_clip_curve(clip, curve, operation, expected_revision),
            Command::InspectInstrument { track, offset } => {
                let track = TrackId(track);
                if self
                    .project()
                    .track(track)
                    .and_then(|t| t.kind.as_instrument())
                    .is_none()
                {
                    return Err("Expected instrument or drum track".into());
                }
                let descriptors = self.instrument_descriptors(track).to_vec();
                if offset > descriptors.len() {
                    return Err("Offset exceeds parameter count".into());
                }
                let parameters: Vec<_> = descriptors.iter().skip(offset).take(32).map(|descriptor| {
                    let target = crate::ParamTarget::Instrument { track, param: descriptor.id };
                    serde_json::json!({"key":descriptor.key,"name":descriptor.name,"value":self.param_value(target,descriptor),"min":descriptor.min,"max":descriptor.max,"unit":descriptor.unit,"choices":descriptor.choices.iter().take(16).collect::<Vec<_>>(),"choice_count":descriptor.choices.len(),"automated":self.is_automated(target)})
                }).collect();
                let next = offset + parameters.len();
                Ok(serde_json::json!({"track":track.0,"parameters":parameters,"total":descriptors.len(),"next_offset":(next < descriptors.len()).then_some(next)}).to_string())
            }
            Command::SetInstrumentParams { edits } => {
                let edits: Vec<_> = edits
                    .into_iter()
                    .map(|edit| crate::InstrumentParamEdit {
                        track: TrackId(edit.track),
                        param: edit.param,
                        value: edit.value,
                    })
                    .collect();
                serde_json::to_string(&self.set_instrument_params(&edits)?)
                    .map_err(|e| e.to_string())
            }
            Command::InspectPerformance { clip, drum_voice } => {
                self.agent_inspect_performance(ClipId(clip), drum_voice.as_deref())
            }
            Command::SetPerformance {
                clip,
                drum_voice,
                settings,
            } => self.agent_set_performance(ClipId(clip), drum_voice.as_deref(), *settings),
            Command::EditHarmony {
                start_bar,
                bars,
                key,
                chords,
                section,
                clear_chords,
            } => self.agent_edit_harmony(
                start_bar,
                bars,
                key.as_deref(),
                chords.as_deref(),
                clear_chords,
                section.as_deref(),
            ),
            Command::WriteLyrics { clip, text, from } => {
                if text.trim().is_empty() || text.len() > 16384 {
                    return Err("text must contain lyrics within 16384 UTF-8 bytes".into());
                }
                let (track, target) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                if !self
                    .project()
                    .track(track)
                    .is_some_and(|t| t.kind.is_singer())
                {
                    return Err("write_lyrics requires a singer clip".into());
                }
                if target.notes.is_empty() || from.is_some_and(|from| from >= target.notes.len()) {
                    return Err(
                        "from must be a current zero-based note index from read_notes".into(),
                    );
                }
                let mut indices: Vec<_> = (0..target.notes.len()).collect();
                indices.sort_by_key(|i| (target.notes[*i].start, *i));
                let first = from
                    .map(|from| {
                        indices
                            .iter()
                            .position(|i| *i == from)
                            .ok_or("Unknown note index")
                    })
                    .transpose()?
                    .unwrap_or(0);
                let indices = &indices[first..];
                let filled = self
                    .write_lyrics(ClipId(clip), indices, &text)
                    .map_err(error)?;
                Ok(
                    serde_json::json!({"filled":filled,"available":indices.len(),"unsaved":true})
                        .to_string(),
                )
            }
            Command::InspectSinger { track, offset } => {
                self.agent_inspect_singer(TrackId(track), offset)
            }
            command @ (Command::Accompaniment { .. }
            | Command::ListVoices { .. }
            | Command::SetSingerVoice { .. }
            | Command::Sing { .. }
            | Command::Render { .. }
            | Command::Preview { .. }
            | Command::ExportMidi { .. }
            | Command::VerifyRender { .. }
            | Command::ImportAudio { .. }
            | Command::ImportMidi { .. }
            | Command::Checkpoints { .. }
            | Command::Arrangement {
                operation: crate::arrangement::Operation::Validate { .. },
            }) => {
                let cancelled = std::sync::atomic::AtomicBool::new(false);
                let mut job = self.begin_agent_job(command, crate::Settings::load())?;
                loop {
                    match self.continue_agent_job(job.run(&cancelled)?, &cancelled)? {
                        AgentStep::Work(next) => job = next,
                        AgentStep::Done(text) => return Ok(text),
                    }
                }
            }
            Command::ImportTranscription {
                draft,
                name,
                at_tick,
            } => self.agent_import_draft(*draft, name, at_tick),
            Command::MixerGraph { operation } => self
                .apply_mixer_graph(&operation)
                .map(|report| report.to_string()),
            Command::InstrumentDiagnostics { offset, limit } => self
                .sound_library_job(plugin_paths)
                .diagnostics(offset, limit),
            Command::SearchInstruments { request, refresh } => {
                self.sound_library_job(plugin_paths).run(request, refresh)
            }
            Command::EditNotes {
                expected_revision,
                edits,
            } => self.agent_edit_notes(&expected_revision, edits),
            Command::Inspect {} => {
                let project = self.project();
                let tracks: Vec<_> = project
                    .tracks
                    .iter()
                    .map(|track| {
                        let clips: Vec<_> = if let Some(clips)=track.kind.note_clips() {
                            clips.iter().map(|clip|serde_json::json!({"clip":clip.id.0,"name":clip.name,"start_tick":clip.start.raw(),"length_ticks":clip.length.raw(),"note_count":clip.notes.len()})).collect()
                        } else if let Some(audio)=track.kind.as_audio() {
                            audio.clips.iter().map(|clip|serde_json::json!({"clip":clip.id.0,"name":clip.name,"start_tick":clip.start.raw(),"length_ticks":project.audio_clip_length_ticks(clip).raw(),"gain_db":clip.gain_db,"source_bpm":clip.source_bpm,"follows_tempo":clip.follows_tempo,"fade_in_frames":clip.fade_in_frames,"fade_out_frames":clip.fade_out_frames,"source_frames":clip.length_frames,"source_sample_rate":project.audio_sources.get(&clip.source).map(|source|source.sample_rate),"fade_in_curve":clip.fade_in_curve,"fade_out_curve":clip.fade_out_curve})).collect()
                        } else { Vec::new() };
                        serde_json::json!({"track":track.id.0, "name":track.name,
                        "kind":track.kind.label(), "mixer":mixer_summary(&track.mixer), "clips":clips,"drum_assignments":self.drum_assignments(track.id),
                        "instrument":track.kind.as_instrument().map(|inner| &inner.instrument_id),
                        "soundfont_preset":self.track_preset(track.id)})
                    })
                    .collect();
                Ok(serde_json::json!({"title":project.name, "tracks":tracks, "revision":self.agent_document_revision()?,
                    "sample_rate":project.sample_rate,"duration_seconds":project.duration_seconds(), "harmony":project.harmony,
                    "sections":project.sections, "tempo_map":project.tempo_map, "signatures":project.signatures, "loop_region":project.loop_region, "loop_enabled":project.loop_enabled, "ticks_per_quarter":Ticks::QUARTER.raw()})
                .to_string())
            }
            Command::InspectAudio {
                start_bar,
                bars,
                track,
            } => {
                let report = self
                    .audio_inspection_job(start_bar, bars, track)?
                    .run(&std::sync::atomic::AtomicBool::new(false))?;
                serde_json::to_string(&report).map_err(|e| e.to_string())
            }
            Command::Listen {
                start_bar,
                bars,
                focus,
                compare_to,
            } => {
                let job =
                    self.begin_listening(start_bar, bars, focus.as_deref(), compare_to.as_deref())?;
                let result = job.run(&std::sync::atomic::AtomicBool::new(false))?;
                self.continue_listening(result)
            }
            Command::ReadNotes { clips } => self.agent_read_notes(&clips),
            Command::Compose {
                spec,
                preset: name,
                replace,
            } => {
                if !replace && !self.project().tracks.is_empty() {
                    return Err("The arrangement is not empty. Edit its tracks, or pass replace=true only when the user requests a replacement.".into());
                }
                let source = match (spec, name) {
                    (Some(spec), None) => spec,
                    (None, Some(name)) => preset(&name)
                        .ok_or("Unknown preset; use list_presets")?
                        .source
                        .to_string(),
                    _ => return Err("Pass exactly one of spec or preset".into()),
                };
                let spec = SongSpec::parse(&source).map_err(|e| format!("{e:?}"))?;
                let piece = compose(&spec);
                let report = self.compose_without_balance(&piece).map_err(error)?;
                Ok(format!(
                    "Composed {} tracks and {} notes in the current document. Changes are unsaved.",
                    report.tracks, report.notes
                ))
            }
            Command::AddTrack { name, kind } => {
                if name.trim().is_empty() {
                    return Err("A track needs a name".into());
                }
                let id = match kind {
                    TrackKind::Instrument => {
                        self.add_default_instrument_track(name).map_err(error)?
                    }
                    TrackKind::Drum => self.add_default_drum_track(name).map_err(error)?,
                    TrackKind::Singer => self.add_singer_track(name),
                    TrackKind::Audio => self.add_audio_track(name),
                    TrackKind::Bus => self.add_bus_track(name),
                };
                Ok(serde_json::json!({"track":id.0,"unsaved":true}).to_string())
            }
            Command::RenameTrack { track, name } => {
                if name.trim().is_empty() {
                    return Err("A track needs a name".into());
                }
                self.rename_track(TrackId(track), name).map_err(error)?;
                Ok("Renamed track".into())
            }
            Command::RemoveTrack { track } => {
                self.remove_track(TrackId(track)).map_err(error)?;
                Ok("Removed track".into())
            }
            Command::SetInstrument { track, instrument } => {
                self.use_library_sound(TrackId(track), &instrument, plugin_paths)?;
                Ok("Changed instrument. Mixer settings stayed the same; inspect_audio again because the new sound may be much louder or quieter.".into())
            }
            Command::NormalizeMix {
                target_lufs,
                ceiling_db,
            } => {
                let ceiling_db = ceiling_db.unwrap_or(-1.0);
                let report = self.normalize_mix(target_lufs, ceiling_db).map_err(error)?;
                Ok(format!(
                    "Moved source faders {:+.2} dB. Mix {:.1} -> {:.1} LUFS; true peak {:.1} -> {:.1} dBTP. Target {:.1} LUFS, ceiling {:.1} dBTP. Inspect_audio again before judging the sound.",
                    report.offset_db,
                    report.before.lufs.unwrap_or(f32::NEG_INFINITY),
                    report.after.lufs.unwrap_or(f32::NEG_INFINITY),
                    report.before.true_peak_db,
                    report.after.true_peak_db,
                    target_lufs,
                    ceiling_db
                ))
            }
            Command::AddClip {
                track,
                name,
                start_bar,
                bars,
            } => {
                if name.trim().is_empty() || start_bar == 0 || !(1..=1024).contains(&bars) {
                    return Err("Use a name, start_bar >= 1 and bars 1..1024".into());
                }
                let end = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
                let start = self.project().signatures.bar_start(start_bar);
                let length = self.project().signatures.bar_start(end) - start;
                let id = self
                    .add_midi_clip(TrackId(track), name, start, length)
                    .map_err(error)?;
                Ok(serde_json::json!({"track":track,"clip":id.0,"start_tick":start.raw(),"length_ticks":length.raw(),"unsaved":true}).to_string())
            }
            Command::GeneratePhrase {
                track,
                part,
                start_bar,
                bars,
                seed,
            } => {
                if start_bar == 0 || !(1..=1024).contains(&bars) {
                    return Err("Use start_bar >= 1 and bars 1..1024".into());
                }
                let preset = ClipPreset::parse(&part).ok_or(
                    "Unknown part; use lead, chords, pad, arp, bass, drums, kick, snare or hat",
                )?;
                let after = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
                let start = self.project().signatures.bar_start(start_bar);
                let end = self.project().signatures.bar_start(after);
                if !preset.is_drums()
                    && self
                        .project()
                        .harmony
                        .events_in(start, end, &self.project().signatures)
                        .is_empty()
                {
                    return Err("No chords in this range; use edit_harmony before generating a melodic phrase".into());
                }
                let clip = self
                    .generate_clip(
                        TrackId(track),
                        start,
                        end - start,
                        ClipRecipe::new(preset, seed),
                    )
                    .map_err(error)?;
                let notes = self
                    .project()
                    .midi_clip(clip)
                    .map_or(0, |(_, clip)| clip.notes.len());
                Ok(
                    serde_json::json!({"clip":clip.0,"track":track,"part":preset.name(),
                    "start_bar":start_bar,"bars":bars,"seed":seed,"notes":notes,
                    "unsaved":true})
                    .to_string(),
                )
            }
            Command::SetLoop { start_bar, bars } => {
                if start_bar == 0 || !(1..=1024).contains(&bars) {
                    return Err("Use start_bar >= 1 and bars 1..1024".into());
                }
                let end_bar = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
                let start = self.project().signatures.bar_start(start_bar);
                let end = self.project().signatures.bar_start(end_bar);
                self.set_agent_loop_region(start, end);
                Ok("Set playback loop".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap()
    }

    #[test]
    fn generated_phrases_keep_their_seed_meter_and_existing_music_and_undo_once() {
        for preset in ClipPreset::ALL {
            let mut session = session();
            session.set_signature_at(Ticks::ZERO, TimeSignature::new(3, 4));
            session
                .agent_edit_harmony(
                    1,
                    8,
                    Some("D major"),
                    Some("| I | IV | V | I |"),
                    false,
                    None,
                )
                .unwrap();
            let track = if preset.is_drums() {
                session.add_default_drum_track("Beat").unwrap()
            } else {
                session.add_default_instrument_track("Lead").unwrap()
            };
            let existing = session
                .add_midi_clip(track, "Keep", Ticks::ZERO, Ticks::QUARTER * 3)
                .unwrap();
            session
                .add_note(existing, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            let before = session.project().clone();
            let command = || Command::GeneratePhrase {
                track: track.0,
                part: preset.name().into(),
                start_bar: 3,
                bars: 2,
                seed: 42,
            };
            let result: serde_json::Value =
                serde_json::from_str(&session.agent_command(command()).unwrap()).unwrap();
            let clip = session
                .project()
                .midi_clip(ClipId(result["clip"].as_u64().unwrap()))
                .unwrap()
                .1;
            assert_eq!(clip.start, Ticks::QUARTER * 6);
            assert_eq!(clip.length, Ticks::QUARTER * 6);
            assert!(!clip.notes.is_empty(), "{preset:?}");
            assert_eq!(result["notes"].as_u64().unwrap(), clip.notes.len() as u64);
            assert_eq!(clip.recipe.as_ref().unwrap().preset, preset);
            assert_eq!(clip.recipe.as_ref().unwrap().seed, 42);
            assert!(
                clip.notes
                    .iter()
                    .all(|n| n.start >= Ticks::ZERO && n.start + n.length <= clip.length)
            );
            let generated = clip.notes.clone();
            assert_eq!(
                session.project().midi_clip(existing),
                before.midi_clip(existing)
            );
            assert!(session.path().is_none());
            session.undo();
            assert_eq!(session.project(), &before);
            let result: serde_json::Value =
                serde_json::from_str(&session.agent_command(command()).unwrap()).unwrap();
            assert_eq!(
                session
                    .project()
                    .midi_clip(ClipId(result["clip"].as_u64().unwrap()))
                    .unwrap()
                    .1
                    .notes,
                generated
            );
        }
    }

    #[test]
    fn invalid_phrase_requests_preserve_the_document_and_undo_history() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let before = session.project().clone();
        for (part, start_bar, bars) in [
            ("lead", 1, 4),
            ("unknown", 1, 4),
            ("lead", 0, 4),
            ("lead", 1, 0),
            ("lead", 1, 1025),
            ("lead", u32::MAX, 1),
        ] {
            assert!(
                session
                    .agent_command(Command::GeneratePhrase {
                        track: track.0,
                        part: part.into(),
                        start_bar,
                        bars,
                        seed: 0
                    })
                    .is_err()
            );
            assert_eq!(session.project(), &before);
        }
        session.undo();
        assert!(session.project().tracks.is_empty());
    }

    #[test]
    fn drum_phrase_generation_needs_no_harmony_and_defaults_its_seed() {
        let mut session = session();
        let track = session.add_default_drum_track("Beat").unwrap();
        let command = serde_json::from_value(serde_json::json!({"action":"generate_phrase",
            "track":track.0,"part":"drums","start_bar":1,"bars":2}))
        .unwrap();
        let result: serde_json::Value =
            serde_json::from_str(&session.agent_command(command).unwrap()).unwrap();
        assert_eq!(result["seed"], 0);
        assert!(result["notes"].as_u64().unwrap() > 0);
        assert!(session.project().harmony.is_empty());
    }

    #[test]
    fn phrase_generation_rejects_wrong_track_kinds_before_recording_an_edit() {
        let mut session = session();
        session
            .agent_edit_harmony(1, 4, None, Some("| I | V |"), false, None)
            .unwrap();
        let melodic = session.add_default_instrument_track("Lead").unwrap();
        let drum = session.add_default_drum_track("Beat").unwrap();
        let singer = session.add_singer_track("Vocal");
        let before = session.project().clone();
        for (track, part) in [
            (melodic, "drums"),
            (drum, "lead"),
            (singer, "lead"),
            (TrackId(u64::MAX), "lead"),
        ] {
            assert!(
                session
                    .agent_command(Command::GeneratePhrase {
                        track: track.0,
                        part: part.into(),
                        start_bar: 1,
                        bars: 4,
                        seed: 0
                    })
                    .is_err()
            );
            assert_eq!(session.project(), &before);
        }
        session.undo();
        assert!(session.project().track(singer).is_none());
    }

    #[test]
    fn inspection_size_does_not_depend_on_hosted_effect_state() {
        let mut mixer = MixerStrip::default();
        mixer.effects.push(auris_core::project::EffectSlot::new(
            EffectSlotId(1),
            "test.fx",
        ));
        let before = mixer_summary(&mixer);
        mixer.effects[0]
            .state
            .set_hosted_bytes(&vec![42; 1_000_000]);
        mixer.effects[0].state.params.insert("hidden".into(), 0.5);
        assert_eq!(mixer_summary(&mixer), before);
        assert!(before.to_string().len() < 256);
    }

    #[test]
    fn direct_replacement_rejects_invalid_notes_without_recording_history() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let note = Note::new(60, Ticks::ZERO, Ticks::QUARTER);
        session.replace_notes(clip, vec![note.clone()]).unwrap();
        let before = session.project().clone();
        for bad in [
            Note {
                pitch: 128,
                ..note.clone()
            },
            Note {
                velocity: f32::NAN,
                ..note.clone()
            },
            Note {
                start: Ticks(-1),
                ..note.clone()
            },
            Note {
                length: Ticks::ZERO,
                ..note.clone()
            },
            Note {
                start: Ticks(i64::MAX),
                ..note.clone()
            },
        ] {
            assert!(
                session
                    .replace_notes(clip, vec![note.clone(), bad])
                    .unwrap_err()
                    .to_string()
                    .contains("notes[1]")
            );
            assert_eq!(session.project(), &before);
        }
        session.undo();
        assert!(
            session
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
    }

    #[test]
    fn musical_controls_validate_before_editing() {
        let mut session = session();
        let before = session.project().clone();
        for command in [
            Command::EditTimeline {
                operation: document_editing::TimelineOperation::SetTempo {
                    at_tick: 0,
                    bpm: f64::NAN,
                },
            },
            Command::SetLoop {
                start_bar: 0,
                bars: 8,
            },
            Command::SetLoop {
                start_bar: u32::MAX,
                bars: 8,
            },
        ] {
            assert!(session.agent_command(command).is_err());
            assert_eq!(session.project(), &before);
        }
        session
            .agent_command(Command::EditTimeline {
                operation: document_editing::TimelineOperation::SetTempo {
                    at_tick: 0,
                    bpm: 144.0,
                },
            })
            .unwrap();
        session.undo();
        assert_eq!(session.project(), &before);
        session
            .agent_command(Command::SetLoop {
                start_bar: 3,
                bars: 4,
            })
            .unwrap();
        assert!(session.project().loop_enabled);
        assert_eq!(
            session.project().loop_region,
            Some((Ticks::from_beats(8.0), Ticks::from_beats(24.0)))
        );
    }

    #[test]
    fn agent_loop_undo_restores_the_switch_without_changing_earlier_history() {
        let mut session = session();
        let empty = session.project().clone();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let before = session.project().clone();
        session.begin_agent_request();
        session
            .agent_command(Command::RenameTrack {
                track: track.0,
                name: "Renamed".into(),
            })
            .unwrap();
        session
            .agent_command(Command::SetLoop {
                start_bar: 3,
                bars: 4,
            })
            .unwrap();
        session.end_agent_request();
        let after = session.project().clone();
        assert!(after.loop_enabled);
        assert!(session.undo().is_some());
        assert_eq!(session.project(), &before);
        assert!(session.redo().is_some());
        assert_eq!(session.project(), &after);
        session.undo();
        session.undo();
        assert_eq!(session.project(), &empty);
    }

    #[test]
    fn composition_changes_the_open_document_without_creating_a_file() {
        let mut session = session();
        let before = session.project().clone();
        let name = PRESETS.first().unwrap().name;
        session
            .agent_command(Command::Compose {
                spec: None,
                preset: Some(name.into()),
                replace: false,
            })
            .unwrap();
        assert!(!session.project().tracks.is_empty());
        assert!(session.project().tracks.iter().any(|track| {
            track
                .kind
                .note_clips()
                .is_some_and(|clips| clips.iter().any(|clip| !clip.notes.is_empty()))
        }));
        assert!(session.path().is_none());
        assert!(session.is_dirty());
        session.undo();
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn composing_does_not_silently_replace_existing_tracks() {
        let mut session = session();
        session.add_audio_track("Existing");
        let before = session.project().clone();
        assert!(
            session
                .agent_command(Command::Compose {
                    spec: None,
                    preset: Some("anything".into()),
                    replace: false,
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn file_commands_and_destinations_are_rejected() {
        for value in [
            serde_json::json!({"action":"create_project", "output":"song.auris"}),
            serde_json::json!({"action":"render", "output":"song.wav"}),
            serde_json::json!({"action":"compose", "preset":"game-loop", "output":"song.auris"}),
            serde_json::json!({"action":"inspect", "project":"other.auris"}),
        ] {
            assert!(serde_json::from_value::<Command>(value).is_err());
        }
    }

    #[test]
    fn saved_document_edits_leave_the_file_unchanged() {
        let mut session = session();
        let folder = tempfile::tempdir().unwrap();
        let path = session
            .save_as(&folder.path().join("Song.auris"))
            .unwrap()
            .document;
        let before = std::fs::read(&path).unwrap();
        session
            .agent_command(Command::AddTrack {
                name: "Unsaved".into(),
                kind: TrackKind::Audio,
            })
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(session.path(), Some(path.as_path()));
        assert!(session.is_dirty());
    }

    #[test]
    fn a_drum_assignment_command_with_remap_clips_moves_the_generated_hits() {
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                Ticks::QUARTER * 4,
                ClipRecipe::new(ClipPreset::Snare, 7),
            )
            .unwrap();
        let assign = |remap_clips: bool, note: Option<u8>| {
            serde_json::from_value::<Command>(serde_json::json!({
                "action": "set_drum_assignment",
                "track": track.0,
                "role": "snare",
                "note": note,
                "remove": note.is_none(),
                "remap_clips": remap_clips,
            }))
            .unwrap()
        };

        session.agent_command(assign(true, Some(84))).unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert!(!notes.is_empty());
        assert!(notes.iter().all(|note| note.pitch == 84), "{notes:?}");

        // Without it the assignment changes and the clip does not, as before.
        session.agent_command(assign(false, Some(40))).unwrap();
        assert!(
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .all(|note| note.pitch == 84)
        );

        let refused = session.agent_command(assign(true, None)).unwrap_err();
        assert!(refused.starts_with("remap_clips needs"), "{refused}");
    }
}
