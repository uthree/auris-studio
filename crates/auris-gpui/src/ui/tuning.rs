//! The document's tuning, from the window: choosing one, reading one from a Scala file, and
//! saying which tracks cannot follow it.
//!
//! The menu that offers all this is the harmony lane's, in `context_menu::timeline`, because a
//! tuning is the same kind of fact as a key. What is here is the work the menu sends for.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use gpui::Context;

use crate::app::AurisApp;
use crate::ui::context_menu::tuning_name;

impl AurisApp {
    /// Says what the tuning now is, on the status line, and which tracks will not follow it.
    pub(crate) fn announce_tuning(&mut self) {
        let name = tuning_name(&self.project().tuning, self.language());
        let mut line = messages::tuning_set(self.language(), name);
        let untuned = self.session.tracks_playing_untuned();
        if !untuned.is_empty() {
            line = format!(
                "{line} · {}",
                messages::tuning_untuned_tracks(self.language(), untuned.len())
            );
        }
        self.set_status(line);
    }

    /// Whether `track`'s instrument plays every note on the nearest semitone in this tuning —
    /// which its header says for as long as it is so.
    pub(crate) fn track_plays_untuned(&mut self, track: TrackId) -> bool {
        let revision = self.session.revision();
        if self.untuned_tracks.0 != revision {
            self.untuned_tracks = (revision, self.session.tracks_playing_untuned());
        }
        self.untuned_tracks.1.contains(&track)
    }

    /// Asks for a Scala file and makes the document's tuning from it: a scale (`.scl`), or a
    /// keyboard map (`.kbm`) placing the scale already chosen.
    pub(crate) fn import_scala(&mut self, keyboard: bool, cx: &mut Context<Self>) {
        let language = self.language();
        let (title, filter, extension) = match keyboard {
            true => (Key::DialogImportKeyboardMap, Key::FilterKeyboardMap, "kbm"),
            false => (Key::DialogImportScala, Key::FilterScala, "scl"),
        };
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(title.get(language))
                .add_filter(filter.get(language), &[extension])
                .pick_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            // A Scala file is a few hundred bytes, and the session refuses one too large to be
            // one before reading it, so this is read here rather than on a background thread.
            let _ = this.update(cx, |this, cx| {
                let result = match keyboard {
                    true => this.session.apply_keyboard_map(&path),
                    false => this.session.import_scala(&path, None),
                };
                match result {
                    Ok(()) => this.announce_tuning(),
                    Err(error) => this.set_failed_status(this.failure(Key::CmdImportScala, &error)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use auris_i18n::Language;
    use auris_session::prelude::*;
    use gpui::{TestAppContext, point, px};

    use crate::ui::context_menu::{MenuCommand, MenuEntry, tuning_name};

    /// Whether the open menu's row for `command` is ticked.
    fn ticked(menu: &crate::ui::context_menu::ContextMenu, command: &MenuCommand) -> bool {
        menu.entries.iter().any(|entry| {
            matches!(entry, MenuEntry::Item(item) if &item.command == command && item.checked)
        })
    }

    #[gpui::test]
    fn choosing_a_tuning_from_the_menu_ticks_it_and_says_so(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let standard = MenuCommand::SetTuningPreset(TuningPreset::Standard);
        let edo = MenuCommand::SetTuningPreset(TuningPreset::Edo(31));
        app.update(cx, |this, cx| {
            let menu = this.tuning_menu(point(px(10.0), px(10.0)));
            assert!(
                ticked(&menu, &standard),
                "a new song is in equal temperament"
            );
            assert!(!ticked(&menu, &edo));
            this.run_menu_command(edo.clone(), cx);
        });
        app.update(cx, |this, _| {
            assert_eq!(this.project().tuning.steps_per_period(), 31);
            let menu = this.tuning_menu(point(px(10.0), px(10.0)));
            assert!(ticked(&menu, &edo));
            assert!(!ticked(&menu, &standard));
            assert!(
                this.status
                    .contains(&tuning_name(&this.project().tuning, this.language())),
                "{}",
                this.status
            );
        });
    }

    #[gpui::test]
    fn the_twelve_note_tunings_are_offered_on_the_songs_key(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            let d = MusicalKey::parse("D major").unwrap();
            this.session.set_key(Ticks::ZERO, d);
            let menu = this.tuning_menu(point(px(10.0), px(10.0)));
            let offered = |command: MenuCommand| {
                menu.entries
                    .iter()
                    .any(|entry| matches!(entry, MenuEntry::Item(item) if item.command == command))
            };
            assert!(offered(MenuCommand::SetTuningPreset(TuningPreset::Just(
                PitchClass::new(2)
            ))));
            assert!(!offered(MenuCommand::SetTuningPreset(TuningPreset::Just(
                PitchClass::new(0)
            ))));
        });
    }

    #[gpui::test]
    fn a_tuning_is_named_by_its_preset_whatever_pitch_it_is_pinned_to(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            let japanese = Language::Japanese;
            assert_eq!(tuning_name(&Tuning::standard(), japanese), "12平均律");
            let raised = Tuning::standard().with_reference(69, 442.0).unwrap();
            assert_eq!(tuning_name(&raised, japanese), "12平均律");
            assert_eq!(tuning_name(&Tuning::edo(31).unwrap(), japanese), "31平均律");
            assert_eq!(
                tuning_name(&Tuning::just(PitchClass::new(9)), japanese),
                "A の純正律"
            );
            let scala =
                Tuning::from_scala("Slendro\n5\n240.0\n480.0\n720.0\n960.0\n2/1\n").unwrap();
            assert_eq!(tuning_name(&scala, japanese), "Slendro");
            // And the status line says what is in force after a change.
            this.session.set_tuning(raised);
            this.announce_tuning();
            assert!(this.status.contains("12"), "{}", this.status);
        });
    }

    #[gpui::test]
    fn a_reference_pitch_typed_into_the_sheet_moves_the_whole_tuning(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.run_menu_command(MenuCommand::PromptTuningReference, cx);
            let field = this.prompt.as_ref().and_then(|prompt| prompt.field());
            assert_eq!(field.map(|field| field.content()), Some("440.00"));
        });
        crate::harness::paint(&app, cx);
        cx.simulate_input("442");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none(), "the sheet took the frequency");
            assert!((this.project().tuning.hz(69) - 442.0).abs() < 1e-9);
            // Every interval kept: an octave up is still twice the frequency.
            assert!((this.project().tuning.hz(81) - 884.0).abs() < 1e-9);
        });
    }
}
