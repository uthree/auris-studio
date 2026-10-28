//! Captures and prepares the native authority transition against the existing session runtime.

use auris_core::pipeline::PipelineLimits;
use auris_core::pipeline::content::{CapabilityProvider, ContentLimits, ProcessorKind};
use auris_engine::native::NativeGraphGeneration;

use crate::native_edit::{NativeDocumentEditor, NativeEditError};
use crate::native_file_session::{NativeFileSession, NativeFileSessionError};
use crate::native_host::{NativeHostError, NativeHostManager, NativeHostResources};
use crate::{Session, SessionError};

/// A native candidate could not be captured or prepared for the originating session.
#[derive(Debug, thiserror::Error)]
pub enum NativeTransitionError {
    /// A gesture or agent request currently owns the conventional document.
    #[error("finish the active edit before preparing a native transition")]
    ActiveEdit,
    /// The session, revision or source contents changed after capture.
    #[error("the native transition no longer belongs to the current session document")]
    Stale,
    /// Capturing actual plugin state failed.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// Legacy conversion failed without changing the document authority.
    #[error(transparent)]
    Conversion(#[from] auris_io::PipelineConversionError),
    /// The ordinary native persistence projection could not retain authored identities.
    #[error(transparent)]
    Persistence(#[from] auris_io::NativePersistenceError),
    /// Native candidate ownership was invalid.
    #[error(transparent)]
    Edit(#[from] NativeEditError),
    /// The isolated native render candidate could not be built.
    #[error(transparent)]
    Host(#[from] NativeHostError),
    /// Source content could not be fingerprinted.
    #[error("could not fingerprint the native transition source: {0}")]
    Fingerprint(String),
    /// Native file-backed audio could not be prepared.
    #[error("could not prepare native audio: {0}")]
    Audio(String),
    /// Guarded native save, reopen or recovery failed.
    #[error(transparent)]
    File(#[from] NativeFileSessionError),
}

/// A native candidate captured once from a conventional Session, with isolated prepared hosts.
///
/// This value owns no audio device and cannot enqueue a graph or write over the source project.
/// Native edits retain persistent IDs captured from ordinary format-33 sessions. Default
/// Session editing remains conventional until all command families have native adapters.
pub struct NativeSessionTransition {
    files: NativeFileSession,
    hosts: Option<NativeHostManager>,
    prepared_revision: Option<u64>,
    origin: String,
    origin_revision: u64,
    origin_digest: String,
    content_limits: ContentLimits,
    assets: crate::native_assets::NativeAudioAssets,
    asset_document: auris_core::pipeline::document::NativeDocument,
}

impl NativeSessionTransition {
    /// File-backed sources that could not be loaded for the most recent prepared candidate.
    pub fn missing_audio(&self) -> &[crate::native_assets::NativeMissingAudio] {
        &self.assets.missing
    }
    /// Native command/history authority for this candidate only.
    pub fn editor(&self) -> &NativeDocumentEditor {
        self.files.editor()
    }

    /// Edits the native candidate without reconverting the originating Project.
    pub fn editor_mut(&mut self) -> &mut NativeDocumentEditor {
        self.files.editor_mut()
    }

    /// Native persistence and checkpoint authority, sharing this candidate's single editor.
    pub fn files(&self) -> &NativeFileSession {
        &self.files
    }

    /// Saves or restores this candidate without changing the conventional Session document.
    pub fn files_mut(&mut self) -> &mut NativeFileSession {
        &mut self.files
    }

    /// Prepared generation, only while it still describes the candidate's current revision.
    pub fn prepared_generation(&self) -> Option<NativeGraphGeneration> {
        if self.prepared_revision != Some(self.editor().revision().document) {
            return None;
        }
        self.hosts.as_ref()?.prepared_generation()
    }
}

impl Session {
    /// Captures actual hosted state and converts this document once into a native candidate.
    ///
    /// No graph is replaced and no file is written. Conversion failures leave the conventional
    /// document as authority. The source digest also detects changes that do not advance the
    /// conventional edit counter, such as a freshly captured opaque plugin state.
    pub fn capture_native_transition(
        &mut self,
        limits: ContentLimits,
    ) -> Result<NativeSessionTransition, NativeTransitionError> {
        self.require_native_transition_boundary()?;
        self.collect_hosted_state()?;
        let mut source = self.project.clone();
        source.reconcile_native_stage_ids();
        let native = auris_io::project_to_native_preserving(&source, limits)?;
        let editor = if self.is_dirty() || self.path.is_none() {
            NativeDocumentEditor::new_unsaved(native.clone(), limits)?
        } else {
            NativeDocumentEditor::new(native.clone(), limits)?
        };
        Ok(NativeSessionTransition {
            asset_document: native.clone(),
            assets: crate::native_assets::NativeAudioAssets {
                bank: self.render_bank.clone(),
                missing: Vec::new(),
            },
            files: NativeFileSession::from_editor(
                editor,
                limits,
                self.project_folder().map(std::path::Path::to_path_buf),
            ),
            hosts: None,
            prepared_revision: None,
            origin: self.sound_scope.clone(),
            origin_revision: self.revision,
            origin_digest: self
                .agent_document_revision()
                .map_err(NativeTransitionError::Fingerprint)?,
            content_limits: limits,
        })
    }

    /// Opens an explicit native candidate against this Session's existing runtime.
    ///
    /// The file's persistent IDs are read directly; no legacy projection is reconstructed.
    /// Reopening does not adopt the candidate as the default Session document or publish audio.
    pub fn open_native_transition(
        &mut self,
        path: &std::path::Path,
        limits: ContentLimits,
    ) -> Result<NativeSessionTransition, NativeTransitionError> {
        self.require_native_transition_boundary()?;
        self.collect_hosted_state()?;
        let files = NativeFileSession::open(path, limits)?;
        self.transition_with_files(files, limits)
    }

    /// Recovers a native snapshot into an unsaved candidate using this Session's runtime.
    ///
    /// Recovery copies inside assets into a leased workspace. It never overwrites the original
    /// project or adopts the autosave path as a permanent save destination.
    pub fn recover_native_transition(
        &mut self,
        snapshot: &crate::native_file_session::NativeRecoverySnapshot,
        limits: ContentLimits,
    ) -> Result<NativeSessionTransition, NativeTransitionError> {
        self.require_native_transition_boundary()?;
        self.collect_hosted_state()?;
        let files = NativeFileSession::begin_recovery(snapshot, limits)
            .run(&std::sync::atomic::AtomicBool::new(false))?
            .ok_or_else(|| NativeFileSessionError::Recovery("recovery was cancelled".into()))?;
        self.transition_with_files(files, limits)
    }

    fn transition_with_files(
        &self,
        files: NativeFileSession,
        limits: ContentLimits,
    ) -> Result<NativeSessionTransition, NativeTransitionError> {
        Ok(NativeSessionTransition {
            asset_document: files.editor().document().clone(),
            assets: crate::native_assets::NativeAudioAssets::default(),
            files,
            hosts: None,
            prepared_revision: None,
            origin: self.sound_scope.clone(),
            origin_revision: self.revision,
            origin_digest: self
                .agent_document_revision()
                .map_err(NativeTransitionError::Fingerprint)?,
            content_limits: limits,
        })
    }

    /// Prepares a native transition using this session's engine geometry, assets and plugin trust.
    ///
    /// This is the normal-session preparation seam for the authority migration. It deliberately
    /// does not queue a graph while conventional commands can still mutate the source document.
    /// Failed or stale preparation keeps the audible graph, readers and editors in place.
    pub fn prepare_native_transition<P: CapabilityProvider>(
        &mut self,
        transition: &mut NativeSessionTransition,
        provider: &P,
        graph_limits: PipelineLimits,
    ) -> Result<NativeGraphGeneration, NativeTransitionError> {
        self.require_native_transition_boundary()?;
        if transition.origin != self.sound_scope
            || transition.origin_revision != self.revision
            || transition.origin_digest
                != self
                    .agent_document_revision()
                    .map_err(NativeTransitionError::Fingerprint)?
        {
            return Err(NativeTransitionError::Stale);
        }
        self.collect_hosted_state()?;
        if transition.origin != self.sound_scope
            || transition.origin_revision != self.revision
            || transition.origin_digest
                != self
                    .agent_document_revision()
                    .map_err(NativeTransitionError::Fingerprint)?
        {
            return Err(NativeTransitionError::Stale);
        }
        let folder = transition.files.asset_folder();
        let mut plugin_paths = self.plugin_trust.extra_paths().to_vec();
        // An explicitly opened CLAP library is already authorized by the conventional host.
        // Carry that exact file permission into isolated preparation without trusting new files.
        for processor in &transition.editor().document().content.processors {
            let plugin = match &processor.kind {
                ProcessorKind::Instrument(plugin) | ProcessorKind::Effect { plugin, .. } => plugin,
                _ => continue,
            };
            if let Some(file) = plugin
                .file
                .as_ref()
                .and_then(|file| file.resolve_for_automatic_access(folder))
                && self.hosted.has_library(&file)
            {
                plugin_paths.push(file);
            }
        }
        let mut hosts = NativeHostManager::new(&plugin_paths, self.engine.is_running());
        // Decode newly landed native sources into an isolated bank. Existing rate-matched
        // buffers may be shared, but a failed candidate never changes the live render bank.
        let assets = crate::native_assets::prepare_native_audio_assets(
            transition.editor().document(),
            folder,
            self.engine.sample_rate(),
            Some((&transition.asset_document, &transition.assets.bank)),
        )
        .map_err(NativeTransitionError::Audio)?;
        let resources = NativeHostResources {
            bank: &assets.bank,
            registry: &self.registry,
            project_folder: folder,
            content_limits: transition.content_limits,
            graph_limits,
        };
        let revision = transition.editor().revision().document;
        let generation = hosts.prepare(
            &self.engine,
            transition.editor().document(),
            revision,
            provider,
            resources,
        )?;
        transition.hosts = Some(hosts);
        transition.asset_document = transition.editor().document().clone();
        transition.assets = assets;
        transition.prepared_revision = Some(revision);
        Ok(generation)
    }

    fn require_native_transition_boundary(&self) -> Result<(), NativeTransitionError> {
        if self.transaction.is_some() || self.agent_request.is_some() || self.take.is_some() {
            return Err(NativeTransitionError::ActiveEdit);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
