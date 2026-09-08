//! The left-hand library: every instrument, sound and effect the session can reach, as a tree.
//!
//! Sections open into groups rather than into a list. Instruments and
//! effects group by [`PluginCategory`]; a SoundFont groups by the MIDI banks the file itself
//! declares. The reason is the same in both cases and it is a matter of scale: eleven built-in
//! plugins read fine as a list, but a General MIDI font carries a hundred and twenty-eight
//! sounds, and a hundred and twenty-eight rows is not a list anybody reads — it is a thing to
//! scroll past on the way to the effects.
//!
//! What a branch does when nobody has touched it is [`Branch::opens_by_default`], and what
//! happens after somebody has is [`LibraryTree`]. The two are separate because the sensible
//! default is not the same everywhere: the plugins want to be visible, the hundred and
//! twenty-eight sounds want to be asked for.
//!
//! Colour identifies what a choice does, using the theme slots shared with track kinds.
//! Headings and item icons share these colours in both the tree and search results;
//! names, indentation and check marks carry the same information without relying on colour.

use std::collections::{HashMap, HashSet};

use auris_i18n::Key;
use gpui::MouseButton;

use crate::ui::icons::icon;
use auris_session::prelude::*;
use gpui::{AnyElement, IntoElement, MouseDownEvent, Pixels, Window, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::icons::Icon;
use crate::ui::inspector::{audio_name, panel_header};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::text_field::TextField;
use crate::ui::widgets::divider;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

/// How far one level of the tree is indented.
const INDENT: Pixels = px(11.0);

/// The branch's disclosure triangle and its following gap, also reserved by tree leaves.
const DISCLOSURE_SPACE: Pixels = px(17.0);

/// A branch of the library — anything that can be open or shut.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Branch {
    /// The instruments section.
    Instruments,
    /// One category of instrument.
    InstrumentCategory(PluginCategory),
    /// The SoundFonts section.
    SoundFonts,
    /// The singer voices section.
    Voices,
    /// One imported font.
    Font(SoundFontId),
    /// One bank of one font.
    Bank(SoundFontId, i32),
    /// The effects section.
    Effects,
    /// One category of effect.
    EffectCategory(PluginCategory),
    /// The installed CLAP plugins section.
    Plugins,
    /// One `.clap` file, by its position in the scanned list.
    PluginFile(usize),
}

impl Branch {
    /// Whether this branch is open before anybody has said otherwise.
    ///
    /// Everything except a font, its banks, and an installed plugin file. The built-in plugins
    /// all fit on screen at once, and a browser that hides them is one you have to operate before
    /// you can look at it; a font's sounds run to three figures and are worth asking for.
    ///
    /// A `.clap` file is shut for a stronger reason than size. Opening one means *loading* it,
    /// and loading a plugin means running somebody else's code in this process. That has to be
    /// something a person did, not something a panel did on their behalf while they were looking
    /// for a reverb.
    fn opens_by_default(self) -> bool {
        !matches!(
            self,
            Branch::Font(_) | Branch::Bank(..) | Branch::PluginFile(_)
        )
    }
}

/// Which branches of the library are open.
///
/// What has been *clicked*, not what is open: the default differs by branch, so recording the
/// departures from it means a font nobody has touched stays shut without this having to know
/// which fonts exist in order to say so.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LibraryTree {
    /// Branches whose state has been chosen, and what was chosen.
    chosen: HashMap<Branch, bool>,
}

/// An imported font offered while choosing a song part, without editing the document.
#[derive(Clone)]
pub(crate) struct SongLibraryFont {
    /// Identity for tree disclosures, distinct from every session font.
    pub(crate) id: SoundFontId,
    /// The font's display name.
    pub(crate) name: String,
    /// The persistent file selected by the user.
    pub(crate) path: std::path::PathBuf,
    /// Every sound declared by that file.
    pub(crate) presets: Vec<SoundFontPreset>,
}

/// A song part's library browser, independent of the document library's navigation.
pub(crate) struct SongLibrary {
    /// Stable part name, so a delayed choice cannot target a different row.
    pub(crate) part: String,
    /// Search editing state.
    pub(crate) search: TextField,
    /// Whether the search field holds the keyboard.
    pub(crate) focused: bool,
    /// Disclosures in this browser only.
    pub(crate) tree: LibraryTree,
    /// File branch to reveal after leaving search.
    pub(crate) reveal: Option<Branch>,
    /// Scroll position in this browser only.
    pub(crate) scroll: gpui::ScrollHandle,
    /// A failed import, shown beside the chooser's import action.
    pub(crate) error: Option<String>,
}

impl SongLibrary {
    /// Starts a browser for a stable song part name.
    pub(crate) fn new(part: String) -> Self {
        Self {
            part,
            search: TextField::new(String::new()),
            focused: false,
            tree: LibraryTree::default(),
            reveal: None,
            scroll: gpui::ScrollHandle::new(),
            error: None,
        }
    }
}

/// The destination captured by every shared library row and its event listener.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryTarget {
    Track,
    SongPart,
}

impl LibraryTarget {
    fn selector(self, id: &str) -> String {
        match self {
            Self::Track => id.to_string(),
            Self::SongPart => format!("song-{id}"),
        }
    }

    fn element_id(self, id: &str) -> gpui::SharedString {
        self.selector(id).into()
    }
}

/// A font imported while composing becomes a document font when the song is adopted.
/// Keep one shelf entry per resolved file, preferring a loaded document font and retaining
/// detached metadata when a document reference is unavailable.
fn song_font_catalog(
    session: Vec<(SoundFontId, String, Option<std::path::PathBuf>, bool)>,
    detached: &[SongLibraryFont],
) -> Vec<(SoundFontId, String)> {
    let loaded: HashSet<_> = session
        .iter()
        .filter(|(_, _, _, loaded)| *loaded)
        .filter_map(|(_, _, path, _)| path.as_ref())
        .collect();
    let detached_paths: HashSet<_> = detached.iter().map(|font| &font.path).collect();
    let mut seen = HashSet::new();
    let mut fonts = Vec::new();
    for (id, name, path, available) in &session {
        if let Some(path) = path {
            if !available && (loaded.contains(path) || detached_paths.contains(path)) {
                continue;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
        }
        fonts.push((*id, name.clone()));
    }
    for font in detached {
        if seen.insert(font.path.clone()) {
            fonts.push((font.id, font.name.clone()));
        }
    }
    fonts
}

impl LibraryTree {
    /// Whether a branch is open.
    pub(crate) fn is_open(&self, branch: Branch) -> bool {
        self.is_open_or(branch, branch.opens_by_default())
    }

    /// Whether a branch is open, for a caller that knows better what it should do untouched.
    ///
    /// Only one caller does: a font's only bank opens along with the font, because a row you
    /// have to open to reach the single thing underneath it is a click asking a question that
    /// has one answer. [`Branch`] cannot see that — it does not know how many banks there are.
    pub(crate) fn is_open_or(&self, branch: Branch, untouched: bool) -> bool {
        self.chosen.get(&branch).copied().unwrap_or(untouched)
    }

    /// Opens or shuts a branch.
    ///
    /// A row passes the opposite of how it was *drawn* rather than asking for a toggle. The two
    /// are not the same thing where the default came from the caller: a font's only bank is drawn
    /// open while [`Branch::opens_by_default`] still says shut, and a toggle reading that would
    /// spend the first click setting it to what it already looked like.
    pub(crate) fn set_open(&mut self, branch: Branch, open: bool) {
        self.chosen.insert(branch, open);
    }

    /// Forgets disclosures whose positional identities become invalid after a rescan.
    pub(crate) fn forget_plugin_files(&mut self) {
        self.chosen
            .retain(|branch, _| !matches!(branch, Branch::PluginFile(_)));
    }
}

/// One plugin, as the library lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LibraryPlugin {
    /// Registry id, which is what choosing it passes on.
    pub(crate) id: String,
    /// Name shown, translated where the term is known.
    pub(crate) name: String,
    /// One-line summary shown under the name.
    pub(crate) description: String,
}

/// Plugins under the category they belong to, in [`PluginCategory::ALL`] order.
///
/// A category nothing is registered under does not appear — an empty group is a row that only
/// says the browser has a concept of Modulation. Order *within* a group is the order they
/// arrived in, which from the registry means by id.
pub(crate) fn by_category(
    plugins: impl IntoIterator<Item = (PluginCategory, LibraryPlugin)>,
) -> Vec<(PluginCategory, Vec<LibraryPlugin>)> {
    let mut groups: Vec<(PluginCategory, Vec<LibraryPlugin>)> = Vec::new();
    for (category, plugin) in plugins {
        match groups
            .iter_mut()
            .find(|(existing, _)| *existing == category)
        {
            Some((_, members)) => members.push(plugin),
            None => groups.push((category, vec![plugin])),
        }
    }
    // Sorted rather than built by walking `ALL`, so a category somehow missing from it lands at
    // the end instead of taking its plugins off the list altogether.
    groups.sort_by_key(|(category, _)| browser_order(*category));
    groups
}

/// Where a category sits in the browser, and after everything else if it is not listed at all.
fn browser_order(category: PluginCategory) -> usize {
    PluginCategory::ALL
        .iter()
        .position(|listed| *listed == category)
        .unwrap_or(PluginCategory::ALL.len())
}

/// The sounds of one font, split into the banks it declares, in bank order.
///
/// A bank number is the font's own metadata rather than a reading of it, so this grouping is
/// true of any font rather than only of a General MIDI one: a GM set arrives as bank 0 and bank
/// 128, and a font that uses neither is grouped by whatever it does use.
pub(crate) fn by_bank(presets: Vec<SoundFontPreset>) -> Vec<(i32, Vec<SoundFontPreset>)> {
    let mut banks: Vec<(i32, Vec<SoundFontPreset>)> = Vec::new();
    for preset in presets {
        match banks.iter_mut().find(|(bank, _)| *bank == preset.bank) {
            Some((_, members)) => members.push(preset),
            None => banks.push((preset.bank, vec![preset])),
        }
    }
    banks.sort_by_key(|(bank, _)| *bank);
    banks
}

/// How far in a row at this depth sits.
fn indent(depth: usize) -> Pixels {
    px(6.0) + INDENT * depth as f32
}

/// The action a library item offers, independent of category order or MIDI program number.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LibraryRole {
    Instrument,
    Drum,
    Effect,
    Voice,
    File,
}

impl LibraryRole {
    fn color(self, theme: &Theme) -> gpui::Hsla {
        match self {
            Self::Instrument => theme.track_color(Color::INSTRUMENT.0),
            Self::Drum => theme.track_color(Color::DRUM.0),
            Self::Effect => theme.track_color(Color::BUS.0),
            Self::Voice => theme.track_color(Color::SINGER.0),
            Self::File => theme.text_muted,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Instrument | Self::Drum => Icon::Keyboard,
            Self::Effect => Icon::Knob,
            Self::Voice => Icon::Microphone,
            Self::File => Icon::Library,
        }
    }
}

/// The mark in a category's icon slot, sharing the role colour of its section.
fn swatch(color: gpui::Hsla) -> impl IntoElement {
    div()
        .w(px(3.0))
        .h(px(14.0))
        .flex_shrink_0()
        .rounded(px(1.5))
        .bg(color)
}

/// How many results a search shows.
///
/// A browser is a list to run an eye down, and a query that answers with two hundred rows has
/// answered nothing. Anybody who cannot see what they wanted in forty types another letter.
pub(crate) const SEARCH_LIMIT: usize = 40;

/// Both names a built-in plugin can be found by: the displayed name and its original name.
///
/// The registry speaks English while the browser translates known names. Indexing both keeps
/// a name copied from a manual useful and also lets somebody search for the name on screen.
fn plugin_search_name(name: &str, language: auris_i18n::Language) -> String {
    let displayed = auris_i18n::audio::plugin_name(name, language);
    if displayed == name {
        name.to_string()
    } else {
        format!("{displayed} {name}")
    }
}

/// The entries `query` finds, best first.
///
/// Generic over what an entry *is*, because the kinds the browser holds — an instrument, an
/// effect, a sound in a font, a plugin file on the disk — are one question when somebody is
/// looking for a name, and the whole point of searching is that they stop being four lists.
///
/// The scoring is the command palette's, so `revb` finds Reverb in both places and means the
/// same thing in both. Ties keep the order they were collected in, which is the order the tree
/// shows them.
pub(crate) fn best_matches<T>(entries: Vec<(String, T)>, query: &str, limit: usize) -> Vec<T> {
    let mut scored: Vec<(usize, i32, T)> = entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, (name, entry))| {
            crate::ui::palette::match_score(query, &name).map(|score| (index, score, entry))
        })
        .collect();
    scored.sort_by_key(|(index, score, _)| (std::cmp::Reverse(*score), *index));
    scored.truncate(limit);
    scored.into_iter().map(|(_, _, entry)| entry).collect()
}

/// One thing a search turned up, and enough to draw and act on it.
enum Found {
    /// A built-in instrument, with the category it is filed under.
    Instrument(LibraryPlugin, PluginCategory),
    /// A built-in effect, likewise.
    Effect(LibraryPlugin, PluginCategory),
    /// One sound in one font.
    Preset(SoundFontId, SoundFontPreset),
    /// A `.clap` file on the disk: its place in the scanned list, its name, and its path.
    ///
    /// The file rather than the plugins in it, because the plugins in it are not known until it
    /// is loaded — and a result list that loads a shared library per row is a result list that
    /// runs somebody else's code to answer a keystroke.
    ClapFile(usize, String, std::path::PathBuf),
    /// A `.vst3` bundle, represented without loading its executable code.
    Vst3File(usize, String, std::path::PathBuf),
    /// A singer voice on the shelf: its name, and the file it is.
    Voice(String, std::path::PathBuf),
}

/// How a branch row is drawn.
#[derive(Copy, Clone, Debug, PartialEq)]
struct RowStyle {
    /// Colour of the name.
    label: gpui::Hsla,
    /// The group this row stands for, when it stands for one.
    accent: Option<gpui::Hsla>,
}

impl AurisApp {
    /// The left-hand library: everything the session can play, ready to load.
    ///
    /// Logic's arrangement, and the reason for it: choosing an instrument is something you do
    /// *to* the track you are looking at, so the list and the track's own settings have to be on
    /// screen together. Sharing one panel meant picking a plugin hid the strip it was going onto.
    pub(crate) fn render_library(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let rows = self.library_rows(LibraryTarget::Track, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .child(panel_header(self.t(Key::Library), &theme))
            .child(crate::ui::widgets::button(
                "open-timbre-map",
                self.t(Key::TimbreMap),
                crate::ui::widgets::ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.timbre_map.open = true;
                    if this.timbre_map.map.is_none() && this.timbre_map.control.is_none() {
                        this.scan_timbre_map(cx);
                    }
                    cx.notify();
                }),
            ))
            .child(self.library_search_field(LibraryTarget::Track, cx))
            .child(
                self.scrolling(
                    ScrollPanel::Library,
                    // Rows own their spacing so branch headings and two-line items can differ.
                    div()
                        .id("library-body")
                        .overflow_y_scroll()
                        .p_1()
                        .flex()
                        .flex_col()
                        .children(rows),
                    cx,
                ),
            )
    }

    /// The ordinary library's search and tree, directed at a song part in the open sheet.
    pub(crate) fn render_song_library(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let Some(browser) = self.song_library.as_ref() else {
            return div().into_any_element();
        };
        let scroll = browser.scroll.clone();
        let error = browser.error.clone();
        let hint = self
            .t(Key::SongLibraryHint)
            .replace("{part}", &browser.part);
        let theme = self.theme.clone();
        let rows = self.library_rows(LibraryTarget::SongPart, cx);
        div()
            .id("song-library")
            .debug_selector(|| "song-library".to_string())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_2()
                    .child(
                        div()
                            .text_color(theme.text)
                            .child(self.t(Key::SongLibraryTitle)),
                    )
                    .child(crate::ui::widgets::button(
                        "song-library-close",
                        self.t(Key::Close),
                        crate::ui::widgets::ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.close_song_library(cx)),
                    )),
            )
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(hint),
            )
            .child(self.library_search_field(LibraryTarget::SongPart, cx))
            .child(div().px_2().pb_1().child(crate::ui::widgets::button(
                "song-library-import-font",
                self.t(Key::MenuImportSoundFontItem),
                crate::ui::widgets::ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| this.import_song_soundfont(window, cx)),
            )))
            .when_some(error, |panel, error| {
                panel.child(
                    div()
                        .debug_selector(|| "song-library-error".to_string())
                        .px_2()
                        .pb_2()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .id("song-library-body")
                            .debug_selector(|| "song-library-body".to_string())
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .p_1()
                            .pr_3()
                            .flex()
                            .flex_col()
                            .children(rows),
                    )
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&scroll).scrollbar_show(ScrollbarShow::Always),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn library_tree(&self, target: LibraryTarget) -> &LibraryTree {
        match target {
            LibraryTarget::Track => &self.library,
            LibraryTarget::SongPart => {
                &self
                    .song_library
                    .as_ref()
                    .expect("song browser is open")
                    .tree
            }
        }
    }

    fn set_library_branch(&mut self, target: LibraryTarget, branch: Branch, open: bool) {
        match target {
            LibraryTarget::Track => self.library.set_open(branch, open),
            LibraryTarget::SongPart => {
                if let Some(browser) = self.song_library.as_mut() {
                    browser.tree.set_open(branch, open);
                }
            }
        }
    }

    fn library_query(&self, target: LibraryTarget) -> &TextField {
        match target {
            LibraryTarget::Track => &self.library_search,
            LibraryTarget::SongPart => {
                &self
                    .song_library
                    .as_ref()
                    .expect("song browser is open")
                    .search
            }
        }
    }

    fn clear_library_query(&mut self, target: LibraryTarget) {
        match target {
            LibraryTarget::Track => self.leave_library_search(),
            LibraryTarget::SongPart => {
                if let Some(browser) = self.song_library.as_mut() {
                    browser.search = TextField::new(String::new());
                    browser.focused = false;
                }
            }
        }
    }

    fn reveal_library_branch(&mut self, target: LibraryTarget, branch: Branch) {
        self.clear_library_query(target);
        self.set_library_branch(target, Branch::Plugins, true);
        self.set_library_branch(target, branch, true);
        match target {
            LibraryTarget::Track => self.library_reveal = Some(branch),
            LibraryTarget::SongPart => {
                if let Some(browser) = self.song_library.as_mut() {
                    browser.reveal = Some(branch);
                }
            }
        }
    }

    fn scroll_library_branch(&mut self, target: LibraryTarget, branch: Branch, row: usize) {
        match target {
            LibraryTarget::Track if self.library_reveal == Some(branch) => {
                self.library_scroll.scroll_to_item(row);
                self.library_reveal = None;
            }
            LibraryTarget::SongPart => {
                if let Some(browser) = self
                    .song_library
                    .as_mut()
                    .filter(|browser| browser.reveal == Some(branch))
                {
                    browser.scroll.scroll_to_item(row);
                    browser.reveal = None;
                }
            }
            _ => {}
        }
    }

    fn library_song_part(&self) -> Option<&PartSpec> {
        let name = &self.song_library.as_ref()?.part;
        self.song_sheet
            .as_ref()?
            .parts
            .iter()
            .find(|part| &part.name == name)
    }

    fn library_song_source(&self) -> Option<PartSource> {
        self.library_song_part()?
            .source
            .as_ref()
            .and_then(|source| self.session.resolve_song_source(source).ok())
    }

    fn library_takes_instrument(&self, target: LibraryTarget) -> bool {
        match target {
            LibraryTarget::Track => self.selected_track_takes_an_instrument(),
            LibraryTarget::SongPart => self.library_song_part().is_some(),
        }
    }

    fn choose_library_instrument(&mut self, target: LibraryTarget, instrument: &str) {
        match target {
            LibraryTarget::Track => self.set_track_instrument(instrument),
            LibraryTarget::SongPart => self.choose_song_library_instrument(instrument),
        }
    }

    fn library_fonts(&self, target: LibraryTarget) -> Vec<(SoundFontId, String)> {
        if target == LibraryTarget::Track {
            return self
                .session
                .soundfonts()
                .map(|font| (font.id, font.name.clone()))
                .collect();
        }
        let session = self
            .session
            .soundfonts()
            .map(|font| {
                (
                    font.id,
                    font.name.clone(),
                    font.path.resolve(self.session.project_folder()),
                    self.session.soundfont_is_loaded(font.id),
                )
            })
            .collect();
        song_font_catalog(session, &self.song_library_fonts)
    }

    fn library_presets(&self, target: LibraryTarget, font: SoundFontId) -> Vec<SoundFontPreset> {
        if target == LibraryTarget::SongPart
            && let Some(font) = self
                .song_library_fonts
                .iter()
                .find(|entry| entry.id == font)
        {
            return font.presets.clone();
        }
        self.session.soundfont_presets(font)
    }

    fn library_font_loaded(&self, target: LibraryTarget, font: SoundFontId) -> bool {
        (target == LibraryTarget::SongPart
            && self.song_library_fonts.iter().any(|entry| entry.id == font))
            || self.session.soundfont_is_loaded(font)
    }

    fn library_preset_source(&self, choice: PresetRef) -> Option<PartSource> {
        if let Some(font) = self
            .song_library_fonts
            .iter()
            .find(|font| font.id == choice.font)
        {
            return Some(PartSource::SoundFont {
                path: font.path.clone(),
                bank: choice.bank,
                patch: choice.patch,
            });
        }
        self.session.song_source_for_preset(choice).ok()
    }

    fn choose_library_preset(&mut self, target: LibraryTarget, choice: PresetRef) {
        match target {
            LibraryTarget::Track => self.set_track_preset(choice),
            LibraryTarget::SongPart => {
                if let Some(source) = self.library_preset_source(choice) {
                    self.choose_song_library_source(source);
                }
            }
        }
    }

    /// Gives the keyboard back to the application and clears the query.
    ///
    /// Both together, always. A query left behind an unfocused field is a browser showing a
    /// filtered list with nothing on screen saying why, and a focus left behind a cleared query
    /// is a panel that has quietly taken the space bar.
    pub(crate) fn leave_library_search(&mut self) {
        self.library_search = crate::ui::text_field::TextField::new(String::new());
        self.library_search_focused = false;
    }

    /// The search box at the top of the browser.
    ///
    /// Always there rather than summoned. A browser holding twenty plugins and a font with a
    /// hundred and twenty-eight sounds is a list nobody scrolls twice, and a search that has to
    /// be opened first is one people forget is there.
    ///
    /// Clicking it takes the keyboard, because a bound key never reaches a key listener in gpui
    /// and a field that did not claim them could not be typed `i` into without the inspector
    /// opening. It says so while it holds them — the accent ring is not decoration — and gives
    /// them back on Escape, on Enter, and as soon as a result is chosen.
    fn library_search_field(
        &mut self,
        target: LibraryTarget,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let focused = match target {
            LibraryTarget::Track => self.library_search_focused,
            LibraryTarget::SongPart => self
                .song_library
                .as_ref()
                .is_some_and(|browser| browser.focused),
        };
        let text = self.library_query(target).content().to_string();
        let empty = text.is_empty();
        let selection = self.library_query(target).selection();
        let marked = self.library_query(target).marked();
        let view = cx.entity();
        // The window's own handle, the one the palette and the prompt type through: the input
        // handler is registered against whatever holds the keyboard, and while this field has it
        // there is nothing else it could be.
        let handle = self.focus.clone();

        div()
            .id(target.element_id("library-search"))
            .debug_selector(move || target.selector("library-search"))
            .flex()
            .items_center()
            // No gap after the icon: the text carries its own left inset, because the field
            // paints its caret at one. A gap on top of it would put the words a third of the
            // way across a box this narrow.
            .mx_1()
            .mb_1()
            .h(Metrics::CONTROL_HEIGHT)
            .px_1p5()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(match focused {
                true => theme.accent,
                false => theme.border_subtle,
            })
            .cursor_text()
            .child(icon(Icon::Library, px(13.0), theme.text_muted))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(match focused {
                        true => crate::ui::prompt::editable_text(
                            text.clone().into(),
                            selection,
                            marked,
                            handle,
                            view,
                            theme.clone(),
                        )
                        .into_any_element(),
                        false => crate::ui::prompt::field_text(text.clone(), theme.text)
                            .into_any_element(),
                    })
                    // The placeholder under the field rather than in it, so the real text is
                    // never something the field has to decide whether to keep. Laid out by the
                    // same helper as the value, so the caret lands on the first letter of it
                    // rather than a character in.
                    .when(empty, |this| {
                        this.child(
                            crate::ui::prompt::field_text(
                                self.t(Key::BrowserSearch),
                                theme.text_muted,
                            )
                            .absolute()
                            .inset_0(),
                        )
                    }),
            )
            // Only once there is something to clear, because a cross on an empty field is a
            // button that does nothing sitting where the eye keeps going.
            .when(!empty, |this| {
                this.child(
                    div()
                        .id(target.element_id("library-search-clear"))
                        .debug_selector(move || target.selector("library-search-clear"))
                        .size(px(18.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(icon(Icon::Cross, px(10.0), theme.text_muted))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.clear_library_query(target);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    // One field in the window types at a time; the agent panel's is the other
                    // one that lives in a panel rather than a sheet.
                    this.agent_chat.focused = None;
                    match target {
                        LibraryTarget::Track => this.library_search_focused = true,
                        LibraryTarget::SongPart => {
                            if let Some(browser) = this.song_library.as_mut() {
                                browser.focused = true;
                            }
                        }
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The whole tree, flattened into the rows that are currently visible.
    ///
    /// A shut branch contributes its own row and nothing else, so the cost of a font with a
    /// hundred and twenty-eight sounds is not paid until somebody opens it.
    fn library_rows(
        &mut self,
        target: LibraryTarget,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let query = self.library_query(target).content().trim().to_string();
        if !query.is_empty() {
            return self.search_rows(target, &query, cx);
        }
        let theme = self.theme.clone();
        let mut rows = self.instrument_rows(target, cx);
        rows.push(divider(&theme).into_any_element());
        let font_row_offset = rows.len();
        rows.extend(self.soundfont_rows(target, font_row_offset, cx));
        rows.push(divider(&theme).into_any_element());
        if target == LibraryTarget::Track {
            rows.extend(self.voice_rows(cx));
            rows.push(divider(&theme).into_any_element());
            rows.extend(self.effect_rows(target, cx));
            rows.push(divider(&theme).into_any_element());
        }
        let plugin_row_offset = rows.len();
        rows.extend(self.installed_plugin_rows(target, plugin_row_offset, cx));
        rows
    }

    /// Everything the browser can name, filtered down to what a query finds.
    ///
    /// A flat list rather than a pruned tree. While a query is on, the sections and the branches
    /// are in the way: what somebody typing `marim` wants is the row, not the three headings
    /// above it, and a tree that opened itself to show one leaf would have to close itself again
    /// afterwards.
    ///
    /// A hosted plugin is matched by its *file*, which is the only thing known about it before it
    /// is loaded. Searching the names inside would mean opening every `.clap` on the machine to
    /// answer one keystroke.
    fn search_rows(
        &mut self,
        target: LibraryTarget,
        query: &str,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let mut entries: Vec<(String, Found)> = Vec::new();
        for descriptor in self
            .registry()
            .instruments()
            .filter(|descriptor| target == LibraryTarget::Track || descriptor.id != SAMPLER_ID)
        {
            entries.push((
                plugin_search_name(&descriptor.name, self.language()),
                Found::Instrument(
                    LibraryPlugin {
                        id: descriptor.id.to_string(),
                        name: descriptor.name.to_string(),
                        description: descriptor.description.to_string(),
                    },
                    descriptor.category,
                ),
            ));
        }
        for descriptor in self
            .registry()
            .effects()
            .filter(|_| target == LibraryTarget::Track)
        {
            entries.push((
                plugin_search_name(&descriptor.name, self.language()),
                Found::Effect(
                    LibraryPlugin {
                        id: descriptor.id.to_string(),
                        name: descriptor.name.to_string(),
                        description: descriptor.description.to_string(),
                    },
                    descriptor.category,
                ),
            ));
        }
        for (font, _) in self.library_fonts(target) {
            for preset in self.library_presets(target, font) {
                entries.push((preset.name.clone(), Found::Preset(font, preset)));
            }
        }
        for (index, file) in self.clap_files().to_vec().into_iter().enumerate() {
            let name = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            entries.push((name.clone(), Found::ClapFile(index, name, file)));
        }
        let vst_offset = self.clap_files().len();
        for (index, file) in self.vst3_files().to_vec().into_iter().enumerate() {
            let name = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            entries.push((
                name.clone(),
                Found::Vst3File(vst_offset + index, name, file),
            ));
        }
        if target == LibraryTarget::Track {
            for (name, path) in self.voice_list() {
                entries.push((name.clone(), Found::Voice(name, path)));
            }
        }

        let mut found = best_matches(entries, query, SEARCH_LIMIT + 1);
        let limited = found.len() > SEARCH_LIMIT;
        found.truncate(SEARCH_LIMIT);
        if found.is_empty() {
            return vec![self.note_row(0, self.t(Key::BrowserNothingFound))];
        }
        let mut rows: Vec<AnyElement> = found
            .into_iter()
            .map(|entry| match entry {
                Found::Instrument(plugin, category) => {
                    let id = plugin.id.clone();
                    self.plugin_row(
                        target,
                        &plugin,
                        LibraryRole::Instrument,
                        category,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_library_instrument(target, &id);
                            this.clear_library_query(target);
                            cx.notify();
                        }),
                    )
                }
                Found::Effect(plugin, category) => {
                    let id = plugin.id.clone();
                    self.plugin_row(
                        target,
                        &plugin,
                        LibraryRole::Effect,
                        category,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.add_effect_to_selection(&id);
                            this.leave_library_search();
                            cx.notify();
                        }),
                    )
                }
                Found::Preset(font, preset) => {
                    let choice = PresetRef {
                        font,
                        bank: preset.bank,
                        patch: preset.patch,
                    };
                    self.preset_row(
                        target,
                        &preset,
                        choice,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_library_preset(target, choice);
                            this.clear_library_query(target);
                            cx.notify();
                        }),
                    )
                }
                // The file, not the plugins in it: opening it here would load it, and a result
                // list that loads a shared library per row is a result list that stutters. The
                // row clears the search and opens the file's branch in the tree, which is where
                // the plugins inside it are listed the way they always were.
                Found::ClapFile(index, name, file) => {
                    let branch = Branch::PluginFile(index);
                    self.plugin_row(
                        target,
                        &LibraryPlugin {
                            id: file.display().to_string(),
                            name,
                            description: file.display().to_string(),
                        },
                        LibraryRole::File,
                        PluginCategory::Utility,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.reveal_library_branch(target, branch);
                            cx.notify();
                        }),
                    )
                }
                Found::Vst3File(index, name, file) => {
                    let branch = Branch::PluginFile(index);
                    self.plugin_row(
                        target,
                        &LibraryPlugin {
                            id: file.display().to_string(),
                            name,
                            description: file.display().to_string(),
                        },
                        LibraryRole::File,
                        PluginCategory::Utility,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.reveal_library_branch(target, branch);
                            cx.notify();
                        }),
                    )
                }
                Found::Voice(name, path) => self.voice_row(&name, path, cx),
            })
            .collect();
        if limited {
            rows.push(self.note_row(0, self.t(Key::BrowserSearchLimited)));
        }
        rows
    }

    /// The singer voices this machine can offer, scanned once and kept.
    pub(crate) fn voice_list(&mut self) -> Vec<(String, std::path::PathBuf)> {
        self.voices
            .get_or_insert_with(|| auris_session::library::voices_with_settings(&self.settings))
            .clone()
    }

    /// The singer voices section — the browser interface a voice shares with the instruments.
    ///
    /// A voice is chosen the way a sound is: one row, one click, onto the selected singer
    /// track. Track → Choose Voice… keeps the file dialog for the one-off file somewhere
    /// unusual; what this section holds is the shelf — Auris `.onnx` files, DiffSinger voicebanks,
    /// VOICEVOX connections, and LeapSinger manifests in a `Voices` folder or in a folder
    /// registered below.
    fn voice_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let target = LibraryTarget::Track;
        let voices = self.voice_list();
        let mut rows = vec![self.section_row(
            target,
            Branch::Voices,
            Key::BrowserVoices,
            Icon::Microphone,
            voices.len(),
            cx,
        )];
        if !self.library.is_open(Branch::Voices) {
            return rows;
        }
        let target = self
            .singer_target()
            .and_then(|track| self.project().track(track));
        let hint = target
            .map(|track| auris_i18n::messages::voice_target(self.language(), &track.name))
            .unwrap_or_else(|| self.t(Key::SingerSelectTrack).to_string());
        rows.push(self.note_row(1, &hint));
        if voices.is_empty() {
            rows.push(self.note_row(1, self.t(Key::BrowserNoVoices)));
        }
        for (name, path) in voices {
            rows.push(self.voice_row(&name, path, cx));
        }
        rows.extend(self.voice_path_rows(cx));
        rows
    }

    /// One voice on the shelf. Clicking it is choosing it, exactly like a sound.
    fn voice_row(
        &self,
        name: &str,
        path: std::path::PathBuf,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let accent = LibraryRole::Voice.color(&theme);
        let shown = path.display().to_string();
        let target = self.singer_target();
        let enabled = target.is_some();
        let selected = target
            .and_then(|track| self.session.singer_voice_info(track).ok().flatten())
            .is_some_and(|info| info.path == path);
        let target_label = target
            .and_then(|track| self.project().track(track))
            .map(|track| auris_i18n::messages::voice_target(self.language(), &track.name))
            .unwrap_or_else(|| self.t(Key::SingerSelectTrack).to_string());
        let tooltip =
            crate::ui::tooltip::keyed_tip(format!("{target_label} · {name} · {shown}"), "", &theme);
        let backend = match auris_session::voice_source_kind(&path) {
            Some(auris_session::VoiceSourceKind::Auris) => Key::VoiceBackendAuris,
            Some(auris_session::VoiceSourceKind::DiffSinger) => Key::VoiceBackendDiffSinger,
            Some(auris_session::VoiceSourceKind::Voicevox) => Key::VoiceBackendVoicevox,
            Some(auris_session::VoiceSourceKind::LeapSinger) => Key::VoiceBackendLeapSinger,
            None => Key::VoiceBackendAuris,
        };
        div()
            .id(gpui::SharedString::from(format!("lib-voice-{shown}")))
            .debug_selector(|| format!("lib-voice-{shown}"))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .pl(if self.library_search.content().trim().is_empty() {
                indent(1) + DISCLOSURE_SPACE
            } else {
                indent(0)
            })
            .pr_1p5()
            .py_1()
            .rounded(Metrics::RADIUS_SM)
            .when(selected, |this| this.bg(theme.surface_raised))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(theme.surface_hover))
            })
            .tooltip(tooltip)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .w_full()
                    .min_w_0()
                    .child(div().flex_shrink_0().child(icon(
                        Icon::Microphone,
                        px(14.0),
                        if enabled { accent } else { theme.text_muted },
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .child(name.to_string()),
                    )
                    .when(selected, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .child(icon(Icon::Check, px(14.0), accent)),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(backend)),
            )
            .when(enabled, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_track_voice(&path);
                        this.leave_library_search();
                        cx.notify();
                    }),
                )
            })
            .into_any_element()
    }

    /// The folders voices are also looked for in, and the row that adds another — the
    /// plugin-path arrangement, on the voice shelf.
    fn voice_path_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, path) in self.settings.voice_paths.clone().into_iter().enumerate() {
            let shown = path.display().to_string();
            rows.push(
                div()
                    .id(("voice-path", index))
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(indent(1))
                    .pr_1()
                    .h(Metrics::CONTROL_HEIGHT)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(div().flex_1().min_w_0().truncate().child(shown))
                    .child(
                        div()
                            .id(("forget-voice-path", index))
                            .cursor_pointer()
                            .child(icon(Icon::Cross, px(10.0), theme.text_faint))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.forget_voice_path(index);
                                    cx.notify();
                                }),
                            ),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .pl(indent(1))
                .pr_1()
                .py_1()
                .child(crate::ui::widgets::icon_label(
                    "add-voice-path",
                    Icon::Plus,
                    self.t(Key::BrowserAddVoiceFolder),
                    &theme,
                    cx.listener(|this, _, _, cx| this.add_voice_path(cx)),
                ))
                .into_any_element(),
        );
        rows.push(
            div()
                .pl(indent(1))
                .pr_1()
                .py_1()
                .child(crate::ui::widgets::icon_label(
                    "setup-voicevox",
                    Icon::Sliders,
                    self.t(Key::BrowserSetupVoicevox),
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.open_voice_setup(
                            crate::voice_setup_window::VoiceSetupTab::Voicevox,
                            cx,
                        )
                    }),
                ))
                .into_any_element(),
        );
        rows.push(
            div()
                .pl(indent(1))
                .pr_1()
                .py_1()
                .child(crate::ui::widgets::icon_label(
                    "setup-diffsinger",
                    Icon::Sliders,
                    self.t(Key::BrowserSetupDiffSinger),
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.open_voice_setup(
                            crate::voice_setup_window::VoiceSetupTab::DiffSinger,
                            cx,
                        )
                    }),
                ))
                .into_any_element(),
        );
        rows
    }

    /// The extra places plugins are looked for, and the row that adds another.
    ///
    /// Each added folder can be forgotten again from its own row. One list-wide "forget them
    /// all" would be shorter to write and wrong to use: these are added one at a time, for
    /// unrelated reasons, and the one that has gone stale is rarely the only one there.
    fn plugin_path_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, path) in self.settings.plugin_paths.clone().into_iter().enumerate() {
            let shown = path.display().to_string();
            rows.push(
                div()
                    .id(("plugin-path", index))
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(indent(1))
                    .pr_1()
                    .h(Metrics::CONTROL_HEIGHT)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(div().flex_1().min_w_0().truncate().child(shown))
                    .child(
                        div()
                            .id(("forget-plugin-path", index))
                            .cursor_pointer()
                            .child(icon(Icon::Cross, px(10.0), theme.text_faint))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.forget_plugin_path(index);
                                    cx.notify();
                                }),
                            ),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .pl(indent(1))
                .pr_1()
                .py_1()
                .child(crate::ui::widgets::icon_label(
                    "add-plugin-path",
                    Icon::Plus,
                    self.t(Key::BrowserAddPluginFolder),
                    &theme,
                    cx.listener(|this, _, _, cx| this.add_plugin_path(cx)),
                ))
                .into_any_element(),
        );
        rows
    }

    /// The installed CLAP plugins section: the files found on this machine, and what is in one
    /// once somebody opens it.
    ///
    /// Two levels rather than the categories the built-ins get, because the grouping that matters
    /// here is the *file*: it is what the document stores, what has to still be installed for the
    /// project to open, and the only thing known about a plugin before it is loaded.
    fn installed_plugin_rows(
        &mut self,
        target: LibraryTarget,
        row_offset: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let files = self.clap_files().to_vec();
        let vst3_files = self.vst3_files().to_vec();

        let mut rows = vec![self.section_row(
            target,
            Branch::Plugins,
            Key::BrowserPlugins,
            Icon::Knob,
            files.len() + vst3_files.len(),
            cx,
        )];
        if !self.library_tree(target).is_open(Branch::Plugins) {
            return rows;
        }
        rows.push(self.note_row(
            1,
            self.t(match files.is_empty() && vst3_files.is_empty() {
                true => Key::BrowserNoPlugins,
                false => Key::BrowserPluginsHint,
            }),
        ));
        // The conventional folders are not the only places plugins live: a build tree, an
        // external disk, a folder shared between the machines in a studio. Until now a plugin
        // outside them could not be reached at all, however plainly somebody could point at it.
        rows.extend(self.plugin_path_rows(cx));

        for (index, file) in files.iter().enumerate() {
            let branch = Branch::PluginFile(index);
            let open = self.library_tree(target).is_open(branch);
            // A search opens the file to choose a plugin, so reveal the first child along
            // with its file instead of stopping with only the file heading at the bottom.
            self.scroll_library_branch(target, branch, row_offset + rows.len() + usize::from(open));
            let name = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string_lossy().into_owned());

            let mut listed = match open {
                true => self.clap_plugins_in(file),
                false => Vec::new(),
            };
            let readable = !listed.is_empty();
            if target == LibraryTarget::SongPart {
                listed.retain(|plugin| plugin.kind == PluginKind::Instrument);
            }
            let theme = self.theme.clone();
            rows.push(
                self.branch_row(
                    target,
                    ("lib-clap-file", 1_000 + index),
                    1,
                    open,
                    true,
                    None,
                    name,
                    match open {
                        true => listed.len().to_string(),
                        false => String::new(),
                    },
                    self.row_style(theme.text, None),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_library_branch(target, branch, !open);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
            if !open {
                continue;
            }
            if listed.is_empty() {
                rows.push(self.note_row(
                    2,
                    self.t(if readable {
                        Key::SongLibraryNoInstruments
                    } else {
                        Key::BrowserPluginUnreadable
                    }),
                ));
                continue;
            }

            for info in listed {
                let file = file.clone();
                let clap_id = info.clap_id.clone();
                let kind = info.kind;
                rows.push(self.plugin_source_row(
                    target,
                    &LibraryPlugin {
                        id: info.auris_id(),
                        name: info.name.clone(),
                        // The vendor rather than a description: a hosted plugin's description is
                        // usually empty, and whose plugin it is answers the question a list of
                        // unfamiliar names actually raises.
                        description: info.vendor.clone(),
                    },
                    // The same icons the built-ins get, so a row reads as a sound or as a
                    // treatment before its name has been read.
                    match kind {
                        PluginKind::Instrument => LibraryRole::Instrument,
                        PluginKind::Effect => LibraryRole::Effect,
                    },
                    info.category,
                    Some(PartSource::Clap {
                        path: file.clone(),
                        plugin_id: clap_id.clone(),
                    }),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if target == LibraryTarget::SongPart {
                            this.choose_song_library_source(PartSource::Clap {
                                path: file.clone(),
                                plugin_id: clap_id.clone(),
                            });
                            cx.notify();
                            return;
                        }
                        match kind {
                            PluginKind::Instrument => {
                                this.set_hosted_instrument_on_selection(&file, &clap_id)
                            }
                            PluginKind::Effect => {
                                this.add_hosted_effect_to_selection(&file, &clap_id)
                            }
                        }
                        cx.notify();
                    }),
                ));
            }
        }
        let offset = files.len();
        for (vst_index, file) in vst3_files.iter().enumerate() {
            let index = offset + vst_index;
            let branch = Branch::PluginFile(index);
            let open = self.library_tree(target).is_open(branch);
            self.scroll_library_branch(target, branch, row_offset + rows.len() + usize::from(open));
            let name = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string_lossy().into_owned());
            let mut listed = if open {
                self.vst3_plugins_in(file)
            } else {
                Vec::new()
            };
            let readable = !listed.is_empty();
            if target == LibraryTarget::SongPart {
                listed.retain(|plugin| plugin.kind == PluginKind::Instrument);
            }
            let theme = self.theme.clone();
            rows.push(
                self.branch_row(
                    target,
                    ("lib-vst3-file", 10_000 + index),
                    1,
                    open,
                    true,
                    None,
                    name,
                    if open {
                        listed.len().to_string()
                    } else {
                        String::new()
                    },
                    self.row_style(theme.text, None),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_library_branch(target, branch, !open);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
            if !open {
                continue;
            }
            if listed.is_empty() {
                rows.push(self.note_row(
                    2,
                    self.t(if readable {
                        Key::SongLibraryNoInstruments
                    } else {
                        Key::BrowserPluginUnreadable
                    }),
                ));
                continue;
            }
            for info in listed {
                let file = file.clone();
                let class_id = info.class_id.clone();
                let kind = info.kind;
                rows.push(self.plugin_source_row(
                    target,
                    &LibraryPlugin {
                        id: info.auris_id(),
                        name: info.name.clone(),
                        description: info.vendor.clone(),
                    },
                    match kind {
                        PluginKind::Instrument => LibraryRole::Instrument,
                        PluginKind::Effect => LibraryRole::Effect,
                    },
                    info.category,
                    Some(PartSource::Vst3 {
                        path: file.clone(),
                        class_id: class_id.clone(),
                    }),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if target == LibraryTarget::SongPart {
                            this.choose_song_library_source(PartSource::Vst3 {
                                path: file.clone(),
                                class_id: class_id.clone(),
                            });
                            cx.notify();
                            return;
                        }
                        match kind {
                            PluginKind::Instrument => {
                                this.set_vst3_instrument_on_selection(&file, &class_id)
                            }
                            PluginKind::Effect => {
                                this.add_vst3_effect_to_selection(&file, &class_id)
                            }
                        }
                        cx.notify();
                    }),
                ));
            }
        }
        rows
    }

    /// The `.clap` files installed on this machine, scanned once.
    fn clap_files(&mut self) -> &[std::path::PathBuf] {
        self.clap_files.get_or_insert_with(|| {
            self.session
                .installed_clap_files(&self.settings.plugin_paths)
        })
    }

    /// What one `.clap` file holds, loading it the first time and remembering after that.
    ///
    /// An empty answer is cached too. A file that cannot be read is a file that will not become
    /// readable while the window is open, and retrying it on every frame would mean trying to
    /// load a broken binary sixty times a second.
    fn clap_plugins_in(&mut self, file: &std::path::Path) -> Vec<auris_session::ClapPluginInfo> {
        if let Some(known) = self.clap_contents.get(file) {
            return known.clone();
        }
        let listed = self
            .session
            .hosted_plugins_in(file)
            .unwrap_or_else(|error| {
                log::warn!("cannot read `{}`: {error}", file.display());
                Vec::new()
            });
        self.clap_contents
            .insert(file.to_path_buf(), listed.clone());
        listed
    }

    /// The `.vst3` bundles installed on this machine, scanned once without loading them.
    fn vst3_files(&mut self) -> &[std::path::PathBuf] {
        self.vst3_files.get_or_insert_with(|| {
            self.session
                .installed_vst3_files(&self.settings.plugin_paths)
        })
    }

    /// The audio classes exported by one VST3 bundle.
    fn vst3_plugins_in(&mut self, file: &std::path::Path) -> Vec<auris_session::Vst3PluginInfo> {
        if let Some(known) = self.vst3_contents.get(file) {
            return known.clone();
        }
        let listed = self.session.vst3_plugins_in(file).unwrap_or_else(|error| {
            log::warn!("cannot read VST3 `{}`: {error}", file.display());
            Vec::new()
        });
        self.vst3_contents
            .insert(file.to_path_buf(), listed.clone());
        listed
    }

    /// The instruments section: every registered instrument, under its category.
    fn instrument_rows(
        &mut self,
        target: LibraryTarget,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let plugins: Vec<(PluginCategory, LibraryPlugin)> = self
            .registry()
            .instruments()
            .filter(|descriptor| target == LibraryTarget::Track || descriptor.id != SAMPLER_ID)
            .map(|d| {
                (
                    d.category,
                    LibraryPlugin {
                        id: d.id.to_string(),
                        name: d.name.to_string(),
                        description: d.description.to_string(),
                    },
                )
            })
            .collect();
        let groups = by_category(plugins);

        let mut rows = vec![self.section_row(
            target,
            Branch::Instruments,
            Key::BrowserInstruments,
            Icon::Keyboard,
            groups.iter().map(|(_, members)| members.len()).sum(),
            cx,
        )];
        if !self.library_tree(target).is_open(Branch::Instruments) {
            return rows;
        }
        // What a click on a *plugin* does, said under the heading rather than on it. And when
        // there is no instrument track to put one on, why the clicks are about to do nothing —
        // this string has existed since the panel was written and was referenced nowhere.
        if target == LibraryTarget::Track {
            rows.push(self.note_row(
                1,
                self.t(if self.selected_track_takes_an_instrument() {
                    Key::BrowserInstrumentsHint
                } else {
                    Key::LibraryNeedsInstrumentTrack
                }),
            ));
        }
        for (category, members) in groups {
            let branch = Branch::InstrumentCategory(category);
            rows.push(self.category_row(target, branch, category, members.len(), cx));
            if !self.library_tree(target).is_open(branch) {
                continue;
            }
            for plugin in members {
                let id = plugin.id.clone();
                rows.push(self.plugin_row(
                    target,
                    &plugin,
                    LibraryRole::Instrument,
                    category,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.choose_library_instrument(target, &id);
                        cx.notify();
                    }),
                ));
            }
        }
        rows
    }

    /// The effects section: every registered effect, under its category.
    fn effect_rows(
        &mut self,
        target: LibraryTarget,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let plugins: Vec<(PluginCategory, LibraryPlugin)> = self
            .registry()
            .effects()
            .map(|d| {
                (
                    d.category,
                    LibraryPlugin {
                        id: d.id.to_string(),
                        name: d.name.to_string(),
                        description: d.description.to_string(),
                    },
                )
            })
            .collect();
        let groups = by_category(plugins);

        let mut rows = vec![self.section_row(
            target,
            Branch::Effects,
            Key::BrowserEffects,
            Icon::Knob,
            groups.iter().map(|(_, members)| members.len()).sum(),
            cx,
        )];
        if !self.library_tree(target).is_open(Branch::Effects) {
            return rows;
        }
        // An effect with no track selected lands on the master bus, which is a reasonable
        // default and was a completely silent one.
        rows.push(self.note_row(
            1,
            self.t(if self.selected_track.is_some() {
                Key::BrowserEffectsHint
            } else {
                Key::LibraryNeedsTrack
            }),
        ));
        for (category, members) in groups {
            let branch = Branch::EffectCategory(category);
            rows.push(self.category_row(target, branch, category, members.len(), cx));
            if !self.library_tree(target).is_open(branch) {
                continue;
            }
            for plugin in members {
                let id = plugin.id.clone();
                rows.push(self.plugin_row(
                    target,
                    &plugin,
                    LibraryRole::Effect,
                    category,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.add_effect_to_selection(&id);
                        cx.notify();
                    }),
                ));
            }
        }
        rows
    }

    /// The SoundFont section: every imported font, its banks, and their sounds.
    ///
    /// A font is a shelf rather than a plugin — importing one adds nothing to the arrangement, so
    /// this is where its contents become reachable.
    fn soundfont_rows(
        &mut self,
        target: LibraryTarget,
        row_offset: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let fonts = self.library_fonts(target);

        let mut rows = vec![self.section_row(
            target,
            Branch::SoundFonts,
            Key::BrowserSoundFonts,
            Icon::Wave,
            fonts.len(),
            cx,
        )];
        if !self.library_tree(target).is_open(Branch::SoundFonts) {
            return rows;
        }
        if fonts.is_empty() {
            rows.push(self.note_row(1, self.t(Key::BrowserNoSoundFonts)));
            return rows;
        }

        for (id, name) in fonts {
            let loaded = self.library_font_loaded(target, id);
            let branch = Branch::Font(id);
            self.scroll_library_branch(target, branch, row_offset + rows.len());
            let open = loaded && self.library_tree(target).is_open(branch);
            // A font whose file has gone keeps its row — that is how somebody finds out it has
            // gone — but it is drawn muted and has nothing to open.
            let detail = if loaded {
                // Counted rather than listed. Building every font's presets each frame would
                // sort a few hundred strings to show a number.
                if target == LibraryTarget::SongPart
                    && let Some(font) = self.song_library_fonts.iter().find(|font| font.id == id)
                {
                    font.presets.len().to_string()
                } else {
                    self.session.soundfont_preset_count(id).to_string()
                }
            } else {
                self.t(Key::BrowserFontFileMissing).to_string()
            };
            rows.push(
                self.branch_row(
                    target,
                    ("lib-font", id.0 as usize),
                    1,
                    open,
                    loaded,
                    None,
                    name,
                    detail,
                    self.row_style(if loaded { theme.text } else { theme.text_muted }, None),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_library_branch(target, branch, !open);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
            if !open {
                continue;
            }

            let banks = by_bank(self.library_presets(target, id));
            if banks.is_empty() {
                rows.push(self.note_row(2, self.t(Key::BrowserFontHasNoSounds)));
                continue;
            }
            // A font that declares one bank has that bank open with it: the row would otherwise
            // stand between somebody and the only thing they could possibly have wanted.
            let solitary = banks.len() == 1;
            for (bank, presets) in banks {
                let branch = Branch::Bank(id, bank);
                let open = self.library_tree(target).is_open_or(branch, solitary);
                rows.push(
                    self.branch_row(
                        target,
                        gpui::SharedString::from(format!("lib-bank-{}-{bank}", id.0)),
                        2,
                        open,
                        true,
                        None,
                        self.bank_label(bank),
                        presets.len().to_string(),
                        self.row_style(theme.text, None),
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.set_library_branch(target, branch, !open);
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
                );
                if !open {
                    continue;
                }
                for preset in presets {
                    let choice = PresetRef {
                        font: id,
                        bank: preset.bank,
                        patch: preset.patch,
                    };
                    rows.push(self.preset_row(
                        target,
                        &preset,
                        choice,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_library_preset(target, choice);
                            cx.notify();
                        }),
                    ));
                }
            }
        }
        rows
    }

    /// What a bank of a font is called: its number, or the name MIDI gives bank 128.
    fn bank_label(&self, bank: i32) -> String {
        if bank == PERCUSSION_BANK {
            self.t(Key::BrowserPercussionBank).to_string()
        } else {
            format!("{} {bank}", self.t(Key::BrowserBank))
        }
    }

    /// One of the top-level sections.
    fn section_row(
        &self,
        target: LibraryTarget,
        branch: Branch,
        label: Key,
        kind: Icon,
        count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let open = self.library_tree(target).is_open(branch);
        self.branch_row(
            target,
            ("lib-section", branch_key(branch)),
            0,
            open,
            true,
            Some(kind),
            self.t(label).to_string(),
            count.to_string(),
            // The heading also serves as a labelled key to the item colours.
            self.row_style(
                theme.text,
                Some(match branch {
                    Branch::Instruments | Branch::SoundFonts => LibraryRole::Instrument,
                    Branch::Effects => LibraryRole::Effect,
                    Branch::Voices => LibraryRole::Voice,
                    _ => LibraryRole::File,
                }),
            ),
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.set_library_branch(target, branch, !open);
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// One category of plugin.
    fn category_row(
        &self,
        target: LibraryTarget,
        branch: Branch,
        category: PluginCategory,
        count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let open = self.library_tree(target).is_open(branch);
        self.branch_row(
            target,
            ("lib-category", branch_key(branch)),
            1,
            open,
            true,
            None,
            self.category_label(category),
            count.to_string(),
            self.row_style(
                theme.text,
                Some(if matches!(branch, Branch::InstrumentCategory(_)) {
                    if category == PluginCategory::Drum {
                        LibraryRole::Drum
                    } else {
                        LibraryRole::Instrument
                    }
                } else {
                    LibraryRole::Effect
                }),
            ),
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.set_library_branch(target, branch, !open);
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// How a row is drawn: what colour its name is, and which group it belongs to.
    ///
    /// Two fields rather than two arguments, because [`Self::branch_row`] already carries as many
    /// as anybody can read.
    fn row_style(&self, label: gpui::Hsla, accent: Option<LibraryRole>) -> RowStyle {
        RowStyle {
            label,
            accent: accent.map(|role| role.color(&self.theme)),
        }
    }

    /// A branch: a disclosure triangle, a name, and how much is inside.
    #[allow(clippy::too_many_arguments)]
    fn branch_row<I, F>(
        &self,
        target: LibraryTarget,
        id: I,
        depth: usize,
        open: bool,
        enabled: bool,
        kind: Option<Icon>,
        label: String,
        detail: String,
        style: RowStyle,
        on_click: F,
    ) -> impl IntoElement + use<I, F>
    where
        I: Into<gpui::ElementId>,
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        let label_color = style.label;
        let tooltip = crate::ui::tooltip::keyed_tip(format!("{label} · {detail}"), "", &theme);
        div()
            .id(id.into())
            .debug_selector({
                let label = label.clone();
                move || target.selector(&format!("lib-branch-{label}"))
            })
            .flex()
            .flex_shrink_0()
            .min_w_0()
            .items_center()
            .gap_1p5()
            .pl(indent(depth))
            .pr_1p5()
            .py_1()
            .min_h(px(30.0))
            .rounded(Metrics::RADIUS_SM)
            .when(depth == 0, |this| this.bg(theme.surface_raised).mt_1())
            .tooltip(tooltip)
            // A branch with nothing to open — a font whose file has gone — must not offer the
            // pointer and the hover fill of a row that would answer a click. It looked exactly
            // like a font that works, and clicking it did nothing for ever.
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(theme.surface_hover))
            })
            .child(div().flex_shrink_0().child(crate::ui::icons::icon(
                // A shut branch points at what opening it would reveal, an open one down at what
                // it has revealed. The two rotations are the whole of the affordance.
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                px(11.0),
                // A branch with nothing to open — a font whose file has gone — keeps its
                // triangle so the row still reads as a branch, faintly, so it does not invite.
                if enabled {
                    theme.text_muted
                } else {
                    theme.text_faint
                },
            )))
            .child(
                // Reserve the same icon slot on every branch so names follow the tree depth.
                div()
                    .size(px(14.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|slot| match kind {
                        Some(kind) => slot.child(icon(
                            kind,
                            px(14.0),
                            style.accent.unwrap_or(theme.text_muted),
                        )),
                        None => {
                            slot.when_some(style.accent, |slot, color| slot.child(swatch(color)))
                        }
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(if depth == 0 { px(14.0) } else { px(13.0) })
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(label_color)
                    .truncate()
                    .child(label),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(detail),
            )
            .when(enabled, |this| {
                this.on_mouse_down(gpui::MouseButton::Left, on_click)
            })
    }

    /// One plugin, ready to load onto the selected track.
    ///
    /// The summary sits below the name so both can use the panel's width. The tooltip carries
    /// their full text when either line is truncated.
    fn plugin_row<F>(
        &self,
        target: LibraryTarget,
        plugin: &LibraryPlugin,
        role: LibraryRole,
        category: PluginCategory,
        on_click: F,
    ) -> AnyElement
    where
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        self.plugin_source_row(target, plugin, role, category, None, on_click)
    }

    #[allow(clippy::too_many_arguments)]
    fn plugin_source_row<F>(
        &self,
        target: LibraryTarget,
        plugin: &LibraryPlugin,
        role: LibraryRole,
        category: PluginCategory,
        source: Option<PartSource>,
        on_click: F,
    ) -> AnyElement
    where
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let source =
            source.map(|source| self.session.resolve_song_source(&source).unwrap_or(source));
        let theme = self.theme.clone();
        let accent = if role == LibraryRole::Instrument && category == PluginCategory::Drum {
            LibraryRole::Drum
        } else {
            role
        }
        .color(&theme);
        let searching = !self.library_query(target).content().trim().is_empty();
        let enabled = role != LibraryRole::Instrument || self.library_takes_instrument(target);
        let selected = role == LibraryRole::Instrument
            && match target {
                LibraryTarget::Track => self
                    .selected_track
                    .and_then(|id| self.project().track(id))
                    .and_then(|track| track.kind.as_instrument())
                    .is_some_and(|track| track.instrument_id == plugin.id),
                LibraryTarget::SongPart => self.library_song_part().is_some_and(|part| {
                    self.library_song_source() == source
                        && (source.is_some()
                            || (part.source.is_none()
                                && part.program.is_none()
                                && part.instrument == plugin.id))
                }),
            };
        let name = audio_name(self, &plugin.name);
        let description = self.plugin_description(&plugin.description);
        let description = if searching && role != LibraryRole::File {
            format!("{} · {description}", self.category_label(category))
        } else {
            description
        };
        let target_hint = if enabled {
            ""
        } else {
            self.t(Key::LibraryNeedsInstrumentTrack)
        };
        let tooltip = crate::ui::tooltip::keyed_tip(
            format!("{name} · {description} {target_hint}"),
            "",
            &theme,
        );
        div()
            .id(target.element_id(&format!("lib-{}", plugin.id)))
            .debug_selector({
                let id = plugin.id.clone();
                move || target.selector(&format!("lib-{id}"))
            })
            .flex()
            .flex_shrink_0()
            .min_w_0()
            .items_center()
            .gap_1p5()
            .pl(if searching {
                indent(0)
            } else {
                indent(2) + DISCLOSURE_SPACE
            })
            .pr_1p5()
            .py_1()
            .rounded(Metrics::RADIUS_SM)
            .when(selected, |this| this.bg(theme.surface_raised))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(theme.surface_hover))
            })
            .tooltip(tooltip)
            .child(div().flex_shrink_0().child(icon(
                role.icon(),
                px(14.0),
                if enabled { accent } else { theme.text_muted },
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .truncate()
                            .child(name),
                    )
                    .when(!description.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(description),
                        )
                    }),
            )
            .when(selected, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .child(icon(Icon::Check, px(14.0), accent)),
                )
            })
            .when(enabled, |this| {
                this.on_mouse_down(gpui::MouseButton::Left, on_click)
            })
            .into_any_element()
    }

    /// One sound of an open bank, named as the font names it and numbered as MIDI does.
    fn preset_row<F>(
        &self,
        target: LibraryTarget,
        preset: &SoundFontPreset,
        choice: PresetRef,
        on_click: F,
    ) -> AnyElement
    where
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        let accent = if choice.bank == PERCUSSION_BANK {
            LibraryRole::Drum
        } else {
            LibraryRole::Instrument
        }
        .color(&theme);
        let searching = !self.library_query(target).content().trim().is_empty();
        let enabled = self.library_takes_instrument(target);
        let selected = match target {
            LibraryTarget::Track => {
                self.selected_track
                    .and_then(|id| self.session.track_preset(id))
                    == Some(choice)
            }
            LibraryTarget::SongPart => self.library_song_part().is_some_and(|part| {
                matches!(&part.source, Some(PartSource::SoundFont { bank, patch, .. })
                    if *bank == choice.bank && *patch == choice.patch)
                    && self.library_song_source() == self.library_preset_source(choice)
            }),
        };
        let source = self
            .library_fonts(target)
            .into_iter()
            .find(|(id, _)| *id == choice.font)
            .map(|(_, name)| name)
            .unwrap_or_default();
        let detail = format!(
            "{source} · {} · {}",
            self.bank_label(choice.bank),
            choice.patch
        );
        let target_hint = if enabled {
            ""
        } else {
            self.t(Key::LibraryNeedsInstrumentTrack)
        };
        let tooltip = crate::ui::tooltip::keyed_tip(
            format!("{} · {detail} {target_hint}", preset.name),
            "",
            &theme,
        );
        div()
            .id(target.element_id(&format!(
                "lib-preset-{}-{}-{}",
                choice.font.0, choice.bank, choice.patch
            )))
            .debug_selector(move || {
                target.selector(&format!(
                    "lib-preset-{}-{}-{}",
                    choice.font.0, choice.bank, choice.patch
                ))
            })
            .flex()
            .flex_shrink_0()
            .min_w_0()
            .items_center()
            .gap_1p5()
            .pl(if searching {
                indent(0)
            } else {
                indent(3) + DISCLOSURE_SPACE
            })
            .pr_1p5()
            .py_1()
            .min_h(px(28.0))
            .rounded(Metrics::RADIUS_SM)
            .when(selected, |this| this.bg(theme.surface_raised))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(theme.surface_hover))
            })
            .tooltip(tooltip)
            .child(div().flex_shrink_0().child(icon(
                Icon::Wave,
                px(14.0),
                if enabled { accent } else { theme.text_muted },
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .truncate()
                            .child(preset.name.clone()),
                    )
                    .when(searching, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(detail),
                        )
                    }),
            )
            .child(
                // The patch number alone: the bank is the row this one is sitting under.
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(preset.patch.to_string()),
            )
            .when(selected, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .child(icon(Icon::Check, px(14.0), accent)),
                )
            })
            .when(enabled, |this| {
                this.on_mouse_down(gpui::MouseButton::Left, on_click)
            })
            .into_any_element()
    }

    /// Whether the selected track is one an instrument can be loaded onto.
    ///
    /// An audio track has no instrument, so clicking a sound with one selected does nothing at
    /// all — which is a thing worth saying rather than a click to be swallowed.
    fn selected_track_takes_an_instrument(&self) -> bool {
        self.selected_track
            .and_then(|id| self.project().track(id))
            .is_some_and(|track| track.kind.as_instrument().is_some())
    }

    /// A short instruction or empty-state explanation beneath a branch.
    fn note_row(&self, depth: usize, text: &str) -> AnyElement {
        div()
            .flex_shrink_0()
            .text_xs()
            .text_color(self.theme.text_muted)
            .pl(indent(depth))
            .pr_1p5()
            .py_1()
            .child(text.to_string())
            .into_any_element()
    }
}

/// MIDI's percussion bank, which a General MIDI font keeps its kits in.
const PERCUSSION_BANK: i32 = 128;

/// A stable per-branch element key, so gpui tracks hover across frames as branches open and shut.
///
/// Keyed by what the branch *is* rather than by where it currently sits in the list. A running
/// index would move every row below whichever branch was opened, which is exactly the frame the
/// pointer is still over one of them.
fn branch_key(branch: Branch) -> usize {
    match branch {
        Branch::Instruments => 0,
        Branch::SoundFonts => 1,
        Branch::Effects => 2,
        Branch::InstrumentCategory(category) => 3 + browser_order(category),
        Branch::EffectCategory(category) => 3 + PluginCategory::ALL.len() + browser_order(category),
        // Fonts and banks have ids of their own and are keyed by those instead; this is only
        // reached if one is ever passed here, and a constant is better than a collision.
        Branch::Font(_) | Branch::Bank(..) => 3 + 2 * PluginCategory::ALL.len(),
        Branch::Plugins => 4 + 2 * PluginCategory::ALL.len(),
        Branch::PluginFile(index) => 5 + 2 * PluginCategory::ALL.len() + index,
        // Far past anything the plugin-file counter reaches; the key only has to be stable.
        Branch::Voices => 1_000_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_roles_follow_each_theme_and_remain_distinct() {
        let roles = [
            LibraryRole::Instrument,
            LibraryRole::Drum,
            LibraryRole::Effect,
            LibraryRole::Voice,
        ];
        for (index, scheme) in crate::theme::SCHEMES.iter().enumerate() {
            let theme = Theme::from_scheme(scheme);
            let colors = roles.map(|role| role.color(&theme));
            for (role_index, color) in colors.iter().enumerate() {
                assert!(!colors[..role_index].contains(color));
                assert!(crate::theme::contrast_ratio(*color, theme.surface_hover) >= 3.0);
                for other in &crate::theme::SCHEMES[..index] {
                    assert_ne!(*color, roles[role_index].color(&Theme::from_scheme(other)));
                }
            }
            assert_eq!(LibraryRole::File.color(&theme), theme.text_muted);
        }
    }

    #[test]
    fn custom_library_colours_match_track_and_clip_palette_slots() {
        let mut scheme = *crate::theme::scheme_or_default(crate::theme::DEFAULT_SCHEME);
        scheme.track_palette = std::array::from_fn(|i| Some(0x204060 + i as u32 * 0x102030));
        let theme = Theme::from_scheme(&scheme);
        for (role, slot) in [
            (LibraryRole::Instrument, Color::INSTRUMENT),
            (LibraryRole::Drum, Color::DRUM),
            (LibraryRole::Effect, Color::BUS),
            (LibraryRole::Voice, Color::SINGER),
        ] {
            assert_eq!(role.color(&theme), theme.track_color(slot.0));
        }
    }

    fn plugin(id: &str) -> LibraryPlugin {
        LibraryPlugin {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
        }
    }

    #[test]
    fn categories_are_listed_in_the_browser_order_and_never_empty() {
        // Registry order is by id, which puts Utility before Synth as easily as after it. The
        // browser's own order is what a musician reads down.
        let groups = by_category(vec![
            (PluginCategory::Utility, plugin("auris.fx.gain")),
            (PluginCategory::Synth, plugin("auris.synth.chiptune")),
            (PluginCategory::Reverb, plugin("auris.fx.reverb")),
            (PluginCategory::Synth, plugin("auris.synth.fm2")),
        ]);
        let order: Vec<PluginCategory> = groups.iter().map(|(category, _)| *category).collect();
        assert_eq!(
            order,
            vec![
                PluginCategory::Synth,
                PluginCategory::Reverb,
                PluginCategory::Utility
            ]
        );
        // A category nobody registered under is not a row saying so.
        assert!(!order.contains(&PluginCategory::Drum));
        // Within a group, the order they arrived in.
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec!["auris.synth.chiptune", "auris.synth.fm2"]
        );
    }

    #[test]
    fn nothing_registered_gives_no_groups_at_all() {
        assert!(by_category(Vec::new()).is_empty());
    }

    #[test]
    fn a_font_is_split_into_the_banks_it_declares() {
        let sound = |bank, patch| SoundFontPreset {
            bank,
            patch,
            name: format!("{bank}:{patch}"),
        };
        let banks = by_bank(vec![
            sound(128, 0),
            sound(0, 1),
            sound(0, 0),
            sound(128, 8),
            sound(8, 4),
        ]);
        assert_eq!(
            banks.iter().map(|(bank, _)| *bank).collect::<Vec<_>>(),
            vec![0, 8, 128]
        );
        assert_eq!(banks[0].1.len(), 2);
        assert_eq!(banks[2].1.len(), 2);
        // Order within a bank is the order the session gave them, which is by patch.
        assert_eq!(
            banks[0].1.iter().map(|p| p.patch).collect::<Vec<_>>(),
            vec![1, 0]
        );
    }

    #[test]
    fn a_font_that_uses_one_bank_still_gets_one() {
        // Splitting is by what the file declares, so a font that is not General MIDI is not
        // forced into GM's two banks — it simply has the one row.
        let banks = by_bank(vec![SoundFontPreset {
            bank: 0,
            patch: 0,
            name: "Only".into(),
        }]);
        assert_eq!(banks.len(), 1);
        assert_eq!(banks[0].0, 0);
        assert!(by_bank(Vec::new()).is_empty());
    }

    #[test]
    fn the_plugins_start_visible_and_a_font_s_hundred_sounds_do_not() {
        let tree = LibraryTree::default();
        assert!(tree.is_open(Branch::Instruments));
        assert!(tree.is_open(Branch::Effects));
        assert!(tree.is_open(Branch::SoundFonts));
        assert!(tree.is_open(Branch::EffectCategory(PluginCategory::Reverb)));
        assert!(!tree.is_open(Branch::Font(SoundFontId(1))));
        assert!(!tree.is_open(Branch::Bank(SoundFontId(1), 0)));
    }

    #[test]
    fn a_font_s_only_bank_opens_with_it_and_can_still_be_shut() {
        let mut tree = LibraryTree::default();
        let only = Branch::Bank(SoundFontId(1), 0);
        let one_of_two = Branch::Bank(SoundFontId(2), 0);

        assert!(tree.is_open_or(only, true));
        assert!(!tree.is_open_or(one_of_two, false));

        // Clicking a row drawn open shuts it in one click. Asking for a toggle instead would
        // consult `opens_by_default`, which says shut, and spend that click reaching the state
        // the row was already drawn in.
        tree.set_open(only, false);
        assert!(!tree.is_open_or(only, true));
    }

    #[test]
    fn a_branch_that_has_been_clicked_stays_where_it_was_put() {
        let mut tree = LibraryTree::default();
        let font = Branch::Font(SoundFontId(3));

        tree.set_open(font, true);
        assert!(tree.is_open(font));
        // A second font is unaffected: one open font does not shut another, which is what the
        // single `expanded_font` this replaced could not express.
        assert!(!tree.is_open(Branch::Font(SoundFontId(4))));

        tree.set_open(font, false);
        assert!(!tree.is_open(font));
        // Shutting something that opens by default has to stick, or the click does nothing.
        tree.set_open(Branch::Instruments, false);
        assert!(!tree.is_open(Branch::Instruments));

        // Importing a font opens it outright rather than flipping it, so importing twice is
        // never a command to shut the font that was just added.
        tree.set_open(font, true);
        tree.set_open(font, true);
        assert!(tree.is_open(font));
    }

    #[test]
    fn a_plugin_rescan_forgets_only_file_disclosures() {
        let mut tree = LibraryTree::default();
        tree.set_open(Branch::PluginFile(0), true);
        tree.set_open(Branch::Instruments, false);

        tree.forget_plugin_files();

        assert!(!tree.is_open(Branch::PluginFile(0)));
        assert!(!tree.is_open(Branch::Instruments));
    }

    #[test]
    fn every_branch_has_a_key_of_its_own() {
        // Two branches sharing a key would share hover state — one row lighting up because the
        // pointer is over a different one.
        let mut keys: Vec<usize> = vec![Branch::Instruments, Branch::SoundFonts, Branch::Effects]
            .into_iter()
            .chain(
                PluginCategory::ALL
                    .into_iter()
                    .map(Branch::InstrumentCategory),
            )
            .chain(PluginCategory::ALL.into_iter().map(Branch::EffectCategory))
            .map(branch_key)
            .collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(count, keys.len());
    }

    #[test]
    fn a_query_finds_what_it_names_and_puts_the_closest_first() {
        let entries = vec![
            ("Analogue Reverb".to_string(), 1),
            ("Reverb".to_string(), 2),
            ("Saw Bass".to_string(), 3),
        ];
        // Both contain the letters; the shorter name that starts with them wins.
        assert_eq!(best_matches(entries, "revb", 10), vec![2, 1]);
    }

    #[test]
    fn a_query_nothing_answers_to_finds_nothing() {
        let entries = vec![("Reverb".to_string(), 1)];
        assert!(best_matches(entries, "zzz", 10).is_empty());
    }

    #[test]
    fn displayed_japanese_plugin_names_and_original_names_find_the_same_plugin() {
        let registry = auris_session::plugin_catalogue();
        let entries: Vec<(String, String)> = registry
            .instruments()
            .chain(registry.effects())
            .map(|descriptor| {
                (
                    plugin_search_name(&descriptor.name, auris_i18n::Language::Japanese),
                    descriptor.name.to_string(),
                )
            })
            .collect();
        for query in ["リバーブ", "Reverb", "revb"] {
            assert_eq!(
                best_matches(entries.clone(), query, SEARCH_LIMIT).first(),
                Some(&"Reverb".to_string()),
                "{query:?} should find the name shown in the browser"
            );
        }
        assert_eq!(
            plugin_search_name("Reverb", auris_i18n::Language::English),
            "Reverb"
        );
        assert_eq!(
            plugin_search_name("Untranslated Plugin", auris_i18n::Language::Japanese),
            "Untranslated Plugin"
        );
    }

    #[test]
    fn the_result_list_stops_where_it_stops_being_useful() {
        // Forty rows is already more than anybody reads; two hundred is an answer that has
        // answered nothing. Anybody who cannot see it types another letter.
        let entries: Vec<(String, usize)> = (0..200).map(|n| (format!("Delay {n}"), n)).collect();
        assert_eq!(
            best_matches(entries, "delay", SEARCH_LIMIT).len(),
            SEARCH_LIMIT
        );
    }

    #[test]
    fn an_empty_query_keeps_the_order_the_tree_shows() {
        // Not that the panel ever asks — an empty query draws the tree — but a tie-break that
        // scrambled the list would scramble it for a one-letter query too.
        let entries = vec![
            ("A".to_string(), 1),
            ("B".to_string(), 2),
            ("C".to_string(), 3),
        ];
        assert_eq!(best_matches(entries, "", 10), vec![1, 2, 3]);
    }
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod interaction_tests;
