#![allow(dead_code)]

use crate::loop_mode::LoopMode;
use crate::synthesizer_settings::SynthesizerSettings;

// In this class, fixed-point numbers are used for speed-up.
// A fixed-point number is expressed by Int64, whose lower 24 bits represent the fraction part,
// and the rest represent the integer part.
// For clarity, fixed-point number variables have a suffix "_fp".

#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Oscillator {
    synthesizer_sample_rate: i32,

    loop_mode: LoopMode,
    sample_sample_rate: i32,
    start: i32,
    end: i32,
    start_loop: i32,
    end_loop: i32,
    root_key: i32,

    tune: f32,
    pitch_change_scale: f32,
    sample_rate_ratio: f32,

    looping: bool,

    position_fp: i64,
}

impl Oscillator {
    const FRAC_BITS: i32 = 24;
    const FRAC_UNIT: i64 = 1_i64 << Oscillator::FRAC_BITS;
    const FP_TO_SAMPLE: f32 = 1_f32 / (32768 * Oscillator::FRAC_UNIT) as f32;

    pub(crate) fn new(settings: &SynthesizerSettings) -> Self {
        Self {
            synthesizer_sample_rate: settings.sample_rate,
            loop_mode: LoopMode::NoLoop,
            sample_sample_rate: 0,
            start: 0,
            end: 0,
            start_loop: 0,
            end_loop: 0,
            root_key: 0,
            tune: 0_f32,
            pitch_change_scale: 0_f32,
            sample_rate_ratio: 0_f32,
            looping: false,
            position_fp: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &mut self,
        loop_mode: LoopMode,
        sample_rate: i32,
        start: i32,
        end: i32,
        start_loop: i32,
        end_loop: i32,
        root_key: i32,
        coarse_tune: i32,
        fine_tune: i32,
        scale_tuning: i32,
    ) {
        self.loop_mode = loop_mode;
        self.sample_sample_rate = sample_rate;
        self.start = start;
        self.end = end;
        self.start_loop = start_loop;
        self.end_loop = end_loop;
        self.root_key = root_key;

        self.tune = coarse_tune as f32 + 0.01_f32 * fine_tune as f32;
        self.pitch_change_scale = 0.01_f32 * scale_tuning as f32;
        self.sample_rate_ratio = sample_rate as f32 / self.synthesizer_sample_rate as f32;
        self.looping = self.loop_mode != LoopMode::NoLoop;
        self.position_fp = (start as i64) << Oscillator::FRAC_BITS;
    }

    pub(crate) fn release(&mut self) {
        if self.loop_mode == LoopMode::LoopUntilNoteOff {
            self.looping = false;
        }
    }

    pub(crate) fn process(&mut self, data: &[i16], block: &mut [f32], pitch: f32) -> bool {
        let pitch_change = self.pitch_change_scale * (pitch - self.root_key as f32) + self.tune;
        let pitch_ratio = self.sample_rate_ratio * 2_f32.powf(pitch_change / 12_f32);
        self.fill_block(data, block, pitch_ratio as f64)
    }

    fn fill_block(&mut self, data: &[i16], block: &mut [f32], pitch_ratio: f64) -> bool {
        let pitch_ratio_fp = (Oscillator::FRAC_UNIT as f64 * pitch_ratio) as i64;

        if self.looping {
            self.fill_block_continuous(data, block, pitch_ratio_fp)
        } else {
            self.fill_block_no_loop(data, block, pitch_ratio_fp)
        }
    }

    fn fill_block_no_loop(&mut self, data: &[i16], block: &mut [f32], pitch_ratio_fp: i64) -> bool {
        for t in 0..block.len() {
            let index = self.position_fp >> Oscillator::FRAC_BITS;
            if index < 0 || index >= i64::from(self.end) {
                if t > 0 {
                    let len = block.len();
                    block[t..len].fill(0_f32);
                    return true;
                } else {
                    return false;
                }
            }

            let Ok(index) = usize::try_from(index) else {
                block[t..].fill(0_f32);
                return t > 0;
            };
            let (Some(&x1), Some(&x2)) = (data.get(index), data.get(index.saturating_add(1)))
            else {
                block[t..].fill(0_f32);
                return t > 0;
            };
            let x1 = i64::from(x1);
            let x2 = i64::from(x2);
            let a_fp = self.position_fp & (Oscillator::FRAC_UNIT - 1);
            block[t] = Oscillator::FP_TO_SAMPLE
                * ((x1 << Oscillator::FRAC_BITS) + a_fp * (x2 - x1)) as f32;

            self.position_fp = self.position_fp.saturating_add(pitch_ratio_fp);
        }

        true
    }

    fn fill_block_continuous(
        &mut self,
        data: &[i16],
        block: &mut [f32],
        pitch_ratio_fp: i64,
    ) -> bool {
        let start_loop_fp = (self.start_loop as i64) << Oscillator::FRAC_BITS;
        let end_loop_fp = (self.end_loop as i64) << Oscillator::FRAC_BITS;
        let loop_length = i64::from(self.end_loop) - i64::from(self.start_loop);
        let loop_length_fp = loop_length << Oscillator::FRAC_BITS;
        if loop_length <= 0 || loop_length_fp <= 0 {
            block.fill(0_f32);
            return false;
        }

        for offset in 0..block.len() {
            if self.position_fp >= end_loop_fp {
                // A high-rate or highly transposed sample can advance by several complete loops
                // in one output frame. Folding only once lets the next unchecked sample access
                // escape the loop. The remainder is the exact same phase, however many loops
                // were crossed, and remains constant-time for hostile SoundFont metadata.
                let relative = i128::from(self.position_fp) - i128::from(start_loop_fp);
                self.position_fp = (i128::from(start_loop_fp)
                    + relative.rem_euclid(i128::from(loop_length_fp)))
                    as i64;
            }

            let index1 = self.position_fp >> Oscillator::FRAC_BITS;
            let index2 = if index1.saturating_add(1) >= i64::from(self.end_loop) {
                i64::from(self.start_loop)
            } else {
                index1.saturating_add(1)
            };

            let samples = usize::try_from(index1)
                .ok()
                .and_then(|index1| data.get(index1))
                .zip(
                    usize::try_from(index2)
                        .ok()
                        .and_then(|index2| data.get(index2)),
                );
            let Some((&x1, &x2)) = samples else {
                block[offset..].fill(0_f32);
                return offset > 0;
            };
            let x1 = i64::from(x1);
            let x2 = i64::from(x2);
            let a_fp = self.position_fp & (Oscillator::FRAC_UNIT - 1);
            block[offset] = Oscillator::FP_TO_SAMPLE
                * ((x1 << Oscillator::FRAC_BITS) + a_fp * (x2 - x1)) as f32;

            self.position_fp = self.position_fp.saturating_add(pitch_ratio_fp);
        }

        true
    }
}
