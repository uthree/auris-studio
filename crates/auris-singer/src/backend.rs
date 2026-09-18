//! The common door every singing engine presents to the session.

use std::path::Path;

use auris_vocal::{SingerFrames, SingerScore};

use crate::{Acceleration, CurveGenerator, CurveSources, SingError, VoiceInfo};

/// The waveform and optional backend pitch produced by one synthesis call.
#[derive(Clone, Debug, Default)]
pub struct SingingRender {
    /// Mono waveform at the voice's sample rate.
    pub samples: Vec<f32>,
    /// Pitch after musical edits, with decoder context removed just like the audio.
    pub backend_pitch: Option<auris_core::SingerPitch>,
}

/// A singing engine understood by Auris Studio.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// Auris' self-contained ONNX voice format.
    Auris,
    /// An OpenUtau-compatible DiffSinger voicebank.
    DiffSinger,
    /// A running VOICEVOX Engine reached through its HTTP API.
    Voicevox,
    /// A LeapSinger acoustic ONNX model paired with an NHVSing vocoder.
    LeapSinger,
}

impl BackendKind {
    /// The backend selected by an entry file's name, without opening the file.
    pub fn from_path(path: &Path) -> Self {
        if path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("dsconfig.yaml"))
        {
            Self::DiffSinger
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".voicevox.json"))
        {
            Self::Voicevox
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".leapsinger.json"))
        {
            Self::LeapSinger
        } else {
            Self::Auris
        }
    }

    /// The score corrections this backend consumes during synthesis.
    pub fn capabilities(self) -> VoiceCapabilities {
        let direct_phonemes = self != Self::Voicevox;
        VoiceCapabilities {
            manual_phonemes: direct_phonemes,
            phoneme_timing: direct_phonemes,
            curves: match self {
                Self::Voicevox => crate::voicevox::VoicevoxBackend::SOURCES,
                Self::Auris | Self::DiffSinger | Self::LeapSinger => CurveSources::default(),
            },
        }
    }
}

/// Editable vocal details that a synthesis backend can honour.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VoiceCapabilities {
    /// Who supplies each curve's acoustic articulation during synthesis.
    pub curves: CurveSources,
    /// Whether manually corrected IPA phonemes affect the voice's pronunciation.
    pub manual_phonemes: bool,
    /// Whether per-phoneme duration pins affect the voice's pronunciation timing.
    pub phoneme_timing: bool,
}

/// The backend contract: metadata, frame curves and an optional note score in; mono waveform out.
///
/// Implementations may use one model or a pipeline of models. They are always called off the
/// realtime audio thread. The trait is public so another engine can be added without teaching
/// the session or its frontends about that engine's files and tensors.
pub trait SingingBackend: Send {
    /// Sings with optional acoustic pitch for display alongside the host's contour.
    /// Backends with pitch prediction override this and return the decoded performance's
    /// pitch on the input frame clock. Waveform-only backends use the default.
    fn sing_render_with(
        &mut self,
        frames: &SingerFrames,
        score: Option<&SingerScore>,
        speaker: u32,
        seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<SingingRender, SingError> {
        self.sing_with(frames, score, speaker, seed, progress)
            .map(|samples| SingingRender {
                samples,
                backend_pitch: None,
            })
    }
    /// Which file format and inference pipeline this backend implements.
    fn kind(&self) -> BackendKind;
    /// Capabilities of this loaded model, overriding format defaults when needed.
    ///
    /// A native model with an optional curve predictor can report its own sources here.
    /// Curve generators should use `CurveGenerator::curve_sources` in this result.
    fn capabilities(&self) -> VoiceCapabilities {
        self.kind().capabilities()
    }
    /// The voice information shared with the document and frontends.
    fn info(&self) -> &VoiceInfo;
    /// What processor preference the backend was opened with.
    fn acceleration(&self) -> Acceleration;
    /// Whether a GPU provider is currently engaged.
    fn on_gpu(&self) -> bool;
    /// The entry file used to open this voice.
    fn path(&self) -> &Path;
    /// Whether this already-loaded voice is safe to reuse for background synthesis.
    ///
    /// Native voices have no subordinate resources. Manifest-backed implementations override
    /// this for child paths or service URLs that an explicit load was allowed to use.
    fn automatic_access_safe(&self) -> bool {
        true
    }
    /// Sings frames, reporting progress as `(completed chunks, total chunks)`.
    ///
    /// Sample score-bearing frames with the sources in [`Self::capabilities`]: host-owned
    /// arrays are acoustic features, backend-owned arrays are musical controls. A predictor
    /// resolves those controls through [`crate::CurveGenerator::prepare_curves`].
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
    /// Renders a score, retaining any backend pitch alongside its mono audio.
    pub fn sing_render_with(
        &mut self,
        frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        seed: u64,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<SingingRender, SingError> {
        self.backend
            .sing_render_with(frames, Some(score), speaker, seed, &mut progress)
    }
    /// Wraps an engine implementation, including its model-specific capabilities.
    pub fn from_backend(backend: impl SingingBackend + 'static) -> Self {
        Self {
            backend: Box::new(backend),
        }
    }

    /// Capabilities reported by the loaded model rather than inferred from its file name.
    pub fn capabilities(&self) -> VoiceCapabilities {
        self.backend.capabilities()
    }

    /// Opens an Auris `.onnx`, DiffSinger `dsconfig.yaml`, `.voicevox.json` connection,
    /// or `.leapsinger.json` voicebank manifest.
    pub fn load(path: &Path, acceleration: Acceleration) -> Result<Self, SingError> {
        Self::load_with_access(path, acceleration, false)
    }

    /// Opens a voice for work triggered by a document rather than a fresh user action.
    ///
    /// In this mode the entry and each existing ancestor must not be a symlink or Windows reparse
    /// point, a manifest may only name resources below its voicebank folder, and a VOICEVOX
    /// connection may only address the loopback host. Rejection happens before the entry,
    /// subordinate file or HTTP service is opened.
    pub fn load_for_automatic_access(
        path: &Path,
        acceleration: Acceleration,
    ) -> Result<Self, SingError> {
        let path = crate::validate_automatic_voice_entry(path)?;
        Self::load_with_access(&path, acceleration, true)
    }

    fn load_with_access(
        path: &Path,
        acceleration: Acceleration,
        automatic: bool,
    ) -> Result<Self, SingError> {
        let backend: Box<dyn SingingBackend> = match BackendKind::from_path(path) {
            BackendKind::DiffSinger => Box::new(crate::diffsinger::DiffSingerBackend::load(
                path,
                acceleration,
                automatic,
            )?),
            BackendKind::Voicevox => Box::new(crate::voicevox::VoicevoxBackend::load(
                path,
                acceleration,
                automatic,
            )?),
            BackendKind::Auris => Box::new(crate::model::AurisBackend::load(path, acceleration)?),
            BackendKind::LeapSinger => Box::new(crate::leapsinger::LeapSingerBackend::load(
                path,
                acceleration,
                automatic,
            )?),
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

    /// Whether this loaded voice may be reused for background synthesis without reopening it.
    ///
    /// An explicitly loaded manifest can legitimately point outside its folder or at a remote
    /// VOICEVOX Engine. The cache must still ask this before an automatic render reuses it.
    pub fn automatic_access_safe(&self) -> bool {
        self.backend.automatic_access_safe()
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
    ///
    /// Sample the parallel frames with [`auris_vocal::render_frames_with_sources`] and this
    /// model's [`Self::capabilities`], so predicted articulation receives only musical edits.
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct NativePredictor(VoiceInfo);

    fn temp_entry_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "auris-singer-entry-policy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn create_directory_redirect(target: &Path, link: &Path) {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).unwrap();
        }
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "could not create the junction fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn remove_directory_redirect(link: &Path) {
        #[cfg(unix)]
        std::fs::remove_file(link).unwrap();
        #[cfg(windows)]
        std::fs::remove_dir(link).unwrap();
    }

    impl SingingBackend for NativePredictor {
        fn kind(&self) -> BackendKind {
            BackendKind::Auris
        }
        fn capabilities(&self) -> VoiceCapabilities {
            let mut capabilities = self.kind().capabilities();
            capabilities.curves.pitch = crate::CurveSource::Backend;
            capabilities
        }
        fn info(&self) -> &VoiceInfo {
            &self.0
        }
        fn acceleration(&self) -> Acceleration {
            Acceleration::Cpu
        }
        fn on_gpu(&self) -> bool {
            false
        }
        fn path(&self) -> &Path {
            Path::new("native-with-predictor.onnx")
        }
        fn sing_with(
            &mut self,
            _: &SingerFrames,
            _: Option<&SingerScore>,
            _: u32,
            _: u64,
            _: &mut dyn FnMut(usize, usize) -> bool,
        ) -> Result<Vec<f32>, SingError> {
            panic!("reading capabilities must not run inference")
        }
    }

    #[test]
    fn a_native_model_can_report_a_predictor_without_changing_format_defaults() {
        let info = serde_json::from_value(serde_json::json!({
            "format_version": crate::FORMAT_VERSION,
            "sample_rate": 24000, "hop_length": 256, "inter_channels": 1,
            "symbols": ["<sil>", "<unk>"]
        }))
        .unwrap();
        let model = VoiceModel::from_backend(NativePredictor(info));
        assert_eq!(model.backend_kind(), BackendKind::Auris);
        assert_eq!(
            model.capabilities().curves.pitch,
            crate::CurveSource::Backend
        );
        assert_eq!(model.capabilities().curves.energy, crate::CurveSource::Host);
        assert_eq!(
            BackendKind::Auris.capabilities().curves,
            CurveSources::default()
        );
    }

    #[test]
    fn capabilities_follow_the_backend_that_consumes_the_score() {
        for (entry, kind, phonemes) in [
            ("voice.onnx", BackendKind::Auris, true),
            ("bank/DSCONFIG.YAML", BackendKind::DiffSinger, true),
            ("voice.VOICEVOX.JSON", BackendKind::Voicevox, false),
            ("voice.LEAPSINGER.JSON", BackendKind::LeapSinger, true),
        ] {
            let backend = BackendKind::from_path(Path::new(entry));
            assert_eq!(backend, kind);
            assert_eq!(backend.capabilities().manual_phonemes, phonemes);
            assert_eq!(backend.capabilities().phoneme_timing, phonemes);
            let expected = if kind == BackendKind::Voicevox {
                crate::voicevox::VoicevoxBackend::SOURCES
            } else {
                CurveSources::default()
            };
            assert_eq!(backend.capabilities().curves, expected);
        }
    }

    #[test]
    fn automatic_load_rejects_a_redirected_ancestor_without_changing_explicit_load() {
        let root = temp_entry_root();
        std::fs::create_dir(&root).unwrap();
        let target = root.join("target");
        let redirect = root.join("redirect");
        std::fs::create_dir(&target).unwrap();
        let target_entry = target.join("entry.voicevox.json");
        let entry = redirect.join("entry.voicevox.json");
        let manifest = serde_json::json!({
            "format_version": 1,
            "name": "Singer",
            "url": "http://127.0.0.1:50021",
            "sample_rate": 24_000,
            "frame_rate": 93.75,
            "styles": [{"name":"Style","query_style_id":1,"decode_style_id":2}],
        });
        std::fs::write(&target_entry, serde_json::to_vec(&manifest).unwrap()).unwrap();
        create_directory_redirect(&target, &redirect);

        let automatic_error =
            VoiceModel::load_for_automatic_access(&entry, Acceleration::Cpu).err();
        let explicit_error = VoiceModel::load(&entry, Acceleration::Cpu).err();

        remove_directory_redirect(&redirect);
        std::fs::remove_file(target_entry).unwrap();
        std::fs::remove_dir(target).unwrap();
        std::fs::remove_dir(root).unwrap();
        assert!(matches!(
            automatic_error,
            Some(SingError::UnsafeAutomaticAccess { .. })
        ));
        assert!(
            explicit_error.is_none(),
            "an explicit user load retains redirected-path compatibility: {explicit_error:?}"
        );
    }
}
