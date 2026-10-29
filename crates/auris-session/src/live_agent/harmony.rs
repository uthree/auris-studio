//! Bounded live harmony edits, validated before recording a single undo transaction.
use super::*;
use serde::{Deserialize, Deserializer, de::Error as _};

/// Explicit bar groups or a chart with written bar separators.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(super) enum ChordChartInput {
    /// One entry per bar; multiple chord symbols in an entry divide that bar equally.
    Bars(#[schemars(length(min = 1, max = 4096))] Vec<String>),
    /// A pipe-delimited chart, a single chord, or a catalogue reference such as @axis.
    Chart(String),
}

pub(super) fn deserialize_chords<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    match Option::<ChordChartInput>::deserialize(deserializer)? {
        None => Ok(None),
        Some(ChordChartInput::Chart(text)) => Ok(Some(text)),
        Some(ChordChartInput::Bars(bars)) => {
            if bars.is_empty() || bars.len() > 4096 {
                return Err(D::Error::custom("Supply 1..4096 chord bars"));
            }
            if bars.iter().any(|bar| {
                let bar = bar.trim();
                bar.is_empty() || bar.contains('|') || bar.starts_with('@')
            }) {
                return Err(D::Error::custom(
                    "Each chords array entry is one bar: use chord symbols such as I or I V, without pipes or catalogue names",
                ));
            }
            Ok(Some(format!("| {} |", bars.join(" | "))))
        }
    }
}

impl Session {
    pub(super) fn agent_edit_harmony(
        &mut self,
        start_bar: u32,
        bars: u32,
        key: Option<&str>,
        chords: Option<&str>,
        clear_chords: bool,
        section: Option<&str>,
    ) -> Result<String, String> {
        if start_bar == 0 || !(1..=4096).contains(&bars) {
            return Err("start_bar must be at least 1 and bars within 1..4096".into());
        }
        if (key.is_none() && chords.is_none() && !clear_chords && section.is_none())
            || (clear_chords && chords.is_some())
        {
            return Err("provide key, chords, section, or clear_chords without chords".into());
        }
        if section.is_some_and(|label| {
            label.trim().is_empty() || label.len() > 256 || label.chars().any(char::is_control)
        }) {
            return Err("section must be a nonempty label within 256 UTF-8 bytes, without control characters".into());
        }
        if chords.is_some_and(|text| {
            let text = text.trim();
            !text.starts_with('@') && !text.contains('|') && text.split_whitespace().count() > 1
        }) {
            return Err(
                r#"Ambiguous chords: an unseparated list puts every chord in one bar and repeats it. For one chord per bar use chords:["I","V","vi","IV"] or "| I | V | vi | IV |". For intentional changes within one bar use ["I V"] or "| I V |"."#.into(),
            );
        }
        let after = start_bar.checked_add(bars).ok_or("bar range overflow")?;
        let key = key
            .map(|text| MusicalKey::parse(text).ok_or_else(|| format!("invalid key: {text}")))
            .transpose()?;
        let start = self.project().signatures.bar_start(start_bar);
        let end = self.project().signatures.bar_start(after);
        let previous_section = self.project().sections.label_at(end).map(str::to_owned);
        let internal_sections = self
            .project()
            .sections
            .points()
            .iter()
            .filter(|point| point.tick > start && point.tick < end)
            .map(|point| point.tick)
            .collect::<Vec<_>>();
        let chart = chords
            .map(|text| {
                Chart::parse(text)
                    .filter(|chart| !chart.bars.is_empty())
                    .map(|chart| {
                        chart
                            .spelled_in(key.unwrap_or_else(|| self.project().harmony.key_at(start)))
                    })
                    .ok_or_else(|| format!("invalid Roman-numeral chord chart: {text}"))
            })
            .transpose()?;
        self.begin_transaction(crate::Edit::StampProgression);
        self.write_bounded_harmony(start_bar, bars, key, chart.as_ref(), clear_chords);
        if let Some(label) = section {
            for tick in internal_sections {
                self.remove_section(tick);
            }
            self.set_section(start, Some(label.trim().to_owned()));
            self.set_section(end, previous_section);
        }
        self.end_transaction();
        let chord_changes = self
            .project()
            .harmony
            .chords
            .points()
            .iter()
            .filter(|point| point.tick >= start && point.tick < end && point.chord.is_some())
            .count();
        Ok(serde_json::json!({"start_bar":start_bar,"bars":bars,"chart_bars":chart.as_ref().map(|chart| chart.bars.len()),"chord_changes":chord_changes,"key":self.project().harmony.key_at(start).to_text(),"section":self.project().sections.label_at(start),"unsaved":true,"notes_preserved":true,"usage":"inspect_project reads the current harmony and section maps. Each chords array entry is one bar; spaces inside an entry divide that bar."}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_bar_arrays_place_one_progression_across_bars_without_touching_notes() {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Existing").unwrap();
        let clip = session
            .add_midi_clip(track, "Existing", Ticks::ZERO, Ticks::QUARTER * 160)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let before = session.project().clone();
        let command: Command = serde_json::from_value(serde_json::json!({
            "action":"edit_harmony", "start_bar":25, "bars":8,
            "chords":["ii", "IV", "V", "V"]
        }))
        .expect("Each array entry explicitly addresses one bar");
        let result: serde_json::Value =
            serde_json::from_str(&session.agent_command(command).unwrap()).unwrap();
        for offset in 0..8 {
            let at = session.project().signatures.bar_start(25 + offset);
            assert_eq!(session.harmony().chords.change_at(at), Some(at));
            assert_eq!(
                session.harmony().chords.change_at(at + Ticks::QUARTER),
                Some(at)
            );
            assert_eq!(
                session.harmony().chord_at(at).unwrap().to_string(),
                ["Dm", "F", "G", "G"][offset as usize % 4]
            );
        }
        assert_eq!(result["chart_bars"], 4);
        assert_eq!(result["chord_changes"], 8);
        assert_eq!(session.project().tracks, before.tracks);
        let after = session.project().clone();
        session.undo();
        assert_eq!(session.project(), &before);
        session.redo();
        assert_eq!(session.project(), &after);
    }

    #[test]
    fn ambiguous_unbarred_progressions_are_rejected_before_any_edit() {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let before = session.project().clone();
        let error = session
            .agent_edit_harmony(
                1,
                8,
                Some("D major"),
                Some("I V vi IV"),
                false,
                Some("Verse"),
            )
            .unwrap_err();
        assert!(error.contains("one bar"), "{error}");
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
    }

    #[test]
    fn mixed_harmonic_rhythm_keeps_bar_and_within_bar_positions() {
        for chords in [
            serde_json::json!(["I", "IV V", "vi", "ii V"]),
            serde_json::json!("| I | IV V | vi | ii V |"),
        ] {
            let mut session =
                Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
            let command = serde_json::from_value(serde_json::json!({
                "action":"edit_harmony", "start_bar":1, "bars":4, "chords":chords,
            }))
            .unwrap();
            let result: serde_json::Value =
                serde_json::from_str(&session.agent_command(command).unwrap()).unwrap();
            assert_eq!(result["chart_bars"], 4);
            assert_eq!(result["chord_changes"], 6);
            for (beat, expected) in [
                (0, "C"),
                (4, "F"),
                (6, "G"),
                (8, "Am"),
                (12, "Dm"),
                (14, "G"),
            ] {
                let at = Ticks::QUARTER * beat;
                assert_eq!(session.harmony().chords.change_at(at), Some(at));
                assert_eq!(
                    session
                        .harmony()
                        .chord_at(at + Ticks::QUARTER)
                        .unwrap()
                        .to_string(),
                    expected
                );
            }
        }
    }

    #[test]
    fn empty_or_nested_bar_arrays_are_rejected_and_catalogues_still_work() {
        for chords in [
            serde_json::json!([]),
            serde_json::json!([" "]),
            serde_json::json!(["I | V"]),
            serde_json::json!(["@axis"]),
            serde_json::json!(vec!["I"; 4097]),
        ] {
            assert!(
                serde_json::from_value::<Command>(serde_json::json!({
                    "action":"edit_harmony", "start_bar":1, "bars":8, "chords":chords,
                }))
                .is_err()
            );
        }
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        session
            .agent_edit_harmony(1, 8, None, Some("@axis x2"), false, None)
            .unwrap();
        assert_eq!(
            session
                .harmony()
                .chords
                .points()
                .iter()
                .filter(|p| p.chord.is_some())
                .count(),
            8
        );
    }

    #[test]
    fn bounded_chords_restore_silence_after_the_range() {
        let mut s = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        s.agent_edit_harmony(2, 2, None, Some("| I | V |"), false, None)
            .unwrap();
        assert!(s.harmony().numeral_at(Ticks::ZERO).is_none());
        assert_eq!(
            s.harmony()
                .chord_at(Ticks::QUARTER * 8)
                .unwrap()
                .to_string(),
            "G"
        );
        assert!(s.harmony().numeral_at(Ticks::QUARTER * 12).is_none());
    }

    #[test]
    fn bar_addressed_harmony_ignores_the_pointer_grid() {
        let mut s = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        s.set_grid(Ticks::QUARTER * 8);
        s.agent_edit_harmony(2, 2, Some("D major"), Some("| I V vi | IV |"), false, None)
            .unwrap();
        let start = Ticks::QUARTER * 4;
        for (offset, chord) in [(0, "D"), (1280, "A"), (2560, "Bm"), (3840, "G")] {
            assert_eq!(
                s.harmony()
                    .chord_at(start + Ticks(offset))
                    .map(|c| c.to_string()),
                Some(chord.into())
            );
        }
        assert_eq!(s.harmony().key_at(start).to_text(), "D major");
        assert_eq!(s.harmony().key_at(Ticks::QUARTER * 12).to_text(), "C major");
        assert!(s.harmony().numeral_at(start - Ticks(1)).is_none());
        assert!(s.harmony().numeral_at(Ticks::QUARTER * 12).is_none());
    }

    #[test]
    fn a_bounded_key_replaces_internal_modulations() {
        let mut s = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        s.set_key(Ticks::QUARTER * 8, MusicalKey::parse("E major").unwrap());
        let before = s.project().clone();
        s.agent_edit_harmony(2, 3, Some("D major"), Some("| I |"), false, None)
            .unwrap();
        for bar in 2..5 {
            assert_eq!(
                s.harmony()
                    .chord_at(s.project().signatures.bar_start(bar))
                    .unwrap()
                    .to_string(),
                "D"
            );
        }
        assert_eq!(s.harmony().key_at(Ticks::QUARTER * 16).to_text(), "E major");
        let after = s.project().clone();
        s.undo();
        assert_eq!(s.project(), &before);
        s.redo();
        assert_eq!(s.project(), &after);
    }

    #[test]
    fn bounded_harmony_follows_meter_changes_and_preserves_outside_chords() {
        let mut s = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        s.set_signature_point(Ticks::QUARTER * 4, TimeSignature::new(7, 8));
        s.set_signature_point(Ticks(10560), TimeSignature::new(3, 4));
        s.stamp_progression(&Chart::parse("| vi |").unwrap(), Ticks::ZERO, 8);
        s.set_grid(Ticks::QUARTER * 8);
        let before = s.project().clone();
        s.agent_edit_harmony(2, 3, None, Some("| I V | IV | ii |"), false, None)
            .unwrap();
        for (tick, chord) in [
            (3839, "Am"),
            (3840, "C"),
            (5520, "G"),
            (7200, "F"),
            (10560, "Dm"),
            (13440, "Am"),
        ] {
            assert_eq!(
                s.harmony().chord_at(Ticks(tick)).unwrap().to_string(),
                chord
            );
        }
        s.undo();
        assert_eq!(s.project(), &before);
        s.agent_edit_harmony(2, 3, None, None, true, None).unwrap();
        assert!(s.harmony().numeral_at(Ticks(3840)).is_none());
        assert!(s.harmony().numeral_at(Ticks(13439)).is_none());
        assert_eq!(
            s.harmony().chord_at(Ticks(13440)).unwrap().to_string(),
            "Am"
        );
    }

    #[test]
    fn section_only_edits_bound_the_form_preserve_music_and_undo_together() {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        session.add_default_instrument_track("Existing").unwrap();
        let track = session.project().tracks[0].id;
        let clip = session
            .add_midi_clip(track, "Existing music", Ticks::ZERO, Ticks::QUARTER * 24)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::QUARTER * 8, Ticks::QUARTER))
            .unwrap();
        session.set_section(Ticks::ZERO, Some("Old verse".into()));
        session.set_section(Ticks::QUARTER * 8, Some("Old bridge".into()));
        let before = session.project().clone();
        let command: Command = serde_json::from_value(serde_json::json!({
            "action":"edit_harmony","start_bar":2,"bars":3,"section":"Chorus"
        }))
        .expect("section metadata must be callable without changing harmony");
        session.agent_command(command).unwrap();
        assert_eq!(
            session.project().sections.label_at(Ticks::QUARTER * 4),
            Some("Chorus")
        );
        assert_eq!(
            session.project().sections.label_at(Ticks::QUARTER * 8),
            Some("Chorus")
        );
        assert_eq!(
            session.project().sections.label_at(Ticks::QUARTER * 16),
            Some("Old bridge")
        );
        assert_eq!(
            session.project().sections.label_at(Ticks::ZERO),
            Some("Old verse")
        );
        assert_eq!(session.project().tracks, before.tracks);
        assert_eq!(session.project().harmony, before.harmony);
        assert!(session.is_dirty());
        let after = session.project().clone();
        session.undo();
        assert_eq!(session.project(), &before);
        session.redo();
        assert_eq!(session.project(), &after);
    }

    #[test]
    fn invalid_section_labels_or_charts_leave_metadata_and_history_untouched() {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let before = session.project().clone();
        for label in [" ", "bad\nlabel", &"長".repeat(100)] {
            assert!(
                session
                    .agent_edit_harmony(1, 4, None, None, false, Some(label))
                    .is_err()
            );
            assert_eq!(session.project(), &before);
        }
        assert!(
            session
                .agent_edit_harmony(1, 4, None, Some("invalid!"), false, Some("Chorus"))
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn harmony_writes_metadata_preserves_notes_and_is_one_undo_step() {
        let mut s = Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let track = s.add_default_instrument_track("Existing").unwrap();
        let clip = s
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 32)
            .unwrap();
        s.add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let before = s.project().clone();
        s.agent_edit_harmony(
            1,
            4,
            Some("D major"),
            Some("| I | IV | V | I |"),
            false,
            None,
        )
        .unwrap();
        assert_eq!(s.project().harmony.key_at(Ticks::ZERO).to_text(), "D major");
        for (bar, chord) in ["D", "G", "A", "D"].iter().enumerate() {
            assert_eq!(
                s.project()
                    .harmony
                    .chord_at(Ticks::QUARTER * (bar * 4) as i64)
                    .unwrap()
                    .to_string(),
                *chord
            );
        }
        assert_eq!(
            s.project().harmony.key_at(Ticks::QUARTER * 16),
            before.harmony.key_at(Ticks::QUARTER * 16)
        );
        assert_eq!(s.project().tracks, before.tracks);
        let after = s.project().clone();
        assert!(
            s.agent_edit_harmony(1, 4, Some("A minor"), Some("invalid chord!"), false, None)
                .is_err()
        );
        assert_eq!(s.project(), &after);
        s.undo();
        assert_eq!(s.project(), &before);
    }
}
