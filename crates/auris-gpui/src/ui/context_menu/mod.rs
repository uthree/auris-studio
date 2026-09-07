//! Right-click menus: what each component offers, where the menu lands, and what a choice does.
//!
//! Items carry a [`MenuCommand`] rather than a closure. A menu is then plain data — it can be
//! built where the component knows what was clicked, placed and drawn somewhere else entirely,
//! and checked by a test without a window.
//!
//! # Where things are
//!
//! Four layers stacked on that one idea, a file each, and the stack only ever leans downwards.
//!
//! * `menu` is the widget: what a row is, the builder that adds rows, how large the result comes
//!   out and how it is drawn. It knows nothing about what any of its rows mean.
//! * `command` is the vocabulary and the dispatcher, [`MenuCommand`] beside its one exhaustive
//!   match, kept in one file because the compiler is what holds those two together.
//! * `tracks`, `timeline` and `clips` are the builders, split by what a menu acts on rather than
//!   by which panel opens it: a track and its strip, the lanes that run along the song, and a
//!   clip with the notes inside it. None of the three calls the others.
//! * `recipe` is the dials of a generated clip, which is document work wearing a menu.
//!
//! The menu widget and command vocabulary are re-exported here.

mod clips;
mod command;
mod menu;
mod recipe;
mod timeline;
mod tracks;

pub use command::MenuCommand;
pub use menu::ContextMenu;
#[cfg(test)]
pub use menu::MenuEntry;

pub(crate) use recipe::{preset_key, subdivision_key};
pub(crate) use timeline::count_in_label;

#[cfg(test)]
use auris_session::prelude::SignatureMap;

/// 4/4 for the whole timeline, which is what the bar arithmetic in the tests is counted in.
///
/// Here rather than in either test module because `timeline` and `recipe` both count bars off it.
#[cfg(test)]
fn meters() -> SignatureMap {
    SignatureMap::default()
}
