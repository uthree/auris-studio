//! What a click does, when the click is not simply a click.
//!
//! Creating and deleting are the two gestures every DAW binds differently — Logic creates on
//! ⌘-click and deletes on a double-click, others use ⌥ or a dedicated tool — so they are
//! configurable rather than baked in, the same way a keystroke is.
//!
//! The names are Logic's, and so is the vocabulary a DAW user arrives with, but the *keys* are
//! whatever the platform calls them: ⌘ on macOS is Ctrl on Windows and Linux. Everything here
//! goes through [`gpui::Modifiers::secondary`] rather than naming a key directly, because
//! gpui's `platform` modifier is the Windows key off a Mac and no application gets to use it.

use auris_i18n::Key;
use gpui::{Modifiers, MouseDownEvent, Pixels, Point, px};

/// How far the pointer must travel before a press counts as a drag.
///
/// Three pixels is about the wobble a firm click produces on a trackpad, and well under what
/// anybody would call a movement. Below it a gesture that snaps to the grid — moving a clip —
/// would turn every selecting click into an edit.
pub const DRAG_THRESHOLD: Pixels = px(3.0);

/// Whether the pointer has moved far enough from `from` to have meant it.
///
/// Squared distance rather than a square root: the comparison is the same and the arithmetic is
/// exact for the small integers a threshold is made of.
pub fn past_drag_threshold(from: Point<Pixels>, to: Point<Pixels>) -> bool {
    let dx = f32::from(to.x - from.x);
    let dy = f32::from(to.y - from.y);
    let limit = f32::from(DRAG_THRESHOLD);
    dx * dx + dy * dy >= limit * limit
}

/// What a turn of the wheel over the arrangement means.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wheel {
    /// Zoom the time axis about the pointer.
    Zoom,
    /// Travel along the song.
    AlongTheSong,
    /// Travel down the track list.
    DownTheTracks,
}

/// Which of the three a wheel event is asking for.
///
/// Ctrl zooms because that is what a wheel does in every application on every platform, and Alt
/// keeps zooming because that is what it does in Logic. The two do not collide, and dropping
/// either would be picking a fight with somebody's hands. Read as `control` rather than through
/// [`gpui::Modifiers::secondary`] deliberately: this is the *zoom* modifier, which is ⌃ on a Mac
/// as well, and not the command one.
///
/// Free rather than decided in the handler because it is the one gesture where being wrong is
/// invisible — the view scrolls when the user asked it to zoom, and nothing says which was meant.
pub fn wheel_action(modifiers: Modifiers) -> Wheel {
    if modifiers.control || modifiers.alt {
        Wheel::Zoom
    } else if modifiers.shift {
        Wheel::AlongTheSong
    } else {
        Wheel::DownTheTracks
    }
}
use serde::{Deserialize, Serialize};

/// A click that means more than "select this".
///
/// Deliberately a short closed list rather than a free choice of modifier. ⇧-click is not here
/// because it extends a selection, and the raw ⌃ key is not here because macOS turns ⌃-click
/// into a right-click before it ever reaches the window — the one place a modifier means
/// something different enough that offering it would be a trap.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// The shared `Click` suffix is the point: these are all clicks, and dropping it would leave a
// variant called `Option`, which in Rust reads as something else entirely.
#[allow(clippy::enum_variant_names)]
pub enum PointerGesture {
    /// Press with nothing held down.
    ///
    /// The gesture somebody who has not read anything will try first, and the reason it is here:
    /// holding a modifier to write a note is a thing you have to be told, and the person being
    /// told is halfway through deciding whether this application is worth learning. It costs the
    /// rubber band over empty space, which is why it is a choice and not the default — see
    /// [`empty_press`], which keeps ⇧-drag sweeping whatever create is set to.
    Click,
    /// Hold the platform's command modifier — ⌘ on macOS, Ctrl elsewhere — and click.
    CommandClick,
    /// Hold the option key — ⌥ on macOS, Alt elsewhere — and click.
    OptionClick,
    /// Click twice in quick succession.
    DoubleClick,
}

impl PointerGesture {
    /// Every gesture, in the order a picker should list them.
    pub const ALL: [PointerGesture; 4] = [
        PointerGesture::Click,
        PointerGesture::CommandClick,
        PointerGesture::OptionClick,
        PointerGesture::DoubleClick,
    ];

    /// Whether `event` is this gesture.
    ///
    /// A modifier gesture ignores the click count: holding ⌘ and clicking twice is still two
    /// ⌘-clicks, and refusing the second one would look like a dropped input.
    pub fn matches(self, event: &MouseDownEvent) -> bool {
        match self {
            // Every modifier is refused rather than only the two that name gestures. ⇧ is the
            // extend-a-selection key everywhere in the application, and a plain-click *create*
            // that also claimed ⇧-click would leave no way at all to sweep a rubber band.
            PointerGesture::Click => {
                event.click_count == 1
                    && !event.modifiers.secondary()
                    && !event.modifiers.alt
                    && !event.modifiers.shift
            }
            PointerGesture::CommandClick => event.modifiers.secondary() && !event.modifiers.alt,
            PointerGesture::OptionClick => event.modifiers.alt && !event.modifiers.secondary(),
            // A modified double-click belongs to whichever modifier gesture claims it, so that a
            // ⌘-double-click on empty space creates once rather than also deleting.
            PointerGesture::DoubleClick => {
                event.click_count >= 2
                    && !event.modifiers.secondary()
                    && !event.modifiers.alt
                    && !event.modifiers.shift
            }
        }
    }

    /// Whether this gesture may be the one that *deletes*.
    ///
    /// [`PointerGesture::Click`] may not. Selecting and destroying would then be the same press:
    /// every attempt to pick a note up would remove it instead, and there would be no gesture
    /// left anywhere that means "just this one, please". Creating on a bare click is recoverable
    /// — the note is there to be seen and undone — and deleting on one is not.
    pub fn may_delete(self) -> bool {
        self != PointerGesture::Click
    }

    /// How the gesture is written in the settings window.
    ///
    /// ⌘ and ⌥ are Apple's glyphs and appear on Apple's keyboards. Naming them at a Windows or
    /// Linux user would be asking them to press a key they cannot find.
    pub fn label(self) -> Key {
        match self {
            PointerGesture::Click => Key::GesturePlainClick,
            PointerGesture::CommandClick if cfg!(target_os = "macos") => Key::GestureCommandClick,
            PointerGesture::CommandClick => Key::GestureControlClick,
            PointerGesture::OptionClick if cfg!(target_os = "macos") => Key::GestureOptionClick,
            PointerGesture::OptionClick => Key::GestureAltClick,
            PointerGesture::DoubleClick => Key::GestureDoubleClick,
        }
    }
}

/// What a press on empty space means: make something, or sweep the selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EmptyPress {
    /// Write a note here, or a clip on this lane.
    Create,
    /// Sweep a rubber band.
    Band {
        /// Add what it catches to the selection rather than replacing it.
        extend: bool,
    },
}

/// Which of the two a press on empty grid or an empty lane is asking for.
///
/// Free rather than decided at each of the two call sites because it is the rule that decides
/// whether a bare click can create at all. Before it, both sites read "create if the create
/// gesture matches, otherwise band", which is fine while create needs a modifier and silently
/// costs the rubber band the moment it does not — the drag that selects and the click that
/// creates would be the same press, and the roll would simply stop being able to select a range.
///
/// ⇧ is what resolves it: [`PointerGesture::Click`] refuses a shifted press, so ⇧-drag sweeps
/// whatever create is set to. That is not a special case for one setting — ⇧ already means
/// "extend the selection" on every other press in the application.
pub fn empty_press(gestures: PointerGestures, event: &MouseDownEvent) -> EmptyPress {
    if gestures.create.matches(event) {
        EmptyPress::Create
    } else {
        EmptyPress::Band {
            extend: event.modifiers.shift,
        }
    }
}

/// The gestures that create and delete.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointerGestures {
    /// Makes a note in the piano roll, or a clip on an empty lane.
    pub create: PointerGesture,
    /// Removes whatever is under the pointer.
    pub delete: PointerGesture,
}

impl Default for PointerGestures {
    /// Logic Pro's arrangement, because it is the one most people arrive with.
    ///
    /// Delete is *not* on the double-click. It was, and the result was that the gesture every
    /// editor with regions uses to open one destroyed it instead — a double-click meant to look
    /// inside a clip removed it, and the only sign was that it had gone. Deleting is now the
    /// option-click, which is a deliberate enough gesture to be safe, and the double-click opens
    /// the clip. Both remain configurable, so anyone who wants the old arrangement can say so.
    fn default() -> Self {
        Self {
            create: PointerGesture::CommandClick,
            delete: PointerGesture::OptionClick,
        }
    }
}

impl PointerGestures {
    /// Assigns the create gesture, swapping with delete if that is where it already was.
    ///
    /// Swapping rather than refusing: two identical gestures would make one of them
    /// unreachable, and a settings panel that silently ignores a click is worse than one that
    /// rearranges itself.
    ///
    /// The swap cannot hand delete a gesture that [`PointerGesture::may_delete`] refuses, which
    /// is the whole reason it is not one line. Create on the bare click and delete on the
    /// double-click is a combination the picker allows; choosing the double-click for *create*
    /// from there would have handed the bare click to delete, and a plain click would have
    /// started deleting notes without anybody having asked for it.
    pub fn set_create(&mut self, gesture: PointerGesture) {
        if self.delete == gesture {
            self.delete = if self.create.may_delete() {
                self.create
            } else {
                Self::spare_delete(gesture)
            };
        }
        self.create = gesture;
    }

    /// Assigns the delete gesture, swapping with create if that is where it already was.
    ///
    /// A gesture [`PointerGesture::may_delete`] refuses changes nothing. The settings window does
    /// not offer one, so this is the second lock rather than the first — but the two are one
    /// serialised struct, and a hand-edited `keymap.json` reaches this the same way.
    pub fn set_delete(&mut self, gesture: PointerGesture) {
        if !gesture.may_delete() {
            return;
        }
        if self.create == gesture {
            self.create = self.delete;
        }
        self.delete = gesture;
    }

    /// A gesture that may delete and is not `taken`.
    fn spare_delete(taken: PointerGesture) -> PointerGesture {
        PointerGesture::ALL
            .into_iter()
            .find(|gesture| gesture.may_delete() && *gesture != taken)
            .expect("three gestures may delete, so one is always free of any single other")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_i18n::Language;
    use gpui::{Modifiers, MouseButton, point, px};

    #[test]
    fn a_press_that_has_not_travelled_is_not_a_drag() {
        // What stops a selecting click from snapping an off-grid clip onto the grid.
        let from = point(px(100.0), px(100.0));
        assert!(!past_drag_threshold(from, from));
        assert!(!past_drag_threshold(from, point(px(102.0), px(100.0))));
        assert!(!past_drag_threshold(from, point(px(102.0), px(102.0))));
        assert!(past_drag_threshold(from, point(px(104.0), px(100.0))));
        assert!(
            past_drag_threshold(from, point(px(97.0), px(100.0))),
            "and it counts in either direction",
        );
        assert!(
            past_drag_threshold(from, point(px(100.0), px(103.0))),
            "vertically too — a clip crosses tracks that way",
        );
    }

    #[test]
    fn the_wheel_zooms_under_either_key_the_hand_reaches_for() {
        // Ctrl is what a wheel zooms with everywhere else; Alt is what it zooms with in Logic.
        // Whichever a person arrives with has to work, and the two never mean anything else here.
        assert_eq!(wheel_action(Modifiers::control()), Wheel::Zoom);
        assert_eq!(wheel_action(Modifiers::alt()), Wheel::Zoom);
        assert_eq!(wheel_action(Modifiers::shift()), Wheel::AlongTheSong);
        assert_eq!(wheel_action(Modifiers::none()), Wheel::DownTheTracks);
        // Zoom outranks travel, so a hand holding both does the one it can see happening.
        let mut both = Modifiers::control();
        both.shift = true;
        assert_eq!(wheel_action(both), Wheel::Zoom);
    }

    fn click(modifiers: Modifiers, count: usize) -> MouseDownEvent {
        MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(0.0), px(0.0)),
            modifiers,
            click_count: count,
            first_mouse: false,
        }
    }

    #[test]
    fn each_gesture_recognises_only_itself() {
        let plain = click(Modifiers::none(), 1);
        let command = click(Modifiers::secondary_key(), 1);
        let option = click(Modifiers::alt(), 1);
        let double = click(Modifiers::none(), 2);

        assert!(PointerGesture::CommandClick.matches(&command));
        assert!(!PointerGesture::CommandClick.matches(&plain));
        assert!(!PointerGesture::CommandClick.matches(&option));

        assert!(PointerGesture::OptionClick.matches(&option));
        assert!(!PointerGesture::OptionClick.matches(&command));

        assert!(PointerGesture::DoubleClick.matches(&double));
        assert!(!PointerGesture::DoubleClick.matches(&plain));
        assert!(!PointerGesture::DoubleClick.matches(&click(Modifiers::shift(), 2)));
    }

    #[test]
    fn command_and_option_together_are_not_either_gesture() {
        let mut both = Modifiers::secondary_key();
        both.alt = true;
        let press = click(both, 1);
        assert!(!PointerGesture::CommandClick.matches(&press));
        assert!(!PointerGesture::OptionClick.matches(&press));
        assert_eq!(
            empty_press(PointerGestures::default(), &press),
            EmptyPress::Band { extend: false }
        );
    }

    #[test]
    fn a_plain_click_is_claimed_by_exactly_one_gesture() {
        // Only the one that *is* a plain click. Every other has to leave it alone, or clicking
        // empty space to sweep a selection would create a note as well.
        let plain = click(Modifiers::none(), 1);
        for gesture in PointerGesture::ALL {
            assert_eq!(
                gesture.matches(&plain),
                gesture == PointerGesture::Click,
                "{gesture:?} is wrong about a plain click"
            );
        }
    }

    #[test]
    fn a_bare_click_is_a_gesture_only_while_nothing_is_held() {
        // ⇧ is the extend-a-selection key, and `empty_press` leans on this to keep the rubber
        // band reachable when create is a bare click. A `Click` that claimed a shifted press
        // would take the last way of sweeping a range with it.
        let mut shifted = Modifiers::none();
        shifted.shift = true;
        assert!(!PointerGesture::Click.matches(&click(shifted, 1)));
        assert!(!PointerGesture::Click.matches(&click(Modifiers::alt(), 1)));
        assert!(!PointerGesture::Click.matches(&click(Modifiers::secondary_key(), 1)));
        // And the second press of a double-click is not a third bare click on top of it.
        assert!(!PointerGesture::Click.matches(&click(Modifiers::none(), 2)));
    }

    #[test]
    fn nothing_but_a_modifier_gesture_may_delete() {
        // Deleting on a bare click would make selecting and destroying the same press: every
        // attempt to pick a note up would remove it, with no gesture left that means "this one".
        assert!(!PointerGesture::Click.may_delete());
        for gesture in PointerGesture::ALL {
            assert_eq!(gesture.may_delete(), gesture != PointerGesture::Click);
        }

        let mut gestures = PointerGestures::default();
        let delete = gestures.delete;
        gestures.set_delete(PointerGesture::Click);
        assert_eq!(gestures.delete, delete, "the bare click was accepted");
    }

    #[test]
    fn taking_the_delete_gesture_for_create_never_hands_back_a_bare_click() {
        // Create on the bare click, delete on the double-click — both offered. Choosing the
        // double-click for create from there used to swap the bare click into delete, and a
        // plain click would have started removing notes with nobody having asked for it.
        let mut gestures = PointerGestures::default();
        gestures.set_create(PointerGesture::Click);
        gestures.set_delete(PointerGesture::DoubleClick);

        gestures.set_create(PointerGesture::DoubleClick);
        assert_eq!(gestures.create, PointerGesture::DoubleClick);
        assert!(
            gestures.delete.may_delete(),
            "delete was handed {:?}",
            gestures.delete
        );
        assert_ne!(gestures.create, gestures.delete);
    }

    #[test]
    fn a_bare_click_creates_and_still_leaves_a_way_to_sweep() {
        // What makes the bare click offerable at all. Without the ⇧ escape the roll would simply
        // lose range selection the moment somebody chose it.
        let mut gestures = PointerGestures::default();
        gestures.set_create(PointerGesture::Click);

        let mut shifted = Modifiers::none();
        shifted.shift = true;
        assert_eq!(
            empty_press(gestures, &click(Modifiers::none(), 1)),
            EmptyPress::Create
        );
        assert_eq!(
            empty_press(gestures, &click(shifted, 1)),
            EmptyPress::Band { extend: true }
        );

        // And under the default the bare click still sweeps, as it always has.
        let defaults = PointerGestures::default();
        assert_eq!(
            empty_press(defaults, &click(Modifiers::none(), 1)),
            EmptyPress::Band { extend: false }
        );
        assert_eq!(
            empty_press(defaults, &click(Modifiers::secondary_key(), 1)),
            EmptyPress::Create
        );
    }

    #[test]
    fn a_modified_double_click_belongs_to_the_modifier() {
        let command_twice = click(Modifiers::secondary_key(), 2);
        assert!(PointerGesture::CommandClick.matches(&command_twice));
        assert!(
            !PointerGesture::DoubleClick.matches(&command_twice),
            "one press must not be both gestures at once"
        );
    }

    #[test]
    fn the_create_gesture_uses_a_key_this_platform_actually_has() {
        // gpui's `platform` modifier is ⌘ on macOS but the *Windows key* on Windows, which the
        // shell claims before an application sees it. Reading it directly would leave note
        // editing dead off a Mac, with nothing in the log to say why.
        assert!(PointerGesture::CommandClick.matches(&click(Modifiers::secondary_key(), 1)));
        if !cfg!(target_os = "macos") {
            assert!(PointerGesture::CommandClick.matches(&click(Modifiers::control(), 1)));
            assert!(!PointerGesture::CommandClick.matches(&click(Modifiers::windows(), 1)));
        }
        if cfg!(target_os = "macos") {
            assert!(PointerGesture::CommandClick.matches(&click(Modifiers::command(), 1)));
        }
    }

    #[test]
    fn the_labels_name_keys_this_platform_has() {
        let create = PointerGesture::CommandClick.label().get(Language::English);
        let option = PointerGesture::OptionClick.label().get(Language::English);
        if cfg!(target_os = "macos") {
            assert_eq!((create, option), ("⌘-click", "⌥-click"));
        } else {
            assert_eq!((create, option), ("Ctrl-click", "Alt-click"));
        }
    }

    #[test]
    fn the_defaults_are_the_ones_logic_users_expect() {
        let gestures = PointerGestures::default();
        assert_eq!(gestures.create, PointerGesture::CommandClick);
        assert_eq!(gestures.delete, PointerGesture::OptionClick);
    }

    #[test]
    fn nothing_destructive_is_bound_to_a_double_click_by_default() {
        // A double-click opens a region in every editor that has regions. While delete was the
        // default there, the gesture people arrive with deleted their work and said nothing.
        let gestures = PointerGestures::default();
        assert_ne!(gestures.delete, PointerGesture::DoubleClick);
    }

    // Both of these are written against whatever the defaults happen to be rather than against
    // the gestures they name, because the rule is about the swap and not about which two the
    // application ships with — the first version of them broke when a default changed.

    #[test]
    fn assigning_a_gesture_that_is_taken_swaps_the_two() {
        let mut gestures = PointerGestures::default();
        let (was_create, was_delete) = (gestures.create, gestures.delete);

        gestures.set_create(was_delete);
        assert_eq!(gestures.create, was_delete);
        assert_eq!(
            gestures.delete, was_create,
            "delete took the gesture create gave up"
        );

        gestures.set_delete(was_delete);
        assert_eq!(gestures.delete, was_delete);
        assert_eq!(gestures.create, was_create);
    }

    #[test]
    fn assigning_a_gesture_that_is_free_leaves_the_other_alone() {
        let mut gestures = PointerGestures::default();
        let free = PointerGesture::ALL
            .into_iter()
            .find(|gesture| *gesture != gestures.create && *gesture != gestures.delete)
            .expect("three gestures fill two slots, so one is always spare");
        let delete = gestures.delete;

        gestures.set_create(free);
        assert_eq!(gestures.create, free);
        assert_eq!(gestures.delete, delete);
    }

    #[test]
    fn the_two_are_never_the_same_however_they_are_set() {
        for create in PointerGesture::ALL {
            for delete in PointerGesture::ALL {
                let mut gestures = PointerGestures::default();
                gestures.set_create(create);
                gestures.set_delete(delete);
                assert_ne!(gestures.create, gestures.delete);
            }
        }
    }
}
