//! Plugin delay compensation: the delay lines, and the arithmetic that sizes them.
//!
//! Its own file because working out what has to be held back is a graph problem, not a mixing one:
//! it walks the whole routing in one pass and answers in frames. [`longest_paths`] is shared with
//! [`RenderGraph::tail_frames`](super::RenderGraph::tail_frames), which asks the same question of
//! the same routing with a different cost per strip — a tail instead of a latency.

use auris_core::AudioBuffer;

use super::strip::RenderStrip;
use super::track::{RenderSource, RenderTrack};

/// A fixed whole-frame delay, used to line the tracks up with each other.
///
/// An effect that looks ahead — the limiter is the one that ships — can only do so by handing
/// back audio it has already held on to, so its output arrives late by however many frames it
/// declares in [`Effect::latency_frames`](auris_core::plugin::Effect::latency_frames). That is
/// not a fault to be removed; it is what makes the effect possible. What has to be removed is
/// the *difference*: a track carrying such an effect would otherwise play behind the tracks that
/// do not, and a mix where one part drags is wrong in a way no fader can fix.
///
/// So every track is delayed up to the longest chain in the graph. The whole mix then sits that
/// far behind the playhead, which for playback is latency the engine cannot avoid and for an
/// export is a lead-in that [`render_project`](crate::render_project) drops.
///
/// The buffer is a ring per channel, sized once here; [`Self::process`] only swaps samples in and
/// out of it, so it allocates nothing and is safe on the audio thread.
pub(crate) struct LatencyDelay {
    lines: Vec<Vec<f32>>,
    frames: usize,
    write: usize,
}

impl LatencyDelay {
    /// A delay of `frames` frames on `channels` channels. Zero frames is free and does nothing.
    pub(super) fn new(frames: usize, channels: usize) -> Self {
        Self {
            lines: if frames == 0 {
                Vec::new()
            } else {
                vec![vec![0.0; frames]; channels]
            },
            frames,
            write: 0,
        }
    }

    /// Frames this delay holds back.
    pub(crate) fn frames(&self) -> usize {
        self.frames
    }

    /// Delays `buffer` in place.
    ///
    /// Every channel starts from the same ring position, so they stay in phase with each other;
    /// the shared write cursor is only moved on once they have all been walked.
    pub(crate) fn process(&mut self, buffer: &mut AudioBuffer) {
        if self.frames == 0 {
            return;
        }
        let start = self.write;
        let mut finished = start;
        for (samples, line) in buffer.channels_mut().iter_mut().zip(&mut self.lines) {
            let mut position = start;
            for sample in samples.iter_mut() {
                std::mem::swap(sample, &mut line[position]);
                position += 1;
                if position == self.frames {
                    position = 0;
                }
            }
            finished = position;
        }
        self.write = finished;
    }

    /// Empties the delay, so nothing held from before a panic comes back afterwards.
    pub(super) fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.write = 0;
    }
}

/// Where every delay line in the graph goes, in frames.
pub(super) struct LatencyPlan {
    /// Total latency from the playhead to the output.
    pub(super) total: usize,
    /// What each track is held back by so that every source lines up, by track index.
    pub(super) node: Vec<usize>,
    /// What each of a track's outgoing edges is held back by so that everything arriving at a bus
    /// lines up: the output edge first, then one per send, by track index.
    pub(super) edges: Vec<Vec<usize>>,
}

/// What each of a track's outgoing edges costs downstream of the track itself, output edge first.
///
/// Written into `out` rather than returned so the caller can reuse one allocation across a whole
/// graph — this runs once per node per pass and none of it happens on the audio thread, but a
/// `Vec` per node for a number that is almost always zero is still waste.
fn edge_costs(
    track: &RenderTrack,
    bus_tracks: &[usize],
    through: &[usize],
    master_cost: usize,
    out: &mut Vec<usize>,
) {
    let resolve = |slot: Option<usize>| match slot {
        None => master_cost,
        Some(slot) => bus_tracks
            .get(slot)
            .and_then(|index| through.get(*index))
            .copied()
            .unwrap_or(master_cost),
    };
    out.clear();
    out.push(resolve(track.output));
    out.extend(track.sends.iter().map(|send| resolve(Some(send.target))));
}

/// The longest path from every track to the output, adding each strip's own `cost` along the way.
///
/// One walk in reverse routing order is enough: the order puts a bus after everything feeding it,
/// so walking it backwards reaches every destination before the tracks that name it.
pub(super) fn longest_paths(
    tracks: &[RenderTrack],
    bus_tracks: &[usize],
    order: &[usize],
    cost: impl Fn(&RenderStrip) -> usize,
    master_cost: usize,
) -> Vec<usize> {
    let mut through = vec![0usize; tracks.len()];
    let mut edges = Vec::new();
    for &index in order.iter().rev() {
        let Some(track) = tracks.get(index) else {
            continue;
        };
        edge_costs(track, bus_tracks, &through, master_cost, &mut edges);
        let downstream = edges.iter().copied().max().unwrap_or(master_cost);
        through[index] = cost(&track.strip).saturating_add(downstream);
    }
    through
}

/// Works out every delay the graph needs so that nothing arrives anywhere out of step.
///
/// The rule is one sentence long: **a signal's whole journey to the output must take the same
/// time however it goes.** A track's chain is only part of that journey — the bus it feeds has a
/// chain too, and so does the master — so what has to be equalised is the *path*, not the chain.
///
/// So each track is held back by the difference between the longest path in the graph and its own,
/// which is what puts the sources in step; and each outgoing *edge* is held back by the difference
/// between the track's longest onward path and that edge's, which is what puts the copies of one
/// track in step with each other. A track feeding the master dry while sending to a bus that looks
/// ahead is exactly the case the second one exists for: without it the dry and the wet arrive at
/// different times and comb-filter each other.
///
/// A bus is never held back itself: everything reaching it was lined up on the way in.
pub(super) fn plan_latency(
    tracks: &[RenderTrack],
    bus_tracks: &[usize],
    order: &[usize],
    master_latency: usize,
) -> LatencyPlan {
    let through = longest_paths(
        tracks,
        bus_tracks,
        order,
        RenderStrip::latency_frames,
        master_latency,
    );

    let mut edges = Vec::with_capacity(tracks.len());
    let mut costs = Vec::new();
    for track in tracks {
        edge_costs(track, bus_tracks, &through, master_latency, &mut costs);
        let longest = costs.iter().copied().max().unwrap_or(0);
        edges.push(costs.iter().map(|cost| longest - cost).collect());
    }

    // Only a track with material of its own is a source to be lined up. A bus has none, and is
    // already in step with whatever reached it.
    let is_source = |track: &RenderTrack| !matches!(track.source, RenderSource::Bus { .. });
    let total = tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| is_source(track))
        .map(|(index, _)| through[index])
        .max()
        .unwrap_or(master_latency);
    let node = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| match is_source(track) {
            true => total.saturating_sub(through[index]),
            false => 0,
        })
        .collect();

    LatencyPlan { total, node, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::RenderGraph;
    use crate::graph::tests::quarter_note_project;
    use crate::testkit;
    use auris_core::project::{AudioSourceBank, Project};

    #[test]
    fn every_track_is_held_back_to_the_longest_chain() {
        let mut project = Project::new("PDC", 48_000.0);
        let late = project.add_instrument_track("Late", testkit::TONE_ID);
        project.add_instrument_track("Plain", testkit::TONE_ID);
        project.add_effect(Some(late), testkit::LOOKAHEAD_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        // The track that runs late needs no delay; the one that does not has to wait for it.
        assert_eq!(graph.tracks()[0].compensation_frames(), 0);
        assert_eq!(
            graph.tracks()[1].compensation_frames(),
            testkit::LOOKAHEAD_FRAMES
        );
        assert_eq!(graph.latency_frames(), testkit::LOOKAHEAD_FRAMES);
        assert!(!graph.latency_is_stale());
    }

    #[test]
    fn latencies_add_up_along_a_chain_and_the_master_adds_on_top() {
        let mut project = Project::new("PDC", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::LOOKAHEAD_ID);
        project.add_effect(Some(track), testkit::LOOKAHEAD_ID);
        project.add_effect(None, testkit::LOOKAHEAD_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(
            graph.tracks()[0].strip().latency_frames(),
            2 * testkit::LOOKAHEAD_FRAMES
        );
        assert_eq!(graph.latency_frames(), 3 * testkit::LOOKAHEAD_FRAMES);
    }

    #[test]
    fn a_bypassed_effect_delays_nothing_and_needs_no_compensation() {
        let mut project = Project::new("PDC", 48_000.0);
        let late = project.add_instrument_track("Late", testkit::TONE_ID);
        project.add_instrument_track("Plain", testkit::TONE_ID);
        project.add_effect(Some(late), testkit::LOOKAHEAD_ID);
        project.tracks[0].mixer.effects[0].enabled = false;

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.latency_frames(), 0);
        assert_eq!(graph.tracks()[1].compensation_frames(), 0);
    }

    #[test]
    fn a_graph_with_nothing_to_compensate_allocates_no_delay_lines() {
        let project = quarter_note_project();
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.latency_frames(), 0);
        assert_eq!(graph.tracks()[0].compensation_frames(), 0);
        assert!(graph.tracks()[0].delay.lines.is_empty());
    }

    #[test]
    fn a_delay_line_hands_samples_back_a_fixed_number_of_frames_later() {
        let mut delay = LatencyDelay::new(3, 2);
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![1.0, 2.0, 3.0, 4.0]; 2], 48_000.0).expect("planar");
        delay.process(&mut buffer);
        // Three frames of the empty line come out first, then the input begins.
        assert_eq!(buffer.channel(0), &[0.0, 0.0, 0.0, 1.0]);
        assert_eq!(buffer.channel(1), &[0.0, 0.0, 0.0, 1.0]);

        let mut next =
            AudioBuffer::from_planar(vec![vec![5.0, 6.0, 7.0, 8.0]; 2], 48_000.0).expect("planar");
        delay.process(&mut next);
        assert_eq!(next.channel(0), &[2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn a_delay_line_reads_the_same_however_the_blocks_are_cut() {
        let feed: Vec<f32> = (0..64).map(|index| index as f32).collect();
        let run = |block: usize| {
            let mut delay = LatencyDelay::new(7, 2);
            let mut out = Vec::new();
            for chunk in feed.chunks(block) {
                let mut buffer =
                    AudioBuffer::from_planar(vec![chunk.to_vec(); 2], 48_000.0).expect("planar");
                delay.process(&mut buffer);
                out.extend_from_slice(buffer.channel(0));
            }
            out
        };
        assert_eq!(run(64), run(5));
        assert_eq!(run(64), run(1));
    }
}
