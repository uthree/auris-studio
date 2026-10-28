//! Identity-free timeline edits for the native document context.

use auris_core::pipeline::document::NativeDocument;
use auris_core::time::{Ticks, TimeSignature};
use serde::{Deserialize, Serialize};

use super::NativeEditError;

/// A bounded edit to the persisted timeline and transport settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NativeTimelineEdit {
    /// Renames the document.
    SetName {
        /// New document name.
        name: String,
    },
    /// Replaces a tempo point, clamping its position and using the core tempo bounds.
    SetTempo {
        /// Timeline position.
        at: Ticks,
        /// Beats per minute.
        bpm: f64,
    },
    /// Turns the tempo of the segment containing a position.
    SetTempoAt {
        /// Timeline position.
        at: Ticks,
        /// Beats per minute.
        bpm: f64,
    },
    /// Removes the tempo point in force at a position.
    RemoveTempo {
        /// Timeline position.
        at: Ticks,
    },
    /// Replaces a time-signature point.
    SetSignature {
        /// Timeline position.
        at: Ticks,
        /// New time signature.
        signature: TimeSignature,
    },
    /// Turns the time signature of the segment containing a position.
    SetSignatureAt {
        /// Timeline position.
        at: Ticks,
        /// New time signature.
        signature: TimeSignature,
    },
    /// Removes a time-signature point, except the timeline anchor.
    RemoveSignature {
        /// Timeline position.
        at: Ticks,
    },
    /// Sets the editor grid. Values below one tick become one tick, matching Session.
    SetGrid {
        /// New grid in ticks.
        grid: Ticks,
    },
    /// Stores a loop region; non-positive spans disable the region.
    SetLoopRegion {
        /// Start position.
        start: Ticks,
        /// End position.
        end: Ticks,
    },
    /// Stores the loop switch. Enabling an absent region seeds the first two bars.
    SetLoopEnabled {
        /// Whether looping is enabled.
        enabled: bool,
    },
    /// Stores a punch region.
    SetPunchRegion {
        /// Start position.
        start: Ticks,
        /// End position.
        end: Ticks,
    },
    /// Stores the punch switch. Enabling an absent region seeds the loop or first two bars.
    SetPunchEnabled {
        /// Whether punch recording is enabled.
        enabled: bool,
    },
    /// Stores whether the click is enabled.
    SetMetronome {
        /// Whether the click is enabled.
        enabled: bool,
    },
    /// Stores the count-in length, bounded by the Session maximum.
    SetCountIn {
        /// Number of bars.
        bars: u32,
    },
}

/// Applies a timeline edit without touching runtime transport or history.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeTimelineEdit,
) -> Result<(), NativeEditError> {
    match edit {
        NativeTimelineEdit::SetName { name } => document.context.name.clone_from(name),
        NativeTimelineEdit::SetTempo { at, bpm } => {
            if !bpm.is_finite() {
                return Err(NativeEditError::Invalid("tempo must be finite".into()));
            }
            let at = snap_harmony(document, *at);
            document.context.tempo_map.set_point(at, *bpm);
        }
        NativeTimelineEdit::SetTempoAt { at, bpm } => {
            if !bpm.is_finite() {
                return Err(NativeEditError::Invalid("tempo must be finite".into()));
            }
            let at = document.context.tempo_map.change_at(safe_position(*at));
            document.context.tempo_map.set_point(at, *bpm);
        }
        NativeTimelineEdit::RemoveTempo { at } => {
            let at = document.context.tempo_map.change_at(safe_position(*at));
            document.context.tempo_map.remove_point(at);
        }
        NativeTimelineEdit::SetSignature { at, signature } => {
            if TimeSignature::new(signature.numerator, signature.denominator) != *signature {
                return Err(NativeEditError::Invalid(
                    "time signature is outside the supported bounds".into(),
                ));
            }
            document
                .context
                .signatures
                .set_point(safe_position(*at), *signature);
        }
        NativeTimelineEdit::SetSignatureAt { at, signature } => {
            if TimeSignature::new(signature.numerator, signature.denominator) != *signature {
                return Err(NativeEditError::Invalid(
                    "time signature is outside the supported bounds".into(),
                ));
            }
            let at = document.context.signatures.change_at(safe_position(*at));
            document.context.signatures.set_point(at, *signature);
        }
        NativeTimelineEdit::RemoveSignature { at } => {
            let at = document.context.signatures.change_at(safe_position(*at));
            document.context.signatures.remove_point(at);
        }
        NativeTimelineEdit::SetGrid { grid } => {
            document.context.grid = (*grid).max(Ticks(1));
        }
        NativeTimelineEdit::SetLoopRegion { start, end } => {
            let (start, end) = if end < start {
                (*end, *start)
            } else {
                (*start, *end)
            };
            document.context.loop_region =
                (end > start).then_some((start.max_zero(), end.max_zero()));
        }
        NativeTimelineEdit::SetLoopEnabled { enabled } => {
            document.context.loop_enabled = *enabled;
            if *enabled && document.context.loop_region.is_none() {
                document.context.loop_region =
                    Some((Ticks::ZERO, document.context.signatures.bar_start(3)));
            }
        }
        NativeTimelineEdit::SetPunchRegion { start, end } => {
            let (start, end) = if end < start {
                (*end, *start)
            } else {
                (*start, *end)
            };
            document.context.punch_region = Some((start.max_zero(), end.max_zero()));
        }
        NativeTimelineEdit::SetPunchEnabled { enabled } => {
            document.context.punch_enabled = *enabled;
            if *enabled && document.context.punch_region.is_none() {
                document.context.punch_region = Some(
                    document
                        .context
                        .loop_region
                        .unwrap_or((Ticks::ZERO, document.context.signatures.bar_start(3))),
                );
            }
        }
        NativeTimelineEdit::SetMetronome { enabled } => document.context.metronome = *enabled,
        NativeTimelineEdit::SetCountIn { bars } => document.context.count_in_bars = (*bars).min(4),
    }
    Ok(())
}

// SignatureMap's bar arithmetic predates hostile external payloads and contains a plain
// multiplication. Keep positions below the final few ticks so snapping cannot overflow.
const SAFE_MAX_TICK: i64 = i64::MAX - 1_000_000;

fn safe_position(at: Ticks) -> Ticks {
    Ticks(at.raw().clamp(0, SAFE_MAX_TICK))
}

fn snap_harmony(document: &NativeDocument, at: Ticks) -> Ticks {
    let at = safe_position(at);
    let origin = document.context.signatures.change_at(at);
    let grid = document
        .context
        .signatures
        .signature_at(at)
        .ticks_per_beat()
        .max(document.context.grid)
        .raw()
        .max(1);
    let relative = i128::from((at - origin).raw());
    let grid = i128::from(grid);
    let snapped = ((relative + grid / 2) / grid) * grid + i128::from(origin.raw());
    Ticks(snapped.min(i128::from(SAFE_MAX_TICK)) as i64)
}

/// Synchronizes an unrecorded timeline command into an undo snapshot.
pub(super) fn synchronize_unrecorded(
    edit: &NativeTimelineEdit,
    source: &NativeDocument,
    historical: &mut NativeDocument,
) {
    match edit {
        NativeTimelineEdit::SetName { .. } => {
            historical.context.name.clone_from(&source.context.name)
        }
        NativeTimelineEdit::SetGrid { .. } => historical.context.grid = source.context.grid,
        NativeTimelineEdit::SetLoopEnabled { .. } => {
            historical.context.loop_enabled = source.context.loop_enabled;
            if historical.context.loop_region.is_none() && source.context.loop_enabled {
                historical.context.loop_region = source.context.loop_region;
            }
        }
        NativeTimelineEdit::SetPunchEnabled { .. } => {
            historical.context.punch_enabled = source.context.punch_enabled;
            if historical.context.punch_region.is_none() && source.context.punch_enabled {
                historical.context.punch_region = source.context.punch_region;
            }
        }
        NativeTimelineEdit::SetMetronome { .. } => {
            historical.context.metronome = source.context.metronome
        }
        NativeTimelineEdit::SetCountIn { .. } => {
            historical.context.count_in_bars = source.context.count_in_bars
        }
        _ => {}
    }
}
