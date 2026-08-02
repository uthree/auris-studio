//! Every fixed string in the interface, in every language.
//!
//! One entry per string, with the translations on adjacent lines. The macro turns each into an
//! enum variant and an arm of one exhaustive `match`, so a language missing a string does not
//! compile — the alternative, a map with a fallback, hides the hole until someone sees it.

use crate::Language;

macro_rules! strings {
    ($($key:ident { en: $en:literal, ja: $ja:literal })*) => {
        /// A fixed string in the interface.
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub enum Key {
            $(
                #[doc = $en]
                $key,
            )*
        }

        impl Key {
            /// Every key, for tests that need to sweep the table.
            pub const ALL: &'static [Key] = &[$(Key::$key,)*];

            /// This string in `language`.
            pub fn get(self, language: Language) -> &'static str {
                match (self, language) {
                    $(
                        (Key::$key, Language::English) => $en,
                        (Key::$key, Language::Japanese) => $ja,
                    )*
                }
            }
        }
    };
}

strings! {
    // ------------------------------------------------------------------ transport bar
    ExportWav { en: "Export WAV", ja: "WAV 書き出し" }
    Grid { en: "Grid", ja: "グリッド" }
    GridFree { en: "free", ja: "自由" }
    Position { en: "Position", ja: "位置" }
    Tempo { en: "Tempo", ja: "テンポ" }
    PianoRoll { en: "Piano Roll", ja: "ピアノロール" }
    Mixer { en: "Mixer", ja: "ミキサー" }
    Inspector { en: "Inspector", ja: "インスペクタ" }
    Master { en: "Master", ja: "マスター" }

    // ------------------------------------------------------------------ arrangement
    AddInstrumentShort { en: "Inst", ja: "音源" }
    AddAudioShort { en: "Audio", ja: "音声" }
    MuteInitial { en: "M", ja: "M" }
    SoloInitial { en: "S", ja: "S" }
    Volume { en: "Vol", ja: "音量" }
    Pan { en: "Pan", ja: "パン" }
    TrackKindInstrument { en: "Instrument", ja: "ソフト音源" }
    TrackKindAudio { en: "Audio", ja: "オーディオ" }

    // ------------------------------------------------------------------ piano roll
    PianoRollHint {
        en: "alt-click: add note · drag: move · right edge: resize · right-click: menu",
        ja: "alt+クリック: ノート追加 · ドラッグ: 移動 · 右端: 長さ変更 · 右クリック: メニュー"
    }
    PianoRollEmpty {
        en: "Select a MIDI clip to edit its notes (alt-click an empty lane to make one)",
        ja: "MIDI クリップを選ぶとノートを編集できます（空のレーンを alt+クリックで新規作成）"
    }

    // ------------------------------------------------------------------ mixer
    Mute { en: "Mute", ja: "ミュート" }
    Solo { en: "Solo", ja: "ソロ" }
    Effect { en: "Effect", ja: "エフェクト" }

    // ------------------------------------------------------------------ inspector
    Track { en: "Track", ja: "トラック" }
    Plugins { en: "Plugins", ja: "プラグイン" }
    Instrument { en: "Instrument", ja: "音源" }
    Effects { en: "Effects", ja: "エフェクト" }
    Add { en: "Add", ja: "追加" }
    NoTrackSelected { en: "No track selected", ja: "トラックが選択されていません" }
    NoEffects {
        en: "No effects — add one from the Plugins tab",
        ja: "エフェクトなし — プラグインタブから追加できます"
    }
    BrowserInstruments {
        en: "Instruments — click to set on the selected track",
        ja: "音源 — クリックで選択中のトラックに設定します"
    }
    BrowserEffects {
        en: "Effects — click to add to the selected track",
        ja: "エフェクト — クリックで選択中のトラックに追加します"
    }

    // ------------------------------------------------------------------ window chrome
    Export { en: "Export", ja: "書き出し" }
    Close { en: "Close", ja: "閉じる" }
    EngineSilent { en: "silent", ja: "無音" }
    NoAudioOutput {
        en: "No audio output — editing and export still work",
        ja: "オーディオ出力なし — 編集と書き出しは可能です"
    }

    // ------------------------------------------------------------------ settings window
    Settings { en: "Settings", ja: "設定" }
    TabGeneral { en: "General", ja: "一般" }
    TabAudio { en: "Audio", ja: "オーディオ" }
    TabKeys { en: "Key Bindings", ja: "キー割り当て" }
    LanguageHeading { en: "Language", ja: "言語" }
    LanguageFollowSystem { en: "System", ja: "システムに合わせる" }
    LanguageNote {
        en: "The interface changes as soon as you choose. Plugin names follow where a translation exists.",
        ja: "選んだ時点で表示が切り替わります。プラグイン名は訳がある範囲で追従します。"
    }
    OutputDevice { en: "Output Device", ja: "出力デバイス" }
    SystemDefaultDevice { en: "System Default", ja: "システムのデフォルト" }
    SystemDefaultDeviceDetail {
        en: "Follows whatever macOS is set to",
        ja: "macOS の設定に追従します"
    }
    SampleRate { en: "Sample Rate", ja: "サンプルレート" }
    DeviceDefaultRate { en: "Device Default", ja: "デバイス標準" }
    BufferSize { en: "Buffer Size", ja: "バッファサイズ" }
    RateUnknown { en: "rate unknown", ja: "レート不明" }
    DeviceIsDefault { en: "default", ja: "既定" }
    RestoreDefaults { en: "Restore Defaults", ja: "既定に戻す" }
    PressAKey { en: "Press a key…", ja: "キーを押してください…" }
    CaptureCancelled { en: "Cancelled", ja: "キャンセルしました" }
    BindingsRestored {
        en: "Key bindings restored to defaults",
        ja: "キー割り当てを既定に戻しました"
    }

    // ------------------------------------------------------------------ command groups
    GroupTransport { en: "Transport", ja: "トランスポート" }
    GroupFile { en: "File", ja: "ファイル" }
    GroupEdit { en: "Edit", ja: "編集" }
    GroupTrack { en: "Track", ja: "トラック" }
    GroupView { en: "View", ja: "表示" }

    // ------------------------------------------------------------------ commands
    CmdPlayStop { en: "Play / Stop", ja: "再生 / 停止" }
    CmdReturnToZero { en: "Return to Zero", ja: "先頭に戻る" }
    CmdToggleCycle { en: "Toggle Cycle", ja: "サイクル切り替え" }
    CmdPanic { en: "Panic", ja: "パニック" }
    CmdNewProject { en: "New Project", ja: "新規プロジェクト" }
    CmdOpenProject { en: "Open Project", ja: "プロジェクトを開く" }
    CmdSave { en: "Save", ja: "保存" }
    CmdSaveAs { en: "Save As", ja: "名前を付けて保存" }
    CmdImportAudio { en: "Import Audio", ja: "オーディオを読み込む" }
    CmdExportWav { en: "Export WAV", ja: "WAV を書き出す" }
    CmdQuit { en: "Quit", ja: "終了" }
    CmdUndo { en: "Undo", ja: "取り消す" }
    CmdRedo { en: "Redo", ja: "やり直す" }
    CmdDeleteSelection { en: "Delete Selection", ja: "選択範囲を削除" }
    CmdAddInstrumentTrack { en: "Add Instrument Track", ja: "ソフト音源トラックを追加" }
    CmdAddAudioTrack { en: "Add Audio Track", ja: "オーディオトラックを追加" }
    CmdDeleteTrack { en: "Delete Track", ja: "トラックを削除" }
    CmdShowInspector { en: "Show Inspector", ja: "インスペクタを表示" }
    CmdShowEditor { en: "Show Editor Panel", ja: "エディタパネルを表示" }
    CmdZoomIn { en: "Zoom In", ja: "拡大" }
    CmdZoomOut { en: "Zoom Out", ja: "縮小" }
    CmdSettings { en: "Settings", ja: "設定" }

    // ------------------------------------------------------------------ application menu
    // Separate from the commands above because a menu item that opens a window or a dialog
    // carries an ellipsis, and the same command has no ellipsis on a button.
    MenuSettingsItem { en: "Settings…", ja: "設定…" }
    MenuServices { en: "Services", ja: "サービス" }
    MenuQuitApp { en: "Quit Auris Studio", ja: "Auris Studio を終了" }
    MenuOpenProjectItem { en: "Open Project…", ja: "プロジェクトを開く…" }
    MenuSaveAsItem { en: "Save As…", ja: "名前を付けて保存…" }
    MenuImportAudioItem { en: "Import Audio…", ja: "オーディオを読み込む…" }
    MenuExportWavItem { en: "Export WAV…", ja: "WAV を書き出す…" }
    MenuDelete { en: "Delete", ja: "削除" }

    // ------------------------------------------------------------------ context menus
    MenuArrangement { en: "Arrangement", ja: "アレンジ" }
    MenuNote { en: "Note", ja: "ノート" }
    MenuCycleTitle { en: "Cycle", ja: "サイクル" }
    MenuDuplicateTrack { en: "Duplicate Track", ja: "トラックを複製" }
    MenuRename { en: "Rename…", ja: "名前を変更…" }
    MenuRenameTrack { en: "Rename Track…", ja: "トラック名を変更…" }
    MenuRenameClip { en: "Rename Clip…", ja: "クリップ名を変更…" }
    MenuAddEffect { en: "Add Effect…", ja: "エフェクトを追加…" }
    MenuNewInstrumentTrack { en: "New Instrument Track", ja: "新規ソフト音源トラック" }
    MenuNewAudioTrack { en: "New Audio Track", ja: "新規オーディオトラック" }
    MenuDuplicate { en: "Duplicate", ja: "複製" }
    MenuSplitAtPlayhead { en: "Split at Playhead", ja: "再生位置で分割" }
    MenuMuteClip { en: "Mute Clip", ja: "クリップをミュート" }
    MenuCycleOverClip { en: "Cycle over Clip", ja: "クリップをサイクル範囲に" }
    MenuEditInPianoRoll { en: "Edit in Piano Roll", ja: "ピアノロールで編集" }
    MenuNewClipHere { en: "New Clip Here", ja: "ここに新規クリップ" }
    MenuCycleStartHere { en: "Cycle Start Here", ja: "ここをサイクル開始に" }
    MenuCycleEndHere { en: "Cycle End Here", ja: "ここをサイクル終了に" }
    MenuClearCycle { en: "Clear Cycle Region", ja: "サイクル範囲を消去" }
    MenuOctaveUp { en: "Octave Up", ja: "1 オクターブ上げる" }
    MenuOctaveDown { en: "Octave Down", ja: "1 オクターブ下げる" }
    MenuSemitoneUp { en: "Semitone Up", ja: "半音上げる" }
    MenuSemitoneDown { en: "Semitone Down", ja: "半音下げる" }
    MenuAddNoteHere { en: "Add Note Here", ja: "ここにノートを追加" }
    MenuSelectAllNotes { en: "Select All Notes", ja: "すべてのノートを選択" }
    MenuEnabled { en: "Enabled", ja: "有効" }
    MenuMoveUp { en: "Move Up", ja: "上へ移動" }
    MenuMoveDown { en: "Move Down", ja: "下へ移動" }
    MenuRemove { en: "Remove", ja: "削除" }

    // ------------------------------------------------------------------ rename sheet
    RenameTrackTitle { en: "Rename track", ja: "トラック名の変更" }
    RenameClipTitle { en: "Rename clip", ja: "クリップ名の変更" }
    Cancel { en: "Cancel", ja: "キャンセル" }
    Rename { en: "Rename", ja: "変更" }
    NameCannotBeEmpty { en: "Name cannot be empty", ja: "名前を空にはできません" }

    // ------------------------------------------------------------------ statuses
    NothingToUndo { en: "Nothing to undo", ja: "取り消せる操作がありません" }
    NothingToRedo { en: "Nothing to redo", ja: "やり直せる操作がありません" }
    NewProjectStatus { en: "New project", ja: "新規プロジェクト" }
    PanicStopped { en: "Panic — all voices stopped", ja: "パニック — すべての発音を停止しました" }
    ExportAlreadyRunning { en: "An export is already running", ja: "すでに書き出しを実行中です" }
    AudioClipsComeFromImport {
        en: "Audio clips come from Import Audio, not from an empty lane",
        ja: "オーディオクリップは空のレーンからではなく読み込みで作成します"
    }
    DuplicatedTrack { en: "Duplicated track", ja: "トラックを複製しました" }
    DuplicatedClip { en: "Duplicated clip", ja: "クリップを複製しました" }
    SplitClipStatus { en: "Split clip", ja: "クリップを分割しました" }

    // ------------------------------------------------------------------ undo labels
    // What the user sees after "Undid …". Nouns rather than imperatives, because that is how
    // both languages name a step that has already happened.
    EditToggleLoop { en: "toggling the cycle", ja: "サイクルの切り替え" }
    EditSetLoopRegion { en: "setting the cycle region", ja: "サイクル範囲の設定" }
    EditChangeTempo { en: "the tempo change", ja: "テンポの変更" }
    EditAddInstrumentTrack { en: "adding an instrument track", ja: "ソフト音源トラックの追加" }
    EditAddAudioTrack { en: "adding an audio track", ja: "オーディオトラックの追加" }
    EditDeleteTrack { en: "deleting a track", ja: "トラックの削除" }
    EditDuplicateTrack { en: "duplicating a track", ja: "トラックの複製" }
    EditRenameTrack { en: "renaming a track", ja: "トラック名の変更" }
    EditMuteTrack { en: "muting a track", ja: "トラックのミュート" }
    EditSoloTrack { en: "soloing a track", ja: "トラックのソロ" }
    EditChangeInstrument { en: "changing the instrument", ja: "音源の変更" }
    EditAddClip { en: "adding a clip", ja: "クリップの追加" }
    EditDeleteClip { en: "deleting a clip", ja: "クリップの削除" }
    EditDuplicateClip { en: "duplicating a clip", ja: "クリップの複製" }
    EditSplitClip { en: "splitting a clip", ja: "クリップの分割" }
    EditRenameClip { en: "renaming a clip", ja: "クリップ名の変更" }
    EditMuteClip { en: "muting a clip", ja: "クリップのミュート" }
    EditMoveClip { en: "moving a clip", ja: "クリップの移動" }
    EditResizeClip { en: "resizing a clip", ja: "クリップの長さ変更" }
    EditAddNote { en: "adding a note", ja: "ノートの追加" }
    EditDeleteNotes { en: "deleting notes", ja: "ノートの削除" }
    EditDuplicateNotes { en: "duplicating notes", ja: "ノートの複製" }
    EditTransposeNotes { en: "transposing notes", ja: "ノートの移調" }
    EditMoveNotes { en: "moving notes", ja: "ノートの移動" }
    EditResizeNote { en: "resizing a note", ja: "ノートの長さ変更" }
    EditAddEffect { en: "adding an effect", ja: "エフェクトの追加" }
    EditRemoveEffect { en: "removing an effect", ja: "エフェクトの削除" }
    EditBypassEffect { en: "bypassing an effect", ja: "エフェクトのバイパス" }
    EditReorderEffects { en: "reordering the effects", ja: "エフェクトの並べ替え" }
    EditAdjustParameter { en: "the parameter change", ja: "パラメーターの変更" }
    EditImportAudio { en: "importing audio", ja: "オーディオの読み込み" }

    // ------------------------------------------------------------------ errors
    ErrorUnknownTrack { en: "that track no longer exists", ja: "そのトラックは存在しません" }
    ErrorUnknownClip { en: "that clip no longer exists", ja: "そのクリップは存在しません" }
    ErrorCannotSplit {
        en: "a clip can only be split inside itself",
        ja: "クリップの内側でしか分割できません"
    }
    ErrorWrongTrackKind {
        en: "that command does not apply to this kind of track",
        ja: "この種類のトラックには使えない操作です"
    }
    ErrorNoPath {
        en: "the project has no path yet; save it somewhere first",
        ja: "保存先が未設定です。先に保存してください"
    }
    ErrorFile { en: "file error", ja: "ファイルエラー" }
    ErrorEngine { en: "audio engine error", ja: "オーディオエンジンのエラー" }
    ErrorDocument { en: "the document is not valid", ja: "ドキュメントが不正です" }

    // ------------------------------------------------------------------ command line
    CliUsage {
        en: "\
auris — Auris Studio from the command line

USAGE
    auris <command> [options]

COMMANDS
    plugins                       List every registered instrument and effect
    info <project.auris>          Print a project's tracks, clips and duration
    render <project.auris> [opts] Render a project to a WAV file
    new <project.auris> [opts]    Create a project with one instrument track
    help                          Show this message

RENDER OPTIONS
    -o, --output <file.wav>       Where to write (default: alongside the project)
        --bit-depth <16|24|32>    Sample format; 32 means 32-bit float (default: 24)
        --dither                  Add TPDF dither, for 16-bit masters
        --no-tail                 Stop at the last clip instead of letting effect tails ring

NEW OPTIONS
        --bpm <tempo>             Tempo of the new project (default: 120)
        --sample-rate <hz>        Rate of the new project (default: 48000)",
        ja: "\
auris — コマンドラインから使う Auris Studio

使い方
    auris <コマンド> [オプション]

コマンド
    plugins                       登録済みの音源とエフェクトを一覧表示
    info <project.auris>          プロジェクトのトラック・クリップ・長さを表示
    render <project.auris> [opts] プロジェクトを WAV に書き出す
    new <project.auris> [opts]    ソフト音源トラック 1 本のプロジェクトを作成
    help                          このメッセージを表示

render のオプション
    -o, --output <file.wav>       出力先（既定: プロジェクトと同じ場所）
        --bit-depth <16|24|32>    量子化ビット数。32 は 32bit float（既定: 24）
        --dither                  TPDF ディザを付加（16bit マスター向け）
        --no-tail                 エフェクトの残響を待たず最後のクリップで終える

new のオプション
        --bpm <tempo>             新規プロジェクトのテンポ（既定: 120）
        --sample-rate <hz>        新規プロジェクトのサンプルレート（既定: 48000）"
    }
    CliInstruments { en: "INSTRUMENTS", ja: "音源" }
    CliEffects { en: "EFFECTS", ja: "エフェクト" }
    CliExpectedProjectPath { en: "expected a project path", ja: "プロジェクトのパスを指定してください" }
    CliExpectedNewPath {
        en: "expected a path for the new project",
        ja: "作成先のパスを指定してください"
    }
    CliFieldPath { en: "path", ja: "パス" }
    CliFieldTempo { en: "tempo", ja: "テンポ" }
    CliFieldSampleRate { en: "sample rate", ja: "サンプルレート" }
    CliNeedsPath { en: "a path", ja: "パス" }
    CliNeedsNumber { en: "a number", ja: "数値" }
    CliFieldSignature { en: "signature", ja: "拍子" }
    CliMaster { en: "master", ja: "マスター" }
    CliFieldDuration { en: "duration", ja: "長さ" }
    CliFieldTracks { en: "tracks", ja: "トラック数" }
    CliKindInstrument { en: "instrument", ja: "音源" }
    CliKindAudio { en: "audio", ja: "オーディオ" }
    CliClipCount { en: "clip(s)", ja: "クリップ" }

    // ------------------------------------------------------------------ parameter values
    ValueOn { en: "On", ja: "オン" }
    ValueOff { en: "Off", ja: "オフ" }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_text_in_every_language() {
        for key in Key::ALL {
            for language in Language::ALL {
                let text = key.get(language);
                assert!(!text.trim().is_empty(), "{key:?} is blank in {language:?}");
            }
        }
    }

    #[test]
    fn no_key_is_left_untranslated_by_accident() {
        // Identical text in both languages is legitimate for a few strings — "M" is "M" — but
        // it is nearly always a forgotten translation, so the exceptions are listed rather than
        // waved through.
        const SHARED: &[Key] = &[Key::MuteInitial, Key::SoloInitial];
        for key in Key::ALL {
            if SHARED.contains(key) {
                continue;
            }
            assert_ne!(
                key.get(Language::English),
                key.get(Language::Japanese),
                "{key:?} has the same text in both languages"
            );
        }
    }

    #[test]
    fn an_ellipsis_survives_translation() {
        // A menu item that opens a dialog says so with a trailing ellipsis, and a translation
        // that drops it turns "opens a window" into "does it now".
        for key in Key::ALL {
            let english = key.get(Language::English);
            if english.ends_with('…') {
                assert!(
                    key.get(Language::Japanese).ends_with('…'),
                    "{key:?} loses its ellipsis in Japanese"
                );
            }
        }
    }
}
