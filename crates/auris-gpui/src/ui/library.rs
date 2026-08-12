//! The left-hand library: every instrument, sound and effect the session can reach, as a tree.
//!
//! Three sections, each of which opens into groups rather than into a list. Instruments and
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
//! # Why the rows are coloured
//!
//! Opening a branch is not the whole of the scale problem. A bank of a General MIDI font is a
//! hundred and twenty-eight rows of small grey text at one indent, and grouping alone does not
//! make that a thing an eye can find a place in — every row looks exactly like the row above it,
//! so finding the strings means reading names one at a time from the top.
//!
//! So every leaf row carries a mark in its group's colour, and the marks line up into a column
//! that says where one group ends and the next begins without a word being read. [`group_hue`] is
//! where the colours come from and [`Theme::group_color`](crate::theme::Theme::group_color) turns
//! one into a colour this scheme can show. The mark is beside the name and never *is* the name:
//! the hues that make the best labels make the worst body text, and a row whose meaning depends on
//! its colour is a row somebody colour-blind cannot read at all.

use std::collections::HashMap;

use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, IntoElement, MouseDownEvent, Pixels, Window, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::inspector::{audio_name, panel_header};
use crate::ui::widgets::divider;

/// How far one level of the tree is indented.
const INDENT: Pixels = px(11.0);

/// A branch of the library — anything that can be open or shut.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Branch {
    /// The instruments section.
    Instruments,
    /// One category of instrument.
    InstrumentCategory(PluginCategory),
    /// The SoundFonts section.
    SoundFonts,
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
    INDENT * depth as f32
}

/// Where the first group's colour sits on the wheel.
///
/// Off pure red, which the interface already spends on clipping and on failure, and off the
/// accent, which every scheme puts somewhere in the blues.
const FIRST_HUE: f32 = 0.08;

/// Where group `index` of `count` sits on the colour wheel.
///
/// An even walk rather than a hand-picked list. Picking by hand reads better for the first six and
/// then turns into a search for a seventh colour that is not one of the six — which is the point
/// at which somebody picks two that are nearly the same. Spreading them evenly is the arrangement
/// that makes the smallest gap as large as it can be, and it is the only one that stays true when
/// a category is added.
fn group_hue(index: usize, count: usize) -> f32 {
    if count == 0 {
        return FIRST_HUE;
    }
    FIRST_HUE + index as f32 / count as f32
}

/// The colour that stands for a category of plugin.
///
/// Instruments and effects walk one wheel between them rather than one each, so no reverb shares a
/// hue with a synth. They are in different sections, but the sections are one scrolling column.
fn category_hue(category: PluginCategory) -> f32 {
    group_hue(browser_order(category), PluginCategory::ALL.len())
}

/// How many of a font's sounds share one colour band.
///
/// General MIDI's own division: the hundred and twenty-eight programs come in sixteen families of
/// eight — Piano, Organ, Guitar, Bass — which is the grouping every chord chart and every hardware
/// panel a musician has seen already uses. A font that is not General MIDI gets bands of eight
/// standing for nothing in particular, and they still do the half of this that matters: a column
/// of a hundred and twenty-eight names is unreadable, and the same column in bands is not.
const BAND: i32 = 8;

/// The colour that stands for one sound of a font.
///
/// The percussion bank is one band rather than sixteen. Its patches are kits — 0, 8, 16, 24 — so
/// dividing them by eight would give each of the handful a colour of its own and claim they are as
/// far apart as a piano is from a trumpet. It gets a seventeenth band instead of one of the
/// families' so that a font with both banks open never draws a kit in a melodic family's colour.
fn preset_hue(bank: i32, patch: i32) -> f32 {
    let families = gm::FAMILIES.len();
    let band = if bank == PERCUSSION_BANK {
        families
    } else {
        // Wrapped, so a font declaring patches past the General MIDI range stays inside the
        // melodic wheel instead of landing on the percussion colour.
        (patch.max(0) / BAND) as usize % families
    };
    group_hue(band, families + 1)
}

/// The mark that carries a row's group colour.
///
/// A bar rather than a dot, and at the head of the row rather than beside the name: the bars line
/// up into a column, and a column is what an eye runs down. Dots at varying indents would not.
fn swatch(color: gpui::Hsla) -> impl IntoElement {
    div().w(px(3.0)).h(px(10.0)).rounded(px(1.5)).bg(color)
}

/// The gap a row with no mark leaves where one would have been, so its name still lines up with
/// the names of the rows that have one.
fn no_swatch() -> impl IntoElement {
    div().w(px(3.0))
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
        let rows = self.library_rows(cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .child(panel_header(self.t(Key::Library), &theme))
            .child(
                // No gap between the rows and half the padding round them. A browser is a list
                // to run an eye down, and every pixel of air between two names is a name that
                // did not fit on the screen — this used to show eleven rows where it now shows
                // twenty-odd.
                div()
                    .id("library-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
    }

    /// The whole tree, flattened into the rows that are currently visible.
    ///
    /// A shut branch contributes its own row and nothing else, so the cost of a font with a
    /// hundred and twenty-eight sounds is not paid until somebody opens it.
    fn library_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows = self.instrument_rows(cx);
        rows.push(divider(&theme).into_any_element());
        rows.extend(self.soundfont_rows(cx));
        rows.push(divider(&theme).into_any_element());
        rows.extend(self.effect_rows(cx));
        rows.push(divider(&theme).into_any_element());
        rows.extend(self.installed_plugin_rows(cx));
        rows
    }

    /// The installed CLAP plugins section: the files found on this machine, and what is in one
    /// once somebody opens it.
    ///
    /// Two levels rather than the categories the built-ins get, because the grouping that matters
    /// here is the *file*: it is what the document stores, what has to still be installed for the
    /// project to open, and the only thing known about a plugin before it is loaded.
    fn installed_plugin_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let files = self.clap_files().to_vec();

        let mut rows = vec![self.section_row(
            Branch::Plugins,
            Key::BrowserPlugins,
            Icon::Knob,
            files.len(),
            cx,
        )];
        if !self.library.is_open(Branch::Plugins) {
            return rows;
        }
        if files.is_empty() {
            rows.push(self.note_row(1, self.t(Key::BrowserNoPlugins)));
            return rows;
        }
        rows.push(self.note_row(1, self.t(Key::BrowserPluginsHint)));

        for (index, file) in files.iter().enumerate() {
            let branch = Branch::PluginFile(index);
            let open = self.library.is_open(branch);
            let name = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string_lossy().into_owned());

            let listed = match open {
                true => self.clap_plugins_in(file),
                false => Vec::new(),
            };
            let theme = self.theme.clone();
            rows.push(
                self.branch_row(
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
                    self.row_style(theme.text, Some(category_hue(PluginCategory::Other))),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.library.set_open(branch, !open);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
            if !open {
                continue;
            }
            if listed.is_empty() {
                rows.push(self.note_row(2, self.t(Key::BrowserPluginUnreadable)));
                continue;
            }

            for info in listed {
                let file = file.clone();
                let clap_id = info.clap_id.clone();
                let hosted = info.kind == PluginKind::Effect;
                rows.push(self.plugin_row(
                    &LibraryPlugin {
                        id: info.auris_id(),
                        name: info.name.clone(),
                        // The vendor rather than a description: a hosted plugin's description is
                        // usually empty, and whose plugin it is answers the question a list of
                        // unfamiliar names actually raises.
                        description: match hosted {
                            true => info.vendor.clone(),
                            // The one thing a user must know before clicking, said on the row
                            // rather than in a dialog after it does nothing.
                            false => self.t(Key::BrowserPluginNotYetHosted).to_string(),
                        },
                    },
                    Icon::Knob,
                    info.category,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if hosted {
                            this.add_hosted_effect_to_selection(&file, &clap_id);
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
        self.clap_files
            .get_or_insert_with(|| self.session.installed_clap_files())
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

    /// The instruments section: every registered instrument, under its category.
    fn instrument_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let plugins: Vec<(PluginCategory, LibraryPlugin)> = self
            .registry()
            .instruments()
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
            Branch::Instruments,
            Key::BrowserInstruments,
            Icon::Keyboard,
            groups.iter().map(|(_, members)| members.len()).sum(),
            cx,
        )];
        if !self.library.is_open(Branch::Instruments) {
            return rows;
        }
        // What a click on a *plugin* does, said under the heading rather than on it. And when
        // there is no instrument track to put one on, why the clicks are about to do nothing —
        // this string has existed since the panel was written and was referenced nowhere.
        rows.push(self.note_row(
            1,
            self.t(if self.selected_track_takes_an_instrument() {
                Key::BrowserInstrumentsHint
            } else {
                Key::LibraryNeedsInstrumentTrack
            }),
        ));
        for (category, members) in groups {
            let branch = Branch::InstrumentCategory(category);
            rows.push(self.category_row(branch, category, members.len(), cx));
            if !self.library.is_open(branch) {
                continue;
            }
            for plugin in members {
                let id = plugin.id.clone();
                rows.push(self.plugin_row(
                    &plugin,
                    Icon::Keyboard,
                    category,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_track_instrument(&id);
                        cx.notify();
                    }),
                ));
            }
        }
        rows
    }

    /// The effects section: every registered effect, under its category.
    fn effect_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
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
            Branch::Effects,
            Key::BrowserEffects,
            Icon::Knob,
            groups.iter().map(|(_, members)| members.len()).sum(),
            cx,
        )];
        if !self.library.is_open(Branch::Effects) {
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
            rows.push(self.category_row(branch, category, members.len(), cx));
            if !self.library.is_open(branch) {
                continue;
            }
            for plugin in members {
                let id = plugin.id.clone();
                rows.push(self.plugin_row(
                    &plugin,
                    Icon::Knob,
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
    fn soundfont_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let fonts: Vec<(SoundFontId, String)> = self
            .session
            .soundfonts()
            .map(|font| (font.id, font.name.clone()))
            .collect();

        let mut rows = vec![self.section_row(
            Branch::SoundFonts,
            Key::BrowserSoundFonts,
            Icon::Wave,
            fonts.len(),
            cx,
        )];
        if !self.library.is_open(Branch::SoundFonts) {
            return rows;
        }
        if fonts.is_empty() {
            rows.push(self.note_row(1, self.t(Key::BrowserNoSoundFonts)));
            return rows;
        }

        for (id, name) in fonts {
            let loaded = self.session.soundfont_is_loaded(id);
            let branch = Branch::Font(id);
            let open = loaded && self.library.is_open(branch);
            // A font whose file has gone keeps its row — that is how somebody finds out it has
            // gone — but it is drawn muted and has nothing to open.
            let detail = if loaded {
                // Counted rather than listed. Building every font's presets each frame would
                // sort a few hundred strings to show a number.
                self.session.soundfont_preset_count(id).to_string()
            } else {
                self.t(Key::BrowserFontFileMissing).to_string()
            };
            rows.push(
                self.branch_row(
                    ("lib-font", id.0 as usize),
                    1,
                    open,
                    loaded,
                    None,
                    name,
                    detail,
                    self.row_style(if loaded { theme.text } else { theme.text_muted }, None),
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.library.set_open(branch, !open);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
            if !open {
                continue;
            }

            let banks = by_bank(self.session.soundfont_presets(id));
            if banks.is_empty() {
                rows.push(self.note_row(2, self.t(Key::BrowserFontHasNoSounds)));
                continue;
            }
            // A font that declares one bank has that bank open with it: the row would otherwise
            // stand between somebody and the only thing they could possibly have wanted.
            let solitary = banks.len() == 1;
            for (bank, presets) in banks {
                let branch = Branch::Bank(id, bank);
                let open = self.library.is_open_or(branch, solitary);
                rows.push(
                    self.branch_row(
                        gpui::SharedString::from(format!("lib-bank-{}-{bank}", id.0)),
                        2,
                        open,
                        true,
                        None,
                        self.bank_label(bank),
                        presets.len().to_string(),
                        self.row_style(theme.text, None),
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.library.set_open(branch, !open);
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
                        &preset,
                        choice,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.set_track_preset(choice);
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

    /// One of the three top-level sections.
    fn section_row(
        &self,
        branch: Branch,
        label: Key,
        kind: Icon,
        count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let open = self.library.is_open(branch);
        self.branch_row(
            ("lib-section", branch_key(branch)),
            0,
            open,
            true,
            Some(kind),
            self.t(label).to_string(),
            count.to_string(),
            // No mark: a section is the heading over the colours rather than one of them, and a
            // fourth colour above three groups would read as a fourth group.
            self.row_style(theme.text_muted, None),
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.library.set_open(branch, !open);
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// One category of plugin.
    fn category_row(
        &self,
        branch: Branch,
        category: PluginCategory,
        count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let open = self.library.is_open(branch);
        self.branch_row(
            ("lib-category", branch_key(branch)),
            1,
            open,
            true,
            None,
            self.category_label(category),
            count.to_string(),
            self.row_style(theme.text, Some(category_hue(category))),
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.library.set_open(branch, !open);
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// How a row is drawn: what colour its name is, and which group it belongs to.
    ///
    /// Two fields rather than two arguments, because [`Self::branch_row`] already carries as many
    /// as anybody can read.
    fn row_style(&self, label: gpui::Hsla, accent: Option<f32>) -> RowStyle {
        RowStyle {
            label,
            accent: accent.map(|hue| self.theme.group_color(hue)),
        }
    }

    /// A branch: a disclosure triangle, a name, and how much is inside.
    #[allow(clippy::too_many_arguments)]
    fn branch_row<I, F>(
        &self,
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
        div()
            .id(id.into())
            .flex()
            .items_center()
            .gap_1p5()
            .pl(indent(depth))
            .px_1p5()
            .py_1()
            .rounded(Metrics::RADIUS_SM)
            // A branch with nothing to open — a font whose file has gone — must not offer the
            // pointer and the hover fill of a row that would answer a click. It looked exactly
            // like a font that works, and clicking it did nothing for ever.
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(theme.surface_hover))
            })
            .map(|this| match style.accent {
                Some(color) => this.child(swatch(color)),
                None => this.child(no_swatch()),
            })
            .child(crate::ui::icons::icon(
                // A shut branch points at what opening it would reveal, an open one down at what
                // it has revealed. The two rotations are the whole of the affordance.
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                px(8.0),
                // A branch with nothing to open — a font whose file has gone — keeps its
                // triangle so the row still reads as a branch, faintly, so it does not invite.
                if enabled {
                    theme.text_muted
                } else {
                    theme.text_faint
                },
            ))
            .when_some(kind, |this, kind| {
                this.child(crate::ui::icons::icon(kind, px(11.0), theme.text_muted))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(label_color)
                    .truncate()
                    .child(label),
            )
            .child(div().text_xs().text_color(theme.text_muted).child(detail))
            .on_mouse_down(gpui::MouseButton::Left, on_click)
    }

    /// One plugin, ready to load onto the selected track.
    ///
    /// Name and summary on one line rather than two. The summary is worth having and is not worth
    /// doubling the height of every row in the panel for — set beside the name and allowed to run
    /// off the end, it costs nothing and still answers "which reverb is that one".
    fn plugin_row<F>(
        &self,
        plugin: &LibraryPlugin,
        kind: Icon,
        category: PluginCategory,
        on_click: F,
    ) -> AnyElement
    where
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        let accent = theme.group_color(category_hue(category));
        // The category is the heading above rather than a label on the row: it was on every row
        // when the list was flat, and repeating it under its own name is noise. Its *colour* is
        // on every row, which is the part that costs nothing to repeat and answers "where does
        // this group end" without the heading having to be back on screen.
        div()
            .id(gpui::SharedString::from(format!("lib-{}", plugin.id)))
            .flex()
            .items_center()
            .gap_1p5()
            .pl(indent(2))
            .px_1p5()
            .py_0p5()
            .rounded(Metrics::RADIUS_SM)
            .cursor_pointer()
            .hover(|this| this.bg(theme.surface_hover))
            .child(swatch(accent))
            .child(crate::ui::icons::icon(kind, px(11.0), accent))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text)
                    .child(audio_name(self, &plugin.name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme.text_faint)
                    .truncate()
                    .child(self.plugin_description(&plugin.description)),
            )
            .on_mouse_down(gpui::MouseButton::Left, on_click)
            .into_any_element()
    }

    /// One sound of an open bank, named as the font names it and numbered as MIDI does.
    fn preset_row<F>(&self, preset: &SoundFontPreset, choice: PresetRef, on_click: F) -> AnyElement
    where
        F: Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        let accent = theme.group_color(preset_hue(preset.bank, preset.patch));
        div()
            .id(gpui::SharedString::from(format!(
                "lib-preset-{}-{}-{}",
                choice.font.0, choice.bank, choice.patch
            )))
            .flex()
            .items_center()
            .gap_1p5()
            .pl(indent(3))
            .px_1p5()
            .py_0p5()
            .rounded(Metrics::RADIUS_SM)
            .cursor_pointer()
            .hover(|this| this.bg(theme.surface_hover))
            .child(swatch(accent))
            .child(crate::ui::icons::icon(Icon::Wave, px(11.0), accent))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme.text)
                    .truncate()
                    .child(preset.name.clone()),
            )
            .child(
                // The patch number alone: the bank is the row this one is sitting under.
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(preset.patch.to_string()),
            )
            .on_mouse_down(gpui::MouseButton::Left, on_click)
            .into_any_element()
    }

    /// A line of explanation where a branch would otherwise open onto nothing.
    /// Whether the selected track is one an instrument can be loaded onto.
    ///
    /// An audio track has no instrument, so clicking a sound with one selected does nothing at
    /// all — which is a thing worth saying rather than a click to be swallowed.
    fn selected_track_takes_an_instrument(&self) -> bool {
        self.selected_track
            .and_then(|id| self.project().track(id))
            .is_some_and(|track| track.kind.as_instrument().is_some())
    }

    fn note_row(&self, depth: usize, text: &str) -> AnyElement {
        div()
            .text_xs()
            .text_color(self.theme.text_muted)
            .pl(indent(depth))
            .px_1p5()
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// How far apart two hues are, going whichever way round the wheel is shorter.
    fn apart(a: f32, b: f32) -> f32 {
        let gap = (a.rem_euclid(1.0) - b.rem_euclid(1.0)).abs();
        gap.min(1.0 - gap)
    }

    #[test]
    fn no_two_categories_wear_the_same_colour() {
        // The mark is the whole of what tells one group from another at a glance, so two
        // categories a hair apart on the wheel are two groups that read as one.
        let hues: Vec<f32> = PluginCategory::ALL.into_iter().map(category_hue).collect();
        let least = 1.0 / PluginCategory::ALL.len() as f32;
        for (index, hue) in hues.iter().enumerate() {
            for other in &hues[index + 1..] {
                assert!(
                    apart(*hue, *other) >= least - 1e-4,
                    "{hue} and {other} are {} apart, wanted {least}",
                    apart(*hue, *other)
                );
            }
        }
    }

    #[test]
    fn a_font_s_sounds_change_colour_exactly_where_general_midi_changes_family() {
        // Eight to a band because that is General MIDI's own division, so the colour breaks fall
        // where the names do: program 7 is the last harpsichord and program 8 the first tuned
        // percussion.
        assert_eq!(preset_hue(0, 0), preset_hue(0, 7));
        assert_ne!(preset_hue(0, 7), preset_hue(0, 8));
        assert_eq!(preset_hue(0, 8), preset_hue(0, 15));
        // Sixteen families, and the sixteenth is not the first again.
        assert_ne!(preset_hue(0, 0), preset_hue(0, 120));
    }

    #[test]
    fn the_kits_never_wear_a_melodic_family_s_colour() {
        // Both banks can be open at once, and a kit drawn in the strings' colour would be saying
        // something untrue about a row three lines below a violin.
        let kits = preset_hue(PERCUSSION_BANK, 0);
        assert_eq!(
            kits,
            preset_hue(PERCUSSION_BANK, 48),
            "one bank, one band: the kits are not sixteen families"
        );
        for patch in 0..128 {
            assert!(
                apart(kits, preset_hue(0, patch)) > 1e-4,
                "kit colour collides with melodic patch {patch}"
            );
        }
        // A font that declares patches past General MIDI's range wraps back into the melodic
        // wheel rather than landing on the kits.
        for patch in [128, 200, 1_000] {
            assert!(apart(kits, preset_hue(0, patch)) > 1e-4, "patch {patch}");
        }
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
}
