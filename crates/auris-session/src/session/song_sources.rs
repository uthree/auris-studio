//! Exact library choices for a song's parts, prepared before a document is built around them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_clap::ClapPlugin;
use auris_core::plugin::{PluginKind, PrepareContext};
use auris_core::{AssetPath, PresetRef, Project, TrackId};
use auris_io::{SoundFont, byte_size, font_name, load_soundfont, presets};
use auris_sampler::{SAMPLER_ID, store_preset};
use auris_vst3::Vst3Plugin;
use serde::{Deserialize, Serialize};

use crate::SessionError;
use crate::session::Session;

/// The exact preset or external instrument a part asks the session to resolve.
///
/// This is an asset description, not a plugin instance: a song request carries it unchanged until
/// the session resolves it against the library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartSource {
    /// One preset in a particular SoundFont file.
    SoundFont {
        /// The SoundFont file, resolved by the session.
        path: PathBuf,
        /// The bank number in the SoundFont header, from 0 to 65535.
        bank: i32,
        /// The preset number in the SoundFont header, from 0 to 65535.
        patch: i32,
    },
    /// One instrument descriptor exported by a CLAP plugin file.
    Clap {
        /// The CLAP plugin file.
        path: PathBuf,
        /// The descriptor's stable plugin identifier.
        plugin_id: String,
    },
    /// One instrument class exported by a VST3 module.
    Vst3 {
        /// The VST3 module or bundle.
        path: PathBuf,
        /// The class's stable identifier.
        class_id: String,
    },
}

pub(in crate::session) enum PreparedSource {
    SoundFont {
        path: PathBuf,
        reference: AssetPath,
        samples: Arc<SoundFont>,
        bank: i32,
        patch: i32,
    },
    Clap {
        path: PathBuf,
        plugin_id: String,
        plugin: ClapPlugin,
    },
    Vst3 {
        path: PathBuf,
        class_id: String,
        plugin: Vst3Plugin,
    },
}

impl PreparedSource {
    pub(in crate::session) fn instrument_id(&self) -> String {
        match self {
            Self::SoundFont { .. } => SAMPLER_ID.into(),
            Self::Clap { plugin_id, .. } => format!("{}{plugin_id}", auris_clap::ID_PREFIX),
            Self::Vst3 { class_id, .. } => format!("{}{class_id}", auris_vst3::ID_PREFIX),
        }
    }
}

impl Session {
    /// Resolves a song source against the current project folder without loading or editing it.
    ///
    /// A chooser uses this to compare a collected relative source with an absolute library path.
    /// This does not validate that the file exists or that a plugin can run; composition performs
    /// those checks before changing the document.
    pub fn resolve_song_source(&self, source: &PartSource) -> Result<PartSource, SessionError> {
        Ok(match source {
            PartSource::SoundFont { path, bank, patch } => PartSource::SoundFont {
                path: self.resolve_song_plugin_path(path)?,
                bank: *bank,
                patch: *patch,
            },
            PartSource::Clap { path, plugin_id } => PartSource::Clap {
                path: self.resolve_song_plugin_path(path)?,
                plugin_id: plugin_id.clone(),
            },
            PartSource::Vst3 { path, class_id } => PartSource::Vst3 {
                path: self.resolve_song_plugin_path(path)?,
                class_id: class_id.clone(),
            },
        })
    }

    /// Captures a library preset as a portable song choice, without changing the document.
    ///
    /// The project-local font id is resolved now: a composed document allocates its own ids, so
    /// copying the library id into a song would select an unrelated font after that replacement.
    pub fn song_source_for_preset(&self, preset: PresetRef) -> Result<PartSource, SessionError> {
        let reference = self
            .project
            .soundfonts
            .get(&preset.font)
            .ok_or(SessionError::UnknownSoundFont(preset.font.0))?;
        let path = reference
            .path
            .resolve(self.project_folder())
            .ok_or_else(|| {
                SessionError::SongSource(format!("cannot resolve {}", reference.path))
            })?;
        let samples = self.fonts.get(preset.font).ok_or_else(|| {
            SessionError::SongSource(format!("SoundFont {} is unavailable", path.display()))
        })?;
        require_preset(&samples, &path, preset.bank, preset.patch)?;
        Ok(PartSource::SoundFont {
            path,
            bank: preset.bank,
            patch: preset.patch,
        })
    }

    pub(in crate::session) fn prepare_composed_source(
        &mut self,
        source: Option<&PartSource>,
    ) -> Result<Option<PreparedSource>, SessionError> {
        let Some(source) = source else {
            return Ok(None);
        };
        let prepare = PrepareContext::new(
            self.engine.sample_rate(),
            self.engine.max_block(),
            auris_engine::RENDER_CHANNELS,
        );
        let prepared = match source {
            PartSource::SoundFont { path, bank, patch } => {
                require_local_relative_path(path)?;
                let reference = if path.is_absolute() {
                    AssetPath::external(path)
                } else {
                    AssetPath::inside(path)
                };
                let path = reference.resolve(self.project_folder()).ok_or_else(|| {
                    SessionError::SongSource(format!("cannot resolve {reference}"))
                })?;
                let reference = self.composed_font_reference(&path);
                // Even a cached selection must still exist: silently keeping a missing source
                // would make this composition unrepeatable in the next application session.
                if !path.is_file() {
                    return Err(SessionError::SongSource(format!(
                        "SoundFont {} is unavailable",
                        path.display()
                    )));
                }
                let samples = match self.font_cache.get(&path) {
                    Some(cached) => Arc::clone(&cached.samples),
                    None => load_soundfont(&path)?,
                };
                require_preset(&samples, &path, *bank, *patch)?;
                self.cache_font(&path, Arc::clone(&samples), false);
                PreparedSource::SoundFont {
                    path,
                    reference,
                    samples,
                    bank: *bank,
                    patch: *patch,
                }
            }
            PartSource::Clap { path, plugin_id } => {
                let path = self.resolve_song_plugin_path(path)?;
                // A CLAP is a bundle directory on macOS and a regular file elsewhere.
                if !path.exists() {
                    return Err(SessionError::SongSource(format!(
                        "CLAP file {} is unavailable",
                        path.display()
                    )));
                }
                // A specification is text a model or another person can write, and loading a
                // plugin runs it: the same plugin-folder rule a document's plugins obey.
                if !self.plugin_file_allowed(&path) {
                    return Err(SessionError::UntrustedPlugin(path));
                }
                let known = self
                    .hosted_plugins_in(&path)?
                    .into_iter()
                    .find(|known| {
                        known.clap_id == *plugin_id && known.kind == PluginKind::Instrument
                    })
                    .ok_or_else(|| {
                        SessionError::SongSource(format!(
                            "{} has no instrument named {plugin_id}",
                            path.display()
                        ))
                    })?;
                let plugin =
                    self.hosted
                        .prepare_composed_instrument(&path, &known.clap_id, &prepare)?;
                PreparedSource::Clap {
                    path,
                    plugin_id: known.clap_id,
                    plugin,
                }
            }
            PartSource::Vst3 { path, class_id } => {
                let path = self.resolve_song_plugin_path(path)?;
                if !path.exists() {
                    return Err(SessionError::SongSource(format!(
                        "VST3 bundle {} is unavailable",
                        path.display()
                    )));
                }
                if !self.plugin_file_allowed(&path) {
                    return Err(SessionError::UntrustedPlugin(path));
                }
                let known = self
                    .vst3_plugins_in(&path)?
                    .into_iter()
                    .find(|known| {
                        known.class_id == *class_id && known.kind == PluginKind::Instrument
                    })
                    .ok_or_else(|| {
                        SessionError::SongSource(format!(
                            "{} has no instrument named {class_id}",
                            path.display()
                        ))
                    })?;
                let plugin = Vst3Plugin::load(&path, &known.class_id, &prepare)?;
                // Allocate the renderer before recording the edit, so even activation failures
                // leave the old piece intact. The same validated instance becomes the live one.
                drop(plugin.instrument()?);
                PreparedSource::Vst3 {
                    path,
                    class_id: known.class_id,
                    plugin,
                }
            }
        };
        Ok(Some(prepared))
    }

    fn resolve_song_plugin_path(&self, path: &Path) -> Result<PathBuf, SessionError> {
        require_local_relative_path(path)?;
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        AssetPath::inside(path)
            .resolve(self.project_folder())
            .ok_or_else(|| SessionError::SongSource(format!("cannot resolve {}", path.display())))
    }

    fn composed_font_reference(&self, path: &Path) -> AssetPath {
        match self
            .project_folder()
            .and_then(|folder| path.strip_prefix(folder).ok())
        {
            Some(relative) => AssetPath::inside(relative),
            None => AssetPath::external(path),
        }
    }

    pub(in crate::session) fn install_composed_source(
        &mut self,
        project: &mut Project,
        track: TrackId,
        source: PreparedSource,
    ) -> Option<PreparedSource> {
        match source {
            PreparedSource::SoundFont {
                path,
                reference,
                samples,
                bank,
                patch,
            } => {
                let existing = project
                    .soundfonts
                    .values()
                    .find(|font| font.path.resolve(self.project_folder()).as_ref() == Some(&path))
                    .map(|font| font.id);
                let font = existing.unwrap_or_else(|| {
                    project.add_soundfont(font_name(&samples, &path), reference, byte_size(&path))
                });
                // The cache restores ids only after the document is adopted; preflight never
                // inserts into the current document's bank where the new id could collide.
                if let Some(inner) = project
                    .track_mut(track)
                    .and_then(|track| track.kind.as_instrument_mut())
                {
                    store_preset(&mut inner.instrument_state, PresetRef { font, bank, patch });
                }
                None
            }
            source => {
                let path = match &source {
                    PreparedSource::Clap { path, .. } | PreparedSource::Vst3 { path, .. } => path,
                    PreparedSource::SoundFont { .. } => unreachable!(),
                };
                project.set_hosted_instrument(
                    track,
                    source.instrument_id(),
                    AssetPath::external(path),
                );
                Some(source)
            }
        }
    }

    pub(in crate::session) fn install_composed_hosted_source(
        &mut self,
        track: TrackId,
        source: PreparedSource,
    ) {
        match source {
            PreparedSource::Clap {
                path,
                plugin_id,
                plugin,
            } => {
                self.hosted
                    .install_composed_instrument(track, path, plugin_id, plugin);
            }
            PreparedSource::Vst3 {
                path,
                class_id,
                plugin,
            } => {
                self.vst3
                    .install_composed_instrument(track, path, class_id, plugin);
            }
            PreparedSource::SoundFont { .. } => unreachable!(),
        }
    }
}

fn require_preset(
    samples: &SoundFont,
    path: &Path,
    bank: i32,
    patch: i32,
) -> Result<(), SessionError> {
    if presets(samples)
        .iter()
        .any(|preset| preset.bank == bank && preset.patch == patch)
    {
        return Ok(());
    }
    Err(SessionError::SongSource(format!(
        "SoundFont {} has no preset at bank {bank}, patch {patch}",
        path.display()
    )))
}

fn require_local_relative_path(path: &Path) -> Result<(), SessionError> {
    if !path.is_absolute()
        && path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(SessionError::SongSource(format!(
            "relative source {} must stay inside the project folder",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "song_sources_tests.rs"]
mod tests;
