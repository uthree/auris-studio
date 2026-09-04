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

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};

/// The server. Stateless on purpose — the state lives in project files, and
/// [`auris_toolbox`]'s account of the design says why.
#[derive(Clone, Copy, Debug, Default)]
struct AurisMcp;

// Every method is one tool: the method name is the tool's wire name, the doc comment its
// description (held equal to the toolbox constant by test), and argument type and work both
// come from the toolbox — this list is the door, not the furniture.
#[tool_router]
impl AurisMcp {
    /// The `.asong` format, taught by example: a two-line song, then a specification using
    /// most of the vocabulary with a comment on every field. Read this before writing a spec.
    #[tool]
    async fn spec_reference(&self) -> Result<CallToolResult, ErrorData> {
        finished(Ok(toolbox::spec_reference::run()))
    }

    /// Validates a specification without composing anything. A rejected spec answers with
    /// every complaint at once, line numbers where they exist; a valid one answers with the
    /// full document, every default filled in — the cheap way to see what a draft actually
    /// means.
    #[tool]
    async fn check_spec(
        &self,
        Parameters(args): Parameters<toolbox::check_spec::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        // No session and no blocking work — parsing a spec is pure text.
        finished(toolbox::check_spec::run(&args))
    }

    /// Composes a song from a specification and saves it as a project. The answer reports
    /// what was written — tracks, notes, seed, where the mix was measured to — and the seed
    /// is what to pin in the spec to ask for this exact take again.
    #[tool]
    async fn compose(
        &self,
        Parameters(args): Parameters<toolbox::compose::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::compose::run(&args)).await
    }

    /// Renders a project to a WAV file — or, with `stems`, to one file per track — and
    /// reports each file's length, channels and peak level.
    #[tool]
    async fn render(
        &self,
        Parameters(args): Parameters<toolbox::render::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::render::run(&args)).await
    }

    /// Describes a project on disk: tempo, meter, duration, and every track with its
    /// instrument, clip count, effects and routing.
    #[tool]
    async fn describe(
        &self,
        Parameters(args): Parameters<toolbox::describe::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::describe::run(&args)).await
    }

    /// Listens to a project and reports what it measured, changing nothing: length,
    /// integrated loudness and peaks for the whole mix, the same per named section — the
    /// piece's dynamic arc as numbers — and, with `per_track`, each track alone. This is the
    /// ears of the improve loop: render, analyze, edit the spec or rewrite one clip, and ask
    /// again.
    #[tool]
    async fn analyze(
        &self,
        Parameters(args): Parameters<toolbox::analyze::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::analyze::run(&args)).await
    }

    /// Reads the mixer as it stands: every track's fader, pan, mute and solo, its sends, and
    /// each effect's parameters with key, value and range — the vocabulary `set_level`,
    /// `set_send` and `set_effect` move. A control marked `[automated]` is driven by its lane,
    /// not its stored value.
    #[tool]
    async fn mixer(
        &self,
        Parameters(args): Parameters<toolbox::mixer::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::mixer::run(&args)).await
    }

    /// Sets a track's fader and/or pan; `track` may be "master". Gain runs -60 to +12 dB, pan
    /// -1 (left) to +1 (right). The change is saved — `analyze` again to hear what it did to
    /// the numbers. A fader that `mixer` marks `[automated]` is ruled by its lane, not this
    /// value; `section_gain` with clear: true removes the lane.
    #[tool]
    async fn set_level(
        &self,
        Parameters(args): Parameters<toolbox::set_level::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_level::run(&args)).await
    }

    /// Sets how much of a track one of its sends carries, addressed by the bus it feeds — the
    /// routing `mixer` and `describe` show. Send levels run -60 to 0 dB; there is no headroom
    /// above unity on a send. The change is saved.
    #[tool]
    async fn set_send(
        &self,
        Parameters(args): Parameters<toolbox::set_send::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_send::run(&args)).await
    }

    /// Sets one parameter of one effect, addressed the way `mixer` lists them: `track` (or
    /// "master"), the effect by its id — or by `slot`, its 1-based position, when a chain
    /// holds the same effect twice — and the parameter by key or name, in the parameter's own
    /// units. Values outside the range `mixer` shows are refused. The change is saved. The
    /// master limiter's `input_db` is the dial to back off when `analyze` says the loud
    /// sections are pinned against the ceiling.
    #[tool]
    async fn set_effect(
        &self,
        Parameters(args): Parameters<toolbox::set_effect::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_effect::run(&args)).await
    }

    /// Holds a track's gain at a level across one named section — dynamics without rewriting
    /// a note. `track` may be "master"; the section is addressed by the label `analyze`
    /// shows, every occurrence unless `instance` picks one. Writes gain automation with short
    /// ramps at the edges: the fader keeps ruling outside the stretch, and holds on different
    /// sections compose. `clear: true` removes the track's whole gain lane instead, giving
    /// the fader back everywhere. The change is saved. The master fader sits after the master
    /// chain, so a boost there is not limited and can clip — widen contrast by holding the
    /// louder sections down instead.
    #[tool]
    async fn section_gain(
        &self,
        Parameters(args): Parameters<toolbox::section_gain::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::section_gain::run(&args)).await
    }

    /// Writes another take of a generated clip: the same ask, the next seed, different notes.
    /// The change is saved into the project — render again to hear it. Aim it with `track`
    /// and the clip number `describe` shows; without a number, every generated clip on the
    /// track gets a new take. Every answer names its seed, and passing `seed` takes that
    /// exact take again — how a rewrite that measured worse is rolled back.
    #[tool]
    async fn another_take(
        &self,
        Parameters(args): Parameters<toolbox::another_take::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::another_take::run(&args)).await
    }

    /// Writes a generated clip again with its own seed, following the key and chords as they
    /// stand now — the tool to reach for after changing the harmony under an existing piece.
    /// The change is saved into the project. Addressed exactly like `another_take`.
    #[tool]
    async fn write_again(
        &self,
        Parameters(args): Parameters<toolbox::write_again::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::write_again::run(&args)).await
    }

    /// Keeps a chord progression under a name on this machine. It then shows up in
    /// `list_progressions` and the desktop picker; a specification still writes the chords
    /// out in full — only the built-in catalogue is quotable as `@name`, so a document stays
    /// portable.
    #[tool]
    async fn teach_progression(
        &self,
        Parameters(args): Parameters<toolbox::teach_progression::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::teach_progression::run(&args)).await
    }

    /// Forgets a progression kept with `teach_progression`, by name.
    #[tool]
    async fn forget_progression(
        &self,
        Parameters(args): Parameters<toolbox::forget_progression::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::forget_progression::run(&args)).await
    }

    /// Lists the chord progressions a specification can quote by name, with the chords each
    /// one plays.
    #[tool]
    async fn list_progressions(&self) -> Result<CallToolResult, ErrorData> {
        blocking(move || Ok(toolbox::list_progressions::run())).await
    }

    /// Lists the whole songs a specification can start from, with each one's key, tempo
    /// and groove.
    #[tool]
    async fn list_presets(&self) -> Result<CallToolResult, ErrorData> {
        finished(Ok(toolbox::list_presets::run()))
    }

    /// Lists the built-in instruments a track can play, by the id `add_track` and
    /// `set_instrument` take. Any General MIDI sound is also available — name it in those
    /// tools' `sound` field instead, as a GM name or program number.
    #[tool]
    async fn list_instruments(&self) -> Result<CallToolResult, ErrorData> {
        blocking(move || Ok(toolbox::list_instruments::run())).await
    }

    /// Adds a track to an existing project and saves. An instrument track by default — voiced
    /// by `instrument` (an id from `list_instruments`) or by `sound` (a General MIDI name or
    /// program number, `drums: true` for a kit) — or, with `kind`, a singer track (notes that
    /// carry lyrics, sung by a voice model), an audio track or a bus. A new instrument track
    /// has no clips: `add_part` writes one.
    #[tool]
    async fn add_track(
        &self,
        Parameters(args): Parameters<toolbox::add_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_track::run(&args)).await
    }

    /// Writes a generated part onto an existing instrument track, from the key and chords
    /// already under the song — lead, chords, pad, arp, bass, stab, drums, kick, snare or
    /// hat. Covers the whole song unless `start_bar` and `bars` aim it. The clip keeps its
    /// recipe, so `another_take` rerolls it and `write_again` follows a harmony change; the
    /// answer numbers it the way `describe` does.
    #[tool]
    async fn add_part(
        &self,
        Parameters(args): Parameters<toolbox::add_part::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_part::run(&args)).await
    }

    /// Re-voices an instrument track: `instrument` names a built-in from `list_instruments`,
    /// or `sound` names a General MIDI sound (a name or a program number, `drums: true` for a
    /// kit). The previous instrument's dial positions and the automation that drove them go
    /// with it. The change is saved.
    #[tool]
    async fn set_instrument(
        &self,
        Parameters(args): Parameters<toolbox::set_instrument::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::set_instrument::run(&args)).await
    }

    /// Renames a track. Every other tool addresses tracks by name, so the new name is the
    /// address from here on. The change is saved.
    #[tool]
    async fn rename_track(
        &self,
        Parameters(args): Parameters<toolbox::rename_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::rename_track::run(&args)).await
    }

    /// Removes a track and everything on it — its clips, its effect chain, its sends and its
    /// automation. The change is saved.
    #[tool]
    async fn remove_track(
        &self,
        Parameters(args): Parameters<toolbox::remove_track::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::remove_track::run(&args)).await
    }

    /// Opens an empty clip on an instrument or singer track, for `edit_notes` to write into —
    /// the way a melody is placed note by note. Aim it with `start_bar` and `bars`; the answer
    /// numbers the clip the way `describe` does.
    #[tool]
    async fn add_clip(
        &self,
        Parameters(args): Parameters<toolbox::add_clip::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::add_clip::run(&args)).await
    }

    /// Reads one clip's notes, numbered in time order — pitch, bar, beat, length in beats,
    /// velocity and, where a note carries one, its lyric. The numbers are the address
    /// `edit_notes` removes and `write_lyrics` starts by; aim with `track` and the clip number
    /// `describe` shows.
    #[tool]
    async fn notes(
        &self,
        Parameters(args): Parameters<toolbox::notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::notes::run(&args)).await
    }

    /// Adds and removes notes in one clip, in one call: `remove` takes the numbers `notes`
    /// lists, `add` takes notes as pitch (a name like "F#4" or a MIDI number), 1-based bar and
    /// beat in the song, length in beats, and velocity 0-1 (0.75 when left out). Removals
    /// happen first. The change is saved. On a generated clip the edit sticks until
    /// `another_take` or `write_again` rewrites the clip whole.
    #[tool]
    async fn edit_notes(
        &self,
        Parameters(args): Parameters<toolbox::edit_notes::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::edit_notes::run(&args)).await
    }

    /// Reads a melody clip and writes a key, a chord progression and backing tracks under it —
    /// the melody-first way around: place the tune with `edit_notes`, then derive the band.
    /// The melody itself is not touched. `parts` picks the band (bass, chords and drums when
    /// left out); the harmony it writes is a first draft to argue with — `write_again`
    /// re-derives any part after a correction. The change is saved.
    #[tool]
    async fn accompany(
        &self,
        Parameters(args): Parameters<toolbox::accompany::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::accompany::run(&args)).await
    }

    /// Lays a phrase across a singer clip's notes, one syllable to each, and derives the
    /// phonemes it will be sung as — kana through the built-in table, other text through the
    /// Japanese dictionary where one is installed. `from` starts partway in, at a number the
    /// way `notes` counts them, so a verse is filled one line at a time; notes past the end of
    /// the phrase keep their words. The change is saved.
    #[tool]
    async fn write_lyrics(
        &self,
        Parameters(args): Parameters<toolbox::write_lyrics::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::write_lyrics::run(&args)).await
    }

    /// Renders a singer track through its voice model and keeps the audio as the track's
    /// take, which is what playback and `render` then play. Aims at the project's only singer
    /// track when `track` is left out. `voice` chooses a model the first time — an absolute
    /// path to an exported `.onnx` voice, which the track keeps. A take is deterministic: the
    /// same notes, lyrics, voice and `seed` render the same audio, and another seed is
    /// another take. The change is saved.
    #[tool]
    async fn sing(
        &self,
        Parameters(args): Parameters<toolbox::sing::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::sing::run(&args)).await
    }

    /// Writes a song from Japanese lyrics and saves it as a new project: a melody searched
    /// under the words the Orpheus way, sung notes carrying each syllable, chords in the
    /// harmony lane, and a backing band unless `melody_only`. Where a Japanese dictionary is
    /// configured the melody follows the lyric's pitch accent; kana lyrics work without one,
    /// free of the accent. Phrases break at line breaks and punctuation. The same lyrics and
    /// `seed` write the same song; `sing` then gives the vocal its voice.
    #[tool]
    async fn compose_lyrics(
        &self,
        Parameters(args): Parameters<toolbox::compose_lyrics::Args>,
    ) -> Result<CallToolResult, ErrorData> {
        blocking(move || toolbox::compose_lyrics::run(&args)).await
    }
}

#[tool_handler]
impl ServerHandler for AurisMcp {
    fn get_info(&self) -> ServerInfo {
        // Field by field because the type is `non_exhaustive`, which rules the literal out.
        // Named explicitly rather than via `Implementation::from_build_env`, whose `env!` was
        // expanded when *rmcp* was compiled — a server introducing itself as "rmcp 3.1.4".
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.instructions = Some(toolbox::INSTRUCTIONS.into());
        info
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
    // Stderr, and only stderr: stdout is the protocol channel, and one stray line on it is a
    // broken connection.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Like the CLI: nothing here reads the configuration, but this may still be the frontend
    // that runs first on a machine, and an installation predating the move to
    // `~/.config/auris-studio` only has its settings carried across by whichever one does.
    auris_session::migrate_legacy_config();

    tokio::runtime::Runtime::new()?.block_on(async {
        let service = AurisMcp.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_introduces_itself_and_carries_the_shared_instructions() {
        let info = AurisMcp.get_info();
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
        use toolbox::{
            accompany, add_clip, add_part, add_track, analyze, another_take, check_spec, compose,
            compose_lyrics, describe, edit_notes, forget_progression, list_instruments,
            list_presets, list_progressions, mixer, notes, remove_track, rename_track, render,
            section_gain, set_effect, set_instrument, set_level, set_send, sing, spec_reference,
            teach_progression, write_again, write_lyrics,
        };
        let expected: std::collections::BTreeMap<&str, &str> = [
            (spec_reference::NAME, spec_reference::DESCRIPTION),
            (check_spec::NAME, check_spec::DESCRIPTION),
            (compose::NAME, compose::DESCRIPTION),
            (render::NAME, render::DESCRIPTION),
            (describe::NAME, describe::DESCRIPTION),
            (analyze::NAME, analyze::DESCRIPTION),
            (mixer::NAME, mixer::DESCRIPTION),
            (set_level::NAME, set_level::DESCRIPTION),
            (set_send::NAME, set_send::DESCRIPTION),
            (set_effect::NAME, set_effect::DESCRIPTION),
            (section_gain::NAME, section_gain::DESCRIPTION),
            (another_take::NAME, another_take::DESCRIPTION),
            (write_again::NAME, write_again::DESCRIPTION),
            (teach_progression::NAME, teach_progression::DESCRIPTION),
            (forget_progression::NAME, forget_progression::DESCRIPTION),
            (list_progressions::NAME, list_progressions::DESCRIPTION),
            (list_presets::NAME, list_presets::DESCRIPTION),
            (list_instruments::NAME, list_instruments::DESCRIPTION),
            (add_track::NAME, add_track::DESCRIPTION),
            (add_part::NAME, add_part::DESCRIPTION),
            (set_instrument::NAME, set_instrument::DESCRIPTION),
            (rename_track::NAME, rename_track::DESCRIPTION),
            (remove_track::NAME, remove_track::DESCRIPTION),
            (add_clip::NAME, add_clip::DESCRIPTION),
            (notes::NAME, notes::DESCRIPTION),
            (edit_notes::NAME, edit_notes::DESCRIPTION),
            (accompany::NAME, accompany::DESCRIPTION),
            (write_lyrics::NAME, write_lyrics::DESCRIPTION),
            (sing::NAME, sing::DESCRIPTION),
            (compose_lyrics::NAME, compose_lyrics::DESCRIPTION),
        ]
        .into_iter()
        .collect();

        let served = AurisMcp::tool_router().list_all();
        assert_eq!(served.len(), expected.len(), "thirty tools at this door");
        for tool in served {
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
