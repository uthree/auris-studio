//! The rename sheet, and the platform input plumbing behind it.
//!
//! gpui hands typed text to whichever view is registered as the window's input handler, so
//! [`AurisApp`] implements [`gpui::EntityInputHandler`] and forwards to the open prompt's
//! [`TextField`]. Registering happens inside the field's paint, which is the only place gpui
//! allows it and conveniently also the only time a prompt is on screen.

use std::ops::Range;

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    Bounds, Context, ElementInputHandler, IntoElement, MouseButton, MouseDownEvent, Pixels,
    SharedString, Window, canvas, div, point, prelude::*, px, size,
};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{ButtonStyle, button};

/// What a prompt is editing.
///
/// A key and a chord are typed rather than picked from a list because there is no list worth
/// showing: twelve tonics times thirteen scales is a hundred and fifty-six menu rows, and
/// [`MusicalKey::parse`] already reads `Bb minor` — the thing a musician would have written down
/// anyway. [`Numeral::parse`] does the same for `bVII7`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PromptTarget {
    /// A track's name.
    Track(TrackId),
    /// A clip's name.
    Clip(ClipId),
    /// The key in force from a position on the timeline.
    Key(Ticks),
    /// The chord sounding from a position on the timeline.
    Chord(Ticks),
    /// The name of the song section in force at a position on the timeline.
    Section(Ticks),
    /// The seed a generated clip is written from.
    ///
    /// Typed for a different reason than the other three: "another take" is the *next* seed, so
    /// the way back to a take somebody liked is to type the number it had. Undo reaches the same
    /// place while the take is still on the stack, and not afterwards.
    Seed(ClipId),
    /// The tempo of the stretch a position falls in, in beats per minute.
    ///
    /// The wheel and the drag are for finding a tempo by feel. This is for the case where the
    /// number is already known — 174 for a drum and bass track, or whatever the last one was —
    /// and spinning up to it a beat at a time is absurd. It edits the tempo change already in
    /// force at the position rather than writing a new one, because it is opened from the
    /// transport readout, which shows exactly that.
    Tempo(Ticks),
    /// The tempo from a position on the timeline onwards — a new tempo change.
    ///
    /// The ruler's counterpart to [`PromptTarget::Tempo`]: the same number, but written *here*,
    /// the way [`PromptTarget::Key`] writes a key change here.
    TempoFrom(Ticks),
    /// The meter of the stretch a position falls in, written `6/8`.
    ///
    /// The transport's own list holds the meters nearly everybody wants. This is the way to the
    /// ones it does not — 11/8, 15/16 — and, like [`PromptTarget::Tempo`], it turns the stretch
    /// already in force rather than writing a new change.
    Signature(Ticks),
    /// The meter from a position on the timeline onwards — a new signature change.
    SignatureFrom(Ticks),
    /// An audio clip's own gain, in decibels.
    ClipGain(ClipId),
    /// A parameter, in whatever units its descriptor is written in.
    ///
    /// The drag is for finding a value by ear and the fine drag for the last of it. This is for
    /// the case where the number is already known — 440 Hz, unity gain, the same cutoff as the
    /// track next door — and creeping up to it a pixel at a time is absurd. Clamped by the
    /// descriptor rather than refused: a range is what the control could have reached anyway.
    Param(ParamTarget),
    /// What tempo an audio clip's material was recorded at.
    ClipSourceTempo(ClipId),
    /// Where the playhead sits, as bar, beat and hundredth.
    Position,
    /// The song sheet's title, which its project is named after.
    ///
    /// These four edit the *sheet* and not the document: nothing they set has been written until
    /// Write is pressed, which is why none of them records an undo step.
    SongTitle,
    /// The key the song sheet is set to.
    SongKey,
    /// The seed the song sheet is set to.
    SongSeed,
    /// The name of the part at this position in the song sheet's roster.
    SongPartName(usize),
    /// The chords the section at this position in the song sheet plays, written out.
    SongSectionChart(usize),
    /// The name to keep the chart of the section at this position under.
    ///
    /// The one prompt here that reaches past the sheet: it writes to the progression book, which
    /// outlives the song being written.
    KeepProgression(usize),
}

/// What a field is written in, as opposed to where it is being written.
///
/// The distinction the sheet turns on. A key typed into the harmony lane and a key typed into the
/// song sheet are one question asked in two places: the same rules, the same parser, the same
/// refusal — and for a while the timeline's had a completion list under it and the sheet's had
/// nothing, because the list was chosen by the *target* rather than by what the target takes.
///
/// Naming the notation is what stops the third place that asks for a key having to remember to
/// ask the same way.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Notation {
    /// A key, `F# minor` or `D dorian`.
    Key,
    /// One chord as a roman numeral, `V7` or `bVII`.
    Chord,
    /// A whole progression, bar by bar: `| IVmaj7 | III7 | vi7 | I7 |`.
    Progression,
    /// The name of a section of the song.
    Section,
    /// A time signature, `6/8`.
    Signature,
    /// The number a take is drawn from.
    Seed,
    /// Beats per minute.
    Tempo,
    /// A level in decibels.
    Gain,
    /// A place in the song, as bar, beat and hundredth.
    Position,
}

impl PromptTarget {
    /// What this field is written in, or `None` when it takes a name.
    ///
    /// A name is not a notation: it has no rules to state, nothing to complete against, and
    /// nothing to refuse.
    pub fn notation(self) -> Option<Notation> {
        Some(match self {
            PromptTarget::Key(_) | PromptTarget::SongKey => Notation::Key,
            PromptTarget::Chord(_) => Notation::Chord,
            PromptTarget::SongSectionChart(_) => Notation::Progression,
            PromptTarget::Section(_) => Notation::Section,
            PromptTarget::Signature(_) | PromptTarget::SignatureFrom(_) => Notation::Signature,
            PromptTarget::Seed(_) | PromptTarget::SongSeed => Notation::Seed,
            PromptTarget::Tempo(_)
            | PromptTarget::TempoFrom(_)
            | PromptTarget::ClipSourceTempo(_) => Notation::Tempo,
            PromptTarget::ClipGain(_) => Notation::Gain,
            PromptTarget::Position => Notation::Position,
            PromptTarget::Track(_)
            | PromptTarget::Clip(_)
            | PromptTarget::SongTitle
            | PromptTarget::SongPartName(_)
            | PromptTarget::KeepProgression(_)
            // No shared notation: every parameter is written in its own units, and the range
            // and the unit are in the prompt's title instead, where they can name this one.
            | PromptTarget::Param(_) => return None,
        })
    }

    /// The line under the field saying what would be a valid answer.
    ///
    /// A name explains itself; everything else here is a small notation with rules, and an empty
    /// box states none of them. The chord field was the worst of it — nothing anywhere on screen
    /// said that the case of a numeral is what makes it major or minor, so the only way to find
    /// out was to type something, have it refused, and read the refusal.
    pub fn hint(self) -> Option<Key> {
        self.notation().map(Notation::hint)
    }
}

impl Notation {
    /// The line under the field saying what would be a valid answer.
    pub fn hint(self) -> Key {
        match self {
            Notation::Key => Key::HintKey,
            Notation::Chord => Key::HintChord,
            Notation::Progression => Key::HintProgression,
            Notation::Section => Key::HintSection,
            Notation::Signature => Key::HintSignature,
            Notation::Seed => Key::HintSeed,
            Notation::Tempo => Key::HintTempo,
            Notation::Gain => Key::HintClipGain,
            Notation::Position => Key::HintPosition,
        }
    }

    /// The words offered under the field, or nothing where the answer is a number.
    ///
    /// A progression shares the chord field's list, because a progression *is* chords: the same
    /// vocabulary, offered one chord at a time. Two lists would be one place learning `bVII` and
    /// the other not.
    fn vocabulary(self) -> &'static [&'static str] {
        match self {
            Notation::Key => KEY_VOCABULARY,
            Notation::Chord | Notation::Progression => CHORD_VOCABULARY,
            Notation::Section => SECTION_VOCABULARY,
            Notation::Signature => SIGNATURE_VOCABULARY,
            // 174 is not on any list worth reading.
            Notation::Seed | Notation::Tempo | Notation::Gain | Notation::Position => &[],
        }
    }

    /// Whether choosing a completion answers the whole question.
    ///
    /// A key, a chord, a meter: choosing one *is* the answer, so the sheet closes on it. A
    /// progression is a line of chords, and a chord chosen partway through it is one word of an
    /// answer nobody has finished writing.
    pub fn completes_whole_field(self) -> bool {
        !matches!(self, Notation::Progression)
    }

    /// The stretch of `typed` a completion would replace.
    ///
    /// Everything, for the notations whose whole answer is one word from the list. For a
    /// progression it is the chord being written — from the last bar line or space to the end —
    /// so that completing `b` into `bVII` at the end of `| I | V | vi | b` leaves the three bars
    /// in front of it exactly where they were.
    pub fn completing_range(self, typed: &str) -> Range<usize> {
        if self.completes_whole_field() {
            return 0..typed.len();
        }
        let start = typed
            .char_indices()
            .rev()
            .find(|(_, character)| *character == '|' || character.is_whitespace())
            .map_or(0, |(at, character)| at + character.len_utf8());
        start..typed.len()
    }
}

/// The chord degrees a person actually writes, offered under the chord field.
///
/// Roman numerals and not chord names, because numerals are what the document stores: `V` is `V`
/// in every key, which is the whole reason the harmony lane holds a key and degrees rather than a
/// list of notes. The diatonic seven, then the sevenths, then the borrowings that turn up in
/// nearly every pop song.
const CHORD_VOCABULARY: &[&str] = &[
    "I", "ii", "iii", "IV", "V", "vi", "vii", "Imaj7", "ii7", "iii7", "IVmaj7", "V7", "vi7",
    "bIII", "bVI", "bVII", "iv", "bVII7",
];

/// The section names a person actually writes, offered under the section field.
///
/// The Japanese conventions first — this is the vocabulary a J-pop chart is discussed in — and
/// the English forms after, so both spellings complete. The point of offering a fixed list is
/// consistency rather than coverage: the composer matches labels exactly, so a サビ typed two
/// ways would be two different sections, and the completion is what keeps the spelling in one.
const SECTION_VOCABULARY: &[&str] = &[
    "イントロ",
    "Aメロ",
    "Bメロ",
    "サビ",
    "Cメロ",
    "間奏",
    "落ちサビ",
    "大サビ",
    "アウトロ",
    "Intro",
    "Verse",
    "Pre-Chorus",
    "Chorus",
    "Bridge",
    "Interlude",
    "Outro",
];

/// The keys offered under the key field.
///
/// Enough of the circle to be a starting point and not so much that it is a table to read. The
/// modes are there because the field accepts them and nothing else would say so.
const KEY_VOCABULARY: &[&str] = &[
    "C major",
    "G major",
    "D major",
    "F major",
    "Bb major",
    "Eb major",
    "A minor",
    "E minor",
    "B minor",
    "D minor",
    "G minor",
    "C minor",
    "D dorian",
    "G mixolydian",
    "F lydian",
];

/// The meters offered under the signature field.
///
/// The transport's own list, written out here because the sheet takes text and a completion has
/// to be text. It is the escape hatch from that list, so the list still belongs under it: somebody
/// who opened this to type 11/8 loses nothing by seeing 6/8 offered, and somebody who opened it by
/// accident finds the ordinary answer without cancelling.
const SIGNATURE_VOCABULARY: &[&str] = &["4/4", "3/4", "2/4", "6/8", "12/8", "5/4", "7/8", "9/8"];

/// How many completions the sheet offers at once.
///
/// A row that wraps to three lines is a table, and a table is something to read rather than
/// something to reach for.
const COMPLETION_LIMIT: usize = 8;

/// What the sheet offers for the text typed so far.
///
/// Prefix matches lead and substring matches follow, so typing `7` reaches the sevenths while
/// typing `v` still leads with `V` rather than with whatever sorts first. Case is ignored for the
/// matching and kept in the answer: `vi` and `VI` are different chords, and half the point of the
/// list is to show that both exist.
///
/// Matched against the stretch a completion would replace rather than against the whole box, so a
/// progression offers chords for the one being written instead of trying to match the line.
pub fn completions(target: PromptTarget, typed: &str) -> Vec<&'static str> {
    let Some(notation) = target.notation() else {
        return Vec::new();
    };
    let vocabulary = notation.vocabulary();
    if vocabulary.is_empty() {
        return Vec::new();
    }
    let needle = typed[notation.completing_range(typed)]
        .trim()
        .to_ascii_lowercase();
    let mut offered: Vec<&'static str> = Vec::new();
    let mut contained: Vec<&'static str> = Vec::new();
    for entry in vocabulary {
        let candidate = entry.to_ascii_lowercase();
        if needle.is_empty() || candidate.starts_with(&needle) {
            offered.push(entry);
        } else if candidate.contains(&needle) {
            contained.push(entry);
        }
    }
    offered.extend(contained);
    offered.truncate(COMPLETION_LIMIT);
    offered
}

/// What the user was doing when the document turned out to be in the way.
///
/// Carried through the sheet so that answering it finishes the command the user actually gave,
/// rather than dismissing a box and leaving them to give it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    /// Empty the document.
    NewProject,
    /// Read another document over this one, asking which.
    OpenProject,
    /// Read *this* document over this one, having already been told which.
    ///
    /// By a drop, or by a choice from the recent list — both know the path before the question
    /// is asked. A separate variant rather than a path on [`Self::OpenProject`], because the two
    /// differ in what happens after Save: one opens a file dialog and the other opens a file.
    /// Carrying the path is what makes the answer be the project that was chosen rather than the
    /// one a second dialog would go on to ask for.
    OpenDropped(std::path::PathBuf),
    /// Read a MIDI file as a new document, having already been told which by a drop.
    ImportMidi(std::path::PathBuf),
    /// Ask which MIDI file to read, and read it as a new document.
    ImportMidiPicked,
    /// Start a take, now that there is a folder to write it into.
    ///
    /// The one pending action that is not about unsaved work. It exists because recording is the
    /// one command that *needs* a saved project — a take is written to disk while it happens —
    /// and refusing it with "save first" is a dead end where a save dialog is an answer.
    StartRecording,
    /// Shut the window.
    CloseWindow,
    /// Leave.
    Quit,
}

/// Which button was pressed on a [`Question`].
///
/// Cancel is not one of them: it closes the sheet and nothing else, so it needs no answer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The safe one — Save, or Replace.
    Confirm,
    /// The destructive one — Discard. Only [`Question::Unsaved`] offers it.
    Deny,
}

/// A question the sheet is asking, and what turns on the answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// Something is about to destroy unsaved work.
    ///
    /// Three answers: save first, throw the changes away, or do neither.
    Unsaved(PendingAction),
    /// Saving would replace a project already in that folder.
    ///
    /// Two answers, because there is no third thing to do with it. The path is the one that
    /// would be *written*, which is not the one the system dialog asked about.
    Replace {
        /// What the user chose in the save dialog.
        chosen: std::path::PathBuf,
        /// The document that already exists there.
        existing: std::path::PathBuf,
        /// What the user was doing, carried through this second question the same way it was
        /// carried through the first: replacing is still on the way to quitting, closing or
        /// opening, and dropping it here silently un-gave the command.
        then: Option<PendingAction>,
    },
}

/// What an open sheet is for.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptBody {
    /// A line of text, committed with Return.
    Text {
        /// What gets renamed on commit.
        target: PromptTarget,
        /// The text being edited.
        field: TextField,
    },
    /// A question, answered with a button.
    Ask(Question),
    /// A list of things that went wrong, which the user reads and closes.
    ///
    /// For what will not fit on the status line: every audio file a project could not find,
    /// every complaint a specification's parser has. The status line is one truncated row that
    /// the next command overwrites, so a list put there is a list nobody sees.
    Notice(Vec<SharedString>),
}

/// An open sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// Heading above the body.
    pub title: SharedString,
    /// What it is asking for.
    pub body: PromptBody,
    /// A Tab walk in progress: what was typed when it began, and how far along it is.
    ///
    /// The text Tab started from has to be kept, because completing *into* the field narrows the
    /// list the next Tab would compute — after one press of `v` → `V` the candidates are those
    /// beginning with `V`, and half of what the user was walking has gone. Remembering the
    /// original prefix is what makes the second press offer the second candidate for what was
    /// typed rather than the second candidate for what the first press wrote.
    completing: Option<(String, usize)>,
}

impl Prompt {
    /// A prompt editing `text`.
    pub fn new(
        title: impl Into<SharedString>,
        target: PromptTarget,
        text: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: PromptBody::Text {
                target,
                field: TextField::new(text),
            },
            completing: None,
        }
    }

    /// A prompt asking a question rather than taking a name.
    pub fn ask(title: impl Into<SharedString>, question: Question) -> Self {
        Self {
            title: title.into(),
            body: PromptBody::Ask(question),
            completing: None,
        }
    }

    /// A prompt reporting a list of things, with nothing to decide.
    pub fn notice(
        title: impl Into<SharedString>,
        lines: impl IntoIterator<Item = SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            body: PromptBody::Notice(lines.into_iter().collect()),
            completing: None,
        }
    }

    /// The text being edited, when this sheet is editing any.
    pub fn field(&self) -> Option<&TextField> {
        match &self.body {
            PromptBody::Text { field, .. } => Some(field),
            PromptBody::Ask(_) | PromptBody::Notice(_) => None,
        }
    }

    /// What the text will be written to, when this sheet is asking for text.
    pub fn target(&self) -> Option<PromptTarget> {
        match &self.body {
            PromptBody::Text { target, .. } => Some(*target),
            PromptBody::Ask(_) | PromptBody::Notice(_) => None,
        }
    }

    /// The same field, to type into.
    ///
    /// Taking it ends any Tab walk in progress, because the next Tab should complete what is in
    /// the box now rather than resume a list derived from what was in it three keystrokes ago.
    /// Every path that changes the text comes through here — the key handler and the platform's
    /// input handler both — which is why the reset can live in one place. [`Self::completing`]
    /// is set by the walk itself, which reaches the field without going through this.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        self.completing = None;
        match &mut self.body {
            PromptBody::Text { field, .. } => Some(field),
            PromptBody::Ask(_) | PromptBody::Notice(_) => None,
        }
    }

    /// Walks to the next completion, returning `true` when there was one to walk to.
    ///
    /// Wraps, so a list that has been walked to the end comes back round to where it started
    /// rather than sticking on the last entry with no way back but retyping.
    pub fn complete_next(&mut self) -> bool {
        let PromptBody::Text { target, field } = &self.body else {
            return false;
        };
        let target = *target;
        let Some(notation) = target.notation() else {
            return false;
        };
        let (from, next) = match &self.completing {
            Some((from, index)) => (from.clone(), index + 1),
            None => (field.content().to_string(), 0),
        };
        let offered = completions(target, &from);
        if offered.is_empty() {
            return false;
        }
        let index = next % offered.len();
        let chosen = offered[index];
        if let PromptBody::Text { field, .. } = &mut self.body {
            // From where the word being completed began, to the end of whatever the last step of
            // the walk left there. The text in front of it is untouched by the walk, so its start
            // is still the one the original text gave.
            let word = notation.completing_range(&from).start;
            field.replace(word..field.content().len(), chosen);
        }
        self.completing = Some((from, index));
        true
    }

    /// The text a Tab walk started from, and how far it has got.
    pub fn completing(&self) -> Option<(&str, usize)> {
        self.completing
            .as_ref()
            .map(|(from, index)| (from.as_str(), *index))
    }
}

/// Font size of the edited text.
pub(crate) const TEXT_SIZE: Pixels = px(13.0);
/// Height of the field's box.
const FIELD_HEIGHT: Pixels = px(28.0);
/// Space between the field's edge and its text.
pub(crate) const FIELD_PADDING: Pixels = px(8.0);

impl AurisApp {
    /// Opens a rename sheet, replacing any open menu.
    pub(crate) fn open_prompt(&mut self, prompt: Prompt) {
        self.menu = None;
        self.prompt = Some(prompt);
    }

    /// Applies the prompt and closes it.
    ///
    /// For a question that is the affirmative answer, which is what Return means on a sheet with
    /// a default button.
    pub(crate) fn commit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let (target, field) = match prompt.body {
            PromptBody::Text { target, field } => (target, field),
            PromptBody::Ask(question) => {
                self.answer(question, Answer::Confirm, window, cx);
                return;
            }
            // Nothing to apply — it has been read, and taking it off the screen is the whole
            // action.
            PromptBody::Notice(_) => return,
        };
        let text = field.content().trim().to_string();
        if text.is_empty() {
            // An empty name would leave an unlabelled row the user cannot tell apart from its
            // neighbours, and an empty key or chord is not a key or a chord.
            self.set_status(self.t(Key::NameCannotBeEmpty));
            return;
        }
        let outcome = match target {
            PromptTarget::Track(track) => self.session.rename_track(track, text),
            PromptTarget::Clip(clip) => self.session.rename_clip(clip, text),
            // These two parse rather than rename, and a rejection has to say what was rejected:
            // `Bbb minor` and `H7` look plausible enough that "invalid input" would not help.
            PromptTarget::Key(at) => match MusicalKey::parse(&text) {
                Some(key) => {
                    self.session.set_key(at, key);
                    Ok(())
                }
                None => {
                    self.set_failed_status(messages::not_a_key(self.language(), &text));
                    return;
                }
            },
            PromptTarget::Chord(at) => match Numeral::parse(&text) {
                Some(chord) => {
                    self.session.set_chord(at, chord);
                    Ok(())
                }
                None => {
                    self.set_failed_status(messages::not_a_chord(self.language(), &text));
                    return;
                }
            },
            // Any text is a name, so there is nothing to parse and nothing to refuse; the
            // empty case was already turned away above, and removing a section is the menu's
            // job rather than an empty field's.
            PromptTarget::Section(at) => {
                self.session.set_section(at, Some(text));
                Ok(())
            }
            // The four that edit the song sheet. None of them records an undo step: the sheet is
            // a question about a song nobody has written yet.
            PromptTarget::SongTitle => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    dials.title = text;
                }
                Ok(())
            }
            PromptTarget::SongKey => match auris_session::prelude::MusicalKey::parse(&text) {
                Some(key) => {
                    if let Some(dials) = self.song_sheet.as_mut() {
                        dials.key = key;
                    }
                    Ok(())
                }
                None => {
                    self.set_failed_status(messages::not_a_key(self.language(), &text));
                    return;
                }
            },
            PromptTarget::SongSeed => match text.parse::<u64>() {
                Ok(seed) => {
                    if let Some(dials) = self.song_sheet.as_mut() {
                        dials.seed = seed;
                    }
                    Ok(())
                }
                Err(_) => {
                    self.set_failed_status(messages::not_a_seed(self.language(), &text));
                    return;
                }
            },
            // A part's name is what the composer keys its material by, so two of one name would
            // be two parts writing the same notes — and the format refuses it outright.
            PromptTarget::SongPartName(index) => {
                let taken = self.song_sheet.as_ref().is_some_and(|dials| {
                    dials
                        .parts
                        .iter()
                        .enumerate()
                        .any(|(other, part)| other != index && part.name == text)
                });
                if taken || text.trim().is_empty() {
                    self.set_failed_status(self.t(Key::NameCannotBeEmpty).to_string());
                    return;
                }
                if let Some(part) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.parts.get_mut(index))
                {
                    part.name = text;
                }
                Ok(())
            }
            // A progression written out by hand. Named after the section it was written for, so
            // there is one prompt rather than two — and a second section can still reach it, from
            // the same picker, under that name.
            PromptTarget::SongSectionChart(index) => {
                let Some(chart) = Chart::parse(&text) else {
                    self.set_failed_status(messages::not_a_chord(self.language(), &text));
                    return;
                };
                let name = self
                    .song_sheet
                    .as_ref()
                    .and_then(|dials| dials.sections.get(index))
                    .map(|section| section.name.clone());
                if let (Some(dials), Some(name)) = (self.song_sheet.as_mut(), name) {
                    crate::ui::compose_sheet::give_section_chart(dials, index, &name, chart);
                }
                Ok(())
            }
            // The one prompt that reaches past the sheet: the book outlives the song.
            PromptTarget::KeepProgression(index) => {
                let held = self.song_sheet.as_ref().and_then(|dials| {
                    let section = dials.sections.get(index)?;
                    let (_, chart) = dials
                        .charts
                        .iter()
                        .find(|(name, _)| name == &section.chords)?;
                    Some(chart.clone())
                });
                let Some(chart) = held else { return };
                if !self.progressions.keep(&text, &chart, chart.mode) {
                    // A name the built-in catalogue already uses, or none at all.
                    self.set_failed_status(self.t(Key::NameCannotBeEmpty).to_string());
                    return;
                }
                if let Err(error) = self.progressions.save() {
                    self.set_failed_status(messages::failed(
                        self.language(),
                        self.t(Key::SongKeepProgression),
                        &error.to_string(),
                    ));
                    return;
                }
                self.set_status(messages::saved(
                    self.language(),
                    &auris_session::progressions::ProgressionBook::path()
                        .display()
                        .to_string(),
                ));
                Ok(())
            }
            PromptTarget::Seed(clip) => match text.parse::<u64>() {
                Ok(seed) => {
                    self.set_clip_seed(clip, seed);
                    Ok(())
                }
                Err(_) => {
                    self.set_failed_status(messages::not_a_seed(self.language(), &text));
                    return;
                }
            },
            // A tempo out of range is clamped rather than refused. The bounds are the session's,
            // and a number a person typed is a number they meant — landing on the nearest tempo
            // that exists says more than a box that empties itself.
            PromptTarget::Tempo(at) => match text.parse::<f64>() {
                Ok(bpm) if bpm.is_finite() && bpm > 0.0 => {
                    self.session.set_tempo_at(at, bpm);
                    Ok(())
                }
                _ => {
                    self.set_failed_status(messages::not_a_tempo(self.language(), &text));
                    return;
                }
            },
            PromptTarget::TempoFrom(at) => match text.parse::<f64>() {
                Ok(bpm) if bpm.is_finite() && bpm > 0.0 => {
                    self.session.set_tempo_point(at, bpm);
                    Ok(())
                }
                _ => {
                    self.set_failed_status(messages::not_a_tempo(self.language(), &text));
                    return;
                }
            },
            // Out of range is clamped for the same reason a tempo is; only a value that is
            // not a number at all is turned away.
            PromptTarget::Param(param) => {
                let Some(descriptor) = self.session.descriptor_for(param) else {
                    return;
                };
                match crate::ui::plugin_editor::parse_param_value(&text) {
                    Some(value) => {
                        self.session.set_param(param, descriptor.clamp(value));
                        Ok(())
                    }
                    None => {
                        self.set_failed_status(messages::not_a_number(self.language(), &text));
                        return;
                    }
                }
            }
            PromptTarget::ClipGain(clip) => match text.parse::<f32>() {
                Ok(gain_db) if gain_db.is_finite() => self.session.set_clip_gain(clip, gain_db),
                _ => {
                    self.set_failed_status(messages::not_a_gain(self.language(), &text));
                    return;
                }
            },
            // An empty box means "nobody knows", which is a thing a clip is allowed to say and
            // the only way back from a tempo typed in by mistake.
            PromptTarget::ClipSourceTempo(clip) => match text.trim().is_empty() {
                true => self.session.set_clip_source_bpm(clip, None),
                false => match text.parse::<f64>() {
                    Ok(bpm) if bpm.is_finite() && bpm > 0.0 => {
                        self.session.set_clip_source_bpm(clip, Some(bpm))
                    }
                    _ => {
                        self.set_failed_status(messages::not_a_tempo(self.language(), &text));
                        return;
                    }
                },
            },
            // A meter is refused rather than clamped, unlike a tempo: there is no nearest
            // signature to land on, and `TimeSignature::new` answering 5/3 with a silent 4/4
            // would be a box that changed the subject.
            PromptTarget::Signature(at) => match text.parse::<TimeSignature>() {
                Ok(signature) => {
                    self.session.set_signature_at(at, signature);
                    Ok(())
                }
                Err(_) => {
                    self.set_failed_status(messages::not_a_signature(self.language(), &text));
                    return;
                }
            },
            PromptTarget::SignatureFrom(at) => match text.parse::<TimeSignature>() {
                Ok(signature) => {
                    self.session.set_signature_point(at, signature);
                    Ok(())
                }
                Err(_) => {
                    self.set_failed_status(messages::not_a_signature(self.language(), &text));
                    return;
                }
            },
            PromptTarget::Position => {
                match crate::ui::transport_bar::parse_position(&text, &self.project().signatures) {
                    Some(at) => {
                        self.seek(at);
                        Ok(())
                    }
                    None => {
                        self.set_failed_status(messages::not_a_position(self.language(), &text));
                        return;
                    }
                }
            }
        };
        if let Err(error) = outcome {
            self.set_failed_status(self.failure(Key::Rename, &error));
        }
    }

    /// Closes the prompt without applying it.
    pub(crate) fn cancel_prompt(&mut self) -> bool {
        self.prompt.take().is_some()
    }

    /// Asks about unsaved work before `next` destroys it.
    ///
    /// Returns `true` when there was nothing to ask about and the caller may go ahead. When it
    /// returns `false` the sheet is open and will finish the command itself.
    pub(crate) fn confirm_discard(&mut self, next: PendingAction) -> bool {
        // A running take is the one piece of unsaved work the dirty flag cannot see: until the
        // take is stopped the document is untouched, so quitting mid-recording used to go through
        // here without a question and take the performance with it. Stopping it makes it a clip,
        // which makes the document dirty, which makes the question below the right one — and it
        // is what closes the file properly whatever the answer turns out to be.
        if self.session.is_recording() {
            self.finish_recording();
        }
        if !self.session.is_dirty() {
            return true;
        }
        self.open_prompt(Prompt::ask(
            self.t(Key::UnsavedTitle),
            Question::Unsaved(next),
        ));
        false
    }

    /// Acts on a question's answer, having already closed the sheet.
    fn answer(
        &mut self,
        question: Question,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (question, answer) {
            // Save first, then carry on — but only if the save worked. A disk that is full must
            // not be the thing that throws the afternoon away.
            (Question::Unsaved(next), Answer::Confirm) => self.save_then(next, window, cx),
            (Question::Unsaved(next), Answer::Deny) => self.run_pending(next, window, cx),
            (Question::Replace { chosen, then, .. }, _) => {
                match self.session.save_as_replacing(&chosen) {
                    Ok(report) => {
                        self.report_save(&report);
                        // The command the user actually gave — quit, close, new, open —
                        // finishes now, exactly as it would have had the name not collided.
                        if let Some(next) = then {
                            self.run_pending(next, window, cx);
                        }
                    }
                    Err(error) => self.set_failed_status(self.failure(Key::CmdSave, &error)),
                }
            }
        }
    }

    /// Saves, and does `next` if that worked.
    fn save_then(&mut self, next: PendingAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.path().is_none() {
            // Never saved, so this needs a name — and the file dialog is asynchronous, which is
            // why `next` has to travel with it rather than being done when this returns.
            self.save_as_then(Some(next), window, cx);
            return;
        }
        match self.session.save_in_place() {
            Ok(()) => {
                let path = self.session.path().map(|p| p.display().to_string());
                self.set_status(messages::saved(self.language(), &path.unwrap_or_default()));
                self.run_pending(next, window, cx);
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdSave, &error)),
        }
    }

    /// Does what the user asked for before the document got in the way.
    pub(crate) fn run_pending(
        &mut self,
        next: PendingAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match next {
            PendingAction::NewProject => self.new_project(),
            PendingAction::OpenProject => self.pick_and_open_project(cx),
            PendingAction::OpenDropped(path) => self.open_project_at(path, cx),
            PendingAction::ImportMidi(path) => self.import_midi_at(path, cx),
            PendingAction::ImportMidiPicked => self.pick_and_import_midi(cx),
            PendingAction::StartRecording => self.toggle_recording(window, cx),
            PendingAction::CloseWindow => window.remove_window(),
            PendingAction::Quit => cx.quit(),
        }
    }

    /// Handles a keystroke aimed at the open prompt.
    ///
    /// Returns `true` when the key was used, so the caller can stop it reaching the rest of the
    /// application. Only the keys the platform does *not* deliver as text are handled here;
    /// everything else arrives through the input handler, which is what keeps an IME working.
    pub(crate) fn prompt_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let shift = event.keystroke.modifiers.shift;
        // ⌘ on macOS, Ctrl elsewhere. Reading `platform` directly would put select-all on the
        // Windows key, which opens the shell's own menu long before the application sees it.
        let command = event.keystroke.modifiers.secondary();

        // Tab walks the completions, and is answered before the field is taken. `field_mut` ends
        // a walk in progress, which is right for every key that changes the text and wrong for
        // the one key that is *continuing* the walk — taking it here would reset on every step.
        //
        // Not while an IME is composing: Tab belongs to the candidate window then, and the
        // platform has already offered it there before the window sees it.
        if event.keystroke.key == "tab"
            && let Some(field) = self.prompt.as_ref().and_then(Prompt::field)
        {
            if field.marked().is_none()
                && let Some(prompt) = self.prompt.as_mut()
            {
                prompt.complete_next();
            }
            // Swallowed either way. There is nothing else on the sheet for Tab to reach, and a
            // sheet that let it out would move the focus ring behind a box that is still open.
            return true;
        }

        let Some(prompt) = self.prompt.as_mut() else {
            return false;
        };
        // A question has no field to type into, so only the two keys that answer it apply.
        let Some(field) = prompt.field_mut() else {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.cancel_prompt();
                }
                "enter" => self.commit_prompt(window, cx),
                _ => return false,
            }
            return true;
        };
        // While the IME is composing, these keys belong to the candidate window, and the
        // platform has already offered them to it before we see them.
        let composing = field.marked().is_some();

        match event.keystroke.key.as_str() {
            "escape" if !composing => {
                self.cancel_prompt();
            }
            "enter" if !composing => {
                self.commit_prompt(window, cx);
            }
            // Up and Down are this sheet's own: it has one line and no rows, so the only
            // sensible reading of them is the ends of that line.
            "up" => field.move_home(shift),
            "down" => field.move_end(shift),
            // Copy, cut and paste. A rename box that cannot take a name off the clipboard means
            // retyping a chord symbol or a path by hand every time, and there is nowhere else in
            // the application to type text.
            "c" if command => {
                let selected = field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                }
            }
            "x" if command => {
                let selected = field.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                    field.backspace();
                }
            }
            "v" if command => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                if let Some(text) = pasted {
                    // One line. A clipboard full of newlines would otherwise be typed into a
                    // field that draws exactly one row of text.
                    field.insert(&text.replace(['\n', '\r'], " "));
                }
            }
            // Everything else that is not a character: backspace, the caret, Select All. Shared
            // with every other field in the window rather than written out again here.
            key => {
                return field.apply_key(key, shift, command)
                    != crate::ui::text_field::KeyEffect::Ignored;
            }
        }
        true
    }

    /// Draws the sheet over everything else.
    pub(crate) fn render_prompt(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let prompt = self.prompt.as_ref()?;
        let theme = self.theme.clone();
        let title = prompt.title.clone();
        let focus = self.focus.clone();
        let view = cx.entity();

        let (body, buttons) = match &prompt.body {
            PromptBody::Text { target, field } => (
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.render_prompt_field(
                        field.content().to_string().into(),
                        field.selection(),
                        field.marked(),
                        focus,
                        view,
                        &theme,
                    ))
                    .children(
                        target
                            .hint()
                            .map(|hint| self.render_prompt_hint(hint, &theme)),
                    )
                    .children(self.render_prompt_completions(prompt, cx))
                    .into_any_element(),
                self.render_prompt_buttons(None, self.t(Key::Rename).into(), cx),
            ),
            PromptBody::Ask(question) => {
                let (message, confirm, deny) = self.question_text(question);
                (
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(message)
                        .into_any_element(),
                    self.render_prompt_buttons(deny, confirm, cx),
                )
            }
            PromptBody::Notice(lines) => (
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(280.0))
                    .overflow_hidden()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .children(lines.iter().cloned().map(|line| div().child(line)))
                    .into_any_element(),
                self.render_prompt_buttons(None, self.t(Key::Close).into(), cx),
            ),
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(120.0))
                .bg(Theme::translucent(theme.background, 0.55))
                // A sheet asking a question has to be answered, so nothing behind it is hit by
                // any button or by the wheel. Stopping left-click propagation was not enough: a
                // right-click went through the dim and opened a menu on top of the sheet.
                .occlude()
                // A click outside the sheet cancels, which is what every rename box does.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.cancel_prompt();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(360.0))
                        .p_3()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .child(div().text_sm().text_color(theme.text).child(title))
                        .child(body)
                        .child(buttons),
                ),
        )
    }

    /// The box the rename sheet types into.
    fn render_prompt_field(
        &self,
        text: SharedString,
        selection: Range<usize>,
        marked: Option<Range<usize>>,
        focus: gpui::FocusHandle,
        view: gpui::Entity<AurisApp>,
        theme: &Theme,
    ) -> impl IntoElement + use<> {
        div()
            .h(FIELD_HEIGHT)
            .w_full()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(theme.accent)
            .child(editable_text(
                text,
                selection,
                marked,
                focus,
                view,
                theme.clone(),
            ))
    }

    /// The line under the field saying what a valid answer looks like.
    fn render_prompt_hint(&self, hint: Key, theme: &Theme) -> impl IntoElement + use<> {
        div()
            .text_xs()
            .text_color(theme.text_faint)
            .child(self.t(hint))
    }

    /// The row of values the field would accept, narrowing as the text is typed.
    ///
    /// Pressing one fills the field *and* commits, because every one of them is a whole answer
    /// rather than a prefix of one. A chip that only filled the box would be asking for a second
    /// press to do what the first one already said.
    fn render_prompt_completions(
        &self,
        prompt: &Prompt,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let PromptBody::Text { target, field } = &prompt.body else {
            return None;
        };
        // While Tab is walking, the row is the list it is walking rather than one recomputed from
        // what the last press wrote — which would narrow under the walk, so that the candidates
        // slid out from under the eye at every step.
        let (typed, walking) = match prompt.completing() {
            Some((from, index)) => (from, Some(index)),
            None => (field.content(), None),
        };
        let offered = completions(*target, typed);
        if offered.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(offered.into_iter().enumerate().map(|(index, entry)| {
                    button(
                        SharedString::from(format!("complete:{entry}")),
                        entry,
                        ButtonStyle::Normal,
                        walking == Some(index),
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, window, cx| {
                            this.complete_prompt(entry, window, cx);
                            cx.notify();
                        }),
                    )
                })),
        )
    }

    /// Writes `text` into the open prompt, and answers with it where it is the whole answer.
    ///
    /// A chip that only filled the box would be asking for a second press to do what the first
    /// one already said — unless the box takes more than one word. On a progression the press
    /// wrote one chord of a line the user is still in the middle of, so the sheet stays up.
    pub(crate) fn complete_prompt(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let notation = self
            .prompt
            .as_ref()
            .and_then(Prompt::target)
            .and_then(PromptTarget::notation);
        if let Some(field) = self.prompt.as_mut().and_then(Prompt::field_mut) {
            let word = notation.map_or(0, |notation| {
                notation.completing_range(field.content()).start
            });
            field.replace(word..field.content().len(), text);
        }
        if notation.is_none_or(Notation::completes_whole_field) {
            self.commit_prompt(window, cx);
        }
    }

    /// The row of answers: always Cancel, sometimes a destructive one, then the default.
    ///
    /// Cancel leads and the default trails, so the button under the pointer when a sheet appears
    /// is never the one that throws work away.
    fn render_prompt_buttons(
        &self,
        deny: Option<SharedString>,
        confirm: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(button(
                "prompt-cancel",
                self.t(Key::Cancel),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.cancel_prompt();
                    cx.notify();
                }),
            ))
            .children(deny.map(|label| {
                button(
                    "prompt-deny",
                    label,
                    ButtonStyle::Normal,
                    false,
                    theme.danger,
                    &theme,
                    cx.listener(|this, _, window, cx| {
                        if let Some(Prompt {
                            body: PromptBody::Ask(question),
                            ..
                        }) = this.prompt.take()
                        {
                            this.answer(question, Answer::Deny, window, cx);
                        }
                        cx.notify();
                    }),
                )
            }))
            .child(button(
                "prompt-ok",
                confirm,
                ButtonStyle::Primary,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| {
                    this.commit_prompt(window, cx);
                    cx.notify();
                }),
            ))
    }

    /// The words a question is asked in: the body, the affirmative, and the destructive answer.
    fn question_text(
        &self,
        question: &Question,
    ) -> (SharedString, SharedString, Option<SharedString>) {
        match question {
            Question::Unsaved(_) => (
                self.t(Key::UnsavedBody).into(),
                self.t(Key::CmdSave).into(),
                Some(self.t(Key::Discard).into()),
            ),
            Question::Replace { existing, .. } => (
                messages::would_replace(self.language(), &existing.display().to_string()).into(),
                self.t(Key::Replace).into(),
                None,
            ),
        }
    }

    /// The field the platform is typing into, when one is open.
    ///
    /// The palette first: opening it closes the rename sheet, so the two are never both open, and
    /// asking in this order means the answer does not depend on that staying true.
    fn writable_field(&mut self) -> Option<&mut TextField> {
        // In the order they sit in front of each other. The library's field is in a panel rather
        // than in a sheet, so anything modal opened over it takes the typing back.
        if let Some(palette) = self.palette.as_mut() {
            return Some(&mut palette.field);
        }
        if let Some(field) = self.prompt.as_mut().and_then(Prompt::field_mut) {
            return Some(field);
        }
        self.library_search_focused
            .then_some(&mut self.library_search)
    }
}

impl crate::ui::text_field::HasTextField for AurisApp {
    fn field(&mut self) -> Option<&mut TextField> {
        self.writable_field()
    }

    fn readable_field(&self) -> Option<&TextField> {
        if let Some(palette) = self.palette.as_ref() {
            return Some(&palette.field);
        }
        if let Some(field) = self.prompt.as_ref().and_then(Prompt::field) {
            return Some(field);
        }
        self.library_search_focused.then_some(&self.library_search)
    }

    /// Puts the palette's highlight back on the first row.
    ///
    /// Typing narrows the list, and the row that inherits the highlight's position is not the row
    /// that had it. Done here because this is where typing arrives — the key handler never sees a
    /// character, which is what lets an IME compose into the field.
    fn text_changed(&mut self) {
        if let Some(palette) = self.palette.as_mut() {
            palette.selected = 0;
        }
    }
}

crate::entity_input_handler!(AurisApp);

/// A one-line editable text element: the caret, the selection, the IME's pre-edit, and the
/// registration that makes the platform type into it.
///
/// Everything is copied in rather than borrowed because a paint closure has to capture `'static`.
/// Shared by the rename sheet and the command palette, which are the same field with different
/// things underneath it.
/// A line of static text laid out exactly where [`editable_text`] would paint one.
///
/// A field that is not being typed into still has to *look* like the same field: a placeholder
/// or a value drawn flush against the box while the real thing is inset by [`FIELD_PADDING`]
/// puts the caret a character into the placeholder the moment somebody clicks, which is what the
/// library's search box did on the day it was written. The size matters for the same reason —
/// text that changed size when the field took focus would be a field that twitched.
///
/// Not a decision so much as a constant with two readers, kept next to the paint it has to agree
/// with rather than in the panel that happens to need it.
pub(crate) fn field_text(text: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h_full()
        .pl(FIELD_PADDING)
        .text_size(TEXT_SIZE)
        .text_color(color)
        .truncate()
        .child(text.into())
}

pub(crate) fn editable_text<V: gpui::EntityInputHandler>(
    text: SharedString,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    focus: gpui::FocusHandle,
    view: gpui::Entity<V>,
    theme: Theme,
) -> impl IntoElement + use<V> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            // Registering the handler is only legal during paint, and only matters while this
            // element exists — which is exactly as long as the sheet holding it is open.
            window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
            paint_field(
                window,
                cx,
                bounds,
                &text,
                &selection,
                marked.clone(),
                &theme,
            );
        },
    )
    .size_full()
}

/// Draws the text, its selection and the IME's pre-edit underline.
/// The offset the field has to keep on screen.
///
/// The end of the IME's pre-edit while one is composing — that is where the candidate is being
/// chosen — and otherwise the moving end of the selection, which is the caret.
fn caret_offset(selection: &Range<usize>, marked: Option<&Range<usize>>) -> usize {
    marked.map_or(selection.end, |marked| marked.end)
}

fn paint_field(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    text: &SharedString,
    selection: &Range<usize>,
    marked: Option<Range<usize>>,
    theme: &Theme,
) {
    let origin = point(
        bounds.origin.x + FIELD_PADDING,
        bounds.origin.y + (bounds.size.height - TEXT_SIZE * 1.35) / 2.0,
    );
    // Measuring by shaping the text up to an offset keeps the caret on the same glyph edge the
    // text is actually drawn at, whatever the font does with the characters in between.
    let advance = |window: &mut Window, offset: usize| -> Pixels {
        if offset == 0 {
            return px(0.0);
        }
        let head: SharedString = text[..offset.min(text.len())].to_string().into();
        let mut run = window.text_style().to_run(head.len());
        run.color = theme.text;
        window
            .text_system()
            .shape_line(head, TEXT_SIZE, &[run], None)
            .width
    };

    paint::clipped(window, bounds, |window| {
        // Everything is drawn relative to a scrolled origin, so a name longer than the box does
        // not walk the caret and everything after it off the right-hand edge. Past about fifty
        // Latin characters — or twenty-five full-width ones, which is a sentence of Japanese —
        // the user was typing blind.
        let caret_at = advance(window, caret_offset(selection, marked.as_ref()));
        let visible = bounds.size.width - FIELD_PADDING * 2.0;
        let scroll = (caret_at - visible).max(px(0.0));
        let origin = point(origin.x - scroll, origin.y);

        // Where the platform should put an IME's candidate list. Only knowable here, from the
        // shaped line, and asked for outside a paint — see `text_field::set_caret_bounds`.
        crate::ui::text_field::set_caret_bounds(Bounds {
            origin: point(origin.x + caret_at, bounds.origin.y),
            size: size(px(1.0), bounds.size.height),
        });

        if !selection.is_empty() {
            let start = advance(window, selection.start);
            let end = advance(window, selection.end);
            paint::rect(
                window,
                Bounds {
                    origin: point(origin.x + start, bounds.origin.y + px(3.0)),
                    size: size(end - start, bounds.size.height - px(6.0)),
                },
                Theme::translucent(theme.accent, 0.35),
            );
        }

        paint::label(window, cx, origin, text.clone(), TEXT_SIZE, theme.text);

        // The pre-edit is underlined rather than boxed, matching what every other application
        // on the platform does while an IME is composing.
        if let Some(marked) = marked {
            let start = advance(window, marked.start);
            let end = advance(window, marked.end);
            paint::rect(
                window,
                Bounds {
                    origin: point(
                        origin.x + start,
                        bounds.origin.y + bounds.size.height - px(5.0),
                    ),
                    size: size(end - start, px(1.5)),
                },
                theme.accent,
            );
        }

        if selection.is_empty() {
            let caret = advance(window, selection.start);
            paint::rect(
                window,
                Bounds {
                    origin: point(origin.x + caret, bounds.origin.y + px(4.0)),
                    size: size(px(1.5), bounds.size.height - px(8.0)),
                },
                theme.accent,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Ticks = Ticks(3840);

    /// Every target, so a new one cannot be added without this file being opened.
    fn every_target() -> Vec<PromptTarget> {
        vec![
            PromptTarget::Track(TrackId(1)),
            PromptTarget::Clip(ClipId(1)),
            PromptTarget::Key(AT),
            PromptTarget::Chord(AT),
            PromptTarget::Section(AT),
            PromptTarget::Seed(ClipId(1)),
            PromptTarget::Tempo(AT),
            PromptTarget::TempoFrom(AT),
            PromptTarget::Signature(AT),
            PromptTarget::SignatureFrom(AT),
            PromptTarget::ClipGain(ClipId(1)),
            PromptTarget::Position,
            PromptTarget::SongTitle,
            PromptTarget::SongKey,
            PromptTarget::SongSeed,
            PromptTarget::SongPartName(0),
            PromptTarget::SongSectionChart(0),
            PromptTarget::KeepProgression(0),
        ]
    }

    /// The targets that take a name rather than a notation.
    fn is_a_name(target: PromptTarget) -> bool {
        matches!(
            target,
            PromptTarget::Track(_)
                | PromptTarget::Clip(_)
                | PromptTarget::SongTitle
                | PromptTarget::SongPartName(_)
                | PromptTarget::KeepProgression(_)
        )
    }

    #[test]
    fn everything_that_is_a_notation_rather_than_a_name_says_so() {
        // A name explains itself. Everything else is a small notation with rules that an empty
        // box states none of, which is exactly the complaint the hints answer.
        for target in every_target() {
            assert_eq!(
                target.hint().is_none(),
                is_a_name(target),
                "{target:?} has the wrong idea about needing a hint"
            );
            assert_eq!(
                target.notation().is_none(),
                is_a_name(target),
                "{target:?} has the wrong idea about being a notation"
            );
        }
    }

    #[test]
    fn the_song_sheet_asks_for_a_key_the_same_way_the_timeline_does() {
        // The bug this whole arrangement exists to make unrepeatable. Both fields parse with
        // `MusicalKey::parse` and refuse with the same message, and both said `like C major` —
        // but the list was chosen by the target, so the sheet's key field offered nothing at all
        // and the timeline's offered the circle.
        assert_eq!(
            PromptTarget::SongKey.notation(),
            PromptTarget::Key(AT).notation()
        );
        assert_eq!(PromptTarget::SongKey.hint(), PromptTarget::Key(AT).hint());
        for typed in ["", "c", "min", "dorian"] {
            assert_eq!(
                completions(PromptTarget::SongKey, typed),
                completions(PromptTarget::Key(AT), typed),
                "the two key fields differ on `{typed}`"
            );
        }
        assert!(!completions(PromptTarget::SongKey, "").is_empty());
        // And the same for the seed, which is the other question the sheet asks twice.
        assert_eq!(
            PromptTarget::SongSeed.notation(),
            PromptTarget::Seed(ClipId(1)).notation()
        );
    }

    #[test]
    fn a_progression_completes_the_chord_being_written_and_leaves_the_bars_alone() {
        // A progression is a line of chords, so the list under it is the chord list — offered
        // for the chord being written. Completing the whole box would have thrown away every bar
        // already typed, which is why the field had no completion at all before.
        let written = "| I | V | vi | b";
        let offered = completions(PromptTarget::SongSectionChart(0), written);
        assert_eq!(offered, ["bIII", "bVI", "bVII", "bVII7"], "{offered:?}");

        let mut prompt = Prompt::new("", PromptTarget::SongSectionChart(0), written);
        assert!(prompt.complete_next());
        assert_eq!(prompt.field().unwrap().content(), "| I | V | vi | bIII");
        // And the walk goes on replacing that one chord rather than growing the line.
        assert!(prompt.complete_next());
        assert_eq!(prompt.field().unwrap().content(), "| I | V | vi | bVI");

        // A bar line with nothing after it yet offers the whole vocabulary.
        assert_eq!(
            completions(PromptTarget::SongSectionChart(0), "| I |"),
            completions(PromptTarget::Chord(AT), "")
        );

        // The chord field itself still takes the whole box: one chord is the whole answer there,
        // and the sheet closes on choosing one.
        assert!(
            PromptTarget::Chord(AT)
                .notation()
                .unwrap()
                .completes_whole_field()
        );
        assert!(
            !PromptTarget::SongSectionChart(0)
                .notation()
                .unwrap()
                .completes_whole_field()
        );
    }

    #[test]
    fn a_progression_completes_after_a_separator_of_any_width() {
        // The word being completed starts after the last bar line or space, found by character
        // rather than by byte — a Japanese section name or an ideographic space in the box would
        // otherwise cut a completion into the middle of one.
        let progression = Notation::Progression;
        assert_eq!(progression.completing_range("| I | b"), 6..7);
        assert_eq!(progression.completing_range("b"), 0..1);
        assert_eq!(progression.completing_range("| I |"), 5..5);
        assert_eq!(progression.completing_range("　V"), "　".len().."　V".len());
        // Everything else replaces the box, which is what makes choosing one the answer.
        assert_eq!(Notation::Key.completing_range("C ma"), 0..4);
    }

    #[test]
    fn every_offered_completion_is_one_the_field_would_accept() {
        // The whole point is to answer "what am I supposed to type here". A list containing
        // something the parser refuses would answer it wrongly, which is worse than not
        // answering — the user would have no reason left to doubt their own typing.
        for entry in CHORD_VOCABULARY {
            assert!(
                Numeral::parse(entry).is_some(),
                "`{entry}` is offered under the chord field and is not a chord"
            );
            // The same list is offered inside a progression now, where a different parser reads
            // it. One that took a numeral the chart refused would write a line the sheet then
            // turns away, with the offending chord chosen from its own list.
            assert!(
                Chart::parse(&format!("| {entry} |")).is_some(),
                "`{entry}` is offered inside a progression and is not one"
            );
        }
        for entry in KEY_VOCABULARY {
            assert!(
                MusicalKey::parse(entry).is_some(),
                "`{entry}` is offered under the key field and is not a key"
            );
        }
        for entry in SIGNATURE_VOCABULARY {
            assert!(
                entry.parse::<TimeSignature>().is_ok(),
                "`{entry}` is offered under the signature field and is not a meter"
            );
        }
        // And the list under the field is the list on the transport's own button, so the two
        // cannot come to offer different meters.
        assert_eq!(
            SIGNATURE_VOCABULARY.len(),
            TimeSignature::COMMON.len(),
            "the sheet and the transport disagree about the common meters"
        );
        for (offered, common) in SIGNATURE_VOCABULARY.iter().zip(TimeSignature::COMMON) {
            assert_eq!(*offered, common.to_string());
        }
    }

    #[test]
    fn an_empty_field_is_offered_somewhere_to_start() {
        // The moment the hint matters most is before anything has been typed.
        assert!(!completions(PromptTarget::Chord(AT), "").is_empty());
        assert!(!completions(PromptTarget::Key(AT), "").is_empty());
        // And a name has no vocabulary to offer, so it gets no row at all.
        assert!(completions(PromptTarget::Track(TrackId(1)), "").is_empty());
        assert!(completions(PromptTarget::Tempo(AT), "1").is_empty());
    }

    #[test]
    fn typing_narrows_the_list_and_prefixes_lead_it() {
        // `v` has to lead with `V` rather than with whatever sorts first, or the list reads as
        // unrelated to what is being typed.
        let offered = completions(PromptTarget::Chord(AT), "v");
        assert_eq!(offered.first(), Some(&"V"));
        assert!(offered.contains(&"vi"), "{offered:?}");
        assert!(!offered.contains(&"I"), "{offered:?} is not narrowed");

        // Case is ignored for matching, so shift is not something to get right before finding
        // out what the options are — and both cases are still offered, which is how the rule
        // about case gets seen.
        let upper = completions(PromptTarget::Chord(AT), "V");
        assert_eq!(upper, offered);

        // A substring match follows the prefixes: `7` reaches the sevenths.
        let sevenths = completions(PromptTarget::Chord(AT), "7");
        assert!(sevenths.contains(&"V7"), "{sevenths:?}");
        assert!(sevenths.iter().all(|entry| entry.contains('7')));

        // Nothing matching is an empty row rather than the whole list back again.
        assert!(completions(PromptTarget::Chord(AT), "zzz").is_empty());
    }

    #[test]
    fn the_list_never_grows_into_a_table() {
        for typed in ["", "i", "v", "b", "major", "minor"] {
            for target in [PromptTarget::Chord(AT), PromptTarget::Key(AT)] {
                assert!(
                    completions(target, typed).len() <= COMPLETION_LIMIT,
                    "{target:?} offered too many for `{typed}`"
                );
            }
        }
    }

    #[test]
    fn tab_walks_the_list_it_started_from_rather_than_the_one_it_wrote() {
        // `b` offers four borrowings. Completing into the field narrows the list to the entry
        // just written, so a walk that recomputed from the box would put `bIII` there and then
        // have nowhere left to go — three of the four unreachable from the keyboard.
        let mut prompt = Prompt::new("", PromptTarget::Chord(AT), "b");
        let mut walked = Vec::new();
        for _ in 0..4 {
            assert!(prompt.complete_next(), "the walk stopped early");
            walked.push(prompt.field().unwrap().content().to_string());
        }
        assert_eq!(walked, ["bIII", "bVI", "bVII", "bVII7"]);
    }

    #[test]
    fn the_walk_wraps_rather_than_sticking_at_the_end() {
        // Sticking on the last entry would leave retyping as the only way back to the first.
        let offered = completions(PromptTarget::Chord(AT), "b");
        let mut prompt = Prompt::new("", PromptTarget::Chord(AT), "b");
        for _ in 0..offered.len() {
            prompt.complete_next();
        }
        assert_eq!(prompt.field().unwrap().content(), *offered.last().unwrap());
        prompt.complete_next();
        assert_eq!(
            prompt.field().unwrap().content(),
            offered[0],
            "the walk did not come back round"
        );
    }

    #[test]
    fn typing_begins_a_new_walk() {
        // A Tab after an edit that resumed a list derived from what was in the box three
        // keystrokes ago is the one thing a completion must never do.
        let mut prompt = Prompt::new("", PromptTarget::Chord(AT), "b");
        prompt.complete_next();
        prompt.complete_next();
        assert_eq!(prompt.field().unwrap().content(), "bVI");
        assert!(prompt.completing().is_some());

        // Through the door every typing path uses, which is where the reset lives.
        prompt.field_mut().unwrap().select_all();
        assert!(prompt.completing().is_none(), "the walk outlived the edit");
        prompt.field_mut().unwrap().insert("v");
        prompt.complete_next();
        assert_eq!(prompt.field().unwrap().content(), "V");
    }

    #[test]
    fn a_field_with_no_vocabulary_has_nothing_to_walk() {
        // Tab is swallowed on every sheet, but on a name it has to leave the text alone rather
        // than replacing it with whatever a chord field would have offered.
        let mut prompt = Prompt::new("", PromptTarget::Track(TrackId(1)), "Drums");
        assert!(!prompt.complete_next());
        assert_eq!(prompt.field().unwrap().content(), "Drums");
        assert!(prompt.completing().is_none());
    }
}

/// The unsaved-work guard, driven through the window.
///
/// The one path in the application where getting it wrong loses an afternoon, and until now the
/// only way to check it was to make a change, press ⌘N and watch. Every answer is reachable from
/// here: the sheet's three buttons carry names a test can find them by.
#[cfg(test)]
mod window_tests {
    use gpui::TestAppContext;

    use crate::actions;
    use crate::harness::{CLIP_LENGTH, click, open, paint, with_a_clip};

    /// Whether a sheet is open, and what it is asking.
    fn asking(app: &gpui::Entity<crate::app::AurisApp>, cx: &gpui::TestAppContext) -> bool {
        app.read_with(cx, |this, _| {
            matches!(
                this.prompt,
                Some(super::Prompt {
                    body: super::PromptBody::Ask(super::Question::Unsaved(_)),
                    ..
                })
            )
        })
    }

    /// How many tracks the document has, which is what says whether it was replaced.
    fn tracks(app: &gpui::Entity<crate::app::AurisApp>, cx: &gpui::TestAppContext) -> usize {
        app.read_with(cx, |this, _| this.session.project().tracks.len())
    }

    /// A document with unsaved work in it, and the window showing it.
    fn with_unsaved_work(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<crate::app::AurisApp>,
        &mut gpui::VisualTestContext,
    ) {
        let (app, cx, _, _) = with_a_clip(cx);
        app.read_with(cx, |this, _| {
            assert!(this.session.is_dirty(), "the fixture left work to lose");
        });
        (app, cx)
    }

    #[gpui::test]
    fn a_new_project_asks_before_throwing_unsaved_work_away(cx: &mut TestAppContext) {
        let (app, cx) = with_unsaved_work(cx);
        let before = tracks(&app, cx);

        cx.dispatch_action(actions::NewProject);

        assert!(asking(&app, cx), "the sheet is up");
        assert_eq!(tracks(&app, cx), before, "and nothing has happened yet");
    }

    /// The answer that costs the work, which is the one that has to do exactly what it says.
    #[gpui::test]
    fn discarding_at_the_sheet_carries_out_the_command_that_raised_it(cx: &mut TestAppContext) {
        let (app, cx) = with_unsaved_work(cx);
        cx.dispatch_action(actions::NewProject);
        paint(&app, cx);

        click("prompt-deny", cx);

        assert!(!asking(&app, cx), "the sheet is gone");
        app.read_with(cx, |this, _| {
            assert!(
                this.session.project().tracks.iter().all(|track| track
                    .kind
                    .as_instrument()
                    .is_none_or(|inner| inner.clips.is_empty())),
                "a new document, with the clip that was in the way gone"
            );
        });
    }

    /// Cancelling has to leave *both* things alone: the sheet and the document.
    #[gpui::test]
    fn cancelling_at_the_sheet_keeps_the_document(cx: &mut TestAppContext) {
        let (app, cx) = with_unsaved_work(cx);
        let before = tracks(&app, cx);
        cx.dispatch_action(actions::NewProject);
        paint(&app, cx);

        click("prompt-cancel", cx);

        assert!(!asking(&app, cx), "the sheet is gone");
        assert_eq!(tracks(&app, cx), before, "and the work is still here");
        app.read_with(cx, |this, _| assert!(this.session.is_dirty()));
    }

    /// Escape is the same answer as Cancel, and is the one a hand reaches for first.
    #[gpui::test]
    fn escape_at_the_sheet_is_a_cancel(cx: &mut TestAppContext) {
        let (app, cx) = with_unsaved_work(cx);
        let before = tracks(&app, cx);
        cx.dispatch_action(actions::NewProject);

        cx.simulate_keystrokes("escape");

        assert!(!asking(&app, cx));
        assert_eq!(tracks(&app, cx), before);
    }

    /// Nothing to lose, nothing to ask. A question over a clean document is a question that
    /// teaches people to dismiss the sheet without reading it.
    #[gpui::test]
    fn a_clean_document_is_replaced_without_a_question(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.read_with(cx, |this, _| {
            assert!(!this.session.is_dirty(), "a window opens clean");
        });

        cx.dispatch_action(actions::NewProject);

        assert!(!asking(&app, cx));
    }

    /// The sheet claims the keyboard while it is up, so a keystroke bound to the window behind it
    /// must not reach through and act on a document the user is being asked about.
    #[gpui::test]
    fn a_binding_behind_the_sheet_does_not_fire_through_it(cx: &mut TestAppContext) {
        let (app, cx) = with_unsaved_work(cx);
        cx.dispatch_action(actions::NewProject);
        let looping = app.read_with(cx, |this, _| this.session.project().loop_enabled);

        cx.simulate_keystrokes("secondary-l");

        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session.project().loop_enabled,
                looping,
                "the cycle did not toggle behind the question"
            );
        });
        assert!(asking(&app, cx), "and the sheet is still asking");
    }

    /// The fixture's clip is what makes the document dirty; if that ever stops being true every
    /// test above would pass while checking nothing.
    #[gpui::test]
    fn the_fixture_really_does_leave_something_to_lose(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session.midi_clip(clip).expect("still there").length,
                CLIP_LENGTH
            );
            assert!(this.session.is_dirty());
        });
    }
}
