# Build performance

Development and test builds use a different optimization policy from release builds. Measure
compilation and audio processing separately: a faster compiler run does not establish that a
callback still meets its deadline.

## Local iteration

The desktop executable uses optimization level 0, while `gpui`, `gpui-component` and `rig-core`
use level 1. Cargo's default `lto = false` still performs Thin LTO within an optimized
crate with multiple code-generation units; level 0 avoids that work for the desktop executable.
Audio crates retain their existing optimization levels and local LTO policy. Development
builds keep limited debug information (`debug = 1`), including source locations for
backtraces, for both workspace crates and dependencies. They do not retain full type and
local-variable information. Incremental compilation is disabled to avoid storing another
large copy of compiler state in every worktree. Test builds inherit these settings. Release
builds use their own Thin LTO and single-code-generation-unit settings.

These settings favor short UI iteration over optimized development UI code. Use
`cargo run --release` when measuring production UI performance.

Keep a worktree's `target` directory between edits. A compiler cache such as `sccache` helps
reuse dependencies across builds, but cannot cache executable linking. Changing compiler
versions, profile settings, features or `RUSTFLAGS` may rebuild dependencies even when source
files have not changed.

```sh
cargo build --locked --timings
cargo test --workspace --locked --timings
```

Cargo writes timestamped HTML reports to `target/cargo-timings/`. Compare the same command,
toolchain, features, job count and cache state. Keep initial builds, unchanged builds and
builds after an edit as separate measurements. Parallel unit durations must not be added
together to estimate elapsed build time.

For an audio-processing check without an audio device:

```sh
cargo run --locked -p auris-synth --example physical_bench
```

This example deliberately uses the development profile to exercise development playback.
Use a release build separately when measuring production performance. Background compilation
can distort callback measurements, so compare repeated runs under similar machine load.

## Build storage

Cargo keeps libraries, executables, test binaries, examples and native build-script outputs
under `target/`. `cargo check`/Clippy metadata, normal builds and tests can coexist there;
features, compiler flags, toolchains, profiles and target architectures can add further
variants. Each Git worktree has its own target directory by default. Native dependencies
such as ONNX Runtime and TLS libraries also have build outputs; the desktop build script
itself only embeds the Windows icon. Downloaded voice models, SoundFonts and dictionaries
are separate from the compiler cache.

The default development/test settings reduce two sources of growth: full workspace debug
information and incremental compiler state. Disabling incremental compilation can make
rebuilds after edits slower; it also changes Cargo's default code-generation-unit count
from 256 to 16. It does not disable reuse of unchanged compiled dependencies. For debugging
local variables or comparing edit times, opt in for the command that needs it (also valid
in PowerShell):

```sh
cargo --config 'profile.dev.debug=2' --config 'profile.dev.incremental=true' run
cargo --config 'profile.dev.debug=2' --config 'profile.dev.incremental=true' test -p auris-core
```

These overrides create additional artifact variants in the same target directory.
`CARGO_INCREMENTAL`, profile environment variables and local Cargo configuration can override
the repository defaults; check them when incremental data keeps growing.

Changing the settings prevents new incremental state but does **not** remove existing
artifacts. Keep a target directory during active work. When retiring a worktree or an old
profile/toolchain, inspect the exact target directory before manually cleaning it:

```sh
cargo clean --dry-run --target-dir /path/to/retired-worktree/target
# Only after reviewing the preview and stopping builds that use this directory:
cargo clean --target-dir /path/to/retired-worktree/target
```

Cleaning discards compiled dependencies too, so the next build must recreate them. Nothing
in the build automatically deletes caches or changes global Cargo settings. The Cargo
registry and Git checkout caches live under `CARGO_HOME`, outside a worktree's target.

For several compatible worktrees, an explicit `--target-dir /path/to/shared-target` on each
build/test command can reuse dependency artifacts. Use the same toolchain, flags and profiles
for useful reuse. Cargo serializes concurrent builds on that directory's lock, and output
executables belong to the most recent build; launch through `cargo run` from the intended
worktree instead of running a shared executable by hand. Separate targets preserve independent
parallel builds at the cost of duplicate storage. The repository does not force either policy.

Cargo documents [profile settings](https://doc.rust-lang.org/cargo/reference/profiles.html)
and the [build cache layout](https://doc.rust-lang.org/cargo/reference/build-cache.html).

### Cloud storage comparison (2026-10-29)

A saved Linux cloud environment already contained 6.37 GiB of target artifacts, including
3.47 GiB of incremental state, 2.47 GiB in `debug/deps`, 206 MiB in native/build-script
outputs and 255 MiB in examples. Its registry and Cargo Git caches were 293 MiB and 34 MiB.
That snapshot includes earlier builds and variants; it is evidence of accumulated compiler
outputs, not a reproducible single build or a measurement of a developer's Mac/Windows disk.

For a comparable workload, the same source at `f2e851f3`, Rust 1.95.0, four jobs and the
same populated Cargo download cache were used with two initially empty target directories:

```sh
cargo test --locked -p auris-core -p auris-dsp -p auris-synth -p auris-compose \
  --lib --no-run --timings -j 4 --target-dir /path/to/empty-target
```

The first build used the previous development settings (`debug = 2`, incremental on for
workspace crates); the second used `debug = 1`, incremental off. Dependency debug information
remained at level 1 in both. Allocated bytes were measured with `du -s -B1` independently
for the target directory and each subdirectory, immediately after each build:

| Generated output | Before (MiB) | After (MiB) |
| --- | ---: | ---: |
| Entire target directory | 1,617.7 | 384.6 |
| `debug/incremental` | 1,005.0 | <0.1 |
| `debug/deps` | 564.1 | 330.7 |
| `debug/build` | 52.5 | 52.5 |

This workload used **76.2% less target storage**. It covers backend test builds, not the
desktop dependency graph or a Mac/Windows build. The builds were single runs; a separate
debug-override smoke build overlapped part of the second run, so no compilation-speed
comparison is claimed. The documented override was also checked to emit `-C debuginfo=2`
and `-C incremental=...` for a workspace test. Existing targets were not cleaned.

## CI caching and timings

CI uses `Swatinem/rust-cache`, pinned to a commit. It keys dependency artifacts by compiler,
manifests, Cargo configuration and compiler environment, and prunes workspace artifacts and
incremental state before saving. The action disables incremental compilation for these CI
builds. Dependency artifacts can be saved even when a later test fails.

Only `main`, `dev` and manually dispatched runs save caches; pull requests restore compatible
base-branch caches. This limits duplicate caches for short-lived branches. A compiler or
profile change starts a new cache generation, so the first CI run can still be expensive.
Cache retention and repository storage limits also affect reuse.

Every platform uploads the test-build timing reports as `cargo-timings-<OS>-<architecture>`
artifacts, retained for seven days, including reports available after a failure. Windows
retains reports for both default and ASIO builds. Formatting, Clippy, tests, the release update
helper and documentation checks retain their existing coverage.

## Investigation baseline

The 2026-10-26 investigation at `0a09ec83` used Rust 1.99.0 on a Ryzen 7 9800X3D Windows
machine with 32 GB RAM. A new target directory with an already populated compiler cache took
326.3 seconds; an unchanged build finished in 0.81 seconds. A small synthetic desktop edit
took 49.1 seconds without incremental compilation and 50.9 seconds with it. These were single
runs and do not establish an incremental-compilation speedup.

A diagnostic compiler-phase comparison for the desktop executable took 58.3 seconds with
the original local Thin LTO and 35.0 seconds with LTO disabled for that executable. These are
compiler-process durations, not whole-workspace or CI speedups. The initial build also spent
152.6 seconds in the `aws-lc-sys` native build script; preserving native dependency outputs
therefore matters alongside Rust compilation.

The successful [CI run 40292515844](https://github.com/uthree/auris-studio/actions/runs/40292515844)
spent 36m27s compiling default Windows tests, 10m45s compiling ASIO tests, 21m49s compiling
macOS tests and 8m06s compiling Linux tests. The corresponding test steps took 43m26s, 17m40s,
29m20s and 11m25s. Use the uploaded reports from subsequent runs to assess the new settings;
these baseline numbers are not promises about future runs.

With the new dependency settings held fixed, rebuilding the desktop at level 1 took 74.2
seconds (72.9 seconds in its executable compiler invocation), followed by 19.9 seconds at
level 0 (19.1 seconds in the compiler). Both used `cargo build --locked --timings -j 16`,
with only the desktop optimization level changed and no incremental compilation. Another
level-0 rebuild took 44.8 seconds (35.9 seconds in the compiler), showing substantial load
variation. These measurements support the faster iteration setting but are not a controlled
estimate of CI speedup. The setup build used four jobs, rebuilt dependencies under the new
profiles and took 13m23s; it is not comparable to the earlier partially cached 16-job build.

## Dependency selection

The desktop uses `gpui-component` 0.5.1 as a complete component library without its optional
language grammar bundle. Conversation Markdown is still parsed with `markdown`'s GFM parser.
Code blocks retain their contents and indentation but reach `TextView` without language
metadata, avoiding its syntax highlighter. Copying a message retains the original Markdown.
The component library's mandatory Tree-sitter core and JSON grammar remain dependencies.

On the locked Windows graph, unique package/version pairs changed as follows:

| Desktop dependencies | Before | After |
| --- | ---: | ---: |
| Normal and build | 750 | 722 |
| Including test dependencies | 757 | 729 |

The 28-package reduction consists of optional language grammars and also applies to the
macOS and Linux desktop graphs. Counts describe selected target graphs rather than the
all-platform lock file. They do not establish an elapsed build-time gain.

## Audio-processing check

The profile change keeps the workspace audio crates at level 1 and audio dependencies at
level 3, including their existing local LTO policy. The before and after `physical_bench`
executables were run four times each in alternating order on the same Windows machine.
The median of the per-run mean callback duration, in milliseconds, was:

| Instrument, 24 held voices | Before | After |
| --- | ---: | ---: |
| Piano | 3.501 | 2.526 |
| Guitar | 0.133 | 0.119 |
| Electric guitar | 0.144 | 0.133 |
| Bass | 0.055 | 0.043 |
| Bell | 0.174 | 0.154 |
| Mallet | 0.195 | 0.153 |
| Violin | 0.163 | 0.117 |

Other worktrees were compiling in the background. For example, the piano's before-run means
ranged from 2.627 to 4.847 ms and after-run means from 2.352 to 3.431 ms. These results did not
show a regression in the sampled medians; the machine load prevents attributing the lower
numbers to this change or guaranteeing realtime deadlines. They are timing measurements,
not listening assessments.

## Validation on Windows

The 2026-10-26 change passed `cargo test --workspace --locked --timings -j 4` (4,677 reported
passes, 50 ignored tests, no failures), `cargo clippy --workspace --all-targets --locked -j 4
-- -D warnings`, `cargo fmt --all` and actionlint 1.7.12 for the CI workflow. The test build
phase took 5m39s after the development build had populated compatible dependencies; this is
not an initial-build measurement. Cross-platform and ASIO verification run in CI.

The 2026-10-27 grammar reduction passed the same workspace test command (4,679 reported
passes, 50 ignored tests, no failures), workspace Clippy with warnings denied, formatting
and a desktop build. Regression coverage includes Japanese lists and quotes, nested code,
literal fences, language metadata, scrolling, resizing and copying the original Markdown.
