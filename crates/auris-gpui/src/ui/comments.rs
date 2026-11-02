//! Comments on screen: the small tags pinned along a track's lane and inside the piano roll.
//!
//! A comment is text the engine never reads, so it is drawn as an element rather than painted
//! into the lane canvas: an element gets hover, a tooltip with the whole text, a press and a menu
//! of its own for free, and none of those would be worth rebuilding as hit tests on a canvas.
//! Each tag stops its press from reaching the surface under it, so taking hold of a comment
//! never also starts a rubber band or writes a note.
//!
//! Where they are kept and why is [`Comment`]'s account; this file is only where they are shown.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{
    AnyElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, SharedString, div,
    prelude::*, px,
};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::automation::RowKind;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::prompt::{Prompt, PromptTarget};

/// How tall a comment tag is.
pub(crate) const TAG_HEIGHT: Pixels = px(16.0);
/// The widest a tag gets before its text is cut; the whole text is in the tooltip.
const TAG_MAX_WIDTH: Pixels = px(180.0);
/// How many characters of the first line a tag shows.
const TAG_CHARS: usize = 28;

/// What a tag says: the first line, cut where it would run long.
pub(crate) fn tag_text(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut shown: String = line.chars().take(TAG_CHARS).collect();
    if shown.len() < text.len() {
        shown.push('…');
    }
    shown
}

/// The selector a window test finds a tag by.
pub(crate) fn tag_selector(host: CommentHost, id: CommentId) -> String {
    match host {
        CommentHost::Track(track) => format!("comment-track-{}-{}", track.0, id.0),
        CommentHost::Clip(clip) => format!("comment-clip-{}-{}", clip.0, id.0),
    }
}

impl AurisApp {
    /// Opens the text sheet for a new comment pinned at `at` (and `pitch`, inside a clip).
    pub(crate) fn prompt_for_new_comment(
        &mut self,
        host: CommentHost,
        at: Ticks,
        pitch: Option<u8>,
    ) {
        let title = self.t(Key::CommentTitle);
        self.open_prompt(Prompt::new(
            title,
            PromptTarget::NewComment { host, at, pitch },
            String::new(),
        ));
    }

    /// Opens the text sheet on an existing comment.
    pub(crate) fn prompt_for_comment(&mut self, host: CommentHost, id: CommentId) {
        let Some(text) = self
            .session
            .comment(host, id)
            .map(|comment| comment.text.clone())
        else {
            return;
        };
        let title = self.t(Key::CommentTitle);
        self.open_prompt(Prompt::new(title, PromptTarget::Comment { host, id }, text));
    }

    /// The menu a right-click on a tag opens, headed by what the comment says.
    pub(crate) fn comment_menu(
        &self,
        anchor: Point<Pixels>,
        host: CommentHost,
        id: CommentId,
    ) -> ContextMenu {
        let title = self.session.comment(host, id).map_or_else(
            || self.t(Key::MenuComment).to_string(),
            |c| tag_text(&c.text),
        );
        ContextMenu::new(anchor, title)
            .item(
                self.t(Key::MenuEditComment),
                MenuCommand::EditComment { host, id },
            )
            .item(
                self.t(Key::MenuRemoveComment),
                MenuCommand::RemoveComment { host, id },
            )
    }

    /// Moves the comment in hand to follow the pointer.
    pub(crate) fn drag_comment(
        &mut self,
        host: CommentHost,
        id: CommentId,
        grab_offset: Ticks,
        event: &MouseMoveEvent,
    ) {
        let Some(comment) = self.session.comment(host, id) else {
            return;
        };
        let pinned_to_a_row = comment.pitch.is_some();
        let (at, pitch) = match host {
            CommentHost::Track(_) => {
                let x = event.position.x - self.lanes_origin().x;
                let at = self
                    .snap_unless_held(self.timeline.x_to_tick(x) - grab_offset, event.modifiers)
                    .max_zero();
                (at, None)
            }
            CommentHost::Clip(clip) => {
                let Some((start, length)) = self
                    .session
                    .midi_clip(clip)
                    .map(|clip| (clip.start, clip.length))
                else {
                    return;
                };
                let origin = self.roll_origin();
                let song = self.snap_unless_held(
                    self.timeline.x_to_tick(event.position.x - origin.x) - grab_offset,
                    event.modifiers,
                );
                let at = (song - start).max_zero().min(length);
                // A comment pinned to a row follows the pointer up and down; one pinned to the
                // moment alone stays along the top.
                let pitch = if pinned_to_a_row {
                    self.pitch
                        .pitch_at(event.position.y - origin.y)
                        .or(comment.pitch)
                } else {
                    None
                };
                (at, pitch)
            }
        };
        let _ = self.session.move_comment(host, id, at, pitch);
    }

    /// One tag, with its left edge at the pin.
    fn comment_tag(
        &self,
        host: CommentHost,
        comment: &Comment,
        left: Pixels,
        top: Pixels,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let id = comment.id;
        let selector = tag_selector(host, id);
        let full = comment.text.clone();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .absolute()
            .left(left)
            .top(top)
            .h(TAG_HEIGHT)
            .max_w(TAG_MAX_WIDTH)
            .flex()
            .items_center()
            .gap_1()
            .pr_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(Metrics::RADIUS_XS)
            .border_1()
            .border_color(theme.warning)
            .bg(theme.surface_raised)
            .hover(|style| style.bg(theme.surface_hover))
            .text_xs()
            .text_color(theme.text)
            .cursor(gpui::CursorStyle::PointingHand)
            // The pin itself: a stripe on the tag's left edge, standing exactly on its moment.
            .child(div().w(px(3.0)).h_full().bg(theme.warning))
            .child(tag_text(&comment.text))
            .tooltip(crate::ui::tooltip::keyed_tip(full, "", theme))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.press_comment(host, id, event);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let menu = this.comment_menu(event.position, host, id);
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// A press on a tag: a double click opens its text, a single one takes hold of it.
    fn press_comment(&mut self, host: CommentHost, id: CommentId, event: &MouseDownEvent) {
        if event.click_count >= 2 {
            self.prompt_for_comment(host, id);
            return;
        }
        let Some(at) = self.session.comment(host, id).map(|comment| comment.at) else {
            return;
        };
        let (pointer, pinned) = match host {
            CommentHost::Track(_) => (
                self.timeline
                    .x_to_tick(event.position.x - self.lanes_origin().x),
                at,
            ),
            CommentHost::Clip(clip) => {
                let start = self
                    .session
                    .midi_clip(clip)
                    .map_or(Ticks::ZERO, |clip| clip.start);
                (
                    self.timeline
                        .x_to_tick(event.position.x - self.roll_origin().x),
                    start + at,
                )
            }
        };
        self.begin_drag(Drag::Comment {
            host,
            id,
            grab_offset: (pointer - pinned).max_zero(),
        });
    }

    /// Every track comment as a tag along the foot of its lane, in the lane column's coordinates.
    pub(crate) fn lane_comment_tags(&self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let mut tags = Vec::new();
        for row in self.lane_rows() {
            if !matches!(row.kind, RowKind::Clips) {
                continue;
            }
            let Some(track) = self.project().track(row.track) else {
                continue;
            };
            // Above the linter's strip along the foot of the lane, so both stay in reach.
            let top = row.top - self.lane_scroll + row.height
                - TAG_HEIGHT
                - crate::ui::lint::MARK_STRIP
                - px(2.0);
            for comment in &track.comments {
                let left = self.timeline.tick_to_x(comment.at);
                tags.push(self.comment_tag(CommentHost::Track(track.id), comment, left, top, cx));
            }
        }
        tags
    }

    /// The open clip's comments as tags over the roll, in the roll's coordinates.
    pub(crate) fn roll_comment_tags(&self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let Some(clip) = self.selected_clip.and_then(|id| self.session.midi_clip(id)) else {
            return Vec::new();
        };
        clip.comments
            .iter()
            .map(|comment| {
                let left = self.timeline.tick_to_x(clip.start + comment.at);
                let top = match comment.pitch {
                    // Just above its row, so the note it is about stays in sight under the pin.
                    Some(pitch) => (self.pitch.pitch_to_y(pitch) - TAG_HEIGHT).max(px(0.0)),
                    None => px(2.0),
                };
                self.comment_tag(CommentHost::Clip(clip.id), comment, left, top, cx)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_shows_the_first_line_and_says_when_there_is_more() {
        assert_eq!(tag_text("short"), "short");
        assert_eq!(tag_text("first\nsecond"), "first…");
        let long = "a".repeat(40);
        assert_eq!(tag_text(&long).chars().count(), TAG_CHARS + 1);
    }
}

#[cfg(test)]
mod window_tests {
    use gpui::TestAppContext;

    use auris_session::prelude::*;

    use crate::harness::{lane_point, paint, roll_point, show_pitch, with_a_clip};

    fn tag(
        cx: &mut gpui::VisualTestContext,
        host: CommentHost,
        id: CommentId,
    ) -> gpui::Bounds<gpui::Pixels> {
        let selector: &'static str = Box::leak(super::tag_selector(host, id).into_boxed_str());
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("no tag drawn for {host:?} {id:?}"))
    }

    #[gpui::test]
    fn a_comment_is_added_from_the_lane_menu_and_dragged_along_it(cx: &mut TestAppContext) {
        let (app, cx, track, _) = with_a_clip(cx);
        let host = CommentHost::Track(track);
        // Past the clip, on empty lane: the lane's own menu.
        let at = lane_point(&app, cx, track, Ticks(4 * 960 + 1920));
        crate::harness::right_press(cx, at);
        let offered = app.read_with(cx, |this, _| {
            this.menu.as_ref().is_some_and(|menu| {
                menu.entries.iter().any(|entry| {
                    matches!(entry, crate::ui::context_menu::MenuEntry::Item(item)
                        if matches!(item.command, crate::ui::context_menu::MenuCommand::AddCommentAt { host: h, .. } if h == host))
                })
            })
        });
        assert!(offered, "the lane menu offers a comment there");

        app.update(cx, |this, cx| {
            this.menu = None;
            this.run_menu_command(
                crate::ui::context_menu::MenuCommand::AddCommentAt {
                    host,
                    at: Ticks(5760),
                    pitch: None,
                },
                cx,
            );
            assert!(matches!(
                this.prompt.as_ref().and_then(|prompt| prompt.target()),
                Some(crate::ui::prompt::PromptTarget::NewComment { .. })
            ));
        });
        cx.simulate_input("ここはベースを休ませる");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        let id = app.read_with(cx, |this, _| {
            let comments = &this.project().track(track).unwrap().comments;
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, "ここはベースを休ませる");
            assert_eq!(comments[0].at, Ticks(5760));
            comments[0].id
        });

        let bounds = tag(cx, host, id);
        let from = bounds.center();
        let step = app.read_with(cx, |this, _| {
            this.timeline.tick_to_x(Ticks(960)) - this.timeline.tick_to_x(Ticks::ZERO)
        });
        crate::harness::drag(cx, from, gpui::point(from.x + step, from.y));
        app.read_with(cx, |this, _| {
            let comment = &this.project().track(track).unwrap().comments[0];
            assert_eq!(comment.at, Ticks(5760 + 960), "moved one beat, on the grid");
            assert!(
                this.project()
                    .track(track)
                    .unwrap()
                    .kind
                    .note_clips()
                    .unwrap()
                    .len()
                    == 1,
                "the drag took the comment, not a clip or a rubber band"
            );
        });
        app.update(cx, |this, _| {
            assert!(this.session.undo().is_some());
            assert_eq!(
                this.project().track(track).unwrap().comments[0].at,
                Ticks(5760)
            );
        });
    }

    #[gpui::test]
    fn a_comment_in_the_roll_sits_on_its_row_and_a_press_on_it_writes_no_note(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| this.select_clip(Some(clip)));
        show_pitch(&app, cx, 60);
        let host = CommentHost::Clip(clip);
        let id = app.update(cx, |this, _| {
            this.session
                .add_comment(host, Ticks(960), Some(55), "もっと強く")
                .unwrap()
        });
        paint(&app, cx);
        let bounds = tag(cx, host, id);
        let row = roll_point(&app, cx, Ticks(960), 55);
        let row_top = row.y - gpui::px(app.read_with(cx, |this, _| this.pitch.row_height) / 2.0);
        assert!(
            (bounds.bottom() - row_top).abs() < gpui::px(2.0),
            "the tag sits just above its pitch row, leaving the note in sight"
        );
        assert!(
            (bounds.left() - row.x).abs() < gpui::px(1.0),
            "its left edge is the pin"
        );

        crate::harness::click_at(cx, bounds.center(), crate::harness::creating());
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.project().midi_clip(clip).unwrap().1.notes.is_empty(),
                "pressing a comment never writes a note under it"
            );
        });

        crate::harness::right_press(cx, bounds.center());
        app.update(cx, |this, cx| {
            let menu = this.menu.take().expect("the tag's own menu");
            assert_eq!(menu.title.as_ref(), "もっと強く");
            this.run_menu_command(
                crate::ui::context_menu::MenuCommand::RemoveComment { host, id },
                cx,
            );
            assert!(
                this.project()
                    .midi_clip(clip)
                    .unwrap()
                    .1
                    .comments
                    .is_empty()
            );
        });
    }
}
