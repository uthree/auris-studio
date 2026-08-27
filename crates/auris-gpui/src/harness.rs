//! The whole window, driven from `cargo test`.
//!
//! gpui ships a platform with no display, no GPU and no font system behind it, and this crate's
//! dev-dependency on `gpui/test-support` is what switches it on. What is left is the real
//! application: the real keymap, the real view tree, the real session and the real commands —
//! everything except the pixels and the audio device. So a test can press a key, click a button
//! by name, and then ask the document what happened, which is most of what "does the interface
//! still work" was being checked by hand for.
//!
//! What it cannot check is what anything *looks like*. gpui's test platform lays text out through
//! `NoopTextSystem`, which gives every glyph the same metrics, and its window throws the scene
//! away instead of rasterising it. So sizes that come from measured text are not the sizes on
//! screen, and nothing here may assert on a pixel. Colour, spacing and legibility stay a human's
//! job; *behaviour* stops being one.
//!
//! The other thing it cannot check is the transport. `Session::is_playing` reads an atomic the
//! *audio thread* writes, and a session with no device has no audio thread to write it — so Play
//! is sent and nothing ever comes back. Assert on the document and on the view state, which are
//! written where the command runs; anything that only becomes true once a block has been
//! rendered belongs in `auris-engine`'s own tests, where there is an offline renderer to run it.

use std::sync::Once;

use auris_session::prelude::*;
use gpui::{
    Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point, px,
};

use crate::app::{AurisApp, Pane};
use crate::ui::automation::RowKind;

/// The rectangle the window is laid out in for a test.
///
/// The size `main` prefers, so that a panel which is only drawn when there is room for it is
/// drawn here too — a layout that collapses at 640 pixels would otherwise hide half the controls
/// a test is trying to click.
const CANVAS: gpui::Size<Pixels> = gpui::Size {
    width: px(1500.),
    height: px(940.),
};

/// Points every `load()` in the frontend at a directory of this run's own.
///
/// The settings, the keymap, the colour scheme, the panel layout and the progression book are all
/// read from `config_dir()`. Left alone, a test would take the developer's own preferences as its
/// starting state — passing or failing depending on whose machine it ran on — and could write
/// back over them. `AURIS_CONFIG_DIR` is the override the session layer already has for this.
fn isolate_config() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("auris-gpui-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp directory can be made");
        // SAFETY: the first thing every test in this crate does, under a `Once`, and before
        // anything in the frontend has read the environment.
        unsafe { std::env::set_var(auris_session::CONFIG_DIR_VAR, &dir) };
    });
}

/// Opens the application in a test window, as `main` opens it in a real one.
pub(crate) fn open(cx: &mut TestAppContext) -> (Entity<AurisApp>, &mut VisualTestContext) {
    isolate_config();
    let (app, cx) = cx.add_window_view(|_, cx| AurisApp::new(cx));
    // `main` focuses the arrangement before anything else, and a keystroke goes to whatever holds
    // the keyboard: without this, every binding scoped to a pane would be off the dispatch path
    // and the test would be checking a window nobody had clicked into yet.
    cx.update(|window, cx| {
        app.update(cx, |this, _| this.focus_pane(Pane::Arrangement, window));
    });
    cx.run_until_parked();
    (app, cx)
}

/// How long the clip a fixture makes is.
///
/// Four beats, which at the opening zoom is a hundred and ninety-two pixels — wide enough that
/// its middle is nowhere near either resize edge, so a press there is unambiguously a press on
/// the clip itself.
pub(crate) const CLIP_LENGTH: Ticks = Ticks(4 * TICKS_PER_QUARTER);

/// A window holding one instrument track with one clip at the top of the timeline, painted.
///
/// Nearly every gesture in the arrangement needs something to take hold of, and the document a
/// window opens with is empty. Built through the session rather than through the interface: what
/// the fixture is for is the gesture the *test* makes, and a clip created by a gesture that is
/// itself under test would leave two things able to fail in one line.
pub(crate) fn with_a_clip(
    cx: &mut TestAppContext,
) -> (Entity<AurisApp>, &mut VisualTestContext, TrackId, ClipId) {
    let (app, cx) = open(cx);
    let (track, clip) = app.update(cx, |this, _| {
        let track = this
            .session
            .add_default_instrument_track("Test")
            .expect("the registry nominates an instrument");
        let clip = this
            .session
            .add_midi_clip(track, "Clip", Ticks::ZERO, CLIP_LENGTH)
            .expect("an instrument track takes a MIDI clip");
        (track, clip)
    });
    paint(&app, cx);
    (app, cx, track, clip)
}

/// Lays the window out and paints it, so that a click has something to land on.
///
/// gpui's test window is never asked for a frame by a platform that does not exist, so nothing is
/// drawn until a test says so. Hit testing reads the last frame, which makes this the line that
/// has to come before any [`click`].
pub(crate) fn paint(app: &Entity<AurisApp>, cx: &mut VisualTestContext) {
    let view = app.clone();
    cx.draw(point(px(0.), px(0.)), CANVAS, |_, _| view);
}

/// Clicks the control that [`crate::ui::widgets::icon_button`] gave this id.
///
/// Panics rather than returning, and says what it was looking for: a selector that matches
/// nothing is a test asking about a button that is not on screen, and the reason it is not is
/// what the test is for.
pub(crate) fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing called `{selector}` was drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

/// Presses at `from`, moves to `to` and lets go — one whole gesture.
///
/// Two moves rather than one. A drag that jumps straight to its destination is a gesture no hand
/// ever makes, and it would step over every rule that reads the *previous* position: the travel
/// threshold that separates a press from a move, and the lane a selection is being carried
/// through. The midpoint is where a real pointer would have been.
pub(crate) fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    press(cx, from);
    drag_to(cx, point((from.x + to.x) / 2., (from.y + to.y) / 2.));
    drag_to(cx, to);
    release(cx, to);
}

/// Puts the left button down at `at`, and leaves it down.
pub(crate) fn press(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
}

/// Moves the pointer to `at` with the left button still held.
pub(crate) fn drag_to(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
}

/// Lets the left button go at `at`.
pub(crate) fn release(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
}

/// The window point that lands on `track`'s clip lane at `tick`.
///
/// Read out of the view rather than worked out here: the arrangement scrolls in both directions
/// and a track's height is the user's, so the only coordinate that means anything is the one the
/// application itself would compute. The row's vertical centre, which is clear of the fade band
/// along its top and of the seam with the row below.
///
/// [`paint`] has to have run, or the lanes have no recorded origin to measure from.
pub(crate) fn lane_point(
    app: &Entity<AurisApp>,
    cx: &mut VisualTestContext,
    track: TrackId,
    tick: Ticks,
) -> Point<Pixels> {
    app.read_with(cx, |this, _| {
        let origin = this.lanes_origin();
        let row = this
            .lane_rows()
            .into_iter()
            .find(|row| row.track == track && matches!(row.kind, RowKind::Clips))
            .expect("every track has a clip lane");
        point(
            origin.x + this.timeline.tick_to_x(tick),
            origin.y + row.top + row.height / 2.0 - this.lane_scroll,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions;

    /// The window opens at all: a session, a keymap, a theme and a full view tree, with no
    /// display and no audio device anywhere.
    #[gpui::test]
    fn the_application_opens_in_a_window_with_nothing_behind_it(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(!this.session.audio_status().running, "no device is opened");
        });
    }

    /// A menu command, dispatched where the menu dispatches it, reaching the document.
    #[gpui::test]
    fn an_action_from_the_menu_edits_the_document(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.read_with(cx, |this, _| this.session.project().tracks.len());
        cx.dispatch_action(actions::AddInstrumentTrack);
        let after = app.read_with(cx, |this, _| this.session.project().tracks.len());
        assert_eq!(after, before + 1, "Track → Add Instrument Track added one");
    }

    /// The same command through the keyboard, which is the half a dispatched action skips: the
    /// binding table, the `secondary-` translation that means ⌘ on one platform and Ctrl on the
    /// other, the key context the window names, and the pane holding focus.
    #[gpui::test]
    fn a_keystroke_reaches_the_command_it_is_bound_to(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.read_with(cx, |this, _| assert!(!this.session.project().loop_enabled));
        cx.simulate_keystrokes("secondary-l");
        app.read_with(cx, |this, _| {
            assert!(
                this.session.project().loop_enabled,
                "secondary-l is bound to ToggleLoop"
            );
        });
    }

    /// A pointer at a position, hit-testing against a real frame — the path a keystroke never
    /// takes, and the one that breaks when a control moves out from under its own click handler.
    #[gpui::test]
    fn clicking_the_cycle_button_turns_the_loop_on(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        paint(&app, cx);
        click("loop", cx);
        app.read_with(cx, |this, _| assert!(this.session.project().loop_enabled));
    }
}
