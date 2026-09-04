//! Tracks: what a clip sits on, and what decides which clips may sit on it.
//!
//! A [`Track`] is a name, a colour, a height and a [`TrackKind`], and the kind is the whole of
//! what separates one lane from another. A bus is a kind rather than a type of its own so that
//! it gets a fader, a pan, a mute, an effect chain, a colour and an automation lane without one
//! of them being written twice.
//!
//! [`Color`] is here rather than beside the ids because a track is the only thing that carries
//! one; a clip is tinted by the track it is on.
//!
//! The lifecycle is here too — adding, duplicating, moving and removing a track — because each
//! of those has to reach into what a track holds. Removing one takes its automation and the
//! routing that named it with it, and replacing its instrument takes the lanes that drove the
//! old one: a lane names a track and a parameter index, never the plugin.

use serde::{Deserialize, Serialize};

use crate::asset::AssetPath;
use crate::plugin::PluginState;
use crate::time::{TempoMap, Ticks};

use super::clip::{AudioClip, MidiClip, audio_clip_ticks};
use super::routing::{AuxSend, MixerStrip, Output};
use super::{ClipId, EffectSlotId, Project, SendId, TrackId};

/// An RGB colour used for track and clip tinting.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub u32);

impl Color {
    /// The palette new tracks cycle through.
    pub const PALETTE: [Color; 8] = [
        Color(0x4f9dde),
        Color(0x5fc9a3),
        Color(0xe0b452),
        Color(0xd97b6c),
        Color(0xb07cc6),
        Color(0xe0a458),
        Color(0x7fb069),
        Color(0xd16b8a),
    ];

    /// Picks a palette entry by index, wrapping around.
    pub fn from_palette(index: usize) -> Color {
        Self::PALETTE[index % Self::PALETTE.len()]
    }

    /// Red, green and blue components.
    pub fn rgb(self) -> (u8, u8, u8) {
        (
            ((self.0 >> 16) & 0xff) as u8,
            ((self.0 >> 8) & 0xff) as u8,
            (self.0 & 0xff) as u8,
        )
    }
}

/// An instrument track: notes rendered by a software instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentTrack {
    /// Registry id of the instrument.
    pub instrument_id: String,
    /// Saved instrument parameters.
    pub instrument_state: PluginState,
    /// Note clips on the timeline.
    pub clips: Vec<MidiClip>,
    /// The plugin file this track's instrument lives in, for one the registry cannot build.
    ///
    /// The same field, for the same reasons, as
    /// [`EffectSlot::file`](crate::project::routing::EffectSlot::file): `None` for every built-in,
    /// and always [`External`](AssetPath::External) for a hosted plugin, because a plugin is a
    /// library shared by every project on the machine rather than one song's asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<AssetPath>,
}

impl InstrumentTrack {
    /// `true` when the instrument names a plugin the registry cannot build.
    pub fn is_hosted(&self) -> bool {
        self.file.is_some()
    }
}

/// An audio track: references to imported audio.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    /// Audio clips on the timeline.
    pub clips: Vec<AudioClip>,
}

/// A singer track: notes carrying lyrics, sung by a voice synthesiser.
///
/// The clips are ordinary [`MidiClip`]s — a sung phrase is still notes on a timeline, and every
/// gesture the piano roll knows applies to it unchanged — but each [`Note`](super::Note) on one
/// may carry a lyric and the phonemes it is sung as. What separates the kind from
/// [`InstrumentTrack`] is what the notes are *for*: a voice model renders frame-level phonemes,
/// pitch and energy from them, offline — and what is heard is either the [`SingerTake`] that
/// render produced, or, while there is none, the built-in preview instrument `instrument_id`
/// names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingerTrack {
    /// Registry id of the instrument previewing the melody.
    pub instrument_id: String,
    /// Saved preview-instrument parameters.
    #[serde(default, skip_serializing_if = "PluginState::is_empty")]
    pub instrument_state: PluginState,
    /// Note clips on the timeline.
    pub clips: Vec<MidiClip>,
    /// Seconds per feature frame handed to the voice model.
    ///
    /// Stored on the track rather than asked at export time because it is a property of the
    /// model the track is written for: exporting the same document twice must produce the same
    /// frames.
    #[serde(default = "default_frame_hop")]
    pub frame_hop: f64,
    /// The voice model this track is sung by, when one has been chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<SingerVoice>,
    /// The last rendered take, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<SingerTake>,
}

/// The voice a singer track is sung by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingerVoice {
    /// Where the model file is.
    ///
    /// Normally [`AssetPath::External`], for the reason a SoundFont's path is: a voice is a
    /// library shared by every project on the machine, not one song's asset.
    pub path: AssetPath,
    /// Display name, from the model's own voice card where it carries one.
    ///
    /// Stored in the document the way a SoundFont's name is, so a track header can say 波音リツ
    /// without opening two hundred megabytes first.
    pub name: String,
    /// The consonant widths the model measured from its own training data, where its export
    /// carried them.
    ///
    /// Copied into the document when the voice is chosen, for the name's reason: the phoneme
    /// timing has to lay out the same on a machine that has not loaded — or does not have —
    /// the model file. `None` means the export predates the table, and the timing rule falls
    /// back to its single fixed width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consonants: Option<ConsonantWidths>,
    /// How loud the model's training data sang each consonant against the vowel after it,
    /// where its export carried the table.
    ///
    /// Copied in beside the widths for the same reason. `None` means the export predates the
    /// table, and every consonant is given the note's full level — which is what the frames
    /// always did, and is measurably wrong: a /k/ at the vowel's loudness is a /k/ the model
    /// has never heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<ConsonantLevels>,
    /// Which of the model's speakers sings, by the name its export gives the speaker.
    ///
    /// `None` is the model's first speaker — the only one a single-speaker model has, so a
    /// document never has to name what it never chose. A name the model does not know is
    /// refused when the voice is asked to sing, never guessed at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// Per-phoneme consonant levels, in decibels against the vowel that follows, measured by a
/// voice model from its training data.
///
/// A voiceless plosive or fricative sings twenty-odd decibels under the vowel after it, a
/// voiced one six to nine, an approximant three. A frames writer that gives every phoneme
/// the note's velocity asks the model for consonants it has never heard at that loudness and
/// gets none; measured on JSUT-song, that plateau alone cost the phoneme error rate 0.25 →
/// 0.56, and these medians recovered most of it. The application rule is
/// [`ConsonantLevels::db`]: the table's entry, or `default` for a consonant it never
/// measured — and no entry at all for a syllabic, which keeps the note's level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsonantLevels {
    /// Decibels for a consonant the table has no entry for.
    pub default: f64,
    /// Decibels per phoneme, keyed by the model's own symbols.
    #[serde(default)]
    pub db: std::collections::BTreeMap<String, f64>,
}

impl ConsonantLevels {
    /// Decibels `phoneme` sings at, against its vowel: its measured level, or the default.
    pub fn db(&self, phoneme: &str) -> f64 {
        self.db.get(phoneme).copied().unwrap_or(self.default)
    }

    /// Whether the table measured `phoneme` itself.
    pub fn measured(&self, phoneme: &str) -> bool {
        self.db.contains_key(phoneme)
    }
}

/// Per-phoneme consonant widths, in seconds, measured by a voice model from its training data.
///
/// Consonant length in sung Japanese spans a factor of three by phoneme class — an affricate
/// like `ts` takes twice what a plain stop does — so a single fixed width mistimes half the
/// inventory. A voice model's export can carry the widths it was actually trained on; this is
/// that table as the document stores it. The application rule is one line:
/// [`ConsonantWidths::width`] answers the table's entry, or `default` for a phoneme it never
/// measured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsonantWidths {
    /// Seconds for a phoneme the table has no entry for.
    pub default: f64,
    /// Seconds per phoneme, keyed by the model's own symbols.
    #[serde(default)]
    pub seconds: std::collections::BTreeMap<String, f64>,
}

impl ConsonantWidths {
    /// Seconds `phoneme` takes: its measured width, or the default where none was measured.
    pub fn width(&self, phoneme: &str) -> f64 {
        self.seconds.get(phoneme).copied().unwrap_or(self.default)
    }
}

/// One rendered performance of a singer track, kept as audio.
///
/// The take is what playback uses whenever it exists — even after the notes move on, because a
/// voice someone chose should not fall back to a formant preview the moment a word is edited.
/// The `fingerprint` is how a frontend *knows* the notes moved on: it hashes everything the
/// render read (the frames, the voice, the seed), so comparing it against a fresh hash says
/// "behind the score" without keeping the frames themselves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingerTake {
    /// The audio source holding the rendered waveform, starting at the beginning of the
    /// timeline.
    pub source: super::SourceId,
    /// Hash of the frames, the voice and the seed the take was rendered from.
    pub fingerprint: u64,
    /// The seed the take's random choices were pinned by.
    pub seed: u64,
}

/// The frame hop a new singer track starts with, in seconds.
///
/// Ten milliseconds is the hop most acoustic-feature pipelines default to; a track written for a
/// model that wants another one stores its own.
pub fn default_frame_hop() -> f64 {
    0.010
}

/// What kind of material a track holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackKind {
    /// Notes played by a software instrument.
    Instrument(InstrumentTrack),
    /// Notes carrying lyrics, sung by a voice synthesiser.
    Singer(SingerTrack),
    /// Recorded or imported audio.
    Audio(AudioTrack),
    /// A mixing point: whatever is routed here, summed, and put through one strip.
    ///
    /// A bus holds no clips of its own — its material arrives from the tracks that name it, as an
    /// [`Output`] or through a [`AuxSend`]. It is a track rather than a thing of its own so that it
    /// gets a fader, a pan, a mute, an effect chain, a colour and an automation lane without any
    /// of them being written twice.
    Bus,
}

impl TrackKind {
    /// Short label for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            TrackKind::Instrument(_) => "Instrument",
            TrackKind::Singer(_) => "Singer",
            TrackKind::Audio(_) => "Audio",
            TrackKind::Bus => "Bus",
        }
    }

    /// `true` when this track's instrument can be chosen — swapped from the registry or a
    /// plugin file.
    ///
    /// A singer track also *has* an instrument, but only as a preview: what the track is for is
    /// chosen by its kind, not by a picker. Code that is about notes rather than about the
    /// instrument wants [`Self::holds_notes`] instead.
    pub fn is_instrument(&self) -> bool {
        matches!(self, TrackKind::Instrument(_))
    }

    /// `true` when this track holds note clips — an instrument track or a singer track.
    ///
    /// This is the question that decides whether a note clip may sit on a track, and it is asked
    /// directly rather than through a pattern match at each call site so that a clip can move
    /// between the two kinds that answer yes: a melody sketched on an instrument track keeps its
    /// notes when it is dragged onto a singer track to be given words.
    pub fn holds_notes(&self) -> bool {
        matches!(self, TrackKind::Instrument(_) | TrackKind::Singer(_))
    }

    /// `true` when this track is a singer track.
    pub fn is_singer(&self) -> bool {
        matches!(self, TrackKind::Singer(_))
    }

    /// `true` when this track is a mixing point rather than a source of material.
    pub fn is_bus(&self) -> bool {
        matches!(self, TrackKind::Bus)
    }

    /// The instrument track data, when this is one.
    pub fn as_instrument(&self) -> Option<&InstrumentTrack> {
        match self {
            TrackKind::Instrument(track) => Some(track),
            _ => None,
        }
    }

    /// The instrument track data mutably, when this is one.
    pub fn as_instrument_mut(&mut self) -> Option<&mut InstrumentTrack> {
        match self {
            TrackKind::Instrument(track) => Some(track),
            _ => None,
        }
    }

    /// The singer track data, when this is one.
    pub fn as_singer(&self) -> Option<&SingerTrack> {
        match self {
            TrackKind::Singer(track) => Some(track),
            _ => None,
        }
    }

    /// The singer track data mutably, when this is one.
    pub fn as_singer_mut(&mut self) -> Option<&mut SingerTrack> {
        match self {
            TrackKind::Singer(track) => Some(track),
            _ => None,
        }
    }

    /// The note clips this track holds, when it holds notes at all.
    ///
    /// One accessor for both kinds that do, because almost everything done to a note clip —
    /// finding it, moving it, splitting it, playing it — is the same done on either, and a match
    /// at each of those sites would be a place for the singer arm to be forgotten.
    pub fn note_clips(&self) -> Option<&Vec<MidiClip>> {
        match self {
            TrackKind::Instrument(track) => Some(&track.clips),
            TrackKind::Singer(track) => Some(&track.clips),
            _ => None,
        }
    }

    /// The note clips mutably, when this track holds notes at all.
    pub fn note_clips_mut(&mut self) -> Option<&mut Vec<MidiClip>> {
        match self {
            TrackKind::Instrument(track) => Some(&mut track.clips),
            TrackKind::Singer(track) => Some(&mut track.clips),
            _ => None,
        }
    }

    /// The audio track data, when this is one.
    pub fn as_audio(&self) -> Option<&AudioTrack> {
        match self {
            TrackKind::Audio(track) => Some(track),
            _ => None,
        }
    }

    /// The audio track data mutably, when this is one.
    pub fn as_audio_mut(&mut self) -> Option<&mut AudioTrack> {
        match self {
            TrackKind::Audio(track) => Some(track),
            _ => None,
        }
    }
}

/// One track in the arrangement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Unique within the project.
    pub id: TrackId,
    /// Name shown in the track header.
    pub name: String,
    /// Tint for the header and its clips.
    pub color: Color,
    /// Height of the lane in the arrangement, in pixels.
    #[serde(default = "default_track_height")]
    pub height: f32,
    /// Instrument or audio content.
    pub kind: TrackKind,
    /// Volume, pan and effects.
    pub mixer: MixerStrip,
    /// Where this track's output goes.
    #[serde(default)]
    pub output: Output,
    /// Copies of this track's signal, fed to buses alongside its own output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<AuxSend>,
}

fn default_track_height() -> f32 {
    72.0
}

impl Track {
    /// Position just past the last clip on this track, in ticks.
    ///
    /// Audio clip lengths depend on the tempo map, so it is passed in.
    pub fn end_tick(&self, tempo_map: &TempoMap, sample_rate: f64) -> Ticks {
        match &self.kind {
            // The *sounding* end of each clip, repeats included. A track whose last clip is
            // looped goes on playing past the clip's own end, and an export measured from that
            // end would cut the repeats off the file.
            TrackKind::Instrument(_) | TrackKind::Singer(_) => self
                .kind
                .note_clips()
                .into_iter()
                .flatten()
                .map(MidiClip::sounding_end)
                .max()
                .unwrap_or(Ticks::ZERO),
            TrackKind::Audio(track) => track
                .clips
                .iter()
                .map(|clip| {
                    // The shared helper, not inline arithmetic: it guards the sample rate, and
                    // a second copy of the conversion is a second place for the guard to be
                    // forgotten — which is exactly how this one came to divide by zero.
                    let length = audio_clip_ticks(tempo_map, sample_rate, clip);
                    clip.start + super::clip::sounding_length(length, clip.loop_end)
                })
                .max()
                .unwrap_or(Ticks::ZERO),
            // A bus holds nothing of its own; what it plays ends when its feeders do.
            TrackKind::Bus => Ticks::ZERO,
        }
    }

    /// Every bus this track feeds: its output when that is one, then each send's target.
    ///
    /// The routing graph's out-edges for one node, in one place, because every question about the
    /// routing — the order to render in, whether an edit would make a loop, how far behind a chain
    /// runs — is a walk over exactly these.
    pub fn feeds(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.output
            .bus()
            .into_iter()
            .chain(self.sends.iter().map(|send| send.target))
    }
}

impl Project {
    /// Appends a track of any kind, routed to the master with no sends.
    fn push_track(&mut self, name: impl Into<String>, kind: TrackKind) -> TrackId {
        let id = TrackId(self.allocate_id());
        let color = Color::from_palette(self.tracks.len());
        self.tracks.push(Track {
            id,
            name: name.into(),
            color,
            height: default_track_height(),
            kind,
            mixer: MixerStrip::default(),
            output: Output::Master,
            sends: Vec::new(),
        });
        id
    }

    /// Appends an instrument track playing `instrument_id`.
    pub fn add_instrument_track(
        &mut self,
        name: impl Into<String>,
        instrument_id: impl Into<String>,
    ) -> TrackId {
        self.push_track(
            name,
            TrackKind::Instrument(InstrumentTrack {
                instrument_id: instrument_id.into(),
                instrument_state: PluginState::empty(),
                clips: Vec::new(),
                file: None,
            }),
        )
    }

    /// Points a track's instrument at a plugin hosted from a file.
    ///
    /// The clips are left exactly where they are, which is the whole point: swapping the sound a
    /// part is played by is not the same as replacing the part. The *state* does go, because it
    /// belongs to the instrument that is being replaced — a cutoff frequency from a chiptune synth
    /// means nothing to Surge XT, and a stale entry under a key the new plugin happens to share
    /// would be worse than nothing.
    ///
    /// `false` when there is no such track, or it is not an instrument track.
    pub fn set_hosted_instrument(
        &mut self,
        track_id: TrackId,
        instrument_id: impl Into<String>,
        file: AssetPath,
    ) -> bool {
        let Some(inner) = self
            .track_mut(track_id)
            .and_then(|track| track.kind.as_instrument_mut())
        else {
            return false;
        };
        inner.instrument_id = instrument_id.into();
        inner.instrument_state = PluginState::empty();
        inner.file = Some(file);
        self.remove_instrument_automation(track_id);
        true
    }

    /// Appends a singer track previewed through `instrument_id`.
    pub fn add_singer_track(
        &mut self,
        name: impl Into<String>,
        instrument_id: impl Into<String>,
    ) -> TrackId {
        self.push_track(
            name,
            TrackKind::Singer(SingerTrack {
                instrument_id: instrument_id.into(),
                instrument_state: PluginState::empty(),
                clips: Vec::new(),
                frame_hop: default_frame_hop(),
                voice: None,
                take: None,
            }),
        )
    }

    /// Appends an empty audio track.
    pub fn add_audio_track(&mut self, name: impl Into<String>) -> TrackId {
        self.push_track(name, TrackKind::Audio(AudioTrack::default()))
    }

    /// Appends a bus, which nothing is routed to yet.
    pub fn add_bus_track(&mut self, name: impl Into<String>) -> TrackId {
        self.push_track(name, TrackKind::Bus)
    }

    /// Copies a track, inserting the copy directly below the original.
    ///
    /// Every nested id is reissued. A shallow clone would leave two clips answering to one
    /// [`ClipId`], and every lookup here returns the *first* match — so an edit aimed at the
    /// copy would silently land on the original.
    pub fn duplicate_track(&mut self, id: TrackId) -> Option<TrackId> {
        let index = self.track_index(id)?;
        // Cloned out first so the id allocator is free to borrow `self` again.
        let mut copy = self.tracks[index].clone();

        copy.id = TrackId(self.allocate_id());
        copy.name = format!("{} copy", copy.name);
        for slot in &mut copy.mixer.effects {
            slot.id = EffectSlotId(self.allocate_id());
        }
        for send in &mut copy.sends {
            send.id = SendId(self.allocate_id());
        }
        match &mut copy.kind {
            TrackKind::Instrument(_) | TrackKind::Singer(_) => {
                // Reserved one by one rather than inside the loop below: the clip list borrows
                // `copy`, not `self`, so the allocator is free — but the accessor is matched here
                // so a future kind that holds clips cannot fall through to "no ids to reissue".
                for clip in copy.kind.note_clips_mut().into_iter().flatten() {
                    clip.id = ClipId(self.allocate_id());
                }
            }
            TrackKind::Audio(inner) => {
                for clip in &mut inner.clips {
                    clip.id = ClipId(self.allocate_id());
                }
            }
            TrackKind::Bus => {}
        }

        let new_id = copy.id;
        self.tracks.insert(index + 1, copy);
        Some(new_id)
    }

    /// Removes a track, returning `true` when it existed.
    pub fn remove_track(&mut self, id: TrackId) -> bool {
        let before = self.tracks.len();
        self.tracks.retain(|track| track.id != id);
        let removed = self.tracks.len() != before;
        if removed {
            // Its automation leaves with it. A lane left behind names a track that is not there,
            // and ids are handed out again — so it would come back to life driving a parameter on
            // whichever track was created next.
            self.automation.remove_track(id);
            // And so does everything that fed it. A deleted bus takes the *routing* with it, not
            // the tracks: what was going through it goes straight to the master instead, which is
            // where it would have been had the bus never existed.
            for track in &mut self.tracks {
                if track.output == Output::Bus(id) {
                    track.output = Output::Master;
                }
                track.sends.retain(|send| send.target != id);
            }
            // Including what it was keying. See `clear_sidechains_from` for why a slot must not
            // be left holding the id.
            self.clear_sidechains_from(id);
        }
        removed
    }

    /// Moves a track to a new index, clamping into range.
    pub fn move_track(&mut self, id: TrackId, to_index: usize) {
        let Some(from) = self.track_index(id) else {
            return;
        };
        let to = to_index.min(self.tracks.len().saturating_sub(1));
        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
    }

    /// Index of a track by id.
    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|track| track.id == id)
    }

    /// A track by id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    /// A track by id, mutably.
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.id == id)
    }

    /// Drops every lane driving a track's instrument, returning `true` when there was one.
    ///
    /// Called when the instrument itself is replaced, for the reason the swap also throws away
    /// the saved parameter values: a lane names a track and a parameter *index*, never the plugin
    /// that owns it, so a curve drawn for one instrument's waveform would carry straight on as a
    /// curve driving whatever the next instrument keeps at that index. It lives here rather than
    /// in the caller so that whatever changes a track's instrument next inherits the cleanup.
    pub fn remove_instrument_automation(&mut self, track: TrackId) -> bool {
        self.automation.remove_lanes_where(|target| {
            matches!(target, crate::param::ParamTarget::Instrument { track: id, .. } if id == track)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Note;
    use crate::project::fixtures::{bussed_project, demo_project};

    #[test]
    fn a_singer_track_written_before_voices_existed_still_opens() {
        // The two new fields ride on defaults, so no format bump was spent on them; this is
        // the receipt. A document from that era names only the preview instrument.
        let old = r#"{
            "instrument_id": "auris.synth.vocal",
            "clips": [],
            "frame_hop": 0.01
        }"#;
        let track: SingerTrack = serde_json::from_str(old).unwrap();
        assert!(track.voice.is_none() && track.take.is_none());

        // And a track that has neither writes neither, so old files stay byte-stable.
        let written = serde_json::to_string(&track).unwrap();
        assert!(!written.contains("voice") && !written.contains("take"));
    }

    #[test]
    fn a_hosted_instrument_keeps_the_part_and_drops_the_old_settings() {
        let mut project = Project::new("Hosted", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.chiptune");
        project
            .track_mut(track)
            .and_then(|track| track.kind.as_instrument_mut())
            .expect("an instrument track")
            .instrument_state
            .params
            .insert("pulse_width".into(), 0.25);
        let instrument_lane = crate::param::ParamTarget::Instrument {
            track,
            param: crate::param::ParamId(0),
        };
        let fader_lane = crate::param::ParamTarget::TrackGain(track);
        for target in [instrument_lane, fader_lane] {
            assert!(project.automation.set_point(
                target,
                None,
                crate::automation::AutomationCurve::Linear,
                Ticks::ZERO,
                0.5,
            ));
        }

        let file = AssetPath::external("/plugins/Surge XT.clap");
        assert!(project.set_hosted_instrument(track, "clap:org.surge-synth-team.surge-xt", file));

        let inner = project
            .track(track)
            .and_then(|track| track.kind.as_instrument())
            .expect("still an instrument track");
        assert_eq!(inner.instrument_id, "clap:org.surge-synth-team.surge-xt");
        assert!(inner.is_hosted());
        assert!(
            inner.instrument_state.params.is_empty(),
            "a setting belongs to the instrument that had it, not to the track"
        );
        assert!(
            project.automation.lane(instrument_lane).is_none(),
            "a parameter index belongs to the instrument that had it"
        );
        assert!(
            project.automation.lane(fader_lane).is_some(),
            "the track's own automation survives its instrument swap"
        );

        // A track that is not an instrument track has no instrument to point anywhere.
        let audio = project.add_audio_track("Sample");
        assert!(!project.set_hosted_instrument(audio, "clap:whatever", AssetPath::external("/x")));
    }

    #[test]
    fn a_built_in_instrument_names_no_file() {
        let mut project = Project::new("Built in", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.chiptune");
        let inner = project
            .track(track)
            .and_then(|track| track.kind.as_instrument())
            .expect("an instrument track");
        assert!(!inner.is_hosted(), "an id alone is enough to find it");
    }

    #[test]
    fn a_duplicated_track_gets_send_ids_of_its_own() {
        // Every id in a copy is reissued for the same reason a clip's is: two sends answering to
        // one id would send an edit aimed at the copy to the original.
        let (mut project, kick, _, bus) = bussed_project();
        let send = project.next_send_id();
        project
            .track_mut(kick)
            .unwrap()
            .sends
            .push(AuxSend::new(send, bus));

        let copy = project.duplicate_track(kick).unwrap();
        assert_ne!(project.track(copy).unwrap().sends[0].id, send);
        // The routing itself is copied as it was: the copy feeds the same bus.
        assert_eq!(project.track(copy).unwrap().output, Output::Bus(bus));
        assert_eq!(project.track(copy).unwrap().sends[0].target, bus);
    }

    #[test]
    fn a_duplicated_track_shares_no_ids_with_its_original() {
        let mut project = demo_project();
        let original = project.tracks[0].id;
        project.add_effect(Some(original), "auris.fx.gain").unwrap();

        let copy = project.duplicate_track(original).unwrap();
        assert_eq!(project.track_index(copy), Some(1), "the copy sits below");
        assert_ne!(copy, original);

        let before = project.track(original).unwrap();
        let after = project.track(copy).unwrap();
        assert_eq!(after.name, format!("{} copy", before.name));

        let ids = |track: &Track| -> Vec<u64> {
            let mut ids: Vec<u64> = track.mixer.effects.iter().map(|slot| slot.id.0).collect();
            if let Some(inner) = track.kind.as_instrument() {
                ids.extend(inner.clips.iter().map(|clip| clip.id.0));
            }
            ids
        };
        let original_ids = ids(before);
        assert!(!original_ids.is_empty(), "the fixture has ids to reissue");
        for id in ids(after) {
            assert!(
                !original_ids.contains(&id),
                "id {id} is shared with the original, so edits would hit the wrong object"
            );
        }
    }

    #[test]
    fn a_singer_track_holds_notes_without_being_an_instrument_track() {
        let mut project = Project::new("Song", 48_000.0);
        let track = project.add_singer_track("Melody", "auris.synth.voice");
        let kind = &project.track(track).unwrap().kind;

        // The distinction every call site leans on: the registry and the plugin editor ask
        // `is_instrument` and must not offer to swap a singer's preview voice; the clip
        // machinery asks `holds_notes` and must treat the two alike.
        assert!(kind.holds_notes() && kind.is_singer());
        assert!(!kind.is_instrument());
        assert_eq!(kind.label(), "Singer");

        let clip = project
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("a singer track accepts a note clip");
        project
            .midi_clip_mut(clip)
            .unwrap()
            .notes
            .push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
        assert_eq!(project.track_of_clip(clip), Some(track));
        assert!(project.remove_clip(clip));
    }

    #[test]
    fn a_melody_moves_between_an_instrument_track_and_a_singer_track() {
        // The reason `move_clip_to_track` asks `holds_notes` of both sides rather than
        // `is_instrument`: a sketched melody is dragged onto a singer track to be given words,
        // and its notes — words and all — are the same material either way.
        let mut project = Project::new("Song", 48_000.0);
        let sketch = project.add_instrument_track("Sketch", "auris.synth.chiptune");
        let singer = project.add_singer_track("Melody", "auris.synth.voice");
        let clip = project
            .add_midi_clip(sketch, "Line", Ticks::ZERO, Ticks::from_beats(2.0))
            .unwrap();
        {
            let midi = project.midi_clip_mut(clip).unwrap();
            let mut note = Note::new(67, Ticks::ZERO, Ticks::QUARTER);
            note.lyric = "ら".into();
            note.phonemes = vec!["ɾ".into(), "a".into()];
            midi.notes.push(note);
        }

        assert!(project.move_clip_to_track(clip, singer));
        assert_eq!(project.track_of_clip(clip), Some(singer));
        let note = &project.midi_clip(clip).unwrap().1.notes[0];
        assert_eq!(note.lyric, "ら");
        assert_eq!(note.phonemes, ["ɾ", "a"]);
        assert!(project.move_clip_to_track(clip, sketch), "and back again");

        // An audio track still refuses the move, exactly as before.
        let audio = project.add_audio_track("Take");
        assert!(!project.move_clip_to_track(clip, audio));
    }

    #[test]
    fn a_duplicated_singer_track_reissues_its_clip_ids() {
        let mut project = Project::new("Song", 48_000.0);
        let track = project.add_singer_track("Melody", "auris.synth.voice");
        let clip = project
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(1.0))
            .unwrap();
        let copy = project.duplicate_track(track).unwrap();
        let copied_clip = project.track(copy).unwrap().kind.note_clips().unwrap()[0].id;
        assert_ne!(
            copied_clip, clip,
            "two clips answering to one id would send edits to the wrong track"
        );
    }

    #[test]
    fn removing_a_track_drops_only_that_track() {
        let mut project = demo_project();
        let id = project.tracks[0].id;
        assert!(project.remove_track(id));
        assert!(project.tracks.is_empty());
        assert!(!project.remove_track(id));
    }
}
