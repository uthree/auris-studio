//! Bounded text discovery and reusable acoustic search over exact library preset addresses.

use super::{Session, TimbreMapControl, TimbreSound};
use crate::prelude::*;
use auris_core::plugin::{Instrument, PluginState, PrepareContext};
use auris_dsp::timbre::{standardize_timbres, timbre_features};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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
    },
    /// Other sounds ordered by full standardized acoustic feature distance.
    Similar {
        /// Exact sound ID returned by search_instruments for this project/session.
        id: String,
        /// Maximum neighbors, 1..50; defaults to 10. Excludes the reference itself.
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 50))]
        limit: usize,
    },
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
    },
    Vst3 {
        file: PathBuf,
        id: String,
        preset: Option<auris_vst3::Vst3Preset>,
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
}

struct AcousticIndex {
    sounds: Vec<usize>,
    rows: Vec<Vec<f64>>,
    skipped: Vec<String>,
}
#[derive(Default)]
struct IndexState {
    started: bool,
    result: Option<Result<AcousticIndex, String>>,
}
struct Catalog {
    key: String,
    sounds: Vec<Sound>,
    errors: Vec<String>,
    index: Mutex<IndexState>,
    completed: AtomicUsize,
    control: TimbreMapControl,
}
static CATALOGS: OnceLock<Mutex<Vec<Arc<Catalog>>>> = OnceLock::new();
static CATALOG_BUILD: Mutex<()> = Mutex::new(());
static CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Immutable snapshot; run discovery on an ordinary worker, never the UI or audio thread.
pub struct SoundLibraryJob {
    key: String,
    registry: Arc<PluginRegistry>,
    sounds: Vec<Sound>,
    plugins: Vec<(PathBuf, bool)>,
    sample_rate: f64,
}

impl Session {
    /// Snapshots loaded libraries and installed plugin paths without loading plugin code.
    pub fn sound_library_job(&self, extra_paths: &[PathBuf]) -> SoundLibraryJob {
        let fonts = auris_sampler::SoundFontBank::shared();
        let mut sounds = self
            .registry()
            .instruments()
            .filter(|d| d.id != SAMPLER_ID)
            .map(|d| {
                Sound::new(
                    d.name.to_string(),
                    "Auris".into(),
                    d.id.to_string(),
                    Source::Builtin(d.id.to_string()),
                )
            })
            .collect::<Vec<_>>();
        let mut stamps = Vec::new();
        for reference in self.soundfonts() {
            if let Some(font) = self.fonts.get(reference.id) {
                fonts.insert(reference.id, font);
                if let Some(path) = reference.path.resolve(self.project_folder()) {
                    stamps.push(stamp(&path));
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
                    sounds.push(Sound::new(
                        preset.name,
                        reference.name.clone(),
                        tags,
                        Source::Font(PresetRef {
                            font: reference.id,
                            bank: preset.bank,
                            patch: preset.patch,
                        }),
                    ));
                }
            }
        }
        let mut plugins = self
            .installed_clap_files(extra_paths)
            .into_iter()
            .map(|p| (p, false))
            .chain(
                self.installed_vst3_files(extra_paths)
                    .into_iter()
                    .map(|p| (p, true)),
            )
            .collect::<Vec<_>>();
        plugins.sort();
        for (path, _) in &plugins {
            stamps.push(stamp(path));
        }
        for path in vst_preset_roots() {
            stamps.push(stamp(&path));
        }
        let key = format!(
            "{:x}",
            Sha256::digest(format!(
                "{:?}{:?}{:?}{}",
                self.path(),
                stamps,
                sounds,
                self.sample_rate()
            ))
        );
        SoundLibraryJob {
            key,
            registry: crate::default_registry(fonts),
            sounds,
            plugins,
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
        let job = self.sound_library_job(paths);
        let catalogs = CATALOGS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Sound catalog lock failed")?;
        let source = catalogs.iter().find(|c| c.key == job.key).and_then(|c| c.sounds.iter().find(|s| s.id == id))
            .cloned().ok_or("Unknown or expired sound ID; call search_instruments for this project/session again")?;
        drop(catalogs);
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
            Source::Clap { file, id, preset } => {
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
                self.collect_hosted_state();
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
            Source::Vst3 { file, id, preset } => {
                let plugin = auris_vst3::Vst3Plugin::load(&file, &id, &prepare)
                    .map_err(|e| e.to_string())?;
                if let Some(preset) = preset {
                    plugin.load_preset(&preset).map_err(|e| e.to_string())?;
                }
                let bytes = plugin.save_state().map_err(|e| e.to_string())?;
                self.collect_hosted_state();
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
    /// Returns a bounded search response. First acoustic use starts a background index and
    /// returns `status: indexing`; subsequent calls report progress or completed neighbors.
    pub fn run(self, request: SoundSearch, refresh: bool) -> Result<String, String> {
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
        let catalog = self.catalog(refresh)?;
        match request {
            SoundSearch::Text { query, offset, .. } => {
                let words = query
                    .split_whitespace()
                    .map(str::to_lowercase)
                    .collect::<Vec<_>>();
                let matches = catalog
                    .sounds
                    .iter()
                    .filter(|s| s.matches(&words))
                    .collect::<Vec<_>>();
                if offset > matches.len() {
                    return Err("offset exceeds matching sound count".into());
                }
                let page = matches
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|s| s.value())
                    .collect::<Vec<_>>();
                let next = offset + page.len();
                Ok(serde_json::json!({"status":"ready","sounds":page,"total":matches.len(),"next_offset":(next<matches.len()).then_some(next),
                    "scan_error_count":catalog.errors.len(),"scan_errors":short_errors(&catalog.errors),
                    "usage":"Copy id into set_instrument (live) or sound_id in add_track/set_instrument (MCP). Use similar_instruments for acoustic alternatives."}).to_string())
            }
            SoundSearch::Similar { id, .. } => {
                let reference = catalog
                    .sounds
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or("Unknown sound ID; search_instruments first")?;
                if !measurable(&catalog.sounds[reference]) {
                    return Err("Drum kits are searchable but are not comparable with the melodic timbre reference".into());
                }
                let mut state = catalog
                    .index
                    .lock()
                    .map_err(|_| "Acoustic index lock failed")?;
                if let Some(result) = &state.result {
                    let index = result.as_ref().map_err(Clone::clone)?;
                    let row = index.sounds.iter().position(|i| *i == reference).ok_or("The reference could not be measured (silent or failed plugin); choose another sound")?;
                    let mut nearest = index
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != row)
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
                    let sounds = nearest
                        .into_iter()
                        .take(limit)
                        .map(|(i, d)| {
                            let mut v = catalog.sounds[index.sounds[i]].value();
                            v["distance"] = d.into();
                            v
                        })
                        .collect::<Vec<_>>();
                    return Ok(serde_json::json!({"status":"ready","reference":id,"sounds":sounds,"indexed":index.sounds.len(),
                        "skipped_count":index.skipped.len(),"skipped":short_errors(&index.skipped),
                        "metric":"Euclidean distance in the full standardized timbre feature space; lower is closer. Not a subjective quality score."}).to_string());
                }
                if !state.started {
                    let worker_catalog = catalog.clone();
                    std::thread::Builder::new()
                        .name("auris-sound-index".into())
                        .spawn(move || {
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    self.measure(&worker_catalog)
                                }))
                                .unwrap_or_else(|_| Err("Sound index worker panicked".into()));
                            if let Ok(mut state) = worker_catalog.index.lock() {
                                state.result = Some(result);
                            }
                        })
                        .map_err(|e| e.to_string())?;
                    state.started = true;
                }
                Ok(serde_json::json!({"status":"indexing","completed":catalog.completed.load(Ordering::Relaxed),"total":catalog.sounds.iter().filter(|s| measurable(s)).count(),"retry_after_seconds":5,
                    "usage":"Acoustic analysis runs in the background once per library snapshot. Continue other work and retry similar_instruments later with the same id; do not refresh while indexing."}).to_string())
            }
        }
    }

    fn catalog(&self, refresh: bool) -> Result<Arc<Catalog>, String> {
        // Discovery can invoke native providers. Serialize builds without holding the cache
        // read lock needed by live selection, and keep refresh ordered with in-flight builds.
        let _build = CATALOG_BUILD
            .lock()
            .map_err(|_| "Sound discovery lock failed")?;
        let cache = CATALOGS.get_or_init(Default::default);
        {
            let mut entries = cache.lock().map_err(|_| "Sound catalog lock failed")?;
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
            if let Some(catalog) = entries.iter().find(|c| c.key == self.key) {
                return Ok(catalog.clone());
            }
        }
        let mut sounds = self.sounds.clone();
        let mut errors = Vec::new();
        let preset_files = vst_preset_files();
        for (file, vst3) in &self.plugins {
            let result = if *vst3 {
                discover_vst3(
                    file,
                    &preset_files,
                    self.sample_rate,
                    &mut sounds,
                    &mut errors,
                )
            } else {
                discover_clap(file, &mut sounds, &mut errors)
            };
            if let Err(error) = result {
                errors.push(format!("{}: {error}", file.display()));
            }
        }
        sounds.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        let mut seen = std::collections::HashSet::new();
        sounds.retain(|s| seen.insert(s.id.clone()));
        let generation = CATALOG_GENERATION.fetch_add(1, Ordering::Relaxed);
        for sound in &mut sounds {
            sound.id = format!(
                "sound:{:x}",
                Sha256::digest(format!("{}:{generation}:{}", self.key, sound.id))
            );
        }
        let catalog = Arc::new(Catalog {
            key: self.key.clone(),
            sounds,
            errors,
            index: Mutex::new(IndexState::default()),
            completed: AtomicUsize::new(0),
            control: TimbreMapControl::default(),
        });
        let mut entries = cache.lock().map_err(|_| "Sound catalog lock failed")?;
        if let Some(existing) = entries.iter().find(|c| c.key == self.key) {
            return Ok(existing.clone());
        }
        if entries.len() >= 4 {
            entries.remove(0).control.cancel();
        }
        entries.push(catalog.clone());
        Ok(catalog)
    }

    fn measure(&self, catalog: &Catalog) -> Result<AcousticIndex, String> {
        let mut sounds = Vec::new();
        let mut rows = Vec::new();
        let mut skipped = Vec::new();
        for (index, sound) in catalog
            .sounds
            .iter()
            .enumerate()
            .filter(|(_, s)| measurable(s))
        {
            match measure_sound(sound, &self.registry, self.sample_rate, &catalog.control) {
                Ok(row) => {
                    sounds.push(index);
                    rows.push(row);
                }
                Err(error) => skipped.push(format!("{}: {error}", sound.name)),
            }
            catalog.completed.fetch_add(1, Ordering::Relaxed);
            catalog.control.check().map_err(|e| e.to_string())?;
        }
        Ok(AcousticIndex {
            sounds,
            rows: standardize_timbres(&rows).map_err(str::to_string)?,
            skipped,
        })
    }
}

fn measurable(sound: &Sound) -> bool {
    match &sound.source {
        Source::Font(p) => p.bank != 128,
        Source::Builtin(id) => {
            !matches!(id.as_str(), "auris.synth.drumkit" | "auris.synth.noisedrum")
        }
        _ => true,
    }
}

fn short_text(text: &str, limit: usize) -> String {
    let mut characters = text.chars();
    let mut result = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        result.push('…');
    }
    result
}
fn short_errors(errors: &[String]) -> Vec<String> {
    errors.iter().take(3).map(|e| short_text(e, 240)).collect()
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
                Source::Clap { file, id, preset } => {
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
                Source::Vst3 { file, id, preset } => {
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
) -> Result<(), String> {
    // SAFETY: discovered in the user's configured plugin library.
    let library = unsafe { auris_clap::ClapLibrary::load(file) }.map_err(|e| e.to_string())?;
    let plugins = library
        .plugins()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|i| i.kind == PluginKind::Instrument)
        .collect::<Vec<_>>();
    if plugins.is_empty() {
        return Ok(());
    }
    let metadata = library.presets();
    errors.extend(
        metadata
            .errors
            .into_iter()
            .map(|e| format!("{}: {e}", file.display())),
    );
    for info in plugins {
        let label = format!("{} / {}", info.vendor, info.name);
        sounds.push(Sound::new(
            format!("{} (default)", info.name),
            label.clone(),
            String::new(),
            Source::Clap {
                file: file.into(),
                id: info.clap_id.clone(),
                preset: None,
            },
        ));
        match library.instantiate(&info.clap_id) {
            Ok(plugin) if plugin.supports_preset_loading() => {}
            Ok(_) => {
                errors.push(format!("{}: no CLAP preset-load extension", info.name));
                continue;
            }
            Err(error) => {
                errors.push(format!("{}: {error}", info.name));
                continue;
            }
        }
        for preset in metadata
            .presets
            .iter()
            .filter(|p| p.plugin_ids.contains(&info.clap_id))
        {
            sounds.push(Sound::new(
                preset.name.clone(),
                label.clone(),
                preset.features.join(" "),
                Source::Clap {
                    file: file.into(),
                    id: info.clap_id.clone(),
                    preset: Some(preset.clone()),
                },
            ));
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
) -> Result<(), String> {
    for info in auris_vst3::plugins_in(file)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|i| i.kind == PluginKind::Instrument)
    {
        let label = format!("{} / {}", info.vendor, info.name);
        sounds.push(Sound::new(
            format!("{} (default)", info.name),
            label.clone(),
            String::new(),
            Source::Vst3 {
                file: file.into(),
                id: info.class_id.clone(),
                preset: None,
            },
        ));
        let mut presets = match auris_vst3::Vst3Plugin::load(
            file,
            &info.class_id,
            &PrepareContext::new(rate, 512, 2),
        )
        .and_then(|p| p.presets())
        {
            Ok(presets) => presets,
            Err(error) => {
                errors.push(format!("{}: {error}", info.name));
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
        if presets.is_empty() {
            errors.push(format!(
                "{}: no advertised VST3 programs or standard preset files",
                info.name
            ));
        }
        for (name, preset) in presets {
            sounds.push(Sound::new(
                name,
                label.clone(),
                String::new(),
                Source::Vst3 {
                    file: file.into(),
                    id: info.class_id.clone(),
                    preset: Some(preset),
                },
            ));
        }
    }
    Ok(())
}

fn stamp(path: &Path) -> String {
    let metadata = path.metadata().ok();
    format!(
        "{}:{:?}:{:?}",
        path.display(),
        metadata.as_ref().map(|m| m.len()),
        metadata.and_then(|m| m.modified().ok())
    )
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
fn vst_preset_files() -> Vec<(PathBuf, String)> {
    use std::io::Read;
    let mut pending = vst_preset_roots();
    let mut found = Vec::new();
    while let Some(path) = pending.pop() {
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if let Ok(entries) = path.read_dir() {
                pending.extend(entries.filter_map(Result::ok).map(|e| e.path()));
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
                found.push((path, class.to_owned()));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{Scratch, session};
    use super::*;

    fn isolated_job(session: &Session) -> SoundLibraryJob {
        let mut job = session.sound_library_job(&[]);
        // Unit tests exercise the catalog independently of machine-installed native binaries.
        job.plugins.clear();
        job
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
        session.forget_history();
        session.use_library_sound(track, id, &[]).unwrap();
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
        let scratch = Scratch::new("sound-neighbors");
        let mut session = session();
        session.save_as(&scratch.join("Neighbors.auris")).unwrap();
        let job = isolated_job(&session);
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
        catalog.index.lock().unwrap().result = Some(Ok(AcousticIndex {
            sounds: indices.clone(),
            rows: vec![
                vec![0.0, 0.0, 0.0],
                vec![0.0, 0.0, 9.0],
                vec![1.0, 0.0, 0.0],
            ],
            skipped: Vec::new(),
        }));
        let response = job
            .run(
                SoundSearch::Similar {
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
    }

    #[test]
    fn acoustic_probe_is_finite_read_only_and_cancellable() {
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
        let scratch = Scratch::new("sound-index-background");
        let mut session = session();
        session.save_as(&scratch.join("Background.auris")).unwrap();
        let job = isolated_job(&session);
        let catalog = job.catalog(false).unwrap();
        let reference = catalog
            .sounds
            .iter()
            .find(|s| measurable(s))
            .unwrap()
            .id
            .clone();
        let request = SoundSearch::Similar {
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
        let result = isolated_job(&session).run(request.clone(), false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "ready");
        assert_eq!(parsed["sounds"].as_array().unwrap().len(), 2);
        assert_eq!(result, isolated_job(&session).run(request, false).unwrap());
    }

    #[test]
    #[ignore = "Requires installed Surge XT and executes third-party preset code"]
    fn installed_surge_presets_produce_distinct_acoustic_features() {
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
