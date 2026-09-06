//! First-launch sound library setup, with network and sample reads off the UI thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use auris_i18n::{Language, messages};
use auris_session::library::{ShippedFont, download_font, font_downloads};
use auris_session::{LoadedFont, read_soundfont};
use gpui::Context;

use crate::app::AurisApp;

/// The worker's progress, kept outside the status message a command or failure owns.
pub(crate) struct SoundFontDownload {
    font: &'static ShippedFont,
    received: Arc<AtomicU64>,
    loading: Arc<AtomicBool>,
}

impl SoundFontDownload {
    fn new(font: &'static ShippedFont) -> Self {
        Self {
            font,
            received: Arc::new(AtomicU64::new(0)),
            loading: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Current progress in the current interface language, repainted with the rest of the window.
    pub(crate) fn message(&self, language: Language) -> String {
        if self.loading.load(Ordering::Relaxed) {
            messages::loading_soundfont(language, self.font.name)
        } else {
            let received = self.received.load(Ordering::Relaxed);
            let percent = received.saturating_mul(100) / self.font.bytes.max(1);
            messages::downloading_soundfont(language, self.font.name, percent.min(100))
        }
    }
}

impl AurisApp {
    /// Downloads any missing shipped fonts after the production window has opened.
    ///
    /// Kept out of the constructor so window tests never start network work. Existing packaged
    /// fonts were already loaded by the session, and do not enter this path at all.
    pub(crate) fn download_missing_soundfonts(&mut self, cx: &mut Context<Self>) {
        let downloads = font_downloads();
        if downloads.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            for download in downloads {
                let progress = SoundFontDownload::new(download.font);
                let received = Arc::clone(&progress.received);
                let loading = Arc::clone(&progress.loading);
                let Ok(previous_status) = this.update(cx, |this, cx| {
                    this.soundfont_download = Some(progress);
                    cx.notify();
                    this.status.clone()
                }) else {
                    return;
                };
                let font = download.font;
                log::info!("downloading {}", font.name);
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let path = download_font(font, &download.directory, |bytes| {
                            received.store(bytes, Ordering::Relaxed);
                        })
                        .map_err(|error| error.to_string())?;
                        loading.store(true, Ordering::Relaxed);
                        let loaded = read_soundfont(&path).map_err(|error| error.to_string())?;
                        Ok((path, loaded))
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.finish_soundfont_download(font, &previous_status, result);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    /// Adopts the global library into whichever document is current when its samples arrive.
    fn finish_soundfont_download(
        &mut self,
        font: &ShippedFont,
        previous_status: &str,
        result: Result<(PathBuf, LoadedFont), String>,
    ) {
        self.soundfont_download = None;
        match result {
            Ok((path, loaded)) => {
                let id = self.session.install_shipped_soundfont(&path, loaded);
                let sounds = self.session.soundfont_preset_count(id);
                let line = messages::soundfont_imported(self.language(), font.name, sounds);
                log::info!("{line}");
                // A command issued while downloading keeps its own feedback, especially a
                // failure. The library tree itself updates as soon as the samples are adopted.
                if !self.status_failed && self.status == previous_status {
                    self.set_status(line);
                }
            }
            Err(error) => {
                let line = messages::soundfont_download_failed(self.language(), font.name, &error);
                log::warn!("{line}");
                self.set_failed_status(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use auris_session::library::{GENERAL_MIDI, shipped};
    use gpui::TestAppContext;

    use crate::harness::{open, paint};

    #[gpui::test]
    fn download_progress_keeps_command_failures_visible(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            assert!(this.soundfont_download.is_none());
            let font = shipped(GENERAL_MIDI).unwrap();
            let progress = SoundFontDownload::new(font);
            progress.received.store(font.bytes / 2, Ordering::Relaxed);
            assert!(progress.message(Language::English).contains("50%"));
            this.soundfont_download = Some(progress);
            this.set_failed_status("A project could not be saved");
        });
        paint(&app, cx);
        app.update(cx, |this, _| {
            let progress = this.soundfont_download.as_ref().unwrap();
            progress.loading.store(true, Ordering::Relaxed);
            assert_eq!(
                progress.message(Language::Japanese),
                messages::loading_soundfont(Language::Japanese, progress.font.name)
            );
            assert_eq!(this.status, "A project could not be saved");
            assert!(this.status_failed);
            assert!(!this.session.is_dirty());
        });
    }

    #[gpui::test]
    fn a_failed_download_clears_progress_and_reports_the_problem(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let font = shipped(GENERAL_MIDI).unwrap();
            this.soundfont_download = Some(SoundFontDownload::new(font));
            this.finish_soundfont_download(font, "", Err("connection timed out".to_string()));
            assert!(this.soundfont_download.is_none());
            assert!(this.status_failed);
            assert!(this.status.contains(font.name));
            assert!(this.status.contains("connection timed out"));
            assert!(!this.session.is_dirty());
            assert_eq!(this.session.soundfonts().count(), 0);
        });
        paint(&app, cx);
    }
}
