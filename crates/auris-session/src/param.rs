//! Addressing a parameter wherever it lives.
//!
//! The types moved down to [`auris_core::param`] when the document gained automation: a lane is a
//! [`ParamTarget`] and a shape, and the document model may not name a crate above it. This module
//! re-exports them so `auris_session::param::ParamTarget` still resolves — the same arrangement
//! `auris-compose` has with `auris_core::theory`, and for the same reason.

pub use auris_core::param::{
    ParamDescriptor, ParamId, ParamTarget, ParamUnit, ParamValueCurve, db_to_gain, gain_to_db,
};
