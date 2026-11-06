//! Release-only measurements for the offline analysis entry points.

use std::hint::black_box;
use std::time::Instant;

use auris_core::AudioBuffer;
use std::sync::atomic::AtomicBool;

const RATE: f64 = 48_000.0;
const TRIALS: usize = 5;

fn fixture(seconds: usize) -> AudioBuffer {
    let frames = seconds * RATE as usize;
    let left: Vec<f32> = (0..frames)
        .map(|frame| {
            let t = frame as f64 / RATE;
            (0.34 * (std::f64::consts::TAU * 220.0 * t).sin()
                + 0.12 * (std::f64::consts::TAU * 1_000.0 * t).sin()) as f32
        })
        .collect();
    let right: Vec<f32> = (0..frames)
        .map(|frame| {
            let t = frame as f64 / RATE;
            (0.29 * (std::f64::consts::TAU * 220.0 * t).sin()
                + 0.08 * (std::f64::consts::TAU * 3_200.0 * t).sin()) as f32
        })
        .collect();
    AudioBuffer::from_planar(vec![left, right], RATE).expect("stereo fixture")
}

fn median_nanos(mut values: Vec<u128>) -> u128 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn measure<T>(label: &str, input: &AudioBuffer, mut operation: impl FnMut(&AudioBuffer) -> T) {
    black_box(operation(input));
    let mut timings = Vec::with_capacity(TRIALS);
    for _ in 0..TRIALS {
        let start = Instant::now();
        black_box(operation(input));
        timings.push(start.elapsed().as_nanos());
    }
    eprintln!(
        "auris-dsp {label}: median {} ms",
        median_nanos(timings) as f64 / 1e6
    );
}

fn benchmark(seconds: usize) {
    let audio = fixture(seconds);
    measure(&format!("{seconds}s drum_analysis"), &audio, |audio| {
        crate::drum_analysis::analyze_drum_audio(audio)
    });
    measure(&format!("{seconds}s timbre_features"), &audio, |audio| {
        crate::timbre::timbre_features(audio, 0.6)
    });
    measure(&format!("{seconds}s spectrogram"), &audio, |audio| {
        crate::Spectrogram::analyse(audio)
    });
    measure(&format!("{seconds}s integrated_lufs"), &audio, |audio| {
        crate::integrated_lufs(audio)
    });
    let cancel = AtomicBool::new(false);
    measure(&format!("{seconds}s mel"), &audio, |audio| {
        crate::mel::analyse(audio, &cancel)
    });
}

#[test]
#[ignore = "release-only performance measurement"]
fn offline_analysis_benchmark_one_second_stereo() {
    benchmark(1);
}

#[test]
#[ignore = "release-only performance measurement"]
fn offline_analysis_benchmark_ten_second_stereo() {
    benchmark(10);
}
