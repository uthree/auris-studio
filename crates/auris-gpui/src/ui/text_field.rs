//! An editable text field, and the platform input plumbing that makes it accept an IME.
//!
//! The field holds one line almost everywhere; lyrics are the exception, and what makes the
//! same struct serve both is that a line is just text without `'\n'` in it. The caret's moves
//! are line-aware — Home, End, Up and Down work within and between lines — which costs a
//! one-line field nothing, because its only line is the whole content.
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

/// What a key handed to [`TextField::apply_key`] did.
///
/// Three answers rather than a `bool`, because a caller usually needs to tell an edit from a
/// caret move: the palette puts its highlight back on the first row when the query *changes* and
/// would be wrong to do it when somebody merely pressed Home.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyEffect {
    /// Not a key a field answers for; the caller should look elsewhere.
    Ignored,
    /// The caret or selection moved. The text is as it was.
    Moved,
    /// The text changed.
    Changed,
}

/// Editable text with a selection and optional IME pre-edit.
///
/// One line almost everywhere; a field asked to hold lyrics holds newlines too, and the caret
/// knows how to move among them. Whether Return commits or inserts a line is the caller's
/// question, not this struct's.
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

    /// Moves the caret to the start of its line.
    ///
    /// For content with no newline that is the start of everything, which is what this did
    /// before the field learnt to hold more than one line.
    pub fn move_home(&mut self, extend: bool) {
        let base = if extend {
            self.head()
        } else {
            self.selection.start
        };
        self.move_caret(self.line_start(base), extend);
    }

    /// Moves the caret to the end of its line.
    pub fn move_end(&mut self, extend: bool) {
        let base = if extend {
            self.head()
        } else {
            self.selection.end
        };
        self.move_caret(self.line_end(base), extend);
    }

    /// Puts the caret after the last character, selecting nothing.
    ///
    /// What an editor opening on existing text wants: [`Self::new`] selects everything so a
    /// rename's first keystroke replaces the old name, and that is exactly wrong for a verse
    /// being extended — one key and the verse is gone.
    pub fn caret_to_end(&mut self) {
        self.move_caret(self.content.len(), false);
    }

    /// Moves the caret up one line, keeping its column where the line above is long enough.
    pub fn move_up(&mut self, extend: bool) {
        let base = if extend {
            self.head()
        } else {
            self.selection.start
        };
        let start = self.line_start(base);
        if start == 0 {
            // Already on the first line; the only place above it is its start.
            self.move_caret(0, extend);
            return;
        }
        let column = self.content[start..base].chars().count();
        let above = self.line_start(start - 1);
        self.move_caret(self.at_column(above, column), extend);
    }

    /// Moves the caret down one line, keeping its column where the line below is long enough.
    pub fn move_down(&mut self, extend: bool) {
        let base = if extend {
            self.head()
        } else {
            self.selection.end
        };
        let end = self.line_end(base);
        if end == self.content.len() {
            // Already on the last line; the only place below it is its end.
            self.move_caret(end, extend);
            return;
        }
        let column = self.content[self.line_start(base)..base].chars().count();
        self.move_caret(self.at_column(end + 1, column), extend);
    }

    /// Where the line holding `offset` begins: just after the previous newline.
    fn line_start(&self, offset: usize) -> usize {
        self.content[..self.floor_boundary(offset)]
            .rfind('\n')
            .map_or(0, |at| at + 1)
    }

    /// Where the line holding `offset` ends: just before the next newline.
    fn line_end(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |at| offset + at)
    }

    /// The offset `column` characters into the line starting at `start`, or the line's end
    /// where the line is shorter — a caret carried down from a long line lands there.
    fn at_column(&self, start: usize, column: usize) -> usize {
        let end = self.line_end(start);
        self.content[start..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(at, _)| start + at)
    }

    /// Puts the caret at `offset` — a click landing it — extending the selection on a
    /// shift-click.
    pub fn place_caret(&mut self, offset: usize, extend: bool) {
        self.move_caret(offset, extend);
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.selection = 0..self.content.len();
        self.reversed = false;
    }

    /// Applies a key that edits or moves, and says what it did.
    ///
    /// The platform delivers a field its *insertions* and nothing else — that is what
    /// [`gpui::Window::handle_input`] is for, and it is what lets an IME compose into one.
    /// Everything that is not a character therefore has to be dispatched by hand, and every
    /// field in this application was doing it in its own `match`: four tables that were supposed
    /// to agree, and did not. The library's search box shipped without a backspace at all.
    ///
    /// Escape, Return and the arrow keys are deliberately *not* here. They mean different things
    /// to each field — the palette walks its rows with Up and Down, the rename sheet takes them
    /// as Home and End, and a browser's search box has neither — so each caller answers those
    /// first and hands the rest here.
    ///
    /// Call [`Self::apply_key_with_clipboard`] from a view to include cut, copy and paste.
    pub fn apply_key(&mut self, key: &str, shift: bool, secondary: bool) -> KeyEffect {
        let before = self.content.len();
        match key {
            "backspace" => self.backspace(),
            "delete" => self.delete_forward(),
            "left" => {
                self.move_left(shift);
                return KeyEffect::Moved;
            }
            "right" => {
                self.move_right(shift);
                return KeyEffect::Moved;
            }
            "home" => {
                self.move_home(shift);
                return KeyEffect::Moved;
            }
            "end" => {
                self.move_end(shift);
                return KeyEffect::Moved;
            }
            "a" if secondary => {
                self.select_all();
                return KeyEffect::Moved;
            }
            _ => return KeyEffect::Ignored,
        }
        if self.content.len() == before {
            KeyEffect::Moved
        } else {
            KeyEffect::Changed
        }
    }

    /// Applies editing keys, including the platform's cut, copy and paste shortcuts.
    ///
    /// Single-line fields replace pasted line breaks with spaces. Multiline fields keep them,
    /// normalizing Windows and classic Mac line endings so caret movement sees the same lines.
    pub fn apply_key_with_clipboard(
        &mut self,
        key: &str,
        shift: bool,
        secondary: bool,
        multiline: bool,
        cx: &mut gpui::App,
    ) -> KeyEffect {
        if !secondary {
            return self.apply_key(key, shift, secondary);
        }
        match key {
            "c" | "x" => {
                let selected = self.selected_text();
                if selected.is_empty() {
                    return KeyEffect::Moved;
                }
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                if key == "x" {
                    self.replace(self.selection(), "");
                    KeyEffect::Changed
                } else {
                    KeyEffect::Moved
                }
            }
            "v" => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return KeyEffect::Moved;
                };
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                let text = if multiline {
                    text
                } else {
                    text.replace('\n', " ")
                };
                let before = self.content.clone();
                self.insert(&text);
                if self.content == before {
                    KeyEffect::Moved
                } else {
                    KeyEffect::Changed
                }
            }
            _ => self.apply_key(key, shift, secondary),
        }
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
            if utf16 + ch.len_utf16() > offset {
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
                let field = $crate::ui::text_field::HasTextField::readable_field(self)?;
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
                let field = $crate::ui::text_field::HasTextField::readable_field(self)?;
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
                //
                // Only when the caret is inside *this* field. One cell serves the application and
                // two windows can both have a field on screen; a caret left there by the other
                // one would put the candidate list in a window nobody is typing into.
                let caret = $crate::ui::text_field::caret_bounds()
                    .filter(|caret| element_bounds.contains(&caret.origin));
                Some(caret.unwrap_or(element_bounds))
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

        let astral = TextField::new("a😀b");
        assert_eq!(
            astral.byte_offset(2),
            1,
            "an offset inside a surrogate pair rounds down"
        );
        assert_eq!(astral.byte_offset(3), 5);
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

    #[test]
    fn a_field_answers_for_every_key_that_is_not_a_character() {
        // The list every field in the window shares. It is asserted as a set rather than one
        // key at a time because the failure it guards against is a key going missing from one
        // copy of it — the library's search box shipped with no backspace at all.
        for key in ["backspace", "delete", "left", "right", "home", "end"] {
            let mut field = TextField::new("abc");
            assert_ne!(
                field.apply_key(key, false, false),
                KeyEffect::Ignored,
                "{key} went unanswered"
            );
        }
        let mut field = TextField::new("abc");
        assert_ne!(field.apply_key("a", false, true), KeyEffect::Ignored);
        // And nothing else, so a caller can still answer Escape, Return and the arrows itself.
        for key in ["escape", "enter", "up", "down", "a"] {
            let mut field = TextField::new("abc");
            assert_eq!(field.apply_key(key, false, false), KeyEffect::Ignored);
        }
    }

    #[test]
    fn backspace_takes_a_character_and_says_the_text_changed() {
        let mut field = TextField::new("ドラム");
        field.move_end(false);
        assert_eq!(
            field.apply_key("backspace", false, false),
            KeyEffect::Changed
        );
        assert_eq!(field.content(), "ドラ");
    }

    #[test]
    fn deletion_at_a_field_boundary_does_not_claim_the_text_changed() {
        let mut field = TextField::new("abc");
        field.move_home(false);
        assert_eq!(field.apply_key("backspace", false, false), KeyEffect::Moved);
        assert_eq!(field.content(), "abc");

        field.move_end(false);
        assert_eq!(field.apply_key("delete", false, false), KeyEffect::Moved);
        assert_eq!(field.content(), "abc");
    }

    #[test]
    fn home_and_end_stay_on_the_caret_s_line() {
        let mut field = TextField::new("ひらり\nはらり");
        field.caret_to_end();
        assert_eq!(field.selection(), 19..19);

        field.move_home(false);
        assert_eq!(
            field.selection(),
            10..10,
            "home of the second line, not of everything"
        );
        field.move_end(false);
        assert_eq!(field.selection(), 19..19);

        // And on a field with no newline, the line is the whole content — what Home and End
        // always did there.
        let mut field = TextField::new("Lead");
        field.caret_to_end();
        field.move_home(false);
        assert_eq!(field.selection(), 0..0);
    }

    #[test]
    fn up_and_down_walk_the_lines_keeping_the_column() {
        // Multi-byte on purpose: a column is characters, and these are three bytes each.
        let mut field = TextField::new("さくら\nちる\nまいちる");
        field.caret_to_end();
        // End of the last line is column 4; the line above has only 2.
        field.move_up(false);
        assert_eq!(field.selection(), 16..16, "clamped to ちる's end");
        field.move_up(false);
        assert_eq!(field.selection(), 6..6, "column 2 of さくら");
        field.move_up(false);
        assert_eq!(field.selection(), 0..0, "above the first line is its start");

        field.move_right(false);
        field.move_down(false);
        assert_eq!(field.selection(), 13..13, "column 1 of ちる");
        field.move_down(false);
        assert_eq!(field.selection(), 20..20, "column 1 of まいちる");
        field.move_down(false);
        field.move_down(false);
        assert_eq!(field.selection(), 29..29, "below the last line is its end");
    }

    #[test]
    fn shift_up_extends_the_selection_across_the_newline() {
        let mut field = TextField::new("ab\ncd");
        field.caret_to_end();
        field.move_up(true);
        assert_eq!(field.selection(), 2..5, "from ab's end to cd's end");
        assert!(field.is_reversed());
        field.insert("X");
        assert_eq!(field.content(), "abX");
    }

    #[test]
    fn moving_the_caret_is_not_reported_as_an_edit() {
        // What the palette turns on: it puts its highlight back on the first row when the query
        // changes, and pressing Home changes no query.
        let mut field = TextField::new("reverb");
        assert_eq!(field.apply_key("home", false, false), KeyEffect::Moved);
        assert_eq!(field.apply_key("a", false, true), KeyEffect::Moved);
        assert_eq!(field.content(), "reverb");
    }

    #[gpui::test]
    fn clipboard_copy_and_cut_use_the_unicode_selection(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut field = TextField::new("A歌B");
            field.place_caret(1, false);
            field.move_right(true);
            assert_eq!(
                field.apply_key_with_clipboard("c", false, true, false, cx),
                KeyEffect::Moved
            );
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("歌")
            );
            assert_eq!(field.content(), "A歌B");
            assert_eq!(
                field.apply_key_with_clipboard("x", false, true, false, cx),
                KeyEffect::Changed
            );
            assert_eq!(field.content(), "AB");
            assert_eq!(field.selection(), 1..1);
            assert_eq!(
                field.apply_key_with_clipboard("x", false, true, false, cx),
                KeyEffect::Moved
            );
            assert_eq!(
                field.content(),
                "AB",
                "cut with no selection must not backspace"
            );
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("歌")
            );
        });
    }

    #[gpui::test]
    fn clipboard_paste_normalizes_lines_and_reports_equal_length_replacements(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("a\r\nb\rc\nd".into()));
            let mut single = TextField::new("old");
            assert_eq!(
                single.apply_key_with_clipboard("v", false, true, false, cx),
                KeyEffect::Changed
            );
            assert_eq!(single.content(), "a b c d");
            let mut multi = TextField::new("old");
            multi.apply_key_with_clipboard("v", false, true, true, cx);
            assert_eq!(multi.content(), "a\nb\nc\nd");

            cx.write_to_clipboard(gpui::ClipboardItem::new_string("new".into()));
            let mut field = TextField::new("old");
            assert_eq!(
                field.apply_key_with_clipboard("v", false, true, false, cx),
                KeyEffect::Changed
            );
            assert_eq!(field.content(), "new");
            assert_eq!(
                field.apply_key_with_clipboard("v", false, false, false, cx),
                KeyEffect::Ignored
            );
        });
    }
}
