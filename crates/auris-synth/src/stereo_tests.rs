//! Numeric guards for where the physical instruments place their notes.
//!
//! Every one of them is built as a *mid* signal, the instrument as heard from in front, and a
//! *side* signal, what the right ear hears more of than the left. Two things follow and are
//! held here: the mono downmix is exactly the instrument at zero width, whatever the width
//! is set to; and where a note sits is how much of its mid signal is in its side.

use auris_core::{Instrument, NoteEvent, Parameterized};

use crate::test_support::{Rig, peak, rms};
use crate::{Model, Physical};

const RATE: f64 = 48_000.0;

fn on(frame: u32, pitch: u8) -> NoteEvent {
    NoteEvent::NoteOn {
        frame,
        pitch,
        velocity: 0.8,
    }
}

/// A low and a high note inside each instrument's range.
fn range(model: Model) -> (u8, u8) {
    match model {
        Model::Piano => (36, 96),
        Model::HammeredDulcimer => (55, 81),
        Model::Guitar | Model::ElectricGuitar => (45, 79),
        Model::Bass => (31, 60),
        Model::Violin => (43, 84),
        Model::Mallet => (45, 84),
        Model::Bell => (60, 84),
        Model::Clarinet => (55, 86),
        Model::TinWhistle => (76, 93),
    }
}

fn instrument(model: Model, width: f32) -> Box<dyn Instrument> {
    let mut instrument = Physical::new(model);
    instrument.set_param_by_key("width", width);
    // One bow with a steady hand: a section's players sit either side of the note.
    instrument.set_param_by_key("players", 1.0);
    instrument.set_param_by_key("vibrato", 0.0);
    Box::new(instrument)
}

/// Where a sound sits: how much of its mid signal is in its side, from -1 hard left to 1 hard
/// right. A note panned by `p` has left `1 - p` and right `1 + p` of itself, and this is `p`.
fn placement(left: &[f32], right: &[f32]) -> f64 {
    let (mut along, mut power) = (0.0_f64, 0.0_f64);
    for (l, r) in left.iter().zip(right) {
        let (mid, side) = (f64::from(l + r), f64::from(r - l));
        along += mid * side;
        power += mid * mid;
    }
    along / power.max(1.0e-30)
}

#[test]
fn every_note_sits_where_its_instrument_puts_it_at_every_width() {
    for model in Model::ALL {
        let (low, high) = range(model);
        for width in [0.5_f32, 1.0, 2.0] {
            for pitch in [low, high] {
                let mut rig = Rig::new(instrument(model, width), RATE, 256, 2);
                let (left, right) = rig.render_stereo(14_400, &[on(0, pitch)]);
                let expected = f64::from(model.stereo_image().position(f32::from(pitch)) * width);
                let measured = placement(&left[2_400..], &right[2_400..]);
                // A body's own width leans each resonance one way or the other, which moves
                // a single note a little off its seat; a pipe or a bar has no body to do it.
                let tolerance = match model {
                    Model::Mallet | Model::Clarinet | Model::TinWhistle | Model::ElectricGuitar => {
                        0.01
                    }
                    // A bell's two halves are heard to either side, and seldom equally.
                    Model::Bell => 0.2 * f64::from(width),
                    _ => 0.14 * f64::from(width),
                };
                assert!(
                    (measured - expected).abs() < tolerance,
                    "{model:?} pitch {pitch} width {width}: at {measured:.3}, seat {expected:.3}"
                );
            }
        }
    }
}

#[test]
fn a_piano_is_heard_from_the_players_seat() {
    let mut rig = Rig::new(instrument(Model::Piano, 1.0), RATE, 256, 2);
    let (left, right) = rig.render_stereo(24_000, &[on(0, 36)]);
    let low = placement(&left, &right);
    let difference = 20.0 * (rms(&left) / rms(&right)).log10();
    rig.instrument.reset();
    let (left, right) = rig.render_stereo(24_000, &[on(0, 96)]);
    let high = placement(&left, &right);
    rig.instrument.reset();
    let (left, right) = rig.render_stereo(24_000, &[on(0, 64)]);
    let middle = placement(&left, &right);
    assert!((-0.5..-0.25).contains(&low), "C2 sits at {low}");
    assert!((0.25..0.55).contains(&high), "C7 sits at {high}");
    assert!(middle.abs() < 0.08, "E4 sits at {middle}");
    // In level, C2 is some decibels louder on the left.
    assert!(
        (4.0..10.0).contains(&difference),
        "C2 is {difference} dB left"
    );
}

#[test]
fn strings_follow_orchestral_seating() {
    let mut rig = Rig::new(instrument(Model::Violin, 1.0), RATE, 256, 2);
    let (left, right) = rig.render_stereo(24_000, &[on(0, 43)]);
    assert!(
        placement(&left, &right) > 0.2,
        "a low string note should sit on the right, with the cellos"
    );
    rig.instrument.reset();
    let (left, right) = rig.render_stereo(24_000, &[on(0, 84)]);
    assert!(
        placement(&left, &right) < -0.2,
        "a high string note should sit on the left, with the violins"
    );
}

#[test]
fn a_section_is_wider_than_its_first_desk() {
    let side_share = |players: f32| {
        let mut strings = Physical::new(Model::Violin);
        strings.set_param_by_key("players", players);
        let mut rig = Rig::new(Box::new(strings), RATE, 256, 2);
        // D4 sits near the middle of the section.
        let (left, right) = rig.render_stereo(48_000, &[on(0, 62)]);
        let side: Vec<f32> = left.iter().zip(&right).map(|(l, r)| r - l).collect();
        let mid: Vec<f32> = left.iter().zip(&right).map(|(l, r)| r + l).collect();
        rms(&side[12_000..]) / rms(&mid[12_000..])
    };
    let (solo, section) = (side_share(1.0), side_share(4.0));
    assert!(section > solo * 1.3, "solo {solo}, four desks {section}");
}

#[test]
fn the_downmix_is_the_centred_instrument() {
    for model in Model::ALL {
        let (low, high) = range(model);
        let middle = ((u16::from(low) + u16::from(high)) / 2) as u8;
        let chord = [on(0, low), on(1_000, middle), on(2_000, high)];

        let mut centred = Rig::new(instrument(model, 0.0), RATE, 256, 2);
        let (left, right) = centred.render_stereo(12_000, &chord);
        assert_eq!(left, right, "{model:?} at zero width is not mono");

        let mut wide = Rig::new(instrument(model, 1.0), RATE, 256, 2);
        let (wide_left, wide_right) = wide.render_stereo(12_000, &chord);
        assert_ne!(wide_left, wide_right, "{model:?} has no stereo image");
        let downmix: Vec<f32> = wide_left
            .iter()
            .zip(&wide_right)
            .map(|(l, r)| (l + r) * 0.5)
            .collect();
        let error: Vec<f32> = downmix.iter().zip(&left).map(|(a, b)| a - b).collect();
        // The two channels each round on their own; -80 dB leaves room for any platform's
        // arithmetic and none for a mid signal that width has coloured.
        assert!(
            peak(&error) <= peak(&left) * 1.0e-4,
            "{model:?}'s downmix differs from its centred render by {}",
            peak(&error)
        );

        let mut mono = Rig::new(instrument(model, 1.0), RATE, 256, 1);
        let mono = mono.render(12_000, &chord);
        let error: Vec<f32> = mono.iter().zip(&left).map(|(a, b)| a - b).collect();
        assert!(
            peak(&error) <= peak(&left) * 1.0e-4,
            "{model:?} on a mono bus differs from its centred render"
        );
    }
}

#[test]
fn a_body_is_wide_on_its_own_and_only_as_wide_as_asked() {
    // A guitar's notes all sit within a hair of the centre; what spreads it is the top plate,
    // whose resonances do not all radiate the same way.
    let side_share = |width: f32| {
        let mut rig = Rig::new(instrument(Model::Guitar, width), RATE, 256, 2);
        let (left, right) = rig.render_stereo(24_000, &[on(0, 64)]);
        let side: Vec<f32> = left.iter().zip(&right).map(|(l, r)| r - l).collect();
        let mid: Vec<f32> = left.iter().zip(&right).map(|(l, r)| r + l).collect();
        rms(&side) / rms(&mid)
    };
    let (narrow, natural, wide) = (side_share(0.0), side_share(1.0), side_share(2.0));
    assert_eq!(narrow, 0.0);
    assert!((0.05..0.6).contains(&natural), "side over mid {natural}");
    assert!((wide / natural - 2.0).abs() < 0.05, "{natural} then {wide}");
}

#[test]
fn surround_buses_carry_the_downmix_beyond_the_first_pair() {
    for model in Model::ALL {
        // The rig checks every channel past the second against the downmix itself.
        let mut rig = Rig::new(instrument(model, 1.0), RATE, 256, 4);
        let rendered = rig.render(4_096, &[on(0, range(model).0)]);
        assert!(
            peak(&rendered) > 0.0,
            "{model:?} was silent on a four-channel bus"
        );
    }
}

#[test]
fn turning_the_width_moves_a_held_note_without_a_step() {
    let mut rig = Rig::new(instrument(Model::Mallet, 1.0), RATE, 256, 2);
    rig.set_param("decay", 8.0);
    let (left, right) = rig.render_stereo(4_096, &[on(0, 45)]);
    let before = placement(&left[2_048..], &right[2_048..]);
    rig.set_param("width", 0.0);
    let (left, right) = rig.render_stereo(4_096, &[]);
    // Five milliseconds in, the note is still on its way; by the end it has arrived.
    let early = placement(&left[..240], &right[..240]);
    let late = placement(&left[2_048..], &right[2_048..]);
    assert!(before < -0.3, "A2 started at {before}");
    assert!(early < -0.05 && early > before, "the note jumped: {early}");
    assert!(late.abs() < 0.005, "the note never arrived: {late}");
}
