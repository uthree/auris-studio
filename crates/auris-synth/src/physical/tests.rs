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
    let held = rig.render(4000, &[]);
    assert!(rms(&held) > 0.001);
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
    let original_ratio =
        goertzel(&moving[8000..], 48_000.0, 880.0) / goertzel(&moving[8000..], 48_000.0, 440.0);
    let changed_ratio = goertzel(&repositioned[8000..], 48_000.0, 880.0)
        / goertzel(&repositioned[8000..], 48_000.0, 440.0);
    assert!((original_ratio - changed_ratio).abs() > 0.05);
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
                instrument.process(&[on(pitch, 0.8)], &mut buffer, &ctx);
            }
            instrument.process(&[NoteEvent::AllSoundOff { frame: 50 }], &mut buffer, &ctx);
        });
        assert_eq!(allocations, 0, "{model:?} allocated on the audio thread");
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
