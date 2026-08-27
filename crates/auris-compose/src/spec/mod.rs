//! The text a piece is asked for in.
//!
//! One document describes a whole song: its key, its tempo, its progression, its form and its
//! parts. It is deliberately a *specification* rather than a notation — it says what to write,
//! not what was written — because that is the layer where an instruction like "make the chorus
//! busier" is one word rather than four hundred edited notes.
//!
//! ```
//! # use auris_compose::spec::SongSpec;
//! let spec = SongSpec::parse(r#"
//!     key    = "C minor"
//!     tempo  = 128
//!     chords = "@marusa"
//! "#).unwrap();
//! assert_eq!(spec.tempo, 128.0);
//! ```
//!
//! # Why TOML
//!
//! The syntax is TOML and the extension is `.asong`, exactly as a project file is JSON inside
//! `.auris`. Three reasons it is not a format of its own:
//!
//! * A hand-written parser can only ever be read; serde can also **write**. A dialog that sets a
//!   song's dials has to be able to save what it was set to, and a format that has no serialiser
//!   makes that a second implementation of the same grammar, free to disagree with the first.
//! * It is not indentation-sensitive, which is the property a file a musician edits by hand
//!   most needs and the reason YAML was not chosen.
//! * `[section.chorus]` is a table and `[[part]]` is an array of tables, which is nearly the
//!   shape the format already had.
//!
//! Sections are a table keyed by name because their order means nothing — `form` decides what
//! plays when. Parts are an array because theirs means something: it is the order the tracks
//! are created in, and a map would sort `bass` above `lead` and quietly rearrange the mixer.
//!
//! # What is reported, and when
//!
//! Two kinds of complaint, reported differently on purpose.
//!
//! **Syntax** — a misspelt field, a number where a string belongs, a bracket that does not
//! close — is caught by `toml`, which stops at the first and says which line and column it is
//! on. Unknown fields are refused rather than ignored, because silently dropping a line would
//! mean the piece quietly ignores an instruction.
//!
//! **Meaning** — a key that is not a key, a fraction outside 0 to 1, a `form` naming a section
//! that does not exist — is caught after the document has stopped being text, and every such
//! complaint is reported at once. They have no line number for that reason; each names the
//! field, section or part it is about instead.
//!
//! # Where things are
//!
//! The model is here: [`SongSpec`], [`SectionSpec`], [`PartSpec`] and the queries that read them.
//! [`Role`] and [`Mood`] are the vocabulary one is written in, and each keeps a file of its own
//! because each is mostly a table — the levels, the pans, the colours, the four dials a mood word
//! means — that somebody edits without wanting to read a parser. The parser, the serialiser and
//! every complaint they raise are in `doc`, on the far side of the seam this module is named for:
//! what a song is, apart from how it is written down.

mod doc;
mod mood;
mod role;

use std::collections::BTreeMap;

use auris_core::Subdivision;
use auris_core::time::TimeSignature;

use crate::gm;
use crate::rhythm::Pattern;
use crate::theory::chart::{Chart, ChartOrigin};
use crate::theory::key::Key;

// Re-exported, so `spec::Role` and the rest are the paths they have always been: which file an
// item is written in is not part of the vocabulary a specification is read with.
pub use self::doc::{SpecError, parse_motif};
pub use self::mood::Mood;
pub use self::role::Role;

/// One part of the arrangement.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSpec {
    /// The name it takes in the document and on its track.
    pub name: String,
    /// What it plays.
    pub role: Role,
    /// The plugin that plays it, when no [`Self::program`] names a SoundFont sound.
    pub instrument: String,
    /// The General MIDI sound it asks for: a program on a pitched part, a kit on a drum one.
    ///
    /// `None` — the default — leaves the part on [`Self::instrument`], which is why the built-in
    /// pieces are still built-in voices. Set, it puts the part on the sampler playing that sound
    /// out of whichever General MIDI font is installed.
    ///
    /// The two coexist rather than replacing one another, and that is the point: a build with no
    /// SoundFont installed falls back to the plugin the part also names, so a specification asking
    /// for a violin comes out as an oscillator rather than as silence.
    pub program: Option<gm::Program>,
    /// Which octave it sits in, as an **absolute** MIDI octave rather than an offset.
    ///
    /// A melody's default is 5, so 6 moves it up one and 1 moves it down four. Worth saying
    /// plainly, because the other octave in this system —
    /// [`ClipRecipe::octave`](auris_core::ClipRecipe::octave), the dial on a generated clip — is
    /// a *relative* ±2 from wherever its preset sits, and the two are easy to write for each
    /// other. [`Self::range`] is where the difference from the role's default becomes a shift.
    pub octave: i32,
    /// How busy it is, as a fraction of the available steps.
    pub density: Option<f32>,
    /// How finely this part divides the beat.
    ///
    /// Per part rather than per song: a stab hammering triplets over a straight kit is a sound
    /// somebody wants, and the bar is the same length either way, so the parts still line up.
    /// A drum part ignores it — a groove is written in sixteenths.
    pub subdivision: Subdivision,
    /// How long a note is held, as a fraction of the gap to the one after it.
    pub gate: f32,
    /// A rhythm written out by hand, which overrides the generated one.
    pub rhythm: Option<Pattern>,
    /// Which MIDI note a drum part strikes, when it is not the General MIDI one.
    ///
    /// GM is the only agreement there is about which number is a kick, and a SoundFont is under
    /// no obligation to keep it — a kit that puts its snare somewhere else comes out silent or
    /// playing a cowbell, and there was nothing to say otherwise. A pitched part ignores this:
    /// its notes come from the harmony.
    pub note: Option<u8>,
    /// Level trim in decibels.
    pub gain_db: f32,
    /// Stereo position from -1 to 1.
    pub pan: f32,
}

impl PartSpec {
    /// A part with everything its role implies.
    pub fn of_role(name: impl Into<String>, role: Role) -> Self {
        Self {
            name: name.into(),
            role,
            instrument: role.default_instrument().to_string(),
            program: None,
            octave: role.default_octave(),
            density: None,
            subdivision: Subdivision::default(),
            gate: role.default_gate(),
            rhythm: None,
            note: None,
            gain_db: role.default_gain_db(),
            pan: role.default_pan(),
        }
    }

    /// The SoundFont sound this part asks for, or `None` when it stays on its plugin.
    ///
    /// Which of General MIDI's two readings the number gets is decided here, by the role, and
    /// nowhere else: a kit on a drum part, a program on everything else.
    pub fn sound(&self) -> Option<gm::Sound> {
        self.program
            .map(|program| program.sound(self.role.is_drum()))
    }

    /// The MIDI note this part strikes, or `None` when it is not a drum.
    ///
    /// What the part says, or what General MIDI says its role is. One place, so the sheet's
    /// picker and the writer cannot disagree about which note a kit is about to play.
    pub fn drum_note(&self) -> Option<u8> {
        self.role
            .drum_voice()
            .map(|voice| self.note.unwrap_or_else(|| voice.pitch()))
    }

    /// The MIDI range this part should stay inside, moved by its octave.
    pub fn range(&self) -> (i32, i32) {
        let (low, high) = self.role.range();
        let shift = (self.octave - self.role.default_octave()) * 12;
        (low + shift, high + shift)
    }
}

/// What one section changes about how one part plays.
///
/// Every field is optional and `None` means "whatever the roster says", so a tweak is a *patch*
/// rather than a second declaration of the part. That is what keeps a busier chorus one line
/// instead of a copy of the part with one number changed — and it is why adding a field to
/// [`PartSpec`] does not silently reset it in every section that names a tweak.
///
/// # What is not here, and cannot be
///
/// The name, the role, the instrument, the program, the level and the pan. Those are not how a
/// part *plays*, they are what its **track** is, and a part is one track for the whole song: one
/// row in the arrangement, one instrument, one fader. A chorus on strings where the verse was on a
/// piano is two parts and not one, and [`SectionSpec::parts`] is what brings each of them in.
///
/// The line between the two is worth stating because it is not arbitrary and it is not a
/// limitation to be lifted later: a track that changed instrument half way through would have to
/// be two tracks, and then it was two parts all along.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartTweak {
    /// How busy it is here, as a fraction of the available steps.
    pub density: Option<f32>,
    /// Which octave it sits in here, as an absolute MIDI octave.
    pub octave: Option<i32>,
    /// How long a note is held here, as a fraction of the gap to the one after it.
    pub gate: Option<f32>,
    /// How finely it divides the beat here.
    pub subdivision: Option<Subdivision>,
    /// A rhythm written out by hand for this section, which overrides the generated one.
    pub rhythm: Option<Pattern>,
    /// Which MIDI note a drum part strikes here.
    pub note: Option<u8>,
}

impl PartTweak {
    /// `true` when the tweak changes nothing, and so has nothing to write down.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The part as this section plays it.
    pub fn applied_to(&self, part: &PartSpec) -> PartSpec {
        PartSpec {
            density: self.density.or(part.density),
            octave: self.octave.unwrap_or(part.octave),
            gate: self.gate.unwrap_or(part.gate),
            subdivision: self.subdivision.unwrap_or(part.subdivision),
            rhythm: self.rhythm.clone().or_else(|| part.rhythm.clone()),
            note: self.note.or(part.note),
            ..part.clone()
        }
    }
}

/// How a section that changes key is arrived at.
///
/// Only ever consulted where the section before is in a different one, so on a piece that does not
/// modulate this says nothing at all.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LeadIn {
    /// The last chord before the change becomes the dominant seventh of the key being arrived at.
    ///
    /// The oldest device there is, and the one an arranger reaches for first: a `V7` names its
    /// tonic before the tonic has sounded, so the ear is already in the new key when the section
    /// starts. Without it the change is a step sideways that the listener notices as an *edit*
    /// rather than as an arrival.
    ///
    /// This rewrites a bar of the section before, which is worth stating plainly because nothing
    /// else in this format does — a chart quoted by name is otherwise played exactly as written.
    /// The trade is deliberate: a modulation is a structural instruction, it was asked for by hand,
    /// and there is no way to prepare one without changing the chord that prepares it. [`Self::None`]
    /// is how somebody says they meant the plain jump.
    #[default]
    Dominant,
    /// Nothing. The new key simply begins.
    None,
}

impl LeadIn {
    /// The name the text format writes.
    pub fn name(self) -> &'static str {
        match self {
            LeadIn::Dominant => "dominant",
            LeadIn::None => "none",
        }
    }

    /// Reads a lead-in name, accepting the obvious synonyms.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "dominant" | "v7" | "prepare" => LeadIn::Dominant,
            "none" | "direct" | "off" => LeadIn::None,
            _ => return None,
        })
    }
}

/// How the piece closes.
///
/// A form is a list of sections and the last one used to play its loop out and stop — mid-figure,
/// mid-groove, as if the tape ran out. A piece that ends is a piece; the difference between the
/// two is one bar, and this is the word for whether that bar is written.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Ending {
    /// One extra bar after the last section: the final key's tonic, held by the band, the kick
    /// and the cymbal striking it once. The bar every performance of anything ends on.
    #[default]
    Held,
    /// No extra bar. The last section plays out and the piece simply stops — what a loop being
    /// exported wants, and nothing else does.
    None,
}

impl Ending {
    /// The name the text format writes.
    pub fn name(self) -> &'static str {
        match self {
            Ending::Held => "held",
            Ending::None => "none",
        }
    }

    /// Reads an ending name, accepting the obvious synonyms.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "held" | "hold" | "tonic" => Ending::Held,
            "none" | "stop" | "off" => Ending::None,
            _ => return None,
        })
    }
}

/// One section of the form.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionSpec {
    /// The name used in `form:` and on the clips.
    pub name: String,
    /// How many bars it lasts.
    pub bars: usize,
    /// Which chart it plays, by name.
    pub chords: String,
    /// How hard the section is played, from 0 to 1.
    pub intensity: f32,
    /// Which parts play; empty means all of them.
    pub parts: Vec<String>,
    /// Semitones to transpose this section by.
    pub transpose: i32,
    /// Beats per minute here, or the song's when `None`.
    ///
    /// A **step**, in force from this section's first bar to whatever changes it next, because a
    /// section is the shortest thing this format can talk about and a step is the only shape that
    /// fits it. Slowing *through* a passage — the ritardando an orchestra ends on — is a
    /// continuous change, and neither this nor
    /// [`TempoMap`](auris_core::time::TempoMap), which is piecewise-constant, can say it. Nothing
    /// here pretends otherwise.
    ///
    /// A property of the section and so of every playing of it: a chorus that lifts to 132 lifts
    /// on both times round, which is what makes it the same chorus.
    pub tempo: Option<f64>,
    /// How this section is arrived at, when it is in a different key from the one before it.
    ///
    /// A property of *this* section that changes the one before, which is the only field here that
    /// reaches outside its own bars. It belongs here anyway: how a key change is prepared is a fact
    /// about the arrival, and a verse that leads into a modulating chorus is not the same eight
    /// bars as one that does not — that difference is what a lead-in *is*.
    pub lead_in: LeadIn,
    /// What this section changes about how particular parts play, by part name.
    ///
    /// Keyed by name and not by position because a roster is reordered by the person editing it
    /// and a tweak pointing at "the third part" would follow the move to somewhere it means
    /// nothing. A name that no part answers to is a mistake the format reports.
    pub tweaks: BTreeMap<String, PartTweak>,
}

impl SectionSpec {
    /// A section with the defaults its name implies.
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        let intensity = match name.as_str() {
            "intro" => 0.30,
            "verse" => 0.55,
            "pre" => 0.70,
            "chorus" => 0.90,
            "bridge" => 0.60,
            "break" => 0.35,
            "solo" => 0.75,
            "outro" => 0.25,
            _ => 0.60,
        };
        Self {
            name,
            bars: 8,
            chords: "main".to_string(),
            intensity,
            parts: Vec::new(),
            transpose: 0,
            tempo: None,
            lead_in: LeadIn::default(),
            tweaks: BTreeMap::new(),
        }
    }
}

/// A whole song, as asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct SongSpec {
    /// What the piece is called.
    pub title: String,
    /// Beats per minute.
    pub tempo: f64,
    /// The time signature.
    pub meter: TimeSignature,
    /// The key everything is measured from.
    pub key: Key,
    /// How the piece should feel.
    pub mood: Mood,
    /// The seed every random decision is drawn from.
    pub seed: u64,
    /// How much the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander, from 0 for a machine to 1 for a sloppy band.
    ///
    /// The timing half is a *time* and not a number of ticks: at 1 a pitched note lands within a
    /// standard deviation of six milliseconds of where it was written, and at the default of
    /// 0.35 within about two, at whatever tempo the piece is played. That is what makes one
    /// setting mean one thing — the same dial used to read as a slight looseness at 148 BPM and as
    /// nobody being together at 64, because the wander was a fraction of a beat and a beat is not
    /// a fixed length of time.
    ///
    /// It scales the whole way down, so a small setting is a small wander rather than the first
    /// step of a staircase.
    ///
    /// The kit is exempt from the wander and only from the wander. A drummer holding the time is
    /// what the rest of the band is loose *against*, so the kick, the snare and the hat land
    /// exactly where they were written; what they keep is their constant lean — the hat a little
    /// early, the snare a little late, by the same amount in every bar — and how much this dial
    /// varies the strength of the stroke.
    pub humanize: f32,
    /// How far apart the hardest and softest notes are struck, from 0 to 1.
    ///
    /// How much the playing varies, where [`Self::mood`]'s energy says how hard it is played at
    /// all. At 0 every note is struck alike — a sequencer, which is sometimes the point.
    ///
    /// The top of the dial is narrower than the vocabulary it scales: a ghost note is written at
    /// a little over half strength and played at nine tenths of a normal one, so a part's strokes
    /// sit within about a tenth of their own level rather than spanning half of it. Playing every
    /// written difference in full sounded like a band that could not strike two notes alike, and
    /// most of all on a kit, where every stroke is otherwise the same sound.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill, from 0 to 1.
    pub fill: f32,
    /// How much a repeat departs from what the section played the first time.
    ///
    /// At 0 a second chorus is note for note the first one, which is what makes it recognisable
    /// as the same chorus. At 1 every playing is written afresh, which is what the composer used
    /// to do always — the result had no repetition anywhere in it and so nothing to remember.
    /// The default leaves most of the material alone and rewrites the occasional bar.
    pub variation: f32,
    /// The drum groove.
    pub groove: String,
    /// The tune's contour, given rather than invented: scale steps around the figure's anchor.
    ///
    /// Empty means the composer draws its own germ from the seed. Given, this *is* the germ —
    /// the line every section's melody wears, re-sampled onto each section's own rhythm — so a
    /// motif hummed into four numbers is restated by the whole piece. Only the line: what the
    /// piece takes from a motif is which way it moves, and the rhythm a section says it in
    /// remains the section's business (a part's `rhythm` pattern pins that half by hand).
    pub motif: Vec<i32>,
    /// How the piece closes: a held tonic bar after the last section, or nothing at all.
    pub ending: Ending,
    /// The charts, by name. `main` is the one a section gets when it does not say.
    pub charts: BTreeMap<String, Chart>,
    /// The sections, by name.
    pub sections: BTreeMap<String, SectionSpec>,
    /// The order the sections play in.
    pub form: Vec<String>,
    /// The parts, in the order their tracks are created.
    pub parts: Vec<PartSpec>,
}

impl Default for SongSpec {
    fn default() -> Self {
        let mut charts = BTreeMap::new();
        // Marked generated, not quoted: a progression the user did not ask for is the composer's
        // own, so the mood is free to colour it. A chart anyone typed or named is left alone.
        let default_chart = Chart::parse("@axis")
            .map(|chart| Chart::new(chart.bars, ChartOrigin::Generated))
            .unwrap_or_else(|| Chart::new(Vec::new(), ChartOrigin::Generated));
        charts.insert("main".to_string(), default_chart);
        let mut sections = BTreeMap::new();
        for name in ["intro", "verse", "chorus", "outro"] {
            sections.insert(name.to_string(), SectionSpec::named(name));
        }
        Self {
            title: "Untitled".to_string(),
            tempo: 120.0,
            meter: TimeSignature::default(),
            key: Key::parse("C major").expect("C major is a key"),
            mood: Mood::default(),
            seed: 0,
            swing: 50,
            humanize: 0.35,
            dynamics: 1.0,
            fill: 0.5,
            variation: 0.25,
            groove: "basic-rock".to_string(),
            motif: Vec::new(),
            ending: Ending::default(),
            charts,
            sections,
            form: ["intro", "verse", "chorus", "verse", "chorus", "outro"]
                .iter()
                .map(|name| name.to_string())
                .collect(),
            parts: vec![
                PartSpec::of_role("lead", Role::Melody),
                PartSpec::of_role("chords", Role::Chords),
                PartSpec::of_role("bass", Role::Bass),
                PartSpec::of_role("kick", Role::Kick),
                PartSpec::of_role("snare", Role::Snare),
                PartSpec::of_role("hat", Role::Hat),
            ],
        }
    }
}

/// How long the whole piece is, in bars.
impl SongSpec {
    /// Total length in bars.
    pub fn total_bars(&self) -> usize {
        self.form
            .iter()
            .filter_map(|name| self.sections.get(name))
            .map(|section| section.bars)
            .sum()
    }

    /// The chart a section plays, falling back to `main` and then to anything at all.
    ///
    /// An [unwritten](Chart::is_unwritten) chart is resolved *here*, into the progression the
    /// composer invents for it, so every caller — the planner, a picker showing what a section
    /// will play, a clip being re-taken — sees the same real bars and none of them has to know
    /// the marker exists. The invention is drawn from the song's seed and the chart's own name,
    /// which is what makes two sections naming one unwritten chart play one progression.
    pub fn chart_for(&self, section: &SectionSpec) -> Chart {
        let named = self
            .charts
            .get_key_value(&section.chords)
            .or_else(|| self.charts.get_key_value("main"))
            .or_else(|| self.charts.iter().next());
        match named {
            Some((name, chart)) if chart.is_unwritten() => {
                crate::progression::invent_chart(self.seed, name, self.key, self.mood)
            }
            Some((_, chart)) => chart.clone(),
            None => Chart::new(Vec::new(), ChartOrigin::Given),
        }
    }

    /// The tempo a section is played at: its own, or the song's.
    pub fn tempo_of(&self, section: &SectionSpec) -> f64 {
        section.tempo.unwrap_or(self.tempo)
    }

    /// The parts that play in a section.
    pub fn parts_in(&self, section: &SectionSpec) -> Vec<&PartSpec> {
        self.parts
            .iter()
            .filter(|part| section.parts.is_empty() || section.parts.contains(&part.name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_roster_is_mixed_rather_than_stacked() {
        // Six parts at unity sum past full scale; the defaults have to leave headroom.
        let spec = SongSpec::default();
        assert!(spec.parts.iter().all(|part| part.gain_db < 0.0));
        let lead = spec.parts.iter().find(|p| p.role == Role::Melody).unwrap();
        let hat = spec.parts.iter().find(|p| p.role == Role::Hat).unwrap();
        assert!(
            hat.gain_db < lead.gain_db,
            "the hat should sit under the tune"
        );
    }

    #[test]
    fn a_section_intensity_follows_its_name() {
        let spec = SongSpec::parse("form = [\"intro\", \"chorus\", \"outro\"]").unwrap();
        assert!(spec.sections["intro"].intensity < spec.sections["chorus"].intensity);
        assert!(spec.sections["outro"].intensity < spec.sections["chorus"].intensity);
    }

    #[test]
    fn a_program_is_a_kit_on_a_drum_part_and_a_program_on_every_other() {
        // The same number means two unrelated things in General MIDI, and which one it means is
        // never a guess: the role has already said. Read wrongly, a violin part would play
        // whatever kit patch 40 is and a snare part would play a violin.
        let violin = PartSpec {
            program: Some(gm::Program(40)),
            ..PartSpec::of_role("lead", Role::Melody)
        };
        assert_eq!(violin.sound(), Some(gm::Sound { bank: 0, patch: 40 }));

        let brushes = PartSpec {
            program: Some(gm::Program(40)),
            ..PartSpec::of_role("snare", Role::Snare)
        };
        assert_eq!(
            brushes.sound(),
            Some(gm::Sound {
                bank: gm::DRUM_BANK,
                patch: 40
            })
        );

        // And a part that named none stays on its plugin, which is what keeps a piece written on
        // the built-in voices written on them.
        assert_eq!(PartSpec::of_role("lead", Role::Melody).sound(), None);
    }

    #[test]
    fn a_section_patches_a_part_rather_than_redeclaring_it() {
        // A tweak is a patch and not a second declaration: what it does not name it does not
        // touch. The whole point of the shape — a busier chorus is one line, and adding a field
        // to a part does not silently reset it in every section that tweaks one.
        let lead = PartSpec {
            density: Some(0.4),
            gate: 0.8,
            octave: 5,
            ..PartSpec::of_role("lead", Role::Melody)
        };
        let tweak = PartTweak {
            density: Some(0.9),
            ..PartTweak::default()
        };
        let played = tweak.applied_to(&lead);
        assert_eq!(played.density, Some(0.9));
        assert_eq!(played.gate, 0.8, "a field the tweak did not name");
        assert_eq!(played.octave, 5);

        // And the identity of the part is never a patch: those are what its *track* is, and a
        // track is one row, one instrument and one fader for the whole song.
        assert_eq!(played.name, lead.name);
        assert_eq!(played.role, lead.role);
        assert_eq!(played.instrument, lead.instrument);
        assert_eq!(played.gain_db, lead.gain_db);
        assert_eq!(played.pan, lead.pan);

        assert!(PartTweak::default().is_empty());
        assert_eq!(PartTweak::default().applied_to(&lead), lead);
    }

    #[test]
    fn a_section_can_name_which_parts_play() {
        let spec = SongSpec::parse(
            r#"
            form = ["intro", "chorus"]

            [section.intro]
            parts = ["bass", "kick"]
            "#,
        )
        .unwrap();
        let intro = &spec.sections["intro"];
        let playing: Vec<&str> = spec
            .parts_in(intro)
            .iter()
            .map(|part| part.name.as_str())
            .collect();
        assert_eq!(playing, ["bass", "kick"]);

        let chorus = &spec.sections["chorus"];
        assert_eq!(
            spec.parts_in(chorus).len(),
            spec.parts.len(),
            "a section that does not say plays everything"
        );
    }
}
