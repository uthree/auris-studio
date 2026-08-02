//! Built-in software instruments for Auris Studio.
//!
//! Three instruments ship in this crate, all of them assembled from the same three primitives:
//!
//! * [`Oscillator`] — band-limited sine, pulse, saw and triangle plus an NES-style noise
//!   register.
//! * [`Adsr`] — a sample-accurate envelope with a de-click ramp for force-silenced voices.
//! * [`VoiceAllocator`] — a fixed voice pool with a steal-the-quietest policy.
//!
//! On top of those sit [`Chiptune`] (the general-purpose synth), [`Fm2`] (two-operator phase
//! modulation) and [`NoiseDrum`] (a one-shot percussion voice). The split is the point: adding
//! a fourth instrument means writing a `process`, not another voice manager. [`SynthPack`]
//! registers all three with a [`PluginRegistry`](auris_core::PluginRegistry).
//!
//! # Realtime behaviour
//!
//! Every allocation happens in `prepare`. `process` runs on the audio callback thread and does
//! no allocation, locking, I/O or panicking indexing. Note events are honoured on the exact
//! frame they carry — [`render_segments`] splits each block at the event boundaries — so timing
//! does not depend on the audio driver's buffer size.
//!
//! ```
//! use auris_core::{AudioBuffer, Instrument, NoteEvent, PrepareContext, ProcessContext};
//! use auris_synth::Chiptune;
//!
//! let mut synth = Chiptune::new();
//! synth.prepare(&PrepareContext::new(48_000.0, 512, 2));
//!
//! let mut out = AudioBuffer::stereo(512, 48_000.0);
//! let events = [NoteEvent::NoteOn { frame: 100, pitch: 69, velocity: 1.0 }];
//! let ctx = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
//! synth.process(&events, &mut out, &ctx);
//!
//! assert!(out.channel(0)[..100].iter().all(|s| *s == 0.0));
//! assert_eq!(synth.active_voices(), 1);
//! ```

#![warn(missing_docs)]

pub mod chiptune;
pub mod envelope;
pub mod fm2;
pub mod noisedrum;
pub mod oscillator;
pub mod pack;
pub mod params;
pub mod render;
pub mod voice;

#[cfg(test)]
mod test_support;

pub use chiptune::Chiptune;
pub use envelope::{Adsr, EnvelopeStage};
pub use fm2::Fm2;
pub use noisedrum::NoiseDrum;
pub use oscillator::{Oscillator, Waveform};
pub use pack::SynthPack;
pub use params::ParamBank;
pub use render::{SegmentRenderer, render_segments, spread_to_all_channels};
pub use voice::{MAX_VOICES, VoiceAllocator, VoiceAssignment, VoiceMask, VoiceSlot, VoiceState};
