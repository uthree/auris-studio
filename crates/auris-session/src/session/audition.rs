//! Timeline ranges for offline audition and export.

use super::Session;
use auris_core::time::Ticks;
use auris_engine::OfflineOptions;

impl Session {
    /// Converts a nonempty range inside the arrangement into offline options.
    ///
    /// Both endpoints use the document's tempo map, including changes within the range.
    /// The end is exclusive. Returns `None` for ranges outside the document.
    pub fn render_range_options(
        &self,
        from: Ticks,
        to: Ticks,
        include_tail: bool,
    ) -> Option<OfflineOptions> {
        if from < Ticks::ZERO || to <= from || to > self.project.end_tick() {
            return None;
        }
        let rate = self.project.sample_rate;
        let start = self.project.tempo_map.ticks_to_samples(from, rate).raw();
        let end = self.project.tempo_map.ticks_to_samples(to, rate).raw();
        Some(OfflineOptions {
            include_tail,
            ..OfflineOptions::whole_project().with_range(start, end)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    #[test]
    fn ranges_follow_tempo_changes_and_reject_outside_the_arrangement() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .add_midi_clip(track, "Range", Ticks::ZERO, Ticks::QUARTER * 12)
            .unwrap();
        session.set_tempo_point(Ticks::ZERO, 120.0);
        session.set_tempo_point(Ticks::QUARTER * 4, 60.0);
        let range = session
            .render_range_options(Ticks::QUARTER * 2, Ticks::QUARTER * 6, false)
            .unwrap();
        let rate = session.project().sample_rate as u64;
        assert_eq!(range.start_frames, rate);
        assert_eq!(range.end_frames, Some(rate * 4));
        assert!(!range.include_tail);
        assert!(
            session
                .render_range_options(Ticks::ZERO, Ticks::QUARTER * 13, false)
                .is_none()
        );
    }
}
