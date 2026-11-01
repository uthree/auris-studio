//! Finding an installed `ffmpeg` and piping frames into it.
//!
//! Auris never carries an ffmpeg of its own. One that is already installed is looked for where
//! the platform's package managers put it, which is not always on the `PATH` a program sees: an
//! application opened from Finder inherits launchd's short `PATH`, and one installed by winget
//! only reaches the `PATH` of programs started after it.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread::JoinHandle;

use crate::VideoError;
use crate::style::VideoSize;

/// The command that installs ffmpeg with this platform's usual package manager.
pub fn install_command() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew install ffmpeg"
    } else if cfg!(target_os = "windows") {
        "winget install --id Gyan.FFmpeg"
    } else {
        "sudo apt install ffmpeg"
    }
}

/// Where ffmpeg can be downloaded by hand.
pub fn install_page() -> &'static str {
    "https://ffmpeg.org/download.html"
}

/// An H.264 (or, failing that, MPEG-4) encoder an ffmpeg build may carry, best first.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    /// x264, the encoder most ffmpeg builds carry and the best of these at a given size.
    X264,
    /// Apple's hardware encoder, in ffmpeg builds for macOS.
    VideoToolbox,
    /// Windows' Media Foundation encoder, in ffmpeg builds for Windows.
    MediaFoundation,
    /// Cisco's OpenH264.
    OpenH264,
    /// ffmpeg's own MPEG-4 Part 2 encoder: in every build, larger files, and still an `.mp4`.
    Mpeg4,
}

impl VideoCodec {
    /// Every codec, in order of preference.
    pub const PREFERENCE: [VideoCodec; 5] = [
        VideoCodec::X264,
        VideoCodec::VideoToolbox,
        VideoCodec::MediaFoundation,
        VideoCodec::OpenH264,
        VideoCodec::Mpeg4,
    ];

    /// ffmpeg's name for the encoder.
    pub fn encoder(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::VideoToolbox => "h264_videotoolbox",
            Self::MediaFoundation => "h264_mf",
            Self::OpenH264 => "libopenh264",
            Self::Mpeg4 => "mpeg4",
        }
    }

    /// Encoder arguments for a video of `size` at `fps`.
    fn arguments(self, size: VideoSize, fps: u32) -> Vec<String> {
        // The bitrate the encoders without a quality mode are given: about 15 Mb/s at 1080p60,
        // generous for flat colour and hard edges, which is what a note roll is.
        let bitrate = format!(
            "{}k",
            (f64::from(size.width) * f64::from(size.height) * f64::from(fps) * 0.12 / 1000.0)
                .round()
                .max(500.0)
        );
        let list: &[&str] = match self {
            // `animation` is the tuning for large flat areas and sharp edges.
            Self::X264 => &[
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-tune",
                "animation",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
            ],
            Self::VideoToolbox => &["-c:v", "h264_videotoolbox", "-pix_fmt", "yuv420p", "-b:v"],
            // Media Foundation's encoders take NV12 and nothing else.
            Self::MediaFoundation => &["-c:v", "h264_mf", "-pix_fmt", "nv12", "-b:v"],
            Self::OpenH264 => &["-c:v", "libopenh264", "-pix_fmt", "yuv420p", "-b:v"],
            Self::Mpeg4 => &["-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p"],
        };
        let mut arguments: Vec<String> = list.iter().map(|arg| (*arg).to_string()).collect();
        if arguments.last().is_some_and(|last| last == "-b:v") {
            arguments.push(bitrate);
        }
        arguments
    }
}

/// An ffmpeg found on this machine, and the encoder it will be asked to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ffmpeg {
    path: PathBuf,
    codec: VideoCodec,
    version: String,
}

impl Ffmpeg {
    /// Finds an installed ffmpeg and checks it can make an `.mp4`.
    pub fn detect() -> Result<Self, VideoError> {
        let mut last = None;
        for candidate in candidates() {
            if !candidate.is_file() {
                continue;
            }
            match Self::probe(&candidate) {
                Ok(found) => return Ok(found),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or(VideoError::FfmpegMissing(install_command())))
    }

    /// Asks the ffmpeg at `path` what it is and which encoders it has.
    pub fn probe(path: &Path) -> Result<Self, VideoError> {
        let version = run(path, &["-hide_banner", "-version"])?;
        let version = version
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("ffmpeg version "))
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("unknown")
            .to_string();
        let encoders = run(path, &["-hide_banner", "-encoders"])?;
        let names: Vec<&str> = encoders
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1))
            .collect();
        if !names.contains(&"aac") {
            return Err(VideoError::Ffmpeg(format!(
                "{} has no AAC encoder",
                path.display()
            )));
        }
        let codec = VideoCodec::PREFERENCE
            .into_iter()
            .find(|codec| names.contains(&codec.encoder()))
            .ok_or_else(|| {
                VideoError::Ffmpeg(format!("{} has no H.264 encoder", path.display()))
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            codec,
            version,
        })
    }

    /// Where the program is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The encoder it will be asked for.
    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    /// Its version, as it reports it.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Every place an ffmpeg might be, most specific first.
fn candidates() -> Vec<PathBuf> {
    let name = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    let mut found = Vec::new();
    if let Some(chosen) = std::env::var_os("AURIS_FFMPEG") {
        found.push(PathBuf::from(chosen));
    }
    if let Some(path) = std::env::var_os("PATH") {
        found.extend(std::env::split_paths(&path).map(|dir| dir.join(&name)));
    }
    let mut extra: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        // Homebrew on Apple silicon and on Intel, then MacPorts.
        extra.extend(
            ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"]
                .into_iter()
                .map(PathBuf::from),
        );
    } else if cfg!(target_os = "windows") {
        let under = |variable: &str, rest: &[&str]| {
            std::env::var_os(variable)
                .map(|base| rest.iter().fold(PathBuf::from(base), |p, s| p.join(s)))
        };
        extra.extend(
            [
                under("LOCALAPPDATA", &["Microsoft", "WinGet", "Links"]),
                under("USERPROFILE", &["scoop", "shims"]),
                under("ProgramData", &["chocolatey", "bin"]),
                under("ProgramFiles", &["ffmpeg", "bin"]),
                Some(PathBuf::from(r"C:\ffmpeg\bin")),
            ]
            .into_iter()
            .flatten(),
        );
    } else {
        extra.extend(
            ["/usr/bin", "/usr/local/bin", "/snap/bin"]
                .into_iter()
                .map(PathBuf::from),
        );
    }
    found.extend(extra.into_iter().map(|dir| dir.join(&name)));
    found
}

/// A command for `program` that opens no console window of its own on Windows.
fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI application starting a console program otherwise flashes a
        // terminal on screen for as long as it runs.
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Runs `program` to completion and returns what it printed.
fn run(program: &Path, arguments: &[&str]) -> Result<String, VideoError> {
    let output = command(program)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| VideoError::Ffmpeg(format!("{}: {error}", program.display())))?;
    if !output.status.success() {
        return Err(VideoError::Ffmpeg(format!(
            "{} exited with {}",
            program.display(),
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A running ffmpeg turning raw frames into an `.mp4`.
///
/// Dropping it before [`Self::finish`] stops the process, which is how a cancelled export ends.
pub struct VideoWriter {
    child: Child,
    stdin: Option<ChildStdin>,
    log: Option<JoinHandle<String>>,
    frame_bytes: usize,
}

impl VideoWriter {
    /// Starts ffmpeg writing `output`, from frames of `size` at `fps` and the audio in `audio`.
    ///
    /// Frames are RGBA, eight bits a channel, rows top to bottom with no padding — what a
    /// `tiny_skia::Pixmap` holds when everything in it is opaque.
    pub fn start(
        ffmpeg: &Ffmpeg,
        size: VideoSize,
        fps: u32,
        audio: Option<&Path>,
        output: &Path,
    ) -> Result<Self, VideoError> {
        let mut command = command(&ffmpeg.path);
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        command.args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{}x{}", size.width, size.height),
            "-framerate",
            &fps.to_string(),
            "-i",
            "pipe:0",
        ]);
        if let Some(audio) = audio {
            command.arg("-i").arg(audio);
            command.args(["-map", "0:v:0", "-map", "1:a:0"]);
        }
        command.args(ffmpeg.codec.arguments(size, fps));
        if audio.is_some() {
            command.args(["-c:a", "aac", "-b:a", "256k"]);
        }
        command.args(["-movflags", "+faststart", "-f", "mp4"]);
        command.arg(output);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| VideoError::Ffmpeg(format!("{}: {error}", ffmpeg.path.display())))?;
        let stdin = child.stdin.take();
        // Read on a thread of its own: a chatty ffmpeg that filled the pipe while nobody read
        // it would stop reading frames, and the writer would wait on it for ever.
        let log = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            })
        });
        Ok(Self {
            child,
            stdin,
            log,
            frame_bytes: size.width as usize * size.height as usize * 4,
        })
    }

    /// Hands over one frame.
    pub fn write_frame(&mut self, rgba: &[u8]) -> Result<(), VideoError> {
        if rgba.len() != self.frame_bytes {
            return Err(VideoError::Ffmpeg(format!(
                "a frame of {} bytes where {} were expected",
                rgba.len(),
                self.frame_bytes
            )));
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(VideoError::Ffmpeg("the encoder input is closed".into()));
        };
        if stdin.write_all(rgba).is_err() {
            // ffmpeg has gone; what it said on the way out is the useful part.
            self.stdin = None;
            let _ = self.child.wait();
            return Err(VideoError::Ffmpeg(self.take_log()));
        }
        Ok(())
    }

    /// Ends the input and waits for ffmpeg to finish the file.
    pub fn finish(mut self) -> Result<(), VideoError> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        let log = self.take_log();
        if status.success() {
            Ok(())
        } else {
            Err(VideoError::Ffmpeg(if log.is_empty() {
                format!("exited with {status}")
            } else {
                log
            }))
        }
    }

    fn take_log(&mut self) -> String {
        let text = self
            .log
            .take()
            .and_then(|log| log.join().ok())
            .unwrap_or_default();
        // The last lines are the reason; the first are usually configuration noise.
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        lines[lines.len().saturating_sub(6)..].join("\n")
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        if self.stdin.take().is_some() || self.log.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_has_an_install_hint() {
        assert!(install_command().contains("ffmpeg") || install_command().contains("FFmpeg"));
        assert!(install_page().starts_with("https://"));
    }

    #[test]
    fn rate_controlled_encoders_are_given_a_bitrate() {
        let arguments = VideoCodec::MediaFoundation.arguments(VideoSize::new(1920, 1080), 60);
        assert_eq!(arguments.last().map(String::as_str), Some("14930k"));
        assert!(arguments.contains(&"nv12".to_string()));
        let arguments = VideoCodec::X264.arguments(VideoSize::new(1920, 1080), 60);
        assert!(!arguments.iter().any(|arg| arg == "-b:v"));
    }

    #[test]
    fn a_missing_program_is_not_found() {
        assert!(Ffmpeg::probe(Path::new("/definitely/not/ffmpeg")).is_err());
    }
}
