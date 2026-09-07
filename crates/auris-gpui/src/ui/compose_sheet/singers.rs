//! Speaker discovery for the song sheet, before its singer track exists.

use std::{path::Path, sync::Arc};

use auris_i18n::Key;
use auris_session::{
    BackendKind, VoicevoxCatalog, VoicevoxConnection, VoicevoxSpeakerChoice, fetch_voicevox_catalog,
};
use gpui::{Context, Pixels, Point};

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};

impl AurisApp {
    fn saved_song_speakers(&mut self, at: Point<Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(at, self.t(Key::SingerSpeakerLabel));
        let Some(dials) = self.song_sheet.as_ref() else {
            return menu;
        };
        let Some(path) = dials.singer.clone() else {
            return menu;
        };
        let selected = dials.singer_speaker.clone();
        match self.session.voice_speakers_at(Path::new(&path)) {
            Ok(speakers) => {
                for (index, speaker) in speakers.into_iter().enumerate() {
                    let checked = selected
                        .as_ref()
                        .map_or(index == 0, |name| name == &speaker);
                    menu = menu.toggle(
                        speaker.clone(),
                        MenuCommand::SongSpeaker {
                            path: path.clone(),
                            speaker,
                        },
                        checked,
                    );
                }
            }
            Err(error) => {
                let message = self.failure(Key::CmdNextSpeaker, &error);
                self.set_failed_status(message.clone());
                menu = menu.item_greyed_unless(false, message, MenuCommand::ChooseSongSinger);
            }
        }
        menu
    }

    /// Opens saved speakers immediately and refreshes VOICEVOX choices in the background.
    pub(crate) fn open_song_speaker_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let fallback = self.saved_song_speakers(at);
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let Some(path) = dials.singer.clone() else {
            return;
        };
        if BackendKind::from_path(Path::new(&path)) != BackendKind::Voicevox {
            self.open_menu(fallback);
            cx.notify();
            return;
        }
        let connection = match self
            .session
            .voicevox_connection_at(Path::new(&path), dials.singer_speaker.as_deref())
        {
            Ok(connection) => connection,
            Err(error) => {
                self.set_failed_status(self.failure(Key::CmdNextSpeaker, &error));
                self.open_menu(fallback);
                cx.notify();
                return;
            }
        };
        self.voicevox_menu_generation = self.voicevox_menu_generation.wrapping_add(1);
        let request = self.voicevox_menu_generation;
        let mut menu = fallback.separator().item_greyed_unless(
            false,
            self.t(Key::VoiceSetupChecking),
            MenuCommand::RefreshSongSpeakers(at),
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
                this.finish_song_speakers(connection, request, result, cx)
            });
        })
        .detach();
    }

    fn finish_song_speakers(
        &mut self,
        connection: VoicevoxConnection,
        request: u64,
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
        if request != self.voicevox_menu_generation {
            return;
        }
        let at = menu.anchor;
        let current = self.song_sheet.as_ref().and_then(|dials| {
            let path = dials.singer.as_ref()?;
            self.session
                .voicevox_connection_at(Path::new(path), dials.singer_speaker.as_deref())
                .ok()
        });
        if current.as_ref() != Some(&connection) {
            self.close_menu();
            cx.notify();
            return;
        }
        let menu = match result {
            Ok(catalog) => {
                let choices = catalog.speaker_choices(&connection);
                let mut menu = if choices.is_empty() {
                    self.saved_song_speakers(at)
                } else {
                    ContextMenu::new(at, self.t(Key::SingerSpeakerLabel))
                };
                let snapshot = Arc::new(connection.clone());
                for choice in choices {
                    let checked = choice.query_style_id == connection.query_style_id
                        && choice.decode_style_id == connection.decode_style_id;
                    menu = menu.toggle(
                        choice.name.clone(),
                        MenuCommand::SongVoicevoxSpeaker {
                            connection: Arc::clone(&snapshot),
                            choice,
                        },
                        checked,
                    );
                }
                menu
            }
            Err(error) => self.saved_song_speakers(at).separator().item_greyed_unless(
                false,
                error,
                MenuCommand::RefreshSongSpeakers(at),
            ),
        }
        .separator()
        .item(
            self.t(Key::VoiceSetupLoadSingers),
            MenuCommand::RefreshSongSpeakers(at),
        );
        self.open_menu(menu);
        cx.notify();
    }

    /// Registers the selected Engine style, then retains its name in the song specification.
    pub(crate) fn select_song_voicevox_speaker(
        &mut self,
        connection: &VoicevoxConnection,
        choice: &VoicevoxSpeakerChoice,
    ) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let Some(path) = dials.singer.clone() else {
            return;
        };
        let selected = dials.singer_speaker.clone();
        match self.session.register_composed_voicevox_speaker(
            Path::new(&path),
            selected.as_deref(),
            connection,
            choice,
        ) {
            Ok(()) => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    dials.singer_speaker = Some(choice.name.clone());
                }
            }
            Err(error) => self.set_failed_status(self.failure(Key::CmdNextSpeaker, &error)),
        }
    }
}
