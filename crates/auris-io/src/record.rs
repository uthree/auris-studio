//! A WAV file that grows while it is being written.
//!
//! [`export`](crate::export) writes a buffer that already exists; this writes one that does not
//! exist yet, block by block, for as long as somebody holds a note. The difference is not the
//! format but the *ownership of the end*: an export knows how many frames it is about to write
//! and a take does not, so the header is only correct once [`WavRecorder::finish`] has run.
//!
//! # Why 32-bit float, always
//!
//! There is no bit depth to choose because there is no choice worth offering. The samples arrive
//! as `f32` and every integer depth is a quantisation of them, which means picking one is picking
//! how much of the take to throw away before anybody has heard it. Float also cannot clip: a
//! singer who leant in on the last chorus is recoverable rather than square. The cost is that a
//! take is a third larger than 24-bit, which is a disk, and disks are cheap next to a lost take.
//!
//! # What a crash costs
//!
//! The frame count in a WAV header is written when the file is closed, so a take whose
//! application died is a file whose header says nothing was recorded. The samples are all there
//! on disk; recovering them means repairing the header, which nothing here does yet. Worth
//! knowing before trusting an hour of it to a first pass.

use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::error::{IoError, Result};

/// A WAV file being recorded into.
///
/// Samples are handed over interleaved, in whatever the device's channel order is, exactly as
/// they arrive: this writes them down and counts them, and does not care what they mean.
pub struct WavRecorder {
    writer: Option<WavWriter<std::io::BufWriter<std::fs::File>>>,
    path: PathBuf,
    channels: usize,
    /// Samples written, which is frames times channels. Kept as samples so that a block that
    /// ends mid-frame — which should not happen, but a driver is free to be strange — is still
    /// counted honestly.
    samples: u64,
}

impl WavRecorder {
    /// Creates `path` and writes the header for a take at `sample_rate` with `channels` channels.
    ///
    /// The file appears at its final name immediately rather than being written to a scratch file
    /// and renamed, unlike every other writer in this crate. A recording has nothing at its
    /// target to protect — the name is new — and a take in progress that a user can see the size
    /// of is worth more than one that appears at the end.
    pub fn create(path: &Path, sample_rate: f64, channels: usize) -> Result<Self> {
        let channel_count = u16::try_from(channels)
            .ok()
            .filter(|count| *count > 0)
            .ok_or_else(|| {
                IoError::WavWrite(format!("{channels} channels is not something to record"))
            })?;
        let rate = sample_rate.round();
        if !(rate.is_finite() && rate > 0.0 && rate <= f64::from(u32::MAX)) {
            return Err(IoError::WavWrite(format!(
                "{sample_rate} is not a sample rate to record at"
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| IoError::from_fs(parent, error))?;
        }
        let spec = WavSpec {
            channels: channel_count,
            sample_rate: rate as u32,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let writer = WavWriter::create(path, spec).map_err(|error| wav_error(path, error))?;
        Ok(Self {
            writer: Some(writer),
            path: path.to_path_buf(),
            channels,
            samples: 0,
        })
    }

    /// Appends one block of interleaved samples.
    ///
    /// A sample that is not finite is written as silence. A driver that produces one is already
    /// broken, and an infinity in a take would go on to poison every mix it was ever dropped into.
    pub fn write(&mut self, block: &[f32]) -> Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(IoError::WavWrite(format!(
                "{} has already been closed",
                self.path.display()
            )));
        };
        for sample in block {
            let sample = if sample.is_finite() { *sample } else { 0.0 };
            writer
                .write_sample(sample)
                .map_err(|error| wav_error(&self.path, error))?;
        }
        self.samples += block.len() as u64;
        Ok(())
    }

    /// Frames written so far, which is how long the take is.
    pub fn frames(&self) -> u64 {
        self.samples / self.channels.max(1) as u64
    }

    /// Channels the take is being recorded with.
    pub fn channel_count(&self) -> usize {
        self.channels
    }

    /// Where the take is being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Closes the file, writing the header that says how long it turned out to be.
    ///
    /// Returns the frame count. Until this has run the file on disk claims to be empty — see the
    /// module note.
    pub fn finish(mut self) -> Result<u64> {
        let frames = self.frames();
        match self.writer.take() {
            Some(writer) => writer
                .finalize()
                .map_err(|error| wav_error(&self.path, error))?,
            None => {
                return Err(IoError::WavWrite(format!(
                    "{} has already been closed",
                    self.path.display()
                )));
            }
        }
        Ok(frames)
    }
}

impl std::fmt::Debug for WavRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WavRecorder")
            .field("path", &self.path)
            .field("channels", &self.channels)
            .field("frames", &self.frames())
            .field("open", &self.writer.is_some())
            .finish()
    }
}

fn wav_error(path: &Path, error: hound::Error) -> IoError {
    match error {
        hound::Error::IoError(source) => IoError::from_fs(path, source),
        other => IoError::WavWrite(format!("{}: {other}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempFile;

    fn read_back(path: &Path) -> (hound::WavSpec, Vec<f32>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples = reader.samples::<f32>().map(Result::unwrap).collect();
        (spec, samples)
    }

    #[test]
    fn a_take_reads_back_as_the_samples_that_went_into_it() {
        let file = TempFile::new("take.wav");
        let mut recorder = WavRecorder::create(file.path(), 48_000.0, 2).unwrap();
        recorder.write(&[0.0, 0.5]).unwrap();
        recorder.write(&[-0.5, 1.0, 0.25, -1.0]).unwrap();
        assert_eq!(recorder.frames(), 3);
        assert_eq!(recorder.finish().unwrap(), 3);

        let (spec, samples) = read_back(file.path());
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 32);
        assert_eq!(spec.sample_format, SampleFormat::Float);
        assert_eq!(samples, vec![0.0, 0.5, -0.5, 1.0, 0.25, -1.0]);
    }

    #[test]
    fn a_take_that_went_past_full_scale_is_kept_rather_than_squared_off() {
        // The whole reason the depth is not a choice. An integer format would have clipped these
        // to the rail and there would be nothing left to pull back down afterwards.
        let file = TempFile::new("hot.wav");
        let mut recorder = WavRecorder::create(file.path(), 44_100.0, 1).unwrap();
        recorder.write(&[1.8, -2.4]).unwrap();
        recorder.finish().unwrap();

        let (_, samples) = read_back(file.path());
        assert_eq!(samples, vec![1.8, -2.4]);
    }

    #[test]
    fn a_sample_that_is_not_a_number_is_written_as_silence() {
        // An infinity from a broken driver would otherwise go on to poison every mix this take
        // was ever dropped into, and the meters of all of them.
        let file = TempFile::new("nan.wav");
        let mut recorder = WavRecorder::create(file.path(), 48_000.0, 1).unwrap();
        recorder.write(&[f32::NAN, f32::INFINITY, 0.5]).unwrap();
        recorder.finish().unwrap();

        let (_, samples) = read_back(file.path());
        assert_eq!(samples, vec![0.0, 0.0, 0.5]);
    }

    #[test]
    fn a_recorder_refuses_what_it_could_not_write_down_honestly() {
        let file = TempFile::new("bad.wav");
        assert!(WavRecorder::create(file.path(), 48_000.0, 0).is_err());
        assert!(WavRecorder::create(file.path(), 0.0, 2).is_err());
        assert!(WavRecorder::create(file.path(), f64::NAN, 2).is_err());
        assert!(WavRecorder::create(file.path(), -48_000.0, 2).is_err());
    }

    #[test]
    fn the_folder_a_take_goes_in_is_made_if_it_is_not_there() {
        // The project's `Audio/` directory does not exist until something is put in it, and the
        // first thing put in it should not have to be an import.
        let file = TempFile::new("nested");
        let path = file.path().join("Audio").join("take.wav");
        let mut recorder = WavRecorder::create(&path, 48_000.0, 1).unwrap();
        recorder.write(&[0.25]).unwrap();
        assert_eq!(recorder.finish().unwrap(), 1);
        assert!(path.exists());
    }
}
