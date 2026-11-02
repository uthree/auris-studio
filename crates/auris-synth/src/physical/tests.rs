use super::*;
use crate::test_support::{Rig, count_allocations, goertzel, peak, rms};

fn rig(model: Model, rate: f64) -> Rig {
    let mut instrument = Physical::new(model);
    instrument.set_param_by_key("body", 0.0);
    Rig::new(Box::new(instrument), rate, 256, 2)
}

fn on(pitch: u8, velocity: f32) -> NoteEvent {
    NoteEvent::NoteOn {
        frame: 0,
        pitch,
        velocity,
    }
}

#[test]
fn each_model_has_a_tuned_fundamental_at_multiple_rates() {
    for model in Model::ALL {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut rig = rig(model, rate);
            let samples = rig.render(rate as usize, &[on(69, 0.8)]);
            let steady = &samples[(rate * 0.15) as usize..(rate * 0.8) as usize];
            let strongest = (425..456)
                .max_by(|a, b| {
                    goertzel(steady, rate, f64::from(*a)).total_cmp(&goertzel(
                        steady,
                        rate,
                        f64::from(*b),
                    ))
                })
                .unwrap();
            assert!(
                (strongest as f32 - 440.0).abs() <= 3.0,
                "{model:?} at {rate}: {strongest} Hz"
            );
            assert!(
                goertzel(steady, rate, 440.0) > 0.0002,
                "{model:?} has no fundamental"
            );
        }
    }
}

#[test]
fn strings_are_tuned_across_their_playing_registers() {
    for (model, pitches) in [
        (Model::Guitar, [45, 60, 81]),
        (Model::ElectricGuitar, [40, 64, 86]),
        (Model::Bass, [28, 40, 55]),
        (Model::Violin, [55, 69, 88]),
    ] {
        let mut rig = rig(model, 48_000.0);
        for pitch in pitches {
            rig.instrument.reset();
            let samples = rig.render(48_000, &[on(pitch, 0.8)]);
            let hz = f64::from(pitch_to_hz(f32::from(pitch)));
            let steady = &samples[10_000..40_000];
            let strongest = (-30_i32..=30)
                .max_by(|a, b| {
                    goertzel(steady, 48_000.0, hz * (f64::from(*a) / 1200.0).exp2()).total_cmp(
                        &goertzel(steady, 48_000.0, hz * (f64::from(*b) / 1200.0).exp2()),
                    )
                })
                .unwrap();
            assert!(
                strongest.abs() <= 8,
                "{model:?} MIDI {pitch}: {strongest} cents"
            );
        }
    }
}

/// Cents from `hz` at which the spectrum peaks, searched within `range` in tenths of a cent.
fn peak_cents(samples: &[f32], rate: f64, hz: f64, range: std::ops::RangeInclusive<i32>) -> f64 {
    range
        .map(|tenths| f64::from(tenths) * 0.1)
        .max_by(|a, b| {
            let strength = |cents: f64| goertzel(samples, rate, hz * (cents / 1200.0).exp2());
            strength(*a).total_cmp(&strength(*b))
        })
        .unwrap()
}

#[test]
fn piano_bass_overtones_do_not_have_tenor_register_stretch() {
    for (pitch, maximum_cents) in [(24, 20.0), (36, 12.0)] {
        let mut piano = rig(Model::Piano, 48_000.0);
        let audio = piano.render(48_000, &[on(pitch, 0.8)]);
        let hz = f64::from(pitch_to_hz(f32::from(pitch)));
        // The key's stretch tuning moves the fundamental; the overtone is measured against it.
        let fundamental = peak_cents(&audio, 48_000.0, hz, -400..=100);
        let eighth = peak_cents(&audio, 48_000.0, hz * 8.0, -400..=700);
        let stretch = eighth - fundamental;
        assert!(
            (0.0..=maximum_cents).contains(&stretch),
            "MIDI {pitch}: eighth partial stretched {stretch:.1} cents"
        );
    }
}

#[test]
fn piano_octaves_are_stretched_between_pure_two_one_and_four_two() {
    // Every fundamental on equal temperament left the tenor's 4:2 octaves 7.6 cents wide.
    let rate = 48_000.0;
    let steady = |pitch: u8| {
        let mut piano = rig(Model::Piano, rate);
        let audio = piano.render(rate as usize * 2, &[on(pitch, 0.7)]);
        audio[(rate * 0.3) as usize..].to_vec()
    };
    for low in [48_u8, 60, 72] {
        let (lower, upper) = (steady(low), steady(low + 12));
        let hz = f64::from(pitch_to_hz(f32::from(low)));
        let partial = |audio: &[f32], hz: f64| peak_cents(audio, rate, hz, -300..=300);
        let two_one = partial(&lower, hz * 2.0) - partial(&upper, hz * 2.0);
        let four_two = partial(&lower, hz * 4.0) - partial(&upper, hz * 4.0);
        assert!(
            two_one < 0.0 && four_two > 0.0 && (two_one + four_two).abs() < 1.0,
            "MIDI {low}: 2:1 {two_one:+.1} cents, 4:2 {four_two:+.1} cents"
        );
        assert!(
            four_two < 4.0,
            "MIDI {low}: 4:2 octave {four_two:+.1} cents wide"
        );
    }
}

#[test]
fn piano_stretch_keeps_a4_and_vanishes_without_stiffness() {
    let mut piano = Physical::new(Model::Piano);
    assert_eq!(piano.stretch(69), 0.0);
    assert!(piano.stretch(21) < -0.15 && piano.stretch(108) > 0.1);
    // Around C2 the wound strings are far less stiff than the plain ones an octave up, which
    // narrows those octaves; from A2 up the stiffness is one and the stretch only grows.
    for pair in (45..=108).collect::<Vec<u8>>().windows(2) {
        assert!(piano.stretch(pair[0]) < piano.stretch(pair[1]));
    }
    piano.set_param_by_key("stiffness", 0.0);
    for pitch in [21, 48, 69, 96, 108] {
        assert!(piano.stretch(pitch).abs() < 1e-6);
    }
    assert_eq!(Physical::new(Model::Mallet).stretch(21), 0.0);
}

#[test]
fn bowed_bass_attacks_settle_into_the_played_period() {
    for rate in [24_000.0, 44_100.0, 48_000.0, 96_000.0] {
        for pitch in [24, 36] {
            let mut violin = Rig::new(Box::new(Physical::new(Model::Violin)), rate, 256, 2);
            let audio = violin.render(rate as usize, &[on(pitch, 0.8)]);
            let window = &audio[(rate * 0.3) as usize..(rate * 0.8) as usize];
            let period = rate / f64::from(pitch_to_hz(f32::from(pitch)));
            let best = ((period * 0.995) as usize..=(period * 1.005).ceil() as usize)
                .map(|lag| {
                    let left = &window[..window.len() - lag];
                    let right = &window[lag..];
                    let mean = |samples: &[f32]| {
                        samples.iter().map(|s| f64::from(*s)).sum::<f64>() / samples.len() as f64
                    };
                    let left_mean = mean(left);
                    let right_mean = mean(right);
                    let mut product = 0.0;
                    let mut left_power = 0.0;
                    let mut right_power = 0.0;
                    for (a, b) in left.iter().zip(right) {
                        let a = f64::from(*a) - left_mean;
                        let b = f64::from(*b) - right_mean;
                        product += a * b;
                        left_power += a * a;
                        right_power += b * b;
                    }
                    product / (left_power * right_power).sqrt()
                })
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                best > 0.965,
                "MIDI {pitch} at {rate}: attack correlation {best}"
            );
        }
    }
}

#[test]
fn continuous_bow_scattering_keeps_the_upper_register_tuned() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for pitch in [69, 85, 88] {
            let mut violin = rig(Model::Violin, rate);
            let audio = violin.render(rate as usize, &[on(pitch, 0.8)]);
            let steady = &audio[(rate * 0.2) as usize..(rate * 0.8) as usize];
            let hz = f64::from(pitch_to_hz(f32::from(pitch)));
            let strongest = (-15_i32..=15)
                .max_by(|a, b| {
                    let strength =
                        |cents| goertzel(steady, rate, hz * (f64::from(cents) / 1200.0).exp2());
                    strength(*a).total_cmp(&strength(*b))
                })
                .unwrap();
            assert!(
                strongest.abs() <= 4,
                "MIDI {pitch} at {rate}: {strongest} cents"
            );
        }
    }
}

#[test]
fn damping_automation_changes_an_existing_resonance_without_retriggering() {
    for model in [Model::Piano, Model::Guitar, Model::Bell, Model::Mallet] {
        let mut instrument = rig(model, 48_000.0);
        instrument.set_param("damping", 0.0);
        instrument.set_param("decay", 6.0);
        instrument.render(8000, &[on(69, 0.8)]);
        let undamped = instrument.render(40_000, &[]);
        instrument.instrument.reset();
        instrument.render(8000, &[on(69, 0.8)]);
        instrument.set_param("damping", 1.0);
        let damped = instrument.render(40_000, &[]);
        assert!(
            rms(&damped[24_000..]) < rms(&undamped[24_000..]) * 0.8,
            "{model:?} ignored live damping"
        );
    }
}

#[test]
fn velocity_changes_energy_and_hard_contact_adds_upper_modes() {
    for model in Model::ALL {
        let mut instrument = rig(model, 48_000.0);
        let soft = instrument.render(12_000, &[on(60, 0.25)]);
        instrument.instrument.reset();
        let loud = instrument.render(12_000, &[on(60, 0.9)]);
        assert!(
            rms(&loud) > rms(&soft) * 1.5,
            "{model:?}: {} vs {}",
            rms(&loud),
            rms(&soft)
        );
    }
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("stiffness", 0.0);
    piano.set_param("hardness", 0.0);
    let soft = piano.render(8000, &[on(69, 0.8)]);
    piano.instrument.reset();
    piano.set_param("hardness", 1.0);
    let hard = piano.render(8000, &[on(69, 0.8)]);
    assert!(goertzel(&hard, 48_000.0, 4400.0) > goertzel(&soft, 48_000.0, 4400.0) * 3.0);
}

#[test]
fn struck_and_plucked_models_decay_while_the_bow_sustains() {
    for model in Model::ALL {
        let mut rig = rig(model, 48_000.0);
        rig.set_param("decay", 0.8);
        let samples = rig.render(96_000, &[on(69, 0.8)]);
        let early = rms(&samples[8000..16_000]);
        let late = rms(&samples[80_000..88_000]);
        if model == Model::Violin {
            assert!(
                late > early * 0.2,
                "bow failed to sustain: {late} vs {early}"
            );
        } else {
            assert!(
                late < early * 0.05,
                "{model:?} failed to decay: {late} vs {early}"
            );
        }
    }
}

#[test]
fn pedal_defers_piano_release_and_all_sound_off_overrides_it() {
    let mut rig = rig(Model::Piano, 48_000.0);
    rig.set_param("release", 0.05);
    rig.render(
        4000,
        &[
            on(60, 0.8),
            NoteEvent::Controller {
                frame: 100,
                number: 64,
                value: 1.0,
            },
            NoteEvent::NoteOff {
                frame: 2000,
                pitch: 60,
            },
        ],
    );
    assert_eq!(rig.instrument.active_voices(), 1);
    // Hold past the soundboard's knock, which no damper stops, so the release is the strings'.
    let held = rig.render(36_000, &[]);
    assert!(rms(&held[32_000..]) > 0.001);
    let released = rig.render(
        5000,
        &[NoteEvent::Controller {
            frame: 0,
            number: 64,
            value: 0.0,
        }],
    );
    assert!(peak(&released[3000..]) < 0.00001);
    assert_eq!(rig.instrument.active_voices(), 0);
    rig.render(
        3000,
        &[
            on(60, 0.8),
            NoteEvent::Controller {
                frame: 0,
                number: 64,
                value: 1.0,
            },
        ],
    );
    let killed = rig.render(512, &[NoteEvent::AllSoundOff { frame: 0 }]);
    assert!(peak(&killed[128..]) < 0.00001);
}

#[test]
fn bowed_expression_and_pitch_bend_control_a_sounding_note() {
    let mut rig = rig(Model::Violin, 48_000.0);
    rig.set_param("bow_response", 0.012);
    rig.render(24_000, &[on(69, 0.8)]);
    let bent = rig.render(
        24_000,
        &[NoteEvent::PitchBend {
            frame: 0,
            semitones: 12.0,
        }],
    );
    assert!(
        goertzel(&bent[8000..], 48_000.0, 880.0) > goertzel(&bent[8000..], 48_000.0, 440.0) * 5.0
    );
    let silent = rig.render(
        4000,
        &[NoteEvent::Controller {
            frame: 0,
            number: 11,
            value: 0.0,
        }],
    );
    assert!(peak(&silent[3000..]) < 0.0002);
}

#[test]
fn violin_expression_changes_preserve_the_wave_at_the_control_sample() {
    let mut changed = rig(Model::Violin, 48_000.0);
    let mut unchanged = rig(Model::Violin, 48_000.0);
    changed.set_param("bow_response", 0.012);
    unchanged.set_param("bow_response", 0.012);
    changed.render(24_000, &[on(69, 0.8)]);
    unchanged.render(24_000, &[on(69, 0.8)]);
    let control = changed.render(
        4000,
        &[NoteEvent::Controller {
            frame: 117,
            number: 11,
            value: 0.1,
        }],
    );
    let baseline = unchanged.render(4000, &[]);
    assert_eq!(&control[..117], &baseline[..117]);
    assert!(
        rms(&control[117..125]) > rms(&baseline[117..125]) * 0.95,
        "expression introduced a gain discontinuity"
    );
    assert!(rms(&control[3000..]) < rms(&baseline[3000..]) * 0.1);
}

#[test]
fn violin_sustains_for_twelve_seconds_at_multiple_rates_and_dynamics() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for velocity in [0.25, 0.8] {
            let mut violin = rig(Model::Violin, rate);
            let audio = violin.render((rate * 12.0) as usize, &[on(62, velocity)]);
            let early = &audio[rate as usize..(rate * 2.0) as usize];
            let late = &audio[(rate * 10.0) as usize..(rate * 11.0) as usize];
            assert!(audio.iter().all(|sample| sample.is_finite()));
            assert!((20.0 * (rms(late) / rms(early)).log10()).abs() < 1.0);
            let hz = f64::from(pitch_to_hz(62.0));
            let cents =
                (-15_i32..=15)
                    .max_by(|a, b| {
                        goertzel(late, rate, hz * (f64::from(*a) / 1200.0).exp2())
                            .total_cmp(&goertzel(late, rate, hz * (f64::from(*b) / 1200.0).exp2()))
                    })
                    .unwrap();
            assert!(
                cents.abs() < 8,
                "sustained pitch drift: {cents} cents at {rate}"
            );
        }
    }
}

#[test]
fn violin_bow_response_is_editable_during_a_held_note() {
    let mut levels = Vec::new();
    for response in [0.002, 0.12] {
        let mut violin = rig(Model::Violin, 48_000.0);
        violin.render(24_000, &[on(69, 0.8)]);
        violin.set_param("bow_response", response);
        let audio = violin.render(
            8000,
            &[NoteEvent::Controller {
                frame: 0,
                number: 11,
                value: 0.2,
            }],
        );
        assert!(audio.iter().all(|sample| sample.is_finite()));
        assert_eq!(violin.instrument.active_voices(), 1);
        levels.push(rms(&audio[100..500]));
    }
    assert!(
        levels[1] > levels[0] * 2.0,
        "live bow response did not change the held transition"
    );
}

#[test]
fn guitar_fundamental_decay_is_calibrated_across_fractional_delays() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for pitch in [45, 57, 69, 81] {
            let mut guitar = rig(Model::Guitar, rate);
            guitar.set_param("decay", 2.0);
            guitar.set_param("damping", 0.2);
            let audio = guitar.render(rate as usize, &[on(pitch, 0.8)]);
            let hz = f64::from(pitch_to_hz(f32::from(pitch)));
            let early = goertzel(
                &audio[(rate * 0.05) as usize..(rate * 0.25) as usize],
                rate,
                hz,
            );
            let late = goertzel(
                &audio[(rate * 0.55) as usize..(rate * 0.75) as usize],
                rate,
                hz,
            );
            let loss_db = 20.0 * (early / late).log10();
            // The calibrated register curve scales nominal decay per two octaves.
            // Measure its requested fundamental T60 independently of delay interpolation.
            let register = (f64::from(pitch) - 55.0) / 24.0;
            let decay = 2.0 * (-0.624 * register).exp();
            let damping = 0.2 + 0.0624 * register;
            let expected = 60.0 * 0.5 * (1.0 + 3.0 * damping) / decay;
            assert!(
                (loss_db - expected).abs() < 1.5,
                "{pitch} at {rate}: {loss_db} dB"
            );
        }
    }
}

#[test]
fn guitar_contact_and_pickup_change_the_spectrum_without_changing_pitch() {
    let mut guitar = rig(Model::Guitar, 48_000.0);
    // Hold loss and position fixed: changing factory calibration must not move the
    // measured tenth harmonic into a pluck-position notch or suppress it through loss.
    guitar.set_param("position", 0.22);
    guitar.set_param("decay", 2.8);
    guitar.set_param("damping", 0.12);
    guitar.set_param("hardness", 0.0);
    let soft = guitar.render(8000, &[on(69, 0.8)]);
    guitar.instrument.reset();
    guitar.set_param("hardness", 1.0);
    let hard = guitar.render(8000, &[on(69, 0.8)]);
    assert!(goertzel(&hard, 48_000.0, 4400.0) > goertzel(&soft, 48_000.0, 4400.0) * 1.8);
    guitar.instrument.reset();
    guitar.set_param("pickup", 1.0);
    let electric = guitar.render(8000, &[on(69, 0.8)]);
    let fundamental = goertzel(&electric, 48_000.0, 440.0);
    assert!(fundamental > 0.005);
    let acoustic_ratio = goertzel(&hard, 48_000.0, 880.0) / goertzel(&hard, 48_000.0, 440.0);
    let electric_ratio = goertzel(&electric, 48_000.0, 880.0) / fundamental;
    assert!((acoustic_ratio - electric_ratio).abs() > 0.1);
}

#[test]
fn level_automation_preserves_model_normalization_and_has_the_expected_gain() {
    for model in [Model::Piano, Model::Guitar, Model::Violin] {
        let mut instrument = Rig::new(Box::new(Physical::new(model)), 48_000.0, 256, 2);
        let original = instrument.render(8000, &[on(60, 0.8)]);
        instrument.instrument.reset();
        instrument.set_param("level", -12.0);
        let same_level = instrument.render(8000, &[on(60, 0.8)]);
        assert_eq!(
            original, same_level,
            "{model:?} changed normalization on automation"
        );
        instrument.instrument.reset();
        instrument.set_param("level", -18.0);
        let quieter = instrument.render(8000, &[on(60, 0.8)]);
        assert!((rms(&quieter) / rms(&original) - db_to_gain(-6.0)).abs() < 1e-5);
    }
}

#[test]
fn guitar_bend_reaches_its_target_and_stays_bounded() {
    let mut guitar = rig(Model::Guitar, 48_000.0);
    guitar.render(12_000, &[on(57, 0.8)]);
    let bent = guitar.render(
        24_000,
        &[NoteEvent::PitchBend {
            frame: 0,
            semitones: 12.0,
        }],
    );
    assert!(peak(&bent) < 1.0);
    assert!(
        goertzel(&bent[12_000..], 48_000.0, 440.0)
            > goertzel(&bent[12_000..], 48_000.0, 220.0) * 5.0
    );
}

#[test]
fn violin_legato_keeps_the_existing_wave_and_counts_overlapping_keys() {
    let mut control = rig(Model::Violin, 48_000.0);
    let mut legato = rig(Model::Violin, 48_000.0);
    for rig in [&mut control, &mut legato] {
        rig.set_param("legato", 1.0);
        rig.set_param("release", 0.05);
        rig.render(24_000, &[on(69, 0.8)]);
    }
    let unchanged = control.render(8000, &[]);
    let repeated = legato.render(8000, &[on(69, 0.8)]);
    assert_eq!(
        unchanged, repeated,
        "overlapping same-pitch legato restarted the string"
    );
    assert_eq!(legato.instrument.active_voices(), 1);
    let held = legato.render(
        8000,
        &[NoteEvent::NoteOff {
            frame: 0,
            pitch: 69,
        }],
    );
    assert!(rms(&held) > 0.001, "one off released both overlapping ons");
    let released = legato.render(
        8000,
        &[NoteEvent::NoteOff {
            frame: 0,
            pitch: 69,
        }],
    );
    assert!(peak(&released[4000..]) < 1e-6);
    assert_eq!(legato.instrument.active_voices(), 0);
}

#[test]
fn violin_legato_returns_to_the_previous_key_and_ignores_stale_offs() {
    let mut violin = rig(Model::Violin, 48_000.0);
    violin.set_param("legato", 1.0);
    violin.render(24_000, &[on(69, 0.8)]);
    let upper = violin.render(16_000, &[on(76, 0.8)]);
    assert_eq!(violin.instrument.active_voices(), 1);
    let hz = f64::from(pitch_to_hz(76.0));
    assert!(
        goertzel(&upper[8000..], 48_000.0, hz) > goertzel(&upper[8000..], 48_000.0, 440.0) * 3.0
    );
    let returned = violin.render(
        16_000,
        &[NoteEvent::NoteOff {
            frame: 0,
            pitch: 76,
        }],
    );
    assert!(
        goertzel(&returned[8000..], 48_000.0, 440.0)
            > goertzel(&returned[8000..], 48_000.0, hz) * 3.0
    );
    let held = violin.render(
        8000,
        &[NoteEvent::NoteOff {
            frame: 0,
            pitch: 76,
        }],
    );
    assert!(rms(&held) > 0.001);
    violin.render(12_000, &[NoteEvent::AllSoundOff { frame: 0 }]);
    assert_eq!(violin.instrument.active_voices(), 0);
    let fresh = violin.render(8000, &[on(72, 0.8)]);
    assert!(rms(&fresh) > 0.001);
}

#[test]
fn violin_defaults_to_polyphony_and_bow_controls_are_independent() {
    let mut violin = rig(Model::Violin, 48_000.0);
    violin.render(8000, &[on(69, 0.8), on(72, 0.8)]);
    assert_eq!(violin.instrument.active_voices(), 2);
    violin.instrument.reset();
    violin.set_param("bow_speed", 0.0);
    let stationary = violin.render(8000, &[on(69, 0.8)]);
    assert_eq!(peak(&stationary), 0.0);
    violin.set_param("bow_speed", 0.65);
    let moving = violin.render(24_000, &[]);
    assert!(rms(&moving[8000..]) > 0.001);
    violin.set_param("position", 0.3);
    let repositioned = violin.render(24_000, &[]);
    assert!(repositioned.iter().all(|sample| sample.is_finite()));
    // Bowing near one third of the string suppresses its third harmonic. The second
    // harmonic need not change appreciably in stable Helmholtz motion.
    let original_ratio =
        goertzel(&moving[8000..], 48_000.0, 1320.0) / goertzel(&moving[8000..], 48_000.0, 440.0);
    let changed_ratio = goertzel(&repositioned[8000..], 48_000.0, 1320.0)
        / goertzel(&repositioned[8000..], 48_000.0, 440.0);
    assert!(
        (original_ratio - changed_ratio).abs() > 0.05,
        "bow position ratios: {original_ratio}, {changed_ratio}"
    );
    violin.set_param("bow_speed", 0.0);
    let stopped = violin.render(48_000, &[]);
    assert!(rms(&stopped[40_000..]) < rms(&moving[8000..]) * 0.2);
}

#[test]
fn violin_legato_events_and_live_bow_automation_allocate_nothing() {
    let mut instrument = Physical::new(Model::Violin);
    instrument.prepare(&PrepareContext::new(48_000.0, 256, 2));
    instrument.set_param_by_key("legato", 1.0);
    let mut buffer = AudioBuffer::stereo(256, 48_000.0);
    let context = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
    let allocations = count_allocations(|| {
        for pitch in 55..85 {
            instrument.set_param(ParamId(P_BOW_SPEED), 0.6);
            instrument.set_param(ParamId(P_POSITION), 0.2);
            instrument.set_param(ParamId(P_BOW_RESPONSE), 0.02);
            instrument.process(
                &[
                    on(pitch, 0.8),
                    NoteEvent::NoteOff {
                        frame: 64,
                        pitch: pitch - 1,
                    },
                ],
                &mut buffer,
                &context,
            );
        }
        instrument.process(
            &[NoteEvent::AllSoundOff { frame: 64 }],
            &mut buffer,
            &context,
        );
    });
    assert_eq!(allocations, 0);
}

#[test]
fn every_model_stays_bounded_for_extreme_notes_and_parameters() {
    for model in Model::ALL {
        let mut rig = rig(model, 48_000.0);
        let ids: Vec<_> = rig
            .instrument
            .parameters()
            .iter()
            .map(|p| (p.id, p.max))
            .collect();
        for (id, value) in ids {
            rig.instrument.set_param(id, value);
        }
        let events: Vec<_> = (0..=127).step_by(5).map(|pitch| on(pitch, 1.0)).collect();
        let samples = rig.render(48_000, &events);
        assert!(
            samples.iter().all(|s| s.is_finite()),
            "{model:?} went non-finite"
        );
        assert!(
            peak(&samples) < 64.0,
            "{model:?} is unstable: {}",
            peak(&samples)
        );
        let released = rig.render(192_000, &[NoteEvent::AllNotesOff { frame: 0 }]);
        assert!(
            peak(&released[180_000..]) < 0.0001,
            "{model:?} never released"
        );
    }
}

#[test]
fn callback_and_parameter_changes_allocate_nothing_even_when_stealing() {
    for model in Model::ALL {
        let mut instrument = Physical::new(model);
        instrument.prepare(&PrepareContext::new(48_000.0, 256, 2));
        let monitor = instrument.motion_monitor().unwrap();
        monitor.watch(true);
        let mut buffer = AudioBuffer::stereo(256, 48_000.0);
        let ctx = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
        let allocations = count_allocations(|| {
            for pitch in 36..96 {
                instrument.set_param(ParamId(P_DAMPING), 0.5);
                // Every level is struck in turn, the oversampled strings among them.
                instrument.set_param_by_key("quality", f32::from(pitch % 3));
                if model == Model::ElectricGuitar {
                    instrument.set_param(ParamId(P_PICKUP_POSITION), 0.1);
                    instrument.set_param(ParamId(P_TONE), 3200.0);
                }
                instrument.process(&[on(pitch, 0.8)], &mut buffer, &ctx);
            }
            instrument.process(&[NoteEvent::AllSoundOff { frame: 50 }], &mut buffer, &ctx);
        });
        assert_eq!(allocations, 0, "{model:?} allocated on the audio thread");
    }
}

#[test]
fn electric_pickup_changes_observation_independently_of_pick_contact() {
    let mut guitar = rig(Model::ElectricGuitar, 48000.0);
    guitar.set_param("position", 0.14);
    guitar.set_param("pickup_position", 0.04);
    let bridge = guitar.render(24000, &[on(64, 0.8)]);
    let contact = guitar.instrument.param_by_key("position");
    guitar.instrument.reset();
    guitar.set_param("pickup_position", 0.18);
    let neck = guitar.render(24000, &[on(64, 0.8)]);
    assert_eq!(guitar.instrument.param_by_key("position"), contact);
    assert_ne!(bridge, neck);
    let hz = f64::from(pitch_to_hz(64.0));
    for audio in [&bridge, &neck] {
        let strongest = (-8_i32..=8)
            .max_by(|a, b| {
                goertzel(
                    &audio[4800..],
                    48000.0,
                    hz * (f64::from(*a) / 1200.0).exp2(),
                )
                .total_cmp(&goertzel(
                    &audio[4800..],
                    48000.0,
                    hz * (f64::from(*b) / 1200.0).exp2(),
                ))
            })
            .unwrap();
        assert!(
            strongest.abs() <= 2,
            "pickup moved the fundamental by {strongest} cents"
        );
    }
    assert!(guitar.instrument.param_by_key("body").is_none());
}

#[test]
fn electric_controls_have_contiguous_ids_and_narrow_pickup_settings_stay_bounded() {
    let mut instrument = Physical::new(Model::ElectricGuitar);
    for (index, descriptor) in instrument.parameters().iter().enumerate() {
        assert_eq!(descriptor.id.index(), index);
    }
    instrument.set_param_by_key("position", 0.05);
    instrument.set_param_by_key("pickup_position", 0.02);
    instrument.set_param_by_key("hardness", 1.0);
    instrument.set_param_by_key("pickup_q", 2.5);
    instrument.set_param_by_key("tone", 12000.0);
    let mut rig = Rig::new(Box::new(instrument), 48000.0, 256, 2);
    for pitch in [0, 10, 28, 40, 64, 86, 127] {
        rig.instrument.reset();
        let audio = rig.render(48000, &[on(pitch, 1.0)]);
        assert!(peak(&audio) < 32.0, "MIDI {pitch}: {}", peak(&audio));
    }
}

#[test]
fn mechanical_observation_reads_actual_state_without_changing_audio() {
    for model in Model::ALL {
        let mut watched = rig(model, 48_000.0);
        let mut plain = rig(model, 48_000.0);
        let monitor = watched.instrument.motion_monitor().unwrap();
        monitor.watch(true);
        let events = [
            on(60, 0.75),
            NoteEvent::PitchBend {
                frame: 150,
                semitones: 0.25,
            },
        ];
        assert_eq!(watched.render(4800, &events), plain.render(4800, &events));
        let frame = monitor.read().unwrap();
        assert_eq!(frame.active, 1);
        assert_eq!(frame.voices[0].pitch, 60.25);
        assert!(frame.voices[0].points.iter().all(|v| v.is_finite()));
        assert!(
            frame.voices[0].points.iter().any(|v| v.abs() > 1e-6),
            "{model:?} has no observed motion"
        );
        if frame.geometry == MotionGeometry::String {
            assert_eq!(frame.voices[0].points[0], 0.);
            assert_eq!(frame.voices[0].points[63], 0.);
        }
        watched.instrument.reset();
        assert_eq!(monitor.read().unwrap().active, 0);
    }
}

#[test]
fn pluck_contact_stays_with_the_excitation_until_the_next_note() {
    let mut guitar = rig(Model::Guitar, 48000.);
    let monitor = guitar.instrument.motion_monitor().unwrap();
    monitor.watch(true);
    guitar.set_param("position", 0.2);
    guitar.render(2000, &[on(60, 0.75)]);
    guitar.set_param("position", 0.4);
    guitar.render(2000, &[]);
    assert!((monitor.read().unwrap().voices[0].contact - 0.2).abs() < 1e-6);
    guitar.render(2000, &[on(64, 0.75)]);
    assert!((monitor.read().unwrap().voices[0].contact - 0.4).abs() < 1e-6);
}

fn level_db(samples: &[f32]) -> f64 {
    20.0 * f64::from(rms(samples)).max(1e-12).log10()
}

#[test]
fn piano_decays_in_two_stages_and_sustains_longer_in_the_bass() {
    let mut piano = rig(Model::Piano, 48_000.0);
    let middle = piano.render(6 * 48_000, &[on(60, 0.65)]);
    let at = |audio: &[f32], seconds: f64| {
        let start = (seconds * 48_000.0) as usize;
        level_db(&audio[start..start + 4800])
    };
    // The bridge drains the strings' shared motion first; what they do not share rings on.
    let prompt = (at(&middle, 0.1) - at(&middle, 0.6)) / 0.5;
    let after = (at(&middle, 2.0) - at(&middle, 5.0)) / 3.0;
    assert!(
        prompt > 2.5 * after,
        "prompt {prompt} dB/s, aftersound {after} dB/s"
    );
    assert!(after > 0.5, "aftersound must still decay: {after} dB/s");
    let mut decays = Vec::new();
    for pitch in [36, 84, 100] {
        piano.instrument.reset();
        let audio = piano.render(3 * 48_000, &[on(pitch, 0.65)]);
        decays.push((at(&audio, 0.5) - at(&audio, 2.5)) / 2.0);
    }
    assert!(
        decays.windows(2).all(|pair| pair[0] < pair[1]),
        "decay rates by register: {decays:?}"
    );
}

#[test]
fn piano_keys_above_the_dampers_ring_after_release() {
    let mut piano = rig(Model::Piano, 48_000.0);
    let release = |pitch| NoteEvent::NoteOff { frame: 4800, pitch };
    let damped = piano.render(48_000, &[on(72, 0.65), release(72)]);
    piano.instrument.reset();
    let free = piano.render(48_000, &[on(96, 0.65), release(96)]);
    let fall = |audio: &[f32]| level_db(&audio[..4800]) - level_db(&audio[19_200..24_000]);
    assert!(
        fall(&damped) > 40.0,
        "C5 damper removed only {} dB",
        fall(&damped)
    );
    assert!(
        fall(&free) < 25.0,
        "C7 has no damper, yet fell {} dB",
        fall(&free)
    );
    assert_eq!(piano.instrument.active_voices(), 1);
}

#[test]
fn piano_pedal_catches_a_note_its_damper_has_begun_to_stop() {
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("release", 0.5);
    let pedal = |frame, value| NoteEvent::Controller {
        frame,
        number: 64,
        value,
    };
    let events = [
        on(60, 0.65),
        NoteEvent::NoteOff {
            frame: 4800,
            pitch: 60,
        },
    ];
    let stopped = piano.render(48_000, &events);
    piano.instrument.reset();
    let caught = piano.render(48_000, &[events[0], events[1], pedal(7200, 1.0)]);
    assert_eq!(&stopped[..7200], &caught[..7200]);
    let tail = 38_400..48_000;
    assert!(
        level_db(&caught[tail.clone()]) > level_db(&stopped[tail]) + 20.0,
        "the pedal did not lift the damper"
    );
}

#[test]
fn piano_hammer_brightens_with_velocity_on_the_existing_loudness_law() {
    let mut piano = Rig::new(Box::new(Physical::new(Model::Piano)), 48_000.0, 256, 2);
    let mut take = |velocity| {
        piano.instrument.reset();
        piano.render(24_000, &[on(60, velocity)])
    };
    let (soft, middle, loud) = (take(0.35), take(0.65), take(0.95));
    let hz = f64::from(pitch_to_hz(60.0));
    let brightness = |audio: &[f32]| {
        let window = &audio[480..9600];
        let upper: f64 = (4..=8)
            .map(|n| goertzel(window, 48_000.0, hz * f64::from(n)))
            .sum();
        20.0 * (upper / goertzel(window, 48_000.0, hz)).log10()
    };
    assert!(
        brightness(&loud) > brightness(&soft) + 10.0,
        "soft {} dB, loud {} dB",
        brightness(&soft),
        brightness(&loud)
    );
    // The previous law gave -7.0 and +4.5 dB here; brightness may widen it only modestly.
    let quieter = level_db(&soft) - level_db(&middle);
    let louder = level_db(&loud) - level_db(&middle);
    assert!((-13.0..-5.0).contains(&quieter), "pp {quieter} dB");
    assert!((3.0..9.0).contains(&louder), "ff {louder} dB");
}

#[test]
fn piano_strike_knocks_the_soundboard_below_a_treble_note() {
    let mut piano = Rig::new(Box::new(Physical::new(Model::Piano)), 48_000.0, 256, 2);
    let audio = piano.render(48_000, &[on(96, 0.8)]);
    let mut lowpass: [_; 4] = std::array::from_fn(|_| {
        auris_dsp::Biquad::new(auris_dsp::BiquadCoefficients::lowpass(
            48_000.0,
            700.0,
            std::f32::consts::FRAC_1_SQRT_2,
        ))
    });
    let low: Vec<_> = audio
        .iter()
        .map(|x| {
            lowpass
                .iter_mut()
                .fold(*x, |y, section| section.process_sample(y))
        })
        .collect();
    // C7's fundamental is 2093 Hz: everything under 700 Hz is the soundboard's knock.
    let knock = level_db(&low[..2880]);
    assert!(knock > level_db(&audio[..2880]) - 30.0, "knock {knock} dB");
    let later = level_db(&low[24_000..26_880]);
    assert!(
        knock > later + 30.0,
        "knock {knock} dB, half a second later {later} dB"
    );
}

#[test]
fn mallet_bars_are_tuned_like_a_marimba() {
    let mut mallet = rig(Model::Mallet, 48_000.0);
    mallet.set_param("hardness", 1.0);
    let audio = mallet.render(24_000, &[on(60, 1.0)]);
    let hz = f64::from(pitch_to_hz(60.0));
    let strongest = (150..=650)
        .map(|hundredths| f64::from(hundredths) / 100.0)
        .max_by(|a, b| {
            goertzel(&audio[..12_000], 48_000.0, hz * a).total_cmp(&goertzel(
                &audio[..12_000],
                48_000.0,
                hz * b,
            ))
        })
        .unwrap();
    assert!(
        (strongest - 4.0).abs() < 0.02,
        "first overtone at {strongest}x"
    );
}

fn quality_rig(model: Model, live: Quality) -> Rig {
    let mut instrument = Physical::new(model);
    instrument.set_param_by_key("body", 0.0);
    instrument.set_param_by_key("quality", live.value());
    Rig::new(Box::new(instrument), 48_000.0, 256, 2)
}

/// Renders one note through `process` directly, offline or live.
fn render_note(instrument: &mut Physical, pitch: u8, frames: usize, offline: bool) -> Vec<f32> {
    instrument.prepare(&PrepareContext::new(48_000.0, 256, 2));
    let mut buffer = AudioBuffer::stereo(256, 48_000.0);
    let ctx = ProcessContext {
        is_offline: offline,
        ..ProcessContext::realtime(48_000.0, 256, 0, 120.0, true)
    };
    let mut samples = Vec::with_capacity(frames);
    for block in 0..frames.div_ceil(256) {
        let events = if block == 0 {
            vec![on(pitch, 0.8)]
        } else {
            Vec::new()
        };
        instrument.process(&events, &mut buffer, &ctx);
        samples.extend_from_slice(buffer.channel(0));
    }
    samples
}

#[test]
fn every_model_offers_a_live_and_a_render_quality() {
    for model in Model::ALL {
        let instrument = Physical::new(model);
        let find = |key: &str| {
            instrument
                .parameters()
                .iter()
                .find(|descriptor| descriptor.key == key)
                .cloned()
                .unwrap_or_else(|| panic!("{model:?} has no `{key}`"))
        };
        let live = find("quality");
        let render = find("render_quality");
        assert_eq!(live.choices.len(), 3);
        assert_eq!(render.choices.len(), 3);
        assert_eq!(Quality::from_value(live.default), Quality::Standard);
        assert_eq!(Quality::from_value(render.default), Quality::High);
        assert_eq!(render.id.0, live.id.0 + 1, "{model:?}");
        assert_eq!(instrument.quality(), Quality::Standard);
    }
}

#[test]
fn an_offline_block_strikes_at_the_render_quality_and_a_live_one_at_the_live_quality() {
    for model in [Model::Piano, Model::Violin, Model::Bell] {
        let mut split = Physical::new(model);
        split.set_param_by_key("quality", Quality::Draft.value());
        split.set_param_by_key("render_quality", Quality::High.value());
        let mut high = Physical::new(model);
        high.set_param_by_key("quality", Quality::High.value());
        let mut draft = Physical::new(model);
        draft.set_param_by_key("quality", Quality::Draft.value());

        let offline = render_note(&mut split, 33, 12_000, true);
        let live = render_note(&mut split, 33, 12_000, false);
        assert_eq!(
            offline,
            render_note(&mut high, 33, 12_000, false),
            "{model:?}"
        );
        assert_eq!(
            live,
            render_note(&mut draft, 33, 12_000, false),
            "{model:?}"
        );
        if model != Model::Bell {
            assert_ne!(offline, live, "{model:?} sounds the same at every quality");
        }
    }
}

#[test]
fn draft_piano_stays_close_to_the_calibrated_strike() {
    for (pitch, ceiling) in [(21, -28.0), (33, -35.0), (60, -48.0)] {
        let render = |quality| quality_rig(Model::Piano, quality).render(24_000, &[on(pitch, 0.9)]);
        let (standard, draft) = (render(Quality::Standard), render(Quality::Draft));
        let difference: Vec<f32> = draft.iter().zip(&standard).map(|(d, s)| d - s).collect();
        let error = 20.0 * f64::from(rms(&difference) / rms(&standard)).log10();
        assert!(
            error < ceiling,
            "MIDI {pitch}: draft differs by {error:.1} dB"
        );
        assert!(
            error.is_finite(),
            "MIDI {pitch}: draft is not cheaper than standard"
        );
    }
}

#[test]
fn oversampled_strings_stay_tuned_across_their_playing_registers() {
    for (model, pitches) in [
        (Model::Guitar, [45, 60, 81]),
        (Model::ElectricGuitar, [40, 64, 86]),
        (Model::Bass, [28, 40, 55]),
        (Model::Violin, [55, 69, 88]),
    ] {
        let mut rig = quality_rig(model, Quality::High);
        for pitch in pitches {
            rig.instrument.reset();
            let samples = rig.render(48_000, &[on(pitch, 0.8)]);
            let hz = f64::from(pitch_to_hz(f32::from(pitch)));
            let steady = &samples[10_000..40_000];
            let strongest = (-30_i32..=30)
                .max_by(|a, b| {
                    goertzel(steady, 48_000.0, hz * (f64::from(*a) / 1200.0).exp2()).total_cmp(
                        &goertzel(steady, 48_000.0, hz * (f64::from(*b) / 1200.0).exp2()),
                    )
                })
                .unwrap();
            assert!(
                strongest.abs() <= 8,
                "{model:?} MIDI {pitch} oversampled: {strongest} cents"
            );
            assert!(
                goertzel(steady, 48_000.0, hz) > 0.0002,
                "{model:?} MIDI {pitch} oversampled has no fundamental"
            );
        }
    }
}

#[test]
fn oversampling_keeps_a_plucked_strings_level_and_decay() {
    for model in [Model::Guitar, Model::Bass, Model::ElectricGuitar] {
        let render = |quality| quality_rig(model, quality).render(48_000, &[on(45, 0.8)]);
        let (standard, high) = (render(Quality::Standard), render(Quality::High));
        let level = |samples: &[f32]| level_db(&samples[2_400..9_600]);
        let fall = |samples: &[f32]| level(samples) - level_db(&samples[36_000..43_200]);
        assert!(
            (level(&high) - level(&standard)).abs() < 1.5,
            "{model:?} level {} vs {}",
            level(&high),
            level(&standard)
        );
        assert!(
            (fall(&high) - fall(&standard)).abs() < 3.0,
            "{model:?} decay {} vs {}",
            fall(&high),
            fall(&standard)
        );
    }
}

/// Tenths of a cent by which the `k`th harmonic of `hz` sits from `k × hz`.
fn harmonic_offset(samples: &[f32], hz: f64, k: f64) -> i32 {
    let at = |tenths: i32| {
        goertzel(
            samples,
            48_000.0,
            k * hz * (f64::from(tenths) / 12_000.0).exp2(),
        )
    };
    (-300..=300)
        .max_by(|a, b| at(*a).total_cmp(&at(*b)))
        .unwrap()
}

#[test]
fn oversampling_straightens_the_top_of_a_plucked_strings_harmonic_series() {
    // E6 on the guitar: a 36-sample loop whose interpolation stretches each upper harmonic
    // sharp at the host rate. Twice the rate takes the stretch out.
    let hz = f64::from(pitch_to_hz(88.0));
    let render = |quality| quality_rig(Model::Guitar, quality).render(36_000, &[on(88, 0.9)]);
    let (standard, high) = (render(Quality::Standard), render(Quality::High));
    let offsets =
        |samples: &[f32]| [3.0, 4.0, 5.0].map(|k| harmonic_offset(&samples[12_000..36_000], hz, k));
    let (standard, high) = (offsets(&standard), offsets(&high));
    assert!(
        standard.iter().all(|&tenths| tenths >= 10),
        "standard {standard:?}"
    );
    assert!(
        high.iter().all(|&tenths| tenths.abs() <= 3),
        "high {high:?}"
    );
}
