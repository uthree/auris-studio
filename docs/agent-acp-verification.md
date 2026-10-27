# Agent ACP verification

Verification date: 2026-10-28. The shared transport uses Rust `agent-client-protocol`
3.5.0. The Codex and Claude adapters used by the ACP launcher are `codex-acp` 2.3.0
and `claude-agent-acp` 0.97.0 respectively.

## Verification matrix

The full workspace run passed 4,754 tests with 49 ignored and no failures. After the final
native-operation identity correction, the auris-agent suite passed 175 tests with 12 ignored,
and the desktop completed-review-history regression test passed. Formatting and workspace
Clippy checks passed; Clippy also reported the existing dependency future-compatibility notice
for `proc-macro-error2`. The
passing coverage includes live MCP editing of an unsaved song with Undo, permission refusal,
history replay, stream ordering, external-note import, audio fixtures, and Windows grandchild
cleanup. Ignored tests are the explicitly opt-in real-agent/evidence cases; they are not counted
as failures.

| Agent | Transport verification | Inference/evidence verification | Status |
| --- | --- | --- | --- |
| Codex ACP 2.3.0 | Startup, model catalogue and live MCP catalogue passed | `gpt-6-luna`: unsaved edit, custom system instructions, Undo, native write deny/allow, text attachment, worker-restart history replay, Stop during pending approval, image, and local two-second audio measurements passed | Passed |
| Claude ACP 0.97.0 | Startup, model catalogue and live MCP catalogue passed | `haiku`: unsaved edit, custom system instructions, Undo, native write deny/allow, text attachment, worker-restart history replay, Stop during pending approval, image, and local two-second audio measurements passed after signing in | Passed |
| OpenCode 1.19.1 | Startup, model catalogue and live MCP catalogue passed | `opencode/nemotron-3.5-super-vl-free`: unsaved edit, system instructions and Undo passed. The image/local-audio case timed out twice after 120 seconds without an answer; its OpenCode session export contained the image and local measurements, but the assistant turn did not finish. An independent raw-ACP image-only request returned the correct colour. Native write denial/approval, text attachments, worker-restart history replay and Stop during pending approval passed after the Windows shell correction. Two attempts hit the startup deadline during a full build; the complete repeat passed. | Partial; media inference not established |

The OpenCode Windows shell boundary is deliberate: a bare PowerShell expression could execute
without an approval prompt, while ordinary cmdlets prompted. The production launcher now selects
`cmd.exe` on Windows and asks the agent to invoke `powershell.exe -NoProfile -Command` explicitly
when PowerShell is needed. An independent raw-ACP probe reproduced the bare-expression gap
without Auris, then received a permission request and left the file absent with `cmd.exe` and
a rejection. The upstream shell implementation is documented in
[OpenCode shell.ts](https://raw.githubusercontent.com/anomalyco/opencode/v1.19.1/packages/opencode/src/tool/shell.ts).

## Reproducing the checks

The local validation host is Windows. macOS/Linux process-group execution and OpenCode 2.x
runtime behavior were not exercised in this run; version-specific configuration is covered by
fixtures. Real-agent checks validate protocol behavior and local audio measurements, not
listening quality or musical quality.

```powershell
cargo test --workspace -j 4
cargo clippy --workspace --all-targets -j 4
cargo fmt --all -- --check
```

Installed-agent tests are opt-in. Set `AURIS_ACP_EVAL_BACKEND` and
`AURIS_ACP_EVAL_MODEL`; Codex/Claude also require their installed adapters described in
[Agent backends](agent-backends.md). Each inference test needs a fresh absolute
`AURIS_ACP_EVAL_OUTPUT` directory path (the test creates it). The media test also needs an
absolute `AURIS_ACP_EVAL_AUDIO` path to a two-second WAV. Run the selected test individually:

```powershell
cargo test -p auris-agent installed_acp_initializes_models_and_live_catalog -- --ignored --nocapture
cargo test -p auris-agent installed_acp_edits_unsaved_song_with_system_instructions_and_undo -- --ignored --nocapture
cargo test -p auris-agent installed_acp_checks_native_permissions_references_memory_and_stop -- --ignored --nocapture
cargo test -p auris-agent installed_acp_reads_image_and_audio_references -- --ignored --nocapture
```

The tests create disposable songs/files and retain evidence in the chosen directory. They
perform real inference using the installed agent's account; they do not sign in automatically.

## Antigravity

The installed Antigravity CLI (`agy.exe` 1.4.8) does not expose an ACP mode in its help
output. It does expose the existing newline-delimited `--input-format stream-json` and
`--output-format stream-json` transport used by Auris' current backend.

The ACP Registry publishes a separate, Google-authored Windows ACP server. The registry
manifest currently lists `antigravity-acp` 1.4.3 and provides Windows x86_64 and arm64
archives. The x86_64 archive was downloaded to the task-local evaluation directory and its
SHA-256 matched the manifest (`54c2e16378d9a4952e1da70e355f3c33c4f284308d6de48ab3e6e34a127bd325`).
The archive contains `agy_acp_server.exe` and `localharness_external.exe`.

Running the standalone server from the extracted directory and sending an ACP v1
`initialize` request succeeded. It advertised:

- `loadSession`, `session/list`, and `session/resume`;
- image, audio, and embedded-context prompt content;
- HTTP and SSE MCP transports;
- an append-mode client system prompt extension;
- authentication methods `oauth-personal`, `oauth-business`, `gemini-api-key`, and
  `agent-platform`.

An unauthenticated `session/new` request returned the typed `auth_required` error and did
not start a model session. The server read the missing settings path
`%USERPROFILE%\.gemini\antigravity-acp\settings.json`; the probe did not create or modify
that file and did not open an interactive authentication flow.

This establishes ACP v1 initialization compatibility only; it does not establish parity with
Auris' shared client or its live-edit behavior. The standalone server is a separate runtime that
must be discovered or explicitly configured, and the probe reached authentication before a model
session could be created. Auris retains the existing `agy.exe` backend for now; the ACP server is
not selected automatically. A future opt-in migration must preserve live MCP calls, permission
refusal, cancellation, structured tool calls, undo, history resume, and image/audio attachments
before the legacy route is removed.

Evidence:

- [ACP Registry Antigravity manifest](https://raw.githubusercontent.com/agentclientprotocol/registry/main/antigravity-acp/agent.json)
- [ACP Registry protocol matrix](https://raw.githubusercontent.com/agentclientprotocol/registry/main/.protocol-matrix/latest.md)
