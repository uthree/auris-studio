//! Instrument facts and bounded musical observations, separate from aesthetic judgments.

use super::*;
use auris_core::project::{ClipCurve, InstrumentTrack};
use std::collections::BTreeMap;

pub(super) fn family(instrument: &InstrumentTrack, drum: bool) -> &'static str {
    if drum {
        return "drums";
    }
    match instrument.instrument_id.as_str() {
        "auris.physical.piano" | "auris.physical.hammered_dulcimer" => "keys",
        "auris.physical.guitar" | "auris.physical.electric_guitar" => "guitar",
        "auris.physical.bass" => "bass",
        "auris.physical.violin" | "auris.physical.cello" => "bowed_strings",
        "auris.physical.bell" | "auris.physical.mallet" => "pitched_percussion",
        "auris.physical.flute" | "auris.physical.clarinet" | "auris.physical.tin_whistle" => {
            "winds"
        }
        "auris.synth.chiptune" => "synth",
        // The subtractive patches are all synthesisers, whatever sound each one imitates.
        id if auris_synth::AnalogPatch::from_id(id).is_some() => "synth",
        auris_sampler::SAMPLER_ID => {
            match auris_sampler::stored_preset(&instrument.instrument_state) {
                Some(p) if p.bank == 128 => "drums",
                Some(p) if p.bank == 0 => match p.patch {
                    0..=7 | 16..=23 => "keys",
                    8..=15 | 112..=118 => "pitched_percussion",
                    24..=31 => "guitar",
                    32..=39 => "bass",
                    40..=51 => "bowed_strings",
                    56..=79 => "winds",
                    80..=103 => "synth",
                    _ => "unknown",
                },
                _ => "unknown",
            }
        }
        _ => "unknown",
    }
}

pub(super) fn capabilities(instrument: &InstrumentTrack, drum: bool) -> Value {
    let id = instrument.instrument_id.as_str();
    let physical = matches!(
        id,
        "auris.physical.piano"
            | "auris.physical.guitar"
            | "auris.physical.electric_guitar"
            | "auris.physical.bass"
            | "auris.physical.violin"
            | "auris.physical.bell"
            | "auris.physical.mallet"
    );
    let dulcimer = id == "auris.physical.hammered_dulcimer";
    let sampler = id == auris_sampler::SAMPLER_ID;
    let wind = matches!(id, "auris.physical.clarinet" | "auris.physical.tin_whistle");
    // The subtractive patches read the wheel as vibrato depth exactly as the chiptune voice does.
    let chiptune = id == "auris.synth.chiptune" || auris_synth::AnalogPatch::from_id(id).is_some();
    let native = physical || sampler || wind || chiptune || dulcimer;
    let status = |known: bool| {
        if known {
            "supported"
        } else if native {
            "unsupported"
        } else {
            "unknown"
        }
    };
    let known_family = family(instrument, drum);
    let suggestions: &[&str] = match known_family {
        "keys" => &[
            "velocity phrasing",
            "small attack spread",
            "sustain when supported",
        ],
        "guitar" => &["strum simultaneous chords", "muted tails", "sparse brushes"],
        "bass" => &["gate", "pickup ghosts", "slides", "coordinate with kick"],
        "bowed_strings" | "winds" => &["phrase dynamics", "long-note swell", "monophonic vibrato"],
        "pitched_percussion" => &["velocity phrasing", "gate", "keep ordinary pitch fixed"],
        "drums" => &[
            "voice-scoped ghosts",
            "accents",
            "fills",
            "preserve long rests",
        ],
        "synth" => &["parameter automation", "pitch gestures", "phrase dynamics"],
        _ => &[
            "inspect exact controls",
            "verify MIDI controller response before use",
        ],
    };
    json!({"family":known_family,
        "family_evidence":if sampler {"bank/program convention; does not prove this patch's articulations"} else if known_family == "unknown" {"no verified instrument-family mapping"} else {"built-in instrument identity or drum track kind"},
        "pitch_bend":status(native),"cc7_volume":status(native),
        "cc1_modulation":if sampler {"unknown"} else {status(id == "auris.physical.violin" || chiptune)},
        "cc1_meaning":if id == "auris.physical.violin" {"bow pressure"} else if chiptune {"vibrato depth"} else {"patch-specific; do not assume a vibrato or filter mapping"},
        "cc11_expression":status(physical || sampler || wind || dulcimer),
        "cc64_sustain":status(id == "auris.physical.piano" || sampler),
        "controller_evidence":"Known built-in event handlers; SoundFont CC1 and hosted MIDI mappings require patch-specific confirmation",
        "channel_scope":"Bend and CC affect every overlapping voice on the track; independent gestures require separate tracks",
        "recommendations":suggestions})
}

pub(super) fn phrase(project: &Project, clip: &MidiClip) -> Value {
    let mut notes: Vec<_> = clip.playable_notes().take(2049).collect();
    let performance =
        crate::live_agent::performance::measure_arrangement_performance(clip, project, None);
    let complete = notes.len() <= 2048
        && clip.transforms.len() <= 32
        && (clip.length <= Ticks::QUARTER * 64
            || !clip
                .performance_curves()
                .any(|c| clip.has_generated_curve(c)))
        && clip.bend.len() <= 4096
        && clip.controllers.values().all(|p| p.len() <= 4096)
        && performance["complete"] == true;
    notes.truncate(2048);
    notes.sort_by_key(|n| n.start);
    let mut attacks = BTreeMap::<_, usize>::new();
    let mut end = Ticks::ZERO;
    let mut overlaps = 0;
    let mut long = 0;
    for note in &notes {
        *attacks.entry(note.start).or_default() += 1;
        overlaps += usize::from(note.start < end);
        end = end.max(note.start + note.length);
        let start = project
            .tempo_map
            .ticks_to_seconds(clip.start + note.start)
            .0;
        let finish = project
            .tempo_map
            .ticks_to_seconds(clip.start + note.start + note.length)
            .0;
        long += usize::from(finish - start >= 0.6);
    }
    let curves: Vec<_> = if complete {
        clip.performance_curves()
            .map(|curve| {
                let points = clip.performed_curve_points(
                    curve,
                    &project.tempo_map,
                    &project.signatures,
                    0,
                    Ticks::ZERO,
                    clip.length,
                );
                let baseline = if curve == ClipCurve::Controller(7) {
                    1.0
                } else {
                    0.0
                };
                let non_neutral = points.iter().any(|p| (p.value - baseline).abs() > 1e-6);
                json!({"curve":format!("{curve:?}"),"points":points.len(),"non_neutral":non_neutral,
                "min":points.iter().map(|p|p.value).reduce(f32::min),
                "max":points.iter().map(|p|p.value).reduce(f32::max)})
            })
            .collect()
    } else {
        Vec::new()
    };
    json!({"sampled_notes":notes.len(),"complete":complete,"simultaneous_chord_groups":attacks.values().filter(|n|**n>1).count(),
        "overlapping_notes":overlaps,"long_notes_600ms":long,"pitch_range":clip.pitch_range(),
        "performance":performance,
        "curve_effects":curves,"curve_measurement_complete":complete,
        "limitation":"First pass and bounded phrase data; musical quality and audible controller response need rendered comparison"})
}

pub(super) fn performance_delta(project: &Project, before: &MidiClip, after: &MidiClip) -> Value {
    if before.notes.len() > 2048
        || after.notes.len() > 2048
        || before.transforms.len() > 32
        || after.transforms.len() > 32
        || (before.length > Ticks::QUARTER * 64
            && before
                .performance_curves()
                .chain(after.performance_curves())
                .any(|c| before.has_generated_curve(c) || after.has_generated_curve(c)))
        || before.bend.len() > 4096
        || after.bend.len() > 4096
        || before
            .controllers
            .values()
            .chain(after.controllers.values())
            .any(|p| p.len() > 4096)
        || crate::live_agent::performance::measure_arrangement_performance(before, project, None)["complete"]
            != true
        || crate::live_agent::performance::measure_arrangement_performance(after, project, None)["complete"]
            != true
    {
        return json!({"complete":false,"reason":"Use a shorter phrase to measure actual event and controller deltas"});
    }
    let context = auris_core::PerformanceContext {
        bpm: project.tempo_map.bpm_at(before.start),
        pass: 0,
        start: before.start,
        length: before.length,
        signatures: &project.signatures,
    };
    let a = auris_core::performed_notes(
        before.playable_notes().collect(),
        &before.transforms,
        context,
    );
    let b =
        auris_core::performed_notes(after.playable_notes().collect(), &after.transforms, context);
    let mut curves: Vec<_> = before.performance_curves().collect();
    for curve in after.performance_curves() {
        if !curves.contains(&curve) {
            curves.push(curve);
        }
    }
    let curves:Vec<_>=curves.into_iter().map(|curve| {
        let a=before.performed_curve_points(curve,&project.tempo_map,&project.signatures,0,Ticks::ZERO,before.length);
        let b=after.performed_curve_points(curve,&project.tempo_map,&project.signatures,0,Ticks::ZERO,after.length);
        json!({"curve":format!("{curve:?}"),"points_changed":a!=b,"before_points":a.len(),"after_points":b.len()})
    }).collect();
    json!({"complete":true,"representation":"First performed pass and combined authored/generated curve points; not a listening judgment",
        "note_events_changed":a!=b,"before_note_count":a.len(),"after_note_count":b.len(),"curves":curves})
}

pub(super) fn check_performance(
    project: &Project,
    clip: &MidiClip,
    instrument: Option<&InstrumentTrack>,
    drum: bool,
    issues: &mut Vec<Value>,
    change: usize,
) {
    let info = phrase(project, clip);
    let add = |issues: &mut Vec<Value>, code, message| {
        issues.push(json!({"change":change,"clip":clip.id.0,"severity":"warning","code":code,"message":message}))
    };
    if info["sampled_notes"] == 0 {
        add(
            issues,
            "empty_phrase",
            "The clip has no playable notes; performance settings alone cannot create the requested musical phrase",
        );
    }
    if info["complete"] != true {
        add(
            issues,
            "measurement_incomplete",
            "Phrase or curve inspection was bounded; inspect a shorter clip before claiming a verified performance change",
        );
    }
    if let Some(instrument) = instrument {
        let fixed = matches!(
            family(instrument, drum),
            "keys" | "pitched_percussion" | "drums"
        );
        if fixed && clip.has_pitch_performance() {
            add(
                issues,
                "fixed_pitch_gesture",
                "Pitch gestures are active on a normally fixed-pitch instrument; reset pitch unless this is intentional",
            );
        }
        if instrument.is_hosted()
            && clip
                .performance_curves()
                .any(|c| clip.has_generated_curve(c))
        {
            add(
                issues,
                "controller_response_unknown",
                "The hosted patch's MIDI mapping is unknown; a generated curve does not prove an audible response",
            );
        }
    }
    if info["overlapping_notes"].as_u64().unwrap_or(0) > 0
        && clip
            .performance_curves()
            .any(|c| clip.has_generated_curve(c))
    {
        add(
            issues,
            "channel_gesture_overlap",
            "Overlapping voices may suppress automatic gestures or share authored channel controls; inspect the generated curves",
        );
    }
}

impl Session {
    pub(super) fn inspect_arrangement(
        &mut self,
        start_bar: u32,
        bars: u32,
        offset: usize,
    ) -> Result<String, String> {
        if start_bar == 0 || !(1..=128).contains(&bars) || start_bar.checked_add(bars).is_none() {
            return Err("Use start_bar >= 1 and bars 1..128 with a representable end".into());
        }
        let start = self.project.signatures.bar_start(start_bar);
        let end = self.project.signatures.bar_start(start_bar + bars);
        if offset > self.project.tracks.len() {
            return Err("Track offset exceeds track count".into());
        }
        let ids: Vec<_> = self
            .project
            .tracks
            .iter()
            .skip(offset)
            .take(16)
            .map(|t| t.id)
            .collect();
        let mut tracks = Vec::new();
        let mut issues = Vec::new();
        let mut complete = offset + ids.len() == self.project.tracks.len();
        for id in ids {
            let descriptors = self.instrument_descriptors(id);
            let parameters: Vec<_> = descriptors
                .iter()
                .take(32)
                .map(|p| {
                    let automatable=self.automatable(ParamTarget::Instrument {track:id,param:p.id}).is_some();
                    json!({"key":p.key,"min":p.min,"max":p.max,"unit":p.unit,"steps":p.steps,"automatable":automatable,
                        "choices":p.choices.iter().take(8).collect::<Vec<_>>(),"choice_count":p.choices.len()})
                })
                .collect();
            let track = self
                .project
                .track(id)
                .ok_or("Track disappeared during inspection")?;
            let mut clips = Vec::new();
            if let Some(midi) = track.kind.note_clips() {
                let selected: Vec<_> = midi
                    .iter()
                    .filter(|c| c.start < end && c.sounding_end() > start)
                    .collect();
                complete &= selected.len() <= 16;
                for clip in selected.iter().take(16) {
                    check_performance(
                        &self.project,
                        clip,
                        track.kind.as_instrument(),
                        track.kind.is_drum(),
                        &mut issues,
                        0,
                    );
                    let info = phrase(&self.project, clip);
                    complete &= info["complete"] == true;
                    clips.push(json!({"clip":clip.id.0,"name":clip.name,"start_tick":clip.start.raw(),"end_tick":clip.sounding_end().raw(),"muted":clip.muted,"phrase":info}));
                }
                for pair in selected.windows(2) {
                    if !pair[0].notes.is_empty() && pair[0].notes == pair[1].notes {
                        issues.push(json!({"severity":"suggestion","code":"repeated_phrase","track":id.0,"clips":[pair[0].id.0,pair[1].id.0],"message":"Adjacent clips repeat the same authored phrase. Consider a rest, response, orchestration change or layer if the requested style calls for contrast; repetition itself is not an error"}));
                    }
                }
            }
            tracks.push(json!({"track":id.0,"name":track.name,"kind":track.kind.label(),
                "capabilities":track.kind.as_instrument().map(|i|capabilities(i,track.kind.is_drum())),
                "parameters":parameters,"parameter_limit":32,"parameter_count":descriptors.len(),"parameters_complete":descriptors.len()<=32,"clips":clips}));
        }
        let next = offset + tracks.len();
        let sections:Vec<_>=self.project.sections.spans_in(start,end).into_iter().map(|s|json!({"start_tick":s.start.raw(),"end_tick":s.end.raw(),"label":s.label,"instance":s.instance})).collect();
        Ok(json!({"revision":self.agent_document_revision()?,"start_tick":start.raw(),"end_tick":end.raw(),
            "sections":sections,"tracks":tracks,"next_offset":(next<self.project.tracks.len()).then_some(next),
            "complete":complete,"issues":issues,
            "workflow":"Plan changes with purposes. Validate up to sixteen changes, review warnings and plan_id, then apply with the same revision. Read back independently; preview/inspect_audio/listen the same bars before and after. Bound local repair attempts to two; preserve melody, lyrics, harmony and length unless explicitly requested otherwise."}).to_string())
    }
}
