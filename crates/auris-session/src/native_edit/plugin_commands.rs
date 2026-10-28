//! Conventional plugin addresses resolved once into native document operations.

use auris_core::param::ParamTarget;
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::{
    ContentError, ContentRef, GroupId, PluginInstance, ProcessorKind,
};
use auris_core::{EffectSlotId, PluginState, TrackId};

use super::{
    NativeDocumentEditor, NativeEdit, NativeEditError, NativeEditOwner, NativeEditReceipt,
    NativeEffectEdit, NativePluginEdit,
};
use crate::{Edit, PluginWindow};

/// Plugin commands using the existing track, slot and parameter addresses.
///
/// Resolution uses explicit native ownership, including the designated primary instrument.
/// These commands perform document edits only: the caller resolves plugin capabilities and
/// prepares a new runtime separately. A stored plugin may remain editable while unavailable.
#[derive(Clone, Debug, PartialEq)]
pub enum NativePluginCommand {
    /// Replaces the designated instrument, including a singer's guide instrument.
    SetInstrument {
        /// Existing timeline track.
        track: TrackId,
        /// Complete replacement plugin payload.
        plugin: PluginInstance,
    },
    /// Replaces one effect payload, retaining its slot and bypass state.
    ReplaceEffect {
        /// Owning track, or the master bus.
        track: Option<TrackId>,
        /// Existing effect slot.
        slot: EffectSlotId,
        /// Complete replacement plugin payload.
        plugin: PluginInstance,
    },
    /// Inserts an effect at an exact position in a conventional insert chain.
    InsertEffect {
        /// Owning track, or the master bus.
        track: Option<TrackId>,
        /// Zero-based position, including the position after the last insert.
        index: usize,
        /// Initial plugin payload.
        plugin: PluginInstance,
    },
    /// Removes an effect and reconnects its conventional insert chain.
    RemoveEffect {
        /// Owning track, or the master bus.
        track: Option<TrackId>,
        /// Existing effect slot.
        slot: EffectSlotId,
    },
    /// Moves an effect by an offset, clamped to its current insert chain.
    MoveEffect {
        /// Owning track, or the master bus.
        track: Option<TrackId>,
        /// Existing effect slot.
        slot: EffectSlotId,
        /// Signed number of positions to move.
        offset: isize,
    },
    /// Changes the bypass state of one effect.
    SetEffectEnabled {
        /// Owning track, or the master bus.
        track: Option<TrackId>,
        /// Existing effect slot.
        slot: EffectSlotId,
        /// Whether the effect processes audio.
        enabled: bool,
    },
    /// Connects a track strip to an effect's sidechain input, or disconnects it.
    SetEffectSidechain {
        /// Owning track, or the master bus.
        track: Option<TrackId>,
        /// Existing effect slot.
        slot: EffectSlotId,
        /// Source track, or no connection.
        source: Option<TrackId>,
    },
    /// Writes a plugin parameter using a stable key resolved by the owning host.
    ///
    /// Numeric IDs on `target` remain compatibility metadata and never select a saved key.
    /// The caller validates range and writability against the current host descriptor.
    SetParameter {
        /// Existing instrument or effect parameter address.
        target: ParamTarget,
        /// Authoritative persistent key, independent of numeric parameter ordering.
        key: String,
        /// Finite native value.
        value: f32,
    },
    /// Lands a complete parameter map and opaque state from the current plugin instance.
    ReplaceState {
        /// Existing instrument or effect window address.
        window: PluginWindow,
        /// Captured state; the caller must also check the runtime generation before landing.
        state: PluginState,
    },
}

impl NativeDocumentEditor {
    /// Resolves a conventional plugin command and commits it through native history.
    ///
    /// Stale revisions and foreign transaction owners are rejected before address resolution.
    /// Every successful changed command is one undo step outside an owned transaction. No
    /// `Project` projection, plugin loading, callback publication or numeric-key fallback occurs.
    pub fn apply_plugin_command(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        command: &NativePluginCommand,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        let bindings = self
            .document
            .content
            .command_bindings(&self.document.external_ids(), self.limits)?;
        let (edit, operation) = match command {
            NativePluginCommand::SetInstrument { track, plugin } => (
                Edit::ChangeInstrument,
                NativeEdit::Plugin(NativePluginEdit::ReplaceInstrument {
                    node: bindings.instrument(*track)?,
                    plugin: plugin.clone(),
                }),
            ),
            NativePluginCommand::ReplaceEffect {
                track,
                slot,
                plugin,
            } => (
                Edit::ExternalChanges,
                NativeEdit::Plugin(NativePluginEdit::ReplaceEffect {
                    node: bindings.effect(*track, *slot)?,
                    plugin: plugin.clone(),
                }),
            ),
            NativePluginCommand::InsertEffect {
                track,
                index,
                plugin,
            } => (
                Edit::AddEffect,
                NativeEdit::Effect(NativeEffectEdit::Insert {
                    group: self.plugin_group(*track)?,
                    index: *index,
                    plugin: plugin.clone(),
                }),
            ),
            NativePluginCommand::RemoveEffect { track, slot } => (
                Edit::RemoveEffect,
                NativeEdit::Effect(NativeEffectEdit::Remove {
                    node: bindings.effect(*track, *slot)?,
                }),
            ),
            NativePluginCommand::MoveEffect {
                track,
                slot,
                offset,
            } => {
                let node = bindings.effect(*track, *slot)?;
                let chain =
                    super::effects::effect_chain(&self.document, self.plugin_group(*track)?)?;
                let index = chain
                    .iter()
                    .position(|candidate| *candidate == node)
                    .ok_or_else(|| {
                        NativeEditError::Invalid("effect is outside the insert chain".into())
                    })?;
                let index = index
                    .saturating_add_signed(*offset)
                    .min(chain.len().saturating_sub(1));
                (
                    Edit::ReorderEffects,
                    NativeEdit::Effect(NativeEffectEdit::Move { node, index }),
                )
            }
            NativePluginCommand::SetEffectEnabled {
                track,
                slot,
                enabled,
            } => (
                Edit::BypassEffect,
                NativeEdit::SetEffectEnabled {
                    node: bindings.effect(*track, *slot)?,
                    enabled: *enabled,
                },
            ),
            NativePluginCommand::SetEffectSidechain {
                track,
                slot,
                source,
            } => (
                Edit::SetEffectSidechain,
                NativeEdit::Effect(NativeEffectEdit::SetSidechain {
                    node: bindings.effect(*track, *slot)?,
                    source: *source,
                }),
            ),
            NativePluginCommand::SetParameter { target, key, value } => {
                if !matches!(
                    target,
                    ParamTarget::Instrument { .. } | ParamTarget::Effect { .. }
                ) {
                    return Err(NativeEditError::Invalid(
                        "target is not a plugin parameter".into(),
                    ));
                }
                let address = bindings.parameter(*target, Some(key))?;
                (
                    Edit::AdjustParameter(*target),
                    NativeEdit::Plugin(NativePluginEdit::SetParameter {
                        node: address.node,
                        key: address.key,
                        value: *value,
                    }),
                )
            }
            NativePluginCommand::ReplaceState { window, state } => {
                let node = match window {
                    PluginWindow::Instrument(track) => bindings.instrument(*track)?,
                    PluginWindow::Effect(slot) => self.plugin_effect(*slot)?,
                };
                (
                    Edit::ExternalChanges,
                    NativeEdit::Plugin(NativePluginEdit::ReplaceState {
                        node,
                        state: state.clone(),
                    }),
                )
            }
        };
        self.apply(expected_revision, owner, edit, &[operation])
    }

    fn plugin_group(&self, track: Option<TrackId>) -> Result<GroupId, NativeEditError> {
        match track {
            None => Ok(self.document.content.master),
            Some(track) => self
                .document
                .content
                .tracks
                .iter()
                .find(|binding| binding.track == track)
                .map(|binding| binding.group)
                .ok_or_else(|| ContentError::Missing(ContentRef::Track(track)).into()),
        }
    }

    fn plugin_effect(&self, slot: EffectSlotId) -> Result<NodeId, NativeEditError> {
        self.document
            .content
            .processors
            .iter()
            .find_map(|processor| {
                matches!(processor.kind, ProcessorKind::Effect { slot: id, .. } if id == slot)
                    .then_some(processor.id)
            })
            .ok_or_else(|| ContentError::Missing(ContentRef::Effect(slot)).into())
    }
}
