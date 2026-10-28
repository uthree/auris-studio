//! Landing completed recording files without a legacy project mutation.

use super::{NativeFileSession, NativeFileSessionError};
use crate::native_edit::{MAX_NATIVE_EDITS, NativeAudioEdit, NativeEditOwner, NativeEditReceipt};
use auris_core::{AssetPath, Ticks, TrackId};

/// One completed recorder output and the retained window, measured at the document sample rate.
#[derive(Clone, Debug)]
pub struct NativeRecordingFile {
    /// Existing destination audio track.
    pub track: TrackId,
    /// Recorder output stored relative to the current asset folder when possible.
    pub path: AssetPath,
    /// Position after count-in and punch trimming.
    pub start: Ticks,
    /// Count-in and leading punch frames retained on disk but excluded from playback.
    pub offset_frames: u64,
    /// Retained source frames, or the remainder of the file.
    pub length_frames: Option<u64>,
}

impl NativeFileSession {
    /// Reads finalized WAV files and lands all retained windows in one native undo step.
    ///
    /// Run after the recorder has closed its files, off the audio thread. A decode, stale-token
    /// or destination failure leaves the document and its allocation counters unchanged. The
    /// full file remains available for later trim edits; decoded metadata describes that file.
    pub fn land_recorded_files(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        files: &[NativeRecordingFile],
    ) -> Result<NativeEditReceipt, NativeFileSessionError> {
        if expected_revision != self.editor.revision().document {
            return Err(crate::native_edit::NativeEditError::StaleRevision {
                expected: expected_revision,
                current: self.editor.revision().document,
            }
            .into());
        }
        if self
            .editor
            .transaction_owner()
            .is_some_and(|active| active != owner)
        {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        if files.len() > MAX_NATIVE_EDITS {
            return Err(crate::native_edit::NativeEditError::Content(
                auris_core::pipeline::content::ContentError::Limit("command_operations"),
            )
            .into());
        }
        let mut takes = Vec::with_capacity(files.len());
        for file in files {
            let path = file
                .path
                .resolve_for_automatic_access(self.asset_folder())
                .ok_or_else(|| {
                    NativeFileSessionError::AssetCopy(
                        "recording path cannot be resolved automatically".into(),
                    )
                })?;
            let buffer =
                auris_io::import_audio_file(&path, self.editor.document().context.sample_rate)
                    .map_err(|error| NativeFileSessionError::AssetCopy(error.to_string()))?;
            let frame_count = buffer.frame_count() as u64;
            let name = path
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Take".into());
            takes.push(NativeAudioEdit {
                track: file.track,
                name,
                path: file.path.clone(),
                frame_count,
                sample_rate: buffer.sample_rate(),
                channel_count: buffer.channel_count(),
                byte_size: std::fs::metadata(&path)
                    .map_err(|error| NativeFileSessionError::AssetCopy(error.to_string()))?
                    .len(),
                provenance: Default::default(),
                start: file.start,
                offset_frames: file.offset_frames,
                length_frames: file
                    .length_frames
                    .unwrap_or_else(|| frame_count.saturating_sub(file.offset_frames)),
                gain_db: 0.0,
                source_bpm: Some(self.editor.document().context.tempo_map.bpm_at(file.start)),
                follows_tempo: false,
            });
        }
        Ok(self
            .editor
            .land_recorded_audio(expected_revision, owner, &takes)?)
    }
}
