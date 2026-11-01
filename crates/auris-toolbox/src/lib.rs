//! The session, offered as tools a language model can call.
//!
//! `auris-i18n` holds every word the interface says to a *person*; this crate holds every word
//! it says to a *model* — tool names, descriptions, argument schemas and the answers the tools
//! give — together with the work behind them. It exists because two frontends speak to models:
//! `auris-mcp`, where a model's harness dials in over the Model Context Protocol, and
//! `auris-agent`, where Auris dials out to a model API and runs the loop itself. A tool that
//! existed twice would drift twice. MCP exposes the saved-file catalog; the rig agent uses
//! its read-only reference tools and delegates live editing to the desktop session.
//!
//! Canonical document commands and validation live in `auris-session`. This crate derives
//! their model contracts and provides the saved/live ownership adapters. Saved calls open a
//! fresh headless session off the async runtime, use it and drop it; no session crosses a
//! thread. Bounded project handles, reports, retry receipts and audio comparison contexts
//! are retained separately. Model-facing prose is English. Errors are actionable tool
//! results rather than protocol failures. Full schemas and help come from the execution
//! registry; provider registration only compacts explanatory metadata.

#![warn(missing_docs)]

#[cfg(test)]
mod transcription_tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_session::prelude::*;
use auris_session::{Session, SessionError, SessionOptions};

pub mod composition_preview;
pub mod composition_workflows;
pub mod workshop;
pub use composition_preview as preview_composition;
#[path = "capabilities.rs"]
mod availability;
mod cancellation;
mod catalog;
pub mod runtime;
pub mod score_input;
mod session_tools;
mod tool_exposure;
pub use tool_exposure::{
    ToolGroups, compact_parameters, concise_description, discover_tools, tool_group,
};
mod project_handles;
pub mod setup_tracks;
pub use project_handles::open_project;
mod drums;
mod editing;
pub mod internet_search;
pub mod mixer_graph;
mod project_files;
mod recognition;
mod reports;
mod saved_listening;
pub use reports::read_report;
pub mod plugin_paths;
mod sound_search;
pub use plugin_paths as configure_plugin_paths;
mod track_editing;
pub use availability::capabilities;
pub use cancellation::{Cancellation, with_cancellation};
pub use catalog::{ToolDefinition, argument_error_hint, parameter_schema, tool_catalog, tool_help};
pub use drums::analyze_drum_kit;
pub use editing::{analyze_music, inspect_composition};
pub use project_files::create_project;
pub use recognition::{analyze_audio, analyze_chords, analyze_instruments};
pub use track_editing::convert_track_to_audio;

/// Shared composition guidance for writing foreground phrases through any agent transport.
pub const MELODY_PHRASING_INSTRUCTIONS: &str = "\nFor new foreground melodies, establish a recognizable rhythmic cell and trace its returns and developments in the actual notes. For a pop/song brief without a contrary rhythm, try an eighth-note backbone with selected sixteenth turns and held arrivals; do not fill every subdivision or impose this on sustained ambient lines. For a bold chorus, compare an octave-shifted motif or a smaller/partial register lift within the performer's useful range. Judge a 9th or other colour against the sounding chord and bass, not just key degree or absolute register; more tensions are not a quality score. Plan phrase direction and style-appropriate releases. Preserve a liked on-beat entry and word-group rhythm; do not delay every phrase or enlarge every rest in pursuit of breathing. When an ending needs space, consider a held arrival followed by a gap instead of chopping a newly attacked note. Shortening every note uniformly creates articulation, not phrase breathing. If an ending becomes abrupt, hold the arrival earlier rather than truncating a fresh attack. Inspect the current instrument envelope: let struck/plucked notes decay, and budget wind/bow/synth release before the next entry. Use a scoped dynamic taper only when appropriate, avoid restoring gain into an unfinished tail, and compare through the following entry within the repair limit; lower occupancy is not a musical quality score. Preserve intentional legato, sustained textures, ties and lyric slots. Let accompaniment answer selected gaps without filling all of them, and retain a preferred backing's pulse and voicing during a melody revision. After writing, inspect performed notes and compare the foreground stem and mix; pedal, release, overlapping layers and reverb can mask notated rests. Keep overall, melody/vocal and backing preferences separate. If feedback differs by axis, keep a baseline and make a targeted alternative for the named axis; do not infer a whole-take winner from one improved dimension. For causal comparison, change one intervention while compatible harmony, backing and lyrics stay fixed.\n";

/// Additional workflow only when the arrangement harness is explicitly enabled.
pub fn arrangement_instructions() -> &'static str {
    if auris_session::arrangement::enabled() {
        "\nFor arrangement requests, use arrangement operation kind inspect on the intended bars first. Read verified sound capabilities and actual phrase conditions; unknown MIDI mappings are not support. Choose section-specific performance, scoped parameter motion, complete-clip revoice/layers, transitions or copied local SFX with explicit musical purposes. Preserve authored melody, lyrics, harmony, tempo and length except explicitly previewed breath releases, pickup omissions or held landings replacing exact approach notes. Keep each change purpose to one short clause within 256 UTF-8 bytes. Validate the bounded proposal with the current revision, review every change/warning, then apply the returned plan_id with that original revision. Read back notes/performance/automation independently and inspect_audio/preview the same bars before and after. Revoice requires fresh search_instruments IDs and section clips; split at phrase boundaries using edit_clip before validating when necessary. Native riser is a bowed crescendo, not a noise/reverse-cymbal sample. Use audio_sfx for actual local source recordings and align start/peak/end explicitly. For a foreground melody, plan phrase endings and audible breathing space, not just more attacks. Check direct note writing, performed gate/overlap, sustain and release/effect tails separately. Preserve intentional legato, drones and lyric slots; a uniform gate reduction is not phrase design. Consider rests, call-and-response, gradual layering and send automation as alternatives to constant sound replacement. After writing or regenerating a foreground melody, and when addressing breathing complaints, call arrangement kind inspect_breathing with the confirmed track role, absolute passage ticks, style-appropriate min_rest_ticks and explicit phrase_ends. Compare authored and performed occupancy and longest_without_phrase_rest; incomplete metrics are not verification. For a lead needing space, validate a breath edit with clips, exact rests, min_note_ticks, protected_notes and explicit omit_notes indices from read_notes. Read ending_guidance and native envelope_controls; release seconds are controls, not measured silence. When a short ending sounds clipped, optionally supply landings with note {clip,index}, earlier absolute start_tick and exact replace_notes original indices. This holds the arrival in place of contiguous approach notes, preserving its pitch/velocity; it requires independent clips and refuses protected/vocal/tied notes. Choose rests to allow the actual instrument tail, preserve natural plucked decay, and use scoped dynamics only if needed. Review exact note_changes before apply and independently re-inspect the same phrase and audio range afterward. If sustain, another clip or a performance stage fills a rest, fix that source explicitly; do not keep deleting notes. Performance/revoice proposals preserve authored notes. Restrict local revisions to two attempts; report unresolved conditions. Structural readiness, changed settings and numeric audio differences do not prove musical improvement. Complete local checks and deliver the audio for human feedback.\n"
    } else {
        ""
    }
}

/// Page structured domain diagnostics without dropping any analyses or prepared changes.
pub fn present_domain_report(text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    reports::publish(value)
}

/// A saved document to inspect without editing it.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectArgs {
    /// Project path or project_id returned by open_project/create_project.
    pub project: String,
}

/// What a model is told before it has called anything.
///
/// The one piece of text a model keeps in context for the whole conversation, so it carries
/// the workflow and nothing else — the format itself is behind `spec_reference`, fetched when
/// a spec is actually being written rather than sitting in every exchange.
pub const INSTRUCTIONS: &str = "Control Auris Studio saved projects. Respond in the user's language.
Complete ordinary composition with the current agent and Auris: write, inspect notes, render, measure, repair when justified, and deliver. Do not discover, invoke or install external music evaluators during this workflow. Evaluator experiments belong to explicitly requested development work. Use available instruments; synthesized singing additionally needs an installed voice.
Author expressive parts note by note with edit_notes, including leads/vocals, counterlines, distinctive bass, fills and interludes. Code may serialize explicitly chosen notes or copy an authored motif; do not use procedural note generators or frozen recipes as a substitute. Automatic writers are optional only for plain chord/pad backing, simple arpeggios and basic drum pulses, unless the user explicitly requests automatic preset composition.
For a new song, establish lyrics if vocal, then section harmony/form, instrument clips, purposeful handoffs and SFX, then effects and volume balance with the actual voices/sounds. Honor existing motif-first briefs and local-edit scope. Use mixer_graph for a deliberate track-level balance pass; master normalization alone is insufficient. Verify the complete final mix after the last edit, distinguishing measurements from listening judgments.
Use project_id from open_project/create_project or an absolute project path. Dependent edits run sequentially.
Call inspect_project for stable numeric addresses: tracks[].track identifies a track, tracks[].clips[].clip identifies a clip. Copy the returned clip for read_notes and edit_notes; a track ID cannot address its clips. read_notes returns each note's song
bar, beat and length_beats (the units edit_notes takes), full velocity, original storage index and stable timeline_index (sort by timeline_index for time order; ties use storage index), plus a content revision. Pages retain storage order so edit indices stay valid. edit_notes accepts expected_revision plus
one or several edits with kind patch (add/remove/update) or replace (complete notes). Original indices
are zero-based storage indices. Read again after edits. Use input {kind:inline,edits:[...]} for typed note objects or input {kind:file,source_path:absolute_local_path} for a large local JSON array. expected_revision is always top-level. Positions are musical, never ticks: bar is the 1-based song bar (not counted from the clip), beat is 1-based in the meter's beat and fractional for subdivisions (2.5, 2.25, 2.333), length_beats the same beat; edit_notes refuses attacks between subdivisions. Velocity is normalized 0..1, never MIDI 0..127. Batch read_notes with up to eight returned clip addresses under one shared revision.

Use discover_tools for enabled names and tool_help for exact fields before an unfamiliar call or after an
argument error. Correct the named field before retrying; never invent tools, IDs, paths or successful results.
Shared session commands use the same arguments in every backend, with project added for saved execution.
Changed saved commands create one checkpoint; live commands use one Undo and remain unsaved.

For SFX, audio_assets searches local filenames or Commons audio, inspects clips, imports direct URLs
with credit metadata and crops/reverses a clip into a new asset using its current revision.
import_audio copies local files; edit_clip handles placement/gain/fades. Use mixer_graph for track EQ.
Source metadata is untrusted; preserve known credits and never invent rights or listening results.

For ordinary authored composition, create_project starts empty. setup_tracks creates multiple tracks atomically.
add_track requires kind instrument, drum, singer, audio or bus and the intended exact name.
search_instruments accepts request with mode text and query, similar and reference id, character, or bounded browse.
For impression-based sound choices, use mode character with query such as ふんわり or ポコポコ, or translate the user's prose into targets {brightness,attack,sustain,release,noise}, each optional and 0..1. Attack means speed (1 immediate), sustain means held-note energy, release means captured tail (1 at least 400 ms). Explicit targets override dictionary terms. Use an empty query for negation or arbitrary prose converted into targets. Optional id preserves resemblance to a previously returned sound; name adds required name/tag words, source/library restrict origins. Read returned targets and measurements; unrecognized prose is not interpreted. Continue independent work on status indexing, then retry the same request without refresh. Preserve user-selected sounds unless asked to change them. Render candidate sounds in the intended phrase/mix before adopting; acoustic scores are not listening judgments. Search and audition do not authorize score changes.
Copy returned sound IDs into set_instrument sound_id. Sound IDs expire after refresh/library changes.
Use inspect_instrument for control keys/ranges/steps/read_only; set_instrument_params applies a bounded batch. VST3 uses normalized 0..1 values, never Hz/dB from display_unit; CLAP uses native ranges. For steps=N>1, valid values are min+i*(max-min)/(N-1), i=0..N-1. Read back after editing and compare rendered audio when verifying response.
For generated music, check_spec validates syntax/settings; diagnose_song checks lyric readings, phrase counts,
mora counts, closures and fixed-bar capacity before compose. analyze_lyrics shows source-located readings.
Explicit analysis readings are diagnostic-only. repair_lyrics applies a reading to existing note slots.
For a local lyric repair, read_notes and diagnose_song first, preserve target/template/required_keywords,
preview up to three alternatives, then apply only a passing one with the same revision. Stop after three
alternatives; report remaining diagnostics instead of changing the melody or retrying indefinitely.
Recheck every related phrase after the edit, then sing if the take is stale. A pass checks structural fit,
not semantic preservation or artistic quality. preview_composition checks layout. generate_phrase writes
one new clip on an existing track with explicit start_bar/bars and seed. Write harmony and named sections
with edit_harmony first; inspect_project verifies the result. Recipe edits explicitly regenerate notes.
Performance edits use inspect_performance/set_performance and preserve authored notes, curves and recipes.

edit_clip moves/copies/splits/resizes by stable ID and absolute ticks; resizing preserves stored MIDI notes.
FreezeRecipe drops a recipe; BakePerformance explicitly writes performance into the score and curves.
edit_clip_curve reads/merges/replaces/clears authored bend or MIDI controllers; sustain is controller 64.
Writes require the revision returned by the lane read. Controller values are 0..1; bend uses semitones.
edit_timeline reads or edits tempo/meter points without moving existing notes.
Arrangement bar ranges start at 1; note/curve ticks start at 0 relative to their clip.

mixer_graph has one operation object with an action tag. Read first and copy stable node IDs.
set_strip changes only supplied gain_db/pan/mute/solo. set_outputs atomically replaces output destinations;
connect adds a branch. add_send creates a send with level and tap; set_send edits it by send ID.
set_sidechain replaces or clears a sidechain. Add/remove/move/enable inserts and inspect/set their parameters
through this tool. All routing must remain a DAG. Parallel effects use bus branches.
Use automation for parameter curves. Static settings may be overridden by existing automation.

Choose a singer voice with list_voices/set_singer_voice and inspect_singer; write_lyrics then sing creates
a real take. Confirm take_state current before completion. A guide voice or stale take is unfinished.
Transcription estimates are drafts. Inspect and explicitly accept them; never invent notes from a filename.

Audio preview/export is not evidence of listening. inspect_audio/analyze measure levels and score data locally.
Deliver verified audio for human feedback; missing subjective feedback does not block delivery.
Compare the same range after a supported edit. After changing an instrument, remeasure its loudness.
Unless the user names other limits, deliver at -14 LUFS integrated under a -1 dBTP ceiling: after the last
sound, note or fader change, call normalize_mix (its defaults are those) and read what it measured; it can stop
short at fader/peak limits. New projects end the master in auris.fx.limiter; keep it last. render exports encoded
WAV or stems; verify_render checks the actual file against requested loudness, peak and optional ending limits.
Treat FAIL as unfinished. Use read_report to page an immutable large report instead of repeating an edit.
Search documentation for application questions. Preserve unrelated score data and report incomplete work honestly.";

/// Shared live/saved guidance for choosing and verifying non-destructive articulations.
pub const PERFORMANCE_INSTRUCTIONS: &str = "\nFor playing-feel edits, inspect_performance first, set only the requested controls, then independently inspect_performance again.
Choose settings.slide for a brief discrete intermediate MIDI note between pitches; it is not continuous pitch bend. It needs an unambiguous single-note line, at least two semitones of movement, no overlap and no gap longer than one quarter note.
Choose settings.pitch.glide_ms for a smooth continuous pitch connection. It follows the final performed notes: isolated notes must last at least 80 ms, the gap must be at most 60 ms and the interval at most 24 semitones. Gate, timing and overlaps can prevent a connection. Repeated pitches need no pitch movement. Do not enable both gestures unless the requested articulation calls for both.
For guitar strokes, settings.strum spreads simultaneous pitches; it cannot turn a single-note arpeggio into chord strokes while preserving the score.
Verify note_effects for strokes and inserted slide notes, and pitch_effects for generated controller changes and the separate glide contribution. Read completeness, effectiveness and warnings, not just stored values. An incomplete measurement is unknown, not success or proof of no effect. Authored bends or another active gesture do not establish that glide contributed.
If a gesture has no effect, explain the reported conditions; do not silently rewrite notes, fill rests or change the instrument to force it. Preserve unrelated performance settings, notes, curves and recipes.
Compare the same short rendered range before and after when verifying audible response. Use local inspect_audio and performance diagnostics for this mechanical check. Read measured gap durations rather than guessing from beat labels. Controller changes do not establish that a hosted patch responds, and changed audio does not prove musical improvement. Never claim to have listened from measurements alone.\n";

/// Shared live/saved workflow for optional mechanical backing around authored parts.
pub const ACCOMPANIMENT_INSTRUCTIONS: &str = "\nAuthor expressive parts note by note with edit_notes: lead/vocals, counterlines, distinctive bass, fills and interludes. Do not replace these choices with rule-based writers, generated scripts or frozen recipes. Reuse and develop explicitly authored motifs.
Use accompaniment only for plain chord comping/held chord pads, simple arpeggios or basic drum pulses. Do not delegate all backing to it; author specific voice leading and rhythmic interplay yourself. An explicit request for automatic preset composition is a separate mode.
When choosing or revising harmony, call suggest_harmony with request {melody_clips,start_bar,bars,mood,ending} using the actual motif clips. Mood is neutral/bright/dark/calm/driving/epic/dreamy/tense/funky; ending is settled/open. Compare its recommended and plain candidates, read per-bar melodic tensions and resolution reasons, and keep simple harmony when colour has no benefit. This read-only step returns harmony objects and a revision for explicit accompaniment regeneration; never apply a proposal just because it contains more extended chords. A score is a heuristic comparison, not evidence of better music. Split requests at meter/key changes.
Inspect the project, explicitly identify protected melody clip IDs and use separate configured backing tracks with written harmony.
Call accompaniment with expected_revision, protected_clips, foreground_clips and operation {kind:generate,parts:[{track,name,part,start_bar,bars,seed,settings}]}.
foreground_clips must be a nonempty subset of protected_clips: choose the actual melody, including hand-written or imported notes. These explicit sources shape chord/arpeggio spacing; inspect each returned adaptation status. Protection alone does not select a musical foreground. Written notes and loops are considered, not performance transforms or sound. Explicit rhythm patterns, driving chord density, sustained ambient textures, bass, pads and drums keep their pattern.
Use recipe controls only for that mechanical backing; use authored note arrays for expressive parts. Drum tracks need assigned sounds first.
For chord changes, operation {kind:regenerate,clips:[...],harmony:{start_bar,bars,key,chords}} changes harmony and only those generated backing clips in one edit; omit harmony after an independent harmony edit.
Keep returned IDs and recipes. Hand-edited or frozen backing requires separate explicit handling.
Split targets at meter/section boundaries; each target is limited to 256 quarter notes and the batch to 1024. Protected and unselected clips remain intact.
Rich chords (7th, sus4, diminished) should serve the motif and harmonic movement; do not impose a complexity quota.
Read compact clip/recipe summaries and inspect_audio on the same bars; structural success alone is not evidence of better music.\n";

/// Canonical session tools for live and saved execution.
pub mod live_agent;

/// Full-text search over the documentation shipped with this build.
pub mod search_documentation {
    /// The tool's name at every door.
    pub const NAME: &str = "search_documentation";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Searches the Auris Studio documentation embedded in this \
        build. Use it for questions about features, workflows, composition, development, \
        evaluation, and singing backends. Returns the most relevant passages with their \
        document paths and section headings.";

    /// A documentation search.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Non-blank words or a short phrase to find in the Auris Studio documentation.
        #[schemars(length(min = 1))]
        pub query: String,
        /// Maximum passages to return. Omitted or null defaults to 5; supplied values are clamped to 1..10.
        pub limit: Option<usize>,
    }

    const DOCUMENTS: &[(&str, &str)] = &[
        (
            "docs/agent-audio-assets.md",
            include_str!("../../../docs/agent-audio-assets.md"),
        ),
        (
            "docs/mixer-graph.md",
            include_str!("../../../docs/mixer-graph.md"),
        ),
        (
            "docs/music-analysis.md",
            include_str!("../../../docs/music-analysis.md"),
        ),
        (
            "docs/agent-workflows.md",
            include_str!("../../../docs/agent-workflows.md"),
        ),
        (
            "docs/agent-accompaniment.md",
            include_str!("../../../docs/agent-accompaniment.md"),
        ),
        (
            "docs/arrangement-harness.md",
            include_str!("../../../docs/arrangement-harness.md"),
        ),
        (
            "docs/melody-breathing-plan.md",
            include_str!("../../../docs/melody-breathing-plan.md"),
        ),
        ("README.md", include_str!("../../../README.md")),
        ("CHANGELOG.md", include_str!("../../../CHANGELOG.md")),
        (
            "docs/features.md",
            include_str!("../../../docs/features.md"),
        ),
        (
            "docs/composition.md",
            include_str!("../../../docs/composition.md"),
        ),
        (
            "docs/development.md",
            include_str!("../../../docs/development.md"),
        ),
        (
            "docs/evaluation.md",
            include_str!("../../../docs/evaluation.md"),
        ),
    ];

    const STOP_WORDS: &[&str] = &[
        "a", "an", "and", "are", "can", "do", "does", "for", "how", "i", "in", "is", "it", "of",
        "on", "the", "to", "what", "when", "where", "with",
    ];

    #[derive(Debug)]
    struct Match<'a> {
        score: usize,
        path: &'a str,
        heading: &'a str,
        passage: String,
    }

    /// Searches the built-in documentation and formats model-readable passages.
    pub fn run(args: &Args) -> Result<String, String> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err("the documentation search needs a non-empty query".to_string());
        }
        let phrase = query.to_lowercase();
        let mut terms: Vec<String> = phrase
            .split(|character: char| {
                !character.is_alphanumeric() && character != '-' && character != '_'
            })
            .filter(|term| {
                !term.is_empty()
                    && (!term.is_ascii() || term.len() > 1)
                    && !STOP_WORDS.contains(term)
            })
            .map(String::from)
            .collect();
        terms.sort();
        terms.dedup();
        if terms.is_empty() {
            terms.push(phrase.clone());
        }

        let mut matches = Vec::new();
        for (path, document) in DOCUMENTS {
            let mut heading = "Overview";
            let mut paragraph = String::new();
            for line in document.lines().chain(std::iter::once("")) {
                if let Some(title) = line.trim_start().strip_prefix('#') {
                    collect(path, heading, &paragraph, &phrase, &terms, &mut matches);
                    paragraph.clear();
                    heading = title.trim_start_matches('#').trim();
                } else if line.trim().is_empty() {
                    collect(path, heading, &paragraph, &phrase, &terms, &mut matches);
                    paragraph.clear();
                } else {
                    if !paragraph.is_empty() {
                        paragraph.push('\n');
                    }
                    paragraph.push_str(line);
                }
            }
        }

        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(right.path))
                .then_with(|| left.heading.cmp(right.heading))
        });
        matches.dedup_by(|left, right| {
            left.path == right.path
                && left.heading == right.heading
                && left.passage == right.passage
        });
        matches.truncate(args.limit.unwrap_or(5).clamp(1, 10));

        if matches.is_empty() {
            return Ok(format!(
                "No Auris Studio documentation matched `{query}`. Try fewer or more specific terms."
            ));
        }
        let mut answer = format!("Auris Studio documentation matches for `{query}`:\n");
        for (index, found) in matches.iter().enumerate() {
            answer.push_str(&format!(
                "\n{}. {} — {}\n{}\n",
                index + 1,
                found.path,
                found.heading,
                found.passage
            ));
        }
        Ok(answer)
    }

    fn collect<'a>(
        path: &'a str,
        heading: &'a str,
        paragraph: &str,
        phrase: &str,
        terms: &[String],
        matches: &mut Vec<Match<'a>>,
    ) {
        let passage = paragraph.trim();
        if passage.is_empty() {
            return;
        }
        let heading_lower = heading.to_lowercase();
        let passage_lower = passage.to_lowercase();
        let mut score = usize::from(heading_lower.contains(phrase)) * 120
            + usize::from(passage_lower.contains(phrase)) * 80;
        let mut matched_terms = 0;
        for term in terms {
            let in_heading = heading_lower.matches(term).count().min(3);
            let in_passage = passage_lower.matches(term).count().min(5);
            if in_heading + in_passage > 0 {
                matched_terms += 1;
                score += in_heading * 20 + in_passage * 4;
            }
        }
        let needed = terms.len().div_ceil(2);
        if matched_terms < needed {
            return;
        }
        score += matched_terms * 10;
        matches.push(Match {
            score,
            path,
            heading,
            passage: shorten(passage, 900),
        });
    }

    fn shorten(text: &str, limit: usize) -> String {
        let mut shortened: String = text.chars().take(limit).collect();
        if text.chars().count() > limit {
            shortened.push('…');
        }
        shortened
    }
}

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
        const DARK_SONG: &str = include_str!("../../../examples/dark-song.asong");
        const NEON_DRIVE: &str = include_str!("../../../examples/neon-drive.asong");
        format!(
            "A specification is TOML; every field has a default, so start small and only say \
             what should differ. Start from an idea:\n\n{DARK_SONG}\n\n\
             tonality: auto, major, minor. pace: auto, slow, moderate, fast. \
             sound: auto, major, minor, dorian, lydian, mixolydian, phrygian. \
             Explicit sound overrides tonality; when both are auto, mood chooses the scale: \
             bright=major, dark=minor, epic=dorian, dreamy=lydian, funky=mixolydian, tense=phrygian. \
             Explicit key/scale and tempo override these choices. \
             style supplies instruments and form, with newly generated chords; use chords \
             to specify your own progression. writing_style selects the score vocabulary \
             independently of performance, which only sets editable playback transforms. \
             Both accept chiptune, pop-band, city-pop, rock, jazz-trio, orchestral, synthwave \
             or ambient; style presets select both automatically. Saved specs contain the \
             resolved settings. Lyrics stay inside each section's bars: every mora is kept \
             and impossible density is rejected. melody_from shares exact vocal slots.\n\n\
             The smallest useful song:\n\n{HELLO}\n\nAnd most of the \
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
        see what a draft actually means. This checks syntax/settings; use diagnose_song for vocal lyric fit and preview_composition for actual bar count and merged tracks before compose.";

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
        measured to — and the seed is what to pin in the spec to ask for this exact take again. \
        For repeating game BGM, use preset game-loop or set ending: loop; the complete form \
        becomes the enabled cycle region.";

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
        cancellation::begin_commit()?;
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
        clip, and ask again. Large results return an immutable report_id snapshot; use read_report for details instead of repeating analysis or edits.";

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
        reports::publish_text(text.trim_end().to_string())
    }
}

/// One section, held at a level.
pub mod section_gain {
    use super::*;

    /// The tool's name at every door.
    pub const NAME: &str = "section_gain";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Holds a track's gain at a level across one named section — \
        dynamics without rewriting a note. Omit `track` for the master; the section is addressed \
        by the label `analyze` shows, every occurrence unless `instance` picks one. Writes gain \
        automation with short ramps at the edges: the fader keeps ruling outside the stretch, \
        and holds on different sections compose. `clear: true` removes the track's whole gain \
        lane instead, giving the fader back everywhere. The change is saved. The master fader \
        sits after the master chain, so a boost there is not limited and can clip — widen \
        contrast by holding the louder sections down instead. Use gain_delta_db instead of gain_db to offset the existing envelope; automation reads it back.";

    /// Arguments to `section_gain`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// Stable numeric track ID from inspect_project; omit for the master.
        pub track: Option<u64>,
        /// The section to hold, by label as `analyze` shows it.
        pub section: Option<String>,
        /// Which occurrence of the label, counting from 1 — every one when left out.
        pub instance: Option<usize>,
        /// The level to hold in decibels, within the session's native gain bounds.
        pub gain_db: Option<f32>,
        /// Relative dB change to the existing envelope; preserves its shape. Use instead of gain_db.
        pub gain_delta_db: Option<f32>,
        /// Remove the track's gain lane instead, giving the fader back everywhere.
        #[serde(default)]
        pub clear: bool,
    }

    /// Holds the span — or takes the whole lane back.
    pub fn run(args: &Args) -> Result<String, String> {
        if (args.gain_db.is_some() && args.gain_delta_db.is_some())
            || (args.clear && (args.gain_db.is_some() || args.gain_delta_db.is_some()))
        {
            return Err("choose gain_db, gain_delta_db or clear, not a combination".into());
        }
        let mut session = opened(&args.project)?;
        let strip = args.track.map(TrackId);
        let target = match strip {
            Some(id) => ParamTarget::TrackGain(id),
            None => ParamTarget::MasterGain,
        };
        let label = strip.map_or_else(|| "master".to_owned(), |id| format!("track {}", id.0));
        let descriptor = session
            .descriptor_for(target)
            .ok_or("Unknown track ID; read inspect_project")?;

        if args.clear {
            if !session.clear_automation(target) {
                return Err(format!("nothing is automated on '{label}'"));
            }
            save_checkpointed(&mut session)?;
            return Ok(format!(
                "The gain lane on '{}' is gone; the fader rules everywhere again. Saved.",
                label
            ));
        }
        let Some(label) = &args.section else {
            return Err("pass `section` and `gain_db`, or clear: true".into());
        };
        let Some(gain) = args.gain_db.or(args.gain_delta_db) else {
            return Err(
                "pass gain_db for an absolute level, or gain_delta_db for a relative change".into(),
            );
        };
        if !gain.is_finite()
            || (args.gain_db.is_some() && !(descriptor.min..=descriptor.max).contains(&gain))
        {
            return Err(format!(
                "gain_db must be within {}..{} dB; gain_delta_db must be finite",
                descriptor.min, descriptor.max
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
            if args.gain_delta_db.is_some() {
                if !session.offset_gain_range(target, *start, *end, gain) {
                    return Err("relative gain would exceed the gain limits or the range is invalid; no changes were saved".into());
                }
            } else {
                session.hold_automation(target, *start, *end, gain);
            }
            let which = match instance {
                1 => label.clone(),
                instance => format!("{label} ({instance})"),
            };
            let operation = if args.gain_delta_db.is_some() {
                "offset by"
            } else {
                "held at"
            };
            text.push_str(&format!(
                "{which} bars {first_bar}-{last_bar}: '{}' {operation} {gain:+.1} dB.\n",
                strip.map_or_else(|| "master".to_owned(), |id| format!("track {}", id.0))
            ));
        }
        save_checkpointed(&mut session)?;
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
        if !auris_session::progressions::ProgressionBook::keep_saved(&args.name, &chart, mode)
            .map_err(|error| error.to_string())?
        {
            return Err(format!(
                "'{}' cannot be kept under that name — it is empty, or the built-in catalogue \
                 already uses it",
                args.name
            ));
        }
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
        if !auris_session::progressions::ProgressionBook::forget_saved(&args.name)
            .map_err(|error| error.to_string())?
        {
            return Err(format!("nothing is kept under '{}'", args.name));
        }
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
        draft to argue with — `edit_recipe` re-derives any part after a correction. The \
        change is saved.";

    /// Arguments to `accompany`.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// The project to change — an absolute path to a `.auris` file.
        pub project: String,
        /// Stable numeric melody clip ID from inspect_project.
        pub clip: u64,
        /// The parts to write: lead, chords, pad, arp, bass, drums, kick, snare or hat.
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
        let id = ClipId(args.clip);
        let report = session
            .accompany(id, &parts, args.seed.unwrap_or(0))
            .map_err(|error| error.to_string())?;
        save_checkpointed(&mut session)?;

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
            args.clip,
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
             ear: `analyze`, then `edit_recipe` on any part after fixing what it follows.",
        );
        Ok(text)
    }
}

/// Saves one headless edit only after this request wins its cancellation/commit race.
fn save_checkpointed(session: &mut Session) -> Result<(), String> {
    cancellation::begin_commit()?;
    session
        .save_with_checkpoint()
        .map_err(|error| error.to_string())
}

/// A session with no audio device and no GPU, with the shipped SoundFonts.
///
/// Loads the same font library used by the desktop application.
fn headless() -> Result<Session, String> {
    cancellation::check()?;
    let mut options = SessionOptions::headless()
        .with_shipped_fonts(true)
        .with_soundfont_library(auris_session::config_dir().join("soundfonts.json"))
        .with_shipped_dictionary(true)
        // A new project made here is the window's new project, limiter and all.
        .with_master_limiter(true)
        // The plugin folders the window uses, so a project opened here loads the same
        // plugins and no others.
        .with_plugin_paths(auris_session::Settings::load().plugin_paths);
    if cfg!(test) {
        options.soundfont_library = None;
    }
    Session::new(options).map_err(|error| error.to_string())
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
    if path.starts_with("p:") && !Path::new(path).is_absolute() {
        return project_handles::resolve(path);
    }
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
    cancellation::check()?;
    Ok(session)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documentation_search_finds_a_named_feature_and_limits_its_answer() {
        let answer = search_documentation::run(&search_documentation::Args {
            query: "Dragging files in".to_string(),
            limit: Some(2),
        })
        .unwrap();
        assert!(answer.contains("docs/features.md"), "{answer}");
        assert!(answer.contains("Dragging files in"), "{answer}");
        assert!(
            !answer.contains("\n3. "),
            "the requested limit is observed: {answer}"
        );
    }

    #[test]
    fn documentation_search_refuses_an_empty_query() {
        let error = search_documentation::run(&search_documentation::Args {
            query: "  ".to_string(),
            limit: None,
        })
        .unwrap_err();
        assert!(error.contains("non-empty"), "{error}");
    }

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
    fn checking_and_composing_refuse_ignored_source_controls_before_writing() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("Untouched.auris");
        let text = "[[part]]\nname = 'Keys'\nsource = { type = 'clap', path = 'Missing.clap', plugin_id = 'keys' }\nparams = { 'clap.18' = 0.42 }";
        let checked = check_spec::run(&spec_args(Some(text), None)).unwrap_err();
        let composed = compose::run(&compose::Args {
            spec: spec_args(Some(text), None),
            output: output.to_string_lossy().into_owned(),
            force: false,
        })
        .unwrap_err();
        assert_eq!(checked, composed);
        assert!(checked.contains("Keys"), "{checked}");
        assert!(checked.contains("clap.18"), "{checked}");
        assert!(checked.contains("set_instrument_param"), "{checked}");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
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
}

/// Model-facing audio inspection images and captions.
pub mod audio_inspection;
