//! New documents and media interchange through the session's file commands.

use super::*;

fn absolute_path<'a>(text: &'a str, field: &str) -> Result<&'a Path, String> {
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err(format!("{field} must be an absolute path"));
    }
    Ok(path)
}

fn extension(path: &Path, allowed: &[&str], field: &str) -> Result<(), String> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            allowed
                .iter()
                .any(|allowed| value.eq_ignore_ascii_case(allowed))
        })
    {
        return Ok(());
    }
    Err(format!(
        "{field} requires a .{} extension",
        allowed.join(" or .")
    ))
}

fn project_destination(output: &str) -> Result<&Path, String> {
    let path = absolute_path(output, "output")?;
    extension(path, &["auris"], "output")?;
    if path.exists() {
        return Err("output already exists; choose a new .auris path".into());
    }
    Ok(path)
}

fn save_new(session: &mut Session, output: &Path) -> Result<PathBuf, String> {
    match session.save_as(output) {
        Ok(saved) => Ok(saved.document),
        Err(SessionError::WouldReplace(path)) => Err(format!(
            "a project already exists at {}; choose a new output path",
            path.display()
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// Creates a document for manual note entry or audio import.
pub mod create_project {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "create_project";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Creates an empty project with one default instrument track and no clips. Optional tempo and meter set its clock. Output must be a new absolute .auris path; choosing Song.auris writes Song/Song.auris. Returns the actual project path to use in later calls. Use add_clip and edit_notes for manual notes, import_audio for recordings, or add_track for more parts.";

    /// The new document's destination and optional clock.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// New absolute .auris path. Existing projects are never replaced.
        pub output: String,
        /// Initial tempo in BPM, 20-400. Defaults to the application's new-project tempo.
        pub tempo: Option<f64>,
        /// Meter such as 4/4, 3/4 or 6/8. Defaults to 4/4.
        pub meter: Option<String>,
    }

    /// Creates and saves an empty document without invoking the composer.
    pub fn run(args: &Args) -> Result<String, String> {
        let output = project_destination(&args.output)?;
        if let Some(tempo) = args.tempo
            && !(20.0..=400.0).contains(&tempo)
        {
            return Err("tempo must be between 20 and 400 BPM".into());
        }
        let meter = args
            .meter
            .as_deref()
            .map(str::parse::<TimeSignature>)
            .transpose()
            .map_err(|e| e.to_string())?;
        let mut session = headless()?;
        session.new_project();
        if let Some(tempo) = args.tempo {
            session.set_tempo_at(Ticks::ZERO, tempo);
        }
        if let Some(meter) = meter {
            session.set_signature_at(Ticks::ZERO, meter);
        }
        let written = save_new(&mut session, output)?;
        let tracks: Vec<_> = session
            .project()
            .tracks
            .iter()
            .map(|track| serde_json::json!({"track":format!("id:{}",track.id.0),"name":track.name}))
            .collect();
        Ok(
            serde_json::json!({"project":written,"tempo":session.project().bpm(),
            "meter":session.signature_at(Ticks::ZERO).to_string(),"tracks":tracks,"clips":0})
            .to_string(),
        )
    }
}

/// Imports a recording or sample into a saved document.
pub mod import_audio {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "import_audio";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Imports an audio file into an existing project as a new audio track, starting at start_bar (1-based, default 1). Source must be an absolute path. The session copies the audio into the project Audio folder when possible; the result reports whether it was copied or remains external. Saves with a checkpoint and returns the actual track ID, clip ID and duration.";

    /// Audio to place at the start of a song bar.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Absolute path to the audio file to import.
        pub source: String,
        /// First bar, 1-4096. Defaults to 1. A new audio track is created for this file.
        pub start_bar: Option<u32>,
    }

    /// Imports the audio through the session and reports its saved asset reference.
    pub fn run(args: &Args) -> Result<String, String> {
        let source = absolute_path(&args.source, "source")?;
        let start_bar = args.start_bar.unwrap_or(1);
        bounded_bars(start_bar, "audio start position")?;
        let mut session = opened(&args.project)?;
        let start = session.project().signatures.bar_start(start_bar);
        let clip_id = session
            .import_audio(source, start)
            .map_err(|e| e.to_string())?;
        session.save_with_checkpoint().map_err(|e| e.to_string())?;
        let track_id = session
            .track_of_clip(clip_id)
            .ok_or("imported clip has no track")?;
        let clip = session
            .project()
            .audio_clip(clip_id)
            .ok_or("imported audio clip is missing")?;
        let asset = session
            .project()
            .audio_sources
            .get(&clip.source)
            .ok_or("imported audio source is missing")?;
        let copied = asset.path.is_inside();
        Ok(serde_json::json!({
            "project":session.path(),"track":format!("id:{}",track_id.0),
            "name":session.project().track(track_id).map(|track|&track.name),
            "clip_id":clip_id.0,"start_bar":start_bar,
            "seconds":asset.frame_count as f64 / asset.sample_rate,
            "copied_into_project":copied,"asset":asset.path.resolve(session.project_folder()),
            "warning":(!copied).then_some("The audio could not be copied into the project; the document still references the external source file.")
        }).to_string())
    }
}

/// Opens a Standard MIDI File as a new project.
pub mod import_midi {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "import_midi";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Imports a .mid or .midi file into a new project, preserving its note timing, tempo and meter. Supply absolute source and new .auris output paths. The MIDI becomes a separate document, with built-in instruments that can be changed using set_instrument. Returns the actual saved project path, track count and note count. Existing projects are never replaced.";

    /// A MIDI source and a new project destination.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute path to a .mid or .midi source file.
        pub source: String,
        /// New absolute .auris path. Choosing Song.auris writes Song/Song.auris.
        pub output: String,
    }

    /// Imports MIDI into a fresh session, then saves a new project folder.
    pub fn run(args: &Args) -> Result<String, String> {
        let source = absolute_path(&args.source, "source")?;
        extension(source, &["mid", "midi"], "source")?;
        let output = project_destination(&args.output)?;
        let mut session = headless()?;
        let report = session.import_midi(source).map_err(|e| e.to_string())?;
        let written = save_new(&mut session, output)?;
        Ok(
            serde_json::json!({"project":written,"tracks":report.tracks,"notes":report.notes,
            "tempo":session.project().bpm(),"meter":session.signature_at(Ticks::ZERO).to_string()})
            .to_string(),
        )
    }
}

/// Exports the document's instrumental score as a Standard MIDI File.
pub mod export_midi {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "export_midi";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Exports instrument-track notes, tempo, meter, pitch bends and MIDI controllers to a new .mid or .midi file. Supply absolute project and output paths. Existing files are never overwritten and the project is unchanged. MIDI does not preserve audio, singer tracks, instruments or the mix; use render for an audio export.";

    /// A project and its new MIDI export path.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Absolute .mid or .midi path whose parent directory already exists; must not exist.
        pub output: String,
    }

    /// Writes a complete temporary MIDI file and installs it without replacing an existing file.
    pub fn run(args: &Args) -> Result<String, String> {
        let output = absolute_path(&args.output, "output")?;
        extension(output, &["mid", "midi"], "output")?;
        if output.exists() {
            return Err("output already exists; choose a new .mid or .midi path".into());
        }
        let parent = output
            .parent()
            .ok_or("output must have a parent directory")?;
        let session = opened(&args.project)?;
        let temporary = tempfile::Builder::new()
            .prefix(".auris-midi-")
            .tempfile_in(parent)
            .map_err(|e| {
                format!(
                    "could not create a MIDI export in {}: {e}",
                    parent.display()
                )
            })?;
        let notes = session
            .export_midi(temporary.path())
            .map_err(|e| e.to_string())?;
        temporary.persist_noclobber(output).map_err(|e| {
            format!(
                "could not save MIDI to {} without replacing a file: {e}",
                output.display()
            )
        })?;
        Ok(serde_json::json!({"output":output,"notes":notes}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn request<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    fn created(root: &Path, name: &str) -> String {
        let answer = create_project::run(&request(
            json!({"output":root.join(format!("{name}.auris")),"tempo":90,"meter":"3/4"}),
        ))
        .unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        answer["project"].as_str().unwrap().to_string()
    }

    #[test]
    fn an_empty_project_supports_manual_notes_and_refuses_replacement() {
        let root = tempfile::tempdir().unwrap();
        let project = created(root.path(), "Manual");
        let session = opened(&project).unwrap();
        assert_eq!(session.project().tracks.len(), 1);
        assert_eq!(session.project().bpm(), 90.0);
        assert_eq!(session.signature_at(Ticks::ZERO), TimeSignature::new(3, 4));
        assert!(
            session.project().tracks[0]
                .kind
                .note_clips()
                .unwrap()
                .is_empty()
        );
        let track = format!("id:{}", session.project().tracks[0].id.0);
        add_clip::run(&request(
            json!({"project":project,"track":track,"name":"Melody","start_bar":1,"bars":2}),
        ))
        .unwrap();
        edit_notes::run(&request(json!({"project":project,"track":track,"clip":1,"add":[{"pitch":"C4","bar":1,"beat":1,"beats":1}]}))).unwrap();
        let reopened = opened(&project).unwrap();
        assert_eq!(
            reopened.project().tracks[0].kind.note_clips().unwrap()[0].notes[0].pitch,
            60
        );
        let before = std::fs::read(&project).unwrap();
        let result =
            create_project::run(&request(json!({"output":root.path().join("Manual.auris")})));
        assert!(result.unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read(&project).unwrap(), before);
    }

    #[test]
    fn midi_export_import_preserves_note_timing_tempo_and_meter() {
        let root = tempfile::tempdir().unwrap();
        let project = created(root.path(), "Score");
        let mut session = opened(&project).unwrap();
        let track = session.project().tracks[0].id;
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::QUARTER * 3, Ticks::QUARTER * 3)
            .unwrap();
        session
            .add_note(clip, Note::new(64, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.set_tempo_point(Ticks::QUARTER * 3, 110.0);
        session.save_in_place().unwrap();
        let before = std::fs::read(&project).unwrap();
        let midi = root.path().join("Score.mid");
        let answer = export_midi::run(&request(json!({"project":project,"output":midi}))).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&answer).unwrap()["notes"], 1);
        assert_eq!(std::fs::read(&project).unwrap(), before);
        let midi_bytes = std::fs::read(&midi).unwrap();
        assert!(midi_bytes.starts_with(b"MThd"));
        assert!(
            export_midi::run(&request(json!({"project":project,"output":midi})))
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(std::fs::read(&midi).unwrap(), midi_bytes);
        let answer = import_midi::run(&request(
            json!({"source":midi,"output":root.path().join("Imported.auris")}),
        ))
        .unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        let imported = opened(answer["project"].as_str().unwrap()).unwrap();
        assert!((imported.project().bpm() - 90.0).abs() < 0.0001);
        assert!((imported.project().tempo_map.bpm_at(Ticks::QUARTER * 3) - 110.0).abs() < 0.0001);
        assert_eq!(imported.signature_at(Ticks::ZERO), TimeSignature::new(3, 4));
        let clip = &imported.project().tracks[0].kind.note_clips().unwrap()[0];
        assert_eq!(clip.start + clip.notes[0].start, Ticks::QUARTER * 3);
        assert_eq!(clip.notes[0].length, Ticks::QUARTER);
        assert_eq!(clip.notes[0].pitch, 64);
    }

    fn write_sample(path: &Path) {
        let frames = 480_u32;
        let data_bytes = frames * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&96_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for index in 0..frames {
            let sample = if index % 2 == 0 {
                2_000_i16
            } else {
                -2_000_i16
            };
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, wav).unwrap();
    }

    #[test]
    fn audio_import_copies_the_source_and_reopens_at_the_requested_bar() {
        let root = tempfile::tempdir().unwrap();
        let project = created(root.path(), "Recording");
        let wav = root.path().join("Take.wav");
        write_sample(&wav);
        let answer = import_audio::run(&request(
            json!({"project":project,"source":wav,"start_bar":3}),
        ))
        .unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(answer["copied_into_project"], true);
        assert!((answer["seconds"].as_f64().unwrap() - 0.01).abs() < 0.005);
        let saved_audio = PathBuf::from(answer["asset"].as_str().unwrap());
        assert!(saved_audio.starts_with(Path::new(&project).parent().unwrap().join("Audio")));
        assert_eq!(
            std::fs::read(&saved_audio).unwrap(),
            std::fs::read(&wav).unwrap()
        );
        std::fs::remove_file(&wav).unwrap();
        let reopened = opened(&project).unwrap();
        assert_eq!(reopened.project().tracks.len(), 2);
        let audio = reopened
            .project()
            .audio_clip(ClipId(answer["clip_id"].as_u64().unwrap()))
            .unwrap();
        assert_eq!(audio.start, Ticks::QUARTER * 6);
        assert!(
            reopened.project().audio_sources[&audio.source]
                .path
                .is_inside()
        );
        assert!(!reopened.checkpoints().unwrap().is_empty());
        let before = std::fs::read(&project).unwrap();
        assert!(
            import_audio::run(&request(
                json!({"project":project,"source":saved_audio,"start_bar":0})
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(&project).unwrap(), before);
    }

    #[test]
    fn file_tools_refuse_relative_paths_invalid_extensions_and_invalid_clocks() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            create_project::run(&request(json!({"output":"Relative.auris"})))
                .unwrap_err()
                .contains("absolute")
        );
        for overrides in [json!({"tempo":0}), json!({"meter":"3/3"})] {
            let mut value = overrides;
            value["output"] = root
                .path()
                .join("Invalid.auris")
                .to_string_lossy()
                .into_owned()
                .into();
            assert!(create_project::run(&request(value)).is_err());
            assert!(!root.path().join("Invalid").exists());
        }
        let project = created(root.path(), "Source");
        assert!(
            export_midi::run(&request(
                json!({"project":project,"output":root.path().join("audio.wav")})
            ))
            .unwrap_err()
            .contains("extension")
        );
        assert!(
            import_audio::run(&request(json!({"project":project,"source":"relative.wav"})))
                .unwrap_err()
                .contains("absolute")
        );
    }
}
