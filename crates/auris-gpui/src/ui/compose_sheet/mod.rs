//! The song sheet: a whole piece asked for with dials rather than with a file.
//!
//! Three files, cut along the line that decides everything else about this module: what the sheet
//! *decides* can be tested and what it *draws* cannot. Everything with a rule in it is on one side
//! of that line, and the tests are there with it.
//!
//! * `dials` — the state the sheet holds, every rule about what a dial or a gesture means, and the
//!   tests. Names no toolkit, so a rule that grew a condition can be checked by hand.
//! * `view` — the panel: three columns of elements over a strip of parts, and the drag, the
//!   write and the save its buttons hand back. Nothing here can be asserted on, which is why so
//!   little is here.
//! * `menus` — the catalogues turned into pickers. Neither an element nor a rule: a list the
//!   composer publishes, in the shape a context menu takes.
//! * `lyrics` — the third column: every section's words, one of them a live multi-line editor.
//!   Holds the one rule the column has (which sections appear, in what order) and its test.
//!
//! Everything `dials` makes public is re-exported here, so the rest of the crate goes on writing
//! `compose_sheet::SongDials` exactly as it did when this was one file.

mod dials;
mod lyrics;
mod menus;
mod view;

pub use dials::*;
pub use lyrics::LyricsEdit;
