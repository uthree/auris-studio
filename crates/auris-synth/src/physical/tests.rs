//! Numeric guards for every physical instrument: what each is tuned to, how it decays and how
//! it answers the player.

use auris_core::motion::MotionGeometry;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamId, Parameterized, PrepareContext, ProcessContext,
};

use super::{Model, Physical, Quality};
use crate::test_support::{Rig, count_allocations, goertzel, peak, rms};

/// An instrument heard at its bridge or its mouth: no body, one player with a steady hand.
fn bare(model: Model) -> Physical {
    let mut instrument = Physical::new(model);
    instrument.set_param_by_key("body", 0.0);
    instrument.set_param_by_key("players", 1.0);
    instrument.set_param_by_key("vibrato", 0.0);
    instrument
}

fn rig(model: Model, rate: f64) -> Rig {
    Rig::new(Box::new(bare(model)), rate, 256, 2)
}

fn whole(model: Model) -> Rig {
    Rig::new(Box::new(Physical::new(model)), 48_000.0, 256, 2)
}

fn on(pitch: u8, velocity: f32) -> NoteEvent {
    NoteEvent::note_on(0, pitch, velocity)
}

fn off(frame: u32, pitch: u8) -> NoteEvent {
    NoteEvent::NoteOff { frame, key: pitch }
}

fn controller(frame: u32, number: u8, value: f32) -> NoteEvent {
    NoteEvent::Controller {
        frame,
        number,
        value,
    }
}

fn hz(pitch: u8) -> f64 {
    f64::from(pitch_to_hz(f32::from(pitch)))
}

fn level_db(samples: &[f32]) -> f64 {
    20.0 * f64::from(rms(samples)).max(1e-12).log10()
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

/// The strongest reading of a stiff string's `n`th partial, which runs a little sharp of
/// `n` times the fundamental.
fn partial(samples: &[f32], fundamental: f64, n: f64) -> f64 {
    (-3..=14)
        .map(|step| {
            goertzel(
                samples,
                48_000.0,
                fundamental * n * (1.0 + f64::from(step) * 0.002),
            )
        })
        .fold(0.0, f64::max)
}

/// How much of `samples` is at `hz`, read through a Hann window: a short stretch of sound
/// has neighbouring partials a few bins away, and a bare rectangle lets them leak in.
fn windowed(samples: &[f32], rate: f64, hz: f64) -> f64 {
    let last = (samples.len() - 1) as f64;
    let shaped: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(n, sample)| {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / last).cos();
            (f64::from(*sample) * window * 2.0) as f32
        })
        .collect();
    goertzel(&shaped, rate, hz)
}

/// A note in each model's range whose own frequency is one of its partials.
fn home(model: Model) -> u8 {
    match model {
        Model::Bass => 45,
        Model::TinWhistle => 81,
        _ => 69,
    }
}

#[test]
fn every_model_plays_its_key_at_every_rate() {
    for model in Model::ALL {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut rig = rig(model, rate);
            let pitch = home(model);
            let samples = rig.render(rate as usize, &[on(pitch, 0.8)]);
            let steady = &samples[(rate * 0.25) as usize..(rate * 0.9) as usize];
            let cents = peak_cents(steady, rate, hz(pitch), -400..=400);
            // A hard pluck starts a few cents sharp and a bow's grip pulls a note a little
            // flat; nothing is further out than a player's ear would let it be.
            assert!(cents.abs() < 6.0, "{model:?} at {rate}: {cents} cents");
            assert!(
                goertzel(steady, rate, hz(pitch)) > 1.0e-4,
                "{model:?} has no fundamental"
            );
        }
    }
}

#[test]
fn every_model_is_in_tune_across_its_playing_register() {
    for (model, pitches) in [
        (Model::HammeredDulcimer, [50, 62, 86]),
        (Model::Guitar, [40, 60, 81]),
        (Model::ElectricGuitar, [40, 64, 86]),
        (Model::Bass, [28, 40, 55]),
        (Model::Violin, [36, 60, 88]),
        (Model::Mallet, [45, 65, 93]),
        (Model::Clarinet, [50, 67, 86]),
        (Model::TinWhistle, [74, 86, 98]),
        (Model::Brass, [55, 67, 82]),
    ] {
        let mut rig = rig(model, 48_000.0);
        for pitch in pitches {
            rig.instrument.reset();
            let samples = rig.render(48_000, &[on(pitch, 0.6)]);
            let cents = peak_cents(&samples[12_000..40_000], 48_000.0, hz(pitch), -300..=300);
            assert!(cents.abs() < 7.0, "{model:?} MIDI {pitch}: {cents} cents");
        }
    }
}

#[test]
fn nothing_depends_on_the_hosts_block_size() {
    // Every control moves on a clock that counts samples, not blocks, so a render is the same
    // whether the host asks for 64 frames at a time or 1024 — sample for sample.
    for model in Model::ALL {
        let pitch = home(model);
        let events = [
            NoteEvent::note_on(73, pitch, 0.8),
            controller(900, 64, 1.0),
            NoteEvent::note_on(1_301, pitch + 4, 0.5),
            NoteEvent::PitchBend {
                frame: 2_000,
                semitones: 0.5,
            },
            controller(2_600, 11, 0.6),
            off(3_333, pitch),
            controller(5_000, 64, 0.0),
            off(6_100, pitch + 4),
        ];
        let mut renders = Vec::new();
        for block in [64, 250, 1_024] {
            let mut rig = Rig::new(Box::new(Physical::new(model)), 48_000.0, block, 2);
            renders.push(rig.render_stereo(12_000, &events));
        }
        assert!(peak(&renders[0].0) > 0.0);
        assert!(
            renders[0] == renders[1],
            "{model:?}: 64 and 250 frames differ"
        );
        assert!(
            renders[0] == renders[2],
            "{model:?}: 64 and 1024 frames differ"
        );
    }
}

#[test]
fn a_reset_instrument_plays_the_same_again() {
    for model in Model::ALL {
        let mut rig = whole(model);
        let events = [on(home(model), 0.8), on(home(model) + 7, 0.6)];
        let first = rig.render(6_000, &events);
        rig.instrument.reset();
        let second = rig.render(6_000, &events);
        assert!(
            first == second,
            "{model:?} remembered something across a reset"
        );
    }
}

#[test]
fn velocity_is_heard_as_level_in_every_model() {
    for model in Model::ALL {
        let mut instrument = rig(model, 48_000.0);
        let soft = instrument.render(14_400, &[on(home(model), 0.25)]);
        instrument.instrument.reset();
        let loud = instrument.render(14_400, &[on(home(model), 0.9)]);
        let (soft, loud) = (rms(&soft[4_800..]), rms(&loud[4_800..]));
        assert!(loud > soft * 1.5, "{model:?}: {loud} against {soft}");
    }
}

#[test]
fn struck_and_plucked_models_decay_while_bowed_and_blown_ones_sustain() {
    for model in Model::ALL {
        let mut rig = rig(model, 48_000.0);
        if !model.is_sustained() {
            rig.set_param("decay", 0.8);
        }
        let samples = rig.render(96_000, &[on(home(model), 0.8)]);
        let early = rms(&samples[14_400..24_000]);
        let late = rms(&samples[81_600..91_200]);
        if model.is_sustained() {
            assert!(
                (late / early - 1.0).abs() < 0.35,
                "{model:?} did not hold its level: {early} then {late}"
            );
        } else {
            assert!(
                late < early * 0.1,
                "{model:?} did not decay: {early} then {late}"
            );
        }
    }
}

#[test]
fn every_model_stays_bounded_at_the_ends_of_every_control() {
    for model in Model::ALL {
        for top in [false, true] {
            let mut rig = whole(model);
            let ends: Vec<_> = rig
                .instrument
                .parameters()
                .iter()
                .filter(|p| p.key != "level")
                .map(|p| (p.id, if top { p.max } else { p.min }))
                .collect();
            for (id, value) in ends {
                rig.instrument.set_param(id, value);
            }
            let events: Vec<_> = (0..=127).step_by(7).map(|pitch| on(pitch, 1.0)).collect();
            let samples = rig.render(48_000, &events);
            assert!(
                samples.iter().all(|s| s.is_finite()),
                "{model:?} went non-finite"
            );
            assert!(peak(&samples) < 16.0, "{model:?}: {}", peak(&samples));
            // A long release is still a release: every voice ends in the end.
            rig.set_param("release", 0.2);
            let released = rig.render(288_000, &[NoteEvent::AllNotesOff { frame: 0 }]);
            assert!(
                peak(&released[280_000..]) < 1.0e-3,
                "{model:?} never came to rest: {}",
                peak(&released[280_000..])
            );
        }
    }
}

#[test]
fn all_sound_off_is_silence_within_a_few_milliseconds() {
    for model in Model::ALL {
        let mut rig = rig(model, 48_000.0);
        // The sympathetic strings of a pedalled piano are not voices; the test is of voices.
        rig.instrument.set_param_by_key("sympathetic", 0.0);
        rig.render(9_600, &[on(home(model), 0.9), controller(0, 64, 1.0)]);
        let killed = rig.render(9_600, &[NoteEvent::AllSoundOff { frame: 0 }]);
        assert!(
            peak(&killed[240..]) < 2.0e-3,
            "{model:?} still sounding: {}",
            peak(&killed[240..])
        );
        rig.render(48_000, &[]);
        assert_eq!(rig.instrument.active_voices(), 0, "{model:?}");
    }
}

#[test]
fn nothing_on_the_audio_thread_allocates() {
    for model in Model::ALL {
        let mut instrument = Physical::new(model);
        instrument.prepare(&PrepareContext::new(48_000.0, 256, 2));
        let monitor = instrument.motion_monitor().unwrap();
        monitor.watch(true);
        let mut buffer = AudioBuffer::stereo(256, 48_000.0);
        let ctx = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
        let controls: Vec<(ParamId, f32)> = instrument
            .parameters()
            .iter()
            .map(|p| (p.id, p.default))
            .collect();
        let quality = instrument.descriptor_for_key("quality").unwrap().id;
        let allocations = count_allocations(|| {
            for pitch in 30..100_u8 {
                // Every control is moved, every quality struck, and the pool overrun.
                let (id, value) = controls[usize::from(pitch) % controls.len()];
                instrument.set_param(id, value);
                instrument.set_param(quality, f32::from(pitch % 3));
                instrument.process(
                    &[
                        controller(0, 20, 0.3),
                        on(pitch, 0.8),
                        controller(40, 64, f32::from(pitch % 2)),
                        NoteEvent::PitchBend {
                            frame: 90,
                            semitones: 0.2,
                        },
                        off(200, pitch - 2),
                    ],
                    &mut buffer,
                    &ctx,
                );
            }
            instrument.process(&[NoteEvent::AllSoundOff { frame: 50 }], &mut buffer, &ctx);
            instrument.reset();
        });
        assert_eq!(allocations, 0, "{model:?} allocated on the audio thread");
    }
}

#[test]
fn watching_a_body_changes_nothing_that_is_heard() {
    for model in Model::ALL {
        let mut watched = whole(model);
        let mut plain = whole(model);
        let monitor = watched.instrument.motion_monitor().unwrap();
        monitor.watch(true);
        let events = [
            on(home(model), 0.75),
            NoteEvent::PitchBend {
                frame: 150,
                semitones: 0.25,
            },
        ];
        assert!(watched.render(4_800, &events) == plain.render(4_800, &events));
        let frame = monitor.read().unwrap();
        assert_eq!(frame.active, 1);
        assert_eq!(frame.geometry, model.geometry());
        assert_eq!(frame.voices[0].pitch, f32::from(home(model)) + 0.25);
        assert!(frame.voices[0].level > 0.0 && frame.voices[0].held);
        assert!(frame.voices[0].points.iter().all(|v| v.is_finite()));
        assert!(
            frame.voices[0].points.iter().any(|v| v.abs() > 1.0e-6),
            "{model:?} has no observed motion"
        );
        if frame.geometry == MotionGeometry::String {
            assert_eq!(frame.voices[0].points[0], 0.0);
            assert_eq!(frame.voices[0].points[63], 0.0);
        }
        watched.instrument.reset();
        assert_eq!(monitor.read().unwrap().active, 0);
    }
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
        let (live, render) = (find("quality"), find("render_quality"));
        assert_eq!(live.choices.len(), 3);
        assert_eq!(Quality::from_value(live.default), Quality::Standard);
        assert_eq!(Quality::from_value(render.default), Quality::High);
        assert_eq!(render.id.0, live.id.0 + 1, "{model:?}");
        assert_eq!(instrument.quality(), Quality::Standard);
        for (index, descriptor) in instrument.parameters().iter().enumerate() {
            assert_eq!(
                descriptor.id.index(),
                index,
                "{model:?} `{}`",
                descriptor.key
            );
        }
        assert_eq!(Model::from_id(model.id()), Some(model));
    }
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
fn an_offline_block_plays_at_the_render_quality_and_a_live_one_at_the_live_quality() {
    for model in [Model::Piano, Model::Guitar, Model::Violin, Model::Clarinet] {
        let mut split = bare(model);
        split.set_param_by_key("quality", Quality::Draft.value());
        split.set_param_by_key("render_quality", Quality::High.value());
        let mut high = bare(model);
        high.set_param_by_key("quality", Quality::High.value());
        let mut draft = bare(model);
        draft.set_param_by_key("quality", Quality::Draft.value());
        let pitch = if model == Model::Clarinet { 57 } else { 33 };
        let offline = render_note(&mut split, pitch, 12_000, true);
        let live = render_note(&mut split, pitch, 12_000, false);
        assert!(
            offline == render_note(&mut high, pitch, 12_000, false),
            "{model:?}"
        );
        assert!(
            live == render_note(&mut draft, pitch, 12_000, false),
            "{model:?}"
        );
        assert!(
            offline != live,
            "{model:?} sounds the same at every quality"
        );
    }
}

#[test]
fn draft_keeps_the_note_and_gives_up_only_its_top() {
    for (model, pitch, ceiling) in [
        (Model::Piano, 33, -20.0),
        (Model::Piano, 60, -35.0),
        (Model::Guitar, 40, -20.0),
    ] {
        let render = |quality: Quality| {
            let mut instrument = bare(model);
            instrument.set_param_by_key("quality", quality.value());
            Rig::new(Box::new(instrument), 48_000.0, 256, 2).render(24_000, &[on(pitch, 0.9)])
        };
        let (standard, draft) = (render(Quality::Standard), render(Quality::Draft));
        let difference: Vec<f32> = draft.iter().zip(&standard).map(|(d, s)| d - s).collect();
        let error = 20.0 * f64::from(rms(&difference) / rms(&standard)).log10();
        assert!(
            error < ceiling,
            "{model:?} MIDI {pitch}: draft differs by {error:.1} dB"
        );
        assert!(
            error.is_finite(),
            "{model:?}: draft is not cheaper than standard"
        );
    }
}

// --- Piano -------------------------------------------------------------------------------

#[test]
fn a_piano_is_tuned_like_a_piano() {
    // Out from A4, each octave a little wide: the bass flat and the treble sharp.
    let rate = 48_000.0;
    let fundamental = |pitch: u8| {
        let mut piano = rig(Model::Piano, rate);
        let audio = piano.render(72_000, &[on(pitch, 0.7)]);
        peak_cents(&audio[12_000..], rate, hz(pitch), -500..=500)
    };
    assert!(
        fundamental(69).abs() < 0.6,
        "A4 is {} cents out",
        fundamental(69)
    );
    let bass = fundamental(24);
    let treble = fundamental(96);
    assert!(
        (-30.0..-6.0).contains(&bass),
        "C1 is {bass} cents from equal"
    );
    assert!(
        (6.0..30.0).contains(&treble),
        "C7 is {treble} cents from equal"
    );
    // Without stiffness every octave is pure and the piano is in equal temperament.
    let mut harmonic = bare(Model::Piano);
    harmonic.set_param_by_key("stiffness", 0.0);
    let mut harmonic = Rig::new(Box::new(harmonic), rate, 256, 2);
    for pitch in [24, 96] {
        harmonic.instrument.reset();
        let audio = harmonic.render(48_000, &[on(pitch, 0.7)]);
        let cents = peak_cents(&audio[12_000..], rate, hz(pitch), -300..=300);
        assert!(cents.abs() < 1.5, "MIDI {pitch} without stiffness: {cents}");
    }
}

#[test]
fn piano_octaves_lie_between_a_pure_two_to_one_and_a_pure_four_to_two() {
    let rate = 48_000.0;
    let steady = |pitch: u8| {
        let mut piano = rig(Model::Piano, rate);
        let audio = piano.render(96_000, &[on(pitch, 0.7)]);
        audio[14_400..].to_vec()
    };
    // Each octave has a width of its own, from the tenor to the top: shared out among the
    // keys under it, the octave on C4 comes out wider than both.
    for low in [48_u8, 60, 72, 84] {
        let (lower, upper) = (steady(low), steady(low + 12));
        let at = |audio: &[f32], at: f64| peak_cents(audio, rate, at, -600..=600);
        // How far the lower note's partial is above the upper note's that it should meet.
        let two_one = at(&lower, hz(low) * 2.0) - at(&upper, hz(low) * 2.0);
        let four_two = at(&lower, hz(low) * 4.0) - at(&upper, hz(low) * 4.0);
        // Each unison is left a cent or so out by its tuner, and what rings on is its strings
        // apart: a partial's peak may stand that far from where the octave was set.
        assert!(
            (-6.0..1.5).contains(&two_one) && (-1.5..6.0).contains(&four_two),
            "MIDI {low}: 2:1 {two_one:+.1} cents, 4:2 {four_two:+.1} cents"
        );
    }
}

#[test]
fn piano_bass_octaves_lie_between_a_pure_six_to_three_and_a_pure_eight_to_four() {
    // From C2 down an octave lies between its pure 6:3 and its pure 8:4, by a width of its own:
    // shared out among its twelve keys, the octave on C2 would be half as wide, and narrower
    // than a pure 6:3 by more than this allows.
    let rate = 48_000.0;
    let steady = |pitch: u8| {
        let mut piano = rig(Model::Piano, rate);
        let audio = piano.render(144_000, &[on(pitch, 0.7)]);
        audio[14_400..].to_vec()
    };
    for low in [21_u8, 24, 33, 36] {
        let (lower, upper) = (steady(low), steady(low + 12));
        let at = |audio: &[f32], at: f64| peak_cents(audio, rate, at, -400..=400);
        // How far the lower note's partial is above the upper note's that it should meet.
        let six_three = at(&lower, hz(low) * 6.0) - at(&upper, hz(low) * 6.0);
        let eight_four = at(&lower, hz(low) * 8.0) - at(&upper, hz(low) * 8.0);
        assert!(
            (-6.0..0.5).contains(&six_three) && (-0.5..6.0).contains(&eight_four),
            "MIDI {low}: 6:3 {six_three:+.1} cents, 8:4 {eight_four:+.1} cents"
        );
    }
}

#[test]
fn a_stiff_string_runs_sharp_of_its_harmonics() {
    // Middle C's seventh partial is a sixth of a semitone above seven times its first.
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("position", 0.1);
    let audio = piano.render(48_000, &[on(60, 0.9)]);
    let first = peak_cents(&audio, 48_000.0, hz(60), -300..=300);
    let seventh = peak_cents(&audio, 48_000.0, hz(60) * 7.0, -300..=900);
    let stretch = seventh - first;
    assert!(
        (10.0..24.0).contains(&stretch),
        "stretched {stretch:.1} cents"
    );
    // In the bass, where the strings are wound, far less than the plain strings' rule gives.
    piano.instrument.reset();
    let audio = piano.render(48_000, &[on(36, 0.9)]);
    let first = peak_cents(&audio, 48_000.0, hz(36), -300..=300);
    let seventh = peak_cents(&audio, 48_000.0, hz(36) * 7.0, -300..=900);
    assert!(
        (2.0..12.0).contains(&(seventh - first)),
        "C2: {:.1}",
        seventh - first
    );
}

#[test]
fn a_piano_note_falls_fast_and_then_slow_and_longest_in_the_bass() {
    let mut piano = rig(Model::Piano, 48_000.0);
    let middle = piano.render(8 * 48_000, &[on(60, 0.65)]);
    let at = |audio: &[f32], seconds: f64| {
        let start = (seconds * 48_000.0) as usize;
        level_db(&audio[start..start + 4_800])
    };
    // The bridge drains what the strings share first; what they do not share rings on.
    let prompt = (at(&middle, 0.1) - at(&middle, 0.6)) / 0.5;
    let after = (at(&middle, 4.0) - at(&middle, 7.0)) / 3.0;
    assert!(
        prompt > 2.5 * after,
        "prompt {prompt} dB/s, aftersound {after} dB/s"
    );
    // The recorded Steinway's middle C loses 31 dB a second over the same half second.
    assert!((20.0..40.0).contains(&prompt), "prompt sound {prompt} dB/s");
    assert!(after > 0.5, "the aftersound must still decay: {after} dB/s");
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
fn a_harder_blow_is_brighter_as_well_as_louder() {
    let mut piano = rig(Model::Piano, 48_000.0);
    let mut take = |velocity| {
        piano.instrument.reset();
        piano.render(24_000, &[on(60, velocity)])
    };
    let (soft, middle, loud) = (take(0.35), take(0.65), take(0.95));
    // The partials between two and three kilohertz, where a piano's brightness is heard.
    let brightness = |audio: &[f32]| {
        let window = &audio[480..9_600];
        let upper: f64 = (7..=12)
            .map(|n| partial(window, hz(60), f64::from(n)))
            .sum();
        20.0 * (upper / partial(window, hz(60), 1.0)).log10()
    };
    assert!(
        brightness(&loud) > brightness(&soft) + 8.0,
        "soft {} dB, loud {} dB",
        brightness(&soft),
        brightness(&loud)
    );
    // And the level follows the recorded grand's: its pianissimo lies 13 dB under its
    // mezzo-forte and its fortissimo 11 dB over.
    let quieter = level_db(&soft) - level_db(&middle);
    let louder = level_db(&loud) - level_db(&middle);
    assert!((-15.0..-9.0).contains(&quieter), "pp {quieter} dB");
    assert!((8.0..13.0).contains(&louder), "ff {louder} dB");
}

#[test]
fn felt_hardness_voices_the_whole_piano() {
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("hardness", 0.1);
    let soft = piano.render(9_600, &[on(60, 0.8)]);
    piano.instrument.reset();
    piano.set_param("hardness", 0.9);
    let hard = piano.render(9_600, &[on(60, 0.8)]);
    // One partial can sit on a notch of either blow's spectrum; the upper ones together do
    // not.
    let brightness = |audio: &[f32]| {
        let upper: f64 = (7..=12).map(|n| partial(audio, hz(60), f64::from(n))).sum();
        20.0 * (upper / partial(audio, hz(60), 1.0)).log10()
    };
    let (soft, hard) = (brightness(&soft), brightness(&hard));
    assert!(
        hard > soft + 5.0,
        "soft felt {soft} dB, hard felt {hard} dB"
    );
}

#[test]
fn dampers_stop_a_released_key_and_the_top_of_the_piano_has_none() {
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("sympathetic", 0.0);
    let damped = piano.render(48_000, &[on(72, 0.65), off(4_800, 72)]);
    piano.instrument.reset();
    let free = piano.render(48_000, &[on(96, 0.65), off(4_800, 96)]);
    let fall = |audio: &[f32]| level_db(&audio[..4_800]) - level_db(&audio[19_200..24_000]);
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
fn the_pedal_holds_released_keys_and_catches_one_already_fading() {
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("sympathetic", 0.0);
    piano.set_param("release", 0.05);
    piano.render(
        4_000,
        &[on(60, 0.8), controller(100, 64, 1.0), off(2_000, 60)],
    );
    assert_eq!(piano.instrument.active_voices(), 1);
    let held = piano.render(36_000, &[]);
    assert!(rms(&held[32_000..]) > 1.0e-4, "the pedal let the note go");
    let released = piano.render(9_600, &[controller(0, 64, 0.0)]);
    assert!(peak(&released[6_000..]) < 1.0e-4);
    assert_eq!(piano.instrument.active_voices(), 0);

    piano.instrument.reset();
    piano.set_param("release", 0.5);
    let events = [on(60, 0.65), off(4_800, 60)];
    let stopped = piano.render(48_000, &events);
    piano.instrument.reset();
    let caught = piano.render(48_000, &[events[0], events[1], controller(7_200, 64, 1.0)]);
    assert!(stopped[..7_200] == caught[..7_200]);
    let tail = 38_400..48_000;
    assert!(
        level_db(&caught[tail.clone()]) > level_db(&stopped[tail]) + 20.0,
        "the pedal did not lift the damper"
    );
}

#[test]
fn free_strings_answer_a_struck_one() {
    // The demonstration every piano teacher gives: take C3 without sounding it, strike the
    // C above and let it go. Its damper stops it, and what goes on ringing is C3's strings,
    // at the pitch of the note that woke them.
    let residue = |silent: bool, amount: f32| {
        let mut piano = whole(Model::Piano);
        piano.set_param("sympathetic", amount);
        let mut events = vec![on(60, 0.9), off(12_000, 60)];
        if silent {
            events.insert(0, on(48, 0.005));
        }
        let audio = piano.render(96_000, &events);
        let struck = partial(&audio[..12_000], hz(60), 1.0);
        let left = partial(&audio[48_000..72_000], hz(60), 1.0);
        (
            20.0 * (left / struck).max(1e-12).log10(),
            piano.instrument.active_voices(),
        )
    };
    let (answered, voices) = residue(true, 0.3);
    assert_eq!(voices, 0, "a key taken silently is not a note");
    assert!(
        (-45.0..-22.0).contains(&answered),
        "C3 answers middle C {answered} dB under it"
    );
    let (unheld, _) = residue(false, 0.3);
    assert!(
        unheld < answered - 40.0,
        "with C3's damper down, {unheld} dB"
    );
    let (without, _) = residue(true, 0.0);
    assert!(without < answered - 40.0, "with no resonance, {without} dB");
    // And the pedal lifts every damper at once: a staccato note under it is fuller a second
    // later than its own strings alone would leave it.
    let pedalled = |amount: f32| {
        let mut piano = whole(Model::Piano);
        piano.set_param("sympathetic", amount);
        let audio = piano.render(
            72_000,
            &[controller(0, 64, 1.0), on(60, 0.9), off(12_000, 60)],
        );
        level_db(&audio[48_000..])
    };
    let lift = pedalled(0.3) - pedalled(0.0);
    assert!(
        (1.0..6.0).contains(&lift),
        "the pedal's wash adds {lift} dB"
    );
}

#[test]
fn the_blow_knocks_the_soundboard_under_a_treble_note() {
    let mut piano = whole(Model::Piano);
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
    let knock = level_db(&low[..2_880]);
    assert!(
        knock > level_db(&audio[..2_880]) - 34.0,
        "knock {knock} dB under a note at {} dB",
        level_db(&audio[..2_880])
    );
    let later = level_db(&low[24_000..26_880]);
    assert!(
        knock > later + 25.0,
        "knock {knock} dB, half a second later {later} dB"
    );
}

#[test]
fn a_honky_tonk_beats_and_a_tuned_piano_does_not() {
    // The envelope of the fundamental: steady when the unison is in tune, beating when not.
    let wobble = |detune: f32| {
        let mut piano = rig(Model::Piano, 48_000.0);
        piano.set_param("detune", detune);
        let audio = piano.render(96_000, &[on(69, 0.7)]);
        let levels: Vec<f64> = (4..36)
            .map(|window| {
                let start = window * 2_400;
                goertzel(&audio[start..start + 2_400], 48_000.0, 440.0)
            })
            .collect();
        // Depth of the ripple left once the decay itself is taken out: each window against
        // the two a fifth of a second either side of it, which is half a beat of strings
        // nine cents apart.
        let mut ripple: f64 = 0.0;
        for nine in levels.windows(9) {
            let smooth = (nine[0] * nine[8]).sqrt();
            ripple = ripple.max((20.0 * (nine[4] / smooth).log10()).abs());
        }
        ripple
    };
    let (tuned, loose) = (wobble(0.26), wobble(9.0));
    assert!(tuned < 1.5, "a tuned unison ripples by {tuned} dB");
    assert!(loose > 4.0, "nine cents apart ripples only {loose} dB");
}

// --- Hammered dulcimer -------------------------------------------------------------------

#[test]
fn a_dulcimer_course_rings_until_it_is_damped() {
    let mut dulcimer = rig(Model::HammeredDulcimer, 48_000.0);
    let ringing = dulcimer.render(48_000, &[on(62, 0.8)]);
    assert!(level_db(&ringing[38_400..]) > level_db(&ringing[4_800..14_400]) - 30.0);
    dulcimer.instrument.reset();
    let damped = dulcimer.render(48_000, &[on(62, 0.8), off(9_600, 62)]);
    assert!(level_db(&damped[24_000..]) < level_db(&ringing[24_000..]) - 30.0);
}

// --- Guitars and bass --------------------------------------------------------------------

#[test]
fn a_plucked_fundamental_decays_in_the_time_the_decay_control_says() {
    for rate in [44_100.0, 96_000.0] {
        for pitch in [45, 57, 69, 81] {
            let mut guitar = rig(Model::Guitar, rate);
            guitar.set_param("decay", 2.0);
            guitar.set_param("damping", 0.2);
            let audio = guitar.render(rate as usize, &[on(pitch, 0.5)]);
            let span = (rate * 0.2) as usize;
            let window = |from: f64| {
                let start = (rate * from) as usize;
                let cents = peak_cents(&audio[start..start + span], rate, hz(pitch), -60..=60);
                goertzel(
                    &audio[start..start + span],
                    rate,
                    hz(pitch) * (cents / 1200.0).exp2(),
                )
            };
            let loss_db = 20.0 * (window(0.05) / window(0.55)).log10();
            // Sixty decibels in the decay time at G3, shortened by damping and by how many
            // times a second a higher note meets its terminations.
            let register = (hz(pitch) / hz(55)).powf(0.45);
            let expected = 60.0 * 0.5 * (1.0 + 3.0 * 0.2) / 2.0 * register;
            assert!(
                (loss_db - expected).abs() < 1.5,
                "{pitch} at {rate}: {loss_db} dB, expected {expected}"
            );
        }
    }
}

#[test]
fn nylon_loses_its_top_long_before_steel() {
    let top_left = |damping: f32| {
        let mut guitar = rig(Model::Guitar, 48_000.0);
        guitar.set_param("damping", damping);
        guitar.set_param("hardness", 1.0);
        guitar.set_param("position", 0.13);
        // Hold the fundamental's decay to the same time for both.
        guitar.set_param("decay", 6.0 * (1.0 + 3.0 * damping));
        let audio = guitar.render(48_000, &[on(55, 0.8)]);
        let band = |window: &[f32]| -> f64 {
            (16..=24)
                .map(|n| partial(window, hz(55), f64::from(n)))
                .sum()
        };
        20.0 * (band(&audio[2_400..12_000]) / band(&audio[24_000..33_600])).log10()
    };
    let (steel, nylon) = (top_left(0.0), top_left(0.4));
    assert!(
        steel < 12.0,
        "steel lost {steel} dB of its top in half a second"
    );
    assert!(nylon > steel + 8.0, "nylon {nylon} dB, steel {steel} dB");
}

#[test]
fn where_and_how_a_string_is_plucked_is_heard_without_moving_its_pitch() {
    let mut guitar = rig(Model::Guitar, 48_000.0);
    guitar.set_param("position", 0.2);
    guitar.set_param("hardness", 0.0);
    // The open top string, which is as long as the control's fraction measures, plucked
    // softly: a hard pluck snaps the first string off a thumb nearly as fast as off a pick.
    let soft = guitar.render(8_000, &[controller(0, 20, 0.2), on(64, 0.3)]);
    guitar.instrument.reset();
    guitar.set_param("hardness", 1.0);
    let hard = guitar.render(8_000, &[controller(0, 20, 0.2), on(64, 0.3)]);
    let at = |audio: &[f32], n: f64| partial(audio, hz(64), n);
    assert!(
        at(&hard, 9.0) > at(&soft, 9.0) * 4.0,
        "a pick should be brighter than a thumb"
    );
    // Plucked at a fifth of its length, a string has no fifth harmonic to speak of.
    assert!(at(&hard, 5.0) < at(&hard, 4.0) * 0.2);
    guitar.instrument.reset();
    let third = guitar.render(8_000, &[controller(0, 20, 1.0 / 3.0), on(64, 0.8)]);
    assert!(at(&third, 3.0) < at(&third, 2.0) * 0.2);
    for audio in [&soft, &hard, &third] {
        let cents = peak_cents(&audio[2_400..], 48_000.0, hz(64), -200..=200);
        assert!(cents.abs() < 6.0, "the pluck moved the note {cents} cents");
    }
}

#[test]
fn a_fast_string_is_let_go_sooner_and_sooner_still_when_it_is_plucked_hard() {
    // How far an open string's twelfth partial lies under its fundamental at the bridge, in
    // decibels, over the note's first tenth of a second.
    let top = |model: Model, key: u8, velocity: f32| {
        let mut guitar = rig(model, 48_000.0);
        if model == Model::Guitar {
            guitar.set_param("body", 0.0);
        } else {
            // The coil and the cable out of the way: it is the string that is asked about.
            guitar.set_param("tone", 12_000.0);
        }
        let audio = guitar.render(4_800, &[on(key, velocity)]);
        20.0 * (partial(&audio, hz(key), 12.0) / partial(&audio, hz(key), 1.0)).log10()
    };
    // The first string's wave runs four times as fast as the sixth's. A hard pluck opens it
    // by much more than it opens the sixth, whose corner moves only as the finger's does.
    let first = top(Model::Guitar, 64, 0.95) - top(Model::Guitar, 64, 0.35);
    let sixth = top(Model::Guitar, 40, 0.95) - top(Model::Guitar, 40, 0.35);
    assert!(first > 12.0, "the first string opened by {first} dB");
    assert!(sixth.abs() < 4.0, "the sixth string opened by {sixth} dB");
    // Played softly the two are let go nearly alike, a sixth string's twelfth partial being
    // two octaves under a first's.
    let soft = top(Model::Guitar, 64, 0.1) - top(Model::Guitar, 40, 0.1);
    assert!(
        soft < -12.0,
        "a soft first string kept {soft} dB more of its top"
    );
    // An electric guitar's first string is let go sooner than its sixth as well, by the same
    // amount however hard it is picked: its recordings are at one dynamic. A hard stroke
    // opens it only as far as a harder pick opens any string.
    let electric = top(Model::ElectricGuitar, 64, 0.95) - top(Model::ElectricGuitar, 64, 0.35);
    assert!(
        electric < 8.0 && electric < first * 0.5,
        "the electric's first string opened by {electric} dB, the guitar's by {first}"
    );
    // With one corner for both, the first string's twelfth partial, two octaves over the
    // sixth's, would lie some 17 dB further under its fundamental.
    let strings = top(Model::ElectricGuitar, 64, 0.8) - top(Model::ElectricGuitar, 40, 0.8);
    assert!(
        strings > -4.0,
        "an electric's first string kept {strings} dB of its top against its sixth"
    );
}

#[test]
fn a_bass_string_rings_for_long_and_a_stopped_one_for_less() {
    // Decibels a second a note's fundamental loses once the pluck has passed.
    let falls = |pitch: u8| {
        let audio = rig(Model::Bass, 48_000.0).render(120_000, &[on(pitch, 0.8)]);
        let at = |from: usize| 20.0 * partial(&audio[from..from + 24_000], hz(pitch), 1.0).log10();
        (at(14_400) - at(86_400)) / 1.5
    };
    // The recorded basses' open strings lose about 2 dB a second, the low ones as the high.
    for open in [28_u8, 33, 38, 43] {
        let fall = falls(open);
        assert!(
            (1.6..3.2).contains(&fall),
            "open {open}: {fall} dB a second"
        );
    }
    // An octave up the top string the same string sounds at half its length, under a finger.
    let (open, octave) = (falls(43), falls(55));
    assert!(
        octave > open * 1.6 && octave < open * 2.6,
        "open {open} dB a second, an octave up it {octave}"
    );
}

/// How far each key's first third of a second is from the keys around it, in decibels.
fn apart_from_neighbours(model: Model, keys: std::ops::RangeInclusive<u8>) -> Vec<(u8, f64)> {
    let levels: Vec<f64> = keys
        .clone()
        .map(|key| level_db(&whole(model).render(16_800, &[on(key, 0.8)])[960..]))
        .collect();
    keys.zip(0_usize..)
        .map(|(key, index)| {
            let mut near: Vec<f64> = (index.saturating_sub(3)..(index + 4).min(levels.len()))
                .filter(|other| *other != index)
                .map(|other| levels[other])
                .collect();
            near.sort_by(f64::total_cmp);
            let median = (near[(near.len() - 1) / 2] + near[near.len() / 2]) / 2.0;
            (key, levels[index] - median)
        })
        .collect()
}

#[test]
fn no_key_of_a_piano_is_louder_or_quieter_than_the_keys_around_it() {
    for (key, apart) in apart_from_neighbours(Model::Piano, 28..=100) {
        assert!(
            apart.abs() < 3.0,
            "key {key} is {apart:+.1} dB from the keys around it"
        );
    }
}

#[test]
fn a_guitar_and_the_strings_are_evened_out_by_those_who_play_them() {
    // A box has resonances a soundboard has not, and a note on one would be twice as loud as
    // the next: the hand and the bow make up most of it.
    for (model, keys) in [(Model::Guitar, 40..=84), (Model::Violin, 43..=96)] {
        for (key, apart) in apart_from_neighbours(model, keys) {
            assert!(
                apart.abs() < 5.0,
                "{model:?}: key {key} is {apart:+.1} dB from the keys around it"
            );
        }
    }
}

#[test]
fn middle_c_is_mostly_its_fundamental_and_more_so_the_softer_it_is_struck() {
    let second = |velocity: f32| {
        let audio = whole(Model::Piano).render(9_600, &[on(60, velocity)]);
        let early = &audio[960..5_760];
        partial(early, hz(60), 2.0) / partial(early, hz(60), 1.0)
    };
    // On the recorded grand the second partial is some sixteen decibels under the first at
    // pianissimo and seven at fortissimo; a hammer as hard as the published ones, thrown at
    // three strings as if they were one, had it louder than the fundamental at both.
    let (soft, hard) = (second(0.35), second(0.95));
    assert!(soft < 0.5, "pianissimo: {soft}");
    assert!(hard < 0.95, "fortissimo: {hard}");
    assert!(hard > soft * 1.4, "{soft} to {hard}");
}

#[test]
fn a_blow_knocks_the_case_and_the_knock_is_gone_in_a_third_of_a_second() {
    let audio = whole(Model::Piano).render(24_000, &[on(96, 0.8)]);
    // From 300 Hz to 1.3 kHz, where C7 has nothing of its own.
    let knock = |from: usize| {
        (300..1_300)
            .step_by(25)
            .map(|hz| goertzel(&audio[from..from + 1_920], 48_000.0, f64::from(hz)).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let tone = partial(&audio[..1_920], hz(96), 1.0);
    // Under the top octaves' thin notes the knock is a good part of what is heard: on the
    // recorded grand, about half as loud as the note in these bands as it is struck.
    let share = knock(0) / tone;
    assert!((0.15..1.5).contains(&share), "knock {share} of the note");
    assert!(
        knock(16_800) < knock(0) * 0.1,
        "still knocking after a third of a second"
    );
    // The same blow under a bass note is nothing beside it.
    let low = whole(Model::Piano).render(9_600, &[on(36, 0.8)]);
    assert!(level_db(&low[..4_800]) > level_db(&audio[..4_800]) + 6.0);
}

#[test]
fn a_guitar_is_its_box_and_a_saddle_pickup_hears_only_the_bridge() {
    let take = |body: f32, pickup: f32| {
        let mut guitar = Physical::new(Model::Guitar);
        guitar.set_param_by_key("body", body);
        guitar.set_param_by_key("pickup", pickup);
        Rig::new(Box::new(guitar), 48_000.0, 256, 2).render(24_000, &[on(40, 0.8)])
    };
    let (acoustic, saddle) = (take(1.0, 0.0), take(1.0, 1.0));
    // The box cannot radiate E2's fundamental, 82 Hz, which is below its air resonance, while
    // the third partial, on a resonance of the top plate, comes through whole; the bridge
    // moves with both all the same.
    let through = |partial: f64| {
        let at = partial * hz(40);
        goertzel(&acoustic, 48_000.0, at) / goertzel(&saddle, 48_000.0, at)
    };
    assert!(
        through(1.0) < through(3.0) * 0.4,
        "{} against {}",
        through(1.0),
        through(3.0)
    );
    // And a note that sits on the air resonance is lifted by it: G#2 comes out of the box
    // far stronger, against what the saddle hears, than the E below it does.
    let (acoustic, saddle) = (
        Rig::new(
            {
                let mut guitar = Physical::new(Model::Guitar);
                guitar.set_param_by_key("pickup", 0.0);
                Box::new(guitar)
            },
            48_000.0,
            256,
            2,
        )
        .render(24_000, &[on(44, 0.8)]),
        Rig::new(
            {
                let mut guitar = Physical::new(Model::Guitar);
                guitar.set_param_by_key("pickup", 1.0);
                Box::new(guitar)
            },
            48_000.0,
            256,
            2,
        )
        .render(24_000, &[on(44, 0.8)]),
    );
    let on_the_air = goertzel(&acoustic, 48_000.0, hz(44)) / goertzel(&saddle, 48_000.0, hz(44));
    let below_it = {
        let (acoustic, saddle) = (take(1.0, 0.0), take(1.0, 1.0));
        goertzel(&acoustic, 48_000.0, hz(40)) / goertzel(&saddle, 48_000.0, hz(40))
    };
    assert!(
        on_the_air > below_it * 2.0,
        "{on_the_air} on the air mode, {below_it} below it"
    );
}

#[test]
fn a_plucked_string_follows_the_bend_and_comes_back_to_pitch_as_it_calms() {
    let mut guitar = rig(Model::Guitar, 48_000.0);
    guitar.render(12_000, &[on(57, 0.8)]);
    let bent = guitar.render(
        24_000,
        &[NoteEvent::PitchBend {
            frame: 0,
            semitones: 12.0,
        }],
    );
    assert!(peak(&bent) < 2.0);
    assert!(
        goertzel(&bent[2_400..], 48_000.0, 440.0) > goertzel(&bent[2_400..], 48_000.0, 220.0) * 5.0
    );
    // A hard pluck on an electric's low string starts sharp and settles.
    let mut electric = rig(Model::ElectricGuitar, 48_000.0);
    let audio = electric.render(96_000, &[on(40, 1.0)]);
    let early = peak_cents(&audio[480..7_200], 48_000.0, hz(40), -300..=300);
    let late = peak_cents(&audio[72_000..], 48_000.0, hz(40), -300..=300);
    assert!(early > late + 2.0, "{early} cents then {late}");
    assert!(late.abs() < 3.0, "settled {late} cents out");
}

#[test]
fn a_pickup_hears_a_comb_that_moves_with_it_and_not_with_the_pick() {
    let mut guitar = rig(Model::ElectricGuitar, 48_000.0);
    guitar.set_param("position", 0.14);
    guitar.set_param("pickup_position", 0.04);
    let bridge = guitar.render(24_000, &[on(64, 0.8)]);
    guitar.instrument.reset();
    guitar.set_param("pickup_position", 0.25);
    let neck = guitar.render(24_000, &[on(64, 0.8)]);
    // A quarter of the way up, a magnet sits on the fourth harmonic's node.
    let at = |audio: &[f32], n: f64| partial(&audio[2_400..], hz(64), n);
    assert!(at(&neck, 4.0) < at(&neck, 3.0) * 0.1);
    assert!(at(&bridge, 4.0) > at(&bridge, 3.0) * 0.4);
    for audio in [&bridge, &neck] {
        let cents = peak_cents(&audio[4_800..], 48_000.0, hz(64), -80..=80);
        assert!(
            cents.abs() <= 4.0,
            "pickup moved the fundamental by {cents} cents"
        );
    }
    assert!(guitar.instrument.param_by_key("body").is_none());
}

#[test]
fn an_upright_is_a_body_and_an_electric_bass_a_magnet() {
    let take = |body: f32| {
        let mut bass = Physical::new(Model::Bass);
        bass.set_param_by_key("body", body);
        Rig::new(Box::new(bass), 48_000.0, 256, 2).render_stereo(24_000, &[on(33, 0.8)])
    };
    let (electric, _) = take(0.0);
    let (upright_left, upright_right) = take(1.0);
    assert!(rms(&electric) > 1.0e-3 && rms(&upright_left) > 1.0e-3);
    // The body's resonances lean; a pickup's signal is one wire.
    let side: Vec<f32> = upright_left
        .iter()
        .zip(&upright_right)
        .map(|(l, r)| l - r)
        .collect();
    assert!(rms(&side) > rms(&upright_left) * 0.02);
    assert!(electric != upright_left);
}

// --- Bowed strings -----------------------------------------------------------------------

#[test]
fn a_bowed_string_moves_as_helmholtz_said_for_as_long_as_it_is_bowed() {
    for rate in [44_100.0, 96_000.0] {
        for velocity in [0.3, 0.85] {
            let mut violin = rig(Model::Violin, rate);
            let audio = violin.render((rate * 8.0) as usize, &[on(62, velocity)]);
            let early = &audio[rate as usize..(rate * 2.0) as usize];
            let late = &audio[(rate * 6.0) as usize..(rate * 7.0) as usize];
            assert!(audio.iter().all(|sample| sample.is_finite()));
            assert!(
                (level_db(late) - level_db(early)).abs() < 1.5,
                "level drifted {} dB at {rate}",
                level_db(late) - level_db(early)
            );
            let cents = peak_cents(late, rate, hz(62), -200..=200);
            assert!(
                cents.abs() < 8.0,
                "pitch {cents} cents at {rate}, velocity {velocity}"
            );
            // A sawtooth at the bridge: every harmonic, falling about as one over its number.
            let at = |n: f64| goertzel(late, rate, hz(62) * n * (cents / 1200.0).exp2());
            for (n, range) in [(2.0, 0.25..0.9), (3.0, 0.12..0.7), (4.0, 0.06..0.55)] {
                let ratio = at(n) / at(1.0);
                assert!(
                    range.contains(&ratio),
                    "harmonic {n} is {ratio} of the first"
                );
            }
        }
    }
}

#[test]
fn where_and_how_hard_a_string_is_bowed_is_heard() {
    // The open G string, so that the bow is where the control says along the sounding length.
    let take = |key: &str, value: f32| {
        let mut violin = rig(Model::Violin, 48_000.0);
        violin.set_param(key, value);
        violin.render(72_000, &[on(55, 0.7)])[36_000..].to_vec()
    };
    let at = |audio: &[f32], n: f64| goertzel(audio, 48_000.0, hz(55) * n);
    // A bow a fifth of the way along sits on the fifth harmonic's node and leaves it out;
    // nearer the bridge it is there, and the same bow stroke moves the bridge further.
    let (near, far) = (take("position", 0.07), take("position", 0.2));
    assert!(
        at(&far, 5.0) < at(&far, 4.0) * 0.35,
        "a fifth of the way along: {} against {}",
        at(&far, 5.0),
        at(&far, 4.0)
    );
    assert!(at(&near, 5.0) > at(&near, 4.0) * 0.5);
    assert!(level_db(&near) > level_db(&far) + 2.0);
    // Pressing harder sharpens the corner the bridge has rounded: the top of the spectrum
    // comes up and the bottom stays where it was.
    let top = |audio: &[f32]| {
        let upper: f64 = (14..=34).map(|n| at(audio, f64::from(n))).sum();
        20.0 * (upper / at(audio, 1.0)).log10()
    };
    let (light, heavy) = (take("bow_pressure", 0.1), take("bow_pressure", 0.9));
    assert!(
        top(&heavy) > top(&light) + 3.0,
        "light {} dB, heavy {} dB",
        top(&light),
        top(&heavy)
    );
    assert!((level_db(&heavy) - level_db(&light)).abs() < 2.0);
}

#[test]
fn expression_and_the_bend_wheel_reach_a_sounding_string() {
    let mut violin = rig(Model::Violin, 48_000.0);
    violin.render(24_000, &[on(69, 0.8)]);
    let bent = violin.render(
        24_000,
        &[NoteEvent::PitchBend {
            frame: 0,
            semitones: 12.0,
        }],
    );
    assert!(
        goertzel(&bent[12_000..], 48_000.0, 880.0)
            > goertzel(&bent[12_000..], 48_000.0, 440.0) * 5.0
    );
    let before = rms(&bent[12_000..]);
    let quiet = violin.render(24_000, &[controller(0, 11, 0.0)]);
    assert!(
        rms(&quiet[18_000..]) < before * 0.01,
        "the bow did not stop"
    );
}

#[test]
fn a_bow_that_does_not_move_makes_no_sound() {
    let mut violin = rig(Model::Violin, 48_000.0);
    let still = violin.render(24_000, &[controller(0, 11, 0.0), on(69, 0.8)]);
    assert!(peak(&still) < 1.0e-4, "{}", peak(&still));
    let moving = violin.render(24_000, &[controller(0, 11, 1.0)]);
    assert!(rms(&moving[12_000..]) > 1.0e-3);
}

#[test]
fn a_slur_keeps_the_string_and_falls_back_to_the_key_still_held() {
    let mut violin = rig(Model::Violin, 48_000.0);
    violin.set_param("legato", 1.0);
    violin.render(24_000, &[on(69, 0.8)]);
    let upper = violin.render(16_000, &[on(76, 0.8)]);
    assert_eq!(violin.instrument.active_voices(), 1);
    assert!(
        goertzel(&upper[8_000..], 48_000.0, hz(76))
            > goertzel(&upper[8_000..], 48_000.0, 440.0) * 3.0
    );
    // No new attack: the string never fell silent between the two notes.
    let quietest = upper.chunks(480).map(rms).fold(f32::INFINITY, f32::min);
    assert!(
        quietest > rms(&upper[8_000..]) * 0.3,
        "the slur broke: {quietest}"
    );
    let returned = violin.render(16_000, &[off(0, 76)]);
    assert!(
        goertzel(&returned[8_000..], 48_000.0, 440.0)
            > goertzel(&returned[8_000..], 48_000.0, hz(76)) * 3.0
    );
    let held = violin.render(8_000, &[off(0, 76)]);
    assert!(rms(&held) > 1.0e-3, "a stale off released the held key");
    violin.render(24_000, &[off(0, 69)]);
    assert_eq!(violin.instrument.active_voices(), 0);
    // Out of legato, two keys are two strings.
    violin.set_param("legato", 0.0);
    violin.render(8_000, &[on(69, 0.8), on(72, 0.8)]);
    assert_eq!(violin.instrument.active_voices(), 2);
}

#[test]
fn a_section_is_several_players_and_not_one_string_copied() {
    let ripple = |players: f32| {
        let mut strings = bare(Model::Violin);
        strings.set_param_by_key("players", players);
        let audio = Rig::new(Box::new(strings), 48_000.0, 256, 2).render(144_000, &[on(62, 0.7)]);
        let levels: Vec<f64> = audio[48_000..]
            .chunks(2_400)
            .map(|chunk| goertzel(chunk, 48_000.0, hz(62)))
            .collect();
        let (low, high) = levels
            .iter()
            .fold((f64::INFINITY, 0.0_f64), |(low, high), level| {
                (low.min(*level), high.max(*level))
            });
        20.0 * (high / low).log10()
    };
    let (solo, section) = (ripple(1.0), ripple(4.0));
    assert!(solo < 2.0, "one steady bow ripples {solo} dB");
    assert!(section > 3.0, "four players beat by only {section} dB");
}

#[test]
fn vibrato_waits_for_the_note_to_settle_and_the_wheel_deepens_it() {
    let swing = |vibrato: f32, wheel: f32| {
        let mut violin = bare(Model::Violin);
        violin.set_param_by_key("vibrato", vibrato);
        let mut rig = Rig::new(Box::new(violin), 48_000.0, 256, 2);
        let audio = rig.render(96_000, &[controller(0, 1, wheel), on(69, 0.7)]);
        let pitch = |from: usize| {
            peak_cents(
                &audio[from..from + 2_400],
                48_000.0,
                440.0 * 3.0,
                -900..=900,
            )
        };
        let early: Vec<f64> = (2..6).map(|n| pitch(n * 1_200)).collect();
        let late: Vec<f64> = (0..16).map(|n| pitch(48_000 + n * 1_200)).collect();
        let spread = |values: &[f64]| {
            values.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b))
                - values.iter().fold(f64::INFINITY, |a, b| a.min(*b))
        };
        (spread(&early), spread(&late))
    };
    let (attack, settled) = swing(0.5, 0.0);
    let (bare_attack, none) = swing(0.0, 0.0);
    // The bow pulls a note a little flat as it catches and the finger brings it back, with
    // or without vibrato; what must not be there yet is the vibrato.
    assert!(
        attack < bare_attack + 6.0,
        "vibrato on the attack: {attack} cents against {bare_attack}"
    );
    assert!(settled > 20.0, "no vibrato once settled: {settled} cents");
    let (_, wheel) = swing(0.0, 1.0);
    assert!(wheel > 50.0, "the wheel opened only {wheel} cents");
    assert!(none < 6.0, "a steady hand wandered {none} cents");
}

// --- Bars and bells ----------------------------------------------------------------------

/// The strongest partial of `audio` between `low` and `high` times `fundamental`.
fn strongest_ratio(audio: &[f32], fundamental: f64, low: f64, high: f64) -> f64 {
    let steps = ((high - low) * 200.0) as i32;
    (0..=steps)
        .map(|step| low + f64::from(step) / 200.0)
        .max_by(|a, b| {
            goertzel(audio, 48_000.0, fundamental * a).total_cmp(&goertzel(
                audio,
                48_000.0,
                fundamental * b,
            ))
        })
        .unwrap()
}

#[test]
fn each_bar_is_tuned_the_way_its_maker_tunes_it() {
    for (bar, overtone) in [(0.0, 4.0), (1.0, 4.0), (2.0, 3.0), (3.0, 2.756)] {
        let mut mallet = rig(Model::Mallet, 48_000.0);
        mallet.set_param("bar", bar);
        mallet.set_param("hardness", 1.0);
        mallet.set_param("position", 0.3);
        let audio = mallet.render(12_000, &[on(60, 1.0)]);
        let found = strongest_ratio(&audio, hz(60), 1.6, 4.6);
        assert!(
            (found - overtone).abs() < 0.03,
            "bar {bar}: first overtone at {found}x"
        );
    }
}

#[test]
fn a_marimba_bar_decays_as_measured_and_aluminium_rings_far_longer() {
    let rate = |bar: f32, pitch: u8| {
        // With its tubes as built: a tube takes the bar's sound out of it faster.
        let mut mallet = rig(Model::Mallet, 48_000.0);
        mallet.set_param("bar", bar);
        mallet.set_param("body", 0.7);
        let audio = mallet.render(48_000, &[on(pitch, 0.8)]);
        let at = |from: usize| goertzel(&audio[from..from + 4_800], 48_000.0, hz(pitch));
        20.0 * (at(2_400) / at(21_600)).log10() / 0.4
    };
    // Twenty-five decibels a second at C3 and fifty-five at B4, on the Iowa marimba.
    let (low, high) = (rate(0.0, 48), rate(0.0, 71));
    assert!((21.0..30.0).contains(&low), "C3 falls {low} dB/s");
    assert!((48.0..64.0).contains(&high), "B4 falls {high} dB/s");
    assert!(rate(1.0, 60) < rate(0.0, 60) / 3.0);
    assert!(rate(2.0, 60) > rate(0.0, 60) * 1.5);
}

#[test]
fn a_harder_mallet_is_brighter_and_the_middle_of_a_bar_is_purest() {
    let overtone = |hardness: f32, velocity: f32, position: f32| {
        let mut mallet = rig(Model::Mallet, 48_000.0);
        mallet.set_param("hardness", hardness);
        mallet.set_param("position", position);
        let audio = mallet.render(4_800, &[on(48, velocity)]);
        20.0 * (goertzel(&audio, 48_000.0, hz(48) * 4.0) / goertzel(&audio, 48_000.0, hz(48)))
            .log10()
    };
    assert!(overtone(0.9, 0.8, 0.3) > overtone(0.1, 0.8, 0.3) + 10.0);
    assert!(overtone(0.4, 1.0, 0.3) > overtone(0.4, 0.2, 0.3) + 1.5);
    // The first overtone has a node in the middle of the bar.
    assert!(overtone(0.9, 0.8, 0.5) < overtone(0.9, 0.8, 0.3) - 20.0);
}

#[test]
fn a_vibraphones_discs_open_and_shut_its_tubes_and_its_pedal_lets_it_ring() {
    let mut vibes = bare(Model::Mallet);
    vibes.set_param_by_key("bar", 1.0);
    vibes.set_param_by_key("body", 1.0);
    vibes.set_param_by_key("tremolo", 1.0);
    vibes.set_param_by_key("tremolo_rate", 5.0);
    let mut rig = Rig::new(Box::new(vibes), 48_000.0, 256, 2);
    let audio = rig.render(48_000, &[on(60, 0.8)]);
    // Five swells a second: the level a tenth of a second apart is far from steady.
    let levels: Vec<f64> = audio[4_800..].chunks(960).map(level_db).collect();
    let swing = levels
        .windows(6)
        .map(|w| (w[5] - w[0]).abs())
        .fold(0.0_f64, f64::max);
    assert!(swing > 3.0, "the tremolo swings {swing} dB");
    rig.instrument.reset();
    rig.set_param("tremolo", 0.0);
    rig.set_param("release", 0.05);
    let damped = rig.render(24_000, &[on(60, 0.8), off(2_400, 60)]);
    rig.instrument.reset();
    let pedalled = rig.render(
        24_000,
        &[controller(0, 64, 1.0), on(60, 0.8), off(2_400, 60)],
    );
    assert!(level_db(&pedalled[12_000..]) > level_db(&damped[12_000..]) + 30.0);
    let lifted = rig.render(24_000, &[controller(0, 64, 0.0)]);
    assert!(level_db(&lifted[12_000..]) < level_db(&pedalled[12_000..]) - 30.0);
}

#[test]
fn a_cast_bell_hums_an_octave_under_its_note_and_a_chime_has_no_partial_on_it() {
    let share = |bell: f32, ratio: f64| {
        let mut rig = rig(Model::Bell, 48_000.0);
        rig.set_param("bell", bell);
        rig.set_param("warble", 0.0);
        let audio = rig.render(24_000, &[on(67, 0.8)]);
        let near = peak_cents(&audio, 48_000.0, hz(67) * ratio, -200..=200);
        goertzel(&audio, 48_000.0, hz(67) * ratio * (near / 1200.0).exp2()) / f64::from(rms(&audio))
    };
    // Hum, prime, the minor third and the octave above.
    for ratio in [0.5, 1.0, 1.2, 2.0] {
        assert!(share(0.0, ratio) > 0.2, "a cast bell lacks {ratio}x");
    }
    assert!(share(1.0, 1.0) < 0.02, "a chime has a partial on its note");
    for ratio in [2.0, 2.99, 4.17] {
        assert!(share(1.0, ratio) > 0.2, "a chime lacks {ratio}x");
    }
}

#[test]
fn a_bells_pairs_beat_and_that_is_also_its_width() {
    // Struck a thirty-second of the way round, half-way between the two shapes of a partial
    // with four lobes: both sound as loudly, and beat as deeply as they can.
    let take = |warble: f32| {
        let mut bell = Physical::new(Model::Bell);
        bell.set_param_by_key("warble", warble);
        bell.set_param_by_key("position", 1.0 / 32.0);
        Rig::new(Box::new(bell), 48_000.0, 256, 2).render_stereo(144_000, &[on(60, 0.8)])
    };
    let ripple = |audio: &[f32]| {
        let levels: Vec<f64> = audio[4_800..62_400]
            .chunks(2_400)
            .map(|chunk| windowed(chunk, 48_000.0, hz(60) * 2.0))
            .collect();
        // A pair beats once a second or so: each window against the two a third of a
        // second either side of it.
        levels
            .windows(13)
            .map(|span| (20.0 * (span[6] / (span[0] * span[12]).sqrt()).log10()).abs())
            .fold(0.0_f64, f64::max)
    };
    let (round_left, round_right) = take(0.0);
    let (cast_left, cast_right) = take(1.0);
    let mid = |l: &[f32], r: &[f32]| -> Vec<f32> { l.iter().zip(r).map(|(l, r)| l + r).collect() };
    let (round, cast) = (
        ripple(&mid(&round_left, &round_right)),
        ripple(&mid(&cast_left, &cast_right)),
    );
    assert!(round < 0.5, "a true bell ripples {round} dB");
    assert!(cast > 1.5, "an untrue one ripples only {cast} dB");
    // The two halves of each pair are heard to either side, and trade places as they beat.
    let lean = |from: usize| {
        level_db(&cast_left[from..from + 4_800]) - level_db(&cast_right[from..from + 4_800])
    };
    let leans: Vec<f64> = (1..28).map(|n| lean(n * 4_800)).collect();
    let (low, high) = leans
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| {
            (l.min(*x), h.max(*x))
        });
    assert!(
        high - low > 1.0,
        "the image stood still: {low} to {high} dB"
    );
}

// --- Winds -------------------------------------------------------------------------------

#[test]
fn a_clarinet_is_a_closed_pipe_and_brightens_as_it_is_blown() {
    let spectrum = |velocity: f32| {
        let mut clarinet = rig(Model::Clarinet, 48_000.0);
        let audio = clarinet.render(48_000, &[on(50, velocity)]);
        let steady = audio[24_000..].to_vec();
        let at = |n: f64| goertzel(&steady, 48_000.0, hz(50) * n);
        (at(1.0), at(2.0), at(3.0), at(4.0), at(7.0))
    };
    let (first, second, third, fourth, seventh) = spectrum(0.7);
    // Closed at the reed, the bore has no place for an even harmonic.
    assert!(
        third > second * 4.0 && third > fourth * 4.0,
        "{second} {third} {fourth}"
    );
    assert!(
        first > third * 0.9,
        "the fundamental should lead: {first} against {third}"
    );
    let (soft_first, _, _, _, soft_seventh) = spectrum(0.25);
    assert!(
        seventh / first > 2.0 * soft_seventh / soft_first,
        "blowing harder should brighten the tone"
    );
}

#[test]
fn every_note_of_a_clarinet_speaks_at_the_softest_breath() {
    for pitch in [40, 52, 64, 76, 88, 96] {
        let mut clarinet = rig(Model::Clarinet, 48_000.0);
        clarinet.set_param("pressure", 0.05);
        let audio = clarinet.render(48_000, &[on(pitch, 0.1)]);
        assert!(
            rms(&audio[36_000..]) > 3.0e-4,
            "MIDI {pitch} did not speak: {}",
            rms(&audio[36_000..])
        );
        let cents = peak_cents(&audio[24_000..], 48_000.0, hz(pitch), -300..=300);
        assert!(cents.abs() < 8.0, "MIDI {pitch}: {cents} cents");
    }
}

#[test]
fn a_whistle_is_nearly_a_sine_with_breath_in_it() {
    for pitch in [74_u8, 79, 86, 93] {
        for velocity in [0.35, 0.95] {
            let mut whistle = rig(Model::TinWhistle, 48_000.0);
            let audio = whistle.render(38_400, &[on(pitch, velocity)]);
            let body = &audio[14_400..];
            let cents = peak_cents(body, 48_000.0, hz(pitch), -300..=300);
            assert!(cents.abs() < 8.0, "MIDI {pitch}: {cents} cents");
            let tuned = hz(pitch) * (cents / 1200.0).exp2();
            let fundamental = goertzel(body, 48_000.0, tuned);
            assert!(fundamental > 1.0e-3, "MIDI {pitch} is silent");
            for harmonic in [2.0, 3.0, 4.0] {
                let level = goertzel(body, 48_000.0, tuned * harmonic);
                assert!(
                    level < fundamental * 0.5,
                    "MIDI {pitch} velocity {velocity}: harmonic {harmonic} is {level} of {fundamental}"
                );
            }
            let mean = body.iter().map(|s| f64::from(*s)).sum::<f64>() / body.len() as f64;
            assert!(
                mean.abs() < f64::from(rms(body)) * 0.05,
                "MIDI {pitch} has DC"
            );
        }
    }
    // The breath is in the tone: between the harmonics there is air, and more of it when
    // asked. Read half-way between harmonics, a fiftieth of a second at a time.
    let air = |noise: f32| {
        let mut whistle = rig(Model::TinWhistle, 48_000.0);
        whistle.set_param("breath_noise", noise);
        let audio = whistle.render(38_400, &[on(81, 0.7)]);
        let mut power = 0.0;
        for chunk in audio[14_400..].as_chunks::<960>().0 {
            for between in [1.5, 2.5, 3.5, 4.5] {
                power += windowed(chunk, 48_000.0, hz(81) * between).powi(2);
            }
        }
        (power / 100.0).sqrt() / goertzel(&audio[14_400..], 48_000.0, hz(81))
    };
    let (quiet, breathy) = (air(0.0), air(0.6));
    assert!(breathy > quiet * 4.0, "{quiet} then {breathy}");
    // Air in the tone, not a tone in the air: each fiftieth-of-a-second reading between the
    // harmonics is sixty or seventy decibels under the note, which over the whole band
    // comes to a breath some fifteen or twenty under it.
    assert!((0.000_1..0.003).contains(&air(0.12)), "{}", air(0.12));
}

#[test]
fn an_edge_off_the_centre_of_the_jet_adds_the_octave() {
    let octave = |jet: f32| {
        let mut whistle = rig(Model::TinWhistle, 48_000.0);
        whistle.set_param("jet_shape", jet);
        whistle.set_param("breath_noise", 0.0);
        let audio = whistle.render(38_400, &[on(81, 0.7)]);
        let body = &audio[19_200..];
        let cents = peak_cents(body, 48_000.0, hz(81), -200..=200);
        let tuned = hz(81) * (cents / 1200.0).exp2();
        goertzel(body, 48_000.0, tuned * 2.0) / goertzel(body, 48_000.0, tuned)
    };
    assert!(octave(0.0) < 0.01, "a centred edge gave {}", octave(0.0));
    assert!(
        octave(0.6) > 0.05,
        "an offset edge gave only {}",
        octave(0.6)
    );
}

#[test]
fn no_breath_makes_no_sound_and_expression_is_breath() {
    for model in [Model::Clarinet, Model::TinWhistle, Model::Brass] {
        let mut wind = rig(model, 48_000.0);
        let full = wind.render(24_000, &[on(home(model), 0.8)]);
        let hushed = wind.render(24_000, &[controller(0, 11, 0.0)]);
        assert!(
            rms(&hushed[12_000..]) < rms(&full[12_000..]) * 0.01,
            "{model:?}"
        );
    }
    let mut whistle = rig(Model::TinWhistle, 48_000.0);
    whistle.set_param("pressure", 0.0);
    let audio = whistle.render(24_000, &[on(81, 0.8)]);
    assert_eq!(
        peak(&audio),
        0.0,
        "a closed air supply cannot excite a pipe"
    );
}

#[test]
fn a_slurred_wind_note_glides_and_releases_when_its_last_key_does() {
    for model in [Model::Clarinet, Model::TinWhistle, Model::Brass] {
        let mut wind = rig(model, 48_000.0);
        wind.set_param("legato", 1.0);
        let low = home(model);
        wind.render(24_000, &[on(low, 0.8)]);
        let upper = wind.render(24_000, &[on(low + 5, 0.8)]);
        assert_eq!(wind.instrument.active_voices(), 1, "{model:?}");
        let cents = peak_cents(&upper[12_000..], 48_000.0, hz(low + 5), -300..=300);
        assert!(cents.abs() < 10.0, "{model:?} slurred to {cents} cents off");
        wind.render(24_000, &[off(0, low + 5), off(0, low)]);
        wind.render(24_000, &[]);
        assert_eq!(wind.instrument.active_voices(), 0, "{model:?}");
    }
}

#[test]
fn open_guitar_strings_answer_a_note_that_shares_their_partials() {
    // A3 is the open fifth string's second harmonic; B♭3 lies between every open string's
    // partials. Let each go after a quarter of a second, and listen a second later.
    let residue = |pitch: u8| {
        let mut guitar = whole(Model::Guitar);
        let audio = guitar.render(96_000, &[on(pitch, 0.9), off(12_000, pitch)]);
        let played = partial(&audio[..12_000], hz(pitch), 1.0);
        let left = partial(&audio[48_000..72_000], hz(pitch), 1.0);
        20.0 * (left / played).max(1e-12).log10()
    };
    let shared = residue(57);
    let between = residue(58);
    assert!(
        (-45.0..-20.0).contains(&shared),
        "the open A answers A3 {shared} dB under it"
    );
    assert!(
        between < shared - 15.0,
        "B♭3 leaves {between} dB, against A3's {shared} dB"
    );
}

#[test]
fn half_a_pedal_shortens_a_released_note_without_stopping_it() {
    // A damper brushing the string: the note dies faster than under the pedal and slower than
    // under the damper.
    let fall = |depth: f32| {
        let mut piano = rig(Model::Piano, 48_000.0);
        piano.set_param("sympathetic", 0.0);
        let audio = piano.render(
            48_000,
            &[controller(0, 64, depth), on(60, 0.65), off(4_800, 60)],
        );
        level_db(&audio[..4_800]) - level_db(&audio[19_200..24_000])
    };
    let (up, half, down) = (fall(0.0), fall(0.5), fall(1.0));
    assert!(up > 40.0, "with the pedal up the note fell {up} dB");
    // Under the pedal it falls only as a struck middle C does: 31 dB a second at first, on the
    // recorded Steinway.
    assert!(down < 22.0, "under the pedal the note fell {down} dB");
    assert!(
        down + 10.0 < half && half < up - 10.0,
        "half a pedal fell {half} dB, between {down} and {up}"
    );
    // The free play: a pedal barely touched has not reached the dampers.
    assert!((fall(0.2) - up).abs() < 0.5);
}

#[test]
fn the_sostenuto_holds_the_keys_down_when_it_went_down_and_no_others() {
    let mut piano = rig(Model::Piano, 48_000.0);
    piano.set_param("sympathetic", 0.0);
    piano.render(
        4_800,
        &[on(48, 0.7), controller(2_400, 66, 1.0), off(4_000, 48)],
    );
    // Caught: C3 goes on ringing with its key up. E4, struck after, is damped as usual.
    let after = piano.render(24_000, &[on(64, 0.7), off(4_800, 64)]);
    let (c3, e4) = (hz(48), hz(64));
    let held = goertzel(&after[19_200..], 48_000.0, c3);
    let damped = goertzel(&after[19_200..], 48_000.0, e4);
    // The same, with C3 held by its key instead: E4 is damped there, so what is left at its
    // pitch is C3's leakage and nothing of E4's own.
    piano.instrument.reset();
    piano.render(4_800, &[on(48, 0.7)]);
    let reference = piano.render(24_000, &[on(64, 0.7), off(4_800, 64)]);
    let leakage = goertzel(&reference[19_200..], 48_000.0, e4);
    assert!(held > 1.0e-3, "the sostenuto let C3 go");
    assert!(
        damped < leakage * 2.0,
        "the sostenuto caught E4 as well: {damped} against {leakage}"
    );
    piano.instrument.reset();
    piano.render(
        4_800,
        &[on(48, 0.7), controller(2_400, 66, 1.0), off(4_000, 48)],
    );
    piano.render(24_000, &[on(64, 0.7), off(4_800, 64)]);
    // Let up, and C3's damper falls.
    let lifted = piano.render(24_000, &[controller(0, 66, 0.0)]);
    assert!(goertzel(&lifted[19_200..], 48_000.0, c3) < held * 1.0e-2);
}

#[test]
fn the_soft_pedal_strikes_two_strings_of_three_with_softer_felt() {
    let blow = |soft: f32| {
        let mut piano = rig(Model::Piano, 48_000.0);
        piano.set_param("sympathetic", 0.0);
        let audio = piano.render(24_000, &[controller(0, 67, soft), on(72, 0.7)]);
        let upper: f64 = (6..=10)
            .map(|n| partial(&audio[..9_600], hz(72), f64::from(n)))
            .sum();
        let fundamental = partial(&audio[..9_600], hz(72), 1.0);
        (
            level_db(&audio[..9_600]),
            20.0 * (upper / fundamental).log10(),
        )
    };
    let ((level, bright), (soft_level, soft_bright)) = (blow(0.0), blow(1.0));
    assert!(
        (1.0..8.0).contains(&(level - soft_level)),
        "the soft pedal took {} dB off",
        level - soft_level
    );
    assert!(
        soft_bright < bright - 2.0,
        "brightness {bright} dB, with the soft pedal {soft_bright} dB"
    );
}

#[test]
fn the_single_line_instruments_slur_out_of_the_box() {
    for model in [
        Model::Clarinet,
        Model::TinWhistle,
        Model::Violin,
        Model::Brass,
    ] {
        let mut line = rig(model, 48_000.0);
        line.render(12_000, &[on(72, 0.8)]);
        line.render(12_000, &[on(74, 0.8)]);
        assert_eq!(
            line.instrument.active_voices(),
            1,
            "{model:?}: a key over a held one should move the line, not start a second note"
        );
    }
}

#[test]
fn the_legato_footswitch_slurs_only_while_it_is_down() {
    let mut clarinet = rig(Model::Clarinet, 48_000.0);
    // The clarinet's own control slurs by default; off, only the footswitch can.
    clarinet.set_param("legato", 0.0);
    // Struck before the footswitch: an ordinary note, which its own release stops.
    clarinet.render(12_000, &[on(60, 0.8), controller(6_000, 68, 1.0)]);
    let slurred = clarinet.render(12_000, &[on(64, 0.8)]);
    assert_eq!(
        clarinet.instrument.active_voices(),
        2,
        "the footswitch went down over a sounding note, which is not the line"
    );
    let quietest = slurred.chunks(480).map(rms).fold(f32::INFINITY, f32::min);
    assert!(quietest > 1.0e-4);
    clarinet.render(12_000, &[on(67, 0.8)]);
    assert_eq!(clarinet.instrument.active_voices(), 2, "E4 to G4 is a slur");
    // Releasing the key struck before the footswitch stops that note, not the line.
    clarinet.render(24_000, &[off(0, 60)]);
    assert_eq!(clarinet.instrument.active_voices(), 1);
    // Releasing G4 falls back to E4, which is still down.
    let back = clarinet.render(12_000, &[off(0, 67)]);
    assert!(
        goertzel(&back[6_000..], 48_000.0, hz(64))
            > goertzel(&back[6_000..], 48_000.0, hz(67)) * 3.0
    );
    // Up again: the next key is another note.
    clarinet.render(12_000, &[controller(0, 68, 0.0), on(72, 0.8)]);
    assert_eq!(clarinet.instrument.active_voices(), 2);
    clarinet.render(48_000, &[off(0, 64), off(0, 72)]);
    assert_eq!(
        clarinet.instrument.active_voices(),
        0,
        "a note was left sounding"
    );
}

#[test]
fn a_vibraphone_pedal_half_down_lets_the_felt_brush_the_bars() {
    let tail = |depth: f32| {
        let mut vibes = bare(Model::Mallet);
        vibes.set_param_by_key("bar", 1.0);
        vibes.set_param_by_key("tremolo", 0.0);
        let mut rig = Rig::new(Box::new(vibes), 48_000.0, 256, 2);
        rig.set_param("release", 0.5);
        let audio = rig.render(
            24_000,
            &[controller(0, 64, depth), on(60, 0.8), off(2_400, 60)],
        );
        level_db(&audio[2_400..4_800]) - level_db(&audio[14_400..16_800])
    };
    let (up, half, down) = (tail(0.0), tail(0.5), tail(1.0));
    assert!(
        down + 5.0 < half && half + 10.0 < up,
        "a quarter of a second after release, up {up} dB, half {half} dB, down {down} dB"
    );
}

#[test]
fn a_palm_at_the_bridge_shortens_a_held_note_and_takes_its_top_first() {
    // An open low string, held throughout: only the palm (controller 75 below its middle)
    // shortens it.
    let pluck = |model: Model, key: u8, decay: f32| {
        let mut guitar = rig(model, 48_000.0);
        let audio = guitar.render(24_000, &[controller(0, 75, decay), on(key, 0.8)]);
        let (early, late) = (&audio[..4_800], &audio[14_400..19_200]);
        // The fundamental's own fall, beneath what little the body and the attack leave.
        let fall = 20.0 * (partial(early, hz(key), 1.0) / partial(late, hz(key), 1.0)).log10();
        // The tone after the pick's click, which the palm does not touch.
        let after = &audio[1_200..6_000];
        let upper: f64 = (6..=10)
            .map(|n| partial(after, hz(key), f64::from(n)))
            .sum();
        let bright = 20.0 * (upper / partial(after, hz(key), 1.0)).log10();
        (fall, bright)
    };
    for (model, key, least) in [
        (Model::Guitar, 40, 60.0),
        (Model::ElectricGuitar, 40, 60.0),
        (Model::Bass, 28, 25.0),
    ] {
        let (open, open_bright) = pluck(model, key, 0.5);
        let (half, _) = pluck(model, key, 0.25);
        let (muted, muted_bright) = pluck(model, key, 0.0);
        assert!(open < 15.0, "{model:?} rang only {open} dB down");
        assert!(muted > least, "{model:?} under the palm fell {muted} dB");
        assert!(
            open + 5.0 < half && half + 5.0 < muted,
            "{model:?}: {half} dB half muted"
        );
        assert!(
            muted_bright < open_bright - 10.0,
            "{model:?}: top {open_bright} dB, under the palm {muted_bright} dB"
        );
        // Above the middle the controller leaves the string as it is.
        assert!((pluck(model, key, 1.0).0 - open).abs() < 0.1);
    }
}

#[test]
fn a_palm_laid_on_a_ringing_string_stops_it_and_lifted_lets_the_next_ring() {
    let mut guitar = rig(Model::Guitar, 48_000.0);
    let e3 = hz(52);
    guitar.render(9_600, &[on(52, 0.8)]);
    let ringing = guitar.render(4_800, &[]);
    let laid = guitar.render(19_200, &[controller(0, 75, 0.0)]);
    let fall = 20.0 * (partial(&ringing, e3, 1.0) / partial(&laid[14_400..], e3, 1.0)).log10();
    assert!(fall > 40.0, "the palm took {fall} dB off");
    guitar.render(48_000, &[controller(0, 75, 0.5), off(0, 52)]);
    let next = guitar.render(19_200, &[on(52, 0.8)]);
    let rings =
        20.0 * (partial(&next[..4_800], e3, 1.0) / partial(&next[14_400..], e3, 1.0)).log10();
    assert!(rings < 15.0, "lifted, the next note fell {rings} dB");
}

#[test]
fn letting_a_fretted_note_go_mutes_it_from_the_top_down() {
    // The fretting finger eases off and lies on the string: a released note stops within its
    // release time, and its upper partials go before its fundamental.
    let mut guitar = rig(Model::Guitar, 48_000.0);
    let e3 = hz(52);
    let held = guitar.render(24_000, &[on(52, 0.8)]);
    guitar.instrument.reset();
    let let_go = guitar.render(24_000, &[on(52, 0.8), off(4_800, 52)]);
    let fall = |audio: &[f32], n: f64| {
        20.0 * (partial(&audio[..4_800], e3, n) / partial(&audio[9_600..14_400], e3, n)).log10()
    };
    assert!(
        fall(&held, 1.0) < 10.0,
        "held, it fell {} dB",
        fall(&held, 1.0)
    );
    assert!(
        fall(&let_go, 1.0) > 40.0,
        "let go, it fell {} dB",
        fall(&let_go, 1.0)
    );
    assert!(
        fall(&let_go, 6.0) > fall(&let_go, 1.0) + 5.0,
        "the sixth partial fell {} dB, the fundamental {} dB",
        fall(&let_go, 6.0),
        fall(&let_go, 1.0)
    );
}

#[test]
fn controller_74_draws_the_bow_toward_the_bridge_or_over_the_fingerboard() {
    let bowed = |events: &[NoteEvent]| {
        let mut violin = rig(Model::Violin, 48_000.0);
        violin.set_param("vibrato", 0.0);
        let mut all = events.to_vec();
        all.push(on(69, 0.7));
        violin.render(48_000, &all)
    };
    // The spectral centroid, in harmonics: one band of partials would sit on a different
    // notch of the bow's comb at every place.
    let top = |audio: &[f32]| {
        let steady = &audio[24_000..48_000];
        let partials: Vec<f64> = (1..=30)
            .map(|n| partial(steady, hz(69), f64::from(n)))
            .collect();
        let weighted: f64 = partials
            .iter()
            .enumerate()
            .map(|(n, a)| (n + 1) as f64 * a)
            .sum();
        weighted / partials.iter().sum::<f64>()
    };
    let plain = bowed(&[]);
    // At its middle the controller is where the bow already was.
    assert!(bowed(&[controller(0, 74, 0.5)]) == plain);
    let (tasto, normal, ponticello) = (
        top(&bowed(&[controller(0, 74, 0.0)])),
        top(&plain),
        top(&bowed(&[controller(0, 74, 1.0)])),
    );
    // Toward the bridge the note is brighter by what a string that still sounds its note
    // can be. It read half as bright again while a bow drawn there crushed the note, and
    // most of that was the noise of the crushing.
    assert!(
        tasto < normal * 0.9 && normal * 1.2 < ponticello,
        "centroid sul tasto {tasto}, ordinario {normal}, sul ponticello {ponticello} harmonics"
    );
}

#[test]
fn portamento_slides_a_slur_and_lets_the_finger_jump_without_it() {
    for model in [
        Model::Violin,
        Model::Clarinet,
        Model::TinWhistle,
        Model::Brass,
    ] {
        let (from, to) = (home(model), home(model) + 7);
        let slur = |portamento: f32| {
            let mut line = rig(model, 48_000.0);
            line.set_param("legato", 1.0);
            line.render(
                24_000,
                &[
                    controller(0, 65, portamento),
                    controller(0, 5, 0.6),
                    on(from, 0.8),
                ],
            );
            line.render(96_000, &[on(to, 0.8)])
        };
        // A tenth of a second into the slur, and two seconds on.
        let arrived = |audio: &[f32], at: std::ops::Range<usize>| {
            goertzel(&audio[at.clone()], 48_000.0, hz(to))
                / goertzel(&audio[at], 48_000.0, hz(from)).max(1.0e-9)
        };
        let (jumped, slid) = (slur(0.0), slur(1.0));
        assert!(
            arrived(&jumped, 2_400..7_200) > 3.0,
            "{model:?} without portamento was still on the old note"
        );
        assert!(
            arrived(&slid, 2_400..7_200) < 1.0,
            "{model:?} with portamento was already on the new note"
        );
        assert!(
            arrived(&slid, 86_400..96_000) > 3.0,
            "{model:?} with portamento never arrived"
        );
    }
}

#[test]
fn the_breath_controller_blows_a_pipe_and_without_breath_it_is_silent() {
    for model in [Model::Clarinet, Model::TinWhistle, Model::Brass] {
        let key = home(model);
        let blown = |events: &[NoteEvent]| {
            let mut wind = rig(model, 48_000.0);
            let mut all = events.to_vec();
            all.push(on(key, 0.7));
            let audio = wind.render(48_000, &all);
            level_db(&audio[24_000..])
        };
        let plain = blown(&[]);
        assert!((blown(&[controller(0, 2, 1.0)]) - plain).abs() < 0.01);
        let (half, none) = (
            blown(&[controller(0, 2, 0.5)]),
            blown(&[controller(0, 2, 0.0)]),
        );
        assert!(
            half < plain - 2.0,
            "{model:?} half a breath is {half} dB against {plain}"
        );
        assert!(
            none < plain - 60.0,
            "{model:?} with no breath sounded at {none} dB"
        );
    }
}

#[test]
fn controller_74_is_a_harder_or_softer_mallet_hammer_or_pick() {
    // How bright the first tenth of a second is: the level of its slope against its own, which
    // reads the same for a bar's or a bell's partials as for a string's harmonics.
    let centroid = |model: Model, key: u8, brightness: Option<f32>| {
        let mut rig = rig(model, 48_000.0);
        let mut events = vec![on(key, 0.7)];
        if let Some(value) = brightness {
            events.insert(0, controller(0, 74, value));
        }
        let audio = rig.render(4_800, &events);
        let slope: Vec<f32> = audio.windows(2).map(|pair| pair[1] - pair[0]).collect();
        (f64::from(rms(&slope) / rms(&audio)), audio)
    };
    for (model, key) in [
        (Model::Piano, 60),
        (Model::HammeredDulcimer, 67),
        (Model::Guitar, 52),
        // Low enough that a soft mallet is not already held to the bar's swing.
        (Model::Mallet, 48),
        (Model::Bell, 72),
    ] {
        let (plain, plain_audio) = centroid(model, key, None);
        assert!(
            centroid(model, key, Some(0.5)).1 == plain_audio,
            "{model:?} moved at the middle"
        );
        let (soft, _) = centroid(model, key, Some(0.0));
        let (hard, _) = centroid(model, key, Some(1.0));
        assert!(
            soft < plain && plain < hard && hard > soft * 1.2,
            "{model:?}: centroid soft {soft}, plain {plain}, hard {hard}"
        );
    }
}

#[test]
fn the_modulation_wheel_turns_a_vibraphones_motor() {
    let mut vibes = bare(Model::Mallet);
    vibes.set_param_by_key("bar", 1.0);
    vibes.set_param_by_key("body", 1.0);
    vibes.set_param_by_key("tremolo", 0.0);
    vibes.set_param_by_key("tremolo_rate", 5.0);
    let mut rig = Rig::new(Box::new(vibes), 48_000.0, 256, 2);
    let swing = |audio: &[f32]| {
        let levels: Vec<f64> = audio[4_800..].chunks(960).map(level_db).collect();
        levels
            .windows(6)
            .map(|w| (w[5] - w[0]).abs())
            .fold(0.0_f64, f64::max)
    };
    let still = rig.render(48_000, &[on(60, 0.8)]);
    rig.instrument.reset();
    let turning = rig.render(48_000, &[controller(0, 1, 1.0), on(60, 0.8)]);
    assert!(
        swing(&still) < 1.5,
        "the motor turned with the wheel down: {}",
        swing(&still)
    );
    assert!(
        swing(&turning) > 3.0,
        "the wheel swings {} dB",
        swing(&turning)
    );
}

#[test]
fn a_damper_pedal_lets_chimes_and_a_cimbalom_ring_after_release() {
    for model in [Model::Bell, Model::HammeredDulcimer] {
        let tail = |depth: f32| {
            let mut rig = rig(model, 48_000.0);
            rig.set_param("release", 0.3);
            let audio = rig.render(
                48_000,
                &[controller(0, 64, depth), on(72, 0.8), off(4_800, 72)],
            );
            level_db(&audio[4_800..9_600]) - level_db(&audio[24_000..28_800])
        };
        let (up, half, down) = (tail(0.0), tail(0.5), tail(1.0));
        assert!(
            down + 5.0 < half && half + 5.0 < up,
            "{model:?} after release: up {up} dB, half {half} dB, down {down} dB"
        );
    }
}

// --- The slapped bass --------------------------------------------------------------------

/// A bass at `rate` played with `technique`, the hand over the last fret.
fn slapped(technique: f32, rate: f64) -> Rig {
    let mut bass = rig(Model::Bass, rate);
    bass.set_param("technique", technique);
    bass.set_param("position", 0.32);
    bass
}

/// How bright a stretch of sound is: the level of its slope over its own, in decibels.
fn edge_db(samples: &[f32]) -> f64 {
    let slope: Vec<f32> = samples.windows(2).map(|pair| pair[1] - pair[0]).collect();
    level_db(&slope) - level_db(samples)
}

#[test]
fn only_a_bass_is_slapped_and_it_is_plucked_until_it_is_told_otherwise() {
    let bass = Physical::new(Model::Bass);
    assert_eq!(bass.param_by_key("technique"), Some(0.0));
    let slapped = |model: Model| {
        let instrument = Physical::new(model);
        let technique = instrument.descriptor_for_key("technique").unwrap();
        technique.choices.iter().any(|choice| choice == "Slap")
    };
    assert!(slapped(Model::Bass));
    assert!(!slapped(Model::Guitar) && !slapped(Model::ElectricGuitar));
}

#[test]
fn a_slapped_string_comes_down_on_its_frets_and_a_plucked_one_does_not() {
    // The same note at the same velocity with the hand in the same place: all that differs
    // is whether the string is driven at the board. Thumbed notes on the low strings, popped
    // ones on the high.
    for (technique, pitches) in [(2.0, [28_u8, 33, 40, 45]), (3.0, [40, 45, 50, 55])] {
        for pitch in pitches {
            let take =
                |technique: f32| slapped(technique, 48_000.0).render(24_000, &[on(pitch, 0.8)]);
            let (plucked, slapped) = (take(0.0), take(technique));
            // The blow is the metal: far brighter than a pluck for its first few hundredths
            // of a second, and standing further over the note it leaves.
            let clank = edge_db(&slapped[..1_920]) - edge_db(&plucked[..1_920]);
            assert!(clank > 5.0, "{technique} on {pitch}: {clank} dB brighter");
            let crest =
                |audio: &[f32]| 20.0 * f64::from(peak(audio)).log10() - level_db(&audio[..19_200]);
            let stands = crest(&slapped) - crest(&plucked);
            assert!(stands > 1.5, "{technique} on {pitch}: {stands} dB of crest");
            // It is the same instrument at the same dynamic, and about as loud.
            let level = level_db(&slapped[..19_200]) - level_db(&plucked[..19_200]);
            assert!(
                (-8.0..4.0).contains(&level),
                "{technique} on {pitch}: {level} dB louder"
            );
        }
    }
}

#[test]
fn a_harder_slap_clanks_more_than_it_gets_louder() {
    // The frets are as far under the string as they are however hard it is slapped, so the
    // note left behind stops growing and what grows is the blow.
    for (technique, pitch) in [(2.0, 28_u8), (2.0, 33), (3.0, 45), (3.0, 50)] {
        let take = |velocity: f32| {
            let audio = slapped(technique, 48_000.0).render(24_000, &[on(pitch, velocity)]);
            let fundamental = 20.0 * partial(&audio[4_800..19_200], hz(pitch), 1.0).log10();
            (
                fundamental,
                level_db(&audio[..1_920]),
                edge_db(&audio[..1_920]),
            )
        };
        let (soft, hard) = (take(0.3), take(1.0));
        let (fundamental, blow, edge) = (hard.0 - soft.0, hard.1 - soft.1, hard.2 - soft.2);
        assert!(
            blow > 6.0,
            "{technique} on {pitch}: the blow grew {blow} dB"
        );
        assert!(edge > 1.5, "{technique} on {pitch}: {edge} dB brighter");
        // A string that rings for long keeps meeting its frets for longer, and a hard pop
        // leaves a little less of its fundamental than a soft one.
        assert!(
            fundamental.abs() < 8.0 && fundamental < blow - 4.0,
            "{technique} on {pitch}: the fundamental grew {fundamental} dB"
        );
    }
}

#[test]
fn the_slap_technique_thumbs_low_notes_and_pops_the_ones_from_e2_up() {
    let take =
        |technique: f32, pitch: u8| slapped(technique, 48_000.0).render(9_600, &[on(pitch, 0.8)]);
    assert!(take(1.0, 39) == take(2.0, 39));
    assert!(take(1.0, 40) == take(3.0, 40));
    assert!(take(1.0, 39) != take(3.0, 39));
    assert!(take(1.0, 40) != take(2.0, 40));
}

#[test]
fn a_slapped_note_is_in_tune_rings_on_and_ends() {
    for technique in [2.0, 3.0] {
        for pitch in [28_u8, 40, 52, 60] {
            let mut bass = slapped(technique, 48_000.0);
            let audio = bass.render(96_000, &[on(pitch, 0.9)]);
            assert!(audio.iter().all(|sample| sample.is_finite()));
            assert!(
                peak(&audio) < 1.0,
                "{technique} on {pitch}: {}",
                peak(&audio)
            );
            let cents = peak_cents(&audio[12_000..48_000], 48_000.0, hz(pitch), -300..=300);
            assert!(cents.abs() < 6.0, "{technique} on {pitch}: {cents} cents");
            // The rattle takes its share at once; what is left rings as a plucked note does,
            // which on a bass is for a long time.
            let fell = level_db(&audio[4_800..9_600]) - level_db(&audio[43_200..48_000]);
            assert!(
                (2.0..22.0).contains(&fell),
                "{technique} on {pitch}: fell {fell} dB in a second"
            );
            bass.render(96_000, &[off(0, pitch)]);
            assert_eq!(bass.instrument.active_voices(), 0);
        }
    }
}

#[test]
fn a_slap_is_much_the_same_at_every_sample_rate() {
    // The frets are met sample by sample, and how often that is must not be heard.
    for technique in [2.0, 3.0] {
        for pitch in [28_u8, 45] {
            let take = |rate: f64| {
                let frames = |seconds: f64| (rate * seconds) as usize;
                let audio = slapped(technique, rate).render(frames(0.5), &[on(pitch, 0.8)]);
                assert!(audio.iter().all(|sample| sample.is_finite()));
                (
                    level_db(&audio[..frames(0.04)]),
                    level_db(&audio[frames(0.1)..frames(0.4)]),
                )
            };
            let reference = take(48_000.0);
            for rate in [44_100.0, 96_000.0] {
                let (blow, held) = take(rate);
                assert!(
                    (blow - reference.0).abs() < 1.5 && (held - reference.1).abs() < 2.0,
                    "{technique} on {pitch} at {rate}: blow {blow} dB, held {held} dB, \
                     against {reference:?}"
                );
            }
        }
    }
}

#[test]
fn slapping_does_not_depend_on_the_block_size_and_allocates_nothing() {
    let events = [
        on(33, 0.9),
        NoteEvent::note_on(700, 45, 0.8),
        controller(1_500, 75, 0.2),
        off(2_000, 33),
        NoteEvent::note_on(2_400, 28, 1.0),
        NoteEvent::PitchBend {
            frame: 3_000,
            semitones: 0.5,
        },
        off(5_000, 45),
    ];
    let mut renders = Vec::new();
    for block in [64, 250, 1_024] {
        let mut bass = Physical::new(Model::Bass);
        bass.set_param_by_key("technique", 1.0);
        let mut rig = Rig::new(Box::new(bass), 48_000.0, block, 2);
        renders.push(rig.render_stereo(12_000, &events));
    }
    assert!(peak(&renders[0].0) > 0.0);
    assert!(renders[0] == renders[1] && renders[0] == renders[2]);

    let mut bass = Physical::new(Model::Bass);
    bass.set_param_by_key("technique", 1.0);
    bass.prepare(&PrepareContext::new(48_000.0, 256, 2));
    let mut buffer = AudioBuffer::stereo(256, 48_000.0);
    let ctx = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
    let quality = bass.descriptor_for_key("quality").unwrap().id;
    let allocations = count_allocations(|| {
        // Thumbs and pops at every quality, past the end of the neck and past the pool.
        for pitch in 26..76_u8 {
            bass.set_param(quality, f32::from(pitch % 3));
            bass.process(&[on(pitch, 0.9), off(200, pitch - 2)], &mut buffer, &ctx);
        }
        bass.process(&[NoteEvent::AllSoundOff { frame: 50 }], &mut buffer, &ctx);
        bass.reset();
    });
    assert_eq!(allocations, 0);
}

// --- Techniques of the other instruments --------------------------------------------------

/// A model at 48 kHz with its technique control at `technique`.
fn played(model: Model, technique: f32) -> Rig {
    let mut rig = rig(model, 48_000.0);
    rig.set_param("technique", technique);
    rig
}

/// How far the level of `samples` swings: the loudest tenth of its five-millisecond stretches
/// over the quietest tenth, in decibels.
fn swing_db(samples: &[f32]) -> f64 {
    let mut levels: Vec<f64> = samples.chunks(240).map(level_db).collect();
    levels.sort_by(f64::total_cmp);
    levels[levels.len() * 9 / 10] - levels[levels.len() / 10]
}

/// How many times a second the level of `samples` swings: the strongest rate, from 4 to 40
/// hertz, in the levels of its five-millisecond stretches.
fn swing_hz(samples: &[f32]) -> i32 {
    let levels: Vec<f64> = samples.chunks(240).map(level_db).collect();
    let mean = levels.iter().sum::<f64>() / levels.len() as f64;
    let at = |rate: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, level) in levels.iter().enumerate() {
            let turn = std::f64::consts::TAU * rate * n as f64 / 200.0;
            re += (level - mean) * turn.cos();
            im += (level - mean) * turn.sin();
        }
        re.hypot(im)
    };
    (4..=40)
        .max_by(|a, b| at(f64::from(*a)).total_cmp(&at(f64::from(*b))))
        .unwrap()
}

#[test]
fn every_technique_control_starts_on_the_way_the_instrument_was_always_played() {
    for (model, choices) in [
        (Model::Guitar, 2),
        (Model::ElectricGuitar, 2),
        (Model::Bass, 5),
        (Model::Violin, 3),
        (Model::Clarinet, 2),
        (Model::TinWhistle, 2),
        (Model::Brass, 2),
    ] {
        let instrument = Physical::new(model);
        let technique = instrument.descriptor_for_key("technique").unwrap();
        assert_eq!(technique.default, 0.0, "{model:?}");
        assert_eq!(technique.choices.len(), choices, "{model:?}");
        assert_eq!(technique.max, (choices - 1) as f32, "{model:?}");
    }
    // A hammer, a mallet and a clapper are what they are.
    for model in [
        Model::Piano,
        Model::HammeredDulcimer,
        Model::Mallet,
        Model::Bell,
    ] {
        assert!(Physical::new(model).param_by_key("technique").is_none());
    }
}

#[test]
fn a_plucked_string_section_rings_at_its_pitch_dies_away_and_ends_with_its_key_down() {
    for players in [1.0, 4.0] {
        for pitch in [43_u8, 55, 69, 81] {
            let mut strings = played(Model::Violin, 1.0);
            strings.set_param("players", players);
            let audio = strings.render(96_000, &[on(pitch, 0.8)]);
            assert!(audio.iter().all(|sample| sample.is_finite()));
            let cents = peak_cents(&audio[1_200..12_000], 48_000.0, hz(pitch), -400..=400);
            // One player is on the note; four are each a few cents from it, and beat.
            let near = if players > 1.0 { 25.0 } else { 8.0 };
            assert!(cents.abs() < near, "{players} on {pitch}: {cents} cents");
            // Nobody is feeding it: a second on, it is far below where it began.
            let fell = level_db(&audio[..4_800]) - level_db(&audio[48_000..52_800]);
            assert!(fell > 15.0, "{players} on {pitch}: fell {fell} dB");
            // And a note that has died away is over, key or no key.
            strings.render(480_000, &[]);
            assert_eq!(strings.instrument.active_voices(), 0, "{pitch} never ended");
        }
    }
}

#[test]
fn a_plucked_string_starts_about_as_loud_as_a_bowed_one_and_cannot_be_slurred() {
    for pitch in [48_u8, 67, 79] {
        let bowed = played(Model::Violin, 0.0).render(48_000, &[on(pitch, 0.8)]);
        let plucked = played(Model::Violin, 1.0).render(48_000, &[on(pitch, 0.8)]);
        let level = 20.0 * f64::from(peak(&plucked) / peak(&bowed[24_000..])).log10();
        assert!(
            (-8.0..8.0).contains(&level),
            "{pitch}: plucked {level} dB over bowed"
        );
    }
    // With legato on, a second key over the first is a second pluck, not a moved finger.
    let mut strings = played(Model::Violin, 1.0);
    strings.set_param("legato", 1.0);
    strings.set_param("players", 1.0);
    strings.render(4_800, &[on(67, 0.8)]);
    let second = strings.render(4_800, &[on(74, 0.8)]);
    assert!(peak(&second[..960]) > peak(&second[3_840..]) * 0.8);
    assert!(goertzel(&second, 48_000.0, hz(74)) > goertzel(&second, 48_000.0, hz(67)) * 0.5);
}

#[test]
fn a_tremolo_is_the_bow_going_back_and_forth_a_dozen_times_a_second() {
    for pitch in [60_u8, 67, 76] {
        let take = |technique: f32| {
            let mut strings = played(Model::Violin, technique);
            strings.set_param("players", 1.0);
            strings.set_param("vibrato", 0.0);
            strings.render(96_000, &[on(pitch, 0.8)])[24_000..].to_vec()
        };
        let (drawn, trembled) = (take(0.0), take(2.0));
        assert!(trembled.iter().all(|sample| sample.is_finite()));
        assert!(swing_db(&drawn) < 2.5, "{pitch}: {} dB", swing_db(&drawn));
        assert!(
            swing_db(&trembled) > 3.5,
            "{pitch}: {} dB",
            swing_db(&trembled)
        );
        let strokes = swing_hz(&trembled);
        assert!((8..=14).contains(&strokes), "{pitch}: {strokes} a second");
        // It is the same note at much the same loudness.
        let level = level_db(&trembled) - level_db(&drawn);
        assert!(level.abs() < 6.0, "{pitch}: {level} dB");
    }
}

#[test]
fn a_harmonic_is_the_octave_of_a_string_twice_as_long() {
    for (model, technique, pitch) in [
        (Model::Guitar, 1.0, 64_u8),
        (Model::Guitar, 1.0, 76),
        (Model::ElectricGuitar, 1.0, 64),
        (Model::ElectricGuitar, 1.0, 71),
        (Model::Bass, 4.0, 52),
        (Model::Bass, 4.0, 43),
    ] {
        let plucked = played(model, 0.0).render(72_000, &[on(pitch, 0.8)]);
        let harmonic = played(model, technique).render(72_000, &[on(pitch, 0.8)]);
        // It sounds at the note that was asked for.
        let cents = peak_cents(&harmonic[9_600..48_000], 48_000.0, hz(pitch), -300..=300);
        assert!(cents.abs() < 6.0, "{model:?} {pitch}: {cents} cents");
        // The long string's own fundamental, an octave under, is gone once the finger has
        // been on it: at first it is there, and a tenth of a second on it is not.
        let under = |audio: &[f32]| {
            20.0 * (partial(audio, hz(pitch) * 0.5, 1.0) / partial(audio, hz(pitch), 1.0)).log10()
        };
        assert!(
            under(&harmonic[4_800..24_000]) < -30.0,
            "{model:?} {pitch}: the octave under at {} dB",
            under(&harmonic[4_800..24_000])
        );
        // A longer string rings longer, and with less above the note than a fretted one.
        let fall =
            |audio: &[f32]| level_db(&audio[4_800..9_600]) - level_db(&audio[62_400..67_200]);
        assert!(
            fall(&harmonic) < fall(&plucked) - 0.5,
            "{model:?} {pitch}: fell {} dB against {}",
            fall(&harmonic),
            fall(&plucked)
        );
        let top = |audio: &[f32]| {
            let upper: f64 = (3..=8)
                .map(|n| partial(&audio[4_800..24_000], hz(pitch), f64::from(n)))
                .sum();
            20.0 * (upper / partial(&audio[4_800..24_000], hz(pitch), 1.0)).log10()
        };
        assert!(
            top(&harmonic) < top(&plucked) - 1.0,
            "{model:?} {pitch}: top {} dB against {}",
            top(&harmonic),
            top(&plucked)
        );
    }
}

#[test]
fn a_fluttered_tongue_rolls_two_dozen_times_a_second_and_the_note_stays_where_it_was() {
    for (model, pitch) in [
        (Model::Clarinet, 69_u8),
        (Model::Clarinet, 81),
        (Model::TinWhistle, 86),
        (Model::TinWhistle, 93),
        // A brass note's buzz is a train of sharp puffs, and five milliseconds of it are as
        // loud as the puffs they hold: G4 and G5 put a whole number of them in each.
        (Model::Brass, 67),
        (Model::Brass, 79),
    ] {
        let take = |technique: f32| {
            let mut wind = played(model, technique);
            wind.set_param("vibrato", 0.0);
            wind.render(72_000, &[on(pitch, 0.8)])[24_000..].to_vec()
        };
        let (plain, fluttered) = (take(0.0), take(1.0));
        assert!(
            swing_db(&plain) < 1.0,
            "{model:?} {pitch}: {}",
            swing_db(&plain)
        );
        assert!(
            swing_db(&fluttered) > 2.0,
            "{model:?} {pitch}: swings {} dB",
            swing_db(&fluttered)
        );
        let cents = peak_cents(&fluttered, 48_000.0, hz(pitch), -300..=300);
        let plain_cents = peak_cents(&plain, 48_000.0, hz(pitch), -300..=300);
        assert!(
            (cents - plain_cents).abs() < 8.0,
            "{model:?} {pitch}: {cents} cents against {plain_cents}"
        );
        // The roll takes some of the breath, and not most of it.
        let level = level_db(&fluttered) - level_db(&plain);
        assert!(
            (-9.0..0.5).contains(&level),
            "{model:?} {pitch}: {level} dB"
        );
        let strongest = swing_hz(&fluttered);
        assert!(
            (20..=28).contains(&strongest),
            "{model:?} {pitch}: {strongest} Hz"
        );
    }
}

#[test]
fn the_new_techniques_do_not_depend_on_the_block_size_and_allocate_nothing() {
    for (model, technique) in [
        (Model::Violin, 1.0),
        (Model::Violin, 2.0),
        (Model::Guitar, 1.0),
        (Model::ElectricGuitar, 1.0),
        (Model::Bass, 4.0),
        (Model::Clarinet, 1.0),
        (Model::TinWhistle, 1.0),
        (Model::Brass, 1.0),
    ] {
        let pitch = home(model);
        let events = [
            NoteEvent::note_on(73, pitch, 0.8),
            NoteEvent::note_on(1_301, pitch + 4, 0.5),
            NoteEvent::PitchBend {
                frame: 2_000,
                semitones: 0.5,
            },
            off(3_333, pitch),
            off(6_100, pitch + 4),
        ];
        let mut renders = Vec::new();
        for block in [64, 250, 1_024] {
            let mut instrument = Physical::new(model);
            instrument.set_param_by_key("technique", technique);
            let mut rig = Rig::new(Box::new(instrument), 48_000.0, block, 2);
            renders.push(rig.render_stereo(12_000, &events));
        }
        assert!(peak(&renders[0].0) > 0.0, "{model:?} {technique}");
        assert!(
            renders[0] == renders[1] && renders[0] == renders[2],
            "{model:?} {technique}: blocks differ"
        );

        let mut instrument = Physical::new(model);
        instrument.set_param_by_key("technique", technique);
        instrument.prepare(&PrepareContext::new(48_000.0, 256, 2));
        let mut buffer = AudioBuffer::stereo(256, 48_000.0);
        let ctx = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
        let quality = instrument.descriptor_for_key("quality").unwrap().id;
        let allocations = count_allocations(|| {
            for pitch in 30..90_u8 {
                instrument.set_param(quality, f32::from(pitch % 3));
                instrument.process(&[on(pitch, 0.9), off(200, pitch - 2)], &mut buffer, &ctx);
            }
            instrument.process(&[NoteEvent::AllSoundOff { frame: 50 }], &mut buffer, &ctx);
            instrument.reset();
        });
        assert_eq!(allocations, 0, "{model:?} {technique}");
    }
}

#[test]
fn a_reeds_breath_stays_under_its_tone_and_a_soft_note_speaks_without_a_hiss_or_an_accent() {
    use crate::test_support::band_amplitude;
    // Between the harmonics, well above where a clarinet's own have faded: what is there
    // is the breath. An even hiss stood ten decibels and more over this.
    for (pitch, velocity) in [(69_u8, 0.65_f32), (57, 0.35), (53, 0.05), (74, 0.95)] {
        let mut reed = rig(Model::Clarinet, 48_000.0);
        let audio = reed.render(96_000, &[on(pitch, velocity)]);
        let settled = &audio[48_000..];
        let tone = goertzel(settled, 48_000.0, hz(pitch));
        let breath = 20.0 * (band_amplitude(settled, 48_000.0, 7_000.0) / tone).log10();
        assert!(
            breath < -72.0,
            "{pitch} at {velocity}: breath at {breath} dB"
        );
        // Even the softest note has all but spoken three tenths of a second in, and none
        // begins louder than it goes on.
        let held = level_db(settled);
        let early = level_db(&audio[14_400..19_200]);
        assert!(
            early > held - 6.0,
            "{pitch} at {velocity}: {early} dB, then {held}"
        );
        let loudest = audio[..48_000]
            .chunks(4_800)
            .map(level_db)
            .fold(f64::MIN, f64::max);
        assert!(
            loudest < held + 2.5,
            "{pitch} at {velocity}: began {loudest} dB, held {held}"
        );
    }
}

#[test]
fn a_grands_lowest_strings_keep_their_top() {
    use crate::test_support::band_amplitude;
    // Ninety-six partials of the lowest C end at 3.1 kHz. A wound string does not, and its
    // hammer strikes one string, which leaves room for half as many partials again.
    let mut piano = whole(Model::Piano);
    let audio = piano.render(24_000, &[on(24, 0.9)]);
    let blow = &audio[480..9_600];
    let (under, over) = (
        band_amplitude(blow, 48_000.0, 2_500.0),
        band_amplitude(blow, 48_000.0, 4_200.0),
    );
    let top = 20.0 * (over / under).log10();
    assert!(top > -20.0, "above the old last partial: {top} dB");
    // And the bass is voiced brighter than it was: G1's blow has its share of two to four
    // kilohertz, where a recorded grand's has a good deal.
    piano.instrument.reset();
    let audio = piano.render(24_000, &[on(31, 0.9)]);
    let blow = &audio[480..4_800];
    let bright = 20.0
        * (band_amplitude(blow, 48_000.0, 2_800.0) / band_amplitude(blow, 48_000.0, 400.0)).log10();
    assert!(bright > -31.0, "G1's blow from 2 to 4 kHz: {bright} dB");
}

#[test]
fn a_soft_breath_still_sounds_its_note_under_a_rolled_tongue() {
    // A soft breath has little over what the note needs, and a tongue that took the pressure
    // as far under as it does a loud one's left only the breath: a fifth of the level.
    for pitch in [50_u8, 62, 74, 86] {
        let take = |technique: f32| {
            let mut clarinet = played(Model::Clarinet, technique);
            clarinet.set_param("vibrato", 0.0);
            clarinet.render(72_000, &[on(pitch, 0.15)])[36_000..].to_vec()
        };
        let (plain, fluttered) = (take(0.0), take(1.0));
        let level = level_db(&fluttered) - level_db(&plain);
        assert!((-10.0..0.5).contains(&level), "{pitch}: {level} dB");
        let cents = peak_cents(&fluttered, 48_000.0, hz(pitch), -300..=300);
        assert!(cents.abs() < 5.0, "{pitch}: {cents} cents");
    }
}

#[test]
fn the_winds_stay_on_the_note_however_hard_they_are_blown() {
    // A reed blown hard is shut for part of every period and went eight cents sharp; a
    // whistle's pipe was a little long for its top notes, and they were four cents flat.
    for (model, pitches) in [
        (Model::Clarinet, [50_u8, 62, 74, 86]),
        (Model::TinWhistle, [62, 74, 86, 105]),
        (Model::Brass, [55, 64, 72, 82]),
    ] {
        for pressure in [0.2, 0.5, 0.9] {
            for pitch in pitches {
                let mut wind = rig(model, 48_000.0);
                wind.set_param("vibrato", 0.0);
                wind.set_param("pressure", pressure);
                let audio = wind.render(60_000, &[on(pitch, 0.9)]);
                let cents = peak_cents(&audio[24_000..], 48_000.0, hz(pitch), -150..=150);
                assert!(
                    cents.abs() < 2.5,
                    "{model:?} {pitch} at pressure {pressure}: {cents} cents"
                );
            }
        }
    }
}

#[test]
fn a_harmonic_is_evened_against_the_note_that_is_heard() {
    // The hand learns which notes the box favours from what it hears, and a harmonic is
    // heard an octave over the string it is played on. Evened against the string's own note,
    // one harmonic stood seven decibels under the two beside it.
    let level = |pitch: u8| {
        let mut guitar = played(Model::Guitar, 1.0);
        level_db(&guitar.render(48_000, &[on(pitch, 0.6)]))
    };
    let levels: Vec<f64> = (52_u8..=84).map(level).collect();
    for (index, pitch) in (53_u8..=83).enumerate() {
        let apart = levels[index + 1] - (levels[index] + levels[index + 2]) / 2.0;
        assert!(
            apart.abs() < 6.0,
            "{pitch}: {apart:+.1} dB from its neighbours"
        );
    }
}

#[test]
fn a_bow_asked_nearer_the_bridge_than_the_string_holds_leans_and_stays_on_the_note() {
    // Three times nearer the bridge than a bow is usually drawn, no force this string takes
    // holds Helmholtz's motion: drawn there, these notes were 7 to 48 cents flat and hoarse.
    // The bow goes as near as the string holds and leans for the rest.
    let near = |position: f32, brightness: Option<f32>, pitch: u8| {
        let mut violin = rig(Model::Violin, 48_000.0);
        violin.set_param("players", 1.0);
        violin.set_param("vibrato", 0.0);
        violin.set_param("position", position);
        let mut events = vec![on(pitch, 0.6)];
        if let Some(value) = brightness {
            events.insert(0, controller(0, 74, value));
        }
        violin.render(84_000, &events)[36_000..].to_vec()
    };
    for pitch in [38_u8, 46, 57, 62, 64, 66, 76] {
        let usual = near(0.09, None, pitch);
        let cents = peak_cents(&usual, 48_000.0, hz(pitch), -600..=600);
        assert!(
            cents.abs() < 5.0,
            "{pitch} at the usual place: {cents} cents"
        );
        // The bow moved by the control, and by controller 74 from where it usually is.
        for (position, brightness) in [(0.03, None), (0.05, None), (0.09, Some(1.0))] {
            let audio = near(position, brightness, pitch);
            let cents = peak_cents(&audio, 48_000.0, hz(pitch), -600..=600);
            assert!(
                cents.abs() < 8.0,
                "{pitch} at {position} ({brightness:?}): {cents} cents"
            );
            // Heard as evenly as a bow in its usual place, which is where it is.
            let level = level_db(&audio) - level_db(&usual);
            assert!(
                level.abs() < 3.0,
                "{pitch} at {position} ({brightness:?}): {level:+.1} dB"
            );
        }
    }
}

/// The brass instruments, each across its own range: (instrument, lowest, highest).
const BRASS: [(f32, u8, u8); 4] = [(0.0, 55, 82), (1.0, 36, 77), (2.0, 40, 72), (3.0, 28, 62)];

fn brass(instrument: f32) -> Rig {
    let mut rig = rig(Model::Brass, 48_000.0);
    rig.set_param("instrument", instrument);
    rig
}

/// Where the weight of the first sixteen harmonics of `samples` lies, as a harmonic number.
fn harmonic_centroid(samples: &[f32], fundamental: f64) -> f64 {
    let (mut weighted, mut total) = (0.0, 0.0);
    for n in 1..=16 {
        let power = windowed(samples, 48_000.0, fundamental * f64::from(n)).powi(2);
        weighted += f64::from(n) * power;
        total += power;
    }
    weighted / total.max(1.0e-30)
}

#[test]
fn every_brass_instrument_speaks_in_tune_across_its_range_at_every_dynamic() {
    for (instrument, low, high) in BRASS {
        for pitch in (low..=high).step_by(3) {
            for velocity in [0.2_f32, 0.6, 1.0] {
                let mut horn = brass(instrument);
                let audio = horn.render(48_000, &[on(pitch, velocity)]);
                let steady = &audio[19_200..45_600];
                assert!(
                    level_db(steady) > -70.0,
                    "instrument {instrument} MIDI {pitch} at {velocity} does not speak"
                );
                let cents = peak_cents(steady, 48_000.0, hz(pitch), -300..=300);
                assert!(
                    cents.abs() < 5.0,
                    "instrument {instrument} MIDI {pitch} at {velocity}: {cents} cents"
                );
            }
        }
    }
}

#[test]
fn louder_brass_is_brighter_and_within_a_players_range_of_level() {
    // A *fortissimo* reaches the bell steeper than a *pianissimo* leaves the lips: its upper
    // harmonics are stronger, not only its level. And a player's dynamics span ten to twenty
    // decibels or so, as the Iowa players' do.
    // Each on a note of its own recordings' range.
    for (instrument, pitch) in [(0.0, 66), (1.0, 66), (2.0, 54), (3.0, 42)] {
        let take = |velocity: f32| {
            let mut horn = brass(instrument);
            horn.render(48_000, &[on(pitch, velocity)])[19_200..45_600].to_vec()
        };
        let (soft, loud) = (take(0.25), take(1.0));
        let span = level_db(&loud) - level_db(&soft);
        assert!(
            (10.0..30.0).contains(&span),
            "instrument {instrument}: {span} dB from pp to ff"
        );
        let (dull, bright) = (
            harmonic_centroid(&soft, hz(pitch)),
            harmonic_centroid(&loud, hz(pitch)),
        );
        // The recordings brighten more than this from *pianissimo* to *fortissimo* on the
        // horn and the trombone (by half and by more than double); the model holds the
        // direction, not yet the whole of it.
        assert!(
            bright > dull * 1.05,
            "instrument {instrument}: centroid {dull} at pp, {bright} at ff"
        );
    }
}

#[test]
fn a_brass_note_speaks_within_a_tenth_of_a_second() {
    for (instrument, low, high) in BRASS {
        for pitch in [low, (low + high) / 2, high] {
            let mut horn = brass(instrument);
            let audio = horn.render(48_000, &[on(pitch, 0.6)]);
            let settled = rms(&audio[24_000..43_200]);
            let reached = audio
                .chunks(240)
                .position(|chunk| rms(chunk) > settled * 0.5)
                .map_or(f32::INFINITY, |chunk| chunk as f32 * 5.0);
            assert!(
                reached < 100.0,
                "instrument {instrument} MIDI {pitch}: half its level after {reached} ms"
            );
        }
    }
}

#[test]
fn the_brass_instruments_sound_unlike_one_another_on_the_same_note() {
    // On the one note they share, the horn's hand and wide bell leave it the darkest and the
    // trumpet's small bright bell the brightest.
    let centroid = |instrument: f32| {
        let mut horn = brass(instrument);
        let audio = horn.render(48_000, &[on(55, 0.8)]);
        harmonic_centroid(&audio[19_200..45_600], hz(55))
    };
    let (trumpet, horn, trombone) = (centroid(0.0), centroid(1.0), centroid(2.0));
    assert!(
        horn < trombone && trombone < trumpet,
        "horn {horn}, trombone {trombone}, trumpet {trumpet}"
    );
}
