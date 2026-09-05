//! Optional singer artwork, loaded off the UI thread and keyed by the saved voice identity.

use std::collections::VecDeque;
use std::sync::Arc;

use auris_i18n::Key;
use auris_session::{SingerPortraitSource, load_singer_portrait, prelude::*};
use gpui::{AnyElement, Context, ObjectFit, RenderImage, Window, div, img, prelude::*, px};

use crate::app::AurisApp;
use crate::dock::Panel;
use crate::ui::portrait_image::decode_portrait;
use crate::ui::widgets::{ButtonStyle, button};

const CACHE_LIMIT: usize = 8;
type PortraitResult = Result<Option<Arc<RenderImage>>, String>;

/// No image, including a failed request, is retried on each repaint.
#[derive(Default)]
pub(crate) struct SingerPortraits {
    entries: VecDeque<(SingerPortraitSource, PortraitResult)>,
    pending: Option<(SingerPortraitSource, u64)>,
    generation: u64,
    retired: Vec<Arc<RenderImage>>,
}

impl SingerPortraits {
    /// A voice file may have been replaced without changing its path.
    pub(crate) fn invalidate(&mut self) {
        for (_, result) in self.entries.drain(..) {
            if let Ok(Some(image)) = result {
                self.retired.push(image);
            }
        }
        self.generation = self.generation.wrapping_add(1);
        // Keep the worker occupied until it finishes, so rapid edits cannot queue unbounded IO.
    }

    fn get(&self, source: &SingerPortraitSource) -> Option<&PortraitResult> {
        self.entries
            .iter()
            .find(|(key, _)| key == source)
            .map(|(_, result)| result)
    }

    fn forget(&mut self, source: &SingerPortraitSource) {
        if let Some(index) = self.entries.iter().position(|(key, _)| key == source)
            && let Some((_, Ok(Some(image)))) = self.entries.remove(index)
        {
            self.retired.push(image);
        }
    }

    /// GPUI retains uploaded atlas tiles even after the last image Arc is dropped.
    pub(crate) fn release_images(&mut self, window: &mut Window) {
        for image in self.retired.drain(..) {
            if let Err(error) = window.drop_image(image) {
                log::debug!("could not release singer portrait: {error}");
            }
        }
    }

    fn finish(&mut self, source: SingerPortraitSource, generation: u64, result: PortraitResult) {
        if self.pending.as_ref() != Some(&(source.clone(), generation)) {
            return;
        }
        self.pending = None;
        if generation != self.generation {
            return;
        }
        self.forget(&source);
        self.entries.push_back((source, result));
        while self.entries.len() > CACHE_LIMIT {
            if let Some((_, Ok(Some(image)))) = self.entries.pop_front() {
                self.retired.push(image);
            }
        }
    }
}

impl AurisApp {
    /// The timer notices selection changes; painting never reads a model or contacts an Engine.
    pub(crate) fn poll_singer_portrait(&mut self, cx: &mut Context<Self>) {
        if !self.panels.is_open(Panel::Inspector) || self.singer_portraits.pending.is_some() {
            return;
        }
        let Some(source) = self
            .selected_track
            .and_then(|track| self.session.singer_portrait_source(track).ok().flatten())
        else {
            return;
        };
        if self.singer_portraits.get(&source).is_some() {
            return;
        }
        let generation = self.singer_portraits.generation;
        self.singer_portraits.pending = Some((source.clone(), generation));
        cx.spawn(async move |this, cx| {
            let worker_source = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    load_singer_portrait(&worker_source)
                        .map_err(|error| error.to_string())
                        .and_then(|portrait| {
                            portrait
                                .map(|portrait| decode_portrait(portrait.bytes()))
                                .transpose()
                        })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = &result {
                    log::debug!("singer portrait unavailable: {error}");
                }
                this.singer_portraits.finish(source, generation, result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Transparent artwork fits within the inspector without cropping the figure.
    pub(crate) fn singer_portrait_row(
        &self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let source = self.session.singer_portrait_source(track).ok()??;
        match self.singer_portraits.get(&source)? {
            Ok(Some(image)) => Some(
                div()
                    .debug_selector(|| "singer-portrait".into())
                    .w_full()
                    .h(px(180.0))
                    .flex_shrink_0()
                    .min_w_0()
                    .py_1()
                    .child(
                        img(Arc::clone(image))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element(),
            ),
            Ok(None) => None,
            Err(_) => Some(
                button(
                    "singer-portrait-retry",
                    self.t(Key::SingerPortraitRetry),
                    ButtonStyle::Ghost,
                    false,
                    self.theme.accent,
                    &self.theme,
                    cx.listener(move |this, _, _, cx| {
                        this.singer_portraits.forget(&source);
                        this.poll_singer_portrait(cx);
                        cx.notify();
                    }),
                )
                .w_full()
                .into_any_element(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, paint, resize, with_a_singer_clip};
    use gpui::{TestAppContext, size};
    use image::{Frame, Rgba, RgbaImage};
    use std::path::PathBuf;

    struct VoiceFile(PathBuf);

    impl VoiceFile {
        fn new() -> Self {
            let path = crate::ui::singer::voicevox_test_file();
            std::fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "format_version": 1,
                    "name": "Artwork test",
                    "url": "http://127.0.0.1:0",
                    "styles": [
                        {"name":"First", "query_style_id":6000, "decode_style_id":3001},
                        {"name":"Second", "query_style_id":6000, "decode_style_id":3003}
                    ]
                }))
                .unwrap(),
            )
            .unwrap();
            Self(path)
        }
    }

    impl Drop for VoiceFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::from_pixel(
            10,
            20,
            Rgba([60, 120, 180, 128]),
        ))]))
    }

    fn supply(app: &mut AurisApp, source: SingerPortraitSource, result: PortraitResult) {
        let generation = app.singer_portraits.generation;
        app.singer_portraits.pending = Some((source.clone(), generation));
        app.singer_portraits.finish(source, generation, result);
    }

    #[gpui::test]
    fn portrait_follows_speaker_and_undo_without_showing_stale_artwork(cx: &mut TestAppContext) {
        let file = VoiceFile::new();
        let (app, cx, track, _) = with_a_singer_clip(cx);
        let first = app.update(cx, |this, _| {
            this.panels = Default::default();
            this.select_track(track);
            this.session.set_singer_voice(track, Some(&file.0)).unwrap();
            let source = this.session.singer_portrait_source(track).unwrap().unwrap();
            supply(this, source.clone(), Ok(Some(image())));
            source
        });
        resize(&app, cx, size(px(960.0), px(720.0)));
        let portrait = cx.debug_bounds("singer-portrait").unwrap();
        let voice_control = cx.debug_bounds("singer-voice").unwrap();
        assert!(portrait.left() >= voice_control.left());
        assert!(portrait.right() <= voice_control.right());
        assert_eq!(portrait.size.height, px(180.0));

        app.update(cx, |this, cx| {
            this.session
                .set_singer_speaker(track, Some("Second"))
                .unwrap();
            // GPUI 0.2 retains old debug bounds after elements disappear. Ask the current
            // view construction about absence; the bounds above still verify the layout.
            assert!(this.singer_portrait_row(track, cx).is_none());
        });
        paint(&app, cx);
        app.update(cx, |this, cx| {
            let source = this.session.singer_portrait_source(track).unwrap().unwrap();
            assert_ne!(source, first);
            supply(this, source, Ok(None));
            assert!(this.singer_portrait_row(track, cx).is_none());
        });
        paint(&app, cx);
        app.update(cx, |this, cx| {
            this.session.undo();
            assert!(this.singer_portrait_row(track, cx).is_some());
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("singer-portrait").is_some());
    }

    #[gpui::test]
    fn a_replaced_voice_rejects_its_pending_portrait_and_releases_the_worker(
        cx: &mut TestAppContext,
    ) {
        let file = VoiceFile::new();
        let (app, cx, track, _) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.select_track(track);
            this.session.set_singer_voice(track, Some(&file.0)).unwrap();
            let source = this.session.singer_portrait_source(track).unwrap().unwrap();
            let generation = this.singer_portraits.generation;
            this.singer_portraits.pending = Some((source.clone(), generation));
            this.invalidate_sung_previews();
            this.singer_portraits
                .finish(source.clone(), generation, Ok(Some(image())));
            assert!(this.singer_portraits.pending.is_none());
            assert!(this.singer_portraits.get(&source).is_none());
            assert!(
                !this.status_failed,
                "artwork never changes synthesis feedback"
            );
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("singer-portrait").is_none());
    }

    #[gpui::test]
    fn eviction_and_invalidation_release_uploaded_images_on_the_next_frame(
        cx: &mut TestAppContext,
    ) {
        let file = VoiceFile::new();
        let (app, cx, track, _) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.select_track(track);
            this.session.set_singer_voice(track, Some(&file.0)).unwrap();
            let source = this.session.singer_portrait_source(track).unwrap().unwrap();
            supply(this, source, Ok(Some(image())));
        });
        paint(&app, cx);
        app.update(cx, |this, _| {
            for _ in 0..CACHE_LIMIT {
                let track = this.session.add_singer_track("Another performer");
                this.session.set_singer_voice(track, Some(&file.0)).unwrap();
                let source = this.session.singer_portrait_source(track).unwrap().unwrap();
                supply(this, source, Ok(Some(image())));
            }
            assert_eq!(this.singer_portraits.entries.len(), CACHE_LIMIT);
            assert_eq!(this.singer_portraits.retired.len(), 1);
            this.singer_portraits.invalidate();
            assert_eq!(this.singer_portraits.retired.len(), CACHE_LIMIT + 1);
        });
        paint(&app, cx);
        app.update(cx, |this, _| {
            assert!(this.singer_portraits.retired.is_empty());
            assert!(this.singer_portraits.entries.is_empty());
        });
    }

    #[gpui::test]
    fn portrait_failure_is_cached_and_the_retry_button_restarts_only_artwork(
        cx: &mut TestAppContext,
    ) {
        let file = VoiceFile::new();
        let (app, cx, track, _) = with_a_singer_clip(cx);
        let (source, revision) = app.update(cx, |this, cx| {
            this.panels = Default::default();
            this.select_track(track);
            this.session.set_singer_voice(track, Some(&file.0)).unwrap();
            let source = this.session.singer_portrait_source(track).unwrap().unwrap();
            supply(this, source.clone(), Err("Engine is offline".into()));
            this.poll_singer_portrait(cx);
            assert!(
                this.singer_portraits.pending.is_none(),
                "a repaint must not retry IO"
            );
            (source, this.session.revision())
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("singer-portrait").is_none());
        click("singer-portrait-retry", cx);
        cx.run_until_parked();
        app.update(cx, |this, _| {
            assert!(matches!(this.singer_portraits.get(&source), Some(Err(_))));
            assert!(this.singer_portraits.pending.is_none());
            assert_eq!(this.session.revision(), revision);
            assert!(!this.status_failed);
            assert!(this.sung_retry.is_empty());
            assert!(this.sung_failures.is_empty());
        });
        assert!(cx.debug_bounds("singer-portrait-retry").is_some());
    }
}
