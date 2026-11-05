---
name: auris-composition
description: Compose, arrange, synthesize vocals, render, and verify music with Auris Studio for original songs with lyrics or game/video background music, themes, cues, and stems. Use the installed Auris CLI/MCP tools; do not activate for unrelated audio editing or music discussion.
---

# Auris Composition

Use this skill for original songs with lyrics and synthesized singing, or when a game or video request needs original music, a musical cue, a looping background track, a transition, or delivery stems. Decide to invoke Auris when the request benefits from an editable score and repeatable renders, even if the user did not name Auris. State the musical role, duration, loop/ending behavior, delivery format, and any constraints before writing.

## Choose the Auris door

- Prefer the live Auris MCP tools when a desktop session is open and the result belongs in the current project. Keep edits in the Auris session so undo, revisions, and the open document remain coherent.
- Use `auris` for headless or scripted work. Use MCP note-editing tools to author expressive parts and `auris render` for WAV/stems. Reserve `auris compose` for an explicit request for automatic preset/recipe composition; it is not the default way to write an original melody. Look for the executable on PATH, then in the installed app: `/Applications/Auris Studio.app/Contents/MacOS/` or `~/Applications/Auris Studio.app/Contents/MacOS/` on macOS, and `%LOCALAPPDATA%\Programs\Auris Studio\` on Windows. Those directories contain `auris` and `auris-mcp` (with `.exe` on Windows). Do not auto-install, download, or replace binaries during a creative task; if neither is present, direct the user to the Auris release page.
- Use `auris-mcp` when the caller already provides an MCP server configuration. Its saved-project catalog includes specification and composition tools. The desktop's live agent has a separate catalog and may omit `compose`, `spec_reference`, `check_spec`, `list_presets`, and `list_progressions`; discover the live names before calling them.

Use the existing Codex or Claude Code environment and installed Auris tools for the complete workflow. Inspect notes, render, measure, and make justified musical repairs before delivery. Do not install, discover, or invoke external music evaluators as part of composing. Use available Auris instruments; synthesized singing additionally needs a suitable installed voice.

## Before the first phase

1. Translate the brief into a short musical contract: purpose, emotional arc, reference-free style words, tempo, key or mode, duration in bars/seconds, instrumentation, loop points, and deliverables. Ask only for missing decisions that materially change the result; otherwise choose conservative defaults and record them.
2. If an existing project is in scope, inspect it first and use its stable project/track/clip IDs plus any revision fields required by the chosen operation. Add a new cue track or clip when the request is additive; do not use `auris compose` to replace the project behind the user's back. Check for existing names, occupied ranges, and routing collisions before writing. For an empty score with no clips, `create_project` is sufficient to start; it creates a default instrument track. Use `setup_tracks` when several named tracks should be created atomically.
3. Convert requested seconds to an integer bar count at the chosen tempo and meter. If (for example) a 45-second loop cannot land exactly on a bar boundary, choose the nearest musical length or adjust tempo and report the tradeoff. Make loop start/end explicit.

Then work in three phases. Each phase fixes what the earlier ones decided: change it only by going back to the phase that owns it, because the user asked or a concrete error requires it. A local fix is done in the phase that owns what is being fixed, inside the requested scope. Read [workflow.md](references/workflow.md) when choosing a command or constructing a specification. Each phase ends with a short technique index; `search_documentation` (in the MCP and live tool catalogs) with a technique's name returns how to make that move with the Auris tools.

## Phase 1: score

Decide what the song is: form, tempo and meter, key and chords per section, lyrics, the foreground melody, and its voice.

- For a new vocal song, establish lyrics and word groups, then each section's chords and form, before placing clips. An instrumental starts with form and harmony; honor an explicitly motif-first or groove-first brief. Write tempo/meter and named section ranges with their harmony into the document, not only into audible notes.
- Read [melody-design.md](references/melody-design.md) before writing the foreground. Establish a short identity-bearing phrase and its intended landing before expanding the form; a scale or chord-tone list alone is not a melodic plan. Make the chorus arrive: higher on average than the section before it, the song's highest notes, a hook that leaps.
- Use `read_notes` before `edit_notes`. Choose pitch, onset, duration and velocity yourself for the melody. Do not use rule-based melody generators, procedural note-writing scripts or frozen recipes as a substitute. Code may serialize explicitly authored notes or copy an authored motif.
- For songs with lyrics or synthesized singing, read [vocals.md](references/vocals.md) before writing. It covers lyric/melody fit, complete song form, voice prerequisites, synthesis and checking every sung section. Sing the melody in this phase, so the demo can be judged with its words.
- Add one plain guide part (piano or pad chords) so the score can be heard. If the user explicitly requests preset generation, inspect available presets and progressions before inventing names: in saved-project MCP use `list_presets`, optionally `list_progressions`, and `check_spec` before `compose`; in the CLI use `auris presets`/`auris progressions` and `auris compose <song.asong> --print`. Keep a custom `.asong` source beside the project; record the preset and seed. In a live session call `discover_tools` and `tool_help` and use the advertised tools.
- Done when sections cover the song, every section has harmony, the foreground is written, and every sung section has a current take. For a new song, offer the demo to the user before arranging unless they asked for everything at once.

Techniques: Section roles; Chorus lift; Motif repetition and development; The chorus hook in the intro; Drop chorus (落ちサビ); Last-chorus key change (ラスサビ転調); Singable range.

## Phase 2: arrangement

Dress the fixed score: sounds, backing parts, drums, performance and transitions. The foreground's notes, lyrics and timing, the harmony, the sections and the tempo map stay as the score left them.

- Follow [arrangement.md](references/arrangement.md) and [arrangement-design.md](references/arrangement-design.md). Perform a deliberate arrangement pass even when `compose` has already generated backing and a mix; a generated accompaniment is a starting point, not proof the pass is complete.
- Choose each track's sound for its role. Decide which parts support or answer the foreground, give each part its own register, introduce purposeful entrances, exits and handoffs, add SFX only when they serve the scene or transition, and shape section contrasts and transitions.
- Use `accompany`/`generate_phrase` only for plain chord comping or held pads, simple arpeggios and basic drum pulses; write counterlines, distinctive bass, fills and backing vocals note by note, and inspect what generators wrote. Use performance controls for feel without rewriting notes. Swing is the song's feel, not seasoning: a straight song stays straight, and a swung song swings every part alike, vocal included.
- Apply justified changes, read them back, and compare affected passages with the earlier version. Replace or remove the guide part. If the arrangement already serves the brief, record the concrete reasons instead of forcing changes. A rough fader balance is fine here; processing belongs to the mix.

Techniques: Part roles and registers; Voicings out of the voice; Kick and bass together; Fills and crashes at transitions; Build-up and break; More in verse 2; Copy, then vary; Performance controls; Generators for scaffolding, authored notes for what is remembered.

## Phase 3: mix and delivery

Balance the finished arrangement and deliver it. Notes, sounds and performance stay as the arrangement left them.

- Read [delivery.md](references/delivery.md) when the request includes game-loop integration, video timing, stems, or loudness requirements.
- Finish the mix with the actual synthesized voice; preview-voice levels are provisional. Adjust track levels, panning and competing registers for foreground intelligibility first, then use supported EQ, dynamics and reverb/delay sends each for a stated purpose. Master normalization alone is not track balancing. If the existing balance works, record the concrete evidence rather than forcing processing.
- Check transitions, tails and headroom, and verify the full mix after the last edit. A preview is a quick structural/listening candidate; a final delivery requires a full render and `verify_render` (or equivalent measurement) against the requested duration, peak, loudness, and ending. Use Auris's local `inspect_audio`/`analyze` for measurable checks. If your model takes audio input, `listen` delivers rendered bars to you to hear, and `compare_to` gives before and after; if you cannot hear audio, do not call it, and never claim to have heard what did not reach you. A successful render or numeric report is not proof that the cue is musically suitable; deliver the audio for the user's listening, and do not let missing subjective feedback block delivery.
- Deliver the requested audio plus the editable project/source, settings, seed, loop/trim notes, and a concise verification report including meaningful arrangement decisions. Keep generated files in the user-requested project directory. Never overwrite unrelated assets.

Techniques: Volume and pan first; Low cut on everything but the bass; Kick-triggered sidechain; Reverb and delay sends for the voice; Bus processing; Section automation; normalize_mix last.

## Boundaries

- Do not silently replace a user's authored melody, lyrics, harmony, or project structure. Preview or create a new clip/track for alternatives; apply a destructive replacement only when explicitly requested.
- Do not claim that a cue loops, is loudness-compliant, or is synchronized until the corresponding boundary or measurement was checked. Report unknowns honestly.
- Keep randomness reproducible with an explicit seed. Regeneration is a new take; preserve the prior take until the user chooses one.
- Prefer the Auris project/render path for sound generation. Do not fabricate a successful tool call, audio file, instrument, plugin, or listening result.
- If Auris or its requested backend is unavailable, explain the missing capability and offer an editable fallback rather than silently switching providers.
