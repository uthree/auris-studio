# Review findings: auris-mcp

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 1 verified findings: 1 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-277 | low | `crates/auris-mcp/src/main.rs:220` | list_progressions does a blocking std::fs::read_to_string directly on the tokio runtime instead of via the crate's spawn_blocking-based `blocking()` helper. |

### F-277 · low · list_progressions does a blocking std::fs::read_to_string directly on the tokio runtime instead of via the crate's spawn_blocking-based `blocking()` helper.

`crates/auris-mcp/src/main.rs:220` · concurrency · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** An MCP client calling list_progressions triggers a synchronous filesystem read directly on a tokio worker thread; under concurrent tool calls this can cause a brief scheduling stall for other in-flight async work on the same runtime, but the call itself still returns correct results with no crash or data loss.

**Trigger.** A model calls the `list_progressions` MCP tool. Every call runs `std::fs::read_to_string` on a tokio (rt-multi-thread) worker thread instead of a `spawn_blocking` thread.

**Mechanism.** The crate doc (lines 19-21) states the design rule: 'Blocking work leaves the runtime. Every tool that opens a session runs inside spawn_blocking, both because the work is honest blocking DSP...'. `list_progressions` (lines 219-222) instead calls `finished(Ok(toolbox::list_progressions::run()))` synchronously inside the `async fn`, with no `spawn_blocking`. `toolbox::list_progressions::run()` calls `auris_session::progressions::ProgressionBook::load()` (crates/auris-session/src/progressions.rs:103-114), which does `std::fs::read_to_string` — real blocking filesystem I/O — on whichever tokio worker thread is handling the request. The sibling implementation in auris-agent's `text_tool!` macro (crates/auris-agent/src/main.rs:348-351) wraps this exact same call in `spawn_blocking`, and its own comment names the reason: 'the listings read the progression book off disk'. `list_presets`, right next to it in the same file, correctly stays unwrapped because it does no I/O at all (pure in-memory constants).

**Expected.** Wrap the call the same way list_instruments does: `blocking(move || Ok(toolbox::list_progressions::run())).await`, matching both the crate's own stated rule and auris-agent's handling of the same toolbox function.

**Fix direction.** Wrap the body in the existing `blocking()` helper, matching every other filesystem/session-touching tool: `blocking(move || Ok(toolbox::list_progressions::run())).await`.

**Written rule it breaks.** crates/auris-mcp/src/main.rs:18-20 doc comment: "Blocking work leaves the runtime. Every tool that opens a session runs inside `spawn_blocking`... because the work is honest blocking DSP" (list_progressions performs blocking std::fs I/O but is not wrapped)
