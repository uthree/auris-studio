//! Measured attack, decay, sustain and release for library sounds, kept on disk between runs.
//!
//! Each sound is played at C2, C4 and C6 at velocity 100, one note at a time, held until the
//! level settles or dies (four seconds at most) and recorded until the tail has fallen 60 dB
//! (four more at most). [`auris_dsp::EnvelopeProbe`] makes both calls while the note renders,
//! so a pluck costs a fraction of a second and only pads and pianos use the whole budget.
//!
//! Those renders are the expensive part of indexing, so the result for every sound that lives
//! in a file — a SoundFont preset or a hosted plugin — is cached under the configuration
//! directory, keyed by the file's path, size and modification time. Built-in instruments are
//! code, which improves between builds, and are cheap; they are measured every time.

use super::TimbreMapControl;
use crate::MeasuredEnvelope;
use auris_core::AudioBuffer;
use auris_core::plugin::{Instrument, NoteEvent, ProcessContext};
use auris_dsp::EnvelopeProbe;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// The pitches a sound is measured at: C2, C4 and C6.
pub const ENVELOPE_PITCHES: [u8; 3] = [36, 60, 84];
/// MIDI velocity 100.
const VELOCITY: f32 = 100. / 127.;
const BLOCK: usize = 512;
/// Bumped whenever the measurement changes, so a cache written by another rule is ignored.
const MEASUREMENT_VERSION: u32 = 1;
/// Cached sounds; past this the oldest entries are dropped on save.
const CACHE_LIMIT: usize = 16_384;

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
            events.push(NoteEvent::NoteOn {
                frame: 0,
                pitch,
                velocity: VELOCITY,
            });
        }
        if held && probe.should_release() {
            events.push(NoteEvent::NoteOff { frame: 0, pitch });
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

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<(String, SoundEnvelope)>,
}

#[derive(Default)]
struct Cache {
    loaded: bool,
    dirty: bool,
    order: Vec<String>,
    entries: HashMap<String, SoundEnvelope>,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache_path() -> Option<PathBuf> {
    // A unit test must never write into the developer's configuration.
    (!cfg!(test)).then(|| {
        crate::config_dir()
            .join("cache")
            .join("sound-envelopes.json")
    })
}

fn with_cache<T>(f: impl FnOnce(&mut Cache) -> T) -> Option<T> {
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if !cache.loaded {
        cache.loaded = true;
        if let Some(file) = cache_path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|file| file.version == MEASUREMENT_VERSION)
        {
            for (key, value) in file.entries {
                if cache.entries.insert(key.clone(), value).is_none() {
                    cache.order.push(key);
                }
            }
        }
    }
    Some(f(&mut cache))
}

/// A cached measurement for `key`, which names the sound and the stamp of the file it lives in.
pub(super) fn cached(key: &str) -> Option<SoundEnvelope> {
    with_cache(|cache| cache.entries.get(key).cloned()).flatten()
}

/// Remembers a measurement; [`save`] writes it out.
pub(super) fn remember(key: String, envelope: SoundEnvelope) {
    with_cache(|cache| {
        if cache.entries.insert(key.clone(), envelope).is_none() {
            cache.order.push(key);
        }
        cache.dirty = true;
    });
}

/// Writes the cache if it changed. Failures are ignored: the cache only saves time.
pub(super) fn save() {
    let Some(path) = cache_path() else { return };
    let file = with_cache(|cache| {
        if !cache.dirty {
            return None;
        }
        cache.dirty = false;
        let excess = cache.order.len().saturating_sub(CACHE_LIMIT);
        for key in cache.order.drain(..excess) {
            cache.entries.remove(&key);
        }
        Some(CacheFile {
            version: MEASUREMENT_VERSION,
            entries: cache
                .order
                .iter()
                .filter_map(|key| Some((key.clone(), cache.entries.get(key)?.clone())))
                .collect(),
        })
    })
    .flatten();
    let Some(file) = file else { return };
    let Ok(bytes) = serde_json::to_vec(&file) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, bytes).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

/// A file-backed sound's cache key: what it is, plus the stamp of every file it depends on.
pub(super) fn cache_key(identity: &str) -> String {
    format!("v{MEASUREMENT_VERSION}|{identity}")
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

    #[test]
    fn the_cache_keeps_what_it_is_given_under_its_version() {
        let envelope = SoundEnvelope {
            summary: MeasuredEnvelope {
                attack_ms: 5.,
                decay_ms: 200.,
                sustain_db: -6.,
                release_ms: 300.,
                decaying: false,
                decay_estimated: false,
                release_estimated: true,
            },
            by_pitch: vec![],
        };
        let key = cache_key("test-sound");
        assert!(key.starts_with("v1|"));
        remember(key.clone(), envelope.clone());
        assert_eq!(cached(&key), Some(envelope.clone()));
        assert_eq!(envelope.value()["release_estimated"], true);
        assert!(envelope.value().get("decaying").is_none());
        save();
    }
}
