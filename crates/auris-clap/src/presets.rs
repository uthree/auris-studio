//! Metadata-only discovery through CLAP providers; opaque load keys are never interpreted.

use clack_extensions::preset_discovery::prelude::*;
use clack_host::prelude::*;
use clack_host::utils::{Timestamp, UniversalPluginId};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

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
    /// Unsupported providers, unreadable locations, or malformed metadata.
    pub errors: Vec<String>,
}

#[derive(Default)]
struct Indexer {
    locations: Vec<Option<String>>,
    extensions: Vec<String>,
}
impl IndexerImpl for Indexer {
    fn declare_filetype(&mut self, ty: FileType) -> Result<(), HostError> {
        self.extensions.push(
            ty.file_extension
                .unwrap_or(c"")
                .to_string_lossy()
                .into_owned(),
        );
        Ok(())
    }
    fn declare_location(&mut self, info: LocationInfo) -> Result<(), HostError> {
        self.locations.push(match info.location {
            Location::Plugin => None,
            Location::File { path } => Some(path.to_string_lossy().into_owned()),
        });
        Ok(())
    }
    fn declare_soundpack(&mut self, _: Soundpack) -> Result<(), HostError> {
        Ok(())
    }
}

struct Receiver<'a> {
    result: &'a mut PresetDiscovery,
    location: Option<String>,
}
impl MetadataReceiverImpl for Receiver<'_> {
    fn on_error(&mut self, code: i32, message: Option<&CStr>) {
        self.result.errors.push(format!(
            "Preset provider error {code}: {}",
            message.unwrap_or(c"").to_string_lossy()
        ));
    }
    fn begin_preset(&mut self, name: Option<&CStr>, key: Option<&CStr>) -> Result<(), HostError> {
        self.result.presets.push(ClapPreset {
            name: name
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    self.location
                        .as_deref()
                        .and_then(|p| Path::new(p).file_stem())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
            plugin_ids: Vec::new(),
            location: self.location.clone(),
            load_key: key.map(|s| s.to_string_lossy().into_owned()),
            features: Vec::new(),
        });
        Ok(())
    }
    fn add_plugin_id(&mut self, id: UniversalPluginId) {
        if id.abi == c"clap"
            && let Some(preset) = self.result.presets.last_mut()
        {
            preset.plugin_ids.push(id.id.to_string_lossy().into_owned());
        }
    }
    fn add_feature(&mut self, feature: &CStr) {
        if let Some(preset) = self.result.presets.last_mut() {
            preset.features.push(feature.to_string_lossy().into_owned());
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
    let Some(factory) = entry.get_factory::<PresetDiscoveryFactory>() else {
        result
            .errors
            .push("Plugin has no CLAP preset discovery provider".into());
        return result;
    };
    for descriptor in factory.provider_descriptors() {
        let Some(id) = descriptor.id() else { continue };
        let mut provider =
            match Provider::instantiate(Indexer::default(), entry, id, &crate::host::host_info()) {
                Ok(provider) => provider,
                Err(error) => {
                    result.errors.push(error.to_string());
                    continue;
                }
            };
        let mut locations = Vec::new();
        for location in &provider.indexer().locations {
            match location {
                None => locations.push(None),
                Some(path) => {
                    let mut files = Vec::new();
                    collect_files(
                        Path::new(path),
                        &provider.indexer().extensions,
                        &mut files,
                        &mut result.errors,
                    );
                    locations.extend(
                        files
                            .into_iter()
                            .filter_map(|p| p.to_str().map(|s| Some(s.to_owned()))),
                    );
                }
            }
        }
        locations.sort();
        locations.dedup();
        for location in locations {
            let path = location.as_deref().map(CString::new).transpose();
            let Ok(path) = path else {
                result.errors.push("Preset location contains NUL".into());
                continue;
            };
            let mut receiver = Receiver {
                result: &mut result,
                location,
            };
            provider.get_metadata(
                path.as_deref()
                    .map_or(Location::Plugin, |path| Location::File { path }),
                &mut receiver,
            );
        }
    }
    result
        .presets
        .retain(|p| !p.name.is_empty() && !p.plugin_ids.is_empty());
    result
}

fn collect_files(
    path: &Path,
    extensions: &[String],
    files: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
) {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        match path.read_dir() {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => collect_files(&entry.path(), extensions, files, errors),
                        Err(error) => errors.push(error.to_string()),
                    }
                }
            }
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    } else if metadata.is_file()
        && (extensions.is_empty()
            || extensions.iter().any(|e| {
                e.is_empty() || path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e))
            }))
    {
        files.push(path.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut receiver = Receiver {
            result: &mut result,
            location: Some("/presets/bank.container".into()),
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
}
