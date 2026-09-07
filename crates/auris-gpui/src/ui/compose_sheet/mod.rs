//! The song sheet: a whole piece asked for with dials rather than with a file.
//!
//! Pure state transformations live in `dials`; the window harness checks keyboard and pointer
//! gestures through the rendered sheet.
//!
//! * `dials` — the state the sheet holds, every rule about what a dial or a gesture means, and the
//!   tests. Names no toolkit, so a rule that grew a condition can be checked by hand.
//! * `view` — the responsive panel, its instrument roster and write/save actions.
//! * `menus` — the catalogues turned into pickers. Neither an element nor a rule: a list the
//!   composer publishes, in the shape a context menu takes.
//! * `lyrics` — the third column: every section's words, one of them a live multi-line editor.
//!   Shared melodies show the phrase counts each later verse must match.
//! * `pad` — paired controls for the song's mood and harmonic/rhythmic character.
//!
//! Everything `dials` makes public is re-exported here, so the rest of the crate goes on writing
//! `compose_sheet::SongDials` exactly as it did when this was one file.

mod dials;
mod lyrics;
mod menus;
mod pad;
mod singers;
mod view;

pub use dials::*;
pub use lyrics::LyricsEdit;
