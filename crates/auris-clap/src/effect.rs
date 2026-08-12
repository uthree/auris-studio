//! The audio-thread half of a hosted plugin.

use std::sync::Arc;

use auris_core::buffer::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{Effect, Parameterized, PluginDescriptor, PrepareContext, ProcessContext};
use clack_host::events::event_types::ParamValueEvent;
use clack_host::prelude::*;
use clack_host::utils::Cookie;

use crate::host::AurisHost;
use crate::plugin::ParamList;

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
pub struct ClapEffect {
    processor: PluginAudioProcessor<AurisHost>,
    descriptor: PluginDescriptor,
    params: Arc<ParamList>,
    values: Vec<f32>,
    changed: Vec<bool>,
    outgoing: EventBuffer,
    replies: EventBuffer,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    max_frames: usize,
    latency: usize,
    /// A counter that only ever goes up, which is what CLAP asks of `steady_time`. The project
    /// playhead is not usable for it: a cycle sends the playhead backwards, and a plugin is
    /// entitled to treat that as impossible.
    steady_time: u64,
}

impl ClapEffect {
    /// Wraps a freshly activated audio processor. Called by
    /// [`ClapPlugin::activate`](crate::ClapPlugin::activate), which is the only place that can
    /// produce the processor in the first place.
    pub(crate) fn new(
        processor: StoppedPluginAudioProcessor<AurisHost>,
        descriptor: PluginDescriptor,
        params: Arc<ParamList>,
        ctx: &PrepareContext,
        latency: usize,
    ) -> Self {
        let channels = ctx.channel_count.max(1);
        let frames = ctx.max_block_frames.max(1);
        let count = params.descriptors.len();
        let values = params.descriptors.iter().map(|p| p.default).collect();

        Self {
            processor: processor.into(),
            descriptor,
            params,
            values,
            changed: vec![false; count],
            // Room for every parameter to change in the same block, which is the most that can
            // happen: `set_param` marks, `process` sends, and a mark cannot be set twice.
            outgoing: EventBuffer::with_capacity(count.max(1)),
            replies: EventBuffer::with_capacity(count.max(1)),
            input_ports: AudioPorts::with_capacity(channels, 1),
            output_ports: AudioPorts::with_capacity(channels, 1),
            input: vec![vec![0.0; frames]; channels],
            output: vec![vec![0.0; frames]; channels],
            max_frames: frames,
            latency,
            steady_time: 0,
        }
    }

    /// Gives the processor back so the plugin can be deactivated.
    pub(crate) fn into_stopped(self) -> StoppedPluginAudioProcessor<AurisHost> {
        self.processor.into_stopped()
    }
}

impl Parameterized for ClapEffect {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.params.descriptors
    }

    fn param(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or(0.0)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        let Some(descriptor) = self.params.descriptors.get(id.index()) else {
            return;
        };
        self.values[id.index()] = descriptor.clamp(value);
        // Marking rather than queueing keeps this allocation-free however often it is called:
        // a hundred automation writes in one block still send one event.
        self.changed[id.index()] = true;
    }
}

impl Effect for ClapEffect {
    fn descriptor(&self) -> PluginDescriptor {
        self.descriptor.clone()
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        // Deliberately nothing. A CLAP plugin sizes its buffers when it is *activated*, from a
        // rate and block size it cannot then be told about again — changing either means
        // deactivating and building it afresh, which only the main-thread half can do. By the
        // time this effect is in a graph, preparing has already happened.
    }

    fn reset(&mut self) {
        self.processor.reset();
        self.steady_time = 0;
        for channel in self.input.iter_mut().chain(self.output.iter_mut()) {
            channel.fill(0.0);
        }
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count().min(self.max_frames);
        if frames == 0 {
            return;
        }
        let channels = self.input.len().min(buffer.channel_count());

        // Destructured so the audio buffers and the processor are borrowed as separate fields.
        let Self {
            processor,
            params,
            values,
            changed,
            outgoing,
            replies,
            input_ports,
            output_ports,
            input,
            output,
            steady_time,
            ..
        } = self;

        outgoing.clear();
        replies.clear();
        for (index, dirty) in changed.iter_mut().enumerate() {
            if !std::mem::take(dirty) {
                continue;
            }
            outgoing.push(&ParamValueEvent::new(
                0,
                params.clap_ids[index],
                Pckn::match_all(),
                values[index] as f64,
                Cookie::empty(),
            ));
        }

        for (index, channel) in input.iter_mut().enumerate().take(channels) {
            channel[..frames].copy_from_slice(&buffer.channel(index)[..frames]);
        }

        let Ok(processor) = processor.ensure_processing_started() else {
            // The plugin refused to start. Leaving the buffer untouched passes the audio through
            // unchanged, which is the least destructive thing a broken effect can do.
            return;
        };

        let events = InputEvents::from_buffer(outgoing);
        let mut event_replies = OutputEvents::from_buffer(replies);

        let audio_in = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input
                    .iter_mut()
                    .take(channels)
                    .map(|channel| InputChannel::variable(&mut channel[..frames])),
            ),
        }]);
        let mut audio_out = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output
                    .iter_mut()
                    .take(channels)
                    .map(|channel| &mut channel[..frames]),
            ),
        }]);

        let rendered = processor.process(
            &audio_in,
            &mut audio_out,
            &events,
            &mut event_replies,
            Some(*steady_time),
            None,
        );
        *steady_time = steady_time.wrapping_add(frames as u64);

        if rendered.is_err() {
            return;
        }

        for (index, channel) in output.iter().enumerate().take(channels) {
            buffer.channel_mut(index)[..frames].copy_from_slice(&channel[..frames]);
        }
    }

    fn latency_frames(&self) -> usize {
        self.latency
    }
}

impl std::fmt::Debug for ClapEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClapEffect")
            .field("id", &self.descriptor.id)
            .field("parameters", &self.params.descriptors.len())
            .field("latency", &self.latency)
            .finish_non_exhaustive()
    }
}
