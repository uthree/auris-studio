//! A question the agent asks with fixed answers, drawn by a frontend as a card.
//!
//! Both conversations use it: the editor's agent offers one through the `offer_choices` tool,
//! and the song workshop's director returns one beside its reply. Either way the card does not
//! wait inside a model turn. The agent ends its turn, the person picks, and the pick goes back as
//! the person's next message, written by [`ChoiceCard::answer`] — so a card that is ignored costs
//! nothing, and typing a different reply is always still possible.

use serde::{Deserialize, Serialize};

/// Most options one card may carry; more than this is a list to read, not a choice to make.
pub const MAX_OPTIONS: usize = 8;

/// One answer the person can pick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    /// Short answer in the user's language, at most 80 characters.
    pub label: String,
    /// Optional one-line consequence or audible difference, at most 200 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A question with two to eight fixed answers, exclusive unless `multiple` is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceCard {
    /// The question in the user's language, at most 200 characters.
    pub question: String,
    /// 2..8 distinct options in display order.
    #[schemars(length(min = 2, max = 8))]
    pub options: Vec<ChoiceOption>,
    /// Allow picking several options (checkboxes) instead of exactly one.
    #[serde(default)]
    pub multiple: bool,
}

impl ChoiceCard {
    /// Reject a card a person could not answer sensibly: too few or too many options, empty or
    /// oversized text, or two options with the same label (which the answer could not tell apart).
    pub fn validate(&self) -> Result<(), String> {
        let blank_or_long =
            |text: &str, limit: usize| text.trim().is_empty() || text.chars().count() > limit;
        if blank_or_long(&self.question, 200) {
            return Err("question must be 1..200 characters".into());
        }
        if !(2..=MAX_OPTIONS).contains(&self.options.len()) {
            return Err(format!("options must hold 2..{MAX_OPTIONS} entries"));
        }
        for (index, option) in self.options.iter().enumerate() {
            if blank_or_long(&option.label, 80) {
                return Err(format!("options[{index}].label must be 1..80 characters"));
            }
            if option
                .detail
                .as_deref()
                .is_some_and(|detail| detail.chars().count() > 200)
            {
                return Err(format!(
                    "options[{index}].detail is limited to 200 characters"
                ));
            }
            if self.options[..index]
                .iter()
                .any(|earlier| earlier.label.trim() == option.label.trim())
            {
                return Err(format!("options[{index}].label repeats an earlier option"));
            }
        }
        Ok(())
    }

    /// Whether `picked` is an answer this card accepts: one option, or for a multiple-choice
    /// card at least one, every index in range and none twice.
    pub fn accepts(&self, picked: &[usize]) -> bool {
        let in_range = picked.iter().all(|&index| index < self.options.len());
        let distinct = picked
            .iter()
            .enumerate()
            .all(|(at, index)| !picked[..at].contains(index));
        in_range
            && distinct
            && if self.multiple {
                !picked.is_empty()
            } else {
                picked.len() == 1
            }
    }

    /// The person's next message for `picked`, in display order: the labels themselves, one per
    /// line, so the transcript reads as what was chosen and the model sees the exact labels it
    /// offered. `None` when [`ChoiceCard::accepts`] would refuse the pick.
    pub fn answer(&self, picked: &[usize]) -> Option<String> {
        if !self.accepts(picked) {
            return None;
        }
        let mut picked = picked.to_vec();
        picked.sort_unstable();
        let labels = picked.iter().map(|&index| self.options[index].label.trim());
        Some(if self.multiple {
            labels
                .map(|label| format!("- {label}"))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            labels.collect::<String>()
        })
    }

    /// The options a reply picked, if the reply is exactly what [`ChoiceCard::answer`] writes for
    /// some pick; empty otherwise. This is how a saved conversation shows its answered cards
    /// without storing the pick twice.
    pub fn picked_by(&self, reply: &str) -> Vec<usize> {
        let reply = reply.trim();
        let labels: Vec<&str> = if self.multiple {
            reply
                .lines()
                .map(|line| line.trim().strip_prefix("- ").unwrap_or("\u{0}").trim())
                .collect()
        } else {
            vec![reply]
        };
        let picked: Vec<usize> = labels
            .iter()
            .filter_map(|label| {
                self.options
                    .iter()
                    .position(|option| option.label.trim() == *label)
            })
            .collect();
        if picked.len() == labels.len() && self.answer(&picked).as_deref() == Some(reply) {
            picked
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(multiple: bool) -> ChoiceCard {
        ChoiceCard {
            question: "Which instrument leads?".into(),
            options: ["Piano", "Strings", "Synth"]
                .into_iter()
                .map(|label| ChoiceOption {
                    label: label.into(),
                    detail: None,
                })
                .collect(),
            multiple,
        }
    }

    #[test]
    fn a_single_choice_answers_with_exactly_one_label() {
        let card = card(false);
        assert_eq!(card.answer(&[1]).as_deref(), Some("Strings"));
        assert_eq!(card.answer(&[]), None);
        assert_eq!(card.answer(&[0, 1]), None);
        assert_eq!(card.answer(&[3]), None);
    }

    #[test]
    fn a_multiple_choice_answers_in_display_order_whatever_the_click_order() {
        let card = card(true);
        assert_eq!(card.answer(&[2, 0]).as_deref(), Some("- Piano\n- Synth"));
        assert_eq!(card.answer(&[]), None);
        assert_eq!(card.answer(&[1, 1]), None);
    }

    #[test]
    fn a_written_answer_is_read_back_as_the_pick_it_came_from() {
        let single = card(false);
        assert_eq!(single.picked_by("Strings"), vec![1]);
        assert!(single.picked_by("Strings, please").is_empty());
        let multiple = card(true);
        let answer = multiple.answer(&[0, 2]).unwrap();
        assert_eq!(multiple.picked_by(&answer), vec![0, 2]);
        assert!(multiple.picked_by("- Piano\nand drums").is_empty());
    }

    #[test]
    fn unanswerable_cards_are_refused() {
        let mut short = card(false);
        short.options.truncate(1);
        assert!(short.validate().is_err());
        let mut repeated = card(false);
        repeated.options[2].label = " Piano ".into();
        assert!(repeated.validate().unwrap_err().contains("repeats"));
        let mut blank = card(false);
        blank.question = "  ".into();
        assert!(blank.validate().is_err());
        let mut many = card(true);
        many.options = (0..9)
            .map(|index| ChoiceOption {
                label: format!("Option {index}"),
                detail: None,
            })
            .collect();
        assert!(many.validate().is_err());
        assert!(card(true).validate().is_ok());
    }
}
