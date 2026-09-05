//! Optional voice-card artwork, read independently of the inference runtime.

use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;

use crate::{METADATA_KEY, SingError};

/// Largest encoded image accepted from a voice card, matching the trainer's export limit.
pub const PORTRAIT_MAX_BYTES: usize = 8 * 1024 * 1024;

// Base64 adds one third; the remaining room allows the phoneme tables and descriptive card.
// ONNX tensor data is skipped with seeks, and no metadata allocation can exceed this limit.
const METADATA_MAX_BYTES: u64 = 16 * 1024 * 1024;
const PORTRAIT_MAX_BASE64: usize = PORTRAIT_MAX_BYTES.div_ceil(3) * 4;

/// Shared encoded PNG, JPEG or WebP artwork from a voice's presentation metadata.
///
/// These are image-file bytes, not pixels. A frontend decodes them off its UI thread, with its
/// own pixel-dimension limit; a corrupt image must never prevent the voice from singing.
#[derive(Clone, PartialEq, Eq)]
pub struct VoicePortrait {
    mime: &'static str,
    bytes: Arc<[u8]>,
}

impl std::fmt::Debug for VoicePortrait {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VoicePortrait")
            .field("mime", &self.mime)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl VoicePortrait {
    /// Accepts supported, nonempty artwork within the exporter's size limit.
    ///
    /// Image decoding is the frontend's responsibility. Invalid optional artwork returns
    /// `None`, so it can be omitted without affecting synthesis or speaker selection.
    pub fn from_bytes(mime: &str, bytes: Vec<u8>) -> Option<Self> {
        let mime = supported_mime(mime)?;
        if bytes.is_empty() || bytes.len() > PORTRAIT_MAX_BYTES {
            return None;
        }
        Some(Self {
            mime,
            bytes: bytes.into(),
        })
    }

    /// Decodes the trainer's `voice.portrait` payload, or an equivalent API image.
    ///
    /// The encoded length is checked before allocating decoded bytes. Oversized, empty,
    /// malformed base64 and unsupported MIME types are all treated as absent artwork.
    pub fn from_base64(mime: &str, encoded: &str) -> Option<Self> {
        supported_mime(mime)?;
        if encoded.is_empty() || encoded.len() > PORTRAIT_MAX_BASE64 {
            return None;
        }
        Self::from_bytes(mime, STANDARD.decode(encoded).ok()?)
    }

    /// The image's declared MIME type: `image/png`, `image/jpeg` or `image/webp`.
    pub fn mime(&self) -> &'static str {
        self.mime
    }

    /// Shared image-file bytes; cloning the `Arc` does not copy the image.
    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
}

fn supported_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

/// Reads artwork embedded in an Auris ONNX model without loading its tensors or runtime.
///
/// Call this on a worker thread. The graph is skipped with file seeks and metadata reads are
/// bounded. A missing, malformed or unsupported portrait returns `None`; file I/O and broken
/// protobuf framing return an error. Only embedded metadata is used, just as during synthesis,
/// so a stale JSON sidecar cannot supply another export's artwork.
pub fn read_voice_portrait(path: &Path) -> Result<Option<VoicePortrait>, SingError> {
    let load_error = |error: io::Error| SingError::Load {
        reason: format!(
            "could not read voice artwork from {}: {error}",
            path.display()
        ),
    };
    let file = File::open(path).map_err(load_error)?;
    let length = file.metadata().map_err(load_error)?.len();
    read_embedded_portrait(&mut BufReader::new(file), length).map_err(load_error)
}

fn read_embedded_portrait(
    reader: &mut (impl Read + Seek),
    length: u64,
) -> io::Result<Option<VoicePortrait>> {
    // ONNX ModelProto.metadata_props is field 14. Each StringStringEntryProto contains
    // key = 1 and value = 2; no knowledge of the inference graph's schema is needed here.
    while let Some((field, wire, range)) = next_field(reader, length)? {
        if field != 14 || wire != 2 || range.end - range.start > METADATA_MAX_BYTES {
            continue;
        }
        let mut entry = vec![0; (range.end - range.start) as usize];
        reader.seek(SeekFrom::Start(range.start))?;
        reader.read_exact(&mut entry)?;
        if let Some(raw) = metadata_entry(&entry)? {
            return Ok(portrait_from_json(raw));
        }
    }
    Ok(None)
}

fn metadata_entry(entry: &[u8]) -> io::Result<Option<&[u8]>> {
    let mut reader = Cursor::new(entry);
    let mut key = None;
    let mut value = None;
    while let Some((field, wire, range)) = next_field(&mut reader, entry.len() as u64)? {
        if wire == 2 {
            let bytes = &entry[range.start as usize..range.end as usize];
            match field {
                1 => key = Some(bytes),
                2 => value = Some(bytes),
                _ => {}
            }
        }
    }
    Ok((key == Some(METADATA_KEY.as_bytes()))
        .then_some(value)
        .flatten())
}

fn portrait_from_json(raw: &[u8]) -> Option<VoicePortrait> {
    #[derive(Deserialize)]
    struct Metadata<'a> {
        #[serde(borrow)]
        voice: Card<'a>,
    }
    #[derive(Deserialize)]
    struct Card<'a> {
        #[serde(borrow)]
        portrait: Portrait<'a>,
    }
    #[derive(Deserialize)]
    struct Portrait<'a> {
        mime: std::borrow::Cow<'a, str>,
        #[serde(borrow)]
        base64: std::borrow::Cow<'a, str>,
    }

    // Failure here only omits artwork. VoiceInfo deliberately ignores the portrait field,
    // so a malformed image cannot make otherwise valid synthesis metadata unreadable.
    let metadata: Metadata<'_> = serde_json::from_slice(raw).ok()?;
    VoicePortrait::from_base64(
        &metadata.voice.portrait.mime,
        &metadata.voice.portrait.base64,
    )
}

/// Returns a field's byte range while leaving the reader at the next field.
fn next_field(
    reader: &mut (impl Read + Seek),
    end: u64,
) -> io::Result<Option<(u64, u8, Range<u64>)>> {
    if reader.stream_position()? == end {
        return Ok(None);
    }
    let key = read_varint(reader, end)?;
    let field = key >> 3;
    let wire = (key & 7) as u8;
    if field == 0 || field >= (1 << 29) {
        return Err(invalid_protobuf());
    }
    let length = match wire {
        0 => {
            read_varint(reader, end)?;
            0
        }
        1 => 8,
        2 => read_varint(reader, end)?,
        5 => 4,
        _ => return Err(invalid_protobuf()),
    };
    let start = reader.stream_position()?;
    let next = start
        .checked_add(length)
        .filter(|next| *next <= end)
        .ok_or_else(invalid_protobuf)?;
    reader.seek(SeekFrom::Start(next))?;
    Ok(Some((field, wire, start..next)))
}

fn read_varint(reader: &mut (impl Read + Seek), end: u64) -> io::Result<u64> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        if reader.stream_position()? >= end {
            return Err(invalid_protobuf());
        }
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        if shift == 63 && byte[0] > 1 {
            return Err(invalid_protobuf());
        }
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid_protobuf())
}

fn invalid_protobuf() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid ONNX metadata framing")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        while value >= 128 {
            bytes.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }

    fn field(number: u64, bytes: &[u8]) -> Vec<u8> {
        let mut result = varint(number << 3 | 2);
        result.extend(varint(bytes.len() as u64));
        result.extend(bytes);
        result
    }

    fn metadata(key: &str, value: &[u8]) -> Vec<u8> {
        // Protobuf fields need not be in numerical order.
        let mut entry = field(2, value);
        entry.extend(field(1, key.as_bytes()));
        field(14, &entry)
    }

    const CARD: &[u8] = br#"{"voice":{"portrait":{"mime":"image/png","base64":"AQIDBA=="}}}"#;

    #[test]
    fn artwork_matches_the_export_contract_and_clones_share_bytes() {
        for mime in ["image/png", "image/jpeg", "image/webp"] {
            let portrait = VoicePortrait::from_base64(mime, "AQIDBA==").unwrap();
            assert_eq!(portrait.mime(), mime);
            assert_eq!(portrait.bytes().as_ref(), &[1, 2, 3, 4]);
            assert!(Arc::ptr_eq(portrait.bytes(), portrait.clone().bytes()));
        }
        for (mime, encoded) in [
            ("image/svg+xml", "AQIDBA=="),
            ("image/png", ""),
            ("image/png", "not base64"),
            ("image/png", "AQIDBA"),
        ] {
            assert!(VoicePortrait::from_base64(mime, encoded).is_none());
        }
    }

    #[test]
    fn artwork_size_is_bounded_before_and_after_base64_decoding() {
        let bytes = vec![42; PORTRAIT_MAX_BYTES];
        let encoded = STANDARD.encode(&bytes);
        assert_eq!(
            VoicePortrait::from_base64("image/png", &encoded)
                .unwrap()
                .bytes()
                .len(),
            PORTRAIT_MAX_BYTES
        );
        // One extra decoded byte can still have the same encoded length because of padding.
        let oversized = STANDARD.encode(vec![42; PORTRAIT_MAX_BYTES + 1]);
        assert_eq!(encoded.len(), oversized.len());
        assert!(VoicePortrait::from_base64("image/png", &oversized).is_none());
        assert!(
            VoicePortrait::from_base64("image/png", &"A".repeat(oversized.len() + 4)).is_none()
        );
        assert!(VoicePortrait::from_bytes("image/png", Vec::new()).is_none());
    }

    #[test]
    fn absent_and_broken_optional_artwork_are_omitted() {
        for raw in [
            "{}",
            r#"{"voice":{}}"#,
            r#"{"voice":{"portrait":null}}"#,
            r#"{"voice":{"portrait":"broken"}}"#,
            r#"{"voice":{"portrait":{"mime":"image/png","base64":42}}}"#,
            r#"{"voice":{"portrait":{"mime":"image/gif","base64":"AQID"}}}"#,
            r#"{"voice":{"portrait":{"mime":"image/png","base64":"!"}}}"#,
        ] {
            assert!(portrait_from_json(raw.as_bytes()).is_none(), "{raw}");
        }
        // JSON escapes are legal even though the exporter normally emits base64 verbatim.
        assert!(
            portrait_from_json(
                br#"{"voice":{"portrait":{"mime":"image/png","base64":"\u0041QIDBA=="}}}"#
            )
            .is_some()
        );
    }

    #[test]
    fn embedded_artwork_is_read_without_reading_graph_bytes() {
        struct MeasuredRead {
            cursor: Cursor<Vec<u8>>,
            bytes_read: usize,
        }
        impl Read for MeasuredRead {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let count = self.cursor.read(buffer)?;
                self.bytes_read += count;
                Ok(count)
            }
        }
        impl Seek for MeasuredRead {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.cursor.seek(position)
            }
        }
        let mut onnx = vec![8, 9]; // ir_version, a varint field.
        onnx.extend(field(7, &vec![0xff; 1_000_000]));
        onnx.extend(metadata("unrelated", b"not JSON"));
        onnx.extend(metadata(METADATA_KEY, CARD));
        let length = onnx.len() as u64;
        let mut reader = MeasuredRead {
            cursor: Cursor::new(onnx),
            bytes_read: 0,
        };
        let portrait = read_embedded_portrait(&mut reader, length)
            .unwrap()
            .unwrap();
        assert_eq!(portrait.bytes().as_ref(), &[1, 2, 3, 4]);
        assert!(reader.bytes_read < 200, "{} bytes read", reader.bytes_read);
    }

    #[test]
    fn protobuf_lengths_are_validated_before_allocation_or_seeking() {
        for bytes in [
            vec![0],
            vec![0x80],
            vec![0x80; 10],
            vec![0x72, 10, 1], // A metadata field extending beyond EOF.
            vec![
                0x72, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1,
            ],
            vec![0x09, 1], // A truncated fixed64 field.
        ] {
            let length = bytes.len() as u64;
            assert!(read_embedded_portrait(&mut Cursor::new(bytes), length).is_err());
        }
        let onnx = field(7, &[1, 2, 3]);
        assert!(
            read_embedded_portrait(&mut Cursor::new(&onnx), onnx.len() as u64)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn oversized_metadata_is_skipped_without_allocating_it() {
        let mut onnx = field(14, &vec![0; METADATA_MAX_BYTES as usize + 1]);
        onnx.extend(metadata(METADATA_KEY, CARD));
        assert!(
            read_embedded_portrait(&mut Cursor::new(&onnx), onnx.len() as u64)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn optional_portrait_errors_do_not_reject_synthesis_metadata() {
        let raw = r#"{"format_version":2,"sample_rate":48000,"hop_length":480,
            "inter_channels":192,"symbols":["<unk>","<sil>"],
            "voice":{"name":"Singer","portrait":{"mime":false,"base64":42}}}"#;
        assert_eq!(
            crate::VoiceInfo::parse(raw).unwrap().display_name(),
            "Singer"
        );
        assert!(portrait_from_json(raw.as_bytes()).is_none());
    }
}
