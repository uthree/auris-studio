//! Views that make up the DAW window.
//!
//! The panel modules add `impl` blocks to [`crate::app::AurisApp`] rather than defining their own
//! gpui entities: one owner is simpler than synchronising the shared project and engine state.
//! [`tooltip`] is the intentional standalone view needed by gpui's tooltip API.

pub mod agent_chat;
mod agent_markdown;
pub mod analyser;
pub mod arrangement;
pub mod automation;
pub mod commands;
mod compose_job;
pub(crate) mod compose_progress;
pub mod compose_sheet;
pub mod context_menu;
mod convert;
pub mod drop;
pub mod drum_assignments;
pub mod drum_editor;
pub mod drums;
pub mod envelope;
pub(crate) mod expression;
pub mod icons;
pub mod inspector;
pub mod library;
pub mod log_panel;
pub mod menu_bar;
pub mod mixer;
pub(crate) mod music_analysis;
pub mod paint;
pub mod palette;
pub mod part;
pub mod performance;
pub mod piano_roll;
pub(crate) mod pitch_performance;
pub mod plugin_editor;
pub mod plugin_window;
mod portrait_image;
pub mod prompt;
pub(crate) mod reference_match;
pub(crate) mod rhythm_grid;
pub mod root;
pub(crate) mod score_layer;
pub mod scrollbars;
pub mod selection;
pub mod singer;
pub(crate) mod singer_portrait;
pub(crate) mod song_search;
pub(crate) mod spectrogram;
pub mod status_bar;
pub(crate) mod strum;
pub mod text_area;
pub mod text_field;
pub(crate) mod timbre_map;
pub mod timeline;
pub mod title_bar;
pub mod tooltip;
pub mod transport_bar;
pub mod typing_panel;
pub(crate) mod visualizer;
pub mod widgets;
