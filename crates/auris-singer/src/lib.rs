//! Singing-voice synthesis: an auris-singer voice model, run offline over a track's frames.
//!
//! This crate is the far end of the pipeline `auris-vocal` begins. That crate turns lyrics
//! into phonemes and notes into [`SingerFrames`](auris_vocal::SingerFrames) — one phoneme, one
//! pitch, one energy per hop; this one hands those frames to a trained voice and gets a
//! waveform back. [`SingingBackend`] is that boundary. [`VoiceModel`] selects the native Auris
//! backend for a self-contained `.onnx` exported by this repository's trainer, or the DiffSinger
//! backend for a voicebank's `dsconfig.yaml`, the LeapSinger backend for a `.leapsinger.json`
//! model entry, or the VOICEVOX backend for a `.voicevox.json` connection; the session above it
//! does not know which inference pipeline is running.
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
//! * **Native voices take randomness as an input.** The native model's stochastic draws — the
//!   prior sample, the excitation noise — are graph inputs by its own design, and this crate
//!   fills them from [`auris_core::rng`] streams named by a seed: the same document, seed and voice are fed
//!   the same numbers on any machine, and on the CPU render the same take to the sample. A
//!   GPU ([`Acceleration`]) rounds in its own way — which is one more reason a take is a
//!   *thing a file keeps*, frozen, rather than a thing another machine re-derives. LeapSinger's
//!   upstream acoustic and vocoder graphs draw their noise internally and do not accept a seed;
//!   their repeated renders can differ, while the saved audio take preserves the performance.

#![warn(missing_docs)]

mod backend;
mod curves;
mod diffsinger;
mod leapsinger;
mod limits;
mod metadata;
mod model;
mod portrait;
mod score;
mod voicevox;

pub use backend::{BackendKind, SingingBackend, SingingRender, VoiceCapabilities, VoiceModel};
pub use curves::{CurveGenerator, CurvePrediction, CurveSource, CurveSources, PreparedCurves};
pub use limits::validate_automatic_voice_entry;
pub use metadata::{FORMAT_VERSION, METADATA_KEY, VoiceCard, VoiceInfo};
pub use model::{Acceleration, NOISE_SCALE};
pub use portrait::{PORTRAIT_MAX_BYTES, VoicePortrait, read_voice_portrait};
pub use score::{ENERGY_FULL_SCALE, MAX_CHUNK_FRAMES, MAX_REST_FRAMES};

/// Whether a VOICEVOX base URL names a numeric loopback address suitable for background work.
///
/// Hostnames are deliberately not resolved here: even `localhost` can be redirected by local
/// resolver configuration, whereas `127.0.0.0/8` and `::1` carry the boundary in the manifest.
pub fn automatic_voicevox_url_safe(url: &str) -> bool {
    voicevox::loopback_url(url.trim_end_matches('/'))
}

/// Checks VOICEVOX lyrics before inference, retaining the original event index on failure.
pub fn validate_voicevox_score(score: &auris_vocal::SingerScore) -> Result<(), SingError> {
    voicevox::validate_lyrics(score)
}

/// A lyric problem the user can correct in the score editor.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LyricIssue {
    /// The stored lyric and pronunciation cannot be expressed as kana.
    #[error("enter a readable kana lyric")]
    Unreadable,
    /// A prolonged-sound mark has no preceding vowel to extend.
    #[error("the prolonged-sound mark needs a preceding vowel; enter ア, イ, ウ, エ or オ")]
    MissingVowel,
    /// The note cannot hold even one frame per mora.
    #[error("lengthen the note or distribute its lyric across more notes")]
    TooShort,
}

/// Why a voice could not be loaded, or frames could not be sung.
#[derive(Debug, thiserror::Error)]
pub enum SingError {
    /// A lyric needs a user's correction; the index refers to the original score.
    #[error("invalid lyric '{lyric}' at score event {event}: {issue}")]
    InvalidLyric {
        /// Zero-based event index before backend normalization or padding.
        event: usize,
        /// The lyric that needs correction.
        lyric: String,
        /// The actionable reason.
        issue: LyricIssue,
    },
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
    /// A frame file has matching arrays but contains a value outside the synthesis contract.
    #[error("invalid singer frames: {reason}")]
    InvalidFrameData {
        /// The rejected invariant, suitable for showing beside the imported frame file.
        reason: String,
    },
    /// Input or generated output exceeds a documented memory or complexity ceiling.
    #[error("{resource} is too large (observed {observed:?}; limit {limit})")]
    TooLarge {
        /// Which bounded input or output exceeded its ceiling.
        resource: &'static str,
        /// Observed units where they fit this process's address space.
        observed: Option<usize>,
        /// Maximum accepted units, in the resource named above.
        limit: usize,
    },
    /// A fallible preallocation failed before inference began.
    #[error("not enough memory to allocate {resource}")]
    Allocation {
        /// The buffer that could not be reserved.
        resource: &'static str,
    },
    /// Background loading refused a manifest reference outside its permitted local boundary.
    #[error("unsafe automatic voice access: {reason}")]
    UnsafeAutomaticAccess {
        /// The path or URL policy violation.
        reason: String,
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
    if phonemes != f0_hz || phonemes != energy {
        return Err(SingError::InvalidFrames {
            phonemes,
            f0_hz,
            energy,
        });
    }
    let invalid = |reason: &str| SingError::InvalidFrameData {
        reason: reason.into(),
    };
    if !(limits::MIN_HOP_SECONDS..=limits::MAX_HOP_SECONDS).contains(&frames.hop_seconds)
        || !frames.hop_seconds.is_finite()
    {
        return Err(invalid(
            "the frame hop must be finite and between 0.001 and 0.100 seconds",
        ));
    }
    validate_frame_count(phonemes)?;
    if frames.inventory.is_empty()
        || frames.inventory.len() > limits::MAX_COLLECTION_ITEMS
        || frames
            .inventory
            .iter()
            .any(|token| token.is_empty() || token.len() > limits::MAX_TOKEN_BYTES)
    {
        return Err(invalid(
            "the phoneme inventory must contain 1..=4096 nonempty tokens of at most 256 UTF-8 bytes",
        ));
    }
    if frames
        .phonemes
        .iter()
        .any(|id| *id as usize >= frames.inventory.len())
    {
        return Err(invalid("a phoneme id is outside the supplied inventory"));
    }
    if frames
        .f0_hz
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=24_000.0).contains(value))
    {
        return Err(invalid("pitch must be finite and between 0 and 24000 Hz"));
    }
    if frames
        .energy
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(invalid("energy must be finite and between 0 and 1"));
    }
    Ok(())
}

fn validate_frame_count(phonemes: usize) -> Result<(), SingError> {
    if phonemes > limits::MAX_FRAME_COUNT {
        return Err(SingError::TooLarge {
            resource: "singer frame count",
            observed: Some(phonemes),
            limit: limits::MAX_FRAME_COUNT,
        });
    }
    Ok(())
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

    fn valid_frames() -> auris_vocal::SingerFrames {
        auris_vocal::SingerFrames {
            hop_seconds: 0.01,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![0, 1],
            f0_hz: vec![0.0, 440.0],
            energy: vec![0.0, 1.0],
        }
    }

    #[test]
    fn frame_clock_inventory_ids_and_curves_are_bounded() {
        let mut frames = valid_frames();
        for hop in [f64::NAN, 0.000_999, 0.100_001] {
            frames.hop_seconds = hop;
            assert!(matches!(
                validate_frames(&frames),
                Err(SingError::InvalidFrameData { .. })
            ));
        }
        frames = valid_frames();
        frames.phonemes[1] = 2;
        assert!(validate_frames(&frames).is_err());
        frames = valid_frames();
        frames.inventory[1] = "x".repeat(limits::MAX_TOKEN_BYTES + 1);
        assert!(validate_frames(&frames).is_err());
        frames = valid_frames();
        frames.f0_hz[1] = f32::INFINITY;
        assert!(validate_frames(&frames).is_err());
        frames = valid_frames();
        frames.energy[1] = 1.000_1;
        assert!(validate_frames(&frames).is_err());

        for hop in [limits::MIN_HOP_SECONDS, limits::MAX_HOP_SECONDS] {
            frames = valid_frames();
            frames.hop_seconds = hop;
            validate_frames(&frames).expect("inclusive hop boundary");
        }
    }

    #[test]
    fn frame_count_boundary_is_checked_without_allocating_an_attack_vector() {
        validate_frame_count(limits::MAX_FRAME_COUNT).expect("inclusive frame-count boundary");
        assert!(matches!(
            validate_frame_count(limits::MAX_FRAME_COUNT + 1),
            Err(SingError::TooLarge {
                resource: "singer frame count",
                ..
            })
        ));
    }
}
