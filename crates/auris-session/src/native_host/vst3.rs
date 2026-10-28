//! Main-thread VST3 ownership for native pipeline candidates.
//!
//! This host is deliberately separate from the legacy session VST3 slots. A candidate gets a
//! fresh control instance and a renderer made from that instance's state, so preparing or
//! inspecting a candidate cannot rewrite an editor-backed published plugin.

use std::path::Path;

use auris_clap::RawWindowHandle;
use auris_core::param::ParamDescriptor;
use auris_core::plugin::{Effect, Instrument, PluginKind, PluginState, PrepareContext};
use auris_vst3::{ID_PREFIX, Vst3Error, Vst3Plugin};

use super::{NativeHostError, RenderHalf};

/// Main-thread owner of one isolated VST3 instance used by a native graph candidate.
pub(super) struct Vst3Host {
    plugin: Vst3Plugin,
    sidechain: bool,
    render_error: Option<Vst3Error>,
}

impl Vst3Host {
    /// Loads a fresh control instance and creates an independent rendering half.
    pub(super) fn load(
        file: &Path,
        plugin_id: &str,
        state: &PluginState,
        prepare: &PrepareContext,
        instrument: bool,
    ) -> Result<(Self, RenderHalf), NativeHostError> {
        let class_id = plugin_id.strip_prefix(ID_PREFIX).unwrap_or(plugin_id);
        let plugin = Vst3Plugin::load(file, class_id, prepare)?;
        let expected = if instrument {
            PluginKind::Instrument
        } else {
            PluginKind::Effect
        };
        if plugin.info().kind != expected {
            return Err(NativeHostError::InvalidPlugin(format!(
                "VST3 plugin `{class_id}` declares {:?}, expected {expected:?}",
                plugin.info().kind
            )));
        }
        if let Some(bytes) = state.hosted_bytes() {
            plugin.load_state(&bytes)?;
        }
        for descriptor in plugin.parameters().to_vec() {
            if let Some(value) = state.params.get(descriptor.key.as_ref()) {
                plugin.set_param(descriptor.id, *value)?;
            }
        }

        if instrument {
            let rendering = plugin.instrument()?;
            Ok((
                Self {
                    plugin,
                    sidechain: false,
                    render_error: None,
                },
                RenderHalf::Instrument(Box::new(rendering) as Box<dyn Instrument>),
            ))
        } else {
            let rendering = plugin.effect()?;
            let sidechain = rendering.wants_sidechain();
            Ok((
                Self {
                    plugin,
                    sidechain,
                    render_error: None,
                },
                RenderHalf::Effect(Box::new(rendering) as Box<dyn Effect>),
            ))
        }
    }

    /// Returns the descriptors published by this isolated control instance.
    pub(super) fn parameters(&self) -> &[ParamDescriptor] {
        self.plugin.parameters()
    }

    /// Whether the hosted effect accepts an audio sidechain.
    pub(super) fn supports_sidechain(&self) -> bool {
        self.sidechain
    }

    /// Services editor/platform requests and reports source changes without touching a document.
    pub(super) fn service(&mut self) -> bool {
        if let Err(error) = self.plugin.service_gui() {
            self.record_error(error);
        }
        match self.plugin.take_source_changes() {
            Ok(changes) => {
                if let Some(failure) = changes.renderer_failure {
                    self.record_render_failure(failure);
                }
                changes.source_changed
            }
            Err(error) => {
                self.record_error(error);
                false
            }
        }
    }

    /// Captures opaque plugin state and all currently readable parameter values.
    pub(super) fn snapshot(&mut self) -> Result<PluginState, NativeHostError> {
        let mut state = PluginState::empty();
        state.set_hosted_bytes(&self.plugin.save_state()?);
        for descriptor in self.plugin.parameters().to_vec() {
            if let Some(value) = self.plugin.value(descriptor.id) {
                state.params.insert(descriptor.key.to_string(), value);
            }
        }
        Ok(state)
    }

    /// Whether this instance advertises a native editor.
    pub(super) fn has_window(&mut self) -> bool {
        self.plugin.has_gui()
    }

    /// Whether this instance's editor is currently open.
    pub(super) fn window_is_open(&self) -> bool {
        self.plugin.gui_is_open()
    }

    /// Opens or closes the editor. VST3's current host path provides a floating editor only.
    pub(super) fn set_window_open(
        &mut self,
        open: bool,
        parent: Option<RawWindowHandle>,
    ) -> Result<(), NativeHostError> {
        let _ = parent;
        self.plugin.set_gui_open(open)?;
        Ok(())
    }

    /// Services returned renderer teardowns and reports whether no renderer remains active.
    pub(super) fn release(&mut self) -> bool {
        self.plugin.is_idle()
    }

    /// Returns the first renderer failure observed by this isolated instance, if any.
    pub(super) fn take_render_error(&mut self) -> Option<Vst3Error> {
        self.render_error.take()
    }

    fn record_render_failure(&mut self, failure: auris_vst3::Vst3RenderFailure) {
        if self.render_error.is_none() {
            self.render_error = Some(Vst3Error::RendererStopped {
                plugin: self.plugin.info().name.clone(),
                failure,
            });
        }
    }

    fn record_error(&mut self, error: Vst3Error) {
        if self.render_error.is_none() {
            self.render_error = Some(error);
        }
    }
}
