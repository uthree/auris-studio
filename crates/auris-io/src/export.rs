//! Writing rendered audio out as WAV, FLAC or MP3.
//!
//! An offline render already holds the finished mix in memory. Each encoder consumes that buffer
//! in bounded chunks so progress and cancellation remain responsive while the file is written.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use auris_core::AudioBuffer;
use hound::{SampleFormat, WavSpec, WavWriter};
use rusty_mp3::{Mp3Encoder, Mp3EncoderConfig};

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
    let in_progress = crate::project_file::in_progress_path(path);
    let written = match settings.format {
        AudioExportFormat::Wav => stream_wav(&in_progress, path, buffer, settings, progress),
        AudioExportFormat::Flac => stream_flac(&in_progress, path, buffer, settings, progress),
        AudioExportFormat::Mp3 => stream_mp3(&in_progress, path, buffer, settings, progress),
    };
    if let Err(error) = written {
        let _ = std::fs::remove_file(&in_progress);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&in_progress, path) {
        let _ = std::fs::remove_file(&in_progress);
        return Err(IoError::from_fs(path, error));
    }
    Ok(())
}

/// Streams the samples into `target`, with errors reported against `reported` — the name the
/// user asked to export, the scratch file being an implementation detail.
fn stream_wav(
    target: &Path,
    reported: &Path,
    buffer: &AudioBuffer,
    settings: &AudioExportSettings,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let channel_count = u16::try_from(buffer.channel_count()).map_err(|_| {
        IoError::WavWrite(format!(
            "{} channels is more than the WAV format can describe",
            buffer.channel_count()
        ))
    })?;
    if settings.sample_rate == 0 {
        return Err(IoError::WavWrite(
            "sample rate must not be zero".to_string(),
        ));
    }

    let spec = WavSpec {
        channels: channel_count,
        sample_rate: settings.sample_rate,
        bits_per_sample: settings.bit_depth.bits(),
        sample_format: settings.bit_depth.sample_format(),
    };

    let mut writer = WavWriter::create(target, spec).map_err(|e| wav_error(reported, e))?;
    let planes = buffer.channels();
    let frames = buffer.frame_count();
    report_progress(progress, 0, frames)?;

    if settings.bit_depth.is_integer() {
        let scale = settings.bit_depth.full_scale();
        let (min_code, max_code) = settings.bit_depth.code_range();
        let mut noise = DitherNoise::new();
        for frame in 0..frames {
            if frame > 0 && frame % EXPORT_CHUNK_FRAMES == 0 {
                report_progress(progress, frame, frames)?;
            }
            for plane in planes {
                let dither_lsb = if settings.dither {
                    noise.next_tpdf()
                } else {
                    0.0
                };
                let code = quantize(plane[frame], dither_lsb, scale, min_code, max_code);
                writer
                    .write_sample(code)
                    .map_err(|e| wav_error(reported, e))?;
            }
        }
    } else {
        for frame in 0..frames {
            if frame > 0 && frame % EXPORT_CHUNK_FRAMES == 0 {
                report_progress(progress, frame, frames)?;
            }
            for plane in planes {
                let sample = plane[frame];
                let sample = if sample.is_finite() { sample } else { 0.0 };
                writer
                    .write_sample(sample)
                    .map_err(|e| wav_error(reported, e))?;
            }
        }
    }

    writer.finalize().map_err(|e| wav_error(reported, e))?;
    report_progress(progress, frames, frames)
}

/// Encodes integer PCM into FLAC in bounded chunks.
fn stream_flac(
    target: &Path,
    reported: &Path,
    buffer: &AudioBuffer,
    settings: &AudioExportSettings,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    if matches!(settings.bit_depth, WavBitDepth::Float32) {
        return Err(IoError::FlacWrite(
            "FLAC export supports 16-bit or 24-bit integer audio".to_string(),
        ));
    }
    let channels = u8::try_from(buffer.channel_count()).map_err(|_| {
        IoError::FlacWrite(format!(
            "{} channels is more than the FLAC format can describe",
            buffer.channel_count()
        ))
    })?;
    if channels == 0 || channels > 8 {
        return Err(IoError::FlacWrite(format!(
            "FLAC export needs between 1 and 8 channels, got {channels}"
        )));
    }
    if settings.sample_rate == 0 {
        return Err(IoError::FlacWrite(
            "sample rate must not be zero".to_string(),
        ));
    }

    let file = File::create(target).map_err(|error| IoError::from_fs(reported, error))?;
    let total_samples = u64::try_from(buffer.frame_count())
        .ok()
        .and_then(|frames| frames.checked_mul(u64::from(channels)))
        .filter(|samples| *samples > 0);
    let mut writer = flac_codec::encode::FlacSampleWriter::new(
        BufWriter::new(file),
        flac_codec::encode::Options::default(),
        settings.sample_rate,
        u32::from(settings.bit_depth.bits()),
        channels,
        total_samples,
    )
    .map_err(|error| flac_error(reported, error))?;

    let frames = buffer.frame_count();
    let planes = buffer.channels();
    let scale = settings.bit_depth.full_scale();
    let (min_code, max_code) = settings.bit_depth.code_range();
    let mut noise = DitherNoise::new();
    report_progress(progress, 0, frames)?;
    for start in (0..frames).step_by(EXPORT_CHUNK_FRAMES) {
        let end = (start + EXPORT_CHUNK_FRAMES).min(frames);
        let mut interleaved = Vec::with_capacity((end - start) * planes.len());
        for frame in start..end {
            for plane in planes {
                let dither_lsb = if settings.dither {
                    noise.next_tpdf()
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
            .write(&interleaved)
            .map_err(|error| flac_error(reported, error))?;
        report_fraction(progress, end as f32 / frames as f32 * 0.9)?;
    }
    writer
        .finalize()
        .map_err(|error| flac_error(reported, error))?;
    report_progress(progress, frames, frames)
}

/// Encodes stereo or mono floating-point PCM into MP3 in bounded chunks.
fn stream_mp3(
    target: &Path,
    reported: &Path,
    buffer: &AudioBuffer,
    settings: &AudioExportSettings,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let channels = u16::try_from(buffer.channel_count()).map_err(|_| {
        IoError::Mp3Write(format!(
            "{} channels is more than MP3 can describe",
            buffer.channel_count()
        ))
    })?;
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
    if buffer.frame_count() == 0 {
        return Err(IoError::Mp3Write(
            "cannot encode a file containing no audio".to_string(),
        ));
    }

    let file = File::create(target).map_err(|error| IoError::from_fs(reported, error))?;
    let mut output = BufWriter::new(file);
    let mut encoder = Mp3Encoder::new(Mp3EncoderConfig {
        bitrate_kbps: settings.mp3_bitrate.kbps(),
        vbr_quality: None,
    });
    let frames = buffer.frame_count();
    let planes = buffer.channels();
    report_progress(progress, 0, frames)?;
    for start in (0..frames).step_by(EXPORT_CHUNK_FRAMES) {
        let end = (start + EXPORT_CHUNK_FRAMES).min(frames);
        let mut interleaved = Vec::with_capacity((end - start) * planes.len());
        for frame in start..end {
            for plane in planes {
                let sample = plane[frame];
                interleaved.push(if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                });
            }
        }
        encoder
            .push_pcm_f32(&interleaved, channels, settings.sample_rate)
            .map_err(|error| mp3_error(reported, error))?;
        // Keep packets queued until `finish`: that lets the encoder prepend its Info frame once
        // it knows the final frame and byte counts, including at 320 kbps where audio packets
        // are otherwise available immediately.
        report_fraction(progress, end as f32 / frames as f32 * 0.9)?;
    }
    encoder.finish();
    drain_mp3_packets(&mut encoder, &mut output, reported)?;
    output
        .flush()
        .map_err(|error| IoError::from_fs(reported, error))?;
    report_progress(progress, frames, frames)
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

fn drain_mp3_packets(
    encoder: &mut Mp3Encoder,
    output: &mut impl Write,
    reported: &Path,
) -> Result<()> {
    loop {
        match encoder.next_packet() {
            Ok(packet) => output
                .write_all(&packet)
                .map_err(|error| IoError::from_fs(reported, error))?,
            Err(rusty_mp3::Error::Again | rusty_mp3::Error::Eof) => return Ok(()),
            Err(error) => return Err(mp3_error(reported, error)),
        }
    }
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
    fn a_failed_export_leaves_the_previous_bounce_intact() {
        // The exporter used to open the destination with truncation, so a failure partway
        // destroyed whatever file was already there — possibly the only render of an older
        // mix. Same defence, and same test shape, as the project save.
        let file = TempFile::new("preserved-bounce.wav");
        write_wav(file.path(), &test_buffer(), &WavExportSettings::default()).unwrap();
        let before = std::fs::read(file.path()).unwrap();

        // A directory in place of the scratch file makes the write fail after the point where
        // a truncating export would already have destroyed the target.
        let blocker = crate::project_file::in_progress_path(file.path());
        std::fs::create_dir(&blocker).unwrap();
        assert!(write_wav(file.path(), &test_buffer(), &WavExportSettings::default()).is_err());
        std::fs::remove_dir(&blocker).unwrap();

        assert_eq!(std::fs::read(file.path()).unwrap(), before);
        let (spec, _) = read_int_samples(file.path());
        assert_eq!(spec.bits_per_sample, 24, "the old bounce still opens");

        // And a successful export leaves no scratch file behind.
        write_wav(file.path(), &test_buffer(), &WavExportSettings::default()).unwrap();
        assert!(!crate::project_file::in_progress_path(file.path()).exists());
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
        let file = TempFile::new("preserved.flac");
        let settings = AudioExportSettings {
            format: AudioExportFormat::Flac,
            ..AudioExportSettings::default()
        };
        write_audio(file.path(), &tone_buffer(4_096), &settings).unwrap();
        let before = std::fs::read(file.path()).unwrap();

        let result = write_audio_with_progress(
            file.path(),
            &tone_buffer(EXPORT_CHUNK_FRAMES * 3),
            &settings,
            &mut |fraction| fraction < 0.5,
        );
        assert!(matches!(result, Err(IoError::ExportCancelled)));
        assert_eq!(std::fs::read(file.path()).unwrap(), before);
        assert!(!crate::project_file::in_progress_path(file.path()).exists());
    }
}
