//! Everything the sheet decides, which is everything about it that can be tested.
//!
//! Transformations from these dials to a [`SongSpec`] live in free functions with tests.
//! The view draws them and hands back what was moved.
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
pub const TEMPO: std::ops::RangeInclusive<f64> = 20.0..=400.0;

/// Straight to as far as a swing dial goes, in the percentage a shuffle is written in.
pub const SWING: std::ops::RangeInclusive<u8> = 50..=75;

/// How far a part's level trim reaches, in decibels.
pub const GAIN_DB: std::ops::RangeInclusive<f32> = -60.0..=12.0;

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

/// Plain style names for the beginner picker, without prescribing a chord progression.
pub fn style_key(name: &str) -> Key {
    match name {
        "chiptune" => Key::SongStyleChiptune,
        "game-loop" => Key::SongStyleGameLoop,
        "pop-band" => Key::SongStylePopBand,
        "city-pop" => Key::SongStyleCityPop,
        "rock" => Key::SongStyleRock,
        "jazz-trio" => Key::SongStyleJazz,
        "orchestral" => Key::SongStyleOrchestral,
        "synthwave" => Key::SongStyleSynthwave,
        "ambient" => Key::SongStyleAmbient,
        _ => Key::SongStyleChoose,
    }
}

/// Everything the sheet is set to.
///
/// Resolved specification values, plus optional mood-driven choices for the current editing
/// session. `sections` and `charts` are ordered lists here and maps in the specification:
/// renaming a section must not move its row while the pointer is still on it.
#[derive(Clone, Debug, PartialEq)]
pub struct SongDials {
    /// Voice model selected for the sung part.
    pub singer: Option<String>,
    /// Speaker name within the selected voice; absent selects its default speaker.
    pub singer_speaker: Option<String>,
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
    /// An active beginner choice; `None` retains an imported or manually edited key.
    pub tonality: Option<Tonality>,
    /// An optional sound character; `None` retains a manually specified scale.
    pub sound: Option<ScaleChoice>,
    /// An active beginner choice; `None` retains an imported or manually edited tempo.
    pub pace: Option<Pace>,
    /// The drum groove.
    pub groove: String,
    /// The seed every random decision is drawn from.
    pub seed: u64,
    /// How much the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander.
    pub humanize: f32,
    /// The preset's articulation palette, retained through edits and song-sheet round trips.
    pub performance: Option<PerformanceStyle>,
    /// Score vocabulary retained independently of performance-only controls.
    pub writing_style: Option<PerformanceStyle>,
    /// How far apart the hardest and softest notes are struck.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill.
    pub fill: f32,
    /// How much a repeat departs from what came before it.
    pub variation: f32,
    /// How the piece closes or returns to its opening, selected in the basic controls.
    pub ending: Ending,
    /// The tune's contour, when one was given: scale steps around the figure's anchor.
    ///
    /// Carried even though the sheet draws no control for it yet, so a
    /// specification that gave the piece a tune must not come back having forgotten it.
    pub motif: Vec<i32>,
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

/// Applies a named mood, resolving only the choices still controlled by the beginner pickers.
pub fn set_song_mood(dials: &mut SongDials, mood: Mood) {
    dials.mood = mood;
    if let Some(tonality) = dials.tonality {
        dials.key = dials
            .sound
            .unwrap_or_default()
            .key(tonality, mood, dials.key.tonic);
    }
    if let Some(pace) = dials.pace {
        dials.tempo = pace.bpm(mood);
    }
    dials.motif.clear();
}

/// Selects a mode without requiring a tonic or scale name.
pub fn set_song_tonality(dials: &mut SongDials, tonality: Tonality) {
    dials.tonality = Some(tonality);
    dials.sound = Some(ScaleChoice::Auto);
    dials.key = tonality.key(dials.mood, dials.key.tonic);
}

/// Selects a sound, keeping it explicit across mood and style changes.
pub fn set_song_sound(dials: &mut SongDials, sound: ScaleChoice) {
    dials.sound = Some(sound);
    let tonality = if sound == ScaleChoice::Auto {
        Tonality::Auto
    } else if sound
        .key(Tonality::Auto, dials.mood, dials.key.tonic)
        .is_minor()
    {
        Tonality::Minor
    } else {
        Tonality::Major
    };
    dials.tonality = Some(tonality);
    dials.key = sound.key(tonality, dials.mood, dials.key.tonic);
}

/// Selects a speed for the whole song.
pub fn set_song_pace(dials: &mut SongDials, pace: Pace) {
    dials.pace = Some(pace);
    dials.tempo = pace.bpm(dials.mood);
    for section in &mut dials.sections {
        section.tempo = None;
    }
}

/// Uses a style's instruments and form with generated harmony and the current character.
pub fn style_dials(preset: &SongPreset, previous: Option<&SongDials>) -> SongDials {
    let mut spec = preset.spec();
    spec.use_generated_harmony();
    let mut dials = song_dials(&spec);
    dials.tonality = Some(Tonality::Auto);
    dials.sound = Some(ScaleChoice::Auto);
    dials.pace = Some(Pace::Auto);
    if let Some(previous) = previous {
        dials.key = previous.key;
        dials.tempo = previous.tempo;
        dials.tonality = previous.tonality;
        dials.sound = previous.sound;
        dials.pace = previous.pace;
        set_song_mood(&mut dials, previous.mood);
    } else {
        set_song_mood(&mut dials, spec.mood);
    }
    dials
}

/// The specification these dials describe.
///
/// The one place the sheet turns into a song. Everything downstream — writing the piece, saving
/// the `.asong`, refilling the sheet from one — goes through the specification, so a dial cannot
/// mean one thing to the composer and another to the file.
pub fn song_spec(dials: &SongDials) -> SongSpec {
    SongSpec {
        singer: dials.singer.clone(),
        singer_speaker: dials.singer_speaker.clone(),
        title: dials.title.clone(),
        key: dials.key,
        tempo: dials.tempo,
        meter: dials.meter,
        mood: dials.mood,
        seed: dials.seed,
        swing: dials.swing,
        humanize: dials.humanize,
        performance: dials.performance,
        writing_style: dials.writing_style,
        dynamics: dials.dynamics,
        fill: dials.fill,
        variation: dials.variation,
        groove: dials.groove.clone(),
        ending: dials.ending,
        motif: dials.motif.clone(),
        charts: dials.charts.iter().cloned().collect(),
        chart_order: dials.charts.iter().map(|(name, _)| name.clone()).collect(),
        sections: dials
            .sections
            .iter()
            .map(|section| (section.name.clone(), section.clone()))
            .collect(),
        form: dials.form.clone(),
        parts: dials.parts.clone(),
    }
}

/// The dials the sheet opens on: what was remembered, corrected by what the document says now.
///
/// `remembered` is the specification stored with the project, which records what the composer was
/// last *asked* for. Three of its fields are also the document's own — the key, the tempo and the
/// meter — and the document is where a user changes them afterwards, from the harmony lane, the
/// transport and the ruler. The stored specification does not move when they do.
///
/// So the document wins for those three and the specification keeps the rest. Without it the sheet
/// opened on a key the song had stopped being in, and writing the song put the old one back — which
/// reads as the composer ignoring the key entirely.
///
/// A free function because the view that calls it cannot be reached by a test, and this is a rule
/// rather than a rendering.
pub fn opening_dials(
    remembered: Option<&SongSpec>,
    key: MusicalKey,
    tempo: f64,
    meter: TimeSignature,
) -> SongDials {
    let mut dials = remembered.map_or_else(SongDials::default, song_dials);
    if remembered.is_none() && key == dials.key {
        dials.tonality = Some(Tonality::Auto);
        dials.sound = Some(ScaleChoice::Auto);
    }
    if remembered.is_none() && tempo == dials.tempo {
        dials.pace = Some(Pace::Auto);
    }
    dials.key = key;
    dials.tempo = tempo;
    dials.meter = meter;
    dials
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
    for name in spec.chart_order.iter().chain(spec.charts.keys()) {
        if name == MAIN_CHART || charts.iter().any(|(seen, _)| seen == name) {
            continue;
        }
        if let Some(chart) = spec.charts.get(name) {
            charts.push((name.clone(), chart.clone()));
        }
    }

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
        singer: spec.singer.clone(),
        singer_speaker: spec.singer_speaker.clone(),
        title: spec.title.clone(),
        key: spec.key,
        tempo: spec.tempo,
        meter: spec.meter,
        mood: spec.mood,
        tonality: None,
        sound: None,
        pace: None,
        groove: spec.groove.clone(),
        seed: spec.seed,
        swing: spec.swing,
        humanize: spec.humanize,
        performance: spec.performance,
        writing_style: spec.writing_style,
        dynamics: spec.dynamics,
        fill: spec.fill,
        variation: spec.variation,
        ending: spec.ending,
        motif: spec.motif.clone(),
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
    // Presets use `pre2` and the picker writes `pre 2`; both display the same number.
    // Keep existing identifiers distinct, but offer a number whose label is still unused.
    (1..=u32::MAX)
        .find(|number| {
            !dials.sections.iter().any(|section| {
                section.name.strip_prefix(stem).is_some_and(|suffix| {
                    let suffix = suffix.trim();
                    let existing = if suffix.is_empty() {
                        Some(1)
                    } else {
                        suffix.parse::<u32>().ok()
                    };
                    existing == Some(*number)
                })
            })
        })
        .map(|n| {
            if n == 1 {
                stem.to_string()
            } else {
                format!("{stem} {n}")
            }
        })
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
    for section in &mut kept {
        if section
            .melody_from
            .as_ref()
            .is_some_and(|source| !dials.form.contains(source))
        {
            section.melody_from = None;
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

/// Leaves the section at `index` with a progression the composer will invent.
///
/// Filed under the section's own name, the same way a progression written out by hand is, so
/// choosing おまかせ for the chorus and later writing one out are edits to the same slot. The
/// marker is what is stored — the invention itself happens when the song is written, from the
/// seed, which is what keeps the seed dial re-dealing it.
pub fn invent_section_chart(dials: &mut SongDials, index: usize) -> bool {
    let Some(name) = dials.sections.get(index).map(|held| held.name.clone()) else {
        return false;
    };
    give_section_chart(dials, index, &name, Chart::unwritten())
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

/// The first writer sharing this part's instrument, which owns the kit's source picker.
pub fn part_source_owner(dials: &SongDials, index: usize) -> Option<usize> {
    part_source_group(dials, index).first().copied()
}

/// Changes the instrument for a pitched part or every writer of its current drum kit.
pub fn set_part_instrument(dials: &mut SongDials, index: usize, instrument: &str) {
    for index in part_source_group(dials, index) {
        dials.parts[index].instrument = instrument.to_string();
        dials.parts[index].program = None;
        dials.parts[index].source = None;
    }
}

/// Chooses an exact library source for a pitched part or every writer of its drum kit.
pub fn set_part_source(dials: &mut SongDials, index: usize, source: PartSource) {
    for index in part_source_group(dials, index) {
        dials.parts[index].source = Some(source.clone());
        dials.parts[index].program = None;
    }
}

/// Changes a part's role, carrying over its adjustments and updating untouched role defaults.
///
/// Selecting the current role leaves the entire part alone. The specification stores values,
/// not edit history, so a value equal to the previous role's default still follows the role.
/// Explicit optional settings and fields shared by every role are retained as written.
pub fn set_part_role(dials: &mut SongDials, index: usize, role: Role) -> bool {
    let Some(part) = dials.parts.get_mut(index) else {
        return false;
    };
    let previous = part.role;
    if previous == role {
        return false;
    }
    if part.instrument == previous.default_instrument() {
        part.instrument = role.default_instrument().to_string();
    }
    if part.octave == previous.default_octave() {
        part.octave = role.default_octave();
    }
    if part.gate == previous.default_gate() {
        part.gate = role.default_gate();
    }
    if part.gain_db == previous.default_gain_db() {
        part.gain_db = role.default_gain_db();
    }
    if part.pan == previous.default_pan() {
        part.pan = role.default_pan();
    }
    part.role = role;
    true
}

/// The shared kit is exactly the group the composer will merge, before the source is changed.
fn part_source_group(dials: &SongDials, index: usize) -> Vec<usize> {
    let Some(selected) = dials.parts.get(index) else {
        return Vec::new();
    };
    if !selected.role.is_drum() {
        return vec![index];
    }
    dials
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| {
            let same_source = match (&part.source, &selected.source) {
                (Some(left), Some(right)) => left == right,
                (None, None) => {
                    part.instrument == selected.instrument && part.program == selected.program
                }
                _ => false,
            };
            (part.role.is_drum() && same_source).then_some(index)
        })
        .collect()
}

/// Adds a part of `role`, named after it.
pub fn add_part(dials: &mut SongDials, role: Role) {
    let name = unused_part_name(dials, role.name());
    dials.parts.push(PartSpec::of_role(name, role));
}

/// Adds or removes the shared kit as one roster entry in the song sheet.
pub fn set_song_drums(dials: &mut SongDials, enabled: bool) {
    if enabled {
        if dials.parts.iter().any(|part| part.role.is_drum()) {
            return;
        }
        for role in [Role::Kick, Role::Snare, Role::Hat, Role::Crash] {
            add_part(dials, role);
        }
    } else if dials.parts.iter().any(|part| !part.role.is_drum()) {
        for index in (0..dials.parts.len()).rev() {
            if dials.parts[index].role.is_drum() {
                remove_part(dials, index);
            }
        }
    }
}

/// Renames the part at `index` and every section reference keyed by its name.
///
/// Part names are identities in the song specification: section rosters and section-specific
/// tweaks both point at them. Moving all three together keeps every sheet state writable.
pub fn rename_part(dials: &mut SongDials, index: usize, name: &str) -> bool {
    if name.trim().is_empty()
        || dials
            .parts
            .iter()
            .enumerate()
            .any(|(other, part)| other != index && part.name == name)
    {
        return false;
    }
    let Some(part) = dials.parts.get_mut(index) else {
        return false;
    };
    let old = std::mem::replace(&mut part.name, name.to_string());
    if old == name {
        return true;
    }
    for section in &mut dials.sections {
        for held in &mut section.parts {
            if held == &old {
                *held = name.to_string();
            }
        }
        if let Some(tweak) = section.tweaks.remove(&old) {
            section.tweaks.insert(name.to_string(), tweak);
        }
    }
    true
}

/// Removes the part at `index`, if the roster would still have one.
///
/// A song with no parts writes no notes, and a sheet whose Write button produces an empty
/// document is a sheet with a broken state reachable from it.
pub fn remove_part(dials: &mut SongDials, index: usize) -> bool {
    if dials.parts.len() <= 1 || index >= dials.parts.len() {
        return false;
    }
    let removed = dials.parts.remove(index).name;
    for section in &mut dials.sections {
        section.parts.retain(|part| part != &removed);
        section.tweaks.remove(&removed);
    }
    true
}

/// How a section's tempo is written on its button.
///
/// A section without an override names the policy explicitly; showing the current song tempo
/// would hide which sections follow later changes to the song.
pub fn section_tempo_label(section: &SectionSpec, language: auris_i18n::Language) -> String {
    match section.tempo {
        Some(bpm) => format!("{bpm} BPM"),
        None => Key::SongSectionTempoFollows.get(language).to_string(),
    }
}

/// Whether a part plays in a section.
///
/// An empty list means *everything*, so this is not the same question as whether the part is named.
pub fn part_plays_in(section: &SectionSpec, part: &str) -> bool {
    section.parts.is_empty() || section.parts.iter().any(|name| name == part)
}

/// Turns one part on or off for one section.
///
/// Three things make this more than adding a name to a list or taking one out of it, and every
/// one of them is a way a plain toggle would be wrong.
///
/// **Empty means everything.** So the stored list is read into the set it *stands for* before
/// anything is changed. Taking the hat out of a section that names no parts is not "remove `hat`
/// from an empty list" — which does nothing — it is "name the other six".
///
/// **A full list goes back to empty.** Turning the last part back on stores everything as nothing,
/// because a section that names all seven parts would go on naming seven when an eighth is added,
/// and would be the one section the new part silently does not play in. The format writes it that
/// way for the same reason.
///
/// **The last part stays.** A section playing nothing is silence, and the list cannot say it: the
/// spelling for an empty set is already taken by the full one. Refusing is the honest answer;
/// somebody who wants a silent stretch removes the section from the form.
pub fn toggle_part_in_section(dials: &mut SongDials, index: usize, part: &str) -> bool {
    let roster: Vec<String> = dials.parts.iter().map(|part| part.name.clone()).collect();
    if !roster.iter().any(|name| name == part) {
        return false;
    }
    let Some(section) = dials.sections.get_mut(index) else {
        return false;
    };
    let mut playing: Vec<String> = if section.parts.is_empty() {
        roster.clone()
    } else {
        roster
            .iter()
            .filter(|name| section.parts.contains(name))
            .cloned()
            .collect()
    };
    if playing.iter().any(|name| name == part) {
        if playing.len() <= 1 {
            return false;
        }
        playing.retain(|name| name != part);
    } else {
        // Rebuilt in roster order rather than pushed onto the end, so the specification reads
        // down the mixer rather than in the order somebody happened to click.
        playing.push(part.to_string());
        playing = roster
            .iter()
            .filter(|name| playing.contains(name))
            .cloned()
            .collect();
    }
    section.parts = if playing.len() == roster.len() {
        Vec::new()
    } else {
        playing
    };
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
        Role::Crash => Key::RoleCrash,
        Role::Riser => Key::RoleRiser,
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
                dials.pace = None;
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
        if matches!(
            self,
            SongDial::Brightness | SongDial::Energy | SongDial::Tension | SongDial::Syncopation
        ) {
            set_song_mood(dials, dials.mood);
        }
    }

    /// What the readout at the end of the bar says.
    pub fn text(self, dials: &SongDials) -> String {
        match self {
            SongDial::Tempo => dials.tempo.to_string(),
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
    pub fn fraction(self, part: &PartSpec, mood: Mood) -> f32 {
        match self {
            // Start a manual adjustment at the same base density the composer currently uses.
            PartDial::Density => part.density.unwrap_or_else(|| mood.density()),
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
    pub fn text(self, part: &PartSpec, mood: Mood) -> String {
        match self {
            PartDial::Gain => format!("{:.1}", part.gain_db),
            PartDial::Pan if part.pan.abs() < 0.005 => "C".to_string(),
            PartDial::Pan if part.pan < 0.0 => format!("L{:.0}", part.pan.abs() * 100.0),
            PartDial::Pan => format!("R{:.0}", part.pan * 100.0),
            other => percent(other.fraction(part, mood)),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn customized_part() -> PartSpec {
        PartSpec {
            instrument: "custom.instrument".to_string(),
            program: Some(gm::Program(48)),
            octave: 6,
            density: Some(0.67),
            subdivision: Subdivision::EighthTriplet,
            gate: 0.42,
            rhythm: Some(Pattern::parse("X..x..x.").unwrap()),
            note: Some(42),
            gain_db: -4.5,
            pan: -0.6,
            ..PartSpec::of_role("my lead", Role::Melody)
        }
    }

    #[test]
    fn reselecting_a_part_role_keeps_every_setting_and_section_reference() {
        let mut dials = SongDials::default();
        dials.parts[0] = customized_part();
        let before = dials.clone();

        assert!(!set_part_role(&mut dials, 0, Role::Melody));
        assert_eq!(dials, before);
        assert!(!set_part_role(&mut dials, usize::MAX, Role::Bass));
        assert_eq!(dials, before);
    }

    #[test]
    fn changing_a_part_role_retains_custom_sound_and_performance_settings() {
        let mut dials = SongDials::default();
        let index = 0;
        let name = dials.parts[index].name.clone();
        dials.parts[index] = PartSpec {
            name,
            ..customized_part()
        };
        let before = dials.clone();
        for role in [Role::Bass, Role::Stab, Role::Pad] {
            assert!(set_part_role(&mut dials, index, role));
            let mut expected = before.clone();
            expected.parts[index].role = role;
            assert_eq!(dials, expected);
        }
    }

    #[test]
    fn changing_an_untouched_part_role_adopts_the_new_roles_defaults() {
        for previous in Role::ALL {
            for role in Role::ALL {
                let mut dials = SongDials {
                    parts: vec![PartSpec::of_role("my part", previous)],
                    ..SongDials::default()
                };
                assert_eq!(set_part_role(&mut dials, 0, role), previous != role);
                assert_eq!(dials.parts[0], PartSpec::of_role("my part", role));
            }
        }
    }

    #[test]
    fn one_source_picker_changes_every_voice_of_only_its_current_kit() {
        let mut dials = SongDials::default();
        let drums: Vec<_> = dials
            .parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| part.role.is_drum().then_some(index))
            .collect();
        let owner = drums[0];
        for index in &drums {
            assert_eq!(part_source_owner(&dials, *index), Some(owner));
        }
        let melodic: Vec<_> = dials
            .parts
            .iter()
            .filter(|part| !part.role.is_drum())
            .cloned()
            .collect();
        let mut separate = PartSpec::of_role("second-kit", Role::Kick);
        separate.program = Some(gm::Program(25));
        dials.parts.push(separate.clone());
        set_part_instrument(&mut dials, drums[1], "external.drumkit");
        for index in &drums {
            assert_eq!(dials.parts[*index].instrument, "external.drumkit");
            assert_eq!(dials.parts[*index].program, None);
        }
        assert_eq!(dials.parts.last(), Some(&separate));
        assert_eq!(
            dials
                .parts
                .iter()
                .filter(|part| !part.role.is_drum())
                .cloned()
                .collect::<Vec<_>>(),
            melodic
        );
        let source = PartSource::SoundFont {
            path: "kit.sf2".into(),
            bank: 128,
            patch: 8,
        };
        set_part_source(&mut dials, owner, source.clone());
        assert!(
            drums
                .iter()
                .all(|index| dials.parts[*index].source.as_ref() == Some(&source))
        );
        assert_eq!(dials.parts.last(), Some(&separate));
        assert_eq!(part_source_owner(&dials, usize::MAX), None);
    }

    #[test]
    fn explicit_sources_group_kit_writers_and_survive_role_and_spec_changes() {
        let source = PartSource::SoundFont {
            path: "custom.sf2".into(),
            bank: 200,
            patch: 300,
        };
        let mut dials = SongDials::default();
        let kick = dials
            .parts
            .iter()
            .position(|part| part.role == Role::Kick)
            .unwrap();
        set_part_source(&mut dials, kick, source.clone());
        assert!(
            dials
                .parts
                .iter()
                .filter(|part| part.role.is_drum())
                .all(|part| part.source.as_ref() == Some(&source) && part.program.is_none())
        );
        let hat = dials
            .parts
            .iter()
            .position(|part| part.role == Role::Hat)
            .unwrap();
        dials.parts[hat].instrument = "obsolete.fallback".into();
        assert_eq!(part_source_owner(&dials, hat), Some(kick));
        let separate = PartSpec {
            source: Some(PartSource::SoundFont {
                path: "other.sf2".into(),
                bank: 200,
                patch: 300,
            }),
            ..PartSpec::of_role("other kit", Role::Kick)
        };
        dials.parts.push(separate.clone());
        set_part_instrument(&mut dials, hat, "auris.synth.noise");
        assert!(
            dials.parts[..dials.parts.len() - 1]
                .iter()
                .filter(|part| part.role.is_drum())
                .all(|part| part.source.is_none() && part.instrument == "auris.synth.noise")
        );
        assert_eq!(dials.parts.last(), Some(&separate));
        set_part_source(&mut dials, 0, source.clone());
        set_part_role(&mut dials, 0, Role::Bass);
        let spec = song_spec(&dials);
        assert_eq!(
            SongSpec::parse(&spec.to_toml()).unwrap().parts[0].source,
            Some(source)
        );
    }

    #[test]
    fn the_sheet_opens_on_the_key_the_document_is_in_now() {
        // The specification remembers what the composer was last asked for. The key, the tempo
        // and the meter are also the document's, and a user changes them there — in the harmony
        // lane, on the transport, on the ruler — long after the specification was stored. The
        // sheet used to open on the remembered ones, so composing put the old key back.
        let remembered = SongSpec {
            key: MusicalKey::parse("C major").unwrap(),
            tempo: 120.0,
            meter: TimeSignature::new(4, 4),
            title: "Remembered".to_string(),
            groove: "shuffle".to_string(),
            ..SongSpec::default()
        };
        let now = MusicalKey::parse("F# minor").unwrap();
        let dials = opening_dials(Some(&remembered), now, 96.0, TimeSignature::new(3, 4));

        assert_eq!(dials.key, now, "the key comes from the document");
        assert_eq!(dials.tempo, 96.0, "and so does the tempo");
        assert_eq!(dials.meter, TimeSignature::new(3, 4), "and the meter");
        // Everything the document does not own is still the specification's.
        assert_eq!(dials.title, "Remembered");
        assert_eq!(dials.groove, "shuffle");
    }

    #[test]
    fn a_document_with_no_specification_still_opens_on_its_own_key() {
        // Nothing composed yet, so there is nothing remembered — but the document has been in
        // A minor since the user set it, and the sheet has no business offering C major.
        let now = MusicalKey::parse("A minor").unwrap();
        let dials = opening_dials(None, now, 88.0, TimeSignature::new(6, 8));

        assert_eq!(dials.key, now);
        assert_eq!(dials.tempo, 88.0);
        assert_eq!(dials.meter, TimeSignature::new(6, 8));
        // And the rest is the default sheet, not an empty one.
        assert_eq!(dials.title, SongDials::default().title);
        assert!(!dials.parts.is_empty());
    }

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
    fn every_preset_keeps_its_performance_palette_through_sheet_edits() {
        for preset in PRESETS {
            let spec = preset.spec();
            let mut dials = song_dials(&spec);
            dials.title = "Renamed song".into();
            dials.tempo += 1.0;
            let restored = SongSpec::parse(&song_spec(&dials).to_toml()).unwrap();
            assert_eq!(restored.performance, spec.performance);
            assert!(restored.performance.is_some());
            assert_eq!(restored.writing_style, spec.writing_style);
            assert!(restored.writing_style.is_some());
        }
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
    fn chart_order_survives_writing_and_reopening_the_sheet() {
        let mut dials = SongDials::default();
        dials.charts.push((
            "marusa".to_string(),
            chart_named("marusa").expect("catalogue chart"),
        ));
        dials.charts.push((
            "junjo".to_string(),
            chart_named("junjo").expect("catalogue chart"),
        ));

        let written = song_spec(&dials).to_toml();
        let reopened = song_dials(&SongSpec::parse(&written).expect("written spec parses"));
        assert_eq!(
            reopened
                .charts
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["main", "marusa", "junjo"]
        );
    }

    /// The names of the parts playing in a section, read the way the composer reads them.
    fn playing(dials: &SongDials, index: usize) -> Vec<&str> {
        dials
            .parts
            .iter()
            .map(|part| part.name.as_str())
            .filter(|name| part_plays_in(&dials.sections[index], name))
            .collect()
    }

    #[test]
    fn turning_a_part_off_in_a_section_that_named_none_names_the_others() {
        // The trap this whole function exists for. An empty list means *everything*, so removing
        // a name from it does nothing at all — the section goes on playing the part that was just
        // switched off, and the row keeps its tick.
        let mut dials = SongDials::default();
        let roster = dials.parts.len();
        assert!(
            dials.sections[0].parts.is_empty(),
            "the fixture starts open"
        );

        assert!(toggle_part_in_section(&mut dials, 0, "hat"));
        assert_eq!(dials.sections[0].parts.len(), roster - 1);
        assert!(!playing(&dials, 0).contains(&"hat"));
        assert!(playing(&dials, 0).contains(&"bass"));
    }

    #[test]
    fn turning_the_last_one_back_on_says_everything_rather_than_listing_it() {
        // Stored as a list, a section naming all six parts would go on naming six when a seventh
        // is added — and would be the one section the new part silently does not play in.
        let mut dials = SongDials::default();
        toggle_part_in_section(&mut dials, 0, "hat");
        toggle_part_in_section(&mut dials, 0, "snare");
        assert_eq!(dials.sections[0].parts.len(), dials.parts.len() - 2);

        toggle_part_in_section(&mut dials, 0, "hat");
        toggle_part_in_section(&mut dials, 0, "snare");
        assert!(
            dials.sections[0].parts.is_empty(),
            "everything should be spelled as nothing: {:?}",
            dials.sections[0].parts
        );

        // And the proof that it matters: a part added afterwards plays there.
        add_part(&mut dials, Role::Pad);
        let added = dials.parts.last().unwrap().name.clone();
        assert!(playing(&dials, 0).contains(&added.as_str()));
    }

    #[test]
    fn the_last_part_in_a_section_cannot_be_switched_off() {
        // A section playing nothing is silence, and the list has no way to say it — the spelling
        // for an empty set is already taken by the full one. Silently storing an empty list would
        // turn every part back on, which is the opposite of what was asked for.
        let mut dials = SongDials::default();
        let names: Vec<String> = dials.parts.iter().map(|part| part.name.clone()).collect();
        for name in names.iter().skip(1) {
            assert!(toggle_part_in_section(&mut dials, 0, name));
        }
        assert_eq!(playing(&dials, 0), [names[0].as_str()]);

        assert!(
            !toggle_part_in_section(&mut dials, 0, &names[0]),
            "the last part went out"
        );
        assert_eq!(playing(&dials, 0), [names[0].as_str()]);
    }

    #[test]
    fn a_sections_roster_is_stored_down_the_mixer_rather_than_in_click_order() {
        // The specification is a document somebody reads. A list in the order the rows happened
        // to be clicked reads as noise next to a roster that is in a deliberate order.
        let mut dials = SongDials::default();
        for name in ["hat", "lead", "kick"] {
            toggle_part_in_section(&mut dials, 0, name);
        }
        for name in ["kick", "hat"] {
            toggle_part_in_section(&mut dials, 0, name);
        }
        let order: Vec<&str> = dials.parts.iter().map(|part| part.name.as_str()).collect();
        let stored = &dials.sections[0].parts;
        let expected: Vec<&&str> = order.iter().filter(|name| **name != "lead").collect();
        assert_eq!(stored.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn a_section_that_sits_a_part_out_still_sits_it_out_after_a_save() {
        // The seam the whole feature crosses: the sheet edits a set, the format writes a list,
        // and the composer reads the list back. A section whose roster did not survive the file
        // would be a button that appears to work in the sheet and does nothing to the song.
        let mut dials = SongDials::default();
        toggle_part_in_section(&mut dials, 0, "hat");
        toggle_part_in_section(&mut dials, 0, "snare");
        let before = playing(&dials, 0);

        let written = song_spec(&dials).to_toml();
        let back = song_dials(&SongSpec::parse(&written).expect("the sheet writes valid TOML"));
        assert_eq!(playing(&back, 0), before, "\n{written}");
        assert_eq!(back, dials);
    }

    #[test]
    fn a_section_pinned_to_a_tempo_says_so_and_one_following_the_song_does_not() {
        // The em dash is the point. Printing the song's tempo on a section that has not pinned
        // one would be four rows all reading `120` with no way to see which of them would move if
        // the song did, which is the one thing the control is about.
        let mut dials = SongDials::default();
        assert_eq!(
            section_tempo_label(&dials.sections[0], auris_i18n::Language::English),
            "Follow the Song"
        );
        assert_eq!(
            section_tempo_label(&dials.sections[0], auris_i18n::Language::Japanese),
            "曲に合わせる"
        );
        dials.sections[0].tempo = Some(132.0);
        assert_eq!(
            section_tempo_label(&dials.sections[0], auris_i18n::Language::Japanese),
            "132 BPM"
        );
    }

    #[test]
    fn a_pinned_section_tempo_reaches_the_specification_and_comes_back() {
        // The sheet holds an `Option` and the format writes a line only when there is one, so a
        // section following the song has to come back following it rather than pinned to whatever
        // the song happened to be at the moment it was saved.
        let mut dials = SongDials {
            tempo: 96.0,
            ..SongDials::default()
        };
        dials.sections[1].tempo = Some(132.0);

        let spec = song_spec(&dials);
        assert_eq!(
            spec.tempo_of(&spec.sections[&dials.sections[1].name]),
            132.0
        );
        assert_eq!(spec.tempo_of(&spec.sections[&dials.sections[0].name]), 96.0);

        let written = spec.to_toml();
        let back = song_dials(&SongSpec::parse(&written).expect("the sheet writes valid TOML"));
        assert_eq!(back.sections[1].tempo, Some(132.0));
        assert_eq!(back.sections[0].tempo, None, "\n{written}");
        assert_eq!(back, dials);
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
                let back = dial.fraction(&part, Mood::default());
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
    fn the_gain_dial_covers_the_specifications_full_legal_range() {
        let mut part = SongDials::default().parts.remove(0);
        for gain in [-60.0, -45.0, 0.0, 12.0] {
            part.gain_db = gain;
            let fraction = PartDial::Gain.fraction(&part, Mood::default());
            PartDial::Gain.set(&mut part, fraction);
            assert!((part.gain_db - gain).abs() < 1e-4, "gain {gain}");
        }
    }

    #[test]
    fn part_names_move_or_leave_every_section_with_their_parts() {
        let mut renamed = peopled();
        let old = renamed.parts[0].name.clone();
        renamed.sections[0].parts = vec![old.clone()];
        renamed.sections[0]
            .tweaks
            .insert(old.clone(), Default::default());
        assert!(rename_part(&mut renamed, 0, "new name"));
        assert_eq!(renamed.sections[0].parts, ["new name"]);
        assert!(renamed.sections[0].tweaks.contains_key("new name"));
        assert!(!renamed.sections[0].tweaks.contains_key(&old));
        let spec = song_spec(&renamed);
        assert!(SongSpec::parse(&spec.to_toml()).is_ok());

        let mut removed = peopled();
        let gone = removed.parts[0].name.clone();
        removed.sections[0].parts = vec![gone.clone(), removed.parts[1].name.clone()];
        removed.sections[0]
            .tweaks
            .insert(gone.clone(), Default::default());
        assert!(remove_part(&mut removed, 0));
        assert!(!removed.sections[0].parts.contains(&gone));
        assert!(!removed.sections[0].tweaks.contains_key(&gone));
        let spec = song_spec(&removed);
        assert!(SongSpec::parse(&spec.to_toml()).is_ok());
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
    fn fresh_section_numbers_skip_compact_and_spaced_preset_identifiers() {
        let mut dials = song_dials(&preset("pop-band").unwrap().spec());
        assert_eq!(unused_section_name(&dials, "pre"), "pre 3");
        assert_eq!(unused_section_name(&dials, "chorus"), "chorus 4");
        add_to_form(&mut dials, 0, "pre 3");
        assert_eq!(unused_section_name(&dials, "pre"), "pre 4");
        add_to_form(&mut dials, 0, "bridge2");
        assert_eq!(unused_section_name(&dials, "bridge"), "bridge 3");
        assert!(dials.sections.iter().any(|s| s.name == "pre2"));
        assert!(dials.sections.iter().any(|s| s.name == "pre 3"));
        assert_eq!(
            SongSpec::parse(&song_spec(&dials).to_toml()).unwrap(),
            song_spec(&dials)
        );
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
    fn leaving_a_section_to_the_composer_files_the_request_under_its_own_name() {
        let mut dials = SongDials::default();
        let verse = dials
            .sections
            .iter()
            .position(|section| section.name == "verse")
            .unwrap();
        let chorus = dials
            .sections
            .iter()
            .position(|section| section.name == "chorus")
            .unwrap();

        assert!(invent_section_chart(&mut dials, chorus));
        assert_eq!(dials.sections[chorus].chords, "chorus");
        assert!(
            dials
                .charts
                .iter()
                .find(|(name, _)| name == "chorus")
                .is_some_and(|(_, chart)| chart.is_unwritten()),
            "what is filed is the request, not any particular deal"
        );

        // Two sections left to the composer are two requests — two names, so two progressions
        // when the song is written. One おまかせ chorus quietly deciding the verse's chords is
        // not what anyone picking おまかせ for the verse meant.
        assert!(invent_section_chart(&mut dials, verse));
        assert_ne!(dials.sections[verse].chords, dials.sections[chorus].chords);

        // The request survives the file, which is what keeps the seed dial re-dealing it.
        let spec = song_spec(&dials);
        assert_eq!(SongSpec::parse(&spec.to_toml()), Ok(spec));

        // And a section past the list is refused rather than filed somewhere.
        assert!(!invent_section_chart(&mut dials, 99));
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
