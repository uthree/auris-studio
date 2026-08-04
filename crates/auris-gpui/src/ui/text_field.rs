//! A one-line text field, and the platform input plumbing that makes it accept an IME.
//!
//! Renaming is the one place in the application where the user types prose rather than pressing
//! a shortcut, so the field goes through the system input handler instead of reading key events.
//! That is what makes Japanese, Chinese and Korean input work at all: with a key-event field the
//! composition window never appears and the pre-edit text has nowhere to live.
//!
//! Offsets inside [`TextField`] are byte offsets into a `String`. The platform speaks UTF-16, so
//! every value crossing that boundary is converted — a multi-byte character is one UTF-16 unit
//! and three bytes, and mixing the two up cuts a character in half.

use std::cell::Cell;
use std::ops::Range;

use gpui::{Bounds, Pixels};

/// A single line of editable text with a selection and optional IME pre-edit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextField {
    content: String,
    /// Byte range of the selection. An empty range is the caret.
    selection: Range<usize>,
    /// Whether the moving end of the selection is its start rather than its end.
    ///
    /// Without this, shift-left followed by shift-right would grow the selection at the wrong
    /// end, because a bare range cannot say which end the user is dragging.
    reversed: bool,
    /// Byte range of the text the IME is still composing, if any.
    marked: Option<Range<usize>>,
}

impl TextField {
    /// A field holding `text`, with all of it selected.
    ///
    /// Selecting everything is what makes a rename dialog behave: the first keystroke replaces
    /// the old name instead of appending to it.
    pub fn new(text: impl Into<String>) -> Self {
        let content = text.into();
        let selection = 0..content.len();
        Self {
            content,
            selection,
            reversed: false,
            marked: None,
        }
    }

    /// The text.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// The selection, as byte offsets.
    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    /// Whether the selection grows from its start.
    pub fn is_reversed(&self) -> bool {
        self.reversed
    }

    /// The text inside the selection, which is empty when the caret is a point.
    pub fn selected_text(&self) -> String {
        self.content[self.clamp(self.selection.clone())].to_string()
    }

    /// The range the IME is composing, as byte offsets.
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Replaces `range` with `text` and puts the caret after it.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let range = self.clamp(range);
        self.content.replace_range(range.clone(), text);
        let caret = range.start + text.len();
        self.selection = caret..caret;
        self.reversed = false;
        self.marked = None;
    }

    /// Replaces `range` with `text` and marks the result as IME pre-edit.
    pub fn replace_and_mark(
        &mut self,
        range: Range<usize>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = self.clamp(range);
        self.content.replace_range(range.clone(), text);
        let marked = range.start..range.start + text.len();
        self.selection = match selected {
            // The IME's selection is relative to the text it just inserted.
            Some(inner) => self.clamp(marked.start + inner.start..marked.start + inner.end),
            None => marked.end..marked.end,
        };
        self.reversed = false;
        self.marked = (!text.is_empty()).then_some(marked);
    }

    /// Types `text` over the selection.
    pub fn insert(&mut self, text: &str) {
        let range = self.replacement_range();
        self.replace(range, text);
    }

    /// Deletes the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        let range = self.replacement_range();
        let range = if range.is_empty() {
            self.previous_boundary(range.start)..range.end
        } else {
            range
        };
        self.replace(range, "");
    }

    /// Deletes the selection, or the character after the caret.
    pub fn delete_forward(&mut self) {
        let range = self.replacement_range();
        let range = if range.is_empty() {
            range.start..self.next_boundary(range.end)
        } else {
            range
        };
        self.replace(range, "");
    }

    /// Moves the caret one character left, or collapses the selection to its start.
    pub fn move_left(&mut self, extend: bool) {
        let head = if extend || self.selection.is_empty() {
            self.previous_boundary(self.head())
        } else {
            self.selection.start
        };
        self.move_caret(head, extend);
    }

    /// Moves the caret one character right, or collapses the selection to its end.
    pub fn move_right(&mut self, extend: bool) {
        let head = if extend || self.selection.is_empty() {
            self.next_boundary(self.head())
        } else {
            self.selection.end
        };
        self.move_caret(head, extend);
    }

    /// Moves the caret to the start of the line.
    pub fn move_home(&mut self, extend: bool) {
        self.move_caret(0, extend);
    }

    /// Moves the caret to the end of the line.
    pub fn move_end(&mut self, extend: bool) {
        self.move_caret(self.content.len(), extend);
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.selection = 0..self.content.len();
        self.reversed = false;
    }

    /// Abandons any IME pre-edit, keeping the text it produced.
    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// The range an edit should replace: the pre-edit when composing, the selection otherwise.
    fn replacement_range(&self) -> Range<usize> {
        self.marked
            .clone()
            .unwrap_or_else(|| self.selection.clone())
    }

    /// The end of the selection the user is moving.
    fn head(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    /// The end of the selection that stays put.
    fn anchor(&self) -> usize {
        if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        }
    }

    fn move_caret(&mut self, head: usize, extend: bool) {
        let head = self.clamp(head..head).start;
        let anchor = if extend { self.anchor() } else { head };
        self.reversed = head < anchor;
        self.selection = head.min(anchor)..head.max(anchor);
        self.marked = None;
    }

    /// Snaps a range onto character boundaries inside the content.
    ///
    /// Every offset that arrives from the platform is treated as untrusted: a stale range from
    /// the IME would otherwise panic `String::replace_range`.
    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let start = self.floor_boundary(range.start.min(self.content.len()));
        let end = self.floor_boundary(range.end.clamp(start, self.content.len()));
        start..end
    }

    fn floor_boundary(&self, mut offset: usize) -> usize {
        while offset > 0 && !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..self.floor_boundary(offset)]
            .chars()
            .next_back()
            .map_or(0, |c| offset - c.len_utf8())
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.content[offset..]
            .chars()
            .next()
            .map_or(self.content.len(), |c| offset + c.len_utf8())
    }

    // ------------------------------------------------------------------ UTF-16

    /// Converts a byte offset into a UTF-16 offset.
    pub fn utf16_offset(&self, offset: usize) -> usize {
        self.content[..self.floor_boundary(offset)]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    /// Converts a UTF-16 offset into a byte offset.
    pub fn byte_offset(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (index, ch) in self.content.char_indices() {
            if utf16 >= offset {
                return index;
            }
            utf16 += ch.len_utf16();
        }
        self.content.len()
    }

    /// Converts a byte range into a UTF-16 range.
    pub fn utf16_range(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_offset(range.start)..self.utf16_offset(range.end)
    }

    /// Converts a UTF-16 range into a byte range.
    pub fn byte_range(&self, range: &Range<usize>) -> Range<usize> {
        let start = self.byte_offset(range.start);
        start..self.byte_offset(range.end).max(start)
    }
}

thread_local! {
    /// Where the caret was drawn last, in window coordinates.
    ///
    /// The platform asks for this when an IME starts composing, so it can put the candidate list
    /// under the text being composed rather than wherever it feels like — bottom right of the
    /// screen, on Windows, when the application does not answer. It can only be worked out during
    /// paint, from the shaped line, and it is asked for outside one; a cell between the two is the
    /// cheapest honest way across.
    ///
    /// One cell for the whole application, because one field at a time is being typed into: the
    /// platform asks whichever input handler is registered, and only one ever is.
    static CARET: Cell<Option<Bounds<Pixels>>> = const { Cell::new(None) };
}

/// Records where the caret was painted, for the platform to place an IME beside.
pub fn set_caret_bounds(bounds: Bounds<Pixels>) {
    CARET.with(|caret| caret.set(Some(bounds)));
}

/// Where the caret was painted last, if a field has painted one.
pub fn caret_bounds() -> Option<Bounds<Pixels>> {
    CARET.with(Cell::get)
}

/// A view the platform can type into.
///
/// One field at a time — whichever sheet, palette or box currently owns the keyboard. Two views
/// implement it now, and the conversions between the platform's UTF-16 offsets and the field's
/// byte offsets are fiddly enough that two copies of them would mean one that is subtly wrong.
pub trait HasTextField {
    /// The field being typed into, if any.
    fn field(&mut self) -> Option<&mut TextField>;

    /// The same field, for the one handler method that only has `&self`.
    fn readable_field(&self) -> Option<&TextField>;

    /// Run after the text changes, for a view with something to keep in step with it.
    ///
    /// Typing arrives here rather than at a key handler — that is what lets an IME compose into
    /// the field — so anything derived from the text has nowhere else to learn that it moved.
    fn text_changed(&mut self) {}
}

/// Writes [`gpui::EntityInputHandler`] for a view that implements [`HasTextField`].
///
/// A macro rather than a blanket implementation because the orphan rule forbids one: the trait is
/// gpui's and a blanket impl over a type parameter names no local type.
#[macro_export]
macro_rules! entity_input_handler {
    ($view:ty) => {
        /// Text input from the platform, including anything an IME composes.
        ///
        /// Every offset crossing this boundary is in UTF-16 units, which is what the platform
        /// counts in; `TextField` stores byte offsets, so each one is converted rather than
        /// passed through.
        impl ::gpui::EntityInputHandler for $view {
            fn text_for_range(
                &mut self,
                range_utf16: ::std::ops::Range<usize>,
                adjusted_range: &mut Option<::std::ops::Range<usize>>,
                _window: &mut ::gpui::Window,
                _cx: &mut ::gpui::Context<Self>,
            ) -> Option<String> {
                let field = $crate::ui::text_field::HasTextField::field(self)?;
                let range = field.byte_range(&range_utf16);
                *adjusted_range = Some(field.utf16_range(&range));
                Some(field.content()[range].to_string())
            }

            fn selected_text_range(
                &mut self,
                _ignore_disabled_input: bool,
                _window: &mut ::gpui::Window,
                _cx: &mut ::gpui::Context<Self>,
            ) -> Option<::gpui::UTF16Selection> {
                let field = $crate::ui::text_field::HasTextField::field(self)?;
                Some(::gpui::UTF16Selection {
                    range: field.utf16_range(&field.selection()),
                    reversed: field.is_reversed(),
                })
            }

            fn marked_text_range(
                &self,
                _window: &mut ::gpui::Window,
                _cx: &mut ::gpui::Context<Self>,
            ) -> Option<::std::ops::Range<usize>> {
                let field = $crate::ui::text_field::HasTextField::readable_field(self)?;
                Some(field.utf16_range(&field.marked()?))
            }

            fn unmark_text(
                &mut self,
                _window: &mut ::gpui::Window,
                _cx: &mut ::gpui::Context<Self>,
            ) {
                if let Some(field) = $crate::ui::text_field::HasTextField::field(self) {
                    field.unmark();
                }
            }

            fn replace_text_in_range(
                &mut self,
                range_utf16: Option<::std::ops::Range<usize>>,
                text: &str,
                _window: &mut ::gpui::Window,
                cx: &mut ::gpui::Context<Self>,
            ) {
                let Some(field) = $crate::ui::text_field::HasTextField::field(self) else {
                    return;
                };
                // No range means "whatever is being replaced right now" — the pre-edit if the
                // IME is composing, the selection otherwise.
                match range_utf16 {
                    Some(range) => {
                        let range = field.byte_range(&range);
                        field.replace(range, text);
                    }
                    None => field.insert(text),
                }
                $crate::ui::text_field::HasTextField::text_changed(self);
                ::gpui::Context::notify(cx);
            }

            fn replace_and_mark_text_in_range(
                &mut self,
                range_utf16: Option<::std::ops::Range<usize>>,
                new_text: &str,
                new_selected_range_utf16: Option<::std::ops::Range<usize>>,
                _window: &mut ::gpui::Window,
                cx: &mut ::gpui::Context<Self>,
            ) {
                let Some(field) = $crate::ui::text_field::HasTextField::field(self) else {
                    return;
                };
                let range = match range_utf16 {
                    Some(range) => field.byte_range(&range),
                    None => field.marked().unwrap_or_else(|| field.selection()),
                };
                // This selection is relative to `new_text`, so it is measured against that
                // rather than against the field's own contents.
                let selected = new_selected_range_utf16.map(|range| {
                    let start = $crate::ui::text_field::utf16_to_byte(new_text, range.start);
                    start..$crate::ui::text_field::utf16_to_byte(new_text, range.end).max(start)
                });
                field.replace_and_mark(range, new_text, selected);
                $crate::ui::text_field::HasTextField::text_changed(self);
                ::gpui::Context::notify(cx);
            }

            fn bounds_for_range(
                &mut self,
                _range_utf16: ::std::ops::Range<usize>,
                element_bounds: ::gpui::Bounds<::gpui::Pixels>,
                _window: &mut ::gpui::Window,
                _cx: &mut ::gpui::Context<Self>,
            ) -> Option<::gpui::Bounds<::gpui::Pixels>> {
                // The caret if a field has painted one, so the candidate list sits under the
                // characters being composed. The field's own box otherwise, which is the right
                // row and the wrong column — still far better than the nothing that sends the
                // list to the corner of the screen.
                Some($crate::ui::text_field::caret_bounds().unwrap_or(element_bounds))
            }

            fn character_index_for_point(
                &mut self,
                _point: ::gpui::Point<::gpui::Pixels>,
                _window: &mut ::gpui::Window,
                _cx: &mut ::gpui::Context<Self>,
            ) -> Option<usize> {
                None
            }
        }
    };
}

/// Converts a UTF-16 offset into a byte offset within `text`.
pub fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (index, ch) in text.char_indices() {
        if utf16 >= offset {
            return index;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_inside_the_ime_s_own_text_are_converted() {
        assert_eq!(utf16_to_byte("かな", 0), 0);
        assert_eq!(utf16_to_byte("かな", 1), 3);
        assert_eq!(utf16_to_byte("かな", 2), 6);
        assert_eq!(utf16_to_byte("かな", 9), 6, "past the end is the end");
        // A surrogate pair is two UTF-16 units and four bytes.
        assert_eq!(utf16_to_byte("𝄞x", 2), 4);
    }

    /// A range whose end precedes its start, which is nonsense the platform can still hand over.
    ///
    /// Built rather than written literally: a `4..1` in the source is a compile error under the
    /// project's lints, and the point of these tests is that such a range arrives at run time.
    fn inverted(start: usize, end: usize) -> Range<usize> {
        Range { start, end }
    }

    #[test]
    fn a_new_field_starts_with_everything_selected() {
        let mut field = TextField::new("Lead");
        assert_eq!(field.selection(), 0..4);
        field.insert("Bass");
        assert_eq!(field.content(), "Bass");
    }

    #[test]
    fn editing_moves_by_characters_not_bytes() {
        // Every one of these is three bytes and one UTF-16 unit.
        let mut field = TextField::new("ドラム");
        field.move_end(false);
        assert_eq!(field.selection(), 9..9);

        field.backspace();
        assert_eq!(field.content(), "ドラ", "a whole character is removed");

        field.move_left(false);
        assert_eq!(field.selection(), 3..3);
        field.insert("キ");
        assert_eq!(field.content(), "ドキラ");
    }

    #[test]
    fn utf16_offsets_survive_a_round_trip() {
        let field = TextField::new("aドb");
        assert_eq!(field.content().len(), 5, "bytes");
        assert_eq!(field.utf16_offset(5), 3, "UTF-16 units");
        assert_eq!(field.byte_offset(3), 5);
        assert_eq!(field.byte_offset(2), 4);
        assert_eq!(field.utf16_range(&(1..4)), 1..2);
        assert_eq!(field.byte_range(&(1..2)), 1..4);
        // An inverted range from the platform must not panic or produce one back.
        assert_eq!(field.byte_range(&inverted(9, 1)), 5..5);
    }

    #[test]
    fn a_composition_replaces_itself_until_it_is_committed() {
        let mut field = TextField::new("");
        field.replace_and_mark(0..0, "k", None);
        assert_eq!(field.content(), "k");
        assert_eq!(field.marked(), Some(0..1));

        // The IME rewrites its own pre-edit rather than appending to it.
        field.replace_and_mark(0..1, "か", None);
        assert_eq!(field.content(), "か");
        field.replace_and_mark(0..3, "課", None);
        assert_eq!(field.content(), "課");

        field.replace(0..3, "課");
        assert_eq!(field.content(), "課");
        assert_eq!(field.marked(), None, "committing clears the pre-edit");
        assert_eq!(field.selection(), 3..3);
    }

    #[test]
    fn typing_while_composing_replaces_the_pre_edit() {
        let mut field = TextField::new("abc");
        field.move_end(false);
        field.replace_and_mark(3..3, "n", None);
        assert_eq!(field.content(), "abcn");

        field.insert("x");
        assert_eq!(
            field.content(),
            "abcx",
            "the pre-edit is what gets replaced, not the character before it"
        );
    }

    #[test]
    fn a_stale_range_from_the_platform_cannot_panic() {
        let mut field = TextField::new("ドラム");
        // A range landing inside a character collapses to the boundary below it rather than
        // cutting the character in half.
        field.replace(1..2, "x");
        assert_eq!(field.content(), "xドラム");
        // Past the end.
        field.replace(99..120, "!");
        assert_eq!(field.content(), "xドラム!");
        // Inverted.
        field.replace(inverted(4, 1), "?");
        assert_eq!(field.content(), "xド?ラム!");
    }

    #[test]
    fn shift_arrow_extends_from_the_end_the_user_is_moving() {
        let mut field = TextField::new("abcd");
        field.move_home(false);
        field.move_right(true);
        field.move_right(true);
        assert_eq!(field.selection(), 0..2);
        assert!(!field.is_reversed());

        field.move_left(true);
        assert_eq!(field.selection(), 0..1, "the moving end shrinks back");

        field.move_home(false);
        field.move_end(true);
        field.move_left(true);
        assert_eq!(field.selection(), 0..3);

        field.insert("Z");
        assert_eq!(field.content(), "Zd");
    }

    #[test]
    fn extending_backwards_then_forwards_moves_the_same_end() {
        let mut field = TextField::new("abcd");
        field.move_end(false);
        field.move_left(true);
        field.move_left(true);
        assert_eq!(field.selection(), 2..4);
        assert!(field.is_reversed());

        field.move_right(true);
        assert_eq!(
            field.selection(),
            3..4,
            "the start is what moves, because that is the end being dragged"
        );
    }
}
