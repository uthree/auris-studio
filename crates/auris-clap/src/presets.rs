//! Metadata-only discovery through CLAP providers; opaque load keys are never interpreted.

use clack_extensions::preset_discovery::prelude::*;
use clack_host::prelude::*;
use clack_host::utils::{Timestamp, UniversalPluginId};
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

const PROVIDER_LIMIT: usize = 256;
const DECLARED_LOCATION_LIMIT: usize = 1_024;
const FILE_TYPE_LIMIT: usize = 256;
const INDEXER_BYTE_LIMIT: usize = 1024 * 1024;
const LOCATION_LIMIT: usize = 16_384;
const LOCATION_BYTE_LIMIT: usize = 16 * 1024 * 1024;
const TRAVERSAL_ENTRY_LIMIT: usize = 65_536;
const TRAVERSAL_PATH_BYTE_LIMIT: usize = 16 * 1024 * 1024;
const TRAVERSAL_DEPTH_LIMIT: usize = 64;
const SINGLE_PATH_BYTE_LIMIT: usize = 32 * 1024;
const PRESET_LIMIT: usize = 16_384;
const PRESET_METADATA_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const METADATA_TEXT_BYTE_LIMIT: usize = 16 * 1024;
const ERROR_LIMIT: usize = 1_024;
const ERROR_BYTE_LIMIT: usize = 1024 * 1024;
const ERROR_TEXT_BYTE_LIMIT: usize = 512;
const TRUNCATION_DIAGNOSTIC: &str =
    "CLAP preset discovery reached its bounded metadata or traversal limit";

#[derive(Clone, Copy)]
struct DiscoveryLimits {
    providers: usize,
    declared_locations: usize,
    file_types: usize,
    indexer_bytes: usize,
    locations: usize,
    location_bytes: usize,
    traversal_entries: usize,
    traversal_path_bytes: usize,
    traversal_depth: usize,
    single_path_bytes: usize,
    presets: usize,
    preset_metadata_bytes: usize,
    metadata_text_bytes: usize,
    errors: usize,
    error_bytes: usize,
    error_text_bytes: usize,
}

const DISCOVERY_LIMITS: DiscoveryLimits = DiscoveryLimits {
    providers: PROVIDER_LIMIT,
    declared_locations: DECLARED_LOCATION_LIMIT,
    file_types: FILE_TYPE_LIMIT,
    indexer_bytes: INDEXER_BYTE_LIMIT,
    locations: LOCATION_LIMIT,
    location_bytes: LOCATION_BYTE_LIMIT,
    traversal_entries: TRAVERSAL_ENTRY_LIMIT,
    traversal_path_bytes: TRAVERSAL_PATH_BYTE_LIMIT,
    traversal_depth: TRAVERSAL_DEPTH_LIMIT,
    single_path_bytes: SINGLE_PATH_BYTE_LIMIT,
    presets: PRESET_LIMIT,
    preset_metadata_bytes: PRESET_METADATA_BYTE_LIMIT,
    metadata_text_bytes: METADATA_TEXT_BYTE_LIMIT,
    errors: ERROR_LIMIT,
    error_bytes: ERROR_BYTE_LIMIT,
    error_text_bytes: ERROR_TEXT_BYTE_LIMIT,
};

/// An exact address advertised by a CLAP preset provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapPreset {
    /// Display name.
    pub name: String,
    /// CLAP IDs explicitly advertised by the provider.
    pub plugin_ids: Vec<String>,
    /// UTF-8 filesystem location, or `None` for plugin-internal storage.
    pub location: Option<String>,
    /// Opaque key inside a preset container.
    pub load_key: Option<String>,
    /// Searchable category metadata supplied by the provider.
    pub features: Vec<String>,
}

/// Discovered presets and recoverable provider errors.
#[derive(Default)]
pub struct PresetDiscovery {
    /// Exact selectable preset addresses.
    pub presets: Vec<ClapPreset>,
    /// Unsupported providers, unreadable locations, malformed metadata, or a reached safety limit.
    pub errors: Vec<String>,
}

struct Indexer {
    locations: Vec<Option<String>>,
    extensions: Vec<String>,
    file_types_declared: bool,
    bytes: usize,
    truncated: bool,
    limits: DiscoveryLimits,
}

impl Indexer {
    fn new(limits: DiscoveryLimits) -> Self {
        Self {
            locations: Vec::new(),
            extensions: Vec::new(),
            file_types_declared: false,
            bytes: 0,
            truncated: false,
            limits,
        }
    }

    fn reserve(&mut self, bytes: usize) -> bool {
        if self.bytes.saturating_add(bytes) > self.limits.indexer_bytes {
            self.truncated = true;
            false
        } else {
            self.bytes += bytes;
            true
        }
    }

    fn push_extension(&mut self, extension: &CStr) {
        self.file_types_declared = true;
        if self.extensions.len() >= self.limits.file_types {
            self.truncated = true;
            return;
        }
        let Some(extension) = bounded_cstr(extension, self.limits.metadata_text_bytes) else {
            self.truncated = true;
            return;
        };
        let bytes = std::mem::size_of::<String>().saturating_add(extension.len());
        if self.reserve(bytes) {
            self.extensions.push(extension);
        }
    }

    fn push_location(&mut self, location: Option<&CStr>) {
        if self.locations.len() >= self.limits.declared_locations {
            self.truncated = true;
            return;
        }
        let location = match location {
            Some(path) => {
                let Some(path) = bounded_cstr(path, self.limits.single_path_bytes) else {
                    self.truncated = true;
                    return;
                };
                Some(path)
            }
            None => None,
        };
        let bytes = std::mem::size_of::<Option<String>>()
            .saturating_add(location.as_ref().map_or(0, String::len));
        if self.reserve(bytes) {
            self.locations.push(location);
        }
    }
}
impl IndexerImpl for Indexer {
    fn declare_filetype(&mut self, ty: FileType) -> Result<(), HostError> {
        self.push_extension(ty.file_extension.unwrap_or(c""));
        Ok(())
    }
    fn declare_location(&mut self, info: LocationInfo) -> Result<(), HostError> {
        self.push_location(match info.location {
            Location::Plugin => None,
            Location::File { path } => Some(path),
        });
        Ok(())
    }
    fn declare_soundpack(&mut self, _: Soundpack) -> Result<(), HostError> {
        Ok(())
    }
}

struct DiscoveryBudget {
    limits: DiscoveryLimits,
    location_count: usize,
    location_bytes: usize,
    traversal_entries: usize,
    traversal_path_bytes: usize,
    preset_bytes: usize,
    error_bytes: usize,
    seen_directories: HashSet<u64>,
    truncated: bool,
    traversal_full: bool,
}

impl DiscoveryBudget {
    fn new(limits: DiscoveryLimits) -> Self {
        Self {
            limits,
            location_count: 0,
            location_bytes: 0,
            traversal_entries: 0,
            traversal_path_bytes: 0,
            preset_bytes: 0,
            error_bytes: 0,
            seen_directories: HashSet::new(),
            truncated: false,
            traversal_full: false,
        }
    }

    fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    fn begin_provider(&mut self) {
        self.seen_directories.clear();
    }

    fn reserve_traversal_path(&mut self, path: &Path) -> bool {
        let bytes = path.as_os_str().len();
        if self.traversal_entries >= self.limits.traversal_entries
            || bytes > self.limits.single_path_bytes
            || self.traversal_path_bytes.saturating_add(bytes) > self.limits.traversal_path_bytes
        {
            self.truncated = true;
            self.traversal_full = true;
            false
        } else {
            self.traversal_entries += 1;
            self.traversal_path_bytes += bytes;
            true
        }
    }

    fn push_location(&mut self, locations: &mut Vec<Option<String>>, path: &Path) -> bool {
        if self.location_count >= self.limits.locations {
            self.truncated = true;
            return false;
        }
        let Some(path) = bounded_path(path, self.limits.single_path_bytes) else {
            self.truncated = true;
            return false;
        };
        let bytes = std::mem::size_of::<Option<String>>().saturating_add(path.len());
        if self.location_bytes.saturating_add(bytes) > self.limits.location_bytes {
            self.truncated = true;
            return false;
        }
        self.location_count += 1;
        self.location_bytes += bytes;
        locations.push(Some(path));
        true
    }

    fn push_plugin_location(&mut self, locations: &mut Vec<Option<String>>) {
        if self.location_count >= self.limits.locations
            || self
                .location_bytes
                .saturating_add(std::mem::size_of::<Option<String>>())
                > self.limits.location_bytes
        {
            self.truncated = true;
            return;
        }
        self.location_count += 1;
        self.location_bytes += std::mem::size_of::<Option<String>>();
        locations.push(None);
    }

    fn reserve_preset(&mut self, presets: &[ClapPreset], bytes: usize) -> bool {
        if presets.len() >= self.limits.presets
            || self.preset_bytes.saturating_add(bytes) > self.limits.preset_metadata_bytes
        {
            self.truncated = true;
            false
        } else {
            self.preset_bytes += bytes;
            true
        }
    }

    fn reserve_preset_field(&mut self, bytes: usize) -> bool {
        if self.preset_bytes.saturating_add(bytes) > self.limits.preset_metadata_bytes {
            self.truncated = true;
            false
        } else {
            self.preset_bytes += bytes;
            true
        }
    }

    fn presets_full(&self, presets: &[ClapPreset]) -> bool {
        presets.len() >= self.limits.presets
            || self.preset_bytes >= self.limits.preset_metadata_bytes
    }

    fn errors_full(&self, errors: &[String]) -> bool {
        errors.len() >= self.limits.errors || self.error_bytes >= self.limits.error_bytes
    }

    fn push_error(&mut self, errors: &mut Vec<String>, error: &str) {
        if self.errors_full(errors) {
            self.truncated = true;
            return;
        }
        let error = truncate_text(error, self.limits.error_text_bytes);
        let bytes = std::mem::size_of::<String>().saturating_add(error.len());
        if errors.len() >= self.limits.errors
            || self.error_bytes.saturating_add(bytes) > self.limits.error_bytes
        {
            self.truncated = true;
            return;
        }
        self.error_bytes += bytes;
        errors.push(error);
    }

    fn finish(&mut self, errors: &mut Vec<String>) {
        if !self.truncated {
            return;
        }
        let diagnostic = truncate_text(TRUNCATION_DIAGNOSTIC, self.limits.error_text_bytes);
        let bytes = std::mem::size_of::<String>().saturating_add(diagnostic.len());
        if errors.len() < self.limits.errors
            && self.error_bytes.saturating_add(bytes) <= self.limits.error_bytes
        {
            self.error_bytes += bytes;
            errors.push(diagnostic);
            return;
        }
        if let Some(last) = errors.last_mut() {
            let without_last = self
                .error_bytes
                .saturating_sub(std::mem::size_of::<String>().saturating_add(last.len()));
            if without_last.saturating_add(bytes) <= self.limits.error_bytes {
                self.error_bytes = without_last + bytes;
                *last = diagnostic;
            }
        }
    }
}

fn bounded_cstr(value: &CStr, limit: usize) -> Option<String> {
    if value.to_bytes().len() > limit {
        return None;
    }
    let value = value.to_string_lossy();
    (value.len() <= limit).then(|| value.into_owned())
}

fn bounded_path(value: &Path, limit: usize) -> Option<String> {
    if value.as_os_str().len() > limit {
        return None;
    }
    value
        .to_str()
        .filter(|value| value.len() <= limit)
        .map(str::to_owned)
}

fn truncate_text(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    if limit < 3 {
        return ".".repeat(limit);
    }
    let mut end = limit - 3;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

fn bounded_error_message(value: &CStr, limit: usize) -> (String, bool) {
    let bytes = value.to_bytes();
    let prefix = &bytes[..bytes.len().min(limit)];
    let text = String::from_utf8_lossy(prefix);
    let truncated = bytes.len() > prefix.len() || text.len() > limit;
    (truncate_text(&text, limit), truncated)
}

fn directory_identity(path: &Path) -> u64 {
    let identity = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = DefaultHasher::new();
    identity.hash(&mut hasher);
    hasher.finish()
}

fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = metadata;
        false
    }
}

struct Receiver<'a> {
    result: &'a mut PresetDiscovery,
    budget: &'a mut DiscoveryBudget,
    location: Option<String>,
    current_preset: Option<usize>,
}
impl MetadataReceiverImpl for Receiver<'_> {
    fn on_error(&mut self, code: i32, message: Option<&CStr>) {
        if self.budget.errors_full(&self.result.errors) {
            self.budget.mark_truncated();
            return;
        }
        let (message, truncated) = bounded_error_message(
            message.unwrap_or(c""),
            self.budget.limits.error_text_bytes.saturating_sub(40),
        );
        if truncated {
            self.budget.mark_truncated();
        }
        self.budget.push_error(
            &mut self.result.errors,
            &format!("Preset provider error {code}: {message}"),
        );
    }
    fn begin_preset(&mut self, name: Option<&CStr>, key: Option<&CStr>) -> Result<(), HostError> {
        self.current_preset = None;
        if self.budget.presets_full(&self.result.presets) {
            self.budget.mark_truncated();
            return Ok(());
        }
        let name = match name {
            Some(name) => bounded_cstr(name, self.budget.limits.metadata_text_bytes),
            None => match self.location.as_deref() {
                Some(path) => Path::new(path).file_stem().and_then(|name| {
                    let name = name.to_string_lossy();
                    (name.len() <= self.budget.limits.metadata_text_bytes)
                        .then(|| name.into_owned())
                }),
                None => Some(String::new()),
            },
        };
        let Some(name) = name else {
            self.budget.mark_truncated();
            return Ok(());
        };
        let load_key = match key {
            Some(key) => {
                let Some(key) = bounded_cstr(key, self.budget.limits.metadata_text_bytes) else {
                    self.budget.mark_truncated();
                    return Ok(());
                };
                Some(key)
            }
            None => None,
        };
        let bytes = std::mem::size_of::<ClapPreset>()
            .saturating_add(name.len())
            .saturating_add(self.location.as_ref().map_or(0, String::len))
            .saturating_add(load_key.as_ref().map_or(0, String::len));
        if !self.budget.reserve_preset(&self.result.presets, bytes) {
            return Ok(());
        }
        self.result.presets.push(ClapPreset {
            name,
            plugin_ids: Vec::new(),
            location: self.location.clone(),
            load_key,
            features: Vec::new(),
        });
        self.current_preset = Some(self.result.presets.len() - 1);
        Ok(())
    }
    fn add_plugin_id(&mut self, id: UniversalPluginId) {
        if id.abi != c"clap" {
            return;
        }
        let Some(index) = self.current_preset else {
            return;
        };
        if self.budget.preset_bytes >= self.budget.limits.preset_metadata_bytes {
            self.budget.mark_truncated();
            return;
        }
        let Some(id) = bounded_cstr(id.id, self.budget.limits.metadata_text_bytes) else {
            self.budget.mark_truncated();
            return;
        };
        let bytes = std::mem::size_of::<String>().saturating_add(id.len());
        if self.budget.reserve_preset_field(bytes) {
            self.result.presets[index].plugin_ids.push(id);
        }
    }
    fn add_feature(&mut self, feature: &CStr) {
        let Some(index) = self.current_preset else {
            return;
        };
        if self.budget.preset_bytes >= self.budget.limits.preset_metadata_bytes {
            self.budget.mark_truncated();
            return;
        }
        let Some(feature) = bounded_cstr(feature, self.budget.limits.metadata_text_bytes) else {
            self.budget.mark_truncated();
            return;
        };
        let bytes = std::mem::size_of::<String>().saturating_add(feature.len());
        if self.budget.reserve_preset_field(bytes) {
            self.result.presets[index].features.push(feature);
        }
    }
    fn set_soundpack_id(&mut self, _: &CStr) {}
    fn set_flags(&mut self, _: Flags) {}
    fn add_creator(&mut self, _: &CStr) {}
    fn set_description(&mut self, description: &CStr) {
        self.add_feature(description);
    }
    fn set_timestamps(&mut self, _: Option<Timestamp>, _: Option<Timestamp>) {}
    fn add_extra_info(&mut self, _: &CStr, _: &CStr) {}
}

pub(crate) fn discover(entry: &PluginEntry) -> PresetDiscovery {
    let mut result = PresetDiscovery::default();
    let mut budget = DiscoveryBudget::new(DISCOVERY_LIMITS);
    let Some(factory) = entry.get_factory::<PresetDiscoveryFactory>() else {
        budget.push_error(
            &mut result.errors,
            "Plugin has no CLAP preset discovery provider",
        );
        return result;
    };
    for (provider_index, descriptor) in factory.provider_descriptors().enumerate() {
        if provider_index >= budget.limits.providers {
            budget.mark_truncated();
            break;
        }
        let Some(id) = descriptor.id() else { continue };
        let mut provider = match Provider::instantiate(
            Indexer::new(budget.limits),
            entry,
            id,
            &crate::host::host_info(),
        ) {
            Ok(provider) => provider,
            Err(error) => {
                budget.push_error(&mut result.errors, &error.to_string());
                continue;
            }
        };
        if provider.indexer().truncated {
            budget.mark_truncated();
        }
        let mut declared_locations = provider.indexer().locations.clone();
        declared_locations.sort();
        declared_locations.dedup();
        let file_types_were_rejected =
            provider.indexer().file_types_declared && provider.indexer().extensions.is_empty();
        budget.begin_provider();
        let mut locations = Vec::new();
        for location in &declared_locations {
            match location {
                None => budget.push_plugin_location(&mut locations),
                Some(path) => {
                    if file_types_were_rejected {
                        budget.mark_truncated();
                        continue;
                    }
                    collect_files(
                        Path::new(path),
                        &provider.indexer().extensions,
                        &mut locations,
                        &mut result,
                        &mut budget,
                    );
                }
            }
            if budget.traversal_full {
                break;
            }
        }
        locations.sort();
        locations.dedup();
        for location in locations {
            let path = location.as_deref().map(CString::new).transpose();
            let Ok(path) = path else {
                budget.push_error(&mut result.errors, "Preset location contains NUL");
                continue;
            };
            let mut receiver = Receiver {
                result: &mut result,
                budget: &mut budget,
                location,
                current_preset: None,
            };
            provider.get_metadata(
                path.as_deref()
                    .map_or(Location::Plugin, |path| Location::File { path }),
                &mut receiver,
            );
        }
        if result.presets.len() >= budget.limits.presets {
            budget.mark_truncated();
        }
        if budget.traversal_full || result.presets.len() >= budget.limits.presets {
            break;
        }
    }
    result
        .presets
        .retain(|p| !p.name.is_empty() && !p.plugin_ids.is_empty());
    for preset in &mut result.presets {
        preset.plugin_ids.shrink_to_fit();
        preset.features.shrink_to_fit();
    }
    result.presets.shrink_to_fit();
    budget.finish(&mut result.errors);
    result.errors.shrink_to_fit();
    result
}

fn collect_files(
    root: &Path,
    extensions: &[String],
    locations: &mut Vec<Option<String>>,
    result: &mut PresetDiscovery,
    budget: &mut DiscoveryBudget,
) {
    if budget.traversal_full || !budget.reserve_traversal_path(root) {
        return;
    }
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((path, depth)) = pending.pop() {
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                budget.push_error(&mut result.errors, &format!("{}: {error}", path.display()));
                continue;
            }
        };
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            if depth >= budget.limits.traversal_depth {
                budget.mark_truncated();
                continue;
            }
            if !budget.seen_directories.insert(directory_identity(&path)) {
                continue;
            }
            match path.read_dir() {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) => {
                                let path = entry.path();
                                if !budget.reserve_traversal_path(&path) {
                                    pending.clear();
                                    break;
                                }
                                pending.push((path, depth + 1));
                            }
                            Err(error) => {
                                budget.push_error(&mut result.errors, &error.to_string());
                            }
                        }
                    }
                }
                Err(error) => {
                    budget.push_error(&mut result.errors, &format!("{}: {error}", path.display()))
                }
            }
        } else if metadata.is_file()
            && (extensions.is_empty()
                || extensions.iter().any(|extension| {
                    extension.is_empty()
                        || path
                            .extension()
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(extension))
                }))
            && !budget.push_location(locations, &path)
        {
            budget.traversal_full = true;
            pending.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_limits() -> DiscoveryLimits {
        DiscoveryLimits {
            providers: 2,
            declared_locations: 2,
            file_types: 2,
            indexer_bytes: 256,
            locations: 2,
            location_bytes: 512,
            traversal_entries: 8,
            traversal_path_bytes: 2_048,
            traversal_depth: 2,
            single_path_bytes: 512,
            presets: 2,
            preset_metadata_bytes: 1_024,
            metadata_text_bytes: 64,
            errors: 2,
            error_bytes: 1_024,
            error_text_bytes: 128,
        }
    }

    fn walk_with_limits(
        root: &Path,
        limits: DiscoveryLimits,
    ) -> (Vec<Option<String>>, PresetDiscovery, DiscoveryBudget) {
        let mut result = PresetDiscovery::default();
        let mut locations = Vec::new();
        let mut budget = DiscoveryBudget::new(limits);
        collect_files(
            root,
            &["preset".into()],
            &mut locations,
            &mut result,
            &mut budget,
        );
        budget.finish(&mut result.errors);
        (locations, result, budget)
    }

    #[test]
    fn a_loaded_native_preset_survives_a_state_round_trip() {
        let library = crate::testkit::instrument_library();
        let id = library.plugins().unwrap()[0].clap_id.clone();
        let preset = ClapPreset {
            name: "Quiet".into(),
            plugin_ids: vec![id.clone()],
            location: None,
            load_key: Some("quiet".into()),
            features: Vec::new(),
        };
        let mut plugin = library.instantiate(&id).unwrap();
        plugin.load_preset(&preset).unwrap();
        let state = plugin.save_state().unwrap();
        assert_eq!(state, 0.125f32.to_le_bytes());
        let mut restored = library.instantiate(&id).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(restored.save_state().unwrap(), state);
        let invalid = ClapPreset {
            load_key: Some("missing".into()),
            ..preset
        };
        assert!(plugin.load_preset(&invalid).is_err());
        assert_eq!(plugin.save_state().unwrap(), state);
    }
    #[test]
    fn metadata_preserves_container_keys_plugin_ids_and_tags() {
        let mut result = PresetDiscovery::default();
        let mut budget = DiscoveryBudget::new(DISCOVERY_LIMITS);
        let mut receiver = Receiver {
            result: &mut result,
            budget: &mut budget,
            location: Some("/presets/bank.container".into()),
            current_preset: None,
        };
        receiver
            .begin_preset(Some(c"Bright Lead"), Some(c"opaque/key:42"))
            .unwrap();
        receiver.add_plugin_id(UniversalPluginId {
            abi: c"clap",
            id: c"vendor.synth",
        });
        receiver.add_plugin_id(UniversalPluginId {
            abi: c"vst3",
            id: c"ignored",
        });
        receiver.add_feature(c"Synth Lead");
        receiver
            .begin_preset(Some(c"Warm Pad"), Some(c"another-key"))
            .unwrap();
        receiver.add_plugin_id(UniversalPluginId {
            abi: c"clap",
            id: c"vendor.other",
        });
        assert_eq!(result.presets[0].load_key.as_deref(), Some("opaque/key:42"));
        assert_eq!(result.presets[0].plugin_ids, vec!["vendor.synth"]);
        assert_eq!(result.presets[0].features, vec!["Synth Lead"]);
        assert_eq!(result.presets[1].plugin_ids, vec!["vendor.other"]);
        assert_eq!(
            result.presets[1].location.as_deref(),
            Some("/presets/bank.container")
        );
    }

    #[test]
    fn rejected_preset_callbacks_never_append_to_the_previous_preset() {
        let mut limits = tiny_limits();
        limits.presets = 1;
        let mut result = PresetDiscovery::default();
        let mut budget = DiscoveryBudget::new(limits);
        {
            let mut receiver = Receiver {
                result: &mut result,
                budget: &mut budget,
                location: None,
                current_preset: None,
            };
            receiver.begin_preset(Some(c"First"), None).unwrap();
            receiver.add_plugin_id(UniversalPluginId {
                abi: c"clap",
                id: c"first.id",
            });
            receiver.begin_preset(Some(c"Rejected"), None).unwrap();
            receiver.add_plugin_id(UniversalPluginId {
                abi: c"clap",
                id: c"must.not.leak",
            });
            receiver.add_feature(c"must not leak");
        }
        budget.finish(&mut result.errors);
        assert_eq!(result.presets.len(), 1);
        assert_eq!(result.presets[0].plugin_ids, ["first.id"]);
        assert!(result.presets[0].features.is_empty());
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);
    }

    #[test]
    fn indexer_and_diagnostics_obey_count_and_byte_limits() {
        let mut limits = tiny_limits();
        limits.declared_locations = 1;
        limits.file_types = 1;
        limits.errors = 2;
        limits.error_text_bytes = 48;
        let mut indexer = Indexer::new(limits);
        indexer.push_location(Some(c"/one"));
        indexer.push_location(Some(c"/two"));
        indexer.push_extension(c"preset");
        indexer.push_extension(c"ignored");
        assert_eq!(indexer.locations, [Some("/one".into())]);
        assert_eq!(indexer.extensions, ["preset"]);
        assert!(indexer.truncated);

        let mut text_limited = limits;
        text_limited.metadata_text_bytes = 3;
        let mut indexer = Indexer::new(text_limited);
        indexer.push_extension(c"preset");
        assert!(indexer.file_types_declared);
        assert!(indexer.extensions.is_empty());
        assert!(indexer.truncated);

        let mut byte_limited = limits;
        byte_limited.indexer_bytes = std::mem::size_of::<Option<String>>();
        let mut indexer = Indexer::new(byte_limited);
        indexer.push_location(Some(c"/does-not-fit"));
        assert!(indexer.locations.is_empty());
        assert!(indexer.truncated);

        let mut result = PresetDiscovery::default();
        let mut budget = DiscoveryBudget::new(limits);
        for _ in 0..4 {
            budget.push_error(&mut result.errors, &"x".repeat(200));
        }
        budget.finish(&mut result.errors);
        assert_eq!(result.errors.len(), limits.errors);
        assert!(
            result
                .errors
                .iter()
                .all(|error| error.len() <= limits.error_text_bytes)
        );
        assert!(result.errors.last().unwrap().contains("CLAP preset"));
    }

    #[test]
    fn file_walk_stops_at_each_depth_entry_path_and_location_budget() {
        let root = tempfile::tempdir().unwrap();
        let level_one = root.path().join("one");
        let level_two = level_one.join("two");
        let level_three = level_two.join("three");
        std::fs::create_dir_all(&level_three).unwrap();
        for (folder, name) in [
            (root.path(), "root.preset"),
            (&level_one, "one.preset"),
            (&level_two, "two.preset"),
            (&level_three, "three.preset"),
        ] {
            std::fs::write(folder.join(name), b"preset").unwrap();
        }

        let mut depth_limits = tiny_limits();
        depth_limits.locations = 32;
        depth_limits.traversal_entries = 32;
        depth_limits.traversal_path_bytes = 16 * 1024;
        depth_limits.traversal_depth = 0;
        let (locations, result, budget) = walk_with_limits(root.path(), depth_limits);
        assert!(locations.is_empty());
        assert!(budget.truncated);
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);

        let mut entry_limits = depth_limits;
        entry_limits.traversal_depth = 8;
        entry_limits.traversal_entries = 1;
        let (locations, result, budget) = walk_with_limits(root.path(), entry_limits);
        assert!(locations.is_empty());
        assert_eq!(budget.traversal_entries, entry_limits.traversal_entries);
        assert!(budget.truncated);
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);

        let mut path_limits = depth_limits;
        path_limits.traversal_depth = 8;
        path_limits.traversal_path_bytes = root.path().as_os_str().len();
        let (locations, result, budget) = walk_with_limits(root.path(), path_limits);
        assert!(locations.is_empty());
        assert!(budget.traversal_path_bytes <= path_limits.traversal_path_bytes);
        assert!(budget.truncated);
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);

        let mut location_limits = depth_limits;
        location_limits.traversal_depth = 8;
        location_limits.locations = 1;
        let (locations, result, budget) = walk_with_limits(root.path(), location_limits);
        assert_eq!(locations.len(), location_limits.locations);
        assert!(budget.truncated);
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);

        let mut location_byte_limits = depth_limits;
        location_byte_limits.traversal_depth = 8;
        location_byte_limits.location_bytes = 1;
        let (locations, result, budget) = walk_with_limits(root.path(), location_byte_limits);
        assert!(locations.is_empty());
        assert!(budget.truncated);
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);
    }

    #[test]
    fn file_walk_does_not_follow_directory_links_or_reparse_points() {
        let root = tempfile::tempdir().unwrap();
        let presets = root.path().join("presets");
        std::fs::create_dir(&presets).unwrap();
        std::fs::write(presets.join("inside.preset"), b"preset").unwrap();
        let alias = presets.join("loop");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&presets, &alias).unwrap();
        #[cfg(target_os = "windows")]
        if let Err(error) = std::os::windows::fs::symlink_dir(&presets, &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                return;
            }
            panic!("could not create directory link: {error}");
        }

        let (locations, _, budget) = walk_with_limits(&presets, DISCOVERY_LIMITS);
        assert_eq!(locations.len(), 1);
        assert!(!budget.truncated);
    }

    #[test]
    fn directory_cycle_state_does_not_hide_files_from_later_providers() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("first.alpha"), b"preset").unwrap();
        std::fs::write(root.path().join("second.beta"), b"preset").unwrap();

        let mut result = PresetDiscovery::default();
        let mut locations = Vec::new();
        let mut budget = DiscoveryBudget::new(DISCOVERY_LIMITS);
        budget.begin_provider();
        collect_files(
            root.path(),
            &["alpha".into()],
            &mut locations,
            &mut result,
            &mut budget,
        );
        budget.begin_provider();
        collect_files(
            root.path(),
            &["beta".into()],
            &mut locations,
            &mut result,
            &mut budget,
        );

        locations.sort();
        assert_eq!(locations.len(), 2);
        assert!(locations[0].as_deref().unwrap().ends_with("first.alpha"));
        assert!(locations[1].as_deref().unwrap().ends_with("second.beta"));
        assert!(!budget.truncated);
    }

    #[test]
    fn preset_metadata_bytes_are_bounded() {
        let mut limits = tiny_limits();
        limits.preset_metadata_bytes = std::mem::size_of::<ClapPreset>() + 8;
        let mut result = PresetDiscovery::default();
        let mut budget = DiscoveryBudget::new(limits);
        {
            let mut receiver = Receiver {
                result: &mut result,
                budget: &mut budget,
                location: None,
                current_preset: None,
            };
            receiver
                .begin_preset(Some(c"metadata exceeds eight bytes"), None)
                .unwrap();
            receiver.add_plugin_id(UniversalPluginId {
                abi: c"clap",
                id: c"ignored",
            });
        }
        budget.finish(&mut result.errors);
        assert!(result.presets.is_empty());
        assert_eq!(result.errors.last().unwrap(), TRUNCATION_DIAGNOSTIC);
    }
}
