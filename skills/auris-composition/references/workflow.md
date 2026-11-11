# Auris workflow reference

This reference records the concrete interfaces the `auris-composition` skill may use. Discover exact fields with MCP `discover_tools`/`tool_help` when the installed build differs.

## Authored notes first

For ordinary composition, create an empty project with MCP and write expressive parts using add_clip/read_notes/edit_notes. Decide the actual notes yourself; scripts may serialize them or copy an authored motif, not invent the melody algorithmically. Use automatic writers only for plain chord/pad backing, simple arpeggios and basic drum pulses. Finish arrangement, effects and track balance with the actual voices before rendering.

## Headless CLI

For a complete delivery, follow [arrangement.md](arrangement.md) between establishing the composition and final export. It maps section/part revisions and protected MCP backing edits to the arrangement review.

```text
auris progressions
auris info <project.auris>
auris render <project.auris> -o <mix.wav>
auris render <project.auris> --stems <directory>
auris render <project.auris> -o <loop.wav> --loop
```

The CLI's exact flags are authoritative for the installed version. Mix and stem exports are separate modes: when `--stems` is supplied, the command writes stems to that directory and does not write the mix `-o` file. `--loop` renders the loop region already stored in the project; it fails when no loop has been set. Run `auris help` when a command rejects an argument. A Save As path may become a nested `<name>/<name>.auris` project; use the path printed by the command or `auris info` rather than guessing.

## Saved-project MCP sequence

The saved-project server works on saved project files:

1. `create_project` or `open_project`.
2. `inspect_project` to obtain stable track and clip IDs.
3. `list_progressions` for named chord progressions.
4. `add_clip`/`read_notes`/`edit_notes` to author expressive parts. `generate_phrase` is optional for mechanical backing only.
5. `accompany` for backing parts under an existing melody.
6. `inspect_project` and `inspect_audio`/`analyze` to verify.
7. `render` and `verify_render` for delivery files.

`accompany` carries its own input contract and do not take a clip revision field. `setup_tracks` uses its request ID/fingerprint contract. For operations that explicitly require an `expected_revision` or current revision, obtain it from the relevant inspection/read call and pass it unchanged. Read the affected project or clip again after writing. Use `tool_help` instead of guessing schemas; argument errors are feedback, not permission to retry unrelated calls.

When scripting MCP, inspect every tool result's `isError` and returned content; a successful subprocess exit alone does not establish tool success. Decode JSON text content before inspecting fields. For a `report_id`, follow the returned `report_path` and `next_offset` with `read_report` in the same server process; do not assume every report contains `/issues` or `/sections`. Reports expire on restart or eviction. If one expires, regenerate the read-only report, not the edit that produced it. Respect each tool's batch/page limits and finish reading all relevant notes before writing.

On Windows, use UTF-8 for script input/output and JSON containing Japanese titles or lyrics (for Python, `-X utf8` or explicit UTF-8 streams). A console encoding failure after a tool call can occur after the operation succeeded; inspect the saved result/project before retrying a mutation.

## Live agent sequence

The desktop live agent has a separate, smaller catalog. Start with `discover_tools` and fetch `tool_help` for the returned candidates; do not assume that saved-only names such as `list_progressions` are available. Inspect the open document, read the target clip, then use the advertised live note/phrase/harmony tools and read back the result. Apply revision checks only to operations whose schema declares them.

## Vocals and loops

For lyrics, singer selection, and the required `sing` step before vocal rendering, follow [vocals.md](vocals.md). Instrumental rendering alone does not synthesize a finished vocal take.

For a loop, make the musical boundary explicit in the score and render an unambiguous first/last bar. Verify that the end does not contain an uncontrolled tail before claiming seamless looping.
