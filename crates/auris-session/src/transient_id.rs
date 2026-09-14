//! Short process-scoped references, never persistent document identities.
use base64::Engine;
use std::hash::{BuildHasher, Hasher};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

/// Issues a short opaque reference with a random process scope and a unique serial.
/// References must still be checked against the owning cache/project before use.
pub fn transient_id(prefix: &str) -> String {
    static SCOPE: OnceLock<String> = OnceLock::new();
    static SERIAL: AtomicU64 = AtomicU64::new(1);
    let scope = SCOPE.get_or_init(|| {
        let nonce = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce.to_be_bytes())
    });
    format!(
        "{prefix}:{scope}:{:x}",
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
