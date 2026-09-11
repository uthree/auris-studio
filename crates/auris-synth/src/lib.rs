//! Built-in software instruments for Auris Studio.
//!
//! The instruments in this crate are assembled from the same few primitives:
//!
//! * [`Oscillator`] — band-limited sine, pulse, saw and triangle plus an NES-style noise
//!   register.
//! * [`Adsr`](auris_dsp::Adsr) — a sample-accurate envelope with a de-click ramp for
//!   force-silenced voices. It lives in `auris-dsp` with the other primitives, because the
//!   sampler shapes a SoundFont with the same one.
//! * [`VoiceAllocator`] — a fixed voice pool with a steal-the-quietest policy.
//!
//! On top of those sit [`Chiptune`] (the general-purpose synth), [`Fm2`] (two-operator phase
//! modulation), [`NoiseDrum`] (a one-shot percussion voice) and [`Vocal`] (the formant-filtered
//! preview voice a singer track plays through, with [`Biquad`](auris_dsp::Biquad) sections from
//! `auris-dsp` for its formants). The split is the point: adding an instrument means writing a
//! `process`, not another voice manager. [`DrumKit`] combines distinct percussion voices in one
//! instrument with shared hat choking. [`SynthPack`] registers every instrument with a
//! [`PluginRegistry`](auris_core::PluginRegistry).
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
pub mod drumkit;
pub mod fm2;
pub mod lfo;
pub mod noisedrum;
pub mod oscillator;
pub mod pack;
pub mod params;
pub mod render;
pub mod vocal;
pub mod voice;

#[cfg(test)]
mod test_support;

pub use chiptune::Chiptune;
pub use drumkit::DrumKit;
pub use fm2::Fm2;
pub use noisedrum::NoiseDrum;
pub use oscillator::{Oscillator, Waveform};
pub use pack::SynthPack;
pub use params::ParamBank;
pub use render::{SegmentRenderer, render_segments, spread_to_all_channels};
pub use vocal::Vocal;
pub use voice::{MAX_VOICES, VoiceAllocator, VoiceAssignment, VoiceMask, VoiceSlot, VoiceState};

/// Narrows a host rate without allowing a finite `f64` to become infinite in DSP state.
fn sample_rate_f32(sample_rate: f64) -> f32 {
    if sample_rate.is_finite() && sample_rate > 0.0 && sample_rate <= f64::from(f32::MAX) {
        sample_rate as f32
    } else {
        48_000.0
    }
}

#[cfg(test)]
mod sample_rate_tests {
    use super::*;

    #[test]
    fn a_rate_too_large_for_dsp_falls_back_before_narrowing() {
        assert_eq!(sample_rate_f32(1.0e40), 48_000.0);
        assert_eq!(sample_rate_f32(f64::INFINITY), 48_000.0);
        assert_eq!(sample_rate_f32(96_000.0), 96_000.0);
    }
}

#[cfg(test)]
mod channel_volume_tests {
    use super::*;
    use crate::test_support::{Rig, rms};
    use auris_core::{Instrument, NoteEvent};

    #[test]
    fn melodic_instruments_apply_channel_volume_and_reset_it() {
        let instruments: [Box<dyn Instrument>; 3] = [
            Box::new(Chiptune::new()),
            Box::new(Fm2::new()),
            Box::new(Vocal::new()),
        ];
        for instrument in instruments {
            let mut rig = Rig::new(instrument, 48_000.0, 256, 2);
            if rig.instrument.descriptor().id == Vocal::ID {
                // Breath noise advances across resets; isolate the tone for a gain comparison.
                rig.set_param("breath", 0.0);
            }
            let note = NoteEvent::NoteOn {
                frame: 0,
                pitch: 69,
                velocity: 0.8,
            };
            let full = rig.render(4096, &[note]);
            rig.instrument.reset();
            let quiet = rig.render(
                4096,
                &[
                    NoteEvent::Controller {
                        frame: 0,
                        number: 7,
                        value: 0.5,
                    },
                    note,
                ],
            );
            assert!(rms(&full) > 0.0001);
            assert!(
                (rms(&quiet) / rms(&full) - 0.25).abs() < 0.001,
                "{}: full={} quiet={}",
                rig.instrument.descriptor().name,
                rms(&full),
                rms(&quiet)
            );
            rig.instrument.reset();
            let reset = rig.render(4096, &[note]);
            assert!((rms(&reset) / rms(&full) - 1.0).abs() < 0.001);
        }
    }
}
