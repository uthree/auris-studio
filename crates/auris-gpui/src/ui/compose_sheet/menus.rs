//! The pickers: a catalogue on one side, a [`ContextMenu`] on the other.
//!
//! Its own file because it is a different job in a different vocabulary. Nothing here builds an
//! element or reads the theme; every function walks a list the composer publishes — the moods,
//! the grooves, the progressions, General MIDI — and turns it into items carrying a
//! [`MenuCommand`]. The panel in `view` opens them, and `context_menu` carries out what was
//! chosen, so a menu that gained an entry needs a command over there to answer it.

use auris_i18n::Key;
use auris_session::prelude::*;

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};

use super::dials::*;

impl AurisApp {
    /// The meters the sheet offers.
    pub(super) fn song_meter_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMeter));
        for (numerator, denominator) in [(4, 4), (3, 4), (6, 8), (5, 4), (7, 8), (12, 8)] {
            menu = menu.item(
                format!("{numerator}/{denominator}"),
                MenuCommand::SongMeter(numerator, denominator),
            );
        }
        menu
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
                menu = menu.item(
                    self.progression_name(&chart_label(name, chart)),
                    MenuCommand::SongSectionChords {
                        section,
                        name: name.clone(),
                    },
                );
            }
        }
        // Writing one out, and keeping the one written. The second only appears where there is
        // something to keep: a section playing a quoted progression already has a name, and
        // offering to file 丸サ進行 under a second one would be a way to end up with two.
        menu = menu.separator();
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
                Some(chart.quoted_as.is_none())
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

    /// Which parts of the roster play in a section.
    ///
    /// One row per part with a tick against the ones that play, rather than a list to edit: the
    /// question a person has here is "does the pad come in yet", and a tick answers it at a glance
    /// for the whole roster at once.
    ///
    /// The last part left is shown ticked and dead. A section that plays nothing is silence, and
    /// the specification cannot say it — an empty list is already how *everything* is written down
    /// — so the row that would produce one is unusable rather than quietly doing nothing.
    pub(super) fn song_section_parts_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        section: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongSectionParts));
        let Some(dials) = self.song_sheet.as_ref() else {
            return menu;
        };
        let Some(plan) = dials.sections.get(section) else {
            return menu;
        };
        let playing = dials
            .parts
            .iter()
            .filter(|part| part_plays_in(plan, &part.name))
            .count();
        for part in &dials.parts {
            let plays = part_plays_in(plan, &part.name);
            menu = menu.toggle_if(
                !(plays && playing <= 1),
                part.name.clone(),
                MenuCommand::SongSectionPart {
                    section,
                    part: part.name.clone(),
                },
                plays,
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
            menu = menu.item(section.name.clone(), command(&section.name));
        }
        menu = menu.separator();
        for stem in SECTION_NAMES {
            let name = unused_section_name(dials, stem);
            menu = menu.item(name.clone(), command(&name));
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
        for role in Role::ALL {
            menu = menu.item(
                self.t(role_key(role)),
                MenuCommand::SongPartRole { part, role },
            );
        }
        menu
    }

    /// A role for a part that does not exist yet.
    pub(super) fn song_add_part_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongAddPart));
        for role in Role::ALL {
            menu = menu.item(self.t(role_key(role)), MenuCommand::SongAddPart(role));
        }
        menu
    }

    /// Every instrument this build can play.
    /// The whole songs the sheet can be filled from.
    pub(super) fn song_preset_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongStyle));
        let language = self.language();
        for entry in PRESETS {
            // The description rather than the name: `city-pop` is what a command line takes, and
            // "Electric piano and slap bass over 丸サ進行" is what tells somebody whether it is
            // the one they want.
            menu = menu.item(
                format!(
                    "{} — {}",
                    entry.name,
                    auris_i18n::audio::preset_description(entry.description, language)
                ),
                MenuCommand::SongPreset(entry.name),
            );
        }
        menu
    }

    /// What one part plays: a General MIDI sound, or one of the built-in plugins.
    ///
    /// The programs go in by family rather than all hundred and twenty-eight at once — a menu
    /// that tall does not fit on a screen, and General MIDI already grouped them in eights.
    pub(super) fn song_instrument_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        part: usize,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartInstrument));
        let language = self.language();
        let drums = self
            .song_sheet
            .as_ref()
            .and_then(|dials| dials.parts.get(part))
            .is_some_and(|part| part.role.is_drum());
        if drums {
            // A drum part's number is a whole kit, so there is nothing to group: the eight kits
            // are the list.
            for (patch, name) in gm::KITS {
                menu = menu.item(
                    name,
                    MenuCommand::SongPartProgram {
                        part,
                        program: patch,
                    },
                );
            }
        } else {
            for (family, name) in gm::FAMILIES.iter().enumerate() {
                menu = menu.item(
                    format!("{name}…"),
                    MenuCommand::SongPartFamily {
                        part,
                        family,
                        anchor,
                    },
                );
            }
        }
        menu = menu.separator();
        let mut instruments: Vec<(String, String)> = self
            .registry()
            .instruments()
            .map(|descriptor| {
                (
                    descriptor.id.to_string(),
                    auris_i18n::audio::plugin_name(&descriptor.name, language).to_string(),
                )
            })
            .collect();
        instruments.sort_by(|one, other| one.1.cmp(&other.1));
        for (id, name) in instruments {
            menu = menu.item(name, MenuCommand::SongPartInstrument { part, id });
        }
        menu
    }

    /// The eight sounds of one General MIDI family.
    pub(crate) fn song_program_menu(
        &self,
        anchor: gpui::Point<gpui::Pixels>,
        part: usize,
        family: usize,
    ) -> ContextMenu {
        let title = gm::FAMILIES.get(family).copied().unwrap_or_default();
        let mut menu = ContextMenu::new(anchor, title);
        for program in gm::Program::family_programs(family) {
            menu = menu.item(
                program.name(),
                MenuCommand::SongPartProgram {
                    part,
                    program: program.0,
                },
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
