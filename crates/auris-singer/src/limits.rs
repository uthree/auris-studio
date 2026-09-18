use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::{Component, Path, PathBuf, Prefix};

use crate::SingError;

pub(crate) const MIN_SAMPLE_RATE: u32 = 8_000;
pub(crate) const MAX_SAMPLE_RATE: u32 = 192_000;
pub(crate) const MIN_HOP_SECONDS: f64 = 0.001;
pub(crate) const MAX_HOP_SECONDS: f64 = 0.100;
pub(crate) const MAX_INTER_CHANNELS: u32 = 4_096;
pub(crate) const MAX_COLLECTION_ITEMS: usize = 4_096;
pub(crate) const MAX_FRAME_COUNT: usize = 4_000_000;
pub(crate) const MAX_TOKEN_BYTES: usize = 256;
pub(crate) const MAX_NAME_BYTES: usize = 1_024;
pub(crate) const MAX_PATH_BYTES: usize = 32 * 1_024;
pub(crate) const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_MEL_BINS: usize = 4_096;

/// The importer accepts at most 512 MiB of decoded mono f32 audio. Singing uses the same
/// ceiling so a generated take cannot bypass the project's ordinary audio-memory budget.
pub(crate) const MAX_OUTPUT_SAMPLES: usize = 134_217_728;

pub(crate) fn invalid_metadata(reason: impl Into<String>) -> SingError {
    SingError::Metadata(reason.into())
}

pub(crate) fn checked_product(
    left: usize,
    right: usize,
    resource: &'static str,
    limit: usize,
) -> Result<usize, SingError> {
    let count = left.checked_mul(right).ok_or(SingError::TooLarge {
        resource,
        observed: None,
        limit,
    })?;
    if count > limit {
        return Err(SingError::TooLarge {
            resource,
            observed: Some(count),
            limit,
        });
    }
    Ok(count)
}

pub(crate) fn checked_sample_count(
    frames: usize,
    hop: usize,
    resource: &'static str,
) -> Result<usize, SingError> {
    checked_product(frames, hop, resource, MAX_OUTPUT_SAMPLES)
}

pub(crate) fn try_zeroed_f32(count: usize, resource: &'static str) -> Result<Vec<f32>, SingError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| SingError::Allocation { resource })?;
    values.resize(count, 0.0);
    Ok(values)
}

pub(crate) fn try_copy_f32(values: &[f32], resource: &'static str) -> Result<Vec<f32>, SingError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(values.len())
        .map_err(|_| SingError::Allocation { resource })?;
    copy.extend_from_slice(values);
    Ok(copy)
}

pub(crate) fn try_copy_f64(values: &[f64], resource: &'static str) -> Result<Vec<f64>, SingError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(values.len())
        .map_err(|_| SingError::Allocation { resource })?;
    copy.extend_from_slice(values);
    Ok(copy)
}

pub(crate) fn try_f32_with(
    count: usize,
    resource: &'static str,
    mut value: impl FnMut() -> f32,
) -> Result<Vec<f32>, SingError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| SingError::Allocation { resource })?;
    values.extend((0..count).map(|_| value()));
    Ok(values)
}

pub(crate) fn read_text_file(path: &Path, resource: &'static str) -> Result<String, SingError> {
    read_text_file_with_limit(path, resource, MAX_TEXT_BYTES)
}

fn read_text_file_with_limit(
    path: &Path,
    resource: &'static str,
    limit: usize,
) -> Result<String, SingError> {
    let file = File::open(path).map_err(|error| SingError::Load {
        reason: format!("{}: {error}", path.display()),
    })?;
    let length = file.metadata().ok().map(|metadata| metadata.len());
    if length.is_some_and(|length| length > limit as u64) {
        return Err(SingError::TooLarge {
            resource,
            observed: length.and_then(|length| usize::try_from(length).ok()),
            limit,
        });
    }
    let mut bytes = Vec::new();
    let initial = length
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(limit);
    bytes
        .try_reserve_exact(initial)
        .map_err(|_| SingError::Allocation { resource })?;
    read_bounded(
        file.take(limit.saturating_add(1) as u64),
        &mut bytes,
        resource,
        limit,
    )?;
    String::from_utf8(bytes)
        .map_err(|error| invalid_metadata(format!("{resource} is not UTF-8: {error}")))
}

fn read_bounded(
    mut reader: Take<impl Read>,
    bytes: &mut Vec<u8>,
    resource: &'static str,
    limit: usize,
) -> Result<(), SingError> {
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = reader.read(&mut chunk).map_err(|error| SingError::Load {
            reason: format!("could not read {resource}: {error}"),
        })?;
        if count == 0 {
            break;
        }
        bytes
            .try_reserve_exact(count)
            .map_err(|_| SingError::Allocation { resource })?;
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() > limit {
        return Err(SingError::TooLarge {
            resource,
            observed: Some(bytes.len()),
            limit,
        });
    }
    Ok(())
}

pub(crate) fn bounded_bytes(
    mut reader: impl Read,
    limit: usize,
    resource: &'static str,
) -> Result<Vec<u8>, SingError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(limit.min(64 * 1024))
        .map_err(|_| SingError::Allocation { resource })?;
    let mut reader = reader.by_ref().take(limit.saturating_add(1) as u64);
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = reader
            .read(&mut chunk)
            .map_err(|error| SingError::Inference(format!("{resource}: {error}")))?;
        if count == 0 {
            break;
        }
        bytes
            .try_reserve_exact(count)
            .map_err(|_| SingError::Allocation { resource })?;
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() > limit {
        return Err(SingError::TooLarge {
            resource,
            observed: Some(bytes.len()),
            limit,
        });
    }
    Ok(bytes)
}

pub(crate) fn automatic_subpath_safe(path: &Path) -> bool {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return false;
    }

    // Parse Windows spellings on every host so a project cannot become unsafe merely by moving
    // from macOS to Windows. On Unix these are otherwise ordinary filename characters.
    let text = path.as_os_str().to_string_lossy();
    let bytes = text.as_bytes();
    let has_windows_parent = text.split(['/', '\\']).any(|component| component == "..");
    !text.starts_with("\\\\")
        && !text.starts_with("//")
        && !has_windows_parent
        && !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn automatic_entry_spelling_safe(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }

    let text = path.as_os_str().to_string_lossy();
    if text.starts_with("\\\\") || text.starts_with("//") {
        return false;
    }
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        return matches!(prefix.kind(), Prefix::Disk(_)) && path.is_absolute();
    }

    // On a non-Windows host, reject a Windows drive spelling rather than treating it as a local
    // filename. A project moved to Windows must not change the access policy of its entry path.
    let bytes = text.as_bytes();
    !(cfg!(not(windows)) && bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn unsafe_entry(reason: impl Into<String>) -> SingError {
    SingError::UnsafeAutomaticAccess {
        reason: reason.into(),
    }
}

/// Resolve an automatic voice entry without following a symlink or Windows reparse component.
///
/// This check deliberately fails closed for missing or inaccessible components and rejects direct
/// UNC and device namespace spellings before filesystem access. The returned absolute spelling is
/// used for the subsequent backend open, so a relative path cannot be redirected by a later current
/// directory change. Like every path-based check, it cannot prevent a privileged concurrent writer
/// from replacing a component after validation.
pub fn validate_automatic_voice_entry(path: &Path) -> Result<PathBuf, SingError> {
    if !automatic_entry_spelling_safe(path) {
        return Err(unsafe_entry(
            "the voice entry must use a local filesystem path",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| unsafe_entry(format!("could not resolve the voice entry: {error}")))?
            .join(path)
    };
    if !automatic_entry_spelling_safe(&absolute) {
        return Err(unsafe_entry(
            "the voice entry must use a local filesystem path",
        ));
    }

    let mut current = PathBuf::new();
    let mut entry_is_file = false;
    for component in absolute.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            unsafe_entry(format!(
                "could not inspect voice entry component {}: {error}",
                current.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(unsafe_entry(format!(
                "voice entry component {} is a filesystem redirect",
                current.display()
            )));
        }
        entry_is_file = metadata.is_file();
    }
    if !entry_is_file {
        return Err(unsafe_entry("the voice entry is not a regular file"));
    }
    Ok(absolute)
}

/// Resolve an automatic manifest child only when it is an existing descendant of `root`.
///
/// Automatic loaders use the returned canonical path for the subsequent read/open. This removes
/// the avoidable gap from checking one symlink spelling and then reopening that spelling, though
/// no path-based API can prevent a privileged concurrent writer from replacing a canonical path
/// after this function returns.
pub(crate) fn automatic_descendant_path(root: &Path, child: &Path) -> Option<PathBuf> {
    if !automatic_subpath_safe(child) {
        return None;
    }
    // Failure is deliberately fail-closed. A successful model load requires these files to exist
    // anyway, and treating an inaccessible path as safe would turn a policy check into a bypass.
    let canonical_root = root.canonicalize().ok()?;
    let canonical_child = canonical_root.join(child).canonicalize().ok()?;
    canonical_child
        .starts_with(&canonical_root)
        .then_some(canonical_child)
}

pub(crate) fn validate_audio_dimensions(
    sample_rate: u32,
    hop_length: u32,
    context: &str,
) -> Result<(), SingError> {
    let hop = f64::from(hop_length) / f64::from(sample_rate);
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
        || !(MIN_HOP_SECONDS..=MAX_HOP_SECONDS).contains(&hop)
    {
        return Err(invalid_metadata(format!(
            "{context} sample_rate must be {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE} Hz and its frame hop {MIN_HOP_SECONDS}..={MAX_HOP_SECONDS} seconds"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_temp_dir() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    // macOS exposes this directory through `/var`; resolve that system redirect so automatic
    // access tests reach the fixture redirects they create inside the temporary directory.
    if cfg!(target_os = "macos") {
        std::fs::canonicalize(temp_dir).expect("the system temp directory can be resolved")
    } else {
        temp_dir
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_file() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        test_temp_dir().join(format!(
            "auris-singer-limit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn bounded_reader_checks_the_actual_stream_not_a_length_hint() {
        let error = bounded_bytes(std::io::Cursor::new([0_u8; 9]), 8, "response")
            .expect_err("the ninth streamed byte exceeds the limit");
        assert!(matches!(
            error,
            SingError::TooLarge {
                resource: "response",
                observed: Some(9),
                limit: 8
            }
        ));
    }

    #[test]
    fn bounded_text_reader_accepts_the_limit_and_rejects_the_next_byte() {
        let path = temp_file();
        for (length, accepted) in [(7, true), (8, true), (9, false)] {
            std::fs::write(&path, vec![b'x'; length]).unwrap();
            assert_eq!(
                read_text_file_with_limit(&path, "config", 8).is_ok(),
                accepted,
                "length {length}"
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn automatic_paths_reject_cross_platform_escape_spellings() {
        for path in [
            "../model.onnx",
            r"..\model.onnx",
            "/model.onnx",
            r"C:\model.onnx",
            r"\\server\share\model.onnx",
            "//server/share/model.onnx",
        ] {
            assert!(!automatic_subpath_safe(Path::new(path)), "{path}");
        }
        assert!(automatic_subpath_safe(Path::new("models/acoustic.onnx")));
        assert!(automatic_subpath_safe(Path::new("./models/acoustic.onnx")));
    }

    #[test]
    fn automatic_entry_accepts_an_existing_regular_file() {
        let path = temp_file();
        std::fs::write(&path, b"voice").unwrap();

        let resolved = validate_automatic_voice_entry(&path).unwrap();

        std::fs::remove_file(&path).unwrap();
        assert_eq!(resolved, path);
    }

    #[test]
    fn automatic_entry_fails_closed_for_a_missing_file() {
        let path = temp_file();

        assert!(matches!(
            validate_automatic_voice_entry(&path),
            Err(SingError::UnsafeAutomaticAccess { .. })
        ));
    }

    #[test]
    fn automatic_entry_requires_a_regular_file() {
        let path = temp_file();
        std::fs::create_dir(&path).unwrap();

        let result = validate_automatic_voice_entry(&path);

        std::fs::remove_dir(path).unwrap();
        assert!(matches!(
            result,
            Err(SingError::UnsafeAutomaticAccess { .. })
        ));
    }

    #[test]
    fn automatic_entry_rejects_network_and_device_spellings_before_access() {
        for path in [
            r"\\server\share\voice.onnx",
            "//server/share/voice.onnx",
            r"\\?\C:\voice.onnx",
            r"\\.\PhysicalDrive0",
        ] {
            assert!(
                matches!(
                    validate_automatic_voice_entry(Path::new(path)),
                    Err(SingError::UnsafeAutomaticAccess { .. })
                ),
                "{path}"
            );
        }
    }

    #[test]
    fn automatic_descendants_fail_closed_when_root_or_child_cannot_be_resolved() {
        let root = temp_file();
        assert!(automatic_descendant_path(&root, Path::new("model.onnx")).is_none());
        std::fs::create_dir(&root).unwrap();
        assert!(automatic_descendant_path(&root, Path::new("model.onnx")).is_none());
        std::fs::remove_dir(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn automatic_descendants_reject_an_existing_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = temp_file();
        let outside = root.with_extension("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.join("model.onnx")).unwrap();
        assert!(automatic_descendant_path(&root, Path::new("model.onnx")).is_none());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn enormous_hop_and_channel_products_fail_without_allocating() {
        assert_eq!(
            checked_sample_count(MAX_OUTPUT_SAMPLES, 1, "audio").unwrap(),
            MAX_OUTPUT_SAMPLES
        );
        assert!(checked_sample_count(MAX_OUTPUT_SAMPLES + 1, 1, "audio").is_err());
        assert!(checked_sample_count(2, u32::MAX as usize, "audio").is_err());
        assert!(
            checked_product(
                1,
                u32::MAX as usize,
                "latent noise",
                MAX_INTER_CHANNELS as usize * crate::score::MAX_CHUNK_FRAMES
            )
            .is_err()
        );
    }
}
