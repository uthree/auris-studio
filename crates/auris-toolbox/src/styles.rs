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
        let (genre, usage) = text.split_once('\n').unwrap();
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
}
