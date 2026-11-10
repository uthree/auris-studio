//! The library's voice shelf: what it lists, the Engine catalogues behind its VOICEVOX rows,
//! and what choosing a row does.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use auris_i18n::Key;
use auris_session::{BackendKind, ShelfVoice, SingerPortraitSource, VoicevoxCatalog, prelude::*};
use gpui::Context;

use crate::app::AurisApp;
use crate::dock::Panel;

/// How long a catalogue the Engine answered with is trusted before it is asked again.
const CATALOG_REFRESH: Duration = Duration::from_secs(60);
/// How soon an Engine that did not answer is asked again: it is most often still starting.
const CATALOG_RETRY: Duration = Duration::from_secs(10);

/// The styles one shelf connection's Engine last advertised, and when it was last asked.
pub(crate) struct ShelfCatalog {
    /// The last catalogue the Engine answered with. A later failure keeps it: the rows it
    /// produced are still the Engine's singers, and they sing again once it is back.
    pub(crate) catalog: Option<VoicevoxCatalog>,
    /// When the last request finished.
    pub(crate) asked: Instant,
    /// Whether that request failed.
    pub(crate) failed: bool,
}

/// Whether a connection's Engine should be asked for its catalogue now.
pub(crate) fn shelf_catalog_due(entry: Option<&ShelfCatalog>, now: Instant) -> bool {
    entry.is_none_or(|entry| {
        let wait = if entry.failed {
            CATALOG_RETRY
        } else {
            CATALOG_REFRESH
        };
        now.saturating_duration_since(entry.asked) >= wait
    })
}

/// The artwork a shelf row shows: its saved style's, or the Engine style it would save.
pub(crate) fn shelf_icon_source(row: &ShelfVoice) -> SingerPortraitSource {
    match &row.style {
        Some(style) if !style.saved => {
            SingerPortraitSource::for_library_style(row.path.clone(), style.choice.decode_style_id)
        }
        Some(style) => SingerPortraitSource::for_library_voice(
            row.path.clone(),
            Some(style.choice.name.clone()),
        ),
        None => SingerPortraitSource::for_library_voice(row.path.clone(), None),
    }
}

impl AurisApp {
    /// The shelf's rows, scanned once and kept until a voice file or a catalogue changes.
    ///
    /// One row per VOICEVOX singing style, so a singer is chosen the way a sound is; a
    /// DiffSinger or LeapSinger bank is one row and its speakers are the inspector's.
    pub(crate) fn voice_list(&mut self) -> Vec<ShelfVoice> {
        if self.voices.is_none() {
            let scanned = auris_session::library::voices_with_settings(&self.settings);
            let catalogs = &self.voicevox_shelf_catalogs;
            self.voices = Some(auris_session::library::voice_shelf(&scanned, |path| {
                catalogs.get(path).and_then(|entry| entry.catalog.as_ref())
            }));
        }
        self.voices.clone().unwrap_or_default()
    }

    /// Asks one shelf connection's Engine for its singers, while the library is open.
    ///
    /// One request at a time, from the timer rather than from painting. An Engine started after
    /// Auris is found within [`CATALOG_RETRY`]; only a loopback Engine is ever asked.
    pub(crate) fn poll_voicevox_shelf(&mut self, cx: &mut Context<Self>) {
        if self.voicevox_shelf_pending || !self.panels.is_open(Panel::Library) {
            return;
        }
        let now = Instant::now();
        let mut paths: Vec<PathBuf> = self
            .voice_list()
            .into_iter()
            .filter(|row| BackendKind::from_path(&row.path) == Some(BackendKind::Voicevox))
            .map(|row| row.path)
            .collect();
        paths.dedup();
        let Some(path) = paths
            .into_iter()
            .find(|path| shelf_catalog_due(self.voicevox_shelf_catalogs.get(path), now))
        else {
            return;
        };
        self.voicevox_shelf_pending = true;
        cx.spawn(async move |this, cx| {
            let worker_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { auris_session::library::fetch_shelf_catalog(&worker_path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.voicevox_shelf_pending = false;
                this.finish_voicevox_shelf(path, result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps a fetched catalogue, and lists its singers when they differ from what is shown.
    pub(crate) fn finish_voicevox_shelf(
        &mut self,
        path: PathBuf,
        result: Result<VoicevoxCatalog, String>,
    ) {
        let previous = self
            .voicevox_shelf_catalogs
            .remove(&path)
            .and_then(|entry| entry.catalog);
        let failed = result.is_err();
        if let Err(error) = &result {
            log::debug!("VOICEVOX singers unavailable for the library: {error}");
        }
        let catalog = result.ok().or_else(|| previous.clone());
        if catalog != previous {
            self.voices = None;
        }
        self.voicevox_shelf_catalogs.insert(
            path,
            ShelfCatalog {
                catalog,
                asked: Instant::now(),
                failed,
            },
        );
    }

    /// Chooses a shelf row for `track`: its voice, and for a VOICEVOX row its style.
    ///
    /// A style the Engine advertised but the connection file does not name yet is written into
    /// the file first, as the inspector's speaker menu would, so the track can name it.
    pub(crate) fn choose_shelf_voice(
        &mut self,
        track: TrackId,
        row: ShelfVoice,
        cx: &mut Context<Self>,
    ) {
        let speaker = match &row.style {
            None => None,
            Some(style) if style.saved => Some(style.choice.name.clone()),
            Some(style) => {
                let catalog = self
                    .voicevox_shelf_catalogs
                    .get(&row.path)
                    .and_then(|entry| entry.catalog.clone());
                // The row came from a catalogue, which is kept until a newer one replaces it.
                let Some(catalog) = catalog else { return };
                match self.session.save_voicevox_shelf_style(
                    &row.path,
                    &catalog,
                    style.choice.query_style_id,
                    style.choice.decode_style_id,
                ) {
                    Ok(name) => {
                        self.voices = None;
                        Some(name)
                    }
                    Err(error) => {
                        self.set_failed_status(self.failure(Key::CmdChooseVoice, &error));
                        return;
                    }
                }
            }
        };
        self.apply_singer_voice_selection(track, row.path, speaker, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engine_that_did_not_answer_is_asked_again_sooner() {
        let now = Instant::now() + Duration::from_secs(3600);
        let entry = |failed, ago| ShelfCatalog {
            catalog: None,
            asked: now - Duration::from_secs(ago),
            failed,
        };
        assert!(shelf_catalog_due(None, now));
        assert!(!shelf_catalog_due(Some(&entry(true, 5)), now));
        assert!(shelf_catalog_due(Some(&entry(true, 10)), now));
        assert!(!shelf_catalog_due(Some(&entry(false, 30)), now));
        assert!(shelf_catalog_due(Some(&entry(false, 60)), now));
    }
}
