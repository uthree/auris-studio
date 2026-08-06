//! Turning the parts into clips a document can hold.

use auris_core::Note;
use auris_core::harmony::{ChordMap, ChordPoint, Harmony, KeyMap, KeyPoint};
use auris_core::plugin::PluginState;
use auris_core::project::Color;
use auris_core::structure::{SectionMap, SectionPoint};
use auris_core::time::{Ticks, TimeSignature};

use crate::frame::{Frame, plan};
use crate::parts::{ScoreSettings, write_parts};
use crate::spec::{Role, SongSpec};

/// The bus a whole kit sits under, so one fader moves the drums.
const DRUM_BUS: &str = "Drums";

/// The room the pitched parts share, fed by sends.
const ROOM_BUS: &str = "Room";

/// The reverb the room bus carries.
const REVERB_ID: &str = "auris.fx.reverb";

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
    /// Level trim in decibels.
    pub gain_db: f32,
    /// Stereo position.
    pub pan: f32,
    /// Which bus this track's output goes to, by position in [`Composition::buses`].
    ///
    /// `None` for the master. A position rather than a name or an id, because the composer has
    /// neither: ids belong to a document and do not exist until one is built.
    pub output: Option<usize>,
    /// Copies of this track fed to buses alongside its own output.
    pub sends: Vec<SendDraft>,
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
    pub effects: Vec<BusEffectDraft>,
}

/// One effect on a bus, and the parameters it should not be left at its defaults for.
#[derive(Clone, Debug, PartialEq)]
pub struct BusEffectDraft {
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
    /// Beats per minute.
    pub tempo: f64,
    /// The time signature.
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
            self.tempo,
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

/// Turns a planned frame and its parts into tracks of clips.
fn render(spec: &SongSpec, frame: &Frame) -> Composition {
    let drafts = write_parts(&ScoreSettings::from(spec), &spec.parts, frame);
    // The roster's roles, so the mix can be laid out before a single track exists. A draft carries
    // its part's name and not its role, and the name is what ties the two together.
    let role_of = |name: &str| {
        spec.parts
            .iter()
            .find(|part| part.name == name)
            .map(|part| part.role)
    };
    let buses = buses_for(
        &drafts
            .iter()
            .filter_map(|draft| role_of(&draft.name))
            .collect::<Vec<_>>(),
    );
    let mut tracks = Vec::new();

    for draft in drafts {
        let mut clips = Vec::new();
        for (index, section) in frame.sections.iter().enumerate() {
            let mut notes: Vec<Note> = draft
                .notes
                .iter()
                .filter(|note| note.section == index)
                .filter_map(|note| {
                    // Rebase onto the clip. A note humanisation nudged a few ticks over a
                    // section boundary is clamped back rather than deleted — dropping it took
                    // the downbeat out of every section at the default humanisation.
                    let start = (note.start - section.start)
                        .max_zero()
                        .min(section.length - Ticks(1));
                    if note.start - section.start >= section.length {
                        return None;
                    }
                    // Truncate rather than let a note overhang: the scheduler would drop it
                    // silently, and `fit_length_to_notes` would grow the clip if it did not.
                    let length = note.length.min(section.length - start).max(Ticks(1));
                    Some(Note {
                        pitch: note.pitch.min(127),
                        velocity: note.velocity.clamp(0.0, 1.0),
                        start,
                        length,
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
            });
        }

        // A part that never plays leaves no track at all.
        if clips.is_empty() {
            continue;
        }
        let role = role_of(&draft.name);
        let (output, sends) = role
            .map(|role| routing_for(role, &buses))
            .unwrap_or_default();
        tracks.push(TrackDraft {
            name: draft.name,
            instrument: draft.instrument,
            sound: draft.sound,
            // A part with no role in the roster cannot happen — a draft is written *from* one —
            // but the melody's colour is the honest answer if it ever did.
            color: role.unwrap_or(Role::Melody).color(),
            gain_db: draft.gain_db,
            pan: draft.pan,
            output,
            sends,
            clips,
        });
    }

    Composition {
        title: spec.title.clone(),
        tempo: spec.tempo,
        meter: spec.meter,
        length: frame.length,
        seed: spec.seed,
        spec: spec.to_toml(),
        harmony: harmony_of(frame),
        sections: sections_of(frame),
        tracks,
        buses,
    }
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
            effects: vec![BusEffectDraft {
                id: REVERB_ID.to_string(),
                state,
            }],
        });
    }
    buses
}

/// `true` when a role belongs under the drum fader.
fn drum_bus_takes(role: Role) -> bool {
    matches!(role, Role::Kick | Role::Snare | Role::Hat)
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
                // will store, while `chord` is what the colouring pass produced and what is
                // heard. Colouring keeps the two saying the same chord, so an arrow here is the
                // shape of that going wrong again — and a fingerprint showing one of them would
                // hide it.
                let numeral = event.name();
                let sounding = event.chord.name_in(event.key);
                if numeral == sounding {
                    out.push_str(&format!(" {numeral}"));
                } else {
                    out.push_str(&format!(" {numeral}→{sounding}"));
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
    /// It last moved when the timing humanisation became a time rather than a count of ticks, lost
    /// its floor, and stopped reaching the kit. Three of the four fixtures moved and the fourth is
    /// `BASE`, which writes `humanize = 0` and so is the one piece here the dial never touched —
    /// that it did not move is the assertion that nothing else did. All four counts are exactly
    /// what they were, which is the property to check first: this edit moves notes and must never
    /// add or drop one. Nor does it restrike any — the wander is drawn whether or not it is used,
    /// so every velocity in all four is the number it always was, and every difference in the
    /// three digests that moved is a start time.
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
            "verse·1 C major | Cmaj7 G Am Fmaj7 Cmaj7 Gmaj7 Am9 F |\n\
             163 notes, digest a275095187edb4bd\n"
        );

        // The same in a minor key, where one chord is a borrow that moves the root: `vi` read in
        // the parallel major is an F sharp minor, and the numeral goes with it. What it becomes
        // is `bbvii`, which looks strange and is the only pair that names that note —
        // `degree_class` measures from the key's own scale at zero and from the major scale
        // otherwise, so F sharp is out of reach of the sixth degree and is named from the seventh
        // instead. Strange and right beats plain and wrong: `vi` left as it was said F minor over
        // parts playing F sharp minor.
        //
        // This and the transposed 丸サ below are the two of the four whose digest moved when the
        // melody started reading the scale each chord implies rather than the key's own, and
        // that is the whole report on the blast radius: they are the two whose chords borrow.
        // The chords, the count and the shape are what they were; some melody notes stepped onto
        // the altered degree instead of onto the one it replaced.
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
            "verse·1 A minor | Amaj7 E Fm7 D Amaj7 Emaj7 Gbm Dmaj7 |\n\
             227 notes, digest 1cee42201fccefc1\n"
        );

        // A quoted chart, which is never coloured, over a form that repeats — and the one fixture
        // here that writes `humanize = 0`. Its digest has not moved and must not: a dial at zero
        // was the identity before this change and still is, so this line is what says the wander
        // was reshaped rather than the writing underneath it.
        assert_eq!(
            fingerprint(BASE),
            "intro·1 C major | C G Am F |\n\
             verse·1 C major | C G Am F C G Am F |\n\
             chorus·1 C major | C G Am F C G Am F |\n\
             629 notes, digest 320d8793d96fc1d4\n"
        );

        // A transposed section, which is about to become a key change on the timeline.
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
            "verse·1 C major | Fmaj7 E7 Am7 C7 |\n\
             chorus·1 Eb major | Abmaj7 G7 Cm7 Eb7 |\n\
             204 notes, digest 19ddfe856fd8e3aa\n"
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
        assert_eq!(piece.tempo, 120.0);
    }

    #[test]
    fn every_section_becomes_a_clip_on_every_playing_part() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            assert_eq!(
                track.clips.len(),
                3,
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
        assert_eq!(piece.length, bar * 20);
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
    fn humanising_never_loses_a_sections_downbeat() {
        // Deleting a note nudged a few ticks before its section took the downbeat out of every
        // section but the first; it is clamped back into the clip instead.
        let straight = compose_text(BASE);
        let loose = compose_text(&BASE.replace("humanize = 0", "humanize = 1.0"));
        for (a, b) in straight.tracks.iter().zip(&loose.tracks) {
            for (before, after) in a.clips.iter().zip(&b.clips) {
                assert_eq!(
                    before.notes.len(),
                    after.notes.len(),
                    "`{}` lost {} notes to humanisation",
                    after.name,
                    before.notes.len() as i64 - after.notes.len() as i64
                );
            }
        }
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
    fn humanising_never_pushes_a_note_out_of_its_clip() {
        // The one place humanisation could corrupt the document rather than just move a note.
        let piece = compose_text(&BASE.replace("humanize = 0", "humanize = 1.0"));
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
