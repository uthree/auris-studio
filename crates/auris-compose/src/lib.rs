//! Phrase-level automatic composition: one part written over a stretch of a document's harmony.
//!
//! [`write_phrase`] is the door: a [`ClipRecipe`](auris_core::ClipRecipe), a range and the chords
//! already under it in, the notes of one clip out. Around it sit the pieces a document's own
//! editing reaches for — reading a melody back into harmony ([`analysis`]), suggesting chords
//! under a line ([`harmony_suggestions`]), making room for a foreground line
//! ([`arrangement::accompany_foreground`]) and writing a sung line from lyrics ([`vocal`]).

#![warn(missing_docs)]

pub mod analysis;
pub mod arrangement;
pub mod frame;
pub mod gm;
pub mod harmony_suggestions;
pub mod melodic;
pub mod parts;
pub mod perform;
pub mod phrase;
pub mod phrasing;
pub mod rhythm;
pub mod spec;
pub mod vocal;

#[cfg(test)]
mod fixture;

/// Music theory, re-exported from where the document model can also reach it.
///
/// It lives in [`auris_core`] so a [`Project`](auris_core::Project) can hold a key and a chord
/// progression of its own, and nothing at that level may depend on this crate. The paths the
/// writers use — `crate::theory::chart` and the rest — resolve through this re-export.
#[doc(no_inline)]
pub use auris_core::theory;

/// Deterministic randomness, re-exported from where the document model can also reach it.
///
/// It lives in [`auris_core`] for the reason [`theory`] does: a clip's note transforms draw their
/// wander from the same named streams the writers draw from.
#[doc(no_inline)]
pub use auris_core::rng;

/// Musical vocabulary a recipe names, re-exported from the document model that stores it.
#[doc(no_inline)]
pub use auris_core::PerformanceStyle;

pub use analysis::{Reading, detect_key, harmonise, motif_of, read_melody};
pub use perform::{clip_performance, part_performance};
pub use phrase::{apply_drum_map, parse_motif, preset_of, roles_of, write_phrase};
pub use spec::{Mood, PartSpec, Role};
pub use vocal::{VocalRange, VocalRhythm, ornament_vocal, vocal_rhythm, write_vocal};
