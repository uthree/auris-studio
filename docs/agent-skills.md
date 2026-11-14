# Agent skills

[日本語](agent-skills.ja.md)

## Auris composition skill

`auris-composition` teaches a coding agent when and how to use Auris Studio for original songs with lyrics and game/video music: editable scores, vocal synthesis, loopable cues, themes, transitions, previews, renders, stems, and measurable delivery checks. Its [vocal workflow](../skills/auris-composition/references/vocals.md) covers lyric/melody development, voice selection, CLI/MCP synthesis, and full-song coverage checks. It is distributed separately from the Auris desktop binary and does not install or update that binary.

Complete music deliveries include a [post-composition arrangement pass](../skills/auris-composition/references/arrangement.md) that carries out and checks the arrangement with the Auris CLI/MCP tools. Existing authored melodies and lyrics remain protected. The skill's references cover operating Auris; the musical techniques are the [composition techniques](composition-techniques.md), which the installed build's `search_documentation` returns, so the desktop agent and the skill read the same text.

Install it globally for Codex with the `skills` CLI:

```sh
npx skills add uthree/auris-studio --skill auris-composition -a codex -g -y
```

To keep it in one project, omit `-g` and run the command from that project's directory:

```sh
npx skills add uthree/auris-studio --skill auris-composition -a codex -y
```

List installed skills and update this skill with:

```sh
npx skills list -g
npx skills update auris-composition -g -y
```

The exact `skills` CLI version may expose an interactive prompt; remove `-y` when choosing a different target or scope. The skill source is the `skills/auris-composition/` directory in this repository. It can also be invoked explicitly as `$auris-composition` after installation.

## Auris runtime prerequisite

Install Auris Studio separately from the [release page](https://github.com/uthree/auris-studio/releases), or build it from source. The skill only provides agent guidance; it does not provide `auris`, `auris-mcp`, a SoundFont library, or a desktop application. Confirm that `auris --help` works before choosing the headless CLI. If Auris is not on PATH, use `/Applications/Auris Studio.app/Contents/MacOS/auris` or `~/Applications/Auris Studio.app/Contents/MacOS/auris` on macOS, and `%LOCALAPPDATA%\Programs\Auris Studio\auris.exe` on Windows; the matching `auris-mcp` path is beside it. For MCP, confirm that `auris-mcp` is reachable from the host's MCP configuration. The skill does not auto-install a missing runtime; use the release page when no binary is found.

## MCP composition group

`auris-mcp` exposes common read-only discovery and help tools by default. If the server is configured with a restricted `AURIS_MCP_TOOL_GROUPS`, enable the `composition` group for generated compositions and the `manual` group for note-level editing. Add `mix` when the agent must change routing or normalize a delivery:

```text
AURIS_MCP_TOOL_GROUPS=composition,manual,mix
```

For songs with synthesized vocals, also enable `vocals` (for example, `composition,manual,mix,vocals`). An installed singing voice or a configured, running VOICEVOX Engine is required for the selected backend. The skill does not install voice models or start a missing service automatically.

On Windows, set the variable in the MCP server process configuration rather than only in an unrelated terminal. On macOS/Linux, export it in the environment used to launch `auris-mcp`. Restart the server after changing it. A value of `all` enables every group; use the narrow groups above when the agent only needs music creation and delivery. The server reports a disabled group in its tool result, so the agent should correct configuration rather than inventing an unavailable tool.
