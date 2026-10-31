//! The note-roll video window: a preview, the look's settings, and the export.
//!
//! The preview is a frame drawn by the same [`FrameRenderer`] the export uses, scaled down, so
//! what is seen here is what is encoded rather than a second drawing meant to resemble it. It
//! shows the moment under the transport's playhead: moving the playhead in the arrangement is how
//! a person looks at another part of the song.
//!
//! The encoder is an ffmpeg already on the machine. The window looks for one when it opens, and
//! when none is found it says how to install it instead of offering an export that would fail.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use auris_session::{
    BackgroundImage, Ffmpeg, FrameRenderer, NoteColoring, NoteRollStyle, NoteScore, VideoError,
    VideoSize,
};
use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, ObjectFit, RenderImage, Window, div, img,
    prelude::*, px,
};
use image::{Frame, RgbaImage};

use crate::app::{AurisApp, ExportState};
use crate::auxiliary_window::Surface;
use crate::theme::{Metrics, Theme};
use crate::ui::widgets::{ButtonState, ButtonStyle, button, button_enabled};

/// Width of the preview image; a portrait video is previewed at this height instead.
const PREVIEW_EDGE: u32 = 640;

/// Colours a palette or single-colour swatch steps through when clicked: bright enough to read
/// on a dark background, and different enough from their neighbours to tell parts apart.
const VIVID: [Color; 16] = [
    Color(0x3d7bff),
    Color(0x35d0e0),
    Color(0x7cf08a),
    Color(0xd8f04a),
    Color(0xffb13d),
    Color(0xff5f7a),
    Color(0xb58cff),
    Color(0x6fe3c1),
    Color(0xff7ad9),
    Color(0x9ad1ff),
    Color(0xf4f1ea),
    Color(0xffd84d),
    Color(0x4dffb8),
    Color(0xff8f4d),
    Color(0x8f9bff),
    Color(0x6fc3ff),
];

/// Background fills on offer: mostly dark, because coloured bars read best on dark, and one
/// paper white for the people who want the opposite.
const BACKGROUNDS: [Color; 6] = [
    Color(0x000000),
    Color(0x0b1020),
    Color(0x14181f),
    Color(0x1a1028),
    Color(0x0e1a14),
    Color(0xf4f1ea),
];

/// What is known about the ffmpeg on this machine.
#[derive(Clone, Debug, Default)]
pub(crate) enum FfmpegProbe {
    /// Nobody has looked yet.
    #[default]
    Unchecked,
    /// A look is under way on a worker.
    Checking,
    /// Found and usable.
    Found(Ffmpeg),
    /// Not found, or found and unusable, with what to tell the person.
    Missing(String),
}

/// The window's state, kept on the document's view so the window itself stays a view.
#[derive(Default)]
pub(crate) struct VideoExportState {
    /// Whether the window is open.
    pub(crate) open: bool,
    pub(crate) ffmpeg: FfmpegProbe,
    /// The notes of the document at a revision.
    score: Option<(u64, NoteScore)>,
    /// The decoded background picture, by path; an error is kept so it is shown, not retried.
    background: Option<(PathBuf, Result<BackgroundImage, String>)>,
    /// A renderer for the previewed style at a document revision.
    renderer: Option<((NoteRollStyle, u64), FrameRenderer)>,
    /// The last preview frame and what it was drawn from.
    preview: Option<((NoteRollStyle, u64, i64), Arc<RenderImage>)>,
    /// Images replaced since the last paint, released through the window that drew them.
    retired: Vec<Arc<RenderImage>>,
}

/// The preview's own size for a video of `size`: the same shape, at most [`PREVIEW_EDGE`] on its
/// longer side.
pub(crate) fn preview_size(size: VideoSize) -> VideoSize {
    let size = size.encodable();
    let (width, height) = (f64::from(size.width), f64::from(size.height));
    let scale = f64::from(PREVIEW_EDGE) / width.max(height);
    VideoSize::new(
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    )
    .encodable()
}

/// The next colour after `color` in [`VIVID`], or the first when it is not one of them.
pub(crate) fn next_vivid(color: Color) -> Color {
    let at = VIVID.iter().position(|candidate| *candidate == color);
    VIVID[at.map_or(0, |index| (index + 1) % VIVID.len())]
}

fn hsla(color: Color) -> gpui::Hsla {
    gpui::rgb(color.0).into()
}

impl AurisApp {
    /// Opens the video window, or brings it forward, and looks for ffmpeg if it has not been
    /// found yet — a person who has just installed it should not have to restart Auris.
    pub(crate) fn open_video_export(&mut self, cx: &mut Context<Self>) {
        self.video_export.open = true;
        self.raise_auxiliary = Some(Surface::VideoExport);
        if !matches!(
            self.video_export.ffmpeg,
            FfmpegProbe::Found(_) | FfmpegProbe::Checking
        ) {
            self.check_ffmpeg(cx);
        }
        cx.notify();
    }

    /// Looks for ffmpeg on a worker; probing it starts a process, which is not for the thread
    /// that draws.
    pub(crate) fn check_ffmpeg(&mut self, cx: &mut Context<Self>) {
        self.video_export.ffmpeg = FfmpegProbe::Checking;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { Ffmpeg::detect() })
                .await;
            let _ = this.update(cx, |this, cx| {
                let language = this.language();
                this.video_export.ffmpeg = match found {
                    Ok(ffmpeg) => FfmpegProbe::Found(ffmpeg),
                    Err(VideoError::FfmpegMissing(_)) => {
                        FfmpegProbe::Missing(messages::ffmpeg_missing(
                            language,
                            auris_session::ffmpeg_install_command(),
                            auris_session::ffmpeg_install_page(),
                        ))
                    }
                    Err(error) => FfmpegProbe::Missing(format!(
                        "{error}\n{}",
                        messages::ffmpeg_missing(
                            language,
                            auris_session::ffmpeg_install_command(),
                            auris_session::ffmpeg_install_page(),
                        )
                    )),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Changes the look and remembers it.
    pub(crate) fn edit_video_style(&mut self, change: impl FnOnce(&mut NoteRollStyle)) {
        let mut style = self.settings.video.clone();
        change(&mut style);
        if style == self.settings.video {
            return;
        }
        self.settings.video = style;
        if let Err(error) = self.save_settings() {
            log::warn!("could not save settings: {error}");
        }
    }

    /// The preview frame for the current look at the playhead, drawn again only when something
    /// it depends on has changed.
    fn video_preview(&mut self) -> Option<Arc<RenderImage>> {
        let style = self.settings.video.clone();
        let revision = self.session.revision();
        let seconds = self
            .project()
            .tempo_map
            .ticks_to_seconds(self.playhead_ticks().max_zero())
            .0;
        let fps = style.sanitised().fps;
        let frame = (seconds * f64::from(fps)).round() as i64;
        let key = (style.clone(), revision, frame);
        if let Some((drawn, image)) = &self.video_export.preview
            && *drawn == key
        {
            return Some(image.clone());
        }

        if self
            .video_export
            .score
            .as_ref()
            .is_none_or(|(at, _)| *at != revision)
        {
            self.video_export.score = Some((revision, self.session.note_score()));
        }
        let wanted = style.background_image.clone();
        let cached = self
            .video_export
            .background
            .as_ref()
            .map(|(path, _)| path.clone());
        if wanted.is_some() && cached != wanted {
            let path = wanted.clone().unwrap_or_default();
            let loaded = BackgroundImage::load(&path).map_err(|error| error.to_string());
            self.video_export.background = Some((path, loaded));
        }
        if self
            .video_export
            .renderer
            .as_ref()
            .is_none_or(|(at, _)| at.0 != style || at.1 != revision)
        {
            let preview_style = NoteRollStyle {
                size: preview_size(style.size),
                ..style.clone()
            };
            let background = match (&wanted, &self.video_export.background) {
                (Some(_), Some((_, Ok(image)))) => Some(image),
                _ => None,
            };
            let score = &self.video_export.score.as_ref()?.1;
            let renderer = FrameRenderer::with_background(score, &preview_style, background);
            self.video_export.renderer = Some(((style.clone(), revision), renderer));
        }
        let renderer = &self.video_export.renderer.as_ref()?.1;
        let mut canvas = renderer.canvas();
        renderer.draw(frame as f64 / f64::from(fps), &mut canvas);
        let (width, height) = (canvas.width(), canvas.height());
        let mut bytes = canvas.take();
        // RenderImage keeps RgbaImage storage but uploads it as BGRA.
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let pixels = RgbaImage::from_raw(width, height, bytes)?;
        let image = Arc::new(RenderImage::new(vec![Frame::new(pixels)]));
        if let Some((_, old)) = self.video_export.preview.replace((key, image.clone())) {
            self.video_export.retired.push(old);
        }
        Some(image)
    }

    /// Asks for a picture to put behind the notes.
    fn choose_video_background(&mut self, cx: &mut Context<Self>) {
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new()
                .set_title(Key::VideoChooseImage.get(language))
                .add_filter("PNG, JPEG, WebP", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
                .await;
            let Some(picked) = picked else {
                return;
            };
            let path = picked.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                this.edit_video_style(|style| style.background_image = Some(path));
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks where to write the video, then renders and encodes it on a worker behind the same
    /// progress overlay and Cancel as an audio export.
    fn start_video_export(&mut self, cx: &mut Context<Self>) {
        if self.choosing_export || self.export.as_ref().is_some_and(|e| e.result.is_none()) {
            self.set_status(self.t(Key::ExportAlreadyRunning));
            return;
        }
        let FfmpegProbe::Found(ffmpeg) = self.video_export.ffmpeg.clone() else {
            return;
        };
        let mut job = self.session.video_job(self.settings.video.clone());
        let guard = self.session.export_guard();
        self.choosing_export = true;
        self.export = None;
        let suggested = format!("{}.mp4", self.session.document_title());
        let language = self.language();

        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogExportVideo.get(language))
                .set_file_name(suggested)
                .add_filter(Key::FilterMp4.get(language), &["mp4"])
                .save_file()
                .await;
            let Some(handle) = handle else {
                let _ = this.update(cx, |this, _| this.choosing_export = false);
                return;
            };
            let path = handle.path().to_path_buf();
            let progress = Arc::new(AtomicU32::new(0));
            let cancel = Arc::new(AtomicBool::new(false));
            let _ = this.update(cx, |this, cx| {
                this.choosing_export = false;
                this.export = Some(ExportState {
                    path: path.clone(),
                    progress: Arc::clone(&progress),
                    result: None,
                    cancel: Arc::clone(&cancel),
                });
                cx.notify();
            });
            let render_path = path.clone();
            let rendered = cx
                .background_executor()
                .spawn(async move {
                    let mut report = |fraction: f32| {
                        progress.store(fraction.to_bits(), Ordering::Relaxed);
                    };
                    guard
                        .check_file(&render_path)
                        .and_then(|()| {
                            job.render(
                                &ffmpeg,
                                &render_path,
                                &mut RenderProgress::reporting(&mut report).cancelled_by(&cancel),
                            )
                        })
                        .map_err(|error| {
                            (
                                error.is_cancellation(),
                                crate::i18n::error_text(&error, language),
                            )
                        })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let language = this.language();
                let message = match rendered {
                    Ok(summary) => {
                        let text = messages::exported_video(
                            language,
                            &path.display().to_string(),
                            &Seconds(summary.seconds).format_clock(),
                        );
                        this.set_status(text.clone());
                        Ok(text)
                    }
                    Err((true, _)) => {
                        let text = Key::ExportCancelled.get(language).to_string();
                        this.set_status(text.clone());
                        Ok(text)
                    }
                    Err((false, error)) => {
                        let text =
                            messages::failed(language, Key::CmdExportVideo.get(language), &error);
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

    /// The window's contents.
    pub(crate) fn render_video_export(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        for image in self.video_export.retired.drain(..) {
            if let Err(error) = window.drop_image(image) {
                log::debug!("could not release a video preview: {error}");
            }
        }
        let theme = self.theme.clone();
        let style = self.settings.video.clone();
        let preview = self.video_preview();
        let empty = self
            .video_export
            .score
            .as_ref()
            .is_some_and(|(_, score)| score.notes.is_empty());

        let mut picture = div()
            .id("video-preview")
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(300.))
            .rounded(Metrics::RADIUS_MD)
            .overflow_hidden()
            .bg(gpui::black());
        if let Some(preview) = preview {
            picture = picture.child(img(preview).size_full().object_fit(ObjectFit::Contain));
        }
        let mut left = div()
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(picture)
            .child(self.note(self.t(Key::VideoPreviewHint), &theme));
        if empty {
            left = left.child(self.note(self.t(Key::VideoNoNotes), &theme));
        }
        if let Some((path, Err(error))) = &self.video_export.background
            && style.background_image.as_ref() == Some(path)
        {
            left = left.child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        left = left.child(self.ffmpeg_status(&theme, cx));

        let right = div()
            .id("video-settings")
            .flex()
            .flex_col()
            .gap_3()
            .w(px(330.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .pr_1()
            .child(self.video_format_section(&style, &theme, cx))
            .child(self.video_color_section(&style, &theme, cx))
            .child(self.video_background_section(&style, &theme, cx))
            .child(self.video_motion_section(&style, &theme, cx));

        div()
            .flex()
            .gap_4()
            .p_4()
            .size_full()
            .min_h_0()
            .child(left)
            .child(right)
            .into_any_element()
    }

    fn note(&self, text: &str, theme: &Theme) -> gpui::Div {
        div()
            .text_xs()
            .text_color(theme.text_muted)
            .child(text.to_string())
    }

    fn ffmpeg_status(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let exporting =
            self.choosing_export || self.export.as_ref().is_some_and(|e| e.result.is_none());
        let mut status = div().flex().flex_col().gap_2();
        match &self.video_export.ffmpeg {
            FfmpegProbe::Unchecked | FfmpegProbe::Checking => {
                status = status.child(self.note(self.t(Key::VideoFfmpegChecking), theme));
            }
            FfmpegProbe::Found(ffmpeg) => {
                status = status.child(self.note(
                    &messages::ffmpeg_found(
                        self.language(),
                        ffmpeg.version(),
                        ffmpeg.codec().encoder(),
                    ),
                    theme,
                ));
            }
            FfmpegProbe::Missing(text) => {
                status = status
                    .child(
                        div()
                            .id("video-ffmpeg-missing")
                            .text_sm()
                            .text_color(theme.warning)
                            .child(text.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(button(
                                "video-ffmpeg-copy",
                                self.t(Key::VideoFfmpegCopy),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                |_, _, cx: &mut App| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        auris_session::ffmpeg_install_command().to_string(),
                                    ));
                                },
                            ))
                            .child(button(
                                "video-ffmpeg-download",
                                self.t(Key::VideoFfmpegDownload),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                |_, _, cx: &mut App| {
                                    cx.open_url(auris_session::ffmpeg_install_page());
                                },
                            ))
                            .child(button(
                                "video-ffmpeg-recheck",
                                self.t(Key::VideoFfmpegRecheck),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(|this, _, _, cx| this.check_ffmpeg(cx)),
                            )),
                    );
            }
        }
        let ready = matches!(self.video_export.ffmpeg, FfmpegProbe::Found(_)) && !exporting;
        status
            .child(div().flex().justify_end().child(button_enabled(
                "video-export",
                self.t(Key::VideoExportButton),
                ButtonStyle::Primary,
                ButtonState::available(false, ready),
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| this.start_video_export(cx)),
            )))
            .into_any_element()
    }

    fn section(&self, title: Key, theme: &Theme) -> gpui::Div {
        div().flex().flex_col().gap_1p5().child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(self.t(title).to_string()),
        )
    }

    /// A row of mutually exclusive choices.
    fn choices<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: Vec<(T, String)>,
        current: T,
        theme: &Theme,
        cx: &mut Context<Self>,
        apply: fn(&mut NoteRollStyle, T),
    ) -> gpui::Div {
        let mut row = div().flex().flex_wrap().gap_1();
        for (index, (value, caption)) in options.into_iter().enumerate() {
            row = row.child(button(
                ElementId::NamedInteger(id.into(), index as u64),
                caption,
                ButtonStyle::Normal,
                value == current,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.edit_video_style(|style| apply(style, value));
                    cx.notify();
                }),
            ));
        }
        row
    }

    /// A labelled value with buttons either side that step it.
    fn stepper(
        &self,
        id: &'static str,
        label: Key,
        value: String,
        theme: &Theme,
        cx: &mut Context<Self>,
        step: fn(&mut NoteRollStyle, f32),
    ) -> gpui::Div {
        let side = |name: &'static str, caption: &'static str, direction: f32| {
            button(
                ElementId::NamedChild(Box::new(id.into()), name.into()),
                caption,
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.edit_video_style(|style| step(style, direction));
                    cx.notify();
                }),
            )
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .truncate()
                    .child(self.t(label).to_string()),
            )
            .child(side("down", "−", -1.0))
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.to_string())
                    .w(px(64.))
                    .text_xs()
                    .text_center()
                    .child(value),
            )
            .child(side("up", "+", 1.0))
    }

    /// A latched on/off button.
    fn toggle(
        &self,
        id: &'static str,
        label: Key,
        on: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
        flip: fn(&mut NoteRollStyle),
    ) -> gpui::Stateful<gpui::Div> {
        button(
            id,
            self.t(label),
            ButtonStyle::Normal,
            on,
            theme.accent,
            theme,
            cx.listener(move |this, _, _, cx| {
                this.edit_video_style(flip);
                cx.notify();
            }),
        )
    }

    fn swatch(
        id: ElementId,
        color: Color,
        selected: bool,
        theme: &Theme,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let selector = id.to_string();
        div()
            .id(id)
            .debug_selector(move || selector.clone())
            .size(px(22.))
            .rounded(Metrics::RADIUS_SM)
            .bg(hsla(color))
            .border_2()
            .border_color(if selected { theme.accent } else { theme.border })
            .cursor_pointer()
            .on_click(on_click)
    }

    fn video_format_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let sizes = VideoSize::CHOICES
            .into_iter()
            .map(|size| (size, format!("{}×{}", size.width, size.height)))
            .collect();
        let rates = NoteRollStyle::FPS_CHOICES
            .into_iter()
            .map(|fps| (fps, format!("{fps} fps")))
            .collect();
        self.section(Key::VideoSize, theme)
            .child(
                self.choices("video-size", sizes, style.size, theme, cx, |style, size| {
                    style.size = size
                }),
            )
            .child(self.note(self.t(Key::VideoFps), theme))
            .child(
                self.choices("video-fps", rates, style.fps, theme, cx, |style, fps| {
                    style.fps = fps
                }),
            )
    }

    fn video_color_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let modes = vec![
            (
                NoteColoring::Palette,
                self.t(Key::VideoColorPalette).to_string(),
            ),
            (
                NoteColoring::Track,
                self.t(Key::VideoColorTrack).to_string(),
            ),
            (
                NoteColoring::Pitch,
                self.t(Key::VideoColorPitch).to_string(),
            ),
            (
                NoteColoring::Single,
                self.t(Key::VideoColorSingle).to_string(),
            ),
        ];
        let mut section = self.section(Key::VideoColors, theme).child(self.choices(
            "video-coloring",
            modes,
            style.coloring,
            theme,
            cx,
            |style, coloring| style.coloring = coloring,
        ));
        match style.coloring {
            NoteColoring::Palette => {
                let mut row = div().flex().flex_wrap().gap_1();
                for (index, color) in style.palette.iter().copied().enumerate() {
                    row = row.child(Self::swatch(
                        ElementId::NamedInteger("video-palette".into(), index as u64),
                        color,
                        false,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            this.edit_video_style(|style| {
                                if let Some(slot) = style.palette.get_mut(index) {
                                    *slot = next_vivid(*slot);
                                }
                            });
                            cx.notify();
                        }),
                    ));
                }
                section = section
                    .child(row)
                    .child(self.note(self.t(Key::VideoPaletteHint), theme));
            }
            NoteColoring::Single => {
                section = section.child(div().flex().child(Self::swatch(
                    "video-single".into(),
                    style.single_color,
                    false,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.edit_video_style(|style| {
                            style.single_color = next_vivid(style.single_color);
                        });
                        cx.notify();
                    }),
                )));
            }
            NoteColoring::Pitch => {
                section = section
                    .child(self.stepper(
                        "video-hue-low",
                        Key::VideoHueLow,
                        format!("{:.0}°", style.low_hue.rem_euclid(360.0)),
                        theme,
                        cx,
                        |style, direction| {
                            style.low_hue = (style.low_hue + 20.0 * direction).rem_euclid(360.0)
                        },
                    ))
                    .child(self.stepper(
                        "video-hue-high",
                        Key::VideoHueHigh,
                        format!("{:.0}°", style.high_hue.rem_euclid(360.0)),
                        theme,
                        cx,
                        |style, direction| {
                            style.high_hue = (style.high_hue + 20.0 * direction).rem_euclid(360.0)
                        },
                    ));
            }
            NoteColoring::Track => {}
        }
        section.child(self.toggle(
            "video-drums",
            Key::VideoDrums,
            style.include_drums,
            theme,
            cx,
            |style| style.include_drums = !style.include_drums,
        ))
    }

    fn video_background_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut fills = div().flex().flex_wrap().gap_1();
        for (index, color) in BACKGROUNDS.into_iter().enumerate() {
            fills = fills.child(Self::swatch(
                ElementId::NamedInteger("video-background".into(), index as u64),
                color,
                style.background == color,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.edit_video_style(|style| style.background = color);
                    cx.notify();
                }),
            ));
        }
        let mut images = div().flex().flex_wrap().gap_1().child(button(
            "video-choose-image",
            self.t(Key::VideoChooseImage),
            ButtonStyle::Normal,
            false,
            theme.accent,
            theme,
            cx.listener(|this, _, _, cx| this.choose_video_background(cx)),
        ));
        let mut section = self.section(Key::VideoBackground, theme).child(fills);
        if let Some(path) = &style.background_image {
            images = images.child(button(
                "video-clear-image",
                self.t(Key::VideoClearImage),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| {
                    this.edit_video_style(|style| style.background_image = None);
                    cx.notify();
                }),
            ));
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            section = section
                .child(images)
                .child(self.note(&name, theme))
                .child(self.stepper(
                    "video-dim",
                    Key::VideoDim,
                    format!("{:.0}%", style.background_dim * 100.0),
                    theme,
                    cx,
                    |style, direction| {
                        style.background_dim =
                            (style.background_dim + 0.05 * direction).clamp(0.0, 1.0);
                    },
                ));
        } else {
            section = section.child(images);
        }
        section
    }

    fn video_motion_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        self.section(Key::VideoMotion, theme)
            .child(self.stepper(
                "video-speed",
                Key::VideoSpeed,
                format!("{:.0} s", style.seconds_visible),
                theme,
                cx,
                |style, direction| {
                    style.seconds_visible = (style.seconds_visible + direction).clamp(2.0, 30.0);
                },
            ))
            .child(self.stepper(
                "video-playhead",
                Key::VideoPlayhead,
                format!("{:.0}%", style.playhead * 100.0),
                theme,
                cx,
                |style, direction| {
                    style.playhead = (style.playhead + 0.05 * direction).clamp(0.0, 1.0);
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.toggle(
                        "video-playhead-line",
                        Key::VideoPlayheadLine,
                        style.playhead_line,
                        theme,
                        cx,
                        |style| style.playhead_line = !style.playhead_line,
                    ))
                    .child(self.toggle(
                        "video-flash",
                        Key::VideoFlash,
                        style.flash,
                        theme,
                        cx,
                        |style| style.flash = !style.flash,
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .pt_1()
                    .child(self.t(Key::VideoNotes).to_string()),
            )
            .child(self.stepper(
                "video-thickness",
                Key::VideoThickness,
                format!("{:.0}%", style.note_thickness * 100.0),
                theme,
                cx,
                |style, direction| {
                    style.note_thickness = (style.note_thickness + 0.1 * direction).clamp(0.1, 1.0);
                },
            ))
            .child(self.stepper(
                "video-roundness",
                Key::VideoRoundness,
                format!("{:.0}%", style.note_roundness * 200.0),
                theme,
                cx,
                |style, direction| {
                    style.note_roundness =
                        (style.note_roundness + 0.05 * direction).clamp(0.0, 0.5);
                },
            ))
            .child(self.stepper(
                "video-played",
                Key::VideoPlayedOpacity,
                format!("{:.0}%", style.played_opacity * 100.0),
                theme,
                cx,
                |style, direction| {
                    style.played_opacity =
                        (style.played_opacity + 0.05 * direction).clamp(0.0, 1.0);
                },
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness;
    use gpui::TestAppContext;

    #[test]
    fn previews_keep_the_shape_of_the_video() {
        assert_eq!(
            preview_size(VideoSize::new(1920, 1080)),
            VideoSize::new(640, 360)
        );
        assert_eq!(
            preview_size(VideoSize::new(1080, 1920)),
            VideoSize::new(360, 640)
        );
        assert_eq!(
            preview_size(VideoSize::new(1080, 1080)),
            VideoSize::new(640, 640)
        );
    }

    #[test]
    fn swatches_step_through_the_vivid_colours_and_wrap() {
        assert_eq!(next_vivid(VIVID[0]), VIVID[1]);
        assert_eq!(next_vivid(VIVID[15]), VIVID[0]);
        assert_eq!(next_vivid(Color(0x123456)), VIVID[0]);
    }

    #[gpui::test]
    fn the_menu_command_opens_the_window_and_settings_are_remembered(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(crate::actions::ExportVideo);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| {
            assert!(app.video_export.open);
            app.auxiliary_windows[&Surface::VideoExport]
        });
        let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            assert!(app.video_export.preview.is_some(), "a preview was drawn");
        });
        let flash = app.read_with(&utility, |app, _| app.settings.video.flash);
        harness::click("video-flash", &mut utility);
        harness::paint(&app, &mut utility);
        app.read_with(&utility, |app, _| {
            assert_eq!(
                app.settings.video.flash, !flash,
                "the toggle changed the look"
            );
            let drawn = &app.video_export.preview.as_ref().unwrap().0.0;
            assert_eq!(
                drawn, &app.settings.video,
                "and the preview was drawn again in it"
            );
        });
        app.update(&mut utility, |app, _| {
            app.edit_video_style(|style| style.flash = flash);
        });
    }
}
