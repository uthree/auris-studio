//! The song sheet: a whole piece asked for with dials rather than with a file.
//!
//! Everything in this half of the module is a decision, not a picture. A gpui view cannot be
//! unit-tested, so what the sheet *decides* — these dials to a [`SongSpec`] — lives in free
//! functions with tests, and the view does nothing but draw them and hand back what was moved.
//!
//! The sheet and `.asong` are two faces of one type. [`song_spec`] builds the specification, and
//! the specification is what writes the piece — so there is no second implementation of what a
//! dial means, and the round trip through [`SongSpec::to_toml`] is a test they share.

use auris_i18n::Key;
use auris_session::prelude::*;

/// How many bars a section may be asked for, either way.
///
/// Four is a phrase; thirty-two is four of them, which is longer than any one section of a song
/// that has more than one. The dial covers what the form is for and nothing beyond it.
pub const BARS: std::ops::RangeInclusive<usize> = 4..=32;

/// The tempo range the specification accepts, which is what the dial has to cover.
pub const TEMPO: std::ops::RangeInclusive<f64> = 40.0..=220.0;

/// Straight to as far as a swing dial goes, in the percentage a shuffle is written in.
pub const SWING: std::ops::RangeInclusive<u8> = 50..=75;

/// How far a part's level trim reaches, in decibels.
pub const GAIN_DB: std::ops::RangeInclusive<f32> = -30.0..=0.0;

/// How far a drag travels before a dial has been turned end to end.
const DRAG_RANGE_PIXELS: f32 = 220.0;

/// The section names a form is offered, in the order a song usually reaches them.
///
/// A vocabulary rather than a rule: any name at all is legal in the file, and these are the ones
/// that save somebody typing `chorus` for the hundredth time. `pre` is the pre-chorus and `drop`
/// is what dance music calls the same arrival by a different name.
pub const SECTION_NAMES: [&str; 8] = [
    "intro", "verse", "pre", "chorus", "bridge", "drop", "solo", "outro",
];

/// How far a section may be moved from the key, in semitones.
///
/// Not a continuous dial. A modulation is a *choice from a short list* — up a tone into a last
/// chorus, up a semitone for the same trick with less warning, down a third for a quiet reprise —
/// and the numbers in between are ones nobody reaches for on purpose.
pub const TRANSPOSES: [i32; 9] = [-5, -3, -2, -1, 0, 1, 2, 3, 5];

/// The General MIDI percussion kit, which is what a drum part's note picker offers.
///
/// Thirty-five to fifty-nine, which is the kit proper. Everything above it is hand percussion
/// rather than anything a kick, a snare or a hat would be pointed at, and a menu of a hundred and
/// twenty-eight numbers is a menu nobody reads. A font that puts its kick somewhere else entirely
/// is what writing `note = 12` in the file is for.
pub const DRUM_NOTES: [(u8, &str); 25] = [
    (35, "Acoustic Bass Drum"),
    (36, "Bass Drum 1"),
    (37, "Side Stick"),
    (38, "Acoustic Snare"),
    (39, "Hand Clap"),
    (40, "Electric Snare"),
    (41, "Low Floor Tom"),
    (42, "Closed Hi-Hat"),
    (43, "High Floor Tom"),
    (44, "Pedal Hi-Hat"),
    (45, "Low Tom"),
    (46, "Open Hi-Hat"),
    (47, "Low-Mid Tom"),
    (48, "Hi-Mid Tom"),
    (49, "Crash Cymbal 1"),
    (50, "High Tom"),
    (51, "Ride Cymbal 1"),
    (52, "Chinese Cymbal"),
    (53, "Ride Bell"),
    (54, "Tambourine"),
    (55, "Splash Cymbal"),
    (56, "Cowbell"),
    (57, "Crash Cymbal 2"),
    (58, "Vibraslap"),
    (59, "Ride Cymbal 2"),
];

/// How a drum note is written where there is room for its name.
///
/// The General MIDI names, left in English on purpose: they are the names printed on the pads of
/// every drum machine and listed in every font's documentation, and translating them would make
/// the picker and the thing being pointed at disagree.
pub fn drum_note_label(note: u8) -> String {
    match DRUM_NOTES.iter().find(|(number, _)| *number == note) {
        Some((_, name)) => format!("{note} · {name}"),
        None => note.to_string(),
    }
}

/// How a transposition is written on its button.
pub fn transpose_label(steps: i32) -> String {
    match steps {
        0 => "±0".to_string(),
        up if up > 0 => format!("+{up}"),
        down => down.to_string(),
    }
}

/// The chart every section plays unless it says otherwise.
///
/// The specification's own word for it, and the reason the sheet always carries one: a section
/// pointed at a name nothing answers to is a document its parser refuses.
pub const MAIN_CHART: &str = "main";

/// Everything the sheet is set to.
///
/// The specification's own fields, held one for one, so that reading the sheet is reading the
/// document — with two of them turned inside out. `sections` and `charts` are ordered lists here
/// and maps there, because **a list is what a person edits**: renaming a section in a `BTreeMap`
/// would slide its row somewhere else in the panel while the pointer was still on it.
#[derive(Clone, Debug, PartialEq)]
pub struct SongDials {
    /// What the piece is called, and what the project is named after.
    pub title: String,
    /// The key everything is measured from.
    pub key: MusicalKey,
    /// Beats per minute.
    pub tempo: f64,
    /// The time signature.
    pub meter: TimeSignature,
    /// How the piece should feel.
    pub mood: Mood,
    /// The drum groove.
    pub groove: String,
    /// The seed every random decision is drawn from.
    pub seed: u64,
    /// How much the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander.
    pub humanize: f32,
    /// How far apart the hardest and softest notes are struck.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill.
    pub fill: f32,
    /// How much a repeat departs from what came before it.
    pub variation: f32,
    /// The progressions the song carries, [`MAIN_CHART`] first.
    ///
    /// More than one is what lets a chorus play something the verse does not. They arrive by
    /// being *chosen* — picking a catalogue progression for a section adds it here under its own
    /// name — because a list somebody has to build before they can use it is a screen standing
    /// between them and the thing they wanted.
    pub charts: Vec<(String, Chart)>,
    /// The sections, in the order the form first reaches them.
    pub sections: Vec<SectionSpec>,
    /// The order the sections play in, by name. A name may appear more than once.
    pub form: Vec<String>,
    /// The roster, in the order the tracks are created.
    pub parts: Vec<PartSpec>,
}

impl Default for SongDials {
    /// The song `SongSpec::default()` describes.
    ///
    /// Read *through* the specification rather than written out again here: two lists of defaults
    /// would drift, and the one that drifted would be the one nobody reads — a dialog that opens
    /// on a different song from `auris compose` with no file.
    fn default() -> Self {
        song_dials(&SongSpec::default())
    }
}

/// The specification these dials describe.
///
/// The one place the sheet turns into a song. Everything downstream — writing the piece, saving
/// the `.asong`, refilling the sheet from one — goes through the specification, so a dial cannot
/// mean one thing to the composer and another to the file.
pub fn song_spec(dials: &SongDials) -> SongSpec {
    SongSpec {
        title: dials.title.clone(),
        key: dials.key,
        tempo: dials.tempo,
        meter: dials.meter,
        mood: dials.mood,
        seed: dials.seed,
        swing: dials.swing,
        humanize: dials.humanize,
        dynamics: dials.dynamics,
        fill: dials.fill,
        variation: dials.variation,
        groove: dials.groove.clone(),
        charts: dials.charts.iter().cloned().collect(),
        sections: dials
            .sections
            .iter()
            .map(|section| (section.name.clone(), section.clone()))
            .collect(),
        form: dials.form.clone(),
        parts: dials.parts.clone(),
    }
}

/// The dials a specification sets, which is [`song_spec`] the other way round.
///
/// What makes the round trip hold is that this *normalises*: [`MAIN_CHART`] always exists and is
/// always first, and the sections come out in the order the form reaches them with any the form
/// never names after. Every list the sheet can produce is already in that shape, because every
/// gesture it offers preserves it.
pub fn song_dials(spec: &SongSpec) -> SongDials {
    let mut charts: Vec<(String, Chart)> = Vec::new();
    if let Some(main) = spec.charts.get(MAIN_CHART) {
        charts.push((MAIN_CHART.to_string(), main.clone()));
    }
    charts.extend(
        spec.charts
            .iter()
            .filter(|(name, _)| name.as_str() != MAIN_CHART)
            .map(|(name, chart)| (name.clone(), chart.clone())),
    );

    let mut sections: Vec<SectionSpec> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for name in spec.form.iter().chain(spec.sections.keys()) {
        if seen.contains(&name.as_str()) {
            continue;
        }
        let Some(section) = spec.sections.get(name) else {
            continue;
        };
        seen.push(name);
        sections.push(section.clone());
    }

    SongDials {
        title: spec.title.clone(),
        key: spec.key,
        tempo: spec.tempo,
        meter: spec.meter,
        mood: spec.mood,
        groove: spec.groove.clone(),
        seed: spec.seed,
        swing: spec.swing,
        humanize: spec.humanize,
        dynamics: spec.dynamics,
        fill: spec.fill,
        variation: spec.variation,
        charts,
        sections,
        form: spec.form.clone(),
        parts: spec.parts.clone(),
    }
}

/// The catalogue chart a name asks for, or `None` for the composer's own.
pub fn chart_named(name: &str) -> Option<Chart> {
    let name = name.trim();
    (!name.is_empty()).then(|| Chart::parse(&format!("@{name}")))?
}

// ---------------------------------------------------------------- the form

/// Where in [`SongDials::sections`] the section played at `place` in the form is defined.
///
/// A name may be played more than once and there is one definition behind all of them — which is
/// the whole point of a form. Editing the second chorus edits the first, because they are the
/// same chorus.
pub fn section_at(dials: &SongDials, place: usize) -> Option<usize> {
    let name = dials.form.get(place)?;
    dials
        .sections
        .iter()
        .position(|section| &section.name == name)
}

/// A section name nothing in the song is using yet.
pub fn unused_section_name(dials: &SongDials, stem: &str) -> String {
    if !dials.sections.iter().any(|section| section.name == stem) {
        return stem.to_string();
    }
    (2..)
        .map(|n| format!("{stem} {n}"))
        .find(|name| !dials.sections.iter().any(|section| &section.name == name))
        .unwrap_or_else(|| stem.to_string())
}

/// Adds a playing of a section after `place`, defining the section if it is a new name.
///
/// A name the song already knows is a *repeat*, and repeats are what a form is made of — so
/// choosing an existing name adds a place in the order and no second definition.
pub fn add_to_form(dials: &mut SongDials, place: usize, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if !dials.sections.iter().any(|section| section.name == name) {
        dials.sections.push(SectionSpec::named(name));
    }
    let at = (place + 1).min(dials.form.len());
    dials.form.insert(at, name.to_string());
    tidy_sections(dials);
}

/// Points the playing at `place` at a different section, defining it if it is a new name.
pub fn set_form_entry(dials: &mut SongDials, place: usize, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || place >= dials.form.len() {
        return false;
    }
    if !dials.sections.iter().any(|section| section.name == name) {
        dials.sections.push(SectionSpec::named(name));
    }
    dials.form[place] = name.to_string();
    tidy_sections(dials);
    true
}

/// Removes the playing at `place`, if the form would still have one.
///
/// A form of nothing writes nothing, and a specification says so — the button goes dead rather
/// than Write producing a document the parser refuses.
pub fn remove_from_form(dials: &mut SongDials, place: usize) -> bool {
    if dials.form.len() <= 1 || place >= dials.form.len() {
        return false;
    }
    dials.form.remove(place);
    tidy_sections(dials);
    true
}

/// Moves the playing at `place` one step earlier or later.
pub fn move_in_form(dials: &mut SongDials, place: usize, later: bool) -> bool {
    let to = match later {
        true if place + 1 < dials.form.len() => place + 1,
        false if place > 0 => place - 1,
        _ => return false,
    };
    dials.form.swap(place, to);
    tidy_sections(dials);
    true
}

/// Puts the section list back into the shape [`song_dials`] produces: only what the form plays,
/// in the order the form first reaches it.
///
/// Called after every change to the form, for two reasons. A section the form no longer plays
/// would otherwise go on contributing a `[section.chorus]` table to the file, describing
/// something that never sounds. And the order is what makes the round trip hold — reading a
/// document back sorts by the form, so a sheet that did not would come back looking different
/// from itself while describing the same song.
///
/// Reordering costs nothing on screen: the form column is drawn from [`SongDials::form`], and
/// this list is storage its rows reach into by name.
fn tidy_sections(dials: &mut SongDials) {
    let mut kept: Vec<SectionSpec> = Vec::new();
    for name in &dials.form {
        if kept.iter().any(|section| &section.name == name) {
            continue;
        }
        if let Some(section) = dials.sections.iter().find(|held| &held.name == name) {
            kept.push(section.clone());
        }
    }
    dials.sections = kept;
}

/// Points the section at `index` at the progression `name`, adding it to the song if it is a
/// catalogue entry the song does not carry yet.
///
/// The one gesture that makes a second progression exist. Choosing 丸サ進行 for the chorus is what
/// makes 丸サ進行 one of the song's charts — there is no list to fill in first.
pub fn set_section_chart(dials: &mut SongDials, index: usize, name: &str) -> bool {
    if dials.charts.iter().any(|(known, _)| known == name) {
        let Some(section) = dials.sections.get_mut(index) else {
            return false;
        };
        section.chords = name.to_string();
        return true;
    }
    match chart_named(name) {
        Some(chart) => give_section_chart(dials, index, name, chart),
        None => false,
    }
}

/// Points the section at `index` at a progression the caller already has, under `name`.
///
/// What a progression written out by hand, or taken from the book somebody keeps, comes in
/// through — neither of which the catalogue can be asked for. Replacing rather than adding when
/// the name is taken, so writing a section's chords twice leaves one chart rather than two.
pub fn give_section_chart(dials: &mut SongDials, index: usize, name: &str, chart: Chart) -> bool {
    let name = name.trim();
    let Some(section) = dials.sections.get_mut(index) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    section.chords = name.to_string();
    match dials.charts.iter_mut().find(|(known, _)| known == name) {
        Some((_, held)) => *held = chart,
        None => dials.charts.push((name.to_string(), chart)),
    }
    true
}

/// How a chart is named on the sheet: the progression it quotes, or its own key.
pub fn chart_label(name: &str, chart: &Chart) -> String {
    chart.quoted_as.clone().unwrap_or_else(|| name.to_string())
}

/// The same song, next take.
///
/// The next seed rather than a random one, for the reason a generated clip's seed is shown and
/// typeable: a take somebody liked has to be reachable again, and it is only reachable by
/// somebody who can count back to it.
pub fn another_take(dials: &mut SongDials) {
    dials.seed = dials.seed.wrapping_add(1);
}

/// A name no part in the roster is using yet.
///
/// `part`, `part 2`, `part 3`: a duplicate name would be refused by the format, and a sheet that
/// added a row the document then rejected would be a button that breaks the song.
pub fn unused_part_name(dials: &SongDials, stem: &str) -> String {
    if !dials.parts.iter().any(|part| part.name == stem) {
        return stem.to_string();
    }
    (2..)
        .map(|n| format!("{stem} {n}"))
        .find(|name| !dials.parts.iter().any(|part| &part.name == name))
        .unwrap_or_else(|| stem.to_string())
}

/// Adds a part of `role`, named after it.
pub fn add_part(dials: &mut SongDials, role: Role) {
    let name = unused_part_name(dials, role.name());
    dials.parts.push(PartSpec::of_role(name, role));
}

/// Removes the part at `index`, if the roster would still have one.
///
/// A song with no parts writes no notes, and a sheet whose Write button produces an empty
/// document is a sheet with a broken state reachable from it.
pub fn remove_part(dials: &mut SongDials, index: usize) -> bool {
    if dials.parts.len() <= 1 || index >= dials.parts.len() {
        return false;
    }
    dials.parts.remove(index);
    true
}

/// The mood word these four numbers are exactly, if any.
///
/// A mood the dials have been nudged away from is no word at all, and the picker says so. Naming
/// the word it started at would be the one caption that is reliably wrong.
pub fn mood_word(mood: Mood) -> Option<&'static str> {
    Mood::NAMES
        .into_iter()
        .find(|name| Mood::named(name) == Some(mood))
}

/// The interface's name for a mood word.
pub fn mood_key(name: &str) -> Key {
    match name {
        "bright" => Key::MoodBright,
        "dark" => Key::MoodDark,
        "calm" => Key::MoodCalm,
        "driving" => Key::MoodDriving,
        "epic" => Key::MoodEpic,
        "dreamy" => Key::MoodDreamy,
        "tense" => Key::MoodTense,
        "funky" => Key::MoodFunky,
        _ => Key::MoodNeutral,
    }
}

/// The interface's name for a part's role.
pub fn role_key(role: Role) -> Key {
    match role {
        Role::Melody => Key::RoleMelody,
        Role::Chords => Key::RoleChords,
        Role::Pad => Key::RolePad,
        Role::Arp => Key::RoleArp,
        Role::Stab => Key::RoleStab,
        Role::Bass => Key::RoleBass,
        Role::Kick => Key::RoleKick,
        Role::Snare => Key::RoleSnare,
        Role::Hat => Key::RoleHat,
    }
}

/// One continuous dial on the song.
///
/// The choices from a set — the key, the meter, the progression, the groove, the seed — are
/// picked from a menu or typed. These are the ones with a range, and so the ones that get a bar
/// to drag.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SongDial {
    /// Beats per minute.
    Tempo,
    /// Dark to bright.
    Brightness,
    /// Calm to driving.
    Energy,
    /// Plain to coloured.
    Tension,
    /// Straight to syncopated.
    Syncopation,
    /// How far the offbeats are delayed.
    Swing,
    /// How far timing and velocity wander.
    Humanize,
    /// How far apart the hardest and softest notes are struck.
    Dynamics,
    /// How much of a section's last bar runs as a fill.
    Fill,
    /// How far a repeat departs from the playing before it.
    Variation,
}

/// The song's dials, in the order they are drawn.
pub const SONG_DIALS: &[SongDial] = &[
    SongDial::Tempo,
    SongDial::Brightness,
    SongDial::Energy,
    SongDial::Tension,
    SongDial::Syncopation,
    SongDial::Swing,
    SongDial::Humanize,
    SongDial::Dynamics,
    SongDial::Fill,
    SongDial::Variation,
];

impl SongDial {
    /// What the row is called.
    pub fn label(self) -> Key {
        match self {
            SongDial::Tempo => Key::Tempo,
            SongDial::Brightness => Key::SongBrightness,
            SongDial::Energy => Key::SongEnergy,
            SongDial::Tension => Key::SongTension,
            SongDial::Syncopation => Key::PartSyncopation,
            SongDial::Swing => Key::PartSwing,
            SongDial::Humanize => Key::PartHumanize,
            SongDial::Dynamics => Key::PartDynamics,
            SongDial::Fill => Key::PartFill,
            SongDial::Variation => Key::SongVariation,
        }
    }

    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, dials: &SongDials) -> f32 {
        match self {
            SongDial::Tempo => between(
                dials.tempo as f32,
                *TEMPO.start() as f32,
                *TEMPO.end() as f32,
            ),
            SongDial::Brightness => dials.mood.brightness,
            SongDial::Energy => dials.mood.energy,
            SongDial::Tension => dials.mood.tension,
            SongDial::Syncopation => dials.mood.syncopation,
            SongDial::Swing => between(
                f32::from(dials.swing),
                f32::from(*SWING.start()),
                f32::from(*SWING.end()),
            ),
            SongDial::Humanize => dials.humanize,
            SongDial::Dynamics => dials.dynamics,
            SongDial::Fill => dials.fill,
            SongDial::Variation => dials.variation,
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, dials: &mut SongDials, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            SongDial::Tempo => {
                // To the nearest whole beat: a tempo of 128.37 is a number nobody chose, and the
                // dial has more pixels than the range has useful values.
                let bpm = lerp(fraction, *TEMPO.start() as f32, *TEMPO.end() as f32);
                dials.tempo = f64::from(bpm.round());
            }
            SongDial::Brightness => dials.mood.brightness = fraction,
            SongDial::Energy => dials.mood.energy = fraction,
            SongDial::Tension => dials.mood.tension = fraction,
            SongDial::Syncopation => dials.mood.syncopation = fraction,
            SongDial::Swing => {
                let swing = lerp(fraction, f32::from(*SWING.start()), f32::from(*SWING.end()));
                dials.swing = swing.round() as u8;
            }
            SongDial::Humanize => dials.humanize = fraction,
            SongDial::Dynamics => dials.dynamics = fraction,
            SongDial::Fill => dials.fill = fraction,
            SongDial::Variation => dials.variation = fraction,
        }
    }

    /// What the readout at the end of the bar says.
    pub fn text(self, dials: &SongDials) -> String {
        match self {
            SongDial::Tempo => format!("{:.0}", dials.tempo),
            SongDial::Swing if dials.swing == 50 => "50".to_string(),
            SongDial::Swing => dials.swing.to_string(),
            other => percent(other.fraction(dials)),
        }
    }
}

/// One continuous dial on a section of the form.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionDial {
    /// How many bars it lasts.
    Bars,
    /// How hard it is played.
    Intensity,
}

/// A section's dials, in the order they are drawn.
pub const SECTION_DIALS: &[SectionDial] = &[SectionDial::Bars, SectionDial::Intensity];

impl SectionDial {
    /// What the row is called.
    pub fn label(self) -> Key {
        match self {
            SectionDial::Bars => Key::SongBars,
            SectionDial::Intensity => Key::PartIntensity,
        }
    }

    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, section: &SectionSpec) -> f32 {
        match self {
            SectionDial::Bars => between(
                section.bars as f32,
                *BARS.start() as f32,
                *BARS.end() as f32,
            ),
            SectionDial::Intensity => section.intensity,
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, section: &mut SectionSpec, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            SectionDial::Bars => {
                let bars = lerp(fraction, *BARS.start() as f32, *BARS.end() as f32);
                section.bars = (bars.round() as usize).clamp(*BARS.start(), *BARS.end());
            }
            SectionDial::Intensity => section.intensity = fraction,
        }
    }

    /// What the readout at the end of the bar says.
    pub fn text(self, section: &SectionSpec) -> String {
        match self {
            SectionDial::Bars => section.bars.to_string(),
            other => percent(other.fraction(section)),
        }
    }
}

/// One continuous dial on a part.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PartDial {
    /// How busy the part is, as a share of the available steps.
    Density,
    /// How long a note is held.
    Gate,
    /// Level trim.
    Gain,
    /// Stereo position.
    Pan,
}

/// A part's dials, in the order they are drawn.
pub const PART_DIALS: &[PartDial] = &[
    PartDial::Density,
    PartDial::Gate,
    PartDial::Gain,
    PartDial::Pan,
];

impl PartDial {
    /// What the row is called.
    pub fn label(self) -> Key {
        match self {
            PartDial::Density => Key::PartDensity,
            PartDial::Gate => Key::PartGate,
            PartDial::Gain => Key::SongPartGain,
            PartDial::Pan => Key::SongPartPan,
        }
    }

    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, part: &PartSpec) -> f32 {
        match self {
            // A part that says nothing about its density is drawn where the mood would put it,
            // which is the middle of the dial: the bar has to start somewhere, and starting at
            // the floor would say the part is silent.
            PartDial::Density => part.density.unwrap_or(0.5),
            PartDial::Gate => part.gate,
            PartDial::Gain => between(part.gain_db, *GAIN_DB.start(), *GAIN_DB.end()),
            PartDial::Pan => (part.pan + 1.0) / 2.0,
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, part: &mut PartSpec, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            PartDial::Density => part.density = Some(fraction),
            // Never zero: a note of no length is a note nobody hears, and a dial whose bottom end
            // silences the part is a dial with a broken position on it.
            PartDial::Gate => part.gate = fraction.max(0.05),
            PartDial::Gain => {
                part.gain_db = lerp(fraction, *GAIN_DB.start(), *GAIN_DB.end());
            }
            PartDial::Pan => part.pan = fraction * 2.0 - 1.0,
        }
    }

    /// Whether a bar grows from the middle rather than from the left.
    pub fn is_centred(self) -> bool {
        matches!(self, PartDial::Pan)
    }

    /// What the readout at the end of the bar says.
    pub fn text(self, part: &PartSpec) -> String {
        match self {
            PartDial::Gain => format!("{:.1}", part.gain_db),
            PartDial::Pan if part.pan.abs() < 0.005 => "C".to_string(),
            PartDial::Pan if part.pan < 0.0 => format!("L{:.0}", part.pan.abs() * 100.0),
            PartDial::Pan => format!("R{:.0}", part.pan * 100.0),
            other => percent(other.fraction(part)),
        }
    }
}

/// Where a drag that began at `start` and has travelled `delta` pixels puts a dial.
pub fn dragged(start: f32, delta: f32) -> f32 {
    (start + delta / DRAG_RANGE_PIXELS).clamp(0.0, 1.0)
}

/// Where `value` sits between `low` and `high`, from 0 to 1.
fn between(value: f32, low: f32, high: f32) -> f32 {
    ((value - low) / (high - low).max(f32::EPSILON)).clamp(0.0, 1.0)
}

/// The value `fraction` of the way from `low` to `high`.
fn lerp(fraction: f32, low: f32, high: f32) -> f32 {
    low + (high - low) * fraction.clamp(0.0, 1.0)
}

/// A fraction as a whole percent, which is the resolution the readout has.
fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).round())
}

/// Which dial a drag is turning.
///
/// The song's dials and a part's are one gesture with two targets, so the drag state names both
/// rather than there being two of everything from the pointer down.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DialTarget {
    /// One of the song's own.
    Song(SongDial),
    /// One belonging to the section at this position in [`SongDials::sections`].
    Section(usize, SectionDial),
    /// One belonging to the part at this position in the roster.
    Part(usize, PartDial),
}

impl DialTarget {
    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, dials: &SongDials) -> f32 {
        match self {
            DialTarget::Song(dial) => dial.fraction(dials),
            DialTarget::Section(index, dial) => dials
                .sections
                .get(index)
                .map_or(0.0, |section| dial.fraction(section)),
            DialTarget::Part(index, dial) => dials
                .parts
                .get(index)
                .map_or(0.0, |part| dial.fraction(part)),
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, dials: &mut SongDials, fraction: f32) {
        match self {
            DialTarget::Song(dial) => dial.set(dials, fraction),
            DialTarget::Section(index, dial) => {
                if let Some(section) = dials.sections.get_mut(index) {
                    dial.set(section, fraction);
                }
            }
            DialTarget::Part(index, dial) => {
                if let Some(part) = dials.parts.get_mut(index) {
                    dial.set(part, fraction);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- the view

use gpui::{
    AnyElement, Context, IntoElement, MouseDownEvent, Window, div, prelude::*, px, relative,
};

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{ButtonStyle, SliderFill, button, divider, value_slider};

/// How wide the label at the start of a row is drawn.
const LABEL_WIDTH: f32 = 116.0;

impl AurisApp {
    /// Opens the song sheet: on the song it was last set to, on the one the document was written
    /// from, or on the default one.
    ///
    /// In that order, and the middle one is the point. A piece composed, saved and reopened used
    /// to come back to a sheet full of defaults — Another Take on it would have written a
    /// different song rather than another take of that one.
    pub(crate) fn open_song_sheet(&mut self) {
        if self.song_sheet.is_some() {
            return;
        }
        // A document written by a build that spelled something differently is not an error worth
        // a dialog: the sheet opens on its defaults, which is where it opened before any of this.
        let remembered = self
            .project()
            .song_spec
            .as_deref()
            .and_then(|text| SongSpec::parse(text).ok())
            .map(|spec| song_dials(&spec));
        self.song_sheet = Some(remembered.unwrap_or_default());
    }

    /// The song sheet, or nothing when it is closed.
    ///
    /// A full-screen panel rather than a [`Prompt`]: a prompt asks for one value, and this asks
    /// for a song. It occludes for the same reason the export overlay does — every click behind
    /// a dimmed screen used to land on the arrangement.
    pub(crate) fn render_song_sheet(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dials = self.song_sheet.clone()?;
        let theme = self.theme.clone();
        let spec = song_spec(&dials);
        let length = format!(
            "{} · {} {}",
            self.t(Key::SongLength),
            spec.total_bars(),
            self.t(Key::SongBarsUnit)
        );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(1120.0))
                        .max_h(relative(0.92))
                        .p_4()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::SongSheetTitle)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_4()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("song-sheet-song")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .w(px(320.0))
                                        .overflow_y_scroll()
                                        .children(self.song_rows(&dials, cx)),
                                )
                                .child(
                                    div()
                                        .id("song-sheet-form")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .w(px(330.0))
                                        .overflow_y_scroll()
                                        .children(self.song_form_rows(&dials, cx)),
                                )
                                .child(
                                    div()
                                        .id("song-sheet-parts")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_y_scroll()
                                        .children(self.song_part_rows(&dials, cx)),
                                ),
                        )
                        .child(divider(&theme))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(length),
                                )
                                .child(button(
                                    "song-sheet-cancel",
                                    self.t(Key::Cancel),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.song_sheet = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-save",
                                    self.t(Key::SongSaveSpec),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.save_song_specification(window, cx);
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-take",
                                    self.t(Key::SongAnotherTake),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(dials) = this.song_sheet.as_mut() {
                                            another_take(dials);
                                        }
                                        this.write_song_from_sheet();
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-write",
                                    self.t(Key::SongWrite),
                                    ButtonStyle::Primary,
                                    false,
                                    theme.accent,
                                    &theme,
                                    // Write closes the sheet and Another Take does not: one is
                                    // "this is the song", the other is "not that one, again".
                                    cx.listener(|this, _, _, cx| {
                                        this.write_song_from_sheet();
                                        this.song_sheet = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// The left half: everything about the song that is not a part.
    fn song_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows: Vec<AnyElement> =
            vec![self.group_heading(Key::SongHeading).into_any_element()];

        // First, because it is the row that sets every other one. Somebody opening this for the
        // first time is looking at thirty dials and no idea which of them matter; a style is the
        // answer to all of them at once, and what they came here to change is what happens next.
        rows.push(
            self.sheet_picker(
                "song-style",
                Key::SongStyle,
                self.t(Key::SongStyleChoose).to_string(),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_preset_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-title",
                Key::SongTitleField,
                dials.title.clone(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SongTitleField);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.title.clone());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongTitle, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-key",
                Key::SongKey,
                dials.key.to_text(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SongKey);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.key.to_text());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongKey, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-meter",
                Key::SongMeter,
                format!("{}/{}", dials.meter.numerator, dials.meter.denominator),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_meter_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-mood",
                Key::SongMood,
                match mood_word(dials.mood) {
                    Some(name) => this_word(self, name),
                    None => self.t(Key::SongMoodCustom).to_string(),
                },
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_mood_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-groove",
                Key::PartGroove,
                dials.groove.clone(),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_groove_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-seed",
                Key::PartSeed,
                dials.seed.to_string(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::PartSeed);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.seed.to_string());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongSeed, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );

        for dial in SONG_DIALS {
            let dial = *dial;
            let target = DialTarget::Song(dial);
            let fraction = dial.fraction(dials);
            rows.push(
                value_slider(
                    ("song-dial", dial as usize),
                    self.t(dial.label()),
                    dial.text(dials),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                    cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                        this.nudge_song_dial(target, notches);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }
        rows
    }

    /// The middle: the form, one block per playing of a section.
    ///
    /// One row per *place in the order*, not one per section — a chorus played twice is two rows,
    /// and both of them edit the one chorus, because that is what makes it the same chorus.
    fn song_form_rows(
        &mut self,
        dials: &SongDials,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let removable = dials.form.len() > 1;
        let mut rows: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .child(self.group_heading(Key::SongFormHeading)),
                )
                .child(button(
                    "song-add-section",
                    self.t(Key::SongAddSection),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let places = this
                            .song_sheet
                            .as_ref()
                            .map_or(0, |dials| dials.form.len().saturating_sub(1));
                        let menu = this.song_section_menu(event.position(), places);
                        this.open_menu(menu);
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ];

        for (place, name) in dials.form.iter().enumerate() {
            let Some(index) = section_at(dials, place) else {
                continue;
            };
            let section = &dials.sections[index];
            let chart = dials
                .charts
                .iter()
                .find(|(known, _)| known == &section.chords)
                .map(|(known, chart)| self.progression_name(&chart_label(known, chart)))
                .unwrap_or_else(|| section.chords.clone());

            let mut dial_row = div().flex().gap_2();
            for dial in SECTION_DIALS {
                let dial = *dial;
                let target = DialTarget::Section(index, dial);
                let fraction = dial.fraction(section);
                dial_row = dial_row.child(div().flex_1().min_w_0().child(value_slider(
                    (
                        "song-section-dial",
                        place * SECTION_DIALS.len() + dial as usize,
                    ),
                    self.t(dial.label()),
                    dial.text(section),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                    cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                        this.nudge_song_dial(target, notches);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )));
            }

            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().w(px(84.0)).child(button(
                                ("song-form-name", place),
                                name.clone(),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_form_name_menu(event.position(), place);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(button(
                                ("song-section-chords", place),
                                chart,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_chords_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(44.0)).child(button(
                                ("song-section-transpose", place),
                                transpose_label(section.transpose),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_transpose_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(22.0)).child(button(
                                ("song-form-up", place),
                                "↑",
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        move_in_form(dials, place, false);
                                    }
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(22.0)).child(button(
                                ("song-form-down", place),
                                "↓",
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        move_in_form(dials, place, true);
                                    }
                                    cx.notify();
                                }),
                            )))
                            // The last playing cannot go: a form of nothing writes nothing, and
                            // the specification refuses one rather than composing silence.
                            .child(div().w(px(22.0)).child(button(
                                ("song-form-remove", place),
                                "✕",
                                ButtonStyle::Normal,
                                false,
                                if removable {
                                    theme.danger
                                } else {
                                    theme.border
                                },
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        remove_from_form(dials, place);
                                    }
                                    cx.notify();
                                }),
                            ))),
                    )
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// The right half: one block per part, with a button that adds another.
    fn song_part_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let removable = dials.parts.len() > 1;
        let mut rows: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .child(self.group_heading(Key::SongPartsHeading)),
                )
                .child(button(
                    "song-add-part",
                    self.t(Key::SongAddPart),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                        let menu = this.song_add_part_menu(event.position());
                        this.open_menu(menu);
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ];

        for (index, part) in dials.parts.iter().enumerate() {
            // What the part will be *heard* as: the General MIDI sound where it names one, and
            // otherwise the plugin. Showing the plugin under a part that asked for a violin would
            // name the fallback and never the sound.
            let instrument = match part.program {
                Some(program) => program.label(part.role.is_drum()).to_string(),
                None => self
                    .registry()
                    .instruments()
                    .find(|descriptor| descriptor.id == part.instrument)
                    .map(|descriptor| {
                        auris_i18n::audio::plugin_name(&descriptor.name, self.language())
                            .to_string()
                    })
                    .unwrap_or_else(|| part.instrument.clone()),
            };

            let mut dial_row = div().flex().gap_2();
            for dial in PART_DIALS {
                let dial = *dial;
                let target = DialTarget::Part(index, dial);
                let fraction = dial.fraction(part);
                dial_row = dial_row.child(div().flex_1().min_w_0().child(value_slider(
                    ("song-part-dial", index * PART_DIALS.len() + dial as usize),
                    self.t(dial.label()),
                    dial.text(part),
                    fraction,
                    theme.accent,
                    match dial.is_centred() {
                        true => SliderFill::FromCentre,
                        false => SliderFill::FromStart,
                    },
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                    cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                        this.nudge_song_dial(target, notches);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )));
            }

            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().w(px(96.0)).child(button(
                                ("song-part-name", index),
                                part.name.clone(),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    let title = this.t(Key::SongPartNameTitle);
                                    let current = this.song_sheet.as_ref().map_or_else(
                                        String::new,
                                        |dials| {
                                            dials
                                                .parts
                                                .get(index)
                                                .map_or_else(String::new, |part| part.name.clone())
                                        },
                                    );
                                    this.open_prompt(Prompt::new(
                                        title,
                                        PromptTarget::SongPartName(index),
                                        current,
                                    ));
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(88.0)).child(button(
                                ("song-part-role", index),
                                self.t(role_key(part.role)),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_role_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(button(
                                ("song-part-instrument", index),
                                instrument,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_instrument_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            // A drum part has no octave — its pitches are drum numbers rather
                            // than notes — and it badly needs the *one* number the octave would
                            // have taken the room for, because General MIDI is the only agreement
                            // there is about which number is a kick and a font need not keep it.
                            .child(div().w(px(52.0)).child({
                                let drum = part.drum_note();
                                button(
                                    ("song-part-note", index),
                                    match drum {
                                        Some(note) => note.to_string(),
                                        None => part.octave.to_string(),
                                    },
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                        let menu = match drum.is_some() {
                                            true => this.song_note_menu(event.position(), index),
                                            false => this.song_octave_menu(event.position(), index),
                                        };
                                        this.open_menu(menu);
                                        cx.notify();
                                    }),
                                )
                            }))
                            // The last part cannot go: a song with no parts writes no notes, and
                            // the button goes dead rather than Write producing an empty document.
                            .child(div().w(px(64.0)).child(button(
                                ("song-part-remove", index),
                                self.t(Key::SongRemovePart),
                                ButtonStyle::Normal,
                                false,
                                if removable {
                                    theme.danger
                                } else {
                                    theme.border
                                },
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        remove_part(dials, index);
                                    }
                                    cx.notify();
                                }),
                            ))),
                    )
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// A row with a label at the start and a button holding the value.
    fn sheet_picker<I, F>(
        &self,
        id: I,
        label: Key,
        value: String,
        on_click: F,
    ) -> impl IntoElement + use<I, F>
    where
        I: Into<gpui::ElementId>,
        F: Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(Metrics::CONTROL_HEIGHT)
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .text_xs()
                    .text_color(theme.text_muted)
                    .truncate()
                    .child(self.t(label)),
            )
            .child(div().flex_1().min_w_0().child(button(
                id,
                value,
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                on_click,
            )))
    }

    /// Moves one of the sheet's dials, from a drag.
    pub(crate) fn drag_song_dial(&mut self, target: DialTarget, start_fraction: f32, delta: f32) {
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        target.set(dials, dragged(start_fraction, delta));
    }

    /// Moves one of the sheet's dials by `notches` of a wheel.
    fn nudge_song_dial(&mut self, target: DialTarget, notches: f32) {
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        let next = (target.fraction(dials) + notches * 0.02).clamp(0.0, 1.0);
        target.set(dials, next);
    }

    /// Writes the piece the sheet describes, replacing the document.
    pub(crate) fn write_song_from_sheet(&mut self) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let spec = song_spec(dials);
        self.compose_spec(&spec);
    }

    /// Saves the sheet as a specification file.
    pub(crate) fn save_song_specification(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let text = song_spec(dials).to_toml();
        let name = format!("{}.{}", dials.title, auris_session::SPEC_EXTENSION);
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::SongSaveSpec.get(language))
                .set_file_name(&name)
                .add_filter(
                    Key::FilterSpec.get(language),
                    &[auris_session::SPEC_EXTENSION],
                )
                .save_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let written = std::fs::write(&path, text);
            let _ = this.update(cx, |this, cx| {
                match written {
                    Ok(()) => this.set_status(auris_i18n::messages::saved(
                        this.language(),
                        &path.display().to_string(),
                    )),
                    Err(error) => this.set_failed_status(auris_i18n::messages::failed(
                        this.language(),
                        this.t(Key::SongSaveSpec),
                        &error.to_string(),
                    )),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The meters the sheet offers.
    fn song_meter_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMeter));
        for (numerator, denominator) in [(4, 4), (3, 4), (6, 8), (5, 4), (7, 8), (12, 8)] {
            menu = menu.item(
                format!("{numerator}/{denominator}"),
                MenuCommand::SongMeter(numerator, denominator),
            );
        }
        menu
    }

    /// The named feelings, each of which means four numbers.
    fn song_mood_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMood));
        for name in Mood::NAMES {
            menu = menu.item(self.t(mood_key(name)), MenuCommand::SongMood(name));
        }
        menu
    }

    /// What one section may play: the progressions this song already carries, and every one the
    /// catalogue knows.
    ///
    /// Choosing a catalogue entry the song does not carry adds it, which is the only way a second
    /// progression comes into existence — there is no chart list to fill in first.
    fn song_chords_menu(&self, anchor: gpui::Point<gpui::Pixels>, section: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongChords));
        let carried: Vec<String> = self
            .song_sheet
            .as_ref()
            .map(|dials| {
                dials
                    .charts
                    .iter()
                    .map(|(name, chart)| chart_label(name, chart))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(dials) = self.song_sheet.as_ref() {
            for (name, chart) in &dials.charts {
                menu = menu.item(
                    self.progression_name(&chart_label(name, chart)),
                    MenuCommand::SongSectionChords {
                        section,
                        name: name.clone(),
                    },
                );
            }
        }
        // Writing one out, and keeping the one written. The second only appears where there is
        // something to keep: a section playing a quoted progression already has a name, and
        // offering to file 丸サ進行 under a second one would be a way to end up with two.
        menu = menu.separator();
        menu = menu.item(
            self.t(Key::SongWriteProgression),
            MenuCommand::SongWriteProgression(section),
        );
        if self.section_chart_is_written(section) {
            menu = menu.item(
                self.t(Key::SongKeepProgression),
                MenuCommand::SongKeepProgression(section),
            );
        }

        // The book somebody keeps, then the catalogue that shipped. Theirs first: a person who
        // has written progressions down is reaching for one of those.
        menu = menu.separator();
        for entry in self.progressions.entries() {
            menu = menu.item(
                entry.name.clone(),
                MenuCommand::SongSectionChords {
                    section,
                    name: entry.name.clone(),
                },
            );
        }
        menu = menu.separator();
        for entry in progression_catalog() {
            // Already offered above under the name this song files it under.
            if carried.iter().any(|held| held == entry.name) {
                continue;
            }
            menu = menu.item(
                auris_i18n::audio::theory_description(entry.description, self.language()),
                MenuCommand::SongSectionChords {
                    section,
                    name: entry.name.to_string(),
                },
            );
        }
        menu
    }

    /// Whether the section's progression is one somebody wrote out rather than one it quotes.
    ///
    /// A quotation already has a name and keeping it under a second would be a way to end up with
    /// the same loop twice in one picker.
    fn section_chart_is_written(&self, section: usize) -> bool {
        self.song_sheet
            .as_ref()
            .and_then(|dials| {
                let section = dials.sections.get(section)?;
                let (_, chart) = dials
                    .charts
                    .iter()
                    .find(|(name, _)| name == &section.chords)?;
                Some(chart.quoted_as.is_none())
            })
            .unwrap_or(false)
    }

    /// How far a section is moved from the key, in semitones.
    fn song_transpose_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        section: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongTranspose));
        for steps in TRANSPOSES {
            menu = menu.item(
                transpose_label(steps),
                MenuCommand::SongSectionTranspose { section, steps },
            );
        }
        menu
    }

    /// The sections a place in the form may play: every one the song has, and a new one.
    fn song_form_name_menu(&self, anchor: gpui::Point<gpui::Pixels>, place: usize) -> ContextMenu {
        self.section_menu(anchor, Key::SongSectionName, move |name| {
            MenuCommand::SongFormName {
                place,
                name: name.to_string(),
            }
        })
    }

    /// The same list, for a section being added after `place`.
    fn song_section_menu(&self, anchor: gpui::Point<gpui::Pixels>, place: usize) -> ContextMenu {
        self.section_menu(anchor, Key::SongAddSection, move |name| {
            MenuCommand::SongAddSection {
                place,
                name: name.to_string(),
            }
        })
    }

    /// Every section this song has, then a fresh one of each name it knows.
    ///
    /// Two groups, and the difference between them is the whole of what a form is. Choosing from
    /// the first is a **repeat** — the same chorus again, sharing one definition, which is what
    /// makes it recognisably the same chorus. Choosing from the second makes a *new* section, and
    /// a name already taken comes back numbered: once there is a verse, the second group offers
    /// `verse 2`, which is how a song gets two verses that are not the same eight bars.
    fn section_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        title: Key,
        command: impl Fn(&str) -> MenuCommand,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(title));
        let Some(dials) = self.song_sheet.as_ref() else {
            return menu;
        };
        for section in &dials.sections {
            menu = menu.item(section.name.clone(), command(&section.name));
        }
        menu = menu.separator();
        for stem in SECTION_NAMES {
            let name = unused_section_name(dials, stem);
            menu = menu.item(name.clone(), command(&name));
        }
        menu
    }

    /// Every drum groove the composer knows by name.
    fn song_groove_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartGroove));
        for groove in groove_catalog() {
            menu = menu.item(
                auris_i18n::audio::theory_description(groove.description, self.language()),
                MenuCommand::SongGroove(groove.name),
            );
        }
        menu
    }

    /// The roles a part may take.
    fn song_role_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartRole));
        for role in Role::ALL {
            menu = menu.item(
                self.t(role_key(role)),
                MenuCommand::SongPartRole { part, role },
            );
        }
        menu
    }

    /// A role for a part that does not exist yet.
    fn song_add_part_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongAddPart));
        for role in Role::ALL {
            menu = menu.item(self.t(role_key(role)), MenuCommand::SongAddPart(role));
        }
        menu
    }

    /// Every instrument this build can play.
    /// The whole songs the sheet can be filled from.
    fn song_preset_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongStyle));
        let language = self.language();
        for entry in PRESETS {
            // The description rather than the name: `city-pop` is what a command line takes, and
            // "Electric piano and slap bass over 丸サ進行" is what tells somebody whether it is
            // the one they want.
            menu = menu.item(
                format!(
                    "{} — {}",
                    entry.name,
                    auris_i18n::audio::preset_description(entry.description, language)
                ),
                MenuCommand::SongPreset(entry.name),
            );
        }
        menu
    }

    /// What one part plays: a General MIDI sound, or one of the built-in plugins.
    ///
    /// The programs go in by family rather than all hundred and twenty-eight at once — a menu
    /// that tall does not fit on a screen, and General MIDI already grouped them in eights.
    fn song_instrument_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartInstrument));
        let language = self.language();
        let drums = self
            .song_sheet
            .as_ref()
            .and_then(|dials| dials.parts.get(part))
            .is_some_and(|part| part.role.is_drum());
        if drums {
            // A drum part's number is a whole kit, so there is nothing to group: the eight kits
            // are the list.
            for (patch, name) in gm::KITS {
                menu = menu.item(
                    name,
                    MenuCommand::SongPartProgram {
                        part,
                        program: patch,
                    },
                );
            }
        } else {
            for (family, name) in gm::FAMILIES.iter().enumerate() {
                menu = menu.item(
                    format!("{name}…"),
                    MenuCommand::SongPartFamily {
                        part,
                        family,
                        anchor,
                    },
                );
            }
        }
        menu = menu.separator();
        let mut instruments: Vec<(String, String)> = self
            .registry()
            .instruments()
            .map(|descriptor| {
                (
                    descriptor.id.to_string(),
                    auris_i18n::audio::plugin_name(&descriptor.name, language).to_string(),
                )
            })
            .collect();
        instruments.sort_by(|one, other| one.1.cmp(&other.1));
        for (id, name) in instruments {
            menu = menu.item(name, MenuCommand::SongPartInstrument { part, id });
        }
        menu
    }

    /// The eight sounds of one General MIDI family.
    pub(crate) fn song_program_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        part: usize,
        family: usize,
    ) -> ContextMenu {
        let title = gm::FAMILIES.get(family).copied().unwrap_or_default();
        let mut menu = ContextMenu::new(anchor, title);
        for program in gm::Program::family_programs(family) {
            menu = menu.item(
                program.name(),
                MenuCommand::SongPartProgram {
                    part,
                    program: program.0,
                },
            );
        }
        menu
    }

    /// The notes a drum part may strike.
    fn song_note_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartNote));
        for (note, _) in DRUM_NOTES {
            menu = menu.item(
                drum_note_label(note),
                MenuCommand::SongPartNote { part, note },
            );
        }
        menu
    }

    /// The octaves a part may sit in.
    fn song_octave_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartOctave));
        for octave in 1..=7 {
            menu = menu.item(
                octave.to_string(),
                MenuCommand::SongPartOctave { part, octave },
            );
        }
        menu
    }
}

/// The interface's word for a mood, as a `String` the picker can hold.
fn this_word(app: &AurisApp, name: &str) -> String {
    app.t(mood_key(name)).to_string()
}

impl AurisApp {
    /// What a progression is called in the interface, or its own name if the catalogue has never
    /// heard of it — which is what a chart somebody typed out by hand looks like.
    fn progression_name(&self, name: &str) -> String {
        progression_catalog()
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| {
                auris_i18n::audio::theory_description(entry.description, self.language())
                    .to_string()
            })
            .unwrap_or_else(|| name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet with every kind of thing on it: two progressions, a modulation, a section made
    /// longer than the rest, and a part somebody has adjusted.
    fn peopled() -> SongDials {
        let mut dials = SongDials {
            title: "Neon Drive".to_string(),
            key: MusicalKey::parse("C minor").unwrap(),
            tempo: 124.0,
            meter: TimeSignature::new(3, 4),
            mood: Mood::named("driving").unwrap(),
            groove: "four-on-the-floor".to_string(),
            seed: 7,
            swing: 54,
            humanize: 0.3,
            variation: 0.4,
            ..SongDials::default()
        };
        let chorus = dials
            .sections
            .iter()
            .position(|section| section.name == "chorus")
            .expect("the default form has a chorus");
        set_section_chart(&mut dials, chorus, "marusa");
        dials.sections[chorus].transpose = 2;
        dials.sections[chorus].bars = 16;
        dials.parts[0].gain_db = -3.5;
        dials.parts[0].pan = -0.4;
        dials.parts[1].density = Some(0.65);
        dials
    }

    #[test]
    fn the_sheet_opens_on_the_song_the_specification_describes() {
        // Two lists of defaults would drift, and the one that drifted would be the one nobody
        // reads: a dialog that opens on a different song from `auris compose` with no file.
        assert_eq!(song_spec(&SongDials::default()), SongSpec::default());
    }

    #[test]
    fn what_the_sheet_writes_is_a_document_that_reads_back_the_same() {
        // The test the sheet and the format share. A dial the specification cannot express, or
        // expresses differently, shows up here rather than as a song that changes when it is
        // saved and opened.
        let spec = song_spec(&peopled());
        assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
        assert_eq!(
            spec.charts["marusa"].bar_count(),
            4,
            "丸サ進行 is four bars"
        );
    }

    #[test]
    fn a_document_refills_the_sheet_it_was_written_from() {
        // What lets a song be composed, saved, reopened and taken again. Through the *file*
        // rather than through the specification, because a file is what a project carries.
        let dials = peopled();
        let written = song_spec(&dials).to_toml();
        let back = song_dials(&SongSpec::parse(&written).expect("the sheet writes valid TOML"));
        assert_eq!(back, dials, "\n{written}");
    }

    #[test]
    fn every_dial_reads_back_what_it_was_set_to() {
        // The bar is drawn from `fraction` and dragged into `set`, so a value that did not
        // survive the round trip would make the bar slide away from the pointer holding it.
        for dial in SONG_DIALS {
            for target in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut dials = SongDials::default();
                dial.set(&mut dials, target);
                let back = dial.fraction(&dials);
                assert!(
                    (back - target).abs() < 0.03,
                    "{dial:?} set to {target} read back {back}"
                );
            }
        }
        for dial in SECTION_DIALS {
            for target in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut section = SectionSpec::named("verse");
                dial.set(&mut section, target);
                let back = dial.fraction(&section);
                assert!(
                    (back - target).abs() < 0.03,
                    "{dial:?} set to {target} read back {back}"
                );
            }
        }
        for dial in PART_DIALS {
            for target in [0.05, 0.25, 0.5, 0.75, 1.0] {
                let mut part = PartSpec::of_role("lead", Role::Melody);
                dial.set(&mut part, target);
                let back = dial.fraction(&part);
                assert!(
                    (back - target).abs() < 0.03,
                    "{dial:?} set to {target} read back {back}"
                );
            }
        }
    }

    #[test]
    fn no_dial_can_be_turned_to_a_value_the_format_refuses() {
        // Every end of every dial, written out and read back: the sheet must not be able to
        // produce a document its own parser rejects.
        for end in [0.0, 1.0] {
            let mut dials = peopled();
            for dial in SONG_DIALS {
                dial.set(&mut dials, end);
            }
            for section in &mut dials.sections {
                for dial in SECTION_DIALS {
                    dial.set(section, end);
                }
            }
            for part in &mut dials.parts {
                for dial in PART_DIALS {
                    dial.set(part, end);
                }
            }
            let spec = song_spec(&dials);
            let written = spec.to_toml();
            assert_eq!(
                SongSpec::parse(&written),
                Ok(spec),
                "every dial at {end}:\n{written}"
            );
            // And the sheet still reads back as itself, which the round trip above does not say
            // on its own: an extreme is exactly where a default would quietly swallow a value.
            assert_eq!(song_dials(&song_spec(&dials)), dials, "every dial at {end}");
        }
    }

    #[test]
    fn another_take_is_the_next_seed_and_nothing_else() {
        let mut dials = SongDials {
            seed: 41,
            ..SongDials::default()
        };
        let before = dials.clone();
        another_take(&mut dials);
        assert_eq!(dials.seed, 42);
        assert_eq!(
            SongDials { seed: 41, ..dials },
            before,
            "only the seed moved"
        );
    }

    #[test]
    fn a_part_added_never_takes_a_name_the_roster_already_has() {
        // Two parts of one name is an error the format reports, and a button that produces one
        // is a button that breaks the song.
        let mut dials = SongDials::default();
        add_part(&mut dials, Role::Bass);
        add_part(&mut dials, Role::Bass);
        let names: Vec<&str> = dials.parts.iter().map(|part| part.name.as_str()).collect();
        assert!(names.contains(&"bass 2"), "{names:?}");
        assert!(names.contains(&"bass 3"), "{names:?}");
        assert!(SongSpec::parse(&song_spec(&dials).to_toml()).is_ok());
    }

    #[test]
    fn the_last_part_cannot_be_removed() {
        // A song with no parts writes no notes; the button goes dead rather than the Write
        // button producing an empty document.
        let mut dials = SongDials::default();
        while dials.parts.len() > 1 {
            assert!(remove_part(&mut dials, 0));
        }
        assert!(!remove_part(&mut dials, 0));
        assert_eq!(dials.parts.len(), 1);
    }

    #[test]
    fn a_name_played_twice_is_one_section_played_twice() {
        // The whole point of a form. Two verse rows read and write the one verse, or the second
        // would be eight different bars wearing the same label.
        let dials = SongDials::default();
        let places: Vec<usize> = dials
            .form
            .iter()
            .enumerate()
            .filter(|(_, name)| name.as_str() == "verse")
            .map(|(place, _)| place)
            .collect();
        assert!(places.len() >= 2, "the default form plays a verse twice");
        let sections: Vec<Option<usize>> = places
            .iter()
            .map(|place| section_at(&dials, *place))
            .collect();
        assert_eq!(sections[0], sections[1]);
        assert!(sections[0].is_some());
    }

    #[test]
    fn the_form_is_edited_and_stays_a_form_the_format_accepts() {
        let mut dials = SongDials::default();
        let places = dials.form.len();
        let sections = dials.sections.len();

        // Adding a name the song already has is a repeat: a place in the order, no definition.
        add_to_form(&mut dials, 0, "chorus");
        assert_eq!(dials.form.len(), places + 1);
        assert_eq!(
            dials.sections.len(),
            sections,
            "a repeat defines nothing new"
        );
        assert_eq!(dials.form[1], "chorus");

        // A name the song has never used brings its definition with it.
        add_to_form(&mut dials, 0, "solo");
        assert_eq!(dials.sections.len(), sections + 1);
        assert!(dials.sections.iter().any(|section| section.name == "solo"));

        // A second verse that is *not* the first one, which is what the numbered names in the
        // menu are for. The space in it has to survive being a TOML table key.
        let second = unused_section_name(&dials, "verse");
        assert_eq!(second, "verse 2");
        add_to_form(&mut dials, 1, &second);
        assert!(dials.sections.iter().any(|section| section.name == second));
        let written = song_spec(&dials).to_toml();
        assert_eq!(
            SongSpec::parse(&written).map(|spec| song_dials(&spec)),
            Ok(dials.clone()),
            "\n{written}"
        );

        // Moving is a swap, and stops at both ends rather than wrapping round.
        let first = dials.form[0].clone();
        assert!(move_in_form(&mut dials, 0, true));
        assert_eq!(dials.form[1], first);
        assert!(!move_in_form(&mut dials, 0, false));
        let last = dials.form.len() - 1;
        assert!(!move_in_form(&mut dials, last, true));

        // A section the form no longer plays takes its definition with it, rather than going on
        // contributing a row to the panel and a table to the file describing nothing audible.
        let solo = dials.form.iter().position(|name| name == "solo").unwrap();
        assert!(remove_from_form(&mut dials, solo));
        assert!(
            !dials.sections.iter().any(|section| section.name == "solo"),
            "a section nothing plays stayed behind: {:?}",
            dials.sections
        );

        assert!(SongSpec::parse(&song_spec(&dials).to_toml()).is_ok());
    }

    #[test]
    fn the_last_playing_cannot_be_removed() {
        // An empty form is a document the specification refuses outright, so the button goes dead
        // rather than Write reporting an error the sheet could have prevented.
        let mut dials = SongDials::default();
        while dials.form.len() > 1 {
            assert!(remove_from_form(&mut dials, 0));
        }
        assert!(!remove_from_form(&mut dials, 0));
        assert_eq!(dials.form.len(), 1);
        assert_eq!(dials.sections.len(), 1, "and its section is the one left");
    }

    #[test]
    fn choosing_a_progression_for_a_section_is_what_makes_the_song_carry_it() {
        // The only way a second chart comes into existence. A list somebody has to build before
        // they can use it would be a screen between them and the thing they wanted.
        let mut dials = SongDials::default();
        assert_eq!(dials.charts.len(), 1);
        assert_eq!(dials.charts[0].0, MAIN_CHART);

        let chorus = dials
            .sections
            .iter()
            .position(|section| section.name == "chorus")
            .unwrap();
        assert!(set_section_chart(&mut dials, chorus, "marusa"));
        assert_eq!(dials.charts.len(), 2);
        assert_eq!(dials.sections[chorus].chords, "marusa");

        // The verse is untouched, which is the whole feature: a progression that changes partway.
        let verse = dials
            .sections
            .iter()
            .position(|section| section.name == "verse")
            .unwrap();
        assert_eq!(dials.sections[verse].chords, MAIN_CHART);

        // Choosing the same one for another section adds nothing.
        assert!(set_section_chart(&mut dials, verse, "marusa"));
        assert_eq!(dials.charts.len(), 2);

        // A name nothing answers to changes nothing at all, rather than pointing a section at a
        // chart that does not exist — which is a document the parser refuses.
        assert!(!set_section_chart(&mut dials, verse, "nonsense"));
        let spec = song_spec(&dials);
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));
    }

    #[test]
    fn a_progression_written_out_by_hand_belongs_to_the_song_that_uses_it() {
        // What a chart nobody named comes in through: neither the catalogue nor the book can be
        // *asked* for one, and it has to end up in the song's own charts or the section would
        // point at a name nothing answers to — a document the parser refuses.
        let mut dials = SongDials::default();
        let chorus = dials
            .sections
            .iter()
            .position(|section| section.name == "chorus")
            .unwrap();
        let written = Chart::parse("| IVmaj7 | III7 | vi7 | I7 |").unwrap();
        assert!(give_section_chart(
            &mut dials,
            chorus,
            "chorus",
            written.clone()
        ));
        assert_eq!(dials.sections[chorus].chords, "chorus");
        assert_eq!(dials.charts.len(), 2);

        // Written twice leaves one chart rather than two of the same name.
        let again = Chart::parse("| ii | V | I | I |").unwrap();
        assert!(give_section_chart(
            &mut dials,
            chorus,
            "chorus",
            again.clone()
        ));
        assert_eq!(dials.charts.len(), 2);
        assert_eq!(dials.charts[1].1, again);

        // A hand-written chart quotes nothing, which is what the sheet reads to decide whether
        // there is anything worth offering to keep.
        assert_eq!(dials.charts[1].1.quoted_as, None);
        assert_eq!(
            dials.charts[0].1.quoted_as, None,
            "the default is its own too"
        );
        set_section_chart(&mut dials, chorus, "marusa");
        let quoted = dials
            .charts
            .iter()
            .find(|(name, _)| name == "marusa")
            .unwrap();
        assert_eq!(quoted.1.quoted_as.as_deref(), Some("marusa"));

        // And it survives the file, which is what makes it the song's rather than the picker's.
        let spec = song_spec(&dials);
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));
    }

    #[test]
    fn no_progression_chosen_leaves_the_one_the_mood_may_colour() {
        // "Nothing chosen" is how the composer gets to invent, not a hole: the default chart is
        // marked as its own and is the only kind colouring is allowed to touch.
        assert_eq!(
            song_spec(&SongDials::default()).charts[MAIN_CHART].origin,
            ChartOrigin::Generated
        );
    }

    #[test]
    fn every_name_the_pickers_offer_has_a_word_in_both_languages() {
        // The pickers list the catalogues, and a name with no entry here would come out as the
        // fallback rather than as a translation.
        for name in Mood::NAMES {
            let key = mood_key(name);
            assert_ne!(
                key.get(auris_i18n::Language::English),
                key.get(auris_i18n::Language::Japanese),
                "the mood `{name}` reads the same in both languages"
            );
        }
        for role in Role::ALL {
            let key = role_key(role);
            assert_ne!(
                key.get(auris_i18n::Language::English),
                key.get(auris_i18n::Language::Japanese),
                "the role `{}` reads the same in both languages",
                role.name()
            );
        }
        for entry in PRESETS {
            assert_ne!(
                auris_i18n::audio::preset_description(
                    entry.description,
                    auris_i18n::Language::Japanese
                ),
                entry.description,
                "the `{}` preset has no Japanese description",
                entry.name
            );
        }
    }

    #[test]
    fn choosing_a_style_fills_the_whole_sheet_with_it() {
        // Half a preset is the arrangement of one style at the tempo of another, which is not a
        // style at all — so this is the assertion that the sheet takes the lot.
        for entry in PRESETS {
            let spec = entry.spec();
            let dials = song_dials(&spec);
            assert_eq!(
                song_spec(&dials),
                spec,
                "{} is not the sheet it fills",
                entry.name
            );
        }
    }

    #[test]
    fn every_program_the_picker_offers_is_reachable_through_a_family() {
        // The part row's menu lists families and each family lists eight programs; a program in
        // no family would be a sound the interface can never choose, however well it composes.
        let mut reachable: Vec<u8> = (0..gm::FAMILIES.len())
            .flat_map(gm::Program::family_programs)
            .map(|program| program.0)
            .collect();
        reachable.sort_unstable();
        reachable.dedup();
        assert_eq!(reachable.len(), gm::PROGRAMS.len());
    }

    #[test]
    fn the_mood_word_is_the_one_the_numbers_actually_are() {
        assert_eq!(mood_word(Mood::named("dreamy").unwrap()), Some("dreamy"));
        let mut nudged = Mood::named("dreamy").unwrap();
        nudged.energy = 0.9;
        assert_eq!(mood_word(nudged), None, "a nudged mood is no word at all");
    }

    #[test]
    fn a_progression_is_named_or_left_to_the_composer() {
        assert!(chart_named("").is_none());
        assert!(chart_named("   ").is_none());
        assert!(chart_named("marusa").is_some());
        assert!(chart_named("nonsense").is_none());
        for entry in progression_catalog() {
            assert!(
                chart_named(entry.name).is_some(),
                "the picker offers `{}` and nothing answers to it",
                entry.name
            );
        }
    }

    #[test]
    fn a_drum_part_offers_a_note_where_a_pitched_one_offers_an_octave() {
        // The substitution the part row makes, and the reason it is free: a kit has no octave —
        // its pitches are drum numbers rather than notes — and it badly needs the one number a
        // font that does not follow General MIDI would otherwise leave it silent for.
        let dials = SongDials::default();
        for part in &dials.parts {
            assert_eq!(
                part.drum_note().is_some(),
                part.role.is_drum(),
                "{} offered the wrong control",
                part.name
            );
        }

        // Every note the picker offers is one the format accepts, and reads back as itself.
        let mut kit = dials.clone();
        let kick = kit
            .parts
            .iter()
            .position(|part| part.role.is_drum())
            .unwrap();
        for (note, _) in DRUM_NOTES {
            kit.parts[kick].note = Some(note);
            let spec = song_spec(&kit);
            assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec), "note {note}");
        }

        // The names are General MIDI's own, and a note outside the kit still reads as itself.
        assert_eq!(drum_note_label(36), "36 · Bass Drum 1");
        assert_eq!(drum_note_label(120), "120");
        let mut numbers: Vec<u8> = DRUM_NOTES.iter().map(|(note, _)| *note).collect();
        let count = numbers.len();
        numbers.dedup();
        assert_eq!(count, numbers.len(), "the picker offers a note twice");
    }

    #[test]
    fn a_transposition_says_which_way_it_goes() {
        assert_eq!(transpose_label(0), "±0");
        assert_eq!(transpose_label(2), "+2");
        assert_eq!(transpose_label(-3), "-3");
        assert!(
            TRANSPOSES.contains(&0),
            "there has to be a way back to none"
        );
    }
}
