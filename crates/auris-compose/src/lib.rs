//! Score-based automatic composition: a text specification in, notes on a timeline out.

#![warn(missing_docs)]

pub mod frame;
pub mod parts;
pub mod render;
pub mod rhythm;
pub mod rng;
pub mod spec;
pub mod theory;

pub use render::{ClipDraft, Composition, TrackDraft, compose};
pub use spec::{Mood, PartSpec, Role, SectionSpec, SongSpec, SpecError};
