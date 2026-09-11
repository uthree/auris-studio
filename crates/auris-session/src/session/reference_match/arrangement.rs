//! Clip-local, non-destructive arrangement proposals for rendered-audio search.

use std::collections::BTreeSet;

use auris_core::{
    ClipId, Expression, GhostNotes, GhostPattern, MidiClip, Note, NoteTransform, PitchPerformance,
    Project, StrokeDirection, Strum, StrumClock, Subdivision,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Swing,
    GhostDensity,
    GhostVelocity,
    GhostLength,
    GhostVariation,
    GhostPattern,
    Mute,
    Slide,
    StrumSpread,
    StrumUpVelocity,
    StrumLowAccent,
    StrumUpNotes,
    StrumDirection,
    Shared,
    Scoop,
    ScoopLength,
    Vibrato,
    VibratoRate,
    VibratoDelay,
    Fall,
    FallLength,
    Glide,
}

/// An address uses clip and voice identity, never a transform index that insertion can shift.
#[derive(Clone)]
pub(super) struct Dial {
    clip: ClipId,
    voice: Option<String>,
    ghost_pitch: Option<u8>,
    control: Control,
}

pub(super) fn dials(project: &Project) -> Vec<Dial> {
    let mut out = Vec::new();
    for track in &project.tracks {
        if track.mixer.mute {
            continue;
        }
        let Some(instrument) = track.kind.as_instrument() else {
            continue;
        };
        for clip in &instrument.clips {
            if clip.muted || clip.playable_notes().next().is_none() {
                continue;
            }
            let scoped_kit = track.kind.is_drum()
                && (clip
                    .playable_notes()
                    .any(|note| !note.drum_voice.is_empty())
                    || clip
                        .transforms
                        .iter()
                        .any(|stage| matches!(stage, NoteTransform::ForDrumVoice { .. })));
            let voices: Vec<Option<String>> = if scoped_kit {
                let mut voices: Vec<_> = clip
                    .playable_notes()
                    .map(|note| Some(note.drum_voice))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                // Existing whole-kit settings remain searchable too, but new stages belong
                // to a writer scope. In particular, swing cannot move an offbeat twice.
                voices.insert(0, None);
                voices
            } else {
                vec![None]
            };
            for voice in voices {
                let notes: Vec<_> = clip
                    .playable_notes()
                    .filter(|note| voice.as_ref().is_none_or(|name| &note.drum_voice == name))
                    .collect();
                let stack = scope(&clip.transforms, voice.as_deref());
                // An authored, unscoped kit must not repeat a simultaneous kick/snare/hat
                // chord. Named writer scopes already isolate the intended part.
                let ghost_pitch = track.kind.is_drum().then(|| dominant_pitch(&notes));
                let mut controls = vec![
                    Control::GhostDensity,
                    Control::GhostVelocity,
                    Control::GhostLength,
                    Control::GhostVariation,
                    Control::GhostPattern,
                    Control::Mute,
                ];
                if !stack.iter().any(|stage| {
                    matches!(stage,
                    NoteTransform::Swing { subdivision, .. } if subdivision.is_triplet())
                }) {
                    controls.insert(0, Control::Swing);
                }
                let expression = expression(stack);
                if expression.timing > 0.0 || expression.velocity > 0.0 {
                    controls.push(Control::Shared);
                }
                if scoped_kit && voice.is_none() {
                    controls.retain(|control| stack.iter().any(|stage| control.answers(stage)));
                } else if scoped_kit
                    && clip.transforms.iter().any(|stage| {
                        matches!(stage,
                        NoteTransform::Swing { percent, .. } if *percent > 50)
                    })
                {
                    controls.retain(|control| *control != Control::Swing);
                }
                if !track.kind.is_drum() {
                    if monophonic(&notes) {
                        controls.push(Control::Slide);
                        if pitch_eligible(project, clip.id) {
                            controls.extend([
                                Control::Scoop,
                                Control::Vibrato,
                                Control::Fall,
                                Control::Glide,
                            ]);
                            let pitch = pitch(stack);
                            if pitch.scoop > 0.0 {
                                controls.push(Control::ScoopLength);
                            }
                            if pitch.vibrato > 0.0 {
                                controls.extend([Control::VibratoRate, Control::VibratoDelay]);
                            }
                            if pitch.fall > 0.0 {
                                controls.push(Control::FallLength);
                            }
                        }
                    }
                    if has_chords(&notes) {
                        controls.extend([
                            Control::StrumSpread,
                            Control::StrumUpVelocity,
                            Control::StrumLowAccent,
                            Control::StrumUpNotes,
                            Control::StrumDirection,
                        ]);
                    }
                }
                out.extend(controls.into_iter().map(|control| Dial {
                    clip: clip.id,
                    voice: voice.clone(),
                    ghost_pitch,
                    control,
                }));
            }
        }
    }
    out
}

impl Dial {
    pub(super) fn adjust(&self, project: &mut Project, original: &Project, occurrence: usize) {
        // Recipe proposals can change the phrase after the dial list was captured.
        if self.control.is_pitch() && !pitch_eligible(project, self.clip) {
            return;
        }
        let Some((_, before)) = original.midi_clip(self.clip) else {
            return;
        };
        let Some(clip) = project.midi_clip_mut(self.clip) else {
            return;
        };
        let base = scope(&before.transforms, self.voice.as_deref());
        let held = scope(&clip.transforms, self.voice.as_deref());
        let Some(stage) = self.proposal(held, base, before, occurrence) else {
            return;
        };
        let stack = scope_mut(&mut clip.transforms, self.voice.as_deref());
        if let Some(at) = stack.iter().position(|stage| self.control.answers(stage)) {
            if !base.iter().any(|stage| self.control.answers(stage)) && !active(&stage) {
                stack.remove(at);
            } else {
                stack[at] = stage;
            }
        } else if active(&stage) {
            insert(stack, stage);
        }
        // Returning a new scoped stage to neutral must not leave an empty wrapper behind.
        if let Some(voice) = &self.voice {
            clip.transforms.retain(|stage| {
                !matches!(stage, NoteTransform::ForDrumVoice { voice: name, transforms }
                    if name == voice && transforms.is_empty()
                        && !before.transforms.iter().any(|old| matches!(old,
                            NoteTransform::ForDrumVoice { voice: old_name, .. } if old_name == voice)))
            });
        }
    }

    fn proposal(
        &self,
        held: &[NoteTransform],
        base: &[NoteTransform],
        original: &MidiClip,
        occurrence: usize,
    ) -> Option<NoteTransform> {
        let seed = original
            .recipe
            .as_ref()
            .map_or(original.id.0, |recipe| recipe.seed);
        let value = self.control.value(held);
        let baseline = self.control.value(base);
        let next = if self.control == Control::GhostPattern {
            category(baseline, occurrence, 4)
        } else if self.control == Control::StrumDirection {
            category(baseline, occurrence, 3)
        } else {
            let direction = if occurrence.is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let (step, radius, low, high) = self.control.bounds();
            if !value.is_finite() || !baseline.is_finite() {
                return None;
            }
            super::bounded(value, baseline, direction, step, radius, low, high)
        };
        Some(match self.control {
            Control::Swing => NoteTransform::Swing {
                percent: next.round() as u8,
                subdivision: held
                    .iter()
                    .find_map(|stage| match stage {
                        NoteTransform::Swing { subdivision, .. } => Some(*subdivision),
                        _ => None,
                    })
                    .unwrap_or(Subdivision::Sixteenth),
            },
            Control::GhostDensity
            | Control::GhostVelocity
            | Control::GhostLength
            | Control::GhostVariation
            | Control::GhostPattern => {
                let mut settings = ghosts(held, seed, self.ghost_pitch);
                match self.control {
                    Control::GhostDensity => settings.density = next,
                    Control::GhostVelocity => settings.velocity = next,
                    Control::GhostLength => settings.length_ms = next,
                    Control::GhostVariation => settings.variation = next,
                    Control::GhostPattern => {
                        settings.pattern = [
                            GhostPattern::Pickup,
                            GhostPattern::Sixteenths,
                            GhostPattern::Offbeats,
                            GhostPattern::Repeating,
                        ][next as usize];
                    }
                    _ => unreachable!(),
                }
                // A new/dormant placement or loudness control must produce an audible
                // candidate. Activate only its companion, preserving existing active values.
                if self.control != Control::GhostDensity && settings.density == 0.0 {
                    settings.density = 0.15;
                }
                if self.control != Control::GhostVelocity && settings.velocity == 0.0 {
                    settings.velocity = 0.15;
                }
                if self.control == Control::GhostVariation {
                    settings.pattern = GhostPattern::Repeating;
                }
                NoteTransform::Ghost { settings }
            }
            Control::Mute => NoteTransform::Mute { amount: next },
            Control::Slide => NoteTransform::Slide { amount: next },
            Control::StrumSpread
            | Control::StrumUpVelocity
            | Control::StrumLowAccent
            | Control::StrumUpNotes
            | Control::StrumDirection => {
                let mut settings = strum(held);
                match self.control {
                    Control::StrumSpread => settings.spread_ms = next,
                    Control::StrumUpVelocity => settings.up_velocity = next,
                    Control::StrumLowAccent => settings.low_accent = next,
                    Control::StrumUpNotes => settings.up_notes = next.round() as u8,
                    Control::StrumDirection => {
                        settings.direction = [
                            StrokeDirection::LowToHigh,
                            StrokeDirection::HighToLow,
                            StrokeDirection::Alternate,
                        ][next as usize];
                        if settings.spread_ms == 0.0 {
                            settings.spread_ms = 8.0;
                        }
                    }
                    _ => unreachable!(),
                }
                // Upstroke controls need an upstroke. Retain an intentional all-up hand.
                if matches!(
                    self.control,
                    Control::StrumUpVelocity | Control::StrumUpNotes
                ) && settings.direction == StrokeDirection::LowToHigh
                {
                    settings.direction = StrokeDirection::Alternate;
                }
                NoteTransform::Strum { settings }
            }
            Control::Shared => {
                let mut settings = expression(held);
                settings.shared = next;
                NoteTransform::Expression { settings }
            }
            _ => {
                let mut settings = pitch(held);
                match self.control {
                    Control::Scoop => settings.scoop = next,
                    Control::ScoopLength => settings.scoop_ms = next,
                    Control::Vibrato => settings.vibrato = next,
                    Control::VibratoRate => settings.vibrato_hz = next,
                    Control::VibratoDelay => settings.vibrato_delay_ms = next,
                    Control::Fall => settings.fall = next,
                    Control::FallLength => settings.fall_ms = next,
                    Control::Glide => settings.glide_ms = next,
                    _ => unreachable!(),
                }
                NoteTransform::Pitch { settings }
            }
        })
    }
}

impl Control {
    fn is_pitch(self) -> bool {
        matches!(
            self,
            Self::Scoop
                | Self::ScoopLength
                | Self::Vibrato
                | Self::VibratoRate
                | Self::VibratoDelay
                | Self::Fall
                | Self::FallLength
                | Self::Glide
        )
    }

    fn answers(self, stage: &NoteTransform) -> bool {
        match self {
            Self::Swing => matches!(stage, NoteTransform::Swing { .. }),
            Self::GhostDensity
            | Self::GhostVelocity
            | Self::GhostLength
            | Self::GhostVariation
            | Self::GhostPattern => {
                matches!(
                    stage,
                    NoteTransform::Ghost { .. } | NoteTransform::Brush { .. }
                )
            }
            Self::Mute => matches!(stage, NoteTransform::Mute { .. }),
            Self::Slide => matches!(stage, NoteTransform::Slide { .. }),
            Self::StrumSpread
            | Self::StrumUpVelocity
            | Self::StrumLowAccent
            | Self::StrumUpNotes
            | Self::StrumDirection => {
                matches!(
                    stage,
                    NoteTransform::Strum { .. } | NoteTransform::Stroke { .. }
                )
            }
            Self::Shared => matches!(
                stage,
                NoteTransform::Expression { .. } | NoteTransform::Humanize { .. }
            ),
            _ => matches!(stage, NoteTransform::Pitch { .. }),
        }
    }

    fn value(self, stack: &[NoteTransform]) -> f32 {
        match self {
            Self::Swing => stack
                .iter()
                .find_map(|stage| match stage {
                    NoteTransform::Swing { percent, .. } => Some(f32::from(*percent)),
                    _ => None,
                })
                .unwrap_or(50.0),
            Self::GhostDensity => ghosts(stack, 0, None).density,
            Self::GhostVelocity => ghosts(stack, 0, None).velocity,
            Self::GhostLength => ghosts(stack, 0, None).length_ms,
            Self::GhostVariation => ghosts(stack, 0, None).variation,
            Self::GhostPattern => match ghosts(stack, 0, None).pattern {
                GhostPattern::Pickup => 0.0,
                GhostPattern::Sixteenths => 1.0,
                GhostPattern::Offbeats => 2.0,
                GhostPattern::Repeating => 3.0,
            },
            Self::Mute | Self::Slide => stack
                .iter()
                .find_map(|stage| match (self, stage) {
                    (Self::Mute, NoteTransform::Mute { amount })
                    | (Self::Slide, NoteTransform::Slide { amount }) => Some(*amount),
                    _ => None,
                })
                .unwrap_or(0.0),
            Self::StrumSpread => strum(stack).spread_ms,
            Self::StrumUpVelocity => strum(stack).up_velocity,
            Self::StrumLowAccent => strum(stack).low_accent,
            Self::StrumUpNotes => f32::from(strum(stack).up_notes),
            Self::StrumDirection => match strum(stack).direction {
                StrokeDirection::LowToHigh => 0.0,
                StrokeDirection::HighToLow => 1.0,
                StrokeDirection::Alternate => 2.0,
            },
            Self::Shared => expression(stack).shared,
            Self::Scoop => pitch(stack).scoop,
            Self::ScoopLength => pitch(stack).scoop_ms,
            Self::Vibrato => pitch(stack).vibrato,
            Self::VibratoRate => pitch(stack).vibrato_hz,
            Self::VibratoDelay => pitch(stack).vibrato_delay_ms,
            Self::Fall => pitch(stack).fall,
            Self::FallLength => pitch(stack).fall_ms,
            Self::Glide => pitch(stack).glide_ms,
        }
    }

    /// Step, maximum distance from the input, and legal parameter limits.
    fn bounds(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Swing => (4.0, 12.0, 50.0, 75.0),
            Self::GhostDensity => (0.15, 0.45, 0.0, 1.0),
            Self::GhostVelocity => (0.1, 0.2, 0.0, 1.0),
            Self::GhostLength => (10.0, 30.0, 1.0, 100.0),
            Self::GhostVariation | Self::Shared => (0.2, 0.4, 0.0, 1.0),
            Self::Mute | Self::Slide => (0.15, 0.45, 0.0, 1.0),
            Self::StrumSpread => (8.0, 24.0, 0.0, 100.0),
            Self::StrumUpVelocity | Self::StrumLowAccent => (0.15, 0.3, 0.0, 1.0),
            Self::StrumUpNotes => (1.0, 4.0, 0.0, 4.0),
            Self::Scoop => (0.2, 0.6, 0.0, 4.0),
            Self::ScoopLength => (25.0, 75.0, 10.0, 300.0),
            Self::Vibrato => (0.05, 0.15, 0.0, 1.0),
            Self::VibratoRate => (0.5, 1.5, 2.0, 9.0),
            Self::VibratoDelay => (50.0, 150.0, 0.0, 1000.0),
            Self::Fall => (0.25, 0.75, 0.0, 12.0),
            Self::FallLength => (30.0, 90.0, 10.0, 500.0),
            Self::Glide => (15.0, 45.0, 0.0, 250.0),
            Self::GhostPattern | Self::StrumDirection => unreachable!("categorical controls"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Swing => "swing (%)",
            Self::GhostDensity => "ghost density",
            Self::GhostVelocity => "ghost velocity",
            Self::GhostLength => "ghost length (ms)",
            Self::GhostVariation => "ghost variation",
            Self::GhostPattern => "ghost placement",
            Self::Mute => "mute articulation",
            Self::Slide => "slide articulation",
            Self::StrumSpread => "strum spread (ms)",
            Self::StrumUpVelocity => "upstroke velocity",
            Self::StrumLowAccent => "strum low accent",
            Self::StrumUpNotes => "upstroke pitches",
            Self::StrumDirection => "strum direction",
            Self::Shared => "shared ensemble motion",
            Self::Scoop => "scoop (semitones)",
            Self::ScoopLength => "scoop length (ms)",
            Self::Vibrato => "vibrato (semitones)",
            Self::VibratoRate => "vibrato rate (Hz)",
            Self::VibratoDelay => "vibrato delay (ms)",
            Self::Fall => "fall (semitones)",
            Self::FallLength => "fall length (ms)",
            Self::Glide => "melodic connection (ms)",
        }
    }

    fn describe(self, stack: &[NoteTransform]) -> String {
        match self {
            Self::GhostPattern => ["pickups", "sixteenths", "offbeats", "repeating"]
                [self.value(stack) as usize]
                .to_owned(),
            Self::StrumDirection => {
                ["low to high", "high to low", "alternating"][self.value(stack) as usize].to_owned()
            }
            _ => format!("{:.2}", self.value(stack)),
        }
    }
}

fn scope<'a>(stack: &'a [NoteTransform], voice: Option<&str>) -> &'a [NoteTransform] {
    match voice {
        None => stack,
        Some(name) => stack
            .iter()
            .find_map(|stage| match stage {
                NoteTransform::ForDrumVoice { voice, transforms } if voice == name => {
                    Some(transforms.as_slice())
                }
                _ => None,
            })
            .unwrap_or(&[]),
    }
}

fn scope_mut<'a>(
    stack: &'a mut Vec<NoteTransform>,
    voice: Option<&str>,
) -> &'a mut Vec<NoteTransform> {
    let Some(name) = voice else {
        return stack;
    };
    let at = if let Some(at) = stack.iter().position(
        |stage| matches!(stage, NoteTransform::ForDrumVoice { voice, .. } if voice == name),
    ) {
        at
    } else {
        insert(
            stack,
            NoteTransform::ForDrumVoice {
                voice: name.to_owned(),
                transforms: vec![],
            },
        )
    };
    let NoteTransform::ForDrumVoice { transforms, .. } = &mut stack[at] else {
        unreachable!();
    };
    transforms
}

fn insert(stack: &mut Vec<NoteTransform>, stage: NoteTransform) -> usize {
    let at = stack
        .iter()
        .position(|held| rank(held) > rank(&stage))
        .unwrap_or(stack.len());
    stack.insert(at, stage);
    at
}

// Match the inspector's order for new stages, without reordering a custom stack.
fn rank(stage: &NoteTransform) -> usize {
    match stage {
        NoteTransform::Swing { .. } | NoteTransform::Groove { .. } => 0,
        NoteTransform::Transpose { .. } => 1,
        NoteTransform::Gate { .. } => 2,
        NoteTransform::Ghost { .. } | NoteTransform::Brush { .. } => 3,
        NoteTransform::Slide { .. } => 4,
        NoteTransform::Mute { .. } => 5,
        NoteTransform::Strum { .. } | NoteTransform::Stroke { .. } => 6,
        NoteTransform::Lean { .. } | NoteTransform::ForDrumVoice { .. } => 7,
        NoteTransform::Expression { .. } | NoteTransform::Humanize { .. } => 8,
        NoteTransform::Pitch { .. } => 9,
        NoteTransform::Octaves { .. } => 10,
    }
}

fn ghosts(stack: &[NoteTransform], seed: u64, target_pitch: Option<u8>) -> GhostNotes {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Ghost { settings } => Some(settings.clone()),
            NoteTransform::Brush { amount } => Some(GhostNotes {
                density: 1.0,
                velocity: 0.3 * amount,
                pattern: GhostPattern::Sixteenths,
                preserve_rests: false,
                seed,
                ..GhostNotes::default()
            }),
            _ => None,
        })
        .unwrap_or(GhostNotes {
            density: 0.0,
            velocity: 0.2,
            seed,
            target_pitch,
            ..GhostNotes::default()
        })
}

fn strum(stack: &[NoteTransform]) -> Strum {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Strum { settings } => Some(settings.clone()),
            NoteTransform::Stroke {
                spread_ms,
                direction,
            } => Some(Strum {
                spread_ms: *spread_ms,
                direction: *direction,
                clock: StrumClock::Attacks,
                ..Strum::default()
            }),
            _ => None,
        })
        .unwrap_or(Strum {
            spread_ms: 0.0,
            ..Strum::default()
        })
}

fn expression(stack: &[NoteTransform]) -> Expression {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Expression { settings } => Some(settings.clone()),
            NoteTransform::Humanize { amount, seed } => Some(Expression {
                timing: *amount,
                velocity: *amount,
                seed: *seed,
                ..Expression::default()
            }),
            _ => None,
        })
        .unwrap_or_default()
}

fn pitch(stack: &[NoteTransform]) -> PitchPerformance {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Pitch { settings } => Some(settings.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn active(stage: &NoteTransform) -> bool {
    match stage {
        NoteTransform::Swing { percent, .. } => *percent > 50,
        NoteTransform::Ghost { settings } => settings.density > 0.0 && settings.velocity > 0.0,
        NoteTransform::Slide { amount } | NoteTransform::Mute { amount } => *amount > 0.0,
        NoteTransform::Strum { settings } => {
            settings.spread_ms > 0.0
                || settings.up_notes > 0
                || settings.up_velocity < 1.0
                || settings.low_accent > 0.0
        }
        NoteTransform::Pitch { settings } => settings.is_active(),
        _ => true,
    }
}

fn category(original: f32, occurrence: usize, count: usize) -> f32 {
    // Visit every alternative even when every previous proposal lost and the incumbent
    // still has its original setting. Walking from the incumbent would repeat its neighbors.
    ((original as usize + 1 + occurrence % (count - 1)) % count) as f32
}

fn dominant_pitch(notes: &[Note]) -> u8 {
    let mut counts = [0usize; 128];
    for note in notes {
        counts[usize::from(note.pitch.min(127))] += 1;
    }
    (0..128).max_by_key(|&pitch| counts[pitch]).unwrap_or(0) as u8
}

fn has_chords(notes: &[Note]) -> bool {
    let mut starts = BTreeSet::new();
    notes.iter().any(|note| !starts.insert(note.start))
}

fn monophonic(notes: &[Note]) -> bool {
    if notes.is_empty() {
        return false;
    }
    let mut notes: Vec<_> = notes.iter().collect();
    notes.sort_by_key(|note| note.start);
    notes.windows(2).all(|pair| pair[0].end() <= pair[1].start)
}

fn pitch_eligible(project: &Project, id: ClipId) -> bool {
    let Some((track_id, clip)) = project.midi_clip(id) else {
        return false;
    };
    let Some(track) = project.track(track_id) else {
        return false;
    };
    if track.kind.is_drum() || !monophonic(&clip.playable_notes().collect::<Vec<_>>()) {
        return false;
    }
    // Pitch bend is channel-wide. Even monophonic clips cannot safely bend independently
    // when another clip on that track may sound at the same time, including loop repeats.
    track.kind.as_instrument().is_some_and(|instrument| {
        instrument.clips.iter().all(|other| {
            other.id == id
                || other.muted
                || other.playable_notes().next().is_none()
                || other.sounding_end() <= clip.start
                || other.start >= clip.sounding_end()
        })
    })
}

pub(super) fn describe_changes(original: &Project, best: &Project) -> Vec<String> {
    let mut changes = Vec::new();
    // Report every supported field, including activation companions and categorical changes.
    let controls = [
        Control::Swing,
        Control::GhostDensity,
        Control::GhostVelocity,
        Control::GhostLength,
        Control::GhostVariation,
        Control::GhostPattern,
        Control::Mute,
        Control::Slide,
        Control::StrumSpread,
        Control::StrumUpVelocity,
        Control::StrumLowAccent,
        Control::StrumUpNotes,
        Control::StrumDirection,
        Control::Shared,
        Control::Scoop,
        Control::ScoopLength,
        Control::Vibrato,
        Control::VibratoRate,
        Control::VibratoDelay,
        Control::Fall,
        Control::FallLength,
        Control::Glide,
    ];
    for track in &original.tracks {
        let Some(instrument) = track.kind.as_instrument() else {
            continue;
        };
        for before in &instrument.clips {
            let Some((_, after)) = best.midi_clip(before.id) else {
                continue;
            };
            let mut voices = BTreeSet::from([None]);
            for stage in before.transforms.iter().chain(&after.transforms) {
                if let NoteTransform::ForDrumVoice { voice, .. } = stage {
                    voices.insert(Some(voice.as_str()));
                }
            }
            for voice in voices {
                let a = scope(&before.transforms, voice);
                let b = scope(&after.transforms, voice);
                for control in controls {
                    if control.value(a) != control.value(b) {
                        let voice = voice.map_or(String::new(), |name| format!(" / {name}"));
                        changes.push(format!(
                            "{} / {}{voice}: {} {} → {}",
                            track.name,
                            before.name,
                            control.label(),
                            control.describe(a),
                            control.describe(b)
                        ));
                    }
                }
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::Ticks;

    fn fixture(drums: bool) -> (Project, ClipId) {
        let mut project = Project::default();
        let track = if drums {
            project.add_drum_track("Kit", "drums")
        } else {
            project.add_instrument_track("Lead", "synth")
        };
        let clip = project
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 8)
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = (0..8)
            .map(|i| {
                Note::new(
                    60 + i as u8,
                    Ticks::QUARTER * i,
                    Ticks(Ticks::QUARTER.0 / 2),
                )
            })
            .collect();
        (project, clip)
    }

    fn dial(project: &Project, id: ClipId, control: Control) -> Dial {
        dials(project)
            .into_iter()
            .find(|dial| dial.clip == id && dial.control == control)
            .unwrap()
    }

    #[test]
    fn arrangement_dials_preserve_all_source_material_and_stay_bounded() {
        let (original, id) = fixture(false);
        for dial in dials(&original) {
            let mut candidate = original.clone();
            for _ in 0..30 {
                dial.adjust(&mut candidate, &original, 0);
            }
            let before = &original.midi_clip(id).unwrap().1;
            let after = &candidate.midi_clip(id).unwrap().1;
            assert_eq!(before.notes, after.notes);
            assert_eq!(before.recipe, after.recipe);
            assert_eq!(before.bend, after.bend);
            assert_eq!(before.controllers, after.controllers);
            assert_eq!(before.start, after.start);
            assert_eq!(before.length, after.length);
            if !matches!(
                dial.control,
                Control::GhostPattern | Control::StrumDirection
            ) {
                let value = dial.control.value(&after.transforms);
                let base = dial.control.value(&before.transforms);
                let (_, radius, low, high) = dial.control.bounds();
                assert!(value.is_finite() && (low..=high).contains(&value));
                assert!((value - base).abs() <= radius + 0.00001);
            }
        }
    }

    #[test]
    fn swing_is_inserted_before_wander_and_only_changes_the_selected_clip() {
        let (mut original, id) = fixture(false);
        original.midi_clip_mut(id).unwrap().notes[1].start = Ticks(Ticks::QUARTER.0 / 4);
        original.midi_clip_mut(id).unwrap().transforms = vec![
            NoteTransform::Gate { amount: 0.7 },
            NoteTransform::Humanize {
                amount: 0.1,
                seed: 12,
            },
        ];
        let sibling = original.duplicate_clip(id).unwrap();
        let mut candidate = original.clone();
        dial(&original, id, Control::Swing).adjust(&mut candidate, &original, 0);
        let clip = candidate.midi_clip(id).unwrap().1;
        assert!(matches!(
            clip.transforms[0],
            NoteTransform::Swing { percent: 54, .. }
        ));
        assert_eq!(
            clip.transforms[1..],
            original.midi_clip(id).unwrap().1.transforms
        );
        assert_eq!(candidate.midi_clip(sibling), original.midi_clip(sibling));
        assert_ne!(
            clip.sounding_notes(120.0).collect::<Vec<_>>(),
            original
                .midi_clip(id)
                .unwrap()
                .1
                .sounding_notes(120.0)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn new_ghosts_are_audible_seeded_and_never_rewrite_the_score() {
        let (original, id) = fixture(false);
        let mut a = original.clone();
        let mut b = original.clone();
        let dial = dial(&original, id, Control::GhostDensity);
        for _ in 0..2 {
            dial.adjust(&mut a, &original, 0);
            dial.adjust(&mut b, &original, 0);
        }
        assert_eq!(a, b);
        let clip = a.midi_clip(id).unwrap().1;
        assert_eq!(clip.notes, original.midi_clip(id).unwrap().1.notes);
        assert!(clip.sounding_notes(120.0).count() > clip.notes.len());
        assert!(
            describe_changes(&original, &a)
                .iter()
                .any(|line| line.contains("ghost density"))
        );
    }

    #[test]
    fn drum_voice_ghost_adjustment_leaves_the_other_writer_exact() {
        let (mut original, id) = fixture(true);
        let clip = original.midi_clip_mut(id).unwrap();
        for note in &mut clip.notes {
            note.pitch = 38;
            note.drum_voice = "snare".to_owned();
        }
        let mut kick = clip.notes.clone();
        for note in &mut kick {
            note.pitch = 36;
            note.drum_voice = "kick".to_owned();
        }
        clip.notes.extend(kick);
        let mut candidate = original.clone();
        let selected = dials(&original)
            .into_iter()
            .find(|dial| {
                dial.clip == id
                    && dial.control == Control::GhostDensity
                    && dial.voice.as_deref() == Some("snare")
            })
            .unwrap();
        for _ in 0..2 {
            selected.adjust(&mut candidate, &original, 0);
        }
        let after = candidate.midi_clip(id).unwrap().1;
        assert!(
            matches!(&after.transforms[0], NoteTransform::ForDrumVoice { voice, .. } if voice == "snare")
        );
        let before_kick: Vec<_> = original
            .midi_clip(id)
            .unwrap()
            .1
            .sounding_notes(120.0)
            .filter(|note| note.drum_voice == "kick")
            .collect();
        let after_kick: Vec<_> = after
            .sounding_notes(120.0)
            .filter(|note| note.drum_voice == "kick")
            .collect();
        assert_eq!(before_kick, after_kick);
        assert!(
            after
                .sounding_notes(120.0)
                .filter(|note| note.drum_voice == "snare")
                .count()
                > 8
        );
        assert!(dials(&original).iter().all(|dial| !dial.control.is_pitch()
            && !matches!(dial.control, Control::Slide | Control::StrumSpread)));
    }

    #[test]
    fn pitch_gestures_exclude_chords_and_overlapping_clips_even_after_regeneration() {
        let (original, id) = fixture(false);
        let pitch_dial = dial(&original, id, Control::Scoop);
        let mut candidate = original.clone();
        candidate
            .midi_clip_mut(id)
            .unwrap()
            .notes
            .push(Note::new(72, Ticks::ZERO, Ticks::QUARTER));
        assert!(!dials(&candidate).iter().any(|dial| dial.control.is_pitch()));
        pitch_dial.adjust(&mut candidate, &original, 0);
        assert!(!candidate.midi_clip(id).unwrap().1.has_pitch_performance());
        assert!(
            dials(&candidate)
                .iter()
                .any(|dial| dial.control == Control::StrumSpread)
        );
        let mut overlap = original.clone();
        let other = overlap.duplicate_clip(id).unwrap();
        overlap.midi_clip_mut(other).unwrap().start = Ticks::QUARTER;
        assert!(!dials(&overlap).iter().any(|dial| dial.control.is_pitch()));
    }

    #[test]
    fn existing_brush_and_stroke_are_updated_in_place_with_legacy_semantics() {
        let (mut original, id) = fixture(false);
        let clip = original.midi_clip_mut(id).unwrap();
        let mut upper = clip.notes.clone();
        for note in &mut upper {
            note.pitch += 12;
        }
        clip.notes.extend(upper);
        clip.transforms = vec![
            NoteTransform::Humanize {
                amount: 0.2,
                seed: 43,
            },
            NoteTransform::Brush { amount: 0.5 },
            NoteTransform::Stroke {
                spread_ms: 30.0,
                direction: StrokeDirection::LowToHigh,
            },
        ];
        let mut candidate = original.clone();
        dial(&original, id, Control::GhostDensity).adjust(&mut candidate, &original, 1);
        dial(&original, id, Control::StrumSpread).adjust(&mut candidate, &original, 0);
        let stack = &candidate.midi_clip(id).unwrap().1.transforms;
        assert_eq!(stack[0], original.midi_clip(id).unwrap().1.transforms[0]);
        let NoteTransform::Ghost { settings } = &stack[1] else {
            panic!("brush")
        };
        assert_eq!(settings.pattern, GhostPattern::Sixteenths);
        assert!(!settings.preserve_rests);
        assert_eq!(settings.velocity, 0.15);
        let NoteTransform::Strum { settings } = &stack[2] else {
            panic!("stroke")
        };
        assert_eq!(settings.clock, StrumClock::Attacks);
        assert_eq!(settings.direction, StrokeDirection::LowToHigh);
        assert_eq!(settings.spread_ms, 38.0);
    }

    #[test]
    fn categorical_arrangement_exhausts_alternatives_when_every_proposal_loses() {
        let (mut original, id) = fixture(false);
        let clip = original.midi_clip_mut(id).unwrap();
        clip.notes.push(Note::new(
            72,
            auris_core::Ticks::ZERO,
            auris_core::Ticks::QUARTER,
        ));
        for (control, count) in [(Control::GhostPattern, 4), (Control::StrumDirection, 3)] {
            let dial = dial(&original, id, control);
            let baseline = control.value(&original.midi_clip(id).unwrap().1.transforms) as u8;
            let mut visited = BTreeSet::from([baseline]);
            for occurrence in 0..count - 1 {
                // Deliberately discard each candidate, as a flat/lower audio score does.
                let mut candidate = original.clone();
                dial.adjust(&mut candidate, &original, occurrence);
                let value = control.value(&candidate.midi_clip(id).unwrap().1.transforms) as u8;
                assert!(visited.insert(value), "{control:?}: repeated {value}");
            }
            assert_eq!(visited.len(), count);
        }
    }
}
