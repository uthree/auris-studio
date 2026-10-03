use super::*;

fn render(rate: f64, frequency: f64, drive: f32, cabinet: f32) -> Vec<f32> {
    let mut amp = GuitarAmp::new();
    amp.set_param_by_key("drive_db", drive);
    amp.set_param_by_key("output_db", 0.0);
    amp.set_param_by_key("cabinet", cabinet);
    amp.prepare(&PrepareContext::new(rate, 256, 1));
    let mut result = Vec::new();
    let mut buffer = AudioBuffer::new(1, 256, rate);
    for block in 0..64 {
        for (index, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
            *sample = (std::f64::consts::TAU * frequency * (block * 256 + index) as f64 / rate)
                .sin() as f32
                * 0.3;
        }
        amp.process(
            &mut buffer,
            &ProcessContext::realtime(rate, 256, 0, 120.0, true),
        );
        result.extend_from_slice(buffer.channel(0));
    }
    result
}

fn amplitude(samples: &[f32], frequency: f64, rate: f64) -> f64 {
    let mut real = 0.0;
    let mut imaginary = 0.0;
    for (index, sample) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * frequency * index as f64 / rate;
        real += f64::from(*sample) * phase.cos();
        imaginary += f64::from(*sample) * phase.sin();
    }
    real.hypot(imaginary) / samples.len() as f64
}

#[test]
fn oversampling_reduces_the_folded_third_harmonic() {
    let audio = render(48000.0, 10000.0, 30.0, 0.0);
    let raw: Vec<_> = (0..4800)
        .map(|index| {
            saturate(
                (std::f32::consts::TAU * 10000.0 * index as f32 / 48000.0).sin() * 0.3,
                10.0_f32.powf(30.0 / 20.0),
            )
        })
        .collect();
    let folded = amplitude(&audio[8000..12800], 18000.0, 48000.0);
    let unfiltered = amplitude(&raw, 18000.0, 48000.0);
    assert!(
        folded < unfiltered * 0.05,
        "folded harmonic: {folded}, direct shaping: {unfiltered}"
    );
}

#[test]
fn cabinet_attenuates_high_frequencies_and_preserves_the_midrange() {
    let level = |hz, cabinet| {
        let audio = render(48000.0, hz, 0.0, cabinet);
        amplitude(&audio[8000..12800], hz, 48000.0)
    };
    assert!(level(10000.0, 1.0) < level(10000.0, 0.0) * 0.05);
    assert!(level(1000.0, 1.0) > level(1000.0, 0.0) * 0.5);
}

#[test]
fn drive_adds_harmonics_and_the_declared_fir_latency_is_32_frames() {
    let clean = render(48000.0, 250.0, 0.0, 0.0);
    let lead = render(48000.0, 250.0, 28.0, 0.0);
    let ratio = |audio: &[f32]| {
        amplitude(&audio[8000..12800], 750.0, 48000.0)
            / amplitude(&audio[8000..12800], 250.0, 48000.0)
    };
    assert!(ratio(&lead) > ratio(&clean) * 5.0);
    assert_eq!(GuitarAmp::new().latency_frames(), 32);
    let mut amp = GuitarAmp::new();
    amp.set_param_by_key("drive_db", 0.0);
    amp.set_param_by_key("output_db", 0.0);
    amp.set_param_by_key("cabinet", 0.0);
    amp.prepare(&PrepareContext::new(48000.0, 128, 1));
    let mut impulse = AudioBuffer::new(1, 128, 48000.0);
    impulse.channel_mut(0)[0] = 0.001;
    amp.process(
        &mut impulse,
        &ProcessContext::realtime(48000.0, 128, 0, 120.0, true),
    );
    let peak = impulse
        .channel(0)
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .unwrap()
        .0;
    assert_eq!(peak, amp.latency_frames());
}

#[test]
fn extreme_controls_and_sample_rates_stay_finite_and_reset_to_silence() {
    for rate in [8000.0, 44100.0, 48000.0, 96000.0, 192000.0] {
        let mut amp = GuitarAmp::new();
        let controls: Vec<_> = amp.parameters().iter().map(|p| (p.id, p.max)).collect();
        for (id, value) in controls {
            amp.set_param(id, value);
        }
        amp.prepare(&PrepareContext::new(rate, 256, 2));
        let mut buffer = AudioBuffer::stereo(256, rate);
        let ctx = ProcessContext::realtime(rate, 256, 0, 120.0, true);
        for block in 0..16 {
            for samples in buffer.channels_mut() {
                for (index, sample) in samples.iter_mut().enumerate() {
                    *sample = ((block * 256 + index) as f32 * 0.07).sin() * 4.0;
                }
            }
            amp.process(&mut buffer, &ctx);
            assert!(
                buffer
                    .channels()
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite() && sample.abs() < 16.0)
            );
        }
        amp.reset();
        buffer.clear();
        amp.process(&mut buffer, &ctx);
        assert_eq!(buffer.peak(), 0.0);
    }
}
