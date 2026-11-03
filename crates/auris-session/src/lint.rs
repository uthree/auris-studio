//! The music linter: problems a rule or a meter can find without anybody's taste.
//!
//! A code linter does not tell anybody their program is beautiful; it says *this line divides by
//! zero*. This is the same instrument pointed at a song. Every rule here answers a question with
//! one right answer — are twelve notes struck on the same tick, is a sustained note outside both
//! the written key and the written chord, does a track that is playing come out of the speakers
//! at all — and leaves the questions without one to the people and the agents who write music.
//!
//! # Two passes, two costs
//!
//! * [`lint_score`] reads the document's notes and its harmony annotation. It is cheap enough to
//!   run after every edit, and the desktop does exactly that.
//! * [`AudioLintJob`] renders the mix and each sounding track once and measures what came out:
//!   clipping, loudness, and tracks that hold notes but produce no signal. It is a `Send` job
//!   captured on the session thread and run anywhere else, because a render is seconds of work.
//!   The desktop runs it in the background only after an edit and only while nothing else is
//!   busy; an agent runs it only when it asks with `lint_project`'s `audio` flag.
//!
//! # Fixes
//!
//! A [`Diagnostic`] carries a [`Fix`] only when the repair is *obvious*: a duplicate note is
//! removed, a sustained out-of-key note moves to the nearest chord tone, a track whose fader is
//! all the way down comes back to 0 dB. A crowded chord or an over-busy bar has many good repairs
//! and none that is clearly right, so those carry no fix and the frontend offers to ask an agent
//! instead. [`crate::Session::apply_lint_fix`] applies one fix as one undo step.
//!
//! A fix names notes by clip and storage index, which is why every diagnostic is bound to the
//! revision it was found at: applying a fix found at another revision is refused rather than
//! guessed at.

mod audio;
mod fix;
mod score;

pub use audio::{AUDIO_LINT_RATE, AudioLintJob, AudioLintResult, AudioSummary};
pub use score::lint_score;

use auris_core::Project;
use auris_core::project::{ClipId, TrackId};
pub use auris_core::project::{LintScope, LintSuppression};
use auris_core::time::Ticks;
use serde::{Deserialize, Serialize};

/// How much a problem matters.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth a look; often deliberate.
    Info,
    /// Probably a mistake.
    Warning,
    /// Certainly heard as a fault: clipping, or a playing track with no sound.
    Error,
}

/// Which check found a problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// Too many notes begin on the same tick.
    CrowdedOnset,
    /// A bar holds more attacks than can be heard as separate notes.
    DenseNotes,
    /// Two sustained notes a semitone apart.
    HarshInterval,
    /// Seconds in the bass register, where they turn to mud.
    MuddyLowInterval,
    /// A sustained note in neither the written key nor the written chord.
    OutOfKey,
    /// A sustained scale note a semitone above a chord tone (an "avoid note").
    AvoidNote,
    /// Bars that sound chords with no chord written over them.
    MissingChord,
    /// Two notes of the same pitch on top of each other in one clip.
    DuplicateNote,
    /// A singer asked to sing two notes at once.
    SingerOverlap,
    /// A sung note with no lyric, which the voice sings as a stock vowel.
    MissingLyric,
    /// A note struck too softly to be heard.
    InaudibleVelocity,
    /// A note outside any instrument's or voice's usable range.
    ExtremePitch,
    /// The rendered mix reaches full scale.
    Clipping,
    /// The rendered mix is far louder than any delivery target.
    TooLoud,
    /// The rendered mix is far quieter than any delivery target.
    TooQuiet,
    /// A track plays notes but its rendered signal is silent.
    SilentTrack,
    /// A note alone on its track sounds off its written pitch, octaves aside.
    PitchMismatch,
    /// Notes that should have a pitch come out without one.
    Unpitched,
    /// The mix loses most of its level summed to mono: left and right cancel.
    PhaseCancellation,
}

impl Rule {
    /// Every rule, score rules first.
    pub const ALL: [Rule; 19] = [
        Rule::CrowdedOnset,
        Rule::DenseNotes,
        Rule::HarshInterval,
        Rule::MuddyLowInterval,
        Rule::OutOfKey,
        Rule::AvoidNote,
        Rule::MissingChord,
        Rule::DuplicateNote,
        Rule::SingerOverlap,
        Rule::MissingLyric,
        Rule::InaudibleVelocity,
        Rule::ExtremePitch,
        Rule::Clipping,
        Rule::TooLoud,
        Rule::TooQuiet,
        Rule::SilentTrack,
        Rule::PitchMismatch,
        Rule::Unpitched,
        Rule::PhaseCancellation,
    ];

    /// The stable identifier used on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Rule::CrowdedOnset => "crowded_onset",
            Rule::DenseNotes => "dense_notes",
            Rule::HarshInterval => "harsh_interval",
            Rule::MuddyLowInterval => "muddy_low_interval",
            Rule::OutOfKey => "out_of_key",
            Rule::AvoidNote => "avoid_note",
            Rule::MissingChord => "missing_chord",
            Rule::DuplicateNote => "duplicate_note",
            Rule::SingerOverlap => "singer_overlap",
            Rule::MissingLyric => "missing_lyric",
            Rule::InaudibleVelocity => "inaudible_velocity",
            Rule::ExtremePitch => "extreme_pitch",
            Rule::Clipping => "clipping",
            Rule::TooLoud => "too_loud",
            Rule::TooQuiet => "too_quiet",
            Rule::SilentTrack => "silent_track",
            Rule::PitchMismatch => "pitch_mismatch",
            Rule::Unpitched => "unpitched",
            Rule::PhaseCancellation => "phase_cancellation",
        }
    }

    /// Whether the rule needs a render to be checked.
    pub fn needs_audio(self) -> bool {
        matches!(
            self,
            Rule::Clipping
                | Rule::TooLoud
                | Rule::TooQuiet
                | Rule::SilentTrack
                | Rule::PitchMismatch
                | Rule::Unpitched
                | Rule::PhaseCancellation
        )
    }
}

/// One stored note, by clip and index into [`MidiClip::notes`](auris_core::project::MidiClip::notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NoteRef {
    /// The clip holding the note.
    pub clip: ClipId,
    /// The note's index in the clip's stored notes.
    pub index: usize,
}

/// The obvious repair of one problem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fix {
    /// Delete these notes of one clip.
    RemoveNotes {
        /// The clip.
        clip: ClipId,
        /// Storage indices to delete.
        indices: Vec<usize>,
    },
    /// Shorten notes so they end where the next note of the same pitch begins.
    TrimNotes {
        /// The clip.
        clip: ClipId,
        /// `(index, new length in ticks)` pairs.
        lengths: Vec<(usize, i64)>,
    },
    /// Move notes to new pitches.
    SetPitches {
        /// The clip.
        clip: ClipId,
        /// `(index, new MIDI pitch)` pairs.
        pitches: Vec<(usize, u8)>,
    },
    /// Give notes an audible velocity.
    SetVelocity {
        /// The clip.
        clip: ClipId,
        /// Storage indices.
        indices: Vec<usize>,
        /// The new velocity, 0..1.
        velocity: f32,
    },
    /// Bring a track's fader back to 0 dB.
    ResetFader {
        /// The track.
        track: TrackId,
    },
    /// Write chords where none are written: one per bar, then nothing again from `end`.
    SetChords {
        /// `(bar start, chord)` pairs, as numerals in the key at that tick.
        chords: Vec<(Ticks, auris_core::theory::numeral::Numeral)>,
        /// Where the run of bars ends.
        end: Ticks,
    },
    /// Run [`Session::normalize_mix`](crate::Session::normalize_mix) toward the standard target.
    NormalizeMix,
}

impl Fix {
    /// Whether applying the fix renders audio, and so takes noticeable time.
    pub fn renders(&self) -> bool {
        matches!(self, Fix::NormalizeMix)
    }
}

/// One problem, where it is, and what would repair it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The check that found it.
    pub rule: Rule,
    /// How much it matters.
    pub severity: Severity,
    /// The track it is on, or `None` for the whole mix.
    pub track: Option<TrackId>,
    /// Where it starts on the timeline.
    pub start: Ticks,
    /// Where it ends on the timeline; at least one tick after `start`.
    pub end: Ticks,
    /// The stored notes involved, when the problem is in notes.
    pub notes: Vec<NoteRef>,
    /// MIDI pitches involved, for describing the problem.
    pub pitches: Vec<u8>,
    /// The measured quantity: a note count, attacks per beat, LUFS, dBFS.
    pub value: Option<f32>,
    /// The threshold the measurement crossed.
    pub limit: Option<f32>,
    /// The obvious repair, when there is one.
    pub fix: Option<Fix>,
}

impl Diagnostic {
    pub(crate) fn new(rule: Rule, severity: Severity, start: Ticks, end: Ticks) -> Self {
        Self {
            rule,
            severity,
            track: None,
            start,
            end: end.max(start + Ticks(1)),
            notes: Vec::new(),
            pitches: Vec::new(),
            value: None,
            limit: None,
            fix: None,
        }
    }

    /// A stable identity: the same problem in the same place has the same id across runs.
    ///
    /// The frontend keys its hover and menus on it, and the agent names fixes with it.
    pub fn id(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.rule.hash(&mut hasher);
        self.track.map(|track| track.0).hash(&mut hasher);
        self.start.raw().hash(&mut hasher);
        self.end.raw().hash(&mut hasher);
        self.notes.hash(&mut hasher);
        self.pitches.hash(&mut hasher);
        format!(
            "{}-{:012x}",
            self.rule.name(),
            hasher.finish() & 0xffff_ffff_ffff
        )
    }

    /// Whether the problem overlaps `from..to`.
    pub fn overlaps(&self, from: Ticks, to: Ticks) -> bool {
        self.start < to && self.end > from
    }

    /// An English account of the problem, for a model and for logs.
    ///
    /// The window says these things through `auris-i18n`, from the same fields.
    pub fn describe(&self) -> String {
        let pitches = self
            .pitches
            .iter()
            .map(|pitch| auris_core::theory::pitch::midi_name(*pitch as i32))
            .collect::<Vec<_>>()
            .join(", ");
        let value = self.value.unwrap_or_default();
        let limit = self.limit.unwrap_or_default();
        match self.rule {
            Rule::CrowdedOnset => format!(
                "{value:.0} notes start at the same moment (more than {limit:.0}); the attack smears and the voicing is unplayable"
            ),
            Rule::DenseNotes => format!(
                "{value:.1} attacks per beat in this bar (limit {limit:.0}); notes this close cannot be heard separately"
            ),
            Rule::HarshInterval => format!(
                "{pitches} sustain together a semitone apart; a held minor second sounds like a wrong note"
            ),
            Rule::MuddyLowInterval => format!(
                "{pitches} sustain together a second apart in the bass register, which sounds muddy"
            ),
            Rule::OutOfKey => format!(
                "{pitches} is held but belongs to neither the written key nor the written chord"
            ),
            Rule::AvoidNote => format!(
                "{pitches} is held a semitone above a chord tone (an avoid note against the written chord)"
            ),
            Rule::MissingChord => {
                let chords = match &self.fix {
                    Some(Fix::SetChords { chords, .. }) => format!(
                        "; the notes spell {}",
                        chords
                            .iter()
                            .map(|(_, numeral)| numeral.to_text())
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ),
                    _ => String::new(),
                };
                format!("{value:.0} bar(s) sound chords but no chord is written over them{chords}")
            }
            Rule::DuplicateNote => {
                format!("{pitches} is written twice on top of itself in one clip")
            }
            Rule::SingerOverlap => {
                format!("{pitches} overlap on a singer track; a voice sings one note at a time")
            }
            Rule::MissingLyric if self.pitches.is_empty() => format!(
                "none of this clip's {value:.0} sung notes has a lyric; each is sung as a stock vowel"
            ),
            Rule::MissingLyric => {
                format!("{pitches} has no lyric and is sung as a stock vowel")
            }
            Rule::PitchMismatch => format!(
                "{pitches} sounds {value:+.0} cents from its written pitch (octaves aside), beyond the {limit:.0} cent tolerance"
            ),
            Rule::Unpitched if self.pitches.is_empty() => format!(
                "{value:.0}% of this track's single notes render with no detectable pitch; fine for percussion, a fault for a melody"
            ),
            Rule::Unpitched => {
                format!("{pitches} is sung with no detectable pitch")
            }
            Rule::PhaseCancellation => format!(
                "summed to mono the mix loses {value:.0} dB here: left and right cancel, and a phone speaker plays a hole"
            ),
            Rule::InaudibleVelocity => {
                format!("{pitches} has velocity {value:.2}, too soft to be heard")
            }
            Rule::ExtremePitch => {
                format!("{pitches} lies outside the usable range of instruments or voices")
            }
            Rule::Clipping => {
                format!("the mix reaches full scale ({value:.1} dBFS sample peak) and distorts")
            }
            Rule::TooLoud => {
                format!("the mix measures {value:.1} LUFS integrated, louder than {limit:.0} LUFS")
            }
            Rule::TooQuiet => {
                format!("the mix measures {value:.1} LUFS integrated, quieter than {limit:.0} LUFS")
            }
            Rule::SilentTrack => {
                format!(
                    "the track plays notes here but its rendered signal stays below {limit:.0} dBFS"
                )
            }
        }
    }

    /// An English account of the fix, for a model.
    pub fn describe_fix(&self) -> Option<&'static str> {
        Some(match self.fix.as_ref()? {
            Fix::RemoveNotes { .. } => "remove the duplicate note",
            Fix::TrimNotes { .. } => "shorten the earlier note to end where the repeat begins",
            Fix::SetPitches { .. } => "move the note to the nearest chord tone",
            Fix::SetVelocity { .. } => "raise the velocity to the default",
            Fix::ResetFader { .. } => "bring the track's fader back to 0 dB",
            Fix::SetChords { .. } => "write the chords the notes spell, one per bar",
            Fix::NormalizeMix => "normalize the mix toward -14 LUFS",
        })
    }
}

/// Whether the project has asked not to hear about this finding.
pub fn is_suppressed(project: &Project, diagnostic: &Diagnostic) -> bool {
    project
        .lint_suppressions
        .iter()
        .any(|suppression| suppresses(project, suppression, diagnostic))
}

/// Whether one suppression covers one finding.
pub fn suppresses(
    project: &Project,
    suppression: &LintSuppression,
    diagnostic: &Diagnostic,
) -> bool {
    if suppression
        .rule
        .as_deref()
        .is_some_and(|rule| rule != diagnostic.rule.name())
    {
        return false;
    }
    match &suppression.scope {
        LintScope::Song => true,
        LintScope::Track { track } => {
            diagnostic.track == Some(*track)
                || (!diagnostic.notes.is_empty()
                    && diagnostic.notes.iter().all(|note| {
                        project
                            .midi_clip(note.clip)
                            .is_some_and(|(owner, _)| owner == *track)
                    }))
        }
        LintScope::Clip { clip } => diagnostic.notes.iter().any(|note| note.clip == *clip),
        LintScope::Section { label } => project
            .sections
            .section_at(diagnostic.start)
            .is_some_and(|(found, _)| found == label),
    }
}

/// Every problem found in one document at one revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LintReport {
    /// The session revision the score was read at.
    pub revision: u64,
    /// How many findings the project's suppressions hid.
    pub suppressed: usize,
    /// Problems, ordered by position and then severity.
    pub diagnostics: Vec<Diagnostic>,
    /// Measurements from the last audio pass, when one ran at this revision.
    pub audio: Option<AudioSummary>,
}

impl LintReport {
    /// The problem with this id.
    pub fn find(&self, id: &str) -> Option<&Diagnostic> {
        self.diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id() == id)
    }

    /// How many problems there are of each severity: `(errors, warnings, infos)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        self.diagnostics
            .iter()
            .fold((0, 0, 0), |(e, w, i), d| match d.severity {
                Severity::Error => (e + 1, w, i),
                Severity::Warning => (e, w + 1, i),
                Severity::Info => (e, w, i + 1),
            })
    }

    /// Adds the audio pass's findings, replacing any older ones, less what `project`
    /// suppresses.
    pub fn merge_audio(&mut self, audio: AudioLintResult, project: &Project) {
        self.diagnostics.retain(|d| !d.rule.needs_audio());
        for diagnostic in audio.diagnostics {
            if is_suppressed(project, &diagnostic) {
                self.suppressed += 1;
            } else {
                self.diagnostics.push(diagnostic);
            }
        }
        sort(&mut self.diagnostics);
        self.audio = Some(audio.summary);
    }
}

/// Position first, then the worse problem first, then the rule, so output is deterministic.
pub(crate) fn sort(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(b.severity.cmp(&a.severity))
            .then(a.rule.name().cmp(b.rule.name()))
            .then(a.track.map(|t| t.0).cmp(&b.track.map(|t| t.0)))
            .then(a.notes.cmp(&b.notes))
    });
}

impl crate::Session {
    /// Lints the open document's score at the current revision, less what it suppresses.
    pub fn lint_score(&self) -> LintReport {
        let project = self.project();
        let (suppressed, diagnostics): (Vec<_>, Vec<_>) = lint_score(project)
            .into_iter()
            .partition(|diagnostic| is_suppressed(project, diagnostic));
        LintReport {
            revision: self.revision(),
            suppressed: suppressed.len(),
            diagnostics,
            audio: None,
        }
    }

    /// Stops reporting what `suppression` covers, as one undo step. Adding one twice is a no-op.
    pub fn suppress_lint(&mut self, suppression: LintSuppression) {
        if self
            .project()
            .lint_suppressions
            .iter()
            .any(|kept| kept.same_rule(&suppression))
        {
            return;
        }
        let before = self.revision();
        self.edit_suppressions().push(suppression);
        self.carry_audio_lint(before);
    }

    /// Reports what `suppression` covered again, as one undo step.
    pub fn unsuppress_lint(&mut self, suppression: &LintSuppression) {
        if !self
            .project()
            .lint_suppressions
            .iter()
            .any(|kept| kept.same_rule(suppression))
        {
            return;
        }
        let before = self.revision();
        self.edit_suppressions()
            .retain(|kept| !kept.same_rule(suppression));
        self.carry_audio_lint(before);
    }

    /// Writes down why an ignore rule exists, as one undo step; empty text clears it.
    ///
    /// Whether the rule was there to annotate.
    pub fn set_lint_suppression_reason(
        &mut self,
        suppression: &LintSuppression,
        reason: &str,
    ) -> bool {
        let reason = auris_core::Comment::clean_text(reason).unwrap_or_default();
        let Some(index) = self
            .project()
            .lint_suppressions
            .iter()
            .position(|kept| kept.same_rule(suppression))
        else {
            return false;
        };
        if self.project().lint_suppressions[index].reason == reason {
            return true;
        }
        let before = self.revision();
        self.edit_suppressions()[index].reason = reason;
        self.carry_audio_lint(before);
        true
    }

    /// The suppressions changed and the sound did not, so an audio pass of the revision before
    /// still describes this one. Without this, ignoring a problem would hide every audio finding
    /// until the song had been rendered again.
    fn carry_audio_lint(&mut self, before: u64) {
        let now = self.revision();
        if let Some(audio) = self.lint_audio.as_mut()
            && audio.revision == before
        {
            audio.revision = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::project::Note;
    use auris_core::time::Ticks;

    /// Two tracks, each with a clip holding one duplicated note, in a song whose second bar is
    /// labelled "Chorus".
    fn session() -> (
        crate::Session,
        [auris_core::project::TrackId; 2],
        [auris_core::project::ClipId; 2],
    ) {
        let mut session = crate::Session::new(SessionOptions::headless()).unwrap();
        let mut tracks = Vec::new();
        let mut clips = Vec::new();
        for (name, start) in [("Keys", Ticks::ZERO), ("Pad", Ticks(3840))] {
            let track = session.add_default_instrument_track(name).unwrap();
            let clip = session
                .add_midi_clip(track, "Clip", start, Ticks(3840))
                .unwrap();
            session
                .replace_notes(
                    clip,
                    vec![
                        Note::new(60, Ticks::ZERO, Ticks(480)),
                        Note::new(60, Ticks::ZERO, Ticks(480)),
                    ],
                )
                .unwrap();
            tracks.push(track);
            clips.push(clip);
        }
        session.set_section(Ticks(3840), Some("Chorus".into()));
        (session, [tracks[0], tracks[1]], [clips[0], clips[1]])
    }

    fn duplicates(session: &crate::Session) -> usize {
        session
            .lint_score()
            .diagnostics
            .iter()
            .filter(|d| d.rule == Rule::DuplicateNote)
            .count()
    }

    #[test]
    fn each_scope_hides_only_what_it_covers_and_undo_brings_it_back() {
        let (mut session, tracks, clips) = session();
        assert_eq!(duplicates(&session), 2);
        let rule = Some(Rule::DuplicateNote.name().to_string());
        for (scope, left) in [
            (LintScope::Track { track: tracks[0] }, 1),
            (LintScope::Clip { clip: clips[1] }, 1),
            (
                LintScope::Section {
                    label: "Chorus".into(),
                },
                1,
            ),
            (LintScope::Song, 0),
        ] {
            let suppression = LintSuppression {
                rule: rule.clone(),
                scope,
                reason: String::new(),
            };
            session.suppress_lint(suppression.clone());
            let report = session.lint_score();
            assert_eq!(duplicates(&session), left, "{suppression:?}");
            assert_eq!(report.suppressed, 2 - left);
            session.unsuppress_lint(&suppression);
            assert_eq!(duplicates(&session), 2);
        }

        // Another rule's suppression covers nothing here.
        session.suppress_lint(LintSuppression {
            rule: Some(Rule::HarshInterval.name().into()),
            scope: LintScope::Song,
            reason: String::new(),
        });
        assert_eq!(duplicates(&session), 2);

        // Every rule on a track, then undone as one step.
        session.suppress_lint(LintSuppression {
            rule: None,
            scope: LintScope::Track { track: tracks[1] },
            reason: String::new(),
        });
        assert_eq!(duplicates(&session), 1);
        session.undo().unwrap();
        assert_eq!(duplicates(&session), 2);
    }

    #[test]
    fn suppressions_travel_with_the_saved_document() {
        let (mut session, tracks, _) = session();
        let suppression = LintSuppression {
            rule: None,
            scope: LintScope::Track { track: tracks[0] },
            reason: String::new(),
        };
        session.suppress_lint(suppression.clone());
        session.suppress_lint(suppression.clone());
        assert_eq!(session.project().lint_suppressions.len(), 1);
        let json = serde_json::to_string(session.project()).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lint_suppressions, vec![suppression]);
    }

    #[test]
    fn a_reason_annotates_the_rule_without_making_another() {
        let (mut session, tracks, _) = session();
        let rule = LintSuppression::new(None, LintScope::Track { track: tracks[0] });
        session.suppress_lint(rule.clone());
        assert!(session.set_lint_suppression_reason(&rule, "  doubled on purpose  "));
        let mut retyped = rule.clone();
        retyped.reason = "something else".into();
        session.suppress_lint(retyped.clone());
        let kept = &session.project().lint_suppressions;
        assert_eq!(
            kept.len(),
            1,
            "the same rule with another reason is not a second rule"
        );
        assert_eq!(kept[0].reason, "doubled on purpose");
        let json = serde_json::to_string(session.project()).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lint_suppressions[0].reason, "doubled on purpose");
        assert!(session.undo().is_some());
        assert!(session.project().lint_suppressions[0].reason.is_empty());
        session.unsuppress_lint(&retyped);
        assert!(
            session.project().lint_suppressions.is_empty(),
            "removal ignores the reason"
        );
    }

    #[test]
    fn writing_the_obvious_chords_is_one_undo_step_and_stops_at_the_run() {
        let mut session = crate::Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let clip = session
            .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks(3840 * 3))
            .unwrap();
        let mut notes: Vec<Note> = [57, 60, 64]
            .into_iter()
            .map(|p| Note::new(p, Ticks::ZERO, Ticks(3840)))
            .collect();
        notes.push(Note::new(72, Ticks(3840), Ticks(3840)));
        session.replace_notes(clip, notes).unwrap();
        let report = session.lint_score();
        let missing = report
            .diagnostics
            .iter()
            .find(|d| d.rule == Rule::MissingChord)
            .unwrap();
        session.apply_lint_fix(missing, report.revision).unwrap();
        let harmony = &session.project().harmony;
        assert_eq!(
            harmony
                .numeral_at(Ticks::ZERO)
                .map(|n| n.name_in(harmony.key_at(Ticks::ZERO)))
                .as_deref(),
            Some("Am")
        );
        assert_eq!(harmony.numeral_at(Ticks(3840)), None);
        assert!(
            !session
                .lint_score()
                .diagnostics
                .iter()
                .any(|d| d.rule == Rule::MissingChord)
        );
        session.undo().unwrap();
        assert!(session.project().harmony.chords.is_empty());
    }
}
