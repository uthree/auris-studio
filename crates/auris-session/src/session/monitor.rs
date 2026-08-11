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
    /// The track the input is playing through.
    pub track: TrackId,
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
    /// The track the live input is being played through, if any.
    pub fn monitored_track(&self) -> Option<TrackId> {
        self.monitored
    }

    /// `true` while the input is being played through the mix.
    pub fn monitoring(&self) -> bool {
        self.monitored.is_some()
    }

    /// Plays the live input through `track`, or stops with `None`.
    ///
    /// Opens the input device if nothing had it open, and closes it when nothing wants it any
    /// more. Only an audio track: an instrument track has no signal path a live input could take.
    ///
    /// Idempotent, which is what lets a caller re-point it from wherever it decides the target —
    /// a frontend whose selection has moved calls this with the new track and pays a rebuild only
    /// when the answer actually changed.
    pub fn set_monitoring(&mut self, track: Option<TrackId>) -> Result<(), SessionError> {
        if let Some(id) = track {
            let found = self
                .project
                .track(id)
                .ok_or(SessionError::UnknownTrack(id.0))?;
            if found.kind.as_audio().is_none() {
                return Err(SessionError::WrongTrackKind {
                    id: id.0,
                    actual: found.kind.label(),
                    expected: "Audio",
                });
            }
        }
        if track == self.monitored {
            return Ok(());
        }
        if track.is_some() {
            self.open_input()?;
        }
        self.monitored = track;
        if let Some(ring) = self.monitor_ring() {
            // Switching on re-seats the reader at the live edge rather than resuming, so a monitor
            // turned off for a minute does not come back a minute behind.
            ring.set_enabled(track.is_some());
        }
        // The tap lives in the graph, and the graph is what has to be told which track.
        self.rebuild_graph();
        self.close_input_if_idle();
        Ok(())
    }

    /// How the monitor is doing, for a meter and a warning.
    pub fn monitor_status(&self) -> Option<MonitorStatus> {
        let track = self.monitored?;
        let capture = self.input.as_ref()?;
        Some(MonitorStatus {
            device: capture.name().to_string(),
            track,
            running: capture.is_running(),
            rebuffers: capture.monitor().rebuffers(),
        })
    }

    /// The ring the open input writes into, if there is one.
    pub(super) fn monitor_ring(&self) -> Option<std::sync::Arc<MonitorRing>> {
        self.input.as_ref().map(|capture| capture.monitor())
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
            session.set_monitoring(Some(synth)),
            Err(SessionError::WrongTrackKind { .. })
        ));
        assert!(!session.monitoring());

        assert!(matches!(
            session.set_monitoring(Some(auris_core::TrackId(9_999))),
            Err(SessionError::UnknownTrack(_))
        ));
    }

    #[test]
    fn monitoring_is_off_until_it_is_asked_for() {
        // It costs latency an interface's own monitoring does not, and somebody using one who
        // also had this on would hear themselves twice, slightly apart.
        let session = session();
        assert!(!session.monitoring());
        assert_eq!(session.monitored_track(), None);
        assert_eq!(session.monitor_status(), None);
    }

    #[test]
    fn turning_it_off_when_it_was_never_on_changes_nothing() {
        // A frontend re-points this from wherever it decides its target, so the no-change call is
        // the common one and must not open a device or rebuild a graph to do nothing.
        let mut session = session();
        session.set_monitoring(None).unwrap();
        assert!(!session.monitoring());
    }
}
