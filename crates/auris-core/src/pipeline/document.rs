//! Standalone native document envelope.
//!
//! This type deliberately does not contain a [`crate::Project`]. The legacy score remains the
//! source format until activation; a converted document has one authoritative native ownership
//! graph and keeps the common timeline/context fields beside it. This makes conversion and a
//! serde round trip testable without changing the project writer or playback activation.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::content::{
    ContentError, ContentLimits, ContentRef, PipelineContent, ProcessorKind, SourceContent,
};
use crate::project::{
    AudioSource, Color, CompositionBaseline, MixerGraphView, SoundFontId, SoundFontRef, TrackId,
    TrackKind,
};
use crate::{ClipId, Harmony, SectionMap, SignatureMap, TempoMap, Ticks};

/// The presentation kind of one ordered arrangement track.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTrackKind {
    /// Pitched instrument material.
    Instrument,
    /// Percussion material, kept distinct from a melodic instrument.
    Drum,
    /// Recorded/imported audio.
    Audio,
    /// Offline singing material.
    Singer,
    /// A mixer bus with no authored source.
    Bus,
}

impl From<&TrackKind> for NativeTrackKind {
    fn from(kind: &TrackKind) -> Self {
        match kind {
            TrackKind::Instrument(_) => Self::Instrument,
            TrackKind::Drum(_) => Self::Drum,
            TrackKind::Audio(_) => Self::Audio,
            TrackKind::Singer(_) => Self::Singer,
            TrackKind::Bus => Self::Bus,
        }
    }
}

/// Ordered arrangement presentation, separate from native ownership and routing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackPresentation {
    /// Existing stable track identity.
    pub id: TrackId,
    /// User-facing name.
    pub name: String,
    /// User-facing tint.
    pub color: Color,
    /// Lane height in pixels.
    pub height: f32,
    /// Explicit kind, never inferred from processor order.
    pub kind: NativeTrackKind,
}

/// Project-level context and registries retained beside the native graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeProjectContext {
    /// Source project schema version; the native envelope is not a replacement writer format.
    pub source_format_version: u32,
    /// Informational build stamp.
    pub saved_by: String,
    /// Document name.
    pub name: String,
    /// Render rate.
    pub sample_rate: f64,
    /// Timeline maps shared by every track.
    pub tempo_map: TempoMap,
    /// Meter map.
    pub signatures: SignatureMap,
    /// Key/chord map.
    pub harmony: Harmony,
    /// Named timeline sections.
    pub sections: SectionMap,
    /// Imported audio metadata.
    pub audio_sources: BTreeMap<crate::SourceId, AudioSource>,
    /// Imported SoundFont metadata.
    pub soundfonts: BTreeMap<SoundFontId, SoundFontRef>,
    /// Loop range and enabled state.
    pub loop_region: Option<(Ticks, Ticks)>,
    /// Whether looping is enabled.
    pub loop_enabled: bool,
    /// Punch range and enabled state.
    pub punch_region: Option<(Ticks, Ticks)>,
    /// Whether punch recording is enabled.
    pub punch_enabled: bool,
    /// Metronome state.
    pub metronome: bool,
    /// Count-in bars.
    pub count_in_bars: u32,
    /// Editing grid.
    pub grid: Ticks,
    /// Composer inputs and audit metadata.
    pub song_spec: Option<String>,
    /// Original creative brief.
    pub creation_brief: Option<String>,
    /// Initial composition comparison metadata.
    pub composition_baseline: Option<CompositionBaseline>,
    /// External transcription claims keyed by clip ID.
    pub transcription_provenance: BTreeMap<ClipId, String>,
    /// Mixer graph camera/layout state.
    pub mixer_graph_view: MixerGraphView,
    /// Legacy allocation high-water mark, retained separately from the native allocator.
    pub source_next_id: u64,
}

/// A converted document with one native ownership graph and no embedded legacy score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeDocument {
    /// Common project fields and external registries.
    pub context: NativeProjectContext,
    /// Ordered track presentation metadata.
    pub track_presentation: Vec<TrackPresentation>,
    /// Native ownership and topology content.
    pub content: PipelineContent,
}

impl NativeDocument {
    /// Validates context identities, track roles and the complete ownership fragment.
    ///
    /// Asset paths and plugin files are retained without filesystem IO. Missing asset contents
    /// are a preparation diagnostic; this check does not establish render readiness.
    pub fn validate_ownership(&self, limits: ContentLimits) -> Result<(), ContentError> {
        if self.track_presentation.len() > limits.tracks {
            return Err(ContentError::Limit("tracks"));
        }
        if self
            .context
            .audio_sources
            .len()
            .saturating_add(self.context.soundfonts.len())
            > limits.external_ids
        {
            return Err(ContentError::Limit("external_ids"));
        }
        for (id, source) in &self.context.audio_sources {
            if *id != source.id {
                return Err(ContentError::DuplicateId(id.0));
            }
        }
        for (id, font) in &self.context.soundfonts {
            if *id != font.id {
                return Err(ContentError::DuplicateId(id.0));
            }
        }
        self.content
            .validate_ownership(&self.external_ids(), limits)?;
        let mut seen = BTreeSet::new();
        for presentation in &self.track_presentation {
            if !seen.insert(presentation.id) {
                return Err(ContentError::DuplicateId(presentation.id.0));
            }
            let reference = ContentRef::Track(presentation.id);
            let binding = self
                .content
                .tracks
                .iter()
                .find(|track| track.track == presentation.id)
                .ok_or(ContentError::Missing(reference))?;
            let group = self
                .content
                .groups
                .iter()
                .find(|group| group.id == binding.group)
                .ok_or(ContentError::Missing(ContentRef::Group(binding.group)))?;
            let source = binding
                .source
                .and_then(|id| self.content.sources.iter().find(|source| source.id == id));
            let correct_source = matches!(
                (presentation.kind, source.map(|source| &source.content)),
                (NativeTrackKind::Bus, None)
                    | (NativeTrackKind::Audio, Some(SourceContent::Audio(_)))
                    | (
                        NativeTrackKind::Instrument
                            | NativeTrackKind::Drum
                            | NativeTrackKind::Singer,
                        Some(SourceContent::Notes(_)),
                    )
            );
            let needs_instrument = matches!(
                presentation.kind,
                NativeTrackKind::Instrument | NativeTrackKind::Drum | NativeTrackKind::Singer
            );
            if !correct_source || needs_instrument != group.primary_instrument.is_some() {
                return Err(ContentError::Role(
                    reference,
                    "track presentation disagrees with its source or primary instrument",
                ));
            }
            if presentation.kind == NativeTrackKind::Singer
                && !self.content.processors.iter().any(|node| {
                    node.group == Some(group.id) && matches!(node.kind, ProcessorKind::Singer(_))
                })
            {
                return Err(ContentError::Role(
                    reference,
                    "singer track requires an owned singer processor",
                ));
            }
        }
        if let Some(track) = self
            .content
            .tracks
            .iter()
            .find(|track| !seen.contains(&track.track))
        {
            return Err(ContentError::Missing(ContentRef::Track(track.track)));
        }
        Ok(())
    }

    /// Returns all external IDs used by the context, for ownership validation/counter repair.
    pub fn external_ids(&self) -> Vec<u64> {
        self.context
            .audio_sources
            .keys()
            .map(|id| id.0)
            .chain(self.context.soundfonts.keys().map(|id| id.0))
            .collect()
    }
}
