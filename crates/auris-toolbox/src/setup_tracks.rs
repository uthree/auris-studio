//! Saved-project transport for atomic setup, with bounded successful-retry receipts.
use super::*;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// Wire name.
pub const NAME: &str = "setup_tracks";
/// Model-facing contract.
pub const DESCRIPTION: &str = "Add 1..16 tracks with optional sound_id and empty clip in one atomic save. Use a unique request_id; identical retries return the same result while the document is unchanged (last 32 receipts, this server lifetime). Existing track names are rejected. On conflict, inspect the project before a new request.";
/// One atomic setup request.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Project path or project_id.
    pub project: String,
    /// Caller-chosen retry key, 1..64 characters; reuse only for identical requests.
    #[schemars(length(min = 1, max = 64))]
    pub request_id: String,
    /// Tracks to create, 1..16. No existing track is overwritten.
    #[schemars(length(min = 1, max = 16))]
    pub tracks: Vec<auris_session::session::SetupTrack>,
}
struct Receipt {
    path: PathBuf,
    key: String,
    arguments: String,
    fingerprint: u64,
    response: String,
}
static RECEIPTS: OnceLock<Mutex<Vec<Receipt>>> = OnceLock::new();
fn fingerprint(path: &Path) -> Result<u64, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    Ok(hash.finish())
}
/// Saves only after the entire session command succeeds.
pub fn run(args: &Args) -> Result<String, String> {
    if args.request_id.trim().is_empty() || args.request_id.chars().count() > 64 {
        return Err("request_id must contain 1..64 characters".into());
    }
    let path = resolve_project(&args.project)?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let arguments = serde_json::to_string(&args.tracks).map_err(|e| e.to_string())?;
    let mut receipts = RECEIPTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Setup receipt lock failed")?;
    if let Some(receipt) = receipts
        .iter()
        .find(|r| r.path == path && r.key == args.request_id)
    {
        if receipt.arguments != arguments {
            return Err(
                "request_id already used with different tracks; use a new request_id".into(),
            );
        }
        if receipt.fingerprint != fingerprint(&path)? {
            return Err(
                "Project changed since setup; inspect it before issuing a new request".into(),
            );
        }
        return Ok(receipt.response.clone());
    }
    let mut session = opened(path.to_str().ok_or("Project path is not Unicode")?)?;
    let response = session.setup_tracks(&args.tracks, &[])?;
    session.save_with_checkpoint().map_err(|e| e.to_string())?;
    if receipts.len() == 32 {
        receipts.remove(0);
    }
    receipts.push(Receipt {
        path: path.clone(),
        key: args.request_id.clone(),
        arguments,
        fingerprint: fingerprint(&path)?,
        response: response.clone(),
    });
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_setup_never_saves_and_successful_retries_do_not_duplicate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Setup.auris");
        let mut session = headless().unwrap();
        session.save(&path).unwrap();
        drop(session);
        let mut args: Args = serde_json::from_value(serde_json::json!({
            "project":path,"request_id":"setup-1","tracks":[
                {"name":"Lead","kind":"instrument","clip":{"name":"Phrase","start_bar":1,"bars":32}},
                {"name":"Kit","kind":"drum","sound_id":"s:expired:1"}
            ]
        })).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(run(&args).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        args.tracks[1].sound_id = None;
        let first = run(&args).unwrap();
        let saved = std::fs::read(&path).unwrap();
        args.project = project_handles::register(&path).unwrap();
        assert_eq!(run(&args).unwrap(), first);
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        args.tracks[0].name = "Different".into();
        assert!(run(&args).unwrap_err().contains("different tracks"));
        args.tracks[0].name = "Lead".into();
        let mut session = opened(&args.project).unwrap();
        session.add_audio_track("Later edit");
        session.save_with_checkpoint().unwrap();
        assert!(run(&args).unwrap_err().contains("Project changed"));
    }
}
