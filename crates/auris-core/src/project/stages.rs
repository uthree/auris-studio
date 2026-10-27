//! Stable identities for clip-local non-destructive performance stages.
//!
//! The current project format stores an ordered transform stack without stage identities. This
//! module provides the format-independent native representation used during M2 preparation. It
//! does not alter the format-32 writer or runtime track model; conversion is explicit and keeps
//! the existing performer arithmetic as the source of truth.

use serde::{Deserialize, Serialize};

use super::{ClipId, MidiClip, NoteTransform};

/// Stable identity for one ordered transform entry inside a clip.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct StageId(pub u64);

/// A clip-local transform with an identity that survives reordering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipStage {
    /// Stable identity allocated by the native document allocator.
    pub id: StageId,
    /// Existing non-destructive performer payload and its seed.
    pub transform: NoteTransform,
}

/// Native-preparation clip shape with stable stage entries.
pub type StableMidiClip = MidiClip<ClipStage>;

/// Failure while converting or editing stable clip stages.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    /// The supplied IDs do not match the transform count.
    #[error("clip {clip:?} has {stages} transforms but {ids} stage IDs were supplied")]
    LengthMismatch {
        /// The clip whose transform stack was being converted.
        clip: ClipId,
        /// Number of transforms in the clip.
        stages: usize,
        /// Number of supplied stable IDs.
        ids: usize,
    },
    /// One stage identity occurs more than once.
    #[error("duplicate stage ID {0:?} in clip")]
    DuplicateId(StageId),
    /// A requested stage identity was not found.
    #[error("stage {stage:?} is not present in clip {clip:?}")]
    UnknownId {
        /// The clip whose stage stack was being edited.
        clip: ClipId,
        /// The requested stage identity.
        stage: StageId,
    },
    /// An ID of zero is reserved for an unassigned stage.
    #[error("stage ID zero is not a persistent identity")]
    ZeroId,
}

impl MidiClip {
    /// Wraps each legacy transform with caller-supplied stable IDs.
    ///
    /// The supplied IDs are validated before the clip is converted, so conversion cannot produce
    /// a partially assigned stack. Existing transform order, payloads, seeds and all clip
    /// metadata are retained exactly.
    pub fn with_stage_ids(self, ids: Vec<StageId>) -> Result<StableMidiClip, StageError> {
        if self.transforms.len() != ids.len() {
            return Err(StageError::LengthMismatch {
                clip: self.id,
                stages: self.transforms.len(),
                ids: ids.len(),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for id in &ids {
            if id.0 == 0 {
                return Err(StageError::ZeroId);
            }
            if !seen.insert(*id) {
                return Err(StageError::DuplicateId(*id));
            }
        }
        let mut index = 0;
        Ok(self.map_transforms(|transform| {
            // Length equality above bounds every access; mapping visits each transform once.
            let id = ids[index];
            index += 1;
            ClipStage { id, transform }
        }))
    }
}

impl MidiClip<ClipStage> {
    /// Converts this native-preparation clip back to the existing performer shape.
    pub fn performer_clip(&self) -> MidiClip {
        self.clone().map_transforms(|stage| stage.transform)
    }

    /// Moves one stage by a signed offset, retaining its identity and payload.
    pub fn move_stage(&mut self, stage: StageId, offset: isize) -> Result<bool, StageError> {
        let index = self.stage_index(stage)?;
        let target = index
            .saturating_add_signed(offset)
            .clamp(0, self.transforms.len().saturating_sub(1));
        if index == target {
            return Ok(false);
        }
        let entry = self.transforms.remove(index);
        self.transforms.insert(target, entry);
        Ok(true)
    }

    /// Removes a stage and returns its transform payload.
    pub fn remove_stage(&mut self, stage: StageId) -> Result<ClipStage, StageError> {
        let index = self.stage_index(stage)?;
        Ok(self.transforms.remove(index))
    }

    /// Returns the ordered position of a stable stage identity.
    pub fn stage_index(&self, stage: StageId) -> Result<usize, StageError> {
        self.validate_stage_ids()?;
        if stage.0 == 0 {
            return Err(StageError::ZeroId);
        }
        self.transforms
            .iter()
            .position(|entry| entry.id == stage)
            .ok_or(StageError::UnknownId {
                clip: self.id,
                stage,
            })
    }

    /// Validates uniqueness and nonzero identity before a native candidate is committed.
    pub fn validate_stage_ids(&self) -> Result<(), StageError> {
        let mut seen = std::collections::BTreeSet::new();
        for stage in &self.transforms {
            if stage.id.0 == 0 {
                return Err(StageError::ZeroId);
            }
            if !seen.insert(stage.id) {
                return Err(StageError::DuplicateId(stage.id));
            }
        }
        Ok(())
    }

    /// Appends a new stage after validating the complete existing identity set.
    pub fn append_stage(
        &mut self,
        id: StageId,
        transform: NoteTransform,
    ) -> Result<(), StageError> {
        self.validate_stage_ids()?;
        if id.0 == 0 {
            return Err(StageError::ZeroId);
        }
        if self.transforms.iter().any(|stage| stage.id == id) {
            return Err(StageError::DuplicateId(id));
        }
        self.transforms.push(ClipStage { id, transform });
        Ok(())
    }

    /// Replaces a stage's transform while preserving its stable identity.
    pub fn replace_stage_transform(
        &mut self,
        id: StageId,
        transform: NoteTransform,
    ) -> Result<(), StageError> {
        let index = self.stage_index(id)?;
        self.transforms[index].transform = transform;
        Ok(())
    }
}
