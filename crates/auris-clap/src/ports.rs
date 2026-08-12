//! What audio ports a plugin has, and which of them carries the track.
//!
//! A host must hand a plugin exactly the ports it declared — not the ports the host happens to
//! care about. Passing one input port to a plugin that declared two is not a layout mismatch the
//! plugin can notice and work around: it reads `audio_inputs[1]` off the end of the array the host
//! built, which on Windows is an access violation and on a Mac is whatever was next in memory.
//!
//! Surge XT Effects is the plugin that taught this crate the lesson. Twenty-seven of its
//! twenty-eight effects use one stereo input; the vocoder uses two, and switching to it took the
//! whole application down.

use clack_extensions::audio_ports::{AudioPortFlags, AudioPortInfoBuffer, PluginAudioPorts};
use clack_host::prelude::PluginMainThreadHandle;

/// The audio ports a plugin declares, in its own order.
///
/// Channel counts rather than names, because that is all the rendering half needs: a buffer per
/// channel of every port, whether or not Auris has anything to put in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortLayout {
    /// Channels on each input port.
    pub inputs: Vec<usize>,
    /// Channels on each output port.
    pub outputs: Vec<usize>,
    /// Which input port carries the track's audio, if any.
    pub main_input: Option<usize>,
    /// Which output port carries the result, if any.
    pub main_output: Option<usize>,
}

impl PortLayout {
    /// One input and one output port of `channels` channels each.
    ///
    /// What a plugin that does not implement the extension gets. The specification says such a
    /// plugin has no audio ports at all, which for an effect would mean silence — and a plugin
    /// that forgot to declare its ports is likelier than one that genuinely has none, so this
    /// guesses the layout every effect in the world actually has.
    pub fn stereo_pair(channels: usize) -> Self {
        Self {
            inputs: vec![channels],
            outputs: vec![channels],
            main_input: Some(0),
            main_output: Some(0),
        }
    }

    /// Channels across every input port, which is how much buffer the host has to find.
    pub fn input_channels(&self) -> usize {
        self.inputs.iter().sum()
    }

    /// Channels across every output port.
    pub fn output_channels(&self) -> usize {
        self.outputs.iter().sum()
    }
}

/// Which port a host should put the track's audio in.
///
/// The flagged one, or the first, or none at all. CLAP requires the main port to be at index 0
/// when it exists, so those two agree in practice — but a plugin that flags a later port is
/// describing its own layout, and taking its word for it costs nothing.
pub fn main_port(flagged: Option<usize>, count: usize) -> Option<usize> {
    match (flagged, count) {
        (Some(index), count) if index < count => Some(index),
        (_, 0) => None,
        _ => Some(0),
    }
}

/// Reads one direction's ports from a plugin.
///
/// Ports may only change while a plugin is deactivated, so what this returns stays true for as
/// long as the effect built from it is rendering.
pub(crate) fn read_side(
    ports: &PluginAudioPorts,
    handle: &mut PluginMainThreadHandle,
    is_input: bool,
) -> (Vec<usize>, Option<usize>) {
    let mut buffer = AudioPortInfoBuffer::new();
    let count = ports.count(handle, is_input);
    let mut channels = Vec::with_capacity(count as usize);
    let mut flagged = None;

    for index in 0..count {
        // A port the plugin will not describe still exists as far as the array it is going to
        // index is concerned, so it is counted as silent rather than skipped.
        let Some(info) = ports.get(handle, index, is_input, &mut buffer) else {
            channels.push(0);
            continue;
        };
        if flagged.is_none() && info.flags.contains(AudioPortFlags::IS_MAIN) {
            flagged = Some(channels.len());
        }
        channels.push(info.channel_count as usize);
    }

    let main = main_port(flagged, channels.len());
    (channels, main)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flagged_port_is_the_main_one_wherever_it_is() {
        assert_eq!(main_port(Some(1), 3), Some(1));
        // Nothing flagged: the first port, which is where the specification puts it.
        assert_eq!(main_port(None, 3), Some(0));
        // A flag pointing past the end is a plugin contradicting itself; the first port is the
        // safer reading, and indexing where it said would be the crash this module is about.
        assert_eq!(main_port(Some(7), 2), Some(0));
        assert_eq!(main_port(None, 0), None);
        assert_eq!(main_port(Some(0), 0), None);
    }

    #[test]
    fn a_layout_counts_every_channel_it_has_to_find_room_for() {
        let layout = PortLayout {
            inputs: vec![2, 2],
            outputs: vec![2],
            main_input: Some(0),
            main_output: Some(0),
        };
        // The sidechain's two channels are the ones that used to go missing.
        assert_eq!(layout.input_channels(), 4);
        assert_eq!(layout.output_channels(), 2);
        assert_eq!(PortLayout::stereo_pair(2).input_channels(), 2);
    }
}
