# Contributing to Auris Studio

[日本語](CONTRIBUTING.ja.md)

Thank you for taking an interest. Bug reports, ideas, documentation fixes and pull requests are
all welcome.

## Generative AI is welcome

Auris Studio is itself built with heavy use of generative AI, so issues and pull requests that
were written partly or entirely with an AI tool are welcome here. You don't need to hide it or
apologise for it, and saying which tool you used is helpful but not required.

What matters is the same as for any contribution: that the change does what it says, that the
checks below pass, and that you have read what you are submitting.

**One request: please don't flood the repository.** A handful of issues or pull requests at a
time is great; hundreds or thousands in a day is not something one maintainer can read. If an
agent finds many problems at once, group related ones into a single issue or pull request, or
open a few and wait for a response before sending more.

## Reporting an issue

Open an issue on [GitHub](https://github.com/uthree/auris-studio/issues). For a bug, please
include:

* the Auris Studio version and your operating system (macOS on Apple silicon, Windows, or Linux
  for the CLI);
* what you did, what you expected, and what happened instead;
* a project file, a log, or a screenshot if you have one.

English or Japanese are both fine.

## Setting up a development build

You need Rust 1.95 or newer.

```sh
git clone https://github.com/uthree/auris-studio.git
cd auris-studio
cargo run                     # launch the desktop application
cargo run -p auris-cli -- help   # the command line frontend
```

macOS builds with only the Command Line Tools; a full Xcode is not needed. Windows also needs
the Visual Studio C++ Build Tools and LLVM: run `.\tools\setup-windows.ps1 -InstallLlvm` in a
Visual Studio developer PowerShell first. See
[Building on each platform](docs/development.md#building-on-each-platform) for the details.

Before changing code, read [Development](docs/development.md) for how the crates are laid out,
and [AGENTS.md](AGENTS.md) for the working rules (layering, realtime rules, platforms, the project
folder). If you are working with a coding agent, point it at `AGENTS.md`; it is written for
exactly that reader. The full architectural account lives in the `auris_session::guide` module:

```sh
cargo doc -p auris-session --no-deps --open
```

## Checks to run before a pull request

CI runs these on macOS and Windows, so running them locally saves a round trip:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo doc --workspace --no-deps   # CI denies documentation warnings
```

A few things CI is strict about:

* Every public item needs a doc comment (`#![warn(missing_docs)]`), and doc links must resolve.
* DSP code is tested against numbers (levels, frequencies, lengths), not just "it runs".
* The desktop window can be tested without a display through `auris_gpui::harness`; run those
  tests with `cargo test -p auris-gpui --bins`. See
  [Testing the window](docs/development.md#testing-the-window).
* If you touch a composition writer or an audio constant, run the measuring tools described in
  [Evaluation](docs/evaluation.md) before and after.

## Opening a pull request

1. Fork the repository and create a branch from `main`.
2. Keep each pull request to one change. Several small pull requests are easier to review than
   one large one.
3. Write comments and documentation in English. If you change a document that has a Japanese
   version (`*.ja.md`), updating both is appreciated; if you can't, say so and it will be
   handled.
4. In the description, explain what changed and why, and how you checked it. For a visible UI
   change, a screenshot helps a lot.
5. User-facing changes get a line in [CHANGELOG.md](CHANGELOG.md) under the next release, or
   mention it in the description and the maintainer will add one.

Auris Studio is at `0.x`, so the project file format and APIs may change in any release; see
[CHANGELOG.md](CHANGELOG.md) for the compatibility policy. If a change could make an older build
misread a newer project file, bump `Project::FORMAT_VERSION` (its doc comment explains when).

## How releases are made

Releases are cut by the maintainer:

1. A pull request bumps the version in the root `Cargo.toml` (and `Cargo.lock`) and adds a
   `## <version> — <date>` section to [CHANGELOG.md](CHANGELOG.md). The release workflow uses
   that section as the release notes, so the heading format matters.
2. Once it is merged and CI on `main` is green, the maintainer pushes a `v<version>` tag.
3. The tag starts the release workflow, which builds the Windows installer, the macOS disk image
   (Apple silicon) and the Linux CLI archive, and publishes them on the
   [releases page](https://github.com/uthree/auris-studio/releases). Installed copies are then
   offered the update.

Contributors don't need to do anything for a release; merged changes go out in the next one.

## License

Auris Studio is licensed under [Apache-2.0](LICENSE). By submitting a contribution, you agree
that it is licensed under the same terms.
