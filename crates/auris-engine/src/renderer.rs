//! The block renderer shared by realtime playback and offline export.
//!
//! One function fills one output buffer. Realtime playback calls it from the device callback and
//! the offline renderer calls it in a loop as fast as the CPU allows; both take exactly the same
//! path, which is what makes an export sound like what was heard.
//!
//! The output buffer's length is *not* the processing block size. [`render_block`] splits the
//! request into segments so that no segment is longer than the size the graph was prepared for
//! and no segment straddles the loop end. Splitting rather than jumping is what keeps a loop
//! sample-continuous: the frames up to the loop point are rendered, the transport wraps, and the
//! rest of the same buffer is rendered from the loop start.

use auris_core::AudioBuffer;
use auris_core::plugin::{NoteEvent, ProcessContext};

use crate::graph::{
    PITCH_COUNT, RenderAudioClip, RenderGraph, RenderSend, RenderSource, RenderTrack,
    ScheduledEvent,
};
use crate::transport::Transport;

/// Renders `out.frame_count()` frames starting at the transport's position.
///
/// `out` is overwritten, not added to. The transport is advanced past everything rendered, so
/// calling this in a loop walks the timeline. When the transport is stopped the sources fall
/// silent but every effect still runs, letting reverb and delay tails ring out.
pub fn render_block(
    graph: &mut RenderGraph,
    transport: &mut Transport,
    out: &mut AudioBuffer,
    offline: bool,
) {
    out.clear();
    let total = out.frame_count();
    for track in graph.tracks_mut() {
        track.peak = 0.0;
    }
    graph.master_peak = [0.0, 0.0];
    if total == 0 {
        return;
    }

    let mut written = 0;
    while written < total {
        let frames = segment_frames(graph, transport, total - written);
        render_segment(graph, transport, out, written, frames, offline);
        transport.advance(frames as u64);
        written += frames;
    }
}

/// Length of the next segment: bounded by what is left, by the prepared block size, and — while
/// the transport is rolling — by the distance to the loop end.
///
/// A stopped transport is excluded because [`Transport::advance`] leaves it where it is: it can
/// never reach the loop end, so splitting there would only re-run the whole graph on a one-frame
/// segment for every remaining frame of the callback, and turn every gain ramp into a step.
fn segment_frames(graph: &RenderGraph, transport: &Transport, remaining: usize) -> usize {
    let mut frames = remaining.min(graph.max_block());
    if transport.playing
        && let Some(to_loop_end) = transport.frames_to_loop_end()
        && to_loop_end < frames as u64
    {
        frames = to_loop_end as usize;
    }
    frames.max(1)
}

fn render_segment(
    graph: &mut RenderGraph,
    transport: &Transport,
    out: &mut AudioBuffer,
    offset: usize,
    frames: usize,
    offline: bool,
) {
    // Before anything is rendered, so the whole segment hears the values in force at its start.
    // A segment is bounded by the prepared block size and by the loop end, which means a wrap
    // re-reads the lanes at the loop start rather than carrying the values from its end across —
    // and, because that is a jump rather than a step, arrives at them outright.
    graph.apply_automation(transport.position_frames, frames);

    let ctx = ProcessContext {
        sample_rate: graph.sample_rate(),
        block_frames: frames,
        playhead_samples: transport.position_frames,
        bpm: graph.bpm_at_frame(transport.position_frames),
        is_playing: transport.playing,
        is_offline: offline,
    };

    let master_scratch = &mut graph.master_scratch;
    master_scratch.set_frame_count(frames);
    master_scratch.clear();

    // Emptied before anything is rendered, so a bus sums this segment alone rather than whatever
    // the last one left in it.
    let bus_inputs = &mut graph.bus_inputs;
    for input in bus_inputs.iter_mut() {
        input.set_frame_count(frames);
        input.clear();
    }

    // A separate field borrow, the same way `master_scratch` is one, so the scope stays reachable
    // while the tracks are being walked mutably.
    let scope = &graph.scope;
    let watching = scope.watching();

    // The routing order rather than the track list: a bus cannot go through its own strip until
    // everything routed into it has been mixed in, and only this order guarantees that.
    let tracks = &mut graph.tracks;
    for &index in &graph.order {
        let Some(track) = tracks.get_mut(index) else {
            continue;
        };
        // A track muted long enough to have finished fading out contributes nothing, so skip it
        // entirely. One muted a moment ago still has a fade to slide down and must be rendered.
        if track.strip.is_silent() {
            continue;
        }
        track.scratch.set_frame_count(frames);
        render_source(track, bus_inputs, transport, &ctx);
        for (effect, enabled) in track.strip.effects.iter_mut().zip(&track.strip.enabled) {
            if *enabled {
                effect.process(&mut track.scratch, &ctx);
            }
        }
        // Delay compensation sits between the chain and the fader: the chain is what runs late,
        // and putting it here keeps the fader and the mute acting when they are moved rather
        // than a few milliseconds afterwards. Every copy the track goes on to produce is taken
        // after this, because this part of the delay belongs to all of them.
        track.delay.process(&mut track.scratch);

        let RenderTrack {
            scratch,
            sends,
            strip,
            ..
        } = track;
        deliver_sends(sends, scratch, bus_inputs, true);
        strip.apply_gain_and_pan(scratch);
        strip.apply_mute(scratch);
        deliver_sends(sends, scratch, bus_inputs, false);

        track.peak = track.peak.max(track.scratch.peak());
        // A spectrum display, if one is open on this strip, reads what the strip actually sends
        // to the bus — the chain applied, the fader applied. The check is one relaxed load per
        // block, hoisted out of the loop, so a graph nobody is looking at pays nothing.
        if watching == crate::scope::ScopeSource::Track(index) {
            scope.publish(track.scratch.channel(0), ctx.sample_rate);
        }
        // The output edge's own delay, after both taps so that each copy carries its own: zero in
        // any graph where nothing looks ahead, which is why this is normally a return on the
        // first line.
        track.output_delay.process(&mut track.scratch);
        match track.output.and_then(|slot| bus_inputs.get_mut(slot)) {
            Some(input) => input.mix_from(&track.scratch, 1.0),
            None => master_scratch.mix_from(&track.scratch, 1.0),
        }
    }

    for (effect, enabled) in graph.master.effects.iter_mut().zip(&graph.master.enabled) {
        if *enabled {
            effect.process(master_scratch, &ctx);
        }
    }
    graph.master.apply_gain_and_pan(master_scratch);
    // Mute is the last stage of the strip: the effects still run — so a reverb tail does not
    // freeze and un-muting does not pop — but nothing leaves the bus.
    graph.master.apply_mute(master_scratch);
    // One NaN out of a misbehaving plugin would otherwise reach the output device.
    master_scratch.sanitize();
    if watching == crate::scope::ScopeSource::Master {
        graph
            .scope
            .publish(master_scratch.channel(0), ctx.sample_rate);
    }
    graph.master_peak[0] = graph.master_peak[0].max(master_scratch.channel_peak(0));
    graph.master_peak[1] = graph.master_peak[1].max(master_scratch.channel_peak(1));

    write_segment(out, offset, master_scratch, frames);
}

/// Feeds one tap of a track into the buses its sends name.
///
/// Called twice per track — once before the fader and once after — and each send is only touched
/// by the pass matching its own tap point, so every send's ramp advances exactly once per segment.
fn deliver_sends(
    sends: &mut [RenderSend],
    source: &AudioBuffer,
    bus_inputs: &mut [AudioBuffer],
    pre_fader: bool,
) {
    for send in sends.iter_mut() {
        if send.pre_fader != pre_fader {
            continue;
        }
        let Some(input) = bus_inputs.get_mut(send.target) else {
            continue;
        };
        let (from, to) = send.gain.advance();
        // A send with no delay to apply is mixed straight from the tap. Only one that is actually
        // held back — because the bus it feeds looks ahead — needs a copy to hold.
        if send.delay.frames() == 0 {
            mix_ramped(input, source, from, to);
            continue;
        }
        send.scratch.set_frame_count(source.frame_count());
        send.scratch.copy_from(source);
        send.delay.process(&mut send.scratch);
        mix_ramped(input, &send.scratch, from, to);
    }
}

/// Adds `source` into `out` through a gain sweeping linearly from `from` to `to`.
///
/// The mixing counterpart of the strip's own ramp, and there for the same reason: a send level
/// that jumped between blocks would click.
fn mix_ramped(out: &mut AudioBuffer, source: &AudioBuffer, from: f32, to: f32) {
    let channels = out.channel_count().min(source.channel_count());
    let frames = out.frame_count().min(source.frame_count());
    if frames == 0 {
        return;
    }
    let step = (to - from) / frames as f32;
    let flat = (to - from).abs() <= f32::EPSILON;
    for channel in 0..channels {
        let input = &source.channel(channel)[..frames];
        let destination = &mut out.channel_mut(channel)[..frames];
        if flat {
            for (sample, value) in destination.iter_mut().zip(input) {
                *sample += *value * from;
            }
            continue;
        }
        let mut gain = from;
        for (sample, value) in destination.iter_mut().zip(input) {
            *sample += *value * gain;
            gain += step;
        }
    }
}

/// Fills a track's scratch buffer with whatever it plays for this segment.
fn render_source(
    track: &mut RenderTrack,
    bus_inputs: &[AudioBuffer],
    transport: &Transport,
    ctx: &ProcessContext,
) {
    let RenderTrack {
        source,
        scratch,
        block_events,
        audition,
        continued_from,
        chase_counts,
        chase_velocity,
        ..
    } = track;

    match source {
        RenderSource::Instrument { instrument, events } => {
            block_events.clear();
            if transport.playing {
                let start = transport.position_frames;
                // A block that does not continue where the last one stopped means the playhead
                // jumped, so re-issue the notes that are meant to be sounding here.
                if *continued_from != Some(start) {
                    chase_notes(events, start, chase_counts, chase_velocity, block_events);
                }
                *continued_from = Some(start + ctx.block_frames as u64);
            } else {
                // Resuming from a stop is itself a jump.
                *continued_from = None;
            }
            // Notes played from the piano roll sound even while the transport is stopped. They
            // go in at frame 0, which keeps the list sorted.
            for event in audition.drain(..) {
                block_events.push(event);
            }
            if transport.playing {
                let start = transport.position_frames;
                let end = start + ctx.block_frames as u64;
                // A binary search rather than a cursor, so seeking and looping need no state.
                let first = events.partition_point(|scheduled| scheduled.frame < start);
                for scheduled in &events[first..] {
                    if scheduled.frame >= end {
                        break;
                    }
                    if block_events.len() == block_events.capacity() {
                        break;
                    }
                    let offset = (scheduled.frame - start) as u32;
                    block_events.push(scheduled.event.with_frame(offset));
                }
            }
            instrument.process(block_events, scratch, ctx);
        }
        RenderSource::Audio { clips } => {
            scratch.clear();
            if transport.playing {
                for clip in clips.iter() {
                    mix_clip(clip, scratch, transport.position_frames, ctx.block_frames);
                }
            }
        }
        // Whatever the tracks feeding this bus have already put there. They come first in the
        // routing order, so by now the sum is complete.
        RenderSource::Bus { input } => match bus_inputs.get(*input) {
            Some(mixed) => scratch.copy_from(mixed),
            None => scratch.clear(),
        },
        RenderSource::Silence => scratch.clear(),
    }
}

/// Works out which notes are sounding at `position` and re-triggers them.
///
/// Without this, starting playback — or looping, or exporting a range — anywhere inside a held
/// note would produce silence until the *next* note began, because the note-on that would have
/// started it is in the past. The scan is linear in the number of events before `position`, but
/// it only runs when the playhead jumps, never block to block.
fn chase_notes(
    events: &[ScheduledEvent],
    position: u64,
    counts: &mut [u8; PITCH_COUNT],
    velocity: &mut [f32; PITCH_COUNT],
    out: &mut Vec<NoteEvent>,
) {
    counts.fill(0);
    let upto = events.partition_point(|scheduled| scheduled.frame < position);
    for scheduled in &events[..upto] {
        match scheduled.event {
            NoteEvent::NoteOn {
                pitch,
                velocity: struck,
                ..
            } => {
                if let Some(count) = counts.get_mut(pitch as usize) {
                    *count = count.saturating_add(1);
                    velocity[pitch as usize] = struck;
                }
            }
            NoteEvent::NoteOff { pitch, .. } => {
                if let Some(count) = counts.get_mut(pitch as usize) {
                    *count = count.saturating_sub(1);
                }
            }
            NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. } => counts.fill(0),
            NoteEvent::PitchBend { .. } | NoteEvent::Modulation { .. } => {}
        }
    }

    // Drop whatever the old position left sounding before restoring what belongs to the new one.
    out.push(NoteEvent::AllNotesOff { frame: 0 });
    for (pitch, count) in counts.iter().enumerate() {
        // Once per sounding note rather than once per pitch: two overlapping notes on the same
        // pitch need two voices, or the first of the two offs that follow would silence the
        // pair. The buffer is sized for the densest chord the track ever holds, so the capacity
        // check is a guard rather than something that trims a real arrangement.
        for _ in 0..*count {
            if out.len() == out.capacity() {
                return;
            }
            out.push(NoteEvent::NoteOn {
                frame: 0,
                pitch: pitch as u8,
                velocity: velocity[pitch],
            });
        }
    }
}

/// Adds the part of `clip` that overlaps `[position, position + frames)` into `out`.
fn mix_clip(clip: &RenderAudioClip, out: &mut AudioBuffer, position: u64, frames: usize) {
    let block_end = position + frames as u64;
    let clip_end = clip.start_frame.saturating_add(clip.length);
    if clip_end <= position || clip.start_frame >= block_end {
        return;
    }

    let from = position.max(clip.start_frame);
    let to = block_end.min(clip_end);
    let into_clip = from - clip.start_frame;
    let source_start = (clip.source_offset + into_clip) as usize;
    let available = clip.buffer.frame_count().saturating_sub(source_start);
    let count = ((to - from) as usize).min(available);
    if count == 0 {
        return;
    }
    let destination_start = (from - position) as usize;
    let source_channels = clip.buffer.channel_count();

    for channel in 0..out.channel_count() {
        // A mono source feeds every output channel rather than only the left one.
        let source = &clip.buffer.channel(channel.min(source_channels - 1))
            [source_start..source_start + count];
        let destination =
            &mut out.channel_mut(channel)[destination_start..destination_start + count];
        for (index, (sample, input)) in destination.iter_mut().zip(source).enumerate() {
            *sample += *input * clip.gain * clip.fade_gain(into_clip + index as u64);
        }
    }
}

/// Copies the stereo mix bus into `out` at `offset`, adapting to its channel count.
fn write_segment(out: &mut AudioBuffer, offset: usize, master: &AudioBuffer, frames: usize) {
    if out.channel_count() == 1 {
        let (left, right) = (master.channel(0), master.channel(1));
        let destination = &mut out.channel_mut(0)[offset..offset + frames];
        for (index, sample) in destination.iter_mut().enumerate() {
            *sample = 0.5 * (left[index] + right[index]);
        }
        return;
    }
    // Channels beyond the second stay silent; `out` was cleared before rendering started.
    for channel in 0..out.channel_count().min(master.channel_count()) {
        let source = &master.channel(channel)[..frames];
        out.channel_mut(channel)[offset..offset + frames].copy_from_slice(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::RENDER_CHANNELS;
    use crate::testkit::{self, TONE_AMPLITUDE};
    use auris_core::ParamId;
    use auris_core::automation::AutomationCurve;
    use auris_core::param::{ParamTarget, db_to_gain};
    use auris_core::project::{AudioSourceBank, AuxSend, Note, Output, Project};
    use auris_core::time::Ticks;
    use std::sync::Arc;

    const SAMPLE_RATE: f64 = 48_000.0;

    /// Length of the mute fade at [`SAMPLE_RATE`]: [`crate::graph::MUTE_FADE_MS`] of 48 kHz.
    const FADE_FRAMES: usize = (SAMPLE_RATE * crate::graph::MUTE_FADE_MS / 1_000.0) as usize;

    /// Renders `frames` frames from `transport` in fixed-size blocks into one buffer.
    fn render_range(
        graph: &mut RenderGraph,
        transport: &mut Transport,
        frames: usize,
        block: usize,
    ) -> AudioBuffer {
        let mut out = AudioBuffer::new(RENDER_CHANNELS, frames, SAMPLE_RATE);
        let mut scratch = AudioBuffer::new(RENDER_CHANNELS, block, SAMPLE_RATE);
        let mut written = 0;
        while written < frames {
            let count = block.min(frames - written);
            scratch.set_frame_count(count);
            render_block(graph, transport, &mut scratch, false);
            for channel in 0..RENDER_CHANNELS {
                out.channel_mut(channel)[written..written + count]
                    .copy_from_slice(&scratch.channel(channel)[..count]);
            }
            written += count;
        }
        out
    }

    fn one_note_project(start: Ticks, length: Ticks) -> Project {
        let mut project = Project::new("Render", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        project
            .midi_clip_mut(clip)
            .unwrap()
            .notes
            .push(Note::new(60, start, length));
        project
    }

    fn build(project: &Project, block: usize) -> RenderGraph {
        RenderGraph::build(
            project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            block,
        )
    }

    /// An audio track carrying one panned, faded, trimmed clip — every per-sample multiplier the
    /// clip path applies, so a block-size dependence anywhere in it shows up.
    fn faded_clip_project() -> (Project, AudioSourceBank) {
        let mut project = Project::new("Clip", SAMPLE_RATE);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            8_000,
            SAMPLE_RATE,
            2,
        );
        let clip_id = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        {
            let clip = project.audio_clip_mut(clip_id).unwrap();
            clip.fade_in_frames = 333;
            clip.fade_out_frames = 777;
            clip.gain_db = -2.5;
        }
        project.tracks[0].mixer.pan = -0.37;
        project.master.pan = 0.21;
        project.master.gain_db = -1.5;

        let wave: Vec<f32> = (0..8_000).map(|i| (i as f32 * 0.0001).sin()).collect();
        let mut bank = AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(
                AudioBuffer::from_planar(vec![wave.clone(), wave], SAMPLE_RATE).expect("planar"),
            ),
        );
        (project, bank)
    }

    #[test]
    fn a_note_sounds_exactly_where_it_sits_on_the_timeline() {
        // One quarter note at beat 1, 120 BPM, 48 kHz: frames 24 000 .. 48 000.
        let project = one_note_project(Ticks::QUARTER, Ticks::QUARTER);
        let mut graph = build(&project, 512);
        let mut transport = Transport::playing_from(0);
        let rendered = render_range(&mut graph, &mut transport, 60_000, 512);

        assert_eq!(rendered.slice(0, 24_000).peak(), 0.0);
        for frame in [24_000usize, 30_000, 47_999] {
            let sample = rendered.channel(0)[frame];
            assert!(
                (sample - TONE_AMPLITUDE).abs() < 1e-5,
                "frame {frame} was {sample}, expected {TONE_AMPLITUDE}"
            );
        }
        assert_eq!(rendered.slice(48_000, 12_000).peak(), 0.0);
        assert_eq!(transport.position_frames, 60_000);
    }

    #[test]
    fn the_block_size_does_not_change_a_single_sample() {
        let project = one_note_project(Ticks::from_beats(0.5), Ticks::from_beats(1.5));
        let frames = 40_000;

        let mut coarse = build(&project, 512);
        let a = render_range(&mut coarse, &mut Transport::playing_from(0), frames, 512);
        let mut odd = build(&project, 97);
        let b = render_range(&mut odd, &mut Transport::playing_from(0), frames, 97);
        let mut fine = build(&project, 4_096);
        let c = render_range(&mut fine, &mut Transport::playing_from(0), frames, 4_096);

        assert_eq!(a.channel(0), b.channel(0));
        assert_eq!(a.channel(1), b.channel(1));
        assert_eq!(a.channel(0), c.channel(0));
    }

    #[test]
    fn an_event_on_a_block_boundary_fires_exactly_once() {
        // The tone's amplitude is proportional to the number of held notes, so a doubled
        // note-on would read 2x and a dropped one 0x.
        let project = one_note_project(Ticks::QUARTER, Ticks::QUARTER);
        let mut graph = build(&project, 500);
        // 24 000 and 48 000 are both exact multiples of 500, so both events land on a boundary.
        let rendered = render_range(&mut graph, &mut Transport::playing_from(0), 50_000, 500);

        assert_eq!(rendered.channel(0)[23_999], 0.0);
        assert!((rendered.channel(0)[24_000] - TONE_AMPLITUDE).abs() < 1e-5);
        assert!((rendered.channel(0)[47_999] - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(rendered.channel(0)[48_000], 0.0);
        // A retriggered note would show up as double amplitude anywhere in the note.
        assert!(rendered.slice(24_000, 24_000).peak() < TONE_AMPLITUDE * 1.5);
    }

    #[test]
    fn a_loop_wraps_inside_a_block_without_a_discontinuity() {
        // A ramp source makes every timeline position identifiable from its sample value.
        let mut project = Project::new("Loop", SAMPLE_RATE);
        let track = project.add_audio_track("Ramp");
        let source = project.add_audio_source(
            "ramp",
            auris_core::AssetPath::inside("Audio/ramp.wav"),
            8_000,
            SAMPLE_RATE,
            2,
        );
        project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let ramp: Vec<f32> = (0..8_000).map(|index| index as f32).collect();
        let mut bank = AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(
                AudioBuffer::from_planar(vec![ramp.clone(), ramp], SAMPLE_RATE)
                    .expect("planar ramp"),
            ),
        );
        let registry = testkit::registry();

        // Reference: the first 924 frames of the loop, rendered without any looping.
        let mut reference_graph = RenderGraph::build(&project, &bank, &registry, 1_024);
        let reference = render_range(
            &mut reference_graph,
            &mut Transport::playing_from(0),
            924,
            1_024,
        );

        let mut graph = RenderGraph::build(&project, &bank, &registry, 1_024);
        let mut transport = Transport {
            playing: true,
            position_frames: 900,
            loop_enabled: true,
            loop_start_frames: 0,
            loop_end_frames: 1_000,
        };
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 1_024, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);

        // Frames 0..100 are timeline 900..1000, then the loop wraps to the start.
        assert!((out.channel(0)[0] - 900.0).abs() < 0.01);
        assert!((out.channel(0)[99] - 999.0).abs() < 0.01);
        assert!(
            out.channel(0)[100].abs() < 0.01,
            "the wrap point should be the loop start, got {}",
            out.channel(0)[100]
        );
        // The wrapped remainder must be sample-identical to rendering from the loop start.
        assert_eq!(&out.channel(0)[100..1_024], reference.channel(0));
        assert_eq!(transport.position_frames, 924);
    }

    #[test]
    fn a_muted_track_contributes_nothing_and_solo_silences_the_rest() {
        let mut project = Project::new("Mix", SAMPLE_RATE);
        for name in ["A", "B"] {
            let track = project.add_instrument_track(name, testkit::TONE_ID);
            let clip = project
                .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::from_beats(4.0))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                60,
                Ticks::ZERO,
                Ticks::from_beats(4.0),
            ));
        }

        let mut both = build(&project, 512);
        let mixed = render_range(&mut both, &mut Transport::playing_from(0), 4_096, 512);
        assert!((mixed.channel(0)[100] - 2.0 * TONE_AMPLITUDE).abs() < 1e-5);

        let mut muted_project = project.clone();
        muted_project.tracks[1].mixer.mute = true;
        let mut muted = build(&muted_project, 512);
        let single = render_range(&mut muted, &mut Transport::playing_from(0), 4_096, 512);
        assert!((single.channel(0)[100] - TONE_AMPLITUDE).abs() < 1e-5);

        let mut solo_project = project.clone();
        solo_project.tracks[0].mixer.solo = true;
        let mut solo = build(&solo_project, 512);
        let soloed = render_range(&mut solo, &mut Transport::playing_from(0), 4_096, 512);
        assert!((soloed.channel(0)[100] - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(soloed.channel(0)[100], single.channel(0)[100]);
    }

    #[test]
    fn a_mute_toggled_by_command_takes_effect_without_a_rebuild() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        graph.set_track_mute(0, true);
        // The switch fades rather than steps, so measure once the fade has run its length.
        let silenced = render_range(&mut graph, &mut Transport::playing_from(0), 2_048, 512);
        assert_eq!(silenced.slice(FADE_FRAMES, 2_048 - FADE_FRAMES).peak(), 0.0);

        graph.set_track_mute(0, false);
        let audible = render_range(&mut graph, &mut Transport::playing_from(2_048), 2_048, 512);
        let settled = audible.slice(FADE_FRAMES, 2_048 - FADE_FRAMES).peak();
        assert!((settled - TONE_AMPLITUDE).abs() < 1e-5, "got {settled}");
    }

    #[test]
    fn a_mute_fades_out_instead_of_stepping_to_silence() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);

        graph.set_track_mute(0, true);
        render_block(&mut graph, &mut transport, &mut out, false);
        // Still at full level where the fade starts, silent once it has run, and nowhere in
        // between does one sample differ from the next by more than a single fade increment —
        // which is the whole of the click this replaces.
        assert!((out.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(out.slice(FADE_FRAMES, 512 - FADE_FRAMES).peak(), 0.0);
        let biggest = out
            .channel(0)
            .windows(2)
            .map(|pair| (pair[0] - pair[1]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            biggest <= TONE_AMPLITUDE / FADE_FRAMES as f32 + 1e-6,
            "the mute stepped by {biggest}"
        );
    }

    #[test]
    fn a_mute_fade_lands_on_the_same_samples_at_any_block_size() {
        // The fade is counted in frames, not in blocks, so a small buffer must not stretch or
        // shorten it. This is the same guarantee every other ramp in the renderer keeps.
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let render = |block: usize| {
            let mut graph = build(&project, block);
            graph.set_track_mute(0, true);
            render_range(&mut graph, &mut Transport::playing_from(0), 2_048, block)
        };
        assert_eq!(render(512).channel(0), render(64).channel(0));
        assert_eq!(render(512).channel(0), render(1_000).channel(0));
    }

    #[test]
    fn a_look_ahead_effect_does_not_drag_its_track_behind_the_others() {
        // Two tracks playing the same note, one of them through an effect that hands its audio
        // back late. Without compensation the plain track would arrive first and there would be
        // a window where only one of the two is sounding; with it, they start together.
        let mut project = Project::new("PDC", SAMPLE_RATE);
        for name in ["Late", "Plain"] {
            let track = project.add_instrument_track(name, testkit::TONE_ID);
            let clip = project
                .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(4.0))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                60,
                Ticks::ZERO,
                Ticks::from_beats(4.0),
            ));
        }
        let late = project.tracks[0].id;
        project.add_effect(Some(late), testkit::LOOKAHEAD_ID);

        let mut graph = build(&project, 512);
        let rendered = render_range(&mut graph, &mut Transport::playing_from(0), 2_048, 512);

        let lead_in = testkit::LOOKAHEAD_FRAMES;
        assert_eq!(rendered.slice(0, lead_in).peak(), 0.0);
        for frame in [lead_in, lead_in + 1, 1_000, 2_047] {
            let sample = rendered.channel(0)[frame];
            assert!(
                (sample - 2.0 * TONE_AMPLITUDE).abs() < 1e-5,
                "frame {frame} was {sample}: the two tracks are not in step"
            );
        }
    }

    #[test]
    fn compensation_holds_at_any_block_size() {
        let mut project = Project::new("PDC", SAMPLE_RATE);
        for name in ["Late", "Plain"] {
            let track = project.add_instrument_track(name, testkit::TONE_ID);
            let clip = project
                .add_midi_clip(track, "c", Ticks::from_beats(1.0), Ticks::from_beats(4.0))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                60,
                Ticks::ZERO,
                Ticks::from_beats(2.0),
            ));
        }
        let late = project.tracks[0].id;
        project.add_effect(Some(late), testkit::LOOKAHEAD_ID);

        let render = |block: usize| {
            let mut graph = build(&project, block);
            render_range(&mut graph, &mut Transport::playing_from(0), 40_000, block)
        };
        assert_eq!(render(512).channel(0), render(97).channel(0));
        assert_eq!(render(512).channel(0), render(4_096).channel(0));
    }

    #[test]
    fn a_master_fader_at_minus_six_decibels_halves_the_peak() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));

        let mut unity = build(&project, 512);
        let full = render_range(&mut unity, &mut Transport::playing_from(0), 4_096, 512);

        let mut quiet = build(&project, 512);
        // -6.0206 dB is exactly a factor of two in amplitude.
        quiet.set_master_gain_db(-6.020_6);
        let halved = render_range(&mut quiet, &mut Transport::playing_from(0), 4_096, 512);

        assert!((db_to_gain(-6.020_6) - 0.5).abs() < 1e-4);
        assert!((full.peak() - TONE_AMPLITUDE).abs() < 1e-5);
        // The first block is the fader ramp, so measure once the move has settled.
        let settled = halved.slice(512, 3_584).peak();
        assert!(
            (settled - full.peak() * 0.5).abs() < 1e-5,
            "expected {} got {settled}",
            full.peak() * 0.5
        );
    }

    #[test]
    fn a_fader_move_ramps_instead_of_stepping() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);

        graph.set_master_gain_db(-120.0);
        render_block(&mut graph, &mut transport, &mut out, false);
        // The first sample of the ramp still carries the old gain and the last is near silence.
        assert!((out.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5);
        assert!(out.channel(0)[511].abs() < TONE_AMPLITUDE * 0.02);
    }

    #[test]
    fn effects_keep_running_while_the_transport_is_stopped() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.add_effect(None, testkit::TAIL_ID);
        let mut graph = build(&project, 512);

        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);
        assert!(out.peak() > 0.0);

        transport.playing = false;
        graph.reset_voices();
        render_block(&mut graph, &mut transport, &mut out, false);
        // The source is silent but the tail effect's state keeps feeding the bus.
        assert!(
            out.peak() > 0.0,
            "a stopped transport must still let tails ring out"
        );
        assert_eq!(transport.position_frames, 512);
    }

    /// A transport stopped one frame short of the loop end — what pressing Stop while looping
    /// leaves behind.
    fn stopped_at_the_loop_end() -> Transport {
        Transport {
            playing: false,
            position_frames: 4_999,
            loop_enabled: true,
            loop_start_frames: 1_000,
            loop_end_frames: 5_000,
        }
    }

    #[test]
    fn a_stopped_transport_at_the_loop_end_renders_one_segment_per_callback() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.add_effect(None, testkit::COUNTER_ID);
        let mut graph = build(&project, 512);
        let mut transport = stopped_at_the_loop_end();
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);

        testkit::take_process_calls();
        render_block(&mut graph, &mut transport, &mut out, false);
        assert_eq!(
            testkit::take_process_calls(),
            1,
            "a stopped playhead cannot reach the loop end, so the block must not be split"
        );
        assert_eq!(transport.position_frames, 4_999);
    }

    #[test]
    fn a_stopped_transport_renders_the_same_whether_or_not_a_loop_is_enabled() {
        // A tail effect keeps the bus ringing after the stop, and a fader move on top makes the
        // per-segment gain ramp observable: splitting would settle the ramp on the first frame.
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.add_effect(None, testkit::TAIL_ID);

        let render = |loop_enabled: bool| {
            let mut graph = build(&project, 512);
            let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
            // One rolling block first, so the tail has something to ring with.
            render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);

            let mut transport = stopped_at_the_loop_end();
            transport.loop_enabled = loop_enabled;
            graph.set_master_gain_db(-60.0);
            render_block(&mut graph, &mut transport, &mut out, false);
            out
        };

        let looping = render(true);
        let plain = render(false);
        // The fader move must still be mid-ramp halfway through the block; a split settles it on
        // the first frame, which is the click this whole segment rule exists to avoid.
        let middle = looping.channel(0)[256].abs();
        assert!(
            middle > 0.1,
            "the fader move stepped instead of ramping: {middle}"
        );
        assert_eq!(looping.channel(0), plain.channel(0));
    }

    #[test]
    fn a_stopped_transport_still_plays_auditioned_notes() {
        let project = one_note_project(Ticks::from_beats(100.0), Ticks::QUARTER);
        let mut graph = build(&project, 512);
        graph.note_on(0, 60, 1.0);

        let mut transport = Transport::new();
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);
        assert!((out.peak() - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(transport.position_frames, 0);
    }

    #[test]
    fn an_audio_clip_lands_on_its_timeline_position_with_its_fades() {
        let mut project = Project::new("Audio", SAMPLE_RATE);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "ones",
            auris_core::AssetPath::inside("Audio/ones.wav"),
            1_000,
            SAMPLE_RATE,
            1,
        );
        let clip_id = project
            .add_audio_clip(track, source, Ticks::from_beats(1.0))
            .unwrap();
        {
            let clip = project.audio_clip_mut(clip_id).unwrap();
            clip.fade_in_frames = 100;
            clip.fade_out_frames = 100;
        }
        let mut bank = AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(
                AudioBuffer::from_planar(vec![vec![1.0; 1_000]], SAMPLE_RATE).expect("mono source"),
            ),
        );

        let mut graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let rendered = render_range(&mut graph, &mut Transport::playing_from(0), 26_000, 512);

        // The clip starts one beat in: 24 000 frames.
        assert_eq!(rendered.channel(0)[23_999], 0.0);
        assert_eq!(rendered.channel(0)[24_000], 0.0); // fade-in starts at zero
        assert!((rendered.channel(0)[24_050] - 0.5).abs() < 1e-5);
        assert!((rendered.channel(0)[24_500] - 1.0).abs() < 1e-5);
        // A mono source must reach both channels.
        assert!((rendered.channel(1)[24_500] - 1.0).abs() < 1e-5);
        assert!(rendered.channel(0)[24_999].abs() < 0.02);
        assert_eq!(rendered.channel(0)[25_000], 0.0);
    }

    #[test]
    fn a_mono_output_buffer_gets_the_downmix() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        let mut out = AudioBuffer::new(1, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        assert_eq!(out.channel_count(), 1);
        assert!((out.channel(0)[10] - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn an_output_longer_than_the_prepared_block_is_split_internally() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 128);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 5_000, SAMPLE_RATE);
        let mut transport = Transport::playing_from(0);
        render_block(&mut graph, &mut transport, &mut out, false);
        assert_eq!(transport.position_frames, 5_000);
        assert!((out.channel(0)[4_999] - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn starting_playback_inside_a_held_note_chases_it_back() {
        // The note runs from frame 0 to 96 000; playback starts a long way into it.
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        let rendered = render_range(&mut graph, &mut Transport::playing_from(50_000), 1_024, 512);
        assert!(
            (rendered.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5,
            "a note already sounding at the start position must be chased in"
        );
    }

    #[test]
    fn a_note_spanning_the_loop_start_retriggers_on_every_wrap() {
        // Note over beats 0..2; the loop covers frames 12 000..30 000, so its start sits inside
        // the note and its end sits after the note has finished.
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(2.0));
        let mut graph = build(&project, 1_024);
        let mut transport = Transport {
            playing: true,
            position_frames: 47_000,
            loop_enabled: true,
            loop_start_frames: 12_000,
            loop_end_frames: 48_000,
        };
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 2_048, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);

        // Frames 0..1000 are timeline 47 000..48 000, past the note's end at 48 000... the note
        // ends exactly there, so it is still sounding, then the loop wraps back inside it.
        assert!((out.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5);
        assert!(
            (out.channel(0)[1_000] - TONE_AMPLITUDE).abs() < 1e-5,
            "the wrapped block landed inside the note, so it must still sound"
        );
        assert_eq!(transport.position_frames, 13_048);
    }

    /// A pedal note over beats 0..4 with a second strike of the same pitch over beats 1..3.
    fn overlapping_same_pitch_project() -> Project {
        let mut project = Project::new("Overlap", SAMPLE_RATE);
        let track = project.add_instrument_track("Pedal", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes
            .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(4.0)));
        midi.notes.push(Note::new(
            60,
            Ticks::from_beats(1.0),
            Ticks::from_beats(2.0),
        ));
        project
    }

    #[test]
    fn seeking_into_overlapping_notes_of_one_pitch_brings_all_of_them_back() {
        // The tone's level counts held notes, so a chase that re-issued one note-on for a pitch
        // sounding twice reads half as loud — and the first release that followed would then take
        // the pedal with it.
        let project = overlapping_same_pitch_project();
        let mut graph = build(&project, 512);
        // Frame 48 000 is beat 2: inside both notes.
        let rendered = render_range(
            &mut graph,
            &mut Transport::playing_from(48_000),
            60_000,
            512,
        );

        assert!(
            (rendered.channel(0)[0] - 2.0 * TONE_AMPLITUDE).abs() < 1e-5,
            "both notes should be chased back, got {}",
            rendered.channel(0)[0]
        );
        // The inner note ends at beat 3 (frame 72 000) and the pedal at beat 4 (frame 96 000).
        assert!((rendered.channel(0)[24_100] - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(rendered.channel(0)[48_100], 0.0);
    }

    #[test]
    fn seeking_into_stacked_notes_matches_playing_through_them() {
        let project = overlapping_same_pitch_project();
        let mut straight = build(&project, 512);
        let played = render_range(&mut straight, &mut Transport::playing_from(0), 108_000, 512);

        let mut seeked = build(&project, 512);
        let jumped = render_range(
            &mut seeked,
            &mut Transport::playing_from(48_000),
            60_000,
            512,
        );
        assert_eq!(&played.channel(0)[48_000..], jumped.channel(0));
    }

    #[test]
    fn a_jump_past_the_end_of_a_note_leaves_silence() {
        let project = one_note_project(Ticks::ZERO, Ticks::QUARTER);
        let mut graph = build(&project, 512);
        // The note ends at 24 000; start well past it.
        let rendered = render_range(&mut graph, &mut Transport::playing_from(30_000), 1_024, 512);
        assert_eq!(rendered.peak(), 0.0);
    }

    #[test]
    fn a_muted_master_bus_reaches_neither_the_output_nor_the_meters() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.master.mute = true;
        let mut graph = build(&project, 512);
        assert!(!graph.master().is_active());

        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        assert_eq!(out.peak(), 0.0);
        assert_eq!(graph.master_peak(), 0.0);
        // The track itself still played; only the bus output is gated.
        assert!((graph.track_peak(0) - TONE_AMPLITUDE).abs() < 1e-5);

        graph.master_mut().set_mute(false);
        render_block(
            &mut graph,
            &mut Transport::playing_from(512),
            &mut out,
            false,
        );
        assert!((out.peak() - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn a_mono_output_averages_a_panned_master_rather_than_dropping_a_side() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.master.pan = 1.0;
        let mut graph = build(&project, 512);
        let mut out = AudioBuffer::new(1, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        // Hard right puts everything on the right at sqrt(2) gain; the downmix halves the sum.
        let expected = 0.5 * std::f32::consts::SQRT_2 * TONE_AMPLITUDE;
        assert!(
            (out.channel(0)[400] - expected).abs() < 1e-5,
            "got {}",
            out.channel(0)[400]
        );
    }

    #[test]
    fn a_buffer_with_more_than_two_channels_leaves_the_extras_silent() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 128);
        // 900 frames through a 128-frame graph also exercises the segment split.
        let mut out = AudioBuffer::new(6, 900, SAMPLE_RATE);
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        for channel in 0..2 {
            assert!((out.channel(channel)[899] - TONE_AMPLITUDE).abs() < 1e-5);
        }
        for channel in 2..6 {
            assert_eq!(out.channel_peak(channel), 0.0, "channel {channel}");
        }
    }

    #[test]
    fn a_dense_chord_is_delivered_whole() {
        let mut project = Project::new("Dense", SAMPLE_RATE);
        let track = project.add_instrument_track("Chords", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Stack", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        for pitch in 0..128u8 {
            midi.notes
                .push(Note::new(pitch, Ticks::ZERO, Ticks::from_beats(2.0)));
        }
        let mut graph = build(&project, 512);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        // The tone scales with the number of held notes, so a dropped event is visible.
        assert!(
            (out.channel(0)[0] - 128.0 * TONE_AMPLITUDE).abs() < 1e-3,
            "got {}",
            out.channel(0)[0]
        );
    }

    #[test]
    fn an_audio_clip_renders_identically_at_any_block_size() {
        let (project, bank) = faded_clip_project();
        let registry = testkit::registry();
        let mut coarse = RenderGraph::build(&project, &bank, &registry, 512);
        let a = render_range(&mut coarse, &mut Transport::playing_from(0), 8_000, 512);
        let mut odd = RenderGraph::build(&project, &bank, &registry, 61);
        let b = render_range(&mut odd, &mut Transport::playing_from(0), 8_000, 61);
        assert_eq!(a.channel(0), b.channel(0));
        assert_eq!(a.channel(1), b.channel(1));
    }

    #[test]
    fn a_loop_starting_away_from_zero_wraps_sample_continuously() {
        let (project, bank) = faded_clip_project();
        let registry = testkit::registry();
        let mut reference = RenderGraph::build(&project, &bank, &registry, 1_024);
        let expected = render_range(
            &mut reference,
            &mut Transport::playing_from(2_000),
            800,
            1_024,
        );

        let mut graph = RenderGraph::build(&project, &bank, &registry, 1_024);
        let mut transport = Transport {
            playing: true,
            position_frames: 4_800,
            loop_enabled: true,
            loop_start_frames: 2_000,
            loop_end_frames: 5_000,
        };
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 1_000, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);
        assert_eq!(transport.position_frames, 2_800);
        assert_eq!(&out.channel(0)[200..1_000], expected.channel(0));
        assert_eq!(&out.channel(1)[200..1_000], expected.channel(1));
    }

    #[test]
    fn a_one_frame_loop_neither_hangs_nor_skips() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        let mut transport = Transport {
            playing: true,
            position_frames: 1_000,
            loop_enabled: true,
            loop_start_frames: 1_000,
            loop_end_frames: 1_001,
        };
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 64, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);
        assert_eq!(transport.position_frames, 1_000);
        for sample in out.channel(0) {
            assert!((sample - TONE_AMPLITUDE).abs() < 1e-5, "got {sample}");
        }
    }

    #[test]
    fn a_missing_effect_does_not_misaddress_the_gains_after_it() {
        // Master chain: an id the registry has never heard of, then two gains. Both gain
        // commands must land on the gains, so the tone comes out multiplied by 2 * 3.
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.add_effect(None, "vendor.gone");
        project.add_effect(None, testkit::GAIN_ID);
        project.add_effect(None, testkit::GAIN_ID);

        let mut graph = build(&project, 512);
        graph.set_effect_param(None, 1, ParamId(0), 2.0);
        graph.set_effect_param(None, 2, ParamId(0), 3.0);

        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        assert!(
            (out.channel(0)[100] - 6.0 * TONE_AMPLITUDE).abs() < 1e-5,
            "got {}",
            out.channel(0)[100]
        );
    }

    #[test]
    fn one_long_call_matches_many_short_ones_through_stateful_effects() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(8.0));
        let track_id = project.tracks[0].id;
        project.add_effect(Some(track_id), testkit::TAIL_ID);
        project.add_effect(None, testkit::TAIL_ID);

        let mut long_graph = build(&project, 128);
        let mut long_out = AudioBuffer::new(RENDER_CHANNELS, 5_000, SAMPLE_RATE);
        render_block(
            &mut long_graph,
            &mut Transport::playing_from(0),
            &mut long_out,
            false,
        );

        let mut short_graph = build(&project, 128);
        let short_out = render_range(
            &mut short_graph,
            &mut Transport::playing_from(0),
            5_000,
            128,
        );
        assert_eq!(long_out.channel(0), short_out.channel(0));
        assert_eq!(long_out.channel(1), short_out.channel(1));
    }

    /// A held note under a fader automated from `from` dB at tick 0 to `to` dB at `over`.
    fn faded_project(from: f32, to: f32, over: Ticks) -> Project {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(8.0));
        let track = project.tracks[0].id;
        let target = ParamTarget::TrackGain(track);
        project
            .automation
            .set_point(target, AutomationCurve::Linear, Ticks::ZERO, from);
        project
            .automation
            .set_point(target, AutomationCurve::Linear, over, to);
        project
    }

    /// Peak of the loudest channel over a rendered range.
    fn peak(buffer: &AudioBuffer, from: usize, to: usize) -> f32 {
        (0..RENDER_CHANNELS)
            .flat_map(|channel| buffer.channel(channel)[from..to].iter())
            .fold(0.0f32, |most, sample| most.max(sample.abs()))
    }

    #[test]
    fn an_automated_fader_is_at_its_written_value_where_the_lane_says() {
        // The numbers, not "it moved". The lane is written in decibels and climbs linearly, so
        // what comes out at each fraction of the way along is a value this test can name in
        // advance rather than compare against itself.
        let project = faded_project(-60.0, 0.0, Ticks::from_beats(4.0));
        let mut graph = build(&project, 64);
        // Four beats at 120 bpm is two seconds.
        let frames = (SAMPLE_RATE * 2.0) as usize;
        let out = render_range(&mut graph, &mut Transport::playing_from(0), frames, 64);

        // Narrow, because the lane sweeps 60 dB across the render: a window a thousandth of the
        // way along still climbs 0.06 dB, and the peak inside it lands at its end.
        const WINDOW: usize = 256;
        for fraction in [0.25, 0.5, 0.75] {
            let at = (frames as f32 * fraction) as usize;
            let heard = peak(&out, at, at + WINDOW);
            let expected = TONE_AMPLITUDE * db_to_gain(-60.0 + 60.0 * fraction);
            assert!(
                (heard - expected).abs() < expected * 0.1,
                "{fraction} of the way along the lane reads {heard}, expected about {expected}"
            );
        }
    }

    #[test]
    fn arriving_in_the_middle_of_a_fade_does_not_swell_into_it() {
        // A seek is not a fader move. Landing at the half way point of a 60 dB climb used to slide
        // up from wherever the strip had been left — at its stored 0 dB — which is a swell nobody
        // wrote, at the loudest possible moment.
        let project = faded_project(-60.0, 0.0, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        let half = (SAMPLE_RATE) as u64;
        let out = render_range(&mut graph, &mut Transport::playing_from(half), 512, 512);
        let expected = TONE_AMPLITUDE * db_to_gain(-30.0);
        let heard = peak(&out, 0, 512);
        assert!(
            heard < expected * 1.5,
            "the first block after a seek peaked at {heard}, four times {expected} would be the \
             old ramp from 0 dB"
        );
    }

    #[test]
    fn a_lane_holds_its_last_value_past_its_end() {
        // The lane's own rule, heard rather than asserted on the map: past the last point the
        // fader stays where it was left instead of sliding back to the strip's stored value.
        let project = faded_project(-60.0, 0.0, Ticks::from_beats(1.0));
        let mut graph = build(&project, 64);
        let frames = (SAMPLE_RATE * 2.0) as usize;
        let out = render_range(&mut graph, &mut Transport::playing_from(0), frames, 64);
        let tail = peak(&out, frames - 4_800, frames);
        assert!(
            (tail - TONE_AMPLITUDE).abs() < TONE_AMPLITUDE * 0.05,
            "past the lane's end the fader should sit at 0 dB, peaked at {tail}"
        );
    }

    #[test]
    fn a_lane_is_resolved_to_the_block_size_and_no_finer() {
        // Everything else in this renderer lands on the same samples at any block size, and
        // automation is the one thing that does not: a lane is read once per segment and the
        // strip ramps to it across that segment, so a coarser block is a coarser staircase.
        //
        // The difference is bounded rather than absent, and this says by how much: a 40 dB climb
        // over four beats, rendered at 512 frames against 64, stays within a per-cent of full
        // scale. What is worth knowing is that it is small and that it is *here* rather than
        // somewhere nobody looked — a per-sample lane read would remove it, at the cost of
        // reading every lane 48 000 times a second to move a fader that is ramped anyway.
        let project = faded_project(-40.0, 0.0, Ticks::from_beats(4.0));
        let render = |block: usize| {
            let mut graph = build(&project, block);
            render_range(&mut graph, &mut Transport::playing_from(0), 8_192, block)
        };
        let coarse = render(512);
        let fine = render(64);
        let worst = coarse
            .channel(0)
            .iter()
            .zip(fine.channel(0).iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "block size moved a sample by {worst}");
    }

    #[test]
    fn a_project_nobody_automated_carries_no_lanes_at_all() {
        // The cost of the feature when it is not in use, asserted rather than assumed: the
        // per-segment call returns on its first line because there is nothing to walk.
        let graph = build(&one_note_project(Ticks::ZERO, Ticks::from_beats(4.0)), 512);
        assert_eq!(graph.automated_count(), 0);
    }

    #[test]
    fn a_lane_pointing_at_a_track_that_is_gone_drives_nothing() {
        // It should not be reachable — the document drops a lane with its track — so this is the
        // second lock: what a wrongly resolved lane would do is drive whatever now sits at that
        // position, which is worse than silence.
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        project.automation.set_point(
            ParamTarget::TrackGain(auris_core::TrackId(9_999)),
            AutomationCurve::Linear,
            Ticks::ZERO,
            -6.0,
        );
        assert_eq!(build(&project, 512).automated_count(), 0);
    }

    /// Two tone tracks holding a note throughout, both routed into one bus.
    ///
    /// The bus is last in the track list, so a renderer that walked the list instead of the
    /// routing order would still get this right — which is why the tests below that care about
    /// the order put the bus first.
    fn bussed_project() -> Project {
        let mut project = Project::new("Bus", SAMPLE_RATE);
        for name in ["A", "B"] {
            let track = project.add_instrument_track(name, testkit::TONE_ID);
            let clip = project
                .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                60,
                Ticks::ZERO,
                Ticks::from_beats(8.0),
            ));
        }
        let bus = project.add_bus_track("Drums");
        project.tracks[0].output = Output::Bus(bus);
        project.tracks[1].output = Output::Bus(bus);
        project
    }

    /// One tone track sending to one bus, with the bus placed *before* it in the track list.
    ///
    /// The order the tracks are stored in is the user's; the order they are mixed in is the
    /// routing's. Putting the bus first is what makes the difference between the two observable.
    fn sent_project(level_db: f32, pre_fader: bool) -> Project {
        let mut project = Project::new("Send", SAMPLE_RATE);
        let bus = project.add_bus_track("Reverb");
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(8.0),
        ));
        let id = project.next_send_id();
        project.track_mut(track).unwrap().sends.push(AuxSend {
            id,
            target: bus,
            level_db,
            pre_fader,
        });
        project
    }

    #[test]
    fn a_bus_sums_what_is_routed_to_it_and_its_fader_moves_all_of_it() {
        let plain = build(&bussed_project(), 512);
        let mut plain = plain;
        let summed = render_range(&mut plain, &mut Transport::playing_from(0), 4_096, 512);
        assert!((peak(&summed, 0, 4_096) - 2.0 * TONE_AMPLITUDE).abs() < 1e-5);

        // -6.0206 dB is exactly a factor of two in amplitude, and one fader on the bus has to
        // move both tracks — which is the whole reason a bus exists.
        let mut project = bussed_project();
        project.tracks[2].mixer.gain_db = -6.020_6;
        let mut graph = build(&project, 512);
        let halved = render_range(&mut graph, &mut Transport::playing_from(0), 4_096, 512);
        let heard = peak(&halved, 0, 4_096);
        assert!((heard - TONE_AMPLITUDE).abs() < 1e-4, "got {heard}");
    }

    #[test]
    fn a_bus_chain_runs_once_on_the_sum_rather_than_on_each_track() {
        let mut project = bussed_project();
        let bus = project.tracks[2].id;
        project.add_effect(Some(bus), testkit::COUNTER_ID);
        let mut graph = build(&project, 512);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);

        testkit::take_process_calls();
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        assert_eq!(
            testkit::take_process_calls(),
            1,
            "two tracks share one bus chain, so it must run once"
        );
        assert!((out.channel(0)[100] - 2.0 * TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn a_send_feeds_a_bus_without_taking_the_track_off_the_master() {
        // The difference between a send and an output: the track is still heard dry.
        let mut graph = build(&sent_project(0.0, false), 512);
        let out = render_range(&mut graph, &mut Transport::playing_from(0), 4_096, 512);
        let heard = peak(&out, 0, 4_096);
        assert!(
            (heard - 2.0 * TONE_AMPLITUDE).abs() < 1e-5,
            "dry plus a unity send should be twice the tone, got {heard}"
        );

        // And the send has a level of its own: -6.0206 dB is half.
        let mut graph = build(&sent_project(-6.020_6, false), 512);
        let out = render_range(&mut graph, &mut Transport::playing_from(0), 4_096, 512);
        let heard = peak(&out, 0, 4_096);
        assert!((heard - 1.5 * TONE_AMPLITUDE).abs() < 1e-4, "got {heard}");
    }

    #[test]
    fn a_pre_fader_send_ignores_the_fader_and_a_post_fader_one_follows_it() {
        let heard = |pre_fader: bool| {
            let mut project = sent_project(0.0, pre_fader);
            // The track itself pulled all the way down, so the only thing that can still be
            // heard is whatever the send took a copy of.
            project.tracks[1].mixer.gain_db = -120.0;
            let mut graph = build(&project, 512);
            let out = render_range(&mut graph, &mut Transport::playing_from(0), 4_096, 512);
            peak(&out, 0, 4_096)
        };
        let before = heard(true);
        assert!(
            (before - TONE_AMPLITUDE).abs() < 1e-5,
            "a pre-fader send is taken before the fader, so it survives it: got {before}"
        );
        let after = heard(false);
        assert!(
            after < 1e-5,
            "a post-fader send follows the fader down to silence: got {after}"
        );
    }

    #[test]
    fn soloing_a_track_leaves_the_bus_it_feeds_audible() {
        // Solo travels along the routing. Without that, soloing a track routed through a bus
        // silences the bus it has to pass through, and the solo produces nothing at all.
        let mut project = bussed_project();
        project.tracks[0].mixer.solo = true;
        let mut graph = build(&project, 512);
        let out = render_range(&mut graph, &mut Transport::playing_from(0), 4_096, 512);
        let heard = peak(&out, 0, 4_096);
        assert!(
            (heard - TONE_AMPLITUDE).abs() < 1e-5,
            "the soloed track was silenced by its own bus: got {heard}"
        );
    }

    #[test]
    fn a_bus_that_looks_ahead_does_not_drag_what_goes_through_it_behind_the_rest() {
        // The bus's own latency belongs to the *path*, not to the chain, so the track that does
        // not pass through it is the one that has to be held back. Per-track compensation could
        // not express that at all: neither track has a look-ahead effect on it.
        let mut project = Project::new("PDC", SAMPLE_RATE);
        for name in ["Bussed", "Plain"] {
            let track = project.add_instrument_track(name, testkit::TONE_ID);
            let clip = project
                .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
                60,
                Ticks::ZERO,
                Ticks::from_beats(8.0),
            ));
        }
        let bus = project.add_bus_track("Late");
        project.tracks[0].output = Output::Bus(bus);
        project.add_effect(Some(bus), testkit::LOOKAHEAD_ID);

        let mut graph = build(&project, 512);
        assert_eq!(graph.latency_frames(), testkit::LOOKAHEAD_FRAMES);
        let out = render_range(&mut graph, &mut Transport::playing_from(0), 2_048, 512);

        let lead_in = testkit::LOOKAHEAD_FRAMES;
        assert_eq!(out.slice(0, lead_in).peak(), 0.0);
        for frame in [lead_in, lead_in + 1, 1_000, 2_047] {
            let sample = out.channel(0)[frame];
            assert!(
                (sample - 2.0 * TONE_AMPLITUDE).abs() < 1e-5,
                "frame {frame} was {sample}: the two tracks are not in step"
            );
        }
    }

    #[test]
    fn a_send_into_a_bus_that_looks_ahead_stays_in_step_with_the_dry_signal() {
        // One track, two paths of different lengths: dry to the master and a copy through a bus
        // that hands its audio back late. Only a delay on each *edge* can make both arrive
        // together — with one delay for the whole track they land 64 frames apart and comb-filter
        // each other, which is a sound nobody asked for and no fader can undo.
        let mut project = sent_project(0.0, false);
        let bus = project.tracks[0].id;
        project.add_effect(Some(bus), testkit::LOOKAHEAD_ID);

        let mut graph = build(&project, 512);
        assert_eq!(graph.latency_frames(), testkit::LOOKAHEAD_FRAMES);
        let out = render_range(&mut graph, &mut Transport::playing_from(0), 2_048, 512);

        let lead_in = testkit::LOOKAHEAD_FRAMES;
        assert_eq!(
            out.slice(0, lead_in).peak(),
            0.0,
            "the dry path has to wait for the wet one"
        );
        for frame in [lead_in, 1_000, 2_047] {
            let sample = out.channel(0)[frame];
            assert!(
                (sample - 2.0 * TONE_AMPLITUDE).abs() < 1e-5,
                "frame {frame} was {sample}: dry and wet are not in step"
            );
        }
    }

    #[test]
    fn a_tail_adds_up_along_the_routing() {
        // Three ringing effects, one on a track, one on the bus it feeds and one on the master.
        // They run in series, so an export has to keep going for all three end to end.
        let mut project = bussed_project();
        let track = project.tracks[0].id;
        let bus = project.tracks[2].id;
        project.add_effect(Some(track), testkit::TAIL_ID);
        project.add_effect(Some(bus), testkit::TAIL_ID);
        project.add_effect(None, testkit::TAIL_ID);

        let graph = build(&project, 512);
        assert_eq!(graph.tail_frames(), 3 * testkit::TAIL_FRAMES);
    }

    #[test]
    fn the_block_size_does_not_change_a_routed_mix() {
        // The guarantee every other path in this renderer keeps, kept by the routing too: a bus
        // with a chain and a pan, a pre-fader send at a level of its own, and a stateful effect
        // in the middle of it.
        let mut project = bussed_project();
        let bus = project.tracks[2].id;
        project.tracks[2].mixer.pan = 0.3;
        project.add_effect(Some(bus), testkit::TAIL_ID);
        let id = project.next_send_id();
        project.tracks[0].sends.push(AuxSend {
            id,
            target: bus,
            level_db: -4.0,
            pre_fader: true,
        });

        let render = |block: usize| {
            let mut graph = build(&project, block);
            render_range(&mut graph, &mut Transport::playing_from(0), 20_000, block)
        };
        assert_eq!(render(512).channel(0), render(97).channel(0));
        assert_eq!(render(512).channel(1), render(4_096).channel(1));
    }

    #[test]
    fn a_routed_graph_never_allocates_either() {
        // The routing's own hot path, under the same rule as everything else on the audio thread:
        // bus inputs to clear and sum, a pre-fader send, and a send whose copy has to be held back
        // — the one case that touches a buffer of its own.
        let mut project = bussed_project();
        let bus = project.tracks[2].id;
        project.add_effect(Some(bus), testkit::LOOKAHEAD_ID);
        for (level_db, pre_fader) in [(-3.0, true), (-9.0, false)] {
            let id = project.next_send_id();
            project.tracks[1].sends.push(AuxSend {
                id,
                target: bus,
                level_db,
                pre_fader,
            });
        }

        let mut graph = build(&project, 512);
        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        // Warm up outside the watched region so first-touch growth is not counted.
        render_block(&mut graph, &mut transport, &mut out, false);

        let allocations = testkit::count_allocations(|| {
            for step in 0..100 {
                graph.set_send_level_db(1, 0, -3.0 - step as f32 * 0.1);
                render_block(&mut graph, &mut transport, &mut out, false);
            }
        });
        assert_eq!(allocations, 0, "the routing allocated on the audio thread");
    }

    #[test]
    fn render_block_never_allocates() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(8.0));
        let track_id = project.tracks[0].id;
        project.add_effect(Some(track_id), testkit::TAIL_ID);
        project.add_effect(None, testkit::GAIN_ID);
        // Automated too, so the per-segment pass is inside the watched region: it runs on the
        // audio thread and is bound by the same rule as everything else there.
        project.automation.set_point(
            ParamTarget::TrackGain(track_id),
            AutomationCurve::Linear,
            Ticks::ZERO,
            -3.0,
        );
        project.automation.set_point(
            ParamTarget::TrackGain(track_id),
            AutomationCurve::Linear,
            Ticks::from_beats(8.0),
            0.0,
        );
        let mut graph = build(&project, 512);
        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        // Warm up outside the watched region so first-touch growth is not counted.
        render_block(&mut graph, &mut transport, &mut out, false);

        let allocations = testkit::count_allocations(|| {
            for _ in 0..200 {
                render_block(&mut graph, &mut transport, &mut out, false);
            }
        });
        assert_eq!(allocations, 0, "render_block allocated on the audio thread");
    }

    #[test]
    fn the_worst_case_event_load_never_reallocates_or_drops_a_note() {
        // Every pitch held at once, a jump on every block so the chase runs every time, and a
        // full audition queue on top: the tightest the per-block event buffer ever gets.
        let mut project = Project::new("Worst", SAMPLE_RATE);
        let track = project.add_instrument_track("All", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        for pitch in 0..128u8 {
            midi.notes
                .push(Note::new(pitch, Ticks::ZERO, Ticks::from_beats(8.0)));
        }
        let mut graph = build(&project, 512);
        let mut transport = Transport::playing_from(0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        render_block(&mut graph, &mut transport, &mut out, false);

        let allocations = testkit::count_allocations(|| {
            for step in 1..50u64 {
                for pitch in 0..24u8 {
                    graph.note_on(0, pitch, 1.0);
                }
                transport.seek(step * 1_111);
                graph.reset_voices();
                render_block(&mut graph, &mut transport, &mut out, false);
            }
        });
        assert_eq!(allocations, 0, "the worst-case event buffer reallocated");
        assert!(
            (out.channel(0)[500] - 128.0 * TONE_AMPLITUDE).abs() < 1e-3,
            "every chased note must come back: got {}",
            out.channel(0)[500]
        );
    }

    #[test]
    fn peaks_are_recorded_post_fader_for_the_meters() {
        let project = one_note_project(Ticks::ZERO, Ticks::from_beats(4.0));
        let mut graph = build(&project, 512);
        graph.set_track_gain_db(0, -120.0);
        let mut out = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        // Two blocks: the first is the ramp down to silence, the second is settled.
        render_block(&mut graph, &mut Transport::playing_from(0), &mut out, false);
        render_block(
            &mut graph,
            &mut Transport::playing_from(512),
            &mut out,
            false,
        );
        assert!(graph.track_peak(0) < 1e-5);
        assert!(graph.master_peak() < 1e-5);
    }
}
