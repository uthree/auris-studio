//! Bounded note-occupancy diagnosis and explicitly previewed phrase-rest repairs.

use super::*;
use auris_core::project::{ClipCurve, Track, loop_passes};

mod endings;
#[cfg(test)]
mod tests;

const NOTE_LIMIT: usize = 8192;
const CLIP_LIMIT: usize = 32;
type Interval = (i64, i64);

/// Native envelope controls are read from the registered implementation, never copied defaults.
pub(super) fn envelope_controls(registry: &PluginRegistry, project: &Project, track: u64) -> Value {
    let Some(source) = project
        .track(auris_core::TrackId(track))
        .and_then(|t| t.kind.as_instrument())
    else {
        return Value::Null;
    };
    let id = source.instrument_id.as_str();
    // The audio sampler shapes every note with the same `auris_dsp::Adsr` as the built-in voices,
    // so its four controls mean what theirs do. A SoundFont plays the envelope its font wrote.
    if !(id.starts_with("auris.physical.")
        || id == "auris.synth.chiptune"
        || auris_synth::AnalogPatch::from_id(id).is_some()
        || auris_synth::BoomPatch::from_id(id).is_some()
        || id == auris_sampler::AUDIO_SAMPLER_ID)
    {
        return Value::Null;
    }
    let Ok(instrument) = registry.create_instrument(id) else {
        return Value::Null;
    };
    json!(instrument.parameters().iter().filter(|p| ["attack", "decay", "sustain", "release"].contains(&p.key.as_ref())).map(|p| {
        let target = ParamTarget::Instrument { track: auris_core::TrackId(track), param: p.id };
        json!({"key":p.key,"base_value":p.clamp(source.instrument_state.params.get(p.key.as_ref()).copied().unwrap_or(p.default)),"unit":p.unit,
            "automated":project.automation.lane(target).is_some(),"target":automation::ControlTarget::Instrument { track, param: p.key.to_string() }})
    }).collect::<Vec<_>>())
}

/// What a SoundFont track can do instead, since it lists no native envelope: the font's own
/// envelope plays, and the way to shape it is to turn one rendered note into an audio sample.
pub(super) fn envelope_route(project: &Project, track: u64) -> Value {
    let soundfont = project
        .track(auris_core::TrackId(track))
        .and_then(|t| t.kind.as_instrument())
        .is_some_and(|source| source.instrument_id == auris_sampler::SAMPLER_ID);
    if !soundfont {
        return Value::Null;
    }
    json!(
        "This SoundFont plays its font's own envelope. To shape attack/release, write one held note, \
render stems and load_sample that track's WAV with that root_note, then set the \
sampler's attack/decay/sustain/release with set_instrument_params. load_sample replaces the font."
    )
}

fn window(start: i64, end: i64) -> Result<(), String> {
    if start < 0 || end <= start || end - start > 128 * 960 {
        return Err("Use a nonnegative passage of 1..122880 ticks (128 quarter notes)".into());
    }
    Ok(())
}

fn merge(mut notes: Vec<Interval>) -> Vec<Interval> {
    notes.sort_unstable();
    let mut result: Vec<Interval> = Vec::new();
    for (start, end) in notes {
        if let Some(last) = result.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            result.push((start, end));
        }
    }
    result
}

fn duration(project: &Project, start: i64, end: i64) -> Value {
    json!({"ticks":end-start,"quarter_notes":(end-start) as f64/960.0,
        "seconds":project.tempo_map.ticks_to_seconds(Ticks(end)).0-project.tempo_map.ticks_to_seconds(Ticks(start)).0})
}

fn metrics(
    project: &Project,
    notes: &[Interval],
    start: i64,
    end: i64,
    threshold: i64,
    ends: &[i64],
) -> Value {
    let mut cursor = start;
    let mut gaps = Vec::new();
    let mut occupied = 0;
    let mut longest = (start, start);
    let mut run = None;
    let mut run_end = start;
    let mut longest_run = (start, start);
    for &(a, b) in notes {
        if a > cursor {
            gaps.push((cursor, a));
        }
        occupied += b - a;
        if b - a > longest.1 - longest.0 {
            longest = (a, b);
        }
        if run.is_none() || a - run_end >= threshold {
            run = Some(a);
        }
        run_end = b;
        let beginning = run.unwrap_or(a);
        if b - beginning > longest_run.1 - longest_run.0 {
            longest_run = (beginning, b);
        }
        cursor = b;
    }
    if cursor < end {
        gaps.push((cursor, end));
    }
    let meaningful: Vec<_> = gaps.iter().filter(|(a, b)| b - a >= threshold).collect();
    let phrasing: Vec<_> = ends.iter().map(|&boundary| {
        let occupied_end=notes.iter().filter(|(a,_)|*a<boundary).map(|(_,b)|(*b).min(boundary)).max().unwrap_or(start);
        let space=boundary-occupied_end;
        json!({"end_tick":boundary,"preceding_space":duration(project,occupied_end,boundary),"has_phrase_rest":space>=threshold})
    }).collect();
    json!({"occupied_ticks":occupied,"occupancy":occupied as f64/(end-start) as f64,
        "longest_continuous":duration(project,longest.0,longest.1),
        "longest_without_phrase_rest":duration(project,longest_run.0,longest_run.1),
        "rest_count":meaningful.len(),"gaps_count":gaps.len(),"gaps_truncated":gaps.len()>64,
        "gaps":gaps.iter().take(64).map(|&(a,b)|json!({"start_tick":a,"end_tick":b,"duration":duration(project,a,b),"phrase_rest":b-a>=threshold})).collect::<Vec<_>>(),
        "phrase_endings":phrasing})
}

struct Observed {
    authored: Vec<Interval>,
    performed: Vec<Interval>,
    complete: bool,
    clips: Vec<Value>,
    sustain: bool,
}

fn observe(project: &Project, track: &Track, start: i64, end: i64) -> Observed {
    let mut data = Observed {
        authored: Vec::new(),
        performed: Vec::new(),
        complete: true,
        clips: Vec::new(),
        sustain: false,
    };
    let Some(clips) = track.kind.note_clips() else {
        data.complete = false;
        return data;
    };
    let mut count = 0;
    for clip in clips
        .iter()
        .filter(|c| c.start.raw() < end && c.sounding_end().raw() > start)
    {
        count += 1;
        if count > CLIP_LIMIT {
            data.complete = false;
            break;
        }
        if clip.muted || track.mixer.mute {
            continue;
        }
        let passes = loop_passes(clip.length, clip.loop_end).count();
        let supported = clip.notes.len() <= 2048
            && clip.transforms.len() <= 32
            && passes <= 128
            && clip.looped_note_instances().is_ok_and(|n| n <= 4096)
            && clip.controllers.values().all(|p| p.len() <= 4096)
            && crate::live_agent::performance::measure_arrangement_performance(clip, project, None)
                ["complete"]
                == true;
        data.clips.push(json!({"clip":clip.id.0,"recipe":clip.recipe.is_some(),"passes":passes,"complete":supported}));
        if !supported {
            data.complete = false;
            continue;
        }
        data.sustain |= clip
            .curve(ClipCurve::Controller(64))
            .iter()
            .any(|p| p.value >= 0.5);
        let mut add = |note: auris_core::Note, offset: Ticks, span: Ticks, performed: bool| {
            let a = (clip.start + offset + note.start).raw().max(start);
            let b = (clip.start + offset + (note.start + note.length).min(span))
                .raw()
                .min(end);
            if a < b {
                let notes = if performed {
                    &mut data.performed
                } else {
                    &mut data.authored
                };
                if notes.len() < NOTE_LIMIT {
                    notes.push((a, b));
                } else {
                    data.complete = false;
                }
            }
        };
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            for note in clip.playable_notes().filter(|n| n.start < span) {
                add(note, offset, span, false);
            }
        }
        for note in clip.sounding_notes_with_meter(
            project.tempo_map.bpm_at(clip.start),
            project.signatures.clone(),
        ) {
            add(note, Ticks::ZERO, clip.sounding_length(), true);
        }
    }
    data.authored = merge(data.authored);
    data.performed = merge(data.performed);
    data
}

/// Shared read-only observations, including joins between clips and actual seeded loop passes.
pub(super) fn inspect(
    project: &Project,
    track: u64,
    role: MelodicRole,
    start: i64,
    end: i64,
    threshold: i64,
    ends: &[i64],
) -> Result<Value, String> {
    window(start, end)?;
    if !(1..=8 * 960).contains(&threshold)
        || ends.len() > 32
        || ends.iter().any(|b| *b <= start || *b > end)
        || ends.windows(2).any(|p| p[0] >= p[1])
    {
        return Err("Use min_rest_ticks 1..7680 and up to 32 strictly ordered phrase endings inside the passage".into());
    }
    let track = project
        .track(auris_core::TrackId(track))
        .ok_or("Unknown breathing track")?;
    if !project.holds_notes(track.id) {
        return Err("Breathing diagnosis requires a note track".into());
    }
    let observed = observe(project, track, start, end);
    let authored = metrics(project, &observed.authored, start, end, threshold, ends);
    let performed = metrics(project, &observed.performed, start, end, threshold, ends);
    let mut suggestions = Vec::new();
    if observed.complete
        && matches!(role, MelodicRole::Lead)
        && performed["phrase_endings"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p["has_phrase_rest"] == false))
    {
        suggestions.push("Some explicit lead phrase endings lack the requested rest. Choose meaningful release points or explicitly omit pickups; do not apply a uniform gate to every note.");
    }
    let sustain_response = track
        .kind
        .as_instrument()
        .map(|instrument| {
            diagnostics::capabilities(instrument, track.kind.is_drum())["cc64_sustain"].clone()
        })
        .unwrap_or(json!("unknown"));
    Ok(
        json!({"track":track.id.0,"role":role,"start_tick":start,"end_tick":end,"min_rest_ticks":threshold,
        "complete":observed.complete,"truncated":!observed.complete,"clip_limit":CLIP_LIMIT,"note_limit_per_representation":NOTE_LIMIT,
        "pass_policy":"All bounded passes, using the playback performer and its actual pass seeds; clip/loop joins are measured together",
        "authored":authored,"performed":performed,"clips":observed.clips,"suggestions":suggestions,
        "ending_guidance":endings::guidance(project, track),
        "audibility":{"sustain_data_present":observed.sustain,"sustain_response":sustain_response,"track_muted":track.mixer.mute,
            "effect_or_extra_output_paths_present":!track.mixer.effects.is_empty()||project.outputs(track.id).len()>1,
            "limitation":"Note occupancy excludes instrument release, pedal-held tails, effect/bus tails and other tracks. Compare the foreground stem and mix with inspect_audio/preview/listen; these flags identify possible causes, not proven audible silence."}}),
    )
}

fn protected(note: &auris_core::Note) -> bool {
    !note.lyric.is_empty()
        || !note.phonemes.is_empty()
        || !note.phoneme_seconds.is_empty()
        || note.scoop.is_some()
        || note.fall.is_some()
        || note.vibrato.is_some()
}

/// Prepare exact score changes on the worker's private candidate; no partial edit is published.
pub(super) fn repair(
    project: &mut Project,
    clips: &[u64],
    rests: &[RestWindow],
    minimum: i64,
    protections: &[NoteAddress],
    omissions: &[NoteAddress],
    landings: &[PhraseLanding],
) -> Result<Value, String> {
    if clips.is_empty()
        || clips.len() > 16
        || rests.is_empty()
        || rests.len() > 16
        || !(1..=3840).contains(&minimum)
        || protections.len() > 256
        || omissions.len() > 256
        || landings.len() > 16
    {
        return Err("Breath requires 1..16 clips/rests, min_note_ticks 1..3840, and at most 256 protected/omitted notes".into());
    }
    for rest in rests {
        window(rest.start_tick, rest.end_tick)?;
    }
    if rests.windows(2).any(|p| p[0].end_tick > p[1].start_tick) {
        return Err("Rest windows must be ordered and nonoverlapping".into());
    }
    let coverage = merge(rests.iter().map(|r| (r.start_tick, r.end_tick)).collect());
    let start = rests[0].start_tick;
    let end = rests.last().ok_or("Missing rests")?.end_tick;
    window(start, end)?;
    let track = project
        .track_of_clip(ClipId(clips[0]))
        .ok_or("Unknown foreground clip")?;
    let source = project.track(track).ok_or("Unknown foreground track")?;
    if source.kind.as_instrument().is_none() || source.kind.is_drum() || source.mixer.mute {
        return Err("Breath repairs require an unmuted foreground instrument track; singer lyric slots and drums are protected".into());
    }
    let mut unique = HashSet::new();
    let mut clip_start = i64::MAX;
    let mut clip_end = 0;
    for id in clips {
        let (owner, clip) = project
            .midi_clip(ClipId(*id))
            .ok_or("Unknown foreground MIDI clip")?;
        if owner != track || !unique.insert(*id) || clip.muted {
            return Err("Select unique, unmuted clips on one instrument track".into());
        }
        clip_start = clip_start.min(clip.start.raw());
        clip_end = clip_end.max(clip.sounding_end().raw());
    }
    if start < clip_start || end > clip_end {
        return Err("Rest windows must lie inside the selected clips' passage".into());
    }
    window(clip_start, clip_end)?;
    let validate_addresses = |addresses: &[NoteAddress]| -> Result<HashSet<NoteAddress>, String> {
        let mut set = HashSet::new();
        for address in addresses {
            if !clips.contains(&address.clip)
                || project
                    .midi_clip(ClipId(address.clip))
                    .is_none_or(|(_, c)| address.index >= c.notes.len())
                || !set.insert(address.clone())
            {
                return Err("Protected/omitted notes require unique original read_notes indices in selected clips".into());
            }
        }
        Ok(set)
    };
    let protections = validate_addresses(protections)?;
    let omissions = validate_addresses(omissions)?;
    if !protections.is_disjoint(&omissions) {
        return Err("A protected note cannot also be omitted".into());
    }
    let before = inspect(project, track.0, MelodicRole::Lead, start, end, 1, &[])?;
    if before["complete"] != true {
        return Err(
            "Breathing measurement is incomplete; select a shorter or simpler passage".into(),
        );
    }
    // There is no stored tie flag. Conservatively protect touching same-pitch notes,
    // including a continuation in an adjacent clip or a neighboring loop pass.
    let mut attacks = HashSet::new();
    let mut releases = HashSet::new();
    let mut tie_events = 0;
    for clip in source
        .kind
        .note_clips()
        .ok_or("Missing foreground clips")?
        .iter()
        .filter(|c| !c.muted && c.start.raw() <= clip_end && c.sounding_end().raw() >= clip_start)
    {
        let passes = loop_passes(clip.length, clip.loop_end).count();
        if clip.notes.len() > 2048 || passes > 128 {
            return Err("Adjacent tie inspection is incomplete; split or simplify the foreground passage first".into());
        }
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            for note in clip.playable_notes().filter(|n| n.start < span) {
                tie_events += 1;
                if tie_events > NOTE_LIMIT {
                    return Err("Adjacent tie inspection exceeds its note budget".into());
                }
                let a = (clip.start + offset + note.start).raw();
                let b = (clip.start + offset + (note.start + note.length).min(span)).raw();
                attacks.insert((a, note.pitch));
                releases.insert((b, note.pitch));
            }
        }
    }
    // Any supported (or unknown) pedal on this channel can mask the rest, including another clip.
    let landing = endings::prepare(
        endings::LandingContext {
            project,
            clips,
            rests,
            protections: &protections,
            omissions: &omissions,
            attacks: &attacks,
            releases: &releases,
        },
        landings,
    )?;
    let pedal_may_sound = source.kind.as_instrument().is_none_or(|instrument| {
        diagnostics::capabilities(instrument, false)["cc64_sustain"] != "unsupported"
    });
    for clip in source
        .kind
        .note_clips()
        .ok_or("Missing foreground clips")?
        .iter()
        .filter(|c| !c.muted && pedal_may_sound)
    {
        for rest in rests {
            if clip.start.raw() >= rest.end_tick || clip.sounding_end().raw() <= rest.start_tick {
                continue;
            }
            for (offset, span) in loop_passes(clip.length, clip.loop_end) {
                let pass_start = (clip.start + offset).raw();
                let a = Ticks(rest.start_tick.max(pass_start) - pass_start);
                let b = Ticks(rest.end_tick.min(pass_start + span.raw()) - pass_start);
                if a >= b {
                    continue;
                }
                if clip.curve_at(ClipCurve::Controller(64), a) >= 0.5
                    || clip.curve_at(ClipCurve::Controller(64), b - Ticks(1)) >= 0.5
                    || clip
                        .curve(ClipCurve::Controller(64))
                        .iter()
                        .any(|p| p.at >= a && p.at < b && p.value >= 0.5)
                {
                    return Err("Sustain is active during a requested rest. Inspect/reset CC64 explicitly instead of deleting more notes".into());
                }
            }
        }
    }
    let mut changes = landing.changes;
    let mut consumed = HashSet::new();
    for id in clips {
        let clip = project
            .midi_clip_mut(ClipId(*id))
            .ok_or("Missing foreground clip")?;
        // Stored loop notes are shared. Map requested windows to one content cycle,
        // then require every audible repetition of that pattern to be explicitly covered.
        let mut pattern = Vec::new();
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            let pass_start = (clip.start + offset).raw();
            let pass_end = pass_start + span.raw();
            for rest in rests {
                let a = rest.start_tick.max(pass_start);
                let b = rest.end_tick.min(pass_end);
                if a < b {
                    pattern.push((a - offset.raw(), b - offset.raw()));
                }
            }
        }
        let pattern = merge(pattern);
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            let pass_end = (clip.start + offset + span).raw();
            for &(a, b) in &pattern {
                let a = a + offset.raw();
                let b = (b + offset.raw()).min(pass_end);
                if a < b && !coverage.iter().any(|(start, end)| *start <= a && *end >= b) {
                    return Err("This score edit would change an unrequested loop pass. Explicitly request the same relative rest in every pass, or edit independent clips".into());
                }
            }
        }
        let mut removed = Vec::new();
        for index in 0..clip.notes.len() {
            let address = NoteAddress { clip: *id, index };
            if landing.replaced.contains(&address) {
                removed.push(index);
                continue;
            }
            if let Some(start) = landing.starts.get(&address) {
                let note = &mut clip.notes[index];
                let end = note.end();
                note.start = Ticks(*start - clip.start.raw());
                note.length = end - note.start;
            }
            let note = &clip.notes[index];
            let a = (clip.start + note.start).raw();
            let b = (clip.start + note.start + note.length)
                .min(clip.end())
                .raw();
            let overlap = pattern.iter().find(|(start, end)| a < *end && b > *start);
            let Some(&(rest_start, rest_end)) = overlap else {
                continue;
            };
            let omit = omissions.contains(&address);
            if omit && !(a >= rest_start && a < rest_end) {
                return Err(
                    "An omitted pickup must start inside an explicitly requested rest".into(),
                );
            }
            if protections.contains(&address) || protected(note) {
                return Err(format!(
                    "Clip {id} note {index} is protected or has vocal data; choose a different breath"
                ));
            }
            if loop_passes(clip.length, clip.loop_end).any(|(offset, span)| {
                let a = (clip.start + offset + note.start).raw();
                let b = (clip.start + offset + (note.start + note.length).min(span)).raw();
                note.start < span
                    && (attacks.contains(&(b, note.pitch)) || releases.contains(&(a, note.pitch)))
            }) {
                return Err(format!(
                    "Clip {id} note {index} has a same-pitch continuation; preserve the possible tie or use an exact intentional note edit"
                ));
            }
            if omit {
                consumed.insert(address);
                removed.push(index);
                changes.push(json!({"clip":id,"index":index,"kind":"omit","before":note}));
            } else {
                if a >= rest_start {
                    return Err(format!(
                        "Clip {id} note {index} attacks inside the rest; name that exact pickup in omit_notes or choose another boundary"
                    ));
                }
                let length = rest_start - a;
                if length < minimum {
                    return Err(format!(
                        "Clip {id} note {index} would be shorter than min_note_ticks; choose a different breath"
                    ));
                }
                changes.push(json!({"clip":id,"index":index,"kind":"shorten","pitch":note.pitch,"start_tick":note.start.raw(),"before_length_ticks":note.length.raw(),"after_length_ticks":length}));
                clip.notes[index].length = Ticks(length);
            }
        }
        for index in removed.into_iter().rev() {
            clip.notes.remove(index);
        }
    }
    if consumed != omissions {
        return Err("Every omit_notes index must name a pickup inside a requested rest".into());
    }
    let after = inspect(project, track.0, MelodicRole::Lead, start, end, 1, &[])?;
    let observed = observe(
        project,
        project.track(track).ok_or("Missing track")?,
        start,
        end,
    );
    if !observed.complete
        || rests.iter().any(|r| {
            observed
                .performed
                .iter()
                .any(|(a, b)| *a < r.end_tick && *b > r.start_tick)
        })
    {
        return Err("Performed notes still fill a requested rest, possibly from another clip or performance stage. Inspect that source before repairing again".into());
    }
    Ok(
        json!({"track":track.0,"rests":rests,"note_changes":changes,"before":before,"after":after,
        "retained_pitches_attacks_velocities_preserved":landings.is_empty(),"retained_pitches_velocities_preserved":true,"attacks_preserved_except_explicit_landings":true,"curves_and_recipes_preserved":true,
        "ending_guidance":endings::guidance(project, project.track(track).ok_or("Missing track")?),
        "audio_review_required":true,"warning":"This verifies note-event space only. Compare the foreground stem and full mix; release, reverb and other layers may mask a rest. Explicit recipe regeneration can replace repaired notes."}),
    )
}
