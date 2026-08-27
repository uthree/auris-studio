//! Decoding audio files into [`AudioBuffer`]s, with optional sample rate conversion.
//!
//! Import is a two-stage pipeline. [`decode_audio_file`] turns any container Symphonia can read
//! into planar `f32` at the file's own rate; [`import_audio_file`] then converts that to the
//! project rate. The stages are separate so a caller that only wants to inspect a file (to show
//! its length or channel layout) does not pay for a resample.
//!
//! Everything here allocates and blocks, so it must run on a worker thread, never on the audio
//! callback.

use std::fs::File;
use std::path::Path;

use auris_core::AudioBuffer;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::error::{IoError, Result};

/// Chunk size aimed for when building the resampler, in frames.
///
/// The FFT resampler rounds this up to a whole number of its internal blocks, so the exact value
/// only trades memory against per-call overhead. A thousand-ish frames is the range the rubato
/// documentation recommends for offline work.
const RESAMPLER_CHUNK_FRAMES: usize = 1024;

/// Largest FFT input block we are willing to build in exchange for a whole-frame delay.
///
/// Sample rate pairs that share almost no common factor (48 000 → 44 101, say) force a block as
/// long as the input rate itself; doubling that to make the block count even would cost more
/// memory than the alignment is worth, so [`resampler_chunk_frames`] gives up past this point.
const MAX_RESAMPLER_BLOCK_FRAMES: usize = 1 << 16;

/// The result of decoding a file, before any sample rate conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedAudio {
    /// Decoded samples, tagged with [`Self::source_sample_rate`].
    pub buffer: AudioBuffer,
    /// Sample rate the file was stored at.
    pub source_sample_rate: f64,
    /// Channel count of the file.
    pub channel_count: usize,
}

impl DecodedAudio {
    /// Length of the decoded audio in seconds, at its own sample rate.
    pub fn duration_seconds(&self) -> f64 {
        self.buffer.duration_seconds()
    }
}

/// File extensions the importer accepts, for a file-dialog filter.
///
/// This mirrors the containers and codecs enabled by the `all` feature of Symphonia. Probing is
/// content-based, so an unlisted extension may still decode; the list exists to give the dialog
/// something sensible to show.
pub fn supported_extensions() -> &'static [&'static str] {
    &[
        "wav", "wave", "aiff", "aif", "aifc", "caf", "flac", "mp3", "mp2", "mp1", "mp4", "m4a",
        "aac", "ogg", "oga", "mkv", "mka", "webm",
    ]
}

/// Decodes a whole audio file into planar `f32`, at the file's own sample rate.
///
/// Every sample format Symphonia can produce (`u8`/`u16`/`u24`/`u32`, `s8`/`s16`/`s24`/`s32`,
/// `f32`/`f64`) and any channel count are handled: the conversion goes through Symphonia's
/// generic buffer, which normalises integer formats to `[-1.0, 1.0]` for us.
pub fn decode_audio_file(path: &Path) -> Result<DecodedAudio> {
    let file = File::open(path).map_err(|e| IoError::from_fs(path, e))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    // The extension is only a hint; it lets the probe try the most likely reader first, and a
    // wrong or missing extension still resolves through content sniffing.
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        // Readers advertise their extensions in lower case, so `TRACK.WAV` has to be folded.
        hint.with_extension(&extension.to_ascii_lowercase());
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| match e {
            SymphoniaError::Unsupported(what) => IoError::UnsupportedFormat(what.to_string()),
            other => IoError::Decode(other.to_string()),
        })?;

    // The track borrow has to end before the decode loop can take `format` mutably.
    let (track_id, codec_params) = {
        let track = format.default_track(TrackType::Audio).ok_or_else(|| {
            IoError::UnsupportedFormat(format!("{} contains no audio track", path.display()))
        })?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or_else(|| {
                IoError::UnsupportedFormat(format!(
                    "{} has an audio track with unknown codec parameters",
                    path.display()
                ))
            })?
            .clone();
        (track.id, params)
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .map_err(|e| match e {
            SymphoniaError::Unsupported(what) => IoError::UnsupportedFormat(what.to_string()),
            other => IoError::Decode(other.to_string()),
        })?;

    // Declared values are only hints for allocation; the decoded buffers are authoritative.
    //
    // The declared rate deliberately does *not* seed `sample_rate`: for parametric codecs such as
    // HE-AAC the container advertises the base rate while the decoder emits twice that, and
    // treating the declared value as the first observation would reject those files as if the
    // rate changed mid-stream. Only rates seen on decoded packets are compared against each other.
    let declared_sample_rate = codec_params.sample_rate.map(f64::from);
    let mut sample_rate: Option<f64> = None;
    let mut channels: Vec<Vec<f32>> = match codec_params.channels.as_ref().map(|c| c.count()) {
        Some(count) if count > 0 => vec![Vec::new(); count],
        _ => Vec::new(),
    };
    let mut scratch: Vec<Vec<f32>> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A truncated final packet is common in files that were cut mid-write; treat it as
            // the end of the stream rather than losing everything decoded so far.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                log::warn!("stopping decode of {}: {e}", path.display());
                break;
            }
            Err(e) => return Err(IoError::Decode(e.to_string())),
        };

        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A single corrupt packet must not abort an otherwise good file.
            Err(SymphoniaError::DecodeError(what)) => {
                log::warn!("skipping undecodable packet in {}: {what}", path.display());
                continue;
            }
            Err(e) => return Err(IoError::Decode(e.to_string())),
        };

        let spec = decoded.spec();
        let packet_rate = f64::from(spec.rate());
        if packet_rate > 0.0 {
            match sample_rate {
                Some(rate) if rate != packet_rate => {
                    return Err(IoError::Decode(format!(
                        "sample rate changes from {rate} Hz to {packet_rate} Hz partway through \
                         {}, which is not supported",
                        path.display()
                    )));
                }
                _ => sample_rate = Some(packet_rate),
            }
        }

        if decoded.frames() == 0 {
            continue;
        }

        decoded.copy_to_vecs_planar::<f32>(&mut scratch);
        if channels.len() != scratch.len() {
            if channels_have_data(&channels) {
                return Err(IoError::Decode(format!(
                    "channel count changes from {} to {} partway through {}",
                    channels.len(),
                    scratch.len(),
                    path.display()
                )));
            }
            channels = vec![Vec::new(); scratch.len()];
        }
        for (destination, source) in channels.iter_mut().zip(&scratch) {
            destination.extend_from_slice(source);
        }
    }

    // Nothing decoded (an audio track with no packets) leaves the declared rate as the only
    // information available, which is enough to describe the file even when it says nothing.
    let sample_rate = sample_rate
        .or(declared_sample_rate)
        .filter(|rate| *rate > 0.0)
        .ok_or_else(|| {
            IoError::UnsupportedFormat(format!("{} declares no sample rate", path.display()))
        })?;
    if channels.is_empty() {
        return Err(IoError::UnsupportedFormat(format!(
            "{} decoded to zero channels",
            path.display()
        )));
    }
    // A header with nothing behind it — a `data` chunk of length zero, or a stream whose every
    // packet failed to decode — used to come back as a buffer of no frames, on the grounds that
    // it honestly described an empty file. It does, and there is nothing to be done with it: a
    // clip of no frames cannot be played, drawn, faded or split, and dragging its edge divided
    // by its own length. The recorder already refuses to keep a take that captured nothing, for
    // the same reason and in the same words; this is the importer agreeing with it.
    if !channels_have_data(&channels) {
        return Err(IoError::UnsupportedFormat(format!(
            "{} contains no audio",
            path.display()
        )));
    }

    // A broken float file carries NaN and infinity as readily as audio — the bit pattern *is*
    // the sample — and one such sample is enough to latch every feedback filter it later flows
    // through. Read as silence at the door, the way the export path writes it, so the poison
    // never enters the project at all. Counted rather than silent: a file that needed this is
    // a file worth a line in the log.
    let mut poisoned = 0usize;
    for channel in &mut channels {
        for sample in channel.iter_mut() {
            if !sample.is_finite() {
                *sample = 0.0;
                poisoned += 1;
            }
        }
    }
    if poisoned > 0 {
        log::warn!(
            "{poisoned} non-finite samples in {} were read as silence",
            path.display()
        );
    }

    let channel_count = channels.len();
    let buffer = AudioBuffer::from_planar(channels, sample_rate)
        .map_err(|e| IoError::Decode(e.to_string()))?;

    Ok(DecodedAudio {
        buffer,
        source_sample_rate: sample_rate,
        channel_count,
    })
}

fn channels_have_data(channels: &[Vec<f32>]) -> bool {
    channels.iter().any(|channel| !channel.is_empty())
}

/// Decodes a file and converts it to `target_sample_rate`.
///
/// When the file already runs at the target rate the samples are returned untouched — running
/// them through a resampler at a ratio of exactly 1 would still apply the anti-alias filter and
/// audibly dull the top octave for no reason.
pub fn import_audio_file(path: &Path, target_sample_rate: f64) -> Result<AudioBuffer> {
    let decoded = decode_audio_file(path)?;
    resample_buffer(&decoded.buffer, target_sample_rate)
}

/// Greatest common divisor, used to work out the resampler's minimum FFT block size.
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Picks the chunk size to build the FFT resampler with, for a given rate pair.
///
/// With `FixedSync::Both`, rubato turns the requested chunk size into an FFT block of
/// `blocks * source / gcd(source, target)` input frames and `blocks * target / gcd` output
/// frames, where `blocks = ceil(chunk / (source / gcd))`. Its startup delay is *half* the output
/// block, rounded down — so an odd output block leaves half an output frame of delay that no
/// amount of trimming can remove, and the resampled audio ends up half a sample late.
///
/// Rounding `blocks` up to an even number makes the output block even as well, which makes the
/// delay a whole number of frames and lets [`resample_buffer`] line the output up with the input
/// exactly. For 48 kHz → 44.1 kHz that is the difference between a peak error of 7 % and 0.03 %
/// against an ideal band-limited resample.
fn resampler_chunk_frames(source_hz: usize, target_hz: usize) -> usize {
    let min_block_in = source_hz / gcd(source_hz, target_hz);
    if min_block_in == 0 || min_block_in > MAX_RESAMPLER_BLOCK_FRAMES / 2 {
        return RESAMPLER_CHUNK_FRAMES;
    }
    let block_pairs = RESAMPLER_CHUNK_FRAMES.div_ceil(2 * min_block_in).max(1);
    block_pairs * 2 * min_block_in
}

/// Converts `buffer` from its own sample rate to `target_sample_rate`.
///
/// Uses rubato's synchronous FFT resampler, which is the highest-quality option for a fixed
/// ratio and is fast enough to convert a long file in a fraction of real time. The resampler's
/// startup delay is trimmed, so the output lines up with the input and its length is the ideal
/// `ceil(frames * target / source)`.
///
/// The chunk loop is written out here rather than delegated to rubato's `process_all_into_buffer`
/// because that helper only trims the startup delay from *inside* its main loop: a clip shorter
/// than one chunk skips the loop entirely and comes back with the delay still on the front (and
/// its tail cut off to compensate), and when the delay is not exactly half of one output chunk
/// the trim copies the wrong number of frames and corrupts one sample at the seam.
pub fn resample_buffer(buffer: &AudioBuffer, target_sample_rate: f64) -> Result<AudioBuffer> {
    let source_sample_rate = buffer.sample_rate();
    if !source_sample_rate.is_finite() || source_sample_rate <= 0.0 {
        return Err(IoError::Resample(format!(
            "source sample rate {source_sample_rate} is not a positive number"
        )));
    }
    if !target_sample_rate.is_finite() || target_sample_rate <= 0.0 {
        return Err(IoError::Resample(format!(
            "target sample rate {target_sample_rate} is not a positive number"
        )));
    }

    // The synchronous resampler works from an integer ratio. Every real sample rate is a whole
    // number of hertz, so rounding here is exact in practice and keeps the ratio rational.
    let source_hz = source_sample_rate.round() as usize;
    let target_hz = target_sample_rate.round() as usize;

    let channel_count = buffer.channel_count();
    let input_frames = buffer.frame_count();

    if source_hz == target_hz || input_frames == 0 {
        let mut passthrough = buffer.clone();
        passthrough.set_sample_rate(target_sample_rate);
        return Ok(passthrough);
    }

    // A rate below half a hertz rounds to zero, which would divide by zero below.
    if source_hz == 0 || target_hz == 0 {
        return Err(IoError::Resample(format!(
            "sample rates {source_sample_rate} Hz and {target_sample_rate} Hz are too low to \
             resample between"
        )));
    }

    let mut resampler = Fft::<f32>::new(
        source_hz,
        target_hz,
        resampler_chunk_frames(source_hz, target_hz),
        channel_count,
        FixedSync::Both,
    )
    .map_err(|e| IoError::Resample(e.to_string()))?;

    // Ideal output length, in exact integer arithmetic so it cannot drift by a frame the way
    // `(frames as f64 * ratio).ceil()` can for long files.
    let output_frames =
        ((input_frames as u128 * target_hz as u128).div_ceil(source_hz as u128)) as usize;
    let delay = resampler.output_delay();
    let chunk_frames_out = resampler.output_frames_max();
    // Room for the startup delay, the audio itself, and the overshoot of the final chunk.
    let capacity = delay + output_frames + chunk_frames_out;

    let input = SequentialSliceOfVecs::new(buffer.channels(), channel_count, input_frames)
        .map_err(|e| IoError::Resample(e.to_string()))?;
    let mut output_planes = vec![vec![0.0f32; capacity]; channel_count];

    {
        let mut output =
            SequentialSliceOfVecs::new_mut(&mut output_planes, channel_count, capacity)
                .map_err(|e| IoError::Resample(e.to_string()))?;
        let mut indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(0),
            active_channels_mask: None,
        };
        let mut consumed = 0usize;
        let mut produced = 0usize;
        while produced < delay + output_frames {
            // Once the input is exhausted `partial_len` goes to zero and the resampler is fed
            // silence, which is what flushes the tail of the anti-alias filter back out.
            let take = (input_frames - consumed).min(resampler.input_frames_next());
            indexing.input_offset = consumed;
            indexing.output_offset = produced;
            indexing.partial_len = Some(take);
            let (_read, written) = resampler
                .process_into_buffer(&input, &mut output, Some(&indexing))
                .map_err(|e| IoError::Resample(e.to_string()))?;
            if written == 0 {
                return Err(IoError::Resample(format!(
                    "resampler stopped producing output after {produced} of \
                     {} frames",
                    delay + output_frames
                )));
            }
            consumed += take;
            produced += written;
        }
    }

    for plane in &mut output_planes {
        plane.copy_within(delay..delay + output_frames, 0);
        plane.truncate(output_frames);
    }
    AudioBuffer::from_planar(output_planes, target_sample_rate)
        .map_err(|e| IoError::Resample(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{WavBitDepth, WavExportSettings, write_wav};
    use crate::test_support::TempFile;

    /// Amplitude of the magnitude spectrum of a real sine of amplitude `a`, at its own frequency,
    /// when normalised by the window length: `a / 2`.
    fn magnitude_at(samples: &[f32], sample_rate: f64, hz: f64) -> f64 {
        let mut real = 0.0f64;
        let mut imaginary = 0.0f64;
        for (n, sample) in samples.iter().enumerate() {
            let phase = std::f64::consts::TAU * hz * n as f64 / sample_rate;
            real += f64::from(*sample) * phase.cos();
            imaginary -= f64::from(*sample) * phase.sin();
        }
        (real * real + imaginary * imaginary).sqrt() / samples.len() as f64
    }

    fn sine(frames: usize, sample_rate: f64, hz: f64, channels: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::new(channels, frames, sample_rate);
        for channel in 0..channels {
            for frame in 0..frames {
                let phase = std::f64::consts::TAU * hz * frame as f64 / sample_rate;
                buffer.channel_mut(channel)[frame] = phase.sin() as f32;
            }
        }
        buffer
    }

    /// Worst absolute deviation of `samples` from the sine an ideal resampler would have
    /// produced, ignoring `edge` frames at each end where the anti-alias filter tapers.
    ///
    /// Comparing sample by sample — rather than looking only at a spectrum — is what catches a
    /// resampler whose output is delayed, truncated, or has a single corrupt frame at a chunk
    /// seam. None of those move a 1 kHz line far enough for a DFT to notice.
    fn worst_deviation_from_sine(
        samples: &[f32],
        sample_rate: f64,
        hz: f64,
        edge: usize,
    ) -> (f64, usize) {
        assert!(
            samples.len() > 2 * edge,
            "window is shorter than the edges to skip"
        );
        let mut worst = 0.0f64;
        let mut worst_at = 0usize;
        for (frame, sample) in samples
            .iter()
            .enumerate()
            .take(samples.len() - edge)
            .skip(edge)
        {
            let ideal = (std::f64::consts::TAU * hz * frame as f64 / sample_rate).sin();
            let error = (f64::from(*sample) - ideal).abs();
            if error > worst {
                worst = error;
                worst_at = frame;
            }
        }
        (worst, worst_at)
    }

    #[test]
    fn supported_extensions_cover_the_common_containers() {
        let extensions = supported_extensions();
        for expected in ["wav", "flac", "mp3", "ogg", "m4a", "aiff"] {
            assert!(extensions.contains(&expected), "missing `{expected}`");
        }
    }

    #[test]
    fn non_finite_samples_read_as_silence() {
        // The exporter refuses to write these, so the file is built by hand: a minimal 32-bit
        // float WAV whose middle samples are a NaN and an infinity, which a broken encoder
        // writes as readily as audio — the bit pattern *is* the sample. One of them is enough
        // to latch a feedback filter downstream, so the importer has to read them as silence.
        let file = TempFile::new("poisoned.wav");
        let samples = [0.25f32, f32::NAN, f32::NEG_INFINITY, -0.25];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&52u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(file.path(), &bytes).unwrap();

        let decoded = decode_audio_file(file.path()).unwrap();
        assert_eq!(decoded.buffer.channel(0), &[0.25, 0.0, 0.0, -0.25]);
    }

    #[test]
    fn importing_a_written_wav_preserves_frame_and_channel_count() {
        let file = TempFile::new("import-round-trip.wav");
        let source = sine(4_096, 48_000.0, 440.0, 2);
        let settings = WavExportSettings {
            bit_depth: WavBitDepth::Int24,
            sample_rate: 48_000,
            dither: false,
        };
        write_wav(file.path(), &source, &settings).unwrap();

        let decoded = decode_audio_file(file.path()).unwrap();
        assert_eq!(decoded.channel_count, 2);
        assert_eq!(decoded.source_sample_rate, 48_000.0);
        assert_eq!(decoded.buffer.frame_count(), 4_096);
        assert_eq!(decoded.buffer.channel_count(), 2);

        let imported = import_audio_file(file.path(), 48_000.0).unwrap();
        assert_eq!(imported.frame_count(), 4_096);
        assert_eq!(imported.channel_count(), 2);
        assert_eq!(imported.sample_rate(), 48_000.0);

        // 24-bit quantisation is 1/2^23; the decoded samples must match that closely.
        for frame in 0..source.frame_count() {
            let error = (imported.channel(0)[frame] - source.channel(0)[frame]).abs();
            assert!(
                error <= 1.0 / 8_388_608.0,
                "frame {frame} drifted by {error}"
            );
        }
    }

    #[test]
    fn a_16_bit_round_trip_is_bit_exact() {
        // The exporter scales by 2^15 and Symphonia normalises 16-bit samples by the same
        // factor, so any level that already sits on a 16-bit code has to survive the round trip
        // unchanged. Scaling the export by 32767 instead would put every sample slightly off.
        let file = TempFile::new("round-trip-16.wav");
        let codes: Vec<f32> = [-32_768i32, -32_767, -16_384, -1, 0, 1, 4_096, 32_767]
            .iter()
            .map(|code| *code as f32 / 32_768.0)
            .collect();
        let buffer = AudioBuffer::from_planar(vec![codes.clone()], 44_100.0).unwrap();
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

        let decoded = decode_audio_file(file.path()).unwrap();
        assert_eq!(decoded.source_sample_rate, 44_100.0);
        assert_eq!(decoded.buffer.channel(0), codes.as_slice());
    }

    #[test]
    fn importing_at_a_different_rate_resamples() {
        let file = TempFile::new("import-resample.wav");
        let source = sine(48_000, 48_000.0, 1_000.0, 1);
        write_wav(
            file.path(),
            &source,
            &WavExportSettings {
                bit_depth: WavBitDepth::Float32,
                sample_rate: 48_000,
                dither: false,
            },
        )
        .unwrap();

        let imported = import_audio_file(file.path(), 44_100.0).unwrap();
        assert_eq!(imported.sample_rate(), 44_100.0);
        assert_eq!(imported.frame_count(), 44_100);
    }

    #[test]
    fn resampling_48k_to_44k1_keeps_length_and_pitch() {
        let source = sine(48_000, 48_000.0, 1_000.0, 1);
        let resampled = resample_buffer(&source, 44_100.0).unwrap();

        let ideal = 48_000.0 * 44_100.0 / 48_000.0;
        let actual = resampled.frame_count() as f64;
        assert!(
            (actual - ideal).abs() / ideal <= 0.005,
            "got {actual} frames, ideal is {ideal}"
        );
        assert_eq!(resampled.sample_rate(), 44_100.0);

        // Analyse 900 whole cycles (39 690 frames at 44.1 kHz) starting past the resampler's
        // startup transient, so there is no spectral leakage to argue about.
        let window = &resampled.channel(0)[2_205..2_205 + 39_690];
        let at_1k = magnitude_at(window, 44_100.0, 1_000.0);
        let at_900 = magnitude_at(window, 44_100.0, 900.0);
        let at_1100 = magnitude_at(window, 44_100.0, 1_100.0);

        // A unit-amplitude sine has a half-amplitude spectral line at its own frequency.
        assert!((at_1k - 0.5).abs() < 0.01, "1 kHz magnitude was {at_1k}");
        assert!(at_900 < 0.005, "900 Hz magnitude was {at_900}");
        assert!(at_1100 < 0.005, "1100 Hz magnitude was {at_1100}");
    }

    #[test]
    fn resampling_48k_to_44k1_lines_up_with_the_input_everywhere() {
        let source = sine(48_000, 48_000.0, 1_000.0, 1);
        let resampled = resample_buffer(&source, 44_100.0).unwrap();
        assert_eq!(resampled.frame_count(), 44_100);

        // Every frame — not just a hand-picked window — has to sit on the ideal sine. A DFT
        // cannot see a single bad sample among forty thousand, and a delayed output still shows
        // a clean 1 kHz line, so the alignment claim has to be checked in the time domain.
        let (worst, at) = worst_deviation_from_sine(resampled.channel(0), 44_100.0, 1_000.0, 64);
        assert!(worst < 1e-3, "frame {at} deviates by {worst}");

        // The very first frames must already carry the signal: a resampler whose startup delay
        // was left in place would open with a few hundred frames of near-silence.
        let head_peak = resampled.channel(0)[..64]
            .iter()
            .fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(head_peak > 0.9, "output starts quiet, peak was {head_peak}");
    }

    #[test]
    fn resampling_a_clip_shorter_than_one_chunk_is_still_aligned() {
        // 500 frames is well under the resampler's internal chunk, which is the case where a
        // naive "trim inside the chunk loop" never trims at all and returns delayed, truncated
        // audio. Anything the user drops on the timeline may be this short.
        let source = sine(500, 48_000.0, 1_000.0, 1);
        let resampled = resample_buffer(&source, 44_100.0).unwrap();
        assert_eq!(resampled.frame_count(), 460); // ceil(500 * 44100 / 48000)

        let (worst, at) = worst_deviation_from_sine(resampled.channel(0), 44_100.0, 1_000.0, 64);
        assert!(worst < 5e-3, "frame {at} deviates by {worst}");
        assert!(
            resampled.channel(0)[..32].iter().any(|s| s.abs() > 0.5),
            "a short clip came back silent at the start"
        );
    }

    #[test]
    fn resampling_to_the_same_rate_is_a_passthrough() {
        let source = sine(1_024, 48_000.0, 1_000.0, 2);
        let resampled = resample_buffer(&source, 48_000.0).unwrap();
        assert_eq!(resampled.frame_count(), 1_024);
        assert_eq!(resampled.channel(0), source.channel(0));
        assert_eq!(resampled.channel(1), source.channel(1));
    }

    #[test]
    fn upsampling_doubles_the_frame_count() {
        let source = sine(4_800, 48_000.0, 1_000.0, 1);
        let resampled = resample_buffer(&source, 96_000.0).unwrap();
        assert_eq!(resampled.frame_count(), 9_600);
        assert_eq!(resampled.sample_rate(), 96_000.0);

        let (worst, at) = worst_deviation_from_sine(resampled.channel(0), 96_000.0, 1_000.0, 64);
        assert!(worst < 1e-3, "frame {at} deviates by {worst}");
    }

    #[test]
    fn upsampling_44k1_to_48k_produces_the_exact_ideal_length() {
        // 44 100 * 48 000 / 44 100 is a whole number, so the output must be exactly 48 000
        // frames. Working the length out in floating point rounds up to 48 001 here.
        let source = sine(44_100, 44_100.0, 1_000.0, 1);
        let resampled = resample_buffer(&source, 48_000.0).unwrap();
        assert_eq!(resampled.frame_count(), 48_000);

        let (worst, at) = worst_deviation_from_sine(resampled.channel(0), 48_000.0, 1_000.0, 64);
        assert!(worst < 1e-3, "frame {at} deviates by {worst}");
    }

    #[test]
    fn resampling_keeps_channels_independent_and_in_phase() {
        // Two channels carrying different signals must come back separated and frame-aligned;
        // a resampler fed the wrong plane, or one channel trimmed differently from the other,
        // shows up here as a phase or amplitude error.
        let mut source = AudioBuffer::new(2, 12_000, 48_000.0);
        for frame in 0..12_000 {
            let phase = std::f64::consts::TAU * 1_000.0 * frame as f64 / 48_000.0;
            source.channel_mut(0)[frame] = phase.sin() as f32;
            source.channel_mut(1)[frame] = -0.5 * phase.sin() as f32;
        }

        let resampled = resample_buffer(&source, 44_100.0).unwrap();
        assert_eq!(resampled.channel_count(), 2);
        assert_eq!(resampled.frame_count(), 11_025);

        let (worst, at) = worst_deviation_from_sine(resampled.channel(0), 44_100.0, 1_000.0, 64);
        assert!(worst < 1e-3, "left frame {at} deviates by {worst}");
        for frame in 64..resampled.frame_count() - 64 {
            let left = resampled.channel(0)[frame];
            let right = resampled.channel(1)[frame];
            assert!(
                (right + 0.5 * left).abs() < 2e-3,
                "channels drifted apart at frame {frame}: {left} vs {right}"
            );
        }
    }

    #[test]
    fn resampling_a_mono_buffer_of_a_few_frames_does_not_panic() {
        // Buffer sizes far below any chunk size still have to produce the ideal length.
        for frames in [1usize, 2, 7, 63] {
            let source = sine(frames, 48_000.0, 1_000.0, 1);
            let resampled = resample_buffer(&source, 44_100.0).unwrap();
            let expected = (frames * 44_100).div_ceil(48_000);
            assert_eq!(
                resampled.frame_count(),
                expected,
                "{frames} frames resampled to the wrong length"
            );
            assert_eq!(resampled.channel_count(), 1);
        }
    }

    #[test]
    fn an_empty_buffer_resamples_to_an_empty_buffer() {
        let source = AudioBuffer::new(2, 0, 48_000.0);
        let resampled = resample_buffer(&source, 44_100.0).unwrap();
        assert_eq!(resampled.frame_count(), 0);
        assert_eq!(resampled.channel_count(), 2);
        assert_eq!(resampled.sample_rate(), 44_100.0);
    }

    #[test]
    fn a_non_positive_sample_rate_is_rejected_rather_than_panicking() {
        let source = sine(128, 48_000.0, 1_000.0, 1);
        assert!(matches!(
            resample_buffer(&source, 0.0),
            Err(IoError::Resample(_))
        ));
        assert!(matches!(
            resample_buffer(&source, f64::NAN),
            Err(IoError::Resample(_))
        ));

        let mut bad = source.clone();
        bad.set_sample_rate(-1.0);
        assert!(matches!(
            resample_buffer(&bad, 48_000.0),
            Err(IoError::Resample(_))
        ));
    }

    #[test]
    fn a_missing_file_reports_file_not_found() {
        let path = std::env::temp_dir().join("auris-io-definitely-missing.wav");
        match decode_audio_file(&path) {
            Err(IoError::FileNotFound(reported)) => assert_eq!(reported, path),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }

    /// A WAV that declares two channels at 44.1 kHz and carries no sample data at all.
    ///
    /// Not a corrupt file: every field is where it should be and every length agrees. It is the
    /// shape a recording that was started and stopped in the same instant leaves behind, and the
    /// shape a copy interrupted before its first block gets written leaves behind.
    fn silent_header() -> Vec<u8> {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // channels
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&176_400u32.to_le_bytes()); // bytes per second
        wav.extend_from_slice(&4u16.to_le_bytes()); // bytes per frame
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0u32.to_le_bytes());
        wav
    }

    #[test]
    fn a_file_that_decodes_to_no_frames_is_refused_rather_than_imported_empty() {
        let file = TempFile::new("no-audio.wav");
        std::fs::write(file.path(), silent_header()).unwrap();

        let error = decode_audio_file(file.path()).expect_err("a file with no audio in it");
        assert!(
            matches!(error, IoError::UnsupportedFormat(ref what) if what.contains("no audio")),
            "expected an unsupported-format error naming the emptiness, got {error:?}"
        );

        // And through the importer, which is the door the session actually uses.
        assert!(import_audio_file(file.path(), 48_000.0).is_err());
    }
}
