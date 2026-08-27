//! Score-based automatic composition: a text specification in, notes on a timeline out.

#![warn(missing_docs)]

pub mod analysis;
pub mod frame;
pub mod gm;
pub mod melodic;
pub mod parts;
pub mod phrase;
pub mod preset;
pub mod progression;
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

pub use analysis::{Reading, detect_key, harmonise, motif_of, read_melody};
pub use phrase::{
    SEED_RANGE, clip_seed, default_instrument, preset_of, recipe_for, roles_of, write_phrase,
};
pub use preset::{PRESETS, SongPreset, preset};
pub use render::{ClipDraft, Composition, TrackDraft, compose};
pub use spec::{Ending, Mood, PartSpec, Role, SectionSpec, SongSpec, SpecError};

/// File extension of a song specification, for a file-picker filter.
pub const SPEC_EXTENSION: &str = "asong";
