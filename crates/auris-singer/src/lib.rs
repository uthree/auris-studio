//! Singing-voice synthesis: an auris-singer voice model, run offline over a track's frames.
//!
//! This crate is the far end of the pipeline `auris-vocal` begins. That crate turns lyrics
//! into phonemes and notes into [`SingerFrames`](auris_vocal::SingerFrames) — one phoneme, one
//! pitch, one energy per hop; this one hands those frames to a trained voice and gets a
//! waveform back. The model file is an ONNX export from the trainer in this repository's
//! `training/` directory, self-contained by that project's design: the phoneme table, the audio
//! parameters and the presentational voice card all ride inside the one file ([`VoiceInfo`]
//! reads them), so pointing at a `.onnx` is the whole installation, exactly the policy a
//! SoundFont gets.
//!
//! The two halves being one repository is what lets them be *checked* against each other:
//! `training/tests/test_host_contract.py` reads the constants below out of this crate's source
//! and fails when the exporter and this reader drift apart on the metadata key, the format
//! version or the phoneme table.
//!
//! Three facts shape the API:
//!
//! * **Inference is never realtime.** A render takes seconds and allocates freely, so it can
//!   only ever run on a normal thread; what the audio thread plays is the *result*, cached
//!   and handed over like any audio clip. Nothing here touches the realtime contract.
//! * **A whole song is never one inference.** The model's attention buffers grow with the
//!   square of the frame count — a three-minute piece asked for at once has taken a machine
//!   down. [`VoiceModel::sing`] cuts the timeline in silence into chunks of at most
//!   [`MAX_CHUNK_FRAMES`] frames and stitches the answers into one waveform, so memory is
//!   bounded by the chunk, not the song.
//! * **The randomness is an input.** The model's stochastic draws — the prior sample, the
//!   excitation noise — are graph inputs by its own design, and this crate fills them from
//!   [`auris_core::rng`] streams named by a seed: the same document, seed and voice are fed
//!   the same numbers on any machine, and on the CPU render the same take to the sample. A
//!   GPU ([`Acceleration`]) rounds in its own way — which is one more reason a take is a
//!   *thing a file keeps*, frozen, rather than a thing another machine re-derives.

#![warn(missing_docs)]

mod metadata;
mod model;
mod score;

pub use metadata::{FORMAT_VERSION, METADATA_KEY, VoiceCard, VoiceInfo};
pub use model::{Acceleration, NOISE_SCALE, VoiceModel};
pub use score::{ENERGY_FULL_SCALE, MAX_CHUNK_FRAMES};

/// Why a voice could not be loaded, or frames could not be sung.
#[derive(Debug, thiserror::Error)]
pub enum SingError {
    /// The file could not be opened as an ONNX model at all.
    #[error("could not open the voice model: {reason}")]
    Load {
        /// What the runtime said.
        reason: String,
    },
    /// The file is ONNX but carries no auris-singer metadata.
    #[error("no auris-singer metadata inside the file — it is not an exported voice")]
    NotAVoice,
    /// The metadata was there but unreadable or unacceptable.
    #[error("the voice model's metadata was refused: {0}")]
    Metadata(String),
    /// The frames were sampled on a different clock than the model sings on.
    #[error("the frames step {frames} s but this voice sings in steps of {model} s")]
    HopMismatch {
        /// Seconds per frame the frames were sampled at.
        frames: f64,
        /// Seconds per frame the model wants.
        model: f64,
    },
    /// The GPU was insisted on, on a platform with no GPU provider to insist on.
    #[error("this platform has no GPU provider for singing — choose Auto or CPU")]
    NoGpu,
    /// The runtime refused an inference mid-render.
    #[error("the voice model refused the score: {0}")]
    Inference(String),
    /// The progress callback asked the render to stop.
    #[error("the render was cancelled")]
    Cancelled,
}
