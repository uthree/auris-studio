//! The structure and harmony lanes: the two strips between the ruler and the clips.
//!
//! One file because they are the same shape twice — a full-width canvas belonging to no track, a
//! left press that either takes a boundary's handle or does the lane's own thing with the tick
//! under it, a right press that opens a menu, and a wheel that must move the same timeline the
//! clips move on. What differs is only what is written on them: sections above, chords below,
//! the song's coarsest division above its harmony.
//!
//! Both grab bars are `super::geometry`'s, painted at the same inset by the same rule. A second
//! copy of that rule used to live here and had drifted off the inset.

use auris_session::prelude::*;

use gpui::{IntoElement, MouseButton, MouseDownEvent, Pixels, canvas, div, prelude::*, px};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::paint;

use super::geometry::{chord_handle_at, section_handle_at};

impl AurisApp {
    /// The strip of section names under the ruler: イントロ, Aメロ, サビ.
    ///
    /// Above the harmony because it is the coarser division — the stack reads ruler, structure,
    /// harmony, lanes: the song from its largest units down to its notes.
    pub(super) fn render_structure_lane(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();

        let width = self
            .canvas
            .structure
            .get()
            .map_or(px(1200.0), |b| b.size.width);
        let (from, to) = view.visible_range(width);
        let sections = &self.project().sections;
        let structure = paint::StructurePaint {
            spans: sections
                .spans_in(from, to)
                .into_iter()
                .map(|span| {
                    // Numbered only when the label repeats: サビ 1 and サビ 2 need telling
                    // apart, a lone イントロ does not.
                    let shown = if sections.repeats(&span.label) > 1 {
                        format!("{} {}", span.label, span.instance)
                    } else {
                        span.label.clone()
                    };
                    (span, shown)
                })
                .collect(),
            held: match self.drag {
                Some(Drag::SectionLabel { at, .. }) => Some(at),
                _ => None,
            },
        };

        div()
            .id("structure-lane")
            .h(Metrics::STRUCTURE_LANE_HEIGHT)
            .w_full()
            .cursor_pointer()
            .child({
                let recorded = self.canvas.structure.clone();
                canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    move |bounds, _, window, cx| {
                        paint::clipped(window, bounds, |window| {
                            paint::structure_lane(window, cx, bounds, &view, &structure, &theme);
                        });
                    },
                )
                .size_full()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.press_structure_lane(event);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                AurisApp::opens_menu(cx, |this, at| {
                    let x = at.x - this.timeline_origin().x;
                    let tick = this.timeline.x_to_tick(x).max_zero();
                    this.structure_menu(at, tick)
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                this.scroll_timeline(event, cx);
            }))
    }

    /// What a left press on the structure lane does: grab a boundary's handle, or — doubled —
    /// open the naming sheet for the section under the pointer.
    fn press_structure_lane(&mut self, event: &MouseDownEvent) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.timeline.x_to_tick(x).max_zero();
        if event.click_count >= 2 {
            self.prompt_for_section(tick);
            return;
        }
        let width = self
            .canvas
            .structure
            .get()
            .map_or(px(1200.0), |bounds| bounds.size.width);
        let visible = self.timeline.visible_range(width);
        if let Some(at) =
            section_handle_at(&self.timeline, self.project().sections.points(), visible, x)
        {
            self.begin_drag(Drag::SectionLabel {
                at,
                grab_offset: tick - at,
            });
        }
    }

    /// The strip of chords under the structure lane.
    ///
    /// It sits above the clip lanes and spans all of them, because that is what harmony is: one
    /// thing the whole arrangement obeys at any one moment, belonging to no track.
    pub(super) fn render_harmony_lane(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();

        // Only what is on screen is painted, and it is copied out because a paint closure has to
        // capture `'static`.
        let width = self
            .canvas
            .harmony
            .get()
            .map_or(px(1200.0), |b| b.size.width);
        let (from, to) = view.visible_range(width);
        let harmony = paint::HarmonyPaint {
            events: self
                .project()
                .harmony
                .events_in(from, to, &self.project().signatures),
            keys: self.project().harmony.keys.points().to_vec(),
            handles: self.chord_handles(),
            held: match self.drag {
                Some(Drag::HarmonyChord { at, .. }) => Some(at),
                _ => None,
            },
        };

        div()
            .id("harmony-lane")
            .h(Metrics::HARMONY_LANE_HEIGHT)
            .w_full()
            .cursor_pointer()
            .child({
                let recorded = self.canvas.harmony.clone();
                canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    move |bounds, _, window, cx| {
                        paint::clipped(window, bounds, |window| {
                            paint::harmony_lane(window, cx, bounds, &view, &harmony, &theme);
                        });
                    },
                )
                .size_full()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.press_harmony_lane(event);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                AurisApp::opens_menu(cx, |this, at| {
                    let x = at.x - this.timeline_origin().x;
                    let tick = this.timeline.x_to_tick(x).max_zero();
                    this.harmony_menu(at, tick)
                }),
            )
            // The chords sit above the clips they belong to and share their horizontal scale, so
            // the wheel has to move both together or the two rows disagree about where bar 9 is.
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                this.scroll_timeline(event, cx);
            }))
    }

    /// What a left press on the harmony lane grabs: a chord's handle, or the sound of it.
    ///
    /// Pressing anywhere but a handle sounds the chord written there, the way you would press a
    /// piano key, and sweeping along the lane plays the progression past. It sounds until the
    /// button comes up, which the window's own mouse-up handler takes care of.
    ///
    /// The tick an audition reads is *not* snapped, unlike everything that writes. A menu acts on
    /// a grid position and so rounds; an audition answers "what is written here", and rounding
    /// forward would sound the chord after the one under the pointer.
    fn press_harmony_lane(&mut self, event: &MouseDownEvent) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.timeline.x_to_tick(x).max_zero();

        if self.harmony_row_at(event.position.y) == Some(HarmonyRow::Chords)
            && let Some(at) = chord_handle_at(&self.timeline, &self.chord_handles(), x)
        {
            self.begin_drag(Drag::HarmonyChord {
                at,
                grab_offset: tick - at,
            });
            return;
        }
        self.begin_drag(Drag::AuditionHarmony);
        self.audition_chord(tick);
    }

    /// Which row of the harmony lane a window-space `y` falls in, if either.
    fn harmony_row_at(&self, y: Pixels) -> Option<HarmonyRow> {
        let bounds = self.canvas.harmony.get()?;
        let (keys, chords) = paint::harmony_rows(bounds);
        if (keys.origin.y..keys.origin.y + keys.size.height).contains(&y) {
            Some(HarmonyRow::Keys)
        } else if (chords.origin.y..chords.origin.y + chords.size.height).contains(&y) {
            Some(HarmonyRow::Chords)
        } else {
            None
        }
    }

    /// Where each chord block on screen may be taken hold of.
    ///
    /// A key change splits a chord into two blocks, and only the first of them is where the
    /// numeral is actually written — so only that one gets a handle. Dragging the second would
    /// move a chord that starts somewhere off to the left of it.
    pub(crate) fn chord_handles(&self) -> Vec<Ticks> {
        let width = self
            .canvas
            .harmony
            .get()
            .map_or(px(1200.0), |bounds| bounds.size.width);
        let (from, to) = self.timeline.visible_range(width);
        self.project()
            .harmony
            .chords
            .points()
            .iter()
            .filter(|point| point.chord.is_some() && point.tick >= from && point.tick <= to)
            .map(|point| point.tick)
            .collect()
    }
}

/// One of the harmony lane's two rows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum HarmonyRow {
    /// The thin strip of key changes along the top.
    Keys,
    /// The chord blocks below it.
    Chords,
}
