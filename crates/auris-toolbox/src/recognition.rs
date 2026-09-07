//! CPU music recognition commands shared by both model frontends.

use super::*;
use auris_session::{AnalysisControl, AudioOptions, ChordOptions};

fn beats(value: f64) -> Result<Ticks, String> {
    if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
        return Err("beat positions must be finite and within 0..1000000".into());
    }
    Ok(Ticks::from_beats(value))
}

/// Optional local instrument-presence tagging without project edits.
pub mod analyze_instruments {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "analyze_instruments";
    /// Model-facing command description.
    pub const DESCRIPTION: &str = "Estimates instrument and singing presence with an explicitly supplied local YAMNet ONNX export on CPU. Returns overlapping source-second windows, multiple candidate labels, raw event scores and model hash. Empty candidates mean unknown. Scores are not calibrated probabilities. No downloads, GPU, source separation, note assignment or project edits.";
    /// Audio input and an explicitly prepared model.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute audio-file path.
        pub audio: String,
        /// Absolute path to the ONNX file prepared by tools/music-models/export_yamnet.py.
        pub model: String,
        /// Minimum displayed instrument score in 0..1; defaults to 0.2.
        pub threshold: Option<f32>,
    }
    /// Runs local inference and serializes the read-only report.
    pub fn run(args: &Args) -> Result<String, String> {
        let report = auris_session::analyze_instrument_file(
            Path::new(&args.audio),
            Path::new(&args.model),
            args.threshold.unwrap_or(0.2),
            &AnalysisControl::default(),
        )
        .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
    }
}

/// Optional noncommercial mixture transcription and explicit draft acceptance.
pub mod transcribe_mixture {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "transcribe_mixture";
    /// Model-facing command description.
    pub const DESCRIPTION: &str = "Uses optional local MuScriptor Small on CPU for instrument-labeled note drafts. Its model is CC BY-NC 4.0, noncommercial only; present this restriction and obtain explicit user acknowledgement for this invocation before setting acknowledge_noncommercial=true. Acknowledgement does not grant commercial rights. Auris itself remains Apache-2.0. Requires a prepared Python environment and local checkpoint; no downloads. Defaults to read-only JSON. Optional MIDI creates a new file; apply adds instrument tracks and saves a checkpoint. Notes and playback patches need review.";
    /// Local inference and optional write destinations.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute input audio path.
        pub audio: String,
        /// Absolute Python executable in the optional muscriptor==0.3.0 environment.
        pub python: String,
        /// Absolute local MuScriptor Small safetensors checkpoint.
        pub model: String,
        /// True only after the user explicitly accepted noncommercial use for this invocation.
        #[serde(default)]
        pub acknowledge_noncommercial: bool,
        /// New MIDI destination; existing files are refused.
        pub midi_output: Option<String>,
        /// Existing project to receive new tracks; requires apply.
        pub project: Option<String>,
        /// Zero-based insertion beat; defaults to zero.
        pub at_beat: Option<f64>,
        /// Explicitly modify and save the project; defaults to false.
        #[serde(default)]
        pub apply: bool,
    }
    /// Requires acknowledgement before any audio I/O, worker creation or project edit.
    pub fn run(args: &Args) -> Result<String, String> {
        if !args.acknowledge_noncommercial {
            return Err(auris_session::MUSCRIPTOR_NOTICE.into());
        }
        if args.apply != args.project.is_some() {
            return Err("project and apply must be supplied together".into());
        }
        if args
            .midi_output
            .as_ref()
            .is_some_and(|p| Path::new(p).exists())
        {
            return Err("MIDI output already exists".into());
        }
        let start = beats(args.at_beat.unwrap_or(0.0))?;
        let config = auris_session::MixtureOptions {
            python: args.python.clone().into(),
            model: args.model.clone().into(),
            acknowledge_noncommercial: true,
        };
        let report = auris_session::transcribe_mixture_file(
            Path::new(&args.audio),
            &config,
            &AnalysisControl::default(),
        )
        .map_err(|e| e.to_string())?;
        if let Some(path) = &args.midi_output {
            let mut output = Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
            output
                .create_mixture_tracks(&report, Ticks::ZERO)
                .map_err(|e| e.to_string())?;
            output
                .export_midi(Path::new(path))
                .map_err(|e| e.to_string())?;
        }
        if let Some(path) = &args.project {
            let mut output = opened(path)?;
            output
                .create_mixture_tracks(&report, start)
                .map_err(|e| e.to_string())?;
            output.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
    }
}

/// Written notes to chromatic chord candidates and optional explicit harmony edits.
pub mod analyze_chords {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "analyze_chords";
    /// Model-facing command description.
    pub const DESCRIPTION: &str = "Recognizes chords from written notes on the CPU without rendering or models. Reports absolute-tick intervals, alternate chord symbols and unknown/silent regions. Known percussion is excluded. Apply explicitly replaces recognized harmony and clears silence while preserving unknown intervals and outside harmony; saves a checkpoint.";
    /// Track selection, time range and optional acceptance.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Track names or id:number selectors; empty selects all pitched tracks.
        #[serde(default)]
        pub tracks: Vec<String>,
        /// Zero-based quarter-note beat at which to start; defaults to zero.
        pub from_beat: Option<f64>,
        /// Exclusive zero-based end beat; defaults to the project end.
        pub to_beat: Option<f64>,
        /// Window size in quarter-note beats; defaults to one.
        pub window_beats: Option<f64>,
        /// Explicitly accept recognized candidates and save; false only reports.
        #[serde(default)]
        pub apply: bool,
    }
    /// Analyzes one immutable selection and optionally accepts its recognized harmony.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let tracks = args
            .tracks
            .iter()
            .map(|t| track_by_name(session.project(), t).map(|t| t.id))
            .collect::<Result<Vec<_>, _>>()?;
        let from = beats(args.from_beat.unwrap_or(0.0))?;
        let to = args
            .to_beat
            .map(beats)
            .transpose()?
            .unwrap_or_else(|| session.project().end_tick());
        let options = ChordOptions {
            window: beats(args.window_beats.unwrap_or(1.0))?,
        };
        let report = session
            .chord_analysis_job(&tracks, from, to, options)
            .and_then(|j| j.run(&AnalysisControl::default()))
            .map_err(|e| e.to_string())?;
        if args.apply {
            session
                .apply_chord_analysis(&report)
                .map_err(|e| e.to_string())?;
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
    }
}

/// File-based tempo and major/minor chord analysis without a project edit.
pub mod analyze_audio {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "analyze_audio";
    /// Model-facing command description.
    pub const DESCRIPTION: &str = "Analyzes an audio file on the CPU without models or GPU: constant BPM alternatives, beat timestamps and half-second major/minor chord windows. Scores are template/periodicity agreement, not calibrated probabilities. No project is changed. Does not identify instruments.";
    /// Audio input.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute audio-file path.
        pub audio: String,
    }
    /// Decodes and reports in source seconds.
    pub fn run(args: &Args) -> Result<String, String> {
        let report = auris_session::analyze_audio_file(
            Path::new(&args.audio),
            AudioOptions::default(),
            &AnalysisControl::default(),
        )
        .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
    }
}

/// Isolated monophonic transcription and explicit MIDI or project output.
pub mod transcribe_audio {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "transcribe_audio";
    /// Model-facing command description.
    pub const DESCRIPTION: &str = "Transcribes an isolated monophonic audio file using CPU YIN, without models or GPU. Supports approximately 65-1000 Hz; does not separate mixed instruments or produce engraved staff notation. Returns source-second note estimates. Optional MIDI output creates a new file; apply adds an editable note track to a project and saves a checkpoint. Existing notes and tempo are preserved.";
    /// Input, optional destination, and explicit acceptance.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute audio-file path.
        pub audio: String,
        /// Existing project receiving a new track; requires apply.
        pub project: Option<String>,
        /// New MIDI file containing only the transcription; existing files are refused.
        pub midi_output: Option<String>,
        /// Name for the new track and clip; defaults to Transcription.
        pub name: Option<String>,
        /// Zero-based insertion beat in the project's current tempo map; defaults to zero.
        pub at_beat: Option<f64>,
        /// Explicitly modify the named project. Defaults to false.
        #[serde(default)]
        pub apply: bool,
    }
    /// Reports notes, optionally writes a new MIDI and/or accepts them in a project.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.apply != args.project.is_some() {
            return Err("project and apply must be supplied together".into());
        }
        if args
            .midi_output
            .as_ref()
            .is_some_and(|p| Path::new(p).exists())
        {
            return Err("MIDI output already exists".into());
        }
        let start = beats(args.at_beat.unwrap_or(0.0))?;
        let report = auris_session::analyze_audio_file(
            Path::new(&args.audio),
            AudioOptions { transcribe: true },
            &AnalysisControl::default(),
        )
        .map_err(|e| e.to_string())?;
        let name = args.name.as_deref().unwrap_or("Transcription");
        if let Some(path) = &args.midi_output {
            let mut output = Session::new(SessionOptions::headless()).map_err(|e| e.to_string())?;
            output
                .create_transcription_track(&report, Ticks::ZERO, name)
                .map_err(|e| e.to_string())?;
            output
                .export_midi(Path::new(path))
                .map_err(|e| e.to_string())?;
        }
        if let Some(path) = &args.project {
            let mut session = opened(path)?;
            session
                .create_transcription_track(&report, start, name)
                .map_err(|e| e.to_string())?;
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixture_tool_defaults_to_no_consent_and_refuses_before_io() {
        let args: transcribe_mixture::Args =
            serde_json::from_str(r#"{"audio":"missing.wav","python":"missing","model":"missing"}"#)
                .unwrap();
        assert!(!args.acknowledge_noncommercial);
        assert!(
            transcribe_mixture::run(&args)
                .unwrap_err()
                .contains("CC BY-NC 4.0")
        );
    }
    #[test]
    fn invalid_ranges_and_accidental_project_writes_are_rejected_before_io() {
        assert!(beats(f64::NAN).is_err());
        assert!(beats(-1.0).is_err());
        let args = transcribe_audio::Args {
            audio: "missing.wav".into(),
            project: Some("missing.auris".into()),
            midi_output: None,
            name: None,
            at_beat: None,
            apply: false,
        };
        assert!(
            transcribe_audio::run(&args)
                .unwrap_err()
                .contains("together")
        );
    }

    #[test]
    fn file_tools_report_without_writing_then_export_only_the_draft_and_apply() {
        let root = std::env::temp_dir().join(format!(
            "auris-recognition-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wav = root.join("mono.wav");
        // An independently encoded, 16-bit 22050 Hz WAV exercises the real decoder/resampler.
        let rate = 22_050u32;
        let samples: Vec<i16> = (0..rate)
            .map(|i| {
                let t = f64::from(i) / f64::from(rate);
                if (0.2..0.8).contains(&t) {
                    ((std::f64::consts::TAU * 440.0 * t).sin() * 12000.0) as i16
                } else {
                    0
                }
            })
            .collect();
        let bytes = samples.len() as u32 * 2;
        let mut wav_bytes = Vec::new();
        wav_bytes.extend_from_slice(b"RIFF");
        wav_bytes.extend_from_slice(&(36 + bytes).to_le_bytes());
        wav_bytes.extend_from_slice(b"WAVEfmt ");
        wav_bytes.extend_from_slice(&16u32.to_le_bytes());
        for value in [1u16, 1u16] {
            wav_bytes.extend_from_slice(&value.to_le_bytes());
        }
        wav_bytes.extend_from_slice(&rate.to_le_bytes());
        wav_bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        for value in [2u16, 16u16] {
            wav_bytes.extend_from_slice(&value.to_le_bytes());
        }
        wav_bytes.extend_from_slice(b"data");
        wav_bytes.extend_from_slice(&bytes.to_le_bytes());
        for value in samples {
            wav_bytes.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(&wav, &wav_bytes).unwrap();
        let project = root.join("Song.auris");
        let mut s = Session::new(SessionOptions::headless()).unwrap();
        let track = s.add_default_instrument_track("Existing").unwrap();
        let clip = s
            .add_midi_clip(track, "Triad", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        for pitch in [60, 64, 67] {
            s.add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }
        s.save(&project).unwrap();
        let original = std::fs::read(&project).unwrap();
        let chord_args = analyze_chords::Args {
            project: project.display().to_string(),
            tracks: vec![],
            from_beat: None,
            to_beat: None,
            window_beats: None,
            apply: false,
        };
        let chord_json: serde_json::Value =
            serde_json::from_str(&analyze_chords::run(&chord_args).unwrap()).unwrap();
        assert_eq!(
            chord_json["segments"][0]["reading"]["candidates"][0]["symbol"],
            "C"
        );
        assert_eq!(std::fs::read(&project).unwrap(), original);
        let mut args = transcribe_audio::Args {
            audio: wav.display().to_string(),
            project: None,
            midi_output: None,
            name: None,
            at_beat: None,
            apply: false,
        };
        let json: serde_json::Value =
            serde_json::from_str(&transcribe_audio::run(&args).unwrap()).unwrap();
        assert_eq!(json["notes"].as_array().unwrap().len(), 1);
        assert_eq!(json["notes"][0]["pitch"], 69);
        let midi = root.join("Draft.mid");
        args.midi_output = Some(midi.display().to_string());
        args.project = Some(project.display().to_string());
        args.apply = true;
        args.at_beat = Some(8.0);
        transcribe_audio::run(&args).unwrap();
        let saved = opened(project.to_str().unwrap()).unwrap();
        assert_eq!(saved.project().tracks.len(), 2);
        assert_eq!(saved.project().midi_clip(clip).unwrap().1.notes.len(), 3);
        let mut exported = Session::new(SessionOptions::headless()).unwrap();
        exported.import_midi(&midi).unwrap();
        let pitches: Vec<_> = exported
            .project()
            .tracks
            .iter()
            .flat_map(|t| t.kind.note_clips().into_iter().flatten())
            .flat_map(|c| c.notes.iter().map(|n| n.pitch))
            .collect();
        assert_eq!(pitches, vec![69]);
        assert!(
            transcribe_audio::run(&args)
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(std::fs::read(&wav).unwrap(), wav_bytes);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
