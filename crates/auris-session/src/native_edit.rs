//! Native document commands and snapshot history, independent of runtime preparation.
//!
//! This is the mutation boundary for native command adapters. It owns no legacy `Project` and
//! does not convert a document after edits. Runtime preparation may fail while the authored
//! document remains editable, including when a referenced plugin is unavailable.

mod commands;

pub use commands::NativeEdit;

use std::collections::VecDeque;

use auris_core::pipeline::content::{ContentError, ContentLimits};
use auris_core::pipeline::document::NativeDocument;

use crate::Edit;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEditReceipt {
    /// Revisions after the operation.
    pub revision: NativeEditRevision,
    /// Whether authored content or presentation changed.
    pub changed: bool,
    /// Whether the musical revision advanced; this is not proof of callback adoption.
    pub needs_prepare: bool,
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

    /// Current revision tokens, unchanged by rejected requests and no-op batches.
    pub fn revision(&self) -> NativeEditRevision {
        self.revision
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
        for operation in operations {
            commands::apply(&mut candidate, operation, self.limits)?;
        }
        candidate.validate_ownership(self.limits)?;
        let receipt = self.change_receipt(&candidate)?;
        if !receipt.changed {
            return Ok(receipt);
        }
        if self.transaction.is_none() {
            self.push_undo(HistoryEntry {
                edit,
                document: self.document.clone(),
            });
            self.redo.clear();
        }
        self.document = candidate;
        self.revision = receipt.revision;
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
        let before = &self
            .transaction
            .as_ref()
            .ok_or(NativeEditError::NoTransaction)?
            .before
            .document;
        let receipt = self.change_receipt(before)?;
        let mut before = self
            .transaction
            .take()
            .ok_or(NativeEditError::NoTransaction)?
            .before
            .document;
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
        let receipt = self.change_receipt(&before.document)?;
        let Some(mut before) = self.undo.pop_back() else {
            return Ok(None);
        };
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
        let receipt = self.change_receipt(&after.document)?;
        let Some(mut after) = self.redo.pop() else {
            return Ok(None);
        };
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
