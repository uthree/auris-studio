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

    /// The name to draw beside one effect slot.
    ///
    /// A hosted plugin has no registry entry, so [`Self::plugin_label`] falls back to its id —
    /// which is a reverse-DNS string somebody else chose, and is what a slot holding Surge XT
    /// showed until this existed. The plugin's own name is not translated: nobody here wrote it,
    /// and a translation table cannot know what is in a file that was installed yesterday.
    pub(crate) fn effect_label(&self, slot: EffectSlotId, effect_id: &str) -> String {
        hosted_label(self.session.hosted_name(slot), self.plugin_label(effect_id))
    }

    /// The name to draw for whatever plays a track, for the same reasons.
    pub(crate) fn instrument_label(&self, track: TrackId, instrument_id: &str) -> String {
        hosted_label(
            self.session.hosted_instrument_name(track),
            self.plugin_label(instrument_id),
        )
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
        Edit::SetPunchRegion => Key::EditSetPunchRegion,
        Edit::ChangeTempo(_) => Key::EditChangeTempo,
        Edit::SetTempoPoint => Key::EditSetTempoPoint,
        Edit::RemoveTempoPoint => Key::EditRemoveTempoPoint,
        Edit::ChangeSignature(_) => Key::EditChangeSignature,
        Edit::SetSignaturePoint => Key::EditSetSignaturePoint,
        Edit::RemoveSignaturePoint => Key::EditRemoveSignaturePoint,
        Edit::AddInstrumentTrack => Key::EditAddInstrumentTrack,
        Edit::AddSingerTrack => Key::EditAddSingerTrack,
        Edit::AddAudioTrack => Key::EditAddAudioTrack,
        Edit::AddBusTrack => Key::EditAddBusTrack,
        Edit::DeleteTrack => Key::EditDeleteTrack,
        Edit::DuplicateTrack => Key::EditDuplicateTrack,
        Edit::MoveTrack => Key::EditMoveTrack,
        Edit::RenameTrack => Key::EditRenameTrack,
        Edit::SetTrackColor => Key::EditSetTrackColor,
        Edit::SetTrackHeight => Key::EditSetTrackHeight,
        Edit::MuteTrack => Key::EditMuteTrack,
        Edit::SoloTrack => Key::EditSoloTrack,
        Edit::SetTrackOutput => Key::EditSetTrackOutput,
        Edit::AddSend => Key::EditAddSend,
        Edit::RemoveSend => Key::EditRemoveSend,
        Edit::SetSendPreFader => Key::EditSetSendPreFader,
        Edit::ChangeInstrument => Key::EditChangeInstrument,
        Edit::Accompany => Key::EditAccompany,
        Edit::AddClip => Key::EditAddClip,
        Edit::DeleteClip => Key::EditDeleteClip,
        Edit::CutClips => Key::EditCutClips,
        Edit::PasteClips => Key::EditPasteClips,
        Edit::DuplicateClip => Key::EditDuplicateClip,
        Edit::SplitClip => Key::EditSplitClip,
        Edit::RenameClip => Key::EditRenameClip,
        Edit::MuteClip => Key::EditMuteClip,
        Edit::MoveClip => Key::EditMoveClip,
        Edit::ResizeClip => Key::EditResizeClip,
        Edit::LoopClip => Key::EditLoopClip,
        Edit::SetClipGain => Key::EditSetClipGain,
        Edit::SetClipFade => Key::EditSetClipFade,
        Edit::Crossfade => Key::EditCrossfade,
        Edit::SetClipTempo => Key::EditSetClipTempo,
        Edit::AddNote => Key::EditAddNote,
        Edit::DeleteNotes => Key::EditDeleteNotes,
        Edit::CutNotes => Key::EditCutNotes,
        Edit::PasteNotes => Key::EditPasteNotes,
        Edit::DuplicateNotes => Key::EditDuplicateNotes,
        Edit::TransposeNotes => Key::EditTransposeNotes,
        Edit::SetNoteVelocity => Key::EditSetNoteVelocity,
        Edit::MoveNotes => Key::EditMoveNotes,
        Edit::ResizeNote => Key::EditResizeNote,
        Edit::QuantizeNotes => Key::EditQuantizeNotes,
        Edit::SetLyric => Key::EditSetLyric,
        Edit::WriteLyrics => Key::EditWriteLyrics,
        Edit::SetPhonemes => Key::EditSetPhonemes,
        Edit::SetPhonemeDuration(..) => Key::EditPhonemeDuration,
        Edit::ResetPhonemeTiming => Key::EditResetPhonemeTiming,
        Edit::SetScoop(..) => Key::EditScoop,
        Edit::SetFall(..) => Key::EditFall,
        Edit::SetVibrato(..) => Key::EditVibrato,
        Edit::ResetOrnaments => Key::EditResetOrnaments,
        Edit::SetFrameHop => Key::EditSetFrameHop,
        Edit::SetSingerVoice => Key::EditSetSingerVoice,
        Edit::Sing => Key::EditSing,
        Edit::AddEffect => Key::EditAddEffect,
        Edit::RemoveEffect => Key::EditRemoveEffect,
        Edit::BypassEffect => Key::EditBypassEffect,
        Edit::ReorderEffects => Key::EditReorderEffects,
        Edit::SetEffectSidechain => Key::EditSetEffectSidechain,
        Edit::AdjustParameter(_) => Key::EditAdjustParameter,
        Edit::WriteAutomation(_) => Key::EditWriteAutomation,
        Edit::EraseAutomation => Key::EditEraseAutomation,
        Edit::WriteBend(_) => Key::EditWriteBend,
        Edit::EraseBend => Key::EditEraseBend,
        // The wheel is named, because everybody knows what it is; any other controller is named by
        // its kind, because "the modulation" over a pedal movement would be a menu that lies.
        Edit::WriteController(number, _) if number == CC_MODULATION => Key::EditWriteModulation,
        Edit::EraseController(number) if number == CC_MODULATION => Key::EditEraseModulation,
        Edit::WriteController(..) => Key::EditWriteController,
        Edit::EraseController(_) => Key::EditEraseController,
        Edit::ClearAutomation => Key::EditClearAutomation,
        Edit::ImportAudio => Key::EditImportAudio,
        Edit::RecordTake => Key::EditRecordTake,
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
        Edit::SetClipTransforms(_) => Key::EditSetClipTransforms,
        Edit::FreezeClipTransforms => Key::EditFreezeClipTransforms,
        Edit::Compose => Key::EditCompose,
        Edit::BalanceLevels => Key::EditBalanceLevels,
    }
}

/// What a track's kind is called in its header.
pub fn track_kind_key(kind: &TrackKind) -> Key {
    match kind {
        TrackKind::Instrument(_) => Key::TrackKindInstrument,
        TrackKind::Singer(_) => Key::TrackKindSinger,
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
        // The plugin's own words: it names a file, an id or a refusal that came from somebody
        // else's binary, and paraphrasing that would lose the only part worth reading.
        SessionError::Clap(inner) => with(Key::ErrorPlugin, inner.to_string()),
        SessionError::Core(inner) => with(Key::ErrorDocument, inner.to_string()),
        SessionError::UnknownPlugin(id) => messages::unknown_plugin(language, id),
        SessionError::UnknownTrack(_) => Key::ErrorUnknownTrack.get(language).to_string(),
        SessionError::UnknownClip(_) => Key::ErrorUnknownClip.get(language).to_string(),
        SessionError::UnknownNote { .. } => Key::ErrorUnknownNote.get(language).to_string(),
        // The one vocal error a person can *fix* gets the sentence naming the fix; the others
        // carry the loader's or the frontend's own words, which name the folder or the text.
        SessionError::Vocal(VocalError::NeedsDictionary { .. }) => {
            Key::ErrorNeedsDictionary.get(language).to_string()
        }
        SessionError::Vocal(inner @ VocalError::Dictionary { .. }) => {
            with(Key::ErrorDictionary, inner.to_string())
        }
        SessionError::Vocal(inner) => with(Key::ErrorLyric, inner.to_string()),
        SessionError::Sing(inner) => with(Key::ErrorSing, inner.to_string()),
        SessionError::NoVoice(_) => Key::ErrorNoVoice.get(language).to_string(),
        SessionError::SingingNeedsFolder => Key::ErrorSingingNeedsFolder.get(language).to_string(),
        SessionError::NothingToSing(_) => Key::ErrorNothingToSing.get(language).to_string(),
        SessionError::UnknownSend { .. } => Key::ErrorUnknownSend.get(language).to_string(),
        SessionError::NotABus(_) => Key::ErrorNotABus.get(language).to_string(),
        SessionError::RoutingLoop { .. } => Key::ErrorRoutingLoop.get(language).to_string(),
        SessionError::UnknownSoundFont(_) => Key::ErrorUnknownSoundFont.get(language).to_string(),
        SessionError::LibraryMissing => Key::ErrorLibraryMissing.get(language).to_string(),
        SessionError::UnknownProgression(name) => messages::unknown_progression(language, name),
        SessionError::CannotSplit(_) => Key::ErrorCannotSplit.get(language).to_string(),
        SessionError::NotAudio(_) => Key::ErrorNotAudio.get(language).to_string(),
        SessionError::NotFinite(_) => Key::ErrorNotFinite.get(language).to_string(),
        SessionError::NotGenerated(_) => Key::ErrorNotGenerated.get(language).to_string(),
        SessionError::NothingToAccompany(_) => {
            Key::ErrorNothingToAccompany.get(language).to_string()
        }
        SessionError::WrongTrackKind { .. } => Key::ErrorWrongTrackKind.get(language).to_string(),
        SessionError::NoPath => Key::ErrorNoPath.get(language).to_string(),
        SessionError::RecordingNeedsFolder => {
            Key::ErrorRecordingNeedsFolder.get(language).to_string()
        }
        SessionError::NothingToRecordOnto => {
            Key::ErrorNothingToRecordOnto.get(language).to_string()
        }
        SessionError::AlreadyRecording => Key::ErrorAlreadyRecording.get(language).to_string(),
        SessionError::NothingToStem => Key::ErrorNothingToStem.get(language).to_string(),
        SessionError::NotOverlapping => Key::ErrorNotOverlapping.get(language).to_string(),
        SessionError::TooManyMonitors { limit } => messages::too_many_monitors(language, *limit),
        SessionError::RecordingInProgress => {
            Key::ErrorRecordingInProgress.get(language).to_string()
        }
        SessionError::NotRecording => Key::ErrorNotRecording.get(language).to_string(),
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

/// Which of the two names a plugin gets — an insert's or a track's, the rule is the same.
///
/// The hosted one wins whenever there is one. A free function because the alternative is a rule
/// that only exists inside a `Render` implementation, which is the one place it cannot be checked.
pub(crate) fn hosted_label(hosted: Option<&str>, from_registry: String) -> String {
    match hosted {
        // A plugin that gives no name at all is not improved by an empty label.
        Some(name) if !name.trim().is_empty() => name.to_string(),
        _ => from_registry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_i18n::audio;
    use auris_session::plugin_catalogue;

    #[test]
    fn a_hosted_plugin_is_named_by_itself_and_not_by_its_id() {
        assert_eq!(
            hosted_label(
                Some("Surge XT Effects"),
                "clap:org.surge-synth-team.surge-xt-fx".into()
            ),
            "Surge XT Effects"
        );
        // A built-in has no hosted name and keeps the translated one.
        assert_eq!(hosted_label(None, "Reverb".into()), "Reverb");
        // And a plugin that answers with nothing does not get a blank row.
        assert_eq!(
            hosted_label(Some("   "), "auris.dsp.gain".into()),
            "auris.dsp.gain"
        );
    }

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
    fn every_progression_has_a_name_short_enough_to_pick_from_a_menu() {
        // The pickers used to show the *description*, which is a sentence: sixteen rows reading
        // "王道進行 (4536): the J-pop staple" is a menu nobody can scan. They show the name now,
        // and a name missing from the table falls back to the catalogue slug — `royal-road`,
        // which is the vocabulary of a file rather than of a person. This is the only place the
        // catalogue and the table can be checked against each other, for the same reason the
        // test above it exists.
        for entry in auris_session::prelude::progression_catalog() {
            for language in Language::ALL {
                let shown = audio::theory_name(entry.name, language);
                assert_ne!(
                    shown, entry.name,
                    "{:?} has no {language:?} name and would show its slug",
                    entry.name
                );
                // Long enough to be a sentence is long enough to be the old bug back again. The
                // longest legitimate one is 丸サ進行 (ii–V) at fifteen.
                assert!(
                    shown.chars().count() <= 24,
                    "{:?} shows {shown:?} in {language:?}, which is a description and not a name",
                    entry.name
                );
            }
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
