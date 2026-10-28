//! Identity-preserving edits to native instrument and effect payloads.

use auris_core::PluginState;
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::{ContentError, ContentLimits, PluginInstance, ProcessorKind};
use auris_core::pipeline::document::NativeDocument;

use super::{NativeCreated, NativeEditError};

/// A bounded edit to one native plugin instance.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativePluginEdit {
    /// Replaces an instrument payload while retaining its node, group and graph edges.
    ReplaceInstrument {
        /// Instrument processor identity.
        node: NodeId,
        /// New plugin identity, parameters and hosted state.
        plugin: PluginInstance,
    },
    /// Replaces an effect payload while retaining its node, slot, group and graph edges.
    ReplaceEffect {
        /// Effect processor identity.
        node: NodeId,
        /// New plugin identity, parameters and hosted state.
        plugin: PluginInstance,
    },
    /// Sets one stable-keyed plugin parameter without loading the plugin.
    SetParameter {
        /// Instrument or effect processor identity.
        node: NodeId,
        /// Stable parameter key from the plugin descriptor.
        key: String,
        /// Parameter value.
        value: f32,
    },
    /// Replaces the complete serialised plugin state.
    ReplaceState {
        /// Instrument or effect processor identity.
        node: NodeId,
        /// Parameter map and optional opaque hosted state.
        state: PluginState,
    },
}

/// Applies one plugin payload edit atomically to a candidate document.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativePluginEdit,
    _limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match edit {
        NativePluginEdit::ReplaceInstrument { node, plugin } => {
            replace_plugin(document, *node, plugin, true)
        }
        NativePluginEdit::ReplaceEffect { node, plugin } => {
            replace_plugin(document, *node, plugin, false)
        }
        NativePluginEdit::SetParameter { node, key, value } => {
            if key.trim().is_empty() {
                return Err(NativeEditError::Invalid(
                    "plugin parameter key must not be empty".into(),
                ));
            }
            if !value.is_finite() {
                return Err(NativeEditError::Invalid(
                    "plugin parameter value must be finite".into(),
                ));
            }
            let plugin = plugin_mut(document, *node)?;
            if plugin.state.params.get(key) == Some(value) {
                return Ok(Vec::new());
            }
            plugin.state.params.insert(key.clone(), *value);
            Ok(Vec::new())
        }
        NativePluginEdit::ReplaceState { node, state } => {
            validate_state(state)?;
            let plugin = plugin_mut(document, *node)?;
            if plugin.state == *state {
                return Ok(Vec::new());
            }
            plugin.state = state.clone();
            Ok(Vec::new())
        }
    }
}

/// Validates a plugin payload without requiring its runtime implementation.
pub(super) fn validate_plugin(plugin: &PluginInstance) -> Result<(), NativeEditError> {
    if plugin.plugin_id.trim().is_empty() {
        return Err(NativeEditError::Invalid(
            "plugin id must not be empty".into(),
        ));
    }
    validate_state(&plugin.state)
}

fn plugin_mut(
    document: &mut NativeDocument,
    node: NodeId,
) -> Result<&mut PluginInstance, NativeEditError> {
    let processor = document
        .content
        .processors
        .iter_mut()
        .find(|processor| processor.id == node)
        .ok_or(NativeEditError::Content(ContentError::Missing(
            auris_core::pipeline::content::ContentRef::Node(node),
        )))?;
    match &mut processor.kind {
        ProcessorKind::Instrument(plugin) | ProcessorKind::Effect { plugin, .. } => Ok(plugin),
        _ => Err(NativeEditError::Invalid(
            "target processor is not an instrument or effect".into(),
        )),
    }
}

fn replace_plugin(
    document: &mut NativeDocument,
    node: NodeId,
    replacement: &PluginInstance,
    instrument: bool,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    validate_plugin(replacement)?;
    let processor = document
        .content
        .processors
        .iter_mut()
        .find(|processor| processor.id == node)
        .ok_or(NativeEditError::Content(ContentError::Missing(
            auris_core::pipeline::content::ContentRef::Node(node),
        )))?;
    let same = match &mut processor.kind {
        ProcessorKind::Instrument(plugin) if instrument => {
            if *plugin == *replacement {
                true
            } else {
                *plugin = replacement.clone();
                false
            }
        }
        ProcessorKind::Effect { plugin, .. } if !instrument => {
            if *plugin == *replacement {
                true
            } else {
                *plugin = replacement.clone();
                false
            }
        }
        ProcessorKind::Instrument(_) | ProcessorKind::Effect { .. } => {
            return Err(NativeEditError::Invalid(
                "plugin kind does not match target".into(),
            ));
        }
        _ => {
            return Err(NativeEditError::Invalid(
                "target processor is not an instrument or effect".into(),
            ));
        }
    };
    if !same {
        document
            .content
            .automation_bindings
            .retain(|binding| binding.target.node != node);
    }
    Ok(Vec::new())
}

fn validate_state(state: &PluginState) -> Result<(), NativeEditError> {
    if state.params.values().any(|value| !value.is_finite()) {
        return Err(NativeEditError::Invalid(
            "plugin state contains a non-finite parameter".into(),
        ));
    }
    let encoded = serde_json::to_vec(state).map_err(|error| {
        NativeEditError::Invalid(format!("plugin state is not serialisable: {error}"))
    })?;
    let decoded: PluginState = serde_json::from_slice(&encoded).map_err(|error| {
        NativeEditError::Invalid(format!("plugin state is not round-trippable: {error}"))
    })?;
    if decoded != *state {
        return Err(NativeEditError::Invalid(
            "plugin state cannot round-trip without losing values".into(),
        ));
    }
    Ok(())
}
