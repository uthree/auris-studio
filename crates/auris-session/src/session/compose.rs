//! Installing a whole written piece as the open document.
//!
//! One command, and it is one edit rather than several hundred: building the project directly and
//! swapping it in means the whole piece is a single undo step and the render graph is rebuilt once
//! rather than once per note. See [`crate::guide::composition`] for what produced the
//! [`Composition`](auris_compose::Composition) in the first place.
//!
//! The kit trims are here because they are the same command's business and nobody else's.
//! *Which* General MIDI font is installed is the session's knowledge — the composer names General
//! MIDI and has no idea whose file will answer — so the correction for a font whose kits disagree
//! with one another can only be applied at the moment a part is given a strip to play through.
//! [`kit_trim_db`] is the table and [`composed_gain_db`] is how it is added to the mix the
//! composer actually wrote.

use auris_core::time::{SignatureMap, Ticks};
use auris_core::{AuxSend, Output, PresetRef, Project, TrackId};
use auris_sampler::{SAMPLER_ID, store_preset};

use crate::error::SessionError;
use crate::history::Edit;

use super::{ComposeReport, Session};

/// The effect a composed piece gets across its master.
const MASTER_LIMITER: &str = "auris.fx.limiter";

/// The limiter parameter that sets the level nothing may pass.
const LIMITER_CEILING: &str = "ceiling_db";

/// Where the composed master's ceiling is set, in decibels.
///
/// Three tenths under full scale, which is the usual place: it is far enough down that a
/// resampler or a lossy encoder — both of which reconstruct a waveform that overshoots the samples
/// it was given — has somewhere to overshoot into, and near enough that nobody could hear what it
/// costs.
const MASTER_CEILING_DB: f32 = -0.3;

/// How much a General MIDI percussion patch has to come down, in decibels, to sit where the
/// quietest kit of the shipped font sits.
///
/// The kits of MuseScore General are not calibrated against one another. Struck at the same
/// velocity through the same gain, the note the composer writes a kick on peaks at +6.51 dBFS on
/// the Brush Kit and at -1.43 on the TR-808 — 7.95 dB between two sounds that are supposed to be
/// the same instrument recorded in different rooms. Nobody decided that; it is the arithmetic of
/// eight kits sampled at eight sessions, and it arrives in a composed mix as a piece that clips
/// because of which kit its style happened to name. This is the correction, and it belongs here
/// because *which font is installed* is the session's knowledge: the composer names General MIDI
/// and has no idea whose file will answer.
///
/// # The table
///
/// Measured on the shipped font, at unity, on the note each kit's kick is written at. The kick,
/// because it is what the excursion is made of: the composer writes three drums, and once each
/// role's own level is on, the kick is the loudest of the three in every one of these kits — by
/// two thirds of a decibel in the closest and by six in the widest. The reference is the quietest
/// of them, the TR-808's, at -1.44 dBFS.
///
/// | kit | patch | kick at unity | trim |
/// |---|---|---|---|
/// | Standard | 0 | +2.38 dBFS | -3.82 dB |
/// | Room | 8 | +5.32 dBFS | -6.76 dB |
/// | Power | 16 | +1.95 dBFS | -3.39 dB |
/// | Electronic | 24 | +2.46 dBFS | -3.90 dB |
/// | TR-808 | 25 | -1.43 dBFS | 0 dB |
/// | Jazz | 32 | +6.51 dBFS | -7.95 dB |
/// | Brush | 40 | +6.51 dBFS | -7.95 dB |
/// | Orchestra | 48 | -1.26 dBFS | 0 dB |
///
/// The TR-808 is the reference and the Orchestra Kit sits 0.18 dB above it, which is not a trim —
/// a fifth of a decibel is below anything a listener or a mix has an opinion about, and writing it
/// down would only claim a precision these numbers do not have.
///
/// # What these numbers are not
///
/// They are not a musical balance. Nothing here says a kick should be 7.95 dB under a brush kit;
/// the levels a piece is *mixed* at are
/// [`Role::default_gain_db`](auris_compose::Role::default_gain_db) and whatever the specification
/// writes over it, and this is added to that rather than replacing it. This is a calibration of
/// one file, and a different General MIDI font — a leaner one, or somebody's own — will want
/// entirely different numbers or none at all.
///
/// A patch that is not in the table gets 0 dB, and so does every pitched program. A font is free
/// to put a kit anywhere in bank 128 and almost none do; a sound nobody has measured is a sound
/// this has nothing true to say about, and inventing a trim for it by rounding down to the nearest
/// kit would be guessing with a number that changes what a person hears.
pub fn kit_trim_db(sound: auris_compose::gm::Sound) -> f32 {
    if sound.bank != auris_compose::gm::DRUM_BANK {
        return 0.0;
    }
    // Every kit of `auris_compose::gm::KITS`, in patch order, including the two that come out at
    // zero — so that the table above and the code under it can be read against each other, and a
    // measured nought is not mistaken for a kit nobody looked at.
    match sound.patch {
        0 => -3.82,
        8 => -6.76,
        16 => -3.39,
        24 => -3.90,
        25 => 0.0,
        32 => -7.95,
        40 => -7.95,
        48 => 0.0,
        _ => 0.0,
    }
}

/// The gain a composed part's mixer strip is set to.
///
/// The part's own gain **plus** whatever [`kit_trim_db`] says about the sound it landed on, never
/// instead of it: the first is the mix the composer wrote and the second is an apology for the
/// font, and collapsing the two would mean a specification that says `gain = -12` could not be
/// read off the strip it produced.
///
/// `None` is a part that stayed on a built-in instrument, because no General MIDI font is
/// installed or because it never asked for one — and it is left exactly where the composer put it.
/// That is the half of this worth stating: the built-in kit is already 21.5 dB under its own
/// pitched parts and a trim aimed at a SoundFont would push it under the floor.
pub fn composed_gain_db(part_gain_db: f32, sound: Option<auris_compose::gm::Sound>) -> f32 {
    part_gain_db + sound.map_or(0.0, kit_trim_db)
}

impl Session {
    /// Replaces the document with a composed piece.
    ///
    /// One edit, not several hundred: building the project directly and swapping it in means the
    /// whole piece is a single undo step, and the render graph is rebuilt once rather than once
    /// per note.
    ///
    /// A part naming an instrument the registry does not have falls back to the first registered
    /// one and is reported, because a missing plugin should cost a timbre, not a whole piece.
    pub fn compose(
        &mut self,
        composition: &auris_compose::Composition,
    ) -> Result<ComposeReport, SessionError> {
        let fallback = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();

        let mut project = Project::new(&composition.title, self.project.sample_rate);
        // The composer's own map, handed over rather than flattened: a section may lift or drop
        // away from the song's tempo, and a piece that arrived as one number would play its
        // chorus at the tempo of its verse.
        project.tempo_map = composition.tempo_map.clone();
        // One meter for the whole piece: a specification says `meter: 6/8` once, and the composer
        // has no vocabulary for changing it part way through. Nothing stops the meter being
        // edited afterwards — the document holds a map either way.
        project.signatures = SignatureMap::constant(composition.meter);
        // The harmony and the structure the composer wrote the parts against, rather than an
        // empty lane over a song that plainly has chords. It is not decoration: a clip generated
        // afterwards reads the label and the chords under it, so a part added by hand to a
        // composed song comes out belonging to the same song.
        project.harmony = composition.harmony.clone();
        project.sections = composition.sections.clone();
        // What it was asked for, kept with what it produced. A song sheet reopened after a save
        // and a reload refills itself from this, and Another Take goes on working on a piece
        // nobody has the original `.asong` for.
        project.song_spec = Some(composition.spec.clone());

        let mut report = ComposeReport {
            tracks: 0,
            clips: 0,
            notes: 0,
            length: composition.length,
            substituted: Vec::new(),
        };

        // The buses first, so that the tracks routed into them have somewhere to land. They are
        // *added* first and then moved below the parts, because the arrangement reads better as
        // the music followed by the places it is mixed at — and a bus above its feeders would be
        // the first thing a person clicked on.
        let mut buses: Vec<TrackId> = Vec::new();
        for bus in &composition.buses {
            let id = project.add_bus_track(&bus.name);
            if let Some(entry) = project.track_mut(id) {
                entry.color = bus.color;
                entry.mixer.gain_db = bus.gain_db;
            }
            for effect in &bus.effects {
                if !self.registry.has_effect(&effect.id) {
                    // The same trade the instruments get: a missing plugin costs a colour, not a
                    // piece. Without the reverb the room bus is a plain sum, which is quiet and
                    // harmless rather than wrong.
                    report.substituted.push(effect.id.clone());
                    continue;
                }
                let Some(slot) = project.add_effect(Some(id), &effect.id) else {
                    continue;
                };
                if let Some(entry) = project.track_mut(id)
                    && let Some(added) = entry.mixer.effects.iter_mut().find(|s| s.id == slot)
                {
                    added.state = effect.state.clone();
                }
            }
            buses.push(id);
        }

        // The General MIDI font, once, and only if a part actually asked for a sound out of it.
        // A piece written entirely on the built-in voices should not carry a two-hundred-megabyte
        // reference it never plays.
        let general_midi = composition
            .tracks
            .iter()
            .any(|track| track.sound.is_some())
            .then(|| self.adopt_general_midi(&mut project))
            .flatten();
        if general_midi.is_none() && composition.tracks.iter().any(|t| t.sound.is_some()) {
            // Named the way a missing plugin is, because it is the same thing happening: the
            // piece plays, on the instruments the parts also name, and the report is where
            // somebody finds out why it sounds like an oscillator.
            report.substituted.push("General MIDI".to_string());
        }

        for track in &composition.tracks {
            let sound = general_midi.and(track.sound);
            let instrument = match &sound {
                // Choosing a sound is choosing the instrument that makes it, exactly as it is in
                // `set_track_preset`.
                Some(_) => SAMPLER_ID.to_string(),
                None if self.registry.has_instrument(&track.instrument) => track.instrument.clone(),
                None => {
                    report.substituted.push(track.instrument.clone());
                    fallback.clone()
                }
            };
            let track_id = project.add_instrument_track(&track.name, instrument);
            if let Some((sound, font)) = sound.zip(general_midi) {
                if let Some(inner) = project
                    .track_mut(track_id)
                    .and_then(|entry| entry.kind.as_instrument_mut())
                {
                    store_preset(
                        &mut inner.instrument_state,
                        PresetRef {
                            font,
                            bank: i32::from(sound.bank),
                            patch: i32::from(sound.patch),
                        },
                    );
                }
            } else if !track.state.params.is_empty()
                && let Some(inner) = project
                    .track_mut(track_id)
                    .and_then(|entry| entry.kind.as_instrument_mut())
            {
                // Only where the part stayed on the plugin it named. The composer's voicing is
                // written for that plugin's own parameters — a crash asks the noise drum for a
                // long decay and no pitch sweep — and the branch above has just put this track on
                // the sampler instead, where the same keys mean nothing and the sound is a
                // recording of a cymbal that needs no help being one.
                inner.instrument_state = track.state.clone();
            }
            if let Some(entry) = project.track_mut(track_id) {
                // The composer's colour, not the palette's. `add_instrument_track` takes the next
                // palette entry by position, so which colour a part got depended on how many
                // parts were declared before it.
                entry.color = track.color;
                // The composer's level, and — where the part landed on a General MIDI kit — the
                // trim that kit needs to sit where the others do. This is the one place that knows
                // both, because it is the one place a part's `program` has been resolved against
                // a font that is actually installed.
                entry.mixer.gain_db = composed_gain_db(track.gain_db, sound);
                entry.mixer.pan = track.pan;
                // A draft names a bus by its position in the composition, because the composer has
                // no ids to name it by; this is where the two meet.
                entry.output = track
                    .output
                    .and_then(|index| buses.get(index))
                    .map_or(Output::Master, |bus| Output::Bus(*bus));
            }
            for send in &track.sends {
                let Some(bus) = buses.get(send.bus).copied() else {
                    continue;
                };
                let id = project.next_send_id();
                if let Some(entry) = project.track_mut(track_id) {
                    entry.sends.push(AuxSend {
                        id,
                        target: bus,
                        level_db: send.level_db,
                        pre_fader: false,
                    });
                }
            }
            report.tracks += 1;

            for clip in &track.clips {
                let Some(clip_id) = project.add_midi_clip(
                    track_id,
                    &clip.name,
                    clip.start,
                    Ticks(clip.length.raw().max(1)),
                ) else {
                    continue;
                };
                if let Some(target) = project.midi_clip_mut(clip_id) {
                    target.notes = clip.notes.clone();
                    // What the composer played, in the vocabulary a person can edit. This is the
                    // whole of what makes a composed piece re-takeable one clip at a time: every
                    // command a clip written by hand answers to — another take, write it again,
                    // the recipe panel, keep this one — reads exactly this field, so nothing
                    // downstream had to learn about composed songs to work on them.
                    target.recipe = clip.recipe.clone();
                    // The length is the section's and was decided by the form, not grown to fit
                    // what happened to be written into it. Without this the first note edit on a
                    // composed clip would let `fit_length_to_notes` stretch it past the section it
                    // belongs to.
                    target.length_is_explicit = true;
                    report.notes += clip.notes.len();
                }
                report.clips += 1;
            }
        }

        // The buses to the bottom, where a mixing point belongs. Moved rather than created there,
        // because a send cannot name a bus that does not exist yet.
        for bus in &buses {
            project.move_track(*bus, project.tracks.len());
        }

        // And a limiter across the master, because a measurement asked for one rather than
        // because a mix bus usually has one. `kit_trim_db` takes the shipped presets off the
        // ceiling and does not quite clear it: `city-pop` used to peak at +3.06 dBFS on every
        // seed and clip 3507 samples in a hundred-second render, and with the kits normalised it
        // still reaches over on four draws in 128, worst +0.63 dBFS and ten samples. That is a
        // hundredfold better and it is not none, and a piece that arrives over full scale is a
        // piece somebody exports to a 24-bit file and hears crack.
        //
        // A net and nothing more. Seven of those 128 draws even reach the ceiling, and no other
        // preset's loudest draw in 24 comes near it, so on almost every piece this is a slot that
        // passes its input through unchanged. That is what makes it the right shape: the same
        // ceiling held by turning every composed mix down instead would have charged the pieces
        // that never went over for the ones that did.
        //
        // Written into the document as a slot anyone can see and take off, rather than clamped
        // somewhere in the renderer. It is a mixing decision, and a mixing decision a person
        // cannot find is one they cannot disagree with.
        match self.registry.has_effect(MASTER_LIMITER) {
            true => {
                if let Some(slot) = project.add_effect(None, MASTER_LIMITER)
                    && let Some(added) = project.master.effects.iter_mut().find(|s| s.id == slot)
                {
                    // Written down rather than left to the plugin's default, so the ceiling the
                    // piece was measured against is the ceiling the file carries.
                    added
                        .state
                        .params
                        .insert(LIMITER_CEILING.to_string(), MASTER_CEILING_DB);
                }
            }
            // Reported the way a missing reverb on a bus is, and for a better reason: without it
            // the piece is the same piece, only with nothing standing between a rare loud bar and
            // the end of the scale.
            false => report.substituted.push(MASTER_LIMITER.to_string()),
        }

        // Loop over the whole piece, so pressing play and leaving it running plays the song.
        project.loop_region = Some((Ticks::ZERO, composition.length));
        project.loop_enabled = false;

        self.record(Edit::Compose);
        self.replace_project(project);
        self.install_shipped_fonts();
        self.dirty = true;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::ParamTarget;
    use crate::session::fixtures::session;
    use auris_core::{ClipId, ClipPreset, ClipRecipe};

    #[test]
    fn composing_replaces_the_document_in_one_undo_step() {
        let mut session = session();
        session.add_default_instrument_track("Old").unwrap();
        session.forget_history();

        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Composed"
                form = "verse"
                chords = "@axis"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();

        assert!(report.tracks > 0);
        assert!(report.notes > 0);
        assert_eq!(session.project().name, "Composed");
        // Every part reported, plus the buses the mix routes through — which the report does not
        // count, because they are plumbing rather than things that play.
        let buses = session
            .project()
            .tracks
            .iter()
            .filter(|track| track.kind.is_bus())
            .count();
        assert_eq!(session.project().tracks.len(), report.tracks + buses);

        // One step takes the whole piece back, not one note.
        assert_eq!(session.undo(), Some(Edit::Compose));
        assert_eq!(session.project().tracks.len(), 1);
        assert_eq!(session.project().tracks[0].name, "Old");
    }

    #[test]
    fn a_composed_piece_can_be_re_taken_one_clip_at_a_time() {
        // What "Write a Part Here…" has always offered, now on the piece the composer wrote. The
        // commands are the ones a hand-written clip already answered to — nothing downstream had
        // to learn what a composed song is, because the recipe is the whole interface.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Retakeable"
                form = "verse chorus verse"
                chords = "@axis"
                [section.verse]
                bars = 4
                [section.chorus]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        // Every clip the composer wrote, in timeline order, so a neighbour can be checked.
        let clips: Vec<ClipId> = session
            .project()
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_instrument())
            .flat_map(|inner| inner.clips.iter().map(|clip| clip.id))
            .collect();
        assert!(clips.len() > 3, "not enough clips to say anything");
        let generated: Vec<ClipId> = clips
            .iter()
            .copied()
            .filter(|clip| session.clip_recipe(*clip).is_some())
            .collect();
        assert_eq!(
            generated.len(),
            clips.len(),
            "a composed clip arrived without a recipe"
        );

        let notes_of = |session: &Session, clip: ClipId| {
            session.midi_clip(clip).expect("the clip").notes.clone()
        };
        let target = generated[0];
        let neighbour = generated[1];
        let (before, beside) = (notes_of(&session, target), notes_of(&session, neighbour));
        let seed = session.clip_recipe(target).unwrap().seed;

        assert!(session.reroll_clip(target).unwrap() > 0);
        assert_ne!(
            notes_of(&session, target),
            before,
            "the take did not change"
        );
        assert_eq!(
            notes_of(&session, neighbour),
            beside,
            "re-taking one clip moved another"
        );
        assert_eq!(session.clip_recipe(target).unwrap().seed, seed + 1);

        // And one step takes the take back — not the whole piece.
        assert_eq!(session.undo(), Some(Edit::GenerateClip));
        assert_eq!(notes_of(&session, target), before);

        // Every clip, not only the one above: a recipe that named a preset which writes nothing
        // over the harmony the composer left in the document would be a menu row that does
        // nothing, on most of the song, with no error to explain it.
        for clip in &generated {
            assert!(
                session.reroll_clip(*clip).unwrap() > 0,
                "another take of `{}` wrote nothing",
                session.midi_clip(*clip).map_or("?", |midi| &midi.name)
            );
        }
    }

    #[test]
    fn a_frozen_composed_clip_stops_being_rewritten() {
        // The other half of carrying a recipe: a take somebody likes can be taken out of reach.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Frozen"
                form = "verse"
                chords = "@axis"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let clip = session
            .project()
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_instrument())
            .flat_map(|inner| inner.clips.iter())
            .find(|clip| clip.is_generated())
            .expect("a composed clip")
            .id;
        let kept = session.midi_clip(clip).unwrap().notes.clone();

        session.freeze_clip(clip).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes, kept);
        assert!(matches!(
            session.reroll_clip(clip),
            Err(SessionError::NotGenerated(_))
        ));
    }

    #[test]
    fn a_composed_clip_keeps_the_length_the_form_gave_it() {
        // A section's length was decided by the form. Without `length_is_explicit` the first note
        // edit would let `fit_length_to_notes` grow the clip past the section it belongs to, which
        // on a composed song is every clip that has a note near its end.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Lengths"
                form = "verse"
                chords = "@axis"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let (clip, length) = session
            .project()
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_instrument())
            .flat_map(|inner| inner.clips.iter())
            .map(|clip| (clip.id, clip.length))
            .next()
            .expect("a composed clip");

        session
            .add_note(
                clip,
                auris_core::Note::new(60, length - Ticks(1), Ticks::QUARTER * 8),
            )
            .unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().length, length);
    }

    #[test]
    fn a_chorus_that_lifts_the_tempo_lifts_it_on_the_timeline() {
        // The composer writes a map and the document holds one, so this seam is a move rather
        // than a conversion. It used to be `set_bpm(composition.tempo)`, which is the shape that
        // would silently keep working while playing every section at the first one's tempo.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                tempo = 120
                form  = "verse chorus"

                [section.chorus]
                tempo = 132
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let project = session.project();
        let bar = project.signatures.signature_at(Ticks::ZERO).ticks_per_bar();
        assert_eq!(project.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(project.tempo_map.bpm_at(bar * 8), 132.0);
    }

    #[test]
    fn a_cymbal_that_stayed_on_its_plugin_arrives_voiced() {
        // The composer knows a crash wants a long decay and no pitch sweep and has never heard of
        // the instrument that will play one. This is where the two meet, and it is the only place
        // a composed track's parameters are set at all — so a crash arriving at the shipped
        // quarter-second decay is this seam having quietly come apart.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = ["chorus"]

                [[part]]
                name = "crash"
                role = "crash"
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let crash = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "crash")
            .and_then(|track| track.kind.as_instrument())
            .expect("the cymbal has a track");
        assert!(
            crash.instrument_state.params["decay"] > 1.0,
            "the cymbal arrived at the noise drum's own decay: {:?}",
            crash.instrument_state.params
        );

        // And nothing else is touched. A composed piece is a mix, not a set of edited plugins.
        let lead = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "lead")
            .and_then(|track| track.kind.as_instrument());
        assert!(lead.is_none_or(|lead| lead.instrument_state.params.is_empty()));
    }

    #[test]
    fn a_piece_asking_for_sounds_this_build_has_none_of_still_plays() {
        // `session()` is headless, which means no shipped library — deliberately, so that this
        // test says the same thing on a machine with the SoundFont installed and one without.
        // What it pins is the fallback: a part naming a violin comes out on the oscillator it
        // *also* names, and the report says why rather than leaving a piece that sounds wrong for
        // no visible reason.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = ["verse"]

                [[part]]
                name    = "lead"
                role    = "melody"
                program = "Violin"
                "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();

        assert!(
            report.substituted.iter().any(|name| name == "General MIDI"),
            "the report should say the font was missing: {:?}",
            report.substituted
        );
        assert!(
            session.project().soundfonts.is_empty(),
            "and the document should not name a font that is not there"
        );
        let lead = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "lead")
            .expect("the part became a track");
        assert_eq!(
            lead.kind.as_instrument().map(|inner| &inner.instrument_id),
            Some(&auris_compose::Role::Melody.default_instrument().to_string()),
            "the part keeps the plugin it named"
        );
        assert_eq!(session.track_preset(lead.id), None);
    }

    #[test]
    fn the_kit_trims_bring_every_kit_of_the_shipped_font_to_the_quietest() {
        // The measurement the table is, restated as the thing it claims: each kit's kick, at
        // unity, on the shipped font, and the trim that puts it where the quietest one already
        // is. Written out again here rather than derived, so that changing a trim without
        // remeasuring — or remeasuring and forgetting a kit — fails rather than passes quietly.
        let kits: [(u8, f32, f32); 8] = [
            (0, 2.38, -3.82),
            (8, 5.32, -6.76),
            (16, 1.95, -3.39),
            (24, 2.46, -3.90),
            (25, -1.43, 0.0),
            (32, 6.51, -7.95),
            (40, 6.51, -7.95),
            (48, -1.26, 0.0),
        ];
        for (patch, unity, trim) in kits {
            let sound = auris_compose::gm::Program(patch).sound(true);
            assert_eq!(
                kit_trim_db(sound),
                trim,
                "{}",
                auris_compose::gm::Program(patch).kit_name()
            );
            // And what it buys: 7.95 dB of spread across the font comes down to a fifth of a
            // decibel, which is the whole claim. -1.44 is the reference, -1.26 the Orchestra Kit
            // that is left alone because it is already within that.
            let landed = unity + trim;
            assert!(
                (-1.45..=-1.25).contains(&landed),
                "{} lands at {landed} dBFS",
                auris_compose::gm::Program(patch).kit_name()
            );
            // A calibration takes away; it never hands a kit gain it did not measure.
            assert!(trim <= 0.0);
        }
        // Every kit the composer can name is one of those eight, so nothing a specification can
        // write falls through to the untouched case by accident.
        for (patch, name) in auris_compose::gm::KITS {
            assert!(
                kits.iter().any(|(measured, _, _)| *measured == patch),
                "{name} is a kit a specification can name and nobody measured it"
            );
        }

        // A patch nobody measured is left where it is. A font may put a kit at any number in
        // bank 128, and a trim for a sound this build has never heard would be a guess.
        assert_eq!(kit_trim_db(auris_compose::gm::Program(1).sound(true)), 0.0);
        assert_eq!(
            kit_trim_db(auris_compose::gm::Program(127).sound(true)),
            0.0
        );
        // And the bank is what decides that this is a kit at all: patch 0 is a grand piano in the
        // pitched set and the Standard Kit in the percussion one, and only the second is trimmed.
        assert_eq!(kit_trim_db(auris_compose::gm::Program(0).sound(false)), 0.0);
        assert_eq!(
            kit_trim_db(auris_compose::gm::Program(40).sound(false)),
            0.0
        );
    }

    #[test]
    fn a_composed_kit_is_trimmed_on_top_of_the_level_its_role_asked_for() {
        // The kit trim is an apology for the font, and the role gain is the mix the composer
        // wrote. Adding them keeps both readable; replacing one with the other would mean a
        // drummer at -5 dB and a drummer at -12 arrived at the same fader.
        let room = auris_compose::gm::Program(8).sound(true);
        let role = auris_compose::Role::Kick.default_gain_db();
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(
            close(composed_gain_db(role, Some(room)), role + kit_trim_db(room)),
            "{} is neither the role's level nor the trim alone",
            composed_gain_db(role, Some(room))
        );
        assert!(composed_gain_db(role, Some(room)) < role.min(kit_trim_db(room)));
        // A part that never reached a font keeps exactly the level it was written at — which is
        // what leaves the built-in kit, already far under its own pitched parts, alone.
        assert_eq!(composed_gain_db(role, None), role);

        // And the document `compose` actually builds says the same. Whether the two hundred
        // megabytes are on this machine is a fact about the machine — `session()` is headless
        // precisely so that a document does not come out different on a CI runner — so what is
        // asserted below is the rule rather than one machine's number.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = ["verse"]

                [section.verse]
                bars = 4

                [[part]]
                name    = "kick"
                role    = "kick"
                program = "Room Kit"
                "#,
        )
        .unwrap();
        let piece = auris_compose::compose(&spec);
        let part = piece
            .tracks
            .iter()
            .find(|track| track.name == "kick")
            .expect("the part was written");
        assert_eq!(part.gain_db, role, "the part is at its role's level");
        assert_eq!(part.sound, Some(room), "and it asked for the Room Kit");

        session.compose(&piece).unwrap();
        let strip = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "kick")
            .expect("the part became a track")
            .mixer
            .gain_db;
        // Which way this comes out depends on whether the font reached the document, so it is
        // stated as the rule rather than as a number: the strip is the part's level put through
        // the same decision, and it moves off that level exactly when there is a kit playing.
        let on_a_kit = !session.project().soundfonts.is_empty();
        let expected = composed_gain_db(role, on_a_kit.then_some(room));
        assert!(
            close(strip, expected),
            "the strip is at {strip} dB and should be at {expected}"
        );
        assert_eq!(
            on_a_kit,
            strip < role,
            "a composed part is trimmed exactly when it landed on the font the trim is for"
        );
    }

    #[test]
    fn a_composed_document_remembers_what_it_was_asked_for() {
        // Without this, a piece composed, saved and reopened comes back to a dialog full of
        // defaults, and Another Take on it writes a different song rather than another take of
        // that one. The text is the format's own, so it reads back as the specification it was.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                title = "Remembered"
                key = "C minor"
                form = "verse chorus"
                chords = "@marusa"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let remembered = session
            .project()
            .song_spec
            .clone()
            .expect("a composed document carries its specification");
        assert_eq!(
            auris_compose::SongSpec::parse(&remembered),
            Ok(spec),
            "\n{remembered}"
        );

        // A document nobody composed carries nothing, rather than a specification that would
        // describe a song it is not.
        assert!(Project::new("By Hand", 48_000.0).song_spec.is_none());
    }

    #[test]
    fn a_composed_document_carries_its_harmony_and_its_structure() {
        // Both were computed by the composer and then dropped on the floor here: a composed song
        // opened with an empty harmony lane and an empty structure lane over a piece that plainly
        // had chords and sections. What made it worse than cosmetic is that `generate_clip` reads
        // both — so a part added to a composed song by hand had nothing to agree with.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
            title  = "Whole"
            key    = "C minor"
            form   = "intro verse chorus"
            chords = "@marusa"

            [section.intro]
            bars = 4

            [section.verse]
            bars = 8

            [section.chorus]
            bars = 8
            "#,
        )
        .unwrap();
        let piece = auris_compose::compose(&spec);
        session.compose(&piece).unwrap();

        let project = session.project();
        assert!(!project.harmony.chords.is_empty(), "no chords were carried");
        assert_eq!(project.harmony.keys.initial(), piece.harmony.keys.initial());
        assert_eq!(
            project.harmony.chords.points().len(),
            piece.harmony.chords.points().len()
        );

        // The labels the clips were named after are the labels on the timeline.
        assert_eq!(
            project
                .sections
                .section_at(Ticks::ZERO)
                .map(|(name, _)| name),
            Some("intro")
        );
        let bar = project.signatures.signature_at(Ticks::ZERO).ticks_per_bar();
        assert_eq!(
            project.sections.section_at(bar * 4).map(|(name, _)| name),
            Some("verse")
        );

        // And a clip generated afterwards finds them: the same question the piano roll asks.
        let track = project.tracks[0].id;
        let clip = session
            .generate_clip(
                track,
                bar * 4,
                bar * 4,
                ClipRecipe::new(ClipPreset::Lead, 7),
            )
            .expect("a clip over the composed song");
        let notes = session.project().midi_clip(clip).unwrap().1.notes.len();
        assert!(
            notes > 0,
            "a clip written over a composed song came out empty"
        );
    }

    #[test]
    fn a_composed_document_arrives_already_routed() {
        // The composer names a bus by its position, because it has no ids to name one by. This is
        // where the two meet, and getting it wrong would point every send at the wrong strip.
        let mut session = session();
        let spec = auris_compose::SongSpec::default();
        let piece = auris_compose::compose(&spec);
        session.compose(&piece).unwrap();

        let project = session.project();
        let bus = |name: &str| {
            project
                .tracks
                .iter()
                .find(|track| track.kind.is_bus() && track.name == name)
                .unwrap_or_else(|| panic!("no {name} bus"))
        };
        let drums = bus("Drums").id;
        let room = bus("Room").id;

        // The kit under one fader, and the room fed rather than routed through.
        let track = |name: &str| project.tracks.iter().find(|t| t.name == name).unwrap();
        assert_eq!(track("kick").output, Output::Bus(drums));
        assert_eq!(track("lead").output, Output::Master);
        assert_eq!(track("lead").sends[0].target, room);
        assert!(track("bass").sends.is_empty());

        // The reverb is on the bus and is all reflection, which is the one setting a send/return
        // reverb cannot be left at its default for.
        let reverb = &bus("Room").mixer.effects[0];
        assert_eq!(reverb.effect_id, "auris.fx.reverb");
        assert_eq!(reverb.state.params.get("mix"), Some(&1.0));

        // The buses sit below the music, and nothing about the routing loops.
        assert!(project.tracks[project.tracks.len() - 1].kind.is_bus());
        for track in &project.tracks {
            for target in track.feeds() {
                assert!(!project.routing_would_cycle(track.id, target));
            }
        }
    }

    #[test]
    fn a_composed_piece_renders_to_audible_audio() {
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = "verse"
                chords = "@marusa"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let rendered = session
            .render_job()
            .render(
                &auris_engine::OfflineOptions::whole_project(),
                &mut auris_engine::RenderProgress::default(),
            )
            .unwrap();
        assert!(
            rendered.peak() > 0.01,
            "a composed piece rendered silence, peak {}",
            rendered.peak()
        );
    }

    #[test]
    fn a_composed_piece_cannot_leave_the_master_over_full_scale() {
        // The net under the kit trim, measured rather than described. `kit_trim_db` took the
        // shipped presets off the ceiling and did not clear it — four seeds of `city-pop` in a
        // hundred and twenty-eight still reached over, the worst at +0.63 dBFS — so a composed
        // document carries a limiter, and what that has to be worth is a number.
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = "verse"
                chords = "@marusa"
                [section.verse]
                bars = 4
                "#,
        )
        .unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();

        let slot = session
            .project()
            .master
            .effects
            .last()
            .expect("a composed master carries a limiter");
        assert_eq!(
            slot.effect_id, MASTER_LIMITER,
            "and it is the last thing in the chain"
        );
        assert_eq!(
            slot.state.params.get(LIMITER_CEILING),
            Some(&MASTER_CEILING_DB),
            "with the ceiling the piece was measured against written down"
        );

        // Twenty decibels of drive, which is far more than any preset has ever asked for, so that
        // what is measured is the ceiling holding rather than the piece happening to be quiet.
        let tracks: Vec<TrackId> = session
            .project()
            .tracks
            .iter()
            .map(|track| track.id)
            .collect();
        for track in tracks {
            session.set_param(ParamTarget::TrackGain(track), 20.0);
        }
        let rendered = session
            .render_job()
            .render(
                &auris_engine::OfflineOptions::whole_project(),
                &mut auris_engine::RenderProgress::default(),
            )
            .unwrap();
        let ceiling = auris_core::param::db_to_gain(MASTER_CEILING_DB);
        assert!(
            rendered.peak() > ceiling * 0.9,
            "the drive did not reach the limiter, peak {}",
            rendered.peak()
        );
        assert!(
            rendered.peak() <= ceiling,
            "the master left {} through a ceiling of {ceiling}",
            rendered.peak()
        );
    }

    #[test]
    fn an_unknown_instrument_costs_a_timbre_rather_than_the_piece() {
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
                form = "verse"
                [section.verse]
                bars = 2
                [[part]]
                name = "lead"
                instrument = "nope.not.here"
                "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();
        assert_eq!(report.substituted, ["nope.not.here"]);
        assert_eq!(report.tracks, 1, "the track was still created");
        assert!(report.notes > 0);
    }
}
