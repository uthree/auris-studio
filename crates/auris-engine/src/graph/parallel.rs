//! Dependency generations for offline track preparation.
//!
//! The renderer still commits each generation's outputs in the graph's original routing order.
//! Only source/strip preparation is parallel; sends, fader/output mixing and sidechain publication
//! remain ordered by the existing renderer.

use super::{RenderSource, RenderTrack};

/// A deterministic offline schedule for the independent preparation phase.
pub(crate) struct OfflineRenderPlan {
    /// Track indices grouped by dependency level, in their original routing order.
    pub(crate) generations: Vec<Vec<usize>>,
    /// Generation number for each track index.
    pub(crate) generation_for_track: Vec<usize>,
    /// Largest number of worker-eligible tracks in any one generation.
    pub(crate) max_parallel_width: usize,
}

/// Builds dependency generations from the graph's existing routing order.
///
/// Bus writers are predecessors of bus tracks, and sidechain sources are predecessors of the
/// tracks whose effects read them. An invalid/incomplete order returns an empty plan, allowing the
/// caller to retain the existing fully serial renderer.
impl OfflineRenderPlan {
    /// Builds dependency generations from the graph's existing routing order.
    pub(crate) fn build(
        tracks: &[RenderTrack],
        bus_tracks: &[usize],
        tap_tracks: &[usize],
        order: &[usize],
    ) -> Self {
        if order.is_empty() {
            return empty_plan(tracks.len());
        }
        let dependencies = dependencies(tracks, bus_tracks, tap_tracks);
        let mut plan = generations_from_dependencies(tracks.len(), order, &dependencies)
            .unwrap_or_else(|| empty_plan(tracks.len()));
        plan.max_parallel_width = plan
            .generations
            .iter()
            .map(|generation| {
                generation
                    .iter()
                    .filter(|&&index| tracks[index].parallel_safe)
                    .count()
            })
            .max()
            .unwrap_or(0);
        plan
    }
}

fn empty_plan(track_count: usize) -> OfflineRenderPlan {
    OfflineRenderPlan {
        generations: Vec::new(),
        generation_for_track: vec![0; track_count],
        max_parallel_width: 0,
    }
}

fn generations_from_dependencies(
    track_count: usize,
    order: &[usize],
    dependencies: &[Vec<usize>],
) -> Option<OfflineRenderPlan> {
    if order.is_empty() {
        return None;
    }
    let mut position = vec![usize::MAX; track_count];
    for (offset, &track) in order.iter().enumerate() {
        if track >= track_count || position[track] != usize::MAX {
            return None;
        }
        position[track] = offset;
    }
    if position.contains(&usize::MAX) {
        return None;
    }

    let mut generation_for_track = vec![0usize; track_count];
    for &reader in order {
        for &writer in dependencies.get(reader)? {
            if writer >= track_count || position[writer] >= position[reader] {
                return None;
            }
            generation_for_track[reader] =
                generation_for_track[reader].max(generation_for_track[writer].saturating_add(1));
        }
    }
    let generation_count = generation_for_track.iter().copied().max().unwrap_or(0) + 1;
    let mut generations = vec![Vec::new(); generation_count];
    for &track in order {
        generations[generation_for_track[track]].push(track);
    }
    Some(OfflineRenderPlan {
        generations,
        generation_for_track,
        max_parallel_width: 0,
    })
}

fn dependencies(
    tracks: &[RenderTrack],
    bus_tracks: &[usize],
    tap_tracks: &[usize],
) -> Vec<Vec<usize>> {
    let mut writers_by_bus = vec![Vec::new(); bus_tracks.len()];
    let mut writers_by_tap = vec![Vec::new(); tap_tracks.len()];
    for (index, track) in tracks.iter().enumerate() {
        for output in &track.outputs {
            if let Some(slot) = output.target
                && let Some(writers) = writers_by_bus.get_mut(slot)
            {
                writers.push(index);
            }
        }
        for send in &track.sends {
            if let Some(writers) = writers_by_bus.get_mut(send.target) {
                writers.push(index);
            }
        }
        if let Some(tap) = track.tap
            && let Some(writers) = writers_by_tap.get_mut(tap)
        {
            writers.push(index);
        }
    }

    tracks
        .iter()
        .map(|track| {
            let mut writers = Vec::new();
            if let RenderSource::Bus { input } = &track.source {
                writers.extend(writers_by_bus.get(*input).into_iter().flatten().copied());
            }
            for tap in track.strip.sidechain.iter().flatten() {
                writers.extend(writers_by_tap.get(*tap).into_iter().flatten().copied());
            }
            writers.sort_unstable();
            writers.dedup();
            writers
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use auris_core::project::{AudioSourceBank, AuxSend, Output, Project};

    fn plan(order: &[usize], deps: &[(usize, &[usize])]) -> OfflineRenderPlan {
        let mut dependencies = vec![Vec::new(); order.len()];
        for &(reader, writers) in deps {
            dependencies[reader] = writers.to_vec();
        }
        generations_from_dependencies(order.len(), order, &dependencies).unwrap()
    }

    #[test]
    fn independent_tracks_share_generation_zero() {
        let result = plan(&[0, 1, 2], &[]);
        assert_eq!(result.generations, [vec![0, 1, 2]]);
    }

    #[test]
    fn dependencies_raise_only_the_reader_generation() {
        let result = plan(&[0, 1, 2, 3], &[(2, &[0]), (3, &[1])]);
        assert_eq!(result.generations, [vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn chained_dependencies_form_stable_generations() {
        let result = plan(&[0, 1, 2, 3], &[(1, &[0]), (2, &[1]), (3, &[0])]);
        assert_eq!(result.generations, [vec![0], vec![1, 3], vec![2]]);
        assert_eq!(result.generation_for_track, [0, 1, 2, 1]);
    }

    #[test]
    fn a_backward_dependency_is_rejected() {
        let mut project = Project::new("Cycle", 48_000.0);
        let source = project.add_instrument_track("Source", testkit::TONE_ID);
        let bus = project.add_bus_track("Bus");
        project.track_mut(source).unwrap().outputs = vec![Output::Bus(bus)];
        let graph = crate::graph::RenderGraph::build(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            512,
        );
        let source_index = project.track_index(source).unwrap();
        let bus_index = project.track_index(bus).unwrap();
        let order = [bus_index, source_index];
        let plan = OfflineRenderPlan::build(&graph.tracks, &graph.bus_tracks, &[], &order);
        assert!(plan.generations.is_empty());
    }

    #[test]
    fn production_extraction_orders_bus_send_and_sidechain_readers() {
        let mut project = Project::new("Routes", 48_000.0);
        let source = project.add_instrument_track("Source", testkit::TONE_ID);
        let listener = project.add_instrument_track("Listener", testkit::TONE_ID);
        let bus = project.add_bus_track("Bus");
        project.track_mut(source).unwrap().outputs = vec![Output::Bus(bus)];
        project.track_mut(listener).unwrap().outputs = vec![Output::Bus(bus)];
        let send_id = project.next_send_id();
        project.track_mut(source).unwrap().sends.push(AuxSend {
            id: send_id,
            target: bus,
            level_db: 0.0,
            pre_fader: false,
        });
        let effect = project
            .add_effect(Some(listener), testkit::KEYED_ID)
            .unwrap();
        project
            .track_mut(listener)
            .unwrap()
            .mixer
            .effects
            .iter_mut()
            .find(|slot| slot.id == effect)
            .unwrap()
            .sidechain = Some(source);
        let graph = crate::graph::RenderGraph::build(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            512,
        );
        let mut tap_tracks = vec![usize::MAX; graph.sidechain_taps.len()];
        for (index, track) in graph.tracks.iter().enumerate() {
            if let Some(tap) = track.tap {
                tap_tracks[tap] = index;
            }
        }
        let plan =
            OfflineRenderPlan::build(&graph.tracks, &graph.bus_tracks, &tap_tracks, &graph.order);
        assert!(!plan.generations.is_empty());
        let source_index = project.track_index(source).unwrap();
        let listener_index = project.track_index(listener).unwrap();
        let bus_index = project.track_index(bus).unwrap();
        assert!(plan.generation_for_track[source_index] < plan.generation_for_track[bus_index]);
        assert!(
            plan.generation_for_track[source_index] < plan.generation_for_track[listener_index]
        );
    }
}
