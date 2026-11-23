//! Small values kept under the configuration directory between runs, one file per key.
//!
//! The sound catalog keeps two kinds here: what a plugin file lists (its instruments and
//! presets) and what measuring a sound found. Both are expensive — a plugin has to be loaded in
//! the worker process, and a sound played nine times — and both are fully determined by files
//! whose metadata the key carries, so a key that still matches is a result that still holds.
//!
//! One file per key, rather than one file for the lot, because a library of plugins can hold
//! thousands of measured presets: reading the few a search needs should not mean parsing all
//! of them. Failures are ignored throughout; the cache only ever saves time.

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Files one kind keeps; past this the oldest are removed when another is written.
const FILE_LIMIT: usize = 16_384;

#[derive(Serialize, serde::Deserialize)]
struct Entry<T> {
    key: String,
    value: T,
}

fn directory(kind: &str) -> Option<PathBuf> {
    // A unit test must never write into the developer's configuration.
    (!cfg!(test)).then(|| crate::config_dir().join("cache").join(kind))
}

fn file(kind: &str, key: &str) -> Option<PathBuf> {
    Some(directory(kind)?.join(format!("{}.json", hex::encode(Sha256::digest(key)))))
}

/// The value stored under `key`, if any. A file whose recorded key differs is a hash collision
/// or a damaged file, and is ignored.
pub(super) fn read<T: DeserializeOwned>(kind: &str, key: &str) -> Option<T> {
    let bytes = std::fs::read(file(kind, key)?).ok()?;
    let entry: Entry<T> = serde_json::from_slice(&bytes).ok()?;
    (entry.key == key).then_some(entry.value)
}

/// Stores `value` under `key`, replacing what was there.
pub(super) fn write<T: Serialize>(kind: &str, key: &str, value: &T) {
    let Some(path) = file(kind, key) else { return };
    let Ok(bytes) = serde_json::to_vec(&Entry {
        key: key.to_owned(),
        value,
    }) else {
        return;
    };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, bytes).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
    // Listing a directory of thousands of files is not free; look now and then.
    static WRITES: AtomicUsize = AtomicUsize::new(0);
    if WRITES.fetch_add(1, Ordering::Relaxed).is_multiple_of(256) {
        prune(parent);
    }
}

/// Removes the oldest files once there are more than [`FILE_LIMIT`].
fn prune(directory: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    if files.len() <= FILE_LIMIT {
        return;
    }
    files.sort();
    for (_, path) in files.drain(..files.len() - FILE_LIMIT) {
        let _ = std::fs::remove_file(path);
    }
}
