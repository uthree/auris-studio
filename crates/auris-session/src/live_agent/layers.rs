//! Layers through the agent door: add one on a track, read or reshape it, detach it.
//!
//! The wire speaks quarter notes for the offset, never ticks, for the same reason notes are
//! placed by bar and beat: a model reasons about music, and a tick count is the editor's
//! resolution, not a duration anyone hears.

use crate::{Session, prelude::*};

/// One layer operation.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Add an instrument track that plays a melodic or singer track's notes, placed below it.
    Add {
        /// Stable ID of the track to follow; not a drum track and not a layer.
        source: u64,
        /// Optional track name; defaults to "<source> Layer".
        name: Option<String>,
    },
    /// Read the followed track and the shape.
    Read {
        /// Stable layer track ID.
        track: u64,
    },
    /// Change only the given shape fields; the others keep their current values.
    Shape {
        /// Stable layer track ID.
        track: u64,
        /// Fields to change.
        shape: ShapePatch,
    },
    /// Copy the followed notes, shaped, into the layer's own clips so it stops following.
    Detach {
        /// Stable layer track ID.
        track: u64,
    },
}

impl Operation {
    /// Whether the operation leaves the document as it was.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Read { .. })
    }
}

/// A partial [`LayerShape`]: every field left out keeps its current value.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShapePatch {
    /// Restore the identity shape first, then apply any other given fields.
    #[serde(default)]
    pub reset: bool,
    /// Lowest source MIDI key kept, inclusive, 0..127.
    #[schemars(range(min = 0, max = 127))]
    pub lowest: Option<u8>,
    /// Highest source MIDI key kept, inclusive, 0..127.
    #[schemars(range(min = 0, max = 127))]
    pub highest: Option<u8>,
    /// all, top (the highest sounding note at each onset) or bottom (the lowest).
    #[schemars(extend("enum"=["all","top","bottom"]))]
    pub voice: Option<String>,
    /// Semitones, -48..48, applied after the key range chose notes by source pitch.
    #[schemars(range(min = -48, max = 48))]
    pub transpose: Option<i32>,
    /// Velocity multiplier, 0..2.
    #[schemars(range(min = 0, max = 2))]
    pub velocity: Option<f32>,
    /// Length multiplier, 0.05..4.
    #[schemars(range(min = 0.05, max = 4))]
    pub length: Option<f32>,
    /// Quarter notes late (positive) or early (negative), -4..4.
    #[schemars(range(min = -4, max = 4))]
    pub offset_quarters: Option<f64>,
}

impl ShapePatch {
    /// `current` with this patch applied, or why the patch cannot be.
    fn apply(&self, current: &LayerShape) -> Result<LayerShape, String> {
        let mut shape = if self.reset {
            LayerShape::default()
        } else {
            current.clone()
        };
        if let Some(lowest) = self.lowest {
            shape.lowest = key(lowest, "lowest")?;
        }
        if let Some(highest) = self.highest {
            shape.highest = key(highest, "highest")?;
        }
        if shape.lowest > shape.highest {
            return Err(format!(
                "lowest {} is above highest {}; the range would keep nothing",
                shape.lowest, shape.highest
            ));
        }
        if let Some(voice) = &self.voice {
            shape.voice = match voice.as_str() {
                "all" => LayerVoice::All,
                "top" => LayerVoice::Top,
                "bottom" => LayerVoice::Bottom,
                other => return Err(format!("voice must be all, top or bottom, not {other}")),
            };
        }
        if let Some(transpose) = self.transpose {
            if !(-48..=48).contains(&transpose) {
                return Err("transpose must be -48..48 semitones".into());
            }
            shape.transpose = transpose;
        }
        if let Some(velocity) = self.velocity {
            shape.velocity = within(velocity, 0.0, 2.0, "velocity")?;
        }
        if let Some(length) = self.length {
            shape.length = within(length, 0.05, 4.0, "length")?;
        }
        if let Some(quarters) = self.offset_quarters {
            let limit = MAX_LAYER_OFFSET.raw() as f64 / Ticks::QUARTER.raw() as f64;
            if !quarters.is_finite() || quarters.abs() > limit {
                return Err(format!("offset_quarters must be -{limit}..{limit}"));
            }
            shape.offset = Ticks((quarters * Ticks::QUARTER.raw() as f64).round() as i64);
        }
        Ok(shape)
    }
}

fn key(value: u8, field: &str) -> Result<u8, String> {
    if value > 127 {
        return Err(format!("{field} must be a MIDI key 0..127"));
    }
    Ok(value)
}

fn within(value: f32, min: f32, max: f32, field: &str) -> Result<f32, String> {
    if !value.is_finite() || value < min || value > max {
        return Err(format!("{field} must be {min}..{max}"));
    }
    Ok(value)
}

/// A shape as the agent reads it: the same units the patch writes.
pub(super) fn shape_json(shape: &LayerShape) -> serde_json::Value {
    serde_json::json!({
        "lowest": shape.lowest,
        "highest": shape.highest,
        "voice": match shape.voice {
            LayerVoice::All => "all",
            LayerVoice::Top => "top",
            LayerVoice::Bottom => "bottom",
        },
        "transpose": shape.transpose,
        "velocity": shape.velocity,
        "length": shape.length,
        "offset_quarters": shape.offset.raw() as f64 / Ticks::QUARTER.raw() as f64,
    })
}

impl Session {
    pub(super) fn agent_layer(&mut self, operation: Operation) -> Result<String, String> {
        let error = |e: crate::SessionError| e.to_string();
        match operation {
            Operation::Add { source, name } => {
                let source = TrackId(source);
                let name = match name {
                    Some(name) if name.trim().is_empty() => {
                        return Err("A track needs a name".into());
                    }
                    Some(name) => name,
                    None => {
                        let source_name = self
                            .project()
                            .track(source)
                            .map(|track| track.name.clone())
                            .ok_or_else(|| format!("No track with ID {}", source.0))?;
                        format!("{source_name} Layer")
                    }
                };
                let track = self.add_default_layer_track(source, name).map_err(error)?;
                Ok(serde_json::json!({
                    "track": track.0,
                    "follows": source.0,
                    "unsaved": true,
                    "note": "The layer plays the source's notes with the default instrument; choose its sound with search_instruments and set_instrument.",
                })
                .to_string())
            }
            Operation::Read { track } => {
                let id = TrackId(track);
                let (source, shape) = self
                    .layer_source(id)
                    .zip(self.layer_shape(id))
                    .ok_or_else(|| format!("Track {track} is not a layer"))?;
                Ok(serde_json::json!({
                    "track": track,
                    "follows": source.0,
                    "source_exists": self.project().track(source).is_some(),
                    "shape": shape_json(shape),
                })
                .to_string())
            }
            Operation::Shape { track, shape } => {
                let id = TrackId(track);
                let current = self
                    .layer_shape(id)
                    .ok_or_else(|| format!("Track {track} is not a layer"))?;
                let next = shape.apply(current)?;
                self.set_layer_shape(id, next.clone()).map_err(error)?;
                Ok(serde_json::json!({"track": track, "shape": shape_json(&next), "unsaved": true})
                    .to_string())
            }
            Operation::Detach { track } => {
                self.detach_layer(TrackId(track)).map_err(error)?;
                Ok(serde_json::json!({
                    "track": track,
                    "detached": true,
                    "unsaved": true,
                    "note": "The track now owns copies of the notes it played; edits to the former source no longer reach it.",
                })
                .to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_agent::Command;

    fn session_with_piano() -> (Session, TrackId) {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let piano = session.add_default_instrument_track("Piano").unwrap();
        (session, piano)
    }

    fn run(
        session: &mut Session,
        operation: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let command: Command =
            serde_json::from_value(serde_json::json!({"action":"layer","operation":operation}))
                .map_err(|e| e.to_string())?;
        session
            .agent_command(command)
            .map(|reply| serde_json::from_str(&reply).unwrap())
    }

    #[test]
    fn an_agent_adds_shapes_reads_and_detaches_a_layer() {
        let (mut session, piano) = session_with_piano();
        let added = run(
            &mut session,
            serde_json::json!({"kind":"add","source":piano.0}),
        )
        .unwrap();
        let layer = added["track"].as_u64().unwrap();
        assert_eq!(added["follows"], piano.0);
        assert_eq!(
            session.project().track(TrackId(layer)).unwrap().name,
            "Piano Layer"
        );

        let shaped = run(
            &mut session,
            serde_json::json!({"kind":"shape","track":layer,"shape":{"transpose":-12,"voice":"bottom","offset_quarters":0.5}}),
        )
        .unwrap();
        assert_eq!(shaped["shape"]["transpose"], -12);
        assert_eq!(shaped["shape"]["offset_quarters"], 0.5);
        let shape = session.layer_shape(TrackId(layer)).unwrap();
        assert_eq!(shape.offset, Ticks(Ticks::QUARTER.raw() / 2));
        assert_eq!(shape.voice, LayerVoice::Bottom);

        // A second patch leaves the fields it does not name alone.
        run(
            &mut session,
            serde_json::json!({"kind":"shape","track":layer,"shape":{"velocity":0.5}}),
        )
        .unwrap();
        let read = run(
            &mut session,
            serde_json::json!({"kind":"read","track":layer}),
        )
        .unwrap();
        assert_eq!(read["follows"], piano.0);
        assert_eq!(read["shape"]["transpose"], -12);
        assert_eq!(read["shape"]["velocity"], 0.5);

        let reset = run(
            &mut session,
            serde_json::json!({"kind":"shape","track":layer,"shape":{"reset":true,"length":2.0}}),
        )
        .unwrap();
        assert_eq!(reset["shape"]["transpose"], 0);
        assert_eq!(reset["shape"]["length"], 2.0);

        run(
            &mut session,
            serde_json::json!({"kind":"detach","track":layer}),
        )
        .unwrap();
        assert_eq!(session.layer_source(TrackId(layer)), None);
    }

    #[test]
    fn bad_layer_requests_are_refused_without_an_edit() {
        let (mut session, piano) = session_with_piano();
        let drums = session.add_default_drum_track("Kit").unwrap();
        assert!(
            run(
                &mut session,
                serde_json::json!({"kind":"add","source":drums.0})
            )
            .is_err()
        );
        assert!(
            run(
                &mut session,
                serde_json::json!({"kind":"read","track":piano.0})
            )
            .is_err()
        );
        let layer = run(
            &mut session,
            serde_json::json!({"kind":"add","source":piano.0}),
        )
        .unwrap()["track"]
            .as_u64()
            .unwrap();
        let before = session.project().clone();
        for shape in [
            serde_json::json!({"lowest":80,"highest":40}),
            serde_json::json!({"transpose":60}),
            serde_json::json!({"velocity":3.0}),
            serde_json::json!({"offset_quarters":8.0}),
            serde_json::json!({"voice":"middle"}),
        ] {
            assert!(
                run(
                    &mut session,
                    serde_json::json!({"kind":"shape","track":layer,"shape":shape.clone()})
                )
                .is_err(),
                "{shape} should be refused"
            );
        }
        assert_eq!(session.project(), &before);
        assert!(
            run(
                &mut session,
                serde_json::json!({"kind":"add","source":layer})
            )
            .is_err()
        );
    }

    #[test]
    fn layer_policy_reads_freely_and_marks_writes_as_edits() {
        let read = crate::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":{"action":"layer","operation":{"kind":"read","track":1}}}),
        )
        .unwrap();
        assert_eq!(read.name, "edit_project.layer");
        assert!(!read.mutating && !read.confirm);
        let shape = crate::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":{"action":"layer","operation":{"kind":"shape","track":1,"shape":{"transpose":12}}}}),
        )
        .unwrap();
        assert!(shape.mutating && !shape.confirm);
    }
}
