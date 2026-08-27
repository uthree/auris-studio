//! OpenJTalk's phoneme names, translated into this crate's IPA tokens.
//!
//! The dictionary path — [`JapaneseDictionary`](crate::JapaneseDictionary) — comes back speaking
//! OpenJTalk: full-context labels whose phonemes are spelt `sh`, `ch`, `ky`, `N`. The kana table
//! speaks IPA directly. This map is what makes the two paths agree, and the test at the bottom
//! is the contract: a syllable read either way must produce the same tokens.

/// One OpenJTalk phoneme as IPA tokens, or `None` for a name this crate has never heard of.
///
/// `sil` and `pau` map to no tokens at all rather than to the silence token: they mark the
/// edges and pauses of the *sentence* the dictionary was shown, and where silence falls in the
/// song is the notes' business, not the text's.
pub fn openjtalk_phoneme(name: &str) -> Option<&'static [&'static str]> {
    let tokens: &[&str] = match name {
        "sil" | "pau" => &[],
        "a" => &["a"],
        "i" => &["i"],
        "u" => &["ɯ"],
        "e" => &["e"],
        "o" => &["o"],
        "N" => &["ɴ"],
        "cl" => &["ʔ"],
        "k" => &["k"],
        "ky" => &["kʲ"],
        "g" => &["g"],
        "gy" => &["gʲ"],
        "s" => &["s"],
        "sh" => &["ɕ"],
        "z" => &["z"],
        "j" => &["dʑ"],
        "t" => &["t"],
        "ch" => &["tɕ"],
        "ts" => &["ts"],
        "ty" => &["tʲ"],
        "d" => &["d"],
        "dy" => &["dʲ"],
        "n" => &["n"],
        "ny" => &["nʲ"],
        "h" => &["h"],
        "hy" => &["ç"],
        "f" => &["ɸ"],
        "b" => &["b"],
        "by" => &["bʲ"],
        "p" => &["p"],
        "py" => &["pʲ"],
        "m" => &["m"],
        "my" => &["mʲ"],
        "y" => &["j"],
        "r" => &["ɾ"],
        "ry" => &["ɾʲ"],
        "w" => &["w"],
        "v" => &["v"],
        _ => return None,
    };
    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kana::kana_phonemes;

    #[test]
    fn the_two_paths_speak_the_same_tokens() {
        // Kana that OpenJTalk pronounces with each of its phoneme names, and the labels it
        // writes for them. If this drifts, the same syllable trains a model as two symbols.
        let syllables = [
            ("しゃ", vec!["sh", "a"]),
            ("ちょ", vec!["ch", "o"]),
            ("きゅ", vec!["ky", "u"]),
            ("じ", vec!["j", "i"]),
            ("ふ", vec!["f", "u"]),
            ("ひゃ", vec!["hy", "a"]),
            ("ら", vec!["r", "a"]),
            ("ん", vec!["N"]),
            ("っ", vec!["cl"]),
        ];
        for (kana, labels) in syllables {
            let through_labels: Vec<String> = labels
                .iter()
                .flat_map(|label| {
                    openjtalk_phoneme(label)
                        .unwrap_or_else(|| panic!("unknown label {label}"))
                        .iter()
                        .map(|s| s.to_string())
                })
                .collect();
            assert_eq!(
                kana_phonemes(kana).unwrap(),
                through_labels,
                "{kana} reads differently through the two paths"
            );
        }
    }

    #[test]
    fn sentence_edges_are_nobodys_phonemes() {
        assert_eq!(openjtalk_phoneme("sil"), Some(&[][..]));
        assert_eq!(openjtalk_phoneme("pau"), Some(&[][..]));
        assert_eq!(openjtalk_phoneme("xx"), None);
    }
}
