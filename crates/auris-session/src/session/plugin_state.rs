//! A hosted CLAP instrument's own state, read and replaced whole.
//!
//! Some plugins keep most of their sound outside their parameter list: a modular synthesiser's
//! modules and cables, a sampler's zone map, a step sequencer's pattern. Those cannot be reached
//! through `set_instrument_params`, and a model asked to change them had no door at all. This is
//! that door, and it is deliberately the same one a project save uses: the opaque bytes CLAP's
//! state extension defines, which the document already stores beside the parameter map.
//!
//! Three rules keep it honest.
//!
//! * **The plugin is the validator.** A write is loaded into the instance first. Only when the
//!   plugin accepts it, and can hand its state back, does the document change, so a malformed
//!   patch is refused without an undo step instead of leaving a silent track behind it.
//! * **A write is aimed at what was read.** It carries the revision of the state it was derived
//!   from, and a plugin whose state moved since — a knob turned in its window, another write —
//!   refuses it rather than silently discarding that change.
//! * **The document records what the plugin kept.** A plugin may normalise what it was given, so
//!   the stored bytes and the reported revision are the plugin's own read-back, and its parameter
//!   values are captured with them. Otherwise the parameter map, which is applied after the bytes
//!   on every rebuild, would quietly put the old values back.

use auris_core::{PluginState, TrackId};
use sha2::{Digest, Sha256};

use super::{PluginWindow, Session};
use crate::{Edit, SessionError};

/// The largest state read or written, in bytes.
///
/// Large enough for a patch a model can reasonably reason about; a plugin whose state is
/// megabytes of samples or wavetables is not something to be edited as text.
pub const PLUGIN_STATE_LIMIT: usize = 256 * 1024;

/// How a state's bytes are spelt as a string.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StateEncoding {
    /// The bytes are UTF-8 text, given as is.
    Utf8,
    /// Standard base64 with padding.
    Base64,
}

/// Read or replace a hosted CLAP instrument's state.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginStateOperation {
    /// Return the current state, its encoding and its revision.
    Read,
    /// Replace the whole state with one undo step.
    Write {
        /// Revision returned by the latest read.
        expected_revision: String,
        /// How `state` is spelt; use the encoding the read returned unless the format is binary.
        encoding: StateEncoding,
        /// The complete new state, in the plugin's own format.
        state: String,
    },
}

impl PluginStateOperation {
    /// Whether this operation can change the document.
    pub fn is_mutating(&self) -> bool {
        matches!(self, Self::Write { .. })
    }
}

/// What a read returns.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PluginStateReport {
    /// The instrument track.
    pub track: u64,
    /// The plugin's CLAP id.
    pub plugin: String,
    /// What the plugin calls itself, when an instance is loaded.
    pub name: Option<String>,
    /// How `state` is spelt.
    pub encoding: StateEncoding,
    /// The complete state.
    pub state: String,
    /// The state's length in bytes.
    pub bytes: usize,
    /// Revision to pass back with a write.
    pub revision: String,
}

/// What a write returns.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PluginStateWrite {
    /// The instrument track.
    pub track: u64,
    /// `false` when the state was already exactly this, which records nothing.
    pub changed: bool,
    /// Revision of the state the plugin now holds.
    pub revision: String,
    /// Its length in bytes.
    pub bytes: usize,
    /// Whether the plugin's read-back is byte-for-byte what was written. A plugin may normalise
    /// a state it accepts; read again before deriving the next edit from it.
    pub exact_readback: bool,
}

/// Spells state bytes as text: as themselves when they are UTF-8 without stray control
/// characters, and as base64 otherwise.
pub fn encode_state(bytes: &[u8]) -> (StateEncoding, String) {
    match std::str::from_utf8(bytes) {
        Ok(text)
            if !text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) =>
        {
            (StateEncoding::Utf8, text.to_owned())
        }
        _ => {
            use base64::Engine;
            (
                StateEncoding::Base64,
                base64::engine::general_purpose::STANDARD.encode(bytes),
            )
        }
    }
}

/// The inverse of [`encode_state`].
pub fn decode_state(encoding: StateEncoding, text: &str) -> Result<Vec<u8>, SessionError> {
    match encoding {
        StateEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
        StateEncoding::Base64 => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map_err(|error| SessionError::PluginState(format!("invalid base64: {error}")))
        }
    }
}

/// A short content hash, so that a write can name the state it was derived from.
fn revision(bytes: &[u8]) -> String {
    hex::encode(&Sha256::digest(bytes)[..8])
}

impl Session {
    /// The CLAP id of a track's hosted instrument, or why there is none.
    fn hosted_clap_instrument(&self, track: TrackId) -> Result<String, SessionError> {
        let instrument = self
            .project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .ok_or_else(|| {
                SessionError::PluginState("expected an instrument or drum track".into())
            })?;
        instrument
            .instrument_id
            .strip_prefix(auris_clap::ID_PREFIX)
            .map(str::to_owned)
            .ok_or_else(|| {
                SessionError::PluginState(
                    "this track's instrument is not a hosted CLAP plugin; use inspect_instrument and set_instrument_params".into(),
                )
            })
    }

    /// The state the track is actually playing with: the instance's own when one is loaded,
    /// the document's otherwise.
    fn current_plugin_state(&mut self, track: TrackId) -> Result<Vec<u8>, SessionError> {
        if let Some(bytes) = self.hosted.save_instrument_state(track)? {
            return Ok(bytes);
        }
        Ok(self
            .project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .and_then(|instrument| instrument.instrument_state.hosted_bytes())
            .unwrap_or_default())
    }

    /// Reads a hosted CLAP instrument's complete state.
    pub fn plugin_state(&mut self, track: TrackId) -> Result<PluginStateReport, SessionError> {
        let plugin = self.hosted_clap_instrument(track)?;
        let bytes = self.current_plugin_state(track)?;
        if bytes.len() > PLUGIN_STATE_LIMIT {
            return Err(SessionError::PluginState(format!(
                "the state is {} bytes, over the {PLUGIN_STATE_LIMIT}-byte limit",
                bytes.len()
            )));
        }
        let (encoding, state) = encode_state(&bytes);
        Ok(PluginStateReport {
            track: track.0,
            plugin,
            name: self.hosted.instrument_name(track).map(str::to_owned),
            encoding,
            state,
            bytes: bytes.len(),
            revision: revision(&bytes),
        })
    }

    /// Replaces a hosted CLAP instrument's complete state as one undo step.
    ///
    /// The plugin loads the bytes before the document changes; a plugin that refuses them leaves
    /// both the sound and the history as they were.
    pub fn set_plugin_state(
        &mut self,
        track: TrackId,
        state: &[u8],
        expected_revision: &str,
    ) -> Result<PluginStateWrite, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        self.hosted_clap_instrument(track)?;
        if state.len() > PLUGIN_STATE_LIMIT {
            return Err(SessionError::PluginState(format!(
                "the state is {} bytes, over the {PLUGIN_STATE_LIMIT}-byte limit",
                state.len()
            )));
        }
        // Bring every hosted state into the document first, so the undo snapshot holds what
        // was actually playing rather than what was last saved.
        self.collect_hosted_state()?;
        let current = self.current_plugin_state(track)?;
        let current_revision = revision(&current);
        if current_revision != expected_revision {
            return Err(SessionError::PluginState(format!(
                "the state changed since it was read (revision is now {current_revision}); read it again and reapply the edit"
            )));
        }
        if current == state {
            return Ok(PluginStateWrite {
                track: track.0,
                changed: false,
                revision: current_revision,
                bytes: current.len(),
                exact_readback: true,
            });
        }
        let loaded = match self.hosted.load_instrument_state(track, state) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.restore_plugin_state(track, &current);
                return Err(SessionError::PluginState(format!(
                    "the plugin rejected the state, nothing changed: {error}"
                )));
            }
        };
        if !loaded {
            return Err(SessionError::PluginState(
                "the plugin is not loaded, so the state cannot be checked; make sure its .clap file is installed".into(),
            ));
        }
        let saved = self
            .project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .map(|instrument| instrument.instrument_state.clone())
            .unwrap_or_else(PluginState::empty);
        let kept = match self
            .hosted
            .sound_state(PluginWindow::Instrument(track), &saved)
        {
            Ok(kept) => kept,
            Err(error) => {
                self.restore_plugin_state(track, &current);
                return Err(error);
            }
        };
        let readback = kept.hosted_bytes().unwrap_or_default();
        self.record(Edit::SetPluginState);
        if let Some(instrument) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
        {
            instrument.instrument_state = kept;
        }
        Ok(PluginStateWrite {
            track: track.0,
            changed: true,
            revision: revision(&readback),
            bytes: readback.len(),
            exact_readback: readback == state,
        })
    }

    /// Puts back the state a refused write found, so a plugin that half-applied it before
    /// failing still sounds as it did.
    fn restore_plugin_state(&mut self, track: TrackId, bytes: &[u8]) {
        if let Err(error) = self.hosted.load_instrument_state(track, bytes) {
            log::warn!("could not restore the previous plugin state on track {track:?}: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{Scratch, session};
    use super::*;
    use auris_clap::testkit::{TONE_ID, instrument_library};

    fn hosted_tone(name: &str) -> (Session, TrackId, Scratch) {
        let scratch = Scratch::new(name);
        let file = scratch.join("tone.clap");
        std::fs::write(&file, b"fixture").unwrap();
        let mut session = session();
        session
            .hosted
            .insert_library(file.clone(), instrument_library());
        let track = session.add_default_instrument_track("Tone").unwrap();
        session
            .set_hosted_instrument(track, &file, TONE_ID)
            .unwrap();
        (session, track, scratch)
    }

    #[test]
    fn encoding_keeps_text_readable_and_binary_lossless() {
        assert_eq!(
            encode_state(b"{\"a\":1}\n"),
            (StateEncoding::Utf8, "{\"a\":1}\n".into())
        );
        let binary = [0u8, 159, 146, 150];
        let (encoding, text) = encode_state(&binary);
        assert_eq!(encoding, StateEncoding::Base64);
        assert_eq!(decode_state(encoding, &text).unwrap(), binary);
        assert!(decode_state(StateEncoding::Base64, "not base64!").is_err());
    }

    #[test]
    fn a_write_reaches_the_plugin_and_the_document_and_undoes_in_one_step() {
        let (mut session, track, _scratch) = hosted_tone("plugin-state-write");
        let before = session.plugin_state(track).unwrap();
        assert_eq!(before.plugin, TONE_ID);
        let old = decode_state(before.encoding, &before.state).unwrap();
        let new = 0.25f32.to_le_bytes();

        let write = session
            .set_plugin_state(track, &new, &before.revision)
            .unwrap();
        assert!(write.changed && write.exact_readback);
        assert_eq!(write.revision, revision(&new));
        let after = session.plugin_state(track).unwrap();
        assert_eq!(decode_state(after.encoding, &after.state).unwrap(), new);
        let stored = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state
            .clone();
        assert_eq!(stored.hosted_bytes(), Some(new.to_vec()));
        // The parameter map travels with the bytes, or a rebuild would put the old level back.
        assert!(stored.params.values().any(|value| *value == 0.25));
        assert!(session.is_dirty());

        assert_eq!(session.undo(), Some(Edit::SetPluginState));
        session.rebuild_graph();
        let undone = session.plugin_state(track).unwrap();
        assert_eq!(decode_state(undone.encoding, &undone.state).unwrap(), old);
        assert_eq!(session.redo(), Some(Edit::SetPluginState));
        session.rebuild_graph();
        let redone = session.plugin_state(track).unwrap();
        assert_eq!(decode_state(redone.encoding, &redone.state).unwrap(), new);
    }

    #[test]
    fn a_stale_revision_or_a_refused_state_changes_nothing() {
        let (mut session, track, _scratch) = hosted_tone("plugin-state-refused");
        let before = session.plugin_state(track).unwrap();
        // A write synchronises every hosted state into the document before it checks anything;
        // do that here too, so the comparison below is about the write alone.
        session.collect_hosted_state().unwrap();
        let project = session.project().clone();

        let stale = session.set_plugin_state(track, &0.5f32.to_le_bytes(), "0000000000000000");
        assert!(
            matches!(stale, Err(SessionError::PluginState(ref m)) if m.contains("read it again"))
        );

        // The fixture reads exactly four bytes; two is a state it cannot load.
        let refused = session.set_plugin_state(track, &[1, 2], &before.revision);
        assert!(matches!(refused, Err(SessionError::PluginState(ref m)) if m.contains("rejected")));

        assert_eq!(session.plugin_state(track).unwrap(), before);
        assert_eq!(session.project(), &project);
        assert_ne!(session.undo(), Some(Edit::SetPluginState));
    }

    #[test]
    fn an_identical_write_records_nothing() {
        let (mut session, track, _scratch) = hosted_tone("plugin-state-identical");
        let before = session.plugin_state(track).unwrap();
        let bytes = decode_state(before.encoding, &before.state).unwrap();
        let write = session
            .set_plugin_state(track, &bytes, &before.revision)
            .unwrap();
        assert!(!write.changed);
        assert_ne!(session.undo(), Some(Edit::SetPluginState));
    }

    #[test]
    fn the_agent_command_reads_and_writes_through_json() {
        let (mut session, track, _scratch) = hosted_tone("plugin-state-agent");
        let run = |session: &mut Session, json: serde_json::Value| {
            let command: crate::live_agent::Command = serde_json::from_value(json).unwrap();
            session.agent_command(command)
        };
        let read: serde_json::Value = serde_json::from_str(
            &run(
                &mut session,
                serde_json::json!({"action":"plugin_state","track":track.0,"operation":{"kind":"read"}}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read["plugin"], TONE_ID);
        assert_eq!(read["encoding"], "base64");

        let state = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(0.5f32.to_le_bytes())
        };
        let write: serde_json::Value = serde_json::from_str(
            &run(
                &mut session,
                serde_json::json!({"action":"plugin_state","track":track.0,"operation":{
                    "kind":"write","expected_revision":read["revision"],"encoding":"base64","state":state}}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(write["changed"], true);
        assert_eq!(write["exact_readback"], true);

        let stale = run(
            &mut session,
            serde_json::json!({"action":"plugin_state","track":track.0,"operation":{
                "kind":"write","expected_revision":read["revision"],"encoding":"base64","state":state}}),
        );
        assert!(stale.unwrap_err().contains("read it again"));
    }

    #[test]
    fn only_hosted_clap_instruments_have_a_native_state() {
        let mut session = session();
        let track = session.add_default_instrument_track("Built in").unwrap();
        assert!(matches!(
            session.plugin_state(track),
            Err(SessionError::PluginState(ref m)) if m.contains("not a hosted CLAP plugin")
        ));
    }
}
