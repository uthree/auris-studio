//! Explicit bounded references to saved projects; there is no implicit current project.
use super::*;
use std::sync::{Mutex, OnceLock};

static HANDLES: OnceLock<Mutex<Vec<(String, PathBuf)>>> = OnceLock::new();

pub(crate) fn register(path: &Path) -> Result<String, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let mut entries = HANDLES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Project handle lock failed")?;
    if let Some((id, _)) = entries.iter().find(|(_, existing)| *existing == path) {
        return Ok(id.clone());
    }
    let id = auris_session::transient_id::transient_id("p");
    if entries.len() == 64 {
        entries.remove(0);
    }
    entries.push((id.clone(), path));
    Ok(id)
}

pub(crate) fn resolve(id: &str) -> Result<PathBuf, String> {
    HANDLES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Project handle lock failed")?
        .iter()
        .find(|(key, _)| key == id)
        .map(|(_, path)| path.clone())
        .ok_or_else(|| {
            "Unknown or expired project handle; call open_project with the absolute path".into()
        })
}

/// Opens a saved project and returns an explicit short reference.
pub mod open_project {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "open_project";
    /// Model-facing contract.
    pub const DESCRIPTION: &str = "Open a saved project and return project_id for subsequent project arguments. Handles expire on server restart or after 64 distinct projects. Does not change files.";
    /// Saved document to inspect.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute .auris path, or an existing project handle.
        pub project: String,
    }
    /// Validates the document before publishing its reference.
    pub fn run(args: &Args) -> Result<String, String> {
        let path = resolve_project(&args.project)?;
        let _session = opened(&args.project)?;
        Ok(serde_json::json!({"project_id":register(&path)?,"project":path}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handles_are_explicit_and_never_fall_back_to_paths() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.auris");
        let b = root.path().join("b.auris");
        std::fs::write(&a, []).unwrap();
        std::fs::write(&b, []).unwrap();
        let id = register(&a).unwrap();
        assert_eq!(register(&a).unwrap(), id);
        assert_ne!(register(&b).unwrap(), id);
        assert_eq!(resolve_project(&id).unwrap(), a.canonicalize().unwrap());
        assert!(
            resolve_project("p:stale:1")
                .unwrap_err()
                .contains("open_project")
        );
    }
}
