//! Score-based automatic composition: a text specification in, notes on a timeline out.

#![warn(missing_docs)]

pub mod analysis;
pub mod frame;
pub mod gm;
pub mod melodic;
pub mod metrics;
pub mod parts;
pub mod perform;
pub mod phrase;
pub mod preset;
pub mod progression;
pub mod render;
pub mod rhythm;
pub mod spec;
pub mod vocal;

/// Music theory, re-exported from where the document model can also reach it.
///
/// It moved down to [`auris_core`] so a [`Project`](auris_core::Project) could hold a key and a
/// chord progression of its own, and nothing at that level may depend on this crate. The paths
/// the composer writes — `crate::theory::chart` and the rest — are unchanged, because a
/// re-export at the crate root is nameable through `crate::`.
#[doc(no_inline)]
pub use auris_core::theory;

/// Deterministic randomness, re-exported from where the document model can also reach it.
///
/// It moved down to [`auris_core`] for the reason [`theory`] did: a clip's note transforms draw
/// their wander from the same named streams the composer draws from, and nothing at that level
/// may depend on this crate. The paths the composer writes — `crate::rng::Rng` and
/// `crate::rng::Key` — are unchanged, because a re-export at the crate root is nameable
/// through `crate::`.
#[doc(no_inline)]
pub use auris_core::rng;

pub use analysis::{Reading, detect_key, harmonise, motif_of, read_melody};
pub use metrics::{pitch_class_entropy, syncopation};
pub use perform::{clip_performance, part_performance};
pub use phrase::{
    SEED_RANGE, clip_seed, default_instrument, preset_of, recipe_for, roles_of, write_phrase,
};
pub use preset::{PRESETS, SongPreset, preset};
pub use render::{ClipDraft, Composition, EffectDraft, TrackDraft, compose};
pub use spec::{Ending, Mood, PartSpec, Role, SectionSpec, SongSpec, SpecError};
pub use vocal::{VocalRange, VocalRhythm, vocal_rhythm, write_vocal};

/// File extension of a song specification, for a file-picker filter.
pub const SPEC_EXTENSION: &str = "asong";
