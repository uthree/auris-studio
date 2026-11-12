//! The one ONNX Runtime environment in the process.
//!
//! Since 2.0.0-rc.14, `ort` builds every session under an explicit [`ort::environment::Environment`], and
//! [`ort::init`] succeeds only once per process: a second build is refused even after the first
//! environment has been dropped. Two crates open sessions — `auris-singer` for voice models and
//! `auris-analysis` for instrument tagging — and the desktop application links both, so neither
//! may build its own. This crate is the single owner they share.
//!
//! The environment is built on first use rather than at start-up, so a session that never loads
//! a model never loads the runtime. It is kept in a static that is never dropped: tearing ONNX
//! Runtime down while the process exits is what `ort` warns against, and a value that is never
//! dropped is never torn down.
#![warn(missing_docs)]

use std::sync::Mutex;

static ENVIRONMENT: Mutex<Option<ort::environment::Environment>> = Mutex::new(None);

/// The process's ONNX Runtime environment, built the first time anything asks for it.
///
/// A failed build is not remembered, so a later call tries again — the runtime library may have
/// been missing the first time and present the next.
///
/// # Errors
///
/// Whatever ONNX Runtime reports when it cannot create the environment.
pub fn environment() -> ort::Result<ort::environment::Environment> {
    let mut slot = ENVIRONMENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(environment) = slot.as_ref() {
        return Ok(environment.clone());
    }
    let environment = ort::init().with_name("auris").build()?;
    *slot = Some(environment.clone());
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_built_once_and_handed_out_again() {
        environment().expect("the runtime builds an environment");
        environment().expect("a second caller reuses it instead of building again");
        assert!(
            ort::init().build().is_err(),
            "ort refuses a second environment, which is why this crate exists"
        );
    }
}
