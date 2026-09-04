//! The voice model itself: load the file once, sing frames whenever asked.
//!
//! Everything here is deliberately dumb about music. The model is a pure function — phonemes,
//! durations, curves and *noise* in, waveform out — and this module's whole job is to feed it:
//! chunk by chunk (see [`score`](crate::score) for why), with the noise drawn from
//! [`auris_core::rng`] streams named by the seed and the chunk, so the same document, seed and
//! voice always render the same take, and no thread ever waits on another render's randomness.

use std::path::{Path, PathBuf};

use ort::execution_providers::{
    CoreMLExecutionProvider, DirectMLExecutionProvider, ExecutionProvider,
    ExecutionProviderDispatch,
};
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

/// Where a voice model's inference runs.
///
/// A preference about the machine, not the song: the same document renders through whichever
/// of these the settings name, and a frozen take keeps whatever it was sung with. The GPU is
/// reached through the platform's own provider — DirectML on Windows, Core ML on macOS — so
/// choosing it never installs anything.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash,
)]
#[serde(rename_all = "lowercase")]
pub enum Acceleration {
    /// Sing on the GPU where the runtime offers one, and on the CPU where it does not.
    #[default]
    Auto,
    /// Insist on the GPU: loading fails visibly when it cannot be used, rather than
    /// falling back to a CPU render the person asked not to have.
    Gpu,
    /// Stay on the CPU.
    Cpu,
}

/// The GPU provider this platform reaches, with whether the linked runtime carries it.
///
/// `cfg!` rather than `#[cfg]` on purpose: the provider types exist on every platform (only
/// their *registration* is feature-gated), so both arms compile and their tests run
/// everywhere — the rule that keeps the Windows path checkable from a Mac.
fn gpu_provider() -> Option<(ExecutionProviderDispatch, bool)> {
    if cfg!(target_os = "windows") {
        let provider = DirectMLExecutionProvider::default();
        let carried = provider.is_available().unwrap_or(false);
        Some((provider.build(), carried))
    } else if cfg!(target_os = "macos") {
        let provider = CoreMLExecutionProvider::default();
        let carried = provider.is_available().unwrap_or(false);
        Some((provider.build(), carried))
    } else {
        None
    }
}

/// A loaded auris-singer voice: the ONNX session and the metadata it carried.
///
/// Loading costs a few hundred milliseconds and a couple of hundred megabytes; keep the model
/// alive between renders rather than reopening it. Singing takes `&mut self` because the
/// underlying runtime session does; share a voice across threads by owning it behind a lock.
pub struct VoiceModel {
    session: Session,
    info: VoiceInfo,
    path: PathBuf,
    acceleration: Acceleration,
    /// Whether the session was built with the GPU provider in it.
    ///
    /// Falls to `false` when an [`Acceleration::Auto`] voice is demoted mid-render — a GPU
    /// provider can accept a session and still refuse its shapes at inference, which is how
    /// DirectML treats this model family today, and "auto" promises the render finishes.
    on_gpu: bool,
}

/// Builds the runtime session `acceleration`'s way, saying whether the GPU is in it.
fn open_session(path: &Path, acceleration: Acceleration) -> Result<(Session, bool), SingError> {
    let threads = std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(2).max(1))
        .unwrap_or(1);
    let refused = |error: ort::Error| SingError::Load {
        reason: error.to_string(),
    };
    let mut builder = Session::builder()
        .and_then(|builder| builder.with_intra_threads(threads))
        .map_err(refused)?;
    let gpu = match acceleration {
        Acceleration::Cpu => None,
        Acceleration::Auto => gpu_provider().filter(|(_, carried)| *carried),
        Acceleration::Gpu => Some(gpu_provider().ok_or(SingError::NoGpu)?),
    };
    let engaged = gpu.is_some();
    if let Some((provider, _)) = gpu {
        let provider = match acceleration {
            Acceleration::Gpu => provider.error_on_failure(),
            _ => provider,
        };
        builder = builder
            .with_execution_providers([provider])
            // DirectML cannot plan buffer reuse ahead of a run; the runtime wants memory
            // patterns off whenever it is in the session, and the other providers do not
            // miss them.
            .and_then(|builder| builder.with_memory_pattern(false))
            .map_err(refused)?;
    }
    match builder.commit_from_file(path) {
        Ok(session) => Ok((session, engaged)),
        Err(error) if engaged && acceleration == Acceleration::Auto => {
            log::warn!("the GPU refused the voice model ({error}); loading it on the CPU instead");
            open_session(path, Acceleration::Cpu)
        }
        Err(error) => Err(refused(error)),
    }
}

impl VoiceModel {
    /// Opens the model file and reads the metadata riding inside it.
    ///
    /// The inference threads are capped two under the machine's parallelism: a render runs
    /// while the audio callback and the window keep their own deadlines, and onnxruntime's
    /// default is to take every core it can see.
    ///
    /// `acceleration` says where inference runs. [`Acceleration::Auto`] takes the platform's
    /// GPU provider when the linked runtime carries it, falls back to the CPU when the device
    /// refuses the session, and demotes itself to the CPU mid-render if the provider accepts
    /// the session and then refuses its shapes; [`Acceleration::Gpu`] makes every one of
    /// those refusals an error instead, because the GPU was asked for by name.
    pub fn load(path: &Path, acceleration: Acceleration) -> Result<VoiceModel, SingError> {
        let (session, on_gpu) = open_session(path, acceleration)?;
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
            acceleration,
            on_gpu,
        })
    }

    /// The model's own account of itself.
    pub fn info(&self) -> &VoiceInfo {
        &self.info
    }

    /// What [`Self::load`] was asked to run this voice on.
    pub fn acceleration(&self) -> Acceleration {
        self.acceleration
    }

    /// Whether the GPU provider is in the session right now.
    ///
    /// `false` under [`Acceleration::Cpu`], on a machine with nothing to offer, or after an
    /// automatic voice was demoted mid-render.
    pub fn on_gpu(&self) -> bool {
        self.on_gpu
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
    pub fn sing(
        &mut self,
        frames: &SingerFrames,
        speaker: u32,
        seed: u64,
    ) -> Result<Vec<f32>, SingError> {
        self.sing_with(frames, speaker, seed, |_, _| true)
    }

    /// [`sing`](Self::sing), reporting each chunk to `progress` as `(done, total)`.
    ///
    /// `progress` is called before every inference and once more when all are done; answering
    /// `false` abandons the render with [`SingError::Cancelled`]. Chunks arrive in timeline
    /// order, so `done / total` is honest about time as well as work.
    pub fn sing_with(
        &mut self,
        frames: &SingerFrames,
        speaker: u32,
        seed: u64,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        crate::validate_frames(frames)?;
        if speaker >= self.info.n_speakers {
            return Err(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            });
        }
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
            let sung = match self.sing_chunk(frames, range.clone(), speaker, seed, at) {
                Ok(sung) => sung,
                // A GPU provider can take the session and still refuse its shapes at
                // inference — DirectML does exactly that to this model family today. Auto
                // promised a render, not a processor, so the voice rebuilds itself on the
                // CPU and sings the same chunk again; the demotion sticks for the model's
                // lifetime, because the shapes will not change. An *insisted-on* GPU stays
                // an error: falling back is precisely what Gpu asks not to have.
                Err(error) if self.on_gpu && self.acceleration == Acceleration::Auto => {
                    log::warn!("the GPU refused a chunk ({error}); singing on the CPU instead");
                    let (session, _) = open_session(&self.path, Acceleration::Cpu)?;
                    self.session = session;
                    self.on_gpu = false;
                    self.sing_chunk(frames, range.clone(), speaker, seed, at)?
                }
                Err(error) => return Err(error),
            };
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
        speaker: u32,
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
            "speaker_ids" => Tensor::from_array(([1], vec![i64::from(speaker)]))?,
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
        match VoiceModel::load(Path::new("nowhere/no-such-voice.onnx"), Acceleration::Auto) {
            Err(error) => assert!(matches!(error, SingError::Load { .. }), "{error}"),
            Ok(_) => panic!("a file that is not there must not load"),
        }
    }

    #[test]
    fn the_acceleration_spells_itself_the_way_a_settings_file_reads() {
        // The variant names are what lands in settings.json, so they are pinned here the way
        // a format is: lowercase, and round-tripping.
        for (choice, spelt) in [
            (Acceleration::Auto, "\"auto\""),
            (Acceleration::Gpu, "\"gpu\""),
            (Acceleration::Cpu, "\"cpu\""),
        ] {
            assert_eq!(serde_json::to_string(&choice).unwrap(), spelt);
            assert_eq!(serde_json::from_str::<Acceleration>(spelt).unwrap(), choice);
        }
        assert_eq!(Acceleration::default(), Acceleration::Auto);
    }

    #[test]
    fn each_desktop_platform_has_a_gpu_provider_to_offer() {
        // Both arms of the platform choice compile everywhere; this pins that the two desktop
        // platforms answer with *a* provider at all. Whether the runtime carries it is the
        // second half of the tuple and a fact about the machine, not asserted here.
        let offered = gpu_provider();
        if let Some((_, carried)) = &offered {
            // A machine fact worth seeing in the log when something GPU-shaped is debugged,
            // and not an assertion: a runner without the provider is healthy, just slower.
            eprintln!("the linked runtime carries the GPU provider: {carried}");
        }
        assert_eq!(
            offered.is_some(),
            cfg!(any(target_os = "windows", target_os = "macos"))
        );
    }
}
