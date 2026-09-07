//! Paired song controls, using the generated-clip inspector's pad gestures.

use super::{SongDial, SongDials};
use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::paint;
use gpui::{
    AnyElement, Bounds, Context, MouseButton, MouseDownEvent, Pixels, Point, canvas, div, point,
    prelude::*, px, size,
};

fn axes(detail: bool) -> (SongDial, SongDial) {
    if detail {
        (SongDial::Tension, SongDial::Syncopation)
    } else {
        (SongDial::Brightness, SongDial::Energy)
    }
}

impl AurisApp {
    pub(super) fn song_pad(
        &self,
        dials: &SongDials,
        detail: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (horizontal, vertical) = axes(detail);
        let x = horizontal.fraction(dials);
        let y = vertical.fraction(dials);
        let theme = self.theme.clone();
        let painted = theme.clone();
        let recorded = std::rc::Rc::new(std::cell::Cell::new(None));
        let pressed = recorded.clone();
        let id = if detail {
            "song-rhythm-pad"
        } else {
            "song-mood-pad"
        };
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .child(div().text_xs().text_color(theme.text_muted).child(format!(
                "→ {} {} · ↑ {} {}",
                self.t(horizontal.label()),
                horizontal.text(dials),
                self.t(vertical.label()),
                vertical.text(dials)
            )))
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.to_string())
                    .h(px(112.0))
                    .w_full()
                    .flex_shrink_0()
                    .p_2()
                    .bg(theme.surface_sunken)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Metrics::RADIUS_SM)
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |bounds, _, _| recorded.set(Some(bounds)),
                            move |bounds, _, window, _| {
                                let x = bounds.left() + bounds.size.width * x;
                                let y = bounds.top() + bounds.size.height * (1.0 - y);
                                paint::vline(
                                    window,
                                    bounds,
                                    bounds.center().x,
                                    px(1.0),
                                    painted.border_subtle,
                                );
                                paint::hline(
                                    window,
                                    bounds,
                                    bounds.center().y,
                                    painted.border_subtle,
                                );
                                paint::vline(window, bounds, x, px(1.0), painted.accent_soft);
                                paint::hline(window, bounds, y, painted.accent_soft);
                                paint::rounded_rect(
                                    window,
                                    Bounds {
                                        origin: point(x - px(6.0), y - px(6.0)),
                                        size: size(px(12.0), px(12.0)),
                                    },
                                    px(6.0),
                                    painted.accent,
                                );
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if let Some(bounds) = pressed.get() {
                                this.begin_drag(Drag::SongPad { detail, bounds });
                                this.drag_song_pad(detail, bounds, event.position);
                                cx.notify();
                            }
                        }),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn drag_song_pad(
        &mut self,
        detail: bool,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
    ) {
        if let Some(dials) = self.song_sheet.as_mut() {
            let (horizontal, vertical) = axes(detail);
            horizontal.set(
                dials,
                (position.x - bounds.left()) / bounds.size.width.max(px(1.0)),
            );
            vertical.set(
                dials,
                1.0 - (position.y - bounds.top()) / bounds.size.height.max(px(1.0)),
            );
            // The mood and seed now supply the melodic contour together.
            dials.motif.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{drag, open, paint};

    #[gpui::test]
    fn song_pads_follow_a_gesture_on_both_axes_and_preserve_the_seed(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.song_sheet.as_mut().unwrap().motif = vec![0, 2, 4];
        });
        paint(&app, cx);
        for (id, detail) in [("song-mood-pad", false), ("song-rhythm-pad", true)] {
            let before = app.read_with(cx, |this, _| this.song_sheet.clone().unwrap());
            let bounds = cx.debug_bounds(id).expect("song pad is visible");
            drag(
                cx,
                bounds.center(),
                point(bounds.right() - px(10.0), bounds.top() + px(10.0)),
            );
            app.read_with(cx, |this, _| {
                let dials = this.song_sheet.as_ref().unwrap();
                let (x, y) = axes(detail);
                assert!(x.fraction(dials) > 0.9 && y.fraction(dials) > 0.9);
                assert_eq!(dials.seed, before.seed);
                assert!(dials.motif.is_empty());
                assert_eq!(dials.parts, before.parts);
            });
        }
    }
}
