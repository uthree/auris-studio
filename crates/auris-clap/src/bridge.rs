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
use auris_core::plugin::{NoteEvent, PluginDescriptor, PrepareContext, ProcessContext};
use clack_host::events::event_types::{
    MidiEvent, NoteChokeEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent,
    ParamValueEvent, TransportEvent, TransportFlags,
};
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::{Match, Pckn, UnknownEvent};
use clack_host::prelude::*;

use crate::host::AurisHost;
use crate::notes::{NoteLanguage, Translated, translate};
use crate::plugin::ParamList;
use crate::ports::PortLayout;

/// Event room a block starts with when the host will not say how much it needs.
///
/// A floor rather than the answer. There is no fixed number that is both safe and not absurd — a
/// block holds as many notes as the arrangement puts in it — which is why
/// [`PrepareContext::max_block_events`](auris_core::plugin::PrepareContext::max_block_events)
/// exists and why the render graph fills it in from the arrangement it has just scheduled. This is
/// what is left for the callers that cannot: an effect prepared on its own, a test, an example.
///
/// A block that exceeds the prepared room is rejected without growing the queue. The graph supplies
/// the exact worst-case count during preparation; this floor is only for standalone callers.
const EVENT_HEADROOM: usize = 256;

/// Largest input-event queue one hosted instance may pre-allocate.
///
/// The render graph already bounds a track's complete flattened schedule, but
/// [`PrepareContext`] is also a public input and may carry a hostile `usize`. More importantly,
/// [`EventBuffer::with_capacity`] reserves worst-case standard-event storage for every requested
/// slot, so even a numerically valid but unrealistic count can multiply into a process-sized
/// allocation. Four thousand and ninety-six simultaneous events is already far beyond an ordinary
/// realtime block; a denser block is rejected as one gesture and counted by
/// [`Bridge::dropped_input_events`] instead of allocating more host storage.
pub(crate) const MAX_PREPARED_EVENT_ROOM: usize = 4_096;

/// Prepared input-event room, including all dirty parameters and a pending note reset.
///
/// [`PrepareContext`] is public and its count may originate in a decoded document or another
/// host. Saturation prevents wraparound and the explicit ceiling prevents that numerical boundary
/// from becoming a huge allocation. Requests above the ceiling follow the same whole-gesture
/// drop-and-diagnostic contract as any other understated preparation.
fn prepared_event_room(parameter_count: usize, max_block_events: usize) -> usize {
    parameter_count
        .saturating_add(max_block_events.max(EVENT_HEADROOM))
        .saturating_add(1)
        .min(MAX_PREPARED_EVENT_ROOM)
}

/// An allocation-free output sink that publishes global parameter changes to the UI.
///
/// [`Effect`](auris_core::plugin::Effect) and [`Instrument`](auris_core::plugin::Instrument) have
/// no event-output channel, so retaining these events in an [`EventBuffer`] only made an unused
/// vector capable of growing on the audio thread. Global parameter values update preallocated
/// atomics; unhandled events are counted and discarded without retaining an event queue.
struct DiscardedOutputEvents {
    count: u64,
    params: Arc<ParamList>,
}

impl OutputEventBuffer for DiscardedOutputEvents {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        if let Some(value) = event.as_event::<ParamValueEvent>()
            && value.pckn() == Pckn::match_all()
            && let Some(index) = value
                .param_id()
                .and_then(|id| self.params.by_clap_id.get(&id.get()))
        {
            self.params
                .set_value(ParamId(*index as u32), value.value() as f32);
            return Ok(());
        }
        self.count = self.count.saturating_add(1);
        Ok(())
    }
}

/// A plugin instance as the render graph drives it.
pub(crate) struct Bridge {
    processor: PluginAudioProcessor<AurisHost>,
    descriptor: PluginDescriptor,
    params: Arc<ParamList>,
    values: Vec<f32>,
    changed: Vec<bool>,
    outgoing: EventBuffer,
    // Keep display values current without a main-thread query racing the audio processor.
    // Generated notes have no downstream route and are counted without allocating storage.
    replies: DiscardedOutputEvents,
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
    /// Which port a key goes in, for a plugin that declared somewhere to put one.
    sidechain_input: Option<usize>,
    /// What the plugin's note input port speaks, or `None` if it has none — which is what an
    /// effect has, and also what an instrument Auris cannot drive has.
    language: Option<NoteLanguage>,
    /// How many events `outgoing` was sized for, parameters included.
    ///
    /// Written down because [`EventBuffer`] cannot be asked its capacity, and
    /// [`Self::reserve_events`] needs the old answer to know whether a new one is bigger.
    event_room: usize,
    /// Requested host-to-plugin events rejected because the caller exceeded `event_room`.
    dropped_input_events: u64,
    max_frames: usize,
    latency: usize,
    /// A counter that only ever goes up, which is what CLAP asks of `steady_time`. The project
    /// playhead is not usable for it: a cycle sends the playhead backwards, and a plugin is
    /// entitled to treat that as impossible.
    steady_time: u64,
    /// A reset must also end notes: some plugins reset only their processing clock.
    pending_note_reset: bool,
    processing_failed: bool,
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
        // What the host says a block can carry, and the old flat guess as a floor under it: a
        // host that says nothing — an effect prepared on its own, a test — still gets room for a
        // reasonable block, and one that has counted the arrangement gets the count.
        let count = params.descriptors.len();
        let event_room = prepared_event_room(count, ctx.max_block_events);
        let values = (0..count)
            .map(|index| params.value(ParamId(index as u32)).unwrap_or(0.0))
            .collect();
        let room = |port: &usize| vec![vec![0.0; frames]; *port];

        Self {
            processor: processor.into(),
            descriptor,
            params: Arc::clone(&params),
            values,
            changed: vec![false; count],
            outgoing: EventBuffer::with_capacity(event_room),
            replies: DiscardedOutputEvents {
                count: 0,
                params: Arc::clone(&params),
            },
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
            sidechain_input: ports.sidechain_input(),
            main_input: ports.main_input,
            main_output: ports.main_output,
            language,
            event_room,
            dropped_input_events: 0,
            max_frames: frames,
            latency,
            steady_time: 0,
            pending_note_reset: false,
            processing_failed: false,
        }
    }

    /// Gives the processor back so the plugin can be deactivated.
    pub(crate) fn into_stopped(self) -> StoppedPluginAudioProcessor<AurisHost> {
        self.processor.into_stopped()
    }

    pub(crate) fn processing_failed(&self) -> bool {
        self.processing_failed
    }

    /// Grows the event buffers to hold what a block is now known to carry.
    ///
    /// Activation sized them, but activation happens before the render graph has counted the
    /// arrangement — the session prepares a plugin with a count of zero and the graph corrects
    /// it through [`Instrument::prepare`](auris_core::plugin::Instrument::prepare) afterwards.
    /// Rate and block size cannot be told to a CLAP plugin twice; these buffers are the host's
    /// own, and re-sizing them here, off the audio thread, is what keeps `render`'s pushes from
    /// allocating on it.
    pub(crate) fn reserve_events(&mut self, max_block_events: usize) {
        let count = self.params.descriptors.len();
        let room = prepared_event_room(count, max_block_events);
        if room > self.event_room {
            self.outgoing = EventBuffer::with_capacity(room);
            self.event_room = room;
        }
    }

    pub(crate) fn descriptor(&self) -> PluginDescriptor {
        self.descriptor.clone()
    }

    pub(crate) fn latency(&self) -> usize {
        self.latency
    }

    /// How many events the queues hold before a push would allocate. For tests.
    #[cfg(test)]
    pub(crate) fn event_room(&self) -> usize {
        self.event_room
    }

    /// How many requested input events have been rejected instead of growing on the audio thread.
    pub(crate) fn dropped_input_events(&self) -> u64 {
        self.dropped_input_events
    }

    /// How many plugin-generated events have been accepted and discarded because Auris has no
    /// event-output route yet.
    pub(crate) fn discarded_output_events(&self) -> u64 {
        self.replies.count
    }

    /// Whether the plugin declared a port for a key to go in.
    pub(crate) fn has_sidechain(&self) -> bool {
        self.sidechain_input.is_some()
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
        self.params.set_value(id, self.values[id.index()]);
        // Marking rather than queueing keeps this allocation-free however often it is called:
        // a hundred automation writes in one block still send one event.
        self.changed[id.index()] = true;
    }

    pub(crate) fn reset(&mut self) {
        self.processor.reset();
        self.pending_note_reset = self.language.is_some();
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
    ///
    /// `sidechain` goes in the first port that is not the main one, where the plugin declared one
    /// to put it in. `None` fills that port with silence rather than leaving it — a slot that has
    /// stopped being keyed must not go on hearing the last block it was given.
    pub(crate) fn render(
        &mut self,
        buffer: &mut AudioBuffer,
        notes: &[NoteEvent],
        overwrite: bool,
        sidechain: Option<&AudioBuffer>,
        ctx: &ProcessContext,
    ) {
        let total_frames = buffer.frame_count();
        if total_frames == 0 {
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
            sidechain_input,
            language,
            event_room,
            dropped_input_events,
            steady_time,
            pending_note_reset,
            processing_failed,
            ..
        } = self;

        let Ok(processor) = processor.ensure_processing_started() else {
            *processing_failed = true;
            // The plugin refused to start.
            if overwrite {
                silence(buffer, 0, total_frames);
            }
            return;
        };

        // An offline renderer may hand us a block larger than the size at which CLAP was
        // activated. CLAP cannot be resized in place, so drive it in legal-sized pieces while
        // keeping the caller's block (and its event timestamps) whole.
        let mut offset = 0;
        while offset < total_frames {
            let frames = (total_frames - offset).min(self.max_frames);
            let end = offset + frames;

            outgoing.clear();
            let dirty_count = changed.iter().filter(|dirty| **dirty).count();
            let note_count = language.map_or(0, |language| {
                notes
                    .iter()
                    .filter(|note| {
                        let at = note.frame() as usize;
                        at >= offset
                            && at < end
                            && !matches!(translate(**note, language), Translated::Nothing)
                    })
                    .count()
            });
            let reset_notes = *pending_note_reset && language.is_some();
            let required = dirty_count
                .saturating_add(note_count)
                .saturating_add(usize::from(reset_notes));

            // A caller that skipped or understated preparation must not turn an input burst into
            // a vector growth on the audio thread. Do not send a partial gesture: losing a late
            // note-off would leave a voice stuck. Keep parameter changes dirty for the next block
            // and replace the rejected note set with one dialect-correct emergency choke.
            let overflowed = required > *event_room;
            if overflowed {
                *processing_failed = true;
                add_dropped_events(dropped_input_events, required);
            }
            if (overflowed || reset_notes)
                && let Some(language) = *language
            {
                // Send before new notes so a seek can chase voices in the same block.
                // EVENT_HEADROOM keeps room even for an unprepared caller.
                if push_note(
                    outgoing,
                    NoteEvent::AllSoundOff { frame: 0 },
                    language,
                    *event_room,
                ) {
                    *pending_note_reset = false;
                }
            }
            // Parameters before notes: an automated cutoff belongs to the block it was written
            // for, and a note struck in the same block should hear it. Dirty flags are only taken
            // after start_processing succeeded, so a refused start retries them next time.
            if !overflowed {
                for (index, dirty) in changed.iter_mut().enumerate() {
                    if !std::mem::take(dirty) {
                        continue;
                    }
                    if !push_bounded(
                        outgoing,
                        &ParamValueEvent::new(
                            0,
                            params.clap_ids[index],
                            Pckn::match_all(),
                            values[index] as f64,
                        ),
                        *event_room,
                    ) {
                        // The preflight and capacity use the same count, so reaching this branch
                        // means their contract drifted. Stay RT-safe and retry the parameter.
                        *dirty = true;
                        *processing_failed = true;
                        add_dropped_events(dropped_input_events, 1);
                    }
                }
                if let Some(language) = *language {
                    for note in notes {
                        let at = note.frame() as usize;
                        if at >= offset
                            && at < end
                            && !push_note(
                                outgoing,
                                note.with_frame((at - offset) as u32),
                                language,
                                *event_room,
                            )
                        {
                            *processing_failed = true;
                            add_dropped_events(dropped_input_events, 1);
                        }
                    }
                }
            }

            // The track's audio in the main port, the key in the first spare one, and silence in
            // everything either of them does not reach.
            if let Some(port) = main_input.and_then(|index| input.get_mut(index)) {
                fill_port(port, Some(buffer), offset, frames);
            }
            if let Some(port) = sidechain_input.and_then(|index| input.get_mut(index)) {
                fill_port(port, sidechain, offset, frames);
            }

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
            let transport = transport_event(ctx, offset);

            let rendered = processor.process(
                &audio_in,
                &mut audio_out,
                &events,
                &mut event_replies,
                Some(*steady_time),
                Some(&transport),
            );
            *steady_time = steady_time.wrapping_add(frames as u64);
            *processing_failed |= rendered.is_err();

            let main_port = main_output.and_then(|index| output.get(index));
            if overwrite && main_port.is_none_or(Vec::is_empty) {
                *processing_failed = true;
            }
            match main_port {
                Some(port) if rendered.is_ok() => {
                    deliver_port(port, buffer, offset, frames, overwrite)
                }
                _ if overwrite => silence(buffer, offset, frames),
                _ => {}
            }
            offset = end;
        }
    }
}

/// The CLAP view of the per-block transport state Auris gives every processor.
///
/// A large offline block may be divided into several legal CLAP blocks, so `offset` advances the
/// timeline to the first sample of the piece being processed rather than repeating the outer
/// block's position for each piece.
fn transport_event(ctx: &ProcessContext, offset: usize) -> TransportEvent {
    let samples = ctx.playhead_samples.saturating_add(offset as u64);
    let seconds = samples as f64 / ctx.sample_rate;
    let beats = seconds * ctx.bpm / 60.0;
    let mut flags = TransportFlags::HAS_TEMPO
        | TransportFlags::HAS_BEATS_TIMELINE
        | TransportFlags::HAS_SECONDS_TIMELINE;
    if ctx.is_playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    TransportEvent {
        header: Default::default(),
        flags,
        song_pos_beats: beats.into(),
        song_pos_seconds: seconds.into(),
        tempo: ctx.bpm,
        tempo_inc: 0.0,
        loop_start_beats: Default::default(),
        loop_end_beats: Default::default(),
        loop_start_seconds: Default::default(),
        loop_end_seconds: Default::default(),
        bar_start: Default::default(),
        bar_number: 0,
        time_signature_numerator: 0,
        time_signature_denominator: 0,
    }
}

/// Copies the plugin's main output into the track's buffer, finishing what the port misses.
///
/// A port narrower than the buffer repeats across the remaining channels. This applies to both
/// instruments and effects: retaining an effect's dry input in those channels would mix it back
/// into the processed result when a singer strip folds to mono. `overwrite` only controls the
/// fallback for an empty port: instruments owe silence, while effects pass the input through.
/// The input-side mirror is [`fill_port`].
fn deliver_port(
    port: &[Vec<f32>],
    buffer: &mut AudioBuffer,
    offset: usize,
    frames: usize,
    overwrite: bool,
) {
    if port.is_empty() {
        if overwrite {
            silence(buffer, offset, frames);
        }
        return;
    }
    for index in 0..buffer.channel_count() {
        let source = &port[index % port.len()];
        buffer.channel_mut(index)[offset..offset + frames].copy_from_slice(&source[..frames]);
    }
}

/// Copies `source` into one port's channels, silencing whatever it does not reach.
///
/// A plugin indexes its ports itself, so every channel of every port it declared has to hold
/// something meant for it. Silence is what a channel gets when the source has none to give it —
/// a stereo track feeding a four-channel port, or a port with nothing routed to it at all — and
/// it has to be written rather than assumed, because the buffer is kept from block to block.
fn fill_port(port: &mut [Vec<f32>], source: Option<&AudioBuffer>, offset: usize, frames: usize) {
    for (index, channel) in port.iter_mut().enumerate() {
        let taken = source
            .and_then(|buffer| buffer.channels().get(index))
            .map_or(0, |samples| {
                let count = frames.min(samples.len().saturating_sub(offset));
                channel[..count].copy_from_slice(&samples[offset..offset + count]);
                count
            });
        channel[taken..frames].fill(0.0);
    }
}

/// Zeroes the first `frames` of every channel.
fn silence(buffer: &mut AudioBuffer, offset: usize, frames: usize) {
    for channel in 0..buffer.channel_count() {
        buffer.channel_mut(channel)[offset..offset + frames].fill(0.0);
    }
}

/// Adds one of the standard events Auris creates, provided the prepared room is not exhausted.
///
/// `EventBuffer::with_capacity` reserves enough bytes for the largest standard CLAP event, and
/// Auris only constructs standard events below. Guarding the index count therefore bounds both of
/// its internal vectors without relying on `EventBuffer::push`, which grows them when full.
fn push_bounded<E: AsRef<UnknownEvent> + ?Sized>(
    queue: &mut EventBuffer,
    event: &E,
    event_room: usize,
) -> bool {
    if queue.len() as usize >= event_room {
        return false;
    }
    queue.push(event);
    true
}

/// Saturating diagnostic accounting for input events that were not delivered.
fn add_dropped_events(counter: &mut u64, count: usize) {
    *counter = counter.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
}

/// Pushes whatever `note` translates to onto the outgoing queue.
///
/// The event's own frame is its timestamp, so a note lands on the sample it was scheduled for
/// rather than at the start of the block it fell in.
fn push_note(
    queue: &mut EventBuffer,
    note: NoteEvent,
    language: NoteLanguage,
    event_room: usize,
) -> bool {
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
        Translated::NoteOn { key, velocity } => push_bounded(
            queue,
            &NoteOnEvent::new(at, voice(key), velocity),
            event_room,
        ),
        // Zero release velocity: Auris does not record one, and inventing a number a plugin might
        // map to its release stage would be putting a gesture in that nobody made.
        Translated::NoteOff { key } => {
            push_bounded(queue, &NoteOffEvent::new(at, voice(key), 0.0), event_room)
        }
        Translated::ReleaseAll => push_bounded(
            queue,
            &NoteOffEvent::new(at, Pckn::match_all(), 0.0),
            event_room,
        ),
        Translated::ChokeAll => push_bounded(
            queue,
            &NoteChokeEvent::new(at, Pckn::match_all()),
            event_room,
        ),
        Translated::Tuning(semitones) => push_bounded(
            queue,
            &NoteExpressionEvent::new(at, Pckn::match_all(), NoteExpressionType::Tuning, semitones),
            event_room,
        ),
        Translated::Midi(data) => push_bounded(queue, &MidiEvent::new(at, 0, data), event_room),
        Translated::Nothing => true,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_carries_tempo_position_and_playing_state_for_each_sub_block() {
        let ctx = ProcessContext::realtime(48_000.0, 48_000, 48_000, 90.0, true);
        let transport = transport_event(&ctx, 24_000);

        assert_eq!(transport.tempo, 90.0);
        assert_eq!(transport.song_pos_seconds.to_float(), 1.5);
        assert_eq!(transport.song_pos_beats.to_float(), 2.25);
        assert!(transport.flags.contains(TransportFlags::HAS_TEMPO));
        assert!(
            transport
                .flags
                .contains(TransportFlags::HAS_SECONDS_TIMELINE)
        );
        assert!(transport.flags.contains(TransportFlags::HAS_BEATS_TIMELINE));
        assert!(transport.flags.contains(TransportFlags::IS_PLAYING));

        let stopped = transport_event(&ProcessContext::realtime(48_000.0, 1, 0, 123.0, false), 0);
        assert!(!stopped.flags.contains(TransportFlags::IS_PLAYING));
    }

    #[test]
    fn hostile_event_counts_are_capped_instead_of_wrapping_or_requesting_huge_storage() {
        assert_eq!(prepared_event_room(7, 2_048), 2_056);
        assert_eq!(prepared_event_room(7, 4_096), MAX_PREPARED_EVENT_ROOM);
        assert_eq!(prepared_event_room(7, 0), EVENT_HEADROOM + 8);
        assert_eq!(prepared_event_room(1, usize::MAX), MAX_PREPARED_EVENT_ROOM);
        assert_eq!(
            prepared_event_room(usize::MAX, EVENT_HEADROOM),
            MAX_PREPARED_EVENT_ROOM
        );
    }

    /// A stereo buffer whose every sample is `residue` — what the previous block left behind.
    fn stale(residue: f32, frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::stereo(frames, 48_000.0);
        for channel in 0..buffer.channel_count() {
            buffer.channel_mut(channel)[..frames].fill(residue);
        }
        buffer
    }

    #[test]
    fn a_mono_port_reaches_every_channel_of_an_overwriting_buffer() {
        // The instrument contract: `out` is overwritten, all of it. The renderer skips its
        // clear on that promise, so a channel the port does not reach used to keep the
        // previous block's samples — a ghost of whatever played there before.
        let port = vec![vec![0.25f32; 8]];
        let mut buffer = stale(0.9, 8);
        deliver_port(&port, &mut buffer, 0, 8, true);
        assert_eq!(buffer.channel(0), &[0.25; 8]);
        assert_eq!(
            buffer.channel(1),
            &[0.25; 8],
            "the second channel repeats the port, not the past"
        );
    }

    #[test]
    fn a_mono_effect_replaces_both_channels_without_leaking_dry_audio() {
        // Singer strips fold the two channels together after the chain. Leaving dry audio in
        // the second channel would mix the original voice back into a mono voice converter.
        let port = vec![vec![0.25f32; 8]];
        let mut buffer = stale(0.9, 16);
        deliver_port(&port, &mut buffer, 4, 8, false);
        for channel in buffer.channels() {
            assert_eq!(&channel[..4], &[0.9; 4]);
            assert_eq!(&channel[4..12], &[0.25; 8], "only the wet signal remains");
            assert_eq!(&channel[12..], &[0.9; 4]);
        }
    }

    #[test]
    fn a_port_with_no_channels_owes_an_overwriting_buffer_silence() {
        let port: Vec<Vec<f32>> = Vec::new();
        let mut buffer = stale(0.9, 8);
        deliver_port(&port, &mut buffer, 0, 8, true);
        assert_eq!(buffer.channel(0), &[0.0; 8]);
        assert_eq!(buffer.channel(1), &[0.0; 8]);
    }

    #[test]
    fn an_empty_effect_port_keeps_the_passthrough_fallback() {
        let mut buffer = stale(0.9, 8);
        deliver_port(&[], &mut buffer, 0, 8, false);
        for channel in buffer.channels() {
            assert_eq!(channel, &[0.9; 8]);
        }
    }

    #[test]
    fn a_stereo_effect_preserves_both_processed_channels() {
        let port = vec![vec![0.25; 8], vec![-0.5; 8]];
        let mut buffer = stale(0.9, 8);
        deliver_port(&port, &mut buffer, 0, 8, false);
        assert_eq!(buffer.channels(), port.as_slice());
    }
}
