# Auris gpui_media fork

`gpui_media` 0.2.2 from crates.io, with one change: `Cargo.toml` asks for
`core-foundation = "0.10"` instead of `=0.10.0`. `vendor/gpui/README-AURIS.md` has the reason —
the exact pin left no version that MIDI input's `coremidi` could share.

Remove the fork together with the GPUI one.
