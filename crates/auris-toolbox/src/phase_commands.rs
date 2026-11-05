//! The chat's composition commands — `/compose`, `/draft`, `/mix`, `/review` — as the
//! assignment a model reads.
//!
//! A command names where a production starts and where it stops; it does not say how a phase
//! is worked. That is [`crate::composition_workflows::instructions`], which `enter_phase`
//! returns, so each assignment here sends the model through `enter_phase` rather than carrying
//! a second copy of the phase text. What a phase keeps fixed is the host's to enforce
//! (`auris_session::phase::Lock`); `/review` is kept from editing by the turn's policy, which a
//! frontend sets from [`PhaseCommand::read_only`].

use auris_session::production_record::Phase;

/// One composition command typed in a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseCommand {
    /// The whole song in one go, through score, arrangement and mix.
    Compose,
    /// A demo: the score phase only, its foreground over one plain chord part.
    Draft,
    /// The mix phase on the open song, every note and sound kept.
    Mix,
    /// Read the song, report its problems and a plan, change nothing.
    Review,
}

impl PhaseCommand {
    /// Every command, in the order a help list shows them.
    pub const ALL: [Self; 4] = [Self::Compose, Self::Draft, Self::Mix, Self::Review];

    /// The command as typed, slash included.
    pub fn name(self) -> &'static str {
        match self {
            Self::Compose => "/compose",
            Self::Draft => "/draft",
            Self::Mix => "/mix",
            Self::Review => "/review",
        }
    }

    /// Whether the command means nothing without the user's brief after it.
    pub fn needs_instruction(self) -> bool {
        matches!(self, Self::Compose | Self::Draft)
    }

    /// Whether the turn must not edit: the frontend runs it under plan mode.
    pub fn read_only(self) -> bool {
        matches!(self, Self::Review)
    }

    /// The phase the command works in, when it enters one.
    pub fn phase(self) -> Option<Phase> {
        match self {
            Self::Compose | Self::Draft => Some(Phase::Score),
            Self::Mix => Some(Phase::Mix),
            Self::Review => None,
        }
    }

    /// Splits typed text into a command and the instruction after it.
    ///
    /// `None` for anything else, including a command glued to more letters (`/mixer`), so a
    /// message that merely starts with a slash still reaches the model as written.
    pub fn parse(text: &str) -> Option<(Self, &str)> {
        let text = text.trim();
        let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let command = Self::ALL.into_iter().find(|c| c.name() == head)?;
        Some((command, rest.trim()))
    }

    /// What the model is asked to do, the user's instruction quoted at the end.
    pub fn assignment(self, instruction: &str) -> String {
        let brief = match instruction.trim() {
            "" => "The user added no further direction.".to_string(),
            text => format!("The user's direction:\n{text}"),
        };
        let task = match self {
            Self::Compose => COMPOSE,
            Self::Draft => DRAFT,
            Self::Mix => MIX,
            Self::Review => REVIEW,
        };
        format!("[{} command] {task}\n\n{brief}", self.name())
    }
}

const COMPOSE: &str = "The user asked for the whole song in one go. Work through all three phases in this turn: enter_phase score, then arrangement, then mix, following the phase_instructions each one returns. The score phase's stop to ask whether to arrange does not apply: go on to the arrangement once the score is complete, and to the mix once the arrangement is. Finish with the mix delivered as the mix phase describes.";

const DRAFT: &str = "The user asked for a demo: the score phase only. enter_phase score and deliver what its phase_instructions describe: the form, harmony and lyrics, the foreground melody (sung, for a vocal song) and one plain chord guide part under it, with roles registered with production_record set_roles. Then stop. Do not enter the arrangement phase, choose final sounds, write other parts or mix. End by asking with offer_choices whether to arrange the demo or revise its melody, lyrics or chords.";

const MIX: &str = "Mix the open song and change nothing else. Read inspect_project and production_record read first. When no roles are registered, register them from the song as it stands with set_roles: its melody or lead vocal as the foreground, or no_foreground with a reason. Then enter_phase mix, with this request as the reason, and follow the phase_instructions it returns. Every note, sound and performance stays as it is: do not enter an earlier phase in this turn. When a problem can only be fixed there, report it with where it is and which phase owns it.";

const REVIEW: &str = "Review the open song and change nothing: this turn runs in plan mode, and the host refuses every edit. Read inspect_project and production_record read, then lint_project, following offset until every finding is read, and again with audio true when the song has tracks that sound. Read the score behind the findings: read_notes for the foreground and the harmony, melody_shape for the lead and arrangement_shape. Measure representative sections, transitions and the ending with inspect_audio, which is measurement, not listening. When listen is among your tools, this review asks for listening: hear those excerpts with listen and judge them by ear. Otherwise say that nothing was listened to. Judge every finding against what the song is trying to do; a finding may be deliberate. Then present, in the user's language: the problems found, each with where it is (bars and tracks), why it matters and the phase that owns its fix; the findings you judge to be false positives, each with its id and why, proposed as ignore rules; and a plan that fixes the rest in phase order (score, arrangement, mix). Ask with offer_choices which of it to carry out. An ignore rule is added only after the user agrees, in a later turn, with ignore_lint, which asks them to confirm.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_parses_to_a_command_and_its_instruction() {
        assert_eq!(
            PhaseCommand::parse("/compose  a song about rain "),
            Some((PhaseCommand::Compose, "a song about rain"))
        );
        assert_eq!(
            PhaseCommand::parse("/draft\n雨の歌"),
            Some((PhaseCommand::Draft, "雨の歌"))
        );
        assert_eq!(PhaseCommand::parse("/mix"), Some((PhaseCommand::Mix, "")));
        assert_eq!(
            PhaseCommand::parse("/review"),
            Some((PhaseCommand::Review, ""))
        );
        assert_eq!(PhaseCommand::parse("/mixer"), None);
        assert_eq!(PhaseCommand::parse("write a /compose"), None);
        assert_eq!(PhaseCommand::parse("/mode plan"), None);
    }

    #[test]
    fn each_assignment_quotes_the_user_and_stops_where_its_command_does() {
        for command in PhaseCommand::ALL {
            let text = command.assignment("ballad in D");
            assert!(text.starts_with(&format!("[{} command]", command.name())));
            assert!(
                text.ends_with("The user's direction:\nballad in D"),
                "{text}"
            );
            assert!(
                command
                    .assignment("  ")
                    .ends_with("The user added no further direction."),
            );
        }
        let compose = PhaseCommand::Compose.assignment("");
        for phase in ["enter_phase score", "arrangement", "mix"] {
            assert!(compose.contains(phase), "{phase}");
        }
        let draft = PhaseCommand::Draft.assignment("");
        assert!(draft.contains("Do not enter the arrangement phase"));
        assert!(draft.contains("set_roles"));
        let mix = PhaseCommand::Mix.assignment("");
        assert!(mix.contains("enter_phase mix"));
        assert!(mix.contains("do not enter an earlier phase"));
        let review = PhaseCommand::Review.assignment("");
        for needle in ["lint_project", "inspect_audio", "listen", "ignore_lint"] {
            assert!(review.contains(needle), "{needle}");
        }
    }

    #[test]
    fn only_review_is_read_only_and_only_the_writers_need_a_brief() {
        let read_only: Vec<_> = PhaseCommand::ALL
            .into_iter()
            .filter(|c| c.read_only())
            .collect();
        assert_eq!(read_only, [PhaseCommand::Review]);
        let briefed: Vec<_> = PhaseCommand::ALL
            .into_iter()
            .filter(|c| c.needs_instruction())
            .collect();
        assert_eq!(briefed, [PhaseCommand::Compose, PhaseCommand::Draft]);
    }

    #[test]
    fn assignments_send_the_model_to_enter_phase_rather_than_copy_it() {
        for command in PhaseCommand::ALL {
            let Some(phase) = command.phase() else {
                continue;
            };
            let instructions = crate::composition_workflows::instructions(phase, None);
            let text = command.assignment("");
            for sentence in instructions.split(". ").filter(|s| s.len() > 40) {
                assert!(!text.contains(sentence), "{command:?} copies {sentence:?}");
            }
            assert!(text.contains("enter_phase"), "{command:?}");
        }
    }
}
