//! Writing rendered audio out as WAV, FLAC or MP3.
//!
//! [`AudioExportWriter`] accepts bounded render blocks and keeps the destination private until
//! the codec is finalised. The buffer convenience functions use the same incremental encoder, so
//! progress and cancellation stay responsive without maintaining a second implementation.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use auris_core::AudioBuffer;
use hound::{SampleFormat, WavSpec, WavWriter};
use rusty_mp3::{Mp3Encode, header::FrameHeader};
use tempfile::NamedTempFile;

use crate::error::{IoError, Result};

/// Container and codec used for an audio export.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum AudioExportFormat {
    /// Uncompressed RIFF/WAVE.
    #[default]
    Wav,
    /// Lossless Free Lossless Audio Codec.
    Flac,
    /// Lossy MPEG-1 Audio Layer III.
    Mp3,
}

impl AudioExportFormat {
    /// Conventional lowercase file extension.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
        }
    }

    /// Whether Auris offers `sample_rate` for this export format.
    pub fn supports_sample_rate(self, sample_rate: u32) -> bool {
        match self {
            Self::Wav | Self::Flac => sample_rate > 0,
            Self::Mp3 => matches!(sample_rate, 32_000 | 44_100 | 48_000),
        }
    }
}

/// Constant bitrate used by the MP3 encoder.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Mp3Bitrate {
    /// 128 kilobits per second.
    Kbps128,
    /// 192 kilobits per second.
    Kbps192,
    /// 256 kilobits per second.
    #[default]
    Kbps256,
    /// 320 kilobits per second.
    Kbps320,
}

impl Mp3Bitrate {
    /// Numeric bitrate in kilobits per second.
    pub fn kbps(self) -> u32 {
        match self {
            Self::Kbps128 => 128,
            Self::Kbps192 => 192,
            Self::Kbps256 => 256,
            Self::Kbps320 => 320,
        }
    }
}

/// Sample format of an exported WAV or FLAC file.
///
/// [`Float32`](Self::Float32) is available only for WAV; FLAC uses either integer variant.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum WavBitDepth {
    /// 16-bit signed integer — CD quality, the safest choice for distribution.
    Int16,
    /// 24-bit signed integer — the studio default: no audible quantisation noise, two thirds
    /// the size of 32-bit float.
    #[default]
    Int24,
    /// 32-bit IEEE float — lossless with respect to the render, and the only depth that keeps
    /// samples above 0 dBFS intact for further processing.
    Float32,
}

impl WavBitDepth {
    /// Bits per stored sample.
    pub fn bits(self) -> u16 {
        match self {
            WavBitDepth::Int16 => 16,
            WavBitDepth::Int24 => 24,
            WavBitDepth::Float32 => 32,
        }
    }

    /// Label for the export dialog.
    pub fn label(self) -> &'static str {
        match self {
            WavBitDepth::Int16 => "16-bit integer",
            WavBitDepth::Int24 => "24-bit integer",
            WavBitDepth::Float32 => "32-bit float",
        }
    }

    /// `true` when the depth stores integers and therefore quantises.
    pub fn is_integer(self) -> bool {
        !matches!(self, WavBitDepth::Float32)
    }

    /// Multiplier that maps `1.0` onto one step past the positive full-scale code.
    ///
    /// Two's complement is asymmetric: 16-bit codes run from -32768 to +32767. Scaling by 2^15
    /// (rather than 32767) makes -1.0 land exactly on negative full scale and keeps the mapping
    /// a pure power of two, so a sample that came *from* a 16-bit file round-trips bit-exactly.
    /// The positive end is one code short and is handled by clamping in [`quantize`].
    fn full_scale(self) -> f64 {
        match self {
            WavBitDepth::Int16 => 32_768.0,
            WavBitDepth::Int24 => 8_388_608.0,
            WavBitDepth::Float32 => 1.0,
        }
    }

    /// Lowest and highest integer code this depth can store.
    fn code_range(self) -> (f64, f64) {
        match self {
            WavBitDepth::Int16 => (-32_768.0, 32_767.0),
            WavBitDepth::Int24 => (-8_388_608.0, 8_388_607.0),
            WavBitDepth::Float32 => (f64::from(f32::MIN), f64::from(f32::MAX)),
        }
    }

    fn sample_format(self) -> SampleFormat {
        match self {
            WavBitDepth::Float32 => SampleFormat::Float,
            _ => SampleFormat::Int,
        }
    }
}

/// How [`write_wav`] should encode a buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WavExportSettings {
    /// Sample format to store.
    pub bit_depth: WavBitDepth,
    /// Rate written into the file header. The buffer is not resampled — pass the rate it was
    /// rendered at, or resample first with [`crate::import::resample_buffer`].
    pub sample_rate: u32,
    /// Add TPDF dither before quantising to an integer depth.
    pub dither: bool,
}

/// Settings shared by every supported audio export format.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AudioExportSettings {
    /// Container and codec to write.
    pub format: AudioExportFormat,
    /// Integer depth for WAV or FLAC. WAV may additionally use [`WavBitDepth::Float32`].
    pub bit_depth: WavBitDepth,
    /// Rate written into the file and used by the encoder.
    pub sample_rate: u32,
    /// Add deterministic TPDF dither before integer quantisation.
    pub dither: bool,
    /// Constant bitrate for MP3 output.
    pub mp3_bitrate: Mp3Bitrate,
}

impl Default for AudioExportSettings {
    fn default() -> Self {
        Self {
            format: AudioExportFormat::Wav,
            bit_depth: WavBitDepth::Int24,
            sample_rate: 48_000,
            dither: false,
            mp3_bitrate: Mp3Bitrate::default(),
        }
    }
}

impl From<WavExportSettings> for AudioExportSettings {
    fn from(settings: WavExportSettings) -> Self {
        Self {
            bit_depth: settings.bit_depth,
            sample_rate: settings.sample_rate,
            dither: settings.dither,
            ..Self::default()
        }
    }
}

impl Default for WavExportSettings {
    /// 24-bit at 48 kHz without dither.
    ///
    /// 24-bit quantisation noise sits around -140 dBFS, far below the noise floor of any
    /// playback chain, so dithering it only adds noise for no audible benefit. Turn `dither` on
    /// for 16-bit masters.
    fn default() -> Self {
        Self {
            bit_depth: WavBitDepth::Int24,
            sample_rate: 48_000,
            dither: false,
        }
    }
}

/// Deterministic noise source for TPDF dither.
///
/// Exports must be reproducible — rendering the same project twice has to produce the same file
/// — so the generator starts from a fixed seed rather than the clock.
struct DitherNoise {
    state: u64,
}

impl DitherNoise {
    /// Seed is the 64-bit golden-ratio constant `floor(2^64 / phi)`, the usual choice for
    /// scrambling a fixed starting state. Any non-zero value works for xorshift.
    const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

    fn new() -> Self {
        Self { state: Self::SEED }
    }

    /// Uniform variate in `[0, 1)`.
    fn next_uniform(&mut self) -> f64 {
        // Marsaglia (2003), "Xorshift RNGs", shift triple (13, 7, 17) for a full 2^64-1 period.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        // 53 bits is the mantissa width of f64, so every draw is exactly representable.
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Triangular variate in `(-1, 1)`, in units of one LSB.
    ///
    /// The difference of two independent uniforms is the standard TPDF dither: it fully
    /// decorrelates the quantisation error from the signal, at the cost of 4.77 dB of added
    /// noise, which is what stops low-level material from developing granular distortion.
    fn next_tpdf(&mut self) -> f64 {
        self.next_uniform() - self.next_uniform()
    }
}

/// Rounds a normalised sample to an integer code, clamping into the representable range.
///
/// `dither_lsb` is added *after* scaling, in code units. It has to be: at 24 bits one LSB is
/// `2^-23`, which for a sample anywhere near full scale is the same order as the spacing of
/// `f32` itself, so adding the dither to the normalised sample first would round most of it away
/// and turn the triangular distribution into a coarse discrete one carrying about 1 dB more
/// noise than TPDF specifies. Scaling in `f64` also keeps the rounding error of the conversion
/// itself well under half an LSB at 24 bits.
///
/// The clamp is what keeps a sample at exactly `1.0` from becoming negative full scale: `1.0`
/// scales to 32768, which does not fit in an `i16` and would wrap to -32768 — an audible click
/// on any material that touches 0 dBFS.
fn quantize(sample: f32, dither_lsb: f64, scale: f64, min_code: f64, max_code: f64) -> i32 {
    // A NaN or infinity anywhere in the mix must not become a random code.
    if !sample.is_finite() {
        return 0;
    }
    (f64::from(sample) * scale + dither_lsb)
        .round()
        .clamp(min_code, max_code) as i32
}

/// Writes `buffer` to `path` as a WAV file.
///
/// The buffer's own sample rate is ignored in favour of `settings.sample_rate`; the caller is
/// responsible for having rendered at that rate.
///
/// Streamed into a sibling scratch file and renamed over the target, exactly as `save_project`
/// writes a document: creating the writer truncates, so
/// a failure partway — a full disk, a dropped network share — would otherwise already have
/// destroyed whatever bounce lived at that path, possibly the only render of an older mix.
pub fn write_wav(path: &Path, buffer: &AudioBuffer, settings: &WavExportSettings) -> Result<()> {
    write_audio(path, buffer, &(*settings).into())
}

/// Writes `buffer` using the selected format.
pub fn write_audio(
    path: &Path,
    buffer: &AudioBuffer,
    settings: &AudioExportSettings,
) -> Result<()> {
    write_audio_with_progress(path, buffer, settings, &mut |_| true)
}

/// Writes `buffer`, reporting progress and stopping when `progress` returns `false`.
///
/// Progress runs from 0 to 1 and includes sample conversion and encoder work. The destination is
/// replaced only after the encoder finalises successfully, so cancelling leaves an older export
/// at the same path intact.
pub fn write_audio_with_progress(
    path: &Path,
    buffer: &AudioBuffer,
    settings: &AudioExportSettings,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let frames = buffer.frame_count();
    let mut writer = AudioExportWriter::create(path, buffer.channel_count(), frames, settings)?;
    report_progress(progress, 0, frames)?;
    for start in (0..frames).step_by(EXPORT_CHUNK_FRAMES) {
        let end = (start + EXPORT_CHUNK_FRAMES).min(frames);
        writer.write_range(buffer, start, end)?;
        match settings.format {
            AudioExportFormat::Wav if end < frames => report_progress(progress, end, frames)?,
            AudioExportFormat::Flac | AudioExportFormat::Mp3 => {
                report_fraction(progress, end as f32 / frames as f32 * 0.9)?;
            }
            AudioExportFormat::Wav => {}
        }
    }
    writer.finalize_encoding()?;
    report_progress(progress, frames, frames)?;
    writer.publish()
}

/// Incremental encoder for one atomic audio export.
///
/// Audio may be supplied in any number of blocks. Conversion scratch is capped at
/// `EXPORT_CHUNK_FRAMES` per call, and the destination is not replaced until [`Self::finish`]
/// finalises the codec successfully. Dropping the writer after a render error or cancellation
/// deletes its randomly named sibling scratch file and leaves an existing destination intact.
pub struct AudioExportWriter {
    path: PathBuf,
    staged: Option<NamedTempFile>,
    encoder: Option<AudioEncoder>,
    settings: AudioExportSettings,
    channels: usize,
    expected_frames: usize,
    written_frames: usize,
    noise: DitherNoise,
}

/// A fully encoded and synchronised audio file that has not yet touched its destination.
///
/// Dropping it removes only its private sibling. A session worker can therefore decode and
/// validate the exact encoded bytes, then hand this value to a short stale-checked continuation.
pub struct StagedAudioExport {
    path: PathBuf,
    staged: NamedTempFile,
}

impl StagedAudioExport {
    /// Private file containing the final encoded bytes.
    pub fn staged_path(&self) -> &Path {
        self.staged.path()
    }

    /// Encoded byte count, read from the already-open staged handle.
    pub fn byte_size(&self) -> Result<u64> {
        self.staged
            .as_file()
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|error| IoError::from_fs(&self.path, error))
    }

    /// Atomically claims the destination without replacing a concurrently-created file.
    pub fn publish_noclobber(self) -> Result<()> {
        crate::project_file::publish_staged_file_noclobber(self.staged, &self.path)
    }
}

enum AudioEncoder {
    Wav(WavWriter<BufWriter<File>>),
    Flac {
        writer: Box<flac_codec::encode::FlacSampleWriter<BufWriter<File>>>,
        interleaved: Vec<i32>,
    },
    Mp3 {
        encoder: Box<Mp3Encode>,
        header: FrameHeader,
        output: BufWriter<File>,
        pending: Vec<Vec<f32>>,
    },
}

impl AudioExportWriter {
    /// Creates an encoder for `expected_frames` of planar audio.
    ///
    /// The channel count and total frame count are fixed up front so FLAC metadata is complete
    /// and an interrupted caller cannot accidentally publish a short file as a successful
    /// export.
    pub fn create(
        path: &Path,
        channels: usize,
        expected_frames: usize,
        settings: &AudioExportSettings,
    ) -> Result<Self> {
        validate_export_geometry(channels, expected_frames, settings)?;
        let staged = crate::project_file::new_staged_file(path)?;
        let file = staged
            .as_file()
            .try_clone()
            .map_err(|error| IoError::from_fs(path, error))?;
        let encoder = match settings.format {
            AudioExportFormat::Wav => {
                let spec = WavSpec {
                    channels: channels as u16,
                    sample_rate: settings.sample_rate,
                    bits_per_sample: settings.bit_depth.bits(),
                    sample_format: settings.bit_depth.sample_format(),
                };
                AudioEncoder::Wav(
                    WavWriter::new(BufWriter::new(file), spec)
                        .map_err(|error| wav_error(path, error))?,
                )
            }
            AudioExportFormat::Flac => {
                let channels = channels as u8;
                let total_samples = u64::try_from(expected_frames)
                    .ok()
                    .and_then(|frames| frames.checked_mul(u64::from(channels)))
                    .filter(|samples| *samples > 0);
                let writer = flac_codec::encode::FlacSampleWriter::new(
                    BufWriter::new(file),
                    flac_codec::encode::Options::default(),
                    settings.sample_rate,
                    u32::from(settings.bit_depth.bits()),
                    channels,
                    total_samples,
                )
                .map_err(|error| flac_error(path, error))?;
                AudioEncoder::Flac {
                    writer: Box::new(writer),
                    interleaved: Vec::with_capacity(EXPORT_CHUNK_FRAMES * usize::from(channels)),
                }
            }
            AudioExportFormat::Mp3 => {
                let header = rusty_mp3::encoder_header(
                    settings.sample_rate,
                    channels as u16,
                    settings.mp3_bitrate.kbps(),
                )
                .map_err(|error| mp3_error(path, error))?;
                let samples_per_frame = header.version.samples_per_frame();
                let audio_frames = expected_frames.div_ceil(samples_per_frame);
                let file_frames = audio_frames
                    .checked_add(1)
                    .ok_or_else(|| IoError::Mp3Write("MP3 frame count is too large".to_string()))?;
                let frame_count = u32::try_from(file_frames).map_err(|_| {
                    IoError::Mp3Write("MP3 is too long for its 32-bit Xing frame count".to_string())
                })?;
                let byte_count = file_frames
                    .checked_mul(header.frame_size())
                    .and_then(|bytes| u32::try_from(bytes).ok())
                    .ok_or_else(|| {
                        IoError::Mp3Write(
                            "MP3 is too long for its 32-bit Xing byte count".to_string(),
                        )
                    })?;
                let mut output = BufWriter::new(file);
                let info = rusty_mp3::encode::bitstream::info_frame(
                    &header,
                    frame_count,
                    byte_count,
                    false,
                );
                output
                    .write_all(&info)
                    .map_err(|error| IoError::from_fs(path, error))?;
                AudioEncoder::Mp3 {
                    // The high-level encoder's CBR reservoir retains every frame until
                    // `finish`. Export directly through the frame encoder instead: the Info
                    // frame is known up front, each reservoir-free CBR frame is written as
                    // soon as it is complete, and only one partial PCM frame remains here.
                    encoder: Box::new(Mp3Encode::new()),
                    pending: (0..channels)
                        .map(|_| Vec::with_capacity(EXPORT_CHUNK_FRAMES + samples_per_frame))
                        .collect(),
                    header,
                    output,
                }
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            staged: Some(staged),
            encoder: Some(encoder),
            settings: *settings,
            channels,
            expected_frames,
            written_frames: 0,
            noise: DitherNoise::new(),
        })
    }

    /// Encodes the next planar block.
    ///
    /// Blocks must have the channel count passed to [`Self::create`], and together they must
    /// contain exactly the declared frame count before [`Self::finish`] is called.
    pub fn write(&mut self, buffer: &AudioBuffer) -> Result<()> {
        if buffer.channel_count() != self.channels {
            return Err(export_error(
                self.settings.format,
                format!(
                    "export expected {} channels but received {}",
                    self.channels,
                    buffer.channel_count()
                ),
            ));
        }
        for start in (0..buffer.frame_count()).step_by(EXPORT_CHUNK_FRAMES) {
            let end = (start + EXPORT_CHUNK_FRAMES).min(buffer.frame_count());
            self.write_range(buffer, start, end)?;
        }
        Ok(())
    }

    fn write_range(&mut self, buffer: &AudioBuffer, start: usize, end: usize) -> Result<()> {
        if buffer.channel_count() != self.channels {
            return Err(export_error(
                self.settings.format,
                format!(
                    "export expected {} channels but received {}",
                    self.channels,
                    buffer.channel_count()
                ),
            ));
        }
        let frames = end.saturating_sub(start);
        if end > buffer.frame_count()
            || self
                .written_frames
                .checked_add(frames)
                .is_none_or(|total| total > self.expected_frames)
        {
            return Err(export_error(
                self.settings.format,
                "more audio was supplied than the declared export length".to_string(),
            ));
        }
        let planes = buffer.channels();
        match self.encoder.as_mut() {
            Some(AudioEncoder::Wav(writer)) => {
                if self.settings.bit_depth.is_integer() {
                    let scale = self.settings.bit_depth.full_scale();
                    let (min_code, max_code) = self.settings.bit_depth.code_range();
                    for frame in start..end {
                        for plane in planes {
                            let dither_lsb = if self.settings.dither {
                                self.noise.next_tpdf()
                            } else {
                                0.0
                            };
                            writer
                                .write_sample(quantize(
                                    plane[frame],
                                    dither_lsb,
                                    scale,
                                    min_code,
                                    max_code,
                                ))
                                .map_err(|error| wav_error(&self.path, error))?;
                        }
                    }
                } else {
                    for frame in start..end {
                        for plane in planes {
                            let sample = plane[frame];
                            writer
                                .write_sample(if sample.is_finite() { sample } else { 0.0 })
                                .map_err(|error| wav_error(&self.path, error))?;
                        }
                    }
                }
            }
            Some(AudioEncoder::Flac {
                writer,
                interleaved,
            }) => {
                let scale = self.settings.bit_depth.full_scale();
                let (min_code, max_code) = self.settings.bit_depth.code_range();
                interleaved.clear();
                for frame in start..end {
                    for plane in planes {
                        let dither_lsb = if self.settings.dither {
                            self.noise.next_tpdf()
                        } else {
                            0.0
                        };
                        interleaved.push(quantize(
                            plane[frame],
                            dither_lsb,
                            scale,
                            min_code,
                            max_code,
                        ));
                    }
                }
                writer
                    .write(interleaved)
                    .map_err(|error| flac_error(&self.path, error))?;
            }
            Some(AudioEncoder::Mp3 {
                encoder,
                header,
                output,
                pending,
            }) => {
                for (channel, plane) in planes.iter().enumerate() {
                    pending[channel].extend((start..end).map(|frame| {
                        let sample = plane[frame];
                        if sample.is_finite() {
                            sample.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        }
                    }));
                }
                write_complete_mp3_frames(encoder, header, pending, output, &self.path)?;
            }
            None => {
                return Err(export_error(
                    self.settings.format,
                    "audio was written after the encoder was finalised".to_string(),
                ));
            }
        }
        self.written_frames += frames;
        Ok(())
    }

    /// Finalises and atomically publishes the completed export.
    ///
    /// If fewer frames than declared were supplied, the scratch file is discarded instead.
    pub fn finish(mut self) -> Result<()> {
        self.finalize_encoding()?;
        self.publish()
    }

    /// Finalises the codec, then publishes only if `commit` still permits the durable change.
    ///
    /// Encoding and container finalisation touch only the private sibling scratch file. Calling
    /// the gate after those potentially expensive steps closes the cancellation race at the
    /// actual destination-replacement boundary without making a cancelled caller wait through a
    /// publication it did not authorise.
    pub fn finish_with_commit(mut self, commit: impl FnOnce() -> bool) -> Result<()> {
        self.finalize_encoding()?;
        if !commit() {
            return Err(IoError::ExportCancelled);
        }
        self.publish()
    }

    /// Finalises and synchronises the encoded bytes without publishing the destination.
    pub fn finish_staged(mut self) -> Result<StagedAudioExport> {
        self.finalize_encoding()?;
        let Some(mut staged) = self.staged.take() else {
            return Err(export_error(
                self.settings.format,
                "audio export scratch file was already published".to_string(),
            ));
        };
        staged
            .flush()
            .and_then(|()| staged.as_file().sync_all())
            .map_err(|error| IoError::from_fs(&self.path, error))?;
        Ok(StagedAudioExport {
            path: self.path,
            staged,
        })
    }

    /// Finalises the codec and publishes only if the destination is still unclaimed.
    ///
    /// This is the stem-export boundary: a folder picker cannot confirm replacement for every
    /// generated track name, and another process may create one after the initial folder check.
    /// The no-clobber persist makes that final race an error while preserving the winner's bytes.
    pub fn finish_noclobber_with_commit(mut self, commit: impl FnOnce() -> bool) -> Result<()> {
        self.finalize_encoding()?;
        if !commit() {
            return Err(IoError::ExportCancelled);
        }
        self.publish_noclobber()
    }

    fn finalize_encoding(&mut self) -> Result<()> {
        if self.written_frames != self.expected_frames {
            return Err(export_error(
                self.settings.format,
                format!(
                    "export expected {} frames but received {}",
                    self.expected_frames, self.written_frames
                ),
            ));
        }
        let Some(encoder) = self.encoder.take() else {
            return Err(export_error(
                self.settings.format,
                "audio encoder was already finalised".to_string(),
            ));
        };
        match encoder {
            AudioEncoder::Wav(writer) => writer
                .finalize()
                .map_err(|error| wav_error(&self.path, error))?,
            AudioEncoder::Flac { writer, .. } => writer
                .finalize()
                .map_err(|error| flac_error(&self.path, error))?,
            AudioEncoder::Mp3 {
                mut encoder,
                header,
                mut output,
                mut pending,
            } => {
                if pending.first().is_some_and(|channel| !channel.is_empty()) {
                    let samples_per_frame = header.version.samples_per_frame();
                    for channel in &mut pending {
                        channel.resize(samples_per_frame, 0.0);
                    }
                    write_complete_mp3_frames(
                        &mut encoder,
                        &header,
                        &mut pending,
                        &mut output,
                        &self.path,
                    )?;
                }
                output
                    .flush()
                    .map_err(|error| IoError::from_fs(&self.path, error))?;
            }
        }
        Ok(())
    }

    fn publish(mut self) -> Result<()> {
        let Some(staged) = self.staged.take() else {
            return Err(export_error(
                self.settings.format,
                "audio export scratch file was already published".to_string(),
            ));
        };
        crate::project_file::publish_staged_file(staged, &self.path)
    }

    fn publish_noclobber(mut self) -> Result<()> {
        let Some(staged) = self.staged.take() else {
            return Err(export_error(
                self.settings.format,
                "audio export scratch file was already published".to_string(),
            ));
        };
        crate::project_file::publish_staged_file_noclobber(staged, &self.path)
    }
}

fn validate_export_geometry(
    channels: usize,
    expected_frames: usize,
    settings: &AudioExportSettings,
) -> Result<()> {
    if settings.sample_rate == 0 {
        return Err(export_error(
            settings.format,
            "sample rate must not be zero".to_string(),
        ));
    }
    match settings.format {
        AudioExportFormat::Wav => {
            if channels == 0 || u16::try_from(channels).is_err() {
                return Err(IoError::WavWrite(format!(
                    "{channels} channels is more than the WAV format can describe"
                )));
            }
            let bytes_per_sample = u128::from(settings.bit_depth.bits().div_ceil(8));
            let data_bytes = expected_frames as u128 * channels as u128 * bytes_per_sample;
            // Hound's RIFF size includes the header in the same u32 as the data length. Reserve
            // its largest (WAVEFORMATEXTENSIBLE) header so finalisation cannot overflow after a
            // length that the constructor accepted.
            if data_bytes > u128::from(u32::MAX - 68) {
                return Err(IoError::WavWrite(
                    "audio is too long for a RIFF/WAVE file".to_string(),
                ));
            }
        }
        AudioExportFormat::Flac => {
            if matches!(settings.bit_depth, WavBitDepth::Float32) {
                return Err(IoError::FlacWrite(
                    "FLAC export supports 16-bit or 24-bit integer audio".to_string(),
                ));
            }
            if !(1..=8).contains(&channels) {
                return Err(IoError::FlacWrite(format!(
                    "FLAC export needs between 1 and 8 channels, got {channels} channels"
                )));
            }
        }
        AudioExportFormat::Mp3 => {
            if !(1..=2).contains(&channels) {
                return Err(IoError::Mp3Write(format!(
                    "MP3 export needs mono or stereo audio, got {channels} channels"
                )));
            }
            if !AudioExportFormat::Mp3.supports_sample_rate(settings.sample_rate) {
                return Err(IoError::Mp3Write(format!(
                    "{} Hz is not supported for MP3 export; choose 32000, 44100 or 48000 Hz",
                    settings.sample_rate
                )));
            }
            if expected_frames == 0 {
                return Err(IoError::Mp3Write(
                    "cannot encode a file containing no audio".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn export_error(format: AudioExportFormat, message: String) -> IoError {
    match format {
        AudioExportFormat::Wav => IoError::WavWrite(message),
        AudioExportFormat::Flac => IoError::FlacWrite(message),
        AudioExportFormat::Mp3 => IoError::Mp3Write(message),
    }
}

/// PCM frames converted between progress checks.
const EXPORT_CHUNK_FRAMES: usize = 4_096;

fn report_progress(
    progress: &mut dyn FnMut(f32) -> bool,
    completed: usize,
    total: usize,
) -> Result<()> {
    let fraction = if total == 0 {
        1.0
    } else {
        completed as f32 / total as f32
    };
    report_fraction(progress, fraction)
}

fn report_fraction(progress: &mut dyn FnMut(f32) -> bool, fraction: f32) -> Result<()> {
    if progress(fraction.clamp(0.0, 1.0)) {
        Ok(())
    } else {
        Err(IoError::ExportCancelled)
    }
}

fn write_complete_mp3_frames(
    encoder: &mut Mp3Encode,
    header: &FrameHeader,
    pending: &mut [Vec<f32>],
    output: &mut impl Write,
    reported: &Path,
) -> Result<()> {
    let samples_per_frame = header.version.samples_per_frame();
    while pending
        .first()
        .is_some_and(|channel| channel.len() >= samples_per_frame)
    {
        let frame: Vec<Vec<f32>> = pending
            .iter_mut()
            .map(|channel| channel.drain(..samples_per_frame).collect())
            .collect();
        let packet = encoder
            .encode_frame(header, &frame, None)
            .map_err(|error| mp3_error(reported, error))?;
        output
            .write_all(&packet)
            .map_err(|error| IoError::from_fs(reported, error))?;
    }
    Ok(())
}

fn flac_error(path: &Path, error: flac_codec::Error) -> IoError {
    match error {
        flac_codec::Error::Io(source) => IoError::from_fs(path, source),
        other => IoError::FlacWrite(format!("{}: {other}", path.display())),
    }
}

fn mp3_error(path: &Path, error: rusty_mp3::Error) -> IoError {
    IoError::Mp3Write(format!("{}: {error}", path.display()))
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
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    fn legacy_predictable_scratch_path(path: &Path) -> PathBuf {
        let mut name = path
            .file_name()
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from("project"));
        name.push(format!(".{}.saving", std::process::id()));
        path.with_file_name(name)
    }

    fn test_buffer() -> AudioBuffer {
        // Values chosen to hit both rails, the midpoint and a few small levels.
        let left = vec![0.0, 0.5, -0.5, 0.25, -1.0, 0.125, -0.75, 0.999];
        let right = vec![1.0, -0.25, 0.75, -0.125, 0.0625, -0.0625, 0.375, -0.375];
        AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap()
    }

    fn read_int_samples(path: &Path) -> (hound::WavSpec, Vec<i32>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples = reader
            .samples::<i32>()
            .collect::<std::result::Result<Vec<i32>, _>>()
            .unwrap();
        (spec, samples)
    }

    #[test]
    fn default_settings_are_24_bit_at_48_khz() {
        let settings = WavExportSettings::default();
        assert_eq!(settings.bit_depth, WavBitDepth::Int24);
        assert_eq!(settings.sample_rate, 48_000);
        assert!(!settings.dither);
    }

    #[test]
    fn a_preplaced_predictable_scratch_link_cannot_damage_another_file() {
        let file = TempFile::new("preserved-bounce.wav");
        write_wav(file.path(), &test_buffer(), &WavExportSettings::default()).unwrap();

        let victim = TempFile::new("audio-export-victim.txt");
        let sentinel = b"this file must not be opened or truncated by an audio export";
        std::fs::write(victim.path(), sentinel).unwrap();
        let scratch = legacy_predictable_scratch_path(file.path());
        std::fs::hard_link(victim.path(), &scratch).unwrap();

        let result = write_wav(file.path(), &test_buffer(), &WavExportSettings::default());
        let victim_after = std::fs::read(victim.path()).unwrap();
        let _ = std::fs::remove_file(&scratch);

        result.unwrap();
        assert_eq!(victim_after, sentinel);
        let (spec, _) = read_int_samples(file.path());
        assert_eq!(spec.bits_per_sample, 24);
    }

    #[test]
    fn simultaneous_exports_to_one_path_use_independent_scratch_files() {
        const WRITERS: usize = 8;

        let file = TempFile::new("parallel-export.wav");
        let encoders_open = Arc::new(Barrier::new(WRITERS));
        let mut writers = Vec::with_capacity(WRITERS);
        for _ in 0..WRITERS {
            let path = file.path().to_path_buf();
            let encoders_open = Arc::clone(&encoders_open);
            writers.push(std::thread::spawn(move || {
                let mut first_progress = true;
                write_audio_with_progress(
                    &path,
                    &tone_buffer(EXPORT_CHUNK_FRAMES * 2),
                    &AudioExportSettings::default(),
                    &mut |fraction| {
                        if first_progress {
                            assert_eq!(fraction, 0.0);
                            first_progress = false;
                            encoders_open.wait();
                        }
                        true
                    },
                )
            }));
        }

        for writer in writers {
            writer
                .join()
                .expect("export thread panicked")
                .expect("independent exports must not collide through a shared scratch file");
        }
        let (spec, samples) = read_int_samples(file.path());
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(samples.len(), EXPORT_CHUNK_FRAMES * 2 * 2);
    }

    #[test]
    fn int16_export_matches_within_one_lsb() {
        let file = TempFile::new("export-int16.wav");
        let buffer = test_buffer();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int16,
                sample_rate: 44_100,
                dither: false,
            },
        )
        .unwrap();

        let (spec, samples) = read_int_samples(file.path());
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(samples.len(), buffer.frame_count() * 2);

        // Half an LSB of rounding error, expressed as a normalised level.
        let tolerance = 0.5 / 32_768.0;
        for (index, code) in samples.iter().enumerate() {
            let expected = buffer.sample(index % 2, index / 2);
            let decoded = *code as f32 / 32_768.0;
            let error = (decoded - expected).abs();
            // The +1.0 sample cannot be represented; it is allowed to fall one code short.
            let allowed = if expected >= 1.0 {
                1.0 / 32_768.0
            } else {
                tolerance
            };
            assert!(error <= allowed, "sample {index}: {decoded} vs {expected}");
        }
    }

    #[test]
    fn int24_export_matches_within_one_lsb() {
        let file = TempFile::new("export-int24.wav");
        let buffer = test_buffer();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int24,
                sample_rate: 96_000,
                dither: false,
            },
        )
        .unwrap();

        let (spec, samples) = read_int_samples(file.path());
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(spec.sample_rate, 96_000);
        assert_eq!(samples.len(), buffer.frame_count() * 2);

        let tolerance = 0.5 / 8_388_608.0;
        for (index, code) in samples.iter().enumerate() {
            let expected = buffer.sample(index % 2, index / 2);
            let decoded = *code as f32 / 8_388_608.0;
            let error = (decoded - expected).abs();
            let allowed = if expected >= 1.0 {
                1.0 / 8_388_608.0
            } else {
                tolerance
            };
            assert!(error <= allowed, "sample {index}: {decoded} vs {expected}");
        }
    }

    #[test]
    fn float32_export_is_bit_exact() {
        let file = TempFile::new("export-f32.wav");
        let buffer = test_buffer();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Float32,
                sample_rate: 48_000,
                dither: false,
            },
        )
        .unwrap();

        let mut reader = hound::WavReader::open(file.path()).unwrap();
        assert_eq!(reader.spec().bits_per_sample, 32);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Float);
        let samples: Vec<f32> = reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<f32>, _>>()
            .unwrap();
        assert_eq!(samples.len(), buffer.frame_count() * 2);
        for (index, sample) in samples.iter().enumerate() {
            assert_eq!(*sample, buffer.sample(index % 2, index / 2));
        }
    }

    #[test]
    fn full_scale_positive_sample_does_not_wrap_in_int16() {
        let file = TempFile::new("export-full-scale.wav");
        let buffer = AudioBuffer::from_planar(vec![vec![1.0, 1.5, -1.0, -1.5]], 48_000.0).unwrap();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int16,
                sample_rate: 48_000,
                dither: false,
            },
        )
        .unwrap();

        let (_spec, samples) = read_int_samples(file.path());
        assert_eq!(samples, vec![32_767, 32_767, -32_768, -32_768]);
    }

    #[test]
    fn full_scale_positive_sample_does_not_wrap_in_int24() {
        let file = TempFile::new("export-full-scale-24.wav");
        let buffer = AudioBuffer::from_planar(vec![vec![1.0, -1.0]], 48_000.0).unwrap();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int24,
                sample_rate: 48_000,
                dither: false,
            },
        )
        .unwrap();

        let (_spec, samples) = read_int_samples(file.path());
        assert_eq!(samples, vec![8_388_607, -8_388_608]);
    }

    #[test]
    fn dither_moves_samples_by_at_most_one_lsb() {
        let file = TempFile::new("export-dither.wav");
        let buffer = AudioBuffer::from_planar(vec![vec![0.25f32; 512]], 48_000.0).unwrap();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int16,
                sample_rate: 48_000,
                dither: true,
            },
        )
        .unwrap();

        let (_spec, samples) = read_int_samples(file.path());
        let undithered = 8_192; // 0.25 * 32768
        // Triangular noise on (-1, 1) LSB pushes a sample to a neighbouring code whenever
        // |noise| > 0.5, which is a quarter of the time. Well under that means it never ran.
        let fraction = dithered_fraction(&samples, undithered);
        assert!(fraction > 0.125, "only {fraction} of samples dithered");
    }

    /// Fraction of `samples` that dither pushed off `undithered`, asserting none moved further
    /// than one code.
    fn dithered_fraction(samples: &[i32], undithered: i32) -> f64 {
        let mut moved = 0usize;
        for code in samples {
            let offset = code - undithered;
            assert!(
                (-1..=1).contains(&offset),
                "dither moved a sample by {offset}"
            );
            if offset != 0 {
                moved += 1;
            }
        }
        moved as f64 / samples.len() as f64
    }

    /// Exports a constant-valued mono buffer with dither on and returns the codes.
    fn dithered_codes(name: &str, value: f32, depth: WavBitDepth, frames: usize) -> Vec<i32> {
        let file = TempFile::new(name);
        let buffer = AudioBuffer::from_planar(vec![vec![value; frames]], 48_000.0).unwrap();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: depth,
                sample_rate: 48_000,
                dither: true,
            },
        )
        .unwrap();
        read_int_samples(file.path()).1
    }

    #[test]
    fn dither_is_triangular_at_both_integer_depths() {
        // TPDF noise spanning (-1, 1) LSB moves a sample that sits exactly on a code whenever
        // |noise| > 0.5, which for a triangular density is exactly a quarter of the time. That
        // number is the whole point of the dither: too low and it never engages, too high and
        // the export carries more noise than the specification calls for.
        //
        // The 24-bit cases are the ones that matter. One LSB there is 2^-23, close enough to the
        // spacing of `f32` near full scale that adding the dither before scaling instead of
        // after quantises the triangle into a handful of discrete steps and pushes this fraction
        // to over 0.30.
        for (name, value, depth) in [
            ("dither-16-quarter.wav", 0.25f32, WavBitDepth::Int16),
            ("dither-16-loud.wav", 0.75, WavBitDepth::Int16),
            ("dither-24-quarter.wav", 0.25, WavBitDepth::Int24),
            ("dither-24-loud.wav", 0.75, WavBitDepth::Int24),
            ("dither-24-hot.wav", 0.984_375, WavBitDepth::Int24),
        ] {
            let samples = dithered_codes(name, value, depth, 8_192);
            let undithered = (f64::from(value) * depth.full_scale()).round() as i32;
            let fraction = dithered_fraction(&samples, undithered);
            // 8192 draws put the sampling error at well under a percent, so a 0.05 band around
            // the ideal 0.25 is generous while still failing anything structurally wrong.
            assert!(
                (0.20..=0.30).contains(&fraction),
                "{name}: {fraction} of samples dithered, expected about 0.25"
            );
        }
    }

    #[test]
    fn dither_is_reproducible_across_exports() {
        // Rendering the same project twice has to produce byte-identical files, so the noise
        // source must be seeded, not clocked.
        let first = dithered_codes("dither-repeat-a.wav", 0.25, WavBitDepth::Int16, 256);
        let second = dithered_codes("dither-repeat-b.wav", 0.25, WavBitDepth::Int16, 256);
        assert_eq!(first, second);
    }

    #[test]
    fn a_mono_buffer_exports_as_one_channel() {
        let file = TempFile::new("export-mono.wav");
        let buffer = AudioBuffer::from_planar(vec![vec![0.5, -0.5, 0.25]], 48_000.0).unwrap();
        write_wav(file.path(), &buffer, &WavExportSettings::default()).unwrap();

        let (spec, samples) = read_int_samples(file.path());
        assert_eq!(spec.channels, 1);
        assert_eq!(samples, vec![4_194_304, -4_194_304, 2_097_152]);
    }

    #[test]
    fn a_six_channel_buffer_keeps_its_frames_interleaved_in_order() {
        let file = TempFile::new("export-6ch.wav");
        // Each channel holds a distinct constant so a swapped or dropped plane is obvious.
        let planes: Vec<Vec<f32>> = (0..6).map(|c| vec![c as f32 / 8.0; 4]).collect();
        let buffer = AudioBuffer::from_planar(planes, 48_000.0).unwrap();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int16,
                sample_rate: 48_000,
                dither: false,
            },
        )
        .unwrap();

        let (spec, samples) = read_int_samples(file.path());
        assert_eq!(spec.channels, 6);
        assert_eq!(samples.len(), 24);
        for (index, code) in samples.iter().enumerate() {
            let channel = index % 6;
            assert_eq!(
                *code,
                (channel as f64 / 8.0 * 32_768.0) as i32,
                "at {index}"
            );
        }
    }

    #[test]
    fn an_empty_buffer_writes_a_readable_header_only_file() {
        let file = TempFile::new("export-empty.wav");
        let buffer = AudioBuffer::stereo(0, 48_000.0);
        write_wav(file.path(), &buffer, &WavExportSettings::default()).unwrap();

        let (spec, samples) = read_int_samples(file.path());
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.bits_per_sample, 24);
        assert!(samples.is_empty());
    }

    #[test]
    fn a_zero_sample_rate_is_rejected() {
        let file = TempFile::new("export-zero-rate.wav");
        let result = write_wav(
            file.path(),
            &test_buffer(),
            &WavExportSettings {
                bit_depth: WavBitDepth::Int16,
                sample_rate: 0,
                dither: false,
            },
        );
        assert!(matches!(result, Err(IoError::WavWrite(_))));
    }

    #[test]
    fn writing_into_a_missing_directory_reports_the_path() {
        let path = std::env::temp_dir()
            .join("auris-io-no-such-directory")
            .join("out.wav");
        match write_wav(&path, &test_buffer(), &WavExportSettings::default()) {
            Err(IoError::FileNotFound(reported)) => assert_eq!(reported, path),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn non_finite_samples_are_written_as_silence() {
        let file = TempFile::new("export-nan.wav");
        let buffer =
            AudioBuffer::from_planar(vec![vec![f32::NAN, f32::INFINITY, 0.5]], 48_000.0).unwrap();
        write_wav(
            file.path(),
            &buffer,
            &WavExportSettings {
                bit_depth: WavBitDepth::Int16,
                sample_rate: 48_000,
                dither: false,
            },
        )
        .unwrap();

        let (_spec, samples) = read_int_samples(file.path());
        assert_eq!(samples, vec![0, 0, 16_384]);
    }

    fn tone_buffer(frames: usize) -> AudioBuffer {
        let left = (0..frames)
            .map(|frame| (std::f32::consts::TAU * 440.0 * frame as f32 / 48_000.0).sin() * 0.5)
            .collect::<Vec<_>>();
        let right = left.iter().map(|sample| -*sample * 0.5).collect();
        AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap()
    }

    #[test]
    fn flac_export_round_trips_losslessly_at_24_bit() {
        let file = TempFile::new("export.flac");
        let buffer = tone_buffer(8_192);
        write_audio(
            file.path(),
            &buffer,
            &AudioExportSettings {
                format: AudioExportFormat::Flac,
                ..AudioExportSettings::default()
            },
        )
        .unwrap();

        assert_eq!(&std::fs::read(file.path()).unwrap()[..4], b"fLaC");
        let decoded = crate::decode_audio_file(file.path()).unwrap();
        assert_eq!(decoded.source_sample_rate, 48_000.0);
        assert_eq!(decoded.channel_count, 2);
        assert_eq!(decoded.buffer.frame_count(), buffer.frame_count());
        let tolerance = 1.0 / 8_388_608.0;
        for frame in [0, 1, 127, 4_095, 8_191] {
            for channel in 0..2 {
                assert!(
                    (decoded.buffer.sample(channel, frame) - buffer.sample(channel, frame)).abs()
                        <= tolerance
                );
            }
        }
    }

    #[test]
    fn mp3_export_is_decodable_at_the_selected_rate() {
        let file = TempFile::new("export.mp3");
        let buffer = tone_buffer(9_600);
        write_audio(
            file.path(),
            &buffer,
            &AudioExportSettings {
                format: AudioExportFormat::Mp3,
                mp3_bitrate: Mp3Bitrate::Kbps192,
                ..AudioExportSettings::default()
            },
        )
        .unwrap();

        let decoded = crate::decode_audio_file(file.path()).unwrap();
        assert_eq!(decoded.source_sample_rate, 48_000.0);
        assert_eq!(decoded.channel_count, 2);
        assert!(decoded.buffer.frame_count() >= buffer.frame_count());
        assert!(decoded.buffer.peak() > 0.1, "the decoded MP3 is not silent");
    }

    #[test]
    fn cancelling_an_encode_preserves_the_previous_export() {
        let folder = TempFile::new("cancelled-export");
        std::fs::create_dir(folder.path()).unwrap();
        let path = folder.path().join("preserved.flac");
        let settings = AudioExportSettings {
            format: AudioExportFormat::Flac,
            ..AudioExportSettings::default()
        };
        write_audio(&path, &tone_buffer(4_096), &settings).unwrap();
        let before = std::fs::read(&path).unwrap();

        let result = write_audio_with_progress(
            &path,
            &tone_buffer(EXPORT_CHUNK_FRAMES * 3),
            &settings,
            &mut |fraction| fraction < 0.5,
        );
        assert!(matches!(result, Err(IoError::ExportCancelled)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn denying_the_final_commit_after_codec_finalisation_preserves_the_target() {
        let folder = TempFile::new("denied-final-commit");
        std::fs::create_dir(folder.path()).unwrap();
        let path = folder.path().join("preserved.mp3");
        let previous = b"previous complete export";
        std::fs::write(&path, previous).unwrap();
        let buffer = tone_buffer(9_600);
        let settings = AudioExportSettings {
            format: AudioExportFormat::Mp3,
            mp3_bitrate: Mp3Bitrate::Kbps192,
            ..AudioExportSettings::default()
        };
        let mut writer = AudioExportWriter::create(
            &path,
            buffer.channel_count(),
            buffer.frame_count(),
            &settings,
        )
        .unwrap();
        writer.write(&buffer).unwrap();

        assert!(matches!(
            writer.finish_with_commit(|| false),
            Err(IoError::ExportCancelled)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn noclobber_commit_loses_a_race_without_replacing_the_winner() {
        let folder = TempFile::new("noclobber-final-commit");
        std::fs::create_dir(folder.path()).unwrap();
        let path = folder.path().join("stem.wav");
        let buffer = tone_buffer(1_024);
        let settings = AudioExportSettings::default();
        let mut writer = AudioExportWriter::create(
            &path,
            buffer.channel_count(),
            buffer.frame_count(),
            &settings,
        )
        .unwrap();
        writer.write(&buffer).unwrap();
        let winner = b"another process won";

        assert!(matches!(
            writer.finish_noclobber_with_commit(|| {
                std::fs::write(&path, winner).unwrap();
                true
            }),
            Err(IoError::ExportDestinationExists(existing)) if existing == path
        ));
        assert_eq!(std::fs::read(&path).unwrap(), winner);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    fn slice(buffer: &AudioBuffer, start: usize, end: usize) -> AudioBuffer {
        AudioBuffer::from_planar(
            buffer
                .channels()
                .iter()
                .map(|channel| channel[start..end].to_vec())
                .collect(),
            buffer.sample_rate(),
        )
        .unwrap()
    }

    #[test]
    fn incremental_blocks_produce_the_same_file_for_every_format() {
        let buffer = tone_buffer(9_600);
        for (name, settings) in [
            (
                "streamed-int16.wav",
                AudioExportSettings {
                    bit_depth: WavBitDepth::Int16,
                    dither: true,
                    ..AudioExportSettings::default()
                },
            ),
            (
                "streamed-float.wav",
                AudioExportSettings {
                    bit_depth: WavBitDepth::Float32,
                    ..AudioExportSettings::default()
                },
            ),
            (
                "streamed.flac",
                AudioExportSettings {
                    format: AudioExportFormat::Flac,
                    dither: true,
                    ..AudioExportSettings::default()
                },
            ),
            (
                "streamed.mp3",
                AudioExportSettings {
                    format: AudioExportFormat::Mp3,
                    mp3_bitrate: Mp3Bitrate::Kbps192,
                    ..AudioExportSettings::default()
                },
            ),
        ] {
            let whole = TempFile::new(&format!("whole-{name}"));
            let incremental = TempFile::new(&format!("incremental-{name}"));
            write_audio(whole.path(), &buffer, &settings).unwrap();

            let mut writer = AudioExportWriter::create(
                incremental.path(),
                buffer.channel_count(),
                buffer.frame_count(),
                &settings,
            )
            .unwrap();
            for start in (0..buffer.frame_count()).step_by(257) {
                writer
                    .write(&slice(
                        &buffer,
                        start,
                        (start + 257).min(buffer.frame_count()),
                    ))
                    .unwrap();
            }
            writer.finish().unwrap();

            assert_eq!(
                std::fs::read(incremental.path()).unwrap(),
                std::fs::read(whole.path()).unwrap(),
                "{name} changed when block boundaries changed"
            );
        }
    }

    #[test]
    fn an_incomplete_incremental_export_preserves_the_target_and_removes_scratch() {
        let folder = TempFile::new("short-stream");
        std::fs::create_dir(folder.path()).unwrap();
        let path = folder.path().join("mix.wav");
        let previous = b"previous complete export";
        std::fs::write(&path, previous).unwrap();

        let mut writer =
            AudioExportWriter::create(&path, 2, 1_024, &AudioExportSettings::default()).unwrap();
        writer.write(&tone_buffer(128)).unwrap();
        assert!(matches!(writer.finish(), Err(IoError::WavWrite(_))));

        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn conversion_workspace_stays_at_one_chunk_for_large_input_blocks() {
        let file = TempFile::new("bounded.flac");
        let settings = AudioExportSettings {
            format: AudioExportFormat::Flac,
            ..AudioExportSettings::default()
        };
        let buffer = tone_buffer(EXPORT_CHUNK_FRAMES * 8);
        let mut writer = AudioExportWriter::create(
            file.path(),
            buffer.channel_count(),
            buffer.frame_count(),
            &settings,
        )
        .unwrap();
        writer.write(&buffer).unwrap();

        let capacity = match writer.encoder.as_ref().unwrap() {
            AudioEncoder::Flac { interleaved, .. } => interleaved.capacity(),
            _ => unreachable!(),
        };
        assert_eq!(capacity, EXPORT_CHUNK_FRAMES * buffer.channel_count());
        writer.finish().unwrap();
    }

    #[test]
    fn mp3_streams_packets_without_growing_with_the_song() {
        let file = TempFile::new("bounded.mp3");
        let settings = AudioExportSettings {
            format: AudioExportFormat::Mp3,
            mp3_bitrate: Mp3Bitrate::Kbps192,
            ..AudioExportSettings::default()
        };
        let block = tone_buffer(EXPORT_CHUNK_FRAMES);
        let repeats = 64;
        let mut writer = AudioExportWriter::create(
            file.path(),
            block.channel_count(),
            block.frame_count() * repeats,
            &settings,
        )
        .unwrap();
        let scratch = writer.staged.as_ref().unwrap().path().to_path_buf();
        let initial_capacities = match writer.encoder.as_ref().unwrap() {
            AudioEncoder::Mp3 { pending, .. } => {
                pending.iter().map(Vec::capacity).collect::<Vec<_>>()
            }
            _ => unreachable!(),
        };

        for _ in 0..repeats {
            writer.write(&block).unwrap();
            match writer.encoder.as_ref().unwrap() {
                AudioEncoder::Mp3 {
                    header, pending, ..
                } => {
                    assert!(
                        pending
                            .iter()
                            .all(|channel| channel.len() < header.version.samples_per_frame())
                    );
                    assert_eq!(
                        pending.iter().map(Vec::capacity).collect::<Vec<_>>(),
                        initial_capacities
                    );
                }
                _ => unreachable!(),
            }
        }
        match writer.encoder.as_mut().unwrap() {
            AudioEncoder::Mp3 { output, .. } => output.flush().unwrap(),
            _ => unreachable!(),
        }
        assert!(
            std::fs::metadata(scratch).unwrap().len() > 8_192,
            "encoded packets should reach disk before finalisation"
        );
        writer.finish().unwrap();
    }
}
