//! Flattening the document's musical time into absolute timeline frames.
//!
//! Its own file because it is the half of building that is pure arithmetic over the tempo map: a
//! clip goes in, a sorted event list or a resolved buffer handle comes out, and nothing here knows
//! what a mixer strip is. The two bounds at the end belong with it for the same reason — they are
//! read off the event list, and what they size is the scratch that stops the audio thread ever
//! reaching for the allocator.

use std::sync::Arc;

use auris_core::AudioBuffer;
use auris_core::param::db_to_gain;
use auris_core::plugin::NoteEvent;
use auris_core::project::{AudioClip, AudioSourceBank, FadeCurve, MidiClip};
use auris_core::time::{TempoMap, Ticks};

/// A note event pinned to an absolute position on the timeline.
///
/// The `frame` inside `event` is meaningless here; the renderer rewrites it to a block-relative
/// offset with [`NoteEvent::with_frame`] as it hands the event to the instrument.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScheduledEvent {
    /// Absolute timeline position, in frames.
    pub frame: u64,
    /// What happens at that position.
    pub event: NoteEvent,
}

/// One audio clip, resolved against the sample bank and the tempo map.
#[derive(Clone, Debug)]
pub struct RenderAudioClip {
    /// Decoded samples, shared with the bank rather than copied.
    pub buffer: Arc<AudioBuffer>,
    /// Where the clip starts on the timeline, in frames.
    pub start_frame: u64,
    /// First frame of `buffer` the clip plays.
    pub source_offset: u64,
    /// How many frames the clip plays. Always within the source's bounds.
    pub length: u64,
    /// Clip trim as a linear multiplier.
    pub gain: f32,
    /// Fade-in length in frames.
    pub fade_in: u64,
    /// Fade-out length in frames.
    pub fade_out: u64,
    /// The shape of the fade-in.
    pub fade_in_curve: FadeCurve,
    /// The shape of the fade-out.
    pub fade_out_curve: FadeCurve,
}

impl RenderAudioClip {
    /// Fade multiplier `position` frames into the clip.
    ///
    /// Mirrors [`AudioClip::fade_gain_at`] so what the arrangement draws is what plays — the
    /// curve included, which is what makes a crossfade hold its power rather than dip through
    /// the middle of the join.
    pub fn fade_gain(&self, position: u64) -> f32 {
        let mut gain = 1.0f32;
        if self.fade_in > 0 && position < self.fade_in {
            gain *= self
                .fade_in_curve
                .gain_in(position as f32 / self.fade_in as f32);
        }
        if self.fade_out > 0 {
            let fade_start = self.length.saturating_sub(self.fade_out);
            if position >= fade_start {
                let into_fade = position - fade_start;
                gain *= self
                    .fade_out_curve
                    .gain_out(into_fade as f32 / self.fade_out as f32);
            }
        }
        gain
    }
}

/// Flattens one MIDI clip's notes into absolute timeline events.
///
/// Notes starting past the clip's end are dropped and notes running past it are cut short, which
/// is what the arrangement shows: the clip's length is the gate. A looped clip goes through the
/// same gate once per pass — looping is expanded here, at build time, so nothing on the audio
/// thread has to know it happened.
pub(super) fn schedule_clip(
    clip: &MidiClip,
    tempo_map: &TempoMap,
    sample_rate: f64,
    out: &mut Vec<ScheduledEvent>,
) {
    if clip.muted || clip.length <= Ticks::ZERO {
        return;
    }
    // Which notes a clip actually plays is `MidiClip`'s own rule, asked rather than repeated: the
    // MIDI writer asks the same question, and an export that answered it differently from the
    // renderer would write a file that is not the piece you can hear. The tempo is the one in
    // force at the clip — the transforms' humanisation is milliseconds, and this is where
    // milliseconds meet ticks.
    for note in clip.sounding_notes(tempo_map.bpm_at(clip.start)) {
        let start_tick = clip.start + note.start;
        let end_tick = clip.start + note.end();
        let start = tempo_map.ticks_to_samples(start_tick, sample_rate).raw();
        // A note must occupy at least one frame or the instrument would see the release before
        // it ever produced a sample.
        let end = tempo_map
            .ticks_to_samples(end_tick, sample_rate)
            .raw()
            .max(start + 1);
        out.push(ScheduledEvent {
            frame: start,
            event: NoteEvent::NoteOn {
                frame: 0,
                pitch: note.pitch,
                velocity: note.velocity.clamp(0.0, 1.0),
            },
        });
        out.push(ScheduledEvent {
            frame: end,
            event: NoteEvent::NoteOff {
                frame: 0,
                pitch: note.pitch,
            },
        });
    }
    // The curves the clip actually carries, sampled the same way and by the same rule — asked of
    // the clip rather than worked out here, so the roll drawing a curve and the renderer playing
    // it read one answer.
    for which in clip.curves() {
        for (at, value) in clip.sounding_curve_events(which, auris_core::project::CURVE_STEP) {
            let frame = tempo_map
                .ticks_to_samples(clip.start + at, sample_rate)
                .raw();
            out.push(ScheduledEvent {
                frame,
                event: match which {
                    auris_core::project::ClipCurve::Bend => NoteEvent::PitchBend {
                        frame: 0,
                        semitones: value,
                    },
                    auris_core::project::ClipCurve::Controller(number) => NoteEvent::Controller {
                        frame: 0,
                        number,
                        value,
                    },
                },
            });
        }
    }
}

/// Rank used to break ties between events landing on the same frame.
///
/// Releases go first so that a note repeated at the same pitch retriggers instead of the new
/// note being cut short by the old note's release.
fn event_rank(event: &NoteEvent) -> u8 {
    match event {
        NoteEvent::NoteOff { .. }
        | NoteEvent::AllNotesOff { .. }
        | NoteEvent::AllSoundOff { .. } => 0,
        // Controller state before the strike, so a note landing on the same frame as the curve
        // that shapes it is already being shaped when it starts.
        NoteEvent::PitchBend { .. } | NoteEvent::Controller { .. } => 1,
        NoteEvent::NoteOn { .. } => 2,
    }
}

pub(super) fn sort_events(events: &mut [ScheduledEvent]) {
    events.sort_by_key(|scheduled| (scheduled.frame, event_rank(&scheduled.event)));
}

/// Resolves a project audio clip against the sample bank, one entry per pass it makes.
///
/// `source_rate` is the rate the clip's frame counts — its trim, its length, its fades — are
/// expressed in, which is the rate the file was decoded at. `sample_rate` is the rate this graph
/// renders at, and the two are not always the same: an output device is free to run at 44.1 kHz
/// under a 48 kHz project, and an export can be asked for any rate at all. Every frame count is
/// therefore converted on the way in, and the caller is expected to have handed the bank buffers
/// at the render rate to match.
///
/// A looped clip becomes several entries rather than one entry that knows it repeats, for the
/// reason [`schedule_clip`] expands its passes too: the renderer walks a flat list of windows
/// onto buffers, and a window that had to work out which repeat it was in would be arithmetic on
/// the audio thread in aid of nothing.
///
/// The **fades stay on the clip's own edges**: the fade-in shapes the first pass and the fade-out
/// the last, and the joins between repeats run flat. A fade-out at the end of every pass would
/// pump once a bar, which is not what somebody who drew one fade meant.
pub(super) fn resolve_audio_clip(
    clip: &AudioClip,
    bank: &AudioSourceBank,
    tempo_map: &TempoMap,
    sample_rate: f64,
    source_rate: f64,
    out: &mut Vec<RenderAudioClip>,
) {
    if clip.muted {
        return;
    }
    // How far this clip's audio is stretched, and therefore which copy of the source it plays.
    // The stretched copies are made where a stretcher may be run at all — the session, off the
    // audio thread — and a missing one is played unstretched rather than not played: a clip out of
    // time is wrong in a way somebody can hear and fix, and silence is not.
    let stretch = clip.stretch_in(tempo_map);
    let key = auris_core::project::stretch_key(stretch);
    let (buffer, stretch) = match bank.stretched(clip.source, key) {
        Some(buffer) => (buffer, stretch),
        None => match bank.get(clip.source) {
            Some(buffer) => {
                log::warn!(
                    "clip `{}` has no audio stretched to {stretch:.3}; playing it as recorded",
                    clip.name
                );
                (buffer, 1.0)
            }
            None => {
                log::warn!(
                    "clip `{}` references source {} which is not loaded",
                    clip.name,
                    clip.source.0
                );
                return;
            }
        },
    };
    // A nonsense rate in the document would otherwise scale every position to zero or to NaN.
    let rate_ratio = if source_rate.is_finite() && source_rate > 0.0 {
        sample_rate / source_rate
    } else {
        1.0
    };
    // Two conversions in one: the file's rate against this graph's, and the stretch. Positions
    // inside a stretched copy move with it — a trim half a second into the file is half a second
    // times the stretch into the copy — so every frame count here goes through both.
    let ratio = rate_ratio * stretch;
    // Casting a float to an integer saturates in Rust, so an absurd figure clamps rather than
    // wrapping round to a small one.
    let convert = |frames: u64| (frames as f64 * ratio).round() as u64;

    let available = buffer.frame_count() as u64;
    let source_offset = convert(clip.offset_frames).min(available);
    let length = convert(clip.length_frames).min(available - source_offset);
    if length == 0 {
        return;
    }
    let start = clip.start.max_zero();
    // How long one pass is on the musical grid, which is what `loop_end` is measured against.
    let content = tempo_map.seconds_to_ticks(auris_core::time::Seconds(
        tempo_map.ticks_to_seconds(start).0 + length as f64 / sample_rate.max(1.0),
    )) - start;
    let passes: Vec<(Ticks, Ticks)> =
        auris_core::project::loop_passes(content, clip.loop_end).collect();
    let last = passes.len().saturating_sub(1);
    for (index, (offset, span)) in passes.into_iter().enumerate() {
        let from = tempo_map
            .ticks_to_samples(start + offset, sample_rate)
            .raw();
        // The pass's own length in frames, which is the trim except where the loop cuts through
        // it — and a tempo change under a repeat makes even a whole pass a different number of
        // frames from the one before, so it is measured rather than carried.
        let frames = match span < content {
            true => tempo_map
                .ticks_to_samples(start + offset + span, sample_rate)
                .raw()
                .saturating_sub(from)
                .min(length),
            false => length,
        };
        if frames == 0 {
            continue;
        }
        out.push(RenderAudioClip {
            buffer: Arc::clone(buffer),
            start_frame: from,
            source_offset,
            length: frames,
            gain: db_to_gain(clip.gain_db),
            fade_in: match index == 0 {
                true => convert(clip.fade_in_frames).min(frames),
                false => 0,
            },
            fade_out: match index == last {
                true => convert(clip.fade_out_frames).min(frames),
                false => 0,
            },
            fade_in_curve: clip.fade_in_curve,
            fade_out_curve: clip.fade_out_curve,
        });
    }
}

/// Largest number of events that can fall inside any window of `window` frames.
///
/// This is the exact bound the per-block event buffer needs: rendering never sees more than one
/// window's worth at a time, so sizing to this makes the buffer allocation-free without ever
/// dropping an event.
/// Largest number of notes this track ever has sounding at once.
///
/// That is exactly what the chase after a seek has to be able to re-issue: it re-triggers every
/// note spanning the new position, including several on one pitch when they overlap. Sizing the
/// event buffer to this makes the chase allocation-free without ever dropping a note.
///
/// The events arrive sorted by frame with releases ranked ahead of strikes on the same frame, so
/// a single pass over them is enough — a note ending exactly where the next begins never counts
/// as two.
pub(super) fn max_sounding_notes(events: &[ScheduledEvent]) -> usize {
    let mut sounding = 0usize;
    let mut most = 0usize;
    for scheduled in events {
        match scheduled.event {
            NoteEvent::NoteOn { .. } => {
                sounding += 1;
                most = most.max(sounding);
            }
            NoteEvent::NoteOff { .. } => sounding = sounding.saturating_sub(1),
            NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. } => sounding = 0,
            NoteEvent::PitchBend { .. } | NoteEvent::Controller { .. } => {}
        }
    }
    most
}

pub(super) fn max_events_in_window(events: &[ScheduledEvent], window: u64) -> usize {
    let mut best = 0;
    let mut low = 0;
    for (high, event) in events.iter().enumerate() {
        while event.frame - events[low].frame >= window {
            low += 1;
        }
        best = best.max(high - low + 1);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::quarter_note_project;
    use crate::graph::{RenderGraph, RenderSource};
    use crate::testkit;
    use auris_core::project::{CurvePoint, Note, Project};
    use auris_core::time::TICKS_PER_QUARTER;

    #[test]
    fn a_clip_that_follows_the_tempo_is_played_out_of_the_stretched_copy() {
        // The drawn length and the played length come from the same stretch, and this is the half
        // of it the audio thread sees. A renderer that read the stored frame count would play a
        // second of audio where the arrangement had drawn two.
        let mut project = Project::new("Follow", 48_000.0);
        project.tempo_map = auris_core::time::TempoMap::constant(60.0);
        let track = project.add_audio_track("Audio");
        let source = project.add_audio_source(
            "loop",
            auris_core::AssetPath::inside("Audio/loop.wav"),
            48_000,
            48_000.0,
            2,
        );
        let clip_id = project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("a clip");
        let clip = project.audio_clip_mut(clip_id).expect("the clip");
        clip.source_bpm = Some(120.0);
        clip.follows_tempo = true;
        let clip = project.audio_clip(clip_id).expect("the clip").clone();

        let mut bank = auris_core::AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(AudioBuffer::from_planar(vec![vec![0.5; 48_000]; 2], 48_000.0).expect("mono")),
        );
        bank.insert_stretched(
            source,
            auris_core::project::stretch_key(2.0),
            Arc::new(AudioBuffer::from_planar(vec![vec![0.5; 96_000]; 2], 48_000.0).expect("copy")),
        );

        let mut out = Vec::new();
        resolve_audio_clip(
            &clip,
            &bank,
            &project.tempo_map,
            48_000.0,
            48_000.0,
            &mut out,
        );
        let played = out.first().expect("one pass");
        assert_eq!(played.length, 96_000, "the stretch was not played");
        assert_eq!(played.buffer.frame_count(), 96_000, "the wrong copy");

        // With no stretched copy to be found the clip is played as recorded rather than not at
        // all: a clip out of time can be heard and put right, and silence cannot.
        let mut bare = auris_core::AudioSourceBank::new();
        bare.insert(
            source,
            Arc::new(AudioBuffer::from_planar(vec![vec![0.5; 48_000]; 2], 48_000.0).expect("mono")),
        );
        let mut out = Vec::new();
        resolve_audio_clip(
            &clip,
            &bare,
            &project.tempo_map,
            48_000.0,
            48_000.0,
            &mut out,
        );
        assert_eq!(out.first().expect("one pass").length, 48_000);
    }

    #[test]
    fn both_of_a_clips_curves_are_scheduled_as_the_message_they_are() {
        // Two curves of the same shape on one clip, and the only thing that tells them apart is
        // the event each becomes. Read the wrong way round, the wheel would detune the part and
        // the bend would open a vibrato.
        let mut project = quarter_note_project();
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        let midi = project.midi_clip_mut(clip).expect("the clip");
        midi.bend = vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 0.0,
            },
            CurvePoint {
                at: Ticks::QUARTER,
                value: 2.0,
            },
        ];
        midi.controllers.insert(
            auris_core::CC_MODULATION,
            vec![
                CurvePoint {
                    at: Ticks::ZERO,
                    value: 0.0,
                },
                CurvePoint {
                    at: Ticks::QUARTER,
                    value: 1.0,
                },
            ],
        );

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        let bends: Vec<f32> = events
            .iter()
            .filter_map(|scheduled| match scheduled.event {
                NoteEvent::PitchBend { semitones, .. } => Some(semitones),
                _ => None,
            })
            .collect();
        let wheel: Vec<f32> = events
            .iter()
            .filter_map(|scheduled| match scheduled.event {
                NoteEvent::Controller { number, value, .. }
                    if number == auris_core::CC_MODULATION =>
                {
                    Some(value)
                }
                _ => None,
            })
            .collect();
        assert!(!bends.is_empty(), "the bend was never scheduled");
        assert!(!wheel.is_empty(), "the wheel was never scheduled");
        // Each rises to its own top and is let go before the clip ends, because both are channel
        // state and a clip that finished holding either would carry it into the next one.
        assert!(bends.iter().cloned().fold(f32::MIN, f32::max) >= 2.0);
        assert!(wheel.iter().cloned().fold(f32::MIN, f32::max) >= 1.0);
        assert_eq!(
            bends.last().copied(),
            Some(0.0),
            "the bend was left hanging"
        );
        assert_eq!(wheel.last().copied(), Some(0.0), "the wheel was left up");
        // And neither ever reads as the other: a wheel is never negative and never past one.
        assert!(wheel.iter().all(|value| (0.0..=1.0).contains(value)));

        // And the whole list stays sorted by frame, which is what the renderer walks it assuming.
        let frames: Vec<u64> = events.iter().map(|scheduled| scheduled.frame).collect();
        let mut sorted = frames.clone();
        sorted.sort_unstable();
        assert_eq!(
            frames, sorted,
            "adding the curves left the events out of order"
        );
    }

    #[test]
    fn notes_land_on_absolute_sample_positions() {
        let project = quarter_note_project();
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        // 120 BPM at 48 kHz: one quarter note is 0.5 s, so 24 000 frames.
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 24_000);
        assert_eq!(events[1].frame, 48_000);
        assert!(matches!(
            events[0].event,
            NoteEvent::NoteOn { pitch: 60, .. }
        ));
        assert!(matches!(
            events[1].event,
            NoteEvent::NoteOff { pitch: 60, .. }
        ));
    }

    #[test]
    fn notes_are_clipped_to_their_clips_length() {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Short", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        // Four beats long inside a one-beat clip, plus a note that starts after the clip ends.
        midi.notes
            .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(4.0)));
        midi.notes
            .push(Note::new(62, Ticks::from_beats(2.0), Ticks::QUARTER));

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 0);
        assert_eq!(events[1].frame, 24_000);
    }

    #[test]
    fn a_looped_midi_clip_is_expanded_into_its_repeats() {
        // Looping is flattened at build time. Nothing on the audio thread knows a clip repeats,
        // which is the only way it can be free of the arithmetic.
        let mut project = quarter_note_project();
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        let (length, was) = {
            let (_, midi) = project.midi_clip(clip).expect("the clip");
            (midi.length, midi.notes.len())
        };
        assert_eq!(was, 1, "the fixture is one note");
        project.midi_clip_mut(clip).expect("the clip").loop_end = length * 3;

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        // Three passes of one note: three strikes and three releases, and the document still
        // holds exactly the one note somebody played.
        assert_eq!(events.len(), 6);
        assert_eq!(project.midi_clip(clip).expect("the clip").1.notes.len(), 1);
        let strikes: Vec<u64> = events
            .iter()
            .filter(|scheduled| matches!(scheduled.event, NoteEvent::NoteOn { .. }))
            .map(|scheduled| scheduled.frame)
            .collect();
        // The fixture's clip is four quarters long, which at 120 BPM and 48 kHz is 96 000 frames
        // a pass, and its note sits one quarter into each one.
        assert_eq!(strikes, vec![24_000, 120_000, 216_000]);
    }

    #[test]
    fn a_looped_audio_clip_becomes_one_window_per_pass() {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_audio_track("Drums");
        let source = project.add_audio_source(
            "loop",
            auris_core::AssetPath::inside("Audio/loop.wav"),
            48_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        {
            let clip = project.audio_clip_mut(clip).unwrap();
            // One second — one bar at 120 BPM is two seconds, so this is half a bar.
            clip.length_frames = 48_000;
            clip.fade_in_frames = 480;
            clip.fade_out_frames = 480;
            // Two and a half passes.
            clip.loop_end = Ticks::QUARTER * 5;
        }
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(48_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips.len(), 3);
        assert_eq!(
            clips
                .iter()
                .map(|clip| clip.start_frame)
                .collect::<Vec<_>>(),
            vec![0, 48_000, 96_000]
        );
        // Every pass reads the same window of the source: a repeat is the same material again.
        assert!(clips.iter().all(|clip| clip.source_offset == 0));
        assert_eq!(clips[0].length, 48_000);
        assert_eq!(clips[2].length, 24_000, "the last pass is cut by the loop");
        // The fades belong to the clip's own edges, and nothing pumps at the joins.
        assert_eq!((clips[0].fade_in, clips[0].fade_out), (480, 0));
        assert_eq!((clips[1].fade_in, clips[1].fade_out), (0, 0));
        assert_eq!((clips[2].fade_in, clips[2].fade_out), (0, 480));
    }

    #[test]
    fn a_muted_clip_schedules_nothing() {
        let mut project = quarter_note_project();
        project.tracks[0].kind.as_instrument_mut().unwrap().clips[0].muted = true;
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.tracks()[0].event_count(), 0);
    }

    #[test]
    fn a_tempo_change_moves_later_notes() {
        let mut project = quarter_note_project();
        project.tempo_map.set_point(Ticks(TICKS_PER_QUARTER), 240.0);
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        // The note still starts one quarter in at 120 BPM (24 000 frames) but now lasts only
        // 0.25 s because the tempo doubles exactly where it begins.
        assert_eq!(events[0].frame, 24_000);
        assert_eq!(events[1].frame, 36_000);
    }

    #[test]
    fn the_sounding_bound_ignores_notes_that_never_overlap() {
        let events: Vec<ScheduledEvent> = [
            (0u64, 60u8, true),
            (10, 60, false),
            (20, 62, true),
            (30, 62, false),
        ]
        .into_iter()
        .map(|(frame, pitch, on)| ScheduledEvent {
            frame,
            event: if on {
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch,
                    velocity: 1.0,
                }
            } else {
                NoteEvent::NoteOff { frame: 0, pitch }
            },
        })
        .collect();
        assert_eq!(max_sounding_notes(&events), 1);
        assert_eq!(max_sounding_notes(&[]), 0);
    }

    #[test]
    fn window_bound_counts_the_densest_run() {
        let events: Vec<ScheduledEvent> = [0u64, 10, 20, 30, 500, 505]
            .into_iter()
            .map(|frame| ScheduledEvent {
                frame,
                event: NoteEvent::AllNotesOff { frame: 0 },
            })
            .collect();
        assert_eq!(max_events_in_window(&events, 100), 4);
        assert_eq!(max_events_in_window(&events, 11), 2);
        assert_eq!(max_events_in_window(&[], 512), 0);
    }

    #[test]
    fn audio_clips_are_clamped_to_the_source_length() {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_audio_track("Drums");
        let source = project.add_audio_source(
            "loop",
            auris_core::AssetPath::inside("Audio/loop.wav"),
            1_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        {
            let clip = project.audio_clip_mut(clip).unwrap();
            clip.offset_frames = 900;
            clip.length_frames = 500;
        }
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].source_offset, 900);
        assert_eq!(clips[0].length, 100);
    }

    #[test]
    fn a_clips_frame_counts_are_converted_to_the_render_rate() {
        // The document counts a clip's trim in the frames of the file it came from. Rendering at
        // another rate — an output device that disagrees with the project, or an export asked for
        // a different rate — means those counts no longer measure the timeline, so they are
        // converted rather than used as they stand.
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            48_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        {
            let clip = project.audio_clip_mut(clip).unwrap();
            clip.offset_frames = 4_800;
            clip.length_frames = 24_000;
            clip.fade_in_frames = 480;
            clip.fade_out_frames = 960;
        }
        // The bank holds the source converted to the rate the graph will render at.
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(96_000, 96_000.0)));

        let graph = RenderGraph::build_at(&project, &bank, &testkit::registry(), 512, 96_000.0);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].source_offset, 9_600);
        assert_eq!(clips[0].length, 48_000);
        assert_eq!(clips[0].fade_in, 960);
        assert_eq!(clips[0].fade_out, 1_920);
    }

    #[test]
    fn a_matching_rate_leaves_a_clips_frame_counts_alone() {
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            1_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        project.audio_clip_mut(clip).unwrap().length_frames = 800;
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].length, 800);
    }

    #[test]
    fn a_source_recorded_with_a_nonsense_rate_is_played_as_it_stands() {
        // A corrupt document should not scale every position to zero or to NaN.
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            1_000,
            0.0,
            2,
        );
        project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].length, 1_000);
    }
}
