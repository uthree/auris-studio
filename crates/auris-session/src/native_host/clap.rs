//! Main-thread CLAP ownership for native pipeline candidates.
//!
//! A value of [`ClapHost`] is deliberately independent from the conventional hosted-plugin
//! slots. The native owner keeps it alive until the graph generation containing its rendering
//! half has been adopted and the old graph has returned, so a rejected or superseded candidate
//! cannot move the published editor or parameter view.

use std::path::Path;

use auris_clap::{ClapLibrary, ClapPlugin, ID_PREFIX, RawWindowHandle};
use auris_core::param::ParamDescriptor;
use auris_core::plugin::{
    Effect, Instrument, Parameterized, PluginKind, PluginState, PrepareContext,
};

use super::{NativeHostError, RenderHalf};

/// Main-thread owner of one isolated CLAP instance used by a native graph candidate.
pub(super) struct ClapHost {
    plugin: ClapPlugin,
    /// The library must outlive every plugin instance created from its entry, so it is declared
    /// after the plugin and therefore dropped after it.
    _library: ClapLibrary,
    sidechain: bool,
}

impl ClapHost {
    /// Loads and activates one fresh instance without changing any published host state.
    pub(super) fn load(
        file: &Path,
        plugin_id: &str,
        state: &PluginState,
        prepare: &PrepareContext,
        instrument: bool,
    ) -> Result<(Self, RenderHalf), NativeHostError> {
        // SAFETY: the caller has already applied the session's plugin trust policy. Loading a
        // CLAP library executes third-party code by definition, and this is the one native-host
        // boundary where that code is admitted.
        let library = unsafe { ClapLibrary::load(file) }?;
        Self::load_from_library(library, plugin_id, state, prepare, instrument)
    }

    /// Loads from an already-open library. The native manager uses this after its trust check;
    /// tests and embedders may use it when the library was supplied by an in-memory fixture.
    pub(super) fn load_from_library(
        library: ClapLibrary,
        plugin_id: &str,
        state: &PluginState,
        prepare: &PrepareContext,
        instrument: bool,
    ) -> Result<(Self, RenderHalf), NativeHostError> {
        let clap_id = plugin_id.strip_prefix(ID_PREFIX).unwrap_or(plugin_id);
        let mut plugin = library.instantiate(clap_id)?;
        let expected = if instrument {
            PluginKind::Instrument
        } else {
            PluginKind::Effect
        };
        if plugin.info().kind != expected {
            return Err(NativeHostError::InvalidPlugin(format!(
                "CLAP plugin `{clap_id}` declares {:?}, expected {expected:?}",
                plugin.info().kind
            )));
        }

        if let Some(bytes) = state.hosted_bytes() {
            plugin.load_state(&bytes)?;
            // Presets can change the parameter list. Resolve the restored instance before
            // activation freezes its descriptor/ID table for the rendering half.
            plugin.refresh_parameters();
        }

        if instrument {
            let mut rendering = plugin.activate_instrument(prepare)?;
            rendering.load_state(state);
            Ok((
                Self {
                    plugin,
                    _library: library,
                    sidechain: false,
                },
                RenderHalf::Instrument(Box::new(rendering) as Box<dyn Instrument>),
            ))
        } else {
            let mut rendering = plugin.activate(prepare)?;
            let sidechain = rendering.wants_sidechain();
            rendering.load_state(state);
            Ok((
                Self {
                    plugin,
                    _library: library,
                    sidechain,
                },
                RenderHalf::Effect(Box::new(rendering) as Box<dyn Effect>),
            ))
        }
    }

    /// Returns the descriptors published by the currently hosted instance.
    pub(super) fn parameters(&self) -> &[ParamDescriptor] {
        self.plugin.parameters()
    }

    /// Whether the activated effect accepts a sidechain input.
    pub(super) fn supports_sidechain(&self) -> bool {
        self.sidechain
    }

    /// Services CLAP timers and host requests without changing the project document.
    ///
    /// The return value reports a plugin-originated state change that the native owner may turn
    /// into a session dirty/rebuild notification. Parameter rescans refresh only the host cache.
    pub(super) fn service(&mut self) -> bool {
        self.plugin.tick_timers();
        let requests = self.plugin.take_requests();
        if requests.gui_closed {
            self.plugin.close_gui();
        }
        if requests.rescan_values || requests.dirty {
            self.plugin.refresh_value_cache();
        }
        requests.dirty || requests.rescan_values || requests.restart
    }

    /// Captures opaque plugin state and all currently readable parameter values.
    pub(super) fn snapshot(&mut self) -> Result<PluginState, NativeHostError> {
        let mut state = PluginState::empty();
        let bytes = self.plugin.save_state()?;
        state.set_hosted_bytes(&bytes);
        for descriptor in self.plugin.parameters().to_vec() {
            if let Some(value) = self.plugin.value(descriptor.id) {
                state.params.insert(descriptor.key.to_string(), value);
            }
        }
        Ok(state)
    }

    /// Whether this instance can provide a plugin editor on the current platform.
    pub(super) fn has_window(&mut self) -> bool {
        self.plugin.has_gui()
    }

    /// Whether the editor belonging to this instance is currently open.
    pub(super) fn window_is_open(&self) -> bool {
        self.plugin.gui_is_open()
    }

    /// Opens or closes this instance's editor.
    pub(super) fn set_window_open(
        &mut self,
        open: bool,
        parent: Option<RawWindowHandle>,
    ) -> Result<(), NativeHostError> {
        if open {
            if self.plugin.gui_is_open() {
                self.plugin.raise_gui()?;
                return Ok(());
            }
            // SAFETY: the native owner receives the application's live parent window and keeps it
            // alive for the lifetime required by the CLAP GUI contract.
            unsafe { self.plugin.open_gui(parent) }?;
        } else {
            self.plugin.close_gui();
        }
        Ok(())
    }

    /// Releases the rendering half when the engine has returned its graph.
    pub(super) fn release(&mut self) -> bool {
        self.plugin.release()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_clap::testkit::{FIXTURE_ID, fixture_library};

    fn prepare() -> PrepareContext {
        PrepareContext::new(48_000.0, 64, 2)
    }

    fn fixture_candidate(state: &PluginState) -> (ClapHost, RenderHalf) {
        ClapHost::load_from_library(fixture_library(), FIXTURE_ID, state, &prepare(), false)
            .expect("fixture candidate")
    }

    #[test]
    fn candidate_editor_state_is_isolated_from_the_published_host() {
        let state = PluginState::empty();
        let (mut published, _published_half) = fixture_candidate(&state);
        published
            .set_window_open(true, None)
            .expect("published editor");
        assert!(published.window_is_open());

        let (candidate, _candidate_half) = fixture_candidate(&state);
        assert!(!candidate.window_is_open());
        assert!(published.window_is_open());
    }

    #[test]
    fn snapshot_contains_opaque_state_and_readable_parameter_values() {
        let mut state = PluginState::empty();
        state.set_hosted_bytes(&0.25f32.to_le_bytes());
        state.params.insert("clap.4242".into(), 0.5);
        let (mut host, half) = fixture_candidate(&state);
        let RenderHalf::Effect(mut effect) = half else {
            panic!("fixture effect")
        };
        // Parameter overrides reach the actual plugin through its first audio block.
        let mut output =
            auris_core::AudioBuffer::from_planar(vec![vec![0.25; 64]; 2], 48_000.0).expect("input");
        effect.process(
            &mut output,
            &auris_core::ProcessContext::realtime(48_000.0, 64, 0, 120.0, true),
        );
        assert!((output.channel(0)[0] - 0.125).abs() < 1.0e-6);
        let snapshot = host.snapshot().expect("snapshot");
        let expected = 0.5f32.to_le_bytes();
        assert_eq!(snapshot.hosted_bytes(), Some(expected.to_vec()));
        assert_eq!(snapshot.params.get("clap.4242"), Some(&0.5));
    }

    #[test]
    fn release_waits_for_the_candidate_render_half_to_return() {
        let state = PluginState::empty();
        let (mut host, half) = fixture_candidate(&state);
        assert!(!host.release());
        drop(half);
        assert!(host.release());
    }
}
