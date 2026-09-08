//! Exact instrument assets selected from the sound library.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The exact preset or external instrument a part asks the session to resolve.
///
/// This is an asset description, not a plugin instance: the composer carries it unchanged and
/// remains independent of the sampler and external plugin hosts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartSource {
    /// One preset in a particular SoundFont file.
    SoundFont {
        /// The SoundFont file, resolved by the session.
        path: PathBuf,
        /// The bank number in the SoundFont header, from 0 to 65535.
        bank: i32,
        /// The preset number in the SoundFont header, from 0 to 65535.
        patch: i32,
    },
    /// One instrument descriptor exported by a CLAP plugin file.
    Clap {
        /// The CLAP plugin file.
        path: PathBuf,
        /// The descriptor's stable plugin identifier.
        plugin_id: String,
    },
    /// One instrument class exported by a VST3 module.
    Vst3 {
        /// The VST3 module or bundle.
        path: PathBuf,
        /// The class's stable identifier.
        class_id: String,
    },
}
