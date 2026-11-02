# Installing Auris Studio

[日本語](installation.ja.md)

The [release page](https://github.com/uthree/auris-studio/releases) offers desktop installers
and portable archives. Desktop releases contain the application, the `auris` CLI, the
`auris-mcp` server, the update helper and the Japanese dictionary from one build. Linux releases
contain the CLI and dictionary. Built-in physical instruments do not require a separate sound
library.

Desktop apps check for updates shortly after launch and every six hours while open. The start
screen shows available releases, download progress and a restart action when an update is ready.
When a newer release has an installer for the current installed app, Auris Studio downloads it
in the background and checks its size and SHA-256 digest against the release metadata. Choose
**Restart and Update** when prompted, or leave the downloaded update ready for the next normal
quit. The update helper applies it after Auris Studio exits. Turn background updates off under
**Settings → General → Automatic Updates**.

**Help → Check for Updates** starts the same check immediately. Portable copies and builds from
source show **Open Release Page** instead of replacing themselves; a copy installed with
`cargo install` also shows the command that upgrades it. If a release has no suitable
installer, use the release page and the platform steps below.

## Windows

Run `auris-studio-v<version>-windows-x86_64-setup.exe`. The installer uses the current user's
`AppData/Local/Programs/Auris Studio` directory, so installation does not require an
administrator account. It creates a Start Menu shortcut and offers a desktop shortcut. The
command-line tools are added to the **user** `PATH` by default; open a new terminal after
installation to use `auris` and `auris-mcp` by name. The installer places the dictionary beside
the executables, where both tools find it. The update helper is installed beside
`auris-studio.exe` and is used by the desktop application's updater.

Run a newer setup file to upgrade the same installation manually. Use Windows **Installed apps** to
uninstall it. Uninstall removes the files and the `PATH` entry added by the installer; projects
and user settings are not installation files and remain available. The portable ZIP still runs
without installation when its folder is kept intact.

Windows v0.10.0 and v0.11.0 need one manual upgrade to v0.11.1 or later: download and run the
new setup file from the release page. Their update helper can crash before the installer starts,
so **Restart and Update** cannot install its own repair. The newer installer replaces the helper
for subsequent automatic updates.

The installer is currently unsigned. Windows SmartScreen may require **More info → Run anyway**
after the publisher and download source have been checked.

## macOS

Auris Studio runs on Apple Silicon Macs; Intel Macs are no longer supported. Builds up to 0.18
were universal and look for an update under the old `macos-universal` name, so updating from one
of them is a manual download, once.

Open `auris-studio-v<version>-macos-aarch64.dmg` and drag `Auris Studio.app` to
**Applications**. The app contains matching command-line executables and the dictionary:

```sh
"/Applications/Auris Studio.app/Contents/MacOS/auris" help
"/Applications/Auris Studio.app/Contents/MacOS/auris-mcp"
```

The DMG does not add commands to the shell `PATH`. For a terminal shortcut, create a link in a
directory already on the user's `PATH`; agents can also use the paths above directly. Replace
the application in Applications with a newer version to upgrade it manually. Automatic updates
require the current user to be able to replace the app bundle in `/Applications` or
`~/Applications`. The portable tar.gz keeps root-level `auris` and `auris-mcp` links to the same
executables inside its app bundle.

The application bundle also contains the update helper beside `auris-studio`. The application is
currently unsigned and not notarized. macOS may block its first launch;
see the release notes for the current manual opening instructions. macOS 13.4 or newer is
required.

## Linux

Extract `auris-v<version>-linux-x86_64.tar.gz` and keep `auris` beside its `Dictionary`
directory. Linux releases do not include the desktop application or MCP server.

## Building and installing from source

Install Rust 1.95 or newer and Git, then complete the prerequisites for your OS in
[Development](development.md#building-on-each-platform). Clone the repository and run the desktop:

```sh
git clone https://github.com/uthree/auris-studio.git
cd auris-studio
cargo run --release
```

On macOS and Windows, install the desktop, CLI and MCP executables with Cargo from the
repository root:

```sh
cargo install --locked --path crates/auris-gpui
cargo install --locked --path crates/auris-cli
cargo install --locked --path crates/auris-mcp
```

For a CLI-only installation, including on Linux, run only the `auris-cli` command above.
Cargo builds release binaries and installs them in its bin directory (by default
`~/.cargo/bin`, or `%USERPROFILE%\.cargo\bin` on Windows). With that directory on `PATH`,
run `auris-studio`, `auris help`, or `auris-mcp`. Cargo can also install a release straight
from its tag, without a checkout:

```sh
cargo install --locked --git https://github.com/uthree/auris-studio --tag v<version> \
  auris-gpui auris-cli auris-mcp
```

Cargo installs executables only. When `auris-studio` starts from Cargo's bin directory, the start
screen offers to add it to the system as an application: `~/Applications/Auris Studio.app` on
macOS, a Start menu shortcut on Windows, or a `.desktop` entry in `~/.local/share/applications`
on Linux. The Windows and Linux shortcuts open Cargo's executable directly. The macOS bundle
holds a copy, because the bundle an executable is in is what gives it its name, icon and
microphone permission; when a later `cargo install` replaces Cargo's executable, the next launch
from the bundle copies the new one in first. An existing shortcut that this offer did not make,
such as a released `Auris Studio.app`, is never replaced.

Cargo installs are not updated by the in-app updater. When a newer release is published, the
start screen and **Help → Check for Updates** show the command that upgrades this installation
instead, with a button that copies it: `cargo install … --tag v<version>` for an install from
Git, or `git pull` followed by the same `cargo install --path` commands for an install from a
checkout.

Cargo does not install the Japanese lyrics dictionary. Follow the dictionary setup in
[Development](development.md#building-on-each-platform) if you use kanji lyrics.

## Building the installers

Tagging a release builds the portable archives, Windows setup executable and macOS DMG in
`.github/workflows/release.yml`. `workflow_dispatch` can build them without publishing. The
Windows setup uses `tools/installer/auris-studio.iss` and Inno Setup 6's `ISCC.exe`; its
`StageDir`, `OutputDir`, `AppVersion` and `IconPath` definitions are supplied by the workflow.
The macOS runner assembles the app bundle and uses `hdiutil` to create the DMG. The release
workflow does not sign either installer yet.
