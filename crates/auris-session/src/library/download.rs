//! Fetching missing shipped fonts into the user's library, away from the audio thread.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use super::{
    LIBRARY_DIR_VAR, LIBRARY_FOLDER, SHIPPED, ShippedFont, config_dir, installed_in, library_roots,
};

/// Set this environment variable to `0` to disable automatic SoundFont downloads.
pub const FETCH_SOUNDFONTS_VAR: &str = "AURIS_FETCH_SOUNDFONTS";

const NOTICE_LIMIT: u64 = 1024 * 1024;

/// A missing shipped font and the writable library directory to install it in.
#[derive(Clone, Debug)]
pub struct FontDownload {
    /// The manifest entry used to download and verify the font.
    pub font: &'static ShippedFont,
    /// The environment override, or the SoundFonts directory under the user's configuration.
    pub directory: PathBuf,
}

/// Plans downloads for missing fonts without doing network I/O or creating directories.
///
/// Existing files retain the library's manual replacement policy. Setting
/// [`FETCH_SOUNDFONTS_VAR`] to `0` returns an empty plan.
pub fn font_downloads() -> Vec<FontDownload> {
    downloads_from(
        std::env::var_os(FETCH_SOUNDFONTS_VAR).as_deref(),
        std::env::var_os(LIBRARY_DIR_VAR).map(PathBuf::from),
        &config_dir(),
        &library_roots(),
    )
}

fn downloads_from(
    fetch_setting: Option<&OsStr>,
    override_directory: Option<PathBuf>,
    configuration: &Path,
    search_roots: &[PathBuf],
) -> Vec<FontDownload> {
    if fetch_setting.is_some_and(|value| value == "0") {
        return Vec::new();
    }
    let directory = override_directory
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| configuration.join(LIBRARY_FOLDER));
    SHIPPED
        .iter()
        .filter(|font| installed_in(font, search_roots).is_none())
        .map(|font| FontDownload {
            font,
            directory: directory.clone(),
        })
        .collect()
}

/// Why a shipped font could not be downloaded or installed.
#[derive(Debug, thiserror::Error)]
pub enum FontDownloadError {
    /// An HTTP request failed or returned an unsuccessful status.
    #[error("SoundFont download failed: {0}")]
    Request(String),
    /// Reading the response or writing the library failed.
    #[error("SoundFont download I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The response had a different length from the manifest.
    #[error("SoundFont length mismatch: expected {expected} bytes, received {actual}")]
    Length {
        /// The manifest's exact byte count.
        expected: u64,
        /// The received byte count, bounded to one byte beyond the expected size.
        actual: u64,
    },
    /// The downloaded bytes did not match the manifest's SHA-256 digest.
    #[error("SoundFont SHA-256 mismatch: expected {expected}, received {actual}")]
    Digest {
        /// The manifest's hexadecimal digest.
        expected: String,
        /// The downloaded file's hexadecimal digest.
        actual: String,
    },
    /// The license response exceeded the bounded notice size.
    #[error("SoundFont license notice exceeds {NOTICE_LIMIT} bytes")]
    NoticeTooLarge,
}

/// Downloads and verifies a font, reporting the cumulative number of font bytes received.
///
/// Call from a worker thread. Both requests have finite connection, read and overall timeouts.
/// The license notice is installed before fetching the font. The font is streamed into a
/// temporary file in the destination directory and published only after its length and SHA-256
/// agree with the manifest. Failures remove the temporary file. An existing font is returned
/// without any request, and a concurrent installer or manual replacement is never overwritten.
pub fn download_font(
    font: &ShippedFont,
    directory: &Path,
    mut progress: impl FnMut(u64),
) -> Result<PathBuf, FontDownloadError> {
    let target = directory.join(font.file);
    if target.is_file() {
        return Ok(target);
    }
    fs::create_dir_all(directory)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build();

    let stem = Path::new(font.file)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let notice = directory.join(format!("{stem}_License.md"));
    let mut notice_file = NamedTempFile::new_in(directory)?;
    let received = io::copy(
        &mut response(&agent, font.license_url)?.take(NOTICE_LIMIT + 1),
        &mut notice_file,
    )?;
    if received > NOTICE_LIMIT {
        return Err(FontDownloadError::NoticeTooLarge);
    }
    publish(notice_file, &notice)?;

    // Another instance might have finished while this one fetched the notice.
    if target.is_file() {
        return Ok(target);
    }
    let mut file = NamedTempFile::new_in(directory)?;
    let mut reader = response(&agent, font.url)?.take(font.bytes.saturating_add(1));
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    progress(0);
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        received += read as u64;
        if received > font.bytes {
            return Err(FontDownloadError::Length {
                expected: font.bytes,
                actual: received,
            });
        }
        file.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
        progress(received);
    }
    if received != font.bytes {
        return Err(FontDownloadError::Length {
            expected: font.bytes,
            actual: received,
        });
    }
    let actual = format!("{:x}", digest.finalize());
    if actual != font.sha256 {
        return Err(FontDownloadError::Digest {
            expected: font.sha256.to_owned(),
            actual,
        });
    }
    publish(file, &target)?;
    Ok(target)
}

fn response(
    agent: &ureq::Agent,
    url: &str,
) -> Result<Box<dyn Read + Send + Sync>, FontDownloadError> {
    let response = agent
        .get(url)
        .call()
        .map_err(|error| FontDownloadError::Request(error.to_string()))?;
    if !(200..300).contains(&response.status()) {
        return Err(FontDownloadError::Request(format!(
            "HTTP status {}",
            response.status()
        )));
    }
    Ok(response.into_reader())
}

fn publish(mut file: NamedTempFile, target: &Path) -> Result<(), FontDownloadError> {
    file.flush()?;
    match file.persist_noclobber(target) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists && target.is_file() => {
            Ok(())
        }
        Err(error) => Err(FontDownloadError::Io(error.error)),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    use super::*;

    const DATA: &[u8] = b"a small stand-in for the pinned SoundFont bytes";
    const NOTICE: &[u8] = b"MIT license fixture";

    struct Server {
        root: String,
        thread: Option<JoinHandle<()>>,
    }

    impl Server {
        fn new(replies: Vec<Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let root = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let thread = thread::spawn(move || {
                for reply in replies {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                assert!(
                                    Instant::now() < deadline,
                                    "download did not request fixture"
                                );
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("fixture accept: {error}"),
                        }
                    };
                    // Windows can inherit the listener's nonblocking mode on accepted sockets.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        stream.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                        assert!(request.len() < 16 * 1024);
                    }
                    // An oversized response may be rejected before the fixture finishes writing.
                    let _ = stream.write_all(&reply);
                }
            });
            Self {
                root,
                thread: Some(thread),
            }
        }

        fn font(&self) -> ShippedFont {
            ShippedFont {
                id: "fixture",
                file: "fixture.sf2",
                name: "Fixture",
                license: "MIT",
                license_url: Box::leak(format!("{}/license", self.root).into_boxed_str()),
                url: Box::leak(format!("{}/font", self.root).into_boxed_str()),
                bytes: DATA.len() as u64,
                sha256: Box::leak(format!("{:x}", Sha256::digest(DATA)).into_boxed_str()),
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let result = self.thread.take().unwrap().join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }

    fn reply(body: &[u8]) -> Vec<u8> {
        let mut reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        reply.extend_from_slice(body);
        reply
    }

    fn assert_only_notice(directory: &Path) {
        let files: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["fixture_License.md"]);
    }

    #[test]
    fn opting_out_leaves_missing_fonts_unplanned() {
        let directory = tempfile::tempdir().unwrap();
        let plans = downloads_from(Some(OsStr::new("0")), None, directory.path(), &[]);
        assert!(plans.is_empty());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn automatic_downloads_use_the_shared_configuration_cache() {
        let directory = tempfile::tempdir().unwrap();
        for override_directory in [None, Some(PathBuf::new())] {
            let plans = downloads_from(None, override_directory, directory.path(), &[]);
            assert_eq!(plans.len(), SHIPPED.len());
            assert!(
                plans
                    .iter()
                    .all(|plan| { plan.directory == directory.path().join(LIBRARY_FOLDER) })
            );
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn explicit_library_override_is_the_download_destination() {
        let directory = tempfile::tempdir().unwrap();
        let override_directory = directory.path().join("external-library");
        let plans = downloads_from(
            None,
            Some(override_directory.clone()),
            &directory.path().join("configuration"),
            &[],
        );
        assert_eq!(plans.len(), SHIPPED.len());
        assert!(
            plans
                .iter()
                .all(|plan| plan.directory == override_directory)
        );
        assert!(!override_directory.exists());
    }

    #[test]
    fn fonts_found_beside_an_executable_do_not_get_downloaded_again() {
        let directory = tempfile::tempdir().unwrap();
        let packaged = directory.path().join("packaged");
        fs::create_dir(&packaged).unwrap();
        for font in SHIPPED {
            fs::write(packaged.join(font.file), b"manual or packaged font").unwrap();
        }
        let plans = downloads_from(
            None,
            None,
            &directory.path().join("configuration"),
            &[packaged],
        );
        assert!(plans.is_empty());
        assert!(!directory.path().join("configuration").exists());
    }

    #[test]
    fn installs_verified_font_with_notice_and_progress() {
        let server = Server::new(vec![reply(NOTICE), reply(DATA)]);
        let directory = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let path = download_font(&server.font(), directory.path(), |bytes| {
            progress.push(bytes)
        })
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), DATA);
        assert_eq!(
            fs::read(directory.path().join("fixture_License.md")).unwrap(),
            NOTICE
        );
        assert_eq!(progress.first(), Some(&0));
        assert_eq!(progress.last(), Some(&(DATA.len() as u64)));
        assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn existing_manual_font_is_reused_without_any_network() {
        let directory = tempfile::tempdir().unwrap();
        let mut font = SHIPPED[0];
        font.url = "not a URL";
        font.license_url = "not a URL";
        let path = directory.path().join(font.file);
        fs::write(&path, b"manual replacement").unwrap();
        assert_eq!(
            download_font(&font, directory.path(), |_| panic!("no download")).unwrap(),
            path
        );
        assert_eq!(fs::read(path).unwrap(), b"manual replacement");
    }

    #[test]
    fn wrong_size_and_hash_leave_no_font_or_temporary_file() {
        for bytes in [
            &DATA[..DATA.len() - 1],
            &[b'x'; DATA.len()][..],
            &[b'x'; DATA.len() + 1][..],
        ] {
            let server = Server::new(vec![reply(NOTICE), reply(bytes)]);
            let directory = tempfile::tempdir().unwrap();
            let error = download_font(&server.font(), directory.path(), |_| {}).unwrap_err();
            if bytes.len() == DATA.len() {
                assert!(matches!(error, FontDownloadError::Digest { .. }));
            } else {
                assert!(matches!(error, FontDownloadError::Length { .. }));
            }
            assert_only_notice(directory.path());
        }
    }

    #[test]
    fn truncated_http_response_leaves_no_font() {
        let mut truncated = reply(DATA);
        truncated.truncate(truncated.len() - 3);
        let server = Server::new(vec![reply(NOTICE), truncated]);
        let directory = tempfile::tempdir().unwrap();
        assert!(download_font(&server.font(), directory.path(), |_| {}).is_err());
        assert_only_notice(directory.path());
    }

    #[test]
    fn license_failure_prevents_the_font_request_and_installation() {
        let server = Server::new(vec![
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            download_font(&server.font(), directory.path(), |_| {}),
            Err(FontDownloadError::Request(_))
        ));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn oversized_license_is_rejected_and_its_temporary_file_removed() {
        let server = Server::new(vec![reply(&vec![b'x'; NOTICE_LIMIT as usize + 1])]);
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            download_font(&server.font(), directory.path(), |_| {}),
            Err(FontDownloadError::NoticeTooLarge)
        ));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn publishing_never_overwrites_a_concurrent_install() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("fixture.sf2");
        let mut temporary = NamedTempFile::new_in(directory.path()).unwrap();
        temporary.write_all(DATA).unwrap();
        fs::write(&target, b"concurrent manual replacement").unwrap();
        publish(temporary, &target).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"concurrent manual replacement");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
