//! Translations for the words plugins use.
//!
//! Plugin metadata belongs to the plugin, not to this crate: a third-party instrument will never
//! appear in any table here, and its parameter names still have to be shown. So these are
//! *lookups keyed by the English term* with a fallback to that term, rather than an enum.
//!
//! Keying on the English word rather than on `(plugin id, parameter key)` is deliberate. Audio
//! vocabulary is shared — every compressor's "Attack" is the same idea — so one entry translates
//! the word everywhere it appears, including in plugins written later by someone else.

use crate::Language;

/// Translation of a plugin's display name, or `name` itself when it is not known here.
pub fn plugin_name(name: &str, language: Language) -> &str {
    lookup(PLUGIN_NAMES, name, language)
}

/// Translation of a plugin's one-line description, or `text` itself when it is not known here.
pub fn plugin_description(text: &str, language: Language) -> &str {
    lookup(PLUGIN_DESCRIPTIONS, text, language)
}

/// Translation of a chord progression's or a groove's one-line description.
///
/// Same shape as [`plugin_description`], and here for the same reason: what the picker has to
/// show is a *sentence*, and the catalogue lives in `auris-core`, which may not name a language.
/// The names themselves — `royal-road`, `basic-rock` — are not translated: they are what the
/// text format writes and what a specification file has to say to get the same progression back.
pub fn theory_description(text: &str, language: Language) -> &str {
    lookup(THEORY_DESCRIPTIONS, text, language)
}

/// The short name a chord progression is shown under, or `name` itself when it is not known here.
///
/// Keyed on the catalogue's own name — `axis`, `royal-road` — because that is the stable thing: it
/// is what a `.asong` writes and what `auris compose` takes, and it does not move when somebody
/// rewords a description.
///
/// The only table here whose English column is a real entry rather than the key, and it has to be:
/// the key is a slug, and a picker row reading `royal-road` is not what anybody calls it. That is
/// also why the picker cannot simply show the description — "王道進行 (4536): the J-pop staple" is
/// a sentence, and a menu of sixteen sentences is a menu nobody can scan.
///
/// The progressions with Japanese names keep them in both columns, for the reason
/// [`theory_description`] gives: 王道進行 is what the thing is called.
pub fn theory_name(name: &str, language: Language) -> &str {
    THEORY_NAMES
        .iter()
        .find(|(key, _, _)| *key == name)
        .map_or(name, |(_, english, japanese)| match language {
            Language::English => english,
            Language::Japanese => japanese,
        })
}

/// Translation of a song preset's one-line description.
///
/// Same shape and the same reason as [`theory_description`]: the catalogue is a set of `.asong`
/// documents in `auris-compose`, which may not name a language, and what a picker shows is a
/// sentence. The names — `city-pop`, `jazz-trio` — are not translated: they are what
/// `auris compose --preset` takes.
pub fn preset_description(text: &str, language: Language) -> &str {
    lookup(PRESET_DESCRIPTIONS, text, language)
}

/// Translation of a parameter's display name, or `name` itself when it is not known here.
pub fn parameter(name: &str, language: Language) -> &str {
    lookup(PARAMETERS, name, language)
}

/// Translation of one option of a choice parameter, or `label` itself when unknown.
pub fn choice(label: &str, language: Language) -> &str {
    lookup(CHOICES, label, language)
}

/// Translation of a plugin category, or `label` itself when it is not known here.
pub fn category(label: &str, language: Language) -> &str {
    lookup(CATEGORIES, label, language)
}

/// Whether `term` appears in any table here.
///
/// Distinct from "the translation differs": a handful of audio terms — `Q`, `dB` — are the same
/// word in Japanese, so a completeness check has to ask whether the term was *considered*, not
/// whether it changed.
pub fn is_known(term: &str) -> bool {
    [
        PLUGIN_NAMES,
        PLUGIN_DESCRIPTIONS,
        PARAMETERS,
        CHOICES,
        CATEGORIES,
    ]
    .into_iter()
    .any(|table| table.iter().any(|(english, _)| *english == term))
}

/// Finds `term` in `table`, falling back to the term itself.
///
/// The fallback is the whole point: English is the canonical label a plugin ships with, so an
/// untranslated term shows as its author wrote it rather than as a missing-string marker.
fn lookup<'a>(
    table: &[(&'static str, &'static str)],
    term: &'a str,
    language: Language,
) -> &'a str {
    match language {
        Language::English => term,
        Language::Japanese => table
            .iter()
            .find(|(english, _)| *english == term)
            .map(|(_, translated)| *translated)
            .unwrap_or(term),
    }
}

/// Japanese names for the built-in plugins.
const PLUGIN_NAMES: &[(&str, &str)] = &[
    ("Chiptune", "チップチューン"),
    ("FM 2-Op", "FM 2 オペレーター"),
    ("Noise Drum", "ノイズドラム"),
    ("Vocal", "ボーカル"),
    ("Chorus", "コーラス"),
    ("Compressor", "コンプレッサー"),
    ("Delay", "ディレイ"),
    ("Distortion", "ディストーション"),
    ("Equalizer", "イコライザー"),
    ("Gain & Pan", "ゲイン & パン"),
    ("Limiter", "リミッター"),
    ("Reverb", "リバーブ"),
    ("SoundFont", "サウンドフォント"),
];

/// Japanese versions of the one-line descriptions shown in the plugin browser.
const PLUGIN_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "Band-limited pulse, saw, triangle and LFSR noise with unison and bit crushing",
        "帯域制限したパルス・ノコギリ・三角波と LFSR ノイズ。ユニゾンとビットクラッシュ付き",
    ),
    (
        "Two-operator phase modulation: bells, basses and electric pianos",
        "2 オペレーターの位相変調。ベル、ベース、エレピ向き",
    ),
    (
        "Pitch-swept band-passed LFSR noise: kicks, snares and hats without a sampler",
        "ピッチスイープとバンドパスを掛けた LFSR ノイズ。サンプラー無しでキック・スネア・ハット",
    ),
    (
        "A formant-filtered preview voice for singer tracks",
        "シンガートラック試聴用のフォルマントフィルター音声",
    ),
    (
        "Modulated delay that doubles and widens a voice",
        "音を二重にして広げるモジュレーションディレイ",
    ),
    (
        "Soft-knee compressor, stereo-linked and keyable from another track",
        "ソフトニー、ステレオリンクのコンプレッサー。他トラックでキー入力できる",
    ),
    (
        "Feedback delay with damping, tempo sync and a ping-pong mode",
        "ダンピング、テンポ同期、ピンポンモードを備えたフィードバックディレイ",
    ),
    (
        "Saturation, hard clipping, wave folding and bitcrushing",
        "サチュレーション、ハードクリップ、ウェーブフォールド、ビットクラッシュ",
    ),
    (
        "Six band EQ: high-pass, low shelf, two bells, high shelf, low-pass",
        "6 バンド EQ。ハイパス、ローシェルフ、ベル 2 基、ハイシェルフ、ローパス",
    ),
    (
        "Level, stereo position, stereo width and polarity",
        "レベル、定位、ステレオ幅、位相",
    ),
    (
        "Look-ahead brickwall limiter with a guaranteed output ceiling",
        "先読み型のブリックウォールリミッター。出力上限を保証します",
    ),
    (
        "Schroeder reverb: eight damped combs into four all-passes per channel",
        "シュレーダー型リバーブ。1 チャンネルあたり減衰コム 8 基とオールパス 4 基",
    ),
    (
        "Plays an imported SoundFont",
        "読み込んだサウンドフォントを再生します",
    ),
];

/// Japanese names for parameters. Shared across plugins on purpose — see the module note.
const PARAMETERS: &[(&str, &str)] = &[
    ("Attack", "アタック"),
    ("Bit Depth", "ビット深度"),
    ("Breath", "ブレス"),
    ("Ceiling", "上限"),
    ("Damping", "ダンピング"),
    ("Decay", "ディケイ"),
    ("Depth", "深さ"),
    ("Detune", "デチューン"),
    ("Drive", "ドライブ"),
    ("Envelope", "エンベロープ"),
    ("Feedback", "フィードバック"),
    ("Gain", "ゲイン"),
    ("Glide", "グライド"),
    ("Index", "変調指数"),
    ("Input", "入力"),
    ("Knee", "ニー"),
    ("Level", "レベル"),
    ("Makeup", "メイクアップ"),
    ("Mix", "ミックス"),
    ("Mod Decay", "変調ディケイ"),
    ("Mode", "モード"),
    ("Octave", "オクターブ"),
    ("Output", "出力"),
    ("Pan", "パン"),
    ("Phase Invert", "位相反転"),
    ("Ping-Pong", "ピンポン"),
    ("Pitch Sweep", "ピッチスイープ"),
    ("Pulse Width", "パルス幅"),
    ("Rate", "レート"),
    ("Ratio", "レシオ"),
    ("Release", "リリース"),
    ("Room Size", "ルームサイズ"),
    ("Steps", "ステップ数"),
    ("Sustain", "サステイン"),
    ("Sync", "同期"),
    ("Tone", "トーン"),
    ("Unison", "ユニゾン"),
    ("Vibrato", "ビブラート"),
    ("Vibrato Rate", "ビブラート速度"),
    ("Mod Depth", "モジュレーション深さ"),
    ("Unison Spread", "ユニゾン幅"),
    ("Waveform", "波形"),
    ("Width", "ステレオ幅"),
    ("Threshold", "スレッショルド"),
    ("Time", "タイム"),
    ("Pre-Delay", "プリディレイ"),
    // The equaliser names its bands by abbreviation, which stays an abbreviation in Japanese.
    // "Q" is "Q" in Japanese as well; the entries exist so the completeness test can tell the
    // difference between a term that was considered and one that was forgotten.
    ("HP Freq", "HP 周波数"),
    ("HP Gain", "HP ゲイン"),
    ("HP On", "HP 有効"),
    ("HP Q", "HP Q"),
    ("LS Freq", "LS 周波数"),
    ("LS Gain", "LS ゲイン"),
    ("LS On", "LS 有効"),
    ("LS Q", "LS Q"),
    ("P1 Freq", "P1 周波数"),
    ("P1 Gain", "P1 ゲイン"),
    ("P1 On", "P1 有効"),
    ("P1 Q", "P1 Q"),
    ("P2 Freq", "P2 周波数"),
    ("P2 Gain", "P2 ゲイン"),
    ("P2 On", "P2 有効"),
    ("P2 Q", "P2 Q"),
    ("HS Freq", "HS 周波数"),
    ("HS Gain", "HS ゲイン"),
    ("HS On", "HS 有効"),
    ("HS Q", "HS Q"),
    ("LP Freq", "LP 周波数"),
    ("LP Gain", "LP ゲイン"),
    ("LP On", "LP 有効"),
    ("LP Q", "LP Q"),
];

/// The short names the progression pickers show, keyed on the catalogue's own name.
///
/// `(catalogue name, English, Japanese)`. See [`theory_name`] for why the English column exists at
/// all, and for why more than half of these are the same in both.
const THEORY_NAMES: &[(&str, &str, &str)] = &[
    ("axis", "Axis", "アクシス進行"),
    (
        "axis-minor",
        "Axis from the sixth",
        "アクシス進行（vi 始まり）",
    ),
    ("epic", "Minor axis", "短調のアクシス"),
    ("komuro", "小室進行", "小室進行"),
    ("marusa", "丸サ進行", "丸サ進行"),
    ("marusa5", "丸サ進行 (ii–V)", "丸サ進行（ii-V 入り）"),
    ("royal-road", "王道進行", "王道進行"),
    ("koakuma", "小悪魔進行", "小悪魔進行"),
    ("naki", "泣きの進行", "泣きの進行"),
    ("canon", "カノン進行", "カノン進行"),
    ("junjo", "純情進行", "純情進行"),
    ("doo-wop", "Doo-wop", "ドゥーワップ進行"),
    ("ii-v-i", "ii–V–I", "ツーファイブワン"),
    ("blues", "Twelve-bar blues", "12 小節ブルース"),
    ("andalusian", "Andalusian cadence", "アンダルシア終止"),
    ("sad-loop", "Sad loop", "哀しい循環"),
];

/// The song presets, whose descriptions are what a style picker shows.
const PRESET_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "The built-in voices, four to the floor",
        "内蔵音源のみ・四つ打ち",
    ),
    (
        "Drums, bass, keys and a lead — the 王道進行",
        "ドラム・ベース・鍵盤・リード — 王道進行",
    ),
    (
        "Electric piano and slap bass over 丸サ進行",
        "エレピとスラップベース・丸サ進行",
    ),
    (
        "Overdriven guitar, organ and a hard kit",
        "歪んだギターとオルガン・強めのドラム",
    ),
    (
        "Piano, upright bass and brushes on a ii-V-I",
        "ピアノ・ウッドベース・ブラシ・ツーファイブワン",
    ),
    (
        "Strings, horns and timpani in 3/4",
        "弦・ホルン・ティンパニ・3拍子",
    ),
    (
        "Saw lead, analogue bass and a TR-808",
        "ノコギリ波リード・アナログベース・TR-808",
    ),
    (
        "Pads and a slow bell, no kit at all",
        "パッドと鐘・ドラムなし",
    ),
];

/// Japanese versions of the one-line descriptions the progression and groove pickers show.
///
/// The progressions with Japanese names keep them: 王道進行 is what the thing is called, and a
/// picker that said "the J-pop staple" instead would be naming it worse in either language.
const THEORY_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "The four chords of a thousand pop songs",
        "ポップスで千曲は書かれた 4 つのコード",
    ),
    (
        "The same four chords starting from the relative minor",
        "同じ 4 つのコードを平行短調から始めたもの",
    ),
    (
        "The minor axis: dark, modal, and everywhere in game music",
        "短調のアクシス。暗く、モーダルで、ゲーム音楽の定番",
    ),
    (
        "小室進行: minor start resolving to major",
        "小室進行: 短調で始まり長調に解決する",
    ),
    (
        "丸サ進行: the Just-the-Two-of-Us loop that never lands on a tonic",
        "丸サ進行: トニックに着地しない Just the Two of Us ループ",
    ),
    (
        "丸サ進行 with the ii-V into the subdominant spelled out",
        "丸サ進行。下属和音への ii-V を明示したもの",
    ),
    (
        "王道進行 (4536): the J-pop staple",
        "王道進行 (4536): J-POP の定番",
    ),
    (
        "小悪魔進行: 王道進行 with the dominant over a subdominant pedal",
        "小悪魔進行: 王道進行の属和音を下属和音のペダル上に置いたもの",
    ),
    (
        "泣きの進行: the royal road with a secondary dominant in its third bar",
        "泣きの進行: 王道進行の 3 小節目をセカンダリードミナントにしたもの",
    ),
    (
        "カノン進行, after Pachelbel",
        "カノン進行。パッヘルベルに由来",
    ),
    (
        "純情進行: the canon over a stepwise descending bass",
        "純情進行: カノン進行を順次下行するベースの上に置いたもの",
    ),
    ("The fifties progression", "50 年代進行"),
    (
        "The cadence jazz is built on",
        "ジャズの土台となるケーデンス",
    ),
    (
        "Twelve-bar blues with a quick change and a turnaround",
        "クイックチェンジとターンアラウンドを備えた 12 小節ブルース",
    ),
    (
        "The descending tetrachord: i bVII bVI V",
        "下行テトラコルド: i bVII bVI V",
    ),
    (
        "A four-bar loop that keeps turning back on itself",
        "同じところへ戻り続ける 4 小節ループ",
    ),
    (
        "Four on the snare's two and four, eighths on the hat",
        "スネアは 2・4 拍、ハットは 8 分",
    ),
    (
        "The straight eight-beat every J-rock song is built on",
        "J-ROCK の土台となる素直な 8 ビート",
    ),
    (
        "Busier hats and a syncopated kick",
        "細かいハットとシンコペーションしたキック",
    ),
    (
        "A kick on every beat, for house and its descendants",
        "全拍にキック。ハウスとその系譜向け",
    ),
    ("A swung eight-beat", "スウィングした 8 ビート"),
    (
        "A broken kick and a snare that lands early, with ghosts around it",
        "崩したキックと食い気味のスネア、まわりにゴーストノート",
    ),
    (
        "The clave on the rim over a surdo on every beat",
        "リムのクラーベと全拍のスルド",
    ),
    (
        "The backbeat moved to bar's centre, which halves the felt tempo",
        "バックビートを小節の中央へ移し、体感テンポを半分にしたもの",
    ),
    (
        "Almost nothing: for intros and ambient sections",
        "ほとんど何も鳴らさない。イントロやアンビエントな場面に",
    ),
    (
        "Two dotted beats, with the hat counting the eighths between them",
        "付点 2 拍。あいだの 8 分をハットが刻む",
    ),
    (
        "The 12/8 shuffle: a backbeat under a ride that skips",
        "12/8 のシャッフル。跳ねるライドの下にバックビート",
    ),
];

/// Japanese names for the browser's category headings.
const CATEGORIES: &[(&str, &str)] = &[
    ("Synth", "シンセ"),
    ("Sampler", "サンプラー"),
    ("Drum", "ドラム"),
    ("Equalizer", "イコライザー"),
    ("Dynamics", "ダイナミクス"),
    ("Delay", "ディレイ"),
    ("Reverb", "リバーブ"),
    ("Distortion", "ディストーション"),
    ("Modulation", "モジュレーション"),
    ("Utility", "ユーティリティ"),
    ("Other", "その他"),
];

/// Japanese labels for the options of choice parameters.
const CHOICES: &[(&str, &str)] = &[
    ("Sine", "サイン波"),
    ("Square", "矩形波"),
    ("Saw", "ノコギリ波"),
    ("Triangle", "三角波"),
    ("Noise", "ノイズ"),
    ("Soft (tanh)", "ソフト (tanh)"),
    ("Hard clip", "ハードクリップ"),
    ("Fold", "フォールド"),
    ("Bitcrush", "ビットクラッシュ"),
    ("Free", "フリー"),
    // The delay's note values read the same in any language; the entries exist so the
    // completeness test can tell a considered label from a forgotten one.
    ("1/1", "1/1"),
    ("1/2.", "1/2."),
    ("1/2", "1/2"),
    ("1/4.", "1/4."),
    ("1/4", "1/4"),
    ("1/8.", "1/8."),
    ("1/8", "1/8"),
    ("1/8T", "1/8T"),
    ("1/16", "1/16"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_term_is_translated_and_an_unknown_one_is_not() {
        assert_eq!(parameter("Attack", Language::Japanese), "アタック");
        assert_eq!(parameter("Attack", Language::English), "Attack");
        assert_eq!(
            parameter("Warp Factor", Language::Japanese),
            "Warp Factor",
            "a plugin we have never heard of must still show its own label"
        );
    }

    #[test]
    fn every_table_entry_says_something() {
        for table in [
            PLUGIN_NAMES,
            PLUGIN_DESCRIPTIONS,
            PARAMETERS,
            CHOICES,
            CATEGORIES,
        ] {
            for (english, japanese) in table {
                assert!(!english.is_empty() && !japanese.is_empty());
                assert!(is_known(english));
            }
        }
    }

    #[test]
    fn only_abbreviations_are_left_as_they_are() {
        // An entry identical in both languages is nearly always a forgotten translation, so the
        // ones that are genuinely the same word are the ones ending in an abbreviation.
        for (english, japanese) in PARAMETERS {
            if english == japanese {
                assert!(
                    english.ends_with(" Q"),
                    "`{english}` is identical in both languages"
                );
            }
        }
    }

    #[test]
    fn no_english_term_appears_twice() {
        // A second entry for a term would be dead: the lookup stops at the first match, so the
        // duplicate silently never applies.
        for table in [
            PLUGIN_NAMES,
            PLUGIN_DESCRIPTIONS,
            PARAMETERS,
            CHOICES,
            CATEGORIES,
        ] {
            let mut terms: Vec<&str> = table.iter().map(|(english, _)| *english).collect();
            terms.sort_unstable();
            let before = terms.len();
            terms.dedup();
            assert_eq!(before, terms.len(), "a term is listed twice");
        }
    }
}
