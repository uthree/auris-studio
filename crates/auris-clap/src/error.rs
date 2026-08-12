//! What can go wrong while hosting somebody else's binary.

use std::path::PathBuf;

/// A failure while loading, instantiating or driving a CLAP plugin.
///
/// Every variant is recoverable: the caller drops the plugin and carries on. A hosted plugin
/// that cannot be loaded leaves its slot empty, exactly as a missing audio file leaves a track
/// silent rather than refusing to open the project.
#[derive(Debug, thiserror::Error)]
pub enum ClapError {
    /// The file could not be opened, or holds no CLAP entry point.
    #[error("`{path}` is not a loadable CLAP plugin: {reason}")]
    Load {
        /// The file that was tried.
        path: PathBuf,
        /// What the loader said.
        reason: String,
    },

    /// The entry loaded, but exposes no plugin factory — so it contains no plugins.
    #[error("`{0}` exposes no plugin factory")]
    NoFactory(PathBuf),

    /// No plugin inside the file carries the requested id.
    #[error("no plugin with id `{0}` in this file")]
    UnknownPlugin(String),

    /// The plugin refused to be instantiated.
    #[error("`{id}` could not be instantiated: {reason}")]
    Instantiate {
        /// The plugin's CLAP id.
        id: String,
        /// What the plugin or the wrapper said.
        reason: String,
    },

    /// The plugin refused to activate at the requested sample rate and block size.
    #[error("`{id}` refused to activate at {sample_rate} Hz, {max_block_frames} frames: {reason}")]
    Activate {
        /// The plugin's CLAP id.
        id: String,
        /// Rate that was asked for.
        sample_rate: f64,
        /// Block size that was asked for.
        max_block_frames: usize,
        /// What the plugin or the wrapper said.
        reason: String,
    },

    /// The plugin does not implement the `params` extension, so nothing can be automated.
    ///
    /// This is not fatal on its own — a plugin with no parameters is legal — but it is worth
    /// reporting, because it is far more often a sign that the wrong file was loaded.
    #[error("`{0}` exposes no parameters")]
    NoParams(String),

    /// Saving or restoring the plugin's own state failed.
    #[error("`{id}` could not {}its state: {reason}", if *.saving { "save " } else { "restore " })]
    State {
        /// The plugin's CLAP id.
        id: String,
        /// `true` when saving, `false` when restoring.
        saving: bool,
        /// What the plugin said.
        reason: String,
    },
}
