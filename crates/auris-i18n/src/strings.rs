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
                // Fenced rather than written straight into the documentation, because these are
                // *strings*, not prose: the command line usage text contains `<command>` and
                // `[options]`, which rustdoc would otherwise read as an HTML tag and a link to a
                // type called `options`. A block also keeps the alignment of a multi-line string,
                // which is the whole point of the ones that have any.
                #[doc = concat!("```text\n", $en, "\n```")]
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
    Zoom { en: "Zoom", ja: "拡大" }
    GridFree { en: "free", ja: "自由" }
    Position { en: "Position", ja: "位置" }
    Tempo { en: "Tempo", ja: "テンポ" }
    Signature { en: "Signature", ja: "拍子" }
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

    // The piano roll's own hints name the gesture that is actually bound, so they are in
    // `messages` and take it as an argument. Fixed strings naming a modifier used to live here
    // and were wrong twice over: the gesture became configurable, and ⌥ is not what a Windows
    // keyboard calls that key.

    // ------------------------------------------------------------------ mixer
    Mute { en: "Mute", ja: "ミュート" }
    Solo { en: "Solo", ja: "ソロ" }
    Effect { en: "Effect", ja: "エフェクト" }

    // ------------------------------------------------------------------ library and channel strip
    Track { en: "Track", ja: "トラック" }
    Instrument { en: "Instrument", ja: "音源" }
    Effects { en: "Effects", ja: "エフェクト" }
    NoTrackSelected { en: "No track selected", ja: "トラックが選択されていません" }
    Library { en: "Library", ja: "ライブラリ" }
    // Not `Instrument`'s Japanese: the two sit next to each other, and
    // `no_key_is_left_untranslated_by_accident` compares English against Japanese within a key
    // rather than key against key, so a duplicate would pass and read as a bug in the window.
    Instruments { en: "Instruments", ja: "音源一覧" }
    Inserts { en: "Inserts", ja: "インサート" }
    LibraryNeedsInstrumentTrack {
        en: "Select an instrument track to load one",
        ja: "音源トラックを選択すると読み込めます"
    }
    // The heading names the section and nothing else. It used to carry the instruction — "click
    // to set on the selected track" — on the one row where clicking does something quite
    // different: it collapses the section the instruction is about.
    BrowserInstruments { en: "Instruments", ja: "音源" }
    BrowserEffects { en: "Effects", ja: "エフェクト" }
    BrowserSoundFonts { en: "SoundFonts", ja: "サウンドフォント" }
    // Under the heading, where it belongs: a line about the rows rather than about the row it is
    // written on.
    BrowserInstrumentsHint {
        en: "Click a sound to set it on the selected track",
        ja: "音色をクリックすると選択中のトラックに設定されます"
    }
    BrowserEffectsHint {
        en: "Click an effect to add it to the selected track",
        ja: "エフェクトをクリックすると選択中のトラックに追加されます"
    }
    LibraryNeedsTrack {
        en: "No track selected — effects will go to the master bus",
        ja: "トラックが未選択です — エフェクトはマスターに追加されます"
    }
    BrowserNoSoundFonts {
        en: "None imported yet",
        ja: "まだ読み込まれていません"
    }
    BrowserFontFileMissing {
        en: "file not found",
        ja: "ファイルが見つかりません"
    }
    BrowserBank {
        en: "Bank",
        ja: "バンク"
    }
    BrowserPercussionBank {
        en: "Percussion",
        ja: "パーカッション"
    }
    BrowserFontHasNoSounds {
        en: "no sounds in this font",
        ja: "この音源に音色がありません"
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
        en: "Follows whatever the system is set to",
        ja: "システムの設定に追従します"
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

    // ------------------------------------------------------------------ pointer gestures
    PointerHeading { en: "Pointer", ja: "ポインタ操作" }
    PointerCreate { en: "Create a note or clip", ja: "ノート・クリップを作成" }
    PointerDelete { en: "Delete what is under the pointer", ja: "ポインタ位置のものを削除" }
    // Two names for each modifier gesture: the frontend picks by platform, because the glyphs
    // are Apple's and a Windows keyboard has neither of them printed on it.
    GestureCommandClick { en: "⌘-click", ja: "⌘＋クリック" }
    GestureOptionClick { en: "⌥-click", ja: "⌥＋クリック" }
    GestureControlClick { en: "Ctrl-click", ja: "Ctrl＋クリック" }
    GestureAltClick { en: "Alt-click", ja: "Alt＋クリック" }
    GestureDoubleClick { en: "Double-click", ja: "ダブルクリック" }
    // The piano roll's tools, named as Logic names them — this is the vocabulary a user arrives
    // with, and the tool is the only way in: the modifier Logic puts velocity on cannot reach a
    // window at all on macOS, where ⌃ and a click become a request for the context menu.
    ToolPointer { en: "Pointer", ja: "ポインタ" }
    ToolVelocity { en: "Velocity", ja: "ベロシティ" }
    CmdNextTool { en: "Next Tool", ja: "次のツール" }
    CmdSetTempo { en: "Tempo…", ja: "テンポ…" }
    CmdSetSignature { en: "Time Signature…", ja: "拍子…" }
    CmdCycleGrid { en: "Next Grid Division", ja: "次のグリッド分割" }
    CmdGoToPosition { en: "Go to Position…", ja: "位置へ移動…" }
    CmdOpenMenuBar { en: "Open Menu Bar", ja: "メニューバーを開く" }
    CmdFocusNextPane { en: "Focus Next Panel", ja: "次のパネルにフォーカス" }
    CmdFocusPreviousPane { en: "Focus Previous Panel", ja: "前のパネルにフォーカス" }
    // Where a key binding reaches. Shown beside the commands that are not reachable everywhere.
    ScopeLibrary { en: "in the Library", ja: "ライブラリ内" }
    ScopeArrangement { en: "in the Arrangement", ja: "アレンジ内" }
    ScopeRoll { en: "in the Piano Roll", ja: "ピアノロール内" }
    ScopeMixer { en: "in the Mixer", ja: "ミキサー内" }
    ScopeInspector { en: "in the Inspector", ja: "インスペクタ内" }
    SearchCommands { en: "Search commands", ja: "コマンドを検索" }
    AddKeystroke { en: "Add another key", ja: "キーを追加" }
    UnbindCommand { en: "Use no key", ja: "キーを割り当てない" }
    NoKeystroke { en: "—", ja: "―" }
    RestoreGroup { en: "Restore this group", ja: "このグループを既定に戻す" }
    NothingMatchesSearch { en: "No command matches.", ja: "一致するコマンドがありません。" }
    PointerNote {
        en: "The two cannot share a gesture; picking one that is taken swaps them.",
        ja: "2 つに同じ操作は割り当てられません。使用中のものを選ぶと入れ替わります。"
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
    CmdComposeSong { en: "Compose", ja: "自動作曲" }
    CmdSave { en: "Save", ja: "保存" }
    CmdSaveAs { en: "Save As", ja: "名前を付けて保存" }
    CmdImportAudio { en: "Import Audio", ja: "オーディオを読み込む" }
    CmdImportSoundFont { en: "Import SoundFont", ja: "サウンドフォントを読み込む" }
    CmdCollectAssets { en: "Collect Assets", ja: "アセットを集める" }
    CmdExportWav { en: "Export WAV", ja: "WAV を書き出す" }
    CmdExportCycle { en: "Export Cycle as WAV", ja: "サイクル範囲を WAV に書き出す" }
    CmdQuit { en: "Quit", ja: "終了" }
    CmdUndo { en: "Undo", ja: "取り消す" }
    CmdRedo { en: "Redo", ja: "やり直す" }
    CmdDeleteSelection { en: "Delete Selection", ja: "選択範囲を削除" }
    CmdAddInstrumentTrack { en: "Add Instrument Track", ja: "ソフト音源トラックを追加" }
    CmdAddAudioTrack { en: "Add Audio Track", ja: "オーディオトラックを追加" }
    CmdDeleteTrack { en: "Delete Track", ja: "トラックを削除" }
    // "Show" was a lie on a toggle with no state beside it: choosing Show Inspector hid the
    // inspector. These say what the command does either way.
    CmdShowLibrary { en: "Library", ja: "ライブラリ" }
    CmdShowInspector { en: "Inspector", ja: "インスペクタ" }
    CmdShowPianoRoll { en: "Piano Roll", ja: "ピアノロール" }
    CmdShowMixer { en: "Mixer", ja: "ミキサー" }
    CmdZoomIn { en: "Zoom In", ja: "拡大" }
    CmdZoomOut { en: "Zoom Out", ja: "縮小" }
    CmdSettings { en: "Settings", ja: "設定" }
    CmdCommandPalette { en: "Command Palette", ja: "コマンドパレット" }

    // ------------------------------------------------------------------ application menu
    // Separate from the commands above because a menu item that opens a window or a dialog
    // carries an ellipsis, and the same command has no ellipsis on a button.
    MenuSettingsItem { en: "Settings…", ja: "設定…" }
    MenuServices { en: "Services", ja: "サービス" }
    MenuQuitApp { en: "Quit Auris Studio", ja: "Auris Studio を終了" }
    MenuOpenProjectItem { en: "Open Project…", ja: "プロジェクトを開く…" }
    MenuComposeItem { en: "Compose from Specification…", ja: "仕様ファイルから作曲…" }
    MenuSaveAsItem { en: "Save As…", ja: "名前を付けて保存…" }
    MenuImportAudioItem { en: "Import Audio…", ja: "オーディオを読み込む…" }
    MenuImportSoundFontItem { en: "Import SoundFont…", ja: "サウンドフォントを読み込む…" }
    MenuCollectAssetsItem { en: "Collect Assets into Project", ja: "アセットをプロジェクトにまとめる" }
    MenuExportWavItem { en: "Export WAV…", ja: "WAV を書き出す…" }
    MenuExportCycleItem { en: "Export Cycle…", ja: "サイクル範囲を書き出す…" }
    MenuDelete { en: "Delete", ja: "削除" }

    // ------------------------------------------------------------------ panels and their docks
    // On the menu a right-click on a panel's status-bar icon opens. "Dock" is the verb, so the
    // Japanese is the instruction rather than the noun: 左に配置 is what the row does.
    DockLeft { en: "Dock Left", ja: "左に配置" }
    DockBottom { en: "Dock Bottom", ja: "下に配置" }
    DockRight { en: "Dock Right", ja: "右に配置" }
    HidePanel { en: "Hide Panel", ja: "パネルを隠す" }

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
    MenuHarmony { en: "Harmony", ja: "コード進行" }
    MenuSetKeyHere { en: "Key Here…", ja: "ここから調を…" }
    MenuRemoveKeyHere { en: "Remove Key Change", ja: "転調を取り消す" }
    MenuSetChordHere { en: "Chord Here…", ja: "ここにコードを…" }
    MenuRemoveChordHere { en: "Remove Chord", ja: "コードを削除" }
    MenuSetTempoHere { en: "Tempo from Here…", ja: "ここからのテンポ…" }
    MenuRemoveTempoHere { en: "Remove Tempo Change", ja: "テンポチェンジを削除" }
    MenuSetSignatureHere { en: "Time Signature from Here…", ja: "ここからの拍子…" }
    MenuRemoveSignatureHere { en: "Remove Signature Change", ja: "拍子記号を削除" }
    MenuOtherSignature { en: "Other…", ja: "その他…" }
    MenuClipGain { en: "Clip Gain…", ja: "クリップゲイン…" }
    MenuClearFades { en: "Remove Fades", ja: "フェードを削除" }
    MenuSetSectionHere { en: "Name Section Here…", ja: "ここからのセクション名を…" }
    MenuRemoveSectionHere { en: "Remove Section", ja: "セクションを削除" }
    MenuEndSectionsHere { en: "End Sections Here", ja: "ここで構造を終える" }
    MenuWriteProgression { en: "Write a Progression", ja: "コード進行を書き込む" }
    MenuGenerateClip { en: "Write a Part Here…", ja: "ここにパートを自動生成…" }
    MenuRerollClip { en: "Another Take", ja: "別のテイク" }
    MenuRegenerateClip { en: "Write It Again", ja: "書き直す" }
    MenuFreezeClip { en: "Keep This One", ja: "このテイクで確定" }
    ClipKept { en: "kept — it will not be written again", ja: "確定しました。以降は書き換えません" }
    PresetLead { en: "Lead", ja: "リード" }
    PresetChords { en: "Chords", ja: "コード" }
    PresetPad { en: "Pad", ja: "パッド" }
    PresetArp { en: "Arpeggio", ja: "アルペジオ" }
    PresetBass { en: "Bass", ja: "ベース" }
    // The short chords hammered on every step that dance music runs on. Japanese producers name
    // it after the sound rather than the gesture — a piano with its release cut off — and that
    // is the word anybody looking for it would search for.
    PresetStab { en: "Stab", ja: "リリースカットピアノ" }
    PresetDrums { en: "Drums", ja: "ドラム" }
    MenuClearHarmony { en: "Clear Chords", ja: "コードを消去" }
    NoInstrumentToHearItOn {
        en: "No instrument track to hear it on",
        ja: "鳴らせるソフト音源トラックがありません"
    }

    // ------------------------------------------------------------------ the part inspector
    // The dials on a generated clip's recipe. `Groove` is deliberately not translated into
    // Japanese as 「溝」: the word a drummer uses in either language is the loan word.
    PartHeading { en: "Part", ja: "パート" }
    PartPreset { en: "Preset", ja: "プリセット" }
    PartDensity { en: "Density", ja: "密度" }
    // How long a note sounds for, as a share of the gap to the next one. Not 「ゲート」 alone,
    // which in a Japanese studio is as likely to mean a noise gate as a note length.
    PartGate { en: "Gate", ja: "音の長さ" }
    PartIntensity { en: "Intensity", ja: "強さ" }
    // How much the playing varies, as against how hard it is played. The Japanese is spelled out
    // rather than left as 「ダイナミクス」, which in a studio means the level and not the spread.
    PartDynamics { en: "Dynamics", ja: "強弱の幅" }
    PartSyncopation { en: "Syncopation", ja: "シンコペーション" }
    PartFill { en: "Fill", ja: "フィルイン" }
    PartOctave { en: "Octave", ja: "オクターブ" }
    PartSubdivision { en: "Subdivision", ja: "音符の細かさ" }
    PartSwing { en: "Swing", ja: "スウィング" }
    PartHumanize { en: "Humanize", ja: "ゆらぎ" }
    PartGroove { en: "Groove", ja: "グルーヴ" }
    PartSeed { en: "Seed", ja: "シード" }
    PartStraight { en: "straight", ja: "イーブン" }
    // Note values. Written as the fraction in both languages, because that is what is printed on
    // every other DAW's grid menu and the words for them are longer than the row is wide.
    SubdivisionEighth { en: "1/8", ja: "1/8" }
    SubdivisionSixteenth { en: "1/16", ja: "1/16" }
    SubdivisionEighthTriplet { en: "1/8 triplet", ja: "1/8 三連" }
    SubdivisionSixteenthTriplet { en: "1/16 triplet", ja: "1/16 三連" }

    // ------------------------------------------------------------------ appearance
    AppearanceHeading { en: "Colour scheme", ja: "カラースキーム" }

    // ------------------------------------------------------------------ command palette
    PaletteNothingMatches { en: "No command matches", ja: "該当するコマンドがありません" }

    // ------------------------------------------------------------------ rename sheet
    RenameTrackTitle { en: "Rename track", ja: "トラック名の変更" }
    RenameClipTitle { en: "Rename clip", ja: "クリップ名の変更" }
    SetKeyTitle { en: "Key from here", ja: "ここからの調" }
    SetChordTitle { en: "Chord from here", ja: "ここからのコード" }
    SetSectionTitle { en: "Section from here", ja: "ここからのセクション" }
    SetSeedTitle { en: "Seed for this part", ja: "このパートのシード" }
    SetTempoTitle { en: "Tempo", ja: "テンポ" }
    SetSignatureTitle { en: "Time signature", ja: "拍子" }
    SetClipGainTitle { en: "Clip gain", ja: "クリップゲイン" }
    SetPositionTitle { en: "Go to position", ja: "位置へ移動" }
    Cancel { en: "Cancel", ja: "キャンセル" }
    Rename { en: "Rename", ja: "変更" }
    NameCannotBeEmpty { en: "Name cannot be empty", ja: "名前を空にはできません" }
    // What each sheet is asking for, shown under the field before anything goes wrong. The chord
    // one carries the rule nothing else on screen states: case is what says major or minor, and
    // a user who has not been told that has no way to guess it from an empty box.
    HintKey {
        en: "Tonic and mode: C major, Bb minor, D dorian",
        ja: "主音とモード: C major、Bb minor、D dorian"
    }
    HintChord {
        en: "Roman numeral of the degree. CAPITALS are major, lower case minor: IV, vi, bVII7",
        ja: "度数のローマ数字。大文字がメジャー、小文字がマイナー: IV、vi、bVII7"
    }
    HintTempo { en: "Beats per minute", ja: "1分あたりの拍数" }
    HintSignature {
        en: "Beats per bar over the note that takes the beat: 4/4, 6/8, 7/8",
        ja: "1小節の拍数と1拍の音価: 4/4、6/8、7/8 のように"
    }
    HintClipGain {
        en: "In decibels. 0 plays the clip as recorded; negative is quieter",
        ja: "デシベル単位。0で元の音量、負の値で小さく"
    }
    HintPosition {
        en: "Bar, beat, hundredth: 17, or 17.3, or 17.3.050",
        ja: "小節・拍・1/100拍: 17、17.3、17.3.050 のように"
    }
    HintSeed {
        en: "Any whole number. Another take is the next one along.",
        ja: "任意の整数。「別テイク」は次の番号です。"
    }
    HintSection {
        en: "Any name. Clips generated in same-named sections share their material.",
        ja: "任意の名前。同じ名前のセクションで生成したクリップは素材を共有します。"
    }

    // ------------------------------------------------------------------ unsaved changes
    UnsavedTitle { en: "Save changes?", ja: "変更を保存しますか？" }
    UnsavedBody {
        en: "This project has changes that have not been saved. They cannot be recovered afterwards.",
        ja: "このプロジェクトには保存されていない変更があります。あとから元に戻すことはできません。"
    }
    Discard { en: "Discard", ja: "破棄" }
    ReplaceTitle { en: "Replace project?", ja: "プロジェクトを置き換えますか？" }
    Replace { en: "Replace", ja: "置き換える" }
    MissingAudioTitle { en: "Audio files not found", ja: "見つからないオーディオファイル" }

    // ------------------------------------------------------------------ file dialogs
    //
    // The system dialog's own chrome — its buttons and its sidebar — is drawn by the platform in
    // the platform's language. The title and the file-type filter are ours, and were the only
    // two English words left in an otherwise translated flow.
    DialogSaveProject { en: "Save project", ja: "プロジェクトを保存" }
    DialogOpenProject { en: "Open project", ja: "プロジェクトを開く" }
    DialogComposeSpec { en: "Compose from specification", ja: "仕様書から作曲" }
    DialogImportAudio { en: "Import audio", ja: "オーディオを読み込む" }
    DialogImportSoundFont { en: "Import SoundFont", ja: "サウンドフォントを読み込む" }
    DialogExportWav { en: "Export WAV", ja: "WAV を書き出す" }
    FilterProject { en: "Auris project", ja: "Auris プロジェクト" }
    FilterSpec { en: "Song specification", ja: "楽曲仕様書" }
    FilterAudio { en: "Audio", ja: "オーディオ" }
    FilterSoundFont { en: "SoundFont", ja: "サウンドフォント" }
    FilterWav { en: "WAV audio", ja: "WAV オーディオ" }
    SpecRejectedTitle { en: "The specification was not accepted", ja: "仕様書を読み取れませんでした" }

    // ------------------------------------------------------------------ statuses
    NothingToUndo { en: "Nothing to undo", ja: "取り消せる操作がありません" }
    NothingToRedo { en: "Nothing to redo", ja: "やり直せる操作がありません" }
    NewProjectStatus { en: "New project", ja: "新規プロジェクト" }
    PanicStopped { en: "Panic — all voices stopped", ja: "パニック — すべての発音を停止しました" }
    ExportAlreadyRunning { en: "An export is already running", ja: "すでに書き出しを実行中です" }
    NoCycleToExport {
        en: "There is no cycle region to export — alt-drag the ruler to mark one",
        ja: "書き出すサイクル範囲がありません — ルーラーを option ドラッグして設定してください"
    }
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
    EditSetTempoPoint { en: "writing a tempo change", ja: "テンポチェンジの追加" }
    EditRemoveTempoPoint { en: "removing a tempo change", ja: "テンポチェンジの削除" }
    EditChangeSignature { en: "the time signature", ja: "拍子の変更" }
    EditSetSignaturePoint { en: "writing a signature change", ja: "拍子記号の追加" }
    EditRemoveSignaturePoint { en: "removing a signature change", ja: "拍子記号の削除" }
    EditSetClipGain { en: "the clip gain", ja: "クリップゲインの変更" }
    EditSetClipFade { en: "the fade", ja: "フェードの変更" }
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
    EditSetNoteVelocity { en: "changing note velocity", ja: "ノートの強さの変更" }
    EditMoveNotes { en: "moving notes", ja: "ノートの移動" }
    EditResizeNote { en: "resizing a note", ja: "ノートの長さ変更" }
    EditAddEffect { en: "adding an effect", ja: "エフェクトの追加" }
    EditRemoveEffect { en: "removing an effect", ja: "エフェクトの削除" }
    EditBypassEffect { en: "bypassing an effect", ja: "エフェクトのバイパス" }
    EditReorderEffects { en: "reordering the effects", ja: "エフェクトの並べ替え" }
    EditAdjustParameter { en: "the parameter change", ja: "パラメーターの変更" }
    EditWriteAutomation { en: "the automation point", ja: "オートメーションの点" }
    EditEraseAutomation { en: "erasing the automation point", ja: "オートメーションの点の削除" }
    EditClearAutomation { en: "clearing the automation", ja: "オートメーションの消去" }
    EditImportAudio { en: "importing audio", ja: "オーディオの読み込み" }
    EditImportSoundFont { en: "importing a SoundFont", ja: "サウンドフォントの読み込み" }
    EditChoosePreset { en: "choosing a sound", ja: "音色の選択" }
    EditSetKey { en: "setting the key", ja: "調の変更" }
    EditSetChord { en: "setting a chord", ja: "コードの変更" }
    EditMoveChord { en: "moving a chord", ja: "コードの移動" }
    EditClearHarmony { en: "clearing the chords", ja: "コードの消去" }
    EditSetSection { en: "changing a section", ja: "セクションの変更" }
    EditMoveSection { en: "moving a section", ja: "セクションの移動" }
    EditStampProgression { en: "writing a progression", ja: "コード進行の書き込み" }
    EditGenerateClip { en: "writing a clip", ja: "クリップの自動生成" }
    EditFreezeClip { en: "keeping a clip", ja: "クリップの確定" }
    EditCompose { en: "composing a piece", ja: "自動作曲" }

    // ------------------------------------------------------------------ errors
    ErrorUnknownTrack { en: "that track no longer exists", ja: "そのトラックは存在しません" }
    ErrorUnknownClip { en: "that clip no longer exists", ja: "そのクリップは存在しません" }
    ErrorUnknownSoundFont {
        en: "that SoundFont is not part of this project",
        ja: "そのサウンドフォントはこのプロジェクトにありません"
    }
    ErrorCannotSplit {
        en: "a clip can only be split inside itself",
        ja: "クリップの内側でしか分割できません"
    }
    ErrorNotAudio {
        en: "that is a note clip; its loudness is its velocities",
        ja: "ノートクリップです — 音量はベロシティで調整してください"
    }
    ErrorNotFinite { en: "that is not a finite number", ja: "有限の数値ではありません" }
    ErrorNotGenerated {
        en: "that clip was played rather than written, so there is nothing to write again",
        ja: "そのクリップは自動生成ではないので、書き直す元がありません"
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
    compose <song.asong> [opts]   Write a piece from a specification
    progressions                  List every chord progression known by name
    plugins                       List every registered instrument and effect
    info <project.auris>          Print a project's tracks, clips and duration
    render <project.auris> [opts] Render a project to a WAV file
    new <project.auris> [opts]    Create a project with one instrument track
    collect <project.auris>       Copy everything the project uses into its folder
    help                          Show this message

COMPOSE OPTIONS
    -o, --output <file.auris>     Where to write (default: alongside the specification)
        --seed <n>                Override the seed, so the same spec writes a different piece
        --key <key>               Override the key, as in `C minor`
        --tempo <bpm>             Override the tempo
        --mood <word>             Override the mood
        --set \"<field>: <value>\"  Override any field at all
        --print                   Print the resolved specification instead of writing

RENDER OPTIONS
    -o, --output <file.wav>       Where to write (default: alongside the project)
        --bit-depth <16|24|32>    Sample format; 32 means 32-bit float (default: 24)
        --dither                  Add TPDF dither, for 16-bit masters
        --no-tail                 Stop at the last clip instead of letting effect tails ring
        --loop                    Render only the project's cycle region

NEW OPTIONS
        --bpm <tempo>             Tempo of the new project (default: 120)
        --sample-rate <hz>        Rate of the new project (default: 48000)",
        ja: "\
auris — コマンドラインから使う Auris Studio

使い方
    auris <コマンド> [オプション]

コマンド
    compose <song.asong> [opts]   仕様ファイルから曲を書き出す
    progressions                  名前の付いたコード進行を一覧表示
    plugins                       登録済みの音源とエフェクトを一覧表示
    info <project.auris>          プロジェクトのトラック・クリップ・長さを表示
    render <project.auris> [opts] プロジェクトを WAV に書き出す
    new <project.auris> [opts]    ソフト音源トラック 1 本のプロジェクトを作成
    collect <project.auris>       プロジェクトが使うファイルをフォルダ内に集める
    help                          このメッセージを表示

compose のオプション
    -o, --output <file.auris>     出力先（既定: 仕様ファイルと同じ場所）
        --seed <n>                シードを上書き。同じ仕様から別の曲になります
        --key <key>               調を上書き（例: `C minor`）
        --tempo <bpm>             テンポを上書き
        --mood <word>             曲調を上書き
        --set \"<field>: <value>\"  任意の項目を上書き
        --print                   書き出さずに解決後の仕様を表示

render のオプション
    -o, --output <file.wav>       出力先（既定: プロジェクトと同じ場所）
        --bit-depth <16|24|32>    量子化ビット数。32 は 32bit float（既定: 24）
        --dither                  TPDF ディザを付加（16bit マスター向け）
        --no-tail                 エフェクトの残響を待たず最後のクリップで終える
        --loop                    サイクル範囲のみを書き出す

new のオプション
        --bpm <tempo>             新規プロジェクトのテンポ（既定: 120）
        --sample-rate <hz>        新規プロジェクトのサンプルレート（既定: 48000）"
    }
    CliInstruments { en: "INSTRUMENTS", ja: "音源" }
    CliEffects { en: "EFFECTS", ja: "エフェクト" }
    CliExpectedProjectPath { en: "expected a project path", ja: "プロジェクトのパスを指定してください" }
    CliNoCycle {
        en: "the project has no cycle region to render",
        ja: "プロジェクトにサイクル範囲がありません"
    }
    CliExpectedNewPath {
        en: "expected a path for the new project",
        ja: "作成先のパスを指定してください"
    }
    CliFieldPath { en: "path", ja: "パス" }
    CliFieldTempo { en: "tempo", ja: "テンポ" }
    CliFieldSampleRate { en: "sample rate", ja: "サンプルレート" }
    CliNeedsPath { en: "a path", ja: "パス" }
    CliNeedsNumber { en: "a number", ja: "数値" }
    CliNeedsValue { en: "a value", ja: "値" }
    CliFieldSignature { en: "signature", ja: "拍子" }
    CliMaster { en: "master", ja: "マスター" }
    CliFieldDuration { en: "duration", ja: "長さ" }
    CliFieldTracks { en: "tracks", ja: "トラック数" }
    CliKindInstrument { en: "instrument", ja: "音源" }
    CliKindAudio { en: "audio", ja: "オーディオ" }
    CliClipCount { en: "clip(s)", ja: "クリップ" }
    CliProgressions { en: "PROGRESSIONS", ja: "コード進行" }
    CliExpectedSpecPath {
        en: "expected a path to a song specification",
        ja: "曲の仕様ファイルのパスを指定してください"
    }

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
        // A note value written as a fraction is that fraction in either language, the same way an
        // initial is an initial. The triplet rows are not here, because "triplet" and 「三連」
        // are words rather than numbers and both languages have their own.
        const SHARED: &[Key] = &[
            Key::MuteInitial,
            Key::SoloInitial,
            Key::SubdivisionEighth,
            Key::SubdivisionSixteenth,
        ];
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
