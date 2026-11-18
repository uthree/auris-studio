//! What a song's genre and usage add to the phase instructions.
//!
//! Each genre and each usage is one page under `docs/genres/` or `docs/usages/`: a title, a
//! summary of what sets it apart, then optionally a `## Score`, `## Arrangement` and `## Mix`
//! section. A phase's instructions carry the summary and that phase's section, so a model
//! writing a lo-fi loop reads lo-fi and looping guidance and nothing about drops or choruses.
//! The pages are also documents `search_documentation` returns, which is how the skill's
//! agents read the same words.

use auris_session::production_record::Phase;
use serde_json::Value;

macro_rules! pages {
    ($folder:literal: $($id:literal),* $(,)?) => {
        &[$((
            $id,
            concat!("docs/", $folder, "/", $id, ".md"),
            include_str!(concat!("../../../docs/", $folder, "/", $id, ".md")),
        )),*]
    };
}

/// Every genre page: the stored id, the document path and the text.
pub(crate) const GENRES: &[(&str, &str, &str)] = pages!("genres":
    "j_pop", "anime_vocaloid", "rock", "city_pop", "rnb_neo_soul", "hip_hop_lofi",
    "dance_electronic", "jazz", "orchestral", "ambient", "folk_acoustic", "latin", "chiptune",
);

/// Every usage page: the stored id, the document path and the text.
pub(crate) const USAGES: &[(&str, &str, &str)] = pages!("usages": "song", "loop_bgm", "cue");

/// The guidance a song's style adds to `phase`'s instructions, or nothing when it is unset.
///
/// `style` is the `style` object an inspection reports: `{"genre": id, "usage": id}`, either
/// key absent when undecided.
pub fn guidance(style: &Value, phase: Phase) -> Option<String> {
    let parts: Vec<String> = [("genre", GENRES), ("usage", USAGES)]
        .into_iter()
        .filter_map(|(key, pages)| {
            let id = style[key].as_str()?;
            let (_, _, text) = pages.iter().find(|(page, _, _)| *page == id)?;
            Some(for_phase(text, phase))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Every phase's guidance for a style just chosen, or nothing when it is unset.
///
/// Choosing the style is the one moment certain to happen: a model writing a short piece often
/// stays in the phase it chose the style in and never reads another phase's instructions, so the
/// arrangement and mix sections would otherwise never reach it. The pages go whole, genre first.
pub fn whole(style: &Value) -> Option<String> {
    let parts: Vec<&str> = [("genre", GENRES), ("usage", USAGES)]
        .into_iter()
        .filter_map(|(key, pages)| {
            let id = style[key].as_str()?;
            let (_, _, text) = pages.iter().find(|(page, _, _)| *page == id)?;
            Some(text.trim())
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// A page's title and summary, followed by the section for `phase` when it has one.
fn for_phase(text: &str, phase: Phase) -> String {
    let heading = match phase {
        Phase::Score => "## Score",
        Phase::Arrangement => "## Arrangement",
        Phase::Mix => "## Mix",
    };
    let mut sections = text.split("\n## ");
    let head = sections.next().unwrap_or_default();
    let mut lines = head.lines().filter(|line| !line.trim().is_empty());
    let title = lines.next().unwrap_or_default().trim_start_matches("# ");
    let summary = lines.collect::<Vec<_>>().join(" ");
    let section = sections
        .find(|section| section.starts_with(&heading[3..]))
        .map(|section| section.split_once('\n').map_or("", |(_, body)| body.trim()))
        .filter(|body| !body.is_empty());
    match section {
        Some(body) => format!("{title}. {summary}\n{body}"),
        None => format!("{title}. {summary}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::style::{Genre, Usage};
    use auris_session::{Session, SessionOptions};
    use serde_json::json;

    #[test]
    fn every_genre_and_usage_has_a_page_under_its_stored_id() {
        let ids: Vec<&str> = GENRES.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, Genre::ALL.map(Genre::id));
        let ids: Vec<&str> = USAGES.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, Usage::ALL.map(Usage::id));
        for (id, _, text) in GENRES.iter().chain(USAGES) {
            assert!(text.starts_with("# "), "{id} has no title");
        }
    }

    #[test]
    fn an_unset_style_adds_nothing() {
        assert_eq!(guidance(&json!({}), Phase::Score), None);
        assert_eq!(guidance(&Value::Null, Phase::Mix), None);
    }

    #[test]
    fn a_phase_gets_the_summary_and_only_its_own_section() {
        let page =
            "# Genre: Test\n\nOne line.\nTwo line.\n\n## Score\nWrite it.\n\n## Mix\nMix it.\n";
        assert_eq!(
            for_phase(page, Phase::Score),
            "Genre: Test. One line. Two line.\nWrite it."
        );
        assert_eq!(
            for_phase(page, Phase::Arrangement),
            "Genre: Test. One line. Two line."
        );
        assert_eq!(
            for_phase(page, Phase::Mix),
            "Genre: Test. One line. Two line.\nMix it."
        );
    }

    #[test]
    fn genre_and_usage_are_both_given_genre_first() {
        let text = guidance(
            &json!({"genre":"chiptune","usage":"loop_bgm"}),
            Phase::Score,
        )
        .unwrap();
        let (genre, usage) = text.split_once("\nUsage: ").unwrap();
        let usage = format!("Usage: {usage}");
        assert!(genre.starts_with("Genre: Chiptune."), "{genre}");
        assert!(
            usage.starts_with("Usage: Loop background music."),
            "{usage}"
        );
        let usage_only = guidance(&json!({"usage":"cue"}), Phase::Mix).unwrap();
        assert!(usage_only.starts_with("Usage: Cue."), "{usage_only}");
    }

    /// The level a page's Mix section names: "Target -14 LUFS" or "target -16 to -18 LUFS".
    fn named_loudness(text: &str) -> Option<(f32, f32)> {
        let mix = text.split("## Mix").nth(1)?;
        let at = mix.to_lowercase().find("target ")? + "target ".len();
        let words: Vec<&str> = mix[at..].split_whitespace().collect();
        let first: f32 = words.first()?.parse().ok()?;
        Some(match words.get(1) {
            Some(&"to") => {
                let second: f32 = words.get(2)?.parse().ok()?;
                (first.min(second), first.max(second))
            }
            _ => (first - 1.0, first + 1.0),
        })
    }

    /// The words of a page's Score section that are written as chord numerals: a roman
    /// numeral, after any accidentals, followed by nothing or by a quality.
    fn numerals(text: &str) -> Vec<String> {
        let score = text
            .split("## Score")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .unwrap_or_default();
        const QUALITIES: [&str; 11] = [
            "maj", "m", "M", "add", "dim", "aug", "sus", "/", "ø", "°", "+",
        ];
        score
            .split(|c: char| c.is_whitespace() || ",;:()`–".contains(c))
            .map(|word| word.trim_end_matches('.'))
            .filter(|word| {
                let bare = word.trim_start_matches(['b', '#']);
                let roman = bare.len() - bare.trim_start_matches(['I', 'V', 'i', 'v']).len();
                let rest = &bare[roman..];
                roman > 0
                    && (rest.is_empty()
                        || rest.starts_with(|c: char| c.is_ascii_digit())
                        || QUALITIES.iter().any(|quality| rest.starts_with(quality)))
            })
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_chord_a_page_writes_is_one_edit_harmony_reads() {
        use auris_session::prelude::Numeral;
        for (id, _, text) in GENRES.iter().chain(USAGES) {
            for numeral in numerals(text) {
                assert!(
                    Numeral::parse(&numeral).is_some(),
                    "{id} writes {numeral}, which edit_harmony does not read"
                );
            }
        }
        assert!(
            numerals(GENRES[0].2).len() > 5,
            "the reader finds the numerals"
        );
    }

    #[test]
    fn the_lint_holds_a_mix_to_the_level_its_page_names() {
        use auris_session::session::style_checks::style_checks;
        use auris_session::style::Style;
        for genre in Genre::ALL {
            let (_, _, text) = GENRES.iter().find(|(id, _, _)| *id == genre.id()).unwrap();
            let style = Style {
                genre: Some(genre),
                usage: None,
            };
            assert_eq!(
                style_checks(style).loudness,
                named_loudness(text),
                "{}",
                genre.id()
            );
        }
        for usage in Usage::ALL {
            let (_, _, text) = USAGES.iter().find(|(id, _, _)| *id == usage.id()).unwrap();
            let style = Style {
                genre: None,
                usage: Some(usage),
            };
            assert_eq!(
                style_checks(style).loudness,
                named_loudness(text),
                "{}",
                usage.id()
            );
        }
    }

    #[test]
    fn a_chosen_style_brings_every_phase_of_both_pages() {
        let text = whole(&json!({"genre":"city_pop","usage":"loop_bgm"})).unwrap();
        assert!(text.starts_with("# Genre: City pop"), "{text}");
        let usage = text.find("# Usage: Loop background music").unwrap();
        for section in ["## Score", "## Arrangement", "## Mix"] {
            assert!(text[..usage].contains(section), "genre lacks {section}");
            assert!(text[usage..].contains(section), "usage lacks {section}");
        }
        assert_eq!(whole(&json!({})), None);
    }

    const TECHNIQUES: &str = include_str!("../../../docs/composition-techniques.md");

    /// Every page that tells an agent which effect to use: the style pages and the techniques.
    fn effect_pages() -> Vec<(&'static str, &'static str)> {
        GENRES
            .iter()
            .chain(USAGES)
            .map(|(_, path, text)| (*path, *text))
            .chain([("docs/composition-techniques.md", TECHNIQUES)])
            .collect()
    }

    /// Words a line about effects may tick that are tools or operations, not parameter keys.
    const OPERATIONS: &[&str] = &[
        "mixer_graph",
        "add_effect",
        "move_effect",
        "enable_effect",
        "parameters",
        "add_output",
        "set_sidechain",
        "set_outputs",
        "normalize_mix",
        "inspect_audio",
        "automation",
    ];

    #[test]
    fn every_effect_and_parameter_a_page_names_is_one_the_session_has() {
        // A page that names a renamed effect or key sends the agent to an argument error, or
        // worse, to a parameter of another effect with the same key and another meaning.
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let ids: Vec<String> = session
            .registry()
            .effects()
            .map(|effect| effect.id.to_string())
            .collect();
        let mut named_any = 0;
        for (path, text) in effect_pages() {
            for line in text.lines() {
                let ticked: Vec<&str> = line.split('`').skip(1).step_by(2).collect();
                let effects: Vec<&str> = ticked
                    .iter()
                    .copied()
                    .filter(|token| token.starts_with("auris.fx."))
                    .collect();
                for id in &effects {
                    assert!(ids.iter().any(|known| known == id), "{path} names {id}");
                }
                if effects.is_empty() {
                    continue;
                }
                named_any += 1;
                let keys: Vec<String> = effects
                    .iter()
                    .flat_map(|id| {
                        session
                            .param_descriptors(id)
                            .iter()
                            .map(|param| param.key.to_string())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let parameter_like = |token: &&&str| {
                    !token.is_empty() && token.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                };
                for token in ticked.iter().filter(parameter_like) {
                    if OPERATIONS.contains(token) {
                        continue;
                    }
                    assert!(
                        keys.iter().any(|key| key == token),
                        "{path}: `{token}` is no parameter of {effects:?} in {line:?}"
                    );
                }
            }
        }
        assert!(named_any >= 20, "only {named_any} lines name an effect");
    }

    /// Whether `list` is technique headings joined by ", " and " and ", nothing else.
    fn headings_only(list: &str, headings: &[&str]) -> bool {
        list.is_empty()
            || headings.iter().any(|heading| {
                list.strip_prefix(heading).is_some_and(|rest| {
                    rest.is_empty()
                        || [", ", " and "].iter().any(|joint| {
                            rest.strip_prefix(joint)
                                .is_some_and(|rest| headings_only(rest, headings))
                        })
                })
            })
    }

    #[test]
    fn every_technique_a_style_page_looks_up_is_a_heading_search_can_find() {
        let headings: Vec<&str> = TECHNIQUES
            .lines()
            .filter_map(|line| line.strip_prefix("### "))
            .collect();
        for (path, text) in effect_pages() {
            for line in text.lines() {
                let Some(list) = line.strip_prefix("- Look up ") else {
                    continue;
                };
                let list = list.trim_end_matches('.');
                assert!(headings_only(list, &headings), "{path}: {line}");
            }
        }
    }
}
