//! The dials on any clip's performance: the transform stack, read and written as three sliders.
//!
//! The section sits under the part dials in the inspector and is offered for *every* MIDI clip,
//! because the stack is not the composer's: a phrase played by hand takes a humanise or a swing
//! exactly as a written one does. Everything here that decides something is a free function over
//! a `&[NoteTransform]`, for the reason given in [`crate::ui::context_menu`]; the `impl AurisApp`
//! block draws the rows and hangs the gestures off them.
//!
//! The panel is deliberately narrower than the model. A stack can hold any transforms in any
//! order — the session and a file both honour that — but the panel offers one of each, kept in
//! the order that reads musically: swing before humanise, so the swing still finds its offbeats
//! on the grid before the wander moves them off it. A transpose set some other way survives
//! everything this panel does, and so does the lean the composer installs on a generated part;
//! neither has a dial here yet.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{AnyElement, IntoElement, MouseDownEvent, Pixels, Point, div, prelude::*};

use crate::app::{AurisApp, Drag};
use crate::ui::context_menu::{ContextMenu, MenuCommand, subdivision_key};
use crate::ui::part::{GATE_MIN, SWING_MAX, SWING_MIN};
use crate::ui::widgets::{ButtonStyle, SliderFill, button, divider, value_slider};

/// One slider of the performance section.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PerformDial {
    /// How far timing and velocity wander. Zero is the text as written, and is stored as no
    /// transform at all.
    Humanize,
    /// Where the offbeat lands. Fifty is straight, and is stored as no transform at all.
    Swing,
    /// How much of each note's length is held. Whole is as written, and is stored as no
    /// transform at all.
    Gate,
}

/// The sliders in the order the panel shows them — which is also the order the stack plays them.
pub const PERFORM_DIALS: &[PerformDial] =
    &[PerformDial::Swing, PerformDial::Humanize, PerformDial::Gate];

/// The grid a swing added by the panel delays, until its picker says otherwise.
///
/// Sixteenths for the reason the recipe's subdivision defaults there: it is what a groove is
/// written in and what most music is felt in.
pub const DEFAULT_SWING_GRID: Subdivision = Subdivision::Sixteenth;

impl PerformDial {
    /// The row's label. The part dials' own words, on purpose: one word, one meaning, whether
    /// the phrase was written or played.
    pub fn label(self) -> Key {
        match self {
            PerformDial::Humanize => Key::PartHumanize,
            PerformDial::Swing => Key::PartSwing,
            PerformDial::Gate => Key::PartGate,
        }
    }
}

/// Where each transform stands in the panel's canonical order.
///
/// Swing before humanise is the audible one — the swing must find its offbeats before the
/// wander moves them — and the rest simply keeps insertion deterministic. A stack somebody
/// arranged another way is honoured as it stands; the rank only places a transform that is
/// *joining* the stack.
fn rank(transform: &NoteTransform) -> usize {
    match transform {
        NoteTransform::Swing { .. } => 0,
        // The lean sits between: deterministic feel before random feel, and after the swing for
        // the same reason the humanise is — a leaned note is off the grid the swing reads.
        NoteTransform::Lean { .. } => 1,
        NoteTransform::Humanize { .. } => 2,
        NoteTransform::Transpose { .. } => 3,
        NoteTransform::Gate { .. } => 4,
    }
}

/// `true` when `transform` is the one `dial` reads and writes.
fn answers_to(transform: &NoteTransform, dial: PerformDial) -> bool {
    matches!(
        (transform, dial),
        (NoteTransform::Humanize { .. }, PerformDial::Humanize)
            | (NoteTransform::Swing { .. }, PerformDial::Swing)
            | (NoteTransform::Gate { .. }, PerformDial::Gate)
    )
}

/// Where a dial's bar stands, from 0 to 1, read off the stack.
///
/// A transform the stack does not hold reads as the dial's neutral end, which is what writing
/// that position back stores — the two directions have to agree or a slider would move on the
/// first paint.
pub fn dial_fraction(stack: &[NoteTransform], dial: PerformDial) -> f32 {
    match dial {
        PerformDial::Humanize => stack
            .iter()
            .find_map(|transform| match transform {
                NoteTransform::Humanize { amount, .. } => Some(amount.clamp(0.0, 1.0)),
                _ => None,
            })
            .unwrap_or(0.0),
        PerformDial::Swing => {
            let percent = swing_percent(stack);
            f32::from(percent - SWING_MIN) / f32::from(SWING_MAX - SWING_MIN)
        }
        PerformDial::Gate => stack
            .iter()
            .find_map(|transform| match transform {
                NoteTransform::Gate { amount } => Some(amount.clamp(GATE_MIN, 1.0)),
                _ => None,
            })
            .unwrap_or(1.0),
    }
}

/// The swing the stack plays, as a whole percent; straight when it holds none.
pub fn swing_percent(stack: &[NoteTransform]) -> u8 {
    stack
        .iter()
        .find_map(|transform| match transform {
            NoteTransform::Swing { percent, .. } => Some((*percent).clamp(SWING_MIN, SWING_MAX)),
            _ => None,
        })
        .unwrap_or(SWING_MIN)
}

/// The grid the stack's swing delays; the default when it holds none.
pub fn swing_grid(stack: &[NoteTransform]) -> Subdivision {
    stack
        .iter()
        .find_map(|transform| match transform {
            NoteTransform::Swing { subdivision, .. } => Some(*subdivision),
            _ => None,
        })
        .unwrap_or(DEFAULT_SWING_GRID)
}

/// The stack with `dial` moved to `fraction`, everything else exactly as it was.
///
/// A dial at its neutral end stores no transform at all rather than one that does nothing: the
/// file stays clean, and "this clip is performed" remains one empty-check rather than a walk.
/// Values are kept to whole percents so a drag settles instead of trembling, an existing
/// transform keeps its place in the stack — and its seed, and its grid — and a new one joins at
/// its [`rank`]. `seed` names the wander a *new* humanise draws from; the caller passes the
/// clip's id, so two clips carrying the same phrase still wobble apart.
pub fn with_dial(
    stack: &[NoteTransform],
    dial: PerformDial,
    fraction: f32,
    seed: u64,
) -> Vec<NoteTransform> {
    let fraction = fraction.clamp(0.0, 1.0);
    let replacement = match dial {
        PerformDial::Humanize => {
            let amount = (fraction * 100.0).round() / 100.0;
            (amount > 0.0).then(|| NoteTransform::Humanize {
                amount,
                seed: stack
                    .iter()
                    .find_map(|transform| match transform {
                        NoteTransform::Humanize { seed, .. } => Some(*seed),
                        _ => None,
                    })
                    .unwrap_or(seed),
            })
        }
        PerformDial::Swing => {
            let range = f32::from(SWING_MAX - SWING_MIN);
            let percent = SWING_MIN + (fraction * range).round() as u8;
            (percent > SWING_MIN).then(|| NoteTransform::Swing {
                percent,
                subdivision: swing_grid(stack),
            })
        }
        PerformDial::Gate => {
            let amount = ((fraction.max(GATE_MIN)) * 100.0).round() / 100.0;
            (amount < 1.0).then_some(NoteTransform::Gate { amount })
        }
    };

    let mut out = stack.to_vec();
    let held = out.iter().position(|transform| answers_to(transform, dial));
    match (held, replacement) {
        (Some(at), Some(transform)) => out[at] = transform,
        (Some(at), None) => {
            out.remove(at);
        }
        (None, Some(transform)) => {
            let at = out
                .iter()
                .position(|other| rank(other) > rank(&transform))
                .unwrap_or(out.len());
            out.insert(at, transform);
        }
        (None, None) => {}
    }
    out
}

/// The stack with its swing delaying `subdivision` instead.
///
/// Only an existing swing has a grid to change, so a stack without one comes back untouched —
/// the picker is only drawn beside a swing that is on.
pub fn with_swing_grid(stack: &[NoteTransform], subdivision: Subdivision) -> Vec<NoteTransform> {
    let mut out = stack.to_vec();
    for transform in &mut out {
        if let NoteTransform::Swing {
            subdivision: grid, ..
        } = transform
        {
            *grid = subdivision;
        }
    }
    out
}

/// What a dial's value button reads, with the swing's straight end in words.
pub fn dial_text(stack: &[NoteTransform], dial: PerformDial, straight: &str) -> String {
    match dial {
        PerformDial::Humanize => format!("{:.0}%", dial_fraction(stack, dial) * 100.0),
        PerformDial::Swing => match swing_percent(stack) {
            SWING_MIN => straight.to_string(),
            percent => format!("{percent}%"),
        },
        PerformDial::Gate => format!("{:.0}%", dial_fraction(stack, dial) * 100.0),
    }
}

/// A stable element key per dial, so gpui tells the sliders apart.
fn dial_element_key(dial: PerformDial) -> usize {
    match dial {
        PerformDial::Swing => 0,
        PerformDial::Humanize => 1,
        PerformDial::Gate => 2,
    }
}

impl AurisApp {
    /// The selected clip's performance section, or nothing when no MIDI clip is selected.
    ///
    /// Returns rows rather than a panel, the way [`Self::part_rows`] does and for the same
    /// reason.
    pub(crate) fn perform_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let Some(clip) = self.selected_clip else {
            return Vec::new();
        };
        let Ok(stack) = self.session.clip_transforms(clip) else {
            return Vec::new();
        };
        let stack = stack.to_vec();
        let theme = self.theme.clone();
        let straight = self.t(Key::PartStraight);

        let mut rows: Vec<AnyElement> =
            vec![self.group_heading(Key::PerformHeading).into_any_element()];

        for dial in PERFORM_DIALS {
            let dial = *dial;
            let fraction = dial_fraction(&stack, dial);
            rows.push(
                value_slider(
                    ("perform-dial", dial_element_key(dial)),
                    self.t(dial.label()),
                    dial_text(&stack, dial, straight),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::PerformDial {
                            clip,
                            dial,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                )
                .into_any_element(),
            );
        }

        if swing_percent(&stack) > SWING_MIN {
            rows.push(
                self.picker_row(
                    "perform-swing-grid",
                    Key::PartSubdivision,
                    self.t(subdivision_key(swing_grid(&stack))).to_string(),
                    Self::opens_menu(cx, move |this, at| this.perform_swing_grid_menu(at, clip)),
                )
                .into_any_element(),
            );
        }

        if !stack.is_empty() {
            rows.push(
                div()
                    .flex()
                    .child(div().flex_1().child(button(
                        "perform-freeze",
                        self.t(Key::PerformFreeze),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            let _ = this.session.freeze_clip_transforms(clip);
                            cx.notify();
                        }),
                    )))
                    .into_any_element(),
            );
        }

        rows.push(divider(&theme).into_any_element());
        rows
    }

    /// The two grids a swing can delay.
    ///
    /// Straight ones only: the transform leaves a triplet grid untouched, and a menu must not
    /// offer a position that does nothing.
    pub(crate) fn perform_swing_grid_menu(
        &self,
        anchor: Point<Pixels>,
        clip: ClipId,
    ) -> ContextMenu {
        let current = self
            .session
            .clip_transforms(clip)
            .map(swing_grid)
            .unwrap_or(DEFAULT_SWING_GRID);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartSubdivision));
        for subdivision in [Subdivision::Eighth, Subdivision::Sixteenth] {
            menu = menu.toggle(
                self.t(subdivision_key(subdivision)),
                MenuCommand::SetPerformSwingGrid { clip, subdivision },
                current == subdivision,
            );
        }
        menu
    }

    /// Moves one performance dial.
    ///
    /// A move too small to change the stored stack writes nothing at all, for the reason
    /// [`Self::set_dial`] gives — and the session's own no-change check backs it up.
    pub(crate) fn set_perform_dial(&mut self, clip: ClipId, dial: PerformDial, fraction: f32) {
        let Ok(stack) = self.session.clip_transforms(clip) else {
            return;
        };
        let next = with_dial(stack, dial, fraction, clip.0);
        let _ = self.session.set_clip_transforms(clip, next);
    }

    /// Applies a performance dial drag, measured in pixels from where it began.
    pub(crate) fn drag_perform_dial(
        &mut self,
        clip: ClipId,
        dial: PerformDial,
        start_fraction: f32,
        delta: f32,
    ) {
        self.set_perform_dial(
            clip,
            dial,
            crate::ui::widgets::dragged(start_fraction, delta),
        );
    }

    /// Points the selected clip's swing at a different grid.
    pub(crate) fn set_perform_swing_grid(&mut self, clip: ClipId, subdivision: Subdivision) {
        let Ok(stack) = self.session.clip_transforms(clip) else {
            return;
        };
        let next = with_swing_grid(stack, subdivision);
        let _ = self.session.set_clip_transforms(clip, next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dial_reads_back_what_it_was_set_to() {
        let stack = with_dial(&[], PerformDial::Humanize, 0.35, 7);
        assert_eq!(dial_fraction(&stack, PerformDial::Humanize), 0.35);
        let stack = with_dial(&stack, PerformDial::Swing, 0.68, 7);
        assert_eq!(swing_percent(&stack), 67);
        let stack = with_dial(&stack, PerformDial::Gate, 0.5, 7);
        assert_eq!(dial_fraction(&stack, PerformDial::Gate), 0.5);
    }

    #[test]
    fn a_neutral_dial_stores_no_transform_at_all() {
        let stack = with_dial(&[], PerformDial::Humanize, 0.4, 7);
        assert_eq!(stack.len(), 1);
        assert!(with_dial(&stack, PerformDial::Humanize, 0.0, 7).is_empty());
        assert!(with_dial(&[], PerformDial::Swing, 0.0, 7).is_empty());
        assert!(with_dial(&[], PerformDial::Gate, 1.0, 7).is_empty());
    }

    #[test]
    fn the_panel_keeps_the_swing_in_front_of_the_wander() {
        // Turned up in the opposite order, the stack still plays swing first: the swing has to
        // find its offbeats on the grid before the humanise moves them off it.
        let stack = with_dial(&[], PerformDial::Humanize, 0.3, 7);
        let stack = with_dial(&stack, PerformDial::Swing, 1.0, 7);
        assert!(matches!(stack[0], NoteTransform::Swing { .. }), "{stack:?}");
        assert!(matches!(stack[1], NoteTransform::Humanize { .. }));
    }

    #[test]
    fn moving_a_dial_keeps_the_seed_and_the_grid_it_had() {
        let stack = vec![
            NoteTransform::Swing {
                percent: 60,
                subdivision: Subdivision::Eighth,
            },
            NoteTransform::Humanize {
                amount: 0.2,
                seed: 99,
            },
        ];
        let moved = with_dial(&stack, PerformDial::Humanize, 0.7, 1);
        assert!(matches!(moved[1], NoteTransform::Humanize { seed: 99, .. }));
        let moved = with_dial(&moved, PerformDial::Swing, 1.0, 1);
        assert!(matches!(
            moved[0],
            NoteTransform::Swing {
                percent: 75,
                subdivision: Subdivision::Eighth,
            }
        ));
    }

    #[test]
    fn a_transform_the_panel_does_not_know_survives_every_dial() {
        let stack = vec![NoteTransform::Transpose { semitones: -12 }];
        let stack = with_dial(&stack, PerformDial::Swing, 0.6, 7);
        let stack = with_dial(&stack, PerformDial::Gate, 0.4, 7);
        let stack = with_dial(&stack, PerformDial::Swing, 0.0, 7);
        assert!(
            stack
                .iter()
                .any(|transform| matches!(transform, NoteTransform::Transpose { semitones: -12 })),
            "{stack:?}"
        );
    }

    #[test]
    fn the_swing_grid_is_changed_in_place_and_nowhere_else() {
        let stack = with_dial(&[], PerformDial::Swing, 0.6, 7);
        let pointed = with_swing_grid(&stack, Subdivision::Eighth);
        assert_eq!(swing_grid(&pointed), Subdivision::Eighth);
        assert_eq!(swing_percent(&pointed), swing_percent(&stack));
        assert!(with_swing_grid(&[], Subdivision::Eighth).is_empty());
    }

    #[test]
    fn the_readout_speaks_percentages_and_straight_in_words() {
        let stack = with_dial(&[], PerformDial::Humanize, 0.35, 7);
        assert_eq!(dial_text(&stack, PerformDial::Humanize, "straight"), "35%");
        assert_eq!(
            dial_text(&stack, PerformDial::Swing, "straight"),
            "straight"
        );
        let stack = with_dial(&stack, PerformDial::Swing, 0.68, 7);
        assert_eq!(dial_text(&stack, PerformDial::Swing, "straight"), "67%");
    }

    #[test]
    fn every_dial_gets_its_own_element_key() {
        let mut keys: Vec<usize> = PERFORM_DIALS
            .iter()
            .map(|dial| dial_element_key(*dial))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), PERFORM_DIALS.len());
    }
}
