//! Stable-ID arrangement and controller editing for every agent transport.

use crate::{Edit, Session, prelude::*};
use serde::{Deserialize, Serialize};

/// One operation on a MIDI or audio clip.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipOperation {
    /// Move a clip without altering its contents.
    Move {
        /// Absolute timeline tick.
        start_tick: i64,
        /// Optional destination track ID; omit to keep the track.
        track: Option<u64>,
    },
    /// Copy a clip, preserving the source, curves, performance and recipe.
    Copy {
        /// Absolute destination tick.
        start_tick: i64,
        /// Destination track ID.
        track: u64,
        /// Optional semitone shift; a nonzero shift freezes only the copy's recipe.
        transpose: Option<i32>,
    },
    /// Split at an absolute tick inside the clip, returning the right-hand clip ID.
    Split {
        /// Absolute timeline tick inside the clip.
        at_tick: i64,
    },
    /// Change only the clip's playback window; stored notes and recipes are preserved.
    Resize {
        /// Absolute exclusive end tick.
        end_tick: i64,
    },
    /// Repeat the clip's content out to an absolute end tick, the way a looped region does in a
    /// DAW. The stored notes or audio stay one pass; every repeat plays them again, and editing
    /// that pass changes every repeat. A partial last pass is cut off at end_tick.
    Loop {
        /// Absolute exclusive end of the repeats. Omit to loop up to the next clip on the track,
        /// or one extra pass when there is none. A value at or before the clip's own end turns
        /// looping off.
        end_tick: Option<i64>,
    },
    /// Stop the clip repeating; its content is unchanged.
    Unloop,
    /// Delete the clip.
    Remove,
    /// Remove a generation recipe while retaining the stored score.
    FreezeRecipe,
    /// Bake non-destructive performance into the stored score and curves.
    BakePerformance,
    /// Rename the clip.
    Rename {
        /// Nonempty intended name, at most 256 bytes.
        name: String,
    },
    /// Change the clip's mute switch.
    Mute {
        /// True silences the clip.
        muted: bool,
    },
    /// Change only supplied audio-clip settings. Fades are expressed in source frames.
    Audio {
        /// Gain, -60..24 dB.
        gain_db: Option<f32>,
        /// Optional recording tempo, in native BPM units.
        source_bpm: Option<f64>,
        /// Explicitly forget the recording tempo; do not combine with source_bpm.
        #[serde(default)]
        clear_source_bpm: bool,
        /// Whether the clip follows project tempo.
        follows_tempo: Option<bool>,
        /// Fade-in source frames; omitted preserves the existing value.
        fade_in_frames: Option<u64>,
        /// Fade-out source frames; omitted preserves the existing value.
        fade_out_frames: Option<u64>,
    },
    /// Crossfade this audio clip with an overlapping audio clip on the same track.
    Crossfade {
        /// Stable other audio clip ID.
        other: u64,
    },
}

impl ClipOperation {
    /// Whether ordinary agent mode requires confirmation for content removal or baking.
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            Self::Remove | Self::BakePerformance | Self::FreezeRecipe
        )
    }
}

/// A MIDI controller or pitch-bend lane, authored independently of performance stages.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurveTarget {
    /// Pitch bend, measured in semitones.
    Bend,
    /// Normalized MIDI controller, 0..1. Sustain is controller 64.
    Controller {
        /// MIDI controller number, 0..127.
        number: u8,
    },
}

impl CurveTarget {
    fn curve(&self) -> Result<auris_core::project::ClipCurve, String> {
        match self {
            Self::Bend => Ok(auris_core::project::ClipCurve::Bend),
            Self::Controller { number } if *number <= 127 => {
                Ok(auris_core::project::ClipCurve::Controller(*number))
            }
            _ => Err("Controller number must be 0..127".into()),
        }
    }
}

/// One exact authored curve point.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurveValue {
    /// Clip-relative nonnegative tick.
    pub at_tick: i64,
    /// Semitones for bend; normalized 0..1 for controllers.
    pub value: f32,
}

/// Read or atomically modify one authored clip curve.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurveOperation {
    /// Read up to 128 points.
    Read {
        /// Zero-based point offset.
        #[serde(default)]
        offset: usize,
    },
    /// Merge supplied positions, preserving other points.
    Merge {
        /// At most 4096 unique positions.
        points: Vec<CurveValue>,
    },
    /// Replace the lane's complete authored sequence.
    Replace {
        /// At most 4096 unique positions; [] clears the lane.
        points: Vec<CurveValue>,
    },
    /// Remove all authored points from this lane.
    Clear,
}

impl CurveOperation {
    /// Whether this operation only reads the document.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Read { .. })
    }
}

/// Positioned tempo and meter changes; existing note positions are preserved.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineOperation {
    /// Read both maps and their native limits.
    Read,
    /// Write a tempo change at an absolute tick.
    SetTempo {
        /// Absolute nonnegative timeline tick.
        at_tick: i64,
        /// Tempo within the native tempo-map bounds.
        bpm: f64,
    },
    /// Remove an explicit tempo point; the initial tempo is retained.
    RemoveTempo {
        /// Absolute nonnegative timeline tick.
        at_tick: i64,
    },
    /// Write a meter change at an absolute tick.
    SetMeter {
        /// Absolute nonnegative timeline tick.
        at_tick: i64,
        /// Beats per bar, 1..32.
        numerator: u32,
        /// Power-of-two beat denominator, 1..32.
        denominator: u32,
    },
    /// Remove an explicit meter point; the initial meter is retained.
    RemoveMeter {
        /// Absolute nonnegative timeline tick.
        at_tick: i64,
    },
}

impl TimelineOperation {
    /// Whether the operation only reads the document.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Read)
    }
}

impl Session {
    /// Read or edit positioned tempo and meter changes using exact absolute ticks.
    pub fn agent_edit_timeline(&mut self, operation: TimelineOperation) -> Result<String, String> {
        let valid_tick = |at| {
            if (0..=Ticks::QUARTER.raw() * 1_000_000).contains(&at) {
                Ok(Ticks(at))
            } else {
                Err("at_tick is outside the supported timeline".to_string())
            }
        };
        match operation {
            TimelineOperation::Read => {}
            TimelineOperation::SetTempo { at_tick, bpm } => {
                if !(TempoMap::MIN_BPM..=TempoMap::MAX_BPM).contains(&bpm) {
                    return Err("bpm outside native tempo-map limits".into());
                }
                self.set_tempo_point(valid_tick(at_tick)?, bpm);
            }
            TimelineOperation::RemoveTempo { at_tick } => {
                self.remove_tempo_point(valid_tick(at_tick)?)
            }
            TimelineOperation::SetMeter {
                at_tick,
                numerator,
                denominator,
            } => {
                if !(1..=32).contains(&numerator)
                    || !(1..=32).contains(&denominator)
                    || !denominator.is_power_of_two()
                {
                    return Err(
                        "Meter requires numerator 1..32 and power-of-two denominator 1..32".into(),
                    );
                }
                self.set_signature_point(
                    valid_tick(at_tick)?,
                    TimeSignature::new(numerator, denominator),
                );
            }
            TimelineOperation::RemoveMeter { at_tick } => {
                self.remove_signature_point(valid_tick(at_tick)?)
            }
        }
        Ok(serde_json::json!({"tempo_map":self.project().tempo_map,"signatures":self.project().signatures,"min_bpm":TempoMap::MIN_BPM,"max_bpm":TempoMap::MAX_BPM,"revision":self.agent_document_revision()?}).to_string())
    }
    /// Apply an arrangement operation as one atomic Undo step.
    pub fn agent_edit_clip(
        &mut self,
        clip: u64,
        operation: ClipOperation,
    ) -> Result<String, String> {
        let clip = ClipId(clip);
        let owner = self.track_of_clip(clip).ok_or("Unknown clip ID")?;
        let tick = |value| {
            if (0..=Ticks::QUARTER.raw() * 1_000_000).contains(&value) {
                Ok(Ticks(value))
            } else {
                Err("Tick must be within 0..1000000 quarter notes".to_string())
            }
        };
        self.begin_transaction(Edit::ExternalChanges);
        let applied = (|| -> Result<serde_json::Value, String> {
            let error = |e: crate::SessionError| e.to_string();
            let mut result_clip = clip;
            match operation {
                ClipOperation::Move { start_tick, track } => {
                    let at = tick(start_tick)?;
                    if let Some(track) = track {
                        if !self.clip_fits_track(clip, TrackId(track)) {
                            return Err("Clip does not fit destination track".into());
                        }
                        if TrackId(track) != owner {
                            self.move_clips_to_track(&[(clip, TrackId(track))])
                                .map_err(error)?;
                        }
                    }
                    self.move_clip(clip, at).map_err(error)?;
                }
                ClipOperation::Copy {
                    start_tick,
                    track,
                    transpose,
                } => {
                    let at = tick(start_tick)?;
                    let target = TrackId(track);
                    if !self.clip_fits_track(clip, target) {
                        return Err("Clip does not fit destination track".into());
                    }
                    let transpose = transpose.unwrap_or(0);
                    if !(-127..=127).contains(&transpose) {
                        return Err("transpose must be -127..127".into());
                    }
                    if transpose != 0
                        && self.project().midi_clip(clip).is_none_or(|(_, source)| {
                            source.notes.iter().any(|note| {
                                !(0..=127).contains(&(i32::from(note.pitch) + transpose))
                            })
                        })
                    {
                        return Err("Transposition requires MIDI notes within 0..127".into());
                    }
                    result_clip = self.duplicate_clip(clip).map_err(error)?;
                    self.move_clips_to_track(&[(result_clip, target)])
                        .map_err(error)?;
                    self.move_clip(result_clip, at).map_err(error)?;
                    if transpose != 0 {
                        let count = self
                            .project()
                            .midi_clip(result_clip)
                            .ok_or("Unknown copied clip")?
                            .1
                            .notes
                            .len();
                        self.transpose_notes(
                            result_clip,
                            &(0..count).collect::<Vec<_>>(),
                            transpose,
                        )
                        .map_err(error)?;
                        if self.clip_recipe(result_clip).is_some() {
                            self.freeze_clip(result_clip).map_err(error)?;
                        }
                    }
                }
                ClipOperation::Split { at_tick } => {
                    result_clip = self.split_clip(clip, tick(at_tick)?).map_err(error)?;
                }
                ClipOperation::Resize { end_tick } => {
                    self.resize_clip_window(clip, tick(end_tick)?)
                        .map_err(error)?;
                }
                ClipOperation::Loop { end_tick } => match end_tick {
                    Some(end_tick) => {
                        let start = self.clip_start(clip).ok_or("Unknown clip ID")?;
                        let end = tick(end_tick)?;
                        self.set_clip_loop(clip, Ticks((end - start).raw().max(0)))
                            .map_err(error)?;
                    }
                    None => {
                        if !self.clip_is_looped(clip) {
                            self.toggle_clip_loop(clip).map_err(error)?;
                        }
                    }
                },
                ClipOperation::Unloop => {
                    self.set_clip_loop(clip, Ticks::ZERO).map_err(error)?;
                }
                ClipOperation::Remove => {
                    self.remove_clip(clip).map_err(error)?;
                }
                ClipOperation::FreezeRecipe => {
                    if self.clip_recipe(clip).is_some() {
                        self.freeze_clip(clip).map_err(error)?;
                    }
                }
                ClipOperation::BakePerformance => {
                    self.freeze_clip_transforms(clip).map_err(error)?;
                }
                ClipOperation::Rename { name } => {
                    if name.trim().is_empty() || name.len() > 256 {
                        return Err("name must contain 1..256 bytes".into());
                    }
                    self.rename_clip(clip, name).map_err(error)?;
                }
                ClipOperation::Mute { muted } => {
                    self.set_clip_muted(clip, muted).map_err(error)?;
                }
                ClipOperation::Audio {
                    gain_db,
                    source_bpm,
                    clear_source_bpm,
                    follows_tempo,
                    fade_in_frames,
                    fade_out_frames,
                } => {
                    let audio = self
                        .project()
                        .audio_clip(clip)
                        .ok_or("Expected audio clip")?
                        .clone();
                    if gain_db.is_some_and(|v| !(-60.0..=24.0).contains(&v))
                        || source_bpm
                            .is_some_and(|v| !(TempoMap::MIN_BPM..=TempoMap::MAX_BPM).contains(&v))
                    {
                        return Err("Audio gain or tempo outside native range".into());
                    }
                    if clear_source_bpm && source_bpm.is_some() {
                        return Err("Supply source_bpm or clear_source_bpm, not both".into());
                    }
                    let fade_in = fade_in_frames.unwrap_or(audio.fade_in_frames);
                    let fade_out = fade_out_frames.unwrap_or(audio.fade_out_frames);
                    if fade_in
                        .checked_add(fade_out)
                        .is_none_or(|v| v > audio.length_frames)
                    {
                        return Err("Fades must fit the source frame length".into());
                    }
                    if let Some(value) = gain_db {
                        self.set_clip_gain(clip, value).map_err(error)?;
                    }
                    if clear_source_bpm || source_bpm.is_some() {
                        self.set_clip_source_bpm(clip, source_bpm).map_err(error)?;
                    }
                    if let Some(value) = follows_tempo {
                        self.set_clip_follows_tempo(clip, value).map_err(error)?;
                    }
                    if fade_in_frames.is_some() || fade_out_frames.is_some() {
                        self.set_clip_fades(clip, fade_in, fade_out)
                            .map_err(error)?;
                    }
                }
                ClipOperation::Crossfade { other } => {
                    self.crossfade_clips(clip, ClipId(other)).map_err(error)?;
                }
            }
            let mut result = serde_json::json!({"clip":result_clip.0});
            if let (Some(start), Some(content), Some(sounding)) = (
                self.clip_start(result_clip),
                self.clip_content_length(result_clip),
                self.clip_sounding_length(result_clip),
            ) && sounding > content
            {
                result["loop_end_tick"] = (start + sounding).raw().into();
            }
            Ok(result)
        })();
        match applied {
            Ok(value) => {
                self.end_transaction();
                Ok(value.to_string())
            }
            Err(error) => {
                self.revert_transaction();
                Err(error)
            }
        }
    }

    /// Read or edit only one authored controller lane, preserving derived performance.
    pub fn agent_clip_curve(
        &mut self,
        clip: u64,
        target: CurveTarget,
        operation: CurveOperation,
        expected_revision: Option<String>,
    ) -> Result<String, String> {
        let clip = ClipId(clip);
        let curve = target.curve()?;
        let (_, midi) = self
            .project()
            .midi_clip(clip)
            .ok_or("Unknown MIDI clip ID")?;
        if let CurveOperation::Read { offset } = operation {
            let all = midi.curve(curve);
            if offset > all.len() {
                return Err("Offset exceeds point count".into());
            }
            let points: Vec<_> = all
                .iter()
                .skip(offset)
                .take(128)
                .map(|point| serde_json::json!({"at_tick":point.at.raw(),"value":point.value}))
                .collect();
            let next = offset + points.len();
            return Ok(serde_json::json!({"clip":clip.0,"revision":self.agent_document_revision()?,"points":points,"total":all.len(),"next_offset":(next<all.len()).then_some(next),"range":curve.range()}).to_string());
        }
        if expected_revision.as_deref() != Some(self.agent_document_revision()?.as_str()) {
            return Err(
                "Document changed or revision missing; read the lane before editing".into(),
            );
        }
        let replace = matches!(
            operation,
            CurveOperation::Replace { .. } | CurveOperation::Clear
        );
        let points = match operation {
            CurveOperation::Merge { points } | CurveOperation::Replace { points } => points,
            _ => Vec::new(),
        };
        if points.len() > 4096 {
            return Err("At most 4096 points".into());
        }
        let (low, high) = curve.range();
        let mut seen = std::collections::HashSet::new();
        for point in &points {
            if point.at_tick < 0
                || point.at_tick > midi.length.raw()
                || !(low..=high).contains(&point.value)
                || !seen.insert(point.at_tick)
            {
                return Err("Points need unique ticks inside the clip and finite values in the lane's native range".into());
            }
        }
        self.begin_transaction(Edit::ExternalChanges);
        if replace {
            self.clear_curve(clip, curve);
        }
        for point in points {
            self.set_curve_point(clip, curve, Ticks(point.at_tick), point.value);
        }
        self.end_transaction();
        Ok(
            serde_json::json!({"clip":clip.0,"revision":self.agent_document_revision()?})
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Session, TrackId, ClipId) {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks(0), Ticks(3840))
            .unwrap();
        session
            .replace_notes(
                clip,
                vec![
                    Note::new(60, Ticks(13), Ticks(997)),
                    Note::new(64, Ticks(1920), Ticks(960)),
                ],
            )
            .unwrap();
        (session, track, clip)
    }

    #[test]
    fn clip_copy_preserves_source_and_undoes_every_destination_edit_together() {
        let (mut session, track, clip) = fixture();
        let before = session.project().clone();
        let result: serde_json::Value = serde_json::from_str(
            &session
                .agent_edit_clip(
                    clip.0,
                    ClipOperation::Copy {
                        start_tick: 7777,
                        track: track.0,
                        transpose: Some(12),
                    },
                )
                .unwrap(),
        )
        .unwrap();
        let copied = ClipId(result["clip"].as_u64().unwrap());
        assert_eq!(session.project().midi_clip(clip), before.midi_clip(clip));
        let copy = session.project().midi_clip(copied).unwrap().1;
        assert_eq!(copy.start.raw(), 7777);
        assert_eq!(
            copy.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![72, 76]
        );
        session.undo();
        assert_eq!(session.project(), &before);
        assert!(
            session
                .agent_edit_clip(
                    clip.0,
                    ClipOperation::Copy {
                        start_tick: 99,
                        track: u64::MAX,
                        transpose: None
                    }
                )
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn a_loop_repeats_one_pass_out_to_the_tick_asked_for_and_is_one_undo_step() {
        let (mut session, track, clip) = fixture();
        let before = session.project().clone();
        // Two and a half passes of a one-bar clip: the last one is cut off, not squeezed.
        let reply: serde_json::Value = serde_json::from_str(
            &session
                .agent_edit_clip(
                    clip.0,
                    ClipOperation::Loop {
                        end_tick: Some(9600),
                    },
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reply["loop_end_tick"], 9600);
        let looped = session.project().midi_clip(clip).unwrap().1;
        assert_eq!(looped.length.raw(), 3840, "the content stays one pass");
        assert_eq!(looped.notes, before.midi_clip(clip).unwrap().1.notes);
        assert_eq!(looped.sounding_length().raw(), 9600);

        // inspect_project says where the repeats stop, so a model can read the loop back.
        let inspected: serde_json::Value = serde_json::from_str(
            &session
                .agent_command(super::super::Command::Inspect {})
                .unwrap(),
        )
        .unwrap();
        let summary = inspected["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["track"] == track.0)
            .unwrap()["clips"][0]
            .clone();
        assert_eq!(summary["loop_end_tick"], 9600);
        assert_eq!(summary["length_ticks"], 3840);

        session.undo();
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn a_loop_without_an_end_fills_to_the_next_clip_and_unloop_stops_it() {
        let (mut session, track, clip) = fixture();
        session
            .add_midi_clip(track, "Next", Ticks(3840 * 4), Ticks(3840))
            .unwrap();
        session
            .agent_edit_clip(clip.0, ClipOperation::Loop { end_tick: None })
            .unwrap();
        assert_eq!(session.clip_sounding_length(clip), Some(Ticks(3840 * 4)));
        // Asking again for a clip that already repeats keeps the length it has rather than
        // toggling it off.
        session
            .agent_edit_clip(clip.0, ClipOperation::Loop { end_tick: None })
            .unwrap();
        assert!(session.clip_is_looped(clip));

        let reply: serde_json::Value = serde_json::from_str(
            &session
                .agent_edit_clip(clip.0, ClipOperation::Unloop)
                .unwrap(),
        )
        .unwrap();
        assert!(reply.get("loop_end_tick").is_none());
        assert!(!session.clip_is_looped(clip));
        // An end at or before the clip's own end is the same as turning it off.
        session
            .agent_edit_clip(
                clip.0,
                ClipOperation::Loop {
                    end_tick: Some(7680),
                },
            )
            .unwrap();
        session
            .agent_edit_clip(
                clip.0,
                ClipOperation::Loop {
                    end_tick: Some(2000),
                },
            )
            .unwrap();
        assert!(!session.clip_is_looped(clip));
    }

    #[test]
    fn the_loop_operation_parses_from_the_wire_shape() {
        let parsed: ClipOperation =
            serde_json::from_value(serde_json::json!({"kind":"loop","end_tick":7680})).unwrap();
        assert!(matches!(
            parsed,
            ClipOperation::Loop {
                end_tick: Some(7680)
            }
        ));
        let parsed: ClipOperation =
            serde_json::from_value(serde_json::json!({"kind":"loop"})).unwrap();
        assert!(matches!(parsed, ClipOperation::Loop { end_tick: None }));
        let parsed: ClipOperation =
            serde_json::from_value(serde_json::json!({"kind":"unloop"})).unwrap();
        assert!(matches!(parsed, ClipOperation::Unloop));
    }

    #[test]
    fn resizing_a_generated_window_does_not_rewrite_its_notes_or_recipe() {
        let (mut session, track, _) = fixture();
        session
            .agent_command(super::super::Command::EditHarmony {
                start_bar: 1,
                bars: 4,
                key: Some("C major".into()),
                chords: Some("| I | IV | V | I |".into()),
                section: None,
                clear_chords: false,
            })
            .unwrap();
        let generated: serde_json::Value = serde_json::from_str(
            &session
                .agent_command(super::super::Command::GeneratePhrase {
                    track: track.0,
                    part: "lead".into(),
                    start_bar: 1,
                    bars: 4,
                    seed: 42,
                })
                .unwrap(),
        )
        .unwrap();
        let clip = ClipId(generated["clip"].as_u64().unwrap());
        let before = session.project().midi_clip(clip).unwrap().1.clone();
        assert!(before.recipe.is_some());
        assert!(!before.notes.is_empty());
        session
            .agent_edit_clip(clip.0, ClipOperation::Resize { end_tick: 1000 })
            .unwrap();
        let after = session.project().midi_clip(clip).unwrap().1;
        assert_eq!(after.notes, before.notes);
        assert_eq!(after.recipe, before.recipe);
        assert_eq!(after.length.raw(), 1000);
        session.undo();
        assert_eq!(session.project().midi_clip(clip).unwrap().1, &before);
    }

    #[test]
    fn sustain_points_are_atomic_revision_checked_and_preserve_bend_and_notes() {
        let (mut session, _, clip) = fixture();
        session.set_curve_point(clip, auris_core::project::ClipCurve::Bend, Ticks(13), 0.25);
        let before = session.project().clone();
        let revision = session.agent_document_revision().unwrap();
        session
            .agent_clip_curve(
                clip.0,
                CurveTarget::Controller { number: 64 },
                CurveOperation::Replace {
                    points: vec![
                        CurveValue {
                            at_tick: 13,
                            value: 1.0,
                        },
                        CurveValue {
                            at_tick: 1999,
                            value: 0.0,
                        },
                    ],
                },
                Some(revision.clone()),
            )
            .unwrap();
        let current = session.project().midi_clip(clip).unwrap().1;
        assert_eq!(current.notes, before.midi_clip(clip).unwrap().1.notes);
        assert_eq!(current.bend, before.midi_clip(clip).unwrap().1.bend);
        let after = session.project().clone();
        assert!(
            session
                .agent_clip_curve(
                    clip.0,
                    CurveTarget::Controller { number: 64 },
                    CurveOperation::Clear,
                    Some(revision)
                )
                .is_err()
        );
        assert_eq!(session.project(), &after);
        session.undo();
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn mixed_meter_and_tempo_edits_leave_notes_in_exact_tick_positions() {
        let (mut session, _, clip) = fixture();
        let notes = session.project().midi_clip(clip).unwrap().1.notes.clone();
        session
            .agent_edit_timeline(TimelineOperation::SetMeter {
                at_tick: 1920,
                numerator: 6,
                denominator: 8,
            })
            .unwrap();
        session
            .agent_edit_timeline(TimelineOperation::SetTempo {
                at_tick: 997,
                bpm: 351.0,
            })
            .unwrap();
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes, notes);
        let before = session.project().clone();
        assert!(
            session
                .agent_edit_timeline(TimelineOperation::SetMeter {
                    at_tick: 0,
                    numerator: 6,
                    denominator: 7
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }
}
