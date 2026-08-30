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

/// The window a test opens in, which gpui's test platform makes 1920×1080 and never varies.
///
/// Worth knowing rather than assuming: the interface was laid out at 1500×940, so a test window
/// is *larger* than the one the design was drawn for and every panel that has a size at all has
/// room for it. A test that wants a window too small for something asks with [`resize`].
pub(crate) const WINDOW: gpui::Size<Pixels> = gpui::Size {
    width: px(1920.),
    height: px(1080.),
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

/// [`with_a_clip`], but the clip sits on a singer track: the fixture the lyric gestures need.
pub(crate) fn with_a_singer_clip(
    cx: &mut TestAppContext,
) -> (Entity<AurisApp>, &mut VisualTestContext, TrackId, ClipId) {
    let (app, cx) = open(cx);
    let (track, clip) = app.update(cx, |this, _| {
        let track = this.session.add_singer_track("Voice");
        let clip = this
            .session
            .add_midi_clip(track, "Verse", Ticks::ZERO, CLIP_LENGTH)
            .expect("a singer track takes a note clip");
        (track, clip)
    });
    paint(&app, cx);
    (app, cx, track, clip)
}

/// Draws the window again, so that a click has the current layout to land on.
///
/// Hit testing and [`VisualTestContext::debug_bounds`] both read the last frame, so anything that
/// changed the document behind the view's back has to be followed by this before a test can point
/// at the result.
///
/// A notify and a turn of the loop, and deliberately *not* `VisualTestContext::draw`. gpui draws
/// its dirty windows itself while flushing effects when it is built with `test-support`, so
/// painting the root view explicitly paints it a second time into the same frame — and every
/// mouse listener in the application is then registered twice. Each press fires its handler
/// twice, which is invisible in a gesture that is idempotent and quietly wrong in one that is
/// not: the note-create gesture makes a note and then, on the same press, takes hold of the note
/// it has just made and drags it away.
pub(crate) fn paint(app: &Entity<AurisApp>, cx: &mut VisualTestContext) {
    app.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

/// Resizes the window and paints it again.
///
/// Both halves, because either alone is a trap: a resize the view has not been redrawn after
/// leaves every recorded canvas where the old layout put it.
pub(crate) fn resize(app: &Entity<AurisApp>, cx: &mut VisualTestContext, size: gpui::Size<Pixels>) {
    cx.simulate_resize(size);
    paint(app, cx);
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

/// Presses the right button at `at`, which is how every surface opens its menu.
pub(crate) fn right_press(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
}

/// Clicks twice at `at` — the double click, made as the platform reports it.
///
/// Spelt out with `simulate_event` because the harness's ordinary helpers hard-code
/// `click_count: 1`: a second press through them is two single clicks, which is exactly the
/// distinction [`PointerGesture::DoubleClick`](crate::gestures::PointerGesture) exists to draw.
pub(crate) fn double_click(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_event(gpui::MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
    });
}

/// Clicks the row of the open context menu that carries `command`.
///
/// Found by command rather than by its words: a row is labelled in whatever language the
/// interface is in, and a test that matched on the label would be a test of the translations.
/// Panics when no row carries it, and says which rows were there instead — a command that has
/// quietly stopped being offered is the thing most worth being told about.
pub(crate) fn choose(
    app: &Entity<AurisApp>,
    cx: &mut VisualTestContext,
    command: &crate::ui::context_menu::MenuCommand,
) {
    let index = app.read_with(cx, |this, _| {
        let menu = this.menu.as_ref().expect("a menu is open");
        menu.entries
            .iter()
            .enumerate()
            .find_map(|(index, entry)| match entry {
                crate::ui::context_menu::MenuEntry::Item(item) if &item.command == command => {
                    Some(index)
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "no row for {command:?} in a menu holding {:?}",
                    menu.entries
                )
            })
    });
    // Leaked because `debug_bounds` keys on a `&'static str` and a row's name is only known once
    // the menu has been read. A handful of bytes per menu row a test clicks, in a test binary.
    let selector: &'static str = Box::leak(format!("menu-item-{index}").into_boxed_str());
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("row {index} of the open menu was not drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
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
        let at = point(
            origin.x + this.timeline.tick_to_x(tick),
            origin.y + row.top + row.height / 2.0 - this.lane_scroll,
        );
        within(this.canvas.lanes.get(), at, "the clip lanes")
    })
}

/// `at`, once it is known to be inside the surface that was asked about.
///
/// A coordinate off the edge of its own canvas is the failure mode this harness is most able to
/// hide: the press goes to whatever is drawn there instead, nothing happens, and a test that
/// asserts something did *not* change passes for the wrong reason. The scroll positions and the
/// zoom are the view's, so this is easy to walk into — better to be told than to be lied to.
fn within(canvas: Option<gpui::Bounds<Pixels>>, at: Point<Pixels>, surface: &str) -> Point<Pixels> {
    let Some(bounds) = canvas else {
        panic!("{surface} has not been painted — call `paint` first");
    };
    assert!(
        bounds.contains(&at),
        "{at:?} is outside {surface}, which was drawn at {bounds:?}: \
         scroll or zoom the view until the position asked for is on screen"
    );
    at
}

/// The window point that lands on the piano roll at `tick` and `pitch`.
///
/// `tick` is the timeline's, not the clip's: the roll draws against the same ruler the
/// arrangement does, which is what lets a clip's notes line up with the bars around it. The row's
/// vertical middle, so a rounding error in either direction stays inside the pitch that was asked
/// for.
///
/// [`paint`] has to have run with a clip open, or the roll has no recorded origin to measure from.
pub(crate) fn roll_point(
    app: &Entity<AurisApp>,
    cx: &mut VisualTestContext,
    tick: Ticks,
    pitch: u8,
) -> Point<Pixels> {
    app.read_with(cx, |this, _| {
        let origin = this.roll_origin();
        let at = point(
            origin.x + this.timeline.tick_to_x(tick),
            origin.y + this.pitch.pitch_to_y(pitch) + px(this.pitch.row_height / 2.0),
        );
        within(this.canvas.roll.get(), at, "the piano roll")
    })
}

/// Scrolls the roll until `pitch` is in the middle of it, and paints again.
///
/// A window opens showing the top two octaves of the keyboard, because that is where the view
/// starts and an empty clip gives `center_roll_on_selection` nothing to centre on. Anything a
/// test wants to press has to be brought into view first — the same thing a hand does with the
/// wheel before writing a note.
pub(crate) fn show_pitch(app: &Entity<AurisApp>, cx: &mut VisualTestContext, pitch: u8) {
    app.update(cx, |this, _| {
        let height = this
            .canvas
            .roll
            .get()
            .map_or(px(0.0), |bounds| bounds.size.height);
        this.pitch.center_on(pitch, height);
    });
    paint(app, cx);
}

/// Holds the platform's command modifier down for one gesture.
///
/// What the default create gesture is bound to, in the roll and on the lanes alike — ⌘ on macOS
/// and Ctrl elsewhere, which is [`Modifiers::secondary_key`]'s whole job. Never spell out either.
pub(crate) fn creating() -> Modifiers {
    Modifiers::secondary_key()
}

/// Holds the option key down for one gesture, which is what the default delete is bound to.
pub(crate) fn deleting() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::none()
    }
}

/// Clicks at `at` with `modifiers` held.
pub(crate) fn click_at(cx: &mut VisualTestContext, at: Point<Pixels>, modifiers: Modifiers) {
    cx.simulate_click(at, modifiers);
}

/// Presses at `from` with `modifiers` held, drags to `to` and lets go.
pub(crate) fn drag_with(
    cx: &mut VisualTestContext,
    from: Point<Pixels>,
    to: Point<Pixels>,
    modifiers: Modifiers,
) {
    cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(
        point((from.x + to.x) / 2., (from.y + to.y) / 2.),
        MouseButton::Left,
        modifiers,
    );
    cx.simulate_mouse_move(to, MouseButton::Left, modifiers);
    cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
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

    /// The agent panel opens by its action, and an unconfigured send opens its settings
    /// section instead of spawning anything — the whole panel driven with no child process
    /// and no model behind it.
    #[gpui::test]
    fn the_agent_panel_opens_and_an_unconfigured_send_asks_for_a_model(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        // Closed by hand, not assumed: the tests share one config directory, and any test
        // that toggles a panel writes the layout another test's window then opens with.
        app.update(cx, |this, _| this.panels.hide(crate::dock::Panel::Agent));
        cx.dispatch_action(actions::ToggleAgent);
        app.read_with(cx, |this, _| {
            assert!(this.panels.is_open(crate::dock::Panel::Agent));
        });
        paint(&app, cx);

        app.update(cx, |this, _| {
            // Whatever this machine has saved, the case under test is the unconfigured one.
            this.settings.agent = Default::default();
            this.focus_agent_field(crate::ui::agent_chat::AgentField::Chat);
            assert!(
                this.taking_text_input(),
                "the chat field claims the letters"
            );
            this.agent_chat.input.insert("make it louder");
            this.agent_send();
            assert!(
                this.agent_chat.configuring,
                "with no model named, sending opens the settings section"
            );
            assert!(
                matches!(
                    this.agent_chat.entries.as_slice(),
                    [crate::ui::agent_chat::ChatEntry::Note(_)]
                ),
                "nothing was sent anywhere, and the refusal is said rather than implied"
            );
        });
        paint(&app, cx);
        assert!(
            cx.debug_bounds("agent-model").is_some(),
            "the model field is on screen to be filled in"
        );
    }

    /// The regression from the second live run: a model picked from the dropdown but never
    /// applied was wiped by Enter, which loaded the saved (empty) preferences back over the
    /// form and then refused to send. Enter now applies a configured form on its way out.
    #[gpui::test]
    fn a_picked_model_survives_enter_and_is_applied(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        // Shown, not toggled: a toggle depends on the shared layout file's mood.
        app.update(cx, |this, _| this.panels.show(crate::dock::Panel::Agent));
        paint(&app, cx);
        app.update(cx, |this, _| {
            this.settings.agent = Default::default();
            this.agent_chat.configuring = true;
            let prefs = this.settings.agent.clone();
            this.agent_chat.load_preferences(&prefs);
            // What the dropdown's pick handler does, minus the mouse.
            this.agent_chat.chosen_model = "gpt-oss:20b".to_string();
            this.agent_chat.context_window = Some(131_072);
            this.focus_agent_field(crate::ui::agent_chat::AgentField::Chat);
            this.agent_chat.input.insert("make it louder");
            this.agent_send();
            assert_eq!(
                this.settings.agent.model, "gpt-oss:20b",
                "Enter applies the form it finds configured"
            );
            assert_eq!(
                this.agent_chat.chosen_model, "gpt-oss:20b",
                "the pick outlives the send"
            );
            assert!(
                !matches!(
                    this.agent_chat.entries.last(),
                    Some(crate::ui::agent_chat::ChatEntry::Note(_))
                ),
                "a complete form is not refused"
            );
        });
    }

    /// The whole gesture the third live run asked for: click the dropdown, click a model,
    /// and the setting is written then and there — no Apply between the menu and the wire.
    #[gpui::test]
    fn picking_a_model_from_the_menu_applies_it_at_once(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            // Shown, not toggled: a toggle depends on the shared layout file's mood.
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent = Default::default();
            this.agent_chat.configuring = true;
            // What the provider would have answered, so no subprocess is involved.
            this.agent_chat.models = vec![crate::ui::agent_chat::ModelOption {
                name: "qwen3.8:27b".to_string(),
                context_length: Some(262_144),
            }];
        });
        paint(&app, cx);
        click("agent-model", cx);
        paint(&app, cx);
        click("agent-model-option-0", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.settings.agent.model, "qwen3.8:27b",
                "the pick wrote itself through"
            );
            assert_eq!(
                this.agent_chat.context_window,
                Some(262_144),
                "the gauge learned the window from the listing"
            );
            assert!(
                this.agent_chat.configuring,
                "the section stays open for the rest of the form"
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
