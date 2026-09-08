//! The shared library hosted above the song sheet, with draft-only source choices.

use std::path::{Path, PathBuf};

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use gpui::{Context, IntoElement, Window, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::library::{Branch, SongLibrary, SongLibraryFont};
use crate::ui::text_field::KeyEffect;

impl AurisApp {
    /// Opens the ordinary library's search and tree for this song part.
    pub(crate) fn open_song_library(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(part) = self
            .song_sheet
            .as_ref()
            .and_then(|dials| dials.parts.get(index))
        else {
            return;
        };
        let mut browser = SongLibrary::new(part.name.clone());
        browser.focused = true;
        self.song_library = Some(browser);
        cx.notify();
    }

    /// Returns to the matrix without changing either the draft or document.
    pub(crate) fn close_song_library(&mut self, cx: &mut Context<Self>) {
        self.song_library = None;
        cx.notify();
    }

    /// A bounded browser above the sheet, retaining the matrix's scroll position underneath.
    pub(crate) fn render_song_library_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let browser = self.song_library.as_ref()?;
        if !self
            .song_sheet
            .as_ref()
            .is_some_and(|dials| dials.parts.iter().any(|part| part.name == browser.part))
        {
            self.song_library = None;
            return None;
        }
        let viewport = window.viewport_size();
        let theme = self.theme.clone();
        let contents = self.render_song_library(window, cx);
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.60))
                .occlude()
                .child(
                    div()
                        .w((viewport.width - px(48.0)).max(px(0.0)).min(px(600.0)))
                        .h((viewport.height - px(64.0)).max(px(0.0)).min(px(720.0)))
                        .rounded(Metrics::RADIUS_LG)
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme.border)
                        .child(contents),
                ),
        )
    }

    /// Sends editing keys to the chooser before considering covered lyrics or library fields.
    pub(crate) fn song_library_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(browser) = self.song_library.as_mut() else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        if browser.search.marked().is_none() {
            match key {
                "escape" => {
                    self.close_song_library(cx);
                    return true;
                }
                "enter" => {
                    browser.focused = false;
                    return true;
                }
                _ => {}
            }
        }
        if !browser.focused {
            return true;
        }
        browser.search.apply_key_with_clipboard(
            key,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.secondary(),
            false,
            cx,
        ) != KeyEffect::Ignored
    }

    fn song_library_part_index(&self) -> Option<usize> {
        let name = &self.song_library.as_ref()?.part;
        self.song_sheet
            .as_ref()?
            .parts
            .iter()
            .position(|part| &part.name == name)
    }

    /// Applies a built-in instrument to the draft part, then returns to the matrix.
    pub(crate) fn choose_song_library_instrument(&mut self, instrument: &str) {
        if let Some(index) = self.song_library_part_index()
            && let Some(dials) = self.song_sheet.as_mut()
        {
            super::set_part_instrument(dials, index, instrument);
        }
        self.song_library = None;
    }

    /// Applies an exact file-backed source to the draft part, then returns to the matrix.
    pub(crate) fn choose_song_library_source(&mut self, source: PartSource) {
        if let Some(index) = self.song_library_part_index()
            && let Some(dials) = self.song_sheet.as_mut()
        {
            super::set_part_source(dials, index, source);
        }
        self.song_library = None;
    }

    /// Reads a font on a worker and adds it only to the composing browser's shelf.
    pub(crate) fn import_song_soundfont(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(part) = self
            .song_library
            .as_ref()
            .map(|browser| browser.part.clone())
        else {
            return;
        };
        let language = self.language();
        if let Some(browser) = self.song_library.as_mut() {
            browser.error = None;
        }
        cx.spawn(async move |this, cx| {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogImportSoundFont.get(language))
                .add_filter(
                    Key::FilterSoundFont.get(language),
                    auris_session::supported_soundfont_extensions(),
                )
                .pick_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let source_path = path.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { auris_session::read_soundfont(&source_path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(font) => {
                        let (name, presets) = this.session.cache_song_soundfont(&path, font);
                        let count = presets.len();
                        let id = this.remember_song_library_font(path, name.clone(), presets);
                        if let Some(browser) = this
                            .song_library
                            .as_mut()
                            .filter(|browser| browser.part == part)
                        {
                            browser.search = crate::ui::text_field::TextField::new(String::new());
                            browser.tree.set_open(Branch::SoundFonts, true);
                            browser.tree.set_open(Branch::Font(id), true);
                            browser.reveal = Some(Branch::Font(id));
                        }
                        this.set_status(messages::soundfont_imported(
                            this.language(),
                            &name,
                            count,
                        ));
                    }
                    Err(error) => {
                        let message = this.failure(Key::CmdImportSoundFont, &error);
                        if let Some(browser) = this
                            .song_library
                            .as_mut()
                            .filter(|browser| browser.part == part)
                        {
                            browser.error = Some(message.clone());
                        }
                        this.set_failed_status(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remember_song_library_font(
        &mut self,
        path: PathBuf,
        name: String,
        presets: Vec<SoundFontPreset>,
    ) -> SoundFontId {
        let loaded_id = self.session.soundfonts().find_map(|font| {
            (self.session.soundfont_is_loaded(font.id)
                && font.path.resolve(self.session.project_folder()).as_ref() == Some(&path))
            .then_some(font.id)
        });
        if let Some(font) = self
            .song_library_fonts
            .iter_mut()
            .find(|font| font.path == path)
        {
            font.name = name;
            font.presets = presets;
            return loaded_id.unwrap_or(font.id);
        }
        let id = (0..u64::MAX)
            .map(|offset| SoundFontId(u64::MAX - offset))
            .find(|id| {
                !self.song_library_fonts.iter().any(|font| font.id == *id)
                    && !self.session.soundfonts().any(|font| font.id == *id)
            })
            .expect("font browser identity space is not exhausted");
        self.song_library_fonts.push(SongLibraryFont {
            id,
            name,
            path,
            presets,
        });
        loaded_id.unwrap_or(id)
    }

    /// Displays a selected source even after its browser or original project has closed.
    pub(super) fn song_library_source_label(&self, source: &PartSource) -> String {
        let resolved = self.session.resolve_song_source(source).ok();
        let source = resolved.as_ref().unwrap_or(source);
        match source {
            PartSource::SoundFont { path, bank, patch } => {
                let detached = self
                    .song_library_fonts
                    .iter()
                    .find(|font| &font.path == path)
                    .and_then(|font| {
                        font.presets
                            .iter()
                            .find(|preset| preset.bank == *bank && preset.patch == *patch)
                    })
                    .map(|preset| preset.name.clone());
                let known = detached.or_else(|| {
                    self.session.soundfonts().find_map(|font| {
                        let reference = PresetRef {
                            font: font.id,
                            bank: *bank,
                            patch: *patch,
                        };
                        (self.session.song_source_for_preset(reference).ok().as_ref()
                            == Some(source))
                        .then(|| {
                            self.session
                                .soundfont_presets(font.id)
                                .into_iter()
                                .find(|preset| preset.bank == *bank && preset.patch == *patch)
                                .map(|preset| preset.name)
                        })
                        .flatten()
                    })
                });
                known.map_or_else(
                    || format!("{} · {bank}/{patch}", source_file_name(path)),
                    |name| format!("{name} · {}", source_file_name(path)),
                )
            }
            PartSource::Clap { path, plugin_id } => self
                .clap_contents
                .get(path)
                .and_then(|plugins| plugins.iter().find(|plugin| &plugin.clap_id == plugin_id))
                .map(|plugin| format!("{} · CLAP", plugin.name))
                .unwrap_or_else(|| format!("{} · {plugin_id}", source_file_name(path))),
            PartSource::Vst3 { path, class_id } => self
                .vst3_contents
                .get(path)
                .and_then(|plugins| plugins.iter().find(|plugin| &plugin.class_id == class_id))
                .map(|plugin| format!("{} · VST3", plugin.name))
                .unwrap_or_else(|| format!("{} · {class_id}", source_file_name(path))),
        }
    }
}

fn source_file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
