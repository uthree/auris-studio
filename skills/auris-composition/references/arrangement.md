# Arrangement after composition

Use this pass for complete songs and instrumental cues after the core melody, lyrics where applicable, harmony, and form exist. It covers how to carry out the arrangement with the Auris tools and how to check it; the musical techniques themselves (part roles, voicing, bass and kick, fills, interludes, performance, loop returns) are headings in the composition techniques that `search_documentation` returns. Author counterlines, distinctive bass, interludes and fills note by note; reserve writers for plain chord/pad backing, simple arpeggios and basic drum pulses. A track count, automatic balance, or successful render does not establish an intentional arrangement. Scale the work to the brief; do not expand a local edit or a short demo into a full production.

## Establish what the arrangement must serve

Save a baseline project and a short preview when available. Inspect actual notes, sound selections, section boundaries, and the requested loop/ending. Identify the foreground and protected material. Briefly map each section's foreground, accompaniment roles, register, density, and entrance/exit. Keep this in `production_record` when available or in concise delivery notes. This is a working musical plan, not a requirement to fill every role in every section.

For each proposed change, name its purpose and range: for example, leave space for verse lyrics, make the refrain open up, or prepare the loop return. Keep recognizable hooks, authored melody/lyrics, and intentional harmonic tensions. Reharmonization or replacing the main theme is a separate creative choice, not an automatic consequence of arranging.

Separate weight, subdivision, attack grid, roughness and density instead of deriving all five from a genre name. If the user asks for simple on-beat backing, do not substitute a stereotypical off-beat bass pattern or quantize the singer and solo with the drums. Keep the user's latest hand edits as the protected baseline. Compare at similar loudness; greater LUFS or band energy does not prove a better arrangement.

## Apply through the available Auris interface

Inspect `discover_tools`/`tool_help` for the actual host and obtain current IDs/revisions. With restricted saved-project MCP groups, `composition` covers `accompaniment` and harmony suggestions, `manual` covers note/arrangement/performance edits, `mix` covers mixing, and `vocals` covers singer work.

- Use `read_notes` and `inspect_project` before editing. `accompaniment` supports explicit `foreground_clips` and `protected_clips`; the nonempty foreground must be a subset of protected clips. Target separate backing tracks/clips and verify returned adaptation reasons. Do not assume the distinct `accompany` command has this same contract.
- Use advertised note and clip tools for authored voicings, rests, fills, and counterlines. Discover `arrangement` operations before using them; the tool name does not mean it automatically completes this musical pass. Keep unrelated tracks and protected material unchanged.
- Give every part written note by note a starting feel from `performance_presets` (listing presets with the clip returns a `suggested_preset`). Read `inspect_performance` before `set_performance`, change only intended settings, read back effective results and unsupported/bypassed reasons, then render the affected phrase. Do not claim an articulation change from a saved setting alone.
- Balance levels, panning, and sends after the parts work together. Discover `mixer_graph`, `automation`, or other advertised mixing tools as needed. Avoid using normalization, widening, or effects as a substitute for resolving conflicting parts. For vocals, follow [vocals.md](vocals.md); refresh synthesis when score/performance changes make a take stale.

For a small mix change, audition the affected range and its dependent routing first. A kick change can affect a sidechain destination; shared nonlinear processing can invalidate an apparently untouched bus. Do not re-synthesize an unchanged singer merely because a fader moved. Keep the final full-song verification even when intermediate checks use short ranges.

## Review before delivery

Inspect the opening, contrasting section, later repeat, transitions, and ending/loop return. Compare the same ranges before and after, at comparable playback loudness where available. Preserve the baseline and adopt changes for the stated musical purpose, not because there are more notes, effects, or a larger waveform. A no-change decision needs specific evidence, not just a successful tool call.

Treat a liked backing's pulse, voicing, bass movement and density as the baseline during a melody revision. Add an answering fill, syncopation, layer or effect only for a named problem or section function. If a new melody requires a harmonic repair, identify that dependency instead of claiming that only the melody changed. When a listener prefers one candidate's melody and another's backing, check the backing against the favored foreground's actual harmony, phrasing, register, and timing before combining them.

Fix the most consequential issues first; after at most two local attempts per issue, report unresolved limitations unless further exploration is requested. Finish with the full render and measurements in [delivery.md](delivery.md). Record what was changed or deliberately retained, which passages were checked, and what remains uncertain. Without actual listening, describe note/structure checks and audio measurements accurately; do not claim that clarity, expression, or seamless looping has been heard.
