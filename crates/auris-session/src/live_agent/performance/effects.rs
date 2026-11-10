//! Bounded first-pass measurements using the same performer as playback.

use super::*;
use auris_core::project::{ClipCurve, CurvePoint, curve_at};
use auris_core::{Note, NoteTransform, PerformanceContext, performed_note_slots, performed_notes};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const NOTE_LIMIT: usize = 2048;
const STAGE_LIMIT: usize = 32;
const CURVE_BEAT_LIMIT: i64 = 64;
const CURVE_SAMPLE_LIMIT: usize = 2048;

fn slide_transition_report(notes: &[Note], context: PerformanceContext<'_>) -> Value {
    let groups = {
        let mut indices: Vec<_> = (0..notes.len()).collect();
        indices.sort_by_key(|&i| (notes[i].start, notes[i].pitch));
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for index in indices {
            if let Some(last) = groups.last_mut()
                && notes[last[0]].start == notes[index].start
            {
                last.push(index);
            } else {
                groups.push(vec![index]);
            }
        }
        groups
    };
    let mut eligible = 0;
    let mut reasons = BTreeMap::<&str, usize>::new();
    for pair in groups.windows(2) {
        let reason = if pair[0].len() != 1 || pair[1].len() != 1 {
            "polyphonic"
        } else {
            let source = &notes[pair[0][0]];
            let target = &notes[pair[1][0]];
            let interval = i16::from(target.pitch) - i16::from(source.pitch);
            if interval.abs() < 2 {
                "same_or_semitone"
            } else if source.end() > target.start {
                "overlap"
            } else if target.start - source.end() > auris_core::Ticks::QUARTER {
                "long_gap"
            } else if source.length.raw() < 4 {
                "source_too_short"
            } else if target.start >= context.length {
                "outside_clip"
            } else {
                let length = auris_core::Ticks(
                    (auris_core::Ticks::QUARTER.raw() as f64 * context.bpm.max(0.0) / 1000.0)
                        .round()
                        .max(1.0) as i64,
                )
                .min(auris_core::Ticks(source.length.raw() / 4));
                let start = target.start - length;
                if notes.iter().enumerate().any(|(i, note)| {
                    i != pair[0][0]
                        && i != pair[1][0]
                        && note.start < target.start
                        && note.end() > start
                }) {
                    "overlap_with_other_note"
                } else {
                    eligible += 1;
                    continue;
                }
            }
        };
        *reasons.entry(reason).or_default() += 1;
    }
    json!({
        "candidate_transitions": groups.len().saturating_sub(1),
        "eligible_transitions": eligible,
        "ineligible_reasons": reasons,
    })
}

fn glide_transition_report(
    notes: &[Note],
    context: PerformanceContext<'_>,
    tempo: &auris_core::TempoMap,
) -> Value {
    let mut ordered = notes.to_vec();
    ordered.sort_by_key(|note| (note.start, note.pitch));
    let mut eligible = 0;
    let mut maximum_gap_ms = None::<f64>;
    let mut reasons = BTreeMap::<&str, usize>::new();
    for i in 0..ordered.len().saturating_sub(1) {
        let source = &ordered[i];
        let target = &ordered[i + 1];
        let gap_ms = (tempo.ticks_to_seconds(context.start + target.start).0
            - tempo.ticks_to_seconds(context.start + source.end()).0)
            .max(0.0)
            * 1000.0;
        maximum_gap_ms = Some(maximum_gap_ms.unwrap_or(0.0).max(gap_ms));
        let other_voice_overlap = ordered.iter().enumerate().any(|(j, note)| {
            j != i && j != i + 1 && note.start < target.end() && note.end() > source.start
        });
        let reason =
            if other_voice_overlap || source.end() > target.start || source.start == target.start {
                "polyphonic_or_overlap"
            } else if (tempo.ticks_to_seconds(context.start + source.end()).0
                - tempo.ticks_to_seconds(context.start + source.start).0)
                < 0.08
                || (tempo.ticks_to_seconds(context.start + target.end()).0
                    - tempo.ticks_to_seconds(context.start + target.start).0)
                    < 0.08
            {
                "note_too_short"
            } else if (tempo.ticks_to_seconds(context.start + target.start).0
                - tempo.ticks_to_seconds(context.start + source.end()).0)
                > 0.0600001
            {
                "gap_over_60ms"
            } else if (i16::from(target.pitch) - i16::from(source.pitch)).abs() > 24 {
                "interval_over_two_octaves"
            } else if target.pitch == source.pitch {
                "same_pitch"
            } else {
                eligible += 1;
                continue;
            };
        *reasons.entry(reason).or_default() += 1;
    }
    json!({
        "candidate_transitions": ordered.len().saturating_sub(1),
        "eligible_transitions": eligible,
        "maximum_gap_ms": maximum_gap_ms,
        "gap_limit_ms": 60,
        "ineligible_reasons": reasons,
    })
}

fn disable_glide(stage: &NoteTransform) -> Option<NoteTransform> {
    match stage {
        NoteTransform::Pitch { settings } if settings.glide_ms > 0.0 => {
            let mut disabled = settings.clone();
            disabled.glide_ms = 0.0;
            Some(NoteTransform::Pitch { settings: disabled })
        }
        _ => None,
    }
}

fn disable_controller(stage: &NoteTransform, which: ClipCurve) -> Option<NoteTransform> {
    let NoteTransform::Pitch { settings } = stage else {
        return None;
    };
    let mut disabled = settings.clone();
    match which {
        ClipCurve::Controller(1) => disabled.modulation = 0.0,
        ClipCurve::Controller(7) => disabled.volume_swell = 0.0,
        _ => return None,
    }
    Some(NoteTransform::Pitch { settings: disabled })
}

fn compare_curve(
    with_curve: &[CurvePoint],
    without_curve: &[CurvePoint],
    span: auris_core::Ticks,
) -> Value {
    let mut ticks = std::collections::BTreeSet::from([auris_core::Ticks::ZERO, span]);
    ticks.extend(with_curve.iter().filter(|p| p.at <= span).map(|p| p.at));
    ticks.extend(without_curve.iter().filter(|p| p.at <= span).map(|p| p.at));
    if ticks.len() > CURVE_SAMPLE_LIMIT {
        return json!({"measured": false, "effective": Value::Null,
            "changed_samples": 0, "sample_count": ticks.len(),
            "max_delta": Value::Null});
    }
    let mut changed = 0;
    let mut max_delta = 0.0_f32;
    let mut samples = 0;
    for at in ticks {
        let delta = (curve_at(with_curve, at) - curve_at(without_curve, at)).abs();
        if delta > 0.0001 {
            changed += 1;
        }
        max_delta = max_delta.max(delta);
        samples += 1;
    }
    json!({"measured": true, "effective": changed > 0 && max_delta > 0.0001,
        "changed_samples": changed, "sample_count": samples,
        "max_delta": max_delta})
}

fn pitch_report(
    clip: &MidiClip,
    stack: &[NoteTransform],
    stage_index: usize,
    voice: Option<&str>,
    project: &Project,
) -> Value {
    if voice.is_some()
        || stack.len() > STAGE_LIMIT
        || stack
            .iter()
            .any(|stage| matches!(stage, NoteTransform::ForDrumVoice { .. }))
    {
        return json!({"measured": false, "effective": Value::Null,
            "warnings": ["Pitch measurement is unavailable for nested/scoped drum writers or stacks exceeding 32 stages; an inner stack cannot describe the whole clip's channel curves."],
            "instrument_response": "unknown"});
    }
    let Some(NoteTransform::Pitch { settings }) = stack.get(stage_index) else {
        return json!({"measured": false, "warnings": ["Pitch stage was unavailable in the bounded scope."]});
    };
    let glide_requested = settings.glide_ms > 0.0;
    let span = clip
        .length
        .min(auris_core::Ticks::QUARTER * CURVE_BEAT_LIMIT);
    if clip.length > auris_core::Ticks::QUARTER * CURVE_BEAT_LIMIT {
        return json!({"measured": false, "glide_ms": settings.glide_ms,
            "effective": Value::Null, "warnings": ["The clip exceeds the 64-beat diagnostic bound; shorten the scope before claiming a complete pitch/controller measurement."],
            "instrument_response": "unknown"});
    }
    let seconds = project.tempo_map.ticks_to_seconds(clip.start + span).0
        - project.tempo_map.ticks_to_seconds(clip.start).0;
    if clip.notes.len() > NOTE_LIMIT {
        return json!({"measured": false, "effective": Value::Null,
            "warnings": ["The authored note count exceeds the diagnostic bound."],
            "instrument_response": "unknown"});
    }
    let source_notes = clip.playable_notes().take(NOTE_LIMIT + 1).count();
    let source_curve_points = clip.bend.len()
        + clip.controllers.values().map(Vec::len).sum::<usize>()
        + stack
            .iter()
            .map(|stage| match stage {
                NoteTransform::Pitch { settings } => settings.volume_contour.points().len(),
                _ => 0,
            })
            .sum::<usize>();
    if source_notes > NOTE_LIMIT || source_curve_points > CURVE_SAMPLE_LIMIT {
        return json!({
            "measured": false,
            "glide_ms": settings.glide_ms,
            "effective": Value::Null,
            "changed_samples": 0,
            "sample_count": 0,
            "max_semitone_delta": Value::Null,
            "warnings": ["Pitch-curve measurement is incomplete because the bounded source note or authored-curve limit was exceeded."],
            "instrument_response": "unknown",
        });
    }
    if seconds > 30.0 {
        return json!({
            "measured": false,
            "glide_ms": settings.glide_ms,
            "effective": Value::Null,
            "changed_samples": 0,
            "sample_count": 0,
            "max_semitone_delta": 0.0,
            "warnings": ["Pitch-curve measurement stops before 30 seconds of performed time to bound the generated 5 ms sample grid; inspect a shorter clip or faster passage."],
            "instrument_response": "unknown",
        });
    }
    let context = PerformanceContext {
        bpm: project.tempo_map.bpm_at(clip.start),
        pass: 0,
        start: clip.start,
        length: clip.length,
        signatures: &project.signatures,
    };
    // Bound expansion before asking the curve performer to run the complete stack.
    // In particular, repeated Slide/Octaves stages can multiply a small source.
    let mut final_notes: Vec<_> = clip.playable_notes().collect();
    for stage in stack {
        final_notes = performed_notes(final_notes, std::slice::from_ref(stage), context);
        if final_notes.len() > NOTE_LIMIT {
            return json!({"measured": false, "effective": Value::Null,
                "warnings": ["Performed note expansion exceeds the diagnostic bound."],
                "instrument_response": "unknown"});
        }
    }
    // Copy only playback data: a recipe and hidden metadata are irrelevant to this probe.
    let mut with_glide = MidiClip::new(clip.id, "Performance measurement", clip.start, clip.length);
    with_glide.notes = clip.playable_notes().collect();
    with_glide.loop_end = clip.loop_end;
    with_glide.bend = clip.bend.clone();
    with_glide.controllers = clip.controllers.clone();
    with_glide.transforms = stack.to_vec();
    let mut without_glide = with_glide.clone();
    if glide_requested {
        without_glide.transforms[stage_index] = disable_glide(&stack[stage_index]).unwrap();
    }
    // Curve playback clips notes at the first pass boundary and coalesces exact octave layers.
    final_notes.retain(|note| note.start < span);
    for note in &mut final_notes {
        note.length = note.length.min(span - note.start);
    }
    final_notes.sort_by_key(|note| (note.start, note.pitch));
    if stack.iter().any(
        |s| matches!(s, NoteTransform::Octaves { above, below } if *above > 0.0 || *below > 0.0),
    ) {
        final_notes.dedup_by(|b, a| {
            a.start == b.start
                && a.end() == b.end()
                && a.pitch % 12 == b.pitch % 12
                && a.drum_voice == b.drum_voice
        });
    }
    let eligibility = if glide_requested {
        glide_transition_report(&final_notes, context, &project.tempo_map)
    } else {
        json!({"candidate_transitions": 0, "eligible_transitions": 0})
    };
    let a = with_glide.performed_curve_points(
        ClipCurve::Bend,
        &project.tempo_map,
        &project.signatures,
        0,
        auris_core::Ticks::ZERO,
        span,
    );
    let b = without_glide.performed_curve_points(
        ClipCurve::Bend,
        &project.tempo_map,
        &project.signatures,
        0,
        auris_core::Ticks::ZERO,
        span,
    );
    let glide = if glide_requested {
        compare_curve(&a, &b, span)
    } else {
        json!({"measured": true, "effective": false, "changed_samples": 0,
            "sample_count": 0, "max_delta": 0.0})
    };
    let mut generated_curves = serde_json::Map::new();
    generated_curves.insert("bend".into(), glide.clone());
    // Also distinguish the complete pitch stage from glide's isolated contribution.
    let mut without_bend = with_glide.clone();
    if let NoteTransform::Pitch { settings } = &mut without_bend.transforms[stage_index] {
        settings.scoop = 0.0;
        settings.vibrato = 0.0;
        settings.fall = 0.0;
        settings.glide_ms = 0.0;
    }
    let stage_bend = without_bend.performed_curve_points(
        ClipCurve::Bend,
        &project.tempo_map,
        &project.signatures,
        0,
        Ticks::ZERO,
        span,
    );
    generated_curves.insert(
        "all_bend_gestures".into(),
        compare_curve(&a, &stage_bend, span),
    );
    for (which, name, active) in [
        (
            ClipCurve::Controller(1),
            "modulation",
            settings.modulation > 0.0,
        ),
        (
            ClipCurve::Controller(7),
            "volume_swell",
            settings.volume_swell > 0.0,
        ),
    ] {
        if !active {
            continue;
        }
        let mut without = with_glide.clone();
        without.transforms[stage_index] = disable_controller(&stack[stage_index], which).unwrap();
        let with_points = with_glide.performed_curve_points(
            which,
            &project.tempo_map,
            &project.signatures,
            0,
            auris_core::Ticks::ZERO,
            span,
        );
        let mut without_points = without.performed_curve_points(
            which,
            &project.tempo_map,
            &project.signatures,
            0,
            auris_core::Ticks::ZERO,
            span,
        );
        // An absent CC7 lane means full channel volume in playback, whereas the generic
        // curve interpolator's empty-lane value is zero (appropriate for bend and CC1).
        if which == ClipCurve::Controller(7) && without_points.is_empty() {
            without_points.push(CurvePoint {
                at: Ticks::ZERO,
                value: 1.0,
            });
        }
        generated_curves.insert(
            name.into(),
            compare_curve(&with_points, &without_points, span),
        );
    }
    let effective = glide["effective"].as_bool();
    let mut warnings = Vec::new();
    match effective {
        Some(false) if glide_requested => warnings.push("glide_ms did not change the performed bend curve. Check the eligibility counts: connections must be monophonic, at most 60 ms apart, within two octaves, and notes must be at least 80 ms long. Repeated pitches have no pitch movement.".to_string()),
        Some(false) => {},
        Some(true) => warnings.push("The project bend curve changed. A hosted instrument may ignore or reinterpret pitch bend, so audible response is not measured here.".to_string()),
        None => warnings.push("Pitch-curve ablation was incomplete, so glide effectiveness is unknown.".to_string()),
    }
    json!({
        "measured": generated_curves.values().all(|report| report["measured"] == true),
        "control": "glide_ms",
        "glide_ms": settings.glide_ms,
        "effective": effective,
        "changed_samples": glide["changed_samples"],
        "sample_count": glide["sample_count"],
        "max_semitone_delta": glide["max_delta"],
        "generated_curves": generated_curves,
        "eligibility": eligibility,
        "instrument_response": "unknown",
        "warnings": warnings,
    })
}

pub(super) fn measure(
    clip: &MidiClip,
    stack: &[NoteTransform],
    voice: Option<&str>,
    project: &Project,
) -> Value {
    let mut notes: Vec<_> = clip
        .playable_notes()
        .filter(|note| voice.is_none_or(|name| note.drum_voice == name))
        .take(NOTE_LIMIT + 1)
        .collect();
    let mut complete = notes.len() <= NOTE_LIMIT && stack.len() <= STAGE_LIMIT;
    notes.truncate(NOTE_LIMIT);
    let sampled = notes.len();
    let context = PerformanceContext {
        bpm: project.tempo_map.bpm_at(clip.start),
        pass: 0,
        start: clip.start,
        length: clip.length,
        signatures: &project.signatures,
    };
    let mut stages = Vec::new();
    let mut pitch_effects = Vec::new();
    for (stage_index, stage) in stack.iter().take(STAGE_LIMIT).enumerate() {
        // Ghost generation scans the time grid, so a note-count cap alone cannot bound it.
        if matches!(
            stage,
            NoteTransform::Ghost { .. } | NoteTransform::Brush { .. }
        ) && clip.length > Ticks::QUARTER * 64
        {
            complete = false;
            stages.push(json!({"stage":family(stage),"measured":false,"warnings":["This generative stage spans more than 64 quarter notes. Measurement stops here to bound time-grid work; inspect a shorter clip to measure its effect."]}));
            break;
        }
        if matches!(stage, NoteTransform::ForDrumVoice { .. }) {
            complete = false;
            stages.push(json!({"stage":"for_drum_voice","warnings":["Select drum_voice to measure this writer's stages; the whole-clip measurement stops before nested scopes."]}));
            break;
        }
        let mut attacks = BTreeMap::new();
        for note in &notes {
            *attacks.entry(note.start.raw()).or_insert(0_usize) += 1;
        }
        let chord_groups = attacks.values().filter(|count| **count > 1).count();
        let slots = performed_note_slots(notes.clone(), std::slice::from_ref(stage), context);
        let paired = || {
            notes
                .iter()
                .zip(&slots)
                .filter_map(|(before, after)| after.as_ref().map(|after| (before, after)))
        };
        let onsets = paired().filter(|(a, b)| a.start != b.start).count();
        let durations = paired().filter(|(a, b)| a.length != b.length).count();
        let velocities = paired().filter(|(a, b)| a.velocity != b.velocity).count();
        let pitches = paired().filter(|(a, b)| a.pitch != b.pitch).count();
        let removed = slots
            .iter()
            .take(notes.len())
            .filter(|slot| slot.is_none())
            .count();
        let added = slots.len().saturating_sub(notes.len());
        let unchanged = onsets + durations + velocities + pitches + removed + added == 0;
        let strum = matches!(
            stage,
            NoteTransform::Strum { .. } | NoteTransform::Stroke { .. }
        );
        let mut warnings: Vec<String> = Vec::new();
        if strum && chord_groups == 0 {
            warnings.push("Strum spreads simultaneous pitches; no simultaneous chord groups were found in the inspected notes. It cannot turn an arpeggio into chord strokes while preserving the score.".into());
        }
        if strum && unchanged && complete {
            warnings.push("This strum stage changes zero performed note events. Do not claim an effective stroke from the stored setting.".into());
        }
        let spread = strum
            .then(|| stroke_spreads(paired(), context.bpm))
            .flatten();
        if let Some(median) = spread.as_ref().and_then(|s| s["median_ms"].as_f64())
            && median < f64::from(crate::lint::TIGHT_STRUM_MS)
        {
            warnings.push(format!("Chords are struck from first to last string in {median} ms (median); under about 15 ms a listener hears every string at once. Widen spread_ms or set string_ms."));
        }
        let pitch = matches!(stage, NoteTransform::Pitch { .. })
            .then(|| pitch_report(clip, stack, stage_index, voice, project));
        if let Some(report) = &pitch {
            complete &= report["measured"] == true;
            pitch_effects.push(json!({"stage_index": stage_index, "report": report}));
        }
        let slide = matches!(stage, NoteTransform::Slide { .. }).then(|| {
            let mut report = slide_transition_report(&notes, context);
            report["added_notes"] = json!(added);
            report["measured"] = json!(complete);
            report["effective"] = if complete {
                json!(added > 0)
            } else {
                Value::Null
            };
            report
        });
        if let Some(report) = &slide {
            let eligible = report["eligible_transitions"].as_u64().unwrap_or(0);
            if eligible == 0 {
                warnings.push("No eligible melodic transition was found for slide; see ineligible_reasons for the bounded candidates inspected.".into());
            }
        }
        let stage_measured = pitch
            .as_ref()
            .is_none_or(|report| report["measured"] == true);
        stages.push(json!({"stage":family(stage),"measured":stage_measured,
            "input_notes":notes.len(),"eligible_chord_groups":chord_groups,
            "changed_onsets":onsets,"changed_durations":durations,"changed_velocities":velocities,
            "changed_pitches":pitches,"added_notes":added,"removed_notes":removed,
            "note_events_unchanged":unchanged,"warnings":warnings,
            "strum_spread":spread,
            "slide":slide,
            "pitch":pitch}));
        notes = slots.into_iter().flatten().take(NOTE_LIMIT + 1).collect();
        complete &= notes.len() <= NOTE_LIMIT;
        notes.truncate(NOTE_LIMIT);
    }
    json!({"representation":"First loop pass, selected scope, stages in order using the playback performer. Note stages report performed note deltas. Pitch stages compare the final performed bend curve with that stage's glide disabled, preserving authored bends and other pitch gestures. Curve changes prove a project-level performance change; hosted-instrument audibility remains unknown.",
        "sampled_authored_notes":sampled,"note_limit":NOTE_LIMIT,"stage_limit":STAGE_LIMIT,
        "complete":complete,"truncated":!complete,"stages":stages,
        "pitch_effects":pitch_effects})
}

/// How long each performed chord takes from its first sounding note to its last, in ms.
///
/// Chords are the authored attacks shared by more than one note; a stroke's skipped strings
/// are absent from `paired` and do not count. `None` when no chord sounds.
fn stroke_spreads<'a>(
    paired: impl Iterator<Item = (&'a Note, &'a Note)>,
    bpm: f64,
) -> Option<Value> {
    let mut chords: BTreeMap<i64, (i64, i64, usize)> = BTreeMap::new();
    for (before, after) in paired {
        let start = after.start.raw();
        let entry = chords
            .entry(before.start.raw())
            .or_insert((start, start, 0));
        entry.0 = entry.0.min(start);
        entry.1 = entry.1.max(start);
        entry.2 += 1;
    }
    let ms_per_tick = 60_000.0 / (bpm.max(1e-6) * auris_core::Ticks::QUARTER.raw() as f64);
    let mut spreads: Vec<f64> = chords
        .values()
        .filter(|(_, _, count)| *count > 1)
        .map(|(first, last, _)| (last - first) as f64 * ms_per_tick)
        .collect();
    if spreads.is_empty() {
        return None;
    }
    spreads.sort_by(f64::total_cmp);
    let tenth = |value: f64| (value * 10.0).round() / 10.0;
    Some(json!({
        "chords": spreads.len(),
        "min_ms": tenth(spreads[0]),
        "median_ms": tenth(spreads[spreads.len() / 2]),
        "max_ms": tenth(spreads[spreads.len() - 1]),
    }))
}
