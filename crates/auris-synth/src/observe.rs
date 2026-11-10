//! Mechanical observation shared by the instruments that publish their bodies' motion.
//!
//! [`Physical`](crate::Physical) and [`DrumKit`](crate::DrumKit) publish through their own
//! shells; the choir and the oscillator synths share this one. Everything here runs on the
//! audio thread and works in fixed-size arrays: no allocation, no locking.

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

/// The magnitudes of the first `out.len()` harmonics of one cycle, relative to the strongest.
///
/// A plain DFT at the harmonic bins: eight bins of a sixty-four point cycle is five hundred
/// multiplies, at display rate, which is less than one voice spends on one block.
pub(crate) fn harmonics(cycle: &[f32], out: &mut [f32]) {
    let length = cycle.len().max(1) as f32;
    for (index, value) in out.iter_mut().enumerate() {
        let harmonic = (index + 1) as f32;
        let (mut real, mut imaginary) = (0.0_f32, 0.0_f32);
        for (at, sample) in cycle.iter().enumerate() {
            let angle = std::f32::consts::TAU * harmonic * at as f32 / length;
            real += sample * angle.cos();
            imaginary += sample * angle.sin();
        }
        *value = real.hypot(imaginary);
    }
    let loudest = out.iter().fold(0.0_f32, |a, b| a.max(*b));
    if loudest > 1e-6 && loudest.is_finite() {
        for value in out.iter_mut() {
            *value /= loudest;
        }
    } else {
        out.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_has_odd_harmonics_falling_as_one_over_n() {
        let cycle: Vec<f32> = (0..64).map(|i| if i < 32 { 1.0 } else { -1.0 }).collect();
        let mut out = [0.0; 8];
        harmonics(&cycle, &mut out);
        assert!((out[0] - 1.0).abs() < 1e-5);
        assert!(out[1] < 1e-4 && out[3] < 1e-4, "{out:?}");
        assert!((out[2] - 1.0 / 3.0).abs() < 0.01, "{out:?}");
        assert!((out[4] - 1.0 / 5.0).abs() < 0.01, "{out:?}");
        let mut silent = [1.0; 8];
        harmonics(&[0.0; 64], &mut silent);
        assert_eq!(silent, [0.0; 8]);
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
    use crate::{Chiptune, Choir, DrumKit, Fm2, Model, Physical};
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
    fn every_physical_model_publishes_its_spectrum_and_what_drives_it() {
        for model in Model::ALL {
            let pitch = if model == Model::Bass { 40 } else { 67 };
            let mut rig = Rig::new(Box::new(Physical::new(model)), 48_000.0, 256, 2);
            let monitor = rig.instrument.motion_monitor().expect("a body to observe");
            monitor.watch(true);
            // The largest each picture gets over the first half second of a note.
            let (mut body, mut partial, mut high, mut low) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
            for block in 0..12 {
                let events = if block == 0 {
                    vec![on(pitch)]
                } else {
                    Vec::new()
                };
                rig.render(2_400, &events);
                let frame = monitor.read().expect("a published frame");
                let voice = &frame.voices[0];
                body = body.max(loudest(&frame.body));
                partial = partial.max(loudest(&voice.partials));
                high = voice.scope.iter().fold(high, |a, b| a.max(*b));
                low = voice.scope.iter().fold(low, |a, b| a.min(*b));
                assert!(
                    voice
                        .scope
                        .iter()
                        .chain(&voice.partials)
                        .all(|x| x.is_finite())
                );
            }
            // Displayed at 24×, like the string or the bore beside it.
            assert!(
                (0.002..0.1).contains(&partial),
                "{model:?}: its loudest partial is {partial}"
            );
            match model {
                // A blow is a force that rises and falls and is never a pull.
                Model::Piano | Model::HammeredDulcimer | Model::Mallet | Model::Bell => {
                    assert!(
                        low == 0.0 && (0.05..1.5).contains(&high),
                        "{model:?}: {low}..{high}"
                    );
                }
                // A string under a bow moves at the bow's speed while it is gripped, and
                // flies back several times faster when it lets go. As the bow slows at the end
                // of a note the string is still going at the speed of a moment before.
                Model::Violin => {
                    assert!(
                        (0.8..2.5).contains(&high),
                        "gripped at {high} of the bow's speed"
                    );
                    assert!(low < -3.0, "flies back at {low}");
                }
                // A reed stands open at rest and is drawn most of the way shut each period.
                Model::Clarinet => {
                    assert!(high > 0.7 && high <= 2.0 && low == 0.0, "{low}..{high}");
                }
                // A jet swings to both sides of the edge.
                Model::TinWhistle => assert!(low < -0.3 && high > 0.2, "{low}..{high}"),
                // A finger lets go and is gone.
                Model::Guitar | Model::ElectricGuitar | Model::Bass => {
                    assert_eq!((low, high), (0.0, 0.0), "{model:?}");
                }
            }
            match model {
                Model::Piano | Model::HammeredDulcimer | Model::Guitar | Model::Violin => {
                    assert!(
                        (0.002..0.1).contains(&body),
                        "{model:?}: its body swings {body}"
                    );
                }
                // No box, or a pickup with its body turned off.
                _ => assert_eq!(body, 0.0, "{model:?}"),
            }
        }
    }

    #[test]
    fn the_reeds_and_whistles_publish_their_bores_with_a_node_at_the_open_end() {
        for model in [Model::Clarinet, Model::TinWhistle] {
            let frame = observe(Box::new(Physical::new(model)), &[on(74)]);
            assert_eq!(frame.geometry, MotionGeometry::Bore, "{model:?}");
            assert_eq!(frame.active, 1);
            let voice = &frame.voices[0];
            assert_eq!(voice.pitch, 74.0);
            assert!(
                voice.held && voice.level > 0.01,
                "{model:?} level {}",
                voice.level
            );
            let swing = loudest(&voice.points);
            // Displayed at 24×, a sounding bore fills a good part of its lane without clipping.
            assert!((0.002..0.05).contains(&swing), "{model:?} swings {swing}");
            assert!(
                voice.points[63].abs() < swing * 0.35,
                "{model:?}: the open end is a pressure node, {} of {swing}",
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
    fn the_chiptune_publishes_one_cycle_of_its_pulse_in_its_bit_depth() {
        let mut synth = Chiptune::new();
        assert!(synth.set_param_by_key("pulse_width", 0.25));
        assert!(synth.set_param_by_key("bits", 3.0));
        assert!(synth.set_param_by_key("octave", 1.0));
        let frame = observe(Box::new(synth), &[on(60)]);
        assert_eq!(frame.geometry, MotionGeometry::Oscillator);
        let voice = &frame.voices[0];
        // The octave switch is part of the note it plays.
        assert_eq!(voice.pitch, 72.0);
        assert!(voice.held && voice.level > 0.4, "level {}", voice.level);
        // A quarter of the period high, at the level the envelope has reached.
        let high = voice.points.iter().filter(|p| **p > 0.0).count();
        assert_eq!(high, 16);
        assert!((loudest(&voice.points) - voice.level).abs() < 1e-4);
        // A 25 % pulse has no fourth harmonic, and its second is strong.
        assert!(
            voice.modes[3] < 0.02 && voice.modes[1] > 0.6,
            "{:?}",
            voice.modes
        );

        let mut triangle = Chiptune::new();
        assert!(triangle.set_param_by_key("waveform", 3.0));
        assert!(triangle.set_param_by_key("bits", 3.0));
        let frame = observe(Box::new(triangle), &[on(60)]);
        // Three bits are four steps either side of zero: nine levels, and no others.
        let mut levels: Vec<i32> = frame.voices[0]
            .points
            .iter()
            .map(|p| (p / frame.voices[0].level * 4.0).round() as i32)
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert_eq!(levels, (-4..=4).collect::<Vec<_>>());
    }

    #[test]
    fn the_fm_operator_publishes_its_index_falling_to_a_sine() {
        let mut synth = Fm2::new();
        assert!(synth.set_param_by_key("index", 6.0));
        assert!(synth.set_param_by_key("mod_decay", 0.4));
        let mut rig = Rig::new(Box::new(synth), 48_000.0, 256, 2);
        let monitor = rig.instrument.motion_monitor().expect("a body to observe");
        monitor.watch(true);
        rig.render(1_600, &[on(57)]);
        let early = monitor.read().expect("a frame");
        assert_eq!(early.geometry, MotionGeometry::Oscillator);
        assert!(
            early.voices[0].excitation > 2.0,
            "{}",
            early.voices[0].excitation
        );
        // Bright: the upper harmonics are there while the index is high.
        assert!(
            early.voices[0].modes[3..].iter().any(|m| *m > 0.3),
            "{:?}",
            early.voices[0].modes
        );
        rig.render(30_000, &[]);
        let settled = monitor.read().expect("a frame");
        let voice = &settled.voices[0];
        assert!(voice.excitation < 0.01, "{}", voice.excitation);
        // Settled into the carrier alone.
        assert!(
            voice.modes[1..].iter().all(|m| *m < 0.02),
            "{:?}",
            voice.modes
        );
        assert_eq!(voice.pitch, 57.0);
    }

    #[test]
    fn the_dulcimer_publishes_its_course_with_fixed_ends_and_its_modes() {
        let frame = observe(Box::new(Physical::new(Model::HammeredDulcimer)), &[on(62)]);
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
            Box::new(|| Box::new(Physical::new(Model::HammeredDulcimer))),
        ];
        for make in models {
            let resting = observe(make(), &[on(60)]).voices[0].contact;
            // Touched most of the way along, which is near the other end.
            let touched = observe(make(), &[touch(0.85), on(60)]).voices[0].contact;
            assert!((touched - 0.15).abs() < 0.02, "touched at {touched}");
            assert!((touched - resting).abs() > 0.01, "resting at {resting}");
            // No two plucks land on quite the same spot, and a fret moves it along the string.
            // The note after the touched one is excited wherever the control says again.
            let after = observe(make(), &[touch(0.75), on(60), on(64)]);
            let next = after
                .voices
                .iter()
                .find(|voice| voice.pitch == 64.0)
                .unwrap();
            assert!(
                (next.contact - resting).abs() < 0.04,
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
            Box::new(Choir::new()) as Box<dyn Instrument>,
            Box::new(Fm2::new()),
            Box::new(Chiptune::new()),
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
