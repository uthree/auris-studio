//! Views that make up the DAW window.
//!
//! Every module here adds `impl` blocks to [`crate::app::AurisApp`] rather than defining its own
//! gpui entity: the panels all read the same project, selection and engine handle, and one owner
//! of that state is simpler than synchronising several.

pub mod arrangement;
pub mod automation;
pub mod commands;
pub mod context_menu;
pub mod drop;
pub mod icons;
pub mod inspector;
pub mod library;
pub mod menu_bar;
pub mod mixer;
pub mod paint;
pub mod palette;
pub mod part;
pub mod piano_roll;
pub mod plugin_editor;
pub mod plugin_window;
pub mod prompt;
pub mod root;
pub mod selection;
pub mod status_bar;
pub mod text_field;
pub mod timeline;
pub mod transport_bar;
pub mod widgets;
