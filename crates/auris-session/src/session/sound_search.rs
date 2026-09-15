//! Bounded text discovery and reusable acoustic search over exact library preset addresses.

use super::{Session, TimbreMapControl, TimbreSound};
use crate::prelude::*;
use auris_core::plugin::{Instrument, PluginState, PrepareContext};
use auris_dsp::timbre::{standardize_timbres, timbre_features};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A bounded discovery operation shared by saved-file and live-session frontends.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SoundSearch {
    /// All query words must occur in the name, library, vendor, or provider tags.
    Text {
        /// Nonempty case-insensitive search words, for example `saw lead` or `Surge bass`.
        query: String,
        /// Maximum results, 1..50; defaults to 10.
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 50))]
        limit: usize,
        /// First matching result, defaults to zero.
        #[serde(default)]
        offset: usize,
        /// Optional exact source and library substring constraints.
        #[serde(default, flatten)]
        filter: SoundFilter,
    },
    /// Other sounds ordered by full standardized acoustic feature distance.
    Similar {
        /// Exact sound ID returned by search_instruments for this project/session.
        id: String,
        /// Maximum neighbors, 1..50; defaults to 10. Excludes the reference itself.
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 50))]
        limit: usize,
        /// Optional constraints on returned neighbors (not on the reference).
        #[serde(default, flatten)]
        filter: SoundFilter,
    },
}
/// Sound origins available to discovery clients.
#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoundSource {
    /// Internal synthesizers.
    Builtin,
    /// Loaded SoundFont presets.
    Soundfont,
    /// CLAP instruments and presets.
    Clap,
    /// VST3 instruments and presets.
    Vst3,
}
/// Constraints applied before pagination or selecting nearest neighbors.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct SoundFilter {
    /// Restrict results to this source; omit for all sources.
    pub source: Option<SoundSource>,
    /// Case-insensitive library name substring; omit for all libraries.
    pub library: Option<String>,
}
impl SoundFilter {
    fn matches(&self, sound: &Sound) -> bool {
        let source = match self.source {
            None => true,
            Some(SoundSource::Builtin) => matches!(sound.source, Source::Builtin(_)),
            Some(SoundSource::Soundfont) => matches!(sound.source, Source::Font(_)),
            Some(SoundSource::Clap) => matches!(sound.source, Source::Clap { .. }),
            Some(SoundSource::Vst3) => matches!(sound.source, Source::Vst3 { .. }),
        };
        source
            && self
                .library
                .as_ref()
                .is_none_or(|name| sound.library.to_lowercase().contains(&name.to_lowercase()))
    }
}
fn default_limit() -> usize {
    10
}

#[derive(Clone, Debug)]
enum Source {
    Builtin(String),
    Font(PresetRef),
    Clap {
        file: PathBuf,
        id: String,
        preset: Option<auris_clap::ClapPreset>,
        discovery_stamp: String,
        isolated_metadata: bool,
    },
    Vst3 {
        file: PathBuf,
        id: String,
        preset: Option<auris_vst3::Vst3Preset>,
        discovery_stamp: String,
        isolated_metadata: bool,
    },
}
#[derive(Clone, Debug)]
struct Sound {
    id: String,
    name: String,
    library: String,
    tags: String,
    source: Source,
}
impl Sound {
    fn new(name: String, library: String, tags: String, source: Source) -> Self {
        Self {
            id: format!("sound:{:x}", Sha256::digest(format!("{source:?}"))),
            name,
            library,
            tags,
            source,
        }
    }
    fn value(&self) -> serde_json::Value {
        let source = match self.source {
            Source::Builtin(_) => "builtin",
            Source::Font(_) => "soundfont",
            Source::Clap { .. } => "clap",
            Source::Vst3 { .. } => "vst3",
        };
        serde_json::json!({"id":self.id,"name":short_text(&self.name,160),"library":short_text(&self.library,160),"source":source})
    }
    fn matches(&self, words: &[String]) -> bool {
        let text = format!("{} {} {}", self.name, self.library, self.tags).to_lowercase();
        words.iter().all(|word| text.contains(word))
    }

    fn retained_bytes(&self) -> usize {
        let bytes = std::mem::size_of::<Self>()
            .saturating_add(self.id.len())
            .saturating_add(self.name.len())
            .saturating_add(self.library.len())
            .saturating_add(self.tags.len());
        let source_bytes = match &self.source {
            Source::Builtin(id) => id.len(),
            Source::Font(_) => 0,
            Source::Clap {
                file,
                id,
                preset,
                discovery_stamp,
                isolated_metadata: _,
            } => {
                let mut bytes = file
                    .as_os_str()
                    .len()
                    .saturating_add(id.len())
                    .saturating_add(discovery_stamp.len());
                if let Some(preset) = preset {
                    bytes = bytes
                        .saturating_add(preset.name.len())
                        .saturating_add(
                            preset
                                .plugin_ids
                                .len()
                                .saturating_mul(std::mem::size_of::<String>()),
                        )
                        .saturating_add(
                            preset
                                .plugin_ids
                                .iter()
                                .map(String::len)
                                .fold(0usize, usize::saturating_add),
                        )
                        .saturating_add(preset.location.as_ref().map_or(0, String::len))
                        .saturating_add(preset.load_key.as_ref().map_or(0, String::len))
                        .saturating_add(
                            preset
                                .features
                                .len()
                                .saturating_mul(std::mem::size_of::<String>()),
                        )
                        .saturating_add(
                            preset
                                .features
                                .iter()
                                .map(String::len)
                                .fold(0usize, usize::saturating_add),
                        );
                }
                bytes
            }
            Source::Vst3 {
                file,
                id,
                preset,
                discovery_stamp,
                isolated_metadata: _,
            } => file
                .as_os_str()
                .len()
                .saturating_add(id.len())
                .saturating_add(discovery_stamp.len())
                .saturating_add(match preset {
                    Some(auris_vst3::Vst3Preset::File(path)) => path.as_os_str().len(),
                    _ => 0,
                }),
        };
        bytes.saturating_add(source_bytes)
    }
}

struct AcousticIndex {
    sounds: Vec<usize>,
    rows: Vec<Vec<f64>>,
    skipped: Vec<String>,
    limited: bool,
}
#[derive(Default)]
struct IndexState {
    started: bool,
    reference: Option<usize>,
    result: Option<Result<AcousticIndex, String>>,
}

impl IndexState {
    fn reset_if_reference_missing(&mut self, reference: usize) -> bool {
        let reusable = match &self.result {
            Some(Ok(index)) => index.sounds.contains(&reference),
            Some(Err(_)) => self.reference == Some(reference),
            None => true,
        };
        if self.result.is_some() && !reusable {
            self.started = false;
            self.reference = None;
            self.result = None;
            true
        } else {
            false
        }
    }
}
struct Catalog {
    key: String,
    fingerprint: String,
    generation: String,
    sounds: Vec<Sound>,
    errors: Vec<String>,
    index: Mutex<IndexState>,
    completed: AtomicUsize,
    control: TimbreMapControl,
    native_status: Option<NativePluginStatus>,
}
static CATALOGS: OnceLock<Mutex<Vec<Arc<Catalog>>>> = OnceLock::new();
static CATALOG_BUILD: Mutex<()> = Mutex::new(());
const CATALOG_LIMIT: usize = 4;
const CATALOG_SOUND_LIMIT: usize = 16_384;
const CATALOG_SOUND_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const CATALOG_ERROR_LIMIT: usize = 1_024;
const CATALOG_ERROR_CHARACTER_LIMIT: usize = 512;
const ACOUSTIC_WORKER_LIMIT: usize = 4;
const ACOUSTIC_SOUND_LIMIT: usize = 1_024;
const ACOUSTIC_INDEX_BYTE_LIMIT: usize = 16 * 1024 * 1024;
const DISCOVERY_FILE_LIMIT: usize = 16_384;
const DISCOVERY_PATH_BYTE_LIMIT: usize = 16 * 1024 * 1024;
const DISCOVERY_ENTRY_LIMIT: usize = 65_536;
const DISCOVERY_DEPTH_LIMIT: usize = 64;
const ISOLATED_PLUGIN_FILE_LIMIT: usize = 256;
const ISOLATED_PLUGIN_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const ISOLATED_PLUGIN_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const ISOLATED_PLUGIN_CATALOG_TIMEOUT: Duration = Duration::from_secs(30);
static ACOUSTIC_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Default)]
struct NativePluginStatus {
    discovered_files: usize,
    inspected_files: usize,
    failed_files: usize,
    skipped_files: usize,
    truncated: bool,
}

impl NativePluginStatus {
    fn value(&self) -> serde_json::Value {
        serde_json::json!({
            "mode": "isolated_metadata",
            "discovered_files": self.discovered_files,
            "inspected_files": self.inspected_files,
            "failed_files": self.failed_files,
            "skipped_files": self.skipped_files,
            "truncated": self.truncated,
            "presets": "not_enumerated",
            "acoustic_similarity": "not_supported"
        })
    }
}

#[derive(Clone, Debug)]
struct IsolatedPluginEntry {
    path: PathBuf,
    metadata: crate::PluginProbeResult,
}

#[derive(Clone, Debug, Default)]
struct IsolatedPluginCatalog {
    entries: Vec<IsolatedPluginEntry>,
    diagnostics: Vec<String>,
    status: NativePluginStatus,
}

fn try_discovery_lock(lock: &Mutex<()>) -> Result<MutexGuard<'_, ()>, String> {
    match lock.try_lock() {
        Ok(guard) => Ok(guard),
        Err(std::sync::TryLockError::WouldBlock) => {
            Err("Sound discovery is busy; retry after the current scan finishes".into())
        }
        Err(std::sync::TryLockError::Poisoned(error)) => {
            // The admission mutex carries no catalog state. A panicking builder may leave a
            // reserved cache slot empty, but the cache and handle registries have their own
            // mutexes and invariants, so retaining this poison would only make every future miss
            // fail permanently.
            lock.clear_poison();
            Ok(error.into_inner())
        }
    }
}

struct AcousticWorkerPermit {
    counter: &'static AtomicUsize,
}

impl AcousticWorkerPermit {
    fn acquire() -> Option<Self> {
        Self::acquire_from(&ACOUSTIC_WORKERS)
    }

    fn acquire_from(counter: &'static AtomicUsize) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |active| {
                (active < ACOUSTIC_WORKER_LIMIT).then_some(active + 1)
            })
            .ok()
            .map(|_| Self { counter })
    }
}

impl Drop for AcousticWorkerPermit {
    fn drop(&mut self) {
        let previous = self.counter.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }
}

fn reserve_catalog_slot(entries: &mut Vec<Arc<Catalog>>) -> Result<(), String> {
    if entries.len() < CATALOG_LIMIT {
        return Ok(());
    }
    let Some(index) = entries
        .iter()
        .position(|cached| Arc::strong_count(cached) == 1)
    else {
        return Err("Sound discovery is busy; retry after another search finishes".into());
    };
    entries.remove(index).control.cancel();
    Ok(())
}

#[derive(Default)]
struct CatalogBudget {
    sound_bytes: usize,
    seen: HashSet<String>,
    sounds_truncated: bool,
    errors_truncated: bool,
}

impl CatalogBudget {
    fn push_sound(&mut self, sounds: &mut Vec<Sound>, sound: Sound) -> bool {
        if self.seen.contains(&sound.id) {
            return true;
        }
        let bytes = sound.retained_bytes();
        if sounds.len() >= CATALOG_SOUND_LIMIT
            || self.sound_bytes.saturating_add(bytes) > CATALOG_SOUND_BYTE_LIMIT
        {
            self.sounds_truncated = true;
            return false;
        }
        self.seen.insert(sound.id.clone());
        self.sound_bytes += bytes;
        sounds.push(sound);
        true
    }

    fn push_error(&mut self, errors: &mut Vec<String>, error: impl AsRef<str>) {
        if errors.len() < CATALOG_ERROR_LIMIT {
            errors.push(short_text(error.as_ref(), CATALOG_ERROR_CHARACTER_LIMIT));
        } else if !self.errors_truncated {
            if let Some(last) = errors.last_mut() {
                *last = "Additional sound discovery diagnostics were omitted".into();
            }
            self.errors_truncated = true;
        }
    }

    fn report_sound_limit(&mut self, errors: &mut Vec<String>) {
        if self.sounds_truncated {
            self.push_error(
                errors,
                "The sound catalog reached its bounded metadata limit; narrow the configured plugin paths",
            );
        }
    }
}

#[derive(Default)]
struct PluginFiles {
    entries: Vec<(PathBuf, bool)>,
    seen: HashSet<(PathBuf, bool)>,
    bytes: usize,
    truncated: bool,
    full: bool,
}

impl PluginFiles {
    fn push(&mut self, path: PathBuf, vst3: bool) -> bool {
        if !self.seen.insert((path.clone(), vst3)) {
            return true;
        }
        let bytes = path.as_os_str().len();
        if self.entries.len() >= DISCOVERY_FILE_LIMIT
            || self.bytes.saturating_add(bytes) > DISCOVERY_PATH_BYTE_LIMIT
        {
            self.seen.remove(&(path, vst3));
            self.truncated = true;
            self.full = true;
            return false;
        }
        self.bytes += bytes;
        self.entries.push((path, vst3));
        true
    }
}

fn scan_plugin_roots(
    roots: impl IntoIterator<Item = PathBuf>,
    extension: &str,
    vst3: bool,
    files: &mut PluginFiles,
    visited: &mut usize,
    traversal_bytes: &mut usize,
) {
    let mut pending = Vec::new();
    for path in roots {
        let bytes = path.as_os_str().len();
        if *visited >= DISCOVERY_ENTRY_LIMIT
            || traversal_bytes.saturating_add(bytes) > DISCOVERY_PATH_BYTE_LIMIT
        {
            files.truncated = true;
            break;
        }
        *visited += 1;
        *traversal_bytes += bytes;
        pending.push((path, 0usize));
    }
    while let Some((path, depth)) = pending.pop() {
        if path
            .extension()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(extension))
        {
            if !files.push(path, vst3) {
                break;
            }
            continue;
        }
        if depth >= DISCOVERY_DEPTH_LIMIT {
            files.truncated = true;
            continue;
        }
        let Ok(metadata) = path.symlink_metadata() else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let Ok(entries) = path.read_dir() else {
            continue;
        };
        for entry in entries.flatten() {
            *visited += 1;
            let path = entry.path();
            let bytes = path.as_os_str().len();
            if *visited > DISCOVERY_ENTRY_LIMIT
                || traversal_bytes.saturating_add(bytes) > DISCOVERY_PATH_BYTE_LIMIT
            {
                files.truncated = true;
                pending.clear();
                break;
            }
            *traversal_bytes += bytes;
            pending.push((path, depth + 1));
        }
    }
}

fn installed_plugin_files(extra_paths: &[PathBuf]) -> PluginFiles {
    let mut files = PluginFiles::default();
    let mut visited = 0usize;
    let mut traversal_bytes = 0usize;
    let clap_roots = super::hosted::clap_search_paths()
        .into_iter()
        .chain(extra_paths.iter().cloned());
    scan_plugin_roots(
        clap_roots,
        "clap",
        false,
        &mut files,
        &mut visited,
        &mut traversal_bytes,
    );
    if !files.full {
        let mut vst3_visited = 0usize;
        let mut vst3_traversal_bytes = 0usize;
        scan_plugin_roots(
            auris_vst3::vst3_search_paths(extra_paths),
            "vst3",
            true,
            &mut files,
            &mut vst3_visited,
            &mut vst3_traversal_bytes,
        );
    }
    files.entries.sort();
    files
}

/// Generations and issued handles outlive the four heavyweight catalogs so ordinary cache
/// eviction cannot invalidate an ID between search and selection. This registry is bounded
/// independently because it holds no provider instances or acoustic measurements.
const CATALOG_GENERATION_LIMIT: usize = 256;
const ISSUED_SOUND_LIMIT: usize = 4_096;
const ISSUED_SOUND_BYTE_LIMIT: usize = 8 * 1024 * 1024;

struct IssuedSound {
    sound: Arc<Sound>,
    bytes: usize,
}

struct CatalogGeneration {
    key: String,
    fingerprint: Option<String>,
    token: String,
    sounds: VecDeque<IssuedSound>,
}

#[derive(Default)]
struct IssuedSounds {
    generations: VecDeque<CatalogGeneration>,
    sound_count: usize,
    byte_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IssueStatus {
    Issued,
    Stale,
    TooLarge,
}

impl IssuedSounds {
    fn evict_oldest_generation(&mut self) {
        if let Some(expired) = self.generations.pop_front() {
            self.sound_count -= expired.sounds.len();
            self.byte_count -= expired
                .sounds
                .iter()
                .map(|sound| sound.bytes)
                .sum::<usize>();
        }
    }

    fn generation(&mut self, key: &str, fingerprint: Option<&str>, refresh: bool) -> String {
        if let Some(index) = self.generations.iter().position(|entry| entry.key == key) {
            let mut entry = self
                .generations
                .remove(index)
                .expect("the generation index exists");
            let same_snapshot = match (entry.fingerprint.as_deref(), fingerprint) {
                (_, None) | (None, Some(_)) => true,
                (Some(current), Some(next)) => current == next,
            };
            if !refresh && same_snapshot {
                if entry.fingerprint.is_none() {
                    entry.fingerprint = fingerprint.map(str::to_owned);
                }
                let generation = entry.token.clone();
                self.generations.push_back(entry);
                return generation;
            }
            self.sound_count -= entry.sounds.len();
            self.byte_count -= entry.sounds.iter().map(|sound| sound.bytes).sum::<usize>();
        }
        let generation = crate::transient_id::transient_id("g");
        while self.generations.len() >= CATALOG_GENERATION_LIMIT {
            self.evict_oldest_generation();
        }
        self.generations.push_back(CatalogGeneration {
            key: key.to_string(),
            fingerprint: fingerprint.map(str::to_owned),
            token: generation.clone(),
            sounds: VecDeque::new(),
        });
        generation
    }

    fn restore_generation(&mut self, key: &str, generation: &str, fingerprint: &str) -> bool {
        if let Some(index) = self.generations.iter().position(|entry| entry.key == key) {
            if self.generations[index].token != generation
                || self.generations[index]
                    .fingerprint
                    .as_deref()
                    .is_some_and(|current| current != fingerprint)
            {
                return false;
            }
            let mut entry = self
                .generations
                .remove(index)
                .expect("the generation index exists");
            if entry.fingerprint.is_none() {
                entry.fingerprint = Some(fingerprint.to_owned());
            }
            self.generations.push_back(entry);
            return true;
        }
        while self.generations.len() >= CATALOG_GENERATION_LIMIT {
            self.evict_oldest_generation();
        }
        self.generations.push_back(CatalogGeneration {
            key: key.to_owned(),
            fingerprint: Some(fingerprint.to_owned()),
            token: generation.to_owned(),
            sounds: VecDeque::new(),
        });
        true
    }

    fn issue(&mut self, key: &str, generation: &str, sounds: Vec<IssuedSound>) -> IssueStatus {
        let incoming_bytes = sounds
            .iter()
            .map(|sound| sound.bytes)
            .fold(0usize, usize::saturating_add);
        if sounds.len() > ISSUED_SOUND_LIMIT || incoming_bytes > ISSUED_SOUND_BYTE_LIMIT {
            return IssueStatus::TooLarge;
        }
        let Some(index) = self.generations.iter().position(|entry| entry.key == key) else {
            return IssueStatus::Stale;
        };
        let mut entry = self
            .generations
            .remove(index)
            .expect("the generation index exists");
        if entry.token != generation {
            self.generations.insert(index, entry);
            return IssueStatus::Stale;
        }
        for sound in sounds {
            if let Some(index) = entry
                .sounds
                .iter()
                .position(|issued| issued.sound.id == sound.sound.id)
            {
                let previous = entry
                    .sounds
                    .remove(index)
                    .expect("the issued sound index exists");
                self.sound_count -= 1;
                self.byte_count -= previous.bytes;
            }
            self.byte_count += sound.bytes;
            entry.sounds.push_back(sound);
            self.sound_count += 1;
        }
        self.generations.push_back(entry);
        while self.sound_count > ISSUED_SOUND_LIMIT || self.byte_count > ISSUED_SOUND_BYTE_LIMIT {
            let removed = self
                .generations
                .iter_mut()
                .find_map(|entry| entry.sounds.pop_front());
            let Some(removed) = removed else {
                break;
            };
            self.sound_count -= 1;
            self.byte_count -= removed.bytes;
        }
        IssueStatus::Issued
    }

    fn resolve(&mut self, key: &str, id: &str) -> Option<Arc<Sound>> {
        let index = self.generations.iter().position(|entry| entry.key == key)?;
        let mut entry = self
            .generations
            .remove(index)
            .expect("the generation index exists");
        let Some(sound_index) = entry.sounds.iter().position(|sound| sound.sound.id == id) else {
            self.generations.insert(index, entry);
            return None;
        };
        let sound = entry
            .sounds
            .remove(sound_index)
            .expect("the issued sound index exists");
        let result = Arc::clone(&sound.sound);
        entry.sounds.push_back(sound);
        self.generations.push_back(entry);
        Some(result)
    }
}

static ISSUED_SOUNDS: OnceLock<Mutex<IssuedSounds>> = OnceLock::new();

fn catalog_generation(
    key: &str,
    fingerprint: Option<&str>,
    refresh: bool,
) -> Result<String, String> {
    Ok(ISSUED_SOUNDS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Sound handle lock failed")?
        .generation(key, fingerprint, refresh))
}

fn restore_catalog_generation(catalog: &Catalog) -> Result<bool, String> {
    Ok(ISSUED_SOUNDS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Sound handle lock failed")?
        .restore_generation(&catalog.key, &catalog.generation, &catalog.fingerprint))
}

fn issue_sounds<'a>(
    key: &str,
    generation: &str,
    sounds: impl IntoIterator<Item = &'a Sound>,
) -> Result<(), String> {
    let sounds = sounds.into_iter().collect::<Vec<_>>();
    let bytes = sounds
        .iter()
        .map(|sound| sound.retained_bytes())
        .collect::<Vec<_>>();
    let byte_count = bytes.iter().copied().fold(0usize, usize::saturating_add);
    if sounds.len() > ISSUED_SOUND_LIMIT || byte_count > ISSUED_SOUND_BYTE_LIMIT {
        return Err("Sound selection metadata exceeds the handle budget".into());
    }
    let sounds = sounds
        .into_iter()
        .zip(bytes)
        .map(|(sound, bytes)| IssuedSound {
            bytes,
            sound: Arc::new(sound.clone()),
        })
        .collect();
    match ISSUED_SOUNDS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Sound handle lock failed")?
        .issue(key, generation, sounds)
    {
        IssueStatus::Issued => Ok(()),
        IssueStatus::Stale => Err("Sound search was refreshed; retry the search".into()),
        IssueStatus::TooLarge => Err("Sound selection metadata exceeds the handle budget".into()),
    }
}

fn issued_sound(key: &str, id: &str) -> Result<Arc<Sound>, String> {
    let sound = ISSUED_SOUNDS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Sound handle lock failed")?
        .resolve(key, id)
        .ok_or_else(|| {
            String::from(
                "Unknown or expired sound ID; call search_instruments for this project/session again",
            )
        })?;
    Ok(sound)
}

fn reuse_cached_catalog(
    entries: &mut Vec<Arc<Catalog>>,
    key: &str,
) -> Result<Option<Arc<Catalog>>, String> {
    let Some(index) = entries.iter().position(|catalog| catalog.key == key) else {
        return Ok(None);
    };
    // The caller holds CATALOGS while the issued generation is restored. Keeping this lock order
    // makes the catalog token and its registry entry observable as one cache-hit operation.
    if !restore_catalog_generation(&entries[index])? {
        // A different token belongs to a newer snapshot. Preserve that registry entry and remove
        // this stale heavyweight catalog so the caller rebuilds it under the current generation.
        entries.remove(index).control.cancel();
        return Ok(None);
    }
    let catalog = entries.remove(index);
    let result = Arc::clone(&catalog);
    entries.push(catalog);
    Ok(Some(result))
}

fn resolve_sound(key: &str, id: &str) -> Result<Sound, String> {
    let sound = issued_sound(key, id)?;
    Ok(sound.as_ref().clone())
}

fn resolve_library_sound(key: &str, id: &str) -> Result<Sound, String> {
    resolve_sound(key, id).or_else(|_| resolve_sound(&isolated_sound_library_key(key), id))
}

fn generated_sound_id(generation: &str, source_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(generation.as_bytes());
    digest.update([0]);
    digest.update(source_id.as_bytes());
    let digest = digest.finalize();
    // Fifteen bytes encode to twenty URL-safe characters. Including `s:` keeps the complete
    // opaque handle at 22 characters, below the public 23-character contract.
    format!(
        "s:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..15])
    )
}

/// Immutable snapshot; run discovery on an ordinary worker, never the UI or audio thread.
pub struct SoundLibraryJob {
    key: String,
    registry: Arc<PluginRegistry>,
    sounds: Vec<Sound>,
    sound_bytes: usize,
    base_truncated: bool,
    font_paths: Vec<PathBuf>,
    extra_paths: Vec<PathBuf>,
    scan_plugins: bool,
    isolated_plugin_discovery: bool,
    isolated_plugin_snapshot: Option<IsolatedPluginCatalog>,
    sample_rate: f64,
}

fn lexical_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn path_identity(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| lexical_path(path))
        .to_string_lossy()
        .into_owned()
}

fn lexical_path_identity(path: &Path) -> String {
    // Preserve case, prefixes, and `..`: resolving any of them without filesystem I/O can merge
    // distinct paths across case-sensitive directories, symlinks, or junctions.
    lexical_path(path).to_string_lossy().into_owned()
}

fn hash_scope_part(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value.as_bytes());
}

fn isolated_sound_library_key(key: &str) -> String {
    format!("{key}:isolated-native-metadata-v1")
}

impl Session {
    fn sound_library_scope(&self, extra_paths: &[PathBuf]) -> String {
        let mut digest = Sha256::new();
        digest.update(b"auris-sound-library-scope-v1");
        match self.path() {
            Some(path) => {
                digest.update(b"saved");
                hash_scope_part(&mut digest, &path_identity(path));
            }
            None => {
                digest.update(b"live");
                hash_scope_part(&mut digest, &self.sound_scope);
            }
        }
        digest.update(self.sample_rate().to_bits().to_le_bytes());
        for reference in self.soundfonts() {
            digest.update(reference.id.0.to_le_bytes());
            hash_scope_part(&mut digest, &reference.name);
            hash_scope_part(&mut digest, &format!("{:?}", reference.path));
            digest.update(reference.byte_size.to_le_bytes());
            digest.update([u8::from(self.soundfont_is_loaded(reference.id))]);
        }
        let mut paths = extra_paths
            .iter()
            // Extra roots may name disconnected or network volumes. Their lexical identity is
            // sufficient for scoping and avoids touching every root on the UI thread.
            .map(|path| lexical_path_identity(path))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        for path in paths {
            hash_scope_part(&mut digest, &path);
        }
        format!("{:x}", digest.finalize())
    }

    /// Snapshots loaded libraries and plugin search roots without loading or walking plugins.
    pub fn sound_library_job(&self, extra_paths: &[PathBuf]) -> SoundLibraryJob {
        let key = self.sound_library_scope(extra_paths);
        let fonts = auris_sampler::SoundFontBank::shared();
        let mut sounds = Vec::new();
        let mut budget = CatalogBudget::default();
        for descriptor in self
            .registry()
            .instruments()
            .filter(|descriptor| descriptor.id != SAMPLER_ID)
        {
            let _ = budget.push_sound(
                &mut sounds,
                Sound::new(
                    descriptor.name.to_string(),
                    "Auris".into(),
                    descriptor.id.to_string(),
                    Source::Builtin(descriptor.id.to_string()),
                ),
            );
        }
        let mut font_paths = Vec::new();
        for reference in self.soundfonts() {
            if let Some(font) = self.fonts.get(reference.id) {
                fonts.insert(reference.id, font);
                if let Some(path) = reference.path.resolve(self.project_folder()) {
                    font_paths.push(path);
                }
                if budget.sounds_truncated {
                    continue;
                }
                for preset in self.soundfont_presets(reference.id) {
                    let tags = if matches!(preset.bank, 0 | 128) && (0..128).contains(&preset.patch)
                    {
                        gm::Program(preset.patch as u8)
                            .label(preset.bank == 128)
                            .to_string()
                    } else {
                        String::new()
                    };
                    if !budget.push_sound(
                        &mut sounds,
                        Sound::new(
                            preset.name,
                            reference.name.clone(),
                            tags,
                            Source::Font(PresetRef {
                                font: reference.id,
                                bank: preset.bank,
                                patch: preset.patch,
                            }),
                        ),
                    ) {
                        break;
                    }
                }
            }
        }
        SoundLibraryJob {
            key,
            registry: crate::default_registry(fonts),
            sounds,
            sound_bytes: budget.sound_bytes,
            base_truncated: budget.sounds_truncated,
            font_paths,
            extra_paths: extra_paths.to_vec(),
            scan_plugins: true,
            isolated_plugin_discovery: false,
            isolated_plugin_snapshot: None,
            sample_rate: self.sample_rate(),
        }
    }

    /// Applies a previously discovered exact sound, including native preset state, in one undo step.
    pub fn use_library_sound(
        &mut self,
        track: TrackId,
        id: &str,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        self.require_track(track).map_err(|e| e.to_string())?;
        if !self
            .project
            .track(track)
            .is_some_and(|t| t.kind.is_instrument())
        {
            return Err("Choose an instrument or drum track".into());
        }
        let key = self.sound_library_scope(paths);
        // Selection can run on the UI thread. Resolve only metadata previously issued by a worker;
        // rebuilding a heavy catalog here would synchronously invoke every native provider.
        let source = resolve_library_sound(&key, id)?;
        let prepare = PrepareContext::new(self.sample_rate(), 512, 2);
        match source.source {
            Source::Builtin(instrument_id) => self
                .use_timbre_sound(
                    track,
                    &TimbreSound {
                        name: source.name,
                        instrument_id,
                        preset: None,
                    },
                )
                .map_err(|e| e.to_string()),
            Source::Font(preset) => self
                .set_track_preset(track, preset)
                .map_err(|e| e.to_string()),
            Source::Clap {
                file,
                id,
                preset,
                discovery_stamp,
                isolated_metadata,
            } => {
                if !isolated_metadata
                    && clap_selection_stamp(&file, preset.as_ref()) != discovery_stamp
                {
                    return Err(
                        "The selected CLAP plugin or preset changed after discovery; refresh the sound search"
                            .into(),
                    );
                }
                // SAFETY: the selected ID was discovered from the user's configured plugin library.
                let library =
                    unsafe { auris_clap::ClapLibrary::load(&file) }.map_err(|e| e.to_string())?;
                let mut plugin = library.instantiate(&id).map_err(|e| e.to_string())?;
                if let Some(preset) = preset {
                    plugin.load_preset(&preset)?;
                }
                let audio = plugin
                    .activate_instrument(&prepare)
                    .map_err(|e| e.to_string())?;
                plugin.deactivate_instrument(audio);
                let bytes = plugin.save_state().map_err(|e| e.to_string())?;
                self.collect_hosted_state().map_err(|e| e.to_string())?;
                self.record(crate::Edit::ChangeInstrument);
                self.project.set_hosted_instrument(
                    track,
                    format!("clap:{id}"),
                    auris_core::asset::AssetPath::external(&file),
                );
                self.store_sound_state(track, &bytes);
                self.hosted
                    .install_composed_instrument(track, file, id, plugin);
                self.invalidate_graph();
                Ok(())
            }
            Source::Vst3 {
                file,
                id,
                preset,
                discovery_stamp,
                isolated_metadata,
            } => {
                if !isolated_metadata
                    && vst3_selection_stamp(&file, preset.as_ref()) != discovery_stamp
                {
                    return Err(
                        "The selected VST3 plugin or preset changed after discovery; refresh the sound search"
                            .into(),
                    );
                }
                let plugin = auris_vst3::Vst3Plugin::load(&file, &id, &prepare)
                    .map_err(|e| e.to_string())?;
                if let Some(preset) = preset {
                    plugin.load_preset(&preset).map_err(|e| e.to_string())?;
                }
                let bytes = plugin.save_state().map_err(|e| e.to_string())?;
                self.collect_hosted_state().map_err(|e| e.to_string())?;
                self.record(crate::Edit::ChangeInstrument);
                self.project.set_hosted_instrument(
                    track,
                    format!("vst3:{id}"),
                    auris_core::asset::AssetPath::external(&file),
                );
                self.store_sound_state(track, &bytes);
                self.vst3
                    .install_composed_instrument(track, file, id, plugin);
                self.invalidate_graph();
                Ok(())
            }
        }
    }
    fn store_sound_state(&mut self, track: TrackId, bytes: &[u8]) {
        if let Some(inner) = self
            .project
            .track_mut(track)
            .and_then(|t| t.kind.as_instrument_mut())
        {
            inner.instrument_state.set_hosted_bytes(bytes);
        }
        self.project.remove_instrument_automation(track);
    }
}

impl SoundLibraryJob {
    /// Reads bounded scan or measurement diagnostics from the existing snapshot only.
    pub fn diagnostics(&self, offset: usize, limit: usize) -> Result<String, String> {
        if !(1..=16).contains(&limit) {
            return Err("limit must be 1..16".into());
        }
        let isolated_key = isolated_sound_library_key(&self.key);
        let catalog = CATALOGS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Sound catalog lock failed")?
            .iter()
            .rev()
            .find(|c| c.key == self.key || c.key == isolated_key)
            .cloned()
            .ok_or("No sound snapshot; search_instruments first")?;
        let state = catalog
            .index
            .lock()
            .map_err(|_| "Acoustic index lock failed")?;
        let mut errors = catalog
            .errors
            .iter()
            .map(|error| ("scan", error))
            .collect::<Vec<_>>();
        if let Some(Ok(index)) = &state.result {
            errors.extend(index.skipped.iter().map(|error| ("measurement", error)));
        }
        if let Some(Err(error)) = &state.result {
            errors.push(("index", error));
        }
        if offset > errors.len() {
            return Err("offset exceeds diagnostic count".into());
        }
        let entries = errors
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(kind, error)| serde_json::json!({"kind":kind,"message":short_text(error,240)}))
            .collect::<Vec<_>>();
        let next = offset + entries.len();
        Ok(serde_json::json!({"entries":entries,"total":errors.len(),"next_offset":(next<errors.len()).then_some(next)}).to_string())
    }
    /// Returns a bounded search response. First acoustic use starts a background index and
    /// returns `status: indexing`; subsequent calls report progress or completed neighbors.
    pub fn run(self, request: SoundSearch, refresh: bool) -> Result<String, String> {
        self.run_inner(request, refresh, None)
    }

    /// Runs live-window discovery through bounded child processes before searching metadata.
    ///
    /// Native presets and acoustic measurements are deliberately unavailable in this mode: both
    /// require instantiating third-party code, while the returned default descriptors remain valid
    /// handles for the ordinary explicit instrument-selection command.
    pub fn run_isolated(
        mut self,
        request: SoundSearch,
        refresh: bool,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        self.key = isolated_sound_library_key(&self.key);
        self.isolated_plugin_discovery = true;
        self.run_inner(request, refresh, Some(cancelled))
    }

    fn run_inner(
        self,
        request: SoundSearch,
        refresh: bool,
        cancelled: Option<&AtomicBool>,
    ) -> Result<String, String> {
        if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
            return Err("Sound search cancelled".into());
        }
        let limit = match &request {
            SoundSearch::Text { limit, .. } | SoundSearch::Similar { limit, .. } => *limit,
        };
        if !(1..=50).contains(&limit) {
            return Err("limit must be between 1 and 50".into());
        }
        if let SoundSearch::Text { query, offset, .. } = &request {
            if query.trim().is_empty() {
                return Err(
                    "query must contain search words; do not request the entire library".into(),
                );
            }
            if refresh && *offset != 0 {
                return Err("Refresh only with offset 0".into());
            }
        }
        if let SoundSearch::Similar { id, .. } = &request {
            issued_sound(&self.key, id)
                .map_err(|_| "Unknown or expired sound ID; search_instruments first")?;
        }
        let catalog = self.catalog_with_cancel(refresh, cancelled)?;
        match request {
            SoundSearch::Text {
                query,
                offset,
                filter,
                ..
            } => {
                let words = query
                    .split_whitespace()
                    .map(str::to_lowercase)
                    .collect::<Vec<_>>();
                let matches = catalog
                    .sounds
                    .iter()
                    .filter(|s| s.matches(&words) && filter.matches(s))
                    .collect::<Vec<_>>();
                if offset > matches.len() {
                    return Err("offset exceeds matching sound count".into());
                }
                let page = matches
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .copied()
                    .collect::<Vec<_>>();
                let next = offset + page.len();
                issue_sounds(&self.key, &catalog.generation, page.iter().copied())?;
                let page = page.into_iter().map(Sound::value).collect::<Vec<_>>();
                Ok(sound_page(with_native_status(
                    serde_json::json!({"status":"ready","sounds":page,"total":matches.len(),"next_offset":(next<matches.len()).then_some(next),
                    "scan_error_count":catalog.errors.len()}),
                    &catalog,
                )))
            }
            SoundSearch::Similar { id, filter, .. } => {
                let reference = catalog
                    .sounds
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or("Unknown sound ID; search_instruments first")?;
                if !measurable(&catalog.sounds[reference]) {
                    if isolated_native(&catalog.sounds[reference]) {
                        return Err(
                            "Native plugins discovered by the live Agent use isolated metadata; acoustic similarity is unavailable because measuring them would load third-party code in the application process"
                                .into(),
                        );
                    }
                    return Err("Drum kits are searchable but are not comparable with the melodic timbre reference".into());
                }
                let mut state = catalog
                    .index
                    .lock()
                    .map_err(|_| "Acoustic index lock failed")?;
                if state.reset_if_reference_missing(reference) {
                    catalog.completed.store(0, Ordering::Relaxed);
                }
                if let Some(result) = &state.result {
                    let index = result.as_ref().map_err(Clone::clone)?;
                    let row = index.sounds.iter().position(|i| *i == reference).ok_or(
                        "The reference is outside this bounded acoustic index or could not be measured; choose a returned neighbor or refresh and start with this sound",
                    )?;
                    let mut nearest = index
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            *i != row && filter.matches(&catalog.sounds[index.sounds[*i]])
                        })
                        .map(|(i, other)| {
                            let distance = other
                                .iter()
                                .zip(&index.rows[row])
                                .map(|(a, b)| (a - b).powi(2))
                                .sum::<f64>()
                                .sqrt();
                            (i, distance)
                        })
                        .collect::<Vec<_>>();
                    nearest.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                    let nearest = nearest.into_iter().take(limit).collect::<Vec<_>>();
                    issue_sounds(
                        &self.key,
                        &catalog.generation,
                        nearest
                            .iter()
                            .map(|(i, _)| &catalog.sounds[index.sounds[*i]]),
                    )?;
                    let sounds = nearest
                        .into_iter()
                        .map(|(i, d)| {
                            let mut v = catalog.sounds[index.sounds[i]].value();
                            v["distance"] = d.into();
                            v
                        })
                        .collect::<Vec<_>>();
                    return Ok(sound_page(with_native_status(
                        serde_json::json!({"status":"ready","reference":id,"sounds":sounds,"indexed":index.sounds.len(),
                        "skipped_count":index.skipped.len(),"limited":index.limited}),
                        &catalog,
                    )));
                }
                if !state.started {
                    let permit = AcousticWorkerPermit::acquire().ok_or(
                        "Sound acoustic analysis is busy; retry after another index finishes",
                    )?;
                    let worker_catalog = catalog.clone();
                    std::thread::Builder::new()
                        .name("auris-sound-index".into())
                        .spawn(move || {
                            let _permit = permit;
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    self.measure(&worker_catalog, reference)
                                }))
                                .unwrap_or_else(|_| Err("Sound index worker panicked".into()));
                            if let Ok(mut state) = worker_catalog.index.lock() {
                                state.result = Some(result);
                            }
                        })
                        .map_err(|e| e.to_string())?;
                    state.started = true;
                    state.reference = Some(reference);
                }
                Ok(with_native_status(
                    serde_json::json!({"status":"indexing","completed":catalog.completed.load(Ordering::Relaxed),"total":catalog.sounds.iter().filter(|s| measurable(s)).count().min(ACOUSTIC_SOUND_LIMIT),"limited":catalog.sounds.iter().filter(|s| measurable(s)).count()>ACOUSTIC_SOUND_LIMIT,"retry_after_seconds":5,
                    "usage":"Acoustic analysis runs in the background once per library snapshot. Continue other work and retry similar_instruments later with the same id; do not refresh while indexing."}),
                    &catalog,
                )
                .to_string())
            }
        }
    }

    #[cfg(test)]
    fn catalog(&self, refresh: bool) -> Result<Arc<Catalog>, String> {
        self.catalog_with_cancel(refresh, None)
    }

    fn catalog_with_cancel(
        &self,
        refresh: bool,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Arc<Catalog>, String> {
        let cache = CATALOGS.get_or_init(Default::default);
        if !refresh {
            let mut entries = cache.lock().map_err(|_| "Sound catalog lock failed")?;
            if let Some(catalog) = reuse_cached_catalog(&mut entries, &self.key)? {
                return Ok(catalog);
            }
        }
        // Native discovery can hang. Never queue caller threads behind it: cached reads bypass
        // this permit, while misses and refreshes fail fast and can be retried by the frontend.
        let _build = try_discovery_lock(&CATALOG_BUILD)?;
        let refreshed_generation = {
            let mut entries = cache.lock().map_err(|_| "Sound catalog lock failed")?;
            // Another builder may have populated this scope before this caller acquired the
            // non-blocking build permit.
            if !refresh && let Some(catalog) = reuse_cached_catalog(&mut entries, &self.key)? {
                return Ok(catalog);
            }
            if refresh
                && let Some(active) = entries
                    .iter()
                    .find(|catalog| catalog.key == self.key && Arc::strong_count(catalog) > 1)
            {
                active.control.cancel();
                return Err(
                    "Sound discovery cancellation was requested; retry after the current search finishes"
                        .into(),
                );
            }
            if refresh
                && entries.len() >= CATALOG_LIMIT
                && !entries
                    .iter()
                    .any(|catalog| catalog.key == self.key || Arc::strong_count(catalog) == 1)
            {
                return Err("Sound discovery is busy; retry after another search finishes".into());
            }
            let generation = if refresh {
                // Rotate handles before a provider scan can block. A late result from the
                // cancelled catalog then fails its generation check instead of reviving stale
                // selections.
                Some(catalog_generation(&self.key, None, true)?)
            } else {
                None
            };
            if refresh {
                entries.retain(|c| {
                    if c.key == self.key {
                        c.control.cancel();
                        false
                    } else {
                        true
                    }
                });
            }
            reserve_catalog_slot(&mut entries)?;
            generation
        };
        // Filesystem discovery and stamps belong to this worker-only method. Constructing a job,
        // resolving an issued handle, and reading diagnostics never recursively walk plugin roots.
        let mut sounds = self.sounds.clone();
        let mut budget = CatalogBudget {
            sound_bytes: self.sound_bytes,
            seen: self.sounds.iter().map(|sound| sound.id.clone()).collect(),
            sounds_truncated: self.base_truncated,
            errors_truncated: false,
        };
        let mut errors = Vec::new();
        let mut stamps = self
            .font_paths
            .iter()
            .map(|path| stamp(path))
            .collect::<Vec<_>>();
        let mut native_status = None;
        if self.scan_plugins && self.isolated_plugin_discovery {
            let snapshot = match &self.isolated_plugin_snapshot {
                Some(snapshot) => snapshot.clone(),
                None => isolated_plugin_catalog(&self.extra_paths, cancelled)?,
            };
            for diagnostic in &snapshot.diagnostics {
                budget.push_error(&mut errors, diagnostic);
            }
            stamps.extend(
                snapshot
                    .entries
                    .iter()
                    .map(|entry| lexical_path_identity(&entry.path)),
            );
            for entry in &snapshot.entries {
                if budget.sounds_truncated {
                    break;
                }
                discover_isolated_plugin(entry, &mut sounds, &mut budget);
            }
            native_status = Some(snapshot.status);
        } else if self.scan_plugins {
            let plugin_files = installed_plugin_files(&self.extra_paths);
            if plugin_files.truncated {
                budget.push_error(
                    &mut errors,
                    "Plugin discovery reached its bounded path or traversal limit",
                );
            }
            let plugins = plugin_files.entries;
            stamps.extend(plugins.iter().map(|(path, _)| stamp(path)));
            stamps.extend(vst_preset_roots().iter().map(|path| stamp(path)));
            let (preset_files, presets_truncated) = if plugins.iter().any(|(_, vst3)| *vst3) {
                vst_preset_files()
            } else {
                (Vec::new(), false)
            };
            if presets_truncated {
                budget.push_error(
                    &mut errors,
                    "VST3 preset discovery reached its bounded path or traversal limit",
                );
            }
            for (file, vst3) in &plugins {
                if budget.sounds_truncated {
                    break;
                }
                let result = if *vst3 {
                    discover_vst3(
                        file,
                        &preset_files,
                        self.sample_rate,
                        &mut sounds,
                        &mut errors,
                        &mut budget,
                    )
                } else {
                    discover_clap(file, &mut sounds, &mut errors, &mut budget)
                };
                if let Err(error) = result {
                    budget.push_error(&mut errors, format!("{}: {error}", file.display()));
                }
            }
        }
        budget.report_sound_limit(&mut errors);
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}{:?}{:?}{}",
                self.key, stamps, self.sounds, self.sample_rate
            ))
        );
        sounds.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        let generation = catalog_generation(&self.key, Some(&fingerprint), false)?;
        if refreshed_generation
            .as_ref()
            .is_some_and(|reserved| reserved != &generation)
        {
            return Err("Sound search refresh was superseded; retry the search".into());
        }
        for sound in &mut sounds {
            sound.id = generated_sound_id(&generation, &sound.id);
        }
        let catalog = Arc::new(Catalog {
            key: self.key.clone(),
            fingerprint,
            generation,
            sounds,
            errors,
            index: Mutex::new(IndexState::default()),
            completed: AtomicUsize::new(0),
            control: TimbreMapControl::default(),
            native_status,
        });
        let mut entries = cache.lock().map_err(|_| "Sound catalog lock failed")?;
        if let Some(existing) = reuse_cached_catalog(&mut entries, &self.key)? {
            return Ok(existing);
        }
        if !restore_catalog_generation(&catalog)? {
            return Err("Sound search generation was superseded; retry the search".into());
        }
        debug_assert!(entries.len() < CATALOG_LIMIT);
        entries.push(catalog.clone());
        Ok(catalog)
    }

    fn measure(&self, catalog: &Catalog, reference: usize) -> Result<AcousticIndex, String> {
        let mut sounds = Vec::new();
        let mut rows = Vec::new();
        let mut skipped = Vec::new();
        let mut skipped_truncated = false;
        let measurable_count = catalog
            .sounds
            .iter()
            .filter(|sound| measurable(sound))
            .count();
        let candidates = std::iter::once(reference)
            .chain(
                (0..catalog.sounds.len())
                    .filter(|index| *index != reference && measurable(&catalog.sounds[*index])),
            )
            .take(ACOUSTIC_SOUND_LIMIT)
            .collect::<Vec<_>>();
        let mut row_bytes = 0usize;
        let mut index_bytes_limited = false;
        for index in candidates {
            let sound = &catalog.sounds[index];
            match measure_sound(sound, &self.registry, self.sample_rate, &catalog.control) {
                Ok(row) => {
                    let bytes = row.len().saturating_mul(std::mem::size_of::<f64>());
                    if row_bytes.saturating_add(bytes) > ACOUSTIC_INDEX_BYTE_LIMIT {
                        if index == reference {
                            return Err(
                                "Reference acoustic features exceed the index budget".into()
                            );
                        }
                        index_bytes_limited = true;
                        break;
                    }
                    row_bytes += bytes;
                    sounds.push(index);
                    rows.push(row);
                }
                Err(error) => {
                    if index == reference {
                        return Err(format!(
                            "Reference {} could not be measured: {}",
                            short_text(&sound.name, 160),
                            short_text(&error, 240)
                        ));
                    }
                    if skipped.len() < CATALOG_ERROR_LIMIT {
                        skipped.push(short_text(
                            &format!("{}: {error}", sound.name),
                            CATALOG_ERROR_CHARACTER_LIMIT,
                        ));
                    } else if !skipped_truncated {
                        if let Some(last) = skipped.last_mut() {
                            *last =
                                "Additional acoustic measurement diagnostics were omitted".into();
                        }
                        skipped_truncated = true;
                    }
                }
            }
            catalog.completed.fetch_add(1, Ordering::Relaxed);
            catalog.control.check().map_err(|e| e.to_string())?;
        }
        Ok(AcousticIndex {
            sounds,
            rows: standardize_timbres(&rows).map_err(str::to_string)?,
            skipped,
            limited: measurable_count > ACOUSTIC_SOUND_LIMIT
                || index_bytes_limited
                || skipped_truncated,
        })
    }
}

fn sound_page(mut value: serde_json::Value) -> String {
    let mut libraries = Vec::new();
    if let Some(sounds) = value["sounds"].as_array_mut() {
        for sound in sounds {
            let metadata = serde_json::json!({"name":sound["library"],"source":sound["source"]});
            let index = libraries
                .iter()
                .position(|entry| *entry == metadata)
                .unwrap_or_else(|| {
                    libraries.push(metadata);
                    libraries.len() - 1
                });
            sound["library"] = index.into();
            sound.as_object_mut().unwrap().remove("source");
        }
    }
    value["libraries"] = libraries.into();
    value.to_string()
}

fn with_native_status(mut value: serde_json::Value, catalog: &Catalog) -> serde_json::Value {
    if let Some(status) = &catalog.native_status {
        value["native_plugins"] = status.value();
    }
    value
}

fn measurable(sound: &Sound) -> bool {
    match &sound.source {
        Source::Font(p) => p.bank != 128,
        Source::Builtin(id) => {
            !matches!(id.as_str(), "auris.synth.drumkit" | "auris.synth.noisedrum")
        }
        Source::Clap {
            isolated_metadata, ..
        }
        | Source::Vst3 {
            isolated_metadata, ..
        } => !isolated_metadata,
    }
}

fn isolated_native(sound: &Sound) -> bool {
    matches!(
        &sound.source,
        Source::Clap {
            isolated_metadata: true,
            ..
        } | Source::Vst3 {
            isolated_metadata: true,
            ..
        }
    )
}

fn short_text(text: &str, limit: usize) -> String {
    let mut characters = text.chars();
    let mut result = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        result.push('…');
    }
    result
}

fn isolated_plugin_catalog(
    extra_paths: &[PathBuf],
    cancelled: Option<&AtomicBool>,
) -> Result<IsolatedPluginCatalog, String> {
    let never_cancelled = AtomicBool::new(false);
    let cancelled = cancelled.unwrap_or(&never_cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Sound search cancelled".into());
    }
    let started = Instant::now();
    let deadline = started + ISOLATED_PLUGIN_CATALOG_TIMEOUT;
    let mut catalog = IsolatedPluginCatalog::default();
    let inventory = match crate::PluginDiscoveryJob::new(extra_paths).run(
        cancelled,
        ISOLATED_PLUGIN_DISCOVERY_TIMEOUT.min(ISOLATED_PLUGIN_CATALOG_TIMEOUT),
    ) {
        Ok(inventory) => inventory,
        Err(_) if cancelled.load(Ordering::Relaxed) => {
            return Err("Sound search cancelled".into());
        }
        Err(error) => {
            catalog.status.truncated = true;
            catalog.diagnostics.push(format!(
                "Isolated native plugin inventory failed; built-in and SoundFont results remain available: {error}"
            ));
            return Ok(catalog);
        }
    };
    catalog.status.discovered_files = inventory.clap.len().saturating_add(inventory.vst3.len());
    catalog.status.truncated = inventory.truncated;
    if inventory.truncated {
        catalog.diagnostics.push(
            "Isolated native plugin inventory reached its shared count or path-byte limit; additional files were omitted"
                .into(),
        );
    }

    let candidates = isolated_plugin_candidates(&inventory);
    if catalog.status.discovered_files > candidates.len() {
        catalog.status.truncated = true;
    }

    let mut metadata_classes = 0usize;
    let mut metadata_bytes = 0usize;
    for (path, format) in candidates {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Sound search cancelled".into());
        }
        let now = Instant::now();
        let Some(overall_remaining) = deadline.checked_duration_since(now) else {
            catalog.status.truncated = true;
            break;
        };
        if overall_remaining < Duration::from_millis(100) {
            catalog.status.truncated = true;
            break;
        }
        let probe_deadline = now + overall_remaining.min(ISOLATED_PLUGIN_PROBE_TIMEOUT);
        catalog.status.inspected_files += 1;
        match crate::PluginProbeJob::new(format, path.clone()).run_until(cancelled, probe_deadline)
        {
            Ok(metadata) => {
                let (classes, bytes) = plugin_probe_usage(&metadata);
                if metadata_classes.saturating_add(classes) > CATALOG_SOUND_LIMIT
                    || metadata_bytes.saturating_add(bytes) > CATALOG_SOUND_BYTE_LIMIT
                {
                    catalog.status.truncated = true;
                    catalog.diagnostics.push(
                        "Isolated native plugin descriptors reached the shared catalog count or byte limit; additional files were omitted"
                            .into(),
                    );
                    break;
                }
                metadata_classes += classes;
                metadata_bytes += bytes;
                catalog.entries.push(IsolatedPluginEntry { path, metadata });
            }
            Err(_) if cancelled.load(Ordering::Relaxed) => {
                return Err("Sound search cancelled".into());
            }
            Err(error) => {
                catalog.status.failed_files += 1;
                catalog.diagnostics.push(format!(
                    "{}: isolated metadata inspection failed: {error}",
                    path.display()
                ));
            }
        }
    }
    catalog.status.skipped_files = catalog
        .status
        .discovered_files
        .saturating_sub(catalog.entries.len());
    if catalog.status.skipped_files > 0 {
        catalog.status.truncated = true;
        catalog.diagnostics.push(format!(
            "Isolated native plugin search inspected {} of {} discovered files; {} were unavailable because of failed inspection or the 256-file, shared-metadata, or 30-second limit",
            catalog.status.inspected_files,
            catalog.status.discovered_files,
            catalog.status.skipped_files
        ));
    }
    if catalog.status.discovered_files > 0 {
        catalog.diagnostics.push(
            "Live Agent native plugins use isolated default descriptors; native preset enumeration and acoustic similarity are unavailable"
                .into(),
        );
    }
    Ok(catalog)
}

fn isolated_plugin_candidates(
    inventory: &crate::InstalledPluginFiles,
) -> Vec<(PathBuf, crate::PluginFormat)> {
    // Alternate formats so a machine with hundreds of CLAP files cannot use the whole live
    // catalog allowance before the first VST3 descriptor (or vice versa).
    let mut clap = inventory.clap.iter().cloned();
    let mut vst3 = inventory.vst3.iter().cloned();
    let mut candidates = Vec::with_capacity(
        inventory
            .clap
            .len()
            .saturating_add(inventory.vst3.len())
            .min(ISOLATED_PLUGIN_FILE_LIMIT),
    );
    while candidates.len() < ISOLATED_PLUGIN_FILE_LIMIT {
        let mut advanced = false;
        if let Some(path) = clap.next() {
            candidates.push((path, crate::PluginFormat::Clap));
            advanced = true;
        }
        if candidates.len() < ISOLATED_PLUGIN_FILE_LIMIT
            && let Some(path) = vst3.next()
        {
            candidates.push((path, crate::PluginFormat::Vst3));
            advanced = true;
        }
        if !advanced {
            break;
        }
    }
    candidates
}

fn plugin_probe_usage(metadata: &crate::PluginProbeResult) -> (usize, usize) {
    match metadata {
        crate::PluginProbeResult::Clap(plugins) => (
            plugins.len(),
            plugins.iter().fold(0usize, |bytes, plugin| {
                bytes
                    .saturating_add(std::mem::size_of_val(plugin))
                    .saturating_add(plugin.clap_id.len())
                    .saturating_add(plugin.name.len())
                    .saturating_add(plugin.vendor.len())
                    .saturating_add(plugin.description.len())
                    .saturating_add(plugin.version.len())
            }),
        ),
        crate::PluginProbeResult::Vst3(plugins) => (
            plugins.len(),
            plugins.iter().fold(0usize, |bytes, plugin| {
                bytes
                    .saturating_add(std::mem::size_of_val(plugin))
                    .saturating_add(plugin.class_id.len())
                    .saturating_add(plugin.name.len())
                    .saturating_add(plugin.vendor.len())
                    .saturating_add(plugin.version.len())
            }),
        ),
    }
}

fn discover_isolated_plugin(
    entry: &IsolatedPluginEntry,
    sounds: &mut Vec<Sound>,
    budget: &mut CatalogBudget,
) {
    match &entry.metadata {
        crate::PluginProbeResult::Clap(plugins) => {
            for info in plugins
                .iter()
                .filter(|info| info.kind == PluginKind::Instrument)
            {
                let label = format!("{} / {}", info.vendor, info.name);
                if !budget.push_sound(
                    sounds,
                    Sound::new(
                        format!("{} (default)", info.name),
                        label,
                        info.description.clone(),
                        Source::Clap {
                            file: entry.path.clone(),
                            id: info.clap_id.clone(),
                            preset: None,
                            discovery_stamp: String::new(),
                            isolated_metadata: true,
                        },
                    ),
                ) {
                    break;
                }
            }
        }
        crate::PluginProbeResult::Vst3(plugins) => {
            for info in plugins
                .iter()
                .filter(|info| info.kind == PluginKind::Instrument)
            {
                let label = format!("{} / {}", info.vendor, info.name);
                if !budget.push_sound(
                    sounds,
                    Sound::new(
                        format!("{} (default)", info.name),
                        label,
                        String::new(),
                        Source::Vst3 {
                            file: entry.path.clone(),
                            id: info.class_id.clone(),
                            preset: None,
                            discovery_stamp: String::new(),
                            isolated_metadata: true,
                        },
                    ),
                ) {
                    break;
                }
            }
        }
    }
}

fn measure_sound(
    sound: &Sound,
    registry: &PluginRegistry,
    rate: f64,
    control: &TimbreMapControl,
) -> Result<Vec<f64>, String> {
    let prepare = PrepareContext::new(rate, 512, 2);
    let mut features = Vec::new();
    for pitch in [48, 60, 72] {
        for velocity in [0.45, 0.85] {
            control.check().map_err(|e| e.to_string())?;
            let render = |instrument: &mut dyn Instrument| {
                super::timbre::render_reference(instrument, rate, pitch, velocity, control)
                    .map_err(|e| e.to_string())
            };
            let audio = match &sound.source {
                Source::Builtin(id) => {
                    let mut i = registry.create_instrument(id).map_err(|e| e.to_string())?;
                    i.prepare(&prepare);
                    render(i.as_mut())?
                }
                Source::Font(preset) => {
                    let mut i = registry
                        .create_instrument(SAMPLER_ID)
                        .map_err(|e| e.to_string())?;
                    let mut state = PluginState::default();
                    auris_sampler::store_preset(&mut state, *preset);
                    i.load_state(&state);
                    i.prepare(&prepare);
                    render(i.as_mut())?
                }
                Source::Clap {
                    file, id, preset, ..
                } => {
                    // SAFETY: file came from the user's configured plugin library.
                    let library = unsafe { auris_clap::ClapLibrary::load(file) }
                        .map_err(|e| e.to_string())?;
                    let mut plugin = library.instantiate(id).map_err(|e| e.to_string())?;
                    if let Some(preset) = preset {
                        plugin.load_preset(preset)?;
                    }
                    let mut instrument = plugin
                        .activate_instrument(&prepare)
                        .map_err(|e| e.to_string())?;
                    let result = render(&mut instrument);
                    plugin.deactivate_instrument(instrument);
                    result?
                }
                Source::Vst3 {
                    file, id, preset, ..
                } => {
                    let plugin = auris_vst3::Vst3Plugin::load(file, id, &prepare)
                        .map_err(|e| e.to_string())?;
                    if let Some(preset) = preset {
                        plugin.load_preset(preset).map_err(|e| e.to_string())?;
                    }
                    let mut instrument = plugin.instrument().map_err(|e| e.to_string())?;
                    render(&mut instrument)?
                }
            };
            features.extend(
                timbre_features(&audio, 0.6)
                    .map_err(str::to_string)?
                    .ok_or("Silent reference trigger")?,
            );
        }
    }
    Ok(features)
}

fn discover_clap(
    file: &Path,
    sounds: &mut Vec<Sound>,
    errors: &mut Vec<String>,
    budget: &mut CatalogBudget,
) -> Result<(), String> {
    // SAFETY: discovered in the user's configured plugin library.
    let library = unsafe { auris_clap::ClapLibrary::load(file) }.map_err(|e| e.to_string())?;
    let plugin_stamp = stamp(file);
    let mut plugins = library.plugins().map_err(|e| e.to_string())?;
    plugins.retain(|info| info.kind == PluginKind::Instrument);
    if plugins.len() > DISCOVERY_FILE_LIMIT {
        plugins.truncate(DISCOVERY_FILE_LIMIT);
        budget.push_error(
            errors,
            format!(
                "{}: additional CLAP plugin descriptors were omitted",
                file.display()
            ),
        );
    }
    if plugins.is_empty() {
        return Ok(());
    }
    let mut metadata = library.presets();
    let metadata_errors_truncated = metadata.errors.len() > CATALOG_ERROR_LIMIT;
    if metadata.errors.len() > CATALOG_ERROR_LIMIT {
        metadata.errors.truncate(CATALOG_ERROR_LIMIT);
    }
    if metadata.presets.len() > CATALOG_SOUND_LIMIT {
        metadata.presets.truncate(CATALOG_SOUND_LIMIT);
        budget.sounds_truncated = true;
    }
    for error in metadata.errors {
        budget.push_error(errors, format!("{}: {error}", file.display()));
    }
    if metadata_errors_truncated {
        budget.push_error(
            errors,
            format!(
                "{}: additional CLAP diagnostics were omitted",
                file.display()
            ),
        );
    }
    for info in plugins {
        let label = format!("{} / {}", info.vendor, info.name);
        if !budget.push_sound(
            sounds,
            Sound::new(
                format!("{} (default)", info.name),
                label.clone(),
                String::new(),
                Source::Clap {
                    file: file.into(),
                    id: info.clap_id.clone(),
                    preset: None,
                    discovery_stamp: plugin_stamp.clone(),
                    isolated_metadata: false,
                },
            ),
        ) {
            return Ok(());
        }
        match library.instantiate(&info.clap_id) {
            Ok(plugin) if plugin.supports_preset_loading() => {}
            Ok(_) => {
                budget.push_error(
                    errors,
                    format!("{}: no CLAP preset-load extension", info.name),
                );
                continue;
            }
            Err(error) => {
                budget.push_error(errors, format!("{}: {error}", info.name));
                continue;
            }
        }
        for preset in metadata
            .presets
            .iter()
            .filter(|p| p.plugin_ids.contains(&info.clap_id))
        {
            if !budget.push_sound(
                sounds,
                Sound::new(
                    preset.name.clone(),
                    label.clone(),
                    preset.features.join(" "),
                    Source::Clap {
                        file: file.into(),
                        id: info.clap_id.clone(),
                        preset: Some(preset.clone()),
                        discovery_stamp: clap_selection_stamp(file, Some(preset)),
                        isolated_metadata: false,
                    },
                ),
            ) {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn discover_vst3(
    file: &Path,
    files: &[(PathBuf, String)],
    rate: f64,
    sounds: &mut Vec<Sound>,
    errors: &mut Vec<String>,
    budget: &mut CatalogBudget,
) -> Result<(), String> {
    let plugin_stamp = stamp(file);
    let mut plugins = auris_vst3::plugins_in(file).map_err(|e| e.to_string())?;
    plugins.retain(|info| info.kind == PluginKind::Instrument);
    if plugins.len() > DISCOVERY_FILE_LIMIT {
        plugins.truncate(DISCOVERY_FILE_LIMIT);
        budget.push_error(
            errors,
            format!(
                "{}: additional VST3 class descriptors were omitted",
                file.display()
            ),
        );
    }
    for info in plugins {
        let label = format!("{} / {}", info.vendor, info.name);
        if !budget.push_sound(
            sounds,
            Sound::new(
                format!("{} (default)", info.name),
                label.clone(),
                String::new(),
                Source::Vst3 {
                    file: file.into(),
                    id: info.class_id.clone(),
                    preset: None,
                    discovery_stamp: plugin_stamp.clone(),
                    isolated_metadata: false,
                },
            ),
        ) {
            return Ok(());
        }
        let mut presets = match auris_vst3::Vst3Plugin::load(
            file,
            &info.class_id,
            &PrepareContext::new(rate, 512, 2),
        )
        .and_then(|p| p.presets())
        {
            Ok(presets) => presets,
            Err(error) => {
                budget.push_error(errors, format!("{}: {error}", info.name));
                Vec::new()
            }
        };
        for (path, class) in files
            .iter()
            .filter(|(_, class)| class.eq_ignore_ascii_case(&info.class_id))
        {
            let _ = class;
            presets.push((
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                auris_vst3::Vst3Preset::File(path.clone()),
            ));
        }
        let remaining = CATALOG_SOUND_LIMIT.saturating_sub(sounds.len());
        if presets.len() > remaining {
            presets.truncate(remaining);
            budget.sounds_truncated = true;
        }
        if presets.is_empty() {
            budget.push_error(
                errors,
                format!(
                    "{}: no advertised VST3 programs or standard preset files",
                    info.name
                ),
            );
        }
        for (name, preset) in presets {
            if !budget.push_sound(
                sounds,
                Sound::new(
                    name,
                    label.clone(),
                    String::new(),
                    Source::Vst3 {
                        file: file.into(),
                        id: info.class_id.clone(),
                        discovery_stamp: vst3_selection_stamp(file, Some(&preset)),
                        preset: Some(preset),
                        isolated_metadata: false,
                    },
                ),
            ) {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn stamp(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let metadata = path.metadata().ok();
    format!(
        "{}:{:?}:{:?}",
        path.display(),
        metadata.as_ref().map(|m| m.len()),
        metadata.and_then(|m| m.modified().ok())
    )
}

fn clap_selection_stamp(file: &Path, preset: Option<&auris_clap::ClapPreset>) -> String {
    match preset.and_then(|preset| preset.location.as_deref()) {
        Some(location) => format!("{}|{}", stamp(file), stamp(Path::new(location))),
        None => stamp(file),
    }
}

fn vst3_selection_stamp(file: &Path, preset: Option<&auris_vst3::Vst3Preset>) -> String {
    match preset {
        Some(auris_vst3::Vst3Preset::File(path)) => {
            format!("{}|{}", stamp(file), stamp(path))
        }
        _ => stamp(file),
    }
}

fn vst_preset_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(documents) = dirs::document_dir() {
            roots.push(documents.join("VST3 Presets"));
        }
        if let Some(data) = dirs::config_dir() {
            roots.push(data.join("VST3 Presets"));
        }
        if let Some(data) = std::env::var_os("PROGRAMDATA") {
            roots.push(PathBuf::from(data).join("VST3 Presets"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Library/Audio/Presets"));
        }
        roots.push(PathBuf::from("/Library/Audio/Presets"));
        roots.push(PathBuf::from("/Network/Library/Audio/Presets"));
    } else {
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".vst3/presets"));
        }
        roots.push(PathBuf::from("/usr/share/vst3/presets"));
        roots.push(PathBuf::from("/usr/local/share/vst3/presets"));
    }
    if let Some(folder) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        roots.push(folder.join(if cfg!(target_os = "linux") {
            "vst3/presets"
        } else {
            "VST3 Presets"
        }));
    }
    roots
}
fn vst_preset_files() -> (Vec<(PathBuf, String)>, bool) {
    use std::io::Read;
    let mut pending = Vec::new();
    let mut traversal_bytes = 0usize;
    let mut truncated = false;
    for path in vst_preset_roots() {
        let bytes = path.as_os_str().len();
        if traversal_bytes.saturating_add(bytes) > DISCOVERY_PATH_BYTE_LIMIT {
            truncated = true;
            break;
        }
        traversal_bytes += bytes;
        pending.push((path, 0usize));
    }
    let mut found = Vec::new();
    let mut found_bytes = 0usize;
    let mut visited = 0usize;
    'walk: while let Some((path, depth)) = pending.pop() {
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if depth >= DISCOVERY_DEPTH_LIMIT {
                truncated = true;
                continue;
            }
            if let Ok(entries) = path.read_dir() {
                for entry in entries.flatten() {
                    visited += 1;
                    let path = entry.path();
                    let bytes = path.as_os_str().len();
                    if visited > DISCOVERY_ENTRY_LIMIT
                        || traversal_bytes.saturating_add(bytes) > DISCOVERY_PATH_BYTE_LIMIT
                    {
                        truncated = true;
                        break 'walk;
                    }
                    traversal_bytes += bytes;
                    pending.push((path, depth + 1));
                }
            }
        } else if path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("vstpreset"))
        {
            let mut header = [0u8; 48];
            if std::fs::File::open(&path)
                .and_then(|mut f| f.read_exact(&mut header))
                .is_ok()
                && &header[..4] == b"VST3"
                && let Ok(class) = std::str::from_utf8(&header[8..40])
            {
                let bytes = path.as_os_str().len().saturating_add(class.len());
                if found.len() >= DISCOVERY_FILE_LIMIT
                    || found_bytes.saturating_add(bytes) > DISCOVERY_PATH_BYTE_LIMIT
                {
                    truncated = true;
                    break;
                }
                found_bytes += bytes;
                found.push((path, class.to_owned()));
            }
        }
    }
    found.sort();
    (found, truncated)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{Scratch, session};
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial_test() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn isolated_job(session: &Session) -> SoundLibraryJob {
        let mut job = session.sound_library_job(&[]);
        // Unit tests exercise the catalog independently of machine-installed native binaries.
        job.scan_plugins = false;
        job
    }

    fn isolated_metadata_job(session: &Session) -> SoundLibraryJob {
        let mut job = session.sound_library_job(&[]);
        job.isolated_plugin_snapshot = Some(IsolatedPluginCatalog {
            entries: vec![IsolatedPluginEntry {
                // This path deliberately does not exist. A successful test therefore proves the
                // live search consumes the supplied descriptor instead of loading the binary.
                path: PathBuf::from("NeverLoad.clap"),
                metadata: crate::PluginProbeResult::Clap(vec![crate::ClapPluginInfo {
                    clap_id: "example.quarantine.synth".into(),
                    name: "Quarantine Synth".into(),
                    vendor: "Example Vendor".into(),
                    description: "safe isolated descriptor".into(),
                    version: "1.0".into(),
                    kind: PluginKind::Instrument,
                    category: PluginCategory::Synth,
                }]),
            }],
            diagnostics: vec![
                "Live Agent native plugins use isolated default descriptors; native preset enumeration and acoustic similarity are unavailable"
                    .into(),
            ],
            status: NativePluginStatus {
                discovered_files: 3,
                inspected_files: 2,
                failed_files: 1,
                skipped_files: 2,
                truncated: true,
            },
        });
        job
    }

    fn pressure_catalog_cache() {
        for _ in 0..6 {
            let other = session();
            drop(isolated_job(&other).catalog(false).unwrap());
        }
    }

    #[test]
    fn isolated_live_search_issues_selectable_metadata_without_loading_native_code() {
        let _serial = serial_test();
        let session = session();
        let cancelled = AtomicBool::new(false);
        let response = isolated_metadata_job(&session)
            .run_isolated(
                SoundSearch::Text {
                    query: "quarantine".into(),
                    limit: 10,
                    offset: 0,
                    filter: SoundFilter::default(),
                },
                false,
                &cancelled,
            )
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let id = response["sounds"][0]["id"].as_str().unwrap();

        assert_eq!(response["native_plugins"]["mode"], "isolated_metadata");
        assert_eq!(response["native_plugins"]["skipped_files"], 2);
        assert_eq!(response["native_plugins"]["truncated"], true);
        assert_eq!(
            response["native_plugins"]["acoustic_similarity"],
            "not_supported"
        );
        let source = resolve_library_sound(&session.sound_library_scope(&[]), id).unwrap();
        assert!(isolated_native(&source));
        assert!(!measurable(&source));
        let diagnostics = session.sound_library_job(&[]).diagnostics(0, 10).unwrap();
        assert!(diagnostics.contains("isolated default descriptors"));
    }

    #[test]
    fn isolated_search_handles_still_drive_the_existing_selection_contract() {
        let _serial = serial_test();
        let mut session = session();
        let track = session
            .add_default_instrument_track("Safe selection")
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let response = isolated_metadata_job(&session)
            .run_isolated(
                SoundSearch::Text {
                    query: "FM 2-Op".into(),
                    limit: 1,
                    offset: 0,
                    filter: SoundFilter {
                        source: Some(SoundSource::Builtin),
                        library: None,
                    },
                },
                false,
                &cancelled,
            )
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let id = response["sounds"][0]["id"].as_str().unwrap();

        session.forget_history();
        session.use_library_sound(track, id, &[]).unwrap();

        assert_eq!(session.undo(), Some(crate::Edit::ChangeInstrument));
    }

    #[test]
    fn isolated_native_sound_explains_why_similarity_is_unavailable() {
        let _serial = serial_test();
        let session = session();
        let cancelled = AtomicBool::new(false);
        let response = isolated_metadata_job(&session)
            .run_isolated(
                SoundSearch::Text {
                    query: "quarantine".into(),
                    limit: 1,
                    offset: 0,
                    filter: SoundFilter::default(),
                },
                false,
                &cancelled,
            )
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let id = response["sounds"][0]["id"].as_str().unwrap().to_string();

        let error = isolated_metadata_job(&session)
            .run_isolated(
                SoundSearch::Similar {
                    id,
                    limit: 10,
                    filter: SoundFilter::default(),
                },
                false,
                &cancelled,
            )
            .unwrap_err();

        assert!(error.contains("isolated metadata"));
        assert!(error.contains("third-party code"));
    }

    #[test]
    fn isolated_live_search_honors_cancellation_before_discovery() {
        let _serial = serial_test();
        let session = session();
        let cancelled = AtomicBool::new(true);

        let error = isolated_metadata_job(&session)
            .run_isolated(
                SoundSearch::Text {
                    query: "quarantine".into(),
                    limit: 1,
                    offset: 0,
                    filter: SoundFilter::default(),
                },
                false,
                &cancelled,
            )
            .unwrap_err();

        assert_eq!(error, "Sound search cancelled");
    }

    #[test]
    fn isolated_native_candidates_are_bounded_without_starving_a_format() {
        let inventory = crate::InstalledPluginFiles {
            clap: (0..300)
                .map(|index| PathBuf::from(format!("Clap-{index:03}.clap")))
                .collect(),
            vst3: vec![PathBuf::from("First.vst3"), PathBuf::from("Second.vst3")],
            truncated: false,
        };

        let candidates = isolated_plugin_candidates(&inventory);

        assert_eq!(candidates.len(), ISOLATED_PLUGIN_FILE_LIMIT);
        assert!(candidates.contains(&(PathBuf::from("First.vst3"), crate::PluginFormat::Vst3)));
        assert!(candidates.contains(&(PathBuf::from("Second.vst3"), crate::PluginFormat::Vst3)));
    }

    fn catalog_is_cached(key: &str) -> bool {
        CATALOGS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .iter()
            .any(|catalog| catalog.key == key)
    }

    #[test]
    fn sound_handles_are_process_scoped_compact_refreshable_and_bounded() {
        let _serial = serial_test();
        let mut handles = IssuedSounds::default();
        let first = handles.generation("current", Some("snapshot-a"), false);
        assert!(first.starts_with("g:"));
        assert_eq!(
            handles.generation("current", Some("snapshot-a"), false),
            first
        );
        assert!(!handles.restore_generation("current", "g:different-token", "snapshot-a"));
        assert!(!handles.restore_generation("current", &first, "snapshot-b"));
        assert_eq!(
            handles.generation("current", Some("snapshot-a"), false),
            first
        );
        let first_id = generated_sound_id(&first, "source digest");
        assert!(first_id.len() <= 23);
        let mut sound = Sound::new(
            "Issued".into(),
            "Auris".into(),
            String::new(),
            Source::Builtin("auris.synth.poly".into()),
        );
        sound.id = first_id.clone();
        let sound = Arc::new(sound);
        assert_eq!(
            handles.issue(
                "current",
                &first,
                vec![IssuedSound {
                    bytes: sound.retained_bytes(),
                    sound,
                }],
            ),
            IssueStatus::Issued
        );
        assert!(handles.resolve("current", &first_id).is_some());

        let refreshed = handles.generation("current", None, true);
        assert_ne!(refreshed, first);
        assert_ne!(generated_sound_id(&refreshed, "source digest"), first_id);
        assert!(handles.resolve("current", &first_id).is_none());
        assert_eq!(
            handles.issue("current", &first, Vec::new()),
            IssueStatus::Stale
        );
        assert_eq!(
            handles.generation("current", Some("snapshot-b"), false),
            refreshed
        );
        let changed = handles.generation("current", Some("snapshot-c"), false);
        assert_ne!(changed, refreshed);
        assert_eq!(
            handles.issue("current", &refreshed, Vec::new()),
            IssueStatus::Stale
        );
        for index in 0..=CATALOG_GENERATION_LIMIT {
            handles.generation(&format!("snapshot-{index}"), Some("same"), false);
        }
        assert_eq!(handles.generations.len(), CATALOG_GENERATION_LIMIT);
        assert_eq!(handles.sound_count, 0);
        assert!(handles.restore_generation("current", &changed, "snapshot-c"));
        assert_eq!(handles.generations.len(), CATALOG_GENERATION_LIMIT);
        assert_eq!(
            handles.generations.back().map(|entry| entry.token.as_str()),
            Some(changed.as_str())
        );

        let bounded = handles.generation("bounded", Some("snapshot"), false);
        let sounds = (0..ISSUED_SOUND_LIMIT)
            .map(|index| {
                let mut sound = Sound::new(
                    "Issued".into(),
                    "Auris".into(),
                    String::new(),
                    Source::Builtin("auris.synth.poly".into()),
                );
                sound.id = format!("s:{index}");
                let sound = Arc::new(sound);
                IssuedSound {
                    bytes: sound.retained_bytes(),
                    sound,
                }
            })
            .collect();
        assert_eq!(
            handles.issue("bounded", &bounded, sounds),
            IssueStatus::Issued
        );
        let mut newest = Sound::new(
            "Newest".into(),
            "Auris".into(),
            String::new(),
            Source::Builtin("auris.synth.poly".into()),
        );
        newest.id = format!("s:{ISSUED_SOUND_LIMIT}");
        let newest = Arc::new(newest);
        assert_eq!(
            handles.issue(
                "bounded",
                &bounded,
                vec![IssuedSound {
                    bytes: newest.retained_bytes(),
                    sound: newest,
                }],
            ),
            IssueStatus::Issued
        );
        assert_eq!(handles.sound_count, ISSUED_SOUND_LIMIT);
        assert!(handles.byte_count <= ISSUED_SOUND_BYTE_LIMIT);
        assert!(handles.resolve("bounded", "s:0").is_none());
        assert!(
            handles
                .resolve("bounded", &format!("s:{ISSUED_SOUND_LIMIT}"))
                .is_some()
        );
        let oversized = Arc::new(Sound::new(
            "Oversized".into(),
            "Provider".into(),
            String::new(),
            Source::Builtin("test".into()),
        ));
        assert_eq!(
            handles.issue(
                "bounded",
                &bounded,
                vec![IssuedSound {
                    sound: oversized,
                    bytes: ISSUED_SOUND_BYTE_LIMIT + 1,
                }],
            ),
            IssueStatus::TooLarge
        );
    }

    #[test]
    fn restored_generation_expires_issued_handles_when_the_fingerprint_changes() {
        let _serial = serial_test();
        let mut handles = IssuedSounds::default();
        let key = "restored scope";
        let generation = handles.generation(key, Some("snapshot-a"), false);
        for index in 0..=CATALOG_GENERATION_LIMIT {
            handles.generation(
                &format!("generation-pressure-{index}"),
                Some("other snapshot"),
                false,
            );
        }
        assert!(handles.generations.iter().all(|entry| entry.key != key));
        assert!(handles.restore_generation(key, &generation, "snapshot-a"));

        let mut sound = Sound::new(
            "Issued before the change".into(),
            "Provider".into(),
            String::new(),
            Source::Builtin("test".into()),
        );
        sound.id = generated_sound_id(&generation, "source digest");
        let id = sound.id.clone();
        let sound = Arc::new(sound);
        assert_eq!(
            handles.issue(
                key,
                &generation,
                vec![IssuedSound {
                    bytes: sound.retained_bytes(),
                    sound,
                }],
            ),
            IssueStatus::Issued
        );

        let changed = handles.generation(key, Some("snapshot-b"), false);
        assert_ne!(changed, generation);
        assert!(handles.resolve(key, &id).is_none());
    }

    #[test]
    fn cached_catalog_restores_its_evicted_generation_for_a_fresh_text_search() {
        let _serial = serial_test();
        let session = session();
        let first = query(&session, "auris", 1, 0, false).unwrap();
        let id = first["sounds"][0]["id"].as_str().unwrap().to_owned();
        let job = isolated_job(&session);
        let key = job.key.clone();
        let catalog = job.catalog(false).unwrap();
        let generation = catalog.generation.clone();
        drop(catalog);

        let prefix = crate::transient_id::transient_id("generation-pressure");
        {
            let mut handles = ISSUED_SOUNDS.get_or_init(Default::default).lock().unwrap();
            for index in 0..=CATALOG_GENERATION_LIMIT {
                handles.generation(
                    &format!("{prefix}-{index}"),
                    Some("isolated snapshot"),
                    false,
                );
            }
            assert!(handles.generations.iter().all(|entry| entry.key != key));
        }
        assert!(catalog_is_cached(&key));
        assert!(issued_sound(&key, &id).is_err());

        let second = query(&session, "auris", 1, 0, false).unwrap();
        assert_eq!(second["sounds"][0]["id"], id);
        assert!(issued_sound(&key, &id).is_ok());
        let handles = ISSUED_SOUNDS.get_or_init(Default::default).lock().unwrap();
        assert!(
            handles
                .generations
                .iter()
                .any(|entry| entry.key == key && entry.token == generation)
        );
    }

    fn empty_catalog(key: &str) -> Arc<Catalog> {
        Arc::new(Catalog {
            key: key.into(),
            fingerprint: format!("fingerprint-{key}"),
            generation: format!("generation-{key}"),
            sounds: Vec::new(),
            errors: Vec::new(),
            index: Mutex::new(IndexState::default()),
            completed: AtomicUsize::new(0),
            control: TimbreMapControl::default(),
            native_status: None,
        })
    }

    #[test]
    fn catalog_cache_reserves_strict_slots_without_cancelling_active_searches() {
        let _serial = serial_test();
        let mut active = Vec::new();
        let mut active_leases = Vec::new();
        let mut active_controls = Vec::new();
        for index in 0..CATALOG_LIMIT {
            let catalog = empty_catalog(&format!("active-{index}"));
            active_controls.push(catalog.control.clone());
            active_leases.push(catalog.clone());
            active.push(catalog);
        }
        assert!(reserve_catalog_slot(&mut active).is_err());
        assert_eq!(active.len(), CATALOG_LIMIT);
        assert!(
            active_controls
                .iter()
                .all(|control| control.check().is_ok())
        );
        drop(active_leases);

        let mut cache_owned = (0..CATALOG_LIMIT)
            .map(|index| empty_catalog(&format!("cached-{index}")))
            .collect::<Vec<_>>();
        let evicted_control = cache_owned[0].control.clone();
        reserve_catalog_slot(&mut cache_owned).unwrap();
        assert_eq!(cache_owned.len(), CATALOG_LIMIT - 1);
        assert!(evicted_control.check().is_err());
        cache_owned.push(empty_catalog("incoming"));
        assert_eq!(cache_owned.len(), CATALOG_LIMIT);
    }

    #[test]
    fn discovery_admission_recovers_after_a_builder_panics() {
        let _serial = serial_test();
        let lock = Arc::new(Mutex::new(()));
        let worker_lock = Arc::clone(&lock);
        let result = std::thread::spawn(move || {
            let _guard = worker_lock.lock().unwrap();
            panic!("poison the test admission mutex");
        })
        .join();
        assert!(result.is_err());
        assert!(lock.is_poisoned());

        drop(try_discovery_lock(&lock).unwrap());
        assert!(!lock.is_poisoned());
    }

    #[test]
    fn acoustic_worker_permits_are_bounded_and_returned_on_drop() {
        let _serial = serial_test();
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        COUNTER.store(0, Ordering::Relaxed);
        let mut permits = (0..ACOUSTIC_WORKER_LIMIT)
            .map(|_| AcousticWorkerPermit::acquire_from(&COUNTER).unwrap())
            .collect::<Vec<_>>();
        assert!(AcousticWorkerPermit::acquire_from(&COUNTER).is_none());
        drop(permits.pop());
        let replacement = AcousticWorkerPermit::acquire_from(&COUNTER).unwrap();
        drop(replacement);
        drop(permits);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn bounded_acoustic_index_resets_for_a_reference_outside_its_snapshot() {
        let _serial = serial_test();
        let mut state = IndexState {
            started: true,
            reference: Some(0),
            result: Some(Ok(AcousticIndex {
                sounds: (0..ACOUSTIC_SOUND_LIMIT).collect(),
                rows: Vec::new(),
                skipped: Vec::new(),
                limited: true,
            })),
        };
        assert!(state.reset_if_reference_missing(ACOUSTIC_SOUND_LIMIT));
        assert!(!state.started);
        assert!(state.reference.is_none());
        assert!(state.result.is_none());

        state.started = true;
        state.reference = Some(4);
        state.result = Some(Err("reference failed".into()));
        assert!(!state.reset_if_reference_missing(4));
        assert!(state.reset_if_reference_missing(5));
        assert!(state.result.is_none());
    }

    #[test]
    fn catalog_metadata_and_diagnostics_have_hard_storage_limits() {
        let _serial = serial_test();
        let mut budget = CatalogBudget {
            sound_bytes: CATALOG_SOUND_BYTE_LIMIT,
            ..CatalogBudget::default()
        };
        let mut sounds = Vec::new();
        assert!(!budget.push_sound(
            &mut sounds,
            Sound::new(
                "Beyond budget".into(),
                "Provider".into(),
                String::new(),
                Source::Builtin("test".into()),
            )
        ));
        assert!(sounds.is_empty());
        assert!(budget.sounds_truncated);

        let mut errors = Vec::new();
        for index in 0..=CATALOG_ERROR_LIMIT {
            budget.push_error(
                &mut errors,
                format!(
                    "{index}: {}",
                    "x".repeat(CATALOG_ERROR_CHARACTER_LIMIT + 20)
                ),
            );
        }
        assert_eq!(errors.len(), CATALOG_ERROR_LIMIT);
        assert!(
            errors
                .iter()
                .take(CATALOG_ERROR_LIMIT - 1)
                .all(|error| error.chars().count() <= CATALOG_ERROR_CHARACTER_LIMIT + 1)
        );
        assert_eq!(
            errors.last().unwrap(),
            "Additional sound discovery diagnostics were omitted"
        );

        let mut deduplicating = CatalogBudget::default();
        let mut unique_sounds = Vec::new();
        let duplicate = Sound::new(
            "Duplicate".into(),
            "Provider".into(),
            String::new(),
            Source::Builtin("duplicate".into()),
        );
        for _ in 0..=CATALOG_SOUND_LIMIT {
            assert!(deduplicating.push_sound(&mut unique_sounds, duplicate.clone()));
        }
        assert!(deduplicating.push_sound(
            &mut unique_sounds,
            Sound::new(
                "Unique".into(),
                "Provider".into(),
                String::new(),
                Source::Builtin("unique".into()),
            )
        ));
        assert_eq!(unique_sounds.len(), 2);
        assert!(!deduplicating.sounds_truncated);
    }

    #[test]
    fn bounded_plugin_walk_treats_vst3_bundles_as_atomic_entries() {
        let _serial = serial_test();
        let scratch = Scratch::new("bounded-plugin-walk");
        let bundle = scratch.join("Outer.vst3");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(bundle.join("Contents").join("Nested.vst3"), b"not a plugin").unwrap();
        let mut files = PluginFiles::default();
        let mut visited = 0;
        let mut bytes = 0;
        scan_plugin_roots(
            [bundle.clone()],
            "vst3",
            true,
            &mut files,
            &mut visited,
            &mut bytes,
        );
        assert_eq!(files.entries, vec![(bundle, true)]);
        assert!(!files.truncated);
    }

    #[test]
    fn selection_stamps_detect_changes_to_external_clap_presets() {
        let _serial = serial_test();
        let scratch = Scratch::new("clap-selection-stamp");
        let plugin = scratch.join("Plugin.clap");
        let preset_path = scratch.join("Preset.bin");
        std::fs::write(&plugin, b"plugin").unwrap();
        std::fs::write(&preset_path, b"preset").unwrap();
        let preset = auris_clap::ClapPreset {
            name: "Preset".into(),
            plugin_ids: vec!["plugin.id".into()],
            location: Some(preset_path.to_string_lossy().into_owned()),
            load_key: None,
            features: Vec::new(),
        };
        let before = clap_selection_stamp(&plugin, Some(&preset));
        std::fs::write(&preset_path, b"changed preset contents").unwrap();
        assert_ne!(before, clap_selection_stamp(&plugin, Some(&preset)));
    }

    #[test]
    fn source_filters_precede_paging_and_metadata_is_shared() {
        let _serial = serial_test();
        let scratch = Scratch::new("filtered-sounds");
        let mut session = session();
        session.save_as(&scratch.join("Filter.auris")).unwrap();
        let result = isolated_job(&session)
            .run(
                SoundSearch::Text {
                    query: "auris".into(),
                    limit: 2,
                    offset: 0,
                    filter: SoundFilter {
                        source: Some(SoundSource::Builtin),
                        library: Some("AURIS".into()),
                    },
                },
                false,
            )
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            value["libraries"],
            serde_json::json!([{"name":"Auris","source":"builtin"}])
        );
        for sound in value["sounds"].as_array().unwrap() {
            assert!(sound["id"].as_str().unwrap().len() < 24);
            assert_eq!(sound["library"], 0);
            assert!(sound.get("source").is_none());
        }
        assert!(value.get("scan_errors").is_none());
        assert!(value.get("usage").is_none());
        let empty = isolated_job(&session)
            .run(
                SoundSearch::Text {
                    query: "auris".into(),
                    limit: 1,
                    offset: 0,
                    filter: SoundFilter {
                        source: Some(SoundSource::Clap),
                        library: None,
                    },
                },
                false,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&empty).unwrap()["total"],
            0
        );
        // Diagnostics intentionally inspect an existing heavy snapshot rather than rebuilding it.
        // Keep that snapshot leased so this assertion remains valid while cache-pressure tests run
        // in parallel.
        let catalog = isolated_job(&session).catalog(false).unwrap();
        let diagnostic = isolated_job(&session).diagnostics(0, 2).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&diagnostic).unwrap()["total"],
            0
        );
        assert!(isolated_job(&session).diagnostics(0, 17).is_err());
        drop(catalog);
    }
    #[test]
    fn handles_belong_to_one_document_and_paths_and_aliases_share_a_snapshot() {
        let _serial = serial_test();
        let mut first = session();
        let mut other = session();
        let track = other.add_default_instrument_track("Other").unwrap();
        let result = query(&first, "auris", 1, 0, false).unwrap();
        let id = result["sounds"][0]["id"].as_str().unwrap();
        assert!(other.use_library_sound(track, id, &[]).is_err());
        first.new_project();
        let track = first.project().tracks[0].id;
        assert!(first.use_library_sound(track, id, &[]).is_err());
        let scratch = Scratch::new("sound-path-alias");
        let saved = first.save_as(&scratch.join("Alias.auris")).unwrap();
        let key = first.sound_library_job(&[]).key;
        let mut canonical = session();
        canonical
            .open(&saved.document.canonicalize().unwrap())
            .unwrap();
        assert_eq!(key, canonical.sound_library_job(&[]).key);
        let plugin_a = scratch.join("plugins").join("..").join("vendor");
        let plugin_b = scratch.join("vendor");
        let plugin_c = scratch.join("other");
        assert_eq!(
            first
                .sound_library_job(&[plugin_a.clone(), plugin_c.clone()])
                .key,
            first.sound_library_job(&[plugin_c, plugin_a.clone()]).key
        );
        assert_ne!(
            first.sound_library_job(&[plugin_a]).key,
            first.sound_library_job(&[plugin_b]).key
        );
        assert_ne!(key, first.sound_library_job(&[scratch.join("vendor")]).key);
    }
    fn query(
        session: &Session,
        query: &str,
        limit: usize,
        offset: usize,
        refresh: bool,
    ) -> Result<serde_json::Value, String> {
        let text = isolated_job(session).run(
            SoundSearch::Text {
                filter: SoundFilter::default(),
                query: query.into(),
                limit,
                offset,
            },
            refresh,
        )?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }
    #[test]
    fn bounded_search_selects_the_exact_sound_and_refresh_expires_old_ids() {
        let _serial = serial_test();
        let scratch = Scratch::new("sound-search");
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.save_as(&scratch.join("Search.auris")).unwrap();
        let before = session.project().clone();
        let first = query(&session, "AURIS", 1, 0, false).unwrap();
        assert_eq!(first["sounds"].as_array().unwrap().len(), 1);
        assert_eq!(first["next_offset"], 1);
        let second = query(&session, "auris", 1, 1, false).unwrap();
        assert_ne!(first["sounds"][0]["id"], second["sounds"][0]["id"]);
        assert_eq!(&before, session.project());
        assert!(
            query(&session, " ", 10, 0, false)
                .unwrap_err()
                .contains("query")
        );
        assert!(
            query(&session, "auris", 51, 0, false)
                .unwrap_err()
                .contains("limit")
        );
        assert_eq!(
            query(&session, "no_such_sound_xyz", 10, 0, false).unwrap()["total"],
            0
        );
        let id = second["sounds"][0]["id"].as_str().unwrap();
        let key = session.sound_library_job(&[]).key;
        session.forget_history();
        pressure_catalog_cache();
        assert!(!catalog_is_cached(&key));
        session.use_library_sound(track, id, &[]).unwrap();
        // Selection resolves the lightweight issued handle; it must not rebuild the provider
        // catalog on this caller thread.
        assert!(!catalog_is_cached(&key));
        assert_eq!(session.undo(), Some(crate::Edit::ChangeInstrument));
        assert_eq!(&before, session.project());
        query(&session, "auris", 1, 0, true).unwrap();
        assert!(
            session
                .use_library_sound(track, id, &[])
                .unwrap_err()
                .contains("expired")
        );
        assert_eq!(&before, session.project());
    }

    #[test]
    fn acoustic_neighbors_use_all_features_exclude_self_and_respect_limit() {
        let _serial = serial_test();
        let scratch = Scratch::new("sound-neighbors");
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.save_as(&scratch.join("Neighbors.auris")).unwrap();
        let job = isolated_job(&session);
        let key = job.key.clone();
        let catalog = job.catalog(false).unwrap();
        let indices = catalog
            .sounds
            .iter()
            .enumerate()
            .filter(|(_, s)| measurable(s))
            .take(3)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        assert_eq!(indices.len(), 3);
        let reference = catalog.sounds[indices[0]].id.clone();
        issue_sounds(&key, &catalog.generation, [&catalog.sounds[indices[0]]]).unwrap();
        assert!(
            isolated_job(&session)
                .run(
                    SoundSearch::Similar {
                        filter: SoundFilter::default(),
                        id: catalog.sounds[indices[1]].id.clone(),
                        limit: 1,
                    },
                    false,
                )
                .unwrap_err()
                .contains("expired")
        );
        catalog.index.lock().unwrap().result = Some(Ok(AcousticIndex {
            sounds: indices.clone(),
            rows: vec![
                vec![0.0, 0.0, 0.0],
                vec![0.0, 0.0, 9.0],
                vec![1.0, 0.0, 0.0],
            ],
            skipped: Vec::new(),
            limited: false,
        }));
        let response = job
            .run(
                SoundSearch::Similar {
                    filter: SoundFilter::default(),
                    id: reference.clone(),
                    limit: 1,
                },
                false,
            )
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["sounds"].as_array().unwrap().len(), 1);
        assert_eq!(response["sounds"][0]["id"], catalog.sounds[indices[2]].id);
        assert_eq!(response["sounds"][0]["distance"], 1.0);
        assert_ne!(response["sounds"][0]["id"], reference);
        let neighbor = response["sounds"][0]["id"].as_str().unwrap();
        drop(catalog);
        pressure_catalog_cache();
        assert!(!catalog_is_cached(&key));
        session.use_library_sound(track, neighbor, &[]).unwrap();
        assert!(!catalog_is_cached(&key));
        query(&session, "auris", 1, 0, true).unwrap();
        assert!(session.use_library_sound(track, neighbor, &[]).is_err());
    }

    #[test]
    fn acoustic_probe_is_finite_read_only_and_cancellable() {
        let _serial = serial_test();
        let session = session();
        let before = session.project().clone();
        let job = isolated_job(&session);
        let source = job.sounds.iter().find(|s| measurable(s)).unwrap();
        let control = TimbreMapControl::default();
        let row = measure_sound(source, &job.registry, job.sample_rate, &control).unwrap();
        assert!(!row.is_empty() && row.iter().all(|v| v.is_finite()));
        control.cancel();
        assert!(
            measure_sound(source, &job.registry, job.sample_rate, &control)
                .unwrap_err()
                .contains("cancelled")
        );
        assert_eq!(&before, session.project());
    }

    #[test]
    fn background_index_finishes_and_reuses_measured_neighbors() {
        let _serial = serial_test();
        let scratch = Scratch::new("sound-index-background");
        let mut session = session();
        session.save_as(&scratch.join("Background.auris")).unwrap();
        let job = isolated_job(&session);
        let catalog = job.catalog(false).unwrap();
        let reference = catalog.sounds.iter().find(|s| measurable(s)).unwrap();
        issue_sounds(&job.key, &catalog.generation, [reference]).unwrap();
        let reference = reference.id.clone();
        let request = SoundSearch::Similar {
            filter: SoundFilter::default(),
            id: reference,
            limit: 2,
        };
        let first: serde_json::Value =
            serde_json::from_str(&job.run(request.clone(), false).unwrap()).unwrap();
        assert_eq!(first["status"], "indexing");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while catalog.index.lock().unwrap().result.is_none() {
            assert!(std::time::Instant::now() < deadline, "index did not finish");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Other sessions may fill the process cache while this caller waits. The live catalog is
        // leased by `catalog`; cache pressure must not replace its opaque IDs underneath the
        // promised same-ID retry.
        pressure_catalog_cache();
        let result = isolated_job(&session).run(request.clone(), false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "ready");
        assert_eq!(parsed["sounds"].as_array().unwrap().len(), 2);
        assert_eq!(result, isolated_job(&session).run(request, false).unwrap());
    }

    #[test]
    #[ignore = "Requires installed Surge XT and executes third-party preset code"]
    fn installed_surge_presets_produce_distinct_acoustic_features() {
        let _serial = serial_test();
        let scratch = Scratch::new("surge-preset-probe");
        let mut session = session();
        session.save_as(&scratch.join("Native.auris")).unwrap();
        let job = session.sound_library_job(&[]);
        let catalog = job.catalog(false).unwrap();
        let sources = catalog
            .sounds
            .iter()
            .filter(|s| {
                s.library.contains("Surge XT")
                    && matches!(
                        &s.source,
                        Source::Clap {
                            preset: Some(_),
                            ..
                        }
                    )
            })
            .take(2)
            .collect::<Vec<_>>();
        assert_eq!(sources.len(), 2, "Surge preset discovery was unavailable");
        let rows = sources
            .iter()
            .map(|sound| {
                measure_sound(sound, &job.registry, job.sample_rate, &catalog.control).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(rows[0].len(), rows[1].len());
        assert!(rows.iter().flatten().all(|v| v.is_finite()));
        assert!(
            rows[0]
                .iter()
                .zip(&rows[1])
                .any(|(a, b)| (a - b).abs() > 1e-6)
        );
        eprintln!(
            "Measured {} and {}: {} features each",
            sources[0].name,
            sources[1].name,
            rows[0].len()
        );
    }
}
