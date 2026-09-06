# Auris Studio

A digital audio workstation written in Rust with a GPU-rendered
[gpui](https://crates.io/crates/gpui) interface for macOS and Windows. Arrange and edit
music, compose from text specifications, record audio, host CLAP and VST3 plugins,
and render singing voices. CLI, MCP and agent frontends share the desktop's editing commands.

## Install

Download binaries from the [releases page](https://github.com/uthree/auris-studio/releases):
the desktop application, `auris`, `auris-mcp` and `auris-agent` for macOS and Windows,
and `auris` for Linux. Release archives include the standard SoundFont.

On macOS, drag `Auris Studio.app` to `/Applications`. The binaries are unsigned;
if macOS blocks the downloaded app, remove its quarantine attribute with
`xattr -dr com.apple.quarantine "Auris Studio.app"`. On Windows, SmartScreen may require
**More info → Run anyway**.

To build from source with Rust 1.90 or newer:

```sh
cargo run --release
```

Windows also requires Visual Studio C++ Build Tools and LLVM. Run
`.\tools\setup-windows.ps1 -InstallLlvm` in a Visual Studio developer PowerShell terminal
before building. See [Development](docs/development.md#building-on-each-platform) for
platform setup. The desktop downloads the standard SoundFont on first launch if needed.

## Documentation

- [Features and usage](docs/features.md)
- [Automatic composition](docs/composition.md)
- [Agent and MCP workflows](docs/agent-workflows.md)
- [Singing backends](docs/singing-backends.md) and [voice training](training/README.md)
- [Development](docs/development.md) and [composition evaluation](docs/evaluation.md)
- [Changelog and compatibility policy](CHANGELOG.md)

Licensed under [Apache-2.0](LICENSE).
