//! Exact training ranges and manifests for the existing snapshot export pipeline.

use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A source interval, with an exclusive end and no implicit shortening.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisRange {
    /// Seconds on the project timeline, rounded to the nearest output frame.
    Seconds {
        /// Inclusive source start.
        start: f64,
        /// Exclusive source end.
        end: f64,
    },
    /// Frames at the explicitly requested output sample rate.
    Frames {
        /// Inclusive source frame.
        start: u64,
        /// Exclusive source frame.
        end: u64,
    },
}

/// Explicit source-preserving export settings; output is always stereo float32 WAV.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRender {
    /// Output sample rate, 8000..96000 Hz; 48000 is recommended for model input.
    pub sample_rate: u32,
    /// Exact interval up to 300 seconds; independent of audition bar limits.
    pub range: AnalysisRange,
    /// Optional source track or bus, heard through its existing solo routing and master.
    /// Omit for the mix or for all source stems requested by destination.
    pub track: Option<u64>,
    /// Source before fader/pan/inserts, routed directly to an unprocessed master.
    /// Requires one source track; instrument automation and clip performance remain active.
    #[serde(default)]
    pub dry: bool,
    /// Render from timeline zero and discard earlier frames to retain effect context.
    #[serde(default)]
    pub context_from_start: bool,
    /// Capture a warmed-up cycle. The range must be one complete intended cycle.
    #[serde(default)]
    pub looping: bool,
    /// New JSON manifest path, workspace-relative or absolute. Outputs survive audition-cache eviction.
    pub manifest_path: String,
}

impl AnalysisRender {
    pub(super) fn select_signal(&self, project: &mut auris_core::Project) -> Result<(), String> {
        if !self.dry {
            return Ok(());
        }
        let selected = TrackId(
            self.track
                .ok_or("Dry analysis requires an explicit source track")?,
        );
        if project
            .track(selected)
            .is_none_or(|track| matches!(track.kind, auris_core::project::TrackKind::Bus))
        {
            return Err("Dry analysis requires an instrument, drum, singer or audio source".into());
        }
        let remove:Vec<_>=project.automation.lanes().iter().filter(|lane|!matches!(lane.target,crate::ParamTarget::Instrument{track,..} if track==selected)).map(|lane|lane.target).collect();
        for target in remove {
            project.automation.remove_lane(target);
        }
        for track in &mut project.tracks {
            track.mixer = Default::default();
            track.mixer.mute = track.id != selected;
            track.outputs = vec![auris_core::project::Output::master()];
        }
        project.master = Default::default();
        Ok(())
    }
    pub(super) fn options(&self) -> Result<OfflineOptions, String> {
        if !(8_000..=96_000).contains(&self.sample_rate) {
            return Err("Analysis sample_rate must be within 8000..96000 Hz".into());
        }
        let rate = f64::from(self.sample_rate);
        let (start, end) = match self.range {
            AnalysisRange::Frames { start, end } => (start, end),
            AnalysisRange::Seconds { start, end } => {
                if !start.is_finite()
                    || !end.is_finite()
                    || start < 0.0
                    || end <= start
                    || end * rate > (u64::MAX / 2) as f64
                {
                    return Err(
                        "Analysis requires finite nonnegative seconds and end > start".into(),
                    );
                }
                ((start * rate).round() as u64, (end * rate).round() as u64)
            }
        };
        if end <= start
            || end - start > u64::from(self.sample_rate) * 300
            || end > u64::from(self.sample_rate) * 86_400
            || (self.looping && self.context_from_start)
        {
            return Err("Analysis range must contain 1 frame..300 seconds within a 24-hour timeline; choose context or looping".into());
        }
        Ok(OfflineOptions {
            start_frames: start,
            end_frames: Some(end),
            sample_rate: Some(rate),
            include_tail: false,
            looping: self.looping,
            ..Default::default()
        })
    }
}

pub(super) struct Manifest {
    pub path: PathBuf,
    pub value: Value,
    pub dependencies: Vec<(PathBuf, String)>,
    pub context_from_start: bool,
    fonts: Vec<(auris_core::SoundFontId, Arc<auris_io::SoundFont>)>,
}

pub(super) fn hash_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    if path.is_dir() {
        fn walk(
            root: &std::path::Path,
            directory: &std::path::Path,
            files: &mut Vec<PathBuf>,
            depth: u8,
        ) -> Result<(), String> {
            if depth > 16 || files.len() > 10_000 {
                return Err("Plugin bundle identity exceeds bounded traversal".into());
            }
            for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink() {
                    return Err("Plugin bundle contains an unresolved symbolic link".into());
                }
                if kind.is_dir() {
                    walk(root, &entry.path(), files, depth + 1)?;
                } else if kind.is_file() {
                    files.push(
                        entry
                            .path()
                            .strip_prefix(root)
                            .map_err(|e| e.to_string())?
                            .to_owned(),
                    );
                }
            }
            Ok(())
        }
        let mut files = Vec::new();
        walk(path, path, &mut files, 0)?;
        files.sort();
        if files.is_empty() {
            return Err("Plugin bundle is empty".into());
        }
        let mut hash = Sha256::new();
        for file in files {
            hash.update(file.to_string_lossy().replace('\\', "/").as_bytes());
            hash.update([0]);
            hash.update(hash_file(&path.join(file))?.as_bytes());
            hash.update([0]);
        }
        return Ok(hex::encode(hash.finalize()));
    }
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Dependency is not a regular file".into());
    }
    let mut hash = Sha256::new();
    let mut block = [0; 64 * 1024];
    loop {
        let count = file.read(&mut block).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&block[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

impl Manifest {
    pub(super) fn freeze_dependencies(&mut self) -> Result<(), String> {
        let mut loaded_fonts = Vec::new();
        for (id, font) in &self.fonts {
            let mut hash = Sha256::new();
            let mut bytes = [0u8; 8192];
            for block in font.get_wave_data().chunks(4096) {
                for (sample, output) in block.iter().zip(bytes.as_chunks_mut::<2>().0) {
                    output.copy_from_slice(&sample.to_le_bytes());
                }
                hash.update(&bytes[..block.len() * 2]);
            }
            let parameters = format!(
                "{:?}{:?}{:?}",
                font.get_sample_headers(),
                font.get_presets(),
                font.get_instruments()
            );
            loaded_fonts.push(json!({"id":id,"sample_data_sha256":hex::encode(hash.finalize()),
                "parameters_sha256":hex::encode(Sha256::digest(parameters.as_bytes())),"parameter_identity_encoding":"rust_debug_build_dependent"}));
        }
        self.value["loaded_soundfonts"] = json!(loaded_fonts);
        if let Some(assets) = self.value["assets"].as_array_mut() {
            for asset in assets {
                if let Some(path) = asset["path"].as_str() {
                    let path = PathBuf::from(path);
                    match hash_file(&path) {
                        Ok(hash) => {
                            asset["sha256"] = hash.clone().into();
                            asset["status"] = "resolved".into();
                            self.dependencies.push((path, hash));
                        }
                        Err(_) => {
                            asset["status"] = "missing_or_unreadable".into();
                        }
                    }
                }
            }
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        self.value["renderer_binary_sha256"] = hash_file(&executable)?.into();
        Ok(())
    }
    pub(super) fn check_dependencies(&self) -> Result<(), String> {
        for (path, expected) in &self.dependencies {
            if hash_file(path).as_ref() != Ok(expected) {
                return Err(format!(
                    "Render dependency changed or disappeared: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn stage(
    job: &mut crate::RenderJob,
    targets: Vec<(PathBuf, Option<TrackId>)>,
    options: &OfflineOptions,
    context_from_start: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<StagedAudio>, String> {
    let mut staged = Vec::new();
    for (path, track) in targets {
        let mut progress = RenderProgress::default().cancelled_by(cancelled);
        let encoded = job
            .stage_analysis_export(&path, options, track, context_from_start, &mut progress)
            .map_err(|e| e.to_string())?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Analysis render cancelled".into());
        }
        let inspection = crate::inspect_export(encoded.staged_path()).map_err(|e| e.to_string())?;
        staged.push(StagedAudio {
            path,
            encoded,
            source_true_peak_db: inspection.true_peak_db,
            source_inspection: Some(inspection.clone()),
            inspection,
            attenuation_db: 0.0,
            ceiling_db: 0.0,
        });
    }
    Ok(staged)
}

impl Session {
    pub(super) fn analysis_manifest(
        &mut self,
        request: &AnalysisRender,
        options: &OfflineOptions,
        project: &auris_core::Project,
    ) -> Result<Manifest, String> {
        let mut assets = Vec::new();
        let folder = self.project_folder().map(std::path::Path::to_owned);
        let mut identify = |kind: &str, id: String, asset: &auris_core::AssetPath| {
            let path = asset.resolve(folder.as_deref());
            assets.push(json!({"kind":kind,"id":id,"path":path,"sha256":null,"status":"pending_worker_identification","identity_scope":"file_at_export"}));
        };
        for (id, source) in &self.project().audio_sources {
            identify("audio", id.0.to_string(), &source.path);
        }
        for (id, font) in &self.project().soundfonts {
            identify("soundfont", id.0.to_string(), &font.path);
        }
        let mut instruments = Vec::new();
        let mut effects = Vec::new();
        let mut singers = Vec::new();
        for track in &project.tracks {
            if let Some(singer) = track.kind.as_singer() {
                if let Some(voice) = &singer.voice {
                    identify(
                        "singing_entry_configuration",
                        track.id.0.to_string(),
                        &voice.path,
                    );
                }
                singers.push(json!({"track":track.id,"voice":singer.voice,"take":singer.take,
                    "synthesis_provenance":singer.take.as_ref().and_then(|take|take.provenance.as_ref()),
                    "synthesis_provenance_status":if singer.take.as_ref().is_some_and(|take|take.provenance.is_some()){"captured_at_synthesis"}else{"unknown"},
                    "guide_instrument_id":singer.instrument_id,"guide_state_sha256":hex::encode(Sha256::digest(serde_json::to_vec(&singer.instrument_state).map_err(|e|e.to_string())?)),
                    "state":self.singer_take_state(track.id).map(|s|format!("{s:?}")).ok()}));
            }
            if let Some(instrument) = track.kind.as_instrument() {
                if let Some(path) = &instrument.file {
                    identify("instrument_binary", track.id.0.to_string(), path);
                }
                let metadata = if instrument.is_hosted() {
                    self.render_plugin_metadata(crate::PluginWindow::Instrument(track.id))
                } else {
                    self.registry().descriptor(&instrument.instrument_id).map(|d|json!({"format":"builtin","id":d.id,"vendor":d.vendor,"version":env!("CARGO_PKG_VERSION")}))
                };
                let descriptors = self.instrument_descriptors(track.id);
                let parameters: Vec<_> = descriptors
                    .iter()
                    .map(|d| {
                        let mut report = self.agent_parameter_report(
                            crate::ParamTarget::Instrument {
                                track: track.id,
                                param: d.id,
                            },
                            d,
                        );
                        report["choices"] = json!(d.choices);
                        report["value"] = instrument
                            .instrument_state
                            .params
                            .get(d.key.as_ref())
                            .copied()
                            .unwrap_or(d.default)
                            .into();
                        report
                    })
                    .collect();
                instruments.push(json!({"track":track.id,"instrument_id":instrument.instrument_id,
                    "state_sha256":hex::encode(Sha256::digest(serde_json::to_vec(&instrument.instrument_state).map_err(|e|e.to_string())?)),
                    "parameters":parameters,"metadata":metadata,"hosted":instrument.is_hosted(),
                    "determinism":if instrument.is_hosted(){"unknown"}else{"build_dependent"}}));
            }
        }
        for (track_id, strip) in project
            .tracks
            .iter()
            .map(|t| (Some(t.id), &t.mixer))
            .chain([(None, &project.master)])
        {
            for slot in &strip.effects {
                if let Some(path) = &slot.file {
                    identify("effect_binary", slot.id.0.to_string(), path);
                }
                let metadata = if slot.is_hosted() {
                    self.render_plugin_metadata(crate::PluginWindow::Effect(slot.id))
                } else {
                    self.registry().descriptor(&slot.effect_id).map(|d|json!({"format":"builtin","id":d.id,"vendor":d.vendor,"version":env!("CARGO_PKG_VERSION")}))
                };
                let descriptors = self.effect_descriptors(slot.id);
                let parameters: Vec<_> = descriptors
                    .iter()
                    .map(|d| {
                        let mut report = self.agent_parameter_report(
                            crate::ParamTarget::Effect {
                                track: track_id,
                                slot: slot.id,
                                param: d.id,
                            },
                            d,
                        );
                        report["choices"] = json!(d.choices);
                        report["value"] = slot
                            .state
                            .params
                            .get(d.key.as_ref())
                            .copied()
                            .unwrap_or(d.default)
                            .into();
                        report
                    })
                    .collect();
                effects.push(json!({"track":track_id,"slot":slot.id,"effect_id":slot.effect_id,"enabled":slot.enabled,
                    "state_sha256":hex::encode(Sha256::digest(serde_json::to_vec(&slot.state).map_err(|e| e.to_string())?)),
                    "parameters":parameters,"metadata":metadata,"hosted":slot.is_hosted(),"determinism":if slot.is_hosted(){"unknown"}else{"build_dependent"}}));
            }
        }
        Ok(Manifest {
            path: self.agent_path(&request.manifest_path)?,
            dependencies: Vec::new(),
            context_from_start: request.context_from_start,
            fonts: self.analysis_loaded_fonts(),
            value: json!({"schema_version":1,"tool":"render","tool_version":env!("CARGO_PKG_VERSION"),
                "project_sha256":self.agent_document_revision()?,"session_revision":self.revision(),
                "render_project_sha256":hex::encode(Sha256::digest(serde_json::to_vec(project).map_err(|e|e.to_string())?)),
                "document_instance":self.document_instance(),"project_format_version":self.project().format_version,
                "tempo_map":self.project().tempo_map,"signature_map":self.project().signatures,
                "automation":project.automation,"instruments":instruments,"effects":effects,
                "routing":project.tracks.iter().map(|track|json!({"track":track.id,"outputs":track.outputs,"gain_db":track.mixer.gain_db,"pan":track.mixer.pan,"mute":track.mixer.mute,"solo":track.mixer.solo})).collect::<Vec<_>>(),
                "master":{"gain_db":project.master.gain_db,"pan":project.master.pan,"mute":project.master.mute},
                "clip_recipes":project.tracks.iter().flat_map(|track|track.kind.note_clips().into_iter().flatten().map(move|clip|json!({"track":track.id,"clip":clip.id,"recipe":clip.recipe,"transforms":clip.transforms}))).collect::<Vec<_>>(),
                "assets":assets,"singers":singers,"playback_readiness":self.playback_readiness(),
                "audio_placements":project.tracks.iter().filter_map(|track|track.kind.as_audio().map(|audio|json!({"track":track.id,"clips":audio.clips}))).collect::<Vec<_>>(),
                "audio_sources":project.audio_sources.values().map(|source|json!({"id":source.id,"frames":source.frame_count,"sample_rate":source.sample_rate,"channels":source.channel_count})).collect::<Vec<_>>(),
                "sample_rate":request.sample_rate,"channels":["left","right"],"sample_format":"float32",
                "timeline_frame_rate":request.sample_rate,
                "source_start_frame":options.start_frames,"source_end_frame":options.end_frames,
                "source_start_seconds":options.start_frames as f64 / f64::from(request.sample_rate),
                "source_end_seconds":options.end_frames.map(|end|end as f64 / f64::from(request.sample_rate)),
                "include_tail":false,"looping":request.looping,"context_from_start":request.context_from_start,
                "signal":if request.dry{"pre_fader_source_no_inserts_direct_unprocessed_master"}else{"post_fader_wet_solo_routing_through_master"},"selected_track":request.track,
                "mode":"offline_analysis","gain_db":0.0,"source_preserving":true,
                "determinism":"not_certified","metadata_authenticity":"declared_not_verified"}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_keeps_delay_echoes_dry_bypasses_them_and_cycles_have_exact_length() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("impulse.wav");
        crate::audio_input::tests::stereo(&source, 24000, [0.5, -0.5], [0.0, 0.0]);
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        session.import_audio(&source, Ticks::ZERO).unwrap();
        let track = session.project().tracks[0].id;
        let delay = session.add_effect(Some(track), "auris.fx.delay").unwrap();
        for (param, value) in [(0, 100.0), (1, 0.5), (3, 1.0)] {
            session.set_param(
                crate::ParamTarget::Effect {
                    track: Some(track),
                    slot: delay,
                    param: auris_core::param::ParamId(param),
                },
                value,
            );
        }
        let before = session.project().clone();
        let cancelled = AtomicBool::new(false);
        let mut peaks = Vec::new();
        for (name, context, dry, looping, start) in [
            ("reset", false, false, false, 4800),
            ("context", true, false, false, 4800),
            ("dry", true, true, false, 4800),
            ("loop", false, false, true, 0),
        ] {
            let command=serde_json::from_value(json!({"action":"render","destination":{"kind":"mix","path":directory.path().join(format!("{name}.wav"))},
                "analysis":{"sample_rate":48000,"range":{"unit":"frames","start":start,"end":start+4800},"track":track.0,
                    "context_from_start":context,"dry":dry,"looping":looping,"manifest_path":directory.path().join(format!("{name}.json"))}})).unwrap();
            let result = session
                .begin_agent_file_job(command)
                .unwrap()
                .run(&cancelled)
                .unwrap();
            session.continue_agent_file_job(result, &cancelled).unwrap();
            let mut wav =
                hound::WavReader::open(directory.path().join(format!("{name}.wav"))).unwrap();
            assert_eq!(wav.duration(), 4800);
            peaks.push(
                wav.samples::<f32>()
                    .map(|s| s.unwrap().abs())
                    .fold(0.0f32, f32::max),
            );
        }
        assert_eq!(peaks[0], 0.0);
        assert!(peaks[1] > 0.0);
        assert_eq!(peaks[2], 0.0);
        assert!(peaks[3] > 0.0);
        assert_eq!(session.project(), &before);
    }

    fn request(directory: &std::path::Path, end: u64) -> Command {
        serde_json::from_value(json!({"action":"render","destination":{"kind":"mix","path":directory.join("analysis.wav")},
            "analysis":{"sample_rate":48000,"range":{"unit":"frames","start":0,"end":end},"manifest_path":directory.join("manifest.json")}})).unwrap()
    }

    #[test]
    fn analysis_keeps_overrange_stereo_and_persistent_snapshot_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.wav");
        crate::audio_input::tests::stereo(&source, 4800, [4.0, -4.0], [2.0, -3.0]);
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        session.import_audio(&source, Ticks::ZERO).unwrap();
        let before = session.project().clone();
        let revision = session.agent_document_revision().unwrap();
        let cancelled = AtomicBool::new(false);
        let result = session
            .begin_agent_file_job(request(directory.path(), 4800))
            .unwrap()
            .run(&cancelled)
            .unwrap();
        assert!(!directory.path().join("analysis.wav").exists());
        let reply: Value =
            serde_json::from_str(&session.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(session.project(), &before);
        assert_eq!(manifest["project_sha256"], revision);
        assert_eq!(
            manifest["files"][0]["encoded"]["sha256"],
            reply["files"][0]["encoded"]["sha256"]
        );
        assert_eq!(manifest["files"][0]["encoded"]["frames"], 4800);
        assert_eq!(manifest["gain_db"], 0.0);
        assert_eq!(reply["files"][0]["attenuation_db"], 0.0);
        let mut wav = hound::WavReader::open(directory.path().join("analysis.wav")).unwrap();
        let samples: Vec<_> = wav.samples::<f32>().map(Result::unwrap).collect();
        assert!(samples.iter().any(|s| *s > 1.0));
        assert!(samples.iter().any(|s| *s < -1.0));
        assert_eq!(manifest["sample_format"], "float32");
        assert_eq!(manifest["loaded_audio"][0]["loaded"]["channels"], 2);
        drop(session);
        assert!(directory.path().join("analysis.wav").exists());
        assert!(directory.path().join("manifest.json").exists());
    }

    #[test]
    fn long_analysis_is_exact_and_not_subject_to_audition_limits() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("whole.wav");
        let frames = 48_000 * 300;
        crate::audio_input::tests::stereo(&source, frames, [0.5, -0.5], [0.25, -0.25]);
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        session.import_audio(&source, Ticks::ZERO).unwrap();
        let cancelled = AtomicBool::new(false);
        let result = session
            .begin_agent_file_job(request(directory.path(), u64::from(frames)))
            .unwrap()
            .run(&cancelled)
            .unwrap();
        session.continue_agent_file_job(result, &cancelled).unwrap();
        let mut wav = hound::WavReader::open(directory.path().join("analysis.wav")).unwrap();
        assert_eq!(wav.duration(), frames);
        assert_eq!(wav.spec().sample_rate, 48_000);
        wav.seek(frames - 1).unwrap();
        let end: Vec<_> = wav.samples::<f32>().map(Result::unwrap).collect();
        assert!(end[0] > 0.0 && end[1] < 0.0, "last event retained: {end:?}");
        let mut invalid = request(directory.path(), u64::from(frames) + 1);
        if let Command::Render {
            analysis: Some(analysis),
            ..
        } = &mut invalid
        {
            analysis.manifest_path = directory
                .path()
                .join("too-long.json")
                .to_string_lossy()
                .into();
        }
        assert!(session.begin_agent_file_job(invalid).is_err());
    }

    #[test]
    fn stale_or_cancelled_analysis_never_publishes_audio_or_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let cancelled = AtomicBool::new(false);
        for cancel in [false, true] {
            let result = session
                .begin_agent_file_job(request(directory.path(), 480))
                .unwrap()
                .run(&cancelled)
                .unwrap();
            if cancel {
                cancelled.store(true, Ordering::Relaxed);
            } else {
                session.add_bus_track("Concurrent human edit");
            }
            assert!(session.continue_agent_file_job(result, &cancelled).is_err());
            assert!(!directory.path().join("analysis.wav").exists());
            assert!(!directory.path().join("manifest.json").exists());
        }
    }

    #[test]
    fn sub_block_seconds_match_frame_ranges_and_context_is_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let mut command = request(directory.path(), 480);
        if let Command::Render {
            analysis: Some(analysis),
            ..
        } = &mut command
        {
            analysis.range = AnalysisRange::Seconds {
                start: 0.01025,
                end: 0.02025,
            };
            analysis.context_from_start = true;
        }
        let cancelled = AtomicBool::new(false);
        let result = session
            .begin_agent_file_job(command)
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let reply: Value =
            serde_json::from_str(&session.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        assert_eq!(reply["files"][0]["encoded"]["frames"], 480);
        assert_eq!(reply["provenance"]["source_start_frame"], 492);
    }
}
