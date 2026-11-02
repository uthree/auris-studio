//! Canonical session commands shared by saved-file and live agents.

use crate::{Session, prelude::*};
pub mod automation;
pub mod document_editing;
pub mod file_jobs;
pub mod harmony_suggestions;
mod jobs;
pub mod layers;
pub mod recipes;
pub mod vocal_editing;
pub use jobs::{AgentJob, AgentResult, AgentStep};
mod harmony;
pub mod note_editing;
pub mod performance;
pub mod phrase_variation;

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

/// Adds `loop_end_tick` to a clip summary when the clip repeats past its own end.
///
/// Left off an unlooped clip so the common case costs a model nothing to read; present, it is the
/// absolute tick where the repeats stop, which is the value `edit_clip`'s `loop` takes back.
fn with_loop_end(
    mut summary: serde_json::Value,
    start: Ticks,
    content: Ticks,
    sounding: Ticks,
) -> serde_json::Value {
    if sounding > content {
        summary["loop_end_tick"] = (start + sounding).raw().into();
    }
    summary
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
    /// Offer an optional local Issue draft after verifying a missing feature or suspected defect. Check tool_help/capabilities first; distinguish application limitations from missing agent tools, configuration, denied permissions and transient errors. Explain workarounds and uncertainty. Never quote or summarize private prompts, logs or learned preferences in this suggestion. The desktop shows an invitation; only the human can open it and authorize each private context category. Does not publish, send context to AI, or grant consent.
    SuggestIssueReport {
        /// Generic title and evidence-based limitation, without private conversation excerpts.
        suggestion: crate::issue_report::Suggestion,
    },
    /// Ask the user one question with 2..8 fixed answers, shown as a card under your reply. Exclusive by default; multiple=true allows several. Use it instead of a numbered list whenever the user must choose before you can continue, then end your turn: the pick arrives as the user's next message with the exact labels, and the user may type something else instead. Never answer it yourself. No document edits.
    OfferChoices {
        /// Question, options and whether several may be picked.
        #[serde(flatten)]
        card: crate::choices::ChoiceCard,
    },
    /// Opt in to bounded transition evidence, read its cursor or stop collection. Starting requires explicit user consent via the host permission gate. No audio or document edits.
    ExecutionTrace {
        /// Collection control or cursor read.
        operation: crate::execution_trace::TraceOperation,
    },
    /// Propose exact local pitch variation with fixed rhythm and protected opening, or cadence pitch/length changes, without editing. Explicit clip-relative range and protected indices are enforced. Returns before/after notes and an equivalent edit_notes patch. Use candidates variation to compare and adopt the exact result; structural validity does not establish musical quality.
    PhraseVariation {
        /// Current content revision from read_notes.
        expected_revision: String,
        /// Explicit range, protected notes and allowed changes.
        request: phrase_variation::Request,
    },
    /// Compare a baseline and 1..3 exact instrument-note or accompaniment alternatives without editing. Create binds snapshots to this document/revision for fifteen minutes; read returns exact note differences; preview renders the same short range with common gain. Apply adopts the selected snapshot as one Undo without regeneration. Cancel retains the original. At most three rounds per revision and three previews per round; stop and report unresolved choices at the limit. These tools establish no musical preference. Read tool_help for operation fields.
    Candidates {
        /// Create, read, preview, apply or cancel a bounded candidate round.
        operation: crate::session::candidates::Operation,
    },
    /// Suggest up to three melody-aware chord progressions without changing notes or harmony. Pass request with melody_clips, start_bar, bars and optional key/mood/ending. Keeps a plain alternative; 7th, sus4 and diminished colour needs melodic evidence and appropriate resolution, never a complexity quota. Returns explanations, tensions, a content revision and harmony objects usable by accompaniment. Read tool_help for bounds. These are heuristic proposals; compare and audition before claiming musical improvement.
    SuggestHarmony {
        /// Explicit foreground clips and musical intent for one short phrase.
        request: harmony_suggestions::Request,
    },
    /// Generate or regenerate backing clips. Preserves all other clips. Protected clips only reserve tracks; foreground requires notes on non-drum tracks. Uses Auris writers. Read inspect_project for expected_revision. Runs on a worker and applies atomically as one Undo; stale/cancelled work and hand-edited regeneration targets are refused. Returns recipes, not notes. Split at meter/section boundaries; see tool_help for limits.
    Accompaniment {
        /// Content revision from inspect_project or read_notes.
        expected_revision: String,
        /// 1..16 distinct melody clip IDs to protect; never inferred from names.
        #[schemars(length(min = 1, max = 16))]
        protected_clips: Vec<u64>,
        /// 1..16 distinct foreground clips to arrange around, a subset of protected_clips. Includes authored and imported melodies; never inferred from names or recipes.
        #[schemars(length(min = 1, max = 16))]
        foreground_clips: Vec<u64>,
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
    /// Find audio on Wikimedia Commons or by local filename, inspect a source, import a direct HTTP(S) audio URL, or trim/reverse a clip into a new asset. Original files remain intact; processing is one Undo and rejects stale revisions. EQ uses mixer_graph add_effect with auris.fx.eq, then parameters/set_parameter. search_online needs no API key; download needs an audio URL, not a webpage. Attribution is not verified rights.
    AudioAssets {
        /// Material operation, performed off the UI thread.
        operation: crate::session::audio_assets::Operation,
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
    /// Inspect, partially edit or explicitly regenerate 1..16 generated clips. Writes preserve other clips and mix state; hand-edited generated notes require explicit replacement permission. Recipe controls change stored notes; performance controls are separate. Subdivision is a grid, not note duration: eighth, sixteenth, eighth-triplet or sixteenth-triplet. Rhythm must use repeating grid steps: x hit, X accent, o ghost, dot/tilde/hyphen rest (example x...x...); empty clears it. Adjust note count and held duration with density and gate, not subdivision. For a piano-style chord comp, low_root adds the root under the voicing and strum staggers onsets upward by a note value (32 or 64).
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
    /// Render an immutable snapshot to a new WAV mix or stems, then measure the encoded files. Default delivery mode attenuates only when needed for ceiling_db (default -1 dBTP) before encoding; stems share one gain. Read attenuation_db, source and encoded measurements. Opt into analysis for an exact 1-frame..300-second range at an explicit rate, stereo float32 with zero protective gain, source/state/environment identities and a persistent manifest. Analysis has its own seconds/frame range and optional source/bus, dry routing, causal context or cycle warmup. Existing outputs are never replaced. No save or document edit; cancellation/stale documents discard private stages.
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
        /// Opt into immutable training output, precise timing and a persistent provenance manifest.
        analysis: Option<file_jobs::AnalysisRender>,
    },
    /// Create a protected 24 kHz WAV audition of 1..120 seconds from the current snapshot. Returns a playable file under the document's preview folder. Does not imply listening.
    Preview {
        /// Explicit one-based bar range within the arrangement.
        range: file_jobs::BarRange,
    },
    /// Export instrumental MIDI with trailing rests. Uses the unsaved score. Never overwrites or saves the project. Returns note count, duration in ticks/seconds and PPQ.
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
    /// Move/copy/split/resize/loop/remove MIDI or audio clips by stable ID. Preserves contents, recipes and performance unless explicitly frozen or baked. A looped clip stores one pass (length_ticks) and repeats it to loop_end_tick, so edit the one pass rather than writing the repeats out. One atomic Undo step.
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
    /// Discover instrument parameter keys, value_domain, units, steps, read_only flags, values and automation overrides. VST3 values are normalized 0..1, not display units such as Hz/dB. Follow next_offset.
    InspectInstrument {
        /// Stable instrument or drum track ID.
        track: u64,
        /// Zero-based parameter offset.
        #[serde(default)]
        offset: usize,
    },
    /// Set 1..256 static instrument controls atomically in the inspected value_domain (VST3: normalized 0..1). Discover exact keys/ranges/steps with inspect_instrument; read_only controls cannot be changed. Omitted controls and existing automation are preserved. Identical retries are no-ops.
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
        /// Optional key such as D major or A minor; replaces internal key changes and restores the prior key after the range.
        key: Option<String>,
        /// Prefer an array of bars, e.g. ["I","IV V","vi","ii V"]. Each entry spans one bar, with one or more chords sharing it equally; two chords in 4/4 start on beats 1 and 3. Choose harmonic rhythm per bar; there is no one-chord limit. Alternatively use an explicit pipe-delimited chart "| I | IV V | vi | ii V |", a single chord, or @catalogue. Unbarred multi-chord strings are rejected to prevent squeezing the whole progression into each bar. Placement follows meter changes, ignores the editing grid, and preserves outside harmony. Notes are unchanged.
        #[serde(default, deserialize_with = "harmony::deserialize_chords")]
        #[schemars(with = "Option<harmony::ChordChartInput>")]
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
    /// Read voice, speakers, take state and section_coverage of note/lyric onsets. Follow next_section_offset and compare later sung sections with the plan. Read-only; no model loading.
    InspectSinger {
        /// Stable singer track ID.
        track: u64,
        /// Zero-based speaker offset; follow next_offset for voices with more than 128 speakers.
        #[serde(default)]
        offset: usize,
        /// Zero-based section page offset, independent of speaker offset. Follow section_coverage.next_section_offset.
        #[serde(default)]
        section_offset: usize,
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
    /// Write original lyrics (graphemes, not readings) across a singer clip's notes in time order. Keep original kanji/kana spelling in text for display; supply optional kana reading separately for pronunciation, including without a dictionary. The clip name becomes its complete written lyrics without abbreviation. Optional from is a zero-based storage index from read_notes; omit to start at the earliest note. Without reading, kana is split into moras and other text requires the installed Japanese dictionary. Kana is read as a phrase: particles は/へ sing wa/e where they end a space-separated word or the dictionary identifies them, so separate words with spaces (きみは わらう); write わ/え in reading only to force it. Insufficient note slots are refused without editing; later unfilled notes keep their words. Returns filled count, available notes and full clip name; one transaction. Use read_notes to verify before sing.
    WriteLyrics {
        /// Stable singer clip ID.
        clip: u64,
        /// Original nonempty written phrase, e.g. 桜咲いた. Never replace it with its reading.
        #[schemars(length(min = 1, max = 16384))]
        text: String,
        /// Optional nonempty kana pronunciation, e.g. さくらさいた; leaves the written text intact.
        #[schemars(length(min = 1, max = 16384))]
        reading: Option<String>,
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
    /// Preview then accept an immutable external draft as new editable tracks in one Undo. Multitrack apply requires the preview revision and digest. Existing timeline maps are retained unless version two explicitly requests replacement. Review notes, timing and playback instruments.
    ImportTranscription {
        /// Validated recognizer-independent draft; resolved by the tool host before authorization.
        draft: Box<crate::TranscriptionDraft>,
        /// New track/clip name for the pitched note draft.
        name: String,
        /// Nonnegative absolute insertion tick.
        at_tick: i64,
        /// Validate and preview without editing the project.
        #[serde(default)]
        preview: bool,
        /// Exact preview revision; required when applying multitrack drafts.
        expected_revision: Option<String>,
        /// Digest returned by preview; required when applying multitrack drafts.
        preview_digest: Option<String>,
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
    /// Inspect actual rendered audio and score without saving: mel image, levels, piano roll and up to 128 harmony_events including within-bar chord/key changes. Event tick is absolute (960 per quarter); bar/beat are 1-based, tick_in_beat uses the meter's beat, start_seconds is relative to the inspected range. Check harmony_events_truncated; inspect_project retains the full harmony maps. Start within the arrangement and request 1..8 bars fitting in 30 seconds. Visual interpretation is not listening. Use the same range before and after editing.
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
    /// For explicitly requested evaluator development experiments only; not part of ordinary composition or delivery. Render the unsaved mix and send actual WAV audio to the configured critic. Use focus for single-excerpt diagnosis. For blind A/B, pass compare_to and a neutral aspect, omitting focus; order is randomized and the critic sees no edit narrative. Returns short untrusted prose in review, review_format=plain_text, exact WAV measurements, revision and audition_id. No preference or confidence is extracted. Some backends describe each excerpt independently. Never automatically adopt an edit from this prose alone. Use 1..8 bars, at most 30 seconds, inside the arrangement. Retains four IDs with matching bars/timing. No save or Undo; network permission applies. Separate-model feedback is not proof that you heard the music.
    Listen {
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Number of bars, 1..8, fitting at most thirty seconds.
        #[schemars(range(min = 1, max = 8))]
        bars: u32,
        /// Diagnostic question for one excerpt, up to 4000 bytes. Must be omitted for comparison.
        focus: Option<String>,
        /// Opaque audition_id from this open document, with the same bars and timing.
        compare_to: Option<String>,
        /// Neutral comparison criterion, required with compare_to and omitted for diagnosis. Comparison never forwards an editing narrative.
        aspect: Option<crate::audio_review::assessment::ComparisonAspect>,
    },

    /// Search instruments by text, acoustic reference, character/impression, or bounded browsing. Character mode accepts Japanese/English impressions and explicit 0..1 targets (brightness, attack speed, sustain, captured release, noise); translate arbitrary prose/negation into targets and omit query when necessary. Optional id retains resemblance to an existing sound; name restricts names/tags. Inspect returned targets, character and relative distance, then render a musical phrase before adopting. Copy returned sound IDs into set_instrument. Retry indexing results later without refreshing. Shared by desktop Agent and MCP.
    SearchInstruments {
        /// Explicit text, similar, character or browse request.
        request: crate::SoundSearch,
        /// Rescan only on the first page; invalidates existing acoustic measurements.
        #[serde(default)]
        refresh: bool,
    },
    /// Edit authored notes atomically across 1..16 clips at musical positions: bar is the 1-based song bar (as in add_clip, not counted from the clip), beat is 1-based within that bar in the meter's beat (quarter in 4/4, eighth in 6/8), fractional for subdivisions (2.5 offbeat, 2.25 sixteenth, 2.333 triplet), and length_beats counts the same beat. Attacks must sit on a straight subdivision down to an eighth of a beat or a triplet down to a sixth; anything between is refused with the nearest choices, so use swing/humanize performance for feel. Velocity is normalized 0..1, never MIDI 0..127; for example {"pitch":60,"bar":5,"beat":1,"length_beats":1,"velocity":0.75}. Use input {"kind":"inline","edits":[...]} or {"kind":"file","source_path":"/absolute/notes.json"}, never both. Edits are typed objects, not JSON strings; expected_revision belongs at the top level. Read read_notes first and supply its revision. Patch indices address original storage order. Preserves recipes, curves, metadata on updated notes and performance. Identical replacements are no-ops.
    EditNotes {
        /// Exact content revision from the latest read_notes or inspect_project.
        expected_revision: String,
        /// One patch or replacement per clip, at most 65536 resulting notes total.
        #[schemars(length(min = 1, max = 16))]
        edits: Vec<note_editing::NoteEdit>,
    },
    /// Read 1..8 MIDI clip pages in one call. Copy each clip from inspect_project tracks[].clips[].clip or add_clip's clip result, never tracks[].track or add_track's track result. Track IDs and clip IDs are different addresses. Each request is `{clip:<returned clip ID>,offset:0}`; offset defaults to zero. Returns one shared revision and clips containing up to 128 notes each with song bar, beat and length_beats in the same units edit_notes takes (each clip also reports its bars `[first,last]`; on_grid:false marks a stored attack between subdivisions), normalized velocity 0..1, original zero-based storage indices (`index`) and a stable timeline rank (`timeline_index`). Pages remain in storage order so edits address the original indices; sort by `timeline_index` for lyric and phrase work. The rank uses start position then storage index as its tie-break. Follow each clip's next_offset; truncated is explicit. Do not edit between pages. Pass the top-level revision to edit_notes; stale indices are rejected.
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
    /// Give the song its title. The title names the project and is what Save As and exports suggest as the file name until the user saves. Name a new song once its character is clear; keep a title the user gave or chose unless asked to change it.
    RenameProject {
        /// The song title, 1-120 characters.
        #[schemars(length(min = 1, max = 120))]
        title: String,
    },
    /// Rename a track using its stable numeric ID from inspect_project or add_track.
    RenameTrack {
        /// Stable track ID.
        track: u64,
        /// New name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
    },
    /// Add a layer (an instrument track playing another melodic or singer track's notes, for doubling or splitting a part), read or reshape it, or detach it into notes of its own. Shape filters by source key range and top/bottom voice, then transposes and scales velocity/length and shifts by quarter notes; omitted fields keep their values. A layer has no clips of its own: edit the source's notes. Drums cannot be followed.
    Layer {
        /// Add, read, shape or detach.
        operation: layers::Operation,
    },
    /// Remove one track and its clips. Layers following it are detached first, keeping their sound.
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

    /// Measure the whole mix, move every source fader by the same amount toward target_lufs under a true-peak ceiling, and measure again. Finish every song with this after the last sound, note or fader change. Keeps relative balance and master gain; refuses automated source gain. When the master chain ends in auris.fx.limiter (new projects have one), its ceiling is set to ceiling_db and peaks may be pushed up to 6 dB into it; without one the loudest moment alone limits the result, which leaves drum-heavy mixes several dB short. The measured result may fall short at fader or peak limits. Persistence follows the host.
    NormalizeMix {
        /// Desired integrated loudness, -60 to -6 LUFS; defaults to -14, the streaming level a finished song is delivered at.
        #[schemars(range(min = -60, max = -6))]
        target_lufs: Option<f32>,
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
            Self::ImportTranscription { preview, .. } => !preview,
            Self::AudioAssets { operation } => operation.is_mutating(),
            Self::Candidates { operation } => matches!(
                operation,
                crate::session::candidates::Operation::Apply { .. }
            ),
            Self::Arrangement { operation } => operation.is_mutating(),
            Self::RepairLyrics { request } => request.apply,
            Self::History { operation } => !matches!(operation, HistoryOperation::Read),
            Self::Checkpoints { operation, .. } => operation.is_mutating(),
            Self::Automation { operation, .. } => !operation.is_read_only(),
            Self::EditRecipe { operation, .. } => !operation.is_read_only(),
            Self::MixerGraph { operation } => !operation.is_read_only(),
            Self::EditClipCurve { operation, .. } => !operation.is_read_only(),
            Self::EditTimeline { operation } => !operation.is_read_only(),
            Self::Layer { operation } => !operation.is_read_only(),
            Self::Inspect {}
            | Self::SuggestIssueReport { .. }
            | Self::OfferChoices { .. }
            | Self::ExecutionTrace { .. }
            | Self::PhraseVariation { .. }
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
            Self::AudioAssets { .. }
                | Self::Candidates {
                    operation: crate::session::candidates::Operation::Create { .. }
                        | crate::session::candidates::Operation::Preview { .. }
                }
                | Self::Arrangement {
                    operation: crate::arrangement::Operation::Validate { .. }
                }
                | Self::Accompaniment { .. }
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
        let trace = if matches!(command, Command::ExecutionTrace { .. }) {
            None
        } else {
            self.trace_call_begin()
        };
        let name = if trace.is_some() {
            serde_json::to_value(&command)
                .ok()
                .and_then(|v| v["action"].as_str().map(str::to_owned))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let result =
            self.with_agent_history(|session| session.execute_agent_command(command, plugin_paths));
        self.trace_call_end(trace, &name, result.is_ok());
        result
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
            Command::SuggestIssueReport { suggestion } => {
                suggestion.validate()?;
                Ok(serde_json::json!({"suggestion":suggestion,"published":false,"private_context_included":false,"next":"The desktop offers a report invitation. The human must open the draft and explicitly approve each private context category. No permission to disclose or publish has been granted."}).to_string())
            }
            Command::OfferChoices { card } => {
                card.validate()?;
                Ok(serde_json::json!({"shown":true,"multiple":card.multiple,"options":card.options.len(),"next":"The desktop shows the card. End your turn now without choosing for the user; the pick arrives as the user's next message."}).to_string())
            }
            Command::ExecutionTrace { operation } => self.execution_trace(operation),
            Command::PhraseVariation {
                expected_revision,
                request,
            } => self.agent_phrase_variation(&expected_revision, request),
            Command::Candidates { operation } => self.agent_candidates(operation),
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
                let parameters: Vec<_> = descriptors
                    .iter()
                    .skip(offset)
                    .take(32)
                    .map(|descriptor| {
                        let target = crate::ParamTarget::Instrument {
                            track,
                            param: descriptor.id,
                        };
                        self.agent_parameter_report(target, descriptor)
                    })
                    .collect();
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
            Command::WriteLyrics {
                clip,
                text,
                reading,
                from,
            } => {
                if text.trim().is_empty() || text.len() > 16384 {
                    return Err("text must contain lyrics within 16384 UTF-8 bytes".into());
                }
                if reading
                    .as_ref()
                    .is_some_and(|reading| reading.len() > 16384)
                {
                    return Err("reading must fit within 16384 UTF-8 bytes".into());
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
                    .write_lyrics_with_reading(ClipId(clip), indices, &text, reading.as_deref())
                    .map_err(error)?;
                Ok(
                    serde_json::json!({"filled":filled,"available":indices.len(),"clip_name":self.midi_clip(ClipId(clip)).map(|clip| &clip.name),"unsaved":true})
                        .to_string(),
                )
            }
            Command::InspectSinger {
                track,
                offset,
                section_offset,
            } => self.agent_inspect_singer(TrackId(track), offset, section_offset),
            command @ (Command::AudioAssets { .. }
            | Command::Accompaniment { .. }
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
                preview,
                expected_revision,
                preview_digest,
            } => {
                let rejection = |code: &str| {
                    serde_json::json!({"error": {"code":code,"repreview_required":true}})
                        .to_string()
                };
                let multitrack =
                    matches!(draft.as_ref(), crate::TranscriptionDraft::Multitrack { .. });
                if !preview && multitrack && expected_revision.is_none() {
                    return Err(rejection("transcription_revision_required"));
                }
                if !preview
                    && expected_revision.as_ref().is_some_and(|expected| {
                        self.agent_document_revision().as_ref() != Ok(expected)
                    })
                {
                    return Err(rejection("transcription_revision_mismatch"));
                }
                let digest = draft.preview_digest(&name, at_tick)?;
                if !preview && multitrack && preview_digest.as_deref() != Some(&digest) {
                    return Err(rejection(if preview_digest.is_none() {
                        "transcription_preview_required"
                    } else {
                        "transcription_preview_mismatch"
                    }));
                }
                if preview {
                    draft.validate()?;
                    let report = if let crate::TranscriptionDraft::Multitrack { transcription } =
                        draft.as_ref()
                    {
                        self.import_multitrack_transcription(transcription, &name, at_tick, true)
                    } else {
                        Ok(serde_json::json!({"preview":true,"notes":draft.note_count(),"at_tick":at_tick,"revision":self.agent_document_revision()?}).to_string())
                    }?;
                    let mut report: serde_json::Value =
                        serde_json::from_str(&report).map_err(|e| e.to_string())?;
                    report["preview_digest"] = digest.into();
                    Ok(report.to_string())
                } else {
                    self.agent_import_draft(*draft, name, at_tick)
                }
            }
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
                            clips.iter().map(|clip|with_loop_end(serde_json::json!({"clip":clip.id.0,"name":clip.name,"start_tick":clip.start.raw(),"length_ticks":clip.length.raw(),"note_count":clip.notes.len()}), clip.start, clip.length, clip.sounding_length())).collect()
                        } else if let Some(audio)=track.kind.as_audio() {
                            audio.clips.iter().map(|clip|with_loop_end(serde_json::json!({"clip":clip.id.0,"name":clip.name,"start_tick":clip.start.raw(),"length_ticks":project.audio_clip_length_ticks(clip).raw(),"gain_db":clip.gain_db,"source_bpm":clip.source_bpm,"follows_tempo":clip.follows_tempo,"fade_in_frames":clip.fade_in_frames,"fade_out_frames":clip.fade_out_frames,"source_frames":clip.length_frames,"source_sample_rate":project.audio_sources.get(&clip.source).map(|source|source.sample_rate),"fade_in_curve":clip.fade_in_curve,"fade_out_curve":clip.fade_out_curve}), clip.start, project.audio_clip_length_ticks(clip), project.audio_clip_sounding_ticks(clip))).collect()
                        } else { Vec::new() };
                        serde_json::json!({"track":track.id.0, "name":track.name,
                        "kind":track.kind.label(), "mixer":mixer_summary(&track.mixer), "clips":clips,"drum_assignments":self.drum_assignments(track.id),
                        "instrument":track.kind.as_instrument().map(|inner| &inner.instrument_id),
                        "layer":track.kind.layer().map(|source| serde_json::json!({"follows":source.track.0,"shape":layers::shape_json(&source.shape)})),
                        "soundfont_preset":self.track_preset(track.id)})
                    })
                    .collect();
                Ok(serde_json::json!({"title":project.name, "tracks":tracks, "revision":self.agent_document_revision()?,
                    "document_instance":self.document_instance(), "session_revision":self.revision(),
                    "verification_regions":crate::production_record::verification_regions(project),
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
                aspect,
            } => {
                let job = self.begin_listening(
                    start_bar,
                    bars,
                    focus.as_deref(),
                    compare_to.as_deref(),
                    aspect,
                )?;
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
            Command::RenameProject { title } => {
                let title = title.trim();
                if title.is_empty() || title.chars().count() > 120 {
                    return Err("A song title needs 1-120 characters".into());
                }
                self.rename_project(title);
                Ok(serde_json::json!({"title":title,"unsaved":true}).to_string())
            }
            Command::RenameTrack { track, name } => {
                if name.trim().is_empty() {
                    return Err("A track needs a name".into());
                }
                self.rename_track(TrackId(track), name).map_err(error)?;
                Ok("Renamed track".into())
            }
            Command::Layer { operation } => self.agent_layer(operation),
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
                let target_lufs = target_lufs.unwrap_or(crate::TARGET_LUFS);
                let ceiling_db = ceiling_db.unwrap_or(crate::session::NEW_DOCUMENT_CEILING_DB);
                let report = self.normalize_mix(target_lufs, ceiling_db).map_err(error)?;
                let reached = report.after.lufs.unwrap_or(f32::NEG_INFINITY);
                let stage = match (report.limited, target_lufs - reached > 1.0) {
                    (true, _) => " The master limiter holds the ceiling.",
                    (false, true) => {
                        " The master chain has no limiter, so the loudest peak stopped the lift; insert auris.fx.limiter last on the master with mixer_graph and normalize again."
                    }
                    (false, false) => "",
                };
                Ok(format!(
                    "Moved source faders {:+.2} dB. Mix {:.1} -> {:.1} LUFS; true peak {:.1} -> {:.1} dBTP. Target {:.1} LUFS, ceiling {:.1} dBTP.{stage} Inspect_audio again before judging the sound.",
                    report.offset_db,
                    report.before.lufs.unwrap_or(f32::NEG_INFINITY),
                    reached,
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
    fn the_agent_titles_the_song_and_the_title_becomes_the_suggested_file_name() {
        let mut session = session();
        assert_eq!(session.document_title(), Project::UNTITLED);
        let before = session.project().clone();
        for title in ["", "   ", &"x".repeat(121)] {
            assert!(
                session
                    .agent_command(Command::RenameProject {
                        title: title.into(),
                    })
                    .is_err()
            );
            assert_eq!(session.project(), &before);
        }
        session
            .agent_command(Command::RenameProject {
                title: "  夜明けのテーマ ".into(),
            })
            .unwrap();
        assert_eq!(session.project().name, "夜明けのテーマ");
        assert_eq!(session.document_title(), "夜明けのテーマ");
        assert!(session.is_dirty());
        session.undo();
        assert_eq!(session.project().name, Project::UNTITLED);
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
