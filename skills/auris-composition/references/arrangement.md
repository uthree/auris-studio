# Arrangement after composition

Use this pass for complete songs and instrumental cues after the core melody, lyrics where applicable, harmony, and form exist. Composing and arranging may overlap, but revisit the actual score before final synthesis/export. Author counterlines, distinctive bass, interludes and fills note by note; reserve writers for plain chord/pad backing, simple arpeggios and basic drum pulses. The CLI specification examples below apply to an explicit automatic-generation request. A preset's track count, automatic balance, or successful render does not establish an intentional arrangement. Scale the work to the brief; do not expand a local edit or a short demo into a full production.

## Establish what the arrangement must serve

Save a baseline project/specification and a short preview when available. Inspect actual notes, sound selections, section boundaries, and the requested loop/ending. Identify the foreground and protected material. Briefly map each section's foreground, accompaniment roles, register, density, and entrance/exit. Keep this in `production_record` when available or in concise delivery notes. This is a working musical plan, not a requirement to fill every role in every section.

For each proposed change, name its purpose and range: for example, leave space for verse lyrics, make the refrain open up, or prepare the loop return. Keep recognizable hooks, authored melody/lyrics, and intentional harmonic tensions. Reharmonization or replacing the main theme is a separate creative choice, not an automatic consequence of arranging.

Separate weight, subdivision, attack grid, roughness and density instead of deriving all five from a genre name. If the user asks for simple on-beat backing, do not substitute a stereotypical off-beat bass pattern or quantize the singer and solo with the drums. Keep the user's latest hand edits as the protected baseline. Add complementary roles only in sections that need them: for example a sub, midrange bass articulation, octave support or later harmony can provide thickness without continuously stacking every layer. Compare at similar loudness; greater LUFS or band energy does not prove a better arrangement.

## Choose techniques that solve the actual problem

Read [arrangement-design.md](arrangement-design.md) when selecting an ensemble, writing bass/inner voices or an interlude, or deciding what an effect should contribute. Use [melody-design.md](melody-design.md) for an authorized foreground rewrite; the arrangement pass itself preserves the authored lead.

### Section contrast and subtraction

Where a section is crowded or flat, try removing or delaying a supporting entrance before adding another layer. A restrained verse can make a fuller refrain meaningful. Contrast can come from register, articulation, duration, rhythmic activity, or a change of lead instrument; raising the fader alone need not supply it. Retain an intentional sparse or static texture when that serves the cue. Ableton's [subtractive arrangement method](https://makingmusic.ableton.com/arranging-as-a-subtractive-process) motivates this option; do not copy its exploratory timeline expansion into a duration-constrained or already authored project.

### Foreground, voicing, and low end

Check simultaneous attacks, sustained overlap, and register around the vocal or lead. Thin chord voicings, shorten backing notes, move a support line to a suitable octave, or place answers in the foreground's gaps. Preserve useful unison/octave doubling deliberately. Coordinate kick and bass attacks and note lengths where those roles exist; do not make every supporting part reproduce the lead rhythm. Avoid indiscriminate low-register chord stacks. In a mix, compare with the suspect part muted before reaching for EQ or compression. These are practical applications of iZotope's [arrangement-first approach to masking](https://www.izotope.com/community/blog/unmasking-your-mix-with-neutron), not fixed frequency cutoffs or a requirement to eliminate all spectral overlap. Pitch overlap alone does not prove audible masking.

### Repetition, transitions, and return

Keep the motif recognizable while changing a meaningful feature in a repeated section, such as its answering phrase, accompaniment rhythm, register, or sound. Use an occasional pickup, fill, brief reduction, held note, or instrumental response to prepare an arrival; avoid a crash or riser at every boundary. Ableton's [variation method](https://makingmusic.ableton.com/creating-variation-2-mutation-over-generations) and [unique events](https://makingmusic.ableton.com/unique-events) offer options, not variation quotas. In a game loop, a one-off gesture still recurs each cycle: check that it does not become an intrusive accent. Audition the end directly into the beginning, including residual reverb, rather than judging the edges separately.

### Playable parts and expressive performance

Choose sounds for their role and range. Match sustained lines to sustained/legato-capable sounds and quick repeated figures to suitable short attacks; allow breaths and believable releases. For orchestral writing, consider handing a returning theme between families instead of doubling the whole orchestra continuously. These decisions follow Spitfire's [virtual orchestration guide](https://www.spitfireaudio.com/en-us/blogs/editorial/how-to-create-orchestra-music-with-virtual-instruments-a-beginners-guide). A shorter MIDI gate on a sustained patch is not automatically a sampled staccato articulation.

Shape emphasis and release over phrases. Use deliberate timing, velocity, gate, or supported automation before adding random humanization; keep ensemble arrivals coherent. Ableton discusses [automation for performance variation](https://makingmusic.ableton.com/humanizing-with-automation-envelopes). Inspect the actual instrument's controls: Spitfire distinguishes [dynamic-layer changes from expression volume](https://support.spitfireaudio.com/en/articles/11816091-what-is-the-difference-between-dynamics-vibrato-and-expression), but its CC mappings are library-specific. Do not assume CC1, CC11, a keyswitch, or a similarly named Auris parameter has the same effect. Verify supported controls and the rendered result; do not claim an articulation change from a saved setting alone.

## Apply through the available Auris interface

### CLI specification revisions

Keep the composition baseline and create a separately named arranged candidate. In `.asong`, section `parts` controls participation; `[section.<section>.part.<name>]` can override `octave`, `density`, `gate`, `subdivision`, `rhythm`, and `note`. Part-level sound, role, gain, and pan belong to the track, not a section patch. Use separate parts for different instruments entering in different sections. For example, with `lead`, `chords`, `bass`, and `arp` already declared:

```toml
[section.verse]
bars = 8
parts = "lead chords bass"

[section.chorus]
bars = 8
parts = "lead chords bass arp"

[section.chorus.part.arp]
density = 0.45
gate = 0.6
```

This is a structural example, not a prescription to always add an arpeggio. Validate with `auris compose arranged.asong --print`, compose to a new project path, then use `auris info` and compare the score/preview. The CLI regenerates the complete specification: a fixed seed does not guarantee unchanged melody after settings change. Never use this route to claim preservation of an existing authored foreground. For exact backing-only edits, use MCP/live tools; if unavailable, preserve the original and explain the limitation. Do not invent a CLI `arrange` command or directly rewrite project JSON to bypass supported edits.

### MCP and live editing

Inspect `discover_tools`/`tool_help` for the actual host and obtain current IDs/revisions. With restricted saved-project MCP groups, `composition` covers `accompaniment` and harmony suggestions, `manual` covers note/arrangement/performance edits, `mix` covers mixing, and `vocals` covers singer work.

- Use `read_notes` and `inspect_project` before editing. `accompaniment` supports explicit `foreground_clips` and `protected_clips`; the nonempty foreground must be a subset of protected clips. Target separate backing tracks/clips and verify returned adaptation reasons. Do not assume the distinct `accompany` command has this same contract.
- Use advertised note and clip tools for authored voicings, rests, fills, and counterlines. Discover `arrangement` operations before using them; the tool name does not mean it automatically completes this musical pass. Keep unrelated tracks and protected material unchanged.
- Every part written note by note starts machine-exact. Apply a `performance_presets` entry for its instrument and genre before tuning; listing presets with the clip returns a `suggested_preset` to start from. Keep a part machine-tight only for a style that wants it (chiptune, a drum machine).
- Read `inspect_performance` before `set_performance`. Change only intended settings, read back effective results and unsupported/bypassed reasons, then render the affected phrase. Use actual parameter metadata for instrument controls and automation; do not translate a tutorial's MIDI controls by name alone.
- Balance levels, panning, and sends after the parts work together. Discover `mixer_graph`, `automation`, or other advertised mixing tools as needed. Avoid using normalization, widening, or effects as a substitute for resolving conflicting parts. For vocals, follow [vocals.md](vocals.md); refresh synthesis when score/performance changes make a take stale.

For a small mix change, audition the affected range and its dependent routing first. A kick change can affect a sidechain destination; shared nonlinear processing can invalidate an apparently untouched bus. Do not re-synthesize an unchanged singer merely because a fader moved. Keep the final full-song verification even when intermediate checks use short ranges.

## Review before delivery

Inspect the opening, contrasting section, later repeat, transitions, and ending/loop return. Compare the same ranges before and after, at comparable playback loudness where available. Preserve the baseline and adopt changes for the stated musical purpose, not because there are more notes, effects, or a larger waveform. A no-change decision needs specific evidence, not just a successful tool call.

Keep component preferences separate when a listener favors one candidate's melody or vocal and another candidate's backing. Do not convert that into an overall winner or infer unreported naturalness, memorability, or breathing ratings. If a combined candidate is requested, retain the favored foreground and check the backing against its actual harmony, phrasing, register, and timing before adoption; independently preferred parts are not automatically compatible.

Treat a liked backing's pulse, voicing, bass movement and density as the baseline during a melody revision. Add an answering fill, syncopation, layer or effect only for a named problem or section function; theoretical sophistication alone is not a reason. For a fair local comparison, keep compatible backing fixed while changing the melody, or protect the melody while changing backing, and then check their interaction in the full mix. If the new melody requires a harmonic repair, identify that dependency instead of claiming that only the melody changed. Do not infer that all backing should be simpler from a few listener preferences.

Fix the most consequential issues first; after at most two local attempts per issue, report unresolved limitations unless further exploration is requested. Finish with the full render and measurements in [delivery.md](delivery.md). Record what was changed or deliberately retained, which passages were checked, and what remains uncertain. Without actual listening, describe note/structure checks and audio measurements accurately; do not claim that clarity, expression, or seamless looping has been heard.

The linked primary sources were consulted on 2026-10-25. The workflow and Auris mappings above are an adaptation, not instructions to install the sources' products or universal genre rules.
