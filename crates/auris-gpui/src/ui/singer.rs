//! The selected singer's voice, speaker and rendering state, kept beside its track controls.

#[cfg(test)]
#[path = "voicevox_speaker_tests.rs"]
mod voicevox_speaker_tests;

use auris_i18n::{Key, messages};
use auris_session::{BackendKind, SingerTakeState, prelude::*};
use auris_session::{
    VoicevoxCatalog, VoicevoxConnection, VoicevoxSpeakerChoice, fetch_voicevox_catalog,
};
use gpui::{AnyElement, Context, IntoElement, Pixels, Point, div, prelude::*};
use std::sync::Arc;

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::tooltip::keyed_tip;
use crate::ui::widgets::{ButtonStyle, button, divider};

impl AurisApp {
    /// The current voice and speaker, without loading a model during a repaint.
    pub(crate) fn singer_voice_label(&self, track: TrackId) -> Option<String> {
        let info = self.session.singer_voice_info(track).ok()??;
        let speaker = self.live_voicevox_speaker_label(track).or(info.speaker);
        Some(match speaker {
            Some(speaker) => format!("{} · {speaker}", info.name),
            None => format!("{} · {}", info.name, self.t(Key::SingerDefaultSpeaker)),
        })
    }

    /// Whether the clip's backend accepts manual IPA and phoneme boundary edits.
    pub(crate) fn clip_accepts_phonemes(&self, clip: ClipId) -> bool {
        self.project()
            .track_of_clip(clip)
            .and_then(|track| self.session.singer_capabilities(track).ok())
            .is_some_and(|capabilities| capabilities.manual_phonemes && capabilities.phoneme_timing)
    }

    /// Persistent controls and feedback for a singer; ordinary tracks add no rows.
    pub(crate) fn singer_rows(
        &mut self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self
            .project()
            .track(track)
            .is_some_and(|track| track.kind.is_singer())
        {
            return Vec::new();
        }
        let theme = self.theme.clone();
        let info = self.session.singer_voice_info(track).ok().flatten();
        let voice = info
            .as_ref()
            .map(|info| info.name.clone())
            .unwrap_or_else(|| self.t(Key::CmdChooseVoice).to_string());
        let speaker = self
            .live_voicevox_speaker_label(track)
            .or_else(|| info.as_ref().and_then(|info| info.speaker.clone()))
            .unwrap_or_else(|| self.t(Key::SingerDefaultSpeaker).to_string());
        let mut rows = vec![
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(Key::SingerVoiceLabel))
                .into_any_element(),
            button(
                "singer-voice",
                voice.clone(),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                Self::opens_menu(cx, move |this, at| this.singer_voice_menu(track, at)),
            )
            .w_full()
            .min_w_0()
            .truncate()
            .tooltip(keyed_tip(voice, "", &theme))
            .into_any_element(),
        ];
        if let Some(info) = &info {
            rows.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(match info.backend {
                        BackendKind::Auris => Key::VoiceBackendAuris,
                        BackendKind::DiffSinger => Key::VoiceBackendDiffSinger,
                        BackendKind::Voicevox => Key::VoiceBackendVoicevox,
                    }))
                    .into_any_element(),
            );
            rows.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SingerSpeakerLabel))
                    .into_any_element(),
            );
            rows.push(
                button(
                    "singer-speaker",
                    speaker.clone(),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.open_singer_speaker_menu(track, event.position(), cx);
                        cx.stop_propagation();
                    }),
                )
                .w_full()
                .min_w_0()
                .truncate()
                .tooltip(keyed_tip(speaker, "", &theme))
                .into_any_element(),
            );
            if !info.capabilities.manual_phonemes || !info.capabilities.phoneme_timing {
                rows.push(
                    div()
                        .debug_selector(|| "singer-phoneme-limits".into())
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::VoicevoxPhonemesHint))
                        .into_any_element(),
                );
            }
        }
        let rendering = self
            .auto_sing
            .as_ref()
            .is_some_and(|job| job.track == track);
        let queued = self.sung_retry.contains(&track);
        let failure = self.singer_failure(track).map(str::to_string);
        let state = if rendering {
            self.t(Key::TakeRendering).to_string()
        } else if queued {
            self.t(Key::SingerQueued).to_string()
        } else if let Some(failure) = &failure {
            failure.clone()
        } else if info.is_none() {
            self.t(Key::SingerNoVoice).to_string()
        } else {
            let state = self.singer_take_badge(track);
            self.t(match state {
                SingerTakeState::Absent => Key::SingerTakeAbsent,
                SingerTakeState::Current => Key::SingerTakeCurrent,
                SingerTakeState::Behind => Key::SingerTakeBehind,
            })
            .to_string()
        };
        rows.push(
            div()
                .debug_selector(|| "singer-status".into())
                .text_xs()
                .text_color(if failure.is_some() && !rendering {
                    theme.danger
                } else {
                    theme.text_muted
                })
                .child(state)
                .into_any_element(),
        );
        if info.is_some() && !rendering && !queued {
            rows.push(
                button(
                    "singer-retry",
                    self.t(if failure.is_some() {
                        Key::SingerRetry
                    } else {
                        Key::CmdSing
                    }),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| this.retry_singer_take(track, cx)),
                )
                .w_full()
                .into_any_element(),
            );
        }
        // Artwork arrives asynchronously; keep the voice and synthesis controls above it so
        // they cannot move away from a pointer while the image is loading.
        if let Some(portrait) = self.singer_portrait_row(track, cx) {
            rows.push(portrait);
        }
        rows.push(divider(&theme).into_any_element());
        rows
    }

    /// A direct choice of installed voices for the named track, plus a file picker.
    pub(crate) fn singer_voice_menu(&mut self, track: TrackId, at: Point<Pixels>) -> ContextMenu {
        let name = self
            .project()
            .track(track)
            .map(|track| track.name.as_str())
            .unwrap_or("");
        let mut menu = ContextMenu::new(at, messages::voice_target(self.language(), name));
        let current = self
            .session
            .singer_voice_info(track)
            .ok()
            .flatten()
            .map(|info| info.path);
        for (name, path) in self.voice_list() {
            let selected = current.as_ref() == Some(&path);
            menu = menu.toggle(name, MenuCommand::SingerVoice { track, path }, selected);
        }
        menu.separator().item(
            self.t(Key::CmdChooseVoice),
            MenuCommand::ChooseSingerVoice(track),
        )
    }

    /// A named list of speakers/styles, with the current one marked. Cold metadata is loaded
    /// only for this deliberate opening gesture, never from the inspector's render path.
    pub(crate) fn singer_speaker_menu(&mut self, track: TrackId, at: Point<Pixels>) -> ContextMenu {
        let title = self
            .project()
            .track(track)
            .map(|track| track.name.clone())
            .unwrap_or_default();
        let mut menu =
            ContextMenu::new(at, format!("{} · {title}", self.t(Key::SingerSpeakerLabel)));
        match self.session.singer_speakers(track) {
            Ok(speakers) => {
                let selected = self
                    .session
                    .singer_voice_info(track)
                    .ok()
                    .flatten()
                    .and_then(|info| info.speaker);
                for (index, speaker) in speakers.into_iter().enumerate() {
                    let checked = selected
                        .as_ref()
                        .map_or(index == 0, |selected| *selected == speaker);
                    menu = menu.toggle(
                        speaker.clone(),
                        MenuCommand::SingerSpeaker { track, speaker },
                        checked,
                    );
                }
            }
            Err(error) => {
                let error = self.failure(Key::CmdNextSpeaker, &error);
                self.set_failed_status(error.clone());
                menu = menu.item_greyed_unless(false, error, MenuCommand::ChooseSingerVoice(track));
            }
        }
        menu
    }

    /// Resolves legacy labels by their actual Engine IDs without editing the document.
    fn live_voicevox_speaker_label(&self, track: TrackId) -> Option<String> {
        let (connection, catalog) = self.voicevox_catalogs.get(&track)?;
        let info = self.session.singer_voice_info(track).ok()??;
        if info.path != connection.path || info.speaker.as_deref() != Some(&connection.speaker) {
            return None;
        }
        if !catalog
            .query
            .iter()
            .any(|style| style.id == connection.query_style_id)
        {
            return None;
        }
        catalog
            .decode
            .iter()
            .find(|style| style.id == connection.decode_style_id)
            .map(|style| style.label())
    }

    /// Opens saved speakers immediately, then asks a VOICEVOX Engine for its current choices.
    pub(crate) fn open_singer_speaker_menu(
        &mut self,
        track: TrackId,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let fallback = self.singer_speaker_menu(track, at);
        if !self
            .session
            .singer_voice_info(track)
            .ok()
            .flatten()
            .is_some_and(|info| info.backend == BackendKind::Voicevox)
        {
            self.open_menu(fallback);
            cx.notify();
            return;
        }
        let connection = match self.session.singer_voicevox_connection(track) {
            Ok(connection) => connection,
            Err(error) => {
                self.set_failed_status(self.failure(Key::CmdNextSpeaker, &error));
                let menu = fallback.separator().item(
                    self.t(Key::VoiceSetupLoadSingers),
                    MenuCommand::RefreshVoicevoxSpeakers { track, at },
                );
                self.open_menu(menu);
                cx.notify();
                return;
            }
        };
        self.voicevox_menu_generation = self.voicevox_menu_generation.wrapping_add(1);
        let request = self.voicevox_menu_generation;
        let voice_generation = self.sung_preview_generation;
        let mut menu = fallback.separator().item_greyed_unless(
            false,
            self.t(Key::VoiceSetupChecking),
            MenuCommand::RefreshVoicevoxSpeakers { track, at },
        );
        menu.async_request = Some(request);
        self.open_menu(menu);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let url = connection.url.clone();
            let result = cx
                .background_executor()
                .spawn(
                    async move { fetch_voicevox_catalog(&url).map_err(|error| error.to_string()) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_voicevox_speakers(
                    track,
                    connection,
                    request,
                    voice_generation,
                    result,
                    cx,
                );
            });
        })
        .detach();
    }

    /// Updates only the menu that requested these rows, while its connection is unchanged.
    pub(crate) fn finish_voicevox_speakers(
        &mut self,
        track: TrackId,
        connection: VoicevoxConnection,
        request: u64,
        voice_generation: u64,
        result: Result<VoicevoxCatalog, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self
            .menu
            .as_ref()
            .filter(|menu| menu.async_request == Some(request))
        else {
            return;
        };
        let at = menu.anchor;
        if request != self.voicevox_menu_generation {
            return;
        }
        if voice_generation != self.sung_preview_generation
            || self.session.singer_voicevox_connection(track).ok().as_ref() != Some(&connection)
        {
            self.close_menu();
            cx.notify();
            return;
        }
        let menu = match result {
            Ok(catalog) => {
                let title = self.singer_speaker_menu(track, at).title;
                let mut menu = ContextMenu::new(at, title);
                let snapshot = Arc::new(connection.clone());
                for choice in catalog.speaker_choices(&connection) {
                    let checked = choice.query_style_id == connection.query_style_id
                        && choice.decode_style_id == connection.decode_style_id;
                    menu = menu.toggle(
                        choice.name.clone(),
                        MenuCommand::VoicevoxSpeaker {
                            track,
                            connection: Arc::clone(&snapshot),
                            choice,
                        },
                        checked,
                    );
                }
                self.voicevox_catalogs.insert(track, (connection, catalog));
                menu
            }
            Err(error) => {
                self.set_failed_status(error.clone());
                self.singer_speaker_menu(track, at)
                    .separator()
                    .item_greyed_unless(
                        false,
                        error,
                        MenuCommand::RefreshVoicevoxSpeakers { track, at },
                    )
            }
        }
        .separator()
        .item(
            self.t(Key::VoiceSetupLoadSingers),
            MenuCommand::RefreshVoicevoxSpeakers { track, at },
        );
        self.open_menu(menu);
        cx.notify();
    }

    /// Persists an explicitly chosen Engine style and invalidates audio from the old speaker.
    pub(crate) fn choose_voicevox_speaker(
        &mut self,
        track: TrackId,
        connection: Arc<VoicevoxConnection>,
        choice: VoicevoxSpeakerChoice,
        cx: &mut Context<Self>,
    ) {
        let catalog = self
            .voicevox_catalogs
            .get(&track)
            .filter(|(known, catalog)| {
                known == connection.as_ref() && catalog.speaker_choices(known).contains(&choice)
            })
            .map(|(_, catalog)| catalog.clone());
        let Some(catalog) = catalog else {
            return;
        };
        match self
            .session
            .set_voicevox_speaker(track, &connection, &choice)
        {
            Ok(()) => {
                self.select_track(track);
                self.invalidate_sung_previews();
                if let Ok(connection) = self.session.singer_voicevox_connection(track) {
                    self.voicevox_catalogs.insert(track, (connection, catalog));
                }
                self.sung_failures.remove(&track);
                self.sung_badges.clear();
                if self
                    .auto_sing
                    .as_ref()
                    .is_some_and(|job| job.track == track)
                {
                    self.cancel_auto_sing();
                }
                self.set_status(choice.name);
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdNextSpeaker, &error)),
        }
        cx.notify();
    }
}

/// A local connection descriptor for window tests; opening metadata never contacts its URL.
#[cfg(test)]
pub(crate) fn voicevox_test_file() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "auris-singer-ui-{}-{}.voicevox.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, r#"{"format_version":1,"name":"Test Voice","url":"http://127.0.0.1:1","sample_rate":24000,"frame_rate":100.0,"styles":[{"name":"Clear","query_style_id":6000,"decode_style_id":3000},{"name":"Warm","query_style_id":6000,"decode_style_id":3001}]}"#).unwrap();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{choose, click, paint, with_a_singer_clip};
    use crate::ui::context_menu::MenuEntry;
    use gpui::px;

    #[gpui::test]
    fn the_inspector_keeps_the_speaker_visible_and_selects_it_by_name(
        cx: &mut gpui::TestAppContext,
    ) {
        let path = voicevox_test_file();
        let (app, cx, track, _) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.select_track(track);
            this.session.set_singer_voice(track, Some(&path)).unwrap();
            this.session
                .set_singer_speaker(track, Some("Warm"))
                .unwrap();
            this.voices = Some(vec![("Test Voice".into(), path.clone())]);
            this.set_status("Unrelated status");
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("singer-speaker").is_some());
        assert!(cx.debug_bounds("singer-phoneme-limits").is_some());
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.singer_voice_label(track).as_deref(),
                Some("Test Voice · Warm")
            )
        });
        click("singer-speaker", cx);
        app.read_with(cx, |this, _| {
            let menu = this.menu.as_ref().unwrap();
            assert!(
                menu.entries
                    .iter()
                    .any(|entry| matches!(entry, MenuEntry::Item(item)
                if item.checked && item.label.as_ref() == "Warm"))
            );
        });
        choose(
            &app,
            cx,
            &MenuCommand::SingerSpeaker {
                track,
                speaker: "Clear".into(),
            },
        );
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.session
                    .singer_voice(track)
                    .unwrap()
                    .unwrap()
                    .speaker
                    .as_deref(),
                Some("Clear")
            );
            assert_eq!(
                this.singer_voice_label(track).as_deref(),
                Some("Test Voice · Clear")
            );
        });
        paint(&app, cx);
        click("singer-voice", cx);
        app.read_with(cx, |this, _| assert!(this.menu.as_ref().unwrap().entries.iter().any(|entry|
            matches!(entry, MenuEntry::Item(item) if item.checked && matches!(item.command, MenuCommand::SingerVoice { .. })))));
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn voicevox_offers_lyrics_but_refuses_manual_phoneme_edits(cx: &mut gpui::TestAppContext) {
        let path = voicevox_test_file();
        let (app, cx, track, clip) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.select_track(track);
            this.session.set_singer_voice(track, Some(&path)).unwrap();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.selected_notes.insert(0);
            let menu = this.roll_menu(gpui::point(px(10.0), px(10.0)), Some(0), 60, Ticks::ZERO);
            assert!(
                menu.entries
                    .iter()
                    .any(|entry| matches!(entry, MenuEntry::Item(item)
                if !item.enabled && matches!(item.command, MenuCommand::EditPhonemes { .. })))
            );
            assert!(
                menu.entries
                    .iter()
                    .any(|entry| matches!(entry, MenuEntry::Item(item)
                if item.enabled && matches!(item.command, MenuCommand::EditLyric { .. })))
            );
            this.open_phonemes_prompt(clip, 0);
            assert!(this.prompt.is_none());
            assert!(this.status_failed);
            this.open_lyric_prompt(clip, 0);
            assert!(this.prompt.is_some(), "lyrics remain editable");
        });
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn choosing_a_library_voice_reveals_the_actual_singer_target(cx: &mut gpui::TestAppContext) {
        let path = voicevox_test_file();
        let selector: &'static str =
            Box::leak(format!("lib-voice-{}", path.display()).into_boxed_str());
        let (app, cx, singer, _) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let piano = this.session.add_default_instrument_track("Piano").unwrap();
            this.select_track(piano);
            this.voices = Some(vec![("Test Voice".into(), path.clone())]);
            this.library_search = crate::ui::text_field::TextField::new("Test Voice");
        });
        paint(&app, cx);
        click(selector, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.selected_track,
                Some(singer),
                "the inspector now names the changed track"
            );
            assert_eq!(
                this.session.singer_voice(singer).unwrap().unwrap().name,
                "Test Voice"
            );
        });
        std::fs::remove_file(path).unwrap();
    }
}
