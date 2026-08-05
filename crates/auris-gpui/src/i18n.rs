//! Where the interface gets its words.
//!
//! The tables live in `auris-i18n`; what belongs here is the mapping from the *backend's* types
//! to those tables. A [`SessionError`] or an [`Edit`] is data the session hands over, and only a
//! frontend knows it has to become a sentence — so every such mapping is an exhaustive `match`
//! in this file, which makes a new variant a compile error rather than an English string leaking
//! into a Japanese window.

use auris_i18n::{Key, Language, audio, messages};
use auris_session::prelude::*;
use auris_session::{Edit, SessionError};

use crate::app::AurisApp;

impl AurisApp {
    /// The language the interface is currently in.
    pub(crate) fn language(&self) -> Language {
        self.language
    }

    /// A fixed string in the current language.
    pub(crate) fn t(&self, key: Key) -> &'static str {
        key.get(self.language)
    }

    /// A plugin's name, translated where we know it and left alone where we do not.
    ///
    /// Falls back to the registry id when the plugin is missing entirely, which is what a
    /// project saved with a plugin this build does not have will hit.
    pub(crate) fn plugin_label(&self, id: &str) -> String {
        match self.registry().descriptor(id) {
            Some(descriptor) => audio::plugin_name(&descriptor.name, self.language).to_string(),
            None => id.to_string(),
        }
    }

    /// A plugin's one-line description for the browser.
    pub(crate) fn plugin_description(&self, text: &str) -> String {
        audio::plugin_description(text, self.language).to_string()
    }

    /// A parameter's name.
    pub(crate) fn param_label(&self, name: &str) -> String {
        audio::parameter(name, self.language).to_string()
    }

    /// A plugin category, as the browser groups them.
    pub(crate) fn category_label(&self, category: PluginCategory) -> String {
        audio::category(category.label(), self.language).to_string()
    }

    /// A parameter's value, as text.
    ///
    /// Numbers and units are the same in every language, so those go straight to the core
    /// formatter; the words — a switch position, a named waveform — do not.
    pub(crate) fn format_param(&self, descriptor: &ParamDescriptor, value: f32) -> String {
        match descriptor.unit {
            ParamUnit::Toggle => {
                let key = if value >= 0.5 {
                    Key::ValueOn
                } else {
                    Key::ValueOff
                };
                self.t(key).to_string()
            }
            ParamUnit::Choice => descriptor
                .choices
                .get(value.round().max(0.0) as usize)
                .map(|label| audio::choice(label, self.language).to_string())
                .unwrap_or_else(|| descriptor.format(value)),
            _ => descriptor.format(value),
        }
    }
}

/// What an undo step is called.
pub fn edit_key(edit: Edit) -> Key {
    match edit {
        Edit::ToggleLoop => Key::EditToggleLoop,
        Edit::SetLoopRegion => Key::EditSetLoopRegion,
        Edit::ChangeTempo(_) => Key::EditChangeTempo,
        Edit::SetTempoPoint => Key::EditSetTempoPoint,
        Edit::RemoveTempoPoint => Key::EditRemoveTempoPoint,
        Edit::ChangeSignature(_) => Key::EditChangeSignature,
        Edit::SetSignaturePoint => Key::EditSetSignaturePoint,
        Edit::RemoveSignaturePoint => Key::EditRemoveSignaturePoint,
        Edit::AddInstrumentTrack => Key::EditAddInstrumentTrack,
        Edit::AddAudioTrack => Key::EditAddAudioTrack,
        Edit::AddBusTrack => Key::EditAddBusTrack,
        Edit::DeleteTrack => Key::EditDeleteTrack,
        Edit::DuplicateTrack => Key::EditDuplicateTrack,
        Edit::RenameTrack => Key::EditRenameTrack,
        Edit::SetTrackColor => Key::EditSetTrackColor,
        Edit::MuteTrack => Key::EditMuteTrack,
        Edit::SoloTrack => Key::EditSoloTrack,
        Edit::SetTrackOutput => Key::EditSetTrackOutput,
        Edit::AddSend => Key::EditAddSend,
        Edit::RemoveSend => Key::EditRemoveSend,
        Edit::SetSendPreFader => Key::EditSetSendPreFader,
        Edit::ChangeInstrument => Key::EditChangeInstrument,
        Edit::AddClip => Key::EditAddClip,
        Edit::DeleteClip => Key::EditDeleteClip,
        Edit::DuplicateClip => Key::EditDuplicateClip,
        Edit::SplitClip => Key::EditSplitClip,
        Edit::RenameClip => Key::EditRenameClip,
        Edit::MuteClip => Key::EditMuteClip,
        Edit::MoveClip => Key::EditMoveClip,
        Edit::ResizeClip => Key::EditResizeClip,
        Edit::SetClipGain => Key::EditSetClipGain,
        Edit::SetClipFade => Key::EditSetClipFade,
        Edit::AddNote => Key::EditAddNote,
        Edit::DeleteNotes => Key::EditDeleteNotes,
        Edit::DuplicateNotes => Key::EditDuplicateNotes,
        Edit::TransposeNotes => Key::EditTransposeNotes,
        Edit::SetNoteVelocity => Key::EditSetNoteVelocity,
        Edit::MoveNotes => Key::EditMoveNotes,
        Edit::ResizeNote => Key::EditResizeNote,
        Edit::AddEffect => Key::EditAddEffect,
        Edit::RemoveEffect => Key::EditRemoveEffect,
        Edit::BypassEffect => Key::EditBypassEffect,
        Edit::ReorderEffects => Key::EditReorderEffects,
        Edit::AdjustParameter(_) => Key::EditAdjustParameter,
        Edit::WriteAutomation(_) => Key::EditWriteAutomation,
        Edit::EraseAutomation => Key::EditEraseAutomation,
        Edit::ClearAutomation => Key::EditClearAutomation,
        Edit::ImportAudio => Key::EditImportAudio,
        Edit::ImportSoundFont => Key::EditImportSoundFont,
        Edit::ChoosePreset => Key::EditChoosePreset,
        Edit::SetKey => Key::EditSetKey,
        Edit::SetChord => Key::EditSetChord,
        Edit::MoveChord => Key::EditMoveChord,
        Edit::ClearHarmony => Key::EditClearHarmony,
        Edit::SetSection => Key::EditSetSection,
        Edit::MoveSection => Key::EditMoveSection,
        Edit::StampProgression => Key::EditStampProgression,
        Edit::GenerateClip => Key::EditGenerateClip,
        Edit::FreezeClip => Key::EditFreezeClip,
        Edit::Compose => Key::EditCompose,
    }
}

/// What a track's kind is called in its header.
pub fn track_kind_key(kind: &TrackKind) -> Key {
    match kind {
        TrackKind::Instrument(_) => Key::TrackKindInstrument,
        TrackKind::Audio(_) => Key::TrackKindAudio,
        TrackKind::Bus => Key::TrackKindBus,
    }
}

/// A session error, in the user's language.
///
/// The variants that wrap another crate's error keep that error's own text: a decoder or a
/// device driver speaks English and translating its message would mean paraphrasing something we
/// did not write. The half we own — which operation failed, and on what — is translated.
pub fn error_text(error: &SessionError, language: Language) -> String {
    let with = |key: Key, detail: String| messages::detailed(language, key.get(language), &detail);
    match error {
        SessionError::Io(inner) => with(Key::ErrorFile, inner.to_string()),
        SessionError::Engine(inner) => with(Key::ErrorEngine, inner.to_string()),
        SessionError::Core(inner) => with(Key::ErrorDocument, inner.to_string()),
        SessionError::UnknownPlugin(id) => messages::unknown_plugin(language, id),
        SessionError::UnknownTrack(_) => Key::ErrorUnknownTrack.get(language).to_string(),
        SessionError::UnknownClip(_) => Key::ErrorUnknownClip.get(language).to_string(),
        SessionError::UnknownSend { .. } => Key::ErrorUnknownSend.get(language).to_string(),
        SessionError::NotABus(_) => Key::ErrorNotABus.get(language).to_string(),
        SessionError::RoutingLoop { .. } => Key::ErrorRoutingLoop.get(language).to_string(),
        SessionError::UnknownSoundFont(_) => Key::ErrorUnknownSoundFont.get(language).to_string(),
        SessionError::UnknownProgression(name) => messages::unknown_progression(language, name),
        SessionError::CannotSplit(_) => Key::ErrorCannotSplit.get(language).to_string(),
        SessionError::NotAudio(_) => Key::ErrorNotAudio.get(language).to_string(),
        SessionError::NotFinite(_) => Key::ErrorNotFinite.get(language).to_string(),
        SessionError::NotGenerated(_) => Key::ErrorNotGenerated.get(language).to_string(),
        SessionError::WrongTrackKind { .. } => Key::ErrorWrongTrackKind.get(language).to_string(),
        SessionError::NoPath => Key::ErrorNoPath.get(language).to_string(),
        SessionError::SettingsWrite { path, source } => messages::settings_write_failed(
            language,
            &path.display().to_string(),
            &source.to_string(),
        ),
        SessionError::AudioRestart(reason) => messages::audio_restart_failed(language, reason),
        SessionError::MissingAudio(paths) => messages::missing_audio_files(language, paths.len()),
        // Reaches a status line only when a host did not ask; the desktop app turns this one into
        // a sheet before it ever gets here.
        SessionError::WouldReplace(path) => {
            messages::would_replace(language, &path.display().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_i18n::audio;
    use auris_session::plugin_catalogue;

    #[test]
    fn every_progression_and_groove_says_what_it_is_in_japanese() {
        // The pickers show these sentences, and a missing entry falls back to English silently.
        // The catalogues live in crates that may not name a language, so this is the only place
        // the two halves can be checked against each other.
        for (kind, description) in auris_session::prelude::progression_catalog()
            .iter()
            .map(|entry| ("progression", entry.description))
            .chain(
                auris_session::prelude::groove_catalog()
                    .iter()
                    .map(|groove| ("groove", groove.description)),
            )
        {
            let translated = audio::theory_description(description, Language::Japanese);
            assert_ne!(
                translated, description,
                "the {kind} described as {description:?} is still in English",
            );
        }
    }

    #[test]
    fn every_built_in_plugin_is_translated() {
        // The lookup falls back to English for a plugin nobody has translated, which is right
        // for a third-party one and wrong for ours — so ours are checked here rather than being
        // silently left in English.
        let registry = plugin_catalogue();
        for descriptor in registry.instruments().chain(registry.effects()) {
            assert!(
                audio::is_known(&descriptor.name),
                "`{}` has no Japanese name",
                descriptor.name
            );
            assert!(
                audio::is_known(&descriptor.description),
                "`{}` has no Japanese description",
                descriptor.name
            );
        }
    }

    #[test]
    fn every_built_in_parameter_is_translated() {
        let registry = plugin_catalogue();
        let mut checked = 0;
        let mut missing: Vec<String> = Vec::new();
        for descriptor in registry.instruments().chain(registry.effects()) {
            let params = match registry.create_instrument(&descriptor.id) {
                Ok(plugin) => plugin.parameters().to_vec(),
                Err(_) => registry
                    .create_effect(&descriptor.id)
                    .map(|plugin| plugin.parameters().to_vec())
                    .unwrap_or_default(),
            };
            for param in params {
                if !audio::is_known(&param.name) {
                    missing.push(format!("parameter `{}`", param.name));
                }
                for choice in param.choices.iter() {
                    if !audio::is_known(choice) {
                        missing.push(format!("choice `{choice}`"));
                    }
                }
                checked += 1;
            }
        }
        missing.sort();
        missing.dedup();
        // Every miss is reported at once: fixing a translation table one panic at a time is a
        // slow way to find out there were nine.
        assert!(missing.is_empty(), "untranslated: {missing:?}");
        assert!(checked > 40, "only {checked} parameters were checked");
    }

    #[test]
    fn every_error_says_something_in_both_languages() {
        let errors = [
            SessionError::UnknownPlugin("auris.fx.nope".into()),
            SessionError::UnknownTrack(1),
            SessionError::UnknownClip(2),
            SessionError::CannotSplit(3),
            SessionError::NoPath,
            SessionError::AudioRestart("device gone".into()),
            SessionError::MissingAudio(vec!["a.wav".into()]),
        ];
        for error in &errors {
            for language in Language::ALL {
                let text = error_text(error, language);
                assert!(
                    !text.trim().is_empty(),
                    "{error:?} is blank in {language:?}"
                );
            }
        }
    }
}
