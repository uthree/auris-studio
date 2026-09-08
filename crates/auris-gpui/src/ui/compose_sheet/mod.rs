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
//! * `lyrics` — every section's words beside the basic controls, one a live multi-line editor.
//!   Shared melodies show the phrase counts each later verse must match.
//! * `matrix` — part participation against the song's section order, with fixed sound pickers.
//! * `pad` — paired controls for the song's mood and harmonic/rhythmic character.
//!
//! Everything `dials` makes public is re-exported here, so the rest of the crate goes on writing
//! `compose_sheet::SongDials` exactly as it did when this was one file.

#[cfg(test)]
mod density_tests;
mod dials;
mod lyrics;
mod matrix;
mod menus;
mod pad;
#[cfg(test)]
mod part_role_tests;
mod singers;
#[cfg(test)]
mod tempo_tests;
mod view;

pub use dials::*;
pub use lyrics::LyricsEdit;
