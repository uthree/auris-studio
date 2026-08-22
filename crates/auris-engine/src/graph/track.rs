//! One track, ready to render: what it plays, where its output goes, and the scratch it needs.
//!
//! Its own file because a track is the graph's *node* — the thing the routing joins up and the
//! renderer walks — while what a track sounds like is [`RenderStrip`]'s business and when it
//! sounds is `schedule`'s. Everything here is either a wire or a pre-sized buffer, which is what
//! keeps the render loop free of the allocator.

use auris_core::plugin::{Instrument, NoteEvent};
use auris_core::project::TrackId;
use auris_core::{AudioBuffer, ParamId};

use super::PITCH_COUNT;
use super::latency::LatencyDelay;
use super::schedule::{RenderAudioClip, ScheduledEvent};
use super::strip::{RenderStrip, SmoothedGain};

/// Where a track's audio comes from.
pub enum RenderSource {
    /// A software instrument driven by a pre-flattened, frame-sorted event list.
    Instrument {
        /// The live plugin instance.
        instrument: Box<dyn Instrument>,
        /// Every note on the track, in absolute timeline frames, sorted by frame.
        events: Vec<ScheduledEvent>,
    },
    /// Audio clips read straight out of the sample bank.
    Audio {
        /// Clips in timeline order.
        clips: Vec<RenderAudioClip>,
    },
    /// Whatever the rest of the graph has routed here.
    ///
    /// A bus plays nothing of its own; its material is the sum sitting in the graph's bus input
    /// buffer at this slot, which every track feeding it has already written into by the time the
    /// routing order reaches the bus.
    Bus {
        /// Which of the graph's bus input buffers this reads.
        input: usize,
    },
    /// Nothing to play.
    ///
    /// Used when a track's instrument id is missing from the registry: the track keeps its slot
    /// so that command indices still line up with the project's track order, but it is silent.
    Silence,
}

/// A copy of a track's signal, on its way to a bus.
///
/// The gain is smoothed for the same reason a fader is: a send level moved during playback would
/// otherwise step, and a step is a click.
pub(crate) struct RenderSend {
    /// Which of the graph's bus input buffers this feeds.
    pub(crate) target: usize,
    /// Send level as a linear gain, ramped across the block it changes in.
    pub(crate) gain: SmoothedGain,
    /// Whether the copy is taken before the fader rather than after it.
    pub(crate) pre_fader: bool,
    /// Holds this copy back so it reaches the bus in step with everything else feeding it.
    pub(crate) delay: LatencyDelay,
    /// Somewhere to hold the delayed copy. Empty, and never touched, when the delay is zero —
    /// which it is for every send in a graph where nothing looks ahead.
    pub(crate) scratch: AudioBuffer,
}

/// One track, ready to render.
pub struct RenderTrack {
    /// Project id of the track this came from.
    pub id: TrackId,
    /// Track name, kept for logging and for the meter tooltips.
    pub name: String,
    pub(crate) source: RenderSource,
    pub(crate) strip: RenderStrip,
    /// Which bus input this track's output is mixed into; `None` for the master.
    pub(crate) output: Option<usize>,
    /// Copies of this track's signal, on their way to buses.
    pub(crate) sends: Vec<RenderSend>,
    /// Holds this track back to the longest path through the graph, so the sources line up.
    pub(crate) delay: LatencyDelay,
    /// Holds this track's *output* back so it reaches its destination in step with everything
    /// else arriving there.
    ///
    /// Distinct from [`Self::delay`], and applied after the fader rather than before it, because
    /// every one of a track's outgoing edges needs a delay of its own: a track feeding the master
    /// dry while sending to a bus that looks ahead has two paths of different lengths, and only a
    /// delay per edge can make both arrive together. Zero unless a send makes it otherwise, which
    /// is why the fader still acts immediately in every ordinary graph.
    pub(crate) output_delay: LatencyDelay,
    pub(crate) scratch: AudioBuffer,
    /// Events handed to the instrument for the current block, block-relative and sorted.
    pub(crate) block_events: Vec<NoteEvent>,
    /// Notes triggered from the UI, consumed at the start of the next block.
    pub(crate) audition: Vec<NoteEvent>,
    /// Frame the previous rendered block ended on. A mismatch with the next block's start means
    /// the playhead jumped — a seek, a loop wrap or a fresh start — and the notes that should be
    /// sounding there have to be chased.
    pub(crate) continued_from: Option<u64>,
    /// How many times each pitch is on at the chase position.
    pub(crate) chase_counts: [u8; PITCH_COUNT],
    /// Velocity each pitch was last struck with, so a chased note comes back at its own level.
    pub(crate) chase_velocity: [f32; PITCH_COUNT],
    /// Post-fader peak of the last block, published to the meters by the engine.
    pub(crate) peak: f32,
    /// Which of the graph's sidechain taps this track's output is copied into, if any chain in
    /// the project keys from it.
    ///
    /// `None` for a track nobody listens to, which is nearly all of them — so a project with no
    /// sidechain in it does not copy a single buffer for this.
    pub(crate) tap: Option<usize>,
}

impl RenderTrack {
    /// The track's mixer strip.
    pub fn strip(&self) -> &RenderStrip {
        &self.strip
    }

    /// Frames this track is held back by to match the longest chain in the graph.
    pub fn compensation_frames(&self) -> usize {
        self.delay.frames()
    }

    /// The track's mixer strip, mutably.
    pub fn strip_mut(&mut self) -> &mut RenderStrip {
        &mut self.strip
    }

    /// What the track plays.
    pub fn source(&self) -> &RenderSource {
        &self.source
    }

    /// Post-fader peak of the most recently rendered block.
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// Number of scheduled note events, or 0 for a track with no instrument.
    pub fn event_count(&self) -> usize {
        match &self.source {
            RenderSource::Instrument { events, .. } => events.len(),
            _ => 0,
        }
    }

    /// Queues a note to sound at the start of the next block, for piano-roll auditioning.
    ///
    /// Dropped when the queue is full, because growing it would allocate on the audio thread.
    pub fn note_on(&mut self, pitch: u8, velocity: f32) {
        self.push_audition(NoteEvent::NoteOn {
            frame: 0,
            pitch,
            velocity: velocity.clamp(0.0, 1.0),
        });
    }

    /// Queues a note release for the start of the next block.
    pub fn note_off(&mut self, pitch: u8) {
        self.push_audition(NoteEvent::NoteOff { frame: 0, pitch });
    }

    /// Queues a bend of the whole instrument for the start of the next block.
    ///
    /// Clamped to [`BEND_LIMIT`](auris_core::project::BEND_LIMIT) either way, which is what the
    /// document allows a written curve — a live wheel and a drawn one must not be able to reach
    /// pitches the other cannot.
    pub fn pitch_bend(&mut self, semitones: f32) {
        self.push_audition(NoteEvent::PitchBend {
            frame: 0,
            semitones: semitones.clamp(
                -auris_core::project::BEND_LIMIT,
                auris_core::project::BEND_LIMIT,
            ),
        });
    }

    /// Queues a move of one of the instrument's controllers for the start of the next block.
    pub fn controller(&mut self, number: u8, value: f32) {
        self.push_audition(NoteEvent::Controller {
            frame: 0,
            number,
            value: value.clamp(0.0, auris_core::project::CONTROLLER_LIMIT),
        });
    }

    fn push_audition(&mut self, event: NoteEvent) {
        if self.audition.len() < self.audition.capacity() {
            self.audition.push(event);
        }
    }

    /// Writes a parameter on the track's instrument.
    pub fn set_instrument_param(&mut self, param: ParamId, value: f32) {
        if let RenderSource::Instrument { instrument, .. } = &mut self.source {
            instrument.set_param(param, value);
        }
    }

    /// Silences the instrument and clears the pending audition queue.
    ///
    /// Playback continuity is left alone, so nothing is chased back in: this is what a panic
    /// wants, where the point is that everything shuts up until the next note begins.
    pub fn silence_voices(&mut self) {
        self.audition.clear();
        if let RenderSource::Instrument { instrument, .. } = &mut self.source {
            instrument.reset();
        }
    }

    /// Silences the instrument and marks the next block as a jump.
    ///
    /// This is what a stop or a seek wants: any note spanning the new position is chased back in
    /// rather than staying silent until its successor.
    pub fn reset_voices(&mut self) {
        self.silence_voices();
        self.continued_from = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::RenderGraph;
    use crate::testkit;
    use auris_core::project::{AudioSourceBank, Project};

    #[test]
    fn an_unknown_instrument_keeps_the_track_slot() {
        let mut project = Project::new("Graph", 48_000.0);
        project.add_instrument_track("Ghost", "does.not.exist");
        project.add_instrument_track("Real", testkit::TONE_ID);
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.track_count(), 2);
        assert!(matches!(graph.tracks()[0].source, RenderSource::Silence));
        assert!(matches!(
            graph.tracks()[1].source,
            RenderSource::Instrument { .. }
        ));
    }
}
