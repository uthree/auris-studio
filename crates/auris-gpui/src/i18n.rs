//! Where the interface gets its words.
//!
//! The tables live in `auris-i18n`; what belongs here is the mapping from the *backend's* types
//! to those tables. A [`SessionError`] or an [`Edit`] is data the session hands over, and only a
//! frontend knows it has to become a sentence — so every such mapping is an exhaustive `match`
//! in this file, which makes a new variant a compile error rather than an English string leaking
//! into a Japanese window.

use auris_i18n::{Key, Language, audio, messages};
use auris_session::prelude::*;
use auris_session::{CoreError, Edit, EngineError, IoError, SessionError};

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
        Edit::ExternalChanges => Key::EditExternalChanges,
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
        Edit::ComposeLyrics => Key::EditComposeLyrics,
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
        Edit::SetSingerSpeaker => Key::EditSetSingerSpeaker,
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
/// Workspace-owned errors are translated variant by variant. Details supplied by a decoder,
/// device driver, parser, or the operating system keep their original text.
pub fn error_text(error: &SessionError, language: Language) -> String {
    let with = |key: Key, detail: String| messages::detailed(language, key.get(language), &detail);
    match error {
        SessionError::InvalidCheckpointName => Key::ErrorCheckpointName.get(language).to_string(),
        SessionError::ExternalChanges(_) => Key::ExternalChangeConflict.get(language).to_string(),
        SessionError::EditInProgress => Key::ErrorEditInProgress.get(language).to_string(),
        SessionError::Io(inner) => with(Key::ErrorFile, io_error_text(inner, language)),
        SessionError::Engine(inner) => with(Key::ErrorEngine, engine_error_text(inner, language)),
        // The plugin's own words: it names a file, an id or a refusal that came from somebody
        // else's binary, and paraphrasing that would lose the only part worth reading.
        SessionError::Clap(inner) => with(Key::ErrorPlugin, inner.to_string()),
        SessionError::Vst3(inner) => with(Key::ErrorPlugin, inner.to_string()),
        SessionError::Core(inner) => with(Key::ErrorDocument, core_error_text(inner, language)),
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
        // The names it offers are the model's own and read the same in every language; the
        // sentence joining them is ours and is translated here.
        SessionError::NoSuchSpeaker { name, offered } => {
            messages::no_such_speaker(language, name, &offered.join(", "))
        }
        SessionError::SingingNeedsFolder => Key::ErrorSingingNeedsFolder.get(language).to_string(),
        SessionError::NothingToSing(_) => Key::ErrorNothingToSing.get(language).to_string(),
        SessionError::NoLyrics => Key::ErrorNoLyrics.get(language).to_string(),
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

fn translated(language: Language, english: String, japanese: String) -> String {
    match language {
        Language::English => english,
        Language::Japanese => japanese,
    }
}

fn core_error_text(error: &CoreError, language: Language) -> String {
    let (english, japanese) = match error {
        CoreError::RaggedChannels {
            channel,
            found,
            expected,
        } => (
            format!("channel {channel} has {found} frames but the buffer has {expected}"),
            format!("チャンネル{channel}は{found}フレームですが、バッファは{expected}フレームです"),
        ),
        CoreError::NoChannels => (
            "audio buffer must have at least one channel".to_string(),
            "オーディオバッファには1つ以上のチャンネルが必要です".to_string(),
        ),
        CoreError::LayoutMismatch(detail) => (
            format!("buffer layout mismatch: {detail}"),
            format!("バッファのレイアウトが一致しません: {detail}"),
        ),
        CoreError::UnknownPlugin(id) => (
            format!("unknown plugin id `{id}`"),
            format!("不明なプラグインIDです: `{id}`"),
        ),
        CoreError::UnknownId { kind, id } => (
            format!("unknown {kind} id {id}"),
            format!("不明な{kind} IDです: {id}"),
        ),
        CoreError::InvalidTempoMap(detail) => (
            format!("invalid tempo map: {detail}"),
            format!("テンポマップが不正です: {detail}"),
        ),
        CoreError::InvalidTimeSignature(value) => (
            format!("`{value}` is not a time signature like 4/4"),
            format!("`{value}`は4/4のような拍子記号ではありません"),
        ),
        CoreError::InvalidAutomationLane(detail) => (
            format!("invalid automation lane: {detail}"),
            format!("オートメーションレーンが不正です: {detail}"),
        ),
    };
    translated(language, english, japanese)
}

fn engine_error_text(error: &EngineError, language: Language) -> String {
    let (english, japanese) = match error {
        EngineError::HostUnavailable(host) => (
            format!("audio driver `{host}` is not available"),
            format!("オーディオドライバー `{host}` は利用できません"),
        ),
        EngineError::NoOutputDevice => (
            "no default audio output device is available".to_string(),
            "既定のオーディオ出力デバイスがありません".to_string(),
        ),
        EngineError::NoInputDevice => (
            "no audio input device is available".to_string(),
            "オーディオ入力デバイスがありません".to_string(),
        ),
        EngineError::UnsupportedSampleFormat(format) => (
            format!("unsupported sample format `{format}`"),
            format!("未対応のサンプル形式です: `{format}`"),
        ),
        EngineError::Backend(detail) => (
            format!("audio backend error: {detail}"),
            format!("オーディオバックエンドのエラー: {detail}"),
        ),
        EngineError::Core(inner) => return core_error_text(inner, language),
        EngineError::InvalidRange { start, end } => (
            format!("invalid render range: start {start} is past end {end}"),
            format!("レンダー範囲が不正です: 開始{start}が終了{end}を超えています"),
        ),
        EngineError::InvalidSampleRate(rate) => (
            format!("invalid sample rate {rate}"),
            format!("サンプルレートが不正です: {rate}"),
        ),
        EngineError::RenderTooLong { frames, limit } => (
            format!("render span of {frames} frames exceeds the {limit} frame limit"),
            format!("{frames}フレームのレンダー範囲が上限{limit}フレームを超えています"),
        ),
        EngineError::CommandQueueFull => (
            "the engine command queue is full".to_string(),
            "エンジンのコマンドキューがいっぱいです".to_string(),
        ),
        EngineError::NotRunning => (
            "the audio engine is not running".to_string(),
            "オーディオエンジンが動作していません".to_string(),
        ),
        EngineError::RenderCancelled => (
            "the render was cancelled".to_string(),
            "レンダーはキャンセルされました".to_string(),
        ),
    };
    translated(language, english, japanese)
}

fn io_error_text(error: &IoError, language: Language) -> String {
    let (english, japanese) = match error {
        IoError::FileNotFound(path) => (
            format!("file not found: {}", path.display()),
            format!("ファイルが見つかりません: {}", path.display()),
        ),
        IoError::UnsupportedFormat(detail) => (
            format!("unsupported audio format: {detail}"),
            format!("未対応のオーディオ形式です: {detail}"),
        ),
        IoError::Decode(detail) => (
            format!("failed to decode audio: {detail}"),
            format!("オーディオをデコードできませんでした: {detail}"),
        ),
        IoError::Resample(detail) => (
            format!("failed to resample audio: {detail}"),
            format!("オーディオをリサンプルできませんでした: {detail}"),
        ),
        IoError::WavWrite(detail) => (
            format!("failed to write WAV file: {detail}"),
            format!("WAVファイルを書き込めませんでした: {detail}"),
        ),
        IoError::MidiParse(detail) => (
            format!("failed to read MIDI file: {detail}"),
            format!("MIDIファイルを読み込めませんでした: {detail}"),
        ),
        IoError::MidiWrite(detail) => (
            format!("failed to write MIDI file: {detail}"),
            format!("MIDIファイルを書き込めませんでした: {detail}"),
        ),
        IoError::MidiTimecode { fps, subframe } => (
            format!(
                "this MIDI file counts time in SMPTE frames ({fps} fps, {subframe} subframes) rather than in beats, so it has no musical positions to import"
            ),
            format!(
                "このMIDIファイルは拍ではなくSMPTEフレーム（{fps} fps、{subframe}サブフレーム）で時間を数えるため、読み込める音楽的位置がありません"
            ),
        ),
        IoError::Json(detail) => (
            format!("project JSON error: {detail}"),
            format!("プロジェクトJSONのエラー: {detail}"),
        ),
        IoError::ProjectVersionMismatch { found, supported } => (
            format!(
                "project format version {found} is newer than the supported version {supported}; update Auris Studio to open this project"
            ),
            format!(
                "プロジェクト形式のバージョン{found}は対応上限{supported}より新しいため、Auris Studioを更新してください"
            ),
        ),
        IoError::ProjectIdsExhausted => (
            "project object ids have exhausted their supported range".to_string(),
            "プロジェクトのオブジェクトIDが対応範囲を使い切りました".to_string(),
        ),
        IoError::Filesystem { path, source } => (
            format!("I/O error on {}: {source}", path.display()),
            format!("{}でI/Oエラーが発生しました: {source}", path.display()),
        ),
    };
    translated(language, english, japanese)
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
    fn workspace_owned_error_details_are_japanese() {
        for (error, english) in [
            (
                SessionError::Engine(EngineError::NoOutputDevice),
                "no default audio output device",
            ),
            (
                SessionError::Core(CoreError::NoChannels),
                "audio buffer must have",
            ),
            (
                SessionError::Io(IoError::FileNotFound("lost.wav".into())),
                "file not found",
            ),
        ] {
            let japanese = error_text(&error, Language::Japanese);
            assert!(!japanese.contains(english), "{japanese}");
        }
    }

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
            SessionError::RecordingInProgress,
            SessionError::NoSuchSpeaker {
                name: "Alice".into(),
                offered: vec!["Bob".into(), "Carol".into()],
            },
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

        let japanese = error_text(
            &SessionError::NoSuchSpeaker {
                name: "Alice".into(),
                offered: vec!["Bob".into(), "Carol".into()],
            },
            Language::Japanese,
        );
        assert!(japanese.contains("Alice") && japanese.contains("Bob, Carol"));
        assert!(!japanese.contains("the voice has no speaker"), "{japanese}");
        assert!(
            !error_text(&SessionError::RecordingInProgress, Language::Japanese)
                .contains("recording")
        );
    }
}
