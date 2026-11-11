//! Optional, graph-owned peak meters for individual mixer connections.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use auris_core::{Destination, EffectSlotId, Project, Tap, TrackId};

use crate::MeterBank;

/// Stable identity of an audio tap, independent of render indices and plugin instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalRoute {
    /// Audio entering an insert, including an insert that is bypassed.
    InsertInput(Option<TrackId>, EffectSlotId),
    /// One of a track's outputs, after its level and its compensation delay.
    Output(TrackId, Destination, Tap),
    /// Key audio consumed by an enabled insert that accepts sidechains.
    Sidechain(Option<TrackId>, EffectSlotId),
}

/// Lock-free connection readings prepared with one render graph.
///
/// The building thread retains an `Arc` before handing the graph to audio. Replacing a graph
/// gives its reader a new, silent bank, so deleted routes cannot inherit another route's level.
/// Only audio writes levels; reading neither consumes peaks nor changes the document.
#[derive(Debug, Default)]
pub struct SignalMeters {
    routes: HashMap<SignalRoute, usize>,
    levels: Vec<AtomicU32>,
    watched: AtomicBool,
}

impl SignalMeters {
    pub(crate) fn for_project(project: &Project) -> Self {
        let mut routes = HashMap::new();
        for (owner, mixer) in project
            .tracks
            .iter()
            .map(|track| (Some(track.id), &track.mixer))
            .chain(std::iter::once((None, &project.master)))
        {
            for effect in &mixer.effects {
                let index = routes.len();
                routes.insert(SignalRoute::InsertInput(owner, effect.id), index);
                if project.sidechain(effect.id).is_some() {
                    let index = routes.len();
                    routes.insert(SignalRoute::Sidechain(owner, effect.id), index);
                }
            }
        }
        for track in &project.tracks {
            for output in project.routing.outputs(track.id) {
                let index = routes.len();
                routes
                    .entry(SignalRoute::Output(track.id, output.to, output.tap))
                    .or_insert(index);
            }
        }
        Self {
            levels: (0..routes.len()).map(|_| AtomicU32::new(0)).collect(),
            routes,
            watched: AtomicBool::new(false),
        }
    }

    /// Enables measurement while a reader is visible. Disabled banks cost no sample scans.
    pub fn watch(&self, watched: bool) {
        if self.watched.swap(watched, Ordering::Relaxed) != watched {
            self.reset();
        }
    }

    /// Whether the renderer should measure connections for this graph.
    pub fn is_watched(&self) -> bool {
        self.watched.load(Ordering::Relaxed)
    }

    /// Held linear peak for this exact connection, or silence when absent or unwatched.
    /// Uses the same instantaneous attack and decay as [`MeterBank`].
    pub fn peak(&self, route: SignalRoute) -> f32 {
        if !self.is_watched() {
            return 0.0;
        }
        self.slot(route)
            .and_then(|index| self.levels.get(index))
            .map_or(0.0, |slot| f32::from_bits(slot.load(Ordering::Relaxed)))
    }

    /// Clears all readings without changing the watched state; safe on the audio thread.
    pub fn reset(&self) {
        for slot in &self.levels {
            slot.store(0, Ordering::Relaxed);
        }
    }

    pub(crate) fn slot(&self, route: SignalRoute) -> Option<usize> {
        self.routes.get(&route).copied()
    }

    // Decay once per callback, including slots skipped by mute or bypass. Segments only raise
    // readings, so a short peak before a loop boundary survives the rest of the callback.
    pub(crate) fn begin_block(&self, frames: usize, sample_rate: f64) {
        let decay = MeterBank::decay(frames, sample_rate);
        for slot in &self.levels {
            let level = f32::from_bits(slot.load(Ordering::Relaxed));
            slot.store((level * decay).to_bits(), Ordering::Relaxed);
        }
    }

    pub(crate) fn observe(&self, index: Option<usize>, peak: f32) {
        if peak.is_finite()
            && let Some(slot) = index.and_then(|index| self.levels.get(index))
        {
            let previous = f32::from_bits(slot.load(Ordering::Relaxed));
            slot.store(previous.max(peak.abs()).to_bits(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
#[path = "signal_meter_tests.rs"]
mod tests;
