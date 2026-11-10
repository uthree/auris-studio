//! Text typed where it is shown, rather than in a sheet over the window.
//!
//! A track's name, a lyric line, a section, a chord, a comment: each is written on the screen
//! already, and a rename that dims the window and asks for it again in a box in the middle is a
//! rename that makes the user look away from the thing they are naming. An inline prompt is the
//! same [`Prompt`] — the same field, the same commit, the same validation and error — drawn on
//! top of the label it edits, sized to it, with nothing behind it dimmed.
//!
//! Where the label is comes from one of two places. Most of these things are drawn from
//! geometry the view already keeps — the timeline's scale, the lane rows, the roll's pitch rows
//! — so their rectangle is *computed* from the target ([`InlinePlace::Target`]). A track's name
//! is laid out by flex in two different panels, so its label records where it was painted with
//! Atelier's [`atelier::SpotBook::spot`] and the prompt names that record
//! ([`InlinePlace::Spot`]). When neither can say where
//! the label is — the panel is closed, the strip is hidden — the prompt falls back to the sheet,
//! so nothing that used to be editable stops being so.
//!
//! Return commits and Escape cancels, as on the sheet. A press anywhere else commits too, which
//! is what every list of names does — except that leaving a field untouched, or emptied where
//! empty is not an answer, closes it instead, so wandering off a box never writes an edit
//! nobody made or traps the pointer behind an error.

use auris_session::prelude::*;

use gpui::{
    AnyElement, Bounds, Context, IntoElement, MouseButton, MouseDownEvent, Pixels, SharedString,
    div, point, prelude::*, px, size,
};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::automation::RowKind;
use crate::ui::prompt::{Prompt, PromptTarget};

/// Where an inline prompt is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlinePlace {
    /// Over the element that recorded this key with [`atelier::SpotBook::spot`] when it was
    /// last painted.
    Spot(SharedString),
    /// Over the target itself, from the geometry the view draws it with.
    Target,
}

/// The narrowest an inline field is drawn, whatever it covers.
///
/// A chord block one beat wide at a far zoom is a few pixels; the field over it still has to hold
/// `bVII7` and a caret.
const MIN_WIDTH: Pixels = px(150.0);
/// The widest a field over a long stretch is drawn: a lyric line twenty bars long does not need a
/// box twenty bars long.
const MAX_WIDTH: Pixels = px(420.0);
/// How wide a comment's editor opens: room for a sentence before the text scrolls.
const COMMENT_WIDTH: Pixels = px(260.0);
/// The height of a one-line inline field.
pub(crate) const LINE_HEIGHT: Pixels = px(22.0);

/// The spot key of a track's name in the arrangement's header.
pub(crate) fn header_name_spot(track: TrackId) -> SharedString {
    format!("track-name:{}", track.0).into()
}

/// The spot key of a track's name on its mixer strip.
pub(crate) fn strip_name_spot(track: TrackId) -> SharedString {
    format!("strip-name:{}", track.0).into()
}

/// The spot key of the transport's position readout.
pub(crate) const POSITION_SPOT: &str = "transport:position";
/// The spot key of the transport's tempo readout.
pub(crate) const TEMPO_SPOT: &str = "transport:tempo";

/// Whether a target is written over several lines, so its field is an area rather than a line.
///
/// Only a comment: every other inline target is a name or a notation, where a line break would
/// be a mistake, and a lyric line's breaks are the song's lines.
pub(crate) fn is_multiline(target: PromptTarget) -> bool {
    matches!(
        target,
        PromptTarget::Comment { .. } | PromptTarget::NewComment { .. }
    )
}

/// `bounds` widened to at least [`MIN_WIDTH`] and at most [`MAX_WIDTH`], kept inside `within`
/// horizontally when it can be.
fn fitted(
    bounds: Bounds<Pixels>,
    height: Pixels,
    within: Option<Bounds<Pixels>>,
) -> Bounds<Pixels> {
    let width = bounds.size.width.max(MIN_WIDTH).min(MAX_WIDTH);
    let mut left = bounds.origin.x;
    if let Some(within) = within {
        let right = within.origin.x + within.size.width;
        if left + width > right {
            left = right - width;
        }
        left = left.max(within.origin.x);
    }
    Bounds {
        origin: point(left, bounds.origin.y),
        size: size(width, height),
    }
}

/// A rectangle from `from` to `to` on the timeline, `height` tall from `top`.
fn span(
    app: &AurisApp,
    origin_x: Pixels,
    from: Ticks,
    to: Ticks,
    top: Pixels,
    height: Pixels,
) -> Bounds<Pixels> {
    let left = origin_x + app.timeline.tick_to_x(from);
    let right = origin_x + app.timeline.tick_to_x(to);
    Bounds {
        origin: point(left, top),
        size: size((right - left).max(px(0.0)), height),
    }
}

impl AurisApp {
    /// Where an open prompt is drawn in place, or `None` when it is a sheet.
    pub(crate) fn inline_bounds(
        &self,
        prompt: &Prompt,
        window: &gpui::Window,
    ) -> Option<Bounds<Pixels>> {
        let target = prompt.target()?;
        match prompt.place()? {
            InlinePlace::Spot(key) => {
                let book = &self.canvas.spots;
                let found = book.get(key, window).or_else(|| match target {
                    // A rename opened from a menu does not know which panel asked; the other
                    // one's label will do when the first is not on screen.
                    PromptTarget::Track(track) => book
                        .get(&header_name_spot(track), window)
                        .or_else(|| book.get(&strip_name_spot(track), window)),
                    _ => None,
                })?;
                let height = found.size.height.max(LINE_HEIGHT);
                let top = found.origin.y + (found.size.height - height) / 2.0;
                Some(fitted(
                    Bounds {
                        origin: point(found.origin.x - px(4.0), top),
                        size: size(found.size.width + px(8.0), height),
                    },
                    height,
                    None,
                ))
            }
            InlinePlace::Target => self.target_bounds(prompt, target, window),
        }
    }

    /// The rectangle a target is drawn in, computed from the view's own geometry.
    fn target_bounds(
        &self,
        prompt: &Prompt,
        target: PromptTarget,
        window: &gpui::Window,
    ) -> Option<Bounds<Pixels>> {
        match target {
            PromptTarget::Section(at) => {
                let lane = self.canvas.structure.get()?;
                let (from, to) = self.timeline.visible_range(lane.size.width);
                let found = self
                    .project()
                    .sections
                    .spans_in(from, to)
                    .into_iter()
                    .find(|span| span.start <= at && at < span.end)
                    .map(|span| (span.start, span.end));
                Some(self.strip_bounds(lane, at, found))
            }
            PromptTarget::LyricLine(at) => {
                let lane = self.canvas.lyrics.get()?;
                let (from, to) = self.timeline.visible_range(lane.size.width);
                let found = self
                    .project()
                    .lyrics
                    .spans_in(from, to)
                    .into_iter()
                    .find(|span| span.start <= at && at < span.end)
                    .map(|span| (span.start, span.end));
                Some(self.strip_bounds(lane, at, found))
            }
            PromptTarget::Key(at) | PromptTarget::Chord(at) => {
                let lane = self.canvas.harmony.get()?;
                let (keys, chords) = crate::ui::paint::harmony_rows(lane, self.panels.lanes.keys);
                let row = match target {
                    PromptTarget::Key(_) if keys.size.height > px(0.0) => keys,
                    PromptTarget::Key(_) => return None,
                    _ => chords,
                };
                if row.size.height <= px(0.0) {
                    return None;
                }
                let at = self.session.snap_harmony(at);
                let height = row.size.height.max(LINE_HEIGHT);
                let top = row.origin.y + (row.size.height - height) / 2.0;
                let mut found = span(self, lane.origin.x, at, at, top, height);
                found.size.width = MIN_WIDTH;
                Some(fitted(found, height, Some(lane)))
            }
            PromptTarget::Clip(clip) => {
                let lanes = self.canvas.lanes.get()?;
                let (start, end) = self.clip_extent(clip)?;
                let track = self.project().track_of_clip(clip)?;
                let row = self
                    .lane_rows()
                    .into_iter()
                    .find(|row| row.track == track && matches!(row.kind, RowKind::Clips))?;
                let top = lanes.origin.y + row.top - self.lane_scroll;
                if top + LINE_HEIGHT < lanes.origin.y || top > lanes.origin.y + lanes.size.height {
                    return None;
                }
                let found = span(self, lanes.origin.x, start, end, top, LINE_HEIGHT);
                Some(fitted(found, LINE_HEIGHT, Some(lanes)))
            }
            PromptTarget::Lyric { clip, index } | PromptTarget::Phonemes { clip, index } => {
                let roll = self.canvas.roll.get()?;
                if self.selected_clip != Some(clip) {
                    return None;
                }
                let midi = self.session.midi_clip(clip)?;
                let note = midi.notes.get(index)?;
                let origin = self.roll_origin();
                let row = px(self.pitch.row_height);
                let top = origin.y + self.pitch.pitch_to_y(note.pitch);
                if top + row < roll.origin.y || top > roll.origin.y + roll.size.height {
                    return None;
                }
                let start = midi.start + note.start;
                let found = span(
                    self,
                    origin.x,
                    start,
                    start + note.length,
                    // Centred on the note's row, so the field covers the word written on it.
                    top + (row - LINE_HEIGHT) / 2.0,
                    LINE_HEIGHT,
                );
                Some(fitted(found, LINE_HEIGHT, Some(roll)))
            }
            PromptTarget::NewComment { host, at, pitch } => {
                self.comment_bounds(prompt, host, at, pitch)
            }
            PromptTarget::Comment { host, id } => self
                .comment_tag_bounds(prompt, host, id, window)
                .or_else(|| {
                    let comment = self.session.comment(host, id)?;
                    self.comment_bounds(prompt, host, comment.at, comment.pitch)
                }),
            _ => None,
        }
    }

    /// The field over a strip of named stretches: over the stretch at `at` when there is one,
    /// starting at `at` when it is a silence about to be given its first words.
    fn strip_bounds(
        &self,
        lane: Bounds<Pixels>,
        at: Ticks,
        found: Option<(Ticks, Ticks)>,
    ) -> Bounds<Pixels> {
        let (from, to) = found.unwrap_or((at, at));
        let height = lane.size.height.max(LINE_HEIGHT);
        // Centred on the strip: a strip thinner than a field still has the field over it.
        let top = lane.origin.y + (lane.size.height - height) / 2.0;
        let bounds = span(self, lane.origin.x, from, to, top, height);
        fitted(bounds, height, Some(lane))
    }

    /// The editor of an existing comment, opened over its tag where the tag recorded its spot.
    ///
    /// Every tag records one under [`crate::ui::comments::tag_selector`], so a host whose tags
    /// are laid out by flex rather than by the timeline is found the same way.
    fn comment_tag_bounds(
        &self,
        prompt: &Prompt,
        host: CommentHost,
        id: CommentId,
        window: &gpui::Window,
    ) -> Option<Bounds<Pixels>> {
        let tag = self
            .canvas
            .spots
            .get(&crate::ui::comments::tag_selector(host, id), window)?;
        let text = prompt.field().map_or("", |field| field.content());
        let height = crate::ui::text_area::area_height(text, 2, 6);
        Some(Bounds {
            origin: tag.origin,
            size: size(COMMENT_WIDTH, height),
        })
    }

    /// The editor of a comment pinned at `at` (and `pitch`), where its tag is or will be.
    fn comment_bounds(
        &self,
        prompt: &Prompt,
        host: CommentHost,
        at: Ticks,
        pitch: Option<u8>,
    ) -> Option<Bounds<Pixels>> {
        let text = prompt.field().map_or("", |field| field.content());
        let height = crate::ui::text_area::area_height(text, 2, 6);
        let (left, top, within) = match host {
            CommentHost::Track(track) => {
                let lanes = self.canvas.lanes.get()?;
                let row = self
                    .lane_rows()
                    .into_iter()
                    .find(|row| row.track == track && matches!(row.kind, RowKind::Clips))?;
                // Where the tag stands: along the foot of the lane, above the linter's strip.
                let tag_top = row.top - self.lane_scroll + row.height
                    - crate::ui::comments::TAG_HEIGHT
                    - crate::ui::lint::MARK_STRIP
                    - px(2.0);
                (
                    lanes.origin.x + self.timeline.tick_to_x(at),
                    lanes.origin.y + tag_top,
                    lanes,
                )
            }
            CommentHost::Clip(clip) => {
                let roll = self.canvas.roll.get()?;
                if self.selected_clip != Some(clip) {
                    return None;
                }
                let start = self.session.midi_clip(clip)?.start;
                let origin = self.roll_origin();
                let tag_top = match pitch {
                    Some(pitch) => (self.pitch.pitch_to_y(pitch) - crate::ui::comments::TAG_HEIGHT)
                        .max(px(0.0)),
                    None => px(2.0),
                };
                (
                    origin.x + self.timeline.tick_to_x(start + at),
                    origin.y + tag_top,
                    roll,
                )
            }
            // A host drawn somewhere this file does not compute — a new kind of comment is found
            // through its tag's spot, and until it has one it opens as the sheet.
            #[allow(unreachable_patterns)]
            _ => return None,
        };
        // Kept inside the surface it belongs to: a tag at the foot of the last lane opens its
        // editor upwards rather than under the status line.
        let bottom = within.origin.y + within.size.height;
        let top = if top + height > bottom {
            (bottom - height).max(within.origin.y)
        } else {
            top
        };
        let mut bounds = fitted(
            Bounds {
                origin: point(left, top),
                size: size(COMMENT_WIDTH, height),
            },
            height,
            Some(within),
        );
        bounds.size.width = COMMENT_WIDTH;
        Some(bounds)
    }

    /// Draws an inline prompt: the field over its label, and under it whatever the sheet would
    /// have said under its field — an error, a hint, the completions.
    pub(crate) fn render_inline_prompt(
        &self,
        prompt: &Prompt,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let target = prompt.target()?;
        let field = prompt.field()?;
        let theme = self.theme.clone();
        let multiline = is_multiline(target);
        let text: SharedString = field.content().to_string().into();
        let editor = if multiline {
            crate::ui::text_area::editable_area(
                text,
                field.selection(),
                field.marked(),
                Vec::new(),
                self.focus.clone(),
                cx.entity(),
                theme.clone(),
            )
            .into_any_element()
        } else {
            crate::ui::prompt::editable_text(
                text,
                field.selection(),
                field.marked(),
                self.focus.clone(),
                cx.entity(),
                theme.clone(),
            )
            .into_any_element()
        };
        let error = prompt.error().cloned();
        let hint = target.hint().map(|hint| self.t(hint));
        let completions = self.render_prompt_completions(prompt, cx);
        let below = error.is_some() || hint.is_some() || completions.is_some();
        Some(
            div()
                .id("inline-prompt")
                .debug_selector(|| "inline-prompt".into())
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .flex()
                .flex_col()
                .gap_1()
                // Anything outside the box finishes the edit; the press itself goes on to
                // whatever it landed on, so clicking the next name both commits this one and
                // starts on that.
                .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.leave_inline_prompt(target, window, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .id("inline-prompt-field")
                        .debug_selector(|| "prompt-text-field".into())
                        .h(bounds.size.height)
                        .w_full()
                        .rounded(Metrics::RADIUS_SM)
                        .bg(theme.surface_sunken)
                        .border_1()
                        .border_color(if error.is_some() {
                            theme.danger
                        } else {
                            theme.accent
                        })
                        .shadow_md()
                        .occlude()
                        .cursor(gpui::CursorStyle::IBeam)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                let focus = this.focus.clone();
                                if let Some(field) =
                                    this.prompt.as_mut().and_then(Prompt::field_mut)
                                {
                                    if multiline {
                                        match crate::ui::text_area::area_offset_at(
                                            window,
                                            field.content(),
                                            event.position,
                                        ) {
                                            Some(offset) => {
                                                field.place_caret(offset, event.modifiers.shift)
                                            }
                                            None => field.caret_to_end(),
                                        }
                                    } else {
                                        crate::ui::prompt::click_field(
                                            field, &focus, true, window, event,
                                        );
                                    }
                                }
                                focus.focus(window);
                                cx.notify();
                            }),
                        )
                        .child(editor),
                )
                .when(below, |this| {
                    this.child(
                        div()
                            .id("inline-prompt-notes")
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_1()
                            .min_w(px(220.0))
                            .max_w(px(360.0))
                            .rounded(Metrics::RADIUS_SM)
                            .bg(theme.surface_raised)
                            .border_1()
                            .border_color(theme.border)
                            .shadow_md()
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                            )
                            .children(error.map(|error| {
                                div()
                                    .id("prompt-error")
                                    .debug_selector(|| "prompt-error".into())
                                    .text_xs()
                                    .text_color(theme.danger)
                                    .child(error)
                            }))
                            .children(hint.map(|hint| {
                                div()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(hint.to_string())
                            }))
                            .children(completions),
                    )
                })
                .into_any_element(),
        )
    }

    /// Finishes an inline edit because the pointer went somewhere else.
    ///
    /// Commits what was typed, unless nothing was — an untouched field, or one emptied where
    /// empty is not an answer — in which case it simply closes, so wandering off never writes an
    /// edit nobody made, and never leaves a box behind complaining that a name cannot be empty.
    /// Only acts on the prompt it was drawn for: the press that landed outside may already have
    /// opened another.
    pub(crate) fn leave_inline_prompt(
        &mut self,
        target: PromptTarget,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.prompt.as_ref() else {
            return;
        };
        if prompt.target() != Some(target) || prompt.place().is_none() {
            return;
        }
        if prompt.field().is_some_and(|field| field.marked().is_some()) {
            // An IME still composing owns the text; a stray press must not cut it short.
            return;
        }
        if prompt.is_untouched() {
            self.cancel_prompt();
        } else {
            self.commit_prompt(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_target_still_gets_a_field_wide_enough_to_type_in() {
        let narrow = Bounds {
            origin: point(px(100.0), px(10.0)),
            size: size(px(12.0), px(18.0)),
        };
        let fitted = fitted(narrow, LINE_HEIGHT, None);
        assert_eq!(fitted.size.width, MIN_WIDTH);
        assert_eq!(fitted.origin.x, px(100.0));
        assert_eq!(fitted.size.height, LINE_HEIGHT);
    }

    #[test]
    fn a_field_at_the_right_edge_opens_leftwards_inside_its_surface() {
        let within = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(400.0), px(100.0)),
        };
        let at_edge = Bounds {
            origin: point(px(380.0), px(10.0)),
            size: size(px(10.0), px(18.0)),
        };
        let fitted = fitted(at_edge, LINE_HEIGHT, Some(within));
        assert_eq!(fitted.origin.x + fitted.size.width, px(400.0));
    }

    #[test]
    fn a_long_stretch_does_not_make_a_box_as_long() {
        let long = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(3000.0), px(18.0)),
        };
        assert_eq!(fitted(long, LINE_HEIGHT, None).size.width, MAX_WIDTH);
    }

    #[test]
    fn only_comments_are_written_over_several_lines() {
        assert!(is_multiline(PromptTarget::Comment {
            host: CommentHost::Track(TrackId(1)),
            id: CommentId(1),
        }));
        assert!(!is_multiline(PromptTarget::Track(TrackId(1))));
        assert!(!is_multiline(PromptTarget::LyricLine(Ticks::ZERO)));
    }
}

#[cfg(test)]
mod window_tests {
    use gpui::{Modifiers, TestAppContext, point, px};

    use auris_session::prelude::*;

    use crate::harness::{double_click, lane_point, paint, with_a_clip};
    use crate::ui::prompt::PromptTarget;

    /// Where the open prompt is drawn in place, or `None` when it came up as the sheet.
    fn inline(
        app: &gpui::Entity<crate::app::AurisApp>,
        cx: &mut gpui::VisualTestContext,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        paint(app, cx);
        cx.update(|window, cx| {
            let this = app.read(cx);
            this.prompt
                .as_ref()
                .and_then(|prompt| this.inline_bounds(prompt, window))
        })
    }

    fn header_name(
        app: &gpui::Entity<crate::app::AurisApp>,
        cx: &mut gpui::VisualTestContext,
        track: TrackId,
    ) -> gpui::Bounds<gpui::Pixels> {
        cx.update(|window, cx| {
            app.read(cx)
                .canvas
                .spots
                .get(&super::header_name_spot(track), window)
                .expect("the header's name was painted")
        })
    }

    #[gpui::test]
    fn a_double_click_on_a_track_name_types_over_the_name(cx: &mut TestAppContext) {
        let (app, cx, track, _) = with_a_clip(cx);
        let name = header_name(&app, cx, track);
        double_click(cx, name.center());
        let field = inline(&app, cx).expect("the rename is drawn in place, not as a sheet");
        assert!(
            field.contains(&name.center()),
            "the field covers the name it edits"
        );
        assert!(
            cx.debug_bounds("prompt-body").is_none(),
            "no sheet is drawn behind it"
        );
        cx.simulate_input("Lead");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert_eq!(this.project().track(track).unwrap().name, "Lead");
        });
    }

    #[gpui::test]
    fn pressing_elsewhere_commits_what_was_typed_and_closes_what_was_not(cx: &mut TestAppContext) {
        let (app, cx, track, _) = with_a_clip(cx);
        let elsewhere = lane_point(&app, cx, track, Ticks(8 * 960));

        app.update(cx, |this, _| this.prompt_to_rename_track(track));
        assert!(inline(&app, cx).is_some());
        cx.simulate_input("Bass");
        cx.simulate_mouse_down(elsewhere, gpui::MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(elsewhere, gpui::MouseButton::Left, Modifiers::none());
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none(), "leaving the field finished the edit");
            assert_eq!(this.project().track(track).unwrap().name, "Bass");
        });

        app.update(cx, |this, _| this.prompt_to_rename_track(track));
        assert!(inline(&app, cx).is_some());
        let before = app.read_with(cx, |this, _| this.session.revision());
        cx.simulate_mouse_down(elsewhere, gpui::MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(elsewhere, gpui::MouseButton::Left, Modifiers::none());
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none(), "an untouched field simply closes");
            assert_eq!(this.session.revision(), before, "and writes nothing");
        });

        // Emptied, a name is no answer: leaving closes the field rather than trapping the
        // pointer behind "a name cannot be empty".
        app.update(cx, |this, _| this.prompt_to_rename_track(track));
        assert!(inline(&app, cx).is_some());
        cx.simulate_keystrokes("backspace");
        cx.simulate_mouse_down(elsewhere, gpui::MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(elsewhere, gpui::MouseButton::Left, Modifiers::none());
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert_eq!(this.project().track(track).unwrap().name, "Bass");
        });
    }

    #[gpui::test]
    fn a_rejected_answer_keeps_the_field_in_place_with_its_reason(cx: &mut TestAppContext) {
        let (app, cx, _, _) = with_a_clip(cx);
        app.update(cx, |this, cx| {
            this.run_menu_command(
                crate::ui::context_menu::MenuCommand::SetKeyAt(Ticks::ZERO),
                cx,
            );
        });
        assert!(inline(&app, cx).is_some(), "a key is typed on the key row");
        cx.simulate_keystrokes("backspace");
        cx.simulate_input("H flat major");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        assert!(
            cx.debug_bounds("prompt-error").is_some(),
            "the reason is shown beside the field"
        );
        assert!(
            inline(&app, cx).is_some(),
            "and the field stays where it was"
        );
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(this.prompt.is_none()));
    }

    #[gpui::test]
    fn a_comment_is_written_over_several_lines_in_place(cx: &mut TestAppContext) {
        let (app, cx, track, _) = with_a_clip(cx);
        let host = CommentHost::Track(track);
        app.update(cx, |this, cx| {
            this.run_menu_command(
                crate::ui::context_menu::MenuCommand::AddCommentAt {
                    host,
                    at: Ticks(5760),
                    pitch: None,
                },
                cx,
            );
        });
        let field = inline(&app, cx).expect("a new comment is typed where its tag will stand");
        let pin = lane_point(&app, cx, track, Ticks(5760));
        assert!(
            (field.origin.x - pin.x).abs() < px(1.0),
            "its left edge is the pin"
        );
        cx.simulate_input("一行目");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("二行目");
        let grown = inline(&app, cx).expect("still in place");
        assert!(grown.size.height >= field.size.height);
        cx.simulate_keystrokes("enter");
        let id = app.read_with(cx, |this, _| {
            let comments = &this.project().track(track).unwrap().comments;
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, "一行目\n二行目");
            comments[0].id
        });

        app.update(cx, |this, _| this.prompt_for_comment(host, id));
        assert!(inline(&app, cx).is_some(), "editing it opens over its tag");
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("書き直し");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(track).unwrap().comments[0].text,
                "書き直し"
            );
        });
    }

    #[gpui::test]
    fn a_lyric_line_is_typed_on_the_lyrics_lane(cx: &mut TestAppContext) {
        let (app, cx, _, _) = with_a_clip(cx);
        let lane = app.read_with(cx, |this, _| this.canvas.lyrics.get().expect("lyrics lane"));
        double_click(cx, point(lane.origin.x + px(40.0), lane.center().y));
        let field = inline(&app, cx).expect("the line is typed on the lane");
        assert!(
            (field.center().y - lane.center().y).abs() < px(2.0),
            "on the lane's own row"
        );
        assert!(matches!(
            app.read_with(cx, |this, _| this.prompt.as_ref().and_then(|p| p.target())),
            Some(PromptTarget::LyricLine(_))
        ));
        cx.simulate_input("ラララ");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert!(
                this.project()
                    .lyrics
                    .points()
                    .iter()
                    .any(|point| point.text.as_deref() == Some("ラララ"))
            );
        });
    }

    #[gpui::test]
    fn a_label_that_is_not_on_screen_falls_back_to_the_sheet(cx: &mut TestAppContext) {
        let (app, cx, track, _) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.open_prompt(
                crate::ui::prompt::Prompt::new("Rename", PromptTarget::Track(track), "Test")
                    .inline(super::InlinePlace::Spot("nowhere".into())),
            );
        });
        // The track's own labels are on screen, so a missing spot for a track still finds the
        // header; a spot that names nothing for another target is the sheet.
        assert!(inline(&app, cx).is_some());
        app.update(cx, |this, _| {
            this.cancel_prompt();
            this.open_prompt(
                crate::ui::prompt::Prompt::new("Seed", PromptTarget::Seed(ClipId(1)), "1")
                    .inline(super::InlinePlace::Spot("nowhere".into())),
            );
        });
        assert!(inline(&app, cx).is_none());
        assert!(
            cx.debug_bounds("prompt-body").is_some(),
            "drawn as the sheet"
        );
    }
}
