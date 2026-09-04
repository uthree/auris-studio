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
    // `main` installs the Markdown renderer before it opens the real window. Mirror that here so
    // a test which opens the agent panel exercises the same initialized component tree.
    cx.update(gpui_component::init);
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

    /// The song sheet takes words per section — in the lyrics boxes standing beside the form,
    /// no popup anywhere — and a piece written from it arrives singing them. Return breaks a
    /// line rather than committing, because in these boxes a line is a phrase.
    #[gpui::test]
    fn the_song_sheet_takes_words_per_section_and_the_piece_sings_them(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        cx.dispatch_action(actions::ComposeSong);
        paint(&app, cx);

        app.update(cx, |this, _| {
            assert!(this.song_sheet.is_some(), "the sheet opened");
            // A tiny song, so the write below is a moment rather than a minute.
            if let Some(dials) = this.song_sheet.as_mut() {
                dials.sections.truncate(1);
                dials.sections[0].bars = 2;
                dials.form = vec![dials.sections[0].name.clone()];
            }
            this.focus_section_lyrics(0);
            assert!(this.lyrics_edit.is_some(), "the box took the keyboard");
        });
        paint(&app, cx);
        cx.simulate_input("さくら");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("さいた");
        paint(&app, cx);
        cx.simulate_keystrokes("escape");
        paint(&app, cx);

        app.update(cx, |this, _| {
            assert!(this.lyrics_edit.is_none(), "escape put the keyboard down");
            assert!(
                this.song_sheet.is_some(),
                "and the song sheet stayed up — the words are on it, not over it"
            );
            let dials = this.song_sheet.as_ref().unwrap();
            assert_eq!(
                dials.sections[0].lyrics, "さくら\nさいた",
                "every keystroke landed on the dials, the line break included"
            );

            let spec = crate::ui::compose_sheet::song_spec(dials);
            assert!(spec.to_toml().contains("さくら"), "and the file would too");
            let piece = auris_session::prelude::compose(&spec);
            let report = this.session.compose(&piece).unwrap();
            assert_eq!(report.sung, 6, "six moras reached the song");
            assert!(
                this.session
                    .project()
                    .tracks
                    .iter()
                    .any(|track| track.kind.is_singer()),
                "on a vocal track of their own"
            );
        });
    }

    /// Tab walks the lyrics boxes from section to section, and each keeps the words it was
    /// given — a verse is usually followed by writing the chorus, without touching the mouse.
    #[gpui::test]
    fn tab_walks_the_lyrics_boxes_from_verse_to_chorus(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        cx.dispatch_action(actions::ComposeSong);
        paint(&app, cx);

        app.update(cx, |this, _| {
            let dials = this.song_sheet.as_mut().expect("the song sheet is open");
            dials.sections.truncate(2);
            // The chorus twice, so the walk also proves a repeated section is one stop.
            dials.form = vec![
                dials.sections[0].name.clone(),
                dials.sections[1].name.clone(),
                dials.sections[1].name.clone(),
            ];
            this.focus_section_lyrics(0);
        });
        paint(&app, cx);
        cx.simulate_input("ひらり");
        cx.simulate_keystrokes("tab");
        paint(&app, cx);
        cx.simulate_input("はらり");
        cx.simulate_keystrokes("tab");
        paint(&app, cx);

        app.update(cx, |this, _| {
            let dials = this.song_sheet.as_ref().unwrap();
            assert_eq!(dials.sections[0].lyrics, "ひらり");
            assert_eq!(dials.sections[1].lyrics, "はらり");
            assert_eq!(
                this.lyrics_edit.as_ref().map(|edit| edit.section.as_str()),
                Some(dials.sections[0].name.as_str()),
                "two stops on the walk, however often the chorus plays: it wrapped"
            );
        });
    }

    /// Editing follows a section by name while form changes rebuild and reorder section storage.
    #[gpui::test]
    fn lyrics_edit_stays_with_its_section_when_the_form_reorders(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        cx.dispatch_action(actions::ComposeSong);
        paint(&app, cx);

        let edited = app.update(cx, |this, _| {
            let dials = this.song_sheet.as_ref().expect("the song sheet is open");
            let edited = dials.sections[1].name.clone();
            this.focus_section_lyrics(1);
            let dials = this.song_sheet.as_mut().expect("the song sheet is open");
            crate::ui::compose_sheet::add_to_form(dials, 0, "bridge");
            assert_ne!(dials.sections[1].name, edited, "the raw index moved");
            edited
        });
        paint(&app, cx);
        cx.simulate_input("ことば");

        app.read_with(cx, |this, _| {
            let dials = this.song_sheet.as_ref().unwrap();
            let section = dials
                .sections
                .iter()
                .find(|section| section.name == edited)
                .expect("the edited section remains in the form");
            assert_eq!(section.lyrics, "ことば");
            assert_eq!(
                dials.sections[1].lyrics, "",
                "the section that inherited the old index was not overwritten"
            );
        });
    }

    /// A voice clicked on the shelf with no singer track anywhere refuses with the line
    /// naming the cure, exactly as the file picker does.
    #[gpui::test]
    fn a_shelf_voice_needs_a_singer_track(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.set_track_voice(std::path::Path::new("/nowhere/voice.onnx"));
            assert!(
                this.status
                    .contains(auris_i18n::Key::ErrorNoSingerTrack.get(this.language())),
                "the status names the missing track, said: {}",
                this.status
            );
        });
    }

    /// One click on the shelf gives the singer its voice — the same interface a sound gets.
    /// Runs only where `AURIS_SINGER_TEST_MODEL` points at a real exported voice.
    #[gpui::test]
    fn a_shelf_voice_lands_on_the_singer_track(cx: &mut TestAppContext) {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            return;
        };
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let track = this.session.add_singer_track("Voice");
            this.selected_track = Some(track);
            this.set_track_voice(std::path::Path::new(&model));
            let voice = this
                .session
                .singer_voice(track)
                .expect("a singer track answers")
                .expect("the click chose a voice");
            assert!(!voice.name.is_empty(), "the voice landed with its name");
            assert!(
                this.status.contains(&voice.name),
                "and the status says so: {}",
                this.status
            );
        });
    }

    /// The whole words-first flow, made as a hand makes it: the palette's action opens the
    /// lyric field, the words are typed — Return breaking a phrase, since the field holds
    /// lines — and secondary-Return composes: a singer track carrying every mora, chords in
    /// the harmony lane, the band behind, and one Undo takes it all back.
    #[gpui::test]
    fn composing_from_lyrics_is_one_action_one_field_one_song(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.read_with(cx, |this, _| this.session.project().tracks.len());

        cx.dispatch_action(actions::ComposeFromLyrics);
        paint(&app, cx);
        assert!(
            app.read_with(cx, |this, _| this.prompt.is_some()),
            "the action opened the lyric field"
        );

        cx.simulate_input("さくら さいた");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("はるが きた");
        app.read_with(cx, |this, _| {
            let field = this
                .prompt
                .as_ref()
                .and_then(crate::ui::prompt::Prompt::field);
            assert_eq!(
                field.map(|field| field.content()),
                Some("さくら さいた\nはるが きた"),
                "Return broke the line instead of committing"
            );
        });
        cx.simulate_keystrokes("secondary-enter");
        paint(&app, cx);

        app.read_with(cx, |this, _| {
            let project = this.session.project();
            // The vocal and the standard band: three parts behind the singer.
            assert_eq!(project.tracks.len(), before + 4);
            let singer = project
                .tracks
                .iter()
                .find(|track| track.kind.is_singer())
                .expect("a singer track was written");
            let clips = &singer.kind.as_singer().unwrap().clips;
            assert_eq!(clips.len(), 1);
            assert_eq!(
                clips[0].notes.len(),
                11,
                "six moras and five, one note each"
            );
            assert_eq!(clips[0].notes[0].lyric, "さ");
            assert!(
                project.harmony.numeral_at(Ticks::ZERO).is_some(),
                "the chords are on the lane, where they can be argued with"
            );
            assert!(
                this.status.contains("11"),
                "the status counts what was written: {}",
                this.status
            );
        });

        app.update(cx, |this, _| {
            this.session.undo();
            assert_eq!(this.session.project().tracks.len(), before, "one step back");
        });
    }

    /// The speaker row refuses the same way singing does on a track with no voice, from
    /// the menu row to the status bar, with no model file involved.
    #[gpui::test]
    fn the_next_speaker_needs_a_voice_first(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            this.session.add_singer_track("Voice");
            cx.notify();
        });
        cx.run_until_parked();
        cx.dispatch_action(actions::NextSingerSpeaker);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(
                this.status
                    .contains(auris_i18n::Key::ErrorNoVoice.get(this.language())),
                "the status must say to choose a voice, said: {}",
                this.status
            );
        });
    }

    /// Sing without a voice refuses with the line naming the cure, and costs no undo step —
    /// the whole refusal path from the menu row to the status bar, no model file involved.
    #[gpui::test]
    fn singing_without_a_voice_names_the_cure_and_records_nothing(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let singer = app.update(cx, |this, cx| {
            let track = this.session.add_singer_track("Voice");
            let clip = this
                .session
                .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
                .expect("a singer track takes a note clip");
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            cx.notify();
            track
        });
        cx.run_until_parked();

        cx.dispatch_action(actions::Sing);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(
                this.status
                    .contains(auris_i18n::Key::ErrorNoVoice.get(this.language())),
                "the status must say to choose a voice, said: {}",
                this.status
            );
            assert!(this.export.is_none(), "no render was started");
            // And the badge machinery answers Absent without a model anywhere near.
            assert_eq!(
                this.session.singer_take_state(singer).unwrap(),
                auris_session::SingerTakeState::Absent
            );
        });
    }

    /// The auto-render poll walks past a voiceless singer track without a word: no render,
    /// no complaint — the preview instrument is that track's whole sound until a voice is
    /// chosen, and an excuse per edit would be noise about a non-problem.
    #[gpui::test]
    fn a_voiceless_singer_track_is_not_auto_rendered(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            let track = this.session.add_singer_track("Voice");
            let clip = this
                .session
                .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
                .unwrap();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            // The poll waits out the debounce on real time; the test hands it a revision
            // that has already sat still long enough.
            this.auto_sing_seen = (
                this.session.revision(),
                std::time::Instant::now() - crate::ui::commands::AUTO_SING_DEBOUNCE,
            );
            this.poll_auto_sing(cx);
            assert!(this.auto_sing.is_none(), "no voice, no render");
            assert!(
                !this.status_failed,
                "and no complaint either, said: {}",
                this.status
            );
        });
    }

    /// With a real voice on the track, an edited score sings itself again: the debounce
    /// elapses, the poll starts a background render with no overlay, and the landed take
    /// matches the notes. Skips silently without `AURIS_SINGER_TEST_MODEL`.
    #[gpui::test]
    fn an_edited_score_sings_itself_again(cx: &mut TestAppContext) {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the auto-sing test");
            return;
        };
        let (app, cx) = open(cx);
        let folder = std::env::temp_dir().join(format!("auris-auto-sing-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();

        let track = app.update(cx, |this, cx| {
            let track = this.session.add_singer_track("Voice");
            let clip = this
                .session
                .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
                .unwrap();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.session.write_lyrics(clip, &[0], "ら").unwrap();
            // A take lands in the project folder, so the project needs one.
            this.session.save(&folder.join("Song.auris")).unwrap();
            this.session
                .set_singer_voice(track, Some(std::path::Path::new(&model)))
                .unwrap();
            this.auto_sing_seen = (
                this.session.revision(),
                std::time::Instant::now() - crate::ui::commands::AUTO_SING_DEBOUNCE,
            );
            this.poll_auto_sing(cx);
            assert!(
                this.auto_sing.is_some(),
                "the stale take starts rendering unasked, status: {}",
                this.status
            );
            assert!(this.export.is_none(), "and no overlay stands over it");
            track
        });
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(this.auto_sing.is_none(), "the render finished");
            assert_eq!(
                this.session.singer_take_state(track).unwrap(),
                auris_session::SingerTakeState::Current,
                "the take matches the score, status: {}",
                this.status
            );
        });
        std::fs::remove_dir_all(&folder).ok();
    }

    /// A singer track with no voice auditions through the formant instrument, exactly as
    /// before: the sung-preview path never files a wish for it.
    #[gpui::test]
    fn a_voiceless_singer_track_auditions_through_the_instrument(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            let track = this.session.add_singer_track("Voice");
            this.selected_track = Some(track);
            this.audition(60);
            assert!(this.sung_preview_wish.is_none(), "no voice, no wish");
            assert!(
                this.auditioning.is_some(),
                "the note still sounds somewhere"
            );
            this.stop_audition();
            cx.notify();
        });
    }

    /// With a real voice, a grabbed note is sung by the model: the audition files a wish,
    /// the poll renders it in the background, and the render lands in the cache so the next
    /// pass over the same pitch is instant. Skips without `AURIS_SINGER_TEST_MODEL`.
    #[gpui::test]
    fn a_dragged_note_previews_in_the_real_voice(cx: &mut TestAppContext) {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the sung-preview test");
            return;
        };
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            let track = this.session.add_singer_track("Voice");
            let clip = this
                .session
                .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
                .unwrap();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.session.write_lyrics(clip, &[0], "か").unwrap();
            this.session
                .set_singer_voice(track, Some(std::path::Path::new(&model)))
                .unwrap();
            this.selected_track = Some(track);
            this.selected_clip = Some(clip);
            this.selected_notes.insert(0);

            this.audition(60);
            assert!(
                this.sung_preview_wish.is_some(),
                "a voiced note is wished for, not struck on the formant"
            );
            this.poll_sung_preview(cx);
        });
        cx.run_until_parked();
        app.update(cx, |this, cx| {
            assert_eq!(
                this.sung_previews.len(),
                1,
                "the render landed in the cache"
            );
            assert!(
                this.sung_preview_wish.is_none(),
                "the wish was played and put down"
            );
            // The same pitch again plays straight from the cache, no new wish filed.
            this.stop_audition();
            this.audition(60);
            assert!(
                this.sung_preview_wish.is_none(),
                "a cache hit files no wish"
            );
            this.stop_audition();
            cx.notify();
        });
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

    /// A reload offered for one document must disappear when another document replaces it.
    #[gpui::test]
    fn an_agent_reload_offer_does_not_follow_a_project_switch(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let root =
            std::env::temp_dir().join(format!("auris-harness-agent-reload-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let (first, second) = app.update(cx, |this, _| {
            this.session.save_as(&root.join("First.auris")).unwrap();
            let first = this.session.path().unwrap().to_path_buf();
            this.session.save_as(&root.join("Second.auris")).unwrap();
            let second = this.session.path().unwrap().to_path_buf();
            this.session.open(&first).unwrap();
            this.agent_chat.pending_reload = Some(first.clone());
            (first, second)
        });

        app.update(cx, |this, cx| this.open_project_at(second.clone(), cx));
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert_eq!(this.session.path(), Some(second.as_path()));
            assert_eq!(this.agent_chat.pending_reload, None);
        });

        // The project directories are disposable fixtures. Windows cannot remove them until
        // every handle from the asynchronous open has returned, which run_until_parked ensures.
        drop(first);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Another writer at the open file, seen from the window: a clean window follows the
    /// disk silently, a dirty one gets a standing offer and autosave holds its fire, and a
    /// save of its own takes the file back and the offer down.
    #[gpui::test]
    fn an_external_write_reloads_a_clean_window_and_offers_a_dirty_one(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let root = std::env::temp_dir().join(format!("auris-harness-watch-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        app.update(cx, |this, _| {
            this.session.save_as(&root.join("Watched.auris")).unwrap();
        });
        // The other writer, played by a bumped modification time — set forward explicitly,
        // because a fast filesystem gives two writes in one timestamp.
        let bump = |app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext, s: u64| {
            app.update(cx, |this, _| {
                let path = this.session.path().unwrap().to_path_buf();
                let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
                file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(s))
                    .unwrap();
            });
        };

        // Clean: followed without a word, and no offer goes up.
        bump(&app, cx, 2);
        app.update(cx, |this, cx| {
            assert!(this.session.externally_modified());
            this.watch_disk(cx);
            assert!(
                this.external_change.is_none(),
                "no offer for a clean window"
            );
        });
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(
                !this.session.externally_modified(),
                "the reload took the disk's version"
            );
        });

        // A sheet opened over the old document does not survive the swap: its target ids were
        // minted by a document that is gone, and the next one numbers from one as well —
        // committed after the reload, the sheet would rename whatever holds that number now.
        app.update(cx, |this, _| {
            let track = match this.project().tracks.first() {
                Some(track) => track.id,
                None => this.session.add_default_instrument_track("Named").unwrap(),
            };
            this.session.save_in_place().unwrap();
            this.prompt_to_rename_track(track);
            this.last_disk_watch = None;
        });
        bump(&app, cx, 3);
        app.update(cx, |this, cx| this.watch_disk(cx));
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(
                this.prompt.is_none(),
                "the rename sheet came down with the document it named"
            );
        });

        // Dirty: the choice goes on screen, and the autosave policy sees the other writer.
        app.update(cx, |this, _| {
            this.session.add_default_instrument_track("Mine").unwrap();
            this.last_disk_watch = None;
        });
        bump(&app, cx, 4);
        app.update(cx, |this, cx| {
            this.watch_disk(cx);
            assert!(this.external_change.is_some(), "the offer stands");
            assert!(this.status_failed, "said in a warning's colour");
            assert!(
                this.session.autosave_state().overwritten,
                "autosave knows not to write over it"
            );
        });
        paint(&app, cx);
        assert!(
            cx.debug_bounds("external-reload").is_some(),
            "the Reload button is drawn in the status bar"
        );

        // A save of our own is the deliberate act that takes the file back.
        app.update(cx, |this, cx| {
            this.session.save_in_place().unwrap();
            this.last_disk_watch = None;
            this.watch_disk(cx);
            assert!(this.external_change.is_none(), "the offer comes down");
        });
        std::fs::remove_dir_all(&root).unwrap();
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
