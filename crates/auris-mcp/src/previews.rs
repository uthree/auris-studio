//! Bounded, connection-local audio resources. No caller-supplied URI is opened as a file.

use base64::Engine as _;
use rmcp::model::{ErrorData, Resource, ResourceContents};
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Debug, Default)]
pub(crate) struct Previews {
    next: u64,
    entries: VecDeque<(Resource, Arc<[u8]>)>,
}

impl Previews {
    pub(crate) fn insert(&mut self, bytes: Vec<u8>) -> Result<Resource, ErrorData> {
        if bytes.len() > 12_000_000
            || !bytes.starts_with(b"RIFF")
            || bytes.get(8..12) != Some(b"WAVE")
        {
            return Err(ErrorData::internal_error(
                "preview is not a bounded WAV",
                None,
            ));
        }
        self.next += 1;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .as_nanos();
        let resource = Resource::new(
            format!(
                "auris-preview://audio/{}-{stamp}-{}",
                std::process::id(),
                self.next
            ),
            format!("Audition {}", self.next),
        )
        .with_mime_type("audio/wav")
        .with_size(bytes.len() as u64)
        .with_description(
            "WAV preview; available for this connection until eight newer previews replace it.",
        );
        self.entries.push_back((resource.clone(), bytes.into()));
        while self.entries.len() > 8 {
            self.entries.pop_front();
        }
        Ok(resource)
    }
    pub(crate) fn list(&self) -> Vec<Resource> {
        self.entries
            .iter()
            .map(|(resource, _)| resource.clone())
            .collect()
    }
    pub(crate) fn bytes(&self, uri: &str) -> Result<Arc<[u8]>, ErrorData> {
        self.entries
            .iter()
            .find(|(resource, _)| resource.uri == uri)
            .map(|(_, bytes)| Arc::clone(bytes))
            .ok_or_else(|| {
                ErrorData::resource_not_found(
                    "unknown or expired preview; call preview again",
                    None,
                )
            })
    }
    pub(crate) fn content(uri: String, bytes: &[u8]) -> ResourceContents {
        ResourceContents::BlobResourceContents {
            uri,
            mime_type: Some("audio/wav".into()),
            blob: base64::engine::general_purpose::STANDARD.encode(bytes),
            meta: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_are_bounded_immutable_and_cannot_open_paths() {
        let mut previews = Previews::default();
        let wav = b"RIFF0000WAVEpayload".to_vec();
        let first = previews.insert(wav.clone()).unwrap();
        assert_eq!(previews.bytes(&first.uri).unwrap().as_ref(), wav);
        assert!(previews.bytes("file:///private.wav").is_err());
        for _ in 0..8 {
            previews.insert(wav.clone()).unwrap();
        }
        assert_eq!(previews.list().len(), 8);
        assert!(previews.bytes(&first.uri).is_err());
        let mut reconnected = Previews::default();
        assert_ne!(reconnected.insert(wav).unwrap().uri, first.uri);
        assert!(reconnected.bytes(&first.uri).is_err());
        assert!(previews.insert(b"not audio".to_vec()).is_err());
    }
}
