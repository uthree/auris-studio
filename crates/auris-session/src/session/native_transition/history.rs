//! Origin-checked entry points shared by native UI and agent command adapters.

use super::{NativeSessionTransition, NativeTransitionError};
use crate::native_edit::{NativeEdit, NativeEditOwner, NativeEditReceipt};
use crate::{Edit, Session};

impl Session {
    /// Applies a native batch only while its ordinary Session origin is unchanged and idle.
    ///
    /// The editor additionally checks the native revision and gesture owner. One successful
    /// batch is one Undo step unless the same owner has opened a transaction. This modifies
    /// only the transition candidate; rendering and callback adoption are separate operations.
    pub fn apply_native_transition(
        &self,
        transition: &mut NativeSessionTransition,
        expected_revision: u64,
        owner: NativeEditOwner,
        edit: Edit,
        operations: &[NativeEdit],
    ) -> Result<NativeEditReceipt, NativeTransitionError> {
        self.require_native_transition_origin(transition)?;
        Ok(transition
            .editor_mut()
            .apply(expected_revision, owner, edit, operations)?)
    }

    /// Opens an owned native gesture after checking both the origin and native revision.
    ///
    /// Nested gestures are rejected. Every subsequent edit and the final commit or rollback
    /// must identify the same owner; another agent request cannot finish a person's gesture.
    pub fn begin_native_transition_transaction(
        &self,
        transition: &mut NativeSessionTransition,
        expected_revision: u64,
        owner: NativeEditOwner,
        edit: Edit,
    ) -> Result<(), NativeTransitionError> {
        self.require_native_transition_origin(transition)?;
        Ok(transition
            .editor_mut()
            .begin_transaction(expected_revision, owner, edit)?)
    }

    /// Commits the owner's native gesture as one Undo step, returning whether it changed data.
    pub fn end_native_transition_transaction(
        &self,
        transition: &mut NativeSessionTransition,
        expected_revision: u64,
        owner: NativeEditOwner,
    ) -> Result<bool, NativeTransitionError> {
        self.require_native_transition_origin(transition)?;
        Ok(transition
            .editor_mut()
            .end_transaction(expected_revision, owner)?)
    }

    /// Rolls back the owner's native gesture without consuming its previous redo branch.
    ///
    /// A changed origin is rejected even for rollback. Such a candidate must be discarded or
    /// explicitly recovered independently; this method never rebases onto a newer Session.
    pub fn revert_native_transition_transaction(
        &self,
        transition: &mut NativeSessionTransition,
        expected_revision: u64,
        owner: NativeEditOwner,
    ) -> Result<NativeEditReceipt, NativeTransitionError> {
        self.require_native_transition_origin(transition)?;
        Ok(transition
            .editor_mut()
            .revert_transaction(expected_revision, owner)?)
    }

    /// Undoes one native step after origin/revision validation; active gestures reject Undo.
    pub fn undo_native_transition(
        &self,
        transition: &mut NativeSessionTransition,
        expected_revision: u64,
    ) -> Result<Option<NativeEditReceipt>, NativeTransitionError> {
        self.require_native_transition_origin(transition)?;
        Ok(transition.editor_mut().undo(expected_revision)?)
    }

    /// Redoes one native step after origin/revision validation, retaining authored identities.
    pub fn redo_native_transition(
        &self,
        transition: &mut NativeSessionTransition,
        expected_revision: u64,
    ) -> Result<Option<NativeEditReceipt>, NativeTransitionError> {
        self.require_native_transition_origin(transition)?;
        Ok(transition.editor_mut().redo(expected_revision)?)
    }
}
