//! Measured attack, decay, sustain and release for library sounds, kept on disk between runs.
//!
//! Each sound is played at C2, C4 and C6 at velocity 100, one note at a time, held until the
//! level settles or dies (four seconds at most) and recorded until the tail has fallen 60 dB
//! (four more at most). [`auris_dsp::EnvelopeProbe`] makes both calls while the note renders,
//! so a pluck costs a fraction of a second and only pads and pianos use the whole budget.
//!
//! Those renders are the expensive part of indexing, so the whole measurement of every sound
//! that lives in a file — a SoundFont preset, a hosted plugin, a saved sound that needs one — is
//! kept on disk by the sound catalog (see `disk_cache`), keyed by the files' path, size and
//! modification time. Built-in instruments are code, which improves between builds, and are
//! cheap; they are measured every time.

use super::TimbreMapControl;
use crate::MeasuredEnvelope;
use auris_core::AudioBuffer;
use auris_core::plugin::{Instrument, NoteEvent, ProcessContext};
use auris_dsp::EnvelopeProbe;

/// The pitches a sound is measured at: C2, C4 and C6.
pub const ENVELOPE_PITCHES: [u8; 3] = [36, 60, 84];
/// MIDI velocity 100.
const VELOCITY: f32 = 100. / 127.;
const BLOCK: usize = 512;

/// A sound's measured envelope: one per pitch and their per-field median.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SoundEnvelope {
    /// Per-field median across the pitches that sounded.
    pub summary: MeasuredEnvelope,
    /// Each pitch that sounded, in ascending order.
    pub by_pitch: Vec<(u8, MeasuredEnvelope)>,
}

impl SoundEnvelope {
    /// The summary and the per-pitch values, as a model reads them.
    pub fn value(&self) -> serde_json::Value {
        let round = |e: &MeasuredEnvelope| {
            let mut value = serde_json::json!({
                "attack_ms": e.attack_ms.round(),
                "decay_ms": e.decay_ms.round(),
                "sustain_db": (e.sustain_db * 10.).round() / 10.,
                "release_ms": e.release_ms.round(),
            });
            for (flag, set) in [
                ("decaying", e.decaying),
                ("decay_estimated", e.decay_estimated),
                ("release_estimated", e.release_estimated),
            ] {
                if set {
                    value[flag] = true.into();
                }
            }
            value
        };
        let mut value = round(&self.summary);
        value["by_pitch"] = self
            .by_pitch
            .iter()
            .map(|(pitch, e)| {
                let mut v = round(e);
                v["pitch"] = (*pitch).into();
                v
            })
            .collect::<Vec<_>>()
            .into();
        value
    }
}

/// Plays one note through a prepared instrument and measures its envelope.
/// `None` when it stayed silent.
pub(super) fn measure_note(
    instrument: &mut dyn Instrument,
    sample_rate: f64,
    pitch: u8,
    control: &TimbreMapControl,
) -> Result<Option<MeasuredEnvelope>, crate::SessionError> {
    let mut probe = EnvelopeProbe::new(sample_rate);
    let mut block = AudioBuffer::stereo(BLOCK, sample_rate);
    let mut held = true;
    let mut start = 0u64;
    // Both phases are bounded by the probe; this is the backstop should it ever disagree.
    let limit = ((auris_dsp::envelope_measure::MAX_HOLD_SECONDS
        + auris_dsp::envelope_measure::MAX_TAIL_SECONDS
        + 1.)
        * sample_rate) as u64;
    while start < limit {
        control.check()?;
        block.set_frame_count(BLOCK);
        block.clear();
        let mut events = Vec::with_capacity(1);
        if start == 0 {
            events.push(NoteEvent::note_on(0, pitch, VELOCITY));
        }
        if held && probe.should_release() {
            events.push(NoteEvent::NoteOff {
                frame: 0,
                key: pitch,
            });
            probe.release();
            held = false;
        }
        instrument.process(
            &events,
            &mut block,
            &ProcessContext {
                sample_rate,
                block_frames: BLOCK,
                playhead_samples: start,
                bpm: 120.0,
                is_playing: true,
                is_offline: true,
            },
        );
        block.sanitize();
        probe.push(&[block.channel(0), block.channel(1)]);
        start += BLOCK as u64;
        if !held && probe.should_stop() {
            break;
        }
    }
    Ok(probe.finish())
}

/// Measures every pitch, making a fresh instrument for each through `play`.
/// `None` when no pitch sounded.
pub(super) fn measure_sound(
    mut play: impl FnMut(u8) -> Result<Option<MeasuredEnvelope>, String>,
) -> Result<Option<SoundEnvelope>, String> {
    let mut by_pitch = Vec::new();
    for pitch in ENVELOPE_PITCHES {
        if let Some(envelope) = play(pitch)? {
            by_pitch.push((pitch, envelope));
        }
    }
    let values: Vec<_> = by_pitch.iter().map(|(_, e)| *e).collect();
    Ok(MeasuredEnvelope::median(&values).map(|summary| SoundEnvelope { summary, by_pitch }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::plugin::PrepareContext;

    #[test]
    fn built_in_voices_measure_in_milliseconds_at_three_pitches() {
        let registry = crate::default_registry(auris_sampler::SoundFontBank::shared());
        let control = TimbreMapControl::default();
        let rate = 44_100.;
        let measure = |id: &str| {
            measure_sound(|pitch| {
                let mut instrument = registry.create_instrument(id).map_err(|e| e.to_string())?;
                instrument.prepare(&PrepareContext::new(rate, BLOCK, 2));
                measure_note(instrument.as_mut(), rate, pitch, &control).map_err(|e| e.to_string())
            })
            .unwrap()
            .unwrap()
        };
        let ids: Vec<String> = registry
            .instruments()
            .map(|d| d.id.to_string())
            .filter(|id| {
                ["bell", "piano", "violin", "fm2"]
                    .iter()
                    .any(|n| id.ends_with(n))
            })
            .collect();
        assert!(!ids.is_empty());
        for id in ids {
            let envelope = measure(&id);
            assert_eq!(envelope.by_pitch.len(), 3, "{id}");
            let e = envelope.summary;
            assert!(e.attack_ms.is_finite() && e.attack_ms >= 0., "{id}: {e:?}");
            assert!((-120. ..=0.).contains(&e.sustain_db), "{id}: {e:?}");
            assert!(e.release_ms >= 0., "{id}: {e:?}");
            if id.contains("bell") {
                assert!(e.decaying, "a struck bell does not sustain: {e:?}");
            }
        }
    }
}
