//! Bounded, optional observation of an instrument's internal mechanical state.
//!
//! One producer publishes coherent frames without waiting for readers. Observation never
//! changes DSP state. Values are relative model coordinates, not calibrated metres.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Maximum simultaneous bodies displayed in a motion frame.
pub const MOTION_VOICES: usize = 4;
/// Spatial samples along a string, bar or shell perimeter.
pub const MOTION_POINTS: usize = 64;
/// Low modes exposed alongside the spatial projection.
pub const MOTION_MODES: usize = 8;
const STRIDE: usize = 5 + MOTION_POINTS + MOTION_MODES;
const WORDS: usize = 5 + MOTION_VOICES * STRIDE;

/// Mechanical geometry of an observed instrument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionGeometry {
    /// Fixed-end string.
    #[default]
    String,
    /// Free bending bar.
    Bar,
    /// Closed shell perimeter.
    Shell,
}

/// One vibrating body, sampled from the live DSP state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionVoice {
    /// MIDI pitch including the current bend.
    pub pitch: f32,
    /// Envelope level, zero when inactive.
    pub level: f32,
    /// Excitation position along the body, from zero to one.
    pub contact: f32,
    /// Current excitation motion; zero after a released bow stops.
    pub excitation: f32,
    /// Whether the note remains held, including piano sustain pedal.
    pub held: bool,
    /// Relative spatial motion, before body filtering and output gain.
    pub points: [f32; MOTION_POINTS],
    /// Relative magnitudes of the first eight modes; zero for delay-line models.
    pub modes: [f32; MOTION_MODES],
}

impl Default for MotionVoice {
    fn default() -> Self {
        Self {
            pitch: 0.,
            level: 0.,
            contact: 0.25,
            excitation: 0.,
            held: false,
            points: [0.; MOTION_POINTS],
            modes: [0.; MOTION_MODES],
        }
    }
}

/// A coherent, fixed-size observation of one instrument instance.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionFrame {
    /// Geometry used for the spatial projection.
    pub geometry: MotionGeometry,
    /// Total voices sounding, including ones omitted from the bounded display.
    pub active: usize,
    /// Current output expression.
    pub expression: f32,
    /// Current bow pressure, or zero for struck and plucked instruments.
    pub pressure: f32,
    /// Whether the sustain pedal is down.
    pub pedal: bool,
    /// Up to four sounding voices, ordered by excitation recency.
    pub voices: [MotionVoice; MOTION_VOICES],
}

/// Reader handle for optional live mechanical observation.
#[derive(Debug)]
pub struct MotionMonitor {
    enabled: AtomicBool,
    version: AtomicU32,
    words: [AtomicU32; WORDS],
}

impl MotionMonitor {
    /// Enables sampling; disabling it eliminates spatial reconstruction work.
    pub fn watch(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
    /// Whether any frontend has enabled observation.
    pub fn is_watched(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    /// Reads a coherent frame, or skips this refresh if publication overlaps three attempts.
    /// This bounded reader never spins until the audio thread finishes.
    pub fn read(&self) -> Option<MotionFrame> {
        for _ in 0..3 {
            let version = self.version.load(Ordering::SeqCst);
            if version & 1 != 0 {
                continue;
            }
            let words =
                std::array::from_fn::<_, WORDS, _>(|i| self.words[i].load(Ordering::SeqCst));
            if self.version.load(Ordering::SeqCst) != version {
                continue;
            }
            let value = |i| f32::from_bits(words[i]);
            let mut frame = MotionFrame {
                geometry: match words[0] {
                    1 => MotionGeometry::Bar,
                    2 => MotionGeometry::Shell,
                    _ => MotionGeometry::String,
                },
                active: words[1] as usize,
                expression: value(2),
                pressure: value(3),
                pedal: words[4] != 0,
                ..Default::default()
            };
            for (index, voice) in frame.voices.iter_mut().enumerate() {
                let start = 5 + index * STRIDE;
                voice.pitch = value(start);
                voice.level = value(start + 1);
                voice.contact = value(start + 2);
                voice.excitation = value(start + 3);
                voice.held = words[start + 4] != 0;
                for (i, point) in voice.points.iter_mut().enumerate() {
                    *point = value(start + 5 + i);
                }
                for (i, mode) in voice.modes.iter_mut().enumerate() {
                    *mode = value(start + 5 + MOTION_POINTS + i);
                }
            }
            return Some(frame);
        }
        None
    }
}

/// Single writer, kept inside the audio-owned instrument. Clones have independent monitors.
#[derive(Debug)]
pub struct MotionCapture(Arc<MotionMonitor>);

impl Default for MotionCapture {
    fn default() -> Self {
        Self(Arc::new(MotionMonitor {
            enabled: AtomicBool::new(false),
            version: AtomicU32::new(0),
            words: std::array::from_fn(|_| AtomicU32::new(0)),
        }))
    }
}
impl Clone for MotionCapture {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl MotionCapture {
    /// Obtains a reader handle off the realtime thread.
    pub fn monitor(&self) -> Arc<MotionMonitor> {
        Arc::clone(&self.0)
    }
    /// Whether spatial reconstruction is currently needed.
    pub fn is_watched(&self) -> bool {
        self.0.is_watched()
    }
    /// Publishes a fixed-size frame without allocation, locking or reader acknowledgement.
    pub fn publish(&mut self, frame: &MotionFrame) {
        let mut words = [0_u32; WORDS];
        words[0] = match frame.geometry {
            MotionGeometry::String => 0,
            MotionGeometry::Bar => 1,
            MotionGeometry::Shell => 2,
        };
        words[1] = frame.active as u32;
        words[2] = frame.expression.to_bits();
        words[3] = frame.pressure.to_bits();
        words[4] = u32::from(frame.pedal);
        for (index, voice) in frame.voices.iter().enumerate() {
            let start = 5 + index * STRIDE;
            words[start] = voice.pitch.to_bits();
            words[start + 1] = voice.level.to_bits();
            words[start + 2] = voice.contact.to_bits();
            words[start + 3] = voice.excitation.to_bits();
            words[start + 4] = u32::from(voice.held);
            for (i, point) in voice.points.iter().enumerate() {
                words[start + 5 + i] = point.to_bits();
            }
            for (i, mode) in voice.modes.iter().enumerate() {
                words[start + 5 + MOTION_POINTS + i] = mode.to_bits();
            }
        }
        // SeqCst on payload and version provides one total order for the bounded seqlock.
        // The writer owns this handle exclusively; readers never mutate the payload.
        self.0.version.fetch_add(1, Ordering::SeqCst);
        for (dest, value) in self.0.words.iter().zip(words) {
            dest.store(value, Ordering::SeqCst);
        }
        self.0.version.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readers_never_mix_frames_and_clones_never_share_writers() {
        let mut writer = MotionCapture::default();
        let monitor = writer.monitor();
        let other = writer.clone();
        assert!(!Arc::ptr_eq(&monitor, &other.monitor()));
        let thread = std::thread::spawn(move || {
            for count in 1..2000 {
                let mut frame = MotionFrame {
                    active: count,
                    ..Default::default()
                };
                frame.voices[0].points.fill(count as f32);
                writer.publish(&frame);
            }
        });
        for _ in 0..4000 {
            if let Some(frame) = monitor.read() {
                assert!(
                    frame.voices[0]
                        .points
                        .iter()
                        .all(|p| *p == frame.active as f32)
                );
            }
        }
        thread.join().unwrap();
        assert_eq!(monitor.read().unwrap().active, 1999);
    }
}
