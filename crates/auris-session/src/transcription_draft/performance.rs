//! Version-two performance semantics, validated before a document transaction starts.

use super::MultitrackTranscription;
use auris_core::Note;
use auris_core::project::CurvePoint;
use auris_core::time::{SignatureMap, SignaturePoint, TempoMap, TempoPoint, Ticks, TimeSignature};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How source musical time relates to the destination.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelinePolicy {
    /// Place source seconds through the existing destination maps without changing them.
    PreserveDestination,
    /// Explicitly replace both global maps; requires source second zero and insertion tick zero.
    ReplaceDestination,
}

/// A source tempo anchor; seconds and ticks must agree with all preceding anchors.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftTempo {
    /// Source-relative tick, at 960 ticks per quarter note.
    pub tick: i64,
    /// Absolute source-audio second at this tick.
    pub seconds: f64,
    /// Constant tempo until the next anchor, 10..999 BPM.
    pub bpm: f64,
}

/// A source meter anchor on an exact bar boundary.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftMeter {
    /// Source-relative tick, at 960 ticks per quarter note.
    pub tick: i64,
    /// Beats per bar, 1..32.
    pub numerator: u32,
    /// Beat denominator, one of 1, 2, 4, 8, 16.
    pub denominator: u32,
}

/// Supported interpolation. Other interpolation modes are rejected by the schema.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DraftInterpolation {
    /// Straight segments between points; endpoint values are held outside the points.
    Linear,
}

/// One control sample in absolute source-audio seconds.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftControlPoint {
    /// Absolute source-audio second, including the selected range's terminal instant.
    pub seconds: f64,
    /// Semitones for bend, normalized 0..1 for MIDI CC.
    pub value: f32,
}

/// A complete control curve including its explicit terminal state.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftControl {
    /// Linear interpolation and endpoint hold semantics.
    pub interpolation: DraftInterpolation,
    /// Ordered unique times; includes source start and source end.
    pub points: Vec<DraftControlPoint>,
    /// Value at source end. Must equal the last point; no implicit reset is inserted.
    pub end_value: f32,
}

/// A pitch curve in semitones, independent of an instrument's wire bend range.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftBend {
    /// Positive maximum absolute bend, at most 12 semitones.
    pub range_semitones: f32,
    /// Absolute semitone offsets, never normalized MIDI bend integers.
    pub curve: DraftControl,
}

/// A lyric span retaining written text separately from pronunciation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftLyric {
    /// Ordered indices in the part's original note list; no note may belong to two spans.
    pub notes: Vec<usize>,
    /// Inclusive source-audio onset of the first note.
    pub start_seconds: f64,
    /// Exclusive source-audio end of the final note.
    pub end_seconds: f64,
    /// Complete written lyrics, kept in provenance; each note is labelled with the kana it sings.
    pub written: String,
    /// Optional reading, retained in provenance without replacing written lyrics.
    pub reading: Option<String>,
    /// Explicit IPA token lists, one per addressed note; no implicit phoneme conversion.
    pub phonemes: Vec<Vec<String>>,
    /// Model-declared confidence, 0..1.
    pub confidence: Option<f32>,
}

/// One channel's performance, parallel to the root part list.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftPartPerformance {
    /// Original zero-based MIDI channel, 0..15. Each part is routed to its own track.
    pub channel: u8,
    /// Optional absolute semitone curve.
    pub bend: Option<DraftBend>,
    /// MIDI controller number 0..127 to normalized control curve; 64 is sustain pedal.
    #[serde(default)]
    pub controllers: BTreeMap<u8, DraftControl>,
    /// Lyrics create a singer track; drums and explicit instrument sound IDs are then invalid.
    #[serde(default)]
    pub lyrics: Vec<DraftLyric>,
}

/// Explicit version-two timeline and performance contract.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftPerformance {
    /// Whether to preserve or explicitly replace the destination's global maps.
    pub timeline_policy: TimelinePolicy,
    /// Source resolution, exactly 960 ticks per quarter note.
    pub ticks_per_quarter: u32,
    /// Ordered source tempo anchors, first at tick zero and source.start_seconds.
    pub tempos: Vec<DraftTempo>,
    /// Ordered source meters, first at tick zero; all changes must be bar aligned.
    pub meters: Vec<DraftMeter>,
    /// Performance for every part, in exactly the root tracks order.
    pub parts: Vec<DraftPartPerformance>,
}

impl DraftPerformance {
    pub(crate) fn maps(&self, source_start: f64) -> Result<(TempoMap, SignatureMap), String> {
        if self.ticks_per_quarter != 960
            || !(1..=4096).contains(&self.tempos.len())
            || !(1..=4096).contains(&self.meters.len())
        {
            return Err("Invalid performance map resolution or size".into());
        }
        let ordered = |ticks: Vec<i64>| {
            ticks[0] == 0
                && ticks.iter().all(|t| (0..=200_000_000).contains(t))
                && ticks.windows(2).all(|w| w[0] < w[1])
        };
        if !ordered(self.tempos.iter().map(|p| p.tick).collect())
            || !ordered(self.meters.iter().map(|p| p.tick).collect())
            || self.tempos.iter().any(|p| {
                !p.seconds.is_finite() || !(TempoMap::MIN_BPM..=TempoMap::MAX_BPM).contains(&p.bpm)
            })
        {
            return Err("Invalid source tempo/meter anchors".into());
        }
        let tempos = TempoMap::from_points(
            self.tempos
                .iter()
                .map(|p| TempoPoint {
                    tick: Ticks(p.tick),
                    bpm: p.bpm,
                    ramp: false,
                })
                .collect(),
        )
        .map_err(|e| e.to_string())?;
        if self.tempos.iter().any(|p| {
            (tempos.ticks_to_seconds(Ticks(p.tick)).0 + source_start - p.seconds).abs() > 1e-6
        }) {
            return Err("Source seconds and musical ticks disagree".into());
        }
        if self.meters.iter().any(|p| {
            !TimeSignature::NUMERATORS.contains(&p.numerator)
                || !TimeSignature::DENOMINATORS.contains(&p.denominator)
        }) {
            return Err("Unsupported time signature".into());
        }
        let points: Vec<_> = self
            .meters
            .iter()
            .map(|p| SignaturePoint {
                tick: Ticks(p.tick),
                signature: TimeSignature::new(p.numerator, p.denominator),
            })
            .collect();
        let meters = SignatureMap::from_points(points.clone());
        if meters.points() != points {
            return Err("Meter changes must be exact bar boundaries".into());
        }
        Ok((tempos, meters))
    }

    pub(crate) fn validate(&self, draft: &MultitrackTranscription) -> Result<(), String> {
        let (tempos, _) = self.maps(draft.source.start_seconds)?;
        if self.parts.len() != draft.tracks.len()
            || self
                .tempos
                .iter()
                .any(|p| p.seconds > draft.source.end_seconds)
            || self.meters.iter().any(|p| {
                tempos.ticks_to_seconds(Ticks(p.tick)).0
                    > draft.source.end_seconds - draft.source.start_seconds
            })
        {
            return Err("Performance parts or maps exceed the selected source".into());
        }
        let mut count = 0usize;
        for (part, track) in self.parts.iter().zip(&draft.tracks) {
            if part.channel > 15
                || part.controllers.len() > 128
                || (!part.lyrics.is_empty() && (track.drums || track.sound_id.is_some()))
                || part.lyrics.len() > track.notes.len()
            {
                return Err("Invalid channel or singer/instrument mapping".into());
            }
            let validate_curve =
                |curve: &DraftControl, lo: f32, hi: f32| -> Result<usize, String> {
                    if curve.points.len() < 2
                        || curve.points.len() > 100_000
                        || curve
                            .points
                            .first()
                            .is_none_or(|p| p.seconds != draft.source.start_seconds)
                        || curve.points.last().is_none_or(|p| {
                            p.seconds != draft.source.end_seconds || p.value != curve.end_value
                        })
                        || curve.points.iter().any(|p| {
                            !p.seconds.is_finite()
                                || !(lo..=hi).contains(&p.value)
                                || p.seconds < draft.source.start_seconds
                                || p.seconds > draft.source.end_seconds
                        })
                        || curve
                            .points
                            .windows(2)
                            .any(|w| w[0].seconds >= w[1].seconds)
                    {
                        return Err(
                            "Invalid ordered control curve, range or explicit endpoint state"
                                .into(),
                        );
                    }
                    Ok(curve.points.len())
                };
            if let Some(bend) = &part.bend {
                if !bend.range_semitones.is_finite()
                    || bend.range_semitones <= 0.0
                    || bend.range_semitones > 12.0
                {
                    return Err("Bend range must be positive and at most 12 semitones".into());
                }
                count += validate_curve(&bend.curve, -bend.range_semitones, bend.range_semitones)?;
            }
            for (&cc, curve) in &part.controllers {
                if cc > 127 {
                    return Err("MIDI controller must be 0..127".into());
                }
                count += validate_curve(curve, 0.0, 1.0)?;
            }
            let mut used = BTreeSet::new();
            for lyric in &part.lyrics {
                if lyric.notes.is_empty()
                    || lyric.notes.len() != lyric.phonemes.len()
                    || lyric.written.trim().is_empty()
                    || lyric.written.len() > 4096
                    || lyric.reading.as_ref().is_some_and(|s| s.len() > 4096)
                    || lyric.confidence.is_some_and(|c| !(0.0..=1.0).contains(&c))
                    || lyric
                        .notes
                        .iter()
                        .any(|&i| i >= track.notes.len() || !used.insert(i))
                    || lyric.phonemes.iter().any(|tokens| {
                        tokens.is_empty()
                            || tokens.len() > 128
                            || tokens.iter().any(|s| s.trim().is_empty() || s.len() > 64)
                    })
                {
                    return Err("Invalid lyric alignment, pronunciation or confidence".into());
                }
                let first = &track.notes[lyric.notes[0]];
                let last = &track.notes[*lyric.notes.last().expect("nonempty")];
                if lyric.start_seconds != first.start
                    || lyric.end_seconds != last.end
                    || lyric
                        .notes
                        .windows(2)
                        .any(|w| track.notes[w[0]].end > track.notes[w[1]].start)
                {
                    return Err(
                        "Lyric spans must match ordered, nonoverlapping addressed notes".into(),
                    );
                }
                count += lyric.notes.len();
            }
        }
        if count > 200_000 {
            return Err("Too many performance events".into());
        }
        Ok(())
    }
}

pub(crate) struct PreparedPerformance {
    pub bend: Vec<CurvePoint>,
    pub controllers: BTreeMap<u8, Vec<CurvePoint>>,
    pub end: Ticks,
    pub singer: bool,
}

impl DraftPartPerformance {
    pub(crate) fn prepare(
        &self,
        notes: &mut [Note],
        map: impl Fn(f64) -> Ticks,
    ) -> Result<PreparedPerformance, String> {
        let convert = |curve: &DraftControl| -> Result<Vec<CurvePoint>, String> {
            let points: Vec<_> = curve
                .points
                .iter()
                .map(|p| CurvePoint {
                    at: map(p.seconds),
                    value: p.value,
                })
                .collect();
            if points.windows(2).any(|w| w[0].at >= w[1].at) {
                return Err("Control points collapse at destination tick resolution".into());
            }
            Ok(points)
        };
        let bend = self
            .bend
            .as_ref()
            .map(|b| convert(&b.curve))
            .transpose()?
            .unwrap_or_default();
        let controllers: BTreeMap<_, _> = self
            .controllers
            .iter()
            .map(|(&cc, curve)| convert(curve).map(|curve| (cc, curve)))
            .collect::<Result<_, _>>()?;
        let end = bend
            .iter()
            .chain(controllers.values().flatten())
            .map(|p| p.at + Ticks(1))
            .max()
            .unwrap_or(Ticks::ZERO);
        for lyric in &self.lyrics {
            for (&index, phonemes) in lyric.notes.iter().zip(&lyric.phonemes) {
                // One note, one mora: each note is labelled with the kana it sings.
                notes[index].lyric = auris_vocal::kana::phonemes_to_kana(phonemes)
                    .unwrap_or_else(|| phonemes.concat());
                notes[index].phonemes = phonemes.clone();
            }
        }
        Ok(PreparedPerformance {
            bend,
            controllers,
            end,
            singer: !self.lyrics.is_empty(),
        })
    }
}
