//! Sample-accurate event dispatch.
//!
//! A note scheduled for frame 100 has to start at frame 100, not at the start of the block it
//! happens to fall in. At a 512-frame block size, applying events only at block boundaries
//! smears every note by up to 11 ms and quantises rhythm to the audio driver's buffer size,
//! which is audible on anything faster than a slow ballad.
//!
//! [`render_segments`] does the splitting once, so each instrument only has to say how to apply
//! one event and how to fill a frame range.

use auris_core::{AudioBuffer, NoteEvent};

/// An instrument that renders a block in pieces split at event boundaries.
pub trait SegmentRenderer {
    /// Applies one event to the instrument's state. Called between segments, so whatever it
    /// changes takes effect on the very next frame.
    fn handle_event(&mut self, event: &NoteEvent);

    /// Renders frames `start..end` of `out`, overwriting whatever was there.
    ///
    /// `start < end` and `end` never exceeds the buffer's frame count.
    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize);
}

/// Renders `frames` of `out` through `renderer`, applying each event at its own frame.
///
/// `events` is expected to be sorted by frame, as the [`Instrument`](auris_core::Instrument)
/// contract guarantees; out-of-order or out-of-range events are applied at the current position
/// instead of being dropped, so a misbehaving caller loses timing but never causes a panic.
pub fn render_segments<R>(
    renderer: &mut R,
    events: &[NoteEvent],
    out: &mut AudioBuffer,
    frames: usize,
) where
    R: SegmentRenderer + ?Sized,
{
    let frames = frames.min(out.frame_count());
    let mut cursor = 0usize;
    let mut index = 0usize;

    while index < events.len() {
        let Some(event) = events.get(index) else {
            break;
        };
        let frame = (event.frame() as usize).min(frames);
        if frame > cursor {
            renderer.render_segment(out, cursor, frame);
            cursor = frame;
        }
        while let Some(event) = events.get(index) {
            if (event.frame() as usize).min(frames) > cursor {
                break;
            }
            renderer.handle_event(event);
            index += 1;
        }
    }

    if cursor < frames {
        renderer.render_segment(out, cursor, frames);
    }
}

/// Copies channel 0 of `out` over every other channel, for the frames that were rendered.
///
/// The built-in instruments are mono generators; this is how they honour the requirement to
/// write every channel of the output buffer without paying for a channel branch per sample.
pub fn spread_to_all_channels(out: &mut AudioBuffer, frames: usize) {
    let frames = frames.min(out.frame_count());
    let Some((first, rest)) = out.channels_mut().split_first_mut() else {
        return;
    };
    let Some(source) = first.get(..frames) else {
        return;
    };
    for channel in rest {
        if let Some(target) = channel.get_mut(..frames) {
            target.copy_from_slice(source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        events: Vec<u32>,
        segments: Vec<(usize, usize)>,
    }

    impl SegmentRenderer for Recorder {
        fn handle_event(&mut self, event: &NoteEvent) {
            self.events.push(event.frame());
        }

        fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
            assert!(start < end, "empty segment {start}..{end}");
            assert!(end <= out.frame_count());
            self.segments.push((start, end));
        }
    }

    fn note_on(frame: u32) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch: 60,
            velocity: 1.0,
        }
    }

    #[test]
    fn a_block_without_events_renders_in_one_piece() {
        let mut buffer = AudioBuffer::stereo(512, 48_000.0);
        let mut recorder = Recorder::default();
        render_segments(&mut recorder, &[], &mut buffer, 512);
        assert_eq!(recorder.segments, vec![(0, 512)]);
    }

    #[test]
    fn events_split_the_block_at_their_own_frames() {
        let mut buffer = AudioBuffer::stereo(512, 48_000.0);
        let mut recorder = Recorder::default();
        let events = [note_on(100), note_on(100), note_on(300)];
        render_segments(&mut recorder, &events, &mut buffer, 512);
        assert_eq!(recorder.segments, vec![(0, 100), (100, 300), (300, 512)]);
        assert_eq!(recorder.events, vec![100, 100, 300]);
    }

    #[test]
    fn an_event_at_frame_zero_does_not_produce_an_empty_segment() {
        let mut buffer = AudioBuffer::stereo(64, 48_000.0);
        let mut recorder = Recorder::default();
        render_segments(&mut recorder, &[note_on(0)], &mut buffer, 64);
        assert_eq!(recorder.segments, vec![(0, 64)]);
        assert_eq!(recorder.events, vec![0]);
    }

    #[test]
    fn out_of_range_and_unsorted_events_are_still_applied() {
        let mut buffer = AudioBuffer::stereo(64, 48_000.0);
        let mut recorder = Recorder::default();
        let events = [note_on(200), note_on(10)];
        render_segments(&mut recorder, &events, &mut buffer, 64);
        assert_eq!(recorder.events.len(), 2);
        assert_eq!(recorder.segments, vec![(0, 64)]);
    }

    #[test]
    fn a_zero_length_block_renders_nothing() {
        let mut buffer = AudioBuffer::stereo(0, 48_000.0);
        let mut recorder = Recorder::default();
        render_segments(&mut recorder, &[note_on(0)], &mut buffer, 0);
        assert!(recorder.segments.is_empty());
        assert_eq!(recorder.events, vec![0]);
    }

    #[test]
    fn spreading_copies_the_first_channel_everywhere() {
        let mut buffer = AudioBuffer::new(3, 4, 48_000.0);
        buffer.channel_mut(0).copy_from_slice(&[0.1, 0.2, 0.3, 0.4]);
        spread_to_all_channels(&mut buffer, 4);
        assert_eq!(buffer.channel(1), &[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(buffer.channel(2), &[0.1, 0.2, 0.3, 0.4]);
    }
}
