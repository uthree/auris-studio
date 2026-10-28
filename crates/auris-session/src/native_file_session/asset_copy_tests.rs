use super::copy_file_noclobber;
use tempfile::TempDir;

struct FailingReader {
    remaining: usize,
}

impl std::io::Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("injected source read failure"));
        }
        let count = self.remaining.min(buffer.len());
        buffer[..count].fill(0x42);
        self.remaining -= count;
        Ok(count)
    }
}

#[test]
fn copy_file_publishes_complete_bytes_and_leaves_no_temporary_file() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source.bin");
    let destination = temp.path().join("nested").join("asset.bin");
    let bytes = vec![0x5a; 131_071];
    std::fs::write(&source, &bytes).unwrap();

    copy_file_noclobber(&source, &destination).unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    assert_eq!(
        std::fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn copy_file_accepts_an_identical_destination_but_rejects_different_bytes() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source.bin");
    let destination = temp.path().join("asset.bin");
    std::fs::write(&source, b"same").unwrap();
    std::fs::write(&destination, b"same").unwrap();

    copy_file_noclobber(&source, &destination).unwrap();
    std::fs::write(&destination, b"different").unwrap();

    let error = copy_file_noclobber(&source, &destination).unwrap_err();
    assert!(error.to_string().contains("destination changed"));
}

#[test]
fn failed_reader_does_not_leave_a_partial_destination_or_temporary_file() {
    let temp = TempDir::new().unwrap();
    let destination = temp.path().join("asset.bin");
    let reader = FailingReader { remaining: 70_000 };

    let error = super::copy_reader_noclobber(reader, temp.path(), &destination).unwrap_err();

    assert!(error.to_string().contains("injected source read failure"));
    assert!(!destination.exists());
    assert_eq!(temp.path().read_dir().unwrap().count(), 0);
}
