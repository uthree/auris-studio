//! Background track conversion using the cancellable export overlay.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use gpui::Context;

use crate::app::{AurisApp, ExportState};

impl AurisApp {
    pub(crate) fn convert_track_to_audio(&mut self, track: TrackId, cx: &mut Context<Self>) {
        if self.auto_sing.is_some()
            || self.choosing_export
            || self
                .export
                .as_ref()
                .is_some_and(|export| export.result.is_none())
        {
            self.set_status(self.t(Key::ExportAlreadyRunning));
            return;
        }
        let job = match self.session.convert_track_job(track) {
            Ok(job) => job,
            Err(error) => {
                self.set_failed_status(self.failure(Key::MenuConvertTrackToAudio, &error));
                return;
            }
        };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let landing_cancel = Arc::clone(&cancel);
        self.export = Some(ExportState {
            path: PathBuf::from(
                self.project()
                    .track(track)
                    .expect("checked track")
                    .name
                    .clone(),
            ),
            progress: Arc::clone(&progress),
            cancel: Arc::clone(&cancel),
            result: None,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move {
                    job.render(
                        &mut |fraction: f32| progress.store(fraction.to_bits(), Ordering::Relaxed),
                        &cancel,
                    )
                    .map_err(|error| (error.is_cancellation(), error.to_string()))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let result = if landing_cancel.load(Ordering::Relaxed) {
                    Err((true, String::new()))
                } else {
                    rendered.and_then(|result| {
                        this.session
                            .land_track_conversion(result)
                            .map_err(|error| (error.is_cancellation(), error.to_string()))
                    })
                };
                let message = match result {
                    Ok(clip) => {
                        this.select_track(track);
                        this.select_clip(Some(clip));
                        if matches!(
                            this.automation_lanes.get(&track),
                            Some(ParamTarget::Instrument { .. })
                        ) {
                            this.automation_lanes.remove(&track);
                        }
                        let text = this.t(Key::TrackConvertedToAudio).to_string();
                        this.set_status(text.clone());
                        Ok(text)
                    }
                    Err((true, _)) => {
                        let text = this.t(Key::ExportCancelled).to_string();
                        this.set_status(text.clone());
                        Ok(text)
                    }
                    Err((false, error)) => {
                        let text = messages::failed(
                            this.language(),
                            this.t(Key::MenuConvertTrackToAudio),
                            &error,
                        );
                        this.set_failed_status(text.clone());
                        Err(text)
                    }
                };
                if let Some(export) = this.export.as_mut() {
                    export.result = Some(message);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::open;
    use crate::ui::context_menu::{MenuCommand, MenuEntry};
    use gpui::{TestAppContext, point, px};

    #[gpui::test]
    fn track_menu_conversion_lands_audio_and_undo_restores_the_notes(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let track = app.update(cx, |this, cx| {
            let track = this.session.add_default_instrument_track("Convert me").unwrap();
            let clip = this.session.add_midi_clip(track, "Notes", Ticks::ZERO, Ticks::QUARTER).unwrap();
            this.session.add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER)).unwrap();
            let menu = this.track_menu(point(px(100.0), px(100.0)), track);
            assert!(menu.entries.iter().any(|entry| matches!(entry, MenuEntry::Item(item) if item.command == MenuCommand::ConvertTrackToAudio(track))));
            this.run_menu_command(MenuCommand::ConvertTrackToAudio(track), cx);
            track
        });
        cx.run_until_parked();
        app.update(cx, |this, _| {
            assert!(this.session.project().track(track).unwrap().kind.as_audio().is_some(), "{:?}", this.export.as_ref().and_then(|export| export.result.as_ref()));
            let menu = this.track_menu(point(px(100.0), px(100.0)), track);
            assert!(!menu.entries.iter().any(|entry| matches!(entry, MenuEntry::Item(item) if item.command == MenuCommand::ConvertTrackToAudio(track))));
            assert_eq!(this.session.undo(), Some(Edit::ConvertTrackToAudio));
            assert_eq!(this.session.project().track(track).unwrap().kind.note_clips().unwrap()[0].notes.len(), 1);
        });
    }
}
