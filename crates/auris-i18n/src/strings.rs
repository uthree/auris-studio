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
    PromptLyric { en: "Lyric", ja: "歌詞" }
    PromptPhonemes { en: "Phonemes (space-separated IPA)", ja: "音素(スペース区切りの IPA)" }
    PromptLyrics { en: "Write Lyrics Across Selection", ja: "選択ノートに歌詞を流し込む" }
    PromptComposeLyrics { en: "Compose from Lyrics", ja: "歌詞から作曲" }
    HintComposeLyrics {
        en: "One line per phrase (、！？ break too) · secondary-Return composes, a fresh take each run",
        ja: "1行が1フレーズ(、や！？も切れ目)。secondary-Returnで作曲、実行のたびに新しいテイク"
    }
    PromptSectionLyrics { en: "Lyrics", ja: "歌詞" }
    HintSectionLyrics {
        en: "One line per phrase · Tab walks the sections · empty makes a section instrumental",
        ja: "1行が1フレーズ(、や！？も切れ目)。Tabで次のセクションへ。空にすると歌なしに戻ります"
    }
    LyricsNoWords { en: "(no words — instrumental)", ja: "(歌詞なし — インスト)" }
    LyricsNotesUnit { en: "notes", ja: "音" }
    MenuEditLyric { en: "Edit Lyric…", ja: "歌詞を編集…" }
    MenuEditPhonemes { en: "Edit Phonemes…", ja: "音素を編集…" }
    MenuResetPhonemeTiming { en: "Reset Phoneme Timing", ja: "音素タイミングをリセット" }
    MenuAddScoop { en: "Add Scoop", ja: "しゃくりを追加" }
    MenuRemoveScoop { en: "Remove Scoop", ja: "しゃくりを削除" }
    MenuAddFall { en: "Add Fall", ja: "フォールを追加" }
    MenuRemoveFall { en: "Remove Fall", ja: "フォールを削除" }
    MenuAddVibrato { en: "Add Vibrato", ja: "ビブラートを追加" }
    MenuRemoveVibrato { en: "Remove Vibrato", ja: "ビブラートを削除" }
    MenuResetOrnaments { en: "Reset Pitch Ornaments", ja: "ピッチ装飾をリセット" }
    MenuWriteLyrics { en: "Write Lyrics…", ja: "歌詞をまとめて入力…" }
    Mixer { en: "Mixer", ja: "ミキサー" }
    Inspector { en: "Inspector", ja: "インスペクタ" }
    Master { en: "Master", ja: "マスター" }
    // Beside the master meter and the same width, so the caption has to be short enough not to
    // push the number off the end of its row.
    InputMeter { en: "In", ja: "入力" }
    TakeClock { en: "Take", ja: "録音" }
    TakeDeviceLost { en: "device lost", ja: "デバイス切断" }
    ClearClipping { en: "Clear Clip Indicators", ja: "クリップ表示をリセット" }

    // ------------------------------------------------------------------ arrangement
    AddInstrumentShort { en: "Inst", ja: "音源" }
    AddAudioShort { en: "Audio", ja: "音声" }
    MuteInitial { en: "M", ja: "M" }
    SoloInitial { en: "S", ja: "S" }
    RecordInitial { en: "R", ja: "R" }
    MonitorInitial { en: "I", ja: "I" }
    // What the `R` above is, said out loud on its tooltip. Arming is not a bindable command —
    // the button and the track menu are the two ways to it — so this is a name rather than a
    // command label, and it says what arming *does* because "arm" is jargon in both languages.
    ArmTrack { en: "Record Onto This Track", ja: "このトラックに録音" }
    Volume { en: "Vol", ja: "音量" }
    Pan { en: "Pan", ja: "パン" }
    TrackKindInstrument { en: "Instrument", ja: "ソフト音源" }
    TrackKindSinger { en: "Singer", ja: "シンガー" }
    TrackKindAudio { en: "Audio", ja: "オーディオ" }
    TrackKindBus { en: "Bus", ja: "バス" }
    // A strip is 128 px wide and every label on it is an abbreviation for that reason.
    OutputShort { en: "Out", ja: "出力" }
    Sends { en: "Sends", ja: "センド" }
    AddBusShort { en: "Bus", ja: "バス" }
    // Marks a send taken before the fader, in the room a send row has beside the bus name. Not an
    // initial like Mute's and Solo's, because "P" says nothing at all in Japanese.
    SendPreFaderMark { en: "Pre", ja: "プリ" }

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
    BrowserSearch { en: "Search the library", ja: "ライブラリを検索" }
    BrowserNothingFound { en: "Nothing here answers to that", ja: "該当するものはありません" }
    BrowserAddPluginFolder { en: "Add a folder…", ja: "フォルダを追加…" }
    BrowserNoSoundFonts {
        en: "None imported yet",
        ja: "まだ読み込まれていません"
    }
    BrowserPlugins { en: "Installed plugins", ja: "インストール済みプラグイン" }
    BrowserNoPlugins {
        en: "No CLAP plugins found on this computer",
        ja: "このコンピューターにCLAPプラグインが見つかりません"
    }
    // Says what opening a file *does*, not what it shows. Loading a plugin runs its code, and
    // that is worth one line above the list rather than nowhere at all.
    BrowserPluginsHint {
        en: "Opening a file loads the plugin's code into Auris Studio",
        ja: "ファイルを開くとプラグインのコードがAuris Studioに読み込まれます"
    }
    BrowserPluginUnreadable {
        en: "could not be read",
        ja: "読み込めませんでした"
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
    InputDevice { en: "Input Device", ja: "入力デバイス" }
    InputDeviceNote {
        en: "Opened only while recording, so changing it does not interrupt playback.",
        ja: "録音中だけ開きます。変更しても再生は止まりません。"
    }
    SystemDefaultDevice { en: "System Default", ja: "システムのデフォルト" }
    SystemDefaultDeviceDetail {
        en: "Follows whatever the system is set to",
        ja: "システムの設定に追従します"
    }
    SampleRate { en: "Sample Rate", ja: "サンプルレート" }
    DeviceDefaultRate { en: "Device Default", ja: "デバイス標準" }
    BufferSize { en: "Buffer Size", ja: "バッファサイズ" }
    ExportFormat { en: "Export Format", ja: "書き出し形式" }
    ExportRate { en: "Export Rate", ja: "書き出しサンプルレート" }
    ProjectRate { en: "Project Rate", ja: "プロジェクトのレート" }
    ExportDither { en: "Dither", ja: "ディザ" }
    ExportDitherNote {
        en: "TPDF dither before quantising. Worth it at 16-bit; pointless at 24, and impossible at float.",
        ja: "量子化前に TPDF ディザを加えます。16 bit では有効ですが、24 bit ではほぼ無意味で、float では適用されません。"
    }
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
    // Named "Click" rather than "plain click" or "no modifier": it is the shortest true thing,
    // and the row it sits in already says what it does.
    GesturePlainClick { en: "Click", ja: "クリック" }
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
    // Under the create row, because that is where somebody is standing when they choose it and
    // find their range selections gone. Deleting is not offered on a bare click at all, so this
    // row needs no such warning.
    PointerClickNote {
        en: "Range selection moves to ⇧-drag, and a click on an empty lane makes a clip rather than moving the playhead.",
        ja: "範囲選択は ⇧＋ドラッグになり、空のレーンをクリックすると再生位置の移動ではなくクリップ作成になります。"
    }

    // ------------------------------------------------------------------ command groups
    // 「トランスポート」 reads as nothing at all to somebody who has never used a DAW, and it was
    // briefly 「再生」 for that reason. It is back, because that argument weighs the wrong reader:
    // Cubase, Studio One and SONAR all ship this menu under that name in Japanese, so it is the
    // word a Japanese DAW user *arrives with* — the same rule the tool names and the gesture
    // names already follow. Plain language wins where there is no established term; where there
    // is one, matching it is what lets somebody use this without being taught.
    GroupTransport { en: "Transport", ja: "トランスポート" }
    GroupFile { en: "File", ja: "ファイル" }
    GroupEdit { en: "Edit", ja: "編集" }
    GroupNotes { en: "Notes", ja: "ノート" }
    GroupClip { en: "Clip", ja: "クリップ" }
    GroupTrack { en: "Track", ja: "トラック" }
    // Composing is a menu of its own rather than three rows in File. It is the thing this
    // application does that others do not, and it was reachable only from the middle of a list
    // of file operations, under a label that named the *file* route to it.
    GroupCompose { en: "Compose", ja: "作曲" }
    GroupView { en: "View", ja: "表示" }

    // ------------------------------------------------------------------ commands
    CmdPlayStop { en: "Play / Stop", ja: "再生 / 停止" }
    // Not in the bindable table: the transport's square is the only way to reach it, and this
    // names it on the tooltip there. "Stop and return", because that is what the button does and
    // is what tells it apart from the pause the play button turns into.
    CmdStop { en: "Stop and Return", ja: "停止して先頭へ" }
    CmdReturnToZero { en: "Return to Zero", ja: "先頭に戻る" }
    CmdStepBack { en: "Move Playhead Back One Division", ja: "再生位置を 1 グリッド戻す" }
    CmdStepForward { en: "Move Playhead On One Division", ja: "再生位置を 1 グリッド進める" }
    CmdToggleCycle { en: "Toggle Cycle", ja: "サイクル切り替え" }
    // 「メトロノーム」 rather than 「拍子木」 or any other translation, for the reason
    // `GroupTransport` above gives: it is the word every Japanese DAW already prints on this
    // button, so it is the word a user arrives with.
    CmdToggleMetronome { en: "Metronome", ja: "メトロノーム" }
    CmdRecord { en: "Record", ja: "録音" }
    RecordingStarted { en: "Recording", ja: "録音中" }
    CmdToggleMonitoring { en: "Input Monitoring", ja: "入力モニタリング" }
    CmdTogglePunch { en: "Punch Recording", ja: "パンチ録音" }
    PunchOn { en: "Punch recording on — takes are trimmed to the punch region", ja: "パンチ録音オン — 録音はパンチ範囲に切り詰められます" }
    PunchOff { en: "Punch recording off", ja: "パンチ録音オフ" }
    MonitoringOff { en: "Monitoring off", ja: "モニタリングを終了しました" }
    CmdMusicalTyping { en: "Musical Typing", ja: "ミュージックタイピング" }
    MusicalTypingOn {
        en: "Musical Typing on — A to ; plays, Z and X move the octave, C and V the velocity, 1 and 2 bend, 3 to 8 the modulation, Tab holds",
        ja: "ミュージックタイピングオン — A〜; で演奏、Z X でオクターブ、C V で強さ、1 2 でベンド、3〜8 でモジュレーション、Tab で保持"
    }
    MusicalTypingOff { en: "Musical Typing off", ja: "ミュージックタイピングオフ" }
    MusicalTypingNeedsInstrument {
        en: "No instrument track to play — add one first",
        ja: "演奏できるソフト音源トラックがありません。先に追加してください"
    }
    // The four readouts on the drawn keyboard, in the order they sit round it. Each names a
    // control the keys move blind, which is the whole reason the window is worth having.
    TypingPitch { en: "Pitch", ja: "ピッチ" }
    TypingModulation { en: "Modulation", ja: "モジュレーション" }
    TypingOctave { en: "Octave", ja: "オクターブ" }
    TypingVelocity { en: "Velocity", ja: "ベロシティ" }
    // Written on the key rather than beside it, so it is as short as the letters around it.
    TypingSustain { en: "sustain", ja: "サステイン" }
    // The two ends of the modulation row. Only the ends are named: the four keys between them
    // are steps of one wheel, and numbering them would be six labels for one control.
    TypingWheelMax { en: "max", ja: "最大" }
    TypingNoTrack { en: "no instrument", ja: "音源なし" }
    RecordingNeedsSave {
        en: "A take is written to disk as it happens — choose where this project lives first.",
        ja: "録音は録りながらディスクに書き込みます。先にプロジェクトの保存先を決めてください。"
    }
    Autosave { en: "Autosave", ja: "自動保存" }
    AutosaveNote {
        en: "Saves a project that already has a folder, about every 30 seconds, once it has changed. Closing without saving stops being a way to undo an afternoon.",
        ja: "保存先が決まっているプロジェクトを、変更があれば30秒ごとに上書き保存します。「保存せずに閉じて元に戻す」はできなくなります。"
    }
    SnapNoteLengths { en: "Snap Note Lengths", ja: "ノートの長さをスナップ" }
    SnapNoteLengthsNote {
        en: "Rounds a note's duration to the editing grid while its right edge is dragged.",
        ja: "ノートの右端をドラッグしたとき、長さを編集グリッドに合わせます。"
    }
    CurrentChord { en: "Chord", ja: "コード" }
    JapaneseDictionaryHeading { en: "Japanese Dictionary", ja: "日本語辞書" }
    JapaneseDictionaryNote {
        en: "Reads kanji lyrics and the pitch accent a composed melody follows. A build ships with naist-jdic; choose a folder here only to override it, and Clear to return to the shipped one.",
        ja: "漢字歌詞の読みと、歌詞から作曲するときのアクセントに使われます。naist-jdic は同梱されています。差し替えたい場合のみフォルダを指定し、「解除」で同梱辞書に戻ります。"
    }
    ValueShippedDictionary {
        en: "Shipped dictionary (naist-jdic)",
        ja: "同梱辞書(naist-jdic)を使用中"
    }
    SingerComputeHeading { en: "Singing Synthesis", ja: "歌声合成" }
    SingerComputeAuto { en: "Auto", ja: "自動" }
    SingerComputeNote {
        en: "Where a voice model sings: Auto takes the GPU when the runtime offers one — DirectML on Windows, Core ML on macOS — and the CPU otherwise. Takes effect from the next render.",
        ja: "歌声モデルの計算をどこで行うか。「自動」は GPU が使えるとき(Windows は DirectML、macOS は Core ML)に GPU を使い、なければ CPU で歌います。次のレンダリングから反映されます。"
    }
    DialogDictionaryFolder { en: "Choose the dictionary folder", ja: "辞書フォルダを選択" }
    ValueNotSet { en: "Not set", ja: "未設定" }
    MenuChoose { en: "Choose…", ja: "選択…" }
    MenuClear { en: "Clear", ja: "解除" }
    CmdPanic { en: "Panic", ja: "パニック" }
    CmdNewProject { en: "New Project", ja: "新規プロジェクト" }
    CmdOpenProject { en: "Open Project", ja: "プロジェクトを開く" }
    CmdComposeSong { en: "Compose a Song…", ja: "曲を作る…" }
    CmdAccompanyMelody { en: "Accompany the Melody", ja: "旋律に伴奏をつける" }
    CmdComposeFromLyrics { en: "Compose from Lyrics…", ja: "歌詞から作曲…" }
    CmdComposeFromSpec { en: "Compose from Specification…", ja: "仕様ファイルから作曲…" }
    CmdBalanceLevels { en: "Balance the Mix", ja: "ミックスの自動調整" }
    CmdOpenPluginWindow { en: "Open Plugin Window", ja: "プラグインの画面を開く" }
    CmdSave { en: "Save", ja: "保存" }
    CmdSaveAs { en: "Save As", ja: "名前を付けて保存" }
    CmdImportAudio { en: "Import Audio", ja: "オーディオを読み込む" }
    CmdImportSoundFont { en: "Import SoundFont", ja: "サウンドフォントを読み込む" }
    CmdImportMidi { en: "Import MIDI File…", ja: "MIDI ファイルを読み込む…" }
    CmdExportMidi { en: "Export MIDI File…", ja: "MIDI ファイルを書き出す…" }
    CmdCollectAssets { en: "Collect Assets", ja: "アセットを集める" }
    CmdExportWav { en: "Export WAV", ja: "WAV を書き出す" }
    CmdExportCycle { en: "Export Cycle as WAV", ja: "サイクル範囲を WAV に書き出す" }
    CmdExportStems { en: "Export Stems", ja: "ステムを書き出す" }
    CmdExportSingerFrames { en: "Export Singer Frames…", ja: "シンガーフレームを書き出す…" }
    CmdQuit { en: "Quit", ja: "終了" }
    CmdUndo { en: "Undo", ja: "取り消す" }
    CmdRedo { en: "Redo", ja: "やり直す" }
    CmdDeleteSelection { en: "Delete Selection", ja: "選択範囲を削除" }
    CmdAddInstrumentTrack { en: "Add Instrument Track", ja: "ソフト音源トラックを追加" }
    CmdAddAudioTrack { en: "Add Audio Track", ja: "オーディオトラックを追加" }
    CmdAddSingerTrack { en: "Add Singer Track", ja: "シンガートラックを追加" }
    CmdAddBusTrack { en: "Add Bus", ja: "バスを追加" }
    CmdSetTrackOutput { en: "Route Track", ja: "出力先を変更" }
    CmdAddSend { en: "Add Send", ja: "センドを追加" }
    CmdRemoveSend { en: "Remove Send", ja: "センドを削除" }
    CmdDuplicateTrack { en: "Duplicate Track", ja: "トラックを複製" }
    CmdToggleTrackMute { en: "Mute Track", ja: "トラックをミュート" }
    CmdToggleTrackSolo { en: "Solo Track", ja: "トラックをソロ" }
    CmdSelectPreviousTrack { en: "Select Previous Track", ja: "前のトラックを選択" }
    CmdSelectNextTrack { en: "Select Next Track", ja: "次のトラックを選択" }
    CmdDeleteTrack { en: "Delete Track", ja: "トラックを削除" }
    // The note and clip commands. Each pair says which of the two it means, because both are on
    // the same keystroke and the settings window lists them one under the other.
    CmdSelectAllNotes { en: "Select All Notes", ja: "すべてのノートを選択" }
    CmdCutNotes { en: "Cut Notes", ja: "ノートを切り取り" }
    CmdCopyNotes { en: "Copy Notes", ja: "ノートをコピー" }
    CmdPasteNotes { en: "Paste Notes", ja: "ノートを貼り付け" }
    CmdCutClips { en: "Cut Clips", ja: "クリップを切り取り" }
    CmdCopyClips { en: "Copy Clips", ja: "クリップをコピー" }
    CmdPasteClips { en: "Paste Clips", ja: "クリップを貼り付け" }
    CmdDuplicateNotes { en: "Duplicate Notes", ja: "ノートを複製" }
    CmdTransposeUp { en: "Transpose Up a Semitone", ja: "半音上げる" }
    CmdTransposeDown { en: "Transpose Down a Semitone", ja: "半音下げる" }
    CmdOctaveUp { en: "Transpose Up an Octave", ja: "1 オクターブ上げる" }
    CmdOctaveDown { en: "Transpose Down an Octave", ja: "1 オクターブ下げる" }
    // "by One Division" rather than a fixed note value: what a nudge moves is the editing grid,
    // which the user sets, so naming a sixteenth here would be a menu row that lies as soon as
    // the grid button is pressed.
    CmdNudgeNotesLeft { en: "Nudge Notes Left One Division", ja: "ノートを 1 グリッド左へ" }
    CmdNudgeNotesRight { en: "Nudge Notes Right One Division", ja: "ノートを 1 グリッド右へ" }
    CmdNudgeClipsLeft { en: "Nudge Clips Left One Division", ja: "クリップを 1 グリッド左へ" }
    CmdNudgeClipsRight { en: "Nudge Clips Right One Division", ja: "クリップを 1 グリッド右へ" }
    CmdSelectAllClips { en: "Select All Clips", ja: "すべてのクリップを選択" }
    CmdDuplicateClip { en: "Duplicate Clip", ja: "クリップを複製" }
    CmdSplitClip { en: "Split Clip at Playhead", ja: "再生位置でクリップを分割" }
    CmdToggleClipMute { en: "Mute Clip", ja: "クリップをミュート" }
    CmdToggleClipLoop { en: "Loop Clip", ja: "クリップをループ" }
    // "Quantise" alone means the starts, which is what everybody reaches for it expecting. The
    // other two say which half of the note they move, because a person who wants one of them
    // specifically is the only person who will ever look for them.
    CmdQuantize { en: "Quantise", ja: "クオンタイズ" }
    CmdQuantizeLengths { en: "Quantise Note Lengths", ja: "音価をクオンタイズ" }
    CmdQuantizeBoth { en: "Quantise Starts and Lengths", ja: "発音位置と音価をクオンタイズ" }
    // "Show" was a lie on a toggle with no state beside it: choosing Show Inspector hid the
    // inspector. These say what the command does either way.
    CmdShowLibrary { en: "Library", ja: "ライブラリ" }
    CmdShowInspector { en: "Inspector", ja: "インスペクタ" }
    CmdShowPianoRoll { en: "Piano Roll", ja: "ピアノロール" }
    CmdShowMixer { en: "Mixer", ja: "ミキサー" }
    // The three strips over the arrangement, named by what is in them rather than by the word
    // "lane" — a person hides the chords, not a lane.
    CmdShowStructureLane { en: "Song Structure", ja: "曲の構成" }
    CmdShowHarmonyLane { en: "Key and Chords", ja: "キーとコード" }
    CmdShowTempoMarks { en: "Tempo Changes", ja: "テンポの変化" }
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
    MenuExportStemsItem { en: "Export Stems…", ja: "ステムを書き出す…" }
    DialogExportStems { en: "Choose a folder for the stems", ja: "ステムの書き出し先フォルダを選択" }
    MenuDelete { en: "Delete", ja: "削除" }

    // ------------------------------------------------------------------ panels and their docks
    // On the menu a right-click on a panel's status-bar icon opens. "Dock" is the verb, so the
    // Japanese is the instruction rather than the noun: 左に配置 is what the row does.
    DockLeft { en: "Dock Left", ja: "左に配置" }
    DockBottom { en: "Dock Bottom", ja: "下に配置" }
    DockRight { en: "Dock Right", ja: "右に配置" }
    HidePanel { en: "Hide Panel", ja: "パネルを隠す" }

    // ------------------------------------------------------------------ context menus
    MenuNewTrack { en: "New Track", ja: "新規トラック" }
    MenuNote { en: "Note", ja: "ノート" }
    MenuCycleTitle { en: "Cycle", ja: "サイクル" }
    MenuDuplicateTrack { en: "Duplicate Track", ja: "トラックを複製" }
    MenuRename { en: "Rename…", ja: "名前を変更…" }
    MenuRenameTrack { en: "Rename Track…", ja: "トラック名を変更…" }
    MenuRenameClip { en: "Rename Clip…", ja: "クリップ名を変更…" }
    MenuAddEffect { en: "Add Effect…", ja: "エフェクトを追加…" }
    MenuNewInstrumentTrack { en: "New Instrument Track", ja: "新規ソフト音源トラック" }
    MenuNewSingerTrack { en: "New Singer Track", ja: "新規シンガートラック" }
    MenuNewAudioTrack { en: "New Audio Track", ja: "新規オーディオトラック" }
    MenuNewBusTrack { en: "New Bus", ja: "新規バス" }
    MenuRouteTo { en: "Output", ja: "出力先" }
    MenuAddSend { en: "Add Send", ja: "センドを追加" }
    MenuRemoveSend { en: "Remove Send", ja: "センドを削除" }
    MenuSendPreFader { en: "Take Before the Fader", ja: "フェーダー前から取る" }
    MenuDuplicate { en: "Duplicate", ja: "複製" }
    MenuCut { en: "Cut", ja: "切り取り" }
    MenuCopy { en: "Copy", ja: "コピー" }
    MenuPaste { en: "Paste", ja: "貼り付け" }
    MenuPasteHere { en: "Paste Here", ja: "ここに貼り付け" }
    // Named for what it does to the *melody*, not for what it adds. "Add Bass and Drums" would
    // be a list of what this build happens to write; a person right-clicking a tune they just
    // played is asking for the thing, not for its parts.
    MenuAccompany { en: "Accompany This Melody", ja: "この旋律に伴奏をつける" }
    MenuSplitAtPlayhead { en: "Split at Playhead", ja: "再生位置で分割" }
    MenuMuteClip { en: "Mute Clip", ja: "クリップをミュート" }
    MenuLoopClip { en: "Loop Clip", ja: "クリップをループ" }
    MenuQuantizeStarts { en: "Quantise Starts", ja: "発音位置をクオンタイズ" }
    MenuQuantizeLengths { en: "Quantise Lengths", ja: "音価をクオンタイズ" }
    MenuQuantizeBoth { en: "Quantise Both", ja: "両方をクオンタイズ" }
    MenuCycleOverClip { en: "Cycle over Clip", ja: "クリップをサイクル範囲に" }
    MenuEditInPianoRoll { en: "Edit in Piano Roll", ja: "ピアノロールで編集" }
    MenuNewClipHere { en: "New Clip Here", ja: "ここに新規クリップ" }
    MenuCycleStartHere { en: "Cycle Start Here", ja: "ここをサイクル開始に" }
    MenuCycleEndHere { en: "Cycle End Here", ja: "ここをサイクル終了に" }
    MenuClearCycle { en: "Clear Cycle Region", ja: "サイクル範囲を消去" }
    MenuPunchTitle { en: "Punch", ja: "パンチ" }
    MenuPunchStartHere { en: "Punch In Here", ja: "ここをパンチイン位置に" }
    MenuPunchEndHere { en: "Punch Out Here", ja: "ここをパンチアウト位置に" }
    MenuPunchFromCycle { en: "Punch Over Cycle Region", ja: "サイクル範囲をパンチ範囲にする" }
    MenuClearPunch { en: "Clear Punch Region", ja: "パンチ範囲を消去" }
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
    MenuFollowTempo { en: "Follow Tempo", ja: "テンポに追従" }
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
    PresetKick { en: "Kick", ja: "キック" }
    PresetSnare { en: "Snare", ja: "スネア" }
    PresetHat { en: "Hi-hat", ja: "ハイハット" }
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
    // Shown while a generated clip's notes differ from what its recipe last wrote, for as long
    // as they differ: writing the part again would replace the edits, and that is worth a
    // sentence on screen before the button rather than a surprise after it.
    PartEditedByHand { en: "Edited by hand — writing it again replaces the edits", ja: "手動編集あり — 再生成すると上書きされます" }
    // The dials on any clip's performance: the transform stack, which changes what is heard
    // without moving a note the piano roll shows.
    PerformHeading { en: "Performance", ja: "演奏" }
    PerformFreeze { en: "Keep the Performance", ja: "演奏を確定" }
    // Note values. Written as the fraction in both languages, because that is what is printed on
    // every other DAW's grid menu and the words for them are longer than the row is wide.
    SubdivisionEighth { en: "1/8", ja: "1/8" }
    SubdivisionSixteenth { en: "1/16", ja: "1/16" }
    SubdivisionEighthTriplet { en: "1/8 triplet", ja: "1/8 三連" }
    SubdivisionSixteenthTriplet { en: "1/16 triplet", ja: "1/16 三連" }

    // ------------------------------------------------------------------ the song sheet
    // The dialog that writes a whole piece. Its dials are the specification's own fields, so
    // where one is already named above — density, gate, swing — the same string is reused and
    // only what the sheet adds appears here.
    SongSheetTitle { en: "Compose a Song", ja: "曲を作る" }
    SongHeading { en: "Song", ja: "曲" }
    SongTitleField { en: "Title", ja: "タイトル" }
    SongKey { en: "Key", ja: "調" }
    SongScale { en: "Scale", ja: "スケール" }
    SongMeter { en: "Time signature", ja: "拍子" }
    // The named feeling, and the four numbers behind it. A mood the dials have been nudged away
    // from matches no word, and the picker says so rather than naming the one it started at.
    SongMood { en: "Mood", ja: "曲調" }
    SongMoodCustom { en: "custom", ja: "カスタム" }
    SongBrightness { en: "Brightness", ja: "明るさ" }
    SongEnergy { en: "Energy", ja: "勢い" }
    SongTension { en: "Tension", ja: "緊張感" }
    SongVariation { en: "Variation", ja: "繰り返しの変化" }
    // How long one section runs, with the whole song's length in the footer — the dial sets one
    // thing and the number that matters is the other.
    SongBars { en: "Bars", ja: "小節数" }
    SongLength { en: "Length", ja: "長さ" }
    SongBarsUnit { en: "bars", ja: "小節" }
    SongChords { en: "Progression", ja: "コード進行" }
    SongChordsOwn { en: "the composer's own", ja: "おまかせ" }
    // The form: which sections play, in what order, and what each of them is.
    SongStyle { en: "Style", ja: "スタイル" }
    SongStyleChoose { en: "Choose…", ja: "選ぶ…" }
    SongFormHeading { en: "Form", ja: "構成" }
    SongAddSection { en: "Add Section", ja: "セクションを追加" }
    SongSectionName { en: "Section", ja: "セクション" }
    SongTranspose { en: "Transpose", ja: "移調" }
    SongSectionParts { en: "Who Plays", ja: "演奏するパート" }
    SongSectionTempo { en: "Section Tempo", ja: "セクションのテンポ" }
    SongSectionTempoFollows { en: "Follow the Song", ja: "曲に合わせる" }
    // Writing a progression out, and keeping the one written. The second reaches past the song
    // into the book this installation carries.
    SongWriteProgression { en: "Write one…", ja: "手で書く…" }
    SongKeepProgression { en: "Keep in the catalogue…", ja: "カタログに登録…" }
    HintProgression { en: "Bars of roman numerals: | IVmaj7 | III7 | vi7 | I7 |",
                      ja: "ローマ数字を小節ごとに: | IVmaj7 | III7 | vi7 | I7 |" }
    // The tune's contour, given by hand or taken from a clip; empty leaves it to the composer.
    SongMotif { en: "Motif", ja: "モチーフ" }
    HintMotif { en: "Scale steps around the anchor: 0 2 4 2 — empty leaves it to the composer",
                ja: "アンカーからのスケール度数: 0 2 4 2 — 空欄ならおまかせ" }
    MenuComposeFromMotif { en: "Take as the Motif…", ja: "モチーフとして取り込む…" }
    ErrorMotifTooShort { en: "that clip has no line to take as a motif",
                         ja: "モチーフにできる旋律がクリップにありません" }
    // The roster. `Part` alone is the heading on a single generated clip, so this one is plural.
    SongPartsHeading { en: "Parts", ja: "パート" }
    SongPartName { en: "Name", ja: "名前" }
    SongPartRole { en: "Role", ja: "役割" }
    SongPartInstrument { en: "Instrument", ja: "音源" }
    // Which MIDI note a drum part strikes. It stands where a pitched part shows its octave,
    // because a kit has no octave and does need this.
    // The strip under the piano roll, named by what is in it rather than by the word "lane".
    BendLane { en: "Bend", ja: "ベンド" }
    ModulationLane { en: "Mod", ja: "モジュ" }
    CurveLanes { en: "Lanes", ja: "レーン" }
    StraightenCurve { en: "Straighten", ja: "カーブを消す" }
    CmdShowBendLane { en: "Pitch Bend", ja: "ピッチベンド" }
    CmdShowModulationLane { en: "Modulation", ja: "モジュレーション" }
    SongPartNote { en: "Note", ja: "ノート" }
    SongPartGain { en: "Level", ja: "音量" }
    SongPartPan { en: "Pan", ja: "定位" }
    SongAddPart { en: "Add Part", ja: "パートを追加" }
    SongRemovePart { en: "Remove", ja: "削除" }
    SongPartNameTitle { en: "Name for this part", ja: "このパートの名前" }
    // The three things the sheet can do. "Another take" is the next seed rather than a random
    // one, which is what makes a take somebody liked reachable again.
    SongWrite { en: "Write", ja: "書き出す" }
    SongAnotherTake { en: "Another Take", ja: "別テイク" }
    SongSaveSpec { en: "Save as Specification…", ja: "仕様ファイルに保存…" }
    // What the roles are called where a part names one. `Role::name` is the word the document
    // writes and stays English; this is the word the picker shows.
    RoleMelody { en: "Melody", ja: "メロディ" }
    RoleChords { en: "Chords", ja: "コード" }
    RolePad { en: "Pad", ja: "パッド" }
    RoleArp { en: "Arpeggio", ja: "アルペジオ" }
    RoleStab { en: "Stab", ja: "スタブ" }
    RoleBass { en: "Bass", ja: "ベース" }
    RoleKick { en: "Kick", ja: "キック" }
    RoleSnare { en: "Snare", ja: "スネア" }
    RoleHat { en: "Hi-hat", ja: "ハイハット" }
    RoleCrash { en: "Crash", ja: "クラッシュ" }
    RoleRiser { en: "Riser", ja: "ライザー" }
    // The mood vocabulary. Nine words that each mean four numbers.
    MoodNeutral { en: "Neutral", ja: "ニュートラル" }
    MoodBright { en: "Bright", ja: "明るい" }
    MoodDark { en: "Dark", ja: "暗い" }
    MoodCalm { en: "Calm", ja: "穏やか" }
    MoodDriving { en: "Driving", ja: "疾走感" }
    MoodEpic { en: "Epic", ja: "壮大" }
    MoodDreamy { en: "Dreamy", ja: "夢見心地" }
    MoodTense { en: "Tense", ja: "張り詰めた" }
    MoodFunky { en: "Funky", ja: "ファンキー" }

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
    SetClipSourceTempoTitle { en: "Recorded tempo", ja: "元のテンポ" }
    SetPositionTitle { en: "Go to position", ja: "位置へ移動" }
    Cancel { en: "Cancel", ja: "キャンセル" }
    Rename { en: "Rename", ja: "変更" }
    NameCannotBeEmpty { en: "Name cannot be empty", ja: "名前を空にはできません" }
    NameAlreadyUsed { en: "Name is already in use", ja: "その名前はすでに使われています" }
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
    DialogPluginFolder { en: "Choose a plugin folder", ja: "プラグインのフォルダを選択" }
    CmdOpenRecent { en: "Open Recent", ja: "最近のプロジェクト" }
    CmdAbout { en: "About Auris Studio", ja: "Auris Studio について" }
    MenuNoRecentProjects { en: "Nothing opened yet", ja: "まだ何も開いていません" }
    MenuForgetRecent { en: "Forget This List", ja: "履歴を消去" }
    DialogComposeSpec { en: "Compose from specification", ja: "仕様書から作曲" }
    DialogImportAudio { en: "Import audio", ja: "オーディオを読み込む" }
    DialogImportSoundFont { en: "Import SoundFont", ja: "サウンドフォントを読み込む" }
    DialogImportMidi { en: "Import a MIDI file", ja: "MIDI ファイルを読み込む" }
    DialogExportMidi { en: "Export a MIDI file", ja: "MIDI ファイルを書き出す" }
    DialogExportFrames { en: "Export singer frames", ja: "シンガーフレームを書き出す" }
    DialogExportWav { en: "Export WAV", ja: "WAV を書き出す" }
    FilterProject { en: "Auris project", ja: "Auris プロジェクト" }
    FilterSpec { en: "Song specification", ja: "楽曲仕様書" }
    FilterAudio { en: "Audio", ja: "オーディオ" }
    FilterSoundFont { en: "SoundFont", ja: "サウンドフォント" }
    FilterMidi { en: "MIDI file", ja: "MIDI ファイル" }
    FilterJson { en: "JSON file", ja: "JSON ファイル" }
    ErrorNoSingerTrack {
        en: "select a singer track first — frames are one track's features",
        ja: "先にシンガートラックを選択してください。フレームは 1 トラック分の特徴量です"
    }
    FilterWav { en: "WAV audio", ja: "WAV オーディオ" }
    FilterVoiceModel { en: "singing voice model", ja: "歌声モデル" }
    DialogChooseVoice { en: "Choose a voice model", ja: "声のモデルを選択" }
    CmdChooseVoice { en: "Choose Voice…", ja: "声を選ぶ…" }
    CmdNextSpeaker { en: "Next Speaker", ja: "次の話者" }
    ErrorOneSpeaker { en: "this voice has only one speaker", ja: "この声には話者が 1 人しかいません" }
    CmdSing { en: "Sing", ja: "歌わせる" }
    SingCancelled { en: "Singing stopped; the previous take stands", ja: "歌唱を中止しました。前のテイクのままです" }
    TakeBehind { en: "behind the notes", ja: "テイクが楽譜より古い" }
    TakeRendering { en: "singing the notes…", ja: "歌声をレンダリング中…" }
    SpecRejectedTitle { en: "The specification was not accepted", ja: "仕様書を読み取れませんでした" }

    // ------------------------------------------------------------------ statuses
    NothingToUndo { en: "Nothing to undo", ja: "取り消せる操作がありません" }
    NothingToRedo { en: "Nothing to redo", ja: "やり直せる操作がありません" }
    NewProjectStatus { en: "New project", ja: "新規プロジェクト" }
    PanicStopped { en: "Panic — all voices stopped", ja: "パニック — すべての発音を停止しました" }
    ExportAlreadyRunning { en: "An export is already running", ja: "すでに書き出しを実行中です" }
    ExportCancelled { en: "Export stopped; no file was written", ja: "書き出しを中止しました。ファイルは作成されていません" }
    ExportCancelling { en: "Stopping…", ja: "中止しています…" }
    NoCycleToExport {
        en: "There is no cycle region to export — alt-drag the ruler to mark one",
        ja: "書き出すサイクル範囲がありません — ルーラーを Alt（option）ドラッグして設定してください"
    }
    AudioClipsComeFromImport {
        en: "Audio clips come from Import Audio, not from an empty lane",
        ja: "オーディオクリップは空のレーンからではなく読み込みで作成します"
    }
    BusHoldsNoClips {
        en: "A bus holds no clips; its material is whatever is routed into it",
        ja: "バスにクリップは置けません — 音は他のトラックからのルーティングで届きます"
    }
    DuplicatedTrack { en: "Duplicated track", ja: "トラックを複製しました" }
    DuplicatedClip { en: "Duplicated clip", ja: "クリップを複製しました" }
    SplitClipStatus { en: "Split clip", ja: "クリップを分割しました" }
    CopiedToClipboard { en: "Copied", ja: "コピーしました" }
    CutToClipboard { en: "Cut", ja: "切り取りました" }
    ClipLooped { en: "Clip looped", ja: "クリップをループしました" }
    ClipUnlooped { en: "Clip no longer loops", ja: "クリップのループを解除しました" }
    NothingSelected { en: "Nothing is selected", ja: "何も選択されていません" }
    AlreadyOnTheGrid { en: "Already on the grid", ja: "すでにグリッド上にあります" }
    PastedFromClipboard { en: "Pasted", ja: "貼り付けました" }
    NothingToPaste { en: "Nothing to paste", ja: "貼り付けるものがありません" }
    NoClipToAccompany { en: "Select a clip holding the melody first", ja: "先に旋律の入ったクリップを選択してください" }
    NoClipToPasteInto { en: "Open a clip in the piano roll first", ja: "先にピアノロールでクリップを開いてください" }
    NoTrackToPasteOnto { en: "Select a track first", ja: "先にトラックを選択してください" }
    MetronomeOn { en: "Metronome on", ja: "メトロノーム オン" }
    MetronomeOff { en: "Metronome off", ja: "メトロノーム オフ" }

    // ------------------------------------------------------------------ undo labels
    // What the user sees after "Undid …". Nouns rather than imperatives, because that is how
    // both languages name a step that has already happened.
    EditToggleLoop { en: "toggling the cycle", ja: "サイクルの切り替え" }
    EditSetLoopRegion { en: "setting the cycle region", ja: "サイクル範囲の設定" }
    EditSetPunchRegion { en: "setting the punch region", ja: "パンチ範囲の設定" }
    EditChangeTempo { en: "the tempo change", ja: "テンポの変更" }
    EditSetTempoPoint { en: "writing a tempo change", ja: "テンポチェンジの追加" }
    EditRemoveTempoPoint { en: "removing a tempo change", ja: "テンポチェンジの削除" }
    EditChangeSignature { en: "the time signature", ja: "拍子の変更" }
    EditSetSignaturePoint { en: "writing a signature change", ja: "拍子記号の追加" }
    EditRemoveSignaturePoint { en: "removing a signature change", ja: "拍子記号の削除" }
    EditSetClipGain { en: "the clip gain", ja: "クリップゲインの変更" }
    EditSetClipFade { en: "the fade", ja: "フェードの変更" }
    EditCrossfade { en: "the crossfade", ja: "クロスフェード" }
    MenuCrossfade { en: "Crossfade", ja: "クロスフェード" }
    MenuFadeInShape { en: "Fade-In Shape", ja: "フェードインの形" }
    MenuFadeOutShape { en: "Fade-Out Shape", ja: "フェードアウトの形" }
    MenuFadeLinear { en: "Straight", ja: "直線" }
    MenuFadeEqualPower { en: "Equal Power", ja: "等パワー" }
    CmdCrossfade { en: "crossfade the clips", ja: "クリップをクロスフェード" }
    EditSetClipTempo { en: "the clip's tempo", ja: "クリップのテンポ" }
    EditAddInstrumentTrack { en: "adding an instrument track", ja: "ソフト音源トラックの追加" }
    EditAddSingerTrack { en: "adding a singer track", ja: "シンガートラックの追加" }
    EditAddAudioTrack { en: "adding an audio track", ja: "オーディオトラックの追加" }
    EditAddBusTrack { en: "adding a bus", ja: "バスの追加" }
    EditDeleteTrack { en: "deleting a track", ja: "トラックの削除" }
    EditDuplicateTrack { en: "duplicating a track", ja: "トラックの複製" }
    EditMoveTrack { en: "moving a track", ja: "トラックの並べ替え" }
    EditRenameTrack { en: "renaming a track", ja: "トラック名の変更" }
    EditMuteTrack { en: "muting a track", ja: "トラックのミュート" }
    EditSoloTrack { en: "soloing a track", ja: "トラックのソロ" }
    EditSetTrackOutput { en: "the routing", ja: "出力先の変更" }
    EditAddSend { en: "adding a send", ja: "センドの追加" }
    EditRemoveSend { en: "removing a send", ja: "センドの削除" }
    EditSetSendLevel { en: "the send level", ja: "センド量の変更" }
    EditSetSendPreFader { en: "where the send is taken from", ja: "センド位置の変更" }
    EditChangeInstrument { en: "changing the instrument", ja: "音源の変更" }
    EditAddClip { en: "adding a clip", ja: "クリップの追加" }
    EditAccompany { en: "adding an accompaniment", ja: "伴奏の追加" }
    EditComposeLyrics { en: "composing from lyrics", ja: "歌詞からの作曲" }
    EditDeleteClip { en: "deleting a clip", ja: "クリップの削除" }
    EditCutClips { en: "cutting clips", ja: "クリップの切り取り" }
    EditPasteClips { en: "pasting clips", ja: "クリップの貼り付け" }
    EditDuplicateClip { en: "duplicating a clip", ja: "クリップの複製" }
    EditSplitClip { en: "splitting a clip", ja: "クリップの分割" }
    EditRenameClip { en: "renaming a clip", ja: "クリップ名の変更" }
    EditMuteClip { en: "muting a clip", ja: "クリップのミュート" }
    EditMoveClip { en: "moving a clip", ja: "クリップの移動" }
    EditResizeClip { en: "resizing a clip", ja: "クリップの長さ変更" }
    EditLoopClip { en: "looping a clip", ja: "クリップのループ" }
    EditAddNote { en: "adding a note", ja: "ノートの追加" }
    EditDeleteNotes { en: "deleting notes", ja: "ノートの削除" }
    EditCutNotes { en: "cutting notes", ja: "ノートの切り取り" }
    EditPasteNotes { en: "pasting notes", ja: "ノートの貼り付け" }
    EditDuplicateNotes { en: "duplicating notes", ja: "ノートの複製" }
    EditTransposeNotes { en: "transposing notes", ja: "ノートの移調" }
    EditSetNoteVelocity { en: "changing note velocity", ja: "ノートの強さの変更" }
    EditMoveNotes { en: "moving notes", ja: "ノートの移動" }
    EditResizeNote { en: "resizing a note", ja: "ノートの長さ変更" }
    EditQuantizeNotes { en: "quantising", ja: "クオンタイズ" }
    EditSetLyric { en: "writing a lyric", ja: "歌詞の入力" }
    EditWriteLyrics { en: "writing lyrics", ja: "歌詞の流し込み" }
    EditSetPhonemes { en: "editing phonemes", ja: "音素の編集" }
    EditPhonemeDuration { en: "adjusting phoneme timing", ja: "音素の長さの調整" }
    EditResetPhonemeTiming { en: "resetting phoneme timing", ja: "音素タイミングのリセット" }
    EditScoop { en: "adjusting a scoop", ja: "しゃくりの調整" }
    EditFall { en: "adjusting a fall", ja: "フォールの調整" }
    EditVibrato { en: "adjusting a vibrato", ja: "ビブラートの調整" }
    EditResetOrnaments { en: "resetting pitch ornaments", ja: "ピッチ装飾のリセット" }
    EditSetFrameHop { en: "changing the frame hop", ja: "フレーム間隔の変更" }
    EditSetSingerVoice { en: "choosing a voice", ja: "声の選択" }
    EditSetSingerSpeaker { en: "choosing a speaker", ja: "話者の選択" }
    EditSing { en: "the sung take", ja: "歌唱テイク" }
    EditAddEffect { en: "adding an effect", ja: "エフェクトの追加" }
    EditRemoveEffect { en: "removing an effect", ja: "エフェクトの削除" }
    EditBypassEffect { en: "bypassing an effect", ja: "エフェクトのバイパス" }
    EditReorderEffects { en: "reordering the effects", ja: "エフェクトの並べ替え" }
    EditSetEffectSidechain { en: "changing the sidechain", ja: "サイドチェインの変更" }
    CmdSetEffectSidechain { en: "key this effect from that track", ja: "エフェクトをそのトラックでキーする" }
    MenuSidechain { en: "Sidechain", ja: "サイドチェイン" }
    MenuSidechainNone { en: "None", ja: "なし" }
    MenuRecordInput { en: "Record Input", ja: "録音入力" }
    MenuInputOff { en: "Not Armed", ja: "録音待機しない" }
    MenuCountInOff { en: "No Count-In", ja: "カウントインしない" }
    CountInOff { en: "Count-in off", ja: "カウントイン オフ" }
    CountIn { en: "Count-In", ja: "カウントイン" }
    EditAdjustParameter { en: "the parameter change", ja: "パラメーターの変更" }
    MenuFreezeTrack { en: "Keep Every Take Here", ja: "このトラックをすべて確定" }
    MenuTrackColor { en: "Colour", ja: "カラー" }
    MenuAutomateVolume { en: "Automate Volume", ja: "ボリュームをオートメーション" }
    MenuAutomatePan { en: "Automate Pan", ja: "パンをオートメーション" }
    MenuClearAutomation { en: "Clear Automation", ja: "オートメーションを消去" }
    MenuAutomate { en: "Automate", ja: "オートメーション" }
    MenuSetValue { en: "Set Value…", ja: "値を入力…" }
    MenuResetValue { en: "Reset to Default", ja: "既定値に戻す" }
    MenuCurveLine { en: "Line", ja: "直線" }
    MenuCurveStep { en: "Step", ja: "ステップ" }
    EditSetTrackColor { en: "the track colour", ja: "トラックの色" }
    EditSetTrackHeight { en: "the lane height", ja: "レーンの高さ" }
    EditWriteAutomation { en: "the automation point", ja: "オートメーションの点" }
    EditEraseAutomation { en: "erasing the automation point", ja: "オートメーションの点の削除" }
    EditWriteBend { en: "the pitch bend", ja: "ピッチベンド" }
    EditEraseBend { en: "erasing the pitch bend", ja: "ピッチベンドの削除" }
    EditWriteModulation { en: "the modulation", ja: "モジュレーション" }
    EditEraseModulation { en: "erasing the modulation", ja: "モジュレーションの削除" }
    EditWriteController { en: "the controller", ja: "コントロールチェンジ" }
    EditEraseController { en: "erasing the controller", ja: "コントロールチェンジの削除" }
    EditClearAutomation { en: "clearing the automation", ja: "オートメーションの消去" }
    EditImportAudio { en: "importing audio", ja: "オーディオの読み込み" }
    EditRecordTake { en: "recording a take", ja: "録音" }
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
    EditSetClipTransforms { en: "shaping a performance", ja: "演奏の調整" }
    EditFreezeClipTransforms { en: "keeping a performance", ja: "演奏の確定" }
    EditCompose { en: "composing a piece", ja: "自動作曲" }
    EditBalanceLevels { en: "balancing the mix", ja: "ミックスの自動調整" }

    // ------------------------------------------------------------------ errors
    ErrorUnknownTrack { en: "that track no longer exists", ja: "そのトラックは存在しません" }
    ErrorUnknownClip { en: "that clip no longer exists", ja: "そのクリップは存在しません" }
    ErrorUnknownSend { en: "that send no longer exists", ja: "そのセンドは存在しません" }
    ErrorNotABus {
        en: "only a bus can be routed into",
        ja: "ルーティングの送り先にできるのはバスだけです"
    }
    ErrorRoutingLoop {
        en: "that would send the signal back into itself",
        ja: "信号がループしてしまいます"
    }
    ErrorUnknownSoundFont {
        en: "that SoundFont is not part of this project",
        ja: "そのサウンドフォントはこのプロジェクトにありません"
    }
    ExternalChangeConflict {
        en: "the project changed on disk while this window holds unsaved work — Reload takes the disk's version, saving keeps this one",
        ja: "未保存の変更がある間にプロジェクトがディスク上で変更されました。「再読み込み」でディスク側を、保存でこちら側を残します"
    }
    ErrorLibraryMissing {
        en: "the shipped sound library is not installed, so General MIDI sounds are unavailable",
        ja: "同梱の音色ライブラリが未インストールのため、General MIDI音源は使えません"
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
    ErrorNothingToAccompany {
        en: "that clip has no notes in it, so there is no melody to read",
        ja: "そのクリップにはノートがないので、読み取れる旋律がありません"
    }
    ErrorWrongTrackKind {
        en: "that command does not apply to this kind of track",
        ja: "この種類のトラックには使えない操作です"
    }
    ErrorUnknownNote {
        en: "that note no longer exists",
        ja: "そのノートは存在しません"
    }
    ErrorNeedsDictionary {
        en: "reading kanji lyrics needs a Japanese dictionary; point the settings at a naist-jdic folder",
        ja: "漢字の歌詞を読むには日本語辞書が必要です。設定で naist-jdic フォルダを指定してください"
    }
    ErrorLyric {
        en: "the lyric could not be read",
        ja: "歌詞を読み取れませんでした"
    }
    ErrorDictionary {
        en: "the dictionary did not load",
        ja: "辞書を読み込めませんでした"
    }
    ErrorNoPath {
        en: "the project has no path yet; save it somewhere first",
        ja: "保存先が未設定です。先に保存してください"
    }
    ErrorRecordingNeedsFolder {
        en: "a recording needs a project folder to write into; save the project first",
        ja: "録音するにはプロジェクトフォルダが必要です。先に保存してください"
    }
    ErrorNothingToRecordOnto {
        en: "select an audio track to record onto, or arm one",
        ja: "録音先のオーディオトラックを選択するか、録音待機にしてください"
    }
    ErrorAlreadyRecording { en: "a recording is already running", ja: "すでに録音中です" }
    ErrorNotOverlapping {
        en: "those clips do not overlap on one track; drag one over the other first",
        ja: "そのクリップは重なっていません。片方をもう片方に重ねてください"
    }
    ErrorNothingToStem {
        en: "there is nothing to export as stems; add a track that makes a sound",
        ja: "ステム書き出しの対象がありません。音を出すトラックを追加してください"
    }
    ErrorRecordingInProgress { en: "stop the recording before changing the audio device", ja: "オーディオデバイスを変更する前に録音を停止してください" }
    ErrorNotRecording { en: "no recording is running", ja: "録音していません" }
    ErrorNoVoice {
        en: "choose a voice model for this track first",
        ja: "先にこのトラックの声のモデルを選んでください"
    }
    ErrorSingingNeedsFolder {
        en: "a sung take needs a project folder to live in; save the project first",
        ja: "歌唱テイクの保存先が必要です。先にプロジェクトを保存してください"
    }
    ErrorNothingToSing {
        en: "there are no notes to sing",
        ja: "歌うノートがありません"
    }
    ErrorNoLyrics {
        en: "the lyrics contain nothing to sing",
        ja: "歌詞に歌える言葉がありません"
    }
    ErrorSing {
        en: "the voice model failed",
        ja: "声のモデルでエラーが発生しました"
    }
    ErrorFile { en: "file error", ja: "ファイルエラー" }
    ErrorEngine { en: "audio engine error", ja: "オーディオエンジンのエラー" }
    ErrorPlugin { en: "plugin error", ja: "プラグインのエラー" }
    ErrorDocument { en: "the document is not valid", ja: "ドキュメントが不正です" }

    // ------------------------------------------------------------------ command line
    CliUsage {
        en: "\
auris — Auris Studio from the command line

USAGE
    auris <command> [options]

COMMANDS
    compose <song.asong> [opts]   Write a piece from a specification
    presets                       List the whole songs a piece can start from
    progressions                  List every chord progression known by name
    plugins                       List every registered instrument and effect
    soundfonts [--manifest]       List the SoundFonts shipped with this build
    dictionary [--manifest]       Show the shipped Japanese dictionary and whether it is installed
    info <project.auris>          Print a project's tracks, clips and duration
    render <project.auris> [opts] Render a project to a WAV file
    sing <project.auris> [opts]   Render a singer track through its voice model
    frames <project.auris> [opts] Write the frames a singer track's voice is fed
    sing-frames <frames.json> [opts]
                                  Sing a frames file through a voice model, into a WAV
    new <project.auris> [opts]    Create a project with one instrument track
    collect <project.auris>       Copy everything the project uses into its folder
    help                          Show this message

COMPOSE OPTIONS
        --preset <name>           Start from a song preset instead of a file
    -o, --output <file.auris>     Where to write (default: alongside the specification)
        --seed <n>                Override the seed, so the same spec writes a different piece
        --key <key>               Override the key, as in `C minor`
        --tempo <bpm>             Override the tempo
        --mood <word>             Override the mood
        --groove <name>           Override the groove
        --scale <name>            Override the scale
        --swing <amount>          Override the swing, from 0 to 1
        --set \"<field>: <value>\"  Override any field at all
        --print                   Print the resolved specification instead of writing
        --force                   Write even where a project folder of that name exists

RENDER OPTIONS
    -o, --output <file.wav>       Where to write (default: alongside the project)
        --bit-depth <16|24|32>    Sample format; 32 means 32-bit float (default: 24)
        --dither                  Add TPDF dither, for 16-bit masters
        --no-tail                 Stop at the last clip instead of letting effect tails ring
        --loop                    Render only the project's cycle region
        --stems <folder>          Write one file per track into a folder instead of a mix

SING OPTIONS
        --track <name>            Which singer track (default: the project's only one)
        --voice <model.onnx>      Choose the voice model first, and remember it
        --speaker <name>          Which of the voice's speakers sings, and remember it
        --seed <n>                Pin the take's random choices (default: the current take's)

FRAMES OPTIONS
        --track <name>            Which singer track (default: the project's only one)
    -o, --output <file.json>      Where to write (default: alongside the project)

SING-FRAMES OPTIONS
        --voice <model.onnx>      The voice to sing with (required)
        --speaker <name>          Which of the voice's speakers sings (default: its first)
        --seed <n>                Pin the take's random choices (default: 0)
        --acceleration <auto|gpu|cpu>
                                  Where the model runs its inference (default: auto)
    -o, --output <file.wav>       Where to write (default: alongside the frames)
        --report <file.json>      Also write the take's facts and timings as JSON

NEW OPTIONS
        --bpm <tempo>             Tempo of the new project (default: 120)
        --sample-rate <hz>        Rate of the new project (default: 48000)
        --force                   Write even where a project folder of that name exists",
        ja: "\
auris — コマンドラインから使う Auris Studio

使い方
    auris <コマンド> [オプション]

コマンド
    compose <song.asong> [opts]   仕様ファイルから曲を書き出す
    presets                       曲の出発点になる楽曲プリセットを一覧表示
    progressions                  名前の付いたコード進行を一覧表示
    plugins                       登録済みの音源とエフェクトを一覧表示
    soundfonts [--manifest]       同梱サウンドフォントを一覧表示
    dictionary [--manifest]       同梱日本語辞書とその導入状態を表示
    info <project.auris>          プロジェクトのトラック・クリップ・長さを表示
    render <project.auris> [opts] プロジェクトを WAV に書き出す
    sing <project.auris> [opts]   シンガートラックを声のモデルで歌わせる
    frames <project.auris> [opts] シンガートラックが声のモデルに渡すフレームを書き出す
    sing-frames <frames.json> [opts]
                                  フレームファイルを声のモデルで歌わせて WAV にする
    new <project.auris> [opts]    ソフト音源トラック 1 本のプロジェクトを作成
    collect <project.auris>       プロジェクトが使うファイルをフォルダ内に集める
    help                          このメッセージを表示

compose のオプション
        --preset <name>           ファイルの代わりに楽曲プリセットから書き出す
    -o, --output <file.auris>     出力先（既定: 仕様ファイルと同じ場所）
        --seed <n>                シードを上書き。同じ仕様から別の曲になります
        --key <key>               調を上書き（例: `C minor`）
        --tempo <bpm>             テンポを上書き
        --mood <word>             曲調を上書き
        --groove <name>           グルーヴを上書き
        --scale <name>            スケールを上書き
        --swing <amount>          スウィングを上書き（0〜1）
        --set \"<field>: <value>\"  任意の項目を上書き
        --print                   書き出さずに解決後の仕様を表示
        --force                   同名のプロジェクトフォルダがあっても書き出す

render のオプション
    -o, --output <file.wav>       出力先（既定: プロジェクトと同じ場所）
        --bit-depth <16|24|32>    量子化ビット数。32 は 32bit float（既定: 24）
        --dither                  TPDF ディザを付加（16bit マスター向け）
        --no-tail                 エフェクトの残響を待たず最後のクリップで終える
        --loop                    サイクル範囲のみを書き出す
        --stems <folder>          ミックスの代わりにトラックごとのファイルをフォルダに書き出す

sing のオプション
        --track <name>            歌わせるシンガートラック（既定: プロジェクト唯一の 1 本）
        --voice <model.onnx>      先に声のモデルを選んで記憶させる
        --speaker <name>          声のどの話者が歌うかを選んで記憶させる
        --seed <n>                テイクの乱数を固定（既定: 現在のテイクのシード）

frames のオプション
        --track <name>            対象のシンガートラック（既定: プロジェクト唯一の 1 本）
    -o, --output <file.json>      出力先（既定: プロジェクトと同じ場所）

sing-frames のオプション
        --voice <model.onnx>      歌わせる声のモデル（必須）
        --speaker <name>          声のどの話者が歌うか（既定: 最初の話者）
        --seed <n>                テイクの乱数を固定（既定: 0）
        --acceleration <auto|gpu|cpu>
                                  モデルの推論を走らせる場所（既定: auto）
    -o, --output <file.wav>       出力先（既定: フレームファイルと同じ場所）
        --report <file.json>      テイクの情報と所要時間を JSON でも書き出す

new のオプション
        --bpm <tempo>             新規プロジェクトのテンポ（既定: 120）
        --sample-rate <hz>        新規プロジェクトのサンプルレート（既定: 48000）
        --force                   同名のプロジェクトフォルダがあっても書き出す"
    }
    LogPanel { en: "Log", ja: "ログ" }
    LogClear { en: "Clear", ja: "消去" }
    LogEmpty { en: "Nothing has been logged.", ja: "ログはまだありません。" }
    CmdShowLog { en: "Log", ja: "ログ" }
    AgentPanel { en: "Agent", ja: "エージェント" }
    CmdShowAgent { en: "Agent", ja: "エージェント" }
    AgentPlaceholder { en: "Ask about this song…", ja: "この曲について話しかける…" }
    AgentNotConfigured { en: "Name a model to talk to.", ja: "話しかけるモデルを設定してください。" }
    AgentConfigure { en: "Settings", ja: "設定" }
    AgentApply { en: "Apply", ja: "適用" }
    AgentProviderLabel { en: "Provider", ja: "プロバイダ" }
    AgentModelLabel { en: "Model", ja: "モデル" }
    AgentUrlLabel { en: "Base URL", ja: "ベースURL" }
    AgentKeyEnvLabel { en: "API key variable", ja: "APIキーの環境変数" }
    AgentWorking { en: "Working…", ja: "作業中…" }
    AgentEnded { en: "The agent process ended.", ja: "エージェントのプロセスが終了しました。" }
    AgentReloaded {
        en: "The agent changed this project; the window reloaded it.",
        ja: "エージェントがこのプロジェクトを変更したため、読み込み直しました。"
    }
    AgentReloadOffer {
        en: "The agent changed this project, but the window holds unsaved edits.",
        ja: "エージェントがこのプロジェクトを変更しましたが、ウィンドウに未保存の編集があります。"
    }
    AgentReload { en: "Reload", ja: "再読み込み" }
    AgentChooseModel { en: "Choose a model…", ja: "モデルを選択…" }
    AgentModelsFetch { en: "Refresh", ja: "更新" }
    AgentModelsFetching { en: "Asking the provider for its models…", ja: "モデル一覧を取得中…" }
    CliPresets { en: "SONG PRESETS", ja: "楽曲プリセット" }
    CliSoundFonts { en: "SHIPPED SOUNDFONTS", ja: "同梱サウンドフォント" }
    CliSoundFontMissing {
        en: "not installed — run tools/fetch-soundfonts.sh",
        ja: "未インストール — tools/fetch-soundfonts.sh を実行してください"
    }
    BrowserVoices { en: "Voices", ja: "シンガーボイス" }
    BrowserNoVoices {
        en: "Put .onnx voices in a Voices folder, or add a folder below",
        ja: "Voices フォルダに .onnx を置くか、下からフォルダを追加してください"
    }
    BrowserAddVoiceFolder { en: "Add Voice Folder…", ja: "ボイスフォルダを追加…" }
    DialogVoiceFolder { en: "Choose a folder of voice models", ja: "ボイスモデルのフォルダを選択" }
    CliDictionary { en: "SHIPPED JAPANESE DICTIONARY", ja: "同梱日本語辞書" }
    CliDictionaryMissing {
        en: "not installed — run tools/fetch-dictionary.sh",
        ja: "未インストール — tools/fetch-dictionary.sh を実行してください"
    }
    CliInstruments { en: "INSTRUMENTS", ja: "音源" }
    CliEffects { en: "EFFECTS", ja: "エフェクト" }
    CliExpectedProjectPath { en: "expected a project path", ja: "プロジェクトのパスを指定してください" }
    CliExpectedFramesPath { en: "expected a frames file", ja: "フレームファイルのパスを指定してください" }
    CliNeedsVoice {
        en: "sing-frames needs --voice <model.onnx>",
        ja: "sing-frames には --voice <model.onnx> が必要です"
    }
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
    CliKindSinger { en: "singer", ja: "シンガー" }
    CliKindAudio { en: "audio", ja: "オーディオ" }
    CliKindBus { en: "bus", ja: "バス" }
    CliFieldRouting { en: "routing", ja: "ルーティング" }
    CliClipCount { en: "clip(s)", ja: "クリップ" }
    CliProgressions { en: "PROGRESSIONS", ja: "コード進行" }
    CliKeptProgressions { en: "KEPT", ja: "登録済みのコード進行" }
    CliExpectedSpecPath {
        en: "expected a path to a song specification",
        ja: "曲の仕様ファイルのパスを指定してください"
    }
    CliSpecOrPreset {
        en: "pass either a song specification or --preset, not both",
        ja: "曲の仕様ファイルと --preset はどちらか一方だけ指定してください"
    }

    // ------------------------------------------------------------------ parameter values
    ValueOn { en: "On", ja: "オン" }
    ValueOff { en: "Off", ja: "オフ" }

    // ------------------------------------------------------------------ plugin warnings
    // Shown while the sampler's envelope is switched on, because switching it on costs something
    // a user cannot otherwise see: a shaped note needs a MIDI channel to itself.
    SamplerEnvelopeOn {
        en: "Envelope on: 14-note polyphony, and drum choke groups stop working.",
        ja: "エンベロープ有効：同時発音は14音まで。ドラムのチョークグループも無効になります。"
    }

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
            Key::RecordInitial,
            Key::MonitorInitial,
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
