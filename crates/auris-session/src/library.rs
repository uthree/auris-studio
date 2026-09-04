//! The sound library the application is packaged with.
//!
//! The built-in instruments are two oscillators, a noise drum and a preview voice. They are
//! enough to hear a piece back and nowhere near enough to *write* one, so a build of Auris
//! Studio ships with a General MIDI SoundFont beside it and every frontend finds it here.
//!
//! # Why the font is not in the repository
//!
//! [`GENERAL_MIDI`] is two hundred megabytes. GitHub refuses a single file over a hundred, and a
//! repository that carried one would charge every clone for it forever — including the clones
//! that only ever build the command line tool. So the bytes are fetched rather than committed:
//! `tools/fetch-soundfonts.sh` downloads them, checks them against [`ShippedFont::sha256`], and
//! writes them into a [`LIBRARY_FOLDER`] directory. The release workflow runs it before it
//! assembles each archive, and a developer runs it once.
//!
//! What is committed is this manifest — the name, the size, the digest and the licence — which is
//! the part that has to be reviewable.
//!
//! # Where the font is looked for
//!
//! [`library_roots`] answers that, and the answer is deliberately several places: beside the
//! executable is where a release archive puts it, `Contents/Resources` is where a macOS bundle
//! does, a few directories above the executable is where a `cargo run` build finds the one a
//! developer fetched into the checkout, and the configuration directory is where somebody who
//! installed a font by hand would have put it. [`LIBRARY_DIR_VAR`] overrides the lot.
//!
//! Nothing here reads a font — a loaded font belongs to a session's bank and a path does not.
//! [`Session`](crate::Session) reads what it finds here whenever a document is created or opened,
//! unless [`SessionOptions::shipped_fonts`](crate::SessionOptions::shipped_fonts) says otherwise.

use std::path::{Path, PathBuf};

use crate::settings::config_dir;

/// Environment variable naming the directory the shipped SoundFonts are installed in.
///
/// Set it and nothing else is searched, which is what makes a font kept on another volume — they
/// are large enough for that to be a real arrangement — usable without moving it.
pub const LIBRARY_DIR_VAR: &str = "AURIS_SOUNDFONTS";

/// What the directory holding the shipped SoundFonts is called, wherever it turns up.
pub const LIBRARY_FOLDER: &str = "SoundFonts";

/// The id of the General MIDI font the composer reaches for.
///
/// MuseScore's, which is the FluidR3 set of Frank Wen's that everything else quotes, remastered
/// and still under the same MIT licence. Choosing between the two is choosing between an original
/// and a curated version of itself, and the curated one is smaller and better recorded.
pub const GENERAL_MIDI: &str = "musescore-general";

/// A SoundFont the application is packaged with.
///
/// Everything needed to fetch one, verify it and say where it came from — which is the whole of
/// what a manifest is for. The bytes themselves are never in the repository; see the module
/// documentation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ShippedFont {
    /// Short name a specification, a command line or [`shipped`] refers to it by.
    pub id: &'static str,
    /// What the file is called, wherever it is installed.
    pub file: &'static str,
    /// What to call it in an interface.
    pub name: &'static str,
    /// The licence it is redistributed under.
    pub license: &'static str,
    /// Where the licence text lives, fetched and installed beside the font.
    ///
    /// The MIT licence asks that the copyright notice travel with the work, and a font in a
    /// release archive with no notice beside it would not be honouring that. It is fetched rather
    /// than committed for one reason only — that it is the *font's* notice, and belongs next to
    /// the font rather than in a source tree that may not have one.
    pub license_url: &'static str,
    /// Where the fetch script downloads it from.
    pub url: &'static str,
    /// Its length in bytes.
    pub bytes: u64,
    /// SHA-256 of the file, in lowercase hexadecimal.
    ///
    /// Checked by the fetch script, not by the application: a two-hundred-megabyte digest at
    /// every start-up would cost more than it could ever catch, and a font that arrived corrupt
    /// fails at the parser a moment later with a message naming the file.
    pub sha256: &'static str,
}

/// Every font the fetch script knows how to install.
pub const SHIPPED: &[ShippedFont] = &[ShippedFont {
    id: GENERAL_MIDI,
    file: "MuseScore_General.sf2",
    name: "MuseScore General",
    license: "MIT",
    license_url: "https://ftp.osuosl.org/pub/musescore/soundfont/MuseScore_General/MuseScore_General_License.md",
    url: "https://ftp.osuosl.org/pub/musescore/soundfont/MuseScore_General/MuseScore_General.sf2",
    bytes: 215_614_036,
    sha256: "ee51d2c4b1525e70f19a45909c4fd7a2e26d91d115fa89dbf5a6bc413d8b9bf3",
}];

/// The manifest entry with this id.
pub fn shipped(id: &str) -> Option<&'static ShippedFont> {
    SHIPPED.iter().find(|font| font.id == id)
}

/// Environment variable naming the directory the shipped Japanese dictionary is installed in.
///
/// The [`LIBRARY_DIR_VAR`] arrangement, for the same reason: set it and nothing else is
/// searched.
pub const DICTIONARY_DIR_VAR: &str = "AURIS_DICTIONARY";

/// What the directory holding the shipped Japanese dictionary is called, wherever it turns up.
pub const DICTIONARY_FOLDER: &str = "Dictionary";

/// The Japanese dictionary a build ships with — a folder rather than a file, which is the one
/// way it differs from a [`ShippedFont`].
///
/// What it is for: kanji lyrics, and the pitch accent that makes a composed melody follow the
/// words. Kana sings without it; this is what reads everything else.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ShippedDictionary {
    /// Short name commands and messages refer to it by.
    pub id: &'static str,
    /// The directory the archive unpacks to, and what the folder is called wherever it is
    /// installed.
    pub folder: &'static str,
    /// What to call it in an interface.
    pub name: &'static str,
    /// The licence it is redistributed under.
    pub license: &'static str,
    /// Where the licence text lives, fetched and installed beside the folder — the
    /// [`ShippedFont::license_url`] reasoning: the notice travels with the work.
    pub license_url: &'static str,
    /// Where the fetch script downloads the archive from.
    ///
    /// The v0.14.0 tarball on purpose: jpreprocess's v0.15.0 release ships no standalone
    /// dictionary archive, only per-platform binaries with one baked in, and the v0.14.0
    /// folder loads under the 0.15 crate — proven by the accent test this repository runs
    /// against it, not assumed. Revisit when a newer standalone archive appears.
    pub url: &'static str,
    /// The archive's length in bytes.
    pub bytes: u64,
    /// SHA-256 of the archive, in lowercase hexadecimal — checked by the fetch script, like a
    /// font's.
    pub sha256: &'static str,
}

/// The dictionary the fetch script knows how to install: NAIST's, in jpreprocess's build.
pub const JAPANESE_DICTIONARY: ShippedDictionary = ShippedDictionary {
    id: "naist-jdic",
    folder: "naist-jdic",
    name: "NAIST Japanese Dictionary",
    license: "BSD-3-Clause",
    license_url: "https://raw.githubusercontent.com/jpreprocess/naist-jdic/main/COPYING",
    url: "https://github.com/jpreprocess/jpreprocess/releases/download/v0.14.0/naist-jdic-jpreprocess.tar.gz",
    bytes: 28_668_709,
    sha256: "d96062f8dc546caa4579a8fc1e3c0baf0a2863b2b8719675c0cbf305c299e52f",
};

/// Directories the shipped dictionary is looked for in, best first.
pub fn dictionary_roots() -> Vec<PathBuf> {
    roots_from(
        std::env::var_os(DICTIONARY_DIR_VAR).map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
        &config_dir(),
        DICTIONARY_FOLDER,
    )
}

/// Where the shipped dictionary is installed, or `None` when it is not.
///
/// A folder answers when it holds the metadata file jpreprocess writes into every dictionary
/// it builds — the cheapest marker that distinguishes an installed dictionary from a
/// half-extracted archive or a stray directory wearing the right name.
pub fn installed_dictionary() -> Option<PathBuf> {
    installed_dictionary_in(&dictionary_roots())
}

/// [`installed_dictionary`] against a given set of directories.
pub fn installed_dictionary_in(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join(JAPANESE_DICTIONARY.folder))
        .find(|candidate| candidate.join("metadata.json").is_file())
}

/// Environment variable naming the directory singer voices are kept in.
///
/// The [`LIBRARY_DIR_VAR`] arrangement once more: set it and nothing else is searched.
pub const VOICES_DIR_VAR: &str = "AURIS_VOICES";

/// What the directory holding singer voice models is called, wherever it turns up.
pub const VOICES_FOLDER: &str = "Voices";

/// Directories singer voices are looked for in, best first.
///
/// The same walk the fonts make, under its own name — beside the executable, in a bundle's
/// resources, up through a checkout, and in the configuration directory, which is where a
/// person who wants a voice "installed" most naturally drops it.
pub fn voice_roots() -> Vec<PathBuf> {
    roots_from(
        std::env::var_os(VOICES_DIR_VAR).map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
        &config_dir(),
        VOICES_FOLDER,
    )
}

/// Every voice model found under `roots`, as `(name, path)`, sorted by name.
///
/// No manifest, unlike the fonts: voices are the user's own exports, so this is enumeration
/// rather than verification — every `.onnx` in a root and every child folder containing a
/// DiffSinger `dsconfig.yaml`, and every `*.voicevox.json` connection, first root to name a
/// voice winning the way the font search wins.
/// Nothing is opened here; whether a file really is a voice is found out by the one deliberate
/// click that loads it.
pub fn installed_voices_in(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut voices: Vec<(String, PathBuf)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let (voice, name_source) = if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("onnx"))
            {
                (path.clone(), path.file_stem())
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".voicevox.json"))
            {
                let stem = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| &name[..name.len() - ".voicevox.json".len()]);
                (path.clone(), stem.map(std::ffi::OsStr::new))
            } else if path.is_dir() && path.join("dsconfig.yaml").is_file() {
                (path.join("dsconfig.yaml"), path.file_name())
            } else {
                continue;
            };
            let name = name_source
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "Voice".to_string());
            let identity = name.to_lowercase();
            if seen.contains(&identity) {
                continue;
            }
            seen.push(identity);
            voices.push((name, voice));
        }
    }
    voices.sort_by(|a, b| a.0.cmp(&b.0));
    voices
}

/// The backend implied by an installed voice entry's path.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoiceSourceKind {
    /// A self-contained voice exported by the Auris trainer.
    Auris,
    /// An OpenUtau-compatible DiffSinger deployment.
    DiffSinger,
    /// A connection to a running VOICEVOX Engine.
    Voicevox,
}

/// Identifies a voice shelf entry without loading its potentially large model.
pub fn voice_source_kind(path: &Path) -> Option<VoiceSourceKind> {
    let name = path.file_name()?.to_str()?;
    if name.eq_ignore_ascii_case("dsconfig.yaml") {
        Some(VoiceSourceKind::DiffSinger)
    } else if name.to_ascii_lowercase().ends_with(".voicevox.json") {
        Some(VoiceSourceKind::Voicevox)
    } else if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("onnx"))
    {
        Some(VoiceSourceKind::Auris)
    } else {
        None
    }
}

/// Directories the shipped SoundFonts are looked for in, best first.
pub fn library_roots() -> Vec<PathBuf> {
    roots_from(
        std::env::var_os(LIBRARY_DIR_VAR).map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
        &config_dir(),
        LIBRARY_FOLDER,
    )
}

/// How many directories *above* the executable are still searched.
///
/// A release archive puts the library beside the binary, so none would do there. A `cargo run`
/// build puts the binary in `target/debug` and the fetched font at the top of the checkout — two —
/// and a cross-compiled release adds `target/<triple>/release`, which is three. Five leaves room
/// for a layout nobody has thought of yet and still stops well short of the file system root,
/// where a stray `SoundFonts` directory would be somebody else's.
const LIBRARY_SEARCH_DEPTH: usize = 5;

/// [`library_roots`] with the environment passed in, so it can be tested without setting
/// variables or moving the executable.
///
/// An empty override counts as unset, for the same reason [`config_dir`] treats one that way: a
/// shell exporting `AURIS_SOUNDFONTS=` would otherwise search the file system root.
fn roots_from(
    override_dir: Option<PathBuf>,
    executable: Option<&Path>,
    config: &Path,
    folder: &str,
) -> Vec<PathBuf> {
    if let Some(dir) = override_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        return vec![dir];
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    };

    if let Some(directory) = executable.and_then(Path::parent) {
        push(directory.join(folder));
        // Then a macOS bundle's resource directory, which is one *sideways* rather than up: a
        // font in `Contents/MacOS` beside the executable is a font Gatekeeper complains about,
        // so a packaged one goes in `Contents/Resources` and is reached from nowhere else.
        if let Some(contents) = directory.parent() {
            push(contents.join("Resources").join(folder));
        }
        for ancestor in directory.ancestors().skip(1).take(LIBRARY_SEARCH_DEPTH) {
            push(ancestor.join(folder));
        }
    }
    push(config.join(folder));
    roots
}

/// Where this font is installed, or `None` when it is not.
pub fn installed(font: &ShippedFont) -> Option<PathBuf> {
    installed_in(font, &library_roots())
}

/// [`installed`] against a given set of directories.
///
/// A file is taken at face value rather than weighed against [`ShippedFont::bytes`]: somebody who
/// put a font of their own under that name in the library directory meant to, and the size that
/// would refuse it is a fact about a download rather than about the sound. What the size and the
/// digest guard is the fetch, which writes to a temporary name and renames only once both agree —
/// so a half-finished download is never a file this can find.
pub fn installed_in(font: &ShippedFont, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join(font.file))
        .find(|candidate| candidate.is_file())
}

/// Every shipped font that is actually on this machine, with where it was found.
pub fn installed_fonts() -> Vec<(&'static ShippedFont, PathBuf)> {
    let roots = library_roots();
    SHIPPED
        .iter()
        .filter_map(|font| installed_in(font, &roots).map(|path| (font, path)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_describes_a_font_that_could_be_fetched() {
        let font = shipped(GENERAL_MIDI).expect("the General MIDI font is shipped");
        assert!(font.url.starts_with("https://"), "fetched over TLS");
        assert!(
            font.license_url.starts_with("https://"),
            "and its notice with it, which the licence asks for"
        );
        assert!(font.file.ends_with(".sf2"), "and readable by the sampler");
        assert_eq!(font.sha256.len(), 64, "a SHA-256 is sixty-four hex digits");
        assert!(font.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(font.bytes > 0);
        assert!(shipped("no-such-font").is_none());
    }

    #[test]
    fn the_header_counts_the_instruments_the_registry_installs() {
        // The first sentence of this module is the reason a font ships at all, so its count has
        // to be the count `default_registry` actually installs — everything but the sampler,
        // which is the instrument this font is *for* and makes no sound until one arrives.
        let registry = crate::plugin_catalogue();
        let without_a_font: Vec<&str> = registry
            .instruments()
            .map(|descriptor| descriptor.id.as_ref())
            .filter(|id| *id != auris_sampler::SAMPLER_ID)
            .collect();
        assert_eq!(
            without_a_font.len(),
            4,
            "three pitched voices and a drum: {without_a_font:?}"
        );
        // The header itself, which is the half of this that nothing else checks. Its own lines
        // rather than the whole file, or the phrase written here would answer for it; joined as
        // one line so that rewrapping the paragraph is not a failure.
        let header: String = include_str!("library.rs")
            .lines()
            .filter_map(|line| line.strip_prefix("//!"))
            .map(str::trim)
            .collect::<Vec<&str>>()
            .join(" ");
        assert!(
            header.contains("two oscillators, a noise drum and a preview voice"),
            "the header counts them in prose, and the prose is what a reader gets"
        );
    }

    #[test]
    fn the_override_is_the_whole_answer() {
        // The point of the variable is a font kept somewhere else entirely; searching on past it
        // would quietly prefer a stale copy beside the executable.
        let roots = roots_from(
            Some(PathBuf::from("/volumes/samples")),
            Some(Path::new("/apps/auris-studio")),
            Path::new("/home/me/.config/auris-studio"),
            LIBRARY_FOLDER,
        );
        assert_eq!(roots, vec![PathBuf::from("/volumes/samples")]);
    }

    #[test]
    fn an_empty_override_is_not_an_override() {
        let roots = roots_from(
            Some(PathBuf::new()),
            None,
            Path::new("/home/me/.config/auris-studio"),
            LIBRARY_FOLDER,
        );
        assert_eq!(
            roots,
            vec![PathBuf::from("/home/me/.config/auris-studio/SoundFonts")]
        );
    }

    #[test]
    fn a_release_archive_is_searched_before_anywhere_else() {
        let roots = roots_from(
            None,
            Some(Path::new("/downloads/auris-studio-v0.1.0/auris-studio")),
            Path::new("/home/me/.config/auris-studio"),
            LIBRARY_FOLDER,
        );
        assert_eq!(
            roots[0],
            PathBuf::from("/downloads/auris-studio-v0.1.0/SoundFonts"),
            "beside the binary, which is what the archive holds"
        );
        assert_eq!(
            roots.last(),
            Some(&PathBuf::from("/home/me/.config/auris-studio/SoundFonts"))
        );
    }

    #[test]
    fn a_macos_bundle_keeps_its_font_in_resources() {
        let roots = roots_from(
            None,
            Some(Path::new(
                "/Applications/Auris Studio.app/Contents/MacOS/auris-studio",
            )),
            Path::new("/Users/me/.config/auris-studio"),
            LIBRARY_FOLDER,
        );
        assert!(
            roots.contains(&PathBuf::from(
                "/Applications/Auris Studio.app/Contents/Resources/SoundFonts"
            )),
            "the bundle's resource directory is where a packaged font goes: {roots:?}"
        );
    }

    #[test]
    fn a_cargo_build_finds_the_font_a_developer_fetched_into_the_checkout() {
        // `cargo run` is two levels down, and a cross-compiled release build is three. Both have
        // to reach the `SoundFonts` directory the fetch script writes at the top of the checkout,
        // or a developer would have to copy two hundred megabytes into `target` after every
        // `cargo clean`.
        for executable in [
            "/checkout/target/debug/auris-studio",
            "/checkout/target/aarch64-apple-darwin/release/auris-studio",
        ] {
            let roots = roots_from(
                None,
                Some(Path::new(executable)),
                Path::new("/home/me/.config/auris-studio"),
                LIBRARY_FOLDER,
            );
            assert!(
                roots.contains(&PathBuf::from("/checkout/SoundFonts")),
                "{executable} should reach the checkout: {roots:?}"
            );
        }
    }

    #[test]
    fn a_root_is_named_once_however_many_ways_it_is_reached() {
        // The walk upwards and the configuration directory can name the same place — a checkout
        // that *is* somebody's config directory is unusual and entirely legal — and a directory
        // searched twice is a second failed lookup for every font in the manifest.
        let roots = roots_from(
            None,
            Some(Path::new("/checkout/target/debug/auris-studio")),
            Path::new("/checkout"),
            LIBRARY_FOLDER,
        );
        assert!(roots.contains(&PathBuf::from("/checkout/SoundFonts")));
        let mut unique = roots.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), roots.len(), "{roots:?}");
    }

    #[test]
    fn the_dictionary_manifest_describes_an_archive_that_could_be_fetched() {
        let dictionary = JAPANESE_DICTIONARY;
        assert!(dictionary.url.starts_with("https://"), "fetched over TLS");
        assert!(dictionary.license_url.starts_with("https://"));
        assert!(
            dictionary.url.ends_with(".tar.gz"),
            "an archive of a folder"
        );
        assert_eq!(dictionary.sha256.len(), 64);
        assert!(dictionary.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(dictionary.bytes > 0);

        // The dictionary walks the same roots the fonts walk, under its own name.
        let roots = roots_from(
            None,
            Some(Path::new("/checkout/target/debug/auris-studio")),
            Path::new("/home/me/.config/auris-studio"),
            DICTIONARY_FOLDER,
        );
        assert!(roots.contains(&PathBuf::from("/checkout/Dictionary")));
        // And an empty machine answers honestly.
        assert_eq!(installed_dictionary_in(&[]), None);
        assert_eq!(
            installed_dictionary_in(&[PathBuf::from("/no/such/place")]),
            None
        );
    }

    #[test]
    fn voices_are_enumerated_named_and_deduplicated() {
        let root = std::env::temp_dir().join(format!("auris-voices-test-{}", std::process::id()));
        let first = root.join("a");
        let second = root.join("b");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("Ritsu.onnx"), b"x").unwrap();
        std::fs::write(first.join("notes.txt"), b"x").unwrap();
        std::fs::write(second.join("Ritsu.ONNX"), b"y").unwrap();
        std::fs::write(second.join("Alto.ONNX"), b"y").unwrap();
        std::fs::write(first.join("Zundamon.voicevox.json"), b"{}").unwrap();
        let diffsinger = first.join("Momo");
        std::fs::create_dir_all(&diffsinger).unwrap();
        std::fs::write(diffsinger.join("dsconfig.yaml"), b"acoustic: acoustic.onnx").unwrap();

        let voices = installed_voices_in(&[first.clone(), second]);
        let names: Vec<&str> = voices.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["Alto", "Momo", "Ritsu", "Zundamon"],
            "sorted, case-blind, no .txt"
        );
        let ritsu = voices.iter().find(|(name, _)| name == "Ritsu").unwrap();
        assert!(ritsu.1.starts_with(&first), "the first root wins the name");
        let momo = voices.iter().find(|(name, _)| name == "Momo").unwrap();
        assert_eq!(momo.1, diffsinger.join("dsconfig.yaml"));
        assert_eq!(
            voice_source_kind(&momo.1),
            Some(VoiceSourceKind::DiffSinger)
        );
        let zundamon = voices.iter().find(|(name, _)| name == "Zundamon").unwrap();
        assert_eq!(
            voice_source_kind(&zundamon.1),
            Some(VoiceSourceKind::Voicevox)
        );
        assert_eq!(voice_source_kind(&ritsu.1), Some(VoiceSourceKind::Auris));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nothing_is_installed_when_nothing_is_there() {
        let font = shipped(GENERAL_MIDI).expect("shipped");
        assert_eq!(installed_in(font, &[]), None);
        assert_eq!(
            installed_in(font, &[PathBuf::from("/no/such/directory")]),
            None
        );
    }
}
