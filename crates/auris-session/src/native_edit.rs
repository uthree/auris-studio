//! Native document commands and snapshot history, independent of runtime preparation.
//!
//! This is the mutation boundary for native command adapters. It owns no legacy `Project` and
//! does not convert a document after edits. Runtime preparation may fail while the authored
//! document remains editable, including when a referenced plugin is unavailable.

mod audio;
mod clips;
mod commands;
mod timeline;
mod tracks;

pub use audio::NativeAudioEdit;
pub use clips::NativeClipEdit;
pub use commands::NativeEdit;
pub use timeline::NativeTimelineEdit;
pub use tracks::{NativeTrackEdit, NativeTrackKindSpec};

#[cfg(test)]
mod audio_tests;
#[cfg(test)]
mod clips_tests;
#[cfg(test)]
mod tracks_tests;

use std::collections::HashSet;
use std::collections::VecDeque;

use auris_core::pipeline::content::{ContentError, ContentLimits};
use auris_core::pipeline::document::NativeDocument;

use crate::Edit;

/// A stable identity created by a successfully committed native batch, in operation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCreated {
    /// Timeline track identity.
    Track(auris_core::TrackId),
    /// Clip identity.
    Clip(auris_core::ClipId),
    /// Audio asset registry identity.
    AudioSource(auris_core::SourceId),
    /// Shared authored source identity.
    Source(auris_core::pipeline::content::SourceDocumentId),
    /// Processing group identity.
    Group(auris_core::pipeline::content::GroupId),
    /// Processor identity.
    Node(auris_core::pipeline::NodeId),
    /// Clip-local performance stage identity.
    Stage(auris_core::project::StageId),
    /// Effect slot identity.
    Effect(auris_core::EffectSlotId),
    /// Send identity.
    Send(auris_core::SendId),
    /// Connection identity.
    Edge(auris_core::pipeline::EdgeId),
}

/// Maximum number of operations committed as one native command batch.
pub const MAX_NATIVE_EDITS: usize = 256;
/// Maximum encoded payload for one native edit batch, before cloning the document.
pub const MAX_NATIVE_EDIT_BYTES: usize = 1 << 20;
const HISTORY_LIMIT: usize = 64;

/// Owner of a native edit gesture; distinct agent requests cannot share a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeEditOwner {
    /// A pointer/keyboard gesture owned by the person editing the document.
    Human,
    /// A session-local agent request identity.
    Agent(u64),
}

/// Monotonic document and audible-content revisions, independent of graph adoption.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeEditRevision {
    /// Every authored change, including presentation changes and history traversal.
    pub document: u64,
    /// Changes that may require a different render plan.
    pub musical: u64,
}

/// Outcome of one atomic document operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeEditReceipt {
    /// Revisions after the operation.
    pub revision: NativeEditRevision,
    /// Whether authored content or presentation changed.
    pub changed: bool,
    /// Whether the musical revision advanced; this is not proof of callback adoption.
    pub needs_prepare: bool,
    /// New identities, present only after a changed batch has committed successfully.
    pub created: Vec<NativeCreated>,
}

/// A rejected native operation leaves the document, history and revision untouched.
#[derive(Debug, thiserror::Error)]
pub enum NativeEditError {
    /// Native identities, roles or ownership were invalid.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// An operation's payload or address was invalid.
    #[error("{0}")]
    Invalid(String),
    /// The request was prepared against another document revision.
    #[error("native revision {expected} is stale; current revision is {current}")]
    StaleRevision {
        /// Request revision.
        expected: u64,
        /// Current document revision.
        current: u64,
    },
    /// A gesture already owns the document, or another owner attempted to finish it.
    #[error("a native edit transaction owns the document")]
    TransactionActive,
    /// No gesture exists to finish or revert.
    #[error("no native edit transaction is active")]
    NoTransaction,
    /// Revision counters cannot advance without reusing an old token.
    #[error("native edit revision space is exhausted")]
    RevisionExhausted,
}

struct HistoryEntry {
    edit: Edit,
    document: NativeDocument,
}

struct Transaction {
    owner: NativeEditOwner,
    before: HistoryEntry,
}

/// One native document authority with atomic command batches and bounded undo history.
///
/// Construction validates ownership without opening plugins. Only read access to the document
/// is exposed: commands, history traversal and transaction rollback all use the same commit
/// boundary. The conventional Session remains the default until its other command families
/// have native adapters.
pub struct NativeDocumentEditor {
    document: NativeDocument,
    saved: NativeDocument,
    revision: NativeEditRevision,
    limits: ContentLimits,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    transaction: Option<Transaction>,
}

impl NativeDocumentEditor {
    /// Takes ownership of a validated native document and starts with empty history.
    pub fn new(document: NativeDocument, limits: ContentLimits) -> Result<Self, NativeEditError> {
        document.validate_ownership(limits)?;
        Ok(Self {
            saved: document.clone(),
            document,
            revision: NativeEditRevision::default(),
            limits,
            undo: VecDeque::new(),
            redo: Vec::new(),
            transaction: None,
        })
    }

    /// Reads the single authored native document; runtime plans are separate snapshots.
    pub fn document(&self) -> &NativeDocument {
        &self.document
    }

    /// Lands recorded audio takes as one atomic, undoable native operation.
    ///
    /// A batch may target several audio tracks, but each track may occur only once: accepting
    /// repeated targets would make a failed take indistinguishable from an intentional shared
    /// source edit. The candidate is cloned before any validation or allocation, so a late bad
    /// take leaves the document, counters, history and revisions untouched.
    pub fn land_recorded_audio(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        takes: &[NativeAudioEdit],
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        if takes.len() > MAX_NATIVE_EDITS {
            return Err(ContentError::Limit("command_operations").into());
        }
        let mut targets = HashSet::with_capacity(takes.len());
        let mut sources = HashSet::with_capacity(takes.len());
        for take in takes {
            if !targets.insert(take.track) {
                return Err(NativeEditError::Invalid(
                    "recording batch contains a duplicate audio target".into(),
                ));
            }
            if let Some(source) = self
                .document
                .content
                .tracks
                .iter()
                .find(|binding| binding.track == take.track)
                .and_then(|binding| binding.source)
                && !sources.insert(source)
            {
                return Err(NativeEditError::Invalid(
                    "recording targets share one authored source".into(),
                ));
            }
        }
        let operations = takes
            .iter()
            .cloned()
            .map(NativeEdit::Audio)
            .collect::<Vec<_>>();
        self.apply(expected_revision, owner, Edit::RecordTake, &operations)
    }

    /// Current revision tokens, unchanged by rejected requests and no-op batches.
    pub fn revision(&self) -> NativeEditRevision {
        self.revision
    }

    /// Current gesture owner, used by save/recovery boundaries to refuse partial gestures.
    pub fn transaction_owner(&self) -> Option<NativeEditOwner> {
        self.transaction
            .as_ref()
            .map(|transaction| transaction.owner)
    }

    /// Whether authored state differs from the last acknowledged save.
    pub fn is_dirty(&self) -> bool {
        !authored_equal(&self.document, &self.saved)
    }

    /// Acknowledges successful persistence of exactly the current document revision.
    ///
    /// Call only after atomic publication succeeds. A stale worker completion cannot mark
    /// newer edits saved, and an open gesture cannot be marked saved halfway through.
    pub fn acknowledge_save(&mut self, expected_revision: u64) -> Result<(), NativeEditError> {
        self.check_revision(expected_revision)?;
        self.require_idle()?;
        self.saved = self.document.clone();
        Ok(())
    }

    /// Validates and commits a bounded batch as one undo step, or within its owner's gesture.
    pub fn apply(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        edit: Edit,
        operations: &[NativeEdit],
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        if operations.len() > MAX_NATIVE_EDITS {
            return Err(ContentError::Limit("command_operations").into());
        }
        let mut budget = PayloadBudget {
            remaining: MAX_NATIVE_EDIT_BYTES,
            exceeded: false,
        };
        if let Err(error) = serde_json::to_writer(&mut budget, operations) {
            return Err(if budget.exceeded {
                ContentError::Limit("command_bytes").into()
            } else {
                NativeEditError::Invalid(error.to_string())
            });
        }
        let mut candidate = self.document.clone();
        let mut created = Vec::new();
        for operation in operations {
            created.extend(commands::apply(&mut candidate, operation, self.limits)?);
        }
        candidate.validate_ownership(self.limits)?;
        let mut receipt = self.change_receipt(&candidate)?;
        if !receipt.changed {
            return Ok(receipt);
        }
        // Unrecorded settings are still persisted, but must not be rewound by an authored undo
        // step. Synchronize the pre-edit snapshot with the final candidate before storing it.
        let mut historical = self.document.clone();
        for operation in operations {
            if let NativeEdit::Timeline(timeline) = operation {
                for entry in self.undo.iter_mut().chain(self.redo.iter_mut()) {
                    timeline::synchronize_unrecorded(timeline, &candidate, &mut entry.document);
                }
            }
        }
        if self.transaction.is_some() {
            // Keep the original authored snapshot for the whole gesture. Only its unrecorded
            // fields follow the live candidate as transport/view settings are changed.
            if let Some(transaction) = &mut self.transaction {
                for operation in operations {
                    if let NativeEdit::Timeline(timeline) = operation {
                        timeline::synchronize_unrecorded(
                            timeline,
                            &candidate,
                            &mut transaction.before.document,
                        );
                    }
                }
            }
        } else {
            for operation in operations {
                if let NativeEdit::Timeline(timeline) = operation {
                    timeline::synchronize_unrecorded(timeline, &candidate, &mut historical);
                }
            }
        }
        if self.transaction.is_none() && !authored_equal(&historical, &candidate) {
            self.push_undo(HistoryEntry {
                edit,
                document: historical,
            });
            self.redo.clear();
        }
        self.document = candidate;
        self.revision = receipt.revision;
        receipt.created = created;
        Ok(receipt)
    }

    /// Restores a validated native snapshot as one undoable edit, retaining allocation history.
    ///
    /// Recovery may replace the whole authored document, but cannot reuse identities consumed
    /// since the snapshot or interrupt another owner's gesture. No legacy document is involved.
    pub fn replace_snapshot(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        edit: Edit,
        mut document: NativeDocument,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        self.require_idle()?;
        preserve_allocator(&self.document, &mut document);
        document.validate_ownership(self.limits)?;
        let receipt = self.change_receipt(&document)?;
        if receipt.changed {
            self.push_undo(HistoryEntry {
                edit,
                document: self.document.clone(),
            });
            self.redo.clear();
            self.document = document;
            self.revision = receipt.revision;
        }
        Ok(receipt)
    }

    /// Opens one gesture. Nested gestures are rejected, never implicitly committed.
    pub fn begin_transaction(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        edit: Edit,
    ) -> Result<(), NativeEditError> {
        self.check_revision(expected_revision)?;
        self.require_idle()?;
        self.transaction = Some(Transaction {
            owner,
            before: HistoryEntry {
                edit,
                document: self.document.clone(),
            },
        });
        Ok(())
    }

    /// Finishes the owner's gesture, recording one step only if it changed authored state.
    pub fn end_transaction(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
    ) -> Result<bool, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.require_transaction(owner)?;
        let transaction = self
            .transaction
            .take()
            .ok_or(NativeEditError::NoTransaction)?;
        let changed = !authored_equal(&transaction.before.document, &self.document);
        if changed {
            self.push_undo(transaction.before);
            self.redo.clear();
        }
        Ok(changed)
    }

    /// Reverts the owner's gesture without creating an undo step or discarding a redo branch.
    pub fn revert_transaction(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.require_transaction(owner)?;
        let mut before = self
            .transaction
            .as_ref()
            .ok_or(NativeEditError::NoTransaction)?
            .before
            .document
            .clone();
        let receipt = self.change_receipt(&before)?;
        let _ = self
            .transaction
            .take()
            .ok_or(NativeEditError::NoTransaction)?;
        preserve_allocator(&self.document, &mut before);
        self.document = before;
        self.revision = receipt.revision;
        Ok(receipt)
    }

    /// Whether undo is available outside an active gesture.
    pub fn can_undo(&self) -> bool {
        self.transaction.is_none() && !self.undo.is_empty()
    }

    /// Whether redo is available outside an active gesture.
    pub fn can_redo(&self) -> bool {
        self.transaction.is_none() && !self.redo.is_empty()
    }

    /// Restores a native snapshot without reassigning any surviving identity.
    pub fn undo(
        &mut self,
        expected_revision: u64,
    ) -> Result<Option<NativeEditReceipt>, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.require_idle()?;
        let Some(before) = self.undo.back() else {
            return Ok(None);
        };
        let target = before.document.clone();
        let receipt = self.change_receipt(&target)?;
        let Some(mut before) = self.undo.pop_back() else {
            return Ok(None);
        };
        before.document = target;
        preserve_allocator(&self.document, &mut before.document);
        let current = std::mem::replace(&mut self.document, before.document);
        self.redo.push(HistoryEntry {
            edit: before.edit,
            document: current,
        });
        self.revision = receipt.revision;
        Ok(Some(receipt))
    }

    /// Reapplies a native snapshot; a stale request never consumes the redo entry.
    pub fn redo(
        &mut self,
        expected_revision: u64,
    ) -> Result<Option<NativeEditReceipt>, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.require_idle()?;
        let Some(after) = self.redo.last() else {
            return Ok(None);
        };
        let target = after.document.clone();
        let receipt = self.change_receipt(&target)?;
        let Some(mut after) = self.redo.pop() else {
            return Ok(None);
        };
        after.document = target;
        preserve_allocator(&self.document, &mut after.document);
        let current = std::mem::replace(&mut self.document, after.document);
        self.push_undo(HistoryEntry {
            edit: after.edit,
            document: current,
        });
        self.revision = receipt.revision;
        Ok(Some(receipt))
    }

    fn push_undo(&mut self, entry: HistoryEntry) {
        if self.undo.len() == HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(entry);
    }

    fn check_revision(&self, expected: u64) -> Result<(), NativeEditError> {
        if expected != self.revision.document {
            return Err(NativeEditError::StaleRevision {
                expected,
                current: self.revision.document,
            });
        }
        Ok(())
    }

    fn require_idle(&self) -> Result<(), NativeEditError> {
        if self.transaction.is_some() {
            return Err(NativeEditError::TransactionActive);
        }
        Ok(())
    }

    fn check_owner(&self, owner: NativeEditOwner) -> Result<(), NativeEditError> {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.owner != owner)
        {
            return Err(NativeEditError::TransactionActive);
        }
        Ok(())
    }

    fn require_transaction(&self, owner: NativeEditOwner) -> Result<(), NativeEditError> {
        if self.transaction.is_none() {
            return Err(NativeEditError::NoTransaction);
        }
        self.check_owner(owner)
    }

    fn change_receipt(
        &self,
        candidate: &NativeDocument,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        let changed = !authored_equal(&self.document, candidate);
        let needs_prepare = changed && !musical_equal(&self.document, candidate);
        let mut revision = self.revision;
        if changed {
            revision.document = revision
                .document
                .checked_add(1)
                .ok_or(NativeEditError::RevisionExhausted)?;
        }
        if needs_prepare {
            revision.musical = revision
                .musical
                .checked_add(1)
                .ok_or(NativeEditError::RevisionExhausted)?;
        }
        Ok(NativeEditReceipt {
            revision,
            changed,
            needs_prepare,
            created: Vec::new(),
        })
    }
}

struct PayloadBudget {
    remaining: usize,
    exceeded: bool,
}

impl std::io::Write for PayloadBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::other(
                "native edit payload exceeds its byte budget",
            ));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn preserve_allocator(current: &NativeDocument, restored: &mut NativeDocument) {
    restored.content.next_id = restored.content.next_id.max(current.content.next_id);
    restored.context.source_next_id = restored
        .context
        .source_next_id
        .max(current.context.source_next_id);
}

/// Allocates only on the private command candidate; failure never consumes a committed ID.
fn allocate(document: &mut NativeDocument) -> Result<u64, NativeEditError> {
    let id = document.content.next_id;
    if id == 0 {
        return Err(ContentError::IdExhausted.into());
    }
    document.content.next_id = id.checked_add(1).ok_or(ContentError::IdExhausted)?;
    Ok(id)
}

fn authored_equal(left: &NativeDocument, right: &NativeDocument) -> bool {
    let mut normalized = right.clone();
    normalized.content.next_id = left.content.next_id;
    normalized.context.source_next_id = left.context.source_next_id;
    normalized
        .context
        .saved_by
        .clone_from(&left.context.saved_by);
    normalized == *left
}

fn musical_equal(left: &NativeDocument, right: &NativeDocument) -> bool {
    let mut normalized = right.clone();
    normalized.context.name.clone_from(&left.context.name);
    normalized.context.grid = left.context.grid;
    normalized.context.loop_region = left.context.loop_region;
    normalized.context.loop_enabled = left.context.loop_enabled;
    normalized.context.punch_region = left.context.punch_region;
    normalized.context.punch_enabled = left.context.punch_enabled;
    normalized.context.metronome = left.context.metronome;
    normalized.context.count_in_bars = left.context.count_in_bars;
    normalized
        .context
        .mixer_graph_view
        .clone_from(&left.context.mixer_graph_view);
    for (before, after) in left
        .track_presentation
        .iter()
        .zip(&mut normalized.track_presentation)
    {
        after.name.clone_from(&before.name);
        after.color = before.color;
        after.height = before.height;
    }
    authored_equal(left, &normalized)
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod timeline_tests;
