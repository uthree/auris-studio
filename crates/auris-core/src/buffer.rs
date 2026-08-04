//! The planar audio buffer used everywhere in the engine.
//!
//! Audio is stored *planar* (one contiguous `Vec<f32>` per channel) rather than interleaved
//! because every processor in the graph — synths, biquads, delays — works one channel at a
//! time, and planar layout keeps those inner loops auto-vectorisable.
//!
//! During rendering a buffer is reused across blocks: [`AudioBuffer::set_frame_count`] only
//! changes each channel's length, never its capacity, so the realtime thread never allocates.

use crate::error::{CoreError, Result};

/// A block of planar `f32` audio.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    channels: Vec<Vec<f32>>,
    sample_rate: f64,
}

impl AudioBuffer {
    /// Creates a silent buffer with `channel_count` channels of `frame_count` frames.
    pub fn new(channel_count: usize, frame_count: usize, sample_rate: f64) -> Self {
        Self {
            channels: vec![vec![0.0; frame_count]; channel_count.max(1)],
            sample_rate,
        }
    }

    /// Creates a silent stereo buffer.
    pub fn stereo(frame_count: usize, sample_rate: f64) -> Self {
        Self::new(2, frame_count, sample_rate)
    }

    /// Wraps already-planar data, checking that every channel has the same length.
    pub fn from_planar(channels: Vec<Vec<f32>>, sample_rate: f64) -> Result<Self> {
        let expected = channels.first().ok_or(CoreError::NoChannels)?.len();
        for (channel, samples) in channels.iter().enumerate() {
            if samples.len() != expected {
                return Err(CoreError::RaggedChannels {
                    channel,
                    found: samples.len(),
                    expected,
                });
            }
        }
        Ok(Self {
            channels,
            sample_rate,
        })
    }

    /// De-interleaves `data` into a planar buffer.
    ///
    /// A trailing partial frame in `data` is ignored.
    pub fn from_interleaved(data: &[f32], channel_count: usize, sample_rate: f64) -> Result<Self> {
        if channel_count == 0 {
            return Err(CoreError::NoChannels);
        }
        let frames = data.len() / channel_count;
        let mut channels = vec![Vec::with_capacity(frames); channel_count];
        for frame in 0..frames {
            for (channel, samples) in channels.iter_mut().enumerate() {
                samples.push(data[frame * channel_count + channel]);
            }
        }
        Ok(Self {
            channels,
            sample_rate,
        })
    }

    /// Number of channels. Always at least 1.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Number of frames currently held by each channel.
    pub fn frame_count(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Sample rate the contents were captured or rendered at.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Overrides the sample rate tag without touching the samples.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
    }

    /// Duration of the buffer in seconds.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate <= 0.0 {
            0.0
        } else {
            self.frame_count() as f64 / self.sample_rate
        }
    }

    /// Returns `true` when the buffer holds no frames.
    pub fn is_empty(&self) -> bool {
        self.frame_count() == 0
    }

    /// Immutable view of one channel.
    pub fn channel(&self, index: usize) -> &[f32] {
        &self.channels[index]
    }

    /// Mutable view of one channel.
    pub fn channel_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.channels[index]
    }

    /// Immutable view of one channel, or `None` when out of range.
    pub fn try_channel(&self, index: usize) -> Option<&[f32]> {
        self.channels.get(index).map(Vec::as_slice)
    }

    /// All channels at once.
    pub fn channels(&self) -> &[Vec<f32>] {
        &self.channels
    }

    /// All channels at once, mutably. Use `split_at_mut` to touch two channels together.
    pub fn channels_mut(&mut self) -> &mut [Vec<f32>] {
        &mut self.channels
    }

    /// Iterates the channels immutably.
    pub fn iter_channels(&self) -> impl Iterator<Item = &[f32]> {
        self.channels.iter().map(Vec::as_slice)
    }

    /// Iterates the channels mutably.
    pub fn iter_channels_mut(&mut self) -> impl Iterator<Item = &mut [f32]> {
        self.channels.iter_mut().map(Vec::as_mut_slice)
    }

    /// Reads a single sample, returning silence for out-of-range coordinates.
    pub fn sample(&self, channel: usize, frame: usize) -> f32 {
        self.channels
            .get(channel)
            .and_then(|c| c.get(frame))
            .copied()
            .unwrap_or(0.0)
    }

    /// Grows every channel's capacity so that later [`Self::set_frame_count`] calls up to
    /// `frames` are allocation-free. Call this off the realtime thread.
    pub fn reserve_frames(&mut self, frames: usize) {
        for channel in &mut self.channels {
            if channel.capacity() < frames {
                channel.reserve(frames - channel.len());
            }
        }
    }

    /// Resizes every channel to `frames`, filling new frames with silence.
    ///
    /// This never shrinks the underlying allocation, so shrinking then re-growing within a
    /// previously reserved size performs no allocation and is safe on the audio thread.
    pub fn set_frame_count(&mut self, frames: usize) {
        for channel in &mut self.channels {
            channel.resize(frames, 0.0);
        }
    }

    /// Changes the channel count, keeping existing channels and adding silent ones.
    pub fn set_channel_count(&mut self, channels: usize) {
        let frames = self.frame_count();
        self.channels.resize(channels.max(1), vec![0.0; frames]);
        // `resize` clones the template only for *new* entries, so make sure they match.
        for channel in &mut self.channels {
            channel.resize(frames, 0.0);
        }
    }

    /// Fills the whole buffer with silence, keeping its dimensions.
    pub fn clear(&mut self) {
        for channel in &mut self.channels {
            channel.fill(0.0);
        }
    }

    /// Multiplies every sample by `gain` (linear).
    pub fn apply_gain(&mut self, gain: f32) {
        if gain == 1.0 {
            return;
        }
        for channel in &mut self.channels {
            for sample in channel.iter_mut() {
                *sample *= gain;
            }
        }
    }

    /// Ramps linearly from `start` to `end` gain across the buffer, avoiding zipper noise
    /// when a fader moves between blocks.
    pub fn apply_gain_ramp(&mut self, start: f32, end: f32) {
        let frames = self.frame_count();
        if frames == 0 {
            return;
        }
        if (start - end).abs() < f32::EPSILON {
            self.apply_gain(start);
            return;
        }
        let step = (end - start) / frames as f32;
        for channel in &mut self.channels {
            let mut gain = start;
            for sample in channel.iter_mut() {
                *sample *= gain;
                gain += step;
            }
        }
    }

    /// Adds `other * gain` into `self`, matching by channel index.
    ///
    /// Extra channels on either side and frames beyond the shorter buffer are ignored, which
    /// makes summing a mono source into a stereo bus a no-op on the right channel rather than
    /// an error. Use [`Self::mix_mono_to_all`] when you want a mono source spread everywhere.
    pub fn mix_from(&mut self, other: &AudioBuffer, gain: f32) {
        let channels = self.channel_count().min(other.channel_count());
        let frames = self.frame_count().min(other.frame_count());
        for channel in 0..channels {
            let src = &other.channels[channel][..frames];
            let dst = &mut self.channels[channel][..frames];
            for (d, s) in dst.iter_mut().zip(src) {
                *d += *s * gain;
            }
        }
    }

    /// Adds `other`'s first channel into every channel of `self`.
    pub fn mix_mono_to_all(&mut self, other: &AudioBuffer, gain: f32) {
        let frames = self.frame_count().min(other.frame_count());
        let Some(src) = other.channels.first() else {
            return;
        };
        for dst in &mut self.channels {
            for (d, s) in dst[..frames].iter_mut().zip(&src[..frames]) {
                *d += *s * gain;
            }
        }
    }

    /// Copies `other` into `self`, matching by channel index and clearing what is not covered.
    pub fn copy_from(&mut self, other: &AudioBuffer) {
        self.clear();
        self.mix_from(other, 1.0);
    }

    /// Interleaves the buffer into a freshly allocated `Vec`.
    pub fn to_interleaved(&self) -> Vec<f32> {
        let frames = self.frame_count();
        let channel_count = self.channel_count();
        let mut out = Vec::with_capacity(frames * channel_count);
        for frame in 0..frames {
            for channel in &self.channels {
                out.push(channel[frame]);
            }
        }
        out
    }

    /// Downmixes to a single channel by averaging.
    pub fn to_mono(&self) -> AudioBuffer {
        let frames = self.frame_count();
        let scale = 1.0 / self.channel_count() as f32;
        let mut mono = vec![0.0; frames];
        for channel in &self.channels {
            for (m, s) in mono.iter_mut().zip(channel) {
                *m += *s * scale;
            }
        }
        AudioBuffer {
            channels: vec![mono],
            sample_rate: self.sample_rate,
        }
    }

    /// Largest absolute sample value across all channels.
    pub fn peak(&self) -> f32 {
        self.channels
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |acc, s| acc.max(s.abs()))
    }

    /// Largest absolute sample value in one channel.
    pub fn channel_peak(&self, channel: usize) -> f32 {
        self.channels
            .get(channel)
            .map(|c| c.iter().fold(0.0f32, |acc, s| acc.max(s.abs())))
            .unwrap_or(0.0)
    }

    /// Root-mean-square level of one channel.
    pub fn channel_rms(&self, channel: usize) -> f32 {
        let Some(samples) = self.channels.get(channel) else {
            return 0.0;
        };
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    /// Replaces NaN and infinite samples with silence.
    ///
    /// A single NaN escaping a misbehaving filter otherwise propagates through the mix bus and
    /// silences (or worse, blasts) the output device, so the engine sanitises before playback.
    pub fn sanitize(&mut self) {
        for channel in &mut self.channels {
            for sample in channel.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
        }
    }

    /// Hard-clips every sample into `[-limit, limit]`.
    pub fn clip(&mut self, limit: f32) {
        for channel in &mut self.channels {
            for sample in channel.iter_mut() {
                *sample = sample.clamp(-limit, limit);
            }
        }
    }

    /// Copies a frame range out into a new buffer, zero-padding past the end.
    pub fn slice(&self, start: usize, frames: usize) -> AudioBuffer {
        let mut out = AudioBuffer::new(self.channel_count(), frames, self.sample_rate);
        let available = self.frame_count().saturating_sub(start).min(frames);
        for (channel, src) in self.channels.iter().enumerate() {
            // Clamped, not indexed: a start past the end has to mean all padding, and the raw
            // `&src[start..start]` is a panic once `start` outruns the slice — Rust bounds the
            // start of a range even when the range is empty. Per channel, so a ragged buffer
            // pads its short channels instead of taking the whole method down.
            let from = start.min(src.len());
            let take = available.min(src.len() - from);
            out.channels[channel][..take].copy_from_slice(&src[from..from + take]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_round_trips() {
        let data = [1.0, -1.0, 0.5, -0.5];
        let buffer = AudioBuffer::from_interleaved(&data, 2, 48_000.0).unwrap();
        assert_eq!(buffer.channel_count(), 2);
        assert_eq!(buffer.frame_count(), 2);
        assert_eq!(buffer.channel(0), &[1.0, 0.5]);
        assert_eq!(buffer.channel(1), &[-1.0, -0.5]);
        assert_eq!(buffer.to_interleaved(), data);
    }

    #[test]
    fn set_frame_count_keeps_capacity() {
        let mut buffer = AudioBuffer::stereo(0, 48_000.0);
        buffer.reserve_frames(512);
        let capacity = buffer.channels[0].capacity();
        buffer.set_frame_count(512);
        buffer.set_frame_count(64);
        buffer.set_frame_count(512);
        assert_eq!(buffer.channels[0].capacity(), capacity);
    }

    #[test]
    fn gain_ramp_interpolates() {
        let mut buffer = AudioBuffer::from_planar(vec![vec![1.0; 4]], 48_000.0).unwrap();
        buffer.apply_gain_ramp(0.0, 1.0);
        assert_eq!(buffer.channel(0), &[0.0, 0.25, 0.5, 0.75]);
    }

    #[test]
    fn sanitize_removes_non_finite() {
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![f32::NAN, f32::INFINITY, 0.5]], 48_000.0).unwrap();
        buffer.sanitize();
        assert_eq!(buffer.channel(0), &[0.0, 0.0, 0.5]);
    }

    #[test]
    fn slice_zero_pads_past_the_end() {
        let buffer = AudioBuffer::from_planar(vec![vec![1.0, 2.0]], 48_000.0).unwrap();
        let tail = buffer.slice(1, 3);
        assert_eq!(tail.channel(0), &[2.0, 0.0, 0.0]);
    }

    #[test]
    fn slice_pads_from_a_start_past_the_end_too() {
        // The doc promises zero-padding, and the end-overrun case above delivered it — but a
        // *start* past the end panicked, because Rust bounds the start of a range even when
        // the range is empty. The documented contract has to hold from either side.
        let buffer = AudioBuffer::from_planar(vec![vec![1.0, 2.0]], 48_000.0).unwrap();
        let silence = buffer.slice(3, 2);
        assert_eq!(silence.channel(0), &[0.0, 0.0]);
        let boundary = buffer.slice(2, 2);
        assert_eq!(boundary.channel(0), &[0.0, 0.0]);
    }
}
