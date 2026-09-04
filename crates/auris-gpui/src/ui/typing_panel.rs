//! The keyboard the typing mode draws, as a floating panel.
//!
//! Musical Typing turns the alphabet into a piano, and until this panel existed the only thing on
//! screen that said so was a line in the status bar. What the hands are doing — which octave they
//! are in, how hard they are striking, where the modulation wheel was left, which letter is which
//! note — was knowledge a user had to carry. Logic and GarageBand draw it, and this is the same
//! picture: the number row above, the two rows of letters laid out as a keyboard, and the octave
//! and velocity keys below, with every control lit while it is in force.
//!
//! # Why it is a panel and not a second window
//!
//! For the reason [`crate::ui::plugin_window`] gives, and one more that is worse. A second
//! operating-system window takes the platform's key events with it, so the keyboard would stop
//! working the moment somebody clicked on the picture of it — which is the one thing a panel about
//! playing must not do. Drawn inside the main window, the keys keep arriving where they always
//! did, through `AurisApp::typing_key`, and nothing about focus changes at all.
//!
//! It floats, and is dragged by its title bar, because it is a *reference* rather than a place to
//! work: it wants to sit next to whichever track is being played into, which is somewhere
//! different every time.
//!
//! # Nothing here is state
//!
//! Every value drawn is read back out of `MusicalTyping` on each frame. The only thing the panel
//! owns is where it was dragged to, and which key the *mouse* is holding — a pointer pressed on
//! one key and let go over another still has to release the first.

use auris_i18n::{Key, Language};
use auris_session::{LAYOUT, TypingRole};
use gpui::{
    AnyElement, Context, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, SharedString, Size, div, point, prelude::*, px, relative, size,
};

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::icons::Icon;
use crate::ui::widgets::chain_button;

/// Width of one white key, and so the unit every other width here is in.
const KEY_WIDTH: Pixels = px(34.0);
/// Height of a white key.
const WHITE_HEIGHT: Pixels = px(94.0);
/// Width of a black key, narrow enough that two of them fit either side of a seam.
const BLACK_WIDTH: Pixels = px(22.0);
/// Height of a black key. Short, as on a real keyboard.
const BLACK_HEIGHT: Pixels = px(58.0);
/// Side of one of the number-row and octave keys.
const CONTROL_WIDTH: Pixels = px(34.0);
/// Height of one of them.
const CONTROL_HEIGHT: Pixels = px(38.0);
/// Width of the sustain key, which is Tab and so is drawn as wide as Tab is.
const SUSTAIN_WIDTH: Pixels = px(70.0);

/// One key of the drawn keyboard.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KeyCap {
    /// The letter on it, spelled as [`LAYOUT`] spells it.
    pub key: &'static str,
    /// Semitones above the C the keyboard is centred on.
    pub semitones: i32,
    /// Whether it is one of the short keys that sit on the seams.
    pub black: bool,
    /// Where it sits, in white-key widths from the left.
    ///
    /// A white key's *left* edge and a black key's *centre*, which is what makes both a single
    /// number: a black key belongs on the seam between the two whites it sits between, and that
    /// seam is exactly where the white above it starts.
    pub slot: usize,
}

/// The note keys of [`LAYOUT`], placed the way a piano places them.
///
/// Derived from the layout rather than written out again, for the reason the layout itself gives:
/// a drawn keyboard that had its own copy of which letter is which note would be a second table
/// to keep in step, and the day the two disagreed the panel would be lighting the wrong key while
/// the right one sounded.
///
/// Which keys are black is decided by pitch class, from the same test the piano roll draws its
/// key strip with — so a `w` is drawn black here for the same reason its row is shaded there.
pub fn key_caps() -> Vec<KeyCap> {
    let mut caps: Vec<KeyCap> = LAYOUT
        .iter()
        .filter_map(|&(key, role)| match role {
            TypingRole::Note(semitones) => Some(KeyCap {
                key,
                semitones,
                black: crate::ui::timeline::is_black_key(semitones.rem_euclid(12) as u8),
                slot: 0,
            }),
            _ => None,
        })
        .collect();
    caps.sort_by_key(|cap| cap.semitones);

    // One pass upwards, counting the whites: a white key takes the next slot, and a black key
    // takes the slot of the white that will follow it — which is the seam it sits on.
    let mut whites = 0;
    for cap in &mut caps {
        cap.slot = whites;
        if !cap.black {
            whites += 1;
        }
    }
    caps
}

/// How many white keys the drawn keyboard is wide.
pub fn white_key_count() -> usize {
    key_caps().iter().filter(|cap| !cap.black).count()
}

/// The keys of the number row, in the order they sit on the keyboard.
///
/// The two bend keys and then the six the modulation wheel is spread across, which is the order
/// [`LAYOUT`] holds them in and the order a synthesiser puts the two wheels in.
pub fn control_caps() -> Vec<(&'static str, TypingRole)> {
    LAYOUT
        .iter()
        .copied()
        .filter(|(_, role)| {
            matches!(
                role,
                TypingRole::BendDown | TypingRole::BendUp | TypingRole::Wheel(_)
            )
        })
        .collect()
}

/// What is written above the letter on a number-row key.
///
/// Only the ends of the modulation row are named. The four keys between `3` and `8` are steps of
/// one wheel and a number on each would read as six controls rather than one — the row is a
/// slider, and a slider's ends are the only part of it that needs words.
pub fn control_label(role: TypingRole, language: Language) -> &'static str {
    match role {
        TypingRole::BendDown => "−",
        TypingRole::BendUp => "＋",
        TypingRole::Wheel(0) => Key::ValueOff.get(language),
        TypingRole::Wheel(step) if step + 1 == auris_session::WHEEL_STEPS => {
            Key::TypingWheelMax.get(language)
        }
        _ => "",
    }
}

/// What is drawn on a key's face.
///
/// Uppercase, because that is what is printed on the key itself — a keyboard drawn in lower case
/// is a keyboard that does not look like the one under the hands. `tab` is the exception and
/// keeps its name, which is also what is written on it.
pub fn key_face(key: &str) -> String {
    match key.len() > 1 {
        true => key.to_string(),
        false => key.to_uppercase(),
    }
}

/// How one key is drawn.
///
/// The three that differ between the rows, together: the number row and the black keys and the
/// white keys are the same element in three sizes, and passing them one at a time made a function
/// whose arguments were mostly measurements.
#[derive(Copy, Clone, Debug)]
struct KeyLook {
    /// The face colour, when the key is not lit.
    face: Hsla,
    width: Pixels,
    height: Pixels,
}

impl KeyLook {
    /// One of the small keys above and below the keyboard.
    fn control(theme: &Theme) -> Self {
        Self {
            face: theme.surface_raised,
            width: CONTROL_WIDTH,
            height: CONTROL_HEIGHT,
        }
    }
}

/// The drawn keyboard, while the typing mode is on.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct TypingPanel {
    /// Top-left corner in window coordinates, once it has been dragged somewhere.
    ///
    /// `None` until then, rather than a position worked out when the mode went on: the window can
    /// be resized while the panel is up, and a keyboard placed once against a viewport that no
    /// longer exists would drift off the bottom of a shortened window.
    pub anchor: Option<Point<Pixels>>,
}

impl TypingPanel {
    /// Roughly how large the panel comes out.
    ///
    /// An *estimate*, and only ever used to decide where it will fit: the panel sizes itself to
    /// its contents, whose width depends on how long the word for "modulation" is in the language
    /// the interface is in. Being a few pixels out moves the keyboard a few pixels, which is the
    /// same bargain [`crate::ui::plugin_window::PluginWindow::height`] makes.
    pub fn frame() -> Size<Pixels> {
        let keys = SUSTAIN_WIDTH + px(8.0) + KEY_WIDTH * white_key_count() as f32;
        size(
            keys + px(150.0),
            Metrics::PANEL_HEADER_HEIGHT + WHITE_HEIGHT + CONTROL_HEIGHT * 2.0 + px(46.0),
        )
    }

    /// Where the panel is drawn, given the viewport it has to fit in.
    ///
    /// Clamped rather than flipped, for the reason the plugin editor is: the title bar is the
    /// thing a hand is reaching for, and moving it out from under that hand is worse than a panel
    /// that overhangs. One larger than the viewport pins to the top-left, which is the corner the
    /// title bar is in.
    pub fn origin(&self, viewport: Size<Pixels>) -> Point<Pixels> {
        let frame = Self::frame();
        let wanted = self
            .anchor
            .unwrap_or_else(|| default_anchor(viewport, frame));
        let clamp = |value: Pixels, room: Pixels| value.min(room.max(px(0.0))).max(px(0.0));
        point(
            clamp(wanted.x, viewport.width - frame.width),
            clamp(wanted.y, viewport.height - frame.height),
        )
    }
}

/// Where a keyboard nobody has moved sits.
///
/// Centred across the window and along the bottom of it, clear of the status bar. That is where
/// the hands are: the panel is looked at while the *arrangement* is being played into, so it
/// belongs at the edge the eye is not working in, and the bottom edge is the one no dock takes
/// the whole of.
fn default_anchor(viewport: Size<Pixels>, frame: Size<Pixels>) -> Point<Pixels> {
    point(
        (viewport.width - frame.width) / 2.0,
        viewport.height - frame.height - Metrics::STATUS_HEIGHT - px(12.0),
    )
}

impl AurisApp {
    /// Draws the keyboard, when the typing mode is on.
    ///
    /// Asked for the mode rather than for a field of its own, because the two are one thing: the
    /// panel is what the mode *looks* like, and a mode that could be on with nothing on screen to
    /// say so is the state this panel exists to abolish.
    pub(crate) fn render_typing_panel(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.session.musical_typing() {
            return None;
        }
        let theme = self.theme.clone();
        let language = self.language();
        let origin = self.typing_panel.origin(viewport);

        let keys = self.session.typing_keyboard();
        let root = keys.root();
        let bend = keys.bend();
        let wheel_step = keys.wheel_step();
        let sustain = keys.sustain();
        let octave_name = keys.octave_name();
        let velocity = (keys.velocity() * 127.0).round() as u32;
        let sounding: Vec<u8> = keys.sounding().collect();

        // Which instrument the keys are reaching, in the title bar: it is the answer to "what am
        // I about to hear", and it changes with the selection while the panel is up.
        let track = self
            .session
            .audition_track(self.selected_track)
            .and_then(|id| self.session.project().track(id))
            .map(|track| track.name.clone())
            .unwrap_or_else(|| self.t(Key::TypingNoTrack).to_string());

        let header = self.typing_header(&track, origin, &theme, cx);
        let wheels = self.typing_wheels(bend, wheel_step, language, &theme, cx);
        let board = self.typing_board(root, &sounding, sustain, language, &theme, cx);
        let steppers = self.typing_steppers(&octave_name, velocity, language, &theme, cx);

        Some(
            div()
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .flex()
                .flex_col()
                .rounded(Metrics::RADIUS_LG)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                // The same pair the plugin editor needs, and for the same two reasons: a floating
                // panel that let the pointer through would press whatever was behind it as well,
                // and one that occludes has to carry a drag begun inside it, because the hit test
                // stops dead at the first blocking hitbox and the root never sees another move.
                .occlude()
                .on_mouse_move(cx.listener(AurisApp::on_mouse_move))
                .on_mouse_up(gpui::MouseButton::Left, cx.listener(AurisApp::on_mouse_up))
                .child(header)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .p_3()
                        .child(range_bar(root, &theme))
                        .child(wheels)
                        .child(board)
                        .child(steppers),
                )
                .into_any_element(),
        )
    }

    /// The title bar, which is also the handle the panel is dragged by.
    fn typing_header(
        &self,
        track: &str,
        origin: Point<Pixels>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .h(Metrics::PANEL_HEADER_HEIGHT)
            .px_1p5()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.begin_drag(Drag::MoveTypingPanel {
                        grab_offset: point(
                            event.position.x - origin.x,
                            event.position.y - origin.y,
                        ),
                    });
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!("{} — {track}", self.t(Key::CmdMusicalTyping))),
            )
            // Shutting the keyboard is how the mode is switched off, the way it is in the keyboard
            // this one is modelled on. A panel that could be closed while the alphabet went on
            // playing notes would be the worst of the arrangements available.
            .child(chain_button(
                "tk-close",
                Icon::Cross,
                theme,
                cx.listener(|this, _, _, cx| {
                    this.stop_musical_typing();
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    /// The number row: the bend keys and the modulation wheel, with their two readouts.
    fn typing_wheels(
        &self,
        bend: f32,
        wheel_step: Option<u8>,
        language: Language,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(readout(
                Key::TypingPitch.get(language),
                &format!("{bend:+.0}"),
                theme,
            ))
            .children(control_caps().into_iter().map(|(key, role)| {
                let lit = match role {
                    TypingRole::BendDown => bend < 0.0,
                    TypingRole::BendUp => bend > 0.0,
                    TypingRole::Wheel(step) => wheel_step == Some(step),
                    _ => false,
                };
                self.typing_key_element(
                    key,
                    control_label(role, language),
                    lit,
                    KeyLook::control(theme),
                    theme,
                    cx,
                )
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(Key::TypingModulation.get(language)),
            )
            .into_any_element()
    }

    /// The two rows of letters, drawn as a piano, with the sustain key beside them.
    fn typing_board(
        &self,
        root: i32,
        sounding: &[u8],
        sustain: bool,
        language: Language,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The whites first and the blacks after, so the short keys are painted over the long ones
        // they overlap rather than under them.
        let (blacks, whites): (Vec<KeyCap>, Vec<KeyCap>) =
            key_caps().into_iter().partition(|cap| cap.black);

        let key = |cap: KeyCap, this: &Self, cx: &mut Context<Self>| {
            let pitch = (root + cap.semitones).clamp(0, 127) as u8;
            let lit = sounding.contains(&pitch);
            // A white key's left edge is its slot; a black key straddles the seam its slot names.
            let (look, left) = match cap.black {
                true => (
                    KeyLook {
                        face: theme.key_black,
                        width: BLACK_WIDTH,
                        height: BLACK_HEIGHT,
                    },
                    KEY_WIDTH * cap.slot as f32 - BLACK_WIDTH / 2.0,
                ),
                false => (
                    KeyLook {
                        face: theme.key_white,
                        width: KEY_WIDTH,
                        height: WHITE_HEIGHT,
                    },
                    KEY_WIDTH * cap.slot as f32,
                ),
            };
            div()
                .absolute()
                .left(left)
                .top(px(0.0))
                .child(this.typing_key_element(cap.key, "", lit, look, theme, cx))
        };

        div()
            .flex()
            .items_start()
            .gap_2()
            .child(self.typing_key_element(
                "tab",
                Key::TypingSustain.get(language),
                sustain,
                KeyLook {
                    face: theme.surface_raised,
                    width: SUSTAIN_WIDTH,
                    height: WHITE_HEIGHT,
                },
                theme,
                cx,
            ))
            .child(
                div()
                    .relative()
                    .w(KEY_WIDTH * white_key_count() as f32)
                    .h(WHITE_HEIGHT)
                    .children(
                        whites
                            .into_iter()
                            .map(|cap| key(cap, self, cx))
                            .collect::<Vec<_>>(),
                    )
                    .children(
                        blacks
                            .into_iter()
                            .map(|cap| key(cap, self, cx))
                            .collect::<Vec<_>>(),
                    ),
            )
            .into_any_element()
    }

    /// The octave and velocity keys, with the readouts they move.
    fn typing_steppers(
        &self,
        octave_name: &str,
        velocity: u32,
        language: Language,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Neither of these latches: they step something and spring back, so nothing is lit.
        let stepper = |key: &'static str, caption: &'static str, cx: &mut Context<Self>| {
            self.typing_key_element(key, caption, false, KeyLook::control(theme), theme, cx)
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(readout(Key::TypingOctave.get(language), octave_name, theme))
            .child(stepper("z", "−", cx))
            .child(stepper("x", "＋", cx))
            .child(div().w(px(12.0)))
            .child(readout(
                Key::TypingVelocity.get(language),
                &velocity.to_string(),
                theme,
            ))
            .child(stepper("c", "−", cx))
            .child(stepper("v", "＋", cx))
            .into_any_element()
    }

    /// One key of the keyboard, whichever row it is in.
    ///
    /// Pressing it plays it, through the same commands the computer keyboard's own keys go
    /// through — the panel is a picture of the keyboard, and a picture of a keyboard that could
    /// not be played would be asking to be clicked on and doing nothing about it.
    fn typing_key_element(
        &self,
        key: &'static str,
        caption: &str,
        lit: bool,
        look: KeyLook,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let KeyLook {
            face,
            width,
            height,
        } = look;
        let background = if lit { theme.accent } else { face };
        let text = theme.text_on(background);
        let hover = theme.surface_hover;
        div()
            .id(SharedString::from(format!("tk:{key}")))
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(px(2.0))
            .w(width)
            .h(height)
            .pb(px(4.0))
            .rounded(Metrics::RADIUS_SM)
            .bg(background)
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .when(!lit, |this| this.hover(|this| this.bg(hover)))
            // The caption above the letter, the way it is printed above the number on a
            // synthesiser's own wheels. An empty one still takes its line, so the letters along
            // the row stay level with each other.
            .child(
                div()
                    .h(px(13.0))
                    .text_xs()
                    .text_color(text)
                    .child(caption.to_string()),
            )
            .child(div().text_xs().text_color(text).child(key_face(key)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.press_typed_key(key);
                    cx.notify();
                }),
            )
            // A pointer held down and slid along the keys plays them, which is what a picture of a
            // keyboard invites somebody to try. Registered on the key rather than on the panel
            // because a bubble-phase move only reaches a hitbox the pointer is over, so the key
            // that answers is the key under the hand and no arithmetic decides which.
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if slides_onto_key(
                    event.pressed_button,
                    this.dragging(),
                    this.clicked_key == Some(key),
                ) {
                    this.press_typed_key(key);
                    cx.notify();
                }
            }))
    }

    /// Plays one key of the drawn keyboard, and remembers it for the release.
    ///
    /// The key is remembered rather than worked out again when the button comes up, because a
    /// press that began on one key and a release that arrives over another — or outside the panel
    /// altogether — are the same gesture: the release has to let go of what was pressed.
    ///
    /// Whatever the pointer was holding is let go of first. One pointer holds one key, and a
    /// slide that took the next one without putting the last one down would leave a note sounding
    /// with nothing left that knows about it — which is the whole of the bug this guards.
    fn press_typed_key(&mut self, key: &'static str) {
        self.release_typed_key();
        let Some(track) = self.session.audition_track(self.selected_track) else {
            return;
        };
        self.clicked_key = Some(key);
        self.session.typing_press(track, key);
    }

    /// Lets go of whatever the pointer was holding on the drawn keyboard.
    ///
    /// Called from the root's mouse-up, which is where every gesture in the window ends — and
    /// from its mouse-*move*, for the releases that never arrive at all. Letting go over another
    /// application, or off the edge of the screen, is a mouse-up the platform hands to somebody
    /// else, and a note that waited for it would sound until the mode was switched off.
    pub(crate) fn release_typed_key(&mut self) {
        if let Some(key) = self.clicked_key.take() {
            self.session.typing_release(key);
        }
    }

    /// Switches the typing mode off, letting go of everything it was holding.
    ///
    /// The one place it goes off, so that the panel's close button and the command cannot come to
    /// mean different things.
    pub(crate) fn stop_musical_typing(&mut self) {
        self.release_typed_key();
        self.session.set_musical_typing(false);
        self.set_status(self.t(Key::MusicalTypingOff));
    }
}

/// Whether the pointer sliding onto a key should sound it.
///
/// Three conditions, and each one is a bug that was available without it:
///
/// * **The button has to be down.** A pointer merely crossing the keyboard on its way somewhere
///   else would otherwise play every note it passed over.
/// * **Nothing may be being dragged.** The panel is dragged by its title bar, and the pointer
///   crosses the keys on the way to wherever it is being put — so a keyboard that played on any
///   move would perform a glissando every time it was moved out of the way. It is also what stops
///   a fader dragged past the panel from sounding it.
/// * **The key must not be the one already down.** A move arrives per pixel, and the same key
///   struck again on each of them is a drum roll where a held note was meant. This is
///   [`crate::app::audition_for`]'s `Hold` in miniature, and the same mistake.
pub fn slides_onto_key(
    button: Option<MouseButton>,
    dragging: bool,
    already_holding_it: bool,
) -> bool {
    button == Some(MouseButton::Left) && !dragging && !already_holding_it
}

/// A label and the value it names, for the four things the keys move blind.
fn readout(label: &str, value: &str, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .flex()
        .items_baseline()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.accent)
                .child(value.to_string()),
        )
}

/// Where the drawn keyboard sits on the whole of MIDI, as a bar.
///
/// The octave keys move seventeen semitones of a hundred and twenty-eight, and a name like `C3`
/// only says where that is to somebody who already thinks in octave numbers. This says it to
/// everybody, and it is the one part of the panel that shows what is *not* under the hands.
fn range_bar(root: i32, theme: &Theme) -> impl IntoElement + use<> {
    let span = key_caps().last().map_or(0, |cap| cap.semitones) as f32 + 1.0;
    let low = (root as f32 / 128.0).clamp(0.0, 1.0);
    let width = (span / 128.0).min(1.0 - low);
    div()
        .relative()
        .h(px(6.0))
        .w_full()
        .rounded(Metrics::RADIUS_SM)
        .bg(theme.surface_sunken)
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(relative(low))
                .w(relative(width))
                .h_full()
                .rounded(Metrics::RADIUS_SM)
                .bg(theme.accent),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_note_key_of_the_layout_is_drawn_exactly_once() {
        let caps = key_caps();
        let played = LAYOUT
            .iter()
            .filter(|(_, role)| matches!(role, TypingRole::Note(_)))
            .count();
        assert_eq!(caps.len(), played, "a key of the layout is not on screen");

        let mut keys: Vec<&str> = caps.iter().map(|cap| cap.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), caps.len(), "a key is drawn twice");
    }

    #[test]
    fn the_white_keys_run_along_the_row_and_each_black_one_sits_on_a_seam() {
        // The whole point of the placement: `w` has to be between `a` and `s`, or the drawing
        // says the letters are in an order the keyboard does not play them in.
        let caps = key_caps();
        let mut expected = 0;
        for (index, cap) in caps.iter().enumerate() {
            if cap.black {
                let below = caps[index - 1];
                let above = caps[index + 1];
                assert!(!below.black && !above.black, "two black keys in a row");
                assert_eq!(
                    cap.slot,
                    below.slot + 1,
                    "`{}` is not on the seam above `{}`",
                    cap.key,
                    below.key
                );
                assert_eq!(
                    cap.slot, above.slot,
                    "and not on the one below `{}`",
                    above.key
                );
            } else {
                assert_eq!(cap.slot, expected, "`{}` is out of order", cap.key);
                expected += 1;
            }
        }
        assert_eq!(white_key_count(), expected);
    }

    #[test]
    fn the_keys_are_drawn_in_pitch_order_and_the_black_ones_are_the_accidentals() {
        let caps = key_caps();
        assert!(
            caps.windows(2)
                .all(|pair| pair[0].semitones < pair[1].semitones),
            "the keyboard is not laid out low to high"
        );
        for cap in &caps {
            let accidental = matches!(cap.semitones.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
            assert_eq!(
                cap.black, accidental,
                "`{}` is drawn on the wrong kind of key",
                cap.key
            );
        }
        // Above the octave as well, which is where a keyboard that folded the pitch class the
        // wrong way would give itself away: `o` is 13 semitones up and still a black key.
        let high = caps.iter().find(|cap| cap.key == "o").expect("`o` plays");
        assert!(high.black && high.semitones > 12);
    }

    #[test]
    fn the_number_row_names_only_the_ends_of_the_wheel() {
        let controls = control_caps();
        assert_eq!(controls.len(), 2 + auris_session::WHEEL_STEPS as usize);
        assert_eq!(controls.first().map(|&(key, _)| key), Some("1"));

        for language in Language::ALL {
            let named: Vec<&str> = controls
                .iter()
                .filter(|(_, role)| matches!(role, TypingRole::Wheel(_)))
                .map(|&(_, role)| control_label(role, language))
                .collect();
            let words = named.iter().filter(|label| !label.is_empty()).count();
            assert_eq!(
                words, 2,
                "the wheel is labelled as {words} controls, not one"
            );
            assert!(!named[0].is_empty(), "the key that turns it off is unnamed");
            assert!(
                !named[named.len() - 1].is_empty(),
                "and so is the top of it"
            );
        }
    }

    #[test]
    fn a_key_is_drawn_the_way_it_is_printed_on_the_keyboard() {
        assert_eq!(key_face("a"), "A");
        assert_eq!(key_face(";"), ";");
        assert_eq!(key_face("tab"), "tab", "a named key keeps its name");
    }

    #[test]
    fn a_pointer_slid_along_the_keys_plays_them_once_each_and_only_while_it_is_pressing() {
        let left = Some(MouseButton::Left);
        assert!(
            slides_onto_key(left, false, false),
            "a pressed pointer arriving on a key sounds it"
        );
        assert!(
            !slides_onto_key(None, false, false),
            "a pointer merely crossing the keyboard played every note under it"
        );
        assert!(
            !slides_onto_key(left, true, false),
            "moving the panel by its title bar performed a glissando on the way"
        );
        assert!(
            !slides_onto_key(left, false, true),
            "the key already down was struck again on every pixel of the move"
        );
        // The right button is not a way to play, whatever it is doing.
        assert!(!slides_onto_key(Some(MouseButton::Right), false, false));
    }

    #[test]
    fn a_keyboard_nobody_has_moved_sits_along_the_bottom_of_the_window() {
        let viewport = size(px(1400.0), px(900.0));
        let frame = TypingPanel::frame();
        let origin = TypingPanel::default().origin(viewport);

        assert!(
            origin.x > px(0.0) && origin.x + frame.width < viewport.width,
            "not centred across the window"
        );
        assert!(
            origin.y + frame.height <= viewport.height - Metrics::STATUS_HEIGHT,
            "the status bar is drawn over the bottom of the keyboard"
        );
        assert!(
            origin.y > viewport.height / 2.0,
            "it belongs at the edge the eye is not working in"
        );
    }

    #[test]
    fn a_keyboard_dragged_off_the_edge_is_pushed_back_inside() {
        let viewport = size(px(1000.0), px(700.0));
        let frame = TypingPanel::frame();
        let panel = TypingPanel {
            anchor: Some(point(px(980.0), px(690.0))),
        };
        let origin = panel.origin(viewport);
        assert!(origin.x + frame.width <= viewport.width);
        assert!(origin.y + frame.height <= viewport.height);

        // One that already fits is left exactly where it was put down.
        let placed = TypingPanel {
            anchor: Some(point(px(120.0), px(90.0))),
        };
        assert_eq!(placed.origin(viewport), point(px(120.0), px(90.0)));
    }

    #[test]
    fn a_window_too_small_for_the_keyboard_keeps_the_title_bar_on_screen() {
        // The corner the panel is dragged by. Pinning to any other one would put the handle off
        // the edge, and there would be no way to get the keyboard back.
        let tiny = size(px(200.0), px(150.0));
        for anchor in [None, Some(point(px(400.0), px(400.0)))] {
            assert_eq!(
                TypingPanel { anchor }.origin(tiny),
                point(px(0.0), px(0.0)),
                "the handle went off the edge"
            );
        }
    }
}
