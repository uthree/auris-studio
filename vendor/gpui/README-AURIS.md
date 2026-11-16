# Auris GPUI fork

This directory contains `gpui` 0.2.2 from crates.io, without its examples. The workspace's
`[patch.crates-io]` puts it under every crate that names GPUI, including `gpui-component` and
Atelier.

The first change, in `src/elements/text.rs`: text is never wrapped at a width narrower than its
font size. Such a width is answered as if no width were given, which is what the min-content
question already gets.

The layout engine (Taffy 0.9.0, which GPUI pins) sizes a shrinkable flex item by laying it out at
its minimum, and an item with `min_w_0()` has a minimum of zero, so text inside it is measured at a
width of zero. Japanese may break between any two characters, so wrapped there it measures one
character to a line and thousands of pixels tall. GPUI's text element then hands that measurement
back when it is asked for the same text's max-content size, and Taffy's own cache reuses results
whose width happens to match a later question. Which of those happens depended on the window's
width, so the symptom came and went:

* a status notice in the editor's agent panel drawn one character per line;
* the AI workshop's transcript thousands of pixels taller than what it held, scrolled to its
  bottom, where nothing was drawn.

The window test `a_japanese_notice_keeps_its_line_beside_a_message` reproduces the first against
the published crate. The second has a fix of its own in `auris-gpui`, which gives the transcript
column a width in pixels so that no flex layout measures it on the way
(`the_transcript_ends_at_its_last_entry_at_every_width`); this change keeps the same trap from
springing on text elsewhere.

A second change in the same file: a text cut short with an ellipsis is cut from its whole runs at
every measurement. The published crate shortened the runs it had captured in place, so the
next, wider measurement of the same element laid out the whole text with runs that ended where the
earlier one had cut — three bytes past it, for the ellipsis. In Japanese that is inside a
character, and DirectWrite, which slices a line at run boundaries, panicked on Windows (`end byte
index 63 is not a char boundary`) while an agent was editing a song. Core Text takes no such
slice, so on macOS the same runs only styled the wrong span.

It takes a text that both wraps and truncates (`line_clamp` with `text_ellipsis`) measured at two
widths in one frame, narrower first. Taffy's cache seldom asks that, and no window test could be
made to, so `src/text_system.rs` adds a check instead: in a debug build `shape_text` panics, on
every platform, when a run ends inside a character, and the message names the text. A run that
is wrong for any other reason then fails on a Mac as well, and says where it came from.

`Cargo.toml` asks for `core-foundation = "0.10"` where the published crate pins `=0.10.0`
exactly, and `vendor/gpui_media` carries the same one-line change. The pin is what kept MIDI input
out: `midir` reaches CoreMIDI through `coremidi` 0.9, which wants `core-foundation ^0.10.1`, and
an exact pin admits no other version anywhere in the graph. 0.10.1 is a patch release of the same
API, and nothing else in either crate changes.

`src/gpui.rs` also allows warnings. From crates.io the crate was built under Cargo's
`--cap-lints`; as a path dependency it would be held to CI's `-D warnings`, and Rust 1.100 already
warns about two float literals in `src/taffy.rs`.

Remove the fork once an upstream release measures text in a way that does not depend on the order
of the questions asked about it.
