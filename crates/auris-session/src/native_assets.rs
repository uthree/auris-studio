//! Isolated preparation of file-backed audio for a native document.
//!
//! Preparation owns a candidate bank only. It never mutates a live session bank, and a missing
//! file is a diagnostic rather than a document failure: the graph can still be prepared with the
//! affected clip silent.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_core::AudioSourceBank;
use auris_core::pipeline::content::SourceContent;
use auris_core::pipeline::document::NativeDocument;
use auris_core::project::{UNSTRETCHED, stretch_key};

/// One native audio source that could not be loaded automatically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMissingAudio {
    /// Stable source identity.
    pub source: auris_core::SourceId,
    /// Resolved path, when the reference could be resolved without prompting.
    pub path: Option<PathBuf>,
    /// Human-readable diagnostic from resolution or decoding.
    pub reason: String,
}

/// Isolated decoded sources for a native render candidate.
#[derive(Clone, Debug, Default)]
pub struct NativeAudioAssets {
    /// Decoded source and stretch cache used only by the candidate graph.
    pub bank: AudioSourceBank,
    /// Sources that were unavailable during preparation.
    pub missing: Vec<NativeMissingAudio>,
}

/// Prepares all native audio sources at `rate` without converting through the legacy `Project`.
///
/// `previous` may contain a document and bank from an earlier candidate. A matching source is
/// shared by `Arc` after its metadata is checked; changed paths or metadata are decoded afresh.
/// The returned bank is always a new bank, so callers may discard it on a stale transition.
pub fn prepare_native_audio_assets(
    document: &NativeDocument,
    project_folder: Option<&Path>,
    rate: f64,
    previous: Option<(&NativeDocument, &AudioSourceBank)>,
) -> Result<NativeAudioAssets, String> {
    if !rate.is_finite() || rate <= 0.0 {
        return Err("native audio render rate must be finite and positive".into());
    }
    let mut prepared = NativeAudioAssets::default();
    for (id, source) in &document.context.audio_sources {
        let path = source.path.resolve_for_automatic_access(project_folder);
        if let Some((old_document, old_bank)) = previous
            && old_document
                .context
                .audio_sources
                .get(id)
                .is_some_and(|old| old == source)
            && let Some(buffer) = old_bank.get(*id)
            && (buffer.sample_rate() - rate).abs() <= f64::EPSILON
        {
            prepared.bank.insert(*id, Arc::clone(buffer));
            continue;
        }
        let Some(path) = path else {
            prepared.missing.push(NativeMissingAudio {
                source: *id,
                path: None,
                reason: "asset requires explicit access or a project folder".into(),
            });
            continue;
        };
        match auris_io::import_audio_file(&path, rate) {
            Ok(buffer) => {
                if buffer.frame_count() == 0 || buffer.channel_count() == 0 {
                    prepared.missing.push(NativeMissingAudio {
                        source: *id,
                        path: Some(path),
                        reason: "decoded audio is empty".into(),
                    });
                } else {
                    prepared.bank.insert(*id, Arc::new(buffer));
                }
            }
            Err(error) => prepared.missing.push(NativeMissingAudio {
                source: *id,
                path: Some(path),
                reason: error.to_string(),
            }),
        }
    }
    fill_native_stretches(document, &mut prepared.bank, previous);
    Ok(prepared)
}

fn fill_native_stretches(
    document: &NativeDocument,
    bank: &mut AudioSourceBank,
    previous: Option<(&NativeDocument, &AudioSourceBank)>,
) {
    let mut wanted = HashSet::new();
    for source_document in &document.content.sources {
        let SourceContent::Audio(clips) = &source_document.content else {
            continue;
        };
        for clip in clips {
            let stretch = clip.stretch_in(&document.context.tempo_map);
            let key = stretch_key(stretch);
            if key == UNSTRETCHED || !wanted.insert((clip.source, key)) {
                continue;
            }
            if bank.stretched(clip.source, key).is_some() {
                continue;
            }
            if let Some((old_document, old_bank)) = previous
                && old_document.context.audio_sources.get(&clip.source)
                    == document.context.audio_sources.get(&clip.source)
                && let Some(cached) = old_bank.stretched(clip.source, key)
                && bank
                    .get(clip.source)
                    .is_some_and(|source| source.sample_rate() == cached.sample_rate())
            {
                bank.insert_stretched(clip.source, key, Arc::clone(cached));
                continue;
            }
            let Some(source) = bank.get(clip.source) else {
                continue;
            };
            bank.insert_stretched(
                clip.source,
                key,
                Arc::new(auris_dsp::stretch::time_stretch(source, stretch)),
            );
        }
    }
    bank.retain_stretched(&|source, key| wanted.contains(&(source, key)));
}

#[cfg(test)]
mod tests;
