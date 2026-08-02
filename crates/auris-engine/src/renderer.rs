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
    PITCH_COUNT, RenderAudioClip, RenderGraph, RenderSource, RenderTrack, ScheduledEvent,
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

    for track in graph.tracks.iter_mut() {
        if !track.strip.is_active() {
            continue;
        }
        track.scratch.set_frame_count(frames);
        render_source(track, transport, &ctx);
        for (effect, enabled) in track.strip.effects.iter_mut().zip(&track.strip.enabled) {
            if *enabled {
                effect.process(&mut track.scratch, &ctx);
            }
        }
        track.strip.apply_gain_and_pan(&mut track.scratch);
        track.peak = track.peak.max(track.scratch.peak());
        master_scratch.mix_from(&track.scratch, 1.0);
    }

    for (effect, enabled) in graph.master.effects.iter_mut().zip(&graph.master.enabled) {
        if *enabled {
            effect.process(master_scratch, &ctx);
        }
    }
    graph.master.apply_gain_and_pan(master_scratch);
    // Mute is the last stage of the strip: the effects still run — so a reverb tail does not
    // freeze and un-muting does not pop — but nothing leaves the bus.
    if !graph.master.is_active() {
        master_scratch.clear();
    }
    // One NaN out of a misbehaving plugin would otherwise reach the output device.
    master_scratch.sanitize();
    graph.master_peak[0] = graph.master_peak[0].max(master_scratch.channel_peak(0));
    graph.master_peak[1] = graph.master_peak[1].max(master_scratch.channel_peak(1));

    write_segment(out, offset, master_scratch, frames);
}

/// Fills a track's scratch buffer with whatever it plays for this segment.
fn render_source(track: &mut RenderTrack, transport: &Transport, ctx: &ProcessContext) {
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
            NoteEvent::PitchBend { .. } => {}
        }
    }

    // Drop whatever the old position left sounding before restoring what belongs to the new one.
    out.push(NoteEvent::AllNotesOff { frame: 0 });
    for (pitch, count) in counts.iter().enumerate() {
        if *count == 0 || out.len() == out.capacity() {
            continue;
        }
        out.push(NoteEvent::NoteOn {
            frame: 0,
            pitch: pitch as u8,
            velocity: velocity[pitch],
        });
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
    use auris_core::param::db_to_gain;
    use auris_core::project::{AudioSourceBank, Note, Project};
    use auris_core::time::Ticks;
    use std::sync::Arc;

    const SAMPLE_RATE: f64 = 48_000.0;

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
        let source = project.add_audio_source("s", "s.wav".into(), 8_000, SAMPLE_RATE, 2);
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
        let source = project.add_audio_source("ramp", "ramp.wav".into(), 8_000, SAMPLE_RATE, 2);
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
        let silent = render_range(&mut graph, &mut Transport::playing_from(0), 2_048, 512);
        assert_eq!(silent.peak(), 0.0);

        graph.set_track_mute(0, false);
        let audible = render_range(&mut graph, &mut Transport::playing_from(2_048), 2_048, 512);
        assert!((audible.peak() - TONE_AMPLITUDE).abs() < 1e-5);
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
        let source = project.add_audio_source("ones", "ones.wav".into(), 1_000, SAMPLE_RATE, 1);
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

    #[test]
    fn render_block_never_allocates() {
        let mut project = one_note_project(Ticks::ZERO, Ticks::from_beats(8.0));
        let track_id = project.tracks[0].id;
        project.add_effect(Some(track_id), testkit::TAIL_ID);
        project.add_effect(None, testkit::GAIN_ID);
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
