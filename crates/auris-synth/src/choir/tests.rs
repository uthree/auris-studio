use super::*;
use crate::test_support::{band_amplitude, count_allocations, goertzel, peak, rms};

fn note(frame: u32, pitch: u8, velocity: f32) -> NoteEvent {
    NoteEvent::NoteOn {
        frame,
        pitch,
        velocity,
    }
}

fn render(
    choir: &mut Choir,
    frames: usize,
    block: usize,
    channels: usize,
    events: &[NoteEvent],
) -> Vec<Vec<f32>> {
    let mut result = vec![Vec::with_capacity(frames); channels];
    let mut buffer = AudioBuffer::new(channels, block, 48_000.0);
    for start in (0..frames).step_by(block) {
        let count = block.min(frames - start);
        buffer.set_frame_count(count);
        for channel in buffer.channels_mut() {
            channel.fill(99.0);
        }
        let events: Vec<_> = events
            .iter()
            .filter(|event| (start..start + count).contains(&(event.frame() as usize)))
            .map(|event| event.with_frame(event.frame() - start as u32))
            .collect();
        choir.process(
            &events,
            &mut buffer,
            &ProcessContext::realtime(48_000.0, count, start as u64, 120.0, true),
        );
        for (index, channel) in result.iter_mut().enumerate() {
            channel.extend_from_slice(buffer.channel(index));
        }
    }
    result
}

fn prepared() -> Choir {
    let mut choir = Choir::new();
    choir.prepare(&PrepareContext::new(48_000.0, 512, 2));
    choir
}

fn steady_vowel(vowel: f32) -> Vec<f32> {
    let mut choir = prepared();
    choir.set_param_by_key("vowel", vowel);
    choir.set_param_by_key("ensemble", 0.0);
    choir.set_param_by_key("breath", 0.0);
    choir.set_param_by_key("vibrato", 0.0);
    render(&mut choir, 48_000, 512, 1, &[note(0, 45, 0.8)]).remove(0)
}

#[test]
fn vowels_change_the_spectrum_while_preserving_the_played_pitch() {
    let rounded = steady_vowel(0.0);
    let open = steady_vowel(1.0);
    let front = steady_vowel(2.0);
    let energy = |samples: &[f32], hz| band_amplitude(&samples[24_000..], 48_000.0, hz);
    let rounded_ratio = energy(&rounded, 880.0) / energy(&rounded, 330.0);
    let open_ratio = energy(&open, 880.0) / energy(&open, 330.0);
    assert!(
        open_ratio > rounded_ratio * 2.0,
        "oo={rounded_ratio}, ah={open_ratio}"
    );
    assert!(
        energy(&front, 1_980.0) / energy(&front, 880.0)
            > energy(&open, 1_980.0) / energy(&open, 880.0)
    );
    for samples in [&rounded, &open, &front] {
        let fundamental = goertzel(&samples[24_000..], 48_000.0, 110.0);
        let mistuned = goertzel(&samples[24_000..], 48_000.0, 116.54);
        assert!(
            fundamental > mistuned * 8.0,
            "fundamental={fundamental}, mistuned={mistuned}"
        );
    }
}

#[test]
fn chords_have_independent_notes_and_audible_bounded_levels() {
    let mut choir = prepared();
    let audio = render(
        &mut choir,
        48_000,
        512,
        2,
        &[
            note(0, 48, 0.8),
            note(0, 55, 0.8),
            note(0, 60, 0.8),
            note(0, 64, 0.8),
        ],
    );
    assert_eq!(choir.active_voices(), 4);
    for channel in audio {
        assert!(
            rms(&channel[24_000..]) > 0.01,
            "rms={}",
            rms(&channel[24_000..])
        );
        assert!(peak(&channel) < 1.0, "peak={}", peak(&channel));
    }
}

#[test]
fn every_vowel_keeps_default_chords_audible_and_below_full_scale() {
    let mut levels = Vec::new();
    for vowel in [0.0, 0.5, 1.0, 1.5, 2.0] {
        let mut choir = prepared();
        choir.set_param_by_key("vowel", vowel);
        choir.reset();
        let audio = render(
            &mut choir,
            48_000,
            512,
            2,
            &[
                note(0, 48, 0.8),
                note(0, 55, 0.8),
                note(0, 60, 0.8),
                note(0, 64, 0.8),
            ],
        );
        let level = peak(&audio[0]).max(peak(&audio[1]));
        assert!((0.01..1.0).contains(&level), "vowel={vowel}, peak={level}");
        levels.push(level);
    }
    let largest = levels.iter().copied().fold(0.0_f32, f32::max);
    let smallest = levels.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(largest / smallest < 8.0, "vowel peaks={levels:?}");
}

#[test]
fn width_controls_stereo_and_mono_is_the_same_downmix() {
    let mut choir = prepared();
    let stereo = render(&mut choir, 12_000, 256, 3, &[note(13, 60, 0.8)]);
    let difference: Vec<_> = stereo[0]
        .iter()
        .zip(&stereo[1])
        .map(|(l, r)| l - r)
        .collect();
    assert!(
        rms(&difference) > 0.005,
        "stereo difference={}",
        rms(&difference)
    );
    choir.reset();
    let mono = render(&mut choir, 12_000, 256, 1, &[note(13, 60, 0.8)]);
    for (index, sample) in mono[0].iter().enumerate() {
        let mix = (stereo[0][index] + stereo[1][index]) * 0.5;
        assert!((sample - mix).abs() < 1e-6);
        assert_eq!(stereo[2][index], mix);
    }
    choir.set_param_by_key("width", 0.0);
    choir.reset();
    let centered = render(&mut choir, 12_000, 256, 2, &[note(0, 60, 0.8)]);
    assert_eq!(centered[0], centered[1]);
}

#[test]
fn events_releases_and_reset_are_independent_of_block_size() {
    let events = [
        note(13, 48, 0.8),
        note(119, 55, 0.7),
        NoteEvent::PitchBend {
            frame: 937,
            semitones: 2.0,
        },
        NoteEvent::NoteOff {
            frame: 2_931,
            pitch: 48,
        },
        NoteEvent::AllSoundOff { frame: 4_173 },
    ];
    let mut first = prepared();
    let mut second = prepared();
    let a = render(&mut first, 5_000, 512, 2, &events);
    let b = render(&mut second, 5_000, 1, 2, &events);
    assert_eq!(a, b);
    assert!(a[0][..13].iter().all(|sample| *sample == 0.0));
    assert!(a[0][4_800..].iter().all(|sample| *sample == 0.0));
    assert_eq!(first.active_voices(), 0);
    first.reset();
    let reset = render(&mut first, 5_000, 127, 2, &events);
    assert_eq!(a, reset);
}

#[test]
fn release_frees_only_the_oldest_overlapping_note() {
    let mut choir = prepared();
    choir.set_param_by_key("release", 0.02);
    render(
        &mut choir,
        2_000,
        512,
        1,
        &[
            note(0, 60, 0.8),
            note(123, 60, 0.8),
            NoteEvent::NoteOff {
                frame: 499,
                pitch: 60,
            },
        ],
    );
    assert_eq!(choir.active_voices(), 1);
    let released = render(
        &mut choir,
        3_000,
        512,
        1,
        &[NoteEvent::AllNotesOff { frame: 0 }],
    );
    assert_eq!(choir.active_voices(), 0);
    assert_eq!(peak(&released[0][2_000..]), 0.0);
}

#[test]
fn velocity_volume_expression_and_bend_affect_sounding_notes() {
    let mut choir = prepared();
    choir.set_param_by_key("ensemble", 0.0);
    choir.set_param_by_key("breath", 0.0);
    choir.set_param_by_key("vibrato", 0.0);
    let full = render(&mut choir, 24_000, 512, 1, &[note(0, 57, 0.8)]);
    choir.reset();
    let soft = render(&mut choir, 24_000, 512, 1, &[note(0, 57, 0.2)]);
    assert!((rms(&soft[0][12_000..]) / rms(&full[0][12_000..]) - 0.25).abs() < 0.001);
    choir.reset();
    let ridden = render(
        &mut choir,
        24_000,
        512,
        1,
        &[
            note(0, 57, 0.8),
            NoteEvent::Controller {
                frame: 0,
                number: 7,
                value: 0.5,
            },
            NoteEvent::Controller {
                frame: 0,
                number: CC_EXPRESSION,
                value: 0.5,
            },
        ],
    );
    assert!((rms(&ridden[0][12_000..]) / rms(&full[0][12_000..]) - 0.125).abs() < 0.001);
    choir.reset();
    let bent = render(
        &mut choir,
        24_000,
        512,
        1,
        &[
            note(0, 57, 0.8),
            NoteEvent::PitchBend {
                frame: 12_000,
                semitones: 12.0,
            },
        ],
    );
    assert!(
        goertzel(&bent[0][18_000..], 48_000.0, 440.0)
            > goertzel(&bent[0][18_000..], 48_000.0, 220.0) * 8.0
    );
}

#[test]
fn automation_controllers_and_voice_stealing_allocate_nothing() {
    let mut choir = prepared();
    let mut buffer = AudioBuffer::stereo(512, 48_000.0);
    let context = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
    let events: Vec<_> = (0..32).map(|i| note(i * 8, 48 + i as u8, 0.8)).collect();
    let allocations = count_allocations(|| {
        choir.process(&events, &mut buffer, &context);
        choir.set_param_by_key("vowel", 0.0);
        choir.set_param_by_key("voice_size", 1.0);
        choir.set_param_by_key("ensemble", 1.0);
        choir.process(
            &[
                NoteEvent::Controller {
                    frame: 13,
                    number: CC_MODULATION,
                    value: 1.0,
                },
                NoteEvent::AllSoundOff { frame: 233 },
            ],
            &mut buffer,
            &context,
        );
    });
    assert_eq!(allocations, 0);
    assert!(
        buffer
            .channels()
            .iter()
            .flatten()
            .all(|sample| sample.is_finite())
    );
}

#[test]
fn extreme_parameters_events_and_sample_rates_remain_finite() {
    for rate in [8_000.0, 44_100.0, 96_000.0, 192_000.0, f64::MAX] {
        let mut choir = Choir::new();
        choir.prepare(&PrepareContext::new(rate, 512, 2));
        let mut buffer = AudioBuffer::stereo(512, rate);
        let context = ProcessContext::realtime(rate, 512, 0, 120.0, true);
        for value in [0.0, 1.0, 2.0, f32::NAN, f32::INFINITY] {
            for param in [
                P_VOWEL, P_SIZE, P_ENSEMBLE, P_BREATH, P_TONE, P_WIDTH, P_VIBRATO,
            ] {
                choir.set_param(ParamId(param), value);
            }
            for pitch in [0, 60, 127] {
                choir.process(
                    &[
                        note(0, pitch, f32::NAN),
                        NoteEvent::PitchBend {
                            frame: 7,
                            semitones: f32::INFINITY,
                        },
                    ],
                    &mut buffer,
                    &context,
                );
                assert!(
                    buffer
                        .channels()
                        .iter()
                        .flatten()
                        .all(|sample| sample.is_finite())
                );
                assert!(buffer.peak() < 20.0, "rate={rate}, peak={}", buffer.peak());
            }
        }
    }
}

#[cfg(feature = "choir-calibration")]
#[test]
fn offline_tract_overrides_reject_invalid_geometry_without_changing_the_instrument() {
    let mut choir = prepared();
    let original = choir.calibration;
    for invalid in [0.0, -1.0, 11.0, f32::NAN, f32::INFINITY] {
        let mut areas = tract::AREAS;
        areas[1][3] = invalid;
        assert!(!choir.set_calibration_tract(areas, tract::RADIATION_HZ, tract::OUTPUT_GAINS));
        assert_eq!(choir.calibration, original);
    }
    for invalid in [0.0, 4_001.0, f32::NAN, f32::INFINITY] {
        assert!(!choir.set_calibration_tract(tract::AREAS, invalid, tract::OUTPUT_GAINS));
        assert_eq!(choir.calibration, original);
    }
    let mut areas = tract::AREAS;
    areas[1][3] *= 1.5;
    assert!(!choir.set_calibration_tract(tract::AREAS, 500.0, [f32::NAN; 3]));
    assert_eq!(choir.calibration, original);
    assert!(choir.set_calibration_tract(areas, 500.0, [1.0; 3]));
    assert_eq!(choir.calibration, (areas, 500.0, [1.0; 3]));
}

#[test]
fn the_formant_table_is_the_profiles_own_first_resonance() {
    for (index, expected) in tract::FIRST_FORMANT.iter().enumerate() {
        let vowel = index as f32 * 0.5;
        let mut tract = Tract::new(48_000.0);
        tract.configure(vowel, 0.17, 48_000.0);
        tract.reset();
        let impulse: Vec<f32> = (0..16_384)
            .map(|i| {
                let back = tract.incoming();
                tract.next(if i == 0 { 1.0 } else { back })
            })
            .collect();
        let spectrum: Vec<(f32, f64)> = (120..1_600)
            .step_by(5)
            .map(|f| (f as f32, goertzel(&impulse, 48_000.0, f64::from(f))))
            .collect();
        let first = spectrum
            .windows(3)
            .find(|w| w[1].1 > w[0].1 && w[1].1 >= w[2].1)
            .map(|w| w[1].0)
            .unwrap();
        assert!(
            (first / expected - 1.0).abs() < 0.03,
            "vowel={vowel}: first resonance {first} Hz, table {expected} Hz"
        );
    }
}

#[test]
fn a_note_above_the_first_resonance_is_sung_on_an_opened_vowel() {
    // Low notes keep the vowel asked for.
    assert_eq!(tract::tuned(0.0, 0.17, 150.0), (0.0, 0.17));
    // A soprano's G5 on "oo" opens towards "ah" until the resonance clears the note.
    let (vowel, length) = tract::tuned(0.0, 0.17, 784.0 / 1.6);
    assert!(
        vowel > 0.0 && vowel <= 1.0 && length == 0.17,
        "{vowel}, {length}"
    );
    assert!(tract::first_formant(vowel, length) >= tract::FORMANT_MARGIN * 784.0 / 1.6);
    // Above "ah" itself the tract is shortened, never past seven tenths.
    let (vowel, length) = tract::tuned(2.0, 0.17, 1_000.0);
    assert_eq!(vowel, 1.0);
    assert!((0.119..0.17).contains(&length), "{length}");
}

/// Amplitude at `hz` through a Hann window, so a low note's harmonics do not leak into it.
fn windowed(samples: &[f32], hz: f64) -> f64 {
    let count = samples.len() as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for (index, sample) in samples.iter().enumerate() {
        let t = index as f64;
        let weight = 0.5 - 0.5 * (std::f64::consts::TAU * t / count).cos();
        let angle = std::f64::consts::TAU * hz * t / 48_000.0;
        re += f64::from(*sample) * weight * angle.cos();
        im += f64::from(*sample) * weight * angle.sin();
    }
    (re * re + im * im).sqrt() * 4.0 / count
}

#[test]
fn the_folds_find_every_note_of_a_choir_in_every_vowel() {
    for vowel in [0.0, 1.0, 2.0] {
        for pitch in (36..=81).step_by(3) {
            let mut choir = prepared();
            choir.set_param_by_key("vowel", vowel);
            choir.set_param_by_key("ensemble", 0.0);
            choir.set_param_by_key("vibrato", 0.0);
            choir.set_param_by_key("breath", 0.0);
            let out = render(&mut choir, 72_000, 512, 1, &[note(0, pitch, 0.8)]).remove(0);
            let tail = &out[24_000..];
            let expected = f64::from(pitch_to_hz(f32::from(pitch)));
            let (cents, strength) = (-60_i32..=60)
                .map(|cents| {
                    let hz = expected * (f64::from(cents) / 1200.0).exp2();
                    (cents, windowed(tail, hz))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            let subharmonic = windowed(tail, expected / 2.0);
            assert!(
                cents.abs() <= 8 && strength > 4.0 * subharmonic && rms(tail) > 0.005,
                "vowel={vowel}, pitch={pitch}, cents={cents}, fundamental={strength}, \
                 half={subharmonic}, rms={}",
                rms(tail)
            );
        }
    }
}
