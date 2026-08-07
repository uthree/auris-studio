//! Score-based automatic composition: a text specification in, notes on a timeline out.

#![warn(missing_docs)]

pub mod frame;
pub mod gm;
pub mod melodic;
pub mod parts;
pub mod phrase;
pub mod preset;
pub mod render;
pub mod rhythm;
pub mod rng;
pub mod spec;

/// Music theory, re-exported from where the document model can also reach it.
///
/// It moved down to [`auris_core`] so a [`Project`](auris_core::Project) could hold a key and a
/// chord progression of its own, and nothing at that level may depend on this crate. The paths
/// the composer writes — `crate::theory::chart` and the rest — are unchanged, because a
/// re-export at the crate root is nameable through `crate::`.
#[doc(no_inline)]
pub use auris_core::theory;

pub use phrase::{default_instrument, roles_of, write_phrase};
pub use preset::{PRESETS, SongPreset, preset};
pub use render::{ClipDraft, Composition, TrackDraft, compose};
pub use spec::{Mood, PartSpec, Role, SectionSpec, SongSpec, SpecError};

/// File extension of a song specification, for a file-picker filter.
pub const SPEC_EXTENSION: &str = "asong";
