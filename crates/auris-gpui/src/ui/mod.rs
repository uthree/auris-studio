//! Views that make up the DAW window.
//!
//! The panel modules add `impl` blocks to [`crate::app::AurisApp`] rather than defining their own
//! gpui entities: one owner is simpler than synchronising the shared project and engine state.
//! [`tooltip`] is the intentional standalone view needed by gpui's tooltip API.

pub mod agent_chat;
pub mod analyser;
pub mod arrangement;
pub mod automation;
pub mod commands;
pub mod compose_sheet;
pub mod context_menu;
pub mod drop;
pub mod envelope;
pub mod icons;
pub mod inspector;
pub mod library;
pub mod log_panel;
pub mod menu_bar;
pub mod mixer;
pub mod paint;
pub mod palette;
pub mod part;
pub mod performance;
pub mod piano_roll;
pub mod plugin_editor;
pub mod plugin_window;
pub mod prompt;
pub mod root;
pub mod scrollbars;
pub mod selection;
pub mod singer;
pub mod status_bar;
pub mod text_area;
pub mod text_field;
pub mod timeline;
pub mod tooltip;
pub mod transport_bar;
pub mod typing_panel;
pub mod widgets;
