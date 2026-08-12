//! The main-thread half of a hosted plugin.

use std::borrow::Cow;
use std::sync::Arc;

use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{PluginDescriptor, PrepareContext};
use clack_extensions::latency::PluginLatency;
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::prelude::*;
use clack_host::utils::ClapId;

use crate::effect::ClapEffect;
use crate::error::ClapError;
use crate::host::{AurisHost, AurisMainThread, AurisShared, HostFlags, host_info};
use crate::library::ClapPluginInfo;

/// The plugin's parameters, in the order the plugin lists them.
///
/// Both halves of a hosted plugin need this and neither may change it, so it is shared by
/// [`Arc`] and never mutated. If the plugin ever asks for a rescan of anything but values, the
/// session must throw the whole plugin away and build it again — which is what
/// [`PendingRequests::restart`] is for.
pub(crate) struct ParamList {
    /// The plugin's own id for each parameter, indexed by [`ParamId`].
    pub(crate) clap_ids: Vec<ClapId>,
    /// What the rest of the application sees.
    pub(crate) descriptors: Vec<ParamDescriptor>,
}

/// Requests a plugin has made since this was last read.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingRequests {
    /// The plugin must be deactivated and rebuilt — its parameter list or layout changed.
    pub restart: bool,
    /// Parameter values changed inside the plugin; read them again.
    pub rescan_values: bool,
    /// The plugin's state changed, so the project is unsaved.
    pub dirty: bool,
    /// The plugin asked for its main-thread callback.
    pub callback: bool,
}

/// An instantiated CLAP plugin, living on the thread that made it.
///
/// This is the half that can answer questions. It is not [`Send`], deliberately: CLAP requires
/// that everything here happens on one thread, and the type system is the cheapest place to
/// enforce that. The rendering half is [`ClapEffect`], which this hands out on
/// [`activate`](Self::activate).
pub struct ClapPlugin {
    instance: PluginInstance<AurisHost>,
    info: ClapPluginInfo,
    params: Arc<ParamList>,
    active: bool,
}

impl ClapPlugin {
    /// Instantiates a plugin from an already-loaded entry.
    pub(crate) fn new(entry: &PluginEntry, info: ClapPluginInfo) -> Result<Self, ClapError> {
        let id = std::ffi::CString::new(info.clap_id.as_str()).map_err(|_| {
            ClapError::UnknownPlugin(format!("{} (contains a NUL byte)", info.clap_id))
        })?;

        let mut instance = PluginInstance::<AurisHost>::new(
            |_| AurisShared::default(),
            |shared| AurisMainThread::new(shared),
            entry,
            &id,
            &host_info(),
        )
        .map_err(|error| ClapError::Instantiate {
            id: info.clap_id.clone(),
            reason: error.to_string(),
        })?;

        let params = Arc::new(read_params(&mut instance));

        Ok(Self {
            instance,
            info,
            params,
            active: false,
        })
    }

    /// What the file said about this plugin.
    pub fn info(&self) -> &ClapPluginInfo {
        &self.info
    }

    /// Presents the plugin the way every other plugin in the application is presented.
    pub fn descriptor(&self) -> PluginDescriptor {
        self.info.descriptor()
    }

    /// The plugin's parameters.
    pub fn parameters(&self) -> &[ParamDescriptor] {
        &self.params.descriptors
    }

    /// Reads one parameter's current value from the plugin itself.
    ///
    /// The plugin is the authority here, not the host: a plugin's own interface, a preset it
    /// loaded, or its internal MIDI mapping can all move a parameter without the host asking.
    pub fn value(&mut self, id: ParamId) -> Option<f32> {
        let clap_id = *self.params.clap_ids.get(id.index())?;
        let params = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginParams>()?;
        params
            .get_value(&mut self.instance.plugin_handle(), clap_id)
            .map(|value| value as f32)
    }

    /// Requests the plugin has made since this was last called. Reading clears them.
    pub fn take_requests(&self) -> PendingRequests {
        self.instance
            .access_shared_handler(|shared| PendingRequests {
                restart: HostFlags::take(&shared.flags.restart)
                    | HostFlags::take(&shared.flags.rescan_info),
                rescan_values: HostFlags::take(&shared.flags.rescan_values),
                dirty: HostFlags::take(&shared.flags.dirty),
                callback: HostFlags::take(&shared.flags.callback),
            })
    }

    /// Runs the plugin's main-thread callback, which it asked for through
    /// [`PendingRequests::callback`].
    pub fn run_callback(&mut self) {
        self.instance.call_on_main_thread_callback();
    }

    /// Activates the plugin and hands out the half that renders.
    ///
    /// A CLAP plugin allocates its buffers here, from the rate and block size it is given, and
    /// must be deactivated and activated again if either changes. That is why the effect's
    /// [`prepare`](auris_core::plugin::Effect::prepare) does nothing: by the time the graph
    /// exists, preparing has already happened, and the only honest response to a rate change is
    /// to build the plugin again.
    pub fn activate(&mut self, ctx: &PrepareContext) -> Result<ClapEffect, ClapError> {
        let config = PluginAudioConfiguration {
            sample_rate: ctx.sample_rate,
            min_frames_count: 1,
            max_frames_count: ctx.max_block_frames.max(1) as u32,
        };

        let processor =
            self.instance
                .activate(|_, _| (), config)
                .map_err(|error| ClapError::Activate {
                    id: self.info.clap_id.clone(),
                    sample_rate: ctx.sample_rate,
                    max_block_frames: ctx.max_block_frames,
                    reason: error.to_string(),
                })?;
        self.active = true;

        let latency = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginLatency>()
            .map(|latency| latency.get(&mut self.instance.plugin_handle()) as usize)
            .unwrap_or(0);

        Ok(ClapEffect::new(
            processor,
            self.descriptor(),
            Arc::clone(&self.params),
            ctx,
            latency,
        ))
    }

    /// Deactivates the plugin, taking back the rendering half.
    pub fn deactivate(&mut self, effect: ClapEffect) {
        self.instance.deactivate(effect.into_stopped());
        self.active = false;
    }

    /// Deactivates a plugin whose rendering half was dropped somewhere else.
    ///
    /// This is the normal path in Auris Studio: a replaced graph travels back from the audio
    /// thread down the engine's return channel and is dropped there, so by the time the session
    /// gets round to the plugin the effect is already gone. Returns `false` when the effect is
    /// still alive, in which case the caller should try again later rather than force it.
    pub fn release(&mut self) -> bool {
        let released = self.instance.try_deactivate().is_ok();
        self.active &= !released;
        released
    }

    /// `true` while a rendering half exists.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The plugin's own state, as the opaque byte stream CLAP defines it to be.
    ///
    /// There is no way around the opacity: a CLAP plugin's state is whatever it says it is, and
    /// a wavetable or a sample path cannot be squeezed into the `f32` map that
    /// [`auris_core::plugin::PluginState`] carries. The session stores these bytes beside that
    /// map rather than in it.
    pub fn save_state(&mut self) -> Result<Vec<u8>, ClapError> {
        let state = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginState>()
            .ok_or_else(|| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: true,
                reason: "the plugin implements no state extension".into(),
            })?;

        let mut bytes = Vec::new();
        state
            .save(&mut self.instance.plugin_handle(), &mut bytes)
            .map_err(|error| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: true,
                reason: error.to_string(),
            })?;
        Ok(bytes)
    }

    /// Restores state previously taken by [`save_state`](Self::save_state).
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), ClapError> {
        let state = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginState>()
            .ok_or_else(|| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: false,
                reason: "the plugin implements no state extension".into(),
            })?;

        state
            .load(&mut self.instance.plugin_handle(), &mut &bytes[..])
            .map_err(|error| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: false,
                reason: error.to_string(),
            })
    }
}

impl std::fmt::Debug for ClapPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClapPlugin")
            .field("id", &self.info.clap_id)
            .field("parameters", &self.params.descriptors.len())
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// Asks a freshly instantiated plugin what its parameters are.
fn read_params(instance: &mut PluginInstance<AurisHost>) -> ParamList {
    let Some(params) = instance
        .plugin_shared_handle()
        .get_extension::<PluginParams>()
    else {
        return ParamList {
            clap_ids: Vec::new(),
            descriptors: Vec::new(),
        };
    };

    let count = params.count(&mut instance.plugin_handle());
    let mut clap_ids = Vec::with_capacity(count as usize);
    let mut descriptors = Vec::with_capacity(count as usize);
    let mut buffer = ParamInfoBuffer::new();

    for index in 0..count {
        let mut handle = instance.plugin_handle();
        let Some(info) = params.get_info(&mut handle, index, &mut buffer) else {
            continue;
        };
        // The position in *our* slice, not the plugin's index: a parameter the plugin refused
        // to describe is skipped, and everything after it shifts up.
        let id = descriptors.len() as u32;
        descriptors.push(describe(
            id,
            info.id.get(),
            &String::from_utf8_lossy(info.name),
            info.min_value,
            info.max_value,
            info.default_value,
            info.flags.contains(ParamInfoFlags::IS_STEPPED),
        ));
        clap_ids.push(info.id);
    }

    ParamList {
        clap_ids,
        descriptors,
    }
}

/// Turns one CLAP parameter description into the one the rest of the application speaks.
///
/// The two mismatches worth naming:
///
/// * CLAP ids are arbitrary `u32`s chosen by the plugin, while a [`ParamId`] is a position in a
///   slice. So the position is assigned here and the plugin's id is kept alongside for talking
///   back to it.
/// * A saved project keys parameters by string. The plugin's own id is the only thing about a
///   CLAP parameter that is promised never to change — the *name* may change, the *index* may
///   move — so the key is built from it, and a project keeps loading after the plugin is updated
///   and reorders its list.
fn describe(
    id: u32,
    clap_id: u32,
    name: &str,
    min: f64,
    max: f64,
    default: f64,
    stepped: bool,
) -> ParamDescriptor {
    let min = min as f32;
    let max = max as f32;
    let steps = match stepped {
        // A stepped CLAP parameter is a range of integers, ends included.
        true => Some(((max - min).round() as u32).saturating_add(1)),
        false => None,
    };
    let unit = match (stepped, min, max) {
        (true, 0.0, 1.0) => ParamUnit::Toggle,
        (true, _, _) => ParamUnit::Choice,
        _ => ParamUnit::Plain,
    };

    ParamDescriptor {
        id: ParamId(id),
        key: Cow::Owned(format!("clap.{clap_id}")),
        name: Cow::Owned(match name.is_empty() {
            true => format!("Parameter {clap_id}"),
            false => name.to_string(),
        }),
        min,
        max,
        default: default as f32,
        unit,
        curve: auris_core::param::ParamValueCurve::Linear,
        steps,
        choices: Cow::Borrowed(&[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameter_is_keyed_by_the_plugins_own_id_not_its_position() {
        // The plugin may reorder its list in the next version; it may not change an id. Keying
        // by position would silently move a saved value onto a different parameter.
        let first = describe(0, 4242, "Gain", 0.0, 2.0, 1.0, false);
        let moved = describe(3, 4242, "Gain", 0.0, 2.0, 1.0, false);
        assert_eq!(first.key, moved.key);
        assert_eq!(first.key, "clap.4242");
        assert_eq!(first.id, ParamId(0));
        assert_eq!(moved.id, ParamId(3));
    }

    #[test]
    fn a_nameless_parameter_still_gets_a_label() {
        let param = describe(0, 7, "", 0.0, 1.0, 0.5, false);
        assert_eq!(param.name, "Parameter 7");
    }

    #[test]
    fn a_two_position_stepped_parameter_is_a_toggle() {
        let toggle = describe(0, 1, "Bypass", 0.0, 1.0, 0.0, true);
        assert_eq!(toggle.unit, ParamUnit::Toggle);
        assert_eq!(toggle.steps, Some(2));

        let choice = describe(0, 2, "Mode", 0.0, 3.0, 0.0, true);
        assert_eq!(choice.unit, ParamUnit::Choice);
        assert_eq!(choice.steps, Some(4), "four positions, both ends included");

        let continuous = describe(0, 3, "Drive", 0.0, 1.0, 0.5, false);
        assert_eq!(continuous.unit, ParamUnit::Plain);
        assert_eq!(continuous.steps, None);
    }
}
