//! A `.clap` file, and what it says is inside it.

use std::ffi::CStr;
use std::path::{Path, PathBuf};

use auris_core::plugin::{PluginCategory, PluginDescriptor, PluginKind};
use clack_host::prelude::*;

use crate::error::ClapError;
use crate::plugin::ClapPlugin;

/// Prefix given to a hosted plugin's Auris id.
///
/// A project file stores plugin ids as strings, and a CLAP id like `com.u-he.diva` is chosen by
/// somebody else — there is nothing stopping it from colliding with a built-in. The prefix keeps
/// the two namespaces apart and makes the origin of a slot readable in a saved project.
pub const ID_PREFIX: &str = "clap:";

/// One plugin inside a `.clap` file, before it is instantiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapPluginInfo {
    /// The plugin's own CLAP id, e.g. `com.u-he.diva`.
    pub clap_id: String,
    /// Display name.
    pub name: String,
    /// Who wrote it.
    pub vendor: String,
    /// One-line summary, empty when the plugin gives none.
    pub description: String,
    /// The plugin's own version string.
    pub version: String,
    /// Whether it generates audio or transforms it.
    pub kind: PluginKind,
    /// Where a browser should file it.
    pub category: PluginCategory,
}

impl ClapPluginInfo {
    /// The id this plugin is known by inside Auris Studio.
    pub fn auris_id(&self) -> String {
        format!("{ID_PREFIX}{}", self.clap_id)
    }

    /// Presents this plugin the way every other plugin in the application is presented.
    pub fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.auris_id().into(),
            name: self.name.clone().into(),
            vendor: self.vendor.clone().into(),
            description: self.description.clone().into(),
            kind: self.kind,
            category: self.category,
        }
    }
}

/// A loaded `.clap` file.
///
/// One file may hold several plugins, so loading and instantiating are separate steps. The
/// library must outlive every [`ClapPlugin`] made from it; that is not a borrow the compiler can
/// see, because `clack` keeps entries alive by reference count internally, but it is still how
/// the file is meant to be used.
pub struct ClapLibrary {
    entry: PluginEntry,
    path: PathBuf,
}

impl ClapLibrary {
    /// Loads a `.clap` file.
    ///
    /// # Safety
    ///
    /// This maps third-party machine code into the process and calls it. A plugin that does not
    /// follow the CLAP specification can cause undefined behaviour no matter what this crate
    /// does. The caller is asserting that loading this particular file is something the user
    /// asked for.
    pub unsafe fn load(path: impl AsRef<Path>) -> Result<Self, ClapError> {
        let path = path.as_ref().to_path_buf();
        // SAFETY: delegated to this function's own contract.
        let entry = unsafe { PluginEntry::load(&path) }.map_err(|error| ClapError::Load {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        Ok(Self { entry, path })
    }

    /// Builds a library around an entry that is already loaded.
    ///
    /// This is how the tests host a plugin that is linked into the test binary rather than read
    /// from disk — the entry point is reached through the same C ABI either way. There is no
    /// reason for the application to want it, and every reason for the tests to.
    #[cfg(test)]
    pub(crate) fn from_entry(entry: PluginEntry, path: impl AsRef<Path>) -> Self {
        Self {
            entry,
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Where this library was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every plugin the file declares, in the order the file declares them.
    pub fn plugins(&self) -> Result<Vec<ClapPluginInfo>, ClapError> {
        let factory = self
            .entry
            .get_plugin_factory()
            .ok_or_else(|| ClapError::NoFactory(self.path.clone()))?;

        Ok(factory
            .plugin_descriptors()
            .filter_map(|descriptor| {
                let features: Vec<&str> = descriptor
                    .features()
                    .filter_map(|feature| feature.to_str().ok())
                    .collect();
                let (kind, category) = classify(&features);

                Some(ClapPluginInfo {
                    clap_id: text(descriptor.id()?),
                    name: descriptor.name().map(text).unwrap_or_default(),
                    vendor: descriptor.vendor().map(text).unwrap_or_default(),
                    description: descriptor.description().map(text).unwrap_or_default(),
                    version: descriptor.version().map(text).unwrap_or_default(),
                    kind,
                    category,
                })
            })
            .collect())
    }

    /// Instantiates one plugin by its CLAP id.
    ///
    /// The plugin is created but not activated; [`ClapPlugin::activate`] does that once the
    /// sample rate and block size are known.
    pub fn instantiate(&self, clap_id: &str) -> Result<ClapPlugin, ClapError> {
        let info = self
            .plugins()?
            .into_iter()
            .find(|plugin| plugin.clap_id == clap_id)
            .ok_or_else(|| ClapError::UnknownPlugin(clap_id.to_string()))?;

        ClapPlugin::new(&self.entry, info)
    }
}

impl std::fmt::Debug for ClapLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClapLibrary")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Reads a C string the plugin owns, replacing anything that is not UTF-8.
fn text(raw: &CStr) -> String {
    raw.to_string_lossy().into_owned()
}

/// Decides what a plugin *is* from the tags it declares.
///
/// CLAP's feature list is free-form: a plugin may declare any strings it likes, in any order,
/// and many declare none at all. So this is a best guess whose only job is to put the plugin
/// somewhere a user can find it — nothing downstream depends on it being right.
///
/// The kind, though, is not cosmetic: an instrument goes in a track's instrument slot and an
/// effect goes in its chain. A plugin that declares nothing is treated as an effect, because a
/// misfiled effect is silent in a chain while a misfiled instrument replaces the one that was
/// making the sound.
pub fn classify(features: &[&str]) -> (PluginKind, PluginCategory) {
    let has = |wanted: &str| features.contains(&wanted);

    let kind = match has("instrument") || has("synthesizer") || has("sampler") || has("drum") {
        true => PluginKind::Instrument,
        false => PluginKind::Effect,
    };

    // First match wins, so the order is the specificity order: a "drum-machine" that also says
    // "sampler" is a drum machine.
    let table: [(&str, PluginCategory); 21] = [
        ("drum-machine", PluginCategory::Drum),
        ("drum", PluginCategory::Drum),
        ("sampler", PluginCategory::Sampler),
        ("synthesizer", PluginCategory::Synth),
        ("equalizer", PluginCategory::Equalizer),
        ("filter", PluginCategory::Equalizer),
        ("de-esser", PluginCategory::Dynamics),
        ("compressor", PluginCategory::Dynamics),
        ("expander", PluginCategory::Dynamics),
        ("limiter", PluginCategory::Dynamics),
        ("gate", PluginCategory::Dynamics),
        ("transient-shaper", PluginCategory::Dynamics),
        ("distortion", PluginCategory::Distortion),
        ("chorus", PluginCategory::Modulation),
        ("flanger", PluginCategory::Modulation),
        ("phaser", PluginCategory::Modulation),
        ("tremolo", PluginCategory::Modulation),
        ("delay", PluginCategory::Delay),
        ("reverb", PluginCategory::Reverb),
        ("utility", PluginCategory::Utility),
        ("analyzer", PluginCategory::Utility),
    ];

    let category = table
        .iter()
        .find(|(feature, _)| has(feature))
        .map(|(_, category)| *category)
        .unwrap_or(match kind {
            PluginKind::Instrument => PluginCategory::Synth,
            PluginKind::Effect => PluginCategory::Other,
        });

    (kind, category)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_that_declares_nothing_is_an_effect() {
        // Not an arbitrary tie-break: a misfiled effect sits in a chain doing nothing, while a
        // misfiled instrument would take the place of whatever was making the sound.
        assert_eq!(
            classify(&[]),
            (PluginKind::Effect, PluginCategory::Other),
            "an untagged plugin must not be offered as an instrument"
        );
        assert_eq!(
            classify(&["stereo"]),
            (PluginKind::Effect, PluginCategory::Other)
        );
    }

    #[test]
    fn the_instrument_tags_all_make_an_instrument() {
        for tag in ["instrument", "synthesizer", "sampler", "drum"] {
            let (kind, _) = classify(&[tag]);
            assert_eq!(
                kind,
                PluginKind::Instrument,
                "`{tag}` should be an instrument"
            );
        }
    }

    #[test]
    fn the_more_specific_tag_wins() {
        assert_eq!(
            classify(&["instrument", "sampler", "drum-machine", "stereo"]),
            (PluginKind::Instrument, PluginCategory::Drum)
        );
        assert_eq!(
            classify(&["audio-effect", "filter", "equalizer"]),
            (PluginKind::Effect, PluginCategory::Equalizer)
        );
    }

    #[test]
    fn an_untagged_instrument_still_lands_under_synth() {
        // "instrument" alone is by far the most common declaration, and Other would bury it.
        assert_eq!(
            classify(&["instrument", "stereo"]),
            (PluginKind::Instrument, PluginCategory::Synth)
        );
    }

    #[test]
    fn a_hosted_id_cannot_collide_with_a_built_in() {
        let info = ClapPluginInfo {
            clap_id: "auris.synth.pulse".into(),
            name: "Impostor".into(),
            vendor: String::new(),
            description: String::new(),
            version: String::new(),
            kind: PluginKind::Effect,
            category: PluginCategory::Other,
        };
        assert_eq!(info.auris_id(), "clap:auris.synth.pulse");
        assert_ne!(info.auris_id(), "auris.synth.pulse");
        assert_eq!(info.descriptor().id, info.auris_id());
    }
}
