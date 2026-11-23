//! Work on an owned document; no partial candidate reaches the live session.

use super::*;
use auris_core::plugin::PrepareContext;
use auris_core::time::Seconds;
use auris_core::{AssetPath, Note};
use std::io::{Read, Write};

impl ArrangementJob {
    /// Prepare sounds, source copies, candidate edits and effect measurements with cancellation.
    pub fn run(mut self, cancelled: &AtomicBool) -> Result<ArrangementResult, String> {
        check(cancelled)?;
        let mut imports = Vec::new();
        let mut sounds = Vec::new();
        let mut output = Vec::new();
        let mut issues = Vec::new();
        let mut bytes = serde_json::to_vec(&self.project)
            .map_err(|e| e.to_string())?
            .len();
        if bytes > MAX_PLAN_BYTES {
            return Err("Document exceeds the 64 MiB arrangement snapshot limit".into());
        }
        let original_bytes = bytes;
        let original_end = self.project.end_tick();
        for (index, (purpose, pending)) in self.changes.into_iter().enumerate() {
            check(cancelled)?;
            let change = index + 1;
            let result = match pending {
                Pending::Other(Action::Breath {
                    clips,
                    rests,
                    min_note_ticks,
                    protected_notes,
                    omit_notes,
                    landings,
                }) => breathing::repair(
                    &mut self.project,
                    &clips,
                    &rests,
                    min_note_ticks,
                    &protected_notes,
                    &omit_notes,
                    &landings,
                )?,
                Pending::Performance { clip, voice, stack } => {
                    let before = self
                        .project
                        .midi_clip(clip)
                        .ok_or("Unknown MIDI clip")?
                        .1
                        .clone();
                    let midi = self
                        .project
                        .midi_clip_mut(clip)
                        .ok_or("Unknown MIDI clip")?;
                    midi.transforms = stack;
                    midi.looped_note_instances().map_err(|e| e.to_string())?;
                    let (track, midi) = self.project.midi_clip(clip).ok_or("Unknown MIDI clip")?;
                    let track = self.project.track(track).ok_or("Unknown track")?;
                    diagnostics::check_performance(
                        &self.project,
                        midi,
                        track.kind.as_instrument(),
                        track.kind.is_drum(),
                        &mut issues,
                        change,
                    );
                    json!({"clip":clip.0,"drum_voice":voice,"performance":diagnostics::phrase(&self.project,midi),
                        "selected_scope":crate::live_agent::performance::measure_arrangement_performance(midi,&self.project,voice.as_deref()),
                        "delta":diagnostics::performance_delta(&self.project,&before,midi),"notes_preserved":true})
                }
                Pending::Automation {
                    target,
                    key,
                    curve,
                    value,
                    start,
                    end,
                    points,
                } => {
                    let original = self.project.automation.lane(target).cloned();
                    let at = |tick| original.as_ref().map_or(value, |lane| lane.value_at(tick));
                    let before = at(start);
                    let after = at(end);
                    let interior: Vec<_> = original
                        .as_ref()
                        .into_iter()
                        .flat_map(|lane| lane.points())
                        .filter(|p| p.tick > start && p.tick < end)
                        .map(|p| p.tick)
                        .collect();
                    for tick in interior {
                        self.project.automation.remove_point(target, tick);
                    }
                    for (tick, value) in [
                        (Ticks::ZERO, at(Ticks::ZERO)),
                        (start, before),
                        (end, after),
                    ] {
                        // An anchor at zero prevents the first point from changing all earlier playback.
                        if tick == Ticks::ZERO && original.is_some() {
                            continue;
                        }
                        self.project
                            .automation
                            .set_point(target, key.clone(), curve, tick, value);
                    }
                    for p in points {
                        self.project.automation.set_point(
                            target,
                            key.clone(),
                            curve,
                            Ticks(p.at_tick),
                            p.value,
                        );
                    }
                    json!({"target":target,"start_tick":start.raw(),"end_tick":end.raw(),"outside_curve_preserved":true,"boundary_values":[before,after]})
                }
                Pending::Revoice {
                    clips,
                    sound,
                    name,
                    layer,
                    stacks,
                } => {
                    let prepared = sound.run(&PrepareContext::new(self.render_rate, 512, 2))?;
                    let instrument = prepared.instrument;
                    sounds.push(*sound);
                    let source = self
                        .project
                        .track_of_clip(clips[0])
                        .ok_or("Unknown source clip")?;
                    let drum = self
                        .project
                        .track(source)
                        .ok_or("Unknown source track")?
                        .kind
                        .is_drum();
                    let mut selected = Vec::new();
                    for (clip, stack) in clips.iter().zip(stacks) {
                        let mut midi = self
                            .project
                            .midi_clip(*clip)
                            .ok_or("Unknown source MIDI clip")?
                            .1
                            .clone();
                        midi.transforms = stack;
                        midi.looped_note_instances().map_err(|e| e.to_string())?;
                        selected.push(midi);
                    }
                    let track = self
                        .project
                        .duplicate_track(source)
                        .ok_or("Could not duplicate source routing")?;
                    let copied_ids: std::collections::HashMap<_, _> = self
                        .project
                        .track(source)
                        .and_then(|t| t.kind.note_clips())
                        .ok_or("Expected source clips")?
                        .iter()
                        .zip(
                            self.project
                                .track(track)
                                .and_then(|t| t.kind.note_clips())
                                .ok_or("Expected copied clips")?,
                        )
                        .map(|(a, b)| (a.id, b.id))
                        .collect();
                    self.project.remove_instrument_automation(track);
                    let mut new_instrument = instrument;
                    for midi in &mut selected {
                        if layer {
                            midi.id = *copied_ids
                                .get(&midi.id)
                                .ok_or("Missing copied clip identity")?;
                        } else {
                            self.project.remove_clip(midi.id);
                        }
                    }
                    let ids: Vec<_> = selected.iter().map(|c| c.id.0).collect();
                    new_instrument.clips = selected;
                    let destination = self
                        .project
                        .track_mut(track)
                        .ok_or("Unknown destination track")?;
                    destination.name = name;
                    destination.kind = if drum {
                        auris_core::TrackKind::Drum(new_instrument)
                    } else {
                        auris_core::TrackKind::Instrument(new_instrument)
                    };
                    if let Some(effects) = prepared.effects {
                        // A saved sound is an instrument with its inserts: the copied track's own
                        // inserts belong to the sound it no longer plays.
                        let old: Vec<_> = self
                            .project
                            .track(track)
                            .into_iter()
                            .flat_map(|entry| &entry.mixer.effects)
                            .map(|slot| slot.id)
                            .collect();
                        for slot in old {
                            self.project.remove_effect(slot);
                        }
                        for effect in &effects {
                            if let Some(slot) =
                                self.project.add_effect(Some(track), &effect.effect_id)
                                && let Some(entry) =
                                    self.project.track_mut(track).and_then(|entry| {
                                        entry
                                            .mixer
                                            .effects
                                            .iter_mut()
                                            .find(|entry| entry.id == slot)
                                    })
                            {
                                entry.state.clone_from(&effect.state);
                                entry.enabled = effect.enabled;
                                entry.file =
                                    effect.file.as_ref().map(auris_core::AssetPath::external);
                            }
                        }
                    }
                    let destination = self
                        .project
                        .track(track)
                        .ok_or("Unknown destination track")?;
                    for midi in destination.kind.note_clips().ok_or("Expected note track")? {
                        diagnostics::check_performance(
                            &self.project,
                            midi,
                            destination.kind.as_instrument(),
                            drum,
                            &mut issues,
                            change,
                        );
                    }
                    issues.push(json!({"change":change,"severity":"warning","code":"revoice_audio_review","message":"Remeasure sound level and compare the join and release tails. Source instrument automation remains on the original track; the new sound needs its own native parameter lanes."}));
                    json!({"track":track.0,"source_track":source.0,"clips":ids,"layer":layer,"capabilities":destination.kind.as_instrument().map(|i|diagnostics::capabilities(i,drum)),"notes_and_authored_curves_preserved":true})
                }
                Pending::Other(Action::ClipMute { clip, muted }) => {
                    let id = ClipId(clip);
                    if let Some(midi) = self.project.midi_clip_mut(id) {
                        midi.muted = muted;
                    } else if let Some(audio) = self.project.audio_clip_mut(id) {
                        audio.muted = muted;
                    } else {
                        return Err("Unknown clip to mute".into());
                    }
                    json!({"clip":clip,"muted":muted,"notes_preserved":true})
                }
                Pending::Other(Action::Transition {
                    transition,
                    at_tick,
                    duration_ticks,
                    name,
                    gain_db,
                }) => transition_sound(
                    &mut self.project,
                    &self.owner,
                    transition,
                    at_tick,
                    duration_ticks,
                    &name,
                    gain_db,
                )?,
                Pending::Other(Action::AudioSfx {
                    path,
                    at_tick,
                    alignment,
                    name,
                    gain_db,
                    fade_in_ms,
                    fade_out_ms,
                }) => {
                    let (import, report, retained) = audio_sfx(
                        &mut self.project,
                        &self.folder,
                        self.render_rate,
                        &path,
                        at_tick,
                        alignment,
                        &name,
                        gain_db,
                        fade_in_ms,
                        fade_out_ms,
                        cancelled,
                    )?;
                    bytes = bytes.saturating_add(retained);
                    if bytes > MAX_PLAN_BYTES {
                        return Err(
                            "Prepared arrangement exceeds its 64 MiB source/snapshot budget".into(),
                        );
                    }
                    imports.push(import);
                    report
                }
                Pending::Other(_) => return Err("Unexpected arrangement operation".into()),
            };
            output.push(json!({"change":change,"purpose":purpose,"result":result}));
        }
        check(cancelled)?;
        self.project
            .validate_loop_expansion()
            .map_err(|e| e.to_string())?;
        bytes = bytes.saturating_sub(original_bytes).saturating_add(
            serde_json::to_vec(&self.project)
                .map_err(|e| e.to_string())?
                .len(),
        );
        if bytes > MAX_PLAN_BYTES {
            return Err("Prepared arrangement exceeds the 64 MiB snapshot/source budget".into());
        }
        if self.project.end_tick() > original_end {
            return Err("Arrangement would extend the song; choose a shorter SFX/transition or move it inside the existing duration".into());
        }
        let id = crate::transient_id::transient_id("arrangement");
        let report = json!({"status":"ready","plan_id":id,"revision":self.revision,"expires_in_seconds":TTL.as_secs(),
            "document_unchanged":true,"changes":output,"issues":issues,"preserved":["authored pitches and timing except explicitly previewed Breath edits","authored clip curves","harmony","lyrics","tempo/meter","unselected clips"],
            "next":"Review these changes and warnings, then apply this plan_id with its original revision. Read back independently and compare the same rendered bars; ready is structural validation, not a listening verdict."});
        let plan = PreparedPlan {
            id,
            scope: self.scope,
            revision: self.revision,
            owner_revision: self.owner_revision,
            created: Instant::now(),
            project: self.project,
            imports,
            sounds,
            report,
            bytes,
            applied: None,
            invalid: false,
        };
        Ok(ArrangementResult {
            owner: self.owner,
            plan,
            folder: self.folder,
            render_rate: self.render_rate,
        })
    }
}

fn transition_sound(
    project: &mut Project,
    registry: &PluginRegistry,
    kind: Transition,
    at: i64,
    duration: i64,
    name: &str,
    gain_db: f32,
) -> Result<Value, String> {
    let riser = matches!(kind, Transition::Riser);
    let id = if riser {
        "auris.physical.violin"
    } else {
        "auris.synth.drumkit"
    };
    if !registry.has_instrument(id) {
        return Err(format!("Transition instrument {id} is unavailable"));
    }
    let start = if riser {
        at.checked_sub(duration)
            .filter(|t| *t >= 0)
            .ok_or("Riser would begin before the song; shorten it or choose a later boundary")?
    } else {
        at
    };
    let track = if riser {
        project.add_instrument_track(name, id)
    } else {
        project.add_drum_track(name, id)
    };
    project
        .track_mut(track)
        .ok_or("Missing transition track")?
        .mixer
        .gain_db = gain_db;
    let clip = project
        .add_midi_clip(track, name, Ticks(start), Ticks(duration))
        .ok_or("Could not place transition")?;
    let midi = project
        .midi_clip_mut(clip)
        .ok_or("Missing transition clip")?;
    let pitches: &[u8] = match kind {
        Transition::Riser => &[60],
        Transition::Crash => &[49],
        Transition::Impact => &[36, 41],
    };
    for pitch in pitches {
        let mut note = Note::new(*pitch, Ticks::ZERO, Ticks(duration));
        note.velocity = 0.7;
        midi.notes.push(note);
    }
    if riser {
        // This is an editable bowed crescendo, not a claim of a reverse-cymbal/noise sample.
        let target = ParamTarget::TrackGain(track);
        project
            .automation
            .set_point(target, None, AutomationCurve::Linear, Ticks(start), -60.0);
        project.automation.set_point(
            target,
            None,
            AutomationCurve::Linear,
            Ticks(at.saturating_sub(1).max(start)),
            gain_db,
        );
        project
            .automation
            .set_point(target, None, AutomationCurve::Linear, Ticks(at), -60.0);
    }
    Ok(
        json!({"track":track.0,"clip":clip.0,"start_tick":start,"boundary_tick":at,"sound":if riser {"bowed crescendo"} else {"native drum attack"},"gain_db":gain_db,"audio_review_required":true}),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "All fields describe one bounded source preparation; keeping them explicit avoids an unvalidated second request type"
)]
fn audio_sfx(
    project: &mut Project,
    folder: &std::path::Path,
    render_rate: f64,
    path: &str,
    at: i64,
    alignment: Alignment,
    name: &str,
    gain_db: f32,
    fade_in_ms: f32,
    fade_out_ms: f32,
    cancelled: &AtomicBool,
) -> Result<(Import, Value, usize), String> {
    check(cancelled)?;
    let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let metadata = input.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
        return Err("SFX must be a regular local file up to 32 MiB".into());
    }
    let directory = folder.join(auris_io::AUDIO_DIR);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");
    let mut file = tempfile::Builder::new()
        .prefix("arrangement-")
        .suffix(&format!(".{extension}"))
        .tempfile_in(directory)
        .map_err(|e| e.to_string())?;
    let mut bytes = 0usize;
    let mut chunk = [0; 65536];
    loop {
        check(cancelled)?;
        let n = input.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes += n;
        if bytes > 32 * 1024 * 1024 {
            return Err("SFX grew beyond 32 MiB while copying".into());
        }
        file.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    let buffer =
        crate::decode_audio(file.path(), project.sample_rate).map_err(|e| e.to_string())?;
    let frames = buffer.frame_count();
    let rate = buffer.sample_rate();
    if frames == 0 || frames as f64 / rate > 30.0 || buffer.channel_count() > 2 {
        return Err("SFX must contain nonempty mono/stereo audio of at most thirty seconds".into());
    }
    let mut peak = 0.0f32;
    let mut peak_frame = 0;
    for frame in 0..frames {
        if frame % 65536 == 0 {
            check(cancelled)?;
        }
        for channel in buffer.channels() {
            let value = channel[frame].abs();
            if !value.is_finite() {
                return Err("SFX contains a non-finite sample".into());
            }
            if value > peak {
                peak = value;
                peak_frame = frame;
            }
        }
    }
    if peak <= 1e-7 {
        return Err("SFX is silent; choose an audible source".into());
    }
    let aligned_frame = match alignment {
        Alignment::Start => 0,
        Alignment::Peak => peak_frame,
        Alignment::End => frames,
    };
    let seconds = project.tempo_map.ticks_to_seconds(Ticks(at)).0 - aligned_frame as f64 / rate;
    if seconds < 0.0 {
        return Err("Aligned SFX would begin before the song; choose a later boundary".into());
    }
    let start = project.tempo_map.seconds_to_ticks(Seconds(seconds));
    let fade_in = (f64::from(fade_in_ms) * rate / 1000.0).round() as u64;
    let fade_out = (f64::from(fade_out_ms) * rate / 1000.0).round() as u64;
    if fade_in + fade_out > frames as u64 {
        return Err("SFX fades exceed the source duration".into());
    }
    let filename = file
        .path()
        .file_name()
        .ok_or("Missing staged SFX filename")?
        .to_owned();
    let source = project.add_audio_source(
        name,
        AssetPath::inside(std::path::Path::new(auris_io::AUDIO_DIR).join(filename)),
        frames as u64,
        rate,
        buffer.channel_count(),
    );
    project
        .audio_sources
        .get_mut(&source)
        .ok_or("Missing SFX source")?
        .byte_size = bytes as u64;
    let track = project.add_audio_track(name);
    let clip = project
        .add_audio_clip(track, source, start)
        .ok_or("Could not place SFX")?;
    let audio = project.audio_clip_mut(clip).ok_or("Missing SFX clip")?;
    audio.name = name.into();
    audio.gain_db = gain_db;
    audio.fade_in_frames = fade_in;
    audio.fade_out_frames = fade_out;
    audio.follows_tempo = false;
    let actual = project.tempo_map.ticks_to_seconds(start).0 + aligned_frame as f64 / rate;
    let requested = project.tempo_map.ticks_to_seconds(Ticks(at)).0;
    let report = json!({"track":track.0,"clip":clip.0,"source":source.0,"start_tick":start.raw(),"alignment_tick":at,
        "alignment_error_ms":(actual-requested)*1000.0,"source_peak_frame":peak_frame,"source_peak":peak,
        "duration_seconds":frames as f64/rate,"gain_db":gain_db,"tempo_following":false,"immutable_source_copy":true});
    let prepared =
        super::super::prepare_audio_source(buffer, render_rate).map_err(|e| e.to_string())?;
    let retained = bytes.saturating_add(prepared.retained_bytes());
    check(cancelled)?;
    Ok((
        Import {
            file: Some(file),
            source,
            prepared,
        },
        report,
        retained,
    ))
}
