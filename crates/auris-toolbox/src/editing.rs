//! Reading and revising the musical decisions in an existing document.

use super::*;

/// Range, density and repetition without an audio render.
pub mod analyze_music {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "analyze_music";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Measures each note clip's pitch range, note density, pitch-class count and exact bar-pattern repetition. Reads stored notes without rendering. These describe musical choices, not aesthetic quality; use analyze for loudness and audio input for listening.";
    /// The project to inspect.
    pub use crate::describe::Args;
    /// Returns measurements with the track and clip numbers used by editing tools.
    pub fn run(args: &Args) -> Result<String, String> {
        let session = opened(&args.project)?;
        let reports: Vec<_> = session.analyze_music().into_iter().map(|report| {
            serde_json::json!({"track": session.project().track(report.track).map(|track| &track.name),
                "clip": clip_number(session.project(), report.track, report.clip), "measurements": report})
        }).collect();
        serde_json::to_string_pretty(&reports).map_err(|error| error.to_string())
    }
}

/// The saved specification and the current musical state, read separately.
pub mod inspect_composition {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "inspect_composition";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Reads the original song specification and the current key, chords, tempo, meter, sections and clip recipes. The specification is provenance; later manual edits are represented by the current state, not by that original text.";
    /// The project to inspect.
    pub use crate::describe::Args;
    /// Reports musical decisions without rendering audio or changing the document.
    pub fn run(args: &Args) -> Result<String, String> {
        let session = opened(&args.project)?;
        let project = session.project();
        let tracks: Vec<_> = project.tracks.iter().map(|track| {
            let clips: Vec<_> = track.kind.note_clips().map(Vec::as_slice).unwrap_or_default().iter().enumerate().map(|(index, clip)| {
                serde_json::json!({"clip": index + 1, "id": clip.id.0, "name": clip.name,
                    "start_bar": project.signatures.bar_of(clip.start), "start_tick": clip.start.raw(),
                    "length_ticks": clip.length.raw(), "notes": clip.notes.len(),
                    "recipe": clip.recipe, "hand_edited": session.clip_hand_edited(clip.id)})
            }).collect();
            serde_json::json!({"track": track.name, "id": track.id.0, "clips": clips})
        }).collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "original_specification": project.song_spec,
            "grooves": groove_catalog().iter().map(|groove| groove.name).collect::<Vec<_>>(),
            "harmony": project.harmony, "tempo": project.tempo_map,
            "meter": project.signatures, "sections": project.sections, "tracks": tracks,
            "time_units": "Map positions are ticks; 960 ticks are one quarter note. Clip numbers are 1-based."
        })).map_err(|error| error.to_string())
    }
}

/// Harmony and timeline settings changed without replacing notes or the mix.
pub mod edit_harmony {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "edit_harmony";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Changes key, chord progression, tempo or section label at a bar in an existing project. Optional bars bounds the progression and restores the previous key and tempo at the end. Existing notes stay unchanged; explicitly call write_again on the parts that should follow the new harmony.";
    /// A bounded change to the musical timeline.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// First bar, 1-based.
        pub start_bar: u32,
        /// Length in bars, 1-4096. Required with chords or clear_chords. Interior key/tempo points stay in place.
        pub bars: Option<u32>,
        /// Key such as D minor or F# dorian.
        pub key: Option<String>,
        /// Roman-numeral chart such as | I | V | vi | IV |, or a built-in @name.
        pub chords: Option<String>,
        /// Tempo in beats per minute, 20-400.
        pub tempo: Option<f64>,
        /// Section label; an empty string makes the stretch unnamed.
        pub section: Option<String>,
        /// Remove chords across bars, instead of writing a progression.
        #[serde(default)]
        pub clear_chords: bool,
    }
    /// Validates the entire request before applying and saving it.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.start_bar == 0 {
            return Err("start_bar is 1-based".into());
        }
        if args.key.is_none()
            && args.chords.is_none()
            && args.tempo.is_none()
            && args.section.is_none()
            && !args.clear_chords
        {
            return Err("provide a key, chords, tempo, section or clear_chords".into());
        }
        if args.clear_chords && args.chords.is_some() {
            return Err("choose chords or clear_chords".into());
        }
        let key = args
            .key
            .as_deref()
            .map(|text| MusicalKey::parse(text).ok_or_else(|| format!("invalid key: {text}")))
            .transpose()?;
        if let Some(tempo) = args.tempo
            && (!tempo.is_finite() || !(20.0..=400.0).contains(&tempo))
        {
            return Err("tempo must be between 20 and 400 BPM".into());
        }
        let bars = args
            .bars
            .map(|bars| bounded_bars(bars, "harmony"))
            .transpose()?;
        if (args.chords.is_some() || args.clear_chords) && bars.is_none() {
            return Err("provide bars for a chord range".into());
        }
        let after = bars
            .map(|bars| bar_after(args.start_bar, bars))
            .transpose()?;
        let mut session = opened(&args.project)?;
        let start = session.project().signatures.bar_start(args.start_bar);
        let end = after.map(|bar| session.project().signatures.bar_start(bar));
        let chart = args
            .chords
            .as_deref()
            .map(|text| {
                let chart = Chart::parse(text).ok_or_else(|| {
                    format!(
                        "invalid chord chart: {text}; call list_progressions for built-in names"
                    )
                })?;
                Ok::<_, String>(
                    chart
                        .spelled_in(key.unwrap_or_else(|| session.project().harmony.key_at(start))),
                )
            })
            .transpose()?;
        let previous_key = end.map(|at| session.project().harmony.key_at(at));
        let previous_tempo = end.map(|at| session.project().tempo_map.bpm_at(at));
        if let Some(key) = key {
            session.set_key(start, key);
            if let (Some(end), Some(previous)) = (end, previous_key) {
                session.set_key(end, previous);
            }
        }
        if let Some(tempo) = args.tempo {
            session.set_tempo_point(start, tempo);
            if let (Some(end), Some(previous)) = (end, previous_tempo) {
                session.set_tempo_point(end, previous);
            }
        }
        if let Some(label) = &args.section {
            session.set_section(start, Some(label.clone()));
        }
        if let Some(end) = end {
            if args.clear_chords {
                session.clear_harmony(start, end);
            }
            if let Some(chart) = chart {
                for offset in 0..bars.unwrap() {
                    let at = session
                        .project()
                        .signatures
                        .bar_start(args.start_bar + offset);
                    let one_bar = Chart {
                        bars: vec![chart.bars[offset as usize % chart.bars.len()].clone()],
                        ..chart.clone()
                    };
                    session.stamp_progression(&one_bar, at, 1);
                }
            }
        }
        session
            .save_with_checkpoint()
            .map_err(|error| error.to_string())?;
        Ok("Saved the timeline changes. Notes and mix are preserved. Use write_again only on parts you want regenerated; inspect_composition reads the result.".into())
    }
}

/// Changes a generated clip's musical controls, preserving unspecified values.
pub mod edit_recipe {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "edit_recipe";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Changes a generated clip's recipe and regenerates that clip only. Unspecified controls and the seed are kept. Read inspect_composition first. Hand-edited notes require replace_hand_edits; a checkpoint preserves the previous document. Use edit_clip with freeze to keep a take without its recipe.";
    /// The controls to change on one generated clip.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Track name from describe.
        pub track: String,
        /// 1-based clip number.
        pub clip: usize,
        /// Density, 0-1.
        pub density: Option<f32>,
        /// Playing intensity, 0-1.
        pub intensity: Option<f32>,
        /// Gate length, 0-1.
        pub gate: Option<f32>,
        /// Dynamic variation, 0-1.
        pub dynamics: Option<f32>,
        /// Syncopation, 0-1.
        pub syncopation: Option<f32>,
        /// Drum fill, 0-1.
        pub fill: Option<f32>,
        /// Relative octave, -2 to 2.
        pub octave: Option<i32>,
        /// Swing percentage, 50-75.
        pub swing: Option<u8>,
        /// Drum groove name; inspect_composition lists the catalogue.
        pub groove: Option<String>,
        /// Beat subdivision: eighth, sixteenth, eighth-triplet or sixteenth-triplet.
        pub subdivision: Option<String>,
        /// Explicitly replace manual changes to this generated clip.
        #[serde(default)]
        pub replace_hand_edits: bool,
    }
    /// Validates each supplied control, regenerates and saves.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let (clip, _) = clip_by_number(session.project(), track, args.clip)?;
        let mut recipe = session
            .clip_recipe(clip)
            .cloned()
            .ok_or("this clip has no recipe")?;
        if session.clip_hand_edited(clip) && !args.replace_hand_edits {
            return Err("this clip has hand-edited notes; use replace_hand_edits: true to replace them, or edit_clip with freeze to keep them".into());
        }
        for (name, value, target) in [
            ("density", args.density, &mut recipe.density),
            ("intensity", args.intensity, &mut recipe.intensity),
            ("gate", args.gate, &mut recipe.gate),
            ("dynamics", args.dynamics, &mut recipe.dynamics),
            ("syncopation", args.syncopation, &mut recipe.syncopation),
            ("fill", args.fill, &mut recipe.fill),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(format!("{name} must be 0-1"));
                }
                *target = value;
            }
        }
        if let Some(value) = args.octave {
            if !(-2..=2).contains(&value) {
                return Err("octave must be -2 to 2".into());
            }
            recipe.octave = value;
        }
        if let Some(value) = args.swing {
            if !(50..=75).contains(&value) {
                return Err("swing must be 50-75".into());
            }
            recipe.swing = value;
        }
        if let Some(groove) = &args.groove {
            if !groove_catalog().iter().any(|entry| entry.name == groove) {
                return Err("unknown groove; read inspect_composition".into());
            }
            recipe.groove = groove.clone();
        }
        if let Some(subdivision) = &args.subdivision {
            recipe.subdivision = Subdivision::parse(subdivision).ok_or(
                "subdivision must be eighth, sixteenth, eighth-triplet or sixteenth-triplet",
            )?;
        }
        if session.clip_recipe(clip) == Some(&recipe) {
            return Ok("Recipe unchanged; no notes were rewritten.".into());
        }
        let notes = session
            .set_clip_recipe(clip, recipe)
            .map_err(|error| error.to_string())?;
        session
            .save_with_checkpoint()
            .map_err(|error| error.to_string())?;
        Ok(format!(
            "Saved clip [{}]: {notes} notes. Other clips and the mix are unchanged.",
            args.clip
        ))
    }
}

/// Arrangement edits to a single clip.
pub mod edit_clip {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "edit_clip";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Moves, duplicates, splits, resizes, removes, mutes or freezes one note clip. Addresses use describe's track name and 1-based clip number. Positions use absolute song bars and quarter-note beats. Resize can regenerate a generated clip; freeze first to preserve its written notes. A checkpoint is saved before the document changes on disk.";
    /// The requested clip operation.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum Action {
        /// Move to an absolute song position.
        Move {
            /// First bar, 1-based.
            bar: u32,
            /// Quarter-note beat, 1-based.
            beat: Option<f64>,
        },
        /// Duplicate at a given position, or immediately after the source if omitted.
        Duplicate {
            /// Destination bar, 1-based.
            bar: Option<u32>,
        },
        /// Split at an absolute song position inside the clip.
        Split {
            /// Bar, 1-based.
            bar: u32,
            /// Quarter-note beat, 1-based.
            beat: Option<f64>,
        },
        /// Change the end position of the clip.
        Resize {
            /// Exclusive end bar, 1-based.
            end_bar: u32,
            /// Explicitly replace hand-edited generated notes.
            #[serde(default)]
            replace_hand_edits: bool,
        },
        /// Delete the clip.
        Remove,
        /// Keep the notes and remove the generation recipe.
        Freeze,
        /// Set the clip's mute state.
        Mute {
            /// Whether the clip should be silent.
            muted: bool,
        },
    }
    /// One arrangement change.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Track name from describe.
        pub track: String,
        /// 1-based clip number.
        pub clip: usize,
        /// Operation and its arguments.
        pub action: Action,
    }
    /// Applies a single session command and saves the result.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let (clip, _) = clip_by_number(session.project(), track, args.clip)?;
        let result = match args.action {
            Action::Move { bar, beat } => {
                let at = placed_at(session.project(), bar, beat.unwrap_or(1.0))?;
                session.move_clip(clip, at)
            }
            Action::Duplicate { bar } => {
                let at = bar
                    .map(|bar| placed_at(session.project(), bar, 1.0))
                    .transpose()?;
                let copy = session
                    .duplicate_clip(clip)
                    .map_err(|error| error.to_string())?;
                if let Some(at) = at {
                    session
                        .move_clip(copy, at)
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }
            Action::Split { bar, beat } => {
                let at = placed_at(session.project(), bar, beat.unwrap_or(1.0))?;
                session.split_clip(clip, at).map(|_| ())
            }
            Action::Resize {
                end_bar,
                replace_hand_edits,
            } => {
                if session.clip_hand_edited(clip) && !replace_hand_edits {
                    return Err(
                        "freeze this clip or set replace_hand_edits before resizing it".into(),
                    );
                }
                let at = placed_at(session.project(), end_bar, 1.0)?;
                let start = session.clip_start(clip).ok_or("clip has no start")?;
                if at <= start {
                    return Err("end_bar must be after the clip start".into());
                }
                bounded_bars(
                    end_bar - session.project().signatures.bar_of(start) + 1,
                    "clip",
                )?;
                session.resize_clip(clip, at)
            }
            Action::Remove => session.remove_clip(clip),
            Action::Freeze => session.freeze_clip(clip),
            Action::Mute { muted } => session.set_clip_muted(clip, muted),
        };
        result.map_err(|error| error.to_string())?;
        session
            .save_with_checkpoint()
            .map_err(|error| error.to_string())?;
        Ok("Saved the clip edit. Call describe again: clip numbers may have changed.".into())
    }
}

/// Saved alternatives that survive a model restart.
pub mod checkpoints {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "checkpoints";
    /// The tool's model-facing description.
    pub const DESCRIPTION: &str = "Lists, creates or restores document checkpoints in the project folder. Editing tools automatically keep the previous document. Create a named checkpoint for an A/B comparison; restore brings its notes, harmony and mix back and saves, keeping the current version in another automatic checkpoint. Assets are referenced, not copied.";
    /// The checkpoint operation.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum Action {
        /// List names.
        List,
        /// Create a new named checkpoint.
        Create,
        /// Restore a named checkpoint and save.
        Restore,
    }
    /// A snapshot query or command.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// list, create or restore.
        pub action: Action,
        /// Required for create/restore; letters, digits, hyphens or underscores, up to 80 bytes.
        pub name: Option<String>,
    }
    /// Reads or changes a checkpoint using the session's document commands.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        match args.action {
            Action::List => Ok(session.checkpoints().map_err(|e| e.to_string())?.join("\n")),
            Action::Create => {
                let path = session
                    .create_checkpoint(args.name.as_deref().ok_or("provide name")?)
                    .map_err(|e| e.to_string())?;
                Ok(format!("Checkpoint saved: {}", path.display()))
            }
            Action::Restore => {
                let missing = session
                    .restore_checkpoint(args.name.as_deref().ok_or("provide name")?)
                    .map_err(|e| e.to_string())?;
                session.save_with_checkpoint().map_err(|e| e.to_string())?;
                Ok(format!(
                    "Checkpoint restored and saved. {} missing assets. Call describe before editing again.",
                    missing.len()
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    struct Fixture {
        root: PathBuf,
        path: String,
        track: TrackId,
        clip: ClipId,
    }
    impl Fixture {
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("auris-editing-{}-{label}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let track = session.add_default_instrument_track("Lead").unwrap();
            session.stamp_progression(
                &Chart::parse("| I | V | vi | IV |").unwrap(),
                Ticks::ZERO,
                4,
            );
            let clip = session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    ClipRecipe::new(ClipPreset::Lead, 7),
                )
                .unwrap();
            let path = root.join("Song.auris");
            session.save(&path).unwrap();
            Self {
                root,
                path: path.to_string_lossy().into_owned(),
                track,
                clip,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn local_harmony_and_recipe_edits_keep_the_rest_of_the_document_and_can_be_restored() {
        let fixture = Fixture::new("local");
        let original = opened(&fixture.path).unwrap().project().clone();
        checkpoints::run(&args(
            json!({"project": fixture.path, "action": "create", "name": "before"}),
        ))
        .unwrap();
        edit_harmony::run(&args(
            json!({"project": fixture.path, "start_bar": 2, "bars": 2,
            "key": "D minor", "tempo": 96, "chords": "| i | iv |"}),
        ))
        .unwrap();
        let revised = opened(&fixture.path).unwrap();
        assert_eq!(
            revised.project().tracks,
            original.tracks,
            "harmony edits never rewrite notes or mix"
        );
        let start = revised.project().signatures.bar_start(2);
        let end = revised.project().signatures.bar_start(4);
        assert_eq!(
            revised.project().harmony.key_at(start),
            MusicalKey::parse("D minor").unwrap()
        );
        assert_eq!(
            revised.project().harmony.key_at(end),
            original.harmony.key_at(end)
        );
        assert_eq!(revised.project().tempo_map.bpm_at(start), 96.0);
        assert_eq!(
            revised.project().tempo_map.bpm_at(end),
            original.tempo_map.bpm_at(end)
        );
        edit_recipe::run(&args(json!({"project": fixture.path, "track": "Lead", "clip": 1, "density": 0.2, "octave": 1}))).unwrap();
        let revised = opened(&fixture.path).unwrap();
        let recipe = revised.clip_recipe(fixture.clip).unwrap();
        assert_eq!(recipe.seed, 7);
        assert_eq!(recipe.density, 0.2);
        assert_eq!(recipe.octave, 1);
        assert_eq!(
            revised.project().track(fixture.track).unwrap().mixer,
            original.track(fixture.track).unwrap().mixer
        );
        let report: serde_json::Value = serde_json::from_str(
            &inspect_composition::run(&args(json!({"project":fixture.path}))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            report["tracks"][0]["clips"][0]["recipe"]["density"],
            json!(0.2_f32)
        );
        checkpoints::run(&args(
            json!({"project": fixture.path, "action": "restore", "name": "before"}),
        ))
        .unwrap();
        assert_eq!(opened(&fixture.path).unwrap().project(), &original);
    }

    #[test]
    fn harmony_range_follows_bar_lines_across_a_meter_change() {
        let fixture = Fixture::new("meter");
        let mut session = opened(&fixture.path).unwrap();
        session.set_signature_point(Ticks::QUARTER * 8, TimeSignature::new(3, 4));
        session.save_in_place().unwrap();
        let end = session.project().signatures.bar_start(4);
        let before = session.project().harmony.clone();
        edit_harmony::run(&args(json!({"project":fixture.path, "start_bar":2,
            "bars":2, "chords":"| ii | V |"})))
        .unwrap();
        let revised = opened(&fixture.path).unwrap();
        assert_eq!(
            revised.project().harmony.chord_at(end),
            before.chord_at(end)
        );
        assert_eq!(revised.project().tracks, session.project().tracks);
    }

    #[test]
    fn a_refused_edit_changes_neither_the_file_nor_its_manual_notes() {
        let fixture = Fixture::new("refusal");
        let before = std::fs::read(&fixture.path).unwrap();
        assert!(
            edit_harmony::run(&args(
                json!({"project": fixture.path, "start_bar": 2, "key": "D minor", "tempo": -1})
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
        let mut session = opened(&fixture.path).unwrap();
        session
            .add_note(fixture.clip, Note::new(99, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.save_in_place().unwrap();
        let before = std::fs::read(&fixture.path).unwrap();
        assert!(
            edit_recipe::run(&args(
                json!({"project": fixture.path, "track": "Lead", "clip": 1, "density": 0.2})
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
        edit_clip::run(&args(json!({"project": fixture.path, "track": "Lead", "clip": 1, "action": {"kind":"freeze"}}))).unwrap();
        let frozen = opened(&fixture.path).unwrap();
        assert!(frozen.clip_recipe(fixture.clip).is_none());
        assert!(
            frozen
                .midi_clip(fixture.clip)
                .unwrap()
                .notes
                .iter()
                .any(|note| note.pitch == 99)
        );
    }

    #[test]
    fn arrangement_edits_duplicate_split_move_and_remove_only_the_target_clip() {
        let fixture = Fixture::new("arrangement");
        let edit = |clip, action| {
            edit_clip::run(&args(
                json!({"project": fixture.path, "track": "Lead", "clip": clip, "action": action}),
            ))
            .unwrap()
        };
        edit(1, json!({"kind":"duplicate", "bar": 5}));
        edit(2, json!({"kind":"freeze"}));
        edit(2, json!({"kind":"split", "bar": 7}));
        edit(3, json!({"kind":"move", "bar": 9}));
        edit(2, json!({"kind":"remove"}));
        let session = opened(&fixture.path).unwrap();
        let clips = session
            .project()
            .track(fixture.track)
            .unwrap()
            .kind
            .note_clips()
            .unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].id, fixture.clip);
        assert_eq!(session.project().signatures.bar_of(clips[1].start), 9);
        assert!(clips[1].recipe.is_none());
    }
}
