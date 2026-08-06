//! The arrangement: track headers on the left, a clip timeline on the right.
//!
//! Five surfaces share the screen — the header column, the ruler, the structure and harmony
//! strips, and the clip lanes — and they sit on one layer of pure functions that decides where a
//! press lands. That layer is `geometry`, and it is the only part of the arrangement that can be
//! exercised without a window, which is why every test in the module is in that file.
//!
//! The rest is cut by what a reader is looking for: `headers` is the left column, `strips` the
//! two lanes between the ruler and the clips, `lanes` the right-hand column and the snapshots it
//! takes of the document, `lane_paint` the free functions that turn one of those snapshots into
//! pixels, `gestures` what the pointer does over the clips, and `rows` where each lane sits in
//! the column.
//!
//! They are private modules, and the two free functions the rest of the crate could name are
//! re-exported, so every path into the arrangement is the one it always was. Almost everything
//! here is an `impl AurisApp` block rather than a type of its own — the panels all read the one
//! document — so the file a method lives in is a matter of where a reader would look for it and
//! nothing else.

mod geometry;
mod gestures;
mod headers;
mod lane_paint;
mod lanes;
mod rows;
mod strips;

pub(crate) use geometry::{reveal_offset, scroll_limit};

use gpui::{Axis, IntoElement, MouseDownEvent, Window, div, prelude::*};

use crate::app::{AurisApp, Drag};
use crate::ui::widgets::splitter;

impl AurisApp {
    /// Renders the arrangement panel.
    pub(crate) fn render_arrangement(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let headers = self.render_track_headers(cx);
        let timeline = self.render_timeline(cx);
        div()
            .flex()
            .flex_1()
            .min_h(crate::dock::PanelLayout::MIN_ARRANGEMENT)
            .overflow_hidden()
            .bg(theme.surface)
            .child(headers)
            .child(splitter(
                "split-headers",
                Axis::Vertical,
                &theme,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    let start_width = this.panels.header_width;
                    this.begin_drag(Drag::ResizeHeaders {
                        start_x: event.position.x,
                        start_width,
                    });
                }),
            ))
            .child(timeline)
    }
}
