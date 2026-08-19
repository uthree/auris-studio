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

/// What kind of material a track holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackKind {
    /// Notes played by a software instrument.
    Instrument(InstrumentTrack),
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
            TrackKind::Audio(_) => "Audio",
            TrackKind::Bus => "Bus",
        }
    }

    /// `true` when this track holds notes rather than audio.
    ///
    /// The two kinds are the one thing that decides whether a clip may move to a track, so the
    /// question is asked directly rather than through a pattern match at each call site.
    pub fn is_instrument(&self) -> bool {
        matches!(self, TrackKind::Instrument(_))
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
            TrackKind::Instrument(track) => track
                .clips
                .iter()
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
        true
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
            TrackKind::Instrument(inner) => {
                for clip in &mut inner.clips {
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
    use crate::project::fixtures::{bussed_project, demo_project};

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
    fn removing_a_track_drops_only_that_track() {
        let mut project = demo_project();
        let id = project.tracks[0].id;
        assert!(project.remove_track(id));
        assert!(project.tracks.is_empty());
        assert!(!project.remove_track(id));
    }
}
