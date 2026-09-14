//! Error types for the vst3-host library

use thiserror::Error;

/// Main error type for vst3-host operations.
///
/// Marked `#[non_exhaustive]`: match with a wildcard arm, as new variants may be added in
/// future versions without it being a breaking change.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// Plugin file not found
    #[error("Plugin not found: {0}")]
    PluginNotFound(String),

    /// Failed to load plugin
    #[error("Failed to load plugin: {0}")]
    PluginLoadFailed(String),

    /// Plugin crashed during operation
    #[error("Plugin crashed")]
    PluginCrashed,

    /// Plugin operation timed out
    #[error("Plugin operation timed out")]
    PluginTimeout,

    /// Invalid parameter
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// Audio backend error
    #[error("Audio backend error: {0}")]
    AudioBackendError(String),

    /// MIDI error
    #[error("MIDI error: {0}")]
    MidiError(String),

    /// COM/VST3 interface error
    #[error("VST3 interface error: {0}")]
    InterfaceError(String),

    /// Process isolation error
    #[error("Process isolation error: {0}")]
    ProcessError(String),

    /// IO error
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    /// The plugin's `process()` returned a failure code. Carries the raw tresult rather than a
    /// formatted `String` so returning it from the audio callback allocates nothing.
    #[error("Plugin process() failed: {0:#x}")]
    ProcessFailed(i32),

    /// The plugin rejected a realtime `IAudioProcessor::setProcessing` transition. Carries only
    /// scalars so returning the error from an audio-thread reset does not allocate.
    #[error("Plugin setProcessing({requested}) failed: {code:#x}")]
    ProcessingStateFailed {
        /// Whether the rejected transition requested processing to start (`true`) or stop.
        requested: bool,
        /// Raw VST3 `tresult` returned by the plugin.
        code: i32,
    },

    /// The plugin is not currently active/processing. A unit variant for the same reason as
    /// [`Self::ProcessFailed`] — this is rejected on the audio path once per block while stopped.
    #[error("Plugin is not processing")]
    NotProcessing,

    /// A preallocated realtime event or parameter queue could not hold the complete batch.
    ///
    /// This is a unit variant so rejecting the batch from an audio callback allocates nothing.
    #[error("Realtime plugin queue capacity exceeded")]
    RealtimeCapacityExceeded,

    /// A caller supplied invalid data to an exclusively owned realtime instance.
    /// Kept allocation-free because validation can fail on an audio callback.
    #[error("Invalid realtime plugin input")]
    RealtimeInputInvalid,

    /// Preallocated realtime processing state was unexpectedly unavailable.
    /// Kept allocation-free because this is detected on an audio callback.
    #[error("Realtime plugin state is unavailable")]
    RealtimeStateInvalid,

    /// Other errors
    #[error("{0}")]
    Other(String),
}

/// Convenient Result type alias
pub type Result<T> = std::result::Result<T, Error>;
