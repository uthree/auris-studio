//! A hosted plugin as a track's sound source.

use auris_core::buffer::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{
    Instrument, NoteEvent, Parameterized, PluginDescriptor, PrepareContext, ProcessContext,
};
use clack_host::prelude::StoppedPluginAudioProcessor;

use crate::bridge::Bridge;
use crate::host::AurisHost;

/// A hosted CLAP instrument, as the render graph sees it.
///
/// The same two-object split as [`ClapEffect`](crate::ClapEffect), and the same reason for it: this
/// half is `Send` and lives in the graph, and [`ClapPlugin`](crate::ClapPlugin) stays on the main
/// thread. The only difference is what arrives every block — notes, translated to whatever the
/// plugin's note port speaks. See [`notes`](crate::notes) for what that costs.
///
/// # Voices
///
/// [`active_voices`](Instrument::active_voices) is the trait's default of zero. CLAP does have an
/// extension that would answer it, but a number that is right for the built-in synths and always
/// zero for a hosted plugin is worse than one that is honestly unknown for both.
#[derive(Debug)]
pub struct ClapInstrument(Bridge);

impl ClapInstrument {
    /// Wraps a freshly activated audio processor. Called by
    /// [`ClapPlugin::activate_instrument`](crate::ClapPlugin::activate_instrument).
    pub(crate) fn new(bridge: Bridge) -> Self {
        Self(bridge)
    }

    /// Gives the processor back so the plugin can be deactivated.
    pub(crate) fn into_stopped(self) -> StoppedPluginAudioProcessor<AurisHost> {
        self.0.into_stopped()
    }
}

impl Parameterized for ClapInstrument {
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

impl Instrument for ClapInstrument {
    fn descriptor(&self) -> PluginDescriptor {
        self.0.descriptor()
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        // Nothing, for the reason `ClapEffect::prepare` gives: activation is where a CLAP plugin
        // is told its rate and block size, and it cannot be told again.
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, _ctx: &ProcessContext) {
        // Overwriting, because that is the trait's contract: whatever was in the buffer is not
        // this instrument's, and a plugin that produces nothing owes the track silence.
        self.0.render(out, events, true);
    }
}
