//! The voice model itself: load the file once, sing frames whenever asked.
//!
//! Everything here is deliberately dumb about music. The model is a pure function — phonemes,
//! durations, curves and *noise* in, waveform out — and this module's whole job is to feed it:
//! chunk by chunk (see [`score`](crate::score) for why), with the noise drawn from
//! [`auris_core::rng`] streams named by the seed and the chunk, so the same document, seed and
//! voice always render the same take, and no thread ever waits on another render's randomness.

use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::value::Tensor;

use auris_core::rng::{Key, Rng};
use auris_vocal::SingerFrames;

use crate::SingError;
use crate::metadata::{METADATA_KEY, VoiceInfo};
use crate::score::{MAX_CHUNK_FRAMES, arrange, chunk_ranges};

/// The prior's sampling temperature.
///
/// The exporter's own default: enough variance to keep long notes alive, well short of the
/// warble a full-temperature prior develops. Zero would make delivery deterministic in the
/// *statistical* sense too — flat, and audibly so.
pub const NOISE_SCALE: f32 = 0.667;

/// A loaded auris-singer voice: the ONNX session and the metadata it carried.
///
/// Loading costs a few hundred milliseconds and a couple of hundred megabytes; keep the model
/// alive between renders rather than reopening it. Singing takes `&mut self` because the
/// underlying runtime session does; share a voice across threads by owning it behind a lock.
pub struct VoiceModel {
    session: Session,
    info: VoiceInfo,
    path: PathBuf,
}

impl VoiceModel {
    /// Opens the model file and reads the metadata riding inside it.
    ///
    /// The inference threads are capped two under the machine's parallelism: a render runs
    /// while the audio callback and the window keep their own deadlines, and onnxruntime's
    /// default is to take every core it can see.
    pub fn load(path: &Path) -> Result<VoiceModel, SingError> {
        let threads = std::thread::available_parallelism()
            .map(|cores| cores.get().saturating_sub(2).max(1))
            .unwrap_or(1);
        let session = Session::builder()
            .and_then(|builder| builder.with_intra_threads(threads))
            .and_then(|builder| builder.commit_from_file(path))
            .map_err(|error| SingError::Load {
                reason: error.to_string(),
            })?;
        let raw = {
            let load = |error: ort::Error| SingError::Load {
                reason: error.to_string(),
            };
            let metadata = session.metadata().map_err(load)?;
            metadata
                .custom(METADATA_KEY)
                .map_err(load)?
                .ok_or(SingError::NotAVoice)?
        };
        let info = VoiceInfo::parse(&raw)?;
        Ok(VoiceModel {
            session,
            info,
            path: path.to_path_buf(),
        })
    }

    /// The model's own account of itself.
    pub fn info(&self) -> &VoiceInfo {
        &self.info
    }

    /// Where the model was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sings the frames and returns the waveform, mono at the model's sample rate.
    ///
    /// The waveform covers the whole timeline the frames cover — `frames.len()` hops of
    /// samples, silence where nothing is sung — so the caller places it at time zero and it
    /// lines up by construction. Same frames, same seed, same voice: same samples.
    pub fn sing(&mut self, frames: &SingerFrames, seed: u64) -> Result<Vec<f32>, SingError> {
        self.sing_with(frames, seed, |_, _| true)
    }

    /// [`sing`](Self::sing), reporting each chunk to `progress` as `(done, total)`.
    ///
    /// `progress` is called before every inference and once more when all are done; answering
    /// `false` abandons the render with [`SingError::Cancelled`]. Chunks arrive in timeline
    /// order, so `done / total` is honest about time as well as work.
    pub fn sing_with(
        &mut self,
        frames: &SingerFrames,
        seed: u64,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        let model_hop = self.info.hop_seconds();
        if (frames.hop_seconds - model_hop).abs() > model_hop * 1e-6 {
            return Err(SingError::HopMismatch {
                frames: frames.hop_seconds,
                model: model_hop,
            });
        }

        let hop = self.info.hop_length as usize;
        let mut out = vec![0.0f32; frames.len() * hop];
        let chunks = chunk_ranges(frames, MAX_CHUNK_FRAMES);
        let total = chunks.len();
        for (at, range) in chunks.into_iter().enumerate() {
            if !progress(at, total) {
                return Err(SingError::Cancelled);
            }
            let sung = self.sing_chunk(frames, range.clone(), seed, at)?;
            let start = range.start * hop;
            out[start..start + sung.len()].copy_from_slice(&sung);
        }
        if !progress(total, total) {
            return Err(SingError::Cancelled);
        }
        Ok(out)
    }

    /// One inference: one chunk of frames in, `range.len() * hop_length` samples out.
    fn sing_chunk(
        &mut self,
        frames: &SingerFrames,
        range: std::ops::Range<usize>,
        seed: u64,
        chunk: usize,
    ) -> Result<Vec<f32>, SingError> {
        let score = arrange(frames, range.clone(), &self.info.symbols);
        let spans = score.tokens.len();
        let count = range.len();
        let hop = self.info.hop_length as usize;
        let inter = self.info.inter_channels as usize;

        // The random draws are inputs by the model's own design: same streams, same take. The
        // chunk index is in the stream name so every chunk draws its own noise rather than a
        // shifted copy of its neighbour's.
        let mut z = Rng::stream(
            seed,
            &[Key::Word("sing"), Key::Index(chunk as u64), Key::Word("z")],
        );
        let z_noise: Vec<f32> = (0..inter * count).map(|_| z.jitter(1.0)).collect();
        let mut source = Rng::stream(
            seed,
            &[
                Key::Word("sing"),
                Key::Index(chunk as u64),
                Key::Word("source"),
            ],
        );
        let source_noise: Vec<f32> = (0..count * hop)
            .map(|_| source.unit() * 2.0 - 1.0)
            .collect();

        let refused = |error: ort::Error| SingError::Inference(error.to_string());
        let inputs = ort::inputs! {
            "phonemes" => Tensor::from_array(([1, spans], score.tokens))?,
            "phoneme_lengths" => Tensor::from_array(([1], vec![spans as i64]))?,
            "durations" => Tensor::from_array(([1, spans], score.durations))?,
            "f0" => Tensor::from_array(([1, count], score.f0))?,
            "energy" => Tensor::from_array(([1, count], score.energy))?,
            "voiced" => Tensor::from_array(([1, count], score.voiced))?,
            "speaker_ids" => Tensor::from_array(([1], vec![0i64]))?,
            "noise_scale" => Tensor::from_array(((), vec![NOISE_SCALE]))?,
            "z_noise" => Tensor::from_array(([1, inter, count], z_noise))?,
            "source_noise" => Tensor::from_array(([1, 1, count * hop], source_noise))?,
        }
        .map_err(refused)?;
        let outputs = self.session.run(inputs).map_err(refused)?;
        let (_, samples) = outputs["wav"]
            .try_extract_raw_tensor::<f32>()
            .map_err(refused)?;
        if samples.len() != count * hop {
            return Err(SingError::Inference(format!(
                "the model answered {} samples where {} frames wanted {}",
                samples.len(),
                count,
                count * hop
            )));
        }
        Ok(samples.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_an_error_rather_than_a_panic() {
        match VoiceModel::load(Path::new("nowhere/no-such-voice.onnx")) {
            Err(error) => assert!(matches!(error, SingError::Load { .. }), "{error}"),
            Ok(_) => panic!("a file that is not there must not load"),
        }
    }
}
