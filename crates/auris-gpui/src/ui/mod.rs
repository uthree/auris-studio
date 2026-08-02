//! Views that make up the DAW window.
//!
//! Every module here adds `impl` blocks to [`crate::app::AurisApp`] rather than defining its own
//! gpui entity: the panels all read the same project, selection and engine handle, and one owner
//! of that state is simpler than synchronising several.

pub mod arrangement;
pub mod commands;
pub mod context_menu;
pub mod icons;
pub mod inspector;
pub mod menu_bar;
pub mod mixer;
pub mod paint;
pub mod piano_roll;
pub mod plugin_editor;
pub mod prompt;
pub mod root;
pub mod selection;
pub mod text_field;
pub mod timeline;
pub mod transport_bar;
pub mod widgets;
