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
    assert_eq!(peak(&silent), 0.0);
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
