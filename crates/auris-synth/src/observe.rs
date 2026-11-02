//! Mechanical observation shared by the instruments that publish their bodies' motion.
//!
//! [`Physical`](crate::Physical) and [`DrumKit`](crate::DrumKit) grew their own before this
//! module existed; the reeds, whistles, dulcimer and choir share this one. Everything here runs
//! on the audio thread and works in fixed-size arrays: no allocation, no locking.

use std::sync::Arc;

use auris_core::motion::{
    MOTION_VOICES, MotionCapture, MotionFrame, MotionGeometry, MotionMonitor,
};

use crate::voice::VoiceAllocator;

/// How many frames a second an observed instrument publishes, which is what a display redraws.
const FRAMES_PER_SECOND: f32 = 30.0;

/// The writing half of a monitor, publishing at display rate rather than once per block.
#[derive(Clone, Debug, Default)]
pub(crate) struct Observer {
    capture: MotionCapture,
    frames: usize,
}

impl Observer {
    /// The reading half, for the session.
    pub(crate) fn monitor(&self) -> Arc<MotionMonitor> {
        self.capture.monitor()
    }

    /// Counts a rendered block and says whether a frame is due. Never due while nobody watches,
    /// so an unobserved instrument spends nothing reconstructing its body.
    pub(crate) fn due(&mut self, frames: usize, rate: f32) -> bool {
        if !self.capture.is_watched() {
            return false;
        }
        self.frames = self.frames.saturating_add(frames);
        if self.frames < (rate / FRAMES_PER_SECOND) as usize {
            return false;
        }
        self.frames = 0;
        true
    }

    /// Publishes one frame.
    pub(crate) fn publish(&mut self, frame: &MotionFrame) {
        self.capture.publish(frame);
    }

    /// Clears the display after a reset, so a stopped instrument is not shown still ringing.
    pub(crate) fn reset(&mut self, geometry: MotionGeometry) {
        self.frames = 0;
        self.capture.publish(&MotionFrame {
            geometry,
            ..MotionFrame::default()
        });
    }
}

/// The sounding voices a frame shows, newest strike first, as indices into the voice pool.
pub(crate) fn recent_voices(
    allocator: &VoiceAllocator,
    sounding: impl Fn(usize) -> bool,
) -> [Option<usize>; MOTION_VOICES] {
    let slots = allocator.slots();
    let mut selected: [Option<usize>; MOTION_VOICES] = [None; MOTION_VOICES];
    for (index, slot) in slots.iter().enumerate() {
        if !sounding(index) {
            continue;
        }
        for rank in 0..MOTION_VOICES {
            if selected[rank].is_none_or(|previous| slots[previous].age < slot.age) {
                selected[rank..].rotate_right(1);
                selected[rank] = Some(index);
                break;
            }
        }
    }
    selected
}

/// Pressure along a waveguide bore, from the mouthpiece (first point) to the open end (last).
///
/// The bore is one delay line holding a whole round trip of `length` samples, written at
/// `write`: a wave leaving the mouthpiece reaches a point `x` of the way down after `x·length/2`
/// samples and passes it again on the way back, `length − x·length/2` samples after it left. The
/// open end inverts pressure, so the two passes subtract and the far end is a pressure node.
pub(crate) fn bore_pressure(delay: &[f32], write: usize, length: usize, points: &mut [f32]) {
    let size = delay.len();
    if size < 2 || length < 2 {
        points.fill(0.0);
        return;
    }
    let length = length.min(size - 1) as f32;
    let last = points.len().saturating_sub(1).max(1) as f32;
    let at = |age: f32| {
        let back = (age.round() as usize).min(size - 1) + 1;
        delay[(write + size - back) % size]
    };
    for (index, point) in points.iter_mut().enumerate() {
        let x = index as f32 / last;
        let outgoing = at(x * length * 0.5);
        let returning = at(length - x * length * 0.5);
        *point = outgoing - returning;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standing_wave_in_the_bore_has_its_node_at_the_open_end() {
        // A quarter-wave resonance written into a round-trip line of 64 samples.
        let length = 64;
        let delay: Vec<f32> = (0..128)
            .map(|i| (std::f32::consts::TAU * i as f32 / (2.0 * length as f32)).sin())
            .collect();
        let mut points = [0.0; 16];
        bore_pressure(&delay, 32, length, &mut points);
        assert!(points[15].abs() < 0.12, "open end {}", points[15]);
        let loudest = points.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
        assert!(
            loudest > 1.0,
            "the closed end should swing, loudest {loudest}"
        );
    }

    #[test]
    fn an_unwatched_observer_is_never_due() {
        let mut observer = Observer::default();
        assert!(!observer.due(48_000, 48_000.0));
        observer.monitor().watch(true);
        assert!(!observer.due(100, 48_000.0));
        assert!(observer.due(1_600, 48_000.0));
        assert!(!observer.due(100, 48_000.0));
    }

    use crate::test_support::{Rig, count_allocations};
    use crate::{Choir, Clarinet, DrumKit, HammeredDulcimer, Model, Physical, TinWhistle};
    use auris_core::motion::TOUCH_CONTROLLER;
    use auris_core::{
        AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
    };

    fn on(pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame: 0,
            pitch,
            velocity: 0.8,
        }
    }

    fn touch(at: f32) -> NoteEvent {
        NoteEvent::Controller {
            frame: 0,
            number: TOUCH_CONTROLLER,
            value: at,
        }
    }

    /// Plays one held note on a watched instrument and reads the frame it publishes.
    fn observe(instrument: Box<dyn Instrument>, events: &[NoteEvent]) -> MotionFrame {
        let mut rig = Rig::new(instrument, 48_000.0, 256, 2);
        let monitor = rig.instrument.motion_monitor().expect("a body to observe");
        monitor.watch(true);
        rig.render(9_600, events);
        monitor.read().expect("a published frame")
    }

    fn loudest(points: &[f32]) -> f32 {
        points.iter().fold(0.0_f32, |a, b| a.max(b.abs()))
    }

    #[test]
    fn the_reeds_and_whistles_publish_their_bores_with_a_node_at_the_open_end() {
        for instrument in [
            Box::new(Clarinet::new()) as Box<dyn Instrument>,
            Box::new(TinWhistle::new()),
        ] {
            let name = instrument.descriptor().name;
            let frame = observe(instrument, &[on(74)]);
            assert_eq!(frame.geometry, MotionGeometry::Bore, "{name}");
            assert_eq!(frame.active, 1);
            let voice = &frame.voices[0];
            assert_eq!(voice.pitch, 74.0);
            assert!(
                voice.held && voice.level > 0.5,
                "{name} level {}",
                voice.level
            );
            let swing = loudest(&voice.points);
            // Displayed at 24×, a sounding bore fills a good part of its lane without clipping.
            assert!((0.01..0.05).contains(&swing), "{name} swings {swing}");
            assert!(
                voice.points[63].abs() < swing * 0.35,
                "{name}: the open end is a pressure node, {} of {swing}",
                voice.points[63]
            );
        }
    }

    #[test]
    fn the_choir_publishes_pressure_along_its_tract_and_the_vowel_it_is_shaped_for() {
        let mut choir = Choir::new();
        assert!(choir.set_param_by_key("vowel", 1.0));
        let frame = observe(Box::new(choir), &[on(57)]);
        assert_eq!(frame.geometry, MotionGeometry::Tract);
        let voice = &frame.voices[0];
        assert!(loudest(&voice.points) > 1e-4, "the tract is sounding");
        assert!(loudest(&voice.points) < 0.1);
        // /a/ opens towards the lips: narrow at the glottis, widest near the mouth.
        assert_eq!(voice.modes.iter().fold(0.0_f32, |a, b| a.max(*b)), 1.0);
        assert!(
            voice.modes[1] < 0.15 && voice.modes[7] == 1.0,
            "{:?}",
            voice.modes
        );
    }

    #[test]
    fn the_dulcimer_publishes_its_course_with_fixed_ends_and_its_modes() {
        let frame = observe(Box::new(HammeredDulcimer::new()), &[on(62)]);
        assert_eq!(frame.geometry, MotionGeometry::String);
        let voice = &frame.voices[0];
        assert!(
            (voice.contact - 0.126).abs() < 0.01,
            "contact {}",
            voice.contact
        );
        assert!(voice.points[0].abs() < 1e-6 && voice.points[63].abs() < 1e-6);
        assert!(loudest(&voice.points) > 1e-4);
        assert!(voice.modes[0] > voice.modes[5], "{:?}", voice.modes);
    }

    #[test]
    fn a_touch_moves_only_the_next_excitation() {
        let models: [Box<dyn Fn() -> Box<dyn Instrument>>; 3] = [
            Box::new(|| Box::new(Physical::new(Model::Guitar))),
            Box::new(|| Box::new(Physical::new(Model::Piano))),
            Box::new(|| Box::new(HammeredDulcimer::new())),
        ];
        for make in models {
            let resting = observe(make(), &[on(60)]).voices[0].contact;
            // Touched most of the way along, which is near the other end.
            let touched = observe(make(), &[touch(0.85), on(60)]).voices[0].contact;
            assert!((touched - 0.15).abs() < 0.02, "touched at {touched}");
            assert!((touched - resting).abs() > 0.01, "resting at {resting}");
            // The note after the touched one is excited wherever the control says again.
            let after = observe(make(), &[touch(0.75), on(60), on(64)]);
            let next = after
                .voices
                .iter()
                .find(|voice| voice.pitch == 64.0)
                .unwrap();
            assert!(
                (next.contact - resting).abs() < 0.02,
                "then at {}",
                next.contact
            );
        }
        let centre = observe(Box::new(DrumKit::new()), &[touch(0.0), on(38)]);
        assert!(centre.voices[0].contact < 0.01, "struck at the centre");
        let rim = observe(Box::new(DrumKit::new()), &[touch(0.9), on(38)]);
        assert!((rim.voices[0].contact - 0.9).abs() < 0.01);
    }

    #[test]
    fn observing_the_new_bodies_allocates_nothing_on_the_audio_thread() {
        for mut instrument in [
            Box::new(Clarinet::new()) as Box<dyn Instrument>,
            Box::new(TinWhistle::new()),
            Box::new(Choir::new()),
            Box::new(HammeredDulcimer::new()),
        ] {
            instrument.prepare(&PrepareContext::new(48_000.0, 256, 2));
            instrument.motion_monitor().unwrap().watch(true);
            let mut buffer = AudioBuffer::stereo(256, 48_000.0);
            let ctx = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
            let allocations = count_allocations(|| {
                for pitch in 50..90 {
                    instrument.process(&[touch(0.3), on(pitch)], &mut buffer, &ctx);
                }
                instrument.reset();
            });
            assert_eq!(allocations, 0, "{}", instrument.descriptor().name);
        }
    }
}
