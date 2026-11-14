//! Snapshot-based file operations; workers stage outputs and the owner publishes them.

use super::Command;
mod analysis;
mod peak_export;
use crate::{Session, prelude::*};
pub use analysis::{AnalysisRange, AnalysisRender};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// A complete mix or independently rendered source stems.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioDestination {
    /// One encoded WAV mix at a new path.
    Mix {
        /// New WAV file path, relative to the project workspace (renders/mix.wav) or absolute.
        path: String,
    },
    /// One WAV per source track; existing stem files are refused.
    Stems {
        /// Destination folder, relative to the project workspace (stems/v1) or absolute.
        folder: String,
    },
}

/// An explicit contiguous arrangement range.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BarRange {
    /// First one-based arrangement bar.
    pub start_bar: u32,
    /// Positive number of bars.
    pub bars: u32,
}

/// Owned, cancellable work detached from the UI and document.
pub struct FileJob {
    revision: u64,
    owner: Arc<auris_core::PluginRegistry>,
    work: Work,
}

enum Work {
    ImportAudio {
        path: PathBuf,
        folder: PathBuf,
        name: String,
        at: Ticks,
        rate: f64,
        render_rate: f64,
    },
    LoadSample {
        samples: Vec<(PathBuf, crate::session::AgentSample)>,
        folder: PathBuf,
        track: TrackId,
        append: bool,
        looped: Option<bool>,
        rate: f64,
        render_rate: f64,
    },
    ImportMidi {
        job: crate::MidiImportJob,
        path: PathBuf,
        at: Ticks,
    },
    Audio {
        job: crate::RenderJob,
        guard: crate::render::ExportGuard,
        targets: Vec<(PathBuf, Option<TrackId>)>,
        folder: Option<PathBuf>,
        options: OfflineOptions,
        settings: WavExportSettings,
        audition_root: Option<PathBuf>,
        ceiling_db: f32,
        manifest: Option<Box<analysis::Manifest>>,
        /// Whether the one file written is the song's whole mix, rather than stems, a range of
        /// bars or an analysis signal.
        whole_mix: bool,
    },
    Midi {
        job: crate::MidiExportJob,
        guard: crate::render::ExportGuard,
        path: PathBuf,
    },
    Lint {
        job: Box<crate::lint::AudioLintJob>,
        request: super::lint::LintRequest,
    },
    Verify {
        path: PathBuf,
        target_lufs: Option<f32>,
        /// The style's loudness range, which holds the file when no target was named.
        style_loudness: Option<(f32, f32)>,
        /// Whether the file is a stem or an excerpt this session rendered, which has no style
        /// level to be held to.
        partial: bool,
        ceiling_db: f32,
        tolerance_lu: f32,
        ending_rms_max_db: Option<f32>,
    },
}

/// A worker result retaining private files until its originating document accepts it.
pub struct FileResult {
    revision: u64,
    owner: Arc<auris_core::PluginRegistry>,
    outcome: Outcome,
}

enum Outcome {
    ImportedAudio {
        file: tempfile::NamedTempFile,
        prepared: crate::PreparedAudioSource,
        folder: PathBuf,
        name: String,
        at: Ticks,
        bytes: u64,
        provenance: std::collections::BTreeMap<String, String>,
    },
    LoadedSample {
        staged: Vec<(StagedSource, String, crate::session::AgentSample)>,
        folder: PathBuf,
        track: TrackId,
        append: bool,
        looped: Option<bool>,
    },
    ImportedMidi(Box<crate::MidiImportResult>, Ticks),
    /// The staged files, the analysis manifest, and whether the file is the whole mix.
    Audio(Vec<StagedAudio>, Option<Box<analysis::Manifest>>, bool),
    Midi(crate::MidiExportResult),
    Verified(String),
    Linted(Box<crate::lint::AudioLintResult>, super::lint::LintRequest),
}

struct StagedAudio {
    path: PathBuf,
    encoded: auris_io::StagedAudioExport,
    inspection: crate::ExportInspection,
    source_true_peak_db: f32,
    attenuation_db: f32,
    ceiling_db: f32,
    source_inspection: Option<crate::ExportInspection>,
}

/// An audio file copied into the project's audio folder and decoded, not yet in the document.
pub(crate) struct StagedSource {
    pub(crate) file: tempfile::NamedTempFile,
    pub(crate) prepared: crate::PreparedAudioSource,
    pub(crate) bytes: u64,
    pub(crate) provenance: std::collections::BTreeMap<String, String>,
}

/// Copies `path` into `folder`'s audio directory under a private name, hashing as it goes, and
/// decodes the copy — everything an import does that may take a while.
fn stage_audio(
    path: &std::path::Path,
    folder: &std::path::Path,
    rate: f64,
    render_rate: f64,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<StagedSource, String> {
    use sha2::Digest;
    use std::io::{Read, Write};
    let mut digest = sha2::Sha256::new();
    let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !input.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Audio input must be a regular file".into());
    }
    let directory = folder.join(auris_io::AUDIO_DIR);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut file = tempfile::Builder::new()
        .prefix("import-")
        .suffix(&format!(
            ".{}",
            path.extension().and_then(|e| e.to_str()).unwrap_or("audio")
        ))
        .tempfile_in(&directory)
        .map_err(|e| e.to_string())?;
    let mut chunk = [0u8; 65536];
    let mut bytes = 0u64;
    loop {
        check()?;
        let size = input.read(&mut chunk).map_err(|e| e.to_string())?;
        if size == 0 {
            break;
        }
        bytes += size as u64;
        if bytes > 512 * 1024 * 1024 {
            return Err("Audio input exceeds 512 MiB".into());
        }
        digest.update(&chunk[..size]);
        file.write_all(&chunk[..size]).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    let buffer = crate::decode_audio(file.path(), rate).map_err(|e| e.to_string())?;
    let prepared = crate::prepare_audio_source(buffer, render_rate).map_err(|e| e.to_string())?;
    check()?;
    Ok(StagedSource {
        file,
        prepared,
        bytes,
        provenance: std::collections::BTreeMap::from([
            ("source_path".into(), path.display().to_string()),
            ("sha256".into(), hex::encode(digest.finalize())),
        ]),
    })
}

fn local_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || value.starts_with("\\\\") || value.starts_with("//") {
        return Err("Use a path relative to the project workspace (renders/mix.wav) or an absolute local file path".into());
    }
    Ok(path)
}

/// What a file that failed on loudness is told to do. A model that verifies after the last edit
/// otherwise reads the failure as something to report rather than to fix.
const LOUDNESS_FIX: &str = "The file is not delivered yet: call normalize_mix (without target_lufs, it aims at the style's level), render to the same path again and verify again. Changing the faders this way keeps every note and sound. The style's level is that of the whole mix: for a stem, an excerpt or a reference file, name target_lufs instead.";

/// What a stem or an excerpt is told in place of a loudness verdict.
const PARTIAL_NOTE: &str = "This file is a stem, a range of bars or an analysis signal rendered in this session, so the style's loudness, which is the whole mix's, was not asked of it. Name target_lufs to hold it to a level.";

/// Whether an encoded file's loudness passes: within `tolerance` of a named target, inside the
/// style's range when none was named, and unchecked when neither says a level. A silent file
/// has no loudness and passes only where nothing was asked of it.
fn loudness_ok(
    measured: Option<f32>,
    target: Option<f32>,
    style: Option<(f32, f32)>,
    tolerance: f32,
) -> bool {
    match (target, style) {
        (Some(target), _) => measured.is_some_and(|value| (value - target).abs() <= tolerance),
        (None, Some(range)) => {
            measured.is_some_and(|value| crate::lint::off_target(value, range).is_none())
        }
        (None, None) => true,
    }
}

impl Session {
    /// An agent's file argument: relative names the project workspace, absolute anything local.
    ///
    /// The permission policy reads the same distinction from the text alone
    /// ([`Command::uses_only_workspace`]), which is why a relative path is resolved here and
    /// nowhere else: what was let through unasked must be what is opened.
    pub(crate) fn agent_path(&self, value: &str) -> Result<PathBuf, String> {
        if crate::workspace_relative(value).is_some() {
            self.resolve_workspace_path(value)
        } else {
            local_path(value)
        }
    }
}

impl FileJob {
    /// Perform expensive rendering/encoding/measurement without changing the live document.
    pub fn run(self, cancelled: &AtomicBool) -> Result<FileResult, String> {
        let check = || {
            if cancelled.load(Ordering::Relaxed) {
                Err("File operation cancelled".to_string())
            } else {
                Ok(())
            }
        };
        check()?;
        let outcome = match self.work {
            Work::Lint { job, request } => Outcome::Linted(
                Box::new(job.run(cancelled).map_err(|e| e.to_string())?),
                request,
            ),
            Work::ImportMidi { job, path, at } => {
                let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
                    return Err("MIDI input must be a regular file up to 32 MiB".into());
                }
                Outcome::ImportedMidi(
                    Box::new(
                        job.run(cancelled)
                            .map_err(|e| e.to_string())?
                            .ok_or("MIDI import cancelled")?,
                    ),
                    at,
                )
            }
            Work::ImportAudio {
                path,
                folder,
                name,
                at,
                rate,
                render_rate,
            } => {
                let staged = stage_audio(&path, &folder, rate, render_rate, &check)?;
                Outcome::ImportedAudio {
                    file: staged.file,
                    prepared: staged.prepared,
                    folder,
                    name,
                    at,
                    bytes: staged.bytes,
                    provenance: staged.provenance,
                }
            }
            Work::LoadSample {
                samples,
                folder,
                track,
                append,
                looped,
                rate,
                render_rate,
            } => {
                let mut staged = Vec::with_capacity(samples.len());
                for (path, request) in samples {
                    let source = stage_audio(&path, &folder, rate, render_rate, &check)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    let name = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    staged.push((source, name, request));
                }
                Outcome::LoadedSample {
                    staged,
                    folder,
                    track,
                    append,
                    looped,
                }
            }
            Work::Audio {
                mut job,
                guard,
                targets,
                folder,
                options,
                settings,
                audition_root,
                ceiling_db,
                mut manifest,
                whole_mix,
            } => {
                if let Some(folder) = &folder {
                    guard
                        .check_stems(
                            folder,
                            &targets
                                .iter()
                                .map(|(path, _)| path.clone())
                                .collect::<Vec<_>>(),
                        )
                        .map_err(|e| e.to_string())?;
                    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
                }
                for (path, _) in &targets {
                    check()?;
                    guard.check_file(path).map_err(|e| e.to_string())?;
                    if path.exists() {
                        return Err(format!("Output already exists: {}", path.display()));
                    }
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    if let Some(root) = &audition_root {
                        let directory = path.parent().ok_or("Missing preview directory")?;
                        if !directory
                            .canonicalize()
                            .map_err(|e| e.to_string())?
                            .starts_with(root.canonicalize().map_err(|e| e.to_string())?)
                        {
                            return Err(
                                "Preview directory must stay inside the project folder".into()
                            );
                        }
                    }
                }
                let staged = if let Some(manifest) = &mut manifest {
                    manifest.freeze_dependencies()?;
                    manifest.value["loaded_audio"] = job.audio_content_identity();
                    manifest.check_dependencies()?;
                    job.validate_complete().map_err(|e| e.to_string())?;
                    analysis::stage(
                        &mut job,
                        targets,
                        &options,
                        manifest.context_from_start,
                        cancelled,
                    )?
                } else {
                    peak_export::stage(
                        &mut job, targets, &options, settings, ceiling_db, cancelled,
                    )?
                };
                Outcome::Audio(staged, manifest, whole_mix)
            }
            Work::Midi { job, guard, path } => {
                guard.check_file(&path).map_err(|e| e.to_string())?;
                Outcome::Midi(
                    job.run(cancelled)
                        .map_err(|e| e.to_string())?
                        .ok_or("MIDI export cancelled")?,
                )
            }
            Work::Verify {
                path,
                target_lufs,
                style_loudness,
                partial,
                ceiling_db,
                tolerance_lu,
                ending_rms_max_db,
            } => {
                let encoded = crate::inspect_export(&path).map_err(|e| e.to_string())?;
                check()?;
                let loudness_ok =
                    loudness_ok(encoded.lufs, target_lufs, style_loudness, tolerance_lu);
                let peak_ok = encoded.true_peak_db <= ceiling_db && encoded.full_scale_samples == 0;
                let ending_ok =
                    ending_rms_max_db.is_none_or(|limit| encoded.ending_rms_db <= limit);
                let held_to = match (target_lufs, style_loudness) {
                    (Some(target), _) => {
                        serde_json::json!({"target_lufs":target,"tolerance_lu":tolerance_lu})
                    }
                    (None, Some((low, high))) => {
                        serde_json::json!({"style_range_lufs":[low, high]})
                    }
                    (None, None) => serde_json::Value::Null,
                };
                let note = (partial && target_lufs.is_none()).then_some(PARTIAL_NOTE);
                Outcome::Verified(serde_json::json!({"path":path,"verdict":if loudness_ok&&peak_ok&&ending_ok {"PASS"}else{"FAIL"},"loudness_ok":loudness_ok,"loudness_held_to":held_to,"loudness_note":note,"next":(!loudness_ok).then_some(LOUDNESS_FIX),"peak_ok":peak_ok,"ending_ok":ending_ok,"encoded":encoded}).to_string())
            }
        };
        check()?;
        Ok(FileResult {
            revision: self.revision,
            owner: self.owner,
            outcome,
        })
    }
}

impl Session {
    /// Capture an immutable document snapshot for audio/MIDI export or file verification.
    pub fn begin_agent_file_job(&mut self, command: Command) -> Result<FileJob, String> {
        let owner = Arc::clone(self.registry());
        let work = match command {
            Command::ImportAudio {
                path,
                at_tick,
                name,
            } => {
                if at_tick < 0
                    || name
                        .as_ref()
                        .is_some_and(|n| n.trim().is_empty() || n.len() > 256)
                {
                    return Err(
                        "Use nonnegative at_tick and a nonempty name up to 256 bytes".into(),
                    );
                }
                let path = self.agent_path(&path)?;
                let name = name.unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
                Work::ImportAudio {
                    path,
                    folder: self
                        .project_folder()
                        .ok_or("Missing project folder")?
                        .to_owned(),
                    name,
                    at: Ticks(at_tick),
                    rate: self.project().sample_rate,
                    render_rate: self.sample_rate(),
                }
            }
            Command::LoadSample {
                track,
                samples,
                append,
                looped,
            } => {
                let track = TrackId(track);
                self.require_sampler_target(track)
                    .map_err(|e| e.to_string())?;
                if samples.is_empty() || samples.len() > 128 {
                    return Err("Give 1..128 samples".into());
                }
                if samples
                    .iter()
                    .any(|sample| sample.root_note.is_some_and(|note| note > 127))
                {
                    return Err("root_note must be a MIDI note 0..127 (60 is C4)".into());
                }
                let samples = samples
                    .into_iter()
                    .map(|sample| Ok((self.agent_path(&sample.path)?, sample)))
                    .collect::<Result<Vec<_>, String>>()?;
                Work::LoadSample {
                    samples,
                    folder: self
                        .project_folder()
                        .ok_or("Missing project folder")?
                        .to_owned(),
                    track,
                    append: append.unwrap_or(false),
                    looped,
                    rate: self.project().sample_rate,
                    render_rate: self.sample_rate(),
                }
            }
            Command::ImportMidi { path, at_tick } => {
                if at_tick < 0 {
                    return Err("at_tick must be nonnegative".into());
                }
                let path = self.agent_path(&path)?;
                Work::ImportMidi {
                    job: self.begin_midi_import(&path).map_err(|e| e.to_string())?,
                    path,
                    at: Ticks(at_tick),
                }
            }
            Command::Render {
                destination,
                range,
                bit_depth,
                include_tail,
                ceiling_db,
                analysis,
            } => {
                let explicit_ceiling = ceiling_db.is_some();
                let ceiling_db = ceiling_db.unwrap_or(-1.0);
                let whole_song = range.is_none() && analysis.is_none();
                if !ceiling_db.is_finite() || !(-12.0..=-0.1).contains(&ceiling_db) {
                    return Err("ceiling_db must be finite within -12..-0.1 dBTP".into());
                }
                if analysis.is_some()
                    && (range.is_some()
                        || include_tail
                        || explicit_ceiling
                        || bit_depth.is_some_and(|bits| bits != 32))
                {
                    return Err("Analysis has its own precise range, no appended tail, no ceiling and float32 encoding; omit range/include_tail/ceiling_db/bit_depth".into());
                }
                let options = match &analysis {
                    Some(request) => request.options()?,
                    None => self.agent_export_range(range, include_tail)?,
                };
                let mut render_project = if analysis.is_some() {
                    self.analysis_render_project().map_err(|e| e.to_string())?
                } else {
                    self.project().clone()
                };
                let source_snapshot_sha256 = if analysis.is_some() {
                    use sha2::{Digest, Sha256};
                    hex::encode(Sha256::digest(
                        serde_json::to_vec(&render_project).map_err(|e| e.to_string())?,
                    ))
                } else {
                    String::new()
                };
                if let Some(request) = &analysis {
                    request.select_signal(&mut render_project)?;
                }
                let mut manifest = analysis
                    .as_ref()
                    .map(|request| {
                        self.analysis_manifest(request, &options, &render_project)
                            .map(Box::new)
                    })
                    .transpose()?;
                if let Some(manifest) = &mut manifest {
                    manifest.value["source_snapshot_sha256"] = source_snapshot_sha256.into();
                }
                let settings = WavExportSettings {
                    bit_depth: match bit_depth.unwrap_or(24) {
                        16 => WavBitDepth::Int16,
                        24 => WavBitDepth::Int24,
                        32 => WavBitDepth::Float32,
                        _ => return Err("bit_depth must be 16,24 or 32".into()),
                    },
                    ..Default::default()
                };
                let job = self.job_for_at(render_project, options.sample_rate);
                let (mut targets, folder) = match destination {
                    AudioDestination::Mix { path } => (vec![(self.agent_path(&path)?, None)], None),
                    AudioDestination::Stems { folder } => {
                        let folder = self.agent_path(&folder)?;
                        let tracks = job.stem_tracks();
                        if tracks.is_empty() {
                            return Err("No source tracks to export".into());
                        }
                        let paths = job.stem_paths(&folder, auris_io::AudioExportFormat::Wav);
                        (
                            paths
                                .into_iter()
                                .zip(tracks)
                                .map(|(path, (track, _))| (path, Some(track)))
                                .collect(),
                            Some(folder),
                        )
                    }
                };
                if let Some(request) = &analysis {
                    if let Some(track) = request.track {
                        if folder.is_some() || self.project().track(TrackId(track)).is_none() {
                            return Err("Analysis track requires a mix destination and an existing track or bus".into());
                        }
                        targets[0].1 = Some(TrackId(track));
                    }
                    let path = self.agent_path(&request.manifest_path)?;
                    self.export_guard()
                        .check_file(&path)
                        .map_err(|e| e.to_string())?;
                    if path.exists() || targets.iter().any(|(target, _)| *target == path) {
                        return Err(
                            "Manifest must be a new file distinct from every audio output".into(),
                        );
                    }
                }
                Work::Audio {
                    job,
                    guard: self.export_guard(),
                    targets,
                    whole_mix: whole_song && folder.is_none(),
                    folder,
                    options,
                    settings,
                    audition_root: None,
                    ceiling_db,
                    manifest,
                }
            }
            Command::Preview { range } => {
                let mut options = self.agent_export_range(Some(range), false)?;
                let duration = options
                    .end_frames
                    .unwrap_or(0)
                    .saturating_sub(options.start_frames) as f64
                    / self.project().sample_rate;
                if duration > 120.0 {
                    return Err("Preview must fit within 120 seconds".into());
                }
                // Keep the time range exact when changing the audition rate.
                let scale = 24_000.0 / self.project().sample_rate;
                options.start_frames = (options.start_frames as f64 * scale).round() as u64;
                options.end_frames = options
                    .end_frames
                    .map(|end| (end as f64 * scale).round() as u64);
                options.sample_rate = Some(24_000.0);
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                let path = self
                    .project_folder()
                    .ok_or("Missing document working folder")?
                    .join(".auris-previews")
                    .join(format!(
                        "preview-{}-{}.wav",
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::Relaxed)
                    ));
                Work::Audio {
                    job: self.render_job_at(options.sample_rate),
                    guard: self.export_guard(),
                    targets: vec![(path, None)],
                    folder: None,
                    options,
                    settings: WavExportSettings {
                        sample_rate: 24_000,
                        bit_depth: WavBitDepth::Int16,
                        ..Default::default()
                    },
                    audition_root: self.project_folder().map(std::path::Path::to_owned),
                    ceiling_db: -1.0,
                    manifest: None,
                    whole_mix: false,
                }
            }
            Command::ExportMidi { output } => {
                let path = self.agent_path(&output)?;
                Work::Midi {
                    job: self.begin_midi_export(&path),
                    guard: self.export_guard(),
                    path,
                }
            }
            Command::VerifyRender {
                path,
                target_lufs,
                ceiling_db,
                tolerance_lu,
                ending_rms_max_db,
            } => {
                let ceiling_db = ceiling_db.unwrap_or(-1.0);
                let tolerance_lu = tolerance_lu.unwrap_or(1.0);
                if !(-12.0..=0.0).contains(&ceiling_db)
                    || !(0.0..=12.0).contains(&tolerance_lu)
                    || target_lufs.is_some_and(|v| !(-60.0..=-6.0).contains(&v))
                    || ending_rms_max_db.is_some_and(|v| !(-120.0..=0.0).contains(&v))
                {
                    return Err("Invalid delivery target, tolerance or ending limit".into());
                }
                let path = self.agent_path(&path)?;
                let partial = self.partial_renders.contains(&path);
                Work::Verify {
                    target_lufs,
                    style_loudness: crate::session::style_checks::style_checks(
                        self.project().style,
                    )
                    .loudness
                    .filter(|_| !partial),
                    partial,
                    path,
                    ceiling_db,
                    tolerance_lu,
                    ending_rms_max_db,
                }
            }
            Command::LintProject {
                audio: true,
                start_bar,
                bars,
                offset,
                limit,
            } => Work::Lint {
                request: super::lint::LintRequest::new(start_bar, bars, offset, limit)?,
                job: Box::new(self.audio_lint_job()),
            },
            _ => return Err("Expected render, preview, export_midi or verify_render".into()),
        };
        Ok(FileJob {
            revision: self.revision(),
            owner,
            work,
        })
    }

    fn agent_export_range(
        &self,
        range: Option<BarRange>,
        include_tail: bool,
    ) -> Result<OfflineOptions, String> {
        let Some(range) = range else {
            return Ok(OfflineOptions {
                include_tail,
                ..Default::default()
            });
        };
        if range.start_bar == 0 || range.bars == 0 || range.bars > 4096 {
            return Err("Use start_bar>=1 and bars within 1..4096".into());
        }
        let end_bar = range
            .start_bar
            .checked_add(range.bars)
            .ok_or("Bar range overflow")?;
        let from = self.project().signatures.bar_start(range.start_bar);
        let to = self.project().signatures.bar_start(end_bar);
        self.render_range_options(from, to, include_tail)
            .ok_or("Export range must lie inside the arrangement".into())
    }

    /// Publish only a still-current, uncancelled worker result; stale stages are deleted on drop.
    pub fn continue_agent_file_job(
        &mut self,
        result: FileResult,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        if cancelled.load(Ordering::Relaxed)
            || self.revision() != result.revision
            || !Arc::ptr_eq(self.registry(), &result.owner)
        {
            return Err(
                "Document changed or export cancelled; no staged output was published".into(),
            );
        }
        match result.outcome {
            Outcome::ImportedMidi(result, at) => self.append_agent_midi(*result, at),
            Outcome::ImportedAudio {
                file,
                prepared,
                folder,
                name,
                at,
                bytes,
                provenance,
            } => {
                if self.project_folder() != Some(folder.as_path()) {
                    return Err("Document folder/rate changed during audio import".into());
                }
                self.adopt_agent_audio(file, prepared, name, at, bytes, provenance)
            }
            Outcome::LoadedSample {
                staged,
                folder,
                track,
                append,
                looped,
            } => {
                if self.project_folder() != Some(folder.as_path()) {
                    return Err("Document folder/rate changed while loading the samples".into());
                }
                self.adopt_agent_samples(staged, track, append, looped)
            }
            Outcome::Verified(text) => Ok(text),
            Outcome::Linted(result, request) => {
                self.lint_audio = Some(*result);
                self.agent_lint_report(&request)
            }
            Outcome::Midi(result) => {
                let report = self
                    .continue_midi_export(result, cancelled)
                    .map_err(|e| e.to_string())?
                    .ok_or("MIDI export became stale")?;
                Ok(serde_json::json!({
                    "workspace_path": self.workspace_path_of(&report.path),
                    "path": report.path,
                    "notes": report.notes,
                    "duration_ticks": report.duration_ticks,
                    "ticks_per_quarter": report.ticks_per_quarter,
                    "duration_seconds": report.duration_seconds,
                    "document_unchanged": true
                })
                .to_string())
            }
            Outcome::Audio(staged, manifest, whole_mix) => {
                let staged_manifest = if let Some(manifest) = &manifest {
                    use sha2::{Digest, Sha256};
                    use std::io::Write;
                    let current = self.analysis_render_project().map_err(|e| e.to_string())?;
                    let current = hex::encode(Sha256::digest(
                        serde_json::to_vec(&current).map_err(|e| e.to_string())?,
                    ));
                    if manifest.value["source_snapshot_sha256"] != current {
                        return Err("Hosted source state changed during analysis render; no output was published".into());
                    }
                    manifest.check_dependencies()?;
                    self.export_guard()
                        .check_file(&manifest.path)
                        .map_err(|e| e.to_string())?;
                    if manifest.path.exists() {
                        return Err("Manifest already exists".into());
                    }
                    let parent = manifest.path.parent().ok_or("Missing manifest parent")?;
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    let mut file =
                        tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
                    let mut value = manifest.value.clone();
                    value["files"] = serde_json::json!(staged.iter().map(|file|serde_json::json!({"path":file.path,"encoded":file.inspection,"source":file.source_inspection,"gain_db":0.0})).collect::<Vec<_>>());
                    serde_json::to_writer_pretty(&mut file, &value).map_err(|e| e.to_string())?;
                    file.flush().map_err(|e| e.to_string())?;
                    Some(file)
                } else {
                    None
                };
                for StagedAudio { path, .. } in &staged {
                    self.export_guard()
                        .check_file(path)
                        .map_err(|e| e.to_string())?;
                    if path.exists() {
                        return Err(format!("Output already exists: {}", path.display()));
                    }
                }
                let mut files = Vec::with_capacity(staged.len());
                for StagedAudio {
                    path,
                    encoded,
                    inspection,
                    source_true_peak_db,
                    attenuation_db,
                    ceiling_db,
                    source_inspection,
                } in staged
                {
                    encoded.publish_noclobber().map_err(|e| {
                        format!("Publication failed: {e}; already published: {files:?}")
                    })?;
                    files.push(serde_json::json!({"path":path,"workspace_path":self.workspace_path_of(&path),"encoded":inspection,"attenuation_db":attenuation_db,"source_true_peak_db":source_true_peak_db,"ceiling_db":if manifest.is_some(){None}else{Some(ceiling_db)},"source":source_inspection}));
                    // A path is whatever was last written to it: the whole mix rendered over an
                    // excerpt is the whole mix.
                    if whole_mix {
                        self.partial_renders.remove(&path);
                    } else {
                        self.partial_renders.insert(path);
                    }
                }
                if let (Some(file), Some(manifest)) = (staged_manifest, &manifest) {
                    file.persist_noclobber(&manifest.path).map_err(|e| {
                        format!("Manifest publication failed: {e}; published: {files:?}")
                    })?;
                }
                self.trace_published_files(&files);
                Ok(serde_json::json!({"files":files,"manifest_path":manifest.as_ref().map(|m|&m.path),"provenance":manifest.map(|m|m.value),"document_unchanged":true}).to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_without_a_named_target_is_held_to_its_styles_range() {
        let neo_soul = Some((-15.0, -13.0));
        assert!(!loudness_ok(Some(-23.9), None, neo_soul, 1.0));
        assert!(loudness_ok(Some(-14.2), None, neo_soul, 1.0));
        // A named target wins over the style, and no level at all checks nothing.
        assert!(loudness_ok(Some(-23.9), Some(-24.0), neo_soul, 1.0));
        assert!(loudness_ok(Some(-23.9), None, None, 1.0));
        assert!(!loudness_ok(None, None, neo_soul, 1.0));
    }

    #[test]
    fn overloaded_mix_is_attenuated_before_integer_encoding_without_document_edits() {
        let directory = tempfile::tempdir().unwrap();
        let mut live = fixture();
        let track = live.project().tracks[0].id;
        live.set_track_instrument(track, "auris.synth.fm2").unwrap();
        live.set_param(crate::ParamTarget::TrackGain(track), 30.0);
        let before = live.project().clone();
        let cancelled = AtomicBool::new(false);
        let raw = live
            .render_job()
            .render(&OfflineOptions::default(), &mut RenderProgress::default())
            .unwrap();
        assert!(raw.peak() > 1.0);
        for bits in [16, 24, 32] {
            let path = directory.path().join(format!("mix-{bits}.wav"));
            let mut command = render(&path);
            if let Command::Render { bit_depth, .. } = &mut command {
                *bit_depth = Some(bits);
            }
            let result = live
                .begin_agent_file_job(command)
                .unwrap()
                .run(&cancelled)
                .unwrap();
            let response: serde_json::Value =
                serde_json::from_str(&live.continue_agent_file_job(result, &cancelled).unwrap())
                    .unwrap();
            let file = &response["files"][0];
            let gain =
                auris_core::param::db_to_gain(file["attenuation_db"].as_f64().unwrap() as f32);
            assert!(gain < 1.0);
            assert!(file["source_true_peak_db"].as_f64().unwrap() > 0.0);
            let actual = crate::inspect_export(&path).unwrap();
            assert_eq!(actual.full_scale_samples, 0, "{bits}-bit export clips");
            assert!(actual.true_peak_db <= -1.0, "{actual:?}");
            assert_eq!(actual.frames as usize, raw.frame_count());
            let decoded = auris_io::decode_audio_file(&path).unwrap().buffer;
            let tolerance = if bits == 16 { 1.0 / 32768.0 } else { 1.0e-6 };
            for (source, output) in raw
                .iter_channels()
                .flatten()
                .zip(decoded.iter_channels().flatten())
            {
                assert!(
                    (source * gain - output).abs() < tolerance,
                    "export must be linear attenuation, not clipping"
                );
            }
            assert_eq!(live.project(), &before);
        }
    }

    #[test]
    fn quiet_float_export_is_sample_exact_and_stems_share_one_attenuation() {
        let directory = tempfile::tempdir().unwrap();
        let mut live = fixture();
        let track = live.project().tracks[0].id;
        live.set_track_instrument(track, "auris.synth.fm2").unwrap();
        let cancelled = AtomicBool::new(false);
        let raw = live
            .render_job()
            .render(&OfflineOptions::default(), &mut RenderProgress::default())
            .unwrap();
        assert!(auris_gpu::analysis::analyze_loudness_cpu(&raw).true_peak_db() < -1.0);
        let path = directory.path().join("quiet.wav");
        let mut command = render(&path);
        if let Command::Render { bit_depth, .. } = &mut command {
            *bit_depth = Some(32);
        }
        let result = live
            .begin_agent_file_job(command)
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&live.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        assert_eq!(response["files"][0]["attenuation_db"], 0.0);
        assert_eq!(auris_io::decode_audio_file(&path).unwrap().buffer, raw);

        let loud = live.add_default_instrument_track("Loud").unwrap();
        live.set_track_instrument(loud, "auris.synth.fm2").unwrap();
        let clip = live
            .add_midi_clip(loud, "Loud", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        live.replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER * 2)])
            .unwrap();
        live.set_param(crate::ParamTarget::TrackGain(loud), 30.0);
        let before = live.project().clone();
        let result = live
            .begin_agent_file_job(Command::Render {
                destination: AudioDestination::Stems {
                    folder: directory
                        .path()
                        .join("stems")
                        .to_string_lossy()
                        .into_owned(),
                },
                range: None,
                bit_depth: Some(24),
                include_tail: false,
                ceiling_db: Some(-3.0),
                analysis: None,
            })
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&live.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        let files = response["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["attenuation_db"], files[1]["attenuation_db"]);
        assert!(files[0]["attenuation_db"].as_f64().unwrap() < 0.0);
        for file in files {
            assert!(file["encoded"]["true_peak_db"].as_f64().unwrap() <= -3.0);
            assert_eq!(file["encoded"]["full_scale_samples"], 0);
        }
        assert_eq!(live.project(), &before);
        for ceiling in [0.0, -0.05, -13.0, f32::NAN, f32::INFINITY] {
            let mut command = render(&directory.path().join("invalid.wav"));
            if let Command::Render { ceiling_db, .. } = &mut command {
                *ceiling_db = Some(ceiling);
            }
            assert!(live.begin_agent_file_job(command).is_err());
        }
    }

    #[test]
    #[ignore = "requires AURIS_CLIPPING_REPLAY pointing to a retained synthetic project snapshot"]
    fn retained_composition_export_preserves_headroom() {
        let path = std::env::var_os("AURIS_CLIPPING_REPLAY").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let copy = directory.path().join("Replayed.auris");
        std::fs::copy(path, &copy).unwrap();
        let mut live = Session::new(crate::SessionOptions::headless()).unwrap();
        assert!(live.open(&copy).unwrap().is_empty());
        let before = live.project().clone();
        let cancelled = AtomicBool::new(false);
        let output = std::env::var_os("AURIS_CLIPPING_REPLAY_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|| directory.path().join("replayed.wav"));
        let mut command = render(&output);
        if let Command::Render { range, .. } = &mut command {
            *range = Some(BarRange {
                start_bar: 1,
                bars: 16,
            });
        }
        let result = live
            .begin_agent_file_job(command)
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let response = live.continue_agent_file_job(result, &cancelled).unwrap();
        println!("{response}");
        let actual = crate::inspect_export(&output).unwrap();
        assert_eq!(actual.full_scale_samples, 0);
        assert!(actual.true_peak_db <= -1.0);
        assert_eq!(actual.frames, 1_645_714);
        assert_eq!(live.project(), &before);
    }

    #[test]
    fn overloaded_preview_attenuates_only_the_audition_and_preserves_the_document() {
        let mut live = fixture();
        let track = live.project().tracks[0].id;
        live.set_track_instrument(track, "auris.synth.fm2").unwrap();
        live.set_param(crate::ParamTarget::TrackGain(track), 30.0);
        let original = live.project().clone();
        let cancelled = AtomicBool::new(false);
        let job = live
            .begin_agent_file_job(Command::Preview {
                range: BarRange {
                    start_bar: 1,
                    bars: 1,
                },
            })
            .unwrap();
        let result = job.run(&cancelled).unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&live.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        let file = &response["files"][0];
        assert!(file["attenuation_db"].as_f64().unwrap() < 0.0);
        assert_eq!(file["encoded"]["sample_rate"], 24000);
        assert_eq!(file["encoded"]["full_scale_samples"], 0);
        assert!(file["encoded"]["true_peak_db"].as_f64().unwrap() <= -0.99);
        assert_eq!(live.project(), &original);
    }

    #[test]
    fn audio_and_midi_import_append_without_replacing_music_and_undo_once() {
        let directory = tempfile::tempdir().unwrap();
        let mut live = fixture();
        let midi = directory.path().join("source.mid");
        live.export_midi(&midi).unwrap();
        let original = live.project().clone();
        let cancelled = AtomicBool::new(false);
        let job = live
            .begin_agent_file_job(Command::ImportMidi {
                path: midi.to_string_lossy().into_owned(),
                at_tick: 997,
            })
            .unwrap();
        let result = job.run(&cancelled).unwrap();
        assert_eq!(live.project(), &original);
        live.continue_agent_file_job(result, &cancelled).unwrap();
        assert_eq!(live.project().tracks.len(), original.tracks.len() + 1);
        assert_eq!(live.project().tempo_map, original.tempo_map);
        assert_eq!(live.project().tracks[0], original.tracks[0]);
        let added = live
            .project()
            .tracks
            .last()
            .unwrap()
            .kind
            .as_instrument()
            .unwrap();
        assert_eq!(added.clips[0].start, Ticks(997));
        live.undo();
        assert_eq!(live.project(), &original);

        let audio = directory.path().join("source.wav");
        let mut writer = hound::WavWriter::create(
            &audio,
            hound::WavSpec {
                channels: 1,
                sample_rate: 24_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..2400 {
            writer.write_sample::<i16>(1234).unwrap();
        }
        writer.finalize().unwrap();
        let job = live
            .begin_agent_file_job(Command::ImportAudio {
                path: audio.to_string_lossy().into_owned(),
                at_tick: 13,
                name: Some("Recording".into()),
            })
            .unwrap();
        let result = job.run(&cancelled).unwrap();
        assert_eq!(live.project(), &original);
        live.continue_agent_file_job(result, &cancelled).unwrap();
        let source = live.project().audio_sources.values().next().unwrap();
        let stored = source.path.resolve(live.project_folder()).unwrap();
        assert_eq!(
            std::fs::read(&audio).unwrap(),
            std::fs::read(&stored).unwrap()
        );
        assert!(source.path.is_inside());
        let added = live.project().tracks.last().unwrap();
        assert_eq!(added.name, "Recording");
        assert_eq!(added.kind.as_audio().unwrap().clips[0].start, Ticks(13));
        live.undo();
        assert_eq!(live.project(), &original);
    }

    #[test]
    fn cancelled_and_stale_audio_imports_discard_private_source_copies() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.wav");
        let mut writer = hound::WavWriter::create(
            &source,
            hound::WavSpec {
                channels: 1,
                sample_rate: 24_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..240 {
            writer.write_sample::<i16>(1).unwrap();
        }
        writer.finalize().unwrap();
        let mut live = fixture();
        let cancelled = AtomicBool::new(false);
        for cancel in [false, true] {
            let job = live
                .begin_agent_file_job(Command::ImportAudio {
                    path: source.to_string_lossy().into_owned(),
                    at_tick: 0,
                    name: None,
                })
                .unwrap();
            let result = job.run(&cancelled).unwrap();
            let Outcome::ImportedAudio { file, .. } = &result.outcome else {
                panic!("audio")
            };
            let private = file.path().to_owned();
            if cancel {
                cancelled.store(true, Ordering::Relaxed);
            } else {
                live.add_bus_track("Concurrent edit");
            }
            let before = live.project().clone();
            assert!(live.continue_agent_file_job(result, &cancelled).is_err());
            assert!(!private.exists());
            assert_eq!(live.project(), &before);
        }
        assert!(source.exists());
    }

    fn fixture() -> Session {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER * 2)])
            .unwrap();
        session
    }

    fn render(path: &std::path::Path) -> Command {
        Command::Render {
            destination: AudioDestination::Mix {
                path: path.to_string_lossy().into(),
            },
            range: None,
            bit_depth: Some(24),
            include_tail: false,
            ceiling_db: None,
            analysis: None,
        }
    }

    #[test]
    fn a_relative_render_lands_in_the_workspace_and_a_sample_loads_back_from_it() {
        let mut live = fixture();
        let cancelled = AtomicBool::new(false);
        let run = |live: &mut Session, command: Command| {
            let result = live
                .begin_agent_file_job(command)
                .unwrap()
                .run(&cancelled)
                .unwrap();
            live.continue_agent_file_job(result, &cancelled).unwrap()
        };
        let response: serde_json::Value = serde_json::from_str(&run(
            &mut live,
            render(std::path::Path::new("renders/mix.wav")),
        ))
        .unwrap();
        assert_eq!(response["files"][0]["workspace_path"], "renders/mix.wav");
        let rendered = live.workspace_folder().join("renders").join("mix.wav");
        assert!(rendered.is_file());
        // An escape is refused before anything is rendered.
        assert!(
            live.begin_agent_file_job(render(std::path::Path::new("../mix.wav")))
                .is_err()
        );
        // The bounce plays back as an instrument, copied into Audio/ like any sample.
        let track = live.project().tracks[0].id;
        run(
            &mut live,
            Command::LoadSample {
                track: track.0,
                samples: vec![crate::session::AgentSample {
                    path: "renders/mix.wav".into(),
                    root_note: Some(60),
                    low_key: None,
                    high_key: None,
                    low_velocity: None,
                    high_velocity: None,
                    tune: None,
                    gain_db: None,
                }],
                append: None,
                looped: None,
            },
        );
        let source = live.project().audio_sources.values().next().unwrap();
        assert!(source.path.is_inside());
        assert!(
            source
                .path
                .resolve(live.project_folder())
                .unwrap()
                .starts_with(live.project_folder().unwrap().join(auris_io::AUDIO_DIR))
        );
    }

    #[test]
    fn a_stem_is_not_held_to_the_loudness_of_the_whole_mix() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = fixture();
        // One soft phrase is nowhere near a dance mix, so the mix fails and says why.
        session.set_style(auris_core::style::Style {
            genre: Some(auris_core::style::Genre::DanceElectronic),
            usage: None,
        });
        let cancelled = AtomicBool::new(false);
        let mut run = |command: Command| -> serde_json::Value {
            let result = session
                .begin_agent_file_job(command)
                .unwrap()
                .run(&cancelled)
                .unwrap();
            serde_json::from_str(&session.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap()
        };
        let verify = |path: String| Command::VerifyRender {
            path,
            target_lufs: None,
            ceiling_db: None,
            tolerance_lu: None,
            ending_rms_max_db: None,
        };
        let mix = directory.path().join("mix.wav");
        run(render(&mix));
        let stems = run(Command::Render {
            destination: AudioDestination::Stems {
                folder: directory.path().join("stems").to_string_lossy().into(),
            },
            range: None,
            bit_depth: Some(24),
            include_tail: false,
            ceiling_db: None,
            analysis: None,
        });
        let stem = stems["files"][0]["path"].as_str().unwrap().to_owned();

        let whole = run(verify(mix.to_string_lossy().into()));
        assert_eq!(whole["loudness_ok"], false);
        assert_eq!(
            whole["loudness_held_to"]["style_range_lufs"],
            serde_json::json!([-11.0, -9.0])
        );
        assert!(whole["loudness_note"].is_null());

        let part = run(verify(stem.clone()));
        assert_eq!(part["loudness_ok"], true, "{part}");
        assert!(part["loudness_held_to"].is_null());
        assert!(part["loudness_note"].as_str().unwrap().contains("stem"));
        assert!(part["next"].is_null());
        // A level asked of it by name is still asked.
        let named = run(Command::VerifyRender {
            path: stem,
            target_lufs: Some(-10.0),
            ceiling_db: None,
            tolerance_lu: None,
            ending_rms_max_db: None,
        });
        assert_eq!(named["loudness_ok"], false);
        assert!(named["loudness_note"].is_null());
    }

    #[test]
    fn unsaved_mix_is_staged_measured_then_published_without_editing_the_score() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mix.wav");
        let mut session = fixture();
        let before = session.project().clone();
        let cancelled = AtomicBool::new(false);
        let job = session.begin_agent_file_job(render(&path)).unwrap();
        let result = job.run(&cancelled).unwrap();
        assert!(!path.exists());
        let text = session.continue_agent_file_job(result, &cancelled).unwrap();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            report["files"][0]["encoded"]["sample_rate"],
            session.project().sample_rate as u32
        );
        assert_eq!(report["files"][0]["encoded"]["bit_depth"], 24);
        assert!(report["files"][0]["encoded"]["frames"].as_u64().unwrap() > 0);
        let actual = crate::inspect_export(&path).unwrap();
        assert_eq!(report["files"][0]["encoded"]["sha256"], actual.sha256);
        assert_eq!(session.project(), &before);
        assert!(session.path().is_none());
        let verify = session
            .begin_agent_file_job(Command::VerifyRender {
                path: path.to_string_lossy().into(),
                target_lufs: Some(-6.0),
                ceiling_db: Some(0.0),
                tolerance_lu: Some(0.0),
                ending_rms_max_db: None,
            })
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&session.continue_agent_file_job(verify, &cancelled).unwrap())
                .unwrap();
        assert_eq!(report["verdict"], "FAIL");
        // A loudness failure says how to repair it, and what the file was held to.
        assert_eq!(report["loudness_ok"], false);
        assert!(report["next"].as_str().unwrap().contains("normalize_mix"));
        assert_eq!(report["loudness_held_to"]["target_lufs"], -6.0);
    }

    #[test]
    fn stale_cancelled_or_raced_exports_preserve_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        for mode in ["stale", "cancelled", "raced"] {
            let path = directory.path().join(format!("{mode}.wav"));
            let mut session = fixture();
            let cancelled = AtomicBool::new(false);
            let result = session
                .begin_agent_file_job(render(&path))
                .unwrap()
                .run(&cancelled)
                .unwrap();
            match mode {
                "stale" => {
                    session.add_bus_track("Changed");
                }
                "cancelled" => cancelled.store(true, Ordering::Relaxed),
                _ => std::fs::write(&path, b"another writer's bytes").unwrap(),
            }
            assert!(session.continue_agent_file_job(result, &cancelled).is_err());
            if mode == "raced" {
                assert_eq!(std::fs::read(&path).unwrap(), b"another writer's bytes");
            } else {
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn midi_and_stems_use_the_unsaved_snapshot_and_preview_uses_24khz() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = fixture();
        let before = session.project().clone();
        let cancelled = AtomicBool::new(false);
        let midi = directory.path().join("score.mid");
        let result = session
            .begin_agent_file_job(Command::ExportMidi {
                output: midi.to_string_lossy().into(),
            })
            .unwrap()
            .run(&cancelled)
            .unwrap();
        assert!(!midi.exists());
        let report: serde_json::Value =
            serde_json::from_str(&session.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        assert_eq!(report["notes"], 1);
        assert_eq!(report["duration_ticks"], 3840);
        assert_eq!(report["ticks_per_quarter"], 960);
        assert_eq!(report["duration_seconds"], 2.0);
        assert!(std::fs::read(&midi).unwrap().starts_with(b"MThd"));
        let result = session
            .begin_agent_file_job(Command::Preview {
                range: BarRange {
                    start_bar: 1,
                    bars: 1,
                },
            })
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&session.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        assert_eq!(report["files"][0]["encoded"]["sample_rate"], 24000);
        let result = session
            .begin_agent_file_job(Command::Render {
                destination: AudioDestination::Stems {
                    folder: directory.path().join("stems").to_string_lossy().into(),
                },
                range: None,
                bit_depth: None,
                include_tail: false,
                ceiling_db: None,
                analysis: None,
            })
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&session.continue_agent_file_job(result, &cancelled).unwrap())
                .unwrap();
        assert_eq!(report["files"].as_array().unwrap().len(), 1);
        assert_eq!(session.project(), &before);
    }
}
