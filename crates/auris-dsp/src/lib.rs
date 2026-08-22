//! Audio effects and DSP primitives for Auris Studio.
//!
//! The crate is split in two halves:
//!
//! * **Primitives** — [`adsr`], [`biquad`], [`delay_line`], [`envelope`] and [`smooth`] are plain
//!   DSP building blocks with no plugin machinery attached. Instruments in `auris-synth` and
//!   `auris-sampler` use them too.
//! * **Effects** — one type per [`auris_core::plugin::Effect`] implementation, all bundled by
//!   [`DspPack`] so an application installs the whole set with
//!   `registry.install::<DspPack>()`.
//!
//! # Realtime contract
//!
//! Every `process` method here is allocation-free, lock-free and panic-free: all buffers are
//! sized in `prepare`, and every slice index is derived from `min`-clamped lengths.

#![warn(missing_docs)]

mod bank;

/// Divisor for parameters stored in milliseconds.
///
/// Dividing by 1000 rather than multiplying by `1.0e-3` matters: `1.0e-3` is not exactly
/// representable in `f32`, so `10 ms * 48000 * 1.0e-3` lands on 480.000023 samples instead of
/// 480, and a delay set to a whole number of samples would then interpolate between two taps.
pub(crate) const MILLISECONDS_PER_SECOND: f32 = 1_000.0;

pub mod adsr;
pub mod biquad;
pub mod compressor;
pub mod delay;
pub mod delay_line;
pub mod distortion;
pub mod envelope;
pub mod eq;
pub mod gain;
pub mod limiter;
pub mod loudness;
pub mod pack;
pub mod reverb;
pub mod smooth;
pub mod spectrum;
pub mod stretch;

pub use adsr::{Adsr, EnvelopeStage};
pub use biquad::{Biquad, BiquadCoefficients};
pub use compressor::Compressor;
pub use delay::Delay;
pub use delay_line::DelayLine;
pub use distortion::{Distortion, DistortionMode};
pub use envelope::{EnvelopeFollower, EnvelopeMode};
pub use eq::{
    BAND_COUNT as EQ_BAND_COUNT, EqBandKind, EqBandLayout, EqBandSetting, Equalizer,
    ID as EQUALIZER_ID, LAYOUT as EQ_LAYOUT, response_db as eq_response_db,
};
pub use gain::GainPan;
pub use limiter::Limiter;
pub use loudness::{integrated_lufs, k_weighting, loudness_quantile};
pub use pack::DspPack;
pub use reverb::Reverb;
pub use smooth::{SmoothedValue, one_pole_coefficient};
pub use spectrum::{SILENCE_DB, SpectrumAnalyzer, bands_from_bins, fft};
