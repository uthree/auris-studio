//! Singing-voice synthesis: an auris-singer voice model, run offline over a track's frames.
//!
//! This crate is the far end of the pipeline `auris-vocal` begins. That crate turns lyrics
//! into phonemes and notes into [`SingerFrames`](auris_vocal::SingerFrames) — one phoneme, one
//! pitch, one energy per hop; this one hands those frames to a trained voice and gets a
//! waveform back. [`SingingBackend`] is that boundary. [`VoiceModel`] selects the native Auris
//! backend for a self-contained `.onnx` exported by this repository's trainer, or the DiffSinger
//! backend for a voicebank's `dsconfig.yaml`, or the VOICEVOX backend for a `.voicevox.json`
//! connection; the session above it does not know which inference pipeline is running.
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

mod backend;
mod diffsinger;
mod metadata;
mod model;
mod score;
mod voicevox;

pub use backend::{BackendKind, SingingBackend, VoiceModel};
pub use metadata::{FORMAT_VERSION, METADATA_KEY, VoiceCard, VoiceInfo};
pub use model::{Acceleration, NOISE_SCALE};
pub use score::{ENERGY_FULL_SCALE, MAX_CHUNK_FRAMES, MAX_REST_FRAMES};

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
    /// A speaker id the model was not trained with.
    #[error("this voice has {count} speaker(s), none numbered {speaker}")]
    NoSuchSpeaker {
        /// The id asked for.
        speaker: u32,
        /// How many the model has, numbered from zero.
        count: u32,
    },
    /// The per-frame sequences disagree about how many frames they contain.
    #[error(
        "the singer frames have mismatched lengths: {phonemes} phonemes, {f0_hz} pitch values, and {energy} energy values"
    )]
    InvalidFrames {
        /// Number of phoneme ids.
        phonemes: usize,
        /// Number of pitch values.
        f0_hz: usize,
        /// Number of energy values.
        energy: usize,
    },
    /// The runtime refused an inference mid-render.
    #[error("the voice model refused the score: {0}")]
    Inference(String),
    /// A voice uses a valid backend format feature this build does not implement yet.
    #[error("the {backend} backend does not support this voice: {reason}")]
    Unsupported {
        /// Backend that understood the voice entry file.
        backend: &'static str,
        /// The unsupported part of the voicebank contract.
        reason: String,
    },
    /// The progress callback asked the render to stop.
    #[error("the render was cancelled")]
    Cancelled,
}

/// Checks the invariants an externally-written frame file must satisfy before inference.
///
/// [`auris_vocal::render_frames`] constructs equal-length sequences, but `SingerFrames` is also a
/// serialisable interchange format and a hand-edited file does not inherit that construction.
pub fn validate_frames(frames: &auris_vocal::SingerFrames) -> Result<(), SingError> {
    let phonemes = frames.phonemes.len();
    let f0_hz = frames.f0_hz.len();
    let energy = frames.energy.len();
    match phonemes == f0_hz && phonemes == energy {
        true => Ok(()),
        false => Err(SingError::InvalidFrames {
            phonemes,
            f0_hz,
            energy,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_frame_sequences_are_refused_before_scoring() {
        let frames = auris_vocal::SingerFrames {
            hop_seconds: 0.01,
            inventory: vec!["<sil>".into()],
            phonemes: vec![0, 0],
            f0_hz: vec![0.0],
            energy: vec![0.0, 0.0],
        };

        assert!(matches!(
            validate_frames(&frames),
            Err(SingError::InvalidFrames {
                phonemes: 2,
                f0_hz: 1,
                energy: 2
            })
        ));
    }
}
