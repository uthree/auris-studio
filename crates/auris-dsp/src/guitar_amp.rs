//! A compact guitar amplifier and analytic speaker-cabinet approximation.
//!
//! A tone stack precedes two asymmetric saturating stages. Eight-times oversampling
//! suppresses folded distortion harmonics; the optional cabinet follows decimation.
//! This is a playable generic model, not a circuit or measured-IR replica of a named amp.

use std::borrow::Cow;

use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};

use crate::{Biquad, BiquadCoefficients, SmoothedValue, bank::ParamBank, settled};

const FACTOR: usize = 8;
const TAPS: usize = 257;
const LATENCY: usize = (TAPS - 1) / FACTOR;
const CABINET_CHOICES: [Cow<'static, str>; 3] = [
    Cow::Borrowed("Bypass"),
    Cow::Borrowed("Open"),
    Cow::Borrowed("Closed"),
];

fn kernel() -> [f32; TAPS] {
    let mut taps = std::array::from_fn(|index| {
        let x = index as f64 - (TAPS - 1) as f64 / 2.0;
        let cutoff = 0.45 / FACTOR as f64;
        let sinc = if x == 0.0 {
            2.0 * cutoff
        } else {
            (std::f64::consts::TAU * cutoff * x).sin() / (std::f64::consts::PI * x)
        };
        let phase = std::f64::consts::TAU * index as f64 / (TAPS - 1) as f64;
        (sinc * (0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos())) as f32
    });
    let sum: f32 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

#[derive(Clone)]
struct Channel {
    tone: [Biquad; 3],
    dc: Biquad,
    cabinet: [Biquad; 6],
    input: [f32; TAPS.div_ceil(FACTOR)],
    output: [f32; TAPS],
    input_cursor: usize,
    output_cursor: usize,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            tone: Default::default(),
            dc: Default::default(),
            cabinet: Default::default(),
            input: [0.0; TAPS.div_ceil(FACTOR)],
            output: [0.0; TAPS],
            input_cursor: 0,
            output_cursor: 0,
        }
    }
}

fn saturate(input: f32, drive: f32) -> f32 {
    let preamp = (input * drive + 0.12).tanh() - 0.12_f32.tanh();
    ((preamp * 1.6 + 0.05).tanh() - 0.05_f32.tanh()) / 1.6
}

impl Channel {
    fn next(&mut self, input: f32, drive: f32, taps: &[f32; TAPS], cabinet: bool) -> f32 {
        let input = self.tone.iter_mut().fold(settled(input), |sample, filter| {
            filter.process_sample(sample)
        });
        self.input[self.input_cursor] = input;
        let mut sample = 0.0;
        for phase in 0..FACTOR {
            let mut interpolated = 0.0;
            for (delay, tap) in taps.iter().skip(phase).step_by(FACTOR).enumerate() {
                let index = (self.input_cursor + self.input.len() - delay) % self.input.len();
                interpolated += *tap * self.input[index];
            }
            self.output[self.output_cursor] = self
                .dc
                .process_sample(saturate(interpolated * FACTOR as f32, drive));
            if phase == 0 {
                for (delay, tap) in taps.iter().enumerate() {
                    sample += *tap * self.output[(self.output_cursor + TAPS - delay) % TAPS];
                }
            }
            self.output_cursor = (self.output_cursor + 1) % TAPS;
        }
        self.input_cursor = (self.input_cursor + 1) % self.input.len();
        // Decimate at phase zero. Compensate the fixed FIR delay through the Effect API.
        let filtered = self
            .cabinet
            .iter_mut()
            .fold(sample, |sample, filter| filter.process_sample(sample));
        if cabinet { filtered } else { sample }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.input_cursor = 0;
        self.output_cursor = 0;
        self.dc.reset();
        for filter in self.tone.iter_mut().chain(&mut self.cabinet) {
            filter.reset();
        }
    }
}

/// Oversampled guitar saturation, tone controls and optional speaker coloration.
///
/// Use after a DI instrument or recording. Drive near 0 dB supplies a clean starting
/// point, 12 dB crunch and 28 dB a more sustained lead; input level affects the result.
pub struct GuitarAmp {
    params: ParamBank,
    channels: Vec<Channel>,
    taps: [f32; TAPS],
    rate: f64,
    drive: SmoothedValue,
    output: SmoothedValue,
}

impl Default for GuitarAmp {
    fn default() -> Self {
        Self::new()
    }
}

impl GuitarAmp {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.guitar_amp";

    /// Builds an amplifier; per-channel state is allocated in `prepare`.
    pub fn new() -> Self {
        Self {
            params: ParamBank::new(vec![
                ParamDescriptor::decibels(0u32, "drive_db", "Drive", 0.0, 42.0, 12.0),
                ParamDescriptor::decibels(1u32, "bass_db", "Bass", -12.0, 12.0, 0.0),
                ParamDescriptor::decibels(2u32, "mid_db", "Middle", -12.0, 12.0, 0.0),
                ParamDescriptor::decibels(3u32, "treble_db", "Treble", -12.0, 12.0, 0.0),
                ParamDescriptor::new(4u32, "cabinet", "Cabinet", 0.0, 2.0, 1.0)
                    .with_choices(&CABINET_CHOICES),
                ParamDescriptor::decibels(5u32, "output_db", "Output", -30.0, 12.0, -6.0),
            ]),
            channels: Vec::new(),
            taps: kernel(),
            rate: 48_000.0,
            drive: SmoothedValue::new(10.0_f32.powf(12.0 / 20.0), 0.02, 48_000.0),
            output: SmoothedValue::new(10.0_f32.powf(-6.0 / 20.0), 0.02, 48_000.0),
        }
    }

    fn configure(&mut self) {
        let tone = [
            BiquadCoefficients::low_shelf(self.rate, 150.0, 0.707, self.params.at(1)),
            BiquadCoefficients::peaking(self.rate, 750.0, 0.8, self.params.at(2)),
            BiquadCoefficients::high_shelf(self.rate, 3000.0, 0.707, self.params.at(3)),
        ];
        let closed = self.params.at(4) >= 1.5;
        let cabinet = [
            BiquadCoefficients::highpass(self.rate, if closed { 85.0 } else { 65.0 }, 0.707),
            BiquadCoefficients::peaking(self.rate, if closed { 140.0 } else { 110.0 }, 1.0, 3.0),
            BiquadCoefficients::peaking(self.rate, 400.0, 0.9, -3.0),
            BiquadCoefficients::peaking(self.rate, 1600.0, 0.7, 2.0),
            BiquadCoefficients::lowpass(self.rate, if closed { 3800.0 } else { 4800.0 }, 0.707),
            BiquadCoefficients::lowpass(self.rate, if closed { 3800.0 } else { 4800.0 }, 0.707),
        ];
        for channel in &mut self.channels {
            for (filter, coefficients) in channel.tone.iter_mut().zip(tone) {
                filter.set_coefficients(coefficients);
            }
            for (filter, coefficients) in channel.cabinet.iter_mut().zip(cabinet) {
                filter.set_coefficients(coefficients);
            }
            channel.dc.set_coefficients(BiquadCoefficients::highpass(
                self.rate * FACTOR as f64,
                25.0,
                0.707,
            ));
        }
    }
}

impl Parameterized for GuitarAmp {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }
    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }
    fn set_param(&mut self, id: ParamId, value: f32) {
        if self.params.set(id, value) {
            self.drive
                .set_target(10.0_f32.powf(self.params.at(0) / 20.0));
            self.output
                .set_target(10.0_f32.powf(self.params.at(5) / 20.0));
            if (1..=4).contains(&id.0) {
                self.configure();
            }
        }
    }
}

impl Effect for GuitarAmp {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Guitar Amp",
            "Oversampled amplifier, tone stack and open/closed speaker cabinet",
            PluginCategory::Distortion,
        )
    }
    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = if ctx.sample_rate.is_finite() {
            ctx.sample_rate.clamp(8000.0, 192000.0)
        } else {
            48000.0
        };
        self.channels = vec![Channel::default(); ctx.channel_count];
        self.drive.set_time(0.02, self.rate as f32);
        self.output.set_time(0.02, self.rate as f32);
        self.configure();
        self.reset();
    }
    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
        self.drive.snap_to(10.0_f32.powf(self.params.at(0) / 20.0));
        self.output.snap_to(10.0_f32.powf(self.params.at(5) / 20.0));
    }
    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = buffer.frame_count().min(ctx.block_frames);
        let cabinet = self.params.at(4) >= 0.5;
        for frame in 0..frames {
            let drive = self.drive.next_value();
            let output = self.output.next_value();
            for (state, samples) in self.channels.iter_mut().zip(buffer.channels_mut()) {
                samples[frame] = state.next(samples[frame], drive, &self.taps, cabinet) * output;
            }
        }
    }
    fn latency_frames(&self) -> usize {
        LATENCY
    }
    fn tail_frames(&self) -> usize {
        (self.rate * 0.2) as usize + LATENCY
    }
}

#[cfg(test)]
mod tests;
