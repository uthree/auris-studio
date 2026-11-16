//! The effects that are bodies: what they are doing, and keeping them tuned to the song.
//!
//! A resonating effect publishes a [`ResonanceFrame`] the way a physical instrument publishes
//! its motion, and the window reads it through here by effect slot. Such an effect may also be
//! tuned to the song's key and chords, which it was handed as a timeline when the graph was
//! built. Harmony edits deliberately leave the engine alone — nothing else that plays depends
//! on them — so while a graph holds such an effect, [`Session::poll`] notices a changed harmony
//! and rebuilds it, which is the one cost of following the song and is paid only by a project
//! that does.

use auris_core::EffectSlotId;
use auris_core::resonance::ResonanceFrame;
use auris_engine::RenderGraph;

use super::Session;

impl Session {
    /// Collects the resonator readers of a freshly built graph, by slot, and remembers the
    /// harmony its followers were tuned to.
    pub(super) fn collect_resonance_monitors(&mut self, graph: &RenderGraph) {
        for monitor in self.resonance_monitors.values() {
            monitor.watch(false);
        }
        let mut monitors = std::collections::HashMap::new();
        for (track, built) in self.project.tracks.iter().zip(graph.tracks()) {
            for (index, slot) in track.mixer.effects.iter().enumerate() {
                if let Some(monitor) = built.strip().effect_resonance_monitor(index) {
                    monitors.insert(slot.id, monitor);
                }
            }
        }
        for (index, slot) in self.project.master.effects.iter().enumerate() {
            if let Some(monitor) = graph.master().effect_resonance_monitor(index) {
                monitors.insert(slot.id, monitor);
            }
        }
        self.resonance_monitors = monitors;
        self.refresh_resonance_watch();
        self.tuned_harmony = graph
            .follows_harmony()
            .then(|| self.project.harmony.clone());
    }

    fn refresh_resonance_watch(&self) {
        for (slot, monitor) in &self.resonance_monitors {
            monitor.watch(Some(*slot) == self.resonance_slot);
        }
    }

    /// Rebuilds the graph when an effect in it follows the harmony and the harmony has moved
    /// since it was tuned. Cheap when nothing follows it: one comparison against `None`.
    pub(super) fn retune_resonators(&mut self) {
        if self
            .tuned_harmony
            .as_ref()
            .is_some_and(|tuned| *tuned != self.project.harmony)
            && self.transaction.is_none()
        {
            self.rebuild_graph();
        }
    }

    /// Whether the effect in `slot` is a resonator with something to draw.
    pub fn has_effect_resonance(&self, slot: EffectSlotId) -> bool {
        self.resonance_monitors.contains_key(&slot)
    }

    /// Asks the resonator in `slot` to publish what it is doing, and every other one to stop;
    /// `None` stops them all.
    pub fn watch_effect_resonance(&mut self, slot: Option<EffectSlotId>) {
        self.resonance_slot = slot;
        self.refresh_resonance_watch();
    }

    /// The resonator in `slot` as it last published itself, without waiting for the audio
    /// thread. `None` before its first frame, and for a slot that is not a resonator.
    pub fn effect_resonance(&self, slot: EffectSlotId) -> Option<ResonanceFrame> {
        self.resonance_monitors.get(&slot)?.read()
    }

    /// Readers by slot, for tests.
    #[cfg(test)]
    pub(super) fn resonance_monitor(
        &self,
        slot: EffectSlotId,
    ) -> Option<std::sync::Arc<auris_core::resonance::ResonanceMonitor>> {
        self.resonance_monitors.get(&slot).cloned()
    }
}

#[cfg(test)]
mod tests {
    use auris_core::Ticks;

    use crate::session::fixtures::session;

    #[test]
    fn a_resonator_on_a_track_is_found_by_its_slot_and_watched_only_while_shown() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let body = session.add_effect(Some(track), "auris.fx.body").unwrap();
        let reverb = session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        assert!(session.has_effect_resonance(body));
        assert!(!session.has_effect_resonance(reverb));
        let monitor = session.resonance_monitor(body).unwrap();
        assert!(!monitor.is_watched());
        session.watch_effect_resonance(Some(body));
        assert!(monitor.is_watched());
        // A rebuild hands out a fresh reader, and the watch carries over to it.
        let strings = session.add_effect(None, "auris.fx.sympathetic").unwrap();
        assert!(session.has_effect_resonance(strings));
        assert!(!monitor.is_watched());
        assert!(session.resonance_monitor(body).unwrap().is_watched());
        session.watch_effect_resonance(None);
        assert!(!session.resonance_monitor(body).unwrap().is_watched());
    }

    #[test]
    fn a_harmony_edit_retunes_the_strings_and_leaves_other_projects_alone() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        assert!(session.tuned_harmony.is_none());
        session
            .add_effect(Some(track), "auris.fx.sympathetic")
            .unwrap();
        assert_eq!(
            session.tuned_harmony.as_ref(),
            Some(&session.project.harmony)
        );
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .unwrap();
        assert_ne!(
            session.tuned_harmony.as_ref(),
            Some(&session.project.harmony)
        );
        session.poll();
        assert_eq!(
            session.tuned_harmony.as_ref(),
            Some(&session.project.harmony)
        );
    }
}
