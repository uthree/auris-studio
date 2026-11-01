//! Bounded audio-material discovery and copy-on-write processing for agent transports.
use super::{PreparedAudioSource, Session};
use crate::{Edit, prelude::*};
use auris_core::AssetPath;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Audio material operations shared by the panel and saved-project tools.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Search Wikimedia Commons audio without an API key. Returns direct URLs and attributed license/creator metadata, not a musical suitability judgment. Requires network authorization.
    SearchOnline {
        /// Sound keywords; English terms generally match more files. At most 256 bytes.
        query: String,
        /// Result count, 1..10; default 5.
        limit: Option<usize>,
        /// Offset returned by the previous page; default zero.
        #[serde(default)]
        offset: usize,
    },
    /// Search supported audio filenames, not audio content. Bounded to 10000 entries and depth 8; symlinks are skipped. Inspect completeness before claiming absence.
    Search {
        /// Absolute local directory; omitted searches the OS Music and Downloads folders.
        root: Option<String>,
        /// Case-insensitive words matched against the relative path; empty lists audio files.
        query: String,
        /// Descend into subdirectories when true.
        #[serde(default)]
        recursive: bool,
        /// Zero-based result offset.
        #[serde(default)]
        offset: usize,
        /// Page size, 1..100; default 20.
        limit: Option<usize>,
    },
    /// Read exact clip/source windows, file provenance and source-clock measurements.
    Inspect {
        /// Audio clip ID from inspect_project.
        clip: u64,
    },
    /// Download a direct HTTP(S) audio file, decode it and add a new audio track at the requested tick. At most 64 MiB/10 minutes; webpages and archives are not audio. No cookies or credentials are sent. Original attribution claims persist in the project.
    ImportUrl {
        /// Direct audio URL, without embedded credentials or a fragment.
        url: String,
        /// Nonnegative absolute timeline tick.
        at_tick: i64,
        /// Track/clip label, 1..256 UTF-8 bytes.
        name: String,
        /// Original source page, if known; not a download URL substitute.
        source_page: Option<String>,
        /// Creator/credit text, if known. Not verified by this tool.
        attribution: Option<String>,
        /// Actual license identifier/text, if known; omit when unknown.
        license: Option<String>,
    },
    /// Crop the current source window, then optionally reverse each channel into a new WAV. Preserves the clip ID, position, loop extent, tempo behavior and gain; clamps fades to the new window. Original files and other clips are unchanged. One Undo restores the original source. Times are source seconds before tempo stretch, relative to this clip's current window.
    Process {
        /// Audio clip ID.
        clip: u64,
        /// Current content revision from inspect_project or audio_assets inspect.
        expected_revision: String,
        /// Reverse sample order independently in each channel.
        #[serde(default)]
        reverse: bool,
        /// Inclusive crop start in source seconds; omitted keeps the beginning.
        start_seconds: Option<f64>,
        /// Exclusive crop end in source seconds; omitted keeps the end.
        end_seconds: Option<f64>,
    },
}
impl Operation {
    /// Whether this operation changes the song.
    pub fn is_mutating(&self) -> bool {
        matches!(self, Self::ImportUrl { .. } | Self::Process { .. })
    }
    /// Remote searches and ingestion require the application's network approval gate.
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::ImportUrl { .. } | Self::SearchOnline { .. })
    }
}

/// Detached bounded work; the originating document retains ownership of publication.
pub struct Job {
    revision: u64,
    owner: Arc<auris_core::PluginRegistry>,
    folder: PathBuf,
    rate: f64,
    render_rate: f64,
    work: Work,
}
enum Work {
    SearchOnline {
        query: String,
        limit: usize,
        offset: usize,
    },
    Search {
        roots: Vec<PathBuf>,
        query: String,
        recursive: bool,
        offset: usize,
        limit: usize,
    },
    Inspect {
        description: serde_json::Value,
        buffer: Arc<AudioBuffer>,
        from: usize,
        frames: usize,
    },
    Download {
        url: String,
        at: Ticks,
        name: String,
        provenance: BTreeMap<String, String>,
    },
    Process {
        clip: ClipId,
        buffer: Arc<AudioBuffer>,
        from: usize,
        frames: usize,
        reverse: bool,
        name: String,
        provenance: BTreeMap<String, String>,
    },
}
/// Private staged result; unused files are removed when this value is dropped.
pub struct Prepared {
    revision: u64,
    owner: Arc<auris_core::PluginRegistry>,
    folder: PathBuf,
    outcome: Outcome,
}
enum Outcome {
    Read(String),
    Audio {
        file: tempfile::NamedTempFile,
        audio: PreparedAudioSource,
        target: Option<ClipId>,
        at: Ticks,
        name: String,
        bytes: u64,
        provenance: BTreeMap<String, String>,
    },
}
fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Audio operation cancelled".into())
    } else {
        Ok(())
    }
}
fn local(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || value.starts_with("\\\\") || value.starts_with("//") {
        return Err("Use an absolute local directory".into());
    }
    Ok(path)
}
fn audio_url(value: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(value).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value.len() > 8192
    {
        return Err("Use a direct HTTP(S) audio URL without credentials or fragment".into());
    }
    Ok(url)
}
fn stage(folder: &Path, suffix: &str) -> Result<tempfile::NamedTempFile, String> {
    let directory = folder.join(auris_io::AUDIO_DIR);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    tempfile::Builder::new()
        .prefix("sfx-")
        .suffix(suffix)
        .tempfile_in(directory)
        .map_err(|e| e.to_string())
}
fn file_hash(path: &Path, cancelled: &AtomicBool) -> Result<String, String> {
    let mut reader = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        check(cancelled)?;
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

impl Session {
    /// Prepare material work against the current owner and revision without blocking the UI.
    pub fn begin_audio_assets(&self, operation: Operation) -> Result<Job, String> {
        let work = match operation {
            Operation::SearchOnline {
                query,
                limit,
                offset,
            } => {
                let limit = limit.unwrap_or(5);
                if query.trim().is_empty()
                    || query.len() > 256
                    || !(1..=10).contains(&limit)
                    || offset > 1000
                {
                    return Err(
                        "Use nonempty query <=256 bytes, limit 1..10 and offset <=1000".into(),
                    );
                }
                Work::SearchOnline {
                    query,
                    limit,
                    offset,
                }
            }
            Operation::Search {
                root,
                query,
                recursive,
                offset,
                limit,
            } => {
                let limit = limit.unwrap_or(20);
                if query.len() > 256 || !(1..=100).contains(&limit) || offset > MAX_ENTRIES {
                    return Err("Use query <=256 bytes, limit 1..100 and offset <=10000".into());
                }
                let roots = match root {
                    Some(root) => vec![local(&root)?],
                    None => [dirs::audio_dir(), dirs::download_dir()]
                        .into_iter()
                        .flatten()
                        .collect(),
                };
                Work::Search {
                    roots,
                    query,
                    recursive,
                    offset,
                    limit,
                }
            }
            Operation::ImportUrl {
                url,
                at_tick,
                name,
                source_page,
                attribution,
                license,
            } => {
                audio_url(&url)?;
                if at_tick < 0
                    || at_tick > Ticks::QUARTER.raw() * 1_000_000
                    || name.trim().is_empty()
                    || name.len() > 256
                {
                    return Err(
                        "Use a nonempty name <=256 bytes and a valid nonnegative tick".into(),
                    );
                }
                let mut provenance = BTreeMap::from([("source_url".into(), url.clone())]);
                for (key, value) in [
                    ("source_page", source_page),
                    ("attribution", attribution),
                    ("license", license),
                ] {
                    if let Some(value) = value {
                        if value.len() > 4096 {
                            return Err(
                                "Attribution fields must fit within 4096 UTF-8 bytes".into()
                            );
                        }
                        provenance.insert(key.into(), value);
                    }
                }
                Work::Download {
                    url,
                    at: Ticks(at_tick),
                    name,
                    provenance,
                }
            }
            Operation::Inspect { clip } => {
                let clip = self
                    .project
                    .audio_clip(ClipId(clip))
                    .ok_or("Unknown audio clip")?;
                let source = self
                    .project
                    .audio_sources
                    .get(&clip.source)
                    .ok_or("Missing audio source")?;
                let buffer = Arc::clone(
                    self.bank
                        .get(clip.source)
                        .ok_or("Audio source is not loaded")?,
                );
                let (from, frames) = source_window(clip, &buffer, None, None)?;
                Work::Inspect {
                    description: json!({"clip":clip.id.0,"revision":self.agent_document_revision()?,"source":source,"offset_frames":clip.offset_frames,"length_frames":clip.length_frames,"start_tick":clip.start.raw(),"gain_db":clip.gain_db,"muted":clip.muted,"follows_tempo":clip.follows_tempo,"source_bpm":clip.source_bpm,"loop_end_tick":clip.loop_end.raw(),"fade_in_frames":clip.fade_in_frames,"fade_out_frames":clip.fade_out_frames,"provenance_verified":false,"measurement":"Raw selected source window before gain, fades, tempo stretching and track effects; not listening."}),
                    buffer,
                    from,
                    frames,
                }
            }
            Operation::Process {
                clip,
                expected_revision,
                reverse,
                start_seconds,
                end_seconds,
            } => {
                if self.agent_document_revision()? != expected_revision {
                    return Err(
                        "Stale document revision; inspect again before processing audio".into(),
                    );
                }
                if !reverse && start_seconds.is_none() && end_seconds.is_none() {
                    return Err("Supply a crop boundary or reverse=true".into());
                }
                let clip = self
                    .project
                    .audio_clip(ClipId(clip))
                    .ok_or("Unknown audio clip")?;
                let source = self
                    .project
                    .audio_sources
                    .get(&clip.source)
                    .ok_or("Missing audio source")?;
                let buffer = Arc::clone(
                    self.bank
                        .get(clip.source)
                        .ok_or("Audio source is not loaded")?,
                );
                let (from, frames) = source_window(clip, &buffer, start_seconds, end_seconds)?;
                if frames as f64 / buffer.sample_rate() > 600.0 {
                    return Err(
                        "Audio processing is limited to 600 source seconds per operation".into(),
                    );
                }
                let mut provenance = source.provenance.clone();
                if let Some(hash) = provenance.get("sha256").cloned() {
                    provenance.insert("parent_sha256".into(), hash);
                }
                provenance.insert("derived_from_source".into(), source.id.0.to_string());
                provenance.insert(
                    "processing".into(),
                    json!({"source_start_frame":from,"frames":frames,"reverse":reverse})
                        .to_string(),
                );
                Work::Process {
                    clip: clip.id,
                    buffer,
                    from,
                    frames,
                    reverse,
                    name: clip.name.clone(),
                    provenance,
                }
            }
        };
        Ok(Job {
            revision: self.revision(),
            owner: Arc::clone(self.registry()),
            folder: self
                .project_folder()
                .ok_or("Missing project folder")?
                .to_owned(),
            rate: self.project.sample_rate,
            render_rate: self.sample_rate(),
            work,
        })
    }
    /// Publish prepared material as one undoable edit, or return its read-only result.
    pub fn continue_audio_assets(
        &mut self,
        prepared: Prepared,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        check(cancelled)?;
        if self.revision() != prepared.revision
            || !Arc::ptr_eq(self.registry(), &prepared.owner)
            || self.project_folder() != Some(prepared.folder.as_path())
        {
            return Err("Document changed during audio operation; staged work discarded".into());
        }
        match prepared.outcome {
            Outcome::Read(text) => Ok(text),
            Outcome::Audio {
                file,
                audio,
                target: None,
                at,
                name,
                bytes,
                provenance,
            } => self.adopt_agent_audio(file, audio, name, at, bytes, provenance),
            Outcome::Audio {
                file,
                audio,
                target: Some(target),
                name,
                bytes,
                provenance,
                ..
            } => {
                if self.sample_rate() != audio.render_rate {
                    return Err("Audio device changed during processing".into());
                }
                let mut clip = self
                    .project
                    .audio_clip(target)
                    .ok_or("Audio clip disappeared")?
                    .clone();
                let filename = file
                    .path()
                    .file_name()
                    .ok_or("Missing staged name")?
                    .to_owned();
                let (_handle, path) = file.keep().map_err(|e| e.to_string())?;
                self.begin_transaction(Edit::ImportAudio);
                let source = self.project.add_audio_source(
                    name,
                    AssetPath::inside(Path::new(auris_io::AUDIO_DIR).join(filename)),
                    audio.buffer.frame_count() as u64,
                    audio.buffer.sample_rate(),
                    audio.buffer.channel_count(),
                );
                if let Some(metadata) = self.project.audio_sources.get_mut(&source) {
                    metadata.byte_size = bytes;
                    metadata.provenance = provenance;
                }
                clip.source = source;
                clip.offset_frames = 0;
                clip.length_frames = audio.buffer.frame_count() as u64;
                clip.fade_in_frames = clip.fade_in_frames.min(clip.length_frames);
                clip.fade_out_frames = clip.fade_out_frames.min(clip.length_frames);
                let Some(slot) = self.project.audio_clip_mut(target) else {
                    self.revert_transaction();
                    let _ = std::fs::remove_file(path);
                    return Err("Audio clip disappeared".into());
                };
                *slot = clip;
                self.install_prepared_audio(source, audio);
                self.end_transaction();
                self.invalidate_graph();
                Ok(json!({"clip":target.0,"source":source.0,"revision":self.agent_document_revision()?,"original_source_preserved":true}).to_string())
            }
        }
    }
}
fn source_window(
    clip: &auris_core::AudioClip,
    buffer: &AudioBuffer,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<(usize, usize), String> {
    let duration = clip.length_frames as f64 / buffer.sample_rate();
    let (start, end) = (start.unwrap_or(0.0), end.unwrap_or(duration));
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start || end > duration {
        return Err("Use a nonempty crop inside the clip's source window".into());
    }
    let a = (start * buffer.sample_rate()).round() as u64;
    let b = (end * buffer.sample_rate()).round() as u64;
    let from = clip
        .offset_frames
        .checked_add(a)
        .ok_or("Source frame overflow")?;
    let to = clip
        .offset_frames
        .checked_add(b)
        .ok_or("Source frame overflow")?;
    if b > clip.length_frames || to > buffer.frame_count() as u64 || to <= from {
        return Err("Audio source window is out of range".into());
    }
    Ok((from as usize, (to - from) as usize))
}
impl Job {
    /// Execute filesystem/network/sample work without mutating the document.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Prepared, String> {
        check(cancelled)?;
        let outcome = match self.work {
            Work::SearchOnline {
                query,
                limit,
                offset,
            } => Outcome::Read(search_online(&query, limit, offset, cancelled)?),
            Work::Search {
                roots,
                query,
                recursive,
                offset,
                limit,
            } => Outcome::Read(search(&roots, &query, recursive, offset, limit, cancelled)?),
            Work::Inspect {
                mut description,
                buffer,
                from,
                frames,
            } => {
                let mut peak = 0.0_f32;
                let mut sum = 0.0_f64;
                for channel in buffer.channels() {
                    for chunk in channel[from..from + frames].chunks(65536) {
                        check(cancelled)?;
                        for &sample in chunk {
                            peak = peak.max(sample.abs());
                            sum += f64::from(sample).powi(2);
                        }
                    }
                }
                description["source_seconds"] = json!(frames as f64 / buffer.sample_rate());
                description["peak"] = json!(peak);
                description["rms"] = json!((sum / (frames * buffer.channel_count()) as f64).sqrt());
                Outcome::Read(description.to_string())
            }
            Work::Download {
                url,
                at,
                name,
                mut provenance,
            } => {
                let (file, bytes, final_url, hash) = download(&url, &self.folder, cancelled)?;
                provenance.insert("resolved_url".into(), final_url);
                provenance.insert("sha256".into(), hash);
                let buffer =
                    crate::decode_audio(file.path(), self.rate).map_err(|e| e.to_string())?;
                if buffer.frame_count() as f64 / buffer.sample_rate() > 600.0 {
                    return Err("Downloaded audio exceeds 600 seconds".into());
                }
                let audio = crate::prepare_audio_source(buffer, self.render_rate)
                    .map_err(|e| e.to_string())?;
                Outcome::Audio {
                    file,
                    audio,
                    target: None,
                    at,
                    name,
                    bytes,
                    provenance,
                }
            }
            Work::Process {
                clip,
                buffer,
                from,
                frames,
                reverse,
                name,
                mut provenance,
            } => {
                let mut channels = Vec::with_capacity(buffer.channel_count());
                for input in buffer.channels() {
                    check(cancelled)?;
                    let mut channel = input[from..from + frames].to_vec();
                    if reverse {
                        channel.reverse();
                    }
                    channels.push(channel);
                }
                let buffer = AudioBuffer::from_planar(channels, buffer.sample_rate())
                    .map_err(|e| e.to_string())?;
                // The WAV writer publishes by rename. Close the placeholder handle first:
                // Windows cannot replace an open NamedTempFile, even on this same thread.
                let path = stage(&self.folder, ".wav")?.into_temp_path();
                auris_io::write_wav(
                    &path,
                    &buffer,
                    &WavExportSettings {
                        bit_depth: WavBitDepth::Float32,
                        sample_rate: buffer.sample_rate() as u32,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?;
                let file = tempfile::NamedTempFile::from_parts(
                    std::fs::File::open(&path).map_err(|e| e.to_string())?,
                    path,
                );
                let bytes = file.as_file().metadata().map_err(|e| e.to_string())?.len();
                provenance.insert("sha256".into(), file_hash(file.path(), cancelled)?);
                let audio = crate::prepare_audio_source(buffer, self.render_rate)
                    .map_err(|e| e.to_string())?;
                Outcome::Audio {
                    file,
                    audio,
                    target: Some(clip),
                    at: Ticks::ZERO,
                    name,
                    bytes,
                    provenance,
                }
            }
        };
        check(cancelled)?;
        Ok(Prepared {
            revision: self.revision,
            owner: self.owner,
            folder: self.folder,
            outcome,
        })
    }
}
/// Whether a redirect from the approved `from` may be followed to `to` without asking again.
///
/// The person approved one address. A redirect within that origin is the same server answering;
/// one to another origin is followed only over HTTPS and only to a public host, so an approved
/// public URL cannot be bounced onto this machine or the local network, or onto plain HTTP.
fn redirect_allowed(from: &url::Url, to: &url::Url) -> bool {
    if from.origin() == to.origin() {
        return true;
    }
    if to.scheme() != "https" {
        return false;
    }
    match to.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name != "localhost" && !name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                // 100.64.0.0/10, carrier-grade NAT and many VPNs' private ranges.
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64))
        }
        Some(url::Host::Ipv6(ip)) => {
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                // fc00::/7 unique local, fe80::/10 link local.
                || segments[0] & 0xfe00 == 0xfc00
                || segments[0] & 0xffc0 == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| {
                    v4.is_loopback() || v4.is_private() || v4.is_link_local()
                }))
        }
        None => false,
    }
}

fn download(
    value: &str,
    folder: &Path,
    cancelled: &AtomicBool,
) -> Result<(tempfile::NamedTempFile, u64, String, String), String> {
    let mut url = audio_url(value)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    for redirect in 0..=5 {
        check(cancelled)?;
        let response = agent.get(url.as_str()).call().map_err(|e| e.to_string())?;
        if response.status().is_redirection() {
            if redirect == 5 {
                return Err("Too many audio URL redirects".into());
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|x| x.to_str().ok())
                .ok_or("Redirect has no location")?;
            let next = audio_url(url.join(location).map_err(|e| e.to_string())?.as_str())?;
            if !redirect_allowed(&url, &next) {
                return Err(format!(
                    "Audio URL redirected to {next}, which was not the address approved; import that URL directly instead"
                ));
            }
            url = next;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("Audio download HTTP {}", response.status()));
        }
        if response
            .headers()
            .get("content-length")
            .and_then(|x| x.to_str().ok())
            .and_then(|x| x.parse::<u64>().ok())
            .is_some_and(|n| n > MAX_BYTES)
        {
            return Err("Audio download exceeds 64 MiB".into());
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|x| x.to_str().ok())
            .is_some_and(|x| x.starts_with("text/") || x.contains("json") || x.contains("zip"))
        {
            return Err("URL returned a page/archive instead of audio".into());
        }
        let ext = Path::new(url.path())
            .extension()
            .and_then(|x| x.to_str())
            .filter(|x| auris_io::supported_extensions().contains(x))
            .unwrap_or("audio");
        let mut file = stage(folder, &format!(".{ext}"))?;
        let mut reader = response.into_body().into_reader();
        let mut chunk = [0u8; 65536];
        let mut bytes = 0u64;
        let mut hash = Sha256::new();
        loop {
            check(cancelled)?;
            let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > MAX_BYTES {
                return Err("Audio download exceeds 64 MiB".into());
            }
            hash.update(&chunk[..n]);
            file.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
        }
        file.flush().map_err(|e| e.to_string())?;
        return Ok((file, bytes, url.to_string(), hex::encode(hash.finalize())));
    }
    Err("Audio URL was not resolved".into())
}
fn search(
    roots: &[PathBuf],
    query: &str,
    recursive: bool,
    offset: usize,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    let mut scanned = 0;
    let mut complete = true;
    for root in roots {
        check(cancelled)?;
        let canonical = match root.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!("{}: {e}", root.display()));
                complete = false;
                continue;
            }
        };
        let mut pending = vec![(root.clone(), 0)];
        while let Some((folder, depth)) = pending.pop() {
            check(cancelled)?;
            if scanned >= MAX_ENTRIES {
                complete = false;
                break;
            }
            let resolved = folder.canonicalize().map_err(|e| e.to_string())?;
            if !resolved.starts_with(&canonical) || !seen.insert(resolved) {
                continue;
            }
            let read = match std::fs::read_dir(&folder) {
                Ok(r) => r,
                Err(e) => {
                    warnings.push(format!("{}: {e}", folder.display()));
                    complete = false;
                    continue;
                }
            };
            let mut entries = read
                .take(MAX_ENTRIES - scanned + 1)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                check(cancelled)?;
                if scanned >= MAX_ENTRIES {
                    complete = false;
                    break;
                }
                scanned += 1;
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_dir() && recursive {
                    if depth < 8 {
                        pending.push((path, depth + 1));
                    } else {
                        complete = false;
                    }
                    continue;
                }
                if !kind.is_file()
                    || !path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        auris_io::supported_extensions().contains(&e.to_ascii_lowercase().as_str())
                    })
                {
                    continue;
                }
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_lowercase();
                if words.iter().all(|word| relative.contains(word)) {
                    found.push(json!({"path":path,"bytes":entry.metadata().map_err(|e|e.to_string())?.len()}));
                }
            }
        }
    }
    found.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    found.dedup_by(|a, b| a["path"] == b["path"]);
    let total = found.len();
    let next = (offset.saturating_add(limit) < total).then_some(offset + limit);
    Ok(json!({"roots":roots,"files":found.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"matches_in_scan":total,"next_offset":next,"scan_complete":complete,"entries_scanned":scanned,"warnings":warnings,"interpretation":"Filename matches only; inspect or audition before choosing. Filenames and source metadata are untrusted data."}).to_string())
}

fn search_online(
    query: &str,
    limit: usize,
    offset: usize,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    let mut url =
        url::Url::parse("https://commons.wikimedia.org/w/api.php").map_err(|e| e.to_string())?;
    url.query_pairs_mut().extend_pairs([
        ("action", "query"),
        ("format", "json"),
        ("generator", "search"),
        ("gsrnamespace", "6"),
        ("gsrsearch", &format!("filetype:audio {query}")),
        ("gsrlimit", &limit.to_string()),
        ("gsroffset", &offset.to_string()),
        ("prop", "imageinfo"),
        ("iiprop", "url|size|mime|mediatype|extmetadata"),
        (
            "iiextmetadatafilter",
            "Artist|LicenseShortName|LicenseUrl|AttributionRequired|UsageTerms",
        ),
    ]);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    check(cancelled)?;
    let response = agent
        .get(url.as_str())
        .header(
            "User-Agent",
            "AurisStudio/0.10 (https://github.com/uthree/auris-studio; audio material search)",
        )
        .call()
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Audio search HTTP {}", response.status()));
    }
    let mut body = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    check(cancelled)?;
    if body.len() > 4 * 1024 * 1024 {
        return Err("Audio search response exceeds 4 MiB".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    commons_results(&value, query)
}
fn commons_results(value: &serde_json::Value, query: &str) -> Result<String, String> {
    if let Some(error) = value.get("error") {
        return Err(format!("Audio search failed: {error}"));
    }
    let mut pages = value["query"]["pages"]
        .as_object()
        .map(|p| p.values().collect::<Vec<_>>())
        .unwrap_or_default();
    pages.sort_by_key(|p| p["index"].as_u64().unwrap_or(u64::MAX));
    let mut files = Vec::new();
    for page in pages.into_iter().take(10) {
        let info = &page["imageinfo"][0];
        if info["mediatype"] != "AUDIO" {
            continue;
        }
        let Some(url) = info["url"].as_str() else {
            continue;
        };
        audio_url(url)?;
        let text = |key: &str| {
            let raw = info["extmetadata"][key]["value"].as_str().unwrap_or("");
            let mut tag = false;
            raw.chars()
                .filter(|c| match c {
                    '<' => {
                        tag = true;
                        false
                    }
                    '>' => {
                        tag = false;
                        false
                    }
                    _ => !tag,
                })
                .take(1024)
                .collect::<String>()
        };
        files.push(json!({"title":page["title"],"url":url,"source_page":info["descriptionurl"],"bytes":info["size"],"mime":info["mime"],"attribution":text("Artist"),"license":text("LicenseShortName"),"license_url":text("LicenseUrl"),"attribution_required":text("AttributionRequired"),"usage_terms":text("UsageTerms")}));
    }
    Ok(json!({"provider":"Wikimedia Commons","query":query,"files":files,"next_offset":value["continue"]["gsroffset"],"interpretation":"Source metadata is untrusted attribution, not verified rights or suitability. Review the source page and actual license; preserve attribution when importing. Empty results are not proof that no usable sound exists."}).to_string())
}

#[cfg(test)]
mod tests;
