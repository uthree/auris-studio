//! Views that make up the DAW window.
//!
//! The panel modules add `impl` blocks to [`crate::app::AurisApp`] rather than defining their own
//! gpui entities: one owner is simpler than synchronising the shared project and engine state.
//! [`tooltip`] owns GPUI tooltip lifetimes. [`reference_piano_roll`] hosts the same note editor
//! on an independent session, keeping conversational references outside the song.
//! Self-contained controls such as [`reasoning_picker`] own transient focus and presentation
//! state, while conversation preferences and the document stay with the application.

pub mod agent_chat;
pub(crate) mod agent_components;
mod agent_markdown;
pub mod analyser;
pub mod arrangement;
pub(crate) mod attachment_preview;
pub(crate) mod audio_player;
pub mod automation;
pub(crate) mod chat_commands;
pub mod commands;
pub mod context_menu;
mod convert;
#[cfg(test)]
mod design_language_tests;
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
pub(crate) mod mixer_graph;
pub(crate) mod music_analysis;
pub(crate) mod note_snap;
pub mod paint;
pub mod palette;
pub mod part;
pub mod performance;
pub(crate) mod physical_model;
pub mod piano_roll;
pub(crate) mod pitch_performance;
pub mod plugin_editor;
pub mod plugin_window;
mod portrait_image;
pub(crate) mod project_chooser;
pub mod prompt;
pub(crate) mod provider_logo;
pub(crate) mod reasoning_picker;
pub(crate) mod reference_piano_roll;
pub(crate) mod rhythm_grid;
pub mod root;
pub(crate) mod score_layer;
pub mod scrollbars;
pub mod selection;
pub mod singer;
pub(crate) mod singer_portrait;
pub(crate) mod song_workshop;
pub(crate) mod spectrogram;
pub mod status_bar;
pub(crate) mod strum;
pub mod text_area;
pub mod text_field;
pub(crate) mod timbre_map;
pub mod timeline;
pub mod title_bar;
pub mod tooltip;
pub(crate) mod track_grid;
pub mod transport_bar;
pub mod typing_panel;
pub(crate) mod updates;
pub(crate) mod visualizer;
pub(crate) mod volume_contour;
pub mod widgets;
