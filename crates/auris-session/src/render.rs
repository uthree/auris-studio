//! Offline rendering, detached from the session that produced it.

use std::path::Path;
use std::sync::Arc;

use auris_core::param::gain_to_db;
use auris_core::{AudioBuffer, AudioSourceBank, PluginRegistry, Project};
use auris_engine::{OfflineOptions, render_project_with_progress};
use auris_io::{WavExportSettings, write_wav};

use crate::error::SessionError;

/// What an export produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSummary {
    /// Length of the rendered audio in seconds, including the effect tail.
    pub seconds: f64,
    /// Frames written per channel.
    pub frames: u64,
    /// Channels written.
    pub channels: usize,
    /// Loudest sample in the render, in dBFS.
    pub peak_db: f32,
}

/// A self-contained copy of everything a render needs.
///
/// Rendering a long project takes seconds, so a GUI runs it off the main thread — but the
/// session is not `Send` in spirit (it owns an audio device) and must stay editable meanwhile.
/// A job captures the document, the sample bank and the registry, all of which are cheap to
/// clone and `Send`, so the render sees a consistent snapshot and later edits cannot disturb it.
#[derive(Clone)]
pub struct RenderJob {
    project: Project,
    bank: AudioSourceBank,
    registry: Arc<PluginRegistry>,
}

impl RenderJob {
    pub(crate) fn new(
        project: Project,
        bank: AudioSourceBank,
        registry: Arc<PluginRegistry>,
    ) -> Self {
        Self {
            project,
            bank,
            registry,
        }
    }

    /// The document this job will render.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Renders to a buffer, reporting progress from 0.0 to 1.0.
    pub fn render(
        &self,
        options: &OfflineOptions,
        progress: &mut dyn FnMut(f32),
    ) -> Result<AudioBuffer, SessionError> {
        Ok(render_project_with_progress(
            &self.project,
            &self.bank,
            &self.registry,
            options,
            progress,
        )?)
    }

    /// Renders and writes a WAV file.
    ///
    /// The file is written at the rate the project was rendered at, not at whatever the
    /// settings say, because the two disagreeing would resample the audio by accident: the
    /// header would claim a rate the samples were never produced at.
    pub fn render_to_wav(
        &self,
        path: &Path,
        settings: &WavExportSettings,
        options: &OfflineOptions,
        progress: &mut dyn FnMut(f32),
    ) -> Result<ExportSummary, SessionError> {
        let buffer = self.render(options, progress)?;
        let rendered_rate = options.sample_rate.unwrap_or(self.project.sample_rate);
        let settings = WavExportSettings {
            sample_rate: rendered_rate.round().max(1.0) as u32,
            ..*settings
        };
        write_wav(path, &buffer, &settings)?;
        Ok(ExportSummary {
            seconds: buffer.duration_seconds(),
            frames: buffer.frame_count() as u64,
            channels: buffer.channel_count(),
            peak_db: gain_to_db(buffer.peak()),
        })
    }
}

impl std::fmt::Debug for RenderJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderJob")
            .field("project", &self.project.name)
            .field("tracks", &self.project.tracks.len())
            .finish()
    }
}
