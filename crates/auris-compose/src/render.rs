//! Turning the parts into clips a document can hold.

use auris_core::automation::AutomationPoint;
use auris_core::harmony::{ChordMap, ChordPoint, Harmony, KeyMap, KeyPoint};
use auris_core::plugin::PluginState;
use auris_core::project::Color;
use auris_core::structure::{SectionMap, SectionPoint};
use auris_core::time::{TempoMap, TempoPoint, Ticks, TimeSignature};
use auris_core::{ClipRecipe, Note, NoteTransform};

use crate::frame::{Frame, plan};
use crate::parts::{PartDraft, ScoreSettings, write_parts};
use crate::perform::part_performance;
#[cfg(test)]
use crate::phrase::SEED_RANGE;
use crate::phrase::clip_seed;
use crate::phrase::recipe_for;
use crate::spec::{Ending, PartSpec, Role, SongSpec};

/// The bus a whole kit sits under, so one fader moves the drums.
const DRUM_BUS: &str = "Drums";

/// The room the pitched parts share, fed by sends.
const ROOM_BUS: &str = "Room";

/// The reverb the room bus carries.
const REVERB_ID: &str = "auris.fx.reverb";

/// The chorus an electric comp part carries — see `inserts_for`.
const CHORUS_ID: &str = "auris.fx.chorus";

/// One clip: a run of notes with a place on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipDraft {
    /// What the clip is called.
    pub name: String,
    /// Where it starts in the song.
    pub start: Ticks,
    /// How long it lasts.
    pub length: Ticks,
    /// Its notes, positioned from the clip's own start.
    pub notes: Vec<Note>,
    /// What the clip is, in the vocabulary a person can edit.
    ///
    /// Carried so that a composed piece arrives as clips that know how they were played, and can
    /// therefore be re-taken, re-dialled and frozen one at a time — the same commands a clip
    /// written by hand from *Write a Part Here…* answers to. Before this a composed song was four
    /// hundred notes with nothing to say about themselves, and the only granularity on offer was
    /// composing the whole piece again.
    ///
    /// `None` for a part no preset names — the crash, which is written against the joins of the
    /// form rather than from a recipe. See [`recipe_for`], which is also where the limits of what
    /// this promises are set out.
    pub recipe: Option<ClipRecipe>,
    /// How the clip's text is played: the transform stack it arrives carrying.
    ///
    /// The notes are the score and this is the feel — the specification's `humanize` dial,
    /// delivered as the per-part lean and wander [`crate::perform`] tables rather than baked
    /// into the notes. It is the clip's from the moment it lands: turning it is the performance
    /// panel's business, and writing the clip's text again leaves it alone.
    pub performance: Vec<NoteTransform>,
}

/// One track: an instrument and the clips it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackDraft {
    /// The track's name.
    pub name: String,
    /// The plugin that plays it, when no [`Self::sound`] names a SoundFont one.
    pub instrument: String,
    /// The General MIDI sound the part asked for, if it asked for one.
    ///
    /// A bank and a patch rather than a preset, because the composer has no font to name one in.
    /// The session resolves it against whichever General MIDI font is installed — and falls back
    /// to [`Self::instrument`] when there is none, which is why both are here.
    pub sound: Option<crate::gm::Sound>,
    /// The colour the track is drawn in, chosen by the part's role.
    pub color: Color,
    /// Parameters the part's role needs [`Self::instrument`] set to, if any.
    ///
    /// Empty for almost every part, and that is the intended state: a role picks an instrument and
    /// then leaves it sounding how it sounds. What this is for is the case where the role and the
    /// instrument together mean something the instrument's own defaults do not — today that is the
    /// crash cymbal on the built-in noise drum, and `voicing_for` is where the exception is argued.
    ///
    /// It has nothing to say about a part that landed on a SoundFont. A sound out of a font is a
    /// recording of the thing itself, and there is no parameter on a sampler that would make it
    /// more of one.
    pub state: PluginState,
    /// Level trim in decibels.
    pub gain_db: f32,
    /// How loud this part should end up, in LUFS, measured on its own.
    ///
    /// [`Role::target_lufs`] for what the part plays, moved by however far the specification asked
    /// this part to sit from its role's usual place. The two halves matter separately: the first
    /// is the balance a band strikes and the second is what *this* piece wants — a jazz kit
    /// written six decibels down is a decision about brushes, and a balance pass that pushed it
    /// back up to where a rock kit sits would be overruling the preset with an average.
    ///
    /// [`Self::gain_db`] is where the fader starts and this is where it is trying to get to. They
    /// are not the same statement and cannot be: a fader is a number about a signal path, and this
    /// is a number about a sound.
    pub target_lufs: f32,
    /// Stereo position.
    pub pan: f32,
    /// Which bus this track's output goes to, by position in [`Composition::buses`].
    ///
    /// `None` for the master. A position rather than a name or an id, because the composer has
    /// neither: ids belong to a document and do not exist until one is built.
    pub output: Option<usize>,
    /// Copies of this track fed to buses alongside its own output.
    pub sends: Vec<SendDraft>,
    /// Insert effects on the track's own strip, in chain order.
    ///
    /// Almost always empty, for the same reason [`Self::state`] almost always is: a part picks a
    /// sound and is then left sounding how it sounds. What earns an insert is a *pairing* — a
    /// role and a sound that idiomatically arrive through a pedal — and `inserts_for` is where
    /// each pairing is argued.
    pub effects: Vec<EffectDraft>,
    /// The clips, in time order.
    pub clips: Vec<ClipDraft>,
}

/// A copy of a track, on its way to a bus.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SendDraft {
    /// Which bus, by position in [`Composition::buses`].
    pub bus: usize,
    /// How much of the track goes there, in decibels.
    pub level_db: f32,
}

/// A mixing point the composed arrangement routes through.
#[derive(Clone, Debug, PartialEq)]
pub struct BusDraft {
    /// The bus's name.
    pub name: String,
    /// The colour the bus is drawn in.
    pub color: Color,
    /// Level trim in decibels.
    pub gain_db: f32,
    /// Effects it carries, in chain order.
    pub effects: Vec<EffectDraft>,
}

/// One effect in a chain — a bus's or a track's — and the parameters it should not be left at
/// its defaults for.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectDraft {
    /// Registry id.
    ///
    /// Named rather than instantiated for the same reason a track's instrument is: this crate
    /// knows what a reverb is *for* and has never heard of the one that ships.
    pub id: String,
    /// Parameter values, by their stable key — the document's own shape, so nothing translates it.
    pub state: PluginState,
}

/// A finished piece, ready to become a project.
#[derive(Clone, Debug, PartialEq)]
pub struct Composition {
    /// What the piece is called.
    pub title: String,
    /// The tempo, on the song's own timeline.
    ///
    /// A map rather than a number because a section may lift or drop away from the song's tempo,
    /// and the document already holds one — handing over the core type means nothing translates
    /// it, the same trade [`Self::harmony`] and [`Self::sections`] make.
    ///
    /// It is piecewise-constant, and that is the honest shape of what the composer can say: a
    /// specification names a tempo per *section*, and a section is a stretch of bars rather than a
    /// moment. Slowing through a passage is a continuous change and neither this nor the
    /// specification pretends to have one.
    pub tempo_map: TempoMap,
    /// The time signature.
    ///
    /// One for the whole piece. A specification says `meter = "6/8"` once, and there is no
    /// vocabulary for changing it part way through — unlike the tempo, where a section can. The
    /// difference is not arbitrary: a change of meter changes the length of a bar, and every part
    /// is written against one grid.
    pub meter: TimeSignature,
    /// How long the piece is.
    pub length: Ticks,
    /// The seed it was written from, so it can be written again.
    pub seed: u64,
    /// The specification it was written from, as the document would hold it.
    ///
    /// Carried rather than thrown away so that a project can remember what it was asked for: a
    /// song sheet reopened after a save and a reload refills itself from this, and Another Take
    /// goes on working on a piece nobody has the original file for. Text rather than a
    /// [`SongSpec`] because the document may not name this crate, and because the format is
    /// already the canonical way of writing one down — nothing is lost by storing it as one.
    pub spec: String,
    /// The key and the chords, on the song's own timeline.
    ///
    /// The same harmony every part was written against, handed over rather than thrown away: it
    /// is what the harmony lane draws, and what a clip generated later reads so that a part added
    /// by hand agrees with the ones the composer wrote.
    pub harmony: Harmony,
    /// The sections, on the song's own timeline.
    ///
    /// A composed piece knows what its stretches are called, and a clip written into a stretch
    /// draws its figures from that label — so a part added to a finished song comes out belonging
    /// to the same サビ as the rest of it.
    pub sections: SectionMap,
    /// The tracks, in the order the parts were declared.
    pub tracks: Vec<TrackDraft>,
    /// The buses the tracks route through, in the order they should be created.
    ///
    /// A rough mix rather than a finished one: a kit under one fader and a room the pitched parts
    /// share. What it is not is a substitute for mixing — it is the state a person would have
    /// spent the first ten minutes setting up before they could hear whether the piece was any
    /// good, which is ten minutes of not listening to it.
    pub buses: Vec<BusDraft>,
    /// Points the master fader is asked to ride through, in decibels; empty leaves it alone.
    ///
    /// The composer's first automation lane, and today the whole of it: a fade-out ending
    /// ([`Ending::Fade`](crate::spec::Ending)) is these points and nothing else. Handed over as
    /// data rather than written into the document, for the reason everything here is — the
    /// composer knows no track ids, and the session is the layer that turns a draft into a
    /// project. When the composer learns to ride more than one fader this becomes a list of
    /// lanes; until then a single fader's points do not need a vocabulary for naming faders.
    pub master_gain: Vec<AutomationPoint>,
}

impl Composition {
    /// How many notes the piece contains.
    pub fn note_count(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .map(|clip| clip.notes.len())
            .sum()
    }

    /// A one-line-per-track summary, for a command line to print.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} · {:.0} BPM · {}/{} · {} bars · seed {}\n",
            self.title,
            self.tempo_map.initial_bpm(),
            self.meter.numerator,
            self.meter.denominator,
            self.length.raw() / self.meter.ticks_per_bar().raw().max(1),
            self.seed
        );
        for track in &self.tracks {
            // What the track will actually play. Printing the plugin id under a part that asked
            // for a violin would name the fallback and never the sound.
            let voice = match track.sound {
                Some(sound) => crate::gm::Program(sound.patch)
                    .label(sound.bank == crate::gm::DRUM_BANK)
                    .to_string(),
                None => track.instrument.clone(),
            };
            out.push_str(&format!(
                "  {:<12} {:<24} {} clips, {} notes\n",
                track.name,
                voice,
                track.clips.len(),
                track
                    .clips
                    .iter()
                    .map(|clip| clip.notes.len())
                    .sum::<usize>()
            ));
        }
        out
    }
}

/// Writes a piece from its specification.
///
/// The whole crate is this one function: a spec in, notes out, with nothing in between that
/// depends on the time of day. The same spec and the same seed always give the same piece.
pub fn compose(spec: &SongSpec) -> Composition {
    let frame = plan(spec);
    render(spec, &frame)
}

/// One part's notes, cut into a clip per section of the frame.
///
/// Empty when the part never plays. That is not a strange case: a groove is free to leave a voice
/// out — `sparse` writes no snare at all — so a part can be declared, written, and produce nothing.
///
/// `part` is the roster entry the draft came from, which each section may patch before playing;
/// the recipe every clip carries is derived from the patched one, so a chorus that asked for the
/// bass an octave up arrives as a clip that says so.
///
/// `looseness` is the specification's `humanize` dial, and decides the performance stack each
/// clip arrives carrying — per role and per clip seed, see [`part_performance`]. It reaches the
/// ending and the crash too: a recipe is a promise about writing the text again, but a feel is
/// how the text is played, and the landing chord is played by the same band.
fn clips_of(
    settings: &ScoreSettings,
    looseness: f32,
    part: Option<&PartSpec>,
    draft: &PartDraft,
    frame: &Frame,
) -> Vec<ClipDraft> {
    let mut clips = Vec::new();
    for (index, section) in frame.sections.iter().enumerate() {
        let mut notes: Vec<Note> = draft
            .notes
            .iter()
            .filter(|note| note.section == index)
            .filter_map(|note| {
                // Rebase onto the clip. A note the swing delayed over a section boundary is
                // clamped back rather than deleted — dropping one took the downbeat out of
                // sections back when the baked wander could nudge a note either way.
                let offset = note.start - section.start;
                if offset >= section.length {
                    return None;
                }
                let start = offset.max_zero().min(section.length - Ticks(1));
                // Truncate rather than let a note overhang: the scheduler would drop it
                // silently, and `fit_length_to_notes` would grow the clip if it did not.
                //
                // Measured from where the note *ends* and not from how long it is, so that
                // clamping a start back to the section's own does not carry the release
                // along with it: `parts::untangle` had just cut this note to where its own
                // pitch is struck again, and a note lengthened here would land back over it.
                let ends = (offset + note.length).min(section.length);
                let length = (ends - start).max(Ticks(1));
                Some(Note {
                    velocity: note.velocity.clamp(0.0, 1.0),
                    ..Note::new(note.pitch.min(127), start, length)
                })
            })
            .collect();

        // A canonical order, so two runs of the same spec compare equal byte for byte.
        notes.sort_by_key(|note| (note.start.raw(), note.pitch));

        // An empty clip is a hole in the arrangement rather than a block of silence.
        if notes.is_empty() {
            continue;
        }
        clips.push(ClipDraft {
            name: format!("{} {} · {}", section.name, section.instance, draft.name),
            start: section.start,
            length: section.length,
            notes,
            // No recipe on an ending clip: a recipe promises that another take is the same part
            // played again, and `write_phrase` would answer with a figure over the tonic rather
            // than the held landing — a promise nothing can keep is better not made, the same
            // answer the crash already gives.
            recipe: (!section.coda)
                .then_some(part)
                .flatten()
                .map(|part| section.played(part))
                .and_then(|played| recipe_for(settings, &played, section, frame.seed)),
            // The same seed the recipe carries, computed rather than read off it so that the
            // ending — which carries no recipe on purpose — is still played loose.
            performance: part.map_or_else(Vec::new, |part| {
                part_performance(
                    part.role,
                    looseness,
                    clip_seed(frame.seed, &part.name, &section.name, section.instance),
                )
            }),
        });
    }
    clips
}

/// Turns a planned frame and its parts into tracks of clips.
fn render(spec: &SongSpec, frame: &Frame) -> Composition {
    let settings = ScoreSettings::from(spec);
    let drafts = write_parts(&settings, &spec.parts, frame);
    // The roster entry each draft came from. A draft carries its part's name and not the part, and
    // the name is what ties the two together — which is enough, because a roster may not hold two
    // parts of one name.
    let part_of = |name: &str| spec.parts.iter().find(|part| part.name == name);
    let role_of = |name: &str| part_of(name).map(|part| part.role);
    // The parts that actually play, and their clips. A part that never plays leaves no track at
    // all — a groove is free to leave a voice out, and `sparse` writes no snare.
    //
    // The buses are decided from *these* rather than from the roster, which is the whole reason
    // this is a separate pass. `buses_for` promises that only the buses something reaches get
    // made, and asking the roster broke that promise: a piece whose only drum part turned out
    // silent still got a Drums bus, which then sat in the arrangement as a track with no clips on
    // it and nothing routed to it.
    let played: Vec<(PartDraft, Vec<ClipDraft>)> = drafts
        .into_iter()
        .map(|draft| {
            let clips = clips_of(
                &settings,
                spec.humanize,
                part_of(&draft.name),
                &draft,
                frame,
            );
            (draft, clips)
        })
        .filter(|(_, clips)| !clips.is_empty())
        .collect();

    let buses = buses_for(
        &played
            .iter()
            .filter_map(|(draft, _)| role_of(&draft.name))
            .collect::<Vec<_>>(),
    );
    let mut tracks = Vec::new();

    for (draft, clips) in played {
        let role = role_of(&draft.name);
        let (output, sends) = role
            .map(|role| routing_for(role, &buses))
            .unwrap_or_default();
        let state = role.map_or_else(PluginState::empty, |role| {
            voicing_for(role, &draft.instrument)
        });
        let effects = role.map_or_else(Vec::new, |role| inserts_for(role, draft.sound));
        tracks.push(TrackDraft {
            name: draft.name,
            instrument: draft.instrument,
            sound: draft.sound,
            // A part with no role in the roster cannot happen — a draft is written *from* one —
            // but the melody's colour is the honest answer if it ever did.
            color: role.unwrap_or(Role::Melody).color(),
            state,
            gain_db: draft.gain_db,
            target_lufs: {
                // Against the role's own default rather than against nothing: what the
                // specification said is the *distance* from where a part of this kind usually
                // sits, and that is the half of the level a measurement must not throw away.
                let role = role.unwrap_or(Role::Melody);
                role.target_lufs() + (draft.gain_db - role.default_gain_db())
            },
            pan: draft.pan,
            output,
            sends,
            effects,
            clips,
        });
    }

    Composition {
        title: spec.title.clone(),
        tempo_map: tempo_of(frame, spec.tempo),
        meter: spec.meter,
        length: frame.length,
        seed: spec.seed,
        spec: spec.to_toml(),
        harmony: harmony_of(frame),
        sections: sections_of(frame),
        tracks,
        buses,
        master_gain: fade_of(spec, frame),
    }
}

/// How many bars a fade-out rides down across, at most.
///
/// Eight bars of the final section — around fifteen seconds at a pop tempo, which is where
/// records put it: long enough that the fade is a gesture rather than an edit, short enough that
/// the listener is not asked to sit through a minute of leaving. A final section shorter than
/// this fades over all of itself.
const FADE_BARS: usize = 8;

/// Where the master fader lands at the end of a fade, in decibels.
///
/// Effectively silence: −60 under a −14 LUFS master is below anything a listener will catch the
/// stop of, and stopping *at* silence rather than asymptotically near it is what lets the
/// renderer's tail end the file.
const FADE_FLOOR_DB: f32 = -60.0;

/// The master fader's ride for a piece that fades out, or nothing for one that does not.
///
/// Two points, linear between them — and linear *in decibels*, which is the curve a fade-out on
/// a console draws: equal loudness lost per bar, all the way down.
fn fade_of(spec: &SongSpec, frame: &Frame) -> Vec<AutomationPoint> {
    if spec.ending != Ending::Fade {
        return Vec::new();
    }
    let Some(last) = frame.sections.last() else {
        return Vec::new();
    };
    let over = frame
        .grid
        .bar_ticks()
        .max(Ticks(1))
        .raw()
        .saturating_mul(FADE_BARS as i64)
        .min(last.length.raw());
    vec![
        AutomationPoint::new(frame.length - Ticks(over), 0.0),
        AutomationPoint::new(frame.length, FADE_FLOOR_DB),
    ]
}

/// The buses a roster needs, in the order they should be created.
///
/// Only the ones something actually goes to: a piece with no drums has no drum bus, and one with
/// nothing pitched in it has no room. A bus nobody feeds is a strip in the mixer that can only be
/// confusing.
fn buses_for(roles: &[Role]) -> Vec<BusDraft> {
    let mut buses = Vec::new();
    if roles.iter().any(|role| drum_bus_takes(*role)) {
        buses.push(BusDraft {
            name: DRUM_BUS.to_string(),
            // The kit's own hue, so the fader that moves the drums is the colour of the drums.
            color: Role::Snare.color(),
            // The parts carry their own balance; the bus is here to move all of it at once.
            gain_db: 0.0,
            effects: Vec::new(),
        });
    }
    if roles.iter().any(|role| room_send_db(*role).is_some()) {
        let mut state = PluginState::empty();
        // Fully wet. A send bus carries the *reflections* — the dry signal is already on its way
        // to the master by its own path, and a bus at the shipped 30 % mix would add a second
        // quieter copy of it, which is a comb filter rather than a room.
        state.params.insert("mix".to_string(), 1.0);
        state.params.insert("room_size".to_string(), 0.55);
        buses.push(BusDraft {
            name: ROOM_BUS.to_string(),
            // Grey, and the only grey in the arrangement: the room is not a part, and a hue would
            // put it in a family with one of them.
            color: Color(0x8792a2),
            // The sends set how much goes in; this sets how loud what comes back is.
            gain_db: -3.0,
            effects: vec![EffectDraft {
                id: REVERB_ID.to_string(),
                state,
            }],
        });
    }
    buses
}

/// The parameters a role needs set on the instrument it also named.
///
/// Keyed by plugin id, exactly as the room bus names `auris.fx.reverb` before setting its mix:
/// this crate knows what a crash cymbal *is* and has never heard of the oscillator that ships. A
/// part on anybody else's plugin gets nothing, because `decay = 1.8` means one thing on the noise
/// drum and could mean anything at all elsewhere.
///
/// # Why only the cymbal
///
/// `auris.synth.noisedrum` is one algorithm — noise through a band-pass swept down from where the
/// note puts it — and the whole built-in kit is that algorithm at its shipped defaults, told apart
/// only by which General MIDI note each part strikes. Measured, one hit each at 48 kHz:
///
/// | Part | note | spectral centroid | 40 dB down at |
/// |---|---|---|---|
/// | Kick | 36 | 190 Hz | 115 ms |
/// | Snare | 38 | 215 Hz | 460 ms |
/// | Hi-hat | 42 | 246 Hz | 285 ms |
///
/// Three low thuds within 56 Hz of each other, which is not a kit, and the hi-hat is the plainest
/// case: nothing about 246 Hz is a hi-hat. That is worth saying here because it is *not* what this
/// function fixes. Those three have sounded like that since the composer could write them, they
/// are what the one preset on the built-in voices sounds like today, and changing them is a
/// decision about how that preset should sound rather than a defect in a part being added.
///
/// The cymbal is different only because it is new. A part that has never had a sound has no sound
/// to preserve, and shipping it at the defaults would be shipping a fourth thud — 342 Hz, 595 ms,
/// which is a low tom — under the name of a crash.
///
/// # What the numbers are
///
/// `tone` is stated at MIDI 60 and transposed by the note struck, so a crash written at 49 sounds
/// it 11 semitones down: the parameter's ceiling of 8 kHz arrives as 4.2 kHz, which is where a
/// crash lives. It is at the ceiling rather than near it because the ceiling is the constraint —
/// a hi-hat would want 7 kHz and cannot have it, since 42 is a further seven semitones down.
///
/// No sweep: the downward pitch move is what reads as a drum head losing tension, and a cymbal has
/// no head. A decay of 1.8 s is most of the range the parameter allows and about what a crash
/// rings for.
///
/// The level is not a taste and is the reason the other three are the numbers they are. Opening
/// the band-pass at 4.2 kHz and stopping it sweeping lets through far more of the noise than the
/// shipped voicing does, and the hit arrived 13.5 dB over the built-in snare measured as RMS
/// across its first 300 ms — which is the measure that matters here, because a cymbal's peak is a
/// fraction of what a listener hears of it. The five General MIDI kits the presets use put their
/// crash within 1.4 dB of their own snare by that measure, and `-19.5` is what puts the built-in
/// one in the same place. What separates a cymbal from a backbeat afterwards is
/// [`Role::default_gain_db`], on both sides alike.
fn voicing_for(role: Role, instrument: &str) -> PluginState {
    let mut state = PluginState::empty();
    if role == Role::Crash && instrument == "auris.synth.noisedrum" {
        state.params.insert("tone".to_string(), 8_000.0);
        state.params.insert("sweep".to_string(), 0.0);
        state.params.insert("decay".to_string(), 1.8);
        state.params.insert("level".to_string(), -19.5);
    }
    state
}

/// The insert effects a part earns from the sound it landed on.
///
/// Keyed by role *and* General MIDI patch, on the same reasoning as [`voicing_for`]: an insert
/// is right where the pairing is idiomatic, not where either half is alone. Today there is one
/// pairing. A **chords part on an electric piano or an undistorted electric guitar** gets a
/// chorus, because that pairing barely exists without one: the Rhodes-and-chorus comp is the
/// centre of the city-pop sound the preset names, and a clean guitar chording through a chorus
/// pedal is how that instrument has been recorded since the pedal was invented.
///
/// Nobody else. A melody on the same Rhodes is a voice, not a bed, and widening it moves the
/// singer to the back of the stage; an acoustic piano through a chorus is a piano out of tune;
/// distorted guitars keep their edge dry. A part that stayed on a built-in synth has no patch to
/// argue from and gets nothing.
///
/// The mix is a third rather than the plugin's half because an insert wets the *whole* part:
/// at 0.5 the comp audibly smears, and a bed that used to sit still starts to wobble. Measured
/// on the city-pop comp's stem, 0.35 takes the left–right correlation from 0.79 to 0.62 — a
/// widening a meter can see — while the Audiobox axes on the two presets the rule touches move
/// by at most 0.03, at the noise floor: the learned ear neither rewards the stereo (it barely
/// looks there) nor finds anything to object to.
///
/// The blend also costs level — dry and wet are incoherent, so at 0.35 they sum by power,
/// about 3 dB of RMS down — and *nothing here compensates*. Composing ends by rendering every
/// part alone and setting the faders from what was measured (`Session::balance_levels`), so the
/// insert's cost is heard and paid there, with the part's target loudness unmoved.
fn inserts_for(role: Role, sound: Option<crate::gm::Sound>) -> Vec<EffectDraft> {
    let Some(sound) = sound else {
        return Vec::new();
    };
    // Electric Piano 1 and 2, Electric Guitar (jazz) and (clean) — pinned by name in a test.
    let electric_comp = sound.bank != crate::gm::DRUM_BANK && [4, 5, 26, 27].contains(&sound.patch);
    if role == Role::Chords && electric_comp {
        let mut state = PluginState::empty();
        state.params.insert("mix".to_string(), 0.35);
        return vec![EffectDraft {
            id: CHORUS_ID.to_string(),
            state,
        }];
    }
    Vec::new()
}

/// `true` when a role belongs under the drum fader.
fn drum_bus_takes(role: Role) -> bool {
    matches!(role, Role::Kick | Role::Snare | Role::Hat | Role::Crash)
}

/// How much of a role goes to the room, in decibels, or `None` for a part that stays dry.
///
/// **More room is further away.** That is the whole of the ordering: the pad is the furthest back
/// because being a wash rather than a chord is what makes it a bed, and the tune is the nearest
/// because it is the thing being sung. A part in front gets *less* of the room, not more.
///
/// Low frequencies in a reverb are mud, and there is no version of a kick or a bass in a room that
/// a mix is better for, so those two get none at all.
fn room_send_db(role: Role) -> Option<f32> {
    Some(match role {
        // Furthest back, in order.
        Role::Pad => -6.0,
        Role::Chords => -10.0,
        // A stab is a chord with its release cut off, and what is left after the cut is the room.
        Role::Stab => -10.0,
        Role::Arp => -12.0,
        // Nearest of the pitched parts.
        Role::Melody => -15.0,
        // A snare in a room is the oldest trick there is; a hat wants a suggestion of one.
        Role::Snare => -12.0,
        Role::Hat => -20.0,
        // A crash is mostly its own decay, and a dry one stops dead where the room lets it spill
        // into the bar it opened. More than the snare gets, because it is further back in the kit
        // and because the tail is the point of the sound rather than a side effect of it.
        Role::Crash => -10.0,
        // The riser sits with the crash: it is the same cymbal run the other way, announcing the
        // bar the crash then opens, and the pair should sound like they share a room.
        Role::Riser => -10.0,
        Role::Bass | Role::Kick => return None,
    })
}

/// Where one role's output goes and what it sends, given the buses [`buses_for`] produced.
///
/// By name rather than by a position worked out twice: the two functions are read together and a
/// second copy of "the drum bus is the first one when there are drums" is a second chance to be
/// wrong about it.
fn routing_for(role: Role, buses: &[BusDraft]) -> (Option<usize>, Vec<SendDraft>) {
    let index_of = |name: &str| buses.iter().position(|bus| bus.name == name);
    let output = drum_bus_takes(role).then(|| index_of(DRUM_BUS)).flatten();
    let sends = room_send_db(role)
        .zip(index_of(ROOM_BUS))
        .map(|(level_db, bus)| SendDraft { bus, level_db })
        .into_iter()
        .collect();
    (output, sends)
}

/// The frame's tempo, as the map a document holds.
///
/// Only where it changes, on the same rule the key lane follows: every section carries a tempo and
/// most pieces are at one throughout, so a point per section would fill the tempo lane with
/// changes that change nothing and leave a person hunting for the one that does.
///
/// A frame with no sections still answers a map — `TempoMap` cannot be empty and the piece has to
/// play at *something* — so the fallback is the tempo the specification named, which is the only
/// number there is.
fn tempo_of(frame: &Frame, song: f64) -> TempoMap {
    let mut points: Vec<TempoPoint> = Vec::new();
    for section in &frame.sections {
        if points.last().is_none_or(|last| last.bpm != section.tempo) {
            points.push(TempoPoint {
                tick: section.start,
                bpm: section.tempo,
            });
        }
    }
    TempoMap::try_from(points).unwrap_or_else(|_| TempoMap::constant(song))
}

/// The frame's harmony, moved onto the song's own timeline.
///
/// A [`SectionPlan`](crate::frame::SectionPlan) positions its chords from its *own* start, which
/// is the frame of reference a clip wants; a document wants them where they sound. This is the one
/// place the two meet, and it is why the offset is added exactly once.
fn harmony_of(frame: &Frame) -> Harmony {
    let mut keys: Vec<KeyPoint> = Vec::new();
    let mut chords: Vec<ChordPoint> = Vec::new();

    for section in &frame.sections {
        // Only where it changes. Every section carries a key, and most songs are in one
        // throughout — a point per section would fill the lane with changes that change nothing.
        if keys.last().is_none_or(|last| last.key != section.key) {
            keys.push(KeyPoint {
                tick: section.start,
                key: section.key,
            });
        }
        for event in &section.events {
            chords.push(ChordPoint {
                tick: section.start + event.start,
                chord: Some(event.numeral),
            });
        }
    }

    // Past the last bar there is no harmony, rather than the final chord ringing on for ever.
    // That is what a `None` point is for, and without it a clip written after the song's end
    // would be given chords the piece does not have.
    if !chords.is_empty() {
        chords.push(ChordPoint {
            tick: frame.length,
            chord: None,
        });
    }

    Harmony {
        keys: KeyMap::from_points(keys),
        chords: ChordMap::new(chords),
    }
}

/// The frame's sections, as the labels a timeline carries.
///
/// The instance numbers are not stored: [`SectionMap`] counts them from the start of the song, so
/// writing them down would be a second copy of a fact that can only disagree with the first.
fn sections_of(frame: &Frame) -> SectionMap {
    let mut points: Vec<SectionPoint> = frame
        .sections
        .iter()
        .map(|section| SectionPoint {
            tick: section.start,
            label: Some(section.name.clone()),
        })
        .collect();
    // The stretch after the last section is a real thing, not an absence: the song has ended.
    if !points.is_empty() {
        points.push(SectionPoint {
            tick: frame.length,
            label: None,
        });
    }
    SectionMap::new(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose_text(text: &str) -> Composition {
        compose(&SongSpec::parse(text).expect("the fixture parses"))
    }

    const BASE: &str = r#"
        title = "Test"
        form = "intro verse chorus"
        chords = "@axis"
        humanize = 0
        [section.intro]
        bars = 4
        [section.verse]
        bars = 8
        [section.chorus]
        bars = 8
    "#;

    #[test]
    fn every_clip_a_piece_arrives_with_knows_what_it_is() {
        // The whole point: a composed song is clips that can be re-taken one at a time, not four
        // hundred anonymous notes whose only granularity is composing the piece again.
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            for clip in &track.clips {
                let Some(recipe) = &clip.recipe else {
                    // The crash is the one part no preset names, and an ending clip makes a
                    // promise nothing could keep: `write_phrase` would answer with a figure over
                    // the tonic rather than the held landing. Nothing else may be missing one.
                    assert!(
                        track.name == "crash" || clip.name.starts_with("ending"),
                        "`{}` arrived with no recipe on `{}`",
                        track.name,
                        clip.name
                    );
                    continue;
                };
                assert!(recipe.density > 0.0, "`{}` has no density", track.name);
                assert!((-2..=2).contains(&recipe.octave));
            }
        }
    }

    #[test]
    fn each_clip_gets_a_seed_of_its_own_and_the_same_one_every_time() {
        // A seed per clip is what "individually" means. The song's own seed on every clip would
        // have made one re-roll land on the number its neighbour's next re-roll would take.
        let seeds = |text: &str| -> Vec<(String, String, u64)> {
            compose_text(text)
                .tracks
                .iter()
                .flat_map(|track| {
                    track.clips.iter().filter_map(move |clip| {
                        Some((
                            track.name.clone(),
                            clip.name.clone(),
                            clip.recipe.as_ref()?.seed,
                        ))
                    })
                })
                .collect()
        };

        let first = seeds(BASE);
        assert!(first.len() > 4, "not enough clips to say anything");
        let distinct: std::collections::BTreeSet<u64> =
            first.iter().map(|(_, _, seed)| *seed).collect();
        assert_eq!(
            distinct.len(),
            first.len(),
            "two clips of one piece share a seed"
        );
        // And it is a function of the specification, like everything else here.
        assert_eq!(first, seeds(BASE));
        // Short enough to be read off the recipe panel and typed back in, which is the only way
        // a take somebody liked can be got back to.
        assert!(
            first
                .iter()
                .all(|(_, _, seed)| (1..=SEED_RANGE).contains(seed)),
            "a seed nobody could retype: {first:?}"
        );
    }

    #[test]
    fn a_section_that_patches_a_part_is_described_by_the_patched_recipe() {
        // A recipe recording the roster's answer would describe a clip that is not the one on the
        // timeline. What a chorus asked for is what the chorus clip says it is.
        let piece = compose_text(
            r#"
            title = "Patched"
            form = "verse chorus"
            chords = "@axis"
            [section.verse]
            bars = 4
            [section.chorus]
            bars = 4
            [section.chorus.part.bass]
            octave = 4
            "#,
        );
        let bass = piece
            .tracks
            .iter()
            .find(|track| track.name == "bass")
            .expect("the default roster has a bass");
        let octave_of = |section: &str| {
            bass.clips
                .iter()
                .find(|clip| clip.name.starts_with(section))
                .and_then(|clip| clip.recipe.as_ref())
                .map(|recipe| recipe.octave)
                .unwrap_or_else(|| panic!("no {section} clip"))
        };
        assert_ne!(
            octave_of("chorus"),
            octave_of("verse"),
            "the chorus lifted the bass and its clip does not say so"
        );
    }

    #[test]
    fn a_piece_arrives_with_a_kit_under_one_fader_and_a_room_to_share() {
        let piece = compose_text(BASE);
        let names: Vec<&str> = piece.buses.iter().map(|bus| bus.name.as_str()).collect();
        assert_eq!(names, vec!["Drums", "Room"]);

        let track = |name: &str| {
            piece
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap_or_else(|| panic!("no {name} track"))
        };
        // The kit goes under its own fader; the pitched parts go straight to the master.
        for drum in ["kick", "snare", "hat"] {
            assert_eq!(track(drum).output, Some(0), "{drum}");
        }
        assert_eq!(track("lead").output, None);

        // Everything but the low end sends to the room, and more room is further away.
        let send = |name: &str| track(name).sends.first().map(|send| send.level_db);
        assert!(send("lead") < send("chords"), "the tune sits in front");
        assert_eq!(send("bass"), None, "low frequencies in a reverb are mud");
        assert_eq!(send("kick"), None);
        assert_eq!(track("snare").sends[0].bus, 1, "the room, not the drum bus");
    }

    #[test]
    fn more_room_is_further_away() {
        // The whole of the ordering, on the policy itself rather than on a roster that happens to
        // hold six of the nine roles. A pad is a wash and that is what makes it a bed; a tune is
        // the thing being sung and sits in front of all of it.
        let db = |role| room_send_db(role).expect("a pitched part sends to the room");
        assert!(db(Role::Melody) < db(Role::Arp));
        assert!(db(Role::Arp) < db(Role::Chords));
        assert!(db(Role::Chords) <= db(Role::Stab));
        assert!(db(Role::Stab) < db(Role::Pad));
        assert!(db(Role::Hat) < db(Role::Snare), "a hat wants a suggestion");
        assert_eq!(room_send_db(Role::Bass), None);
        assert_eq!(room_send_db(Role::Kick), None);
    }

    #[test]
    fn a_room_bus_carries_a_reverb_that_is_all_reflection() {
        // The shipped reverb passes 70 % of its input through dry. On a send bus that is a second,
        // quieter copy of a signal already on its way to the master by another path — a comb
        // filter rather than a room.
        let piece = compose_text(BASE);
        let room = piece
            .buses
            .iter()
            .find(|bus| bus.name == "Room")
            .expect("a room");
        let reverb = &room.effects[0];
        assert_eq!(reverb.id, "auris.fx.reverb");
        assert_eq!(reverb.state.params.get("mix"), Some(&1.0));
    }

    #[test]
    fn only_the_electric_comp_arrives_through_a_chorus() {
        // The pairing earns the insert, not either half of it alone.
        let sound = |patch| Some(crate::gm::Sound { bank: 0, patch });
        let chorused = |role, sound| !inserts_for(role, sound).is_empty();
        assert!(chorused(Role::Chords, sound(4)), "a Rhodes comp");
        assert!(chorused(Role::Chords, sound(27)), "a clean guitar comp");
        assert!(!chorused(Role::Melody, sound(4)), "a voice, not a bed");
        assert!(
            !chorused(Role::Chords, sound(0)),
            "an acoustic piano through a chorus is a piano out of tune"
        );
        assert!(
            !chorused(Role::Chords, sound(30)),
            "a distorted guitar keeps its edge dry"
        );
        assert!(
            !chorused(Role::Chords, None),
            "a built-in synth has no patch to argue from"
        );

        let inserts = inserts_for(Role::Chords, sound(4));
        assert_eq!(inserts[0].id, "auris.fx.chorus");
        assert_eq!(inserts[0].state.params.get("mix"), Some(&0.35));

        // The patches the rule names, pinned to the names they were chosen for — a renumbering
        // of the General MIDI table would otherwise move the pedal to four other instruments.
        for (patch, name) in [
            (4, "Electric Piano 1"),
            (5, "Electric Piano 2"),
            (26, "Electric Guitar (jazz)"),
            (27, "Electric Guitar (clean)"),
        ] {
            assert_eq!(crate::gm::Program(patch).name(), name);
        }
    }

    #[test]
    fn a_piece_with_no_drums_has_no_drum_bus() {
        // A bus nobody feeds is a strip in the mixer that can only be confusing.
        let text = format!(
            r#"
            {BASE}
            [[part]]
            name = "lead"
            role = "melody"
            "#
        );
        let spec = SongSpec {
            parts: vec![crate::spec::PartSpec::of_role("lead", Role::Melody)],
            ..SongSpec::parse(&text).expect("the fixture parses")
        };
        let piece = compose(&spec);
        let names: Vec<&str> = piece.buses.iter().map(|bus| bus.name.as_str()).collect();
        assert_eq!(names, vec!["Room"]);
        // And the one bus that does exist is the one the send names.
        assert_eq!(piece.tracks[0].sends[0].bus, 0);
    }

    #[test]
    fn a_kit_that_turns_out_silent_leaves_no_bus_behind() {
        // `sparse` writes no snare at all, so a piece whose only drum part is a snare has a kit
        // on paper and none in the arrangement. The bus used to be decided from the roster, so
        // one was made anyway and nothing was ever routed to it — which is what a user sees as a
        // drum track with no clips on it.
        let piece = compose_text(
            r#"
            title  = "Silent kit"
            groove = "sparse"
            fill = 0

            [[part]]
            name = "lead"
            role = "melody"

            [[part]]
            name = "snare"
            role = "snare"
            "#,
        );
        let names: Vec<&str> = piece.buses.iter().map(|bus| bus.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Room"],
            "the snare wrote nothing, so there is no kit to bus"
        );
        assert!(
            !piece.tracks.iter().any(|track| track.name == "snare"),
            "a part that never plays leaves no track"
        );
        // The point of the whole exercise: nothing in the piece is a bus with no track feeding it.
        for (index, bus) in piece.buses.iter().enumerate() {
            assert!(
                piece.tracks.iter().any(|track| {
                    track.output == Some(index) || track.sends.iter().any(|send| send.bus == index)
                }),
                "nothing reaches the {} bus",
                bus.name
            );
        }
    }

    #[test]
    fn a_kit_that_plays_still_gets_its_bus() {
        // The other half, so the fix above cannot pass by making no buses at all.
        let piece = compose_text(BASE);
        let names: Vec<&str> = piece.buses.iter().map(|bus| bus.name.as_str()).collect();
        assert!(names.contains(&DRUM_BUS), "{names:?}");
        let drums = names.iter().position(|name| *name == DRUM_BUS);
        assert!(
            piece
                .tracks
                .iter()
                .any(|track| track.output == drums && track.name == "kick"),
            "the kick goes to the drum bus"
        );
    }

    #[test]
    fn a_cymbal_on_the_built_in_drum_is_voiced_as_one() {
        // The shipped noise drum is a tom: a band-pass swept down from where the note puts it,
        // decaying in a quarter of a second. At the defaults a part striking 49 comes out a
        // fourth thud rather than a cymbal, so the voicing is what makes the crash a crash.
        let voiced = voicing_for(Role::Crash, "auris.synth.noisedrum");
        assert_eq!(
            voiced.params.get("sweep"),
            Some(&0.0),
            "a cymbal has no head"
        );
        assert!(
            voiced.params["decay"] > 1.0,
            "a crash that stops in {} s is a tick",
            voiced.params["decay"]
        );
        // The whole reason a level is set at all: opening the filter this far lets through far
        // more of the noise, and the hit arrived over the rest of the kit until it was corrected.
        assert!(voiced.params["level"] < -6.0);
    }

    #[test]
    fn nothing_but_the_cymbal_is_told_how_to_sound() {
        // A role picks an instrument and then leaves it sounding how it sounds. Reaching into a
        // plugin's parameters is the exception, and it stays one — the kick, the snare and the
        // hat on this same instrument are what the built-in kit has always sounded like, and
        // revoicing them is a decision about a preset rather than part of adding a cymbal.
        for role in Role::ALL {
            let voiced = voicing_for(role, "auris.synth.noisedrum");
            assert_eq!(
                voiced.params.is_empty(),
                role != Role::Crash,
                "{} was voiced when it should not have been, or the other way round",
                role.name()
            );
        }
        // And a cymbal somebody put on another plugin gets nothing either. `decay` means one
        // thing on the noise drum and could mean anything at all elsewhere.
        assert!(
            voicing_for(Role::Crash, "auris.synth.chiptune")
                .params
                .is_empty()
        );
    }

    #[test]
    fn a_composed_cymbal_carries_its_voicing_onto_its_track() {
        let piece = compose_text(
            r#"
            form = "chorus"
            [[part]]
            name = "crash"
            role = "crash"
            "#,
        );
        let crash = piece
            .tracks
            .iter()
            .find(|track| track.name == "crash")
            .expect("the cymbal plays");
        assert!(!crash.state.params.is_empty(), "the voicing was dropped");
        // Every other part is left alone, which is what makes this a exception rather than a pass
        // over the roster.
        let piece = compose_text(BASE);
        assert!(
            piece
                .tracks
                .iter()
                .all(|track| track.state.params.is_empty())
        );
    }

    #[test]
    fn the_parts_are_spread_across_the_image_but_the_anchors_stay_centred() {
        // Six parts stacked in the middle are six parts fighting for the same space. What a
        // listener localises the song by does not move.
        let piece = compose_text(BASE);
        let pan = |name: &str| {
            piece
                .tracks
                .iter()
                .find(|track| track.name == name)
                .map(|track| track.pan)
        };
        assert_eq!(pan("lead"), Some(0.0));
        assert_eq!(pan("bass"), Some(0.0));
        assert_eq!(pan("kick"), Some(0.0));
        assert_ne!(pan("chords"), Some(0.0));
        assert_ne!(pan("hat"), Some(0.0));
        // Nothing hard over: a part at the edge of the image disappears in mono.
        for track in &piece.tracks {
            assert!(track.pan.abs() <= 0.5, "{} is too far over", track.name);
        }
    }

    #[test]
    fn a_piece_carries_the_harmony_its_parts_were_written_against() {
        // It was computed and then thrown away: the document had an empty harmony lane over a
        // song that plainly has chords, and a clip generated afterwards had nothing to read.
        let piece = compose_text(BASE);
        let frame = plan(&SongSpec::parse(BASE).expect("the fixture parses"));

        // Every chord in the frame is on the song's timeline exactly once, plus the point that
        // ends the harmony where the piece does.
        let written: usize = frame.sections.iter().map(|s| s.events.len()).sum();
        assert_eq!(piece.harmony.chords.points().len(), written + 1);
        assert!(!piece.harmony.chords.is_empty());

        // And they are where they sound, not where they sat inside their own section.
        let second = &frame.sections[1];
        let first_of_second = second.events.first().expect("the section has chords");
        assert_eq!(
            piece
                .harmony
                .chords
                .numeral_at(second.start + first_of_second.start),
            Some(first_of_second.numeral),
        );

        // Past the last bar there is no harmony rather than a final chord ringing for ever.
        assert_eq!(piece.harmony.chords.numeral_at(piece.length), None);
        assert_eq!(
            piece.harmony.keys.key_at(Ticks::ZERO),
            frame.sections[0].key
        );
    }

    #[test]
    fn a_piece_carries_its_own_structure() {
        let piece = compose_text(BASE);
        assert_eq!(
            piece.sections.section_at(Ticks::ZERO).map(|(name, _)| name),
            Some("intro")
        );
        // Four bars of intro at 4/4: the verse starts there.
        let bar = piece.meter.ticks_per_bar();
        assert_eq!(
            piece.sections.section_at(bar * 4).map(|(name, _)| name),
            Some("verse")
        );
        // The song ends rather than the outro running on for ever.
        assert_eq!(piece.sections.section_at(piece.length), None);
    }

    #[test]
    fn one_key_throughout_writes_one_key_point() {
        // A point per section would fill the lane with changes that change nothing.
        let piece = compose_text(BASE);
        assert_eq!(piece.harmony.keys.points().len(), 1);

        // A section that transposes is a change, and gets one. Written out in full rather than
        // appended to `BASE`, which already declares the chorus — TOML refuses the same table
        // twice, and quite right too: which of the two would `bars` have come from?
        let piece = compose_text(
            r#"
            title    = "Test"
            form     = "intro verse chorus"
            chords   = "@axis"
            humanize = 0

            [section.intro]
            bars = 4

            [section.verse]
            bars = 8

            [section.chorus]
            bars      = 8
            transpose = 2
            "#,
        );
        assert_eq!(piece.harmony.keys.points().len(), 2);
    }

    /// Everything a piece is, as one line per section and one number for the notes.
    ///
    /// Written so that a change shows up as a diff a person can read: the chords are the part a
    /// musician would notice, and the digest catches a note that moved by one tick.
    fn fingerprint(text: &str) -> String {
        let spec = SongSpec::parse(text).expect("the fixture parses");
        let frame = plan(&spec);
        let piece = render(&spec, &frame);

        let mut out = String::new();
        for section in &frame.sections {
            out.push_str(&format!("{}·{} ", section.name, section.instance));
            out.push_str(&section.key.to_text());
            out.push_str(" |");
            for event in &section.events {
                // Both, and not only one: `name()` reads the numeral, which is what the timeline
                // will store, while `chord` is what is heard. Every pass that rewrites one keeps
                // the other with it, so an arrow here is the shape of that going wrong again —
                // and a fingerprint showing one of them would hide it.
                //
                // Compared as *chords* and not as text. The two are spelled by different rules on
                // purpose — a numeral knows which letter its degree demands, a chord only knows
                // whether the key leans sharp or flat — so B flat and A sharp are the same chord
                // written twice, and an arrow between them would be an alarm about nothing.
                let numeral = event.name();
                if event.chord == event.numeral.chord_in(event.key) {
                    out.push_str(&format!(" {numeral}"));
                } else {
                    out.push_str(&format!(" {numeral}→{}", event.chord.name_in(event.key)));
                }
            }
            out.push_str(" |\n");
        }
        // A cheap order-sensitive digest: a note that moves, changes pitch or changes length
        // changes it, and two pieces that differ anywhere differ here.
        let mut digest: u64 = 1469598103934665603;
        for track in &piece.tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    for value in [
                        note.pitch as i64,
                        note.start.raw(),
                        note.length.raw(),
                        (note.velocity * 1000.0) as i64,
                        clip.start.raw(),
                    ] {
                        digest ^= value as u64;
                        digest = digest.wrapping_mul(1099511628211);
                    }
                }
            }
        }
        out.push_str(&format!(
            "{} notes, digest {digest:016x}\n",
            piece.note_count()
        ));
        out
    }

    #[test]
    fn a_track_carries_the_sound_its_part_asked_for() {
        // The composer has no font and cannot resolve a preset; what it can do is carry the
        // request as far as the session, which does. A track that dropped it on the way would
        // compose a full orchestra and play all of it on an oscillator.
        let spec = SongSpec::parse(
            r#"
            form = ["verse"]

            [[part]]
            name    = "lead"
            role    = "melody"
            program = "Violin"

            [[part]]
            name = "bass"
            role = "bass"

            [[part]]
            name    = "kick"
            role    = "kick"
            program = "TR-808 Kit"
            "#,
        )
        .expect("a valid specification");
        let piece = compose(&spec);
        let sound_of = |name: &str| {
            piece
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap_or_else(|| panic!("{name} is in the piece"))
                .sound
        };
        assert_eq!(
            sound_of("lead"),
            Some(crate::gm::Sound { bank: 0, patch: 40 })
        );
        // The kit is the other reading of the same field, and the bank is what says so.
        assert_eq!(
            sound_of("kick"),
            Some(crate::gm::Sound {
                bank: crate::gm::DRUM_BANK,
                patch: 25
            })
        );
        // A part that asked for nothing stays on its plugin, so a piece written on the built-in
        // voices is still written on them.
        assert_eq!(sound_of("bass"), None);
    }

    #[test]
    fn a_track_is_the_colour_of_what_it_plays() {
        // Which colour a part got used to depend on how many parts were declared before it, so
        // the bass was green in one piece and pink in the next. Colour that means nothing is
        // colour nobody reads.
        let piece = compose(&SongSpec::default());
        for track in &piece.tracks {
            let role = SongSpec::default()
                .parts
                .iter()
                .find(|part| part.name == track.name)
                .map(|part| part.role)
                .expect("every track came from a part");
            assert_eq!(
                track.color,
                role.color(),
                "{} is the wrong colour",
                track.name
            );
        }
        // And the fader that moves the drums is the colour of the drums.
        let drums = piece
            .buses
            .iter()
            .find(|bus| bus.name == DRUM_BUS)
            .expect("the default roster has a kit");
        assert_eq!(drums.color, Role::Snare.color());
    }

    /// The pieces the composer writes today, pinned exactly.
    ///
    /// Not because this output is sacred — it is a composer, and what it writes is a matter of
    /// taste — but because it is about to be taken apart and reassembled, and a change that
    /// nobody chose is the one thing that must not happen quietly. A fixture that moves is either
    /// a bug or a decision, and this is what makes anyone look.
    ///
    /// It last moved when the feel left the text: the humanise wander, its velocity scatter and
    /// the per-role lean stopped being baked into the notes and became the transform stack each
    /// clip arrives carrying (`crate::perform`). The fixtures that pin a nonzero `humanize` —
    /// or none, and so the default — moved back onto the grid: not one chord and not one note
    /// count changed, because the dial never decided what to play, only how loosely, and the
    /// looseness now happens at performance time where these digests cannot see it. That
    /// blindness is correct — the digest pins the *score*.
    ///
    /// Before that it moved when the kit stopped missing: the survival roll used to thin everything
    /// below the downbeat by how quiet the section was, so at the default settings one backbeat
    /// in nine and one four-on-the-floor kick in nine simply vanished, a different bar of holes
    /// every bar — heard as mistakes, never as dynamics. A hit the groove spells now always
    /// plays, and what breathes with the intensity is the ghosts alone, the finest steps first
    /// — see `parts::drums::survival`. All four moved: every count rose by the spelled hits
    /// thinning used to take, less the ghosts a verse no longer plays, and not one chord went
    /// anywhere, because which bar carries which chord was never the kit's to decide.
    ///
    /// Before that it moved when the melody grew a germ: one piece-level contour per part, which every
    /// section's figure wears re-sampled onto its own rhythm, so a verse and a chorus became two
    /// statements of one tune instead of two tunes. Every count stayed and every chord stayed —
    /// the germ changes which degrees a figure asks for and nothing about when anything sounds —
    /// so all four digests moved and nothing else did. Measured over the presets: contour
    /// correlation between different sections of one song rose from 0.40 to 0.46 while
    /// correlation between different songs *fell* from 0.15 to 0.04, and the line's own grammar
    /// improved in the bargain — steps 55.4% → 60.5% (the corpus says 68), mean interval 2.53 →
    /// 2.29 semitones — because a busy section now fills the germ's line in with passing steps
    /// where it used to draw fresh leaps.
    ///
    /// Before that it moved when the comp learned to push: a section may strike each chord change half
    /// a beat early and hold it over the line, drawn per section at a rate the syncopation dial
    /// sets. Only the third fixture moved — the one whose mood leaves syncopation at the default
    /// and whose seed drew a pushing section — and its count *fell* by the line-strikes and
    /// borrowed half-beats a push replaces. The chords are untouched everywhere, because a push
    /// moves when a chord is struck and never which.
    ///
    /// Before that it moved when the figure's variations grew from three to six — retrograde, the
    /// ornament on the longest note, the late entry — so every closing bar's variation draw sees
    /// six weights where it saw three and lands differently. Not one chord moved, and the counts
    /// drifted by a note or two where an ornament splits one note into a pair or a late entry
    /// takes one away: the scope of a change that touches nothing but which variation a bar
    /// draws.
    ///
    /// Before that it moved when the piece learned to end: every fixture gains an `ending` section —
    /// one bar of the final key's tonic, held, spelled through one numeral so the lane agrees —
    /// and the two whose charts are the composer's own also turn their last bar around into it,
    /// which is why `F → G7` and `Dm7 → E7` appear in their chord lines and the quoted fixtures'
    /// lines gained only the ending bar. Every count rose by the landing's own notes.
    ///
    /// Before that it moved when the fill grew a vocabulary: `parts::drums::FillShape` draws one of
    /// four shapes per join where every fill used to be the rising run. The two multi-section
    /// fixtures moved — their counts by the few snare hits a sparser shape leaves out — and the
    /// two single-section ones did not, because a piece's last section runs no fill: which is
    /// the scope of a change that touches nothing but the bar before a join.
    ///
    /// Before that it moved when the bass learned to walk: `BassFigure::Walk` joined the figure table,
    /// so every bar's figure draw sees five weights where it saw four and some bars land on a
    /// different line. The chords did not move — the walk plays the harmony, it does not choose
    /// it — and the counts drifted by a handful of notes where a bar that followed the kick now
    /// walks quarter notes, or the other way round. The third fixture's count held at 629 while
    /// its digest moved, which is the drift at its smallest: the same number of notes on
    /// different pitches.
    ///
    /// Before that it moved when the melody stopped repeating notes nobody drew — the second pass of
    /// [`crate::melodic`]: the join is chosen against the chord-snapped landing and ranks a
    /// repeat below anything within a fourth, and `unstick` undoes the repeat the range clamp
    /// made by folding two degrees onto one pitch. All four digests moved and not one chord or
    /// note count did, which is the scope of it: the tune's pitches are all that changed, and
    /// repeated notes went from 22.8 per cent of the line to 10.5 against a corpus 11.
    ///
    /// Before that it moved when the band stopped varying so much: `parts::writer::WIDEST` narrowed how
    /// far apart two strokes of one part may sit, and `parts::WANDER_MS` how far the timing of a
    /// pitched one wanders. All four moved, the fourth included — it writes `humanize = 0` and so
    /// holds still in *time*, but a velocity is a velocity at every setting of that dial. Not one
    /// chord or note count changed, which is the scope of it: this decides how hard a note is
    /// struck and when, never which note it is.
    ///
    /// Before that it moved when a note stopped being left sounding into the next strike of its own
    /// pitch — see `parts::untangle`. Three of the four moved and the third did not, which is the
    /// assertion about the scope: the third is the one fixture writing `humanize = 0` over a
    /// groove that does not swing, so it is the only one that never held an overlap to cut. No
    /// chord moved and no note count changed in any of them, because nothing here writes or drops
    /// a note — it only shortens ones that were running over their own successor.
    ///
    /// Before that it moved when the melody stopped choosing each note without looking at the one
    /// before it — see [`crate::melodic`], which is the measurement that change came out of. All four
    /// digests moved, and not one chord or note count did: the tune's *pitches* and the length of
    /// its phrase-ending notes are all that is different, and every other part is untouched. The
    /// piece is the same piece with a singable line in it. A third of the composer's melodic
    /// intervals used to be a fourth or wider; it is now one in seven.
    ///
    /// Before that it moved when the bass's octave figure started actually leaping one — the same
    /// shape of report, pitches of some weak-beat bass notes and nothing else. The `Gmaj7 → G7`
    /// and `Amaj7 → Am7` corrections below are older still, and are described where each fixture
    /// is.
    ///
    /// Before that it moved when `colour` stopped adding sevenths through [`Quality::with_seventh`],
    /// which can only ever give a major triad a *major* seventh and so wrote `Vmaj7` where `V7`
    /// belongs, and when a borrow started asking the parallel mode for its own chord on the degree
    /// instead of replaying the numeral's case at it.
    ///
    /// The two fixtures that moved are the two whose charts the composer wrote itself. The other
    /// two quote `@axis` and `@marusa`, and a quoted chart is never coloured — that they did not
    /// move is the assertion that the trade is still exactly what it was documented to be, and
    /// that nothing outside the colouring changed. Both counts rose, which is the property to
    /// check here: the edit adds chord tones that were being written wrongly or dropped, so notes
    /// may appear, but no chord may move to a degree the chart did not name.
    #[test]
    fn the_composer_writes_what_it_wrote_before() {
        // A chart nobody asked for is the composer's own, and so the only kind it colours. In a
        // major key every colour it can reach is writable as a numeral.
        assert_eq!(
            fingerprint(
                r#"
                    form = "verse"
                    key = "C major"
                    seed = 7
                    tension = 0.95
                    [section.verse]
                    bars = 8
                    "#
            ),
            "verse·1 C major | Cmaj7 Gm7 Am Fmaj7 Cmaj7 G7 Am9 G7 |\n\
             ending·1 C major | C |\n\
             182 notes, digest 9761272ff4f3833e\n"
        );

        // The same in a minor key, and the fixture that moved furthest when colouring stopped
        // reaching for `Quality::with_seventh`. It used to read
        //
        //     Amaj7 E Fm7 D Amaj7 Emaj7 Gbm Dmaj7
        //
        // in **A minor** — a tonic spelled A C♯ E G♯, a subdominant spelled D F♯ A C♯ and a
        // dominant carrying D♯. Four of the eight bars were chromatic in a way nobody asked for,
        // because a seventh added to a triad's *quality* is always the major one and the key was
        // never consulted. It now takes the seventh the key stacks on that degree, so the tonic
        // is `Am7`, the subdominant `Dm`, and the dominant `E9` — major third, minor seventh, the
        // one chord in a minor key that is supposed to be chromatic and the only one that is.
        //
        // One chord is still a borrow that moves the root: `vi` read in the parallel major is an
        // F sharp minor, and the numeral goes with it. The source F minor and its moved F-sharp
        // destination are both shown; the destination is the plain major-scale sixth rather than
        // the double-flat seventh the old inverse spelling produced in a minor key.
        //
        // The count rose from 227 because a borrow used to *discard* a seventh already added in
        // the same pass, and now composes with it.
        //
        // This is also the only fixture whose mood names a brightness away from the middle —
        // `tense` writes 0.2 — so it is the only one the register slide reaches. That it moved the
        // digest and not the chords, and not the count, is the whole assertion about that change:
        // brightness decides how high the skeleton sits and nothing else.
        assert_eq!(
            fingerprint(
                r#"
                    form = "verse"
                    key = "A minor"
                    seed = 1
                    mood = "tense"
                    [section.verse]
                    bars = 8
                    "#
            ),
            "verse·1 A minor | Am7 E9 Fmaj7 Dm Am7 Em7 Fm7→Gbm7 E7 |\n\
             ending·1 A minor | Am |\n\
             252 notes, digest fe792a40951da0c3\n"
        );

        // A quoted chart, which is never coloured, over a form that repeats — and the one fixture
        // here that writes `humanize = 0`, so nothing it holds has ever been moved by the wander.
        // Its chords are therefore the assertion that colouring reaches no quoted chart: they have
        // not changed through any of this. Its digest has, because the bass leaps where it used to
        // restrike.
        assert_eq!(
            fingerprint(BASE),
            "intro·1 C major | C G Am F |\n\
             verse·1 C major | C G Am F C G Am F |\n\
             chorus·1 C major | C G Am F C G Am F |\n\
             ending·1 C major | C |\n\
             627 notes, digest ded762cc6bf7af13\n"
        );

        // A transposed section, which is a key change on the timeline — and the one fixture here
        // that modulates, so the only one a lead-in reaches. 丸サ進行's last chord is `C7` and the
        // verse plays `Bb7` instead: the dominant of the E flat the chorus arrives in, named from
        // the key still in force. Every other chord of the quoted chart is untouched, which is the
        // scope of the trade — one bar, only where a modulation was asked for by hand.
        //
        // The count is unchanged at 204. This edit moves a chord and must never add or drop a
        // note; the digest moved because the parts play what the chord says.
        assert_eq!(
            fingerprint(
                r#"
                    form = "verse chorus"
                    chords = "@marusa"
                    key = "C major"
                    seed = 3
                    [section.verse]
                    bars = 4
                    [section.chorus]
                    bars = 4
                    transpose = 3
                    "#
            ),
            "verse·1 C major | Fmaj7 E7 Am7 Bb7 |\n\
             chorus·1 Eb major | Abmaj7 G7 Cm7 Eb7 |\n\
             ending·1 Eb major | Eb |\n\
             221 notes, digest 358d5fd52944492d\n"
        );
    }

    #[test]
    fn a_default_spec_writes_a_playable_piece() {
        let piece = compose_text("");
        assert!(!piece.tracks.is_empty(), "no tracks");
        assert!(
            piece.note_count() > 100,
            "only {} notes",
            piece.note_count()
        );
        assert_eq!(piece.tempo_map, TempoMap::constant(120.0));
    }

    #[test]
    fn a_section_that_names_a_tempo_becomes_a_change_on_the_timeline() {
        let piece = compose_text(
            r#"
            tempo = 120
            form  = "verse chorus verse"

            [section.verse]
            bars = 4
            [section.chorus]
            bars  = 4
            tempo = 132
            "#,
        );
        let bar = piece.meter.ticks_per_bar();
        assert_eq!(piece.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(piece.tempo_map.bpm_at(bar * 4), 132.0);
        assert_eq!(
            piece.tempo_map.bpm_at(bar * 8),
            120.0,
            "the second verse goes back to the song's own tempo"
        );
    }

    #[test]
    fn a_piece_at_one_tempo_writes_one_point() {
        // The same rule the key lane follows. A point per section would fill the tempo lane with
        // changes that change nothing, and leave a person hunting for the one that does.
        let piece = compose_text(r#"form = "verse chorus verse chorus outro""#);
        assert_eq!(piece.tempo_map.points().len(), 1);

        // And a section that lifts and drops back writes two rather than one per section.
        let piece = compose_text(
            r#"
            form = "verse chorus verse"
            [section.chorus]
            tempo = 90
            "#,
        );
        assert_eq!(piece.tempo_map.points().len(), 3);
    }

    #[test]
    fn every_section_becomes_a_clip_on_every_playing_part() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            // Three sections of the form, and the ending — which the snare and the hat sit out,
            // because a held final bar has nothing for them to keep time for.
            let expected = match track.name.as_str() {
                "snare" | "hat" => 3,
                _ => 4,
            };
            assert_eq!(
                track.clips.len(),
                expected,
                "`{}` has {} clips",
                track.name,
                track.clips.len()
            );
        }
        let bar = TimeSignature::default().ticks_per_bar();
        let lead = &piece.tracks[0];
        assert_eq!(lead.clips[0].start, Ticks::ZERO);
        assert_eq!(lead.clips[0].length, bar * 4);
        assert_eq!(lead.clips[1].start, bar * 4);
        assert_eq!(lead.clips[2].start, bar * 12);
        assert_eq!(lead.clips[3].start, bar * 20, "the ending, one bar");
        assert_eq!(lead.clips[3].length, bar);
        assert_eq!(piece.length, bar * 21);
    }

    #[test]
    fn clip_notes_are_rebased_and_fit_inside_their_clip() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    assert!(
                        note.start >= Ticks::ZERO,
                        "`{}` has a note before its start",
                        clip.name
                    );
                    assert!(
                        note.start < clip.length,
                        "`{}` has a note starting past its end",
                        clip.name
                    );
                    assert!(
                        note.end() <= clip.length,
                        "`{}` has a note overhanging by {}",
                        clip.name,
                        (note.end() - clip.length).raw()
                    );
                    assert!(note.length > Ticks::ZERO);
                    assert!((0.0..=1.0).contains(&note.velocity));
                }
            }
        }
    }

    #[test]
    fn notes_come_out_in_a_canonical_order() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            for clip in &track.clips {
                let keys: Vec<(i64, u8)> = clip
                    .notes
                    .iter()
                    .map(|note| (note.start.raw(), note.pitch))
                    .collect();
                let mut sorted = keys.clone();
                sorted.sort_unstable();
                assert_eq!(keys, sorted, "`{}` is out of order", clip.name);
            }
        }
    }

    #[test]
    fn the_humanize_dial_reaches_the_stack_and_never_the_text() {
        // The dial used to bake a wander into the notes; it now decides the transform stack a
        // clip arrives carrying, so two settings of it write byte-for-byte the same text.
        let straight = compose_text(BASE);
        let loose = compose_text(&BASE.replace("humanize = 0", "humanize = 1.0"));
        for (a, b) in straight.tracks.iter().zip(&loose.tracks) {
            for (before, after) in a.clips.iter().zip(&b.clips) {
                assert_eq!(before.notes, after.notes, "`{}`'s text moved", after.name);
                assert!(
                    before.performance.is_empty(),
                    "`{}` is performed at humanize 0",
                    before.name
                );
            }
        }
    }

    #[test]
    fn every_clip_arrives_with_its_parts_own_feel() {
        // The table `perform` keeps, read off a whole composed piece: the pitched parts lean
        // and wander, the snare and hat lean without wandering, the kick starts square.
        let piece = compose_text(&BASE.replace("humanize = 0", "humanize = 1.0"));
        let stack_of = |name: &str| {
            let track = piece
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap_or_else(|| panic!("no `{name}` track"));
            track.clips[0].performance.clone()
        };
        assert!(matches!(
            stack_of("lead").as_slice(),
            [
                NoteTransform::Lean { ticks: -4 },
                NoteTransform::Humanize { amount, .. }
            ] if *amount == 1.0
        ));
        assert_eq!(
            stack_of("hat"),
            vec![NoteTransform::Lean { ticks: -8 }],
            "a drum leans and never wanders"
        );
        assert!(stack_of("kick").is_empty(), "the kick keeps the time");

        // The wander's seed is the clip's own — the same number its recipe carries — so a take
        // and its feel are named together, and two clips of one part wobble apart.
        for track in &piece.tracks {
            for clip in &track.clips {
                let (Some(recipe), Some(NoteTransform::Humanize { seed, .. })) = (
                    clip.recipe.as_ref(),
                    clip.performance
                        .iter()
                        .find(|t| matches!(t, NoteTransform::Humanize { .. })),
                ) else {
                    continue;
                };
                assert_eq!(
                    *seed, recipe.seed,
                    "`{}` wanders off another take",
                    clip.name
                );
            }
        }

        // The ending carries no recipe — nothing can promise to write it again — but it is
        // played by the same band, so it is played loose all the same.
        let landing = piece
            .tracks
            .iter()
            .find(|track| track.name == "lead")
            .and_then(|track| track.clips.iter().find(|clip| clip.recipe.is_none()))
            .expect("the lead lands somewhere");
        assert!(
            !landing.performance.is_empty(),
            "the landing chord is played by a machine"
        );
    }

    #[test]
    fn an_extended_chord_is_voiced_upward_rather_than_folded_flat() {
        // A ninth folded into the triad sounds as a second against the root.
        let piece = compose_text(
            r#"
            key = "C major"
            form = "verse"
            chords = "| Imaj9 | Imaj9 | Imaj9 | Imaj9 |"
            humanize = 0
            [section.verse]
            bars = 4
            [[part]]
            name = "chords"
            "#,
        );
        let clip = &piece.tracks[0].clips[0];
        let first: Vec<u8> = clip
            .notes
            .iter()
            .filter(|note| note.start == Ticks::ZERO)
            .map(|note| note.pitch)
            .collect();
        assert!(first.len() >= 4, "only {} notes in the chord", first.len());
        let span = first.iter().max().unwrap() - first.iter().min().unwrap();
        assert!(
            span > 12,
            "a ninth chord spanning {span} semitones has been folded into one octave"
        );
    }

    #[test]
    fn the_same_spec_writes_the_same_piece() {
        assert_eq!(compose_text(BASE), compose_text(BASE));
    }

    #[test]
    fn a_part_that_never_plays_leaves_no_track() {
        let piece = compose_text(
            r#"
            form = "intro"
            humanize = 0

            [section.intro]
            bars = 4
            parts = "bass"

            [[part]]
            name = "bass"
            [[part]]
            name = "hat"
            "#,
        );
        let names: Vec<&str> = piece.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["bass"],
            "a silent part should not leave an empty track"
        );
    }

    #[test]
    fn a_clip_is_named_after_its_section_and_part() {
        let piece = compose_text(BASE);
        let lead = &piece.tracks[0];
        assert_eq!(lead.clips[0].name, format!("intro 1 · {}", lead.name));
        assert_eq!(lead.clips[1].name, format!("verse 1 · {}", lead.name));
    }

    #[test]
    fn the_summary_names_every_track() {
        let piece = compose_text(BASE);
        let summary = piece.summary();
        assert!(summary.contains("Test"));
        assert!(summary.contains("120 BPM"));
        for track in &piece.tracks {
            assert!(summary.contains(&track.name), "`{}` is missing", track.name);
        }
    }

    #[test]
    fn a_fade_out_is_a_ride_on_the_master_and_no_landing_bar() {
        let held = compose_text(BASE);
        let fade = compose_text(&BASE.replace("form =", "ending = \"fade\"\nform ="));
        let bar = TimeSignature::default().ticks_per_bar();

        // A fade is the deliberate refusal of a landing bar: the piece is exactly its form, one
        // bar shorter than the held version, and no clip carries the landing chord.
        assert_eq!(fade.length, held.length - bar);

        // The ride itself: unity where the fade begins, silence where the piece ends, and the
        // final eight bars — the whole of BASE's chorus — between them.
        assert_eq!(fade.master_gain.len(), 2);
        assert_eq!(fade.master_gain[0].tick, fade.length - bar * 8);
        assert_eq!(fade.master_gain[0].value, 0.0);
        assert_eq!(fade.master_gain[1].tick, fade.length);
        assert_eq!(fade.master_gain[1].value, -60.0);

        // And a piece that does not fade asks nothing of the fader.
        assert!(held.master_gain.is_empty());
    }

    #[test]
    fn a_final_section_shorter_than_the_fade_fades_over_all_of_itself() {
        let piece = compose_text(
            r#"
            form = "verse chorus"
            chords = "@axis"
            ending = "fade"
            [section.verse]
            bars = 8
            [section.chorus]
            bars = 2
            "#,
        );
        let bar = TimeSignature::default().ticks_per_bar();
        assert_eq!(piece.master_gain[0].tick, piece.length - bar * 2);
    }

    #[test]
    fn the_swing_never_pushes_a_note_out_of_its_clip() {
        // The one text-side feel left, and the one place it could corrupt the document rather
        // than delay a note: an offbeat swung over the section line is clamped and truncated.
        let piece = compose_text(&BASE.replace("form =", "swing = 75\nform ="));
        for track in &piece.tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    assert!(note.start >= Ticks::ZERO);
                    assert!(note.end() <= clip.length);
                }
            }
        }
    }

    #[test]
    fn a_named_progression_reaches_the_notes() {
        // The bass plays roots, so its first note of each bar spells the progression out.
        let piece = compose_text(
            r#"
            key = "C major"
            form = "verse"
            chords = "@marusa"
            humanize = 0
            [section.verse]
            bars = 4
            [[part]]
            name = "bass"
            "#,
        );
        let bass = &piece.tracks[0];
        let bar = TimeSignature::default().ticks_per_bar();
        let roots: Vec<u8> = (0..4)
            .filter_map(|index| {
                bass.clips[0]
                    .notes
                    .iter()
                    .find(|note| note.start == bar * index)
                    .map(|note| note.pitch % 12)
            })
            .collect();
        // F, E, A, C — the roots of 丸サ進行 in C.
        assert_eq!(roots, vec![5, 4, 9, 0]);
    }
}
