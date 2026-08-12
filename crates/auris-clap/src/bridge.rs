//! The audio-thread half of a hosted plugin, whichever kind it turned out to be.
//!
//! An effect and an instrument differ in almost nothing here. Both hand the plugin every audio
//! port it declared, both send parameter changes as events on the block that applies them, and
//! both take back whatever came out of the main output port. The differences are that an
//! instrument usually declares no audio input, and that it is given notes. Neither is worth a
//! second copy of the port bookkeeping — the copy is where the two would drift apart, and the port
//! bookkeeping is the part that crashes when it is wrong.

use std::sync::Arc;

use auris_core::buffer::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{NoteEvent, PluginDescriptor, PrepareContext};
use clack_host::events::event_types::{
    MidiEvent, NoteChokeEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent,
    ParamValueEvent,
};
use clack_host::events::{Match, Pckn};
use clack_host::prelude::*;
use clack_host::utils::Cookie;

use crate::host::AurisHost;
use crate::notes::{NoteLanguage, Translated, translate};
use crate::plugin::ParamList;
use crate::ports::PortLayout;

/// How much event room a block starts with, over and above one event per parameter.
///
/// A block with more notes than this in it costs one allocation, on the audio thread, once: the
/// buffer is reused every block and keeps whatever capacity it grew to. That is the honest trade
/// against sizing for a worst case — a block can hold as many notes as the arrangement puts in it,
/// and there is no number that is both safe and not absurd.
const EVENT_HEADROOM: usize = 256;

/// A plugin instance as the render graph drives it.
pub(crate) struct Bridge {
    processor: PluginAudioProcessor<AurisHost>,
    descriptor: PluginDescriptor,
    params: Arc<ParamList>,
    values: Vec<f32>,
    changed: Vec<bool>,
    outgoing: EventBuffer,
    replies: EventBuffer,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    /// A buffer per channel of every input port the plugin declared, in its order. Every port is
    /// here, including the ones Auris has nothing to put in: the plugin indexes this array itself,
    /// and a port it declared but did not get is a read off the end.
    input: Vec<Vec<Vec<f32>>>,
    /// The same for outputs.
    output: Vec<Vec<Vec<f32>>>,
    /// Which port the track's audio goes in and comes out of.
    main_input: Option<usize>,
    main_output: Option<usize>,
    /// What the plugin's note input port speaks, or `None` if it has none — which is what an
    /// effect has, and also what an instrument Auris cannot drive has.
    language: Option<NoteLanguage>,
    max_frames: usize,
    latency: usize,
    /// A counter that only ever goes up, which is what CLAP asks of `steady_time`. The project
    /// playhead is not usable for it: a cycle sends the playhead backwards, and a plugin is
    /// entitled to treat that as impossible.
    steady_time: u64,
}

impl Bridge {
    /// Wraps a freshly activated audio processor.
    pub(crate) fn new(
        processor: StoppedPluginAudioProcessor<AurisHost>,
        descriptor: PluginDescriptor,
        params: Arc<ParamList>,
        ports: PortLayout,
        language: Option<NoteLanguage>,
        ctx: &PrepareContext,
        latency: usize,
    ) -> Self {
        let frames = ctx.max_block_frames.max(1);
        let count = params.descriptors.len();
        let values = params.descriptors.iter().map(|p| p.default).collect();
        let room = |port: &usize| vec![vec![0.0; frames]; *port];

        Self {
            processor: processor.into(),
            descriptor,
            params,
            values,
            changed: vec![false; count],
            outgoing: EventBuffer::with_capacity(count + EVENT_HEADROOM),
            replies: EventBuffer::with_capacity(count + EVENT_HEADROOM),
            input_ports: AudioPorts::with_capacity(
                ports.input_channels().max(1),
                ports.inputs.len().max(1),
            ),
            output_ports: AudioPorts::with_capacity(
                ports.output_channels().max(1),
                ports.outputs.len().max(1),
            ),
            input: ports.inputs.iter().map(room).collect(),
            output: ports.outputs.iter().map(room).collect(),
            main_input: ports.main_input,
            main_output: ports.main_output,
            language,
            max_frames: frames,
            latency,
            steady_time: 0,
        }
    }

    /// Gives the processor back so the plugin can be deactivated.
    pub(crate) fn into_stopped(self) -> StoppedPluginAudioProcessor<AurisHost> {
        self.processor.into_stopped()
    }

    pub(crate) fn descriptor(&self) -> PluginDescriptor {
        self.descriptor.clone()
    }

    pub(crate) fn latency(&self) -> usize {
        self.latency
    }

    pub(crate) fn parameters(&self) -> &[ParamDescriptor] {
        &self.params.descriptors
    }

    pub(crate) fn param(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or(0.0)
    }

    pub(crate) fn set_param(&mut self, id: ParamId, value: f32) {
        let Some(descriptor) = self.params.descriptors.get(id.index()) else {
            return;
        };
        self.values[id.index()] = descriptor.clamp(value);
        // Marking rather than queueing keeps this allocation-free however often it is called:
        // a hundred automation writes in one block still send one event.
        self.changed[id.index()] = true;
    }

    pub(crate) fn reset(&mut self) {
        self.processor.reset();
        self.steady_time = 0;
        for port in self.input.iter_mut().chain(self.output.iter_mut()) {
            for channel in port {
                channel.fill(0.0);
            }
        }
    }

    /// Renders one block, sending `notes` first and whatever parameters have moved with them.
    ///
    /// `overwrite` says what happens when the plugin produces nothing: an instrument's contract is
    /// to overwrite its output, so a plugin that refuses to run has to leave silence, while an
    /// effect leaving the buffer alone passes the audio through untouched.
    pub(crate) fn render(
        &mut self,
        buffer: &mut AudioBuffer,
        notes: &[NoteEvent],
        overwrite: bool,
    ) {
        let frames = buffer.frame_count().min(self.max_frames);
        if frames == 0 {
            return;
        }

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
            main_input,
            main_output,
            language,
            steady_time,
            ..
        } = self;

        outgoing.clear();
        replies.clear();
        // Parameters before notes: an automated cutoff belongs to the block it was written for,
        // and a note struck in the same block should hear it.
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
        if let Some(language) = *language {
            for note in notes {
                push_note(outgoing, *note, language);
            }
        }

        // Only the main port is filled. Every other one — a vocoder's modulator, a compressor's
        // sidechain — is left at silence, which is the honest answer while nothing in Auris can
        // route audio to it, and is a great deal better than not passing it at all.
        if let Some(port) = main_input.and_then(|index| input.get_mut(index)) {
            for (index, channel) in port.iter_mut().enumerate().take(buffer.channel_count()) {
                channel[..frames].copy_from_slice(&buffer.channel(index)[..frames]);
            }
        }

        let Ok(processor) = processor.ensure_processing_started() else {
            // The plugin refused to start.
            if overwrite {
                silence(buffer, frames);
            }
            return;
        };

        let events = InputEvents::from_buffer(outgoing);
        let mut event_replies = OutputEvents::from_buffer(replies);

        let audio_in = input_ports.with_input_buffers(input.iter_mut().map(|port| {
            AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    port.iter_mut()
                        .map(|channel| InputChannel::variable(&mut channel[..frames])),
                ),
            }
        }));
        let mut audio_out =
            output_ports.with_output_buffers(output.iter_mut().map(|port| AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    port.iter_mut().map(|channel| &mut channel[..frames]),
                ),
            }));

        let rendered = processor.process(
            &audio_in,
            &mut audio_out,
            &events,
            &mut event_replies,
            Some(*steady_time),
            None,
        );
        *steady_time = steady_time.wrapping_add(frames as u64);

        match main_output.and_then(|index| output.get(index)) {
            Some(port) if rendered.is_ok() => {
                for (index, channel) in port.iter().enumerate().take(buffer.channel_count()) {
                    buffer.channel_mut(index)[..frames].copy_from_slice(&channel[..frames]);
                }
            }
            _ if overwrite => silence(buffer, frames),
            _ => {}
        }
    }
}

/// Zeroes the first `frames` of every channel.
fn silence(buffer: &mut AudioBuffer, frames: usize) {
    for channel in 0..buffer.channel_count() {
        buffer.channel_mut(channel)[..frames].fill(0.0);
    }
}

/// Pushes whatever `note` translates to onto the outgoing queue.
///
/// The event's own frame is its timestamp, so a note lands on the sample it was scheduled for
/// rather than at the start of the block it fell in.
fn push_note(queue: &mut EventBuffer, note: NoteEvent, language: NoteLanguage) {
    let at = note.frame();
    // Port zero and channel zero: Auris has one note stream per track, and no concept of a
    // plugin's second note port to point the other one at.
    let voice = |key: u16| Pckn {
        port_index: Match::Specific(0),
        channel: Match::Specific(0),
        key: Match::Specific(key),
        note_id: Match::All,
    };

    match translate(note, language) {
        Translated::NoteOn { key, velocity } => {
            queue.push(&NoteOnEvent::new(at, voice(key), velocity))
        }
        // Zero release velocity: Auris does not record one, and inventing a number a plugin might
        // map to its release stage would be putting a gesture in that nobody made.
        Translated::NoteOff { key } => queue.push(&NoteOffEvent::new(at, voice(key), 0.0)),
        Translated::ReleaseAll => queue.push(&NoteOffEvent::new(at, Pckn::match_all(), 0.0)),
        Translated::ChokeAll => queue.push(&NoteChokeEvent::new(at, Pckn::match_all())),
        Translated::Tuning(semitones) => queue.push(&NoteExpressionEvent::new(
            at,
            Pckn::match_all(),
            NoteExpressionType::Tuning,
            semitones,
        )),
        Translated::Midi(data) => queue.push(&MidiEvent::new(at, 0, data)),
        Translated::Nothing => {}
    };
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("id", &self.descriptor.id)
            .field("parameters", &self.params.descriptors.len())
            .field("notes", &self.language)
            .field("latency", &self.latency)
            .finish_non_exhaustive()
    }
}
