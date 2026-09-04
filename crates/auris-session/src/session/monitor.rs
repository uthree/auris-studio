//! Hearing the input through the mix, before it is a recording.
//!
//! Recording answers "keep what I played". Monitoring answers "let me hear what I am playing",
//! and the two are independent: somebody sets a level, plays along to the song and decides where
//! to come in, all without a take running, and somebody else records through an interface that is
//! monitoring in hardware and never turns this on at all.
//!
//! # Why it is not simply on
//!
//! It costs latency that hardware does not. The signal has to reach the output callback through a
//! ring the input callback fills, which is a buffer on top of both devices' own — see
//! [`auris_engine::monitor`] for the figure and why it is what it is. An interface with direct
//! monitoring beats every software path there will ever be, and somebody using one who also had
//! this on would hear themselves twice, slightly apart, which is worse than either alone.
//!
//! So it is a switch, off by default, per the same rule as everything else here that trades
//! something away: a feature that costs is switched on rather than inferred.
//!
//! # What it does to the device
//!
//! Opens it. The input device is open while a take is running *or* somebody is monitoring, and is
//! closed the moment neither is true — a live microphone is a light on the menu bar and a battery
//! cost, and holding one open against the next take would be both.
//!
//! That is why a take is a *phase* of an open device rather than the device's whole life, and why
//! [`auris_engine::Capture`] grew [`begin_take`](auris_engine::Capture::begin_take): stopping a
//! take must not take the monitor down with it.

use auris_core::TrackId;
use auris_engine::MonitorRing;

use super::Session;
use crate::error::SessionError;

/// How the monitor is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorStatus {
    /// The device being listened to.
    pub device: String,
    /// The tracks the input is playing through, in the order they were switched on.
    pub tracks: Vec<TrackId>,
    /// `false` once the device has disappeared out from under it.
    pub running: bool,
    /// Times the monitor has had to jump to catch up with the input, each of them a heard gap.
    ///
    /// A handful over a long session is two clocks drifting and is nothing to act on. A steady
    /// stream of them is a machine that cannot keep up with the block size it has been given, and
    /// the person at the keyboard is the only one who can do anything about that.
    pub rebuffers: u64,
}

impl Session {
    /// How many tracks may be monitored at once.
    ///
    /// A ring each, all of them made when the device opens because the input callback may not
    /// allocate — [`auris_engine::MONITOR_SLOTS`] is the number and the reason.
    pub const MAX_MONITORS: usize = auris_engine::MONITOR_SLOTS;

    /// The tracks the live input is being played through, in the order they were switched on.
    pub fn monitored_tracks(&self) -> &[TrackId] {
        &self.monitored
    }

    /// `true` while `track` is one of them.
    pub fn is_monitored(&self, track: TrackId) -> bool {
        self.monitored.contains(&track)
    }

    /// `true` while the input is being played through the mix at all.
    pub fn monitoring(&self) -> bool {
        !self.monitored.is_empty()
    }

    /// Plays the live input through `track`, or stops doing so.
    ///
    /// Opens the input device if nothing had it open, and closes it when nothing wants it any
    /// more. Only an audio track: an instrument track has no signal path a live input could take.
    ///
    /// Idempotent, which is what lets a caller call it with whatever it decides the target should
    /// be and pay a rebuild only when the answer actually changed.
    ///
    /// Several tracks at once, up to [`Self::MAX_MONITORS`], because a band monitors as a band —
    /// each track hears the channels *it* is armed to, so the singer hears the microphone their
    /// own take will be made of. Past the limit this refuses rather than quietly listening to
    /// fewer: a monitor nobody can hear is worse than one that said why.
    pub fn set_track_monitoring(&mut self, track: TrackId, on: bool) -> Result<(), SessionError> {
        let found = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        if found.kind.as_audio().is_none() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: found.kind.label(),
                expected: "Audio",
            });
        }
        if on == self.is_monitored(track) {
            return Ok(());
        }
        if on {
            if self.monitored.len() >= Self::MAX_MONITORS {
                return Err(SessionError::TooManyMonitors {
                    limit: Self::MAX_MONITORS,
                });
            }
            self.open_input()?;
            self.monitored.push(track);
        } else {
            self.monitored.retain(|held| *held != track);
        }
        self.publish_monitors();
        // The taps live in the graph, and the graph is what has to be told which tracks.
        self.rebuild_graph();
        self.close_input_if_idle();
        Ok(())
    }

    /// Stops monitoring every track that was.
    pub fn stop_monitoring(&mut self) {
        if self.monitored.is_empty() {
            return;
        }
        self.monitored.clear();
        self.publish_monitors();
        self.rebuild_graph();
        self.close_input_if_idle();
    }

    /// How the monitor is doing, for a meter and a warning.
    pub fn monitor_status(&self) -> Option<MonitorStatus> {
        if self.monitored.is_empty() {
            return None;
        }
        let capture = self.input.as_ref()?;
        Some(MonitorStatus {
            device: capture.name().to_string(),
            tracks: self.monitored.clone(),
            running: capture.is_running(),
            rebuffers: capture.monitor_rebuffers(),
        })
    }

    /// Points each slot at the track it carries and silences the rest.
    ///
    /// A slot per monitored track, in the order they were switched on, at the channels that track
    /// would record from — so what is heard is what would be kept. Switching one on re-seats its
    /// reader at the live edge rather than resuming, which is what stops a monitor turned off for
    /// a minute coming back a minute behind.
    pub(super) fn publish_monitors(&self) {
        let Some(capture) = self.input.as_ref() else {
            return;
        };
        for slot in 0..Self::MAX_MONITORS {
            let Some(ring) = capture.monitor(slot) else {
                continue;
            };
            match self.monitored.get(slot) {
                Some(track) => {
                    let input = self
                        .track_arm(*track)
                        .unwrap_or_else(|| super::InputChannels::stereo(0));
                    ring.set_source_channels(input.first, input.count);
                    // Only where it was not already listening. Switching a ring on re-seats its
                    // reader at the live edge, which is right for one that has been off and is a
                    // heard gap for one that has not — and this runs whenever an arm changes, so
                    // arming a track would otherwise drop out every monitor in the room.
                    if !ring.is_enabled() {
                        ring.set_enabled(true);
                    }
                }
                None => ring.set_enabled(false),
            }
        }
    }

    /// The rings the graph should read, paired with the tracks they come out of.
    pub(super) fn monitor_taps(&self) -> Vec<(std::sync::Arc<MonitorRing>, TrackId)> {
        let Some(capture) = self.input.as_ref() else {
            return Vec::new();
        };
        self.monitored
            .iter()
            .enumerate()
            .filter_map(|(slot, track)| Some((capture.monitor(slot)?, *track)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::error::SessionError;
    use crate::{Session, SessionOptions};

    fn session() -> Session {
        Session::new(SessionOptions::headless()).expect("a headless session")
    }

    #[test]
    fn only_an_audio_track_can_be_monitored() {
        // An instrument track has no signal path a live input could take, and pointing a monitor
        // at one would either be silent or need a rule about which of the two it played.
        let mut session = session();
        let synth = session.add_default_instrument_track("Synth").unwrap();
        assert!(matches!(
            session.set_track_monitoring(synth, true),
            Err(SessionError::WrongTrackKind { .. })
        ));
        assert!(!session.monitoring());

        assert!(matches!(
            session.set_track_monitoring(auris_core::TrackId(9_999), true),
            Err(SessionError::UnknownTrack(_))
        ));
    }

    #[test]
    fn several_tracks_can_be_monitored_at_once_and_each_is_switched_on_its_own() {
        // A band monitors as a band: everybody hears themselves through their own track, at that
        // track's fader and through its effects.
        let mut session = session();
        let vocal = session.add_audio_track("Vocal");
        let guitar = session.add_audio_track("Guitar");

        session.set_track_monitoring(vocal, true).unwrap();
        session.set_track_monitoring(guitar, true).unwrap();
        assert_eq!(session.monitored_tracks(), [vocal, guitar]);
        assert!(session.is_monitored(vocal) && session.is_monitored(guitar));

        // Switching one off leaves the other listening, which is the whole point of a list.
        session.set_track_monitoring(vocal, false).unwrap();
        assert_eq!(session.monitored_tracks(), [guitar]);
        assert!(session.monitoring());

        session.stop_monitoring();
        assert!(!session.monitoring());
        assert_eq!(session.monitor_status(), None);
    }

    #[test]
    fn arming_a_track_does_not_drop_out_the_monitors() {
        // Pointing the rings at their channels runs whenever an arm changes, and switching a ring
        // on re-seats its reader — so a monitor that was already listening must be left alone, or
        // arming a track would be a heard gap in everybody's headphones.
        let mut session = session();
        let vocal = session.add_audio_track("Vocal");
        let guitar = session.add_audio_track("Guitar");
        session.set_track_monitoring(vocal, true).unwrap();
        session.arm_track(guitar, None).unwrap();
        // Nothing to assert on but the state that survived it: the ring is the engine's and there
        // is no device behind a headless session. What this pins is that the call is made and
        // does not disturb what was listening.
        assert_eq!(session.monitored_tracks(), [vocal]);
        assert!(session.is_monitored(vocal));
    }

    #[test]
    fn switching_one_on_twice_is_not_two_monitors() {
        // A frontend calls this with whatever it decides the answer is, so the no-change call is
        // the common one and must not stack up rings on one track.
        let mut session = session();
        let vocal = session.add_audio_track("Vocal");
        session.set_track_monitoring(vocal, true).unwrap();
        session.set_track_monitoring(vocal, true).unwrap();
        assert_eq!(session.monitored_tracks(), [vocal]);

        session.set_track_monitoring(vocal, false).unwrap();
        session.set_track_monitoring(vocal, false).unwrap();
        assert!(session.monitored_tracks().is_empty());
    }

    #[test]
    fn more_tracks_than_there_are_rings_is_refused_rather_than_half_done() {
        // Every ring is made when the device opens, because the input callback may not make one
        // while it is running. A monitor nobody can hear is worse than one that said why.
        let mut session = session();
        for index in 0..=Session::MAX_MONITORS {
            let track = session.add_audio_track(format!("Take {index}"));
            let result = session.set_track_monitoring(track, true);
            if index < Session::MAX_MONITORS {
                result.expect("within the limit");
            } else {
                assert!(matches!(result, Err(SessionError::TooManyMonitors { .. })));
            }
        }
        assert_eq!(session.monitored_tracks().len(), Session::MAX_MONITORS);
    }

    #[test]
    fn monitoring_is_off_until_it_is_asked_for() {
        // It costs latency an interface's own monitoring does not, and somebody using one who
        // also had this on would hear themselves twice, slightly apart.
        let session = session();
        assert!(!session.monitoring());
        assert!(session.monitored_tracks().is_empty());
        assert_eq!(session.monitor_status(), None);
    }

    #[test]
    fn turning_it_off_when_it_was_never_on_changes_nothing() {
        // A frontend re-points this from wherever it decides its target, so the no-change call is
        // the common one and must not open a device or rebuild a graph to do nothing.
        let mut session = session();
        let track = session.add_audio_track("Take");
        session.set_track_monitoring(track, false).unwrap();
        session.stop_monitoring();
        assert!(!session.monitoring());
    }

    #[test]
    fn deleting_a_track_takes_its_monitor_with_it() {
        let mut session = session();
        let track = session.add_audio_track("Take");
        session.set_track_monitoring(track, true).unwrap();

        session.remove_track(track).unwrap();

        assert!(session.monitored_tracks().is_empty());
        assert!(!session.monitoring());
    }
}
