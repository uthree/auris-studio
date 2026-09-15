//! Errors produced while opening audio hardware or rendering a project.

use thiserror::Error;

/// Everything that can go wrong in the engine.
#[derive(Debug, Error)]
pub enum EngineError {
    /// A requested host is not available in this build or on this platform.
    #[error("audio host `{0}` is not available")]
    HostUnavailable(String),

    /// The host reported no default output device.
    #[error("no default audio output device is available")]
    NoOutputDevice,

    /// The host reported no default input device, so there is nothing to record from.
    #[error("no audio input device is available")]
    NoInputDevice,

    /// The input device reports a frame wider than one preallocated callback buffer.
    #[error(
        "input device has {channels} channels, above the {limit}-channel realtime capture limit"
    )]
    CaptureChannelsTooWide {
        /// Channels reported by the device.
        channels: usize,
        /// Greatest frame width the fixed callback pool can retain atomically.
        limit: usize,
    },

    /// The device's sample format is not one the engine knows how to read or write.
    #[error("unsupported sample format `{0}`")]
    UnsupportedSampleFormat(String),

    /// The audio backend refused an operation.
    #[error("audio backend error: {0}")]
    Backend(#[from] cpal::Error),

    /// A core data structure rejected an operation.
    #[error(transparent)]
    Core(#[from] auris_core::CoreError),

    /// An offline render was asked for a range whose end precedes its start.
    #[error("invalid render range: start {start} is past end {end}")]
    InvalidRange {
        /// First frame of the requested range.
        start: u64,
        /// One past the last frame of the requested range.
        end: u64,
    },

    /// A sample rate that is not finite and positive was requested.
    #[error("invalid sample rate {0}")]
    InvalidSampleRate(f64),

    /// Software input monitoring cannot safely bridge the selected device clocks and block.
    #[error(
        "input monitoring cannot bridge {input_rate} Hz input to {output_rate} Hz output in \
         {block_frames}-frame blocks within the {limit}-frame realtime buffer; choose closer \
         sample rates or a smaller audio buffer"
    )]
    MonitorConfiguration {
        /// Rate delivered by the input device.
        input_rate: f64,
        /// Rate consumed by the output device.
        output_rate: f64,
        /// Largest block the output engine may request.
        block_frames: usize,
        /// Greatest ring capacity permitted for one monitor.
        limit: usize,
    },

    /// An offline render covered more frames than the engine is willing to attempt.
    #[error("render span of {frames} frames exceeds the {limit} frame limit")]
    RenderTooLong {
        /// Frames the requested render would have covered.
        frames: u64,
        /// Largest span the engine will attempt.
        limit: u64,
    },

    /// A complete in-memory render would exceed its bounded sample allocation.
    #[error(
        "a {frames}-frame, {channels}-channel render exceeds the {limit_bytes}-byte in-memory \
         render limit; stream the render to a file instead"
    )]
    RenderBufferTooLarge {
        /// Frames each output channel would contain.
        frames: usize,
        /// Number of output channels that would be retained.
        channels: usize,
        /// Greatest supported raw sample allocation, in bytes.
        limit_bytes: usize,
    },

    /// The allocator refused an otherwise bounded complete render buffer.
    #[error(
        "could not reserve a {frames}-frame, {channels}-channel in-memory render buffer; stream \
         the render to a file instead"
    )]
    RenderBufferAllocation {
        /// Frames each output channel would contain.
        frames: usize,
        /// Number of output channels that would be retained.
        channels: usize,
    },

    /// Flattening a clip would exceed the graph's bounded event allocation.
    #[error(
        "clip {clip} would grow this track schedule to {events} events, above the {limit}-event \
         limit; reduce its notes, curves, or repeats"
    )]
    ScheduleTooLarge {
        /// Clip whose expansion crossed the limit.
        clip: u64,
        /// Total track events the operation would require.
        events: u128,
        /// Greatest supported events on one track.
        limit: usize,
    },

    /// The allocator refused a bounded event reservation while preparing the graph.
    #[error(
        "could not reserve {events} events while preparing clip {clip}; reduce its notes, curves, \
         or repeats"
    )]
    ScheduleAllocation {
        /// Clip being prepared.
        clip: u64,
        /// Events the track would contain after the reservation.
        events: usize,
    },

    /// Retaining another valid track would exceed the complete graph's event budget.
    #[error(
        "the project would retain {events} scheduled events, above the {limit}-event limit; \
         reduce its notes, curves, or repeats"
    )]
    ProjectScheduleTooLarge {
        /// Events the complete graph would retain.
        events: u128,
        /// Greatest supported event count in one graph.
        limit: usize,
    },

    /// Retaining another valid audio clip would exceed the complete graph's window budget.
    #[error(
        "the project would retain {windows} audio loop windows, above the {limit}-window limit; \
         reduce its audio clips or repeats"
    )]
    ProjectAudioScheduleTooLarge {
        /// Audio windows the complete graph would retain.
        windows: u128,
        /// Greatest supported audio-window count in one graph.
        limit: usize,
    },

    /// The allocator refused a bounded audio-window reservation while preparing the graph.
    #[error(
        "could not reserve {windows} audio loop windows while preparing clip {clip}; reduce its \
         audio clips or repeats"
    )]
    AudioScheduleAllocation {
        /// Clip being prepared.
        clip: u64,
        /// Windows the current track would contain after the reservation.
        windows: usize,
    },

    /// The command queue was full; the caller should retry on the next UI frame.
    #[error("the engine command queue is full")]
    CommandQueueFull,

    /// The audio thread is gone, so commands can no longer be delivered.
    #[error("the audio engine is not running")]
    NotRunning,

    /// An offline render was stopped by whoever asked for it.
    ///
    /// Not a failure, and it has to stay distinguishable from one: a render that was cancelled
    /// on purpose reported in the colour of a broken export would have people looking for what
    /// went wrong. What is here is the fact; whether it reads as red is the frontend's to decide.
    #[error("the render was cancelled")]
    RenderCancelled,
}
