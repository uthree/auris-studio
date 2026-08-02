//! What a click does, when the click is not simply a click.
//!
//! Creating and deleting are the two gestures every DAW binds differently — Logic creates on
//! ⌘-click and deletes on a double-click, others use ⌥ or a dedicated tool — so they are
//! configurable rather than baked in, the same way a keystroke is.

use auris_i18n::Key;
use gpui::MouseDownEvent;
use serde::{Deserialize, Serialize};

/// A click that means more than "select this".
///
/// Deliberately a short closed list rather than a free choice of modifier. ⌃-click is not here
/// because macOS turns it into a right-click before it ever reaches the window, and ⇧-click is
/// not here because it extends a selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// The shared `Click` suffix is the point: these are all clicks, and dropping it would leave a
// variant called `Option`, which in Rust reads as something else entirely.
#[allow(clippy::enum_variant_names)]
pub enum PointerGesture {
    /// Hold the command key and click.
    CommandClick,
    /// Hold the option key and click.
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
            PointerGesture::CommandClick => event.modifiers.platform,
            PointerGesture::OptionClick => event.modifiers.alt,
            // A modified double-click belongs to whichever modifier gesture claims it, so that a
            // ⌘-double-click on empty space creates once rather than also deleting.
            PointerGesture::DoubleClick => {
                event.click_count >= 2 && !event.modifiers.platform && !event.modifiers.alt
            }
        }
    }

    /// How the gesture is written in the settings window.
    pub fn label(self) -> Key {
        match self {
            PointerGesture::CommandClick => Key::GestureCommandClick,
            PointerGesture::OptionClick => Key::GestureOptionClick,
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
    fn default() -> Self {
        Self {
            create: PointerGesture::CommandClick,
            delete: PointerGesture::DoubleClick,
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
    use gpui::{Modifiers, MouseButton, point, px};

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
        let command = click(Modifiers::command(), 1);
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
        let command_twice = click(Modifiers::command(), 2);
        assert!(PointerGesture::CommandClick.matches(&command_twice));
        assert!(
            !PointerGesture::DoubleClick.matches(&command_twice),
            "one press must not be both gestures at once"
        );
    }

    #[test]
    fn the_defaults_are_the_ones_logic_users_expect() {
        let gestures = PointerGestures::default();
        assert_eq!(gestures.create, PointerGesture::CommandClick);
        assert_eq!(gestures.delete, PointerGesture::DoubleClick);
    }

    #[test]
    fn assigning_a_gesture_that_is_taken_swaps_the_two() {
        let mut gestures = PointerGestures::default();
        gestures.set_create(PointerGesture::DoubleClick);
        assert_eq!(gestures.create, PointerGesture::DoubleClick);
        assert_eq!(
            gestures.delete,
            PointerGesture::CommandClick,
            "delete took the gesture create gave up"
        );

        gestures.set_delete(PointerGesture::DoubleClick);
        assert_eq!(gestures.delete, PointerGesture::DoubleClick);
        assert_eq!(gestures.create, PointerGesture::CommandClick);
    }

    #[test]
    fn assigning_a_gesture_that_is_free_leaves_the_other_alone() {
        let mut gestures = PointerGestures::default();
        gestures.set_create(PointerGesture::OptionClick);
        assert_eq!(gestures.create, PointerGesture::OptionClick);
        assert_eq!(gestures.delete, PointerGesture::DoubleClick);
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
