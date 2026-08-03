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
use gpui::{MouseDownEvent, Pixels, Point, px};

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
    /// Hold the platform's command modifier — ⌘ on macOS, Ctrl elsewhere — and click.
    CommandClick,
    /// Hold the option key — ⌥ on macOS, Alt elsewhere — and click.
    OptionClick,
    /// Click twice in quick succession.
    DoubleClick,
}

impl PointerGesture {
    /// Every gesture, in the order a picker should list them.
    pub const ALL: [PointerGesture; 3] = [
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
            PointerGesture::CommandClick => event.modifiers.secondary(),
            PointerGesture::OptionClick => event.modifiers.alt,
            // A modified double-click belongs to whichever modifier gesture claims it, so that a
            // ⌘-double-click on empty space creates once rather than also deleting.
            PointerGesture::DoubleClick => {
                event.click_count >= 2 && !event.modifiers.secondary() && !event.modifiers.alt
            }
        }
    }

    /// How the gesture is written in the settings window.
    ///
    /// ⌘ and ⌥ are Apple's glyphs and appear on Apple's keyboards. Naming them at a Windows or
    /// Linux user would be asking them to press a key they cannot find.
    pub fn label(self) -> Key {
        match self {
            PointerGesture::CommandClick if cfg!(target_os = "macos") => Key::GestureCommandClick,
            PointerGesture::CommandClick => Key::GestureControlClick,
            PointerGesture::OptionClick if cfg!(target_os = "macos") => Key::GestureOptionClick,
            PointerGesture::OptionClick => Key::GestureAltClick,
            PointerGesture::DoubleClick => Key::GestureDoubleClick,
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
    pub fn set_create(&mut self, gesture: PointerGesture) {
        if self.delete == gesture {
            self.delete = self.create;
        }
        self.create = gesture;
    }

    /// Assigns the delete gesture, swapping with create if that is where it already was.
    pub fn set_delete(&mut self, gesture: PointerGesture) {
        if self.create == gesture {
            self.create = self.delete;
        }
        self.delete = gesture;
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
    }

    #[test]
    fn a_plain_click_is_never_a_gesture() {
        // Otherwise clicking empty space to move the playhead would also create a note.
        let plain = click(Modifiers::none(), 1);
        for gesture in PointerGesture::ALL {
            assert!(
                !gesture.matches(&plain),
                "{gesture:?} claimed a plain click"
            );
        }
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
        #[cfg(not(target_os = "macos"))]
        {
            assert!(PointerGesture::CommandClick.matches(&click(Modifiers::control(), 1)));
            assert!(!PointerGesture::CommandClick.matches(&click(Modifiers::windows(), 1)));
        }
        #[cfg(target_os = "macos")]
        assert!(PointerGesture::CommandClick.matches(&click(Modifiers::command(), 1)));
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
