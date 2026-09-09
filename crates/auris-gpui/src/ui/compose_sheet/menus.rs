//! The pickers: a catalogue on one side, a [`ContextMenu`] on the other.
//!
//! Its own file because it is a different job in a different vocabulary. Nothing here builds an
//! element or reads the theme; every function walks a list the composer publishes — the moods,
//! the grooves and the progressions — and turns it into items carrying a
//! [`MenuCommand`]. The panel in `view` opens them, and `context_menu` carries out what was
//! chosen, so a menu that gained an entry needs a command over there to answer it.

use auris_i18n::Key;
use auris_session::prelude::*;

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};

use super::dials::*;
use super::lyrics::section_label;

impl AurisApp {
    pub(super) fn song_tonality_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongTonality));
        for (choice, label) in [
            (Tonality::Auto, Key::SongAutomatic),
            (Tonality::Major, Key::SongMajor),
            (Tonality::Minor, Key::SongMinor),
        ] {
            menu = menu.toggle(
                self.t(label),
                MenuCommand::SongTonality(choice),
                self.song_sheet.as_ref().and_then(|d| d.tonality) == Some(choice),
            );
        }
        menu
    }

    pub(super) fn song_pace_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPace));
        for (choice, label) in [
            (Pace::Auto, Key::SongAutomatic),
            (Pace::Slow, Key::SongSlow),
            (Pace::Moderate, Key::SongModerate),
            (Pace::Fast, Key::SongFast),
        ] {
            menu = menu.toggle(
                self.t(label),
                MenuCommand::SongPace(choice),
                self.song_sheet.as_ref().and_then(|d| d.pace) == Some(choice),
            );
        }
        menu
    }

    /// The closing gesture, including a return to the opening for background music.
    pub(super) fn song_ending_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let current = self.song_sheet.as_ref().map(|dials| dials.ending);
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongEnding));
        for (ending, label) in [
            (Ending::Held, Key::SongEndingHeld),
            (Ending::Fade, Key::SongEndingFade),
            (Ending::Loop, Key::SongEndingLoop),
            (Ending::None, Key::SongEndingNone),
        ] {
            menu = menu.toggle(
                self.t(label),
                MenuCommand::SongEnding(ending),
                current == Some(ending),
            );
        }
        menu
    }

    /// Original vocal sections available as a shared melody for this lyric.
    pub(super) fn song_melody_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        index: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMelodyFrom));
        let Some(dials) = &self.song_sheet else {
            return menu;
        };
        let Some(section) = dials.sections.get(index) else {
            return menu;
        };
        menu = menu.toggle(
            self.t(Key::SongChordsOwn),
            MenuCommand::SongMelodySource {
                section: index,
                source: None,
            },
            section.melody_from.is_none(),
        );
        // A source with dependants stays an original; linking it would create a chain.
        if dials
            .sections
            .iter()
            .any(|s| s.melody_from.as_ref() == Some(&section.name))
        {
            return menu;
        }
        for source in &dials.sections {
            if source.name == section.name || source.melody_from.is_some() {
                continue;
            }
            menu = menu.toggle(
                section_label(self, &source.name),
                MenuCommand::SongMelodySource {
                    section: index,
                    source: Some(source.name.clone()),
                },
                section.melody_from.as_ref() == Some(&source.name),
            );
        }
        menu
    }

    /// Installed singers, and a file picker for a model outside the library.
    pub(super) fn song_singer_menu(&mut self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let current = self.song_sheet.as_ref().and_then(|d| d.singer.clone());
        let mut menu = ContextMenu::new(anchor, self.t(Key::SingerVoiceLabel)).toggle(
            self.t(Key::SingerNoVoice),
            MenuCommand::SongSinger(None),
            current.is_none(),
        );
        for (name, path) in self.voice_list() {
            let path = path.to_string_lossy().into_owned();
            menu = menu.toggle(
                name,
                MenuCommand::SongSinger(Some(path.clone())),
                current.as_ref() == Some(&path),
            );
        }
        menu.separator()
            .item(self.t(Key::CmdChooseVoice), MenuCommand::ChooseSongSinger)
    }

    pub(crate) fn choose_song_singer(&mut self, cx: &mut gpui::Context<Self>) {
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let file = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogChooseVoice.get(language))
                .add_filter(
                    Key::FilterVoiceModel.get(language),
                    &["onnx", "yaml", "json"],
                )
                .pick_file()
                .await;
            if let Some(file) = file {
                let _ = this.update(cx, |this, cx| {
                    if let Some(dials) = this.song_sheet.as_mut() {
                        let path = Some(file.path().to_string_lossy().into_owned());
                        if dials.singer != path {
                            dials.singer_speaker = None;
                        }
                        dials.singer = path;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
    /// The meters the sheet offers: the common list with a tick beside the one in force, then a
    /// way to type any meter the list does not hold — the same two halves as the transport's
    /// signature field, because they are the same question asked in two places.
    pub(super) fn song_meter_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let current = self.song_sheet.as_ref().map(|dials| dials.meter);
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMeter));
        for signature in TimeSignature::COMMON {
            menu = menu.toggle(
                signature.to_string(),
                MenuCommand::SongMeter(signature.numerator, signature.denominator),
                Some(signature) == current,
            );
        }
        menu.separator()
            .item(self.t(Key::MenuOtherSignature), MenuCommand::SongTypeMeter)
    }

    /// The named feelings, each of which means four numbers.
    pub(super) fn song_mood_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMood));
        for name in Mood::NAMES {
            menu = menu.item(self.t(mood_key(name)), MenuCommand::SongMood(name));
        }
        menu
    }

    /// What one section may play: the progressions this song already carries, and every one the
    /// catalogue knows.
    ///
    /// Choosing a catalogue entry the song does not carry adds it, which is the only way a second
    /// progression comes into existence — there is no chart list to fill in first.
    pub(super) fn song_chords_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        section: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongChords));
        let carried: Vec<String> = self
            .song_sheet
            .as_ref()
            .map(|dials| {
                dials
                    .charts
                    .iter()
                    .map(|(name, chart)| chart_label(name, chart))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(dials) = self.song_sheet.as_ref() {
            for (name, chart) in &dials.charts {
                // An unwritten chart is a request, not a progression: its row here would say
                // おまかせ while quietly *sharing* another section's deal, which is not what
                // anyone picking おまかせ means. The row below files a fresh one instead.
                if chart.is_unwritten() {
                    continue;
                }
                menu = menu.item(
                    self.progression_name(&chart_label(name, chart)),
                    MenuCommand::SongSectionChords {
                        section,
                        name: name.clone(),
                    },
                );
            }
        }
        // Leaving it to the composer, writing one out, and keeping the one written. The last
        // only appears where there is something to keep: a section playing a quoted progression
        // already has a name, and offering to file 丸サ進行 under a second one would be a way to
        // end up with two.
        menu = menu.separator();
        menu = menu.item(
            self.t(Key::SongChordsOwn),
            MenuCommand::SongInventProgression(section),
        );
        menu = menu.item(
            self.t(Key::SongWriteProgression),
            MenuCommand::SongWriteProgression(section),
        );
        if self.section_chart_is_written(section) {
            menu = menu.item(
                self.t(Key::SongKeepProgression),
                MenuCommand::SongKeepProgression(section),
            );
        }

        // The book somebody keeps, then the catalogue that shipped. Theirs first: a person who
        // has written progressions down is reaching for one of those.
        menu = menu.separator();
        for entry in self.progressions.entries() {
            menu = menu.item(
                entry.name.clone(),
                MenuCommand::SongSectionChords {
                    section,
                    name: entry.name.clone(),
                },
            );
        }
        menu = menu.separator();
        for entry in progression_catalog() {
            // Already offered above under the name this song files it under.
            if carried.iter().any(|held| held == entry.name) {
                continue;
            }
            menu = menu.item(
                // The name, not the description. A description is a sentence — "王道進行 (4536):
                // the J-pop staple" — and sixteen sentences stacked in a menu is a menu nobody
                // can scan. The name is what the thing is called and what a `.asong` writes.
                auris_i18n::audio::theory_name(entry.name, self.language()),
                MenuCommand::SongSectionChords {
                    section,
                    name: entry.name.to_string(),
                },
            );
        }
        menu
    }

    /// Whether the section's progression is one somebody wrote out rather than one it quotes.
    ///
    /// A quotation already has a name and keeping it under a second would be a way to end up with
    /// the same loop twice in one picker.
    fn section_chart_is_written(&self, section: usize) -> bool {
        self.song_sheet
            .as_ref()
            .and_then(|dials| {
                let section = dials.sections.get(section)?;
                let (_, chart) = dials
                    .charts
                    .iter()
                    .find(|(name, _)| name == &section.chords)?;
                // An unwritten chart has nothing to keep — the bars do not exist until the song
                // is written, and would be different bars next seed anyway.
                Some(chart.quoted_as.is_none() && !chart.is_unwritten())
            })
            .unwrap_or(false)
    }

    /// How far a section is moved from the key, in semitones.
    pub(super) fn song_transpose_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        section: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongTranspose));
        for steps in TRANSPOSES {
            menu = menu.item(
                transpose_label(steps),
                MenuCommand::SongSectionTranspose { section, steps },
            );
        }
        menu
    }

    /// The sections a place in the form may play: every one the song has, and a new one.
    pub(super) fn song_form_name_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        place: usize,
    ) -> ContextMenu {
        self.section_menu(anchor, Key::SongSectionName, move |name| {
            MenuCommand::SongFormName {
                place,
                name: name.to_string(),
            }
        })
    }

    /// The same list, for a section being added after `place`.
    pub(super) fn song_section_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        place: usize,
    ) -> ContextMenu {
        self.section_menu(anchor, Key::SongAddSection, move |name| {
            MenuCommand::SongAddSection {
                place,
                name: name.to_string(),
            }
        })
    }

    /// Every section this song has, then a fresh one of each name it knows.
    ///
    /// Two groups, and the difference between them is the whole of what a form is. Choosing from
    /// the first is a **repeat** — the same chorus again, sharing one definition, which is what
    /// makes it recognisably the same chorus. Choosing from the second makes a *new* section, and
    /// a name already taken comes back numbered: once there is a verse, the second group offers
    /// `verse 2`, which is how a song gets two verses that are not the same eight bars.
    fn section_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        title: Key,
        command: impl Fn(&str) -> MenuCommand,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(title));
        let Some(dials) = self.song_sheet.as_ref() else {
            return menu;
        };
        for section in &dials.sections {
            menu = menu.item(section_label(self, &section.name), command(&section.name));
        }
        menu = menu.separator();
        for stem in SECTION_NAMES {
            let name = unused_section_name(dials, stem);
            menu = menu.item(section_label(self, &name), command(&name));
        }
        menu
    }

    /// Every drum groove the composer knows by name.
    pub(super) fn song_groove_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartGroove));
        for groove in groove_catalog() {
            menu = menu.item(
                auris_i18n::audio::theory_description(groove.description, self.language()),
                MenuCommand::SongGroove(groove.name),
            );
        }
        menu
    }

    /// The roles a part may take.
    pub(super) fn song_role_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        part: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartRole));
        let current = self
            .song_sheet
            .as_ref()
            .and_then(|dials| dials.parts.get(part));
        for role in Role::ALL.into_iter().filter(|role| !role.is_drum()) {
            menu = menu.toggle(
                self.t(role_key(role)),
                MenuCommand::SongPartRole { part, role },
                current.is_some_and(|part| part.role == role),
            );
        }
        menu
    }

    /// A role for a part that does not exist yet.
    pub(super) fn song_add_part_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongAddPart));
        for role in Role::ALL {
            if role.is_drum() {
                continue;
            }
            menu = menu.item(self.t(role_key(role)), MenuCommand::SongAddPart(role));
        }
        menu
    }

    /// The whole songs the sheet can be filled from.
    pub(super) fn song_preset_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongStyle));
        for entry in PRESETS {
            menu = menu.item(
                self.t(style_key(entry.name)),
                MenuCommand::SongPreset(entry.name),
            );
        }
        menu
    }

    /// The notes a drum part may strike.
    pub(super) fn song_note_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        part: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartNote));
        for (note, _) in DRUM_NOTES {
            menu = menu.item(
                drum_note_label(note),
                MenuCommand::SongPartNote { part, note },
            );
        }
        menu
    }

    /// The octaves a part may sit in.
    pub(super) fn song_octave_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        part: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartOctave));
        for octave in 1..=7 {
            menu = menu.item(
                octave.to_string(),
                MenuCommand::SongPartOctave { part, octave },
            );
        }
        menu
    }
}

impl AurisApp {
    /// What a progression is called in the interface, or its own name if the catalogue has never
    /// heard of it — which is what a chart somebody typed out by hand looks like.
    pub(super) fn progression_name(&self, name: &str) -> String {
        auris_i18n::audio::theory_name(name, self.language()).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_i18n::Language;
    use gpui::{TestAppContext, point, px};

    use crate::harness::open;
    use crate::ui::context_menu::MenuEntry;

    #[gpui::test]
    fn pre_choruses_and_bridges_can_be_added_repeated_and_saved(cx: &mut TestAppContext) {
        use crate::harness::{choose, paint};

        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.language = Language::Japanese;
        });
        for (name, label) in [
            ("pre", "1番 Bメロ"),
            ("pre 2", "2番 Bメロ"),
            ("bridge", "1番 Cメロ"),
            ("bridge 2", "2番 Cメロ"),
            ("pre", "1番 Bメロ"),
        ] {
            app.update(cx, |this, _| {
                let menu = this.song_section_menu(point(px(100.0), px(100.0)), 0);
                let command = MenuCommand::SongAddSection {
                    place: 0,
                    name: name.into(),
                };
                assert!(menu.entries.iter().any(|entry| matches!(entry,
                    MenuEntry::Item(item) if item.command == command && item.label.as_ref() == label
                )));
                this.open_menu(menu);
            });
            paint(&app, cx);
            choose(
                &app,
                cx,
                &MenuCommand::SongAddSection {
                    place: 0,
                    name: name.into(),
                },
            );
            paint(&app, cx);
            app.read_with(cx, |this, _| {
                assert!(this.menu.is_none());
                let dials = this.song_sheet.as_ref().unwrap();
                assert_eq!(dials.form[1], name);
                assert_eq!(dials.sections.iter().filter(|s| s.name == name).count(), 1);
                let spec = song_spec(dials);
                assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
            });
        }
        app.update(cx, |this, _| this.language = Language::English);
        app.read_with(cx, |this, _| {
            for (name, label) in [
                ("pre", "Pre-Chorus 1"),
                ("pre 2", "Pre-Chorus 2"),
                ("bridge", "Bridge 1"),
                ("bridge 2", "Bridge 2"),
                ("prelude", "prelude"),
                ("bridge reprise", "bridge reprise"),
                ("pre3", "Pre-Chorus 3"),
                ("bridge3", "Bridge 3"),
            ] {
                assert_eq!(section_label(this, name), label);
            }
            let menu = this.song_melody_menu(point(px(100.0), px(100.0)), 0);
            for name in ["pre", "pre 2", "bridge", "bridge 2"] {
                assert!(menu.entries.iter().any(|entry| matches!(entry,
                    MenuEntry::Item(item) if item.command == MenuCommand::SongMelodySource {
                        section: 0, source: Some(name.into())
                    } && item.label.as_ref() == section_label(this, name)
                )));
            }
        });
    }

    #[gpui::test]
    fn localized_section_choices_keep_the_original_identifiers(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            let dials = this.song_sheet.as_mut().unwrap();
            let mut custom = dials.sections[0].clone();
            custom.name = "verse reprise".to_string();
            custom.melody_from = None;
            dials.sections.push(custom);
        });
        let anchor = point(px(0.0), px(0.0));
        for (language, verse, next_verse) in [
            (Language::English, "Verse 1", "Verse 2"),
            (Language::Japanese, "1番 Aメロ", "2番 Aメロ"),
        ] {
            app.update(cx, |this, _| this.language = language);
            app.read_with(cx, |this, _| {
                let form = this.song_form_name_menu(anchor, 0);
                for (identifier, label) in [
                    ("verse", verse),
                    ("verse 2", next_verse),
                    ("verse reprise", "verse reprise"),
                ] {
                    let item = form
                        .entries
                        .iter()
                        .find_map(|entry| match entry {
                            MenuEntry::Item(item)
                                if item.command
                                    == (MenuCommand::SongFormName {
                                        place: 0,
                                        name: identifier.to_string(),
                                    }) =>
                            {
                                Some(item)
                            }
                            _ => None,
                        })
                        .expect(
                            "the picker carries an existing or fresh section by its identifier",
                        );
                    assert_eq!(item.label.as_ref(), label);
                }

                let dials = this.song_sheet.as_ref().unwrap();
                let chorus = dials
                    .sections
                    .iter()
                    .position(|section| section.name == "chorus")
                    .unwrap();
                let melody = this.song_melody_menu(anchor, chorus);
                let source = melody
                    .entries
                    .iter()
                    .find_map(|entry| match entry {
                        MenuEntry::Item(item)
                            if item.command
                                == (MenuCommand::SongMelodySource {
                                    section: chorus,
                                    source: Some("verse".to_string()),
                                }) =>
                        {
                            Some(item)
                        }
                        _ => None,
                    })
                    .expect("the same translated source is available for a shared melody");
                assert_eq!(source.label.as_ref(), verse);
                assert!(dials.sections.iter().any(|section| section.name == "verse"));
            });
        }
    }

    #[gpui::test]
    fn sections_with_colliding_translations_remain_distinguishable(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            let dials = this.song_sheet.as_mut().unwrap();
            for name in ["verse 1", "1番 Aメロ", "Verse 1"] {
                let mut section = dials
                    .sections
                    .iter()
                    .find(|section| section.name == "verse")
                    .unwrap()
                    .clone();
                section.name = name.to_string();
                section.melody_from = None;
                dials.sections.push(section);
            }
        });
        let anchor = point(px(0.0), px(0.0));
        for (language, translated, custom, unchanged) in [
            (Language::English, "Verse 1", "Verse 1", "1番 Aメロ"),
            (Language::Japanese, "1番 Aメロ", "1番 Aメロ", "Verse 1"),
        ] {
            app.update(cx, |this, _| this.language = language);
            app.read_with(cx, |this, _| {
                let dials = this.song_sheet.as_ref().unwrap();
                let chorus = dials
                    .sections
                    .iter()
                    .position(|section| section.name == "chorus")
                    .unwrap();
                let form = this.song_form_name_menu(anchor, 0);
                let melody = this.song_melody_menu(anchor, chorus);
                for name in ["verse", "verse 1", custom] {
                    let expected = format!("{translated} ({name})");
                    assert_eq!(section_label(this, name), expected);
                    for menu in [&form, &melody] {
                        let item = menu
                            .entries
                            .iter()
                            .find_map(|entry| match entry {
                                MenuEntry::Item(item) => match &item.command {
                                    MenuCommand::SongFormName {
                                        name: identifier, ..
                                    }
                                    | MenuCommand::SongMelodySource {
                                        source: Some(identifier),
                                        ..
                                    } if identifier == name => Some(item),
                                    _ => None,
                                },
                                _ => None,
                            })
                            .expect("the choice retains its stored identifier in either menu");
                        assert_eq!(item.label.as_ref(), expected);
                    }
                }
                assert_eq!(section_label(this, unchanged), unchanged);
            });
        }
    }

    #[gpui::test]
    fn custom_names_cannot_collide_with_generated_disambiguation(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        for (language, translated) in [
            (Language::English, "Verse 1"),
            (Language::Japanese, "1番 Aメロ"),
        ] {
            let first_collision = format!("{translated} (verse)");
            let second_collision = format!("{first_collision} (verse)");
            app.update(cx, |this, _| {
                this.language = language;
                this.song_sheet = None;
                this.open_song_sheet();
                let dials = this.song_sheet.as_mut().unwrap();
                for name in ["verse 1", &first_collision, &second_collision] {
                    let mut section = dials
                        .sections
                        .iter()
                        .find(|section| section.name == "verse")
                        .unwrap()
                        .clone();
                    section.name = name.to_string();
                    section.melody_from = None;
                    dials.sections.push(section);
                }
            });
            app.read_with(cx, |this, _| {
                let dials = this.song_sheet.as_ref().unwrap();
                let labels: Vec<_> = dials
                    .sections
                    .iter()
                    .map(|section| section_label(this, &section.name))
                    .collect();
                let unique: std::collections::HashSet<_> = labels.iter().collect();
                assert_eq!(
                    unique.len(),
                    labels.len(),
                    "even nested custom names remain distinct: {labels:?}"
                );
                let chorus = dials
                    .sections
                    .iter()
                    .position(|section| section.name == "chorus")
                    .unwrap();
                for menu in [
                    this.song_form_name_menu(point(px(0.0), px(0.0)), 0),
                    this.song_melody_menu(point(px(0.0), px(0.0)), chorus),
                ] {
                    for item in menu.entries.iter().filter_map(|entry| match entry {
                        MenuEntry::Item(item) => Some(item),
                        _ => None,
                    }) {
                        let identifier = match &item.command {
                            MenuCommand::SongFormName { name, .. }
                            | MenuCommand::SongMelodySource {
                                source: Some(name), ..
                            } => name,
                            _ => continue,
                        };
                        assert_eq!(item.label.as_ref(), section_label(this, identifier));
                    }
                }
                assert_eq!(
                    section_label(this, "verse"),
                    format!("{second_collision} (verse)")
                );
            });
        }
    }

    #[gpui::test]
    fn duplicate_section_identifiers_do_not_require_disambiguation(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.language = Language::English;
            this.open_song_sheet();
            let dials = this.song_sheet.as_mut().unwrap();
            let verse = dials
                .sections
                .iter()
                .find(|section| section.name == "verse")
                .unwrap()
                .clone();
            dials.sections.push(verse);
        });
        app.read_with(cx, |this, _| {
            assert_eq!(section_label(this, "verse"), "Verse 1");
        });
    }
}
