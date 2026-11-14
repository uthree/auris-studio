//! Playing a MIDI keyboard into the session.
//!
//! The keyboard is heard the moment a key goes down: [`auris_engine::midi`] answers it on the
//! MIDI backend's own thread, straight into the engine, because a note that waited for a
//! frontend's timer would sound late. What is left for the session is the part a timer is
//! good enough for:
//!
//! * **which track** it plays — the one a take is writing onto, otherwise whichever track the
//!   typing keyboard would play ([`Session::audition_track`]), so that the two keyboards never
//!   disagree about where a note goes;
//! * **which keyboards** are listened to — every input the system has, looked for again every
//!   couple of seconds so that a keyboard plugged in after launch plays without a restart;
//! * **writing it down** — the events the MIDI thread stamped wait in a queue, and a note take
//!   on the track they were sounded on reads them at the frame they were heard, not the frame
//!   the session got round to them.
//!
//! Nothing here is an edit, for the reason nothing in [`typing`](super::typing) is: somebody
//! playing is not somebody writing, until a take turns what they played into a clip.

use std::time::{Duration, Instant};

use auris_core::TrackId;
use auris_engine::{LiveTarget, MidiInputs, Performed, VelocityResponse};

use super::Session;

/// How often the system is asked which MIDI inputs it has.
///
/// Slow enough that asking costs nothing measurable, quick enough that a keyboard plugged in
/// mid-session plays before anybody wonders why it does not.
pub const MIDI_SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// What the session knows about the MIDI keyboards it is listening to.
#[derive(Debug, Default)]
pub(super) struct MidiInput {
    /// Whether to listen at all. Off for a headless session, which must not depend on what is
    /// plugged into the machine it runs on.
    enabled: bool,
    /// The inputs, and what they are holding.
    inputs: MidiInputs,
    /// The track the frontend would like played, before [`Session::audition_track`] has had
    /// its say.
    preferred: Option<TrackId>,
    /// When the system was last asked which inputs it has.
    scanned: Option<Instant>,
    /// The last thing that went wrong opening an input, until a frontend asks.
    error: Option<String>,
    /// Inputs that refused to open, so that a scan every two seconds says so once. Tried
    /// again on every scan all the same: the program holding one may let go of it.
    refused: Vec<String>,
    /// Inputs opened since a frontend last asked, for it to announce.
    opened: Vec<String>,
    /// Inputs the system has, as of the last look, whether listened to or not.
    available: Vec<String>,
    /// Inputs somebody has said not to listen to.
    ignored: Vec<String>,
    /// Notes struck since a frontend last asked, for a control learning which note a key sends.
    struck: Vec<u8>,
}

/// How many struck notes wait for a frontend before the oldest are forgotten.
const STRUCK_CAPACITY: usize = 64;

impl Session {
    /// Whether MIDI keyboards are being listened to.
    pub fn midi_input_enabled(&self) -> bool {
        self.midi.enabled
    }

    /// Starts or stops listening to every MIDI keyboard the system has.
    ///
    /// Starting looks for them at once; stopping closes them and silences whatever they held.
    pub fn set_midi_input(&mut self, enabled: bool) {
        if self.midi.enabled == enabled {
            return;
        }
        self.midi.enabled = enabled;
        self.midi.scanned = None;
        self.midi.refused.clear();
        if enabled {
            self.service_midi(Instant::now());
        } else {
            self.midi.inputs.close();
        }
    }

    /// Every MIDI input the system had at the last look, listened to or not.
    pub fn available_midi_inputs(&self) -> &[String] {
        &self.midi.available
    }

    /// Stops listening to the inputs named in `ignored`, and starts again on any no longer named.
    ///
    /// By name, because a name is what survives a keyboard being unplugged and plugged back in;
    /// a name the system does not have is kept, so the choice holds for when it comes back.
    pub fn set_ignored_midi_inputs(&mut self, ignored: Vec<String>) {
        if self.midi.ignored == ignored {
            return;
        }
        self.midi.ignored = ignored;
        self.midi.scanned = None;
        if self.midi.enabled {
            self.service_midi(Instant::now());
        }
    }

    /// Takes up everything in a set of MIDI preferences that the session acts on.
    ///
    /// Whether to listen at all is left to the frontend, which knows which of its windows is
    /// being heard; see [`Self::set_midi_input`].
    pub fn apply_midi_preferences(&mut self, preferences: &crate::settings::MidiPreferences) {
        self.set_ignored_midi_inputs(preferences.ignored_inputs.clone());
        self.set_midi_velocity(preferences.velocity.response());
        self.midi.inputs.set_octave(preferences.octave);
        self.set_note_record_mode(preferences.record_mode);
        self.set_record_quantize(preferences.record_quantize());
    }

    /// Chooses how hard a key has to be struck for how loud a note, from the next note on.
    pub fn set_midi_velocity(&mut self, response: VelocityResponse) {
        self.midi.inputs.set_velocity_response(response);
    }

    /// The notes struck on a MIDI keyboard since the last call, oldest first.
    pub fn take_midi_struck(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.midi.struck)
    }

    /// The MIDI inputs being listened to, by the names the system gives them.
    pub fn midi_inputs(&self) -> Vec<String> {
        self.midi.inputs.connected().map(str::to_string).collect()
    }

    /// Asks for the keyboard to be played on `preferred` — the selection, usually.
    ///
    /// A preference, as the typing keyboard's track is: a track that cannot play notes gives way
    /// to one that can, and a take that is running keeps the keyboard on the track it is writing.
    pub fn set_midi_target(&mut self, preferred: Option<TrackId>) {
        if self.midi.preferred != preferred {
            self.midi.preferred = preferred;
            self.point_midi();
        }
    }

    /// The track a MIDI keyboard plays right now, if any.
    pub fn midi_target(&self) -> Option<TrackId> {
        if let Some(take) = self.note_take.as_ref() {
            return Some(take.track());
        }
        self.audition_track(self.midi.preferred)
    }

    /// Whether anything has been played on a MIDI keyboard since the last call.
    ///
    /// What a frontend reads to light an activity lamp, and to wake an audio output it keeps
    /// asleep until something needs to be heard.
    pub fn take_midi_activity(&mut self) -> bool {
        self.midi.inputs.take_activity()
    }

    /// The notes a MIDI keyboard is sounding now, for a frontend to draw on its keys.
    pub fn midi_sounding(&self) -> Vec<u8> {
        self.midi.inputs.sounding()
    }

    /// The MIDI inputs opened since the last call, for a frontend to say a keyboard is ready.
    pub fn take_opened_midi_inputs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.midi.opened)
    }

    /// The last thing that went wrong opening a MIDI input, if a frontend has not asked yet.
    pub fn take_midi_error(&mut self) -> Option<String> {
        self.midi.error.take()
    }

    /// Silences every note the MIDI keyboards are holding.
    pub fn release_midi_notes(&mut self) {
        self.midi.inputs.release_all();
        self.drain_midi();
    }

    /// The part of [`Self::poll`] that keeps the keyboards in step with the session.
    pub(super) fn service_midi(&mut self, now: Instant) {
        if !self.midi.enabled {
            return;
        }
        if self
            .midi
            .scanned
            .is_none_or(|then| now.duration_since(then) >= MIDI_SCAN_INTERVAL)
        {
            self.midi.scanned = Some(now);
            self.rescan_midi();
        }
        // The engine is replaced when the output is, and a track's position moves when one
        // before it goes, so the route is renewed on every tick rather than tracked.
        self.point_midi();
        self.drain_midi();
    }

    /// Opens every input that has appeared and lets go of every one that has gone.
    fn rescan_midi(&mut self) {
        let available = match self.midi.inputs.available() {
            Ok(available) => available,
            Err(error) => {
                self.note_midi_error(error.to_string());
                return;
            }
        };
        self.midi.available = available.clone();
        let ignored = &self.midi.ignored;
        let gone: Vec<String> = self
            .midi
            .inputs
            .connected()
            .filter(|open| {
                !available.iter().any(|name| name == open)
                    || ignored.iter().any(|name| name == open)
            })
            .map(str::to_string)
            .collect();
        self.midi
            .refused
            .retain(|refused| available.contains(refused));
        for name in &gone {
            log::info!("MIDI input `{name}` has gone");
            self.midi.inputs.disconnect(name);
        }
        if !gone.is_empty() {
            // Most likely unplugged with a key down, and that key will never come up now.
            self.midi.inputs.release_all();
        }
        for name in available {
            if self.midi.inputs.connected().any(|open| open == name)
                || self.midi.ignored.contains(&name)
            {
                continue;
            }
            match self.midi.inputs.connect(&name) {
                Ok(()) => {
                    log::info!("listening to MIDI input `{name}`");
                    self.midi.refused.retain(|refused| *refused != name);
                    self.midi.opened.push(name);
                }
                Err(error) => {
                    if !self.midi.refused.contains(&name) {
                        self.note_midi_error(error.to_string());
                        self.midi.refused.push(name);
                    }
                }
            }
        }
    }

    /// Remembers an error for a frontend to report.
    fn note_midi_error(&mut self, error: String) {
        log::warn!("{error}");
        self.midi.error = Some(error);
    }

    /// Tells the inputs where to sound what is played.
    fn point_midi(&mut self) {
        let target = self.midi_target().and_then(|track| {
            self.project
                .tracks
                .iter()
                .position(|found| found.id == track)
                .map(|index| LiveTarget { index, track })
        });
        self.midi.inputs.set_target(&self.engine, target);
    }

    /// Hands what was played to the take listening for it.
    ///
    /// Each event is moved earlier by the latency a tap-along measured, so a note lands where
    /// the player meant it rather than where it reached the computer.
    fn drain_midi(&mut self) {
        let rate = self.engine.sample_rate().max(1.0);
        let lag = self.midi_lag_ms() / 1000.0 * rate;
        for event in self.midi.inputs.drain() {
            if let Performed::NoteOn { .. } = event.performed {
                // A tap is measured as heard, before any correction: what is being found out is
                // the very lateness the correction takes off.
                self.tap_midi_calibration(event.frame);
            }
            let at = event.frame - lag;
            match event.performed {
                Performed::NoteOn { pitch, velocity } => {
                    if self.midi.struck.len() >= STRUCK_CAPACITY {
                        self.midi.struck.remove(0);
                    }
                    self.midi.struck.push(pitch);
                    self.note_take_struck_at(event.track, pitch, velocity, at);
                    self.step_struck(event.track, pitch, velocity);
                }
                Performed::NoteOff { pitch } => {
                    self.note_take_released_at(event.track, pitch, at);
                    self.step_released(event.track, pitch);
                }
                Performed::PitchBend(semitones) => {
                    self.note_take_bent_at(event.track, semitones, at);
                }
                Performed::Controller { number, value } => {
                    self.note_take_controller_at(event.track, number, value, at);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::session::fixtures::session;

    #[test]
    fn a_headless_session_listens_to_no_keyboard() {
        let mut session = session();
        assert!(!session.midi_input_enabled());
        session.poll();
        assert!(session.midi_inputs().is_empty());
    }

    #[test]
    fn the_keyboard_plays_the_selection_or_the_track_being_recorded() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let pad = session.add_default_instrument_track("Pad").unwrap();
        session.set_midi_target(Some(pad));
        assert_eq!(session.midi_target(), Some(pad));
        session.set_midi_target(None);
        assert_eq!(
            session.midi_target(),
            Some(lead),
            "with nothing chosen, the first track that plays"
        );
        session.start_recording(Some(pad)).unwrap();
        session.set_midi_target(Some(lead));
        assert_eq!(
            session.midi_target(),
            Some(pad),
            "a take keeps the keyboard on its own track"
        );
        session.stop_recording().unwrap();
        assert_eq!(session.midi_target(), Some(lead));
    }

    #[test]
    fn what_a_keyboard_plays_into_a_take_becomes_its_clip() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_midi_target(Some(lead));
        session.start_recording(Some(lead)).unwrap();
        session.point_midi();
        let keyboard = &session.midi.inputs;
        keyboard.receive(&[0x90, 60, 127]);
        keyboard.receive(&[0x90, 67, 64]);
        keyboard.receive(&[0xe0, 0x7f, 0x7f]);
        keyboard.receive(&[0x80, 60, 0]);
        // A silent engine's clock stands still, so the wheel is left where it was pushed: a
        // move back at the same instant would be the same tick, and only the last one stays.
        keyboard.receive(&[0x90, 67, 0]);
        session.drain_midi();
        let report = session.stop_recording().unwrap();
        let clip = report.takes[0].clip.expect("a clip was written");
        let (_, written) = session.project().midi_clip(clip).unwrap();
        let mut pitches: Vec<(u8, f32)> = written
            .notes
            .iter()
            .map(|note| (note.pitch, note.velocity))
            .collect();
        pitches.sort_by_key(|pitch| pitch.0);
        assert_eq!(pitches.len(), 2);
        assert_eq!(pitches[0].0, 60);
        assert!(
            (pitches[0].1 - 1.0).abs() < 1e-6,
            "a full strike is full velocity"
        );
        assert_eq!(pitches[1].0, 67);
        assert!(
            written.bend.iter().any(|point| point.value > 1.9),
            "the wheel was written down: {:?}",
            written.bend
        );
    }

    #[test]
    fn notes_played_while_nothing_records_sound_but_are_not_kept() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_midi_target(Some(lead));
        session.midi.inputs.receive(&[0x90, 60, 100]);
        assert_eq!(session.midi_sounding(), vec![60]);
        session.release_midi_notes();
        assert!(session.midi_sounding().is_empty());
        assert!(session.take_midi_activity());
    }

    #[test]
    fn preferences_reach_the_take() {
        let mut session = session();
        let preferences = crate::settings::MidiPreferences {
            quantize_division: 16,
            quantize_strength: 0.5,
            record_mode: crate::NoteRecordMode::Merge,
            ..Default::default()
        };
        session.apply_midi_preferences(&preferences);
        assert_eq!(session.note_record_mode(), crate::NoteRecordMode::Merge);
        let quantize = session.record_quantize().unwrap();
        assert_eq!(quantize.grid, auris_core::Ticks(240), "a sixteenth");
        assert_eq!(quantize.strength, 0.5);
    }

    #[test]
    fn the_octave_in_the_preferences_moves_what_a_key_sounds() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_midi_target(Some(lead));
        session.apply_midi_preferences(&crate::settings::MidiPreferences {
            octave: -1,
            ..Default::default()
        });
        session.midi.inputs.receive(&[0x90, 60, 100]);
        assert_eq!(session.midi_sounding(), vec![48]);
        session.release_midi_notes();
    }

    #[test]
    fn a_tap_along_on_a_midi_keyboard_takes_its_notes_as_taps() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_midi_target(Some(lead));
        session.start_midi_calibration().unwrap();
        assert!(session.calibrating_midi());
        assert!(
            !session.calibrating_typing(),
            "the typing keyboard is not the one measured"
        );
        session.midi.inputs.receive(&[0x90, 60, 100]);
        session.drain_midi();
        let taps = session
            .typing_calibration_run
            .as_ref()
            .map(|run| run.tap_count())
            .unwrap();
        assert_eq!(taps, 1);
        session.cancel_typing_calibration();
        assert!(!session.calibrating_midi());
        assert!(session.take_midi_calibration().is_none());
    }

    #[test]
    fn a_measured_midi_latency_moves_recorded_notes_earlier() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_midi_target(Some(lead));
        session.audio.midi_calibration = Some(crate::settings::TypingCalibration {
            offset_ms: 10.0,
            spread_ms: 1.0,
        });
        session.start_recording(Some(lead)).unwrap();
        session.midi.inputs.receive(&[0x90, 60, 100]);
        session.drain_midi();
        // A silent session hears frame zero; the note is written ten milliseconds before it.
        let rate = session.engine.sample_rate();
        let struck = session
            .note_take
            .as_ref()
            .and_then(|take| take.first_struck());
        assert_eq!(struck, Some(-rate / 100.0));
        assert_eq!(session.midi_lag_ms(), 10.0);
    }

    #[test]
    fn struck_notes_wait_for_whoever_is_learning_one() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.set_midi_target(Some(lead));
        session.midi.inputs.receive(&[0x90, 38, 100]);
        session.midi.inputs.receive(&[0x80, 38, 0]);
        session.drain_midi();
        assert_eq!(session.take_midi_struck(), vec![38]);
        assert!(session.take_midi_struck().is_empty(), "read once");
    }
}
