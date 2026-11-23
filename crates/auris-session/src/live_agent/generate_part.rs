//! Experimental: one part of a song from a local music generation model.
//!
//! The model is not Auris's. A Python program outside the workspace
//! (`tools/genmodel/genpart.py` in a source checkout) runs it, and this module only hands
//! that program the stems of a bar range and takes one WAV back. The tool exists only when
//! `AURIS_GENMODEL_PYTHON` and `AURIS_GENMODEL_SCRIPT` name the interpreter and the script,
//! so no build ships with it switched on and nobody is offered a model they do not have.
//!
//! What comes back is audio: a performance of the range, not notes. It lands on a new audio
//! track and nothing already in the song is changed, so one Undo takes it away and the score
//! the agent wrote stays where it was.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::prelude::*;

/// The interpreter that runs the generation script.
const PYTHON_VAR: &str = "AURIS_GENMODEL_PYTHON";
/// The generation script.
const SCRIPT_VAR: &str = "AURIS_GENMODEL_SCRIPT";
/// How long one generation may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(1800);

/// Whether the experimental tool is configured on this machine.
pub fn enabled() -> bool {
    configured().is_some()
}

fn configured() -> Option<(PathBuf, PathBuf)> {
    let python = std::env::var_os(PYTHON_VAR).filter(|v| !v.is_empty())?;
    let script = std::env::var_os(SCRIPT_VAR).filter(|v| !v.is_empty())?;
    Some((python.into(), script.into()))
}

/// What the model is asked to make.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartRequest {
    /// Play one track's part again in another sound (Foundation-1 audio-to-audio from the
    /// track's own stem): the notes and timing stay the track's, the prompt names the new
    /// instrument and character. strength 0..1 (default 0.4) says how far the sound may move
    /// from the original; above about 0.6 the notes start to change. Replace the original by
    /// muting it.
    Restyle {
        /// Track ID to play again; defaults to the clip's track.
        track: Option<u64>,
        /// How far the take may move from the original, 0..1; default 0.4.
        strength: Option<f32>,
    },
    /// Rewrite bars of one track's audio (ACE-Step 1.5 repaint). The model hears only that
    /// track; outside the repainted bars the result is the track as rendered, so the new
    /// audio track can replace the original over the whole range.
    Repaint {
        /// Track ID whose stem is rewritten; defaults to the clip's track.
        track: Option<u64>,
        /// First one-based bar to rewrite, inside the range.
        start_bar: u32,
        /// Number of bars to rewrite.
        bars: u32,
    },
    /// A tempo-synced loop tiled over the range (Foundation-1). It knows the key and tempo
    /// but not the chords, and only 100, 110, 120, 128, 130, 140 and 150 BPM.
    Loop {
        /// Loop length in bars, 4 or 8.
        loop_bars: u32,
    },
}

/// What the off-thread half needs: everything is a snapshot or a path.
pub(super) struct Plan {
    pub(super) job: crate::RenderJob,
    pub(super) options: OfflineOptions,
    pub(super) request: serde_json::Value,
    pub(super) target_track: Option<TrackId>,
    pub(super) python: PathBuf,
    pub(super) script: PathBuf,
}

impl crate::Session {
    /// Validates a generate_part call and snapshots what its worker needs.
    pub(super) fn plan_generated_part(
        &mut self,
        request: PartRequest,
        range: Option<super::file_jobs::BarRange>,
        clip: Option<u64>,
        prompt: String,
        seed: Option<u32>,
    ) -> Result<(Plan, Ticks, String), String> {
        let (python, script) = configured().ok_or(
            "generate_part is experimental and not configured: set AURIS_GENMODEL_PYTHON and AURIS_GENMODEL_SCRIPT",
        )?;
        if prompt.trim().is_empty() || prompt.len() > 512 {
            return Err(
                "prompt must be a nonempty description of the sound, up to 512 bytes".into(),
            );
        }
        let clip_track = match clip {
            Some(id) => {
                let id = ClipId(id);
                Some((
                    self.track_of_clip(id).ok_or("No such clip")?,
                    self.clip_start(id).ok_or("No such clip")?,
                    self.clip_sounding_length(id).ok_or("No such clip")?,
                ))
            }
            None => None,
        };
        let project = self.project();
        let range = match (range, clip_track) {
            (Some(range), _) => range,
            (None, Some((_, start, length))) => {
                let first = project.signatures.bar_of(start);
                let end = Ticks(start.0 + length.0 - 1);
                super::file_jobs::BarRange {
                    start_bar: first,
                    bars: project.signatures.bar_of(end) - first + 1,
                }
            }
            (None, None) => return Err("Give range, or a clip whose bars set it".into()),
        };
        if range.bars > 64 {
            return Err("Ask for at most 64 bars at a time".into());
        }
        let (start_bar, bars) = (range.start_bar, range.bars);
        let options = self.agent_export_range(Some(range), false)?;
        let range = super::file_jobs::BarRange { start_bar, bars };
        let at = project.signatures.bar_start(range.start_bar);
        let end_bar = range.start_bar + range.bars;
        let tempo_points = project.tempo_map.points();
        let bpm = project.tempo_map.bpm_at(at);
        let end_tick = project.signatures.bar_start(end_bar);
        if tempo_points
            .iter()
            .any(|p| p.tick > at && p.tick < end_tick)
        {
            return Err(
                "The tempo changes inside the range; ask for a range with one tempo".into(),
            );
        }
        let signature = project.signatures.signature_at(at);
        let key = serde_json::to_value(project.harmony.key_at(at))
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let track_exists = |id: u64| -> Result<TrackId, String> {
            let id = TrackId(id);
            project
                .track(id)
                .map(|_| id)
                .ok_or_else(|| "No such track".to_string())
        };
        let mut body = serde_json::json!({
            "prompt": prompt,
            "bpm": bpm,
            "key": key,
            "beats_per_bar": signature.numerator,
            "seed": seed.unwrap_or(0),
        });
        let mut target_track = None;
        let name = match request {
            PartRequest::Restyle { track, strength } => {
                let track = match (track, clip_track) {
                    (Some(id), _) => track_exists(id)?,
                    (None, Some((id, _, _))) => id,
                    (None, None) => return Err("Restyle needs track or clip".into()),
                };
                let strength = strength.unwrap_or(0.4);
                if !(0.0..=1.0).contains(&strength) {
                    return Err("strength must lie within 0..1".into());
                }
                body["operation"] = "restyle".into();
                body["strength"] = strength.into();
                target_track = Some(track);
                let track_name = project
                    .track(track)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                format!("{track_name} (restyled)")
            }
            PartRequest::Repaint {
                track,
                start_bar,
                bars,
            } => {
                let track = match (track, clip_track) {
                    (Some(id), _) => track_exists(id)?,
                    (None, Some((id, _, _))) => id,
                    (None, None) => return Err("Repaint needs track or clip".into()),
                };
                if bars == 0
                    || start_bar < range.start_bar
                    || start_bar + bars > range.start_bar + range.bars
                {
                    return Err("The repainted bars must lie inside the range".into());
                }
                let seconds = |bar: u32| {
                    let tick = project.signatures.bar_start(bar);
                    project.tempo_map.ticks_to_seconds(tick).0
                        - project.tempo_map.ticks_to_seconds(at).0
                };
                body["operation"] = "repaint".into();
                body["repaint_seconds"] =
                    serde_json::json!([seconds(start_bar), seconds(start_bar + bars)]);
                target_track = Some(track);
                let track_name = project
                    .track(track)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                format!("{track_name} (repainted)")
            }
            PartRequest::Loop { loop_bars } => {
                body["operation"] = "loop".into();
                body["loop_bars"] = loop_bars.into();
                "Generated loop".into()
            }
        };
        let snapshot = project.clone();
        let job = self.job_for_at(snapshot, options.sample_rate);
        Ok((
            Plan {
                job,
                options,
                request: body,
                target_track,
                python,
                script,
            },
            at,
            name,
        ))
    }
}

/// Renders the stems, runs the script, and returns the WAV it wrote and what it reported.
pub(super) fn run(
    plan: Plan,
    folder: &Path,
    cancelled: &AtomicBool,
) -> Result<(tempfile::TempDir, PathBuf, serde_json::Value), String> {
    let Plan {
        mut job,
        options,
        mut request,
        target_track,
        python,
        script,
    } = plan;
    let work = tempfile::Builder::new()
        .prefix("generate-part-")
        .tempdir_in(folder)
        .map_err(|e| e.to_string())?;
    let stems_dir = work.path().join("stems");
    std::fs::create_dir_all(&stems_dir).map_err(|e| e.to_string())?;
    let settings = WavExportSettings {
        bit_depth: WavBitDepth::Float32,
        ..Default::default()
    };
    let mut progress = RenderProgress::default().cancelled_by(cancelled);
    let stems = job
        .render_stems(&stems_dir, &settings, &options, &mut progress)
        .map_err(|e| e.to_string())?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Generation cancelled".into());
    }
    let mut map = serde_json::Map::new();
    let mut names = std::collections::BTreeMap::new();
    for stem in &stems {
        names.insert(stem.track, stem.name.clone());
        map.insert(stem.name.clone(), stem.path.display().to_string().into());
    }
    if let Some(track) = target_track {
        let name = names
            .get(&track)
            .ok_or("The track is muted or has no stem in the range; unmute it first")?;
        request["target"] = name.clone().into();
    }
    request["stems"] = map.into();
    let output = work.path().join("generated.wav");
    request["output"] = output.display().to_string().into();
    let request_path = work.path().join("request.json");
    std::fs::write(
        &request_path,
        serde_json::to_vec_pretty(&request).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let stdout_path = work.path().join("stdout.json");
    let stderr_path = work.path().join("stderr.txt");
    let mut command = std::process::Command::new(&python);
    command
        .arg(&script)
        .arg(&request_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&stdout_path).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(&stderr_path).map_err(|e| e.to_string())?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", python.display()))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancelled.load(Ordering::Relaxed) {
                "Generation cancelled".into()
            } else {
                "Generation timed out".into()
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
    let report: serde_json::Value = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| {
            let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
            let tail: String = stderr
                .chars()
                .rev()
                .take(600)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("The generator ({status}) wrote no report: {tail}")
        })?;
    if let Some(error) = report.get("error").and_then(|e| e.as_str()) {
        return Err(format!("The generator refused: {error}"));
    }
    if !output.is_file() {
        return Err("The generator reported success but wrote no audio".into());
    }
    Ok((work, output, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_name_every_operation_the_script_knows() {
        for (json, kind) in [
            (
                serde_json::json!({"kind":"restyle","strength":0.4}),
                "restyle",
            ),
            (
                serde_json::json!({"kind":"repaint","start_bar":1,"bars":2}),
                "repaint",
            ),
            (serde_json::json!({"kind":"loop","loop_bars":8}), "loop"),
        ] {
            let parsed: PartRequest = serde_json::from_value(json).unwrap();
            assert_eq!(serde_json::to_value(&parsed).unwrap()["kind"], kind);
        }
        // ACE-Step's lego is not offered: its parts come out garbled on every device.
        assert!(
            serde_json::from_value::<PartRequest>(
                serde_json::json!({"kind":"stem","instrument":"drums"})
            )
            .is_err()
        );
    }
}
