//! The common door every singing engine presents to the session.

use std::path::Path;

use auris_vocal::{SingerFrames, SingerScore};

use crate::{Acceleration, SingError, VoiceInfo};

/// A singing engine understood by Auris Studio.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// Auris' self-contained ONNX voice format.
    Auris,
    /// An OpenUtau-compatible DiffSinger voicebank.
    DiffSinger,
    /// A running VOICEVOX Engine reached through its HTTP API.
    Voicevox,
}

/// The backend contract: metadata, frame curves and an optional note score in; mono waveform out.
///
/// Implementations may use one model or a pipeline of models. They are always called off the
/// realtime audio thread. The trait is public so another engine can be added without teaching
/// the session or its frontends about that engine's files and tensors.
pub trait SingingBackend: Send {
    /// Which file format and inference pipeline this backend implements.
    fn kind(&self) -> BackendKind;
    /// The voice information shared with the document and frontends.
    fn info(&self) -> &VoiceInfo;
    /// What processor preference the backend was opened with.
    fn acceleration(&self) -> Acceleration;
    /// Whether a GPU provider is currently engaged.
    fn on_gpu(&self) -> bool;
    /// The entry file used to open this voice.
    fn path(&self) -> &Path;
    /// Sings frames, reporting progress as `(completed chunks, total chunks)`.
    fn sing_with(
        &mut self,
        frames: &SingerFrames,
        score: Option<&SingerScore>,
        speaker: u32,
        seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError>;
}

/// A loaded voice whose concrete synthesis engine is selected from its entry file.
pub struct VoiceModel {
    backend: Box<dyn SingingBackend>,
}

impl VoiceModel {
    /// Opens an Auris `.onnx`, DiffSinger `dsconfig.yaml`, or `.voicevox.json` connection.
    pub fn load(path: &Path, acceleration: Acceleration) -> Result<Self, SingError> {
        let is_diffsinger = path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("dsconfig.yaml"));
        let is_voicevox = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".voicevox.json"));
        let backend: Box<dyn SingingBackend> = if is_diffsinger {
            Box::new(crate::diffsinger::DiffSingerBackend::load(
                path,
                acceleration,
            )?)
        } else if is_voicevox {
            Box::new(crate::voicevox::VoicevoxBackend::load(path, acceleration)?)
        } else {
            Box::new(crate::model::AurisBackend::load(path, acceleration)?)
        };
        Ok(Self { backend })
    }

    /// Which synthesis engine owns this voice.
    pub fn backend_kind(&self) -> BackendKind {
        self.backend.kind()
    }

    /// The model's own account of itself.
    pub fn info(&self) -> &VoiceInfo {
        self.backend.info()
    }

    /// What [`Self::load`] was asked to run this voice on.
    pub fn acceleration(&self) -> Acceleration {
        self.backend.acceleration()
    }

    /// Whether a GPU provider is in the active inference sessions.
    pub fn on_gpu(&self) -> bool {
        self.backend.on_gpu()
    }

    /// Where the voice was loaded from.
    pub fn path(&self) -> &Path {
        self.backend.path()
    }

    /// Sings frames and returns mono samples at [`VoiceInfo::sample_rate`].
    pub fn sing(
        &mut self,
        frames: &SingerFrames,
        speaker: u32,
        seed: u64,
    ) -> Result<Vec<f32>, SingError> {
        self.sing_with(frames, speaker, seed, |_, _| true)
    }

    /// [`Self::sing`], reporting each completed inference chunk.
    pub fn sing_with(
        &mut self,
        frames: &SingerFrames,
        speaker: u32,
        seed: u64,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        self.backend
            .sing_with(frames, None, speaker, seed, &mut progress)
    }

    /// Sings a note-level score, using its parallel frame curves where the backend supports it.
    pub fn sing_score(
        &mut self,
        frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        seed: u64,
    ) -> Result<Vec<f32>, SingError> {
        self.sing_score_with(frames, score, speaker, seed, |_, _| true)
    }

    /// [`Self::sing_score`], reporting progress as the backend advances.
    pub fn sing_score_with(
        &mut self,
        frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        seed: u64,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        self.backend
            .sing_with(frames, Some(score), speaker, seed, &mut progress)
    }
}
