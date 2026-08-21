//! One scrollbar, wired once, for every panel that has one.
//!
//! Five panels scroll, and each of them needs the same four things: somewhere to keep the offset,
//! a bar drawn from it, the rectangle that bar was drawn into so a press can be measured against
//! it, and a drag that carries the thumb afterwards. The mixer had all four written out by hand;
//! writing them out four more times is how the copies start to disagree about which end of the
//! track means the end of the content.
//!
//! So the panels are an enum, and everything that differs between them is a `match` in this file:
//! which way the panel scrolls, and where its offset is kept. The arithmetic is
//! [`crate::ui::widgets::scrollbar_thumb`] and its two neighbours, which never knew about a panel
//! or an axis at all.

use gpui::{
    Axis, Bounds, Div, MouseDownEvent, Pixels, Point, ScrollHandle, Stateful, canvas, div, point,
    prelude::*, px,
};

use crate::app::{AurisApp, Drag};

/// A panel that scrolls, and so draws a bar saying where in it you are.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScrollPanel {
    /// The browser on the left.
    Library,
    /// The track and clip settings on the right.
    Inspector,
    /// The message log along the bottom.
    Log,
    /// The arrangement's clip lanes, which carry the track headers beside them.
    Lanes,
    /// The channel strips, the one panel that scrolls sideways.
    Mixer,
}

impl ScrollPanel {
    /// Every panel there is, in the order that indexes the arrays keyed by one.
    pub(crate) const ALL: [ScrollPanel; 5] = [
        ScrollPanel::Library,
        ScrollPanel::Inspector,
        ScrollPanel::Log,
        ScrollPanel::Lanes,
        ScrollPanel::Mixer,
    ];

    /// How many there are.
    pub(crate) const COUNT: usize = ScrollPanel::ALL.len();

    /// Where this panel sits in an array keyed by panel.
    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    /// Which way the panel scrolls, and so which way its bar is drawn and dragged.
    pub(crate) fn axis(self) -> Axis {
        match self {
            ScrollPanel::Mixer => Axis::Horizontal,
            _ => Axis::Vertical,
        }
    }

    /// The bar's element id, which is what gpui keeps its hitbox under between frames.
    fn element_id(self) -> &'static str {
        match self {
            ScrollPanel::Library => "library-scrollbar",
            ScrollPanel::Inspector => "inspector-scrollbar",
            ScrollPanel::Log => "log-scrollbar",
            ScrollPanel::Lanes => "lanes-scrollbar",
            ScrollPanel::Mixer => "mixer-scrollbar",
        }
    }
}

/// Where a panel has been scrolled to, in the three numbers a bar is drawn from.
///
/// gpui's own convention throughout: `offset` is zero at the start and `-max_offset` at the end,
/// because it measures how far the *content* has been moved rather than how far the view has
/// travelled.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ScrollView {
    /// How far the content has been moved, at or below zero.
    pub(crate) offset: f32,
    /// How much further it can go before its end is flush with the end of the view.
    pub(crate) max_offset: f32,
    /// How much of it is visible.
    pub(crate) viewport: f32,
}

/// gpui's scroll offset for a column that keeps its own, measured downward from the top.
///
/// The arrangement's lane column is scrolled by a `Pixels` counting how far the content has been
/// pushed *up*; gpui counts the same distance as a move *down*, and so as a negative. Stating the
/// conversion once, in both directions, is what stops a bar drawn under one convention from being
/// dragged under the other — the thumb would start at the far end and walk backwards, which reads
/// as a broken widget rather than as a sign error.
pub(crate) fn as_scroll_offset(scrolled: Pixels) -> f32 {
    -f32::from(scrolled)
}

/// How far a column has been pushed up, from gpui's offset for it.
pub(crate) fn as_scrolled(offset: f32) -> Pixels {
    px(-offset)
}

impl AurisApp {
    /// The scroll position gpui keeps for a panel, for the four that scroll a container.
    ///
    /// `None` for the lane column, which is painted on a canvas at an offset of its own rather
    /// than laid out inside a scrolling element — there is nothing there for gpui to keep.
    fn scroll_handle(&self, panel: ScrollPanel) -> Option<&ScrollHandle> {
        match panel {
            ScrollPanel::Library => Some(&self.library_scroll),
            ScrollPanel::Inspector => Some(&self.inspector_scroll),
            ScrollPanel::Log => Some(&self.log_scroll),
            ScrollPanel::Mixer => Some(&self.mixer_scroll),
            ScrollPanel::Lanes => None,
        }
    }

    /// Where a panel has been scrolled to.
    pub(crate) fn scroll_view(&self, panel: ScrollPanel) -> ScrollView {
        let Some(handle) = self.scroll_handle(panel) else {
            return ScrollView {
                offset: as_scroll_offset(self.lane_scroll),
                max_offset: f32::from(self.max_lane_scroll()),
                viewport: f32::from(
                    self.canvas
                        .lanes
                        .get()
                        .map_or(px(0.0), |bounds| bounds.size.height),
                ),
            };
        };
        match panel.axis() {
            Axis::Horizontal => ScrollView {
                offset: f32::from(handle.offset().x),
                max_offset: f32::from(handle.max_offset().width),
                viewport: f32::from(handle.bounds().size.width),
            },
            Axis::Vertical => ScrollView {
                offset: f32::from(handle.offset().y),
                max_offset: f32::from(handle.max_offset().height),
                viewport: f32::from(handle.bounds().size.height),
            },
        }
    }

    /// Scrolls a panel to `offset`, in gpui's convention.
    pub(crate) fn set_scroll_offset(&mut self, panel: ScrollPanel, offset: f32) {
        let Some(handle) = self.scroll_handle(panel) else {
            // Clamped again here, though the caller clamps too: the lane column's limit moves as
            // tracks are opened and closed, and an offset that was the bottom a moment ago is
            // past the end once an automation lane closes under the pointer.
            self.lane_scroll = as_scrolled(offset).clamp(px(0.0), self.max_lane_scroll());
            return;
        };
        let at = handle.offset();
        match panel.axis() {
            Axis::Horizontal => handle.set_offset(point(px(offset), at.y)),
            Axis::Vertical => handle.set_offset(point(at.x, px(offset))),
        }
    }

    /// How much of a panel its own bar is taking up, which is nothing while everything fits.
    ///
    /// For the one caller that has to reserve the same strip in a *different* element: the
    /// arrangement's ruler is above the lanes rather than inside them, and a ruler running on past
    /// the last lane would put bar numbers over a stretch of song that has no lane to drop a clip
    /// into.
    pub(crate) fn scrollbar_width(&self, panel: ScrollPanel) -> Pixels {
        let view = self.scroll_view(panel);
        match crate::ui::widgets::scrollbar_thumb(view.offset, view.max_offset, view.viewport) {
            Some(_) => crate::ui::widgets::SCROLLBAR_THICKNESS,
            None => px(0.0),
        }
    }

    /// A panel's scrolling body, with its bar beside it.
    ///
    /// The bar takes its own strip of the panel rather than floating over the content, and takes
    /// none at all while everything fits. An overlay would sit on the last few pixels of whatever
    /// is being read, and in a list of names those are the pixels the long ones run out into.
    pub(crate) fn scrolling(
        &self,
        panel: ScrollPanel,
        body: Stateful<Div>,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let body = match self.scroll_handle(panel) {
            Some(handle) => body.track_scroll(handle),
            None => body,
        };
        div()
            .flex()
            .map(|wrapper| match panel.axis() {
                Axis::Horizontal => wrapper.flex_col(),
                Axis::Vertical => wrapper.flex_row(),
            })
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(body.flex_1().min_h_0().min_w_0())
            .child(self.panel_scrollbar(panel, cx))
    }

    /// The bar itself, and the canvas that remembers where it was drawn.
    ///
    /// The press is measured against the bar rather than against the panel it scrolls: a panel
    /// carries its own padding, and a fraction taken from the wrong rectangle puts the thumb a
    /// few pixels from the pointer at one end of the track and nowhere near it at the other.
    fn panel_scrollbar(&self, panel: ScrollPanel, cx: &mut gpui::Context<Self>) -> Div {
        let view = self.scroll_view(panel);
        let recorded = self.canvas.scrollbar(panel);
        div()
            .relative()
            .child(
                crate::ui::widgets::scrollbar(
                    panel.axis(),
                    panel.element_id(),
                    view.offset,
                    view.max_offset,
                    view.viewport,
                    &self.theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.press_scrollbar(panel, event, cx);
                    }),
                )
                .into_any_element(),
            )
            .child(
                canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full(),
            )
    }

    /// Takes hold of a bar, jumping to the pointer first when the press landed off the thumb.
    fn press_scrollbar(
        &mut self,
        panel: ScrollPanel,
        event: &MouseDownEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(bar) = self.canvas.scrollbar(panel).get() else {
            return;
        };
        let view = self.scroll_view(panel);
        let (along, track) = measure(panel.axis(), event.position, bar);
        let offset = crate::ui::widgets::scrollbar_pressed(
            along / track.max(1.0),
            view.offset,
            view.max_offset,
            view.viewport,
        );
        self.set_scroll_offset(panel, offset);
        self.drag = Some(Drag::PanelScroll {
            panel,
            start: along_axis(panel.axis(), event.position),
            start_offset: offset,
        });
        cx.notify();
    }

    /// Carries a bar the pointer has hold of.
    pub(crate) fn drag_scrollbar(
        &mut self,
        panel: ScrollPanel,
        at: Point<Pixels>,
        start: Pixels,
        start_offset: f32,
    ) {
        let view = self.scroll_view(panel);
        let travelled = along_axis(panel.axis(), at) - start;
        let offset = crate::ui::widgets::scrollbar_dragged(
            start_offset,
            f32::from(travelled),
            view.max_offset,
            view.viewport,
        );
        self.set_scroll_offset(panel, offset);
    }
}

/// The pointer's coordinate along an axis.
fn along_axis(axis: Axis, at: Point<Pixels>) -> Pixels {
    match axis {
        Axis::Horizontal => at.x,
        Axis::Vertical => at.y,
    }
}

/// How far into a bar a press landed, and how long that bar is.
fn measure(axis: Axis, at: Point<Pixels>, bar: Bounds<Pixels>) -> (f32, f32) {
    match axis {
        Axis::Horizontal => (f32::from(at.x - bar.origin.x), f32::from(bar.size.width)),
        Axis::Vertical => (f32::from(at.y - bar.origin.y), f32::from(bar.size.height)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::scrollbar_thumb;

    #[test]
    fn every_panel_indexes_its_own_slot() {
        // The bar rectangles are an array keyed by `index`, so a variant added without a matching
        // entry in `ALL` would quietly hand two panels the same cell.
        for (slot, panel) in ScrollPanel::ALL.into_iter().enumerate() {
            assert_eq!(panel.index(), slot);
        }
        assert_eq!(ScrollPanel::COUNT, ScrollPanel::ALL.len());
    }

    #[test]
    fn a_column_scrolled_halfway_puts_its_thumb_halfway() {
        // The lane column counts its scroll downward and gpui counts it as a negative; the bar is
        // drawn in gpui's, so this conversion is what the arrangement's thumb rides on.
        let scrolled = px(300.0);
        assert_eq!(as_scroll_offset(scrolled), -300.0);
        assert_eq!(as_scrolled(as_scroll_offset(scrolled)), scrolled);

        let (start, length) =
            scrollbar_thumb(as_scroll_offset(scrolled), 600.0, 400.0).expect("scrollable");
        assert!((start - (1.0 - length) / 2.0).abs() < 1e-6);
    }
}
