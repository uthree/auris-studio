//! A hosted plugin in a track's effect chain.

use auris_core::buffer::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{Effect, Parameterized, PluginDescriptor, PrepareContext, ProcessContext};
use clack_host::prelude::StoppedPluginAudioProcessor;

use crate::bridge::Bridge;
use crate::host::AurisHost;

/// A hosted CLAP plugin, as the render graph sees it.
///
/// This is an ordinary [`Effect`]: the engine cannot tell it from a built-in one, and does not
/// know this crate exists. Everything that is *not* rendering — what the parameters are, what
/// the state is, when to rebuild — belongs to [`ClapPlugin`](crate::ClapPlugin), which stays on
/// the main thread.
///
/// # What is not realtime-safe here
///
/// The wrapper is: its buffers are allocated in [`ClapPlugin::activate`](crate::ClapPlugin::activate)
/// and its event queue has room for every parameter at once, so nothing on the block path
/// allocates. The plugin behind it is another matter, and no host can make it behave. A CLAP
/// plugin that allocates in `process` will glitch, and the only fix is to stop loading it.
#[derive(Debug)]
pub struct ClapEffect(Bridge);

impl ClapEffect {
    /// Wraps a freshly activated audio processor. Called by
    /// [`ClapPlugin::activate`](crate::ClapPlugin::activate), which is the only place that can
    /// produce the processor in the first place.
    pub(crate) fn new(bridge: Bridge) -> Self {
        Self(bridge)
    }

    /// Gives the processor back so the plugin can be deactivated.
    pub(crate) fn into_stopped(self) -> StoppedPluginAudioProcessor<AurisHost> {
        self.0.into_stopped()
    }
}

impl Parameterized for ClapEffect {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.0.parameters()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.0.param(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        self.0.set_param(id, value);
    }
}

impl Effect for ClapEffect {
    fn descriptor(&self) -> PluginDescriptor {
        self.0.descriptor()
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        // Deliberately nothing. A CLAP plugin sizes its buffers when it is *activated*, from a
        // rate and block size it cannot then be told about again — changing either means
        // deactivating and building it afresh, which only the main-thread half can do. By the
        // time this effect is in a graph, preparing has already happened.
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        // No notes, and the buffer is left alone if the plugin produces nothing: an effect that
        // cannot run should pass the audio through rather than silence the track.
        self.0.render(buffer, &[], false, None);
    }

    fn wants_sidechain(&self) -> bool {
        self.0.has_sidechain()
    }

    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: &AudioBuffer,
        _ctx: &ProcessContext,
    ) {
        self.0.render(buffer, &[], false, Some(sidechain));
    }

    fn latency_frames(&self) -> usize {
        self.0.latency()
    }
}
