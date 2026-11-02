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

use crate::app::{AurisApp, Drag, ExportState};
use crate::auxiliary_window::Surface;
use crate::theme::{Metrics, Theme};
use crate::ui::colour_picker::{Hsv, HsvChannel, hex};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{
    ButtonState, ButtonStyle, SliderFill, SliderKeyboardEvent, button, button_enabled, value_slider,
};

/// Width of the preview image; a portrait video is previewed at this height instead.
const PREVIEW_EDGE: u32 = 640;

/// Presets a palette or single-colour swatch's picker offers: bright enough to read on a dark
/// background, and different enough from their neighbours to tell parts apart.
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
    // Dark, but each far enough from black and from the others to be told apart as a swatch:
    // the first set were all within a few steps of black and read as five black squares.
    Color(0x101c3a),
    Color(0x262c36),
    Color(0x2a1640),
    Color(0x10301f),
    Color(0xf4f1ea),
];

/// Colours the chord caption's picker offers: the whites and black that read over most
/// backgrounds, then a few of the note presets.
const CAPTIONS: [Color; 8] = [
    Color(0xffffff),
    Color(0xf4f1ea),
    Color(0x000000),
    Color(0xffd84d),
    Color(0x9ad1ff),
    Color(0x7cf08a),
    Color(0xff8f4d),
    Color(0xff7ad9),
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

/// One of the colours the video's look holds, as the picker addresses it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum VideoColourSlot {
    /// An entry of the palette the tracks take in order.
    Palette(usize),
    /// The one colour every note takes.
    Single,
    /// The fill behind the notes.
    Background,
    /// The chord caption in the corner.
    Chords,
}

impl VideoColourSlot {
    /// The colour in this slot, or `None` for a palette entry past the end.
    pub fn get(self, style: &NoteRollStyle) -> Option<Color> {
        match self {
            VideoColourSlot::Palette(index) => style.palette.get(index).copied(),
            VideoColourSlot::Single => Some(style.single_color),
            VideoColourSlot::Background => Some(style.background),
            VideoColourSlot::Chords => Some(style.chord_color),
        }
    }

    /// Puts `color` in this slot; a palette entry past the end is left alone.
    pub fn set(self, style: &mut NoteRollStyle, color: Color) {
        match self {
            VideoColourSlot::Palette(index) => {
                if let Some(entry) = style.palette.get_mut(index) {
                    *entry = color;
                }
            }
            VideoColourSlot::Single => style.single_color = color,
            VideoColourSlot::Background => style.background = color,
            VideoColourSlot::Chords => style.chord_color = color,
        }
    }

    /// Whether the window shows this slot for `style`: a palette entry only while the notes are
    /// coloured by palette, the single colour only while they take one, the caption's only while
    /// it is drawn.
    fn shown(self, style: &NoteRollStyle) -> bool {
        match self {
            VideoColourSlot::Palette(index) => {
                style.coloring == NoteColoring::Palette && index < style.palette.len()
            }
            VideoColourSlot::Single => style.coloring == NoteColoring::Single,
            VideoColourSlot::Background => true,
            VideoColourSlot::Chords => style.chords,
        }
    }

    /// A stable name for element ids and test selectors.
    fn id(self) -> String {
        match self {
            VideoColourSlot::Palette(index) => format!("palette-{index}"),
            VideoColourSlot::Single => "single".to_owned(),
            VideoColourSlot::Background => "background".to_owned(),
            VideoColourSlot::Chords => "chords".to_owned(),
        }
    }
}

/// The colour picker that is open, and what it holds while it is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ColourPicking {
    /// Which colour it edits.
    pub(crate) slot: VideoColourSlot,
    /// The colour as the sliders hold it, which keeps a hue through grey.
    pub(crate) hsv: Hsv,
}

/// The window's state, kept on the document's view so the window itself stays a view.
#[derive(Default)]
pub(crate) struct VideoExportState {
    /// Whether the window is open.
    pub(crate) open: bool,
    /// The colour picker open under a swatch, if one is.
    pub(crate) picking: Option<ColourPicking>,
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

/// How a frame size is named: width by height, in pixels.
fn size_label(size: VideoSize) -> String {
    format!("{}×{}", size.width, size.height)
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

    /// Opens the picker on `slot`, or closes it if it was already open there.
    pub(crate) fn toggle_video_colour_picker(&mut self, slot: VideoColourSlot) {
        let open_here = self
            .video_export
            .picking
            .is_some_and(|picking| picking.slot == slot);
        self.video_export.picking = match (open_here, slot.get(&self.settings.video)) {
            (false, Some(color)) => Some(ColourPicking {
                slot,
                hsv: Hsv::from_color(color),
            }),
            _ => None,
        };
    }

    /// Puts a whole colour in a slot — a preset, or a typed code — and remembers it.
    pub(crate) fn set_video_colour(&mut self, slot: VideoColourSlot, color: Color) {
        self.edit_video_style(|style| slot.set(style, color));
        if let Some(picking) = self.video_export.picking.as_mut()
            && picking.slot == slot
        {
            picking.hsv = Hsv::from_color(color);
        }
    }

    /// Moves one of the open picker's sliders to `fraction`.
    ///
    /// `save` is false while a drag is under way: the look is changed at once, so the preview
    /// follows the hand, and written to the settings file once, when the drag lets go.
    pub(crate) fn set_video_colour_channel(
        &mut self,
        channel: HsvChannel,
        fraction: f32,
        save: bool,
    ) {
        let Some(picking) = self.video_export.picking.as_mut() else {
            return;
        };
        picking.hsv = picking.hsv.with(channel, fraction);
        let (slot, color) = (picking.slot, picking.hsv.to_color());
        if save {
            self.edit_video_style(|style| slot.set(style, color));
        } else {
            slot.set(&mut self.settings.video, color);
        }
    }

    /// Closes a picker whose slot is no longer on screen, and catches its sliders up with a
    /// colour that was changed some other way.
    fn sync_video_colour_picker(&mut self) {
        let style = &self.settings.video;
        let Some(picking) = self.video_export.picking.as_mut() else {
            return;
        };
        match picking.slot.get(style) {
            Some(color) if picking.slot.shown(style) => {
                if picking.hsv.to_color() != color {
                    picking.hsv = Hsv::from_color(color);
                }
            }
            _ => self.video_export.picking = None,
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
        self.sync_video_colour_picker();
        let theme = self.theme.clone();
        let style = self.settings.video.clone();
        let preview = self.video_preview();
        let empty = self
            .video_export
            .score
            .as_ref()
            .is_some_and(|(_, score)| score.notes.is_empty());

        // Two columns while there is room for both, one scrolling column below the split
        // width, as Atelier lays out any two panes that share a region.
        let stacked = window.viewport_size().width < theme.atelier.density.split_width;

        let mut picture = div()
            .id("video-preview")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(if stacked { 220. } else { 300. }))
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

        let settings = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_3()
            .child(self.video_format_section(&style, &theme, cx))
            .child(self.video_color_section(&style, &theme, cx))
            .child(self.video_background_section(&style, &theme, cx))
            .child(self.video_motion_section(&style, &theme, cx))
            .child(self.video_chords_section(&style, &theme, cx))
            .child(self.video_lyrics_section(&style, &theme, cx));

        let body = if stacked {
            div()
                .id("video-body")
                .flex()
                .flex_col()
                .gap_4()
                .p_4()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(left.flex_none())
                .child(settings.w_full())
                .into_any_element()
        } else {
            div()
                .flex()
                .gap_4()
                .p_4()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("video-preview-column")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .child(left),
                )
                .child(
                    div()
                        .id("video-settings")
                        .w(px(330.))
                        .flex_none()
                        .h_full()
                        .overflow_y_scroll()
                        .pr_1()
                        .child(settings),
                )
                .into_any_element()
        };

        // The window's one commit, at the trailing end of a foot of its own, where every other
        // dialog in the application keeps it. It used to float under the notes in the left
        // column, wherever they happened to end.
        let exporting =
            self.choosing_export || self.export.as_ref().is_some_and(|e| e.result.is_none());
        let ready =
            matches!(self.video_export.ffmpeg, FfmpegProbe::Found(_)) && !exporting && !empty;
        let footer = div()
            .flex()
            .flex_none()
            .justify_end()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(theme.border_subtle)
            .child(button_enabled(
                "video-export",
                self.t(Key::VideoExportButton),
                ButtonStyle::Primary,
                ButtonState::available(false, ready),
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| this.start_video_export(cx)),
            ));

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(body)
            .child(footer)
            .into_any_element()
    }

    fn note(&self, text: &str, theme: &Theme) -> gpui::Div {
        div()
            .text_xs()
            .text_color(theme.text_muted)
            .child(text.to_string())
    }

    fn ffmpeg_status(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
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
        status.into_any_element()
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

    /// An on/off setting, applied at once: a switch with its label beside it.
    fn toggle(
        &self,
        id: &'static str,
        label: Key,
        on: bool,
        _theme: &Theme,
        cx: &mut Context<Self>,
        flip: fn(&mut NoteRollStyle),
    ) -> atelier::Switch {
        atelier::Switch::new(id, on)
            .label(self.t(label))
            .on_change(cx.listener(move |this, _: &bool, _, cx| {
                this.edit_video_style(flip);
                cx.notify();
            }))
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
            .debug_selector(move || selector.clone()) // atelier: allow — a colour swatch has no component.
            .tab_index(0)
            .key_context(atelier::CONTROL_CONTEXT)
            .size(px(22.))
            .rounded(Metrics::RADIUS_SM)
            .bg(hsla(color))
            .border_2()
            .border_color(if selected { theme.accent } else { theme.border })
            .focus(|this| this.border_color(theme.selection))
            .on_click(on_click)
    }

    fn video_format_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let size = style.size;
        let fps = NoteRollStyle::FPS_CHOICES
            .iter()
            .position(|choice| *choice == style.fps)
            .unwrap_or(usize::MAX);
        self.section(Key::VideoSize, theme)
            // Five sizes with long names: a menu, which the choosing guide gives anything past a
            // handful, rather than a row of buttons that wrapped onto two lines.
            .child(
                crate::ui::widgets::menu_picker(
                    "video-size",
                    div().text_xs().child(size_label(size)),
                    theme,
                    Self::opens_menu(cx, move |this, at| this.video_size_menu(at, size)),
                )
                .w(px(180.)),
            )
            .child(self.note(self.t(Key::VideoFps), theme))
            .child(
                div().flex().child(
                    atelier::SegmentedControl::new(
                        "video-fps",
                        NoteRollStyle::FPS_CHOICES
                            .map(|fps| atelier::Segment::label(format!("{fps} fps"))),
                        fps,
                    )
                    .size(atelier::ControlSize::Small)
                    .on_select(cx.listener(|this, index: &usize, _, cx| {
                        if let Some(fps) = NoteRollStyle::FPS_CHOICES.get(*index).copied() {
                            this.edit_video_style(|style| style.fps = fps);
                        }
                        cx.notify();
                    })),
                ),
            )
    }

    /// The frame sizes on offer, the current one ticked.
    fn video_size_menu(&self, at: gpui::Point<gpui::Pixels>, current: VideoSize) -> ContextMenu {
        VideoSize::CHOICES.into_iter().fold(
            ContextMenu::new(at, self.t(Key::VideoSize)),
            |menu, size| {
                menu.toggle(
                    size_label(size),
                    MenuCommand::VideoSize(size),
                    size == current,
                )
            },
        )
    }

    /// Opens the sheet a colour code is typed into.
    fn open_video_colour_code(&mut self, slot: VideoColourSlot) {
        let Some(color) = slot.get(&self.settings.video) else {
            return;
        };
        self.open_prompt(Prompt::new(
            self.t(Key::VideoColourCode),
            PromptTarget::VideoColour(slot),
            hex(color),
        ));
    }

    /// A swatch that opens and closes the picker on its slot.
    fn picker_swatch(
        &self,
        slot: VideoColourSlot,
        color: Color,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self
            .video_export
            .picking
            .is_some_and(|picking| picking.slot == slot);
        Self::swatch(
            ElementId::Name(format!("video-swatch-{}", slot.id()).into()),
            color,
            open,
            theme,
            cx.listener(move |this, _, _, cx| {
                this.toggle_video_colour_picker(slot);
                cx.notify();
            }),
        )
    }

    /// The open picker, when it is open on `slot`: the colour and its code, the three sliders,
    /// and `presets` to start from.
    fn video_colour_picker(
        &self,
        slot: VideoColourSlot,
        presets: &[Color],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let picking = self
            .video_export
            .picking
            .filter(|picking| picking.slot == slot)?;
        let hsv = picking.hsv;
        let color = hsv.to_color();
        let name = slot.id();
        let head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(hsla(color))
                    .border_1()
                    .border_color(theme.border),
            )
            .child(button(
                ElementId::Name(format!("video-colour-code-{name}").into()),
                hex(color),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.open_video_colour_code(slot);
                    cx.notify();
                }),
            ));
        let selector = format!("video-picker-{name}");
        let mut panel = div()
            .id(ElementId::Name(selector.clone().into()))
            .debug_selector(move || selector.clone()) // atelier: allow — the picker is the application's own; Atelier has none yet.
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded(Metrics::RADIUS_MD)
            .border_1()
            .border_color(theme.border_subtle)
            .child(head);
        for channel in HsvChannel::ALL {
            let fraction = hsv.fraction(channel);
            let label = match channel {
                HsvChannel::Hue => Key::VideoHue,
                HsvChannel::Saturation => Key::VideoSaturation,
                HsvChannel::Value => Key::VideoBrightness,
            };
            panel = panel.child(value_slider(
                ElementId::Name(format!("video-{}-{name}", channel.id()).into()),
                self.t(label),
                hsv.label(channel),
                fraction,
                hsla(hsv.slider_fill(channel)),
                SliderFill::FromStart,
                // A degree of hue, a percent of the others: the steps their readouts count in.
                match channel {
                    HsvChannel::Hue => 1.0 / 360.0,
                    HsvChannel::Saturation | HsvChannel::Value => 0.01,
                },
                theme,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                    this.begin_drag(Drag::VideoColour {
                        channel,
                        start_fraction: fraction,
                        start_x: event.position.x,
                    });
                }),
                cx.listener(move |this, event: &SliderKeyboardEvent, _, cx| {
                    this.set_video_colour_channel(channel, event.fraction, true);
                    cx.notify();
                }),
            ));
        }
        if !presets.is_empty() {
            let mut row = div().flex().flex_wrap().gap_1().pt_1();
            for (index, preset) in presets.iter().copied().enumerate() {
                row = row.child(Self::swatch(
                    ElementId::NamedInteger(format!("video-preset-{name}").into(), index as u64),
                    preset,
                    preset == color,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.set_video_colour(slot, preset);
                        cx.notify();
                    }),
                ));
            }
            panel = panel.child(row);
        }
        Some(panel)
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
        let selected = modes
            .iter()
            .position(|(mode, _)| *mode == style.coloring)
            .unwrap_or(usize::MAX);
        let order: Vec<NoteColoring> = modes.iter().map(|(mode, _)| *mode).collect();
        let mut section = self.section(Key::VideoColors, theme).child(
            div().flex().child(
                atelier::SegmentedControl::new(
                    "video-coloring",
                    modes
                        .into_iter()
                        .map(|(_, caption)| atelier::Segment::label(caption)),
                    selected,
                )
                .size(atelier::ControlSize::Small)
                .on_select(cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(coloring) = order.get(*index).copied() {
                        this.edit_video_style(|style| style.coloring = coloring);
                    }
                    cx.notify();
                })),
            ),
        );
        match style.coloring {
            NoteColoring::Palette => {
                let mut row = div().flex().flex_wrap().gap_1();
                for (index, color) in style.palette.iter().copied().enumerate() {
                    row = row.child(self.picker_swatch(
                        VideoColourSlot::Palette(index),
                        color,
                        theme,
                        cx,
                    ));
                }
                section = section.child(row);
                if let Some(picker) = (0..style.palette.len()).find_map(|index| {
                    self.video_colour_picker(VideoColourSlot::Palette(index), &VIVID, theme, cx)
                }) {
                    section = section.child(picker);
                }
                section = section.child(self.note(self.t(Key::VideoPaletteHint), theme));
            }
            NoteColoring::Single => {
                section = section
                    .child(div().flex().child(self.picker_swatch(
                        VideoColourSlot::Single,
                        style.single_color,
                        theme,
                        cx,
                    )))
                    .children(self.video_colour_picker(VideoColourSlot::Single, &VIVID, theme, cx));
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

    fn video_chords_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut section = self
            .section(Key::VideoChordSection, theme)
            .child(self.toggle(
                "video-chords",
                Key::VideoChords,
                style.chords,
                theme,
                cx,
                |style| style.chords = !style.chords,
            ));
        if style.chords {
            section = section
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.picker_swatch(
                            VideoColourSlot::Chords,
                            style.chord_color,
                            theme,
                            cx,
                        ))
                        .child(self.note(self.t(Key::VideoChordColour), theme)),
                )
                .children(self.video_colour_picker(VideoColourSlot::Chords, &CAPTIONS, theme, cx));
        }
        section.child(self.note(self.t(Key::VideoChordsHint), theme))
    }

    fn video_lyrics_section(
        &self,
        style: &NoteRollStyle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        self.section(Key::VideoLyricSection, theme)
            .child(self.toggle(
                "video-lyrics",
                Key::VideoLyrics,
                style.lyrics,
                theme,
                cx,
                |style| style.lyrics = !style.lyrics,
            ))
            .child(self.note(self.t(Key::VideoLyricsHint), theme))
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
                    this.set_video_colour(VideoColourSlot::Background, color);
                    cx.notify();
                }),
            ));
        }
        // The presets are the row itself, so the picker under it holds only the sliders and
        // the code. Its button reads as on while the fill in use is none of the presets.
        let custom = !BACKGROUNDS.contains(&style.background);
        let picking = self
            .video_export
            .picking
            .is_some_and(|picking| picking.slot == VideoColourSlot::Background);
        fills = fills.child(button(
            "video-background-custom",
            self.t(Key::VideoCustomColour),
            ButtonStyle::Normal,
            picking || custom,
            theme.accent,
            theme,
            cx.listener(|this, _, _, cx| {
                this.toggle_video_colour_picker(VideoColourSlot::Background);
                cx.notify();
            }),
        ));
        let mut images = div().flex().flex_wrap().gap_1().child(button(
            "video-choose-image",
            self.t(Key::VideoChooseImage),
            ButtonStyle::Normal,
            false,
            theme.accent,
            theme,
            cx.listener(|this, _, _, cx| this.choose_video_background(cx)),
        ));
        let mut section = self
            .section(Key::VideoBackground, theme)
            .child(fills)
            .children(self.video_colour_picker(VideoColourSlot::Background, &[], theme, cx));
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
                    .flex_col()
                    .gap_2()
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
            .child(self.stepper(
                "video-sink",
                Key::VideoSink,
                if style.sink <= 0.0 {
                    self.t(Key::VideoParticlesOff).to_string()
                } else {
                    format!("{:.0}%", style.sink * 100.0)
                },
                theme,
                cx,
                |style, direction| {
                    style.sink =
                        (style.sink + 0.25 * direction).clamp(0.0, NoteRollStyle::MAX_SINK);
                },
            ))
            .child(self.stepper(
                "video-particles",
                Key::VideoParticles,
                if style.particles <= 0.0 {
                    self.t(Key::VideoParticlesOff).to_string()
                } else {
                    format!("{:.0}%", style.particles * 100.0)
                },
                theme,
                cx,
                |style, direction| {
                    style.particles = (style.particles + 0.25 * direction)
                        .clamp(0.0, NoteRollStyle::MAX_PARTICLES);
                },
            ))
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

    /// The video window, open and painted, with the look it started with to put back.
    fn open_window(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<AurisApp>,
        gpui::VisualTestContext,
        NoteRollStyle,
    ) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(crate::actions::ExportVideo);
        harness::paint(&app, cx);
        let (handle, style) = app.read_with(cx, |app, _| {
            (
                app.auxiliary_windows[&Surface::VideoExport],
                app.settings.video.clone(),
            )
        });
        let mut window = gpui::VisualTestContext::from_window(handle.into(), cx);
        harness::paint(&app, &mut window);
        (app, window, style)
    }

    #[gpui::test]
    fn a_swatch_opens_a_picker_whose_sliders_and_presets_set_the_colour(cx: &mut TestAppContext) {
        let (app, mut cx, original) = open_window(cx);
        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| {
                style.coloring = NoteColoring::Palette;
                style.palette[0] = Color(0x3d7bff);
            });
        });
        harness::paint(&app, &mut cx);
        assert!(cx.debug_bounds("video-picker-palette-0").is_none());

        harness::click("video-swatch-palette-0", &mut cx);
        harness::paint(&app, &mut cx);
        assert!(
            cx.debug_bounds("video-picker-palette-0").is_some(),
            "the picker opens under the palette"
        );

        // A preset is still one click away.
        harness::click("video-preset-palette-0-4", &mut cx);
        harness::paint(&app, &mut cx);
        app.read_with(&cx, |app, _| {
            assert_eq!(app.settings.video.palette[0], VIVID[4]);
        });

        // Dragging brightness to the bottom makes black, and back up again restores the hue
        // the colour had: the picker holds it through black.
        let bar = cx.debug_bounds("video-value-palette-0").unwrap();
        let from = bar.center();
        harness::drag(&mut cx, from, from - gpui::point(px(240.), px(0.)));
        harness::paint(&app, &mut cx);
        app.read_with(&cx, |app, _| {
            assert_eq!(app.settings.video.palette[0], Color(0x000000));
        });
        // Back up in two shorter strokes, which keep the pointer inside the window: a full
        // sweep is [`crate::ui::widgets::DRAG_RANGE_PIXELS`].
        for _ in 0..2 {
            let from = cx.debug_bounds("video-value-palette-0").unwrap().center();
            harness::drag(&mut cx, from, from + gpui::point(px(120.), px(0.)));
            harness::paint(&app, &mut cx);
        }
        app.read_with(&cx, |app, _| {
            let back = Hsv::from_color(app.settings.video.palette[0]);
            let preset = Hsv::from_color(VIVID[4]);
            assert!(
                (back.hue - preset.hue).abs() < 1.0,
                "{back:?} against {preset:?}"
            );
            assert_eq!(back.value, 1.0);
        });

        // Clicking the open swatch again closes it.
        harness::click("video-swatch-palette-0", &mut cx);
        harness::paint(&app, &mut cx);
        app.read_with(&cx, |app, _| assert_eq!(app.video_export.picking, None));

        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| *style = original);
        });
    }

    #[gpui::test]
    fn a_colour_code_is_typed_and_a_wrong_one_is_refused(cx: &mut TestAppContext) {
        let (app, mut cx, original) = open_window(cx);
        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| style.coloring = NoteColoring::Single);
        });
        harness::paint(&app, &mut cx);
        harness::click("video-swatch-single", &mut cx);
        harness::paint(&app, &mut cx);
        harness::click("video-colour-code-single", &mut cx);
        harness::paint(&app, &mut cx);

        let answer = |text: &str, cx: &mut gpui::VisualTestContext| {
            app.update(cx, |app, _| {
                let Some(Prompt {
                    body: crate::ui::prompt::PromptBody::Text { target, field },
                    ..
                }) = app.prompt.as_mut()
                else {
                    panic!("the code sheet is open");
                };
                assert_eq!(*target, PromptTarget::VideoColour(VideoColourSlot::Single));
                let all = 0..field.content().len();
                field.replace(all, text);
            });
            cx.simulate_keystrokes("enter");
            harness::paint(&app, cx);
        };
        answer("blue", &mut cx);
        app.read_with(&cx, |app, _| {
            assert!(
                app.prompt.is_some(),
                "a word is not a colour, and the sheet stays"
            );
        });
        answer("#38f", &mut cx);
        app.read_with(&cx, |app, _| {
            assert!(app.prompt.is_none());
            assert_eq!(app.settings.video.single_color, Color(0x3388ff));
            let picking = app.video_export.picking.unwrap();
            assert_eq!(
                picking.hsv.to_color(),
                Color(0x3388ff),
                "the sliders follow"
            );
        });

        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| *style = original);
        });
    }

    #[gpui::test]
    fn the_background_takes_any_colour_beside_its_presets(cx: &mut TestAppContext) {
        let (app, mut cx, original) = open_window(cx);
        harness::click("video-background-custom", &mut cx);
        harness::paint(&app, &mut cx);
        assert!(cx.debug_bounds("video-picker-background").is_some());
        app.update(&mut cx, |app, _| {
            app.set_video_colour_channel(HsvChannel::Value, 0.5, true);
            app.set_video_colour_channel(HsvChannel::Saturation, 1.0, true);
            app.set_video_colour_channel(HsvChannel::Hue, 0.0, true);
            assert_eq!(app.settings.video.background, Color(0x800000));
        });
        // A preset in the row above moves the open picker's sliders too.
        harness::click("video-background-0", &mut cx);
        harness::paint(&app, &mut cx);
        app.read_with(&cx, |app, _| {
            assert_eq!(app.settings.video.background, BACKGROUNDS[0]);
            assert_eq!(app.video_export.picking.unwrap().hsv.value, 0.0);
        });
        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| *style = original);
        });
    }

    #[gpui::test]
    fn the_chord_caption_takes_any_colour_while_it_is_shown(cx: &mut TestAppContext) {
        let (app, mut cx, original) = open_window(cx);
        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| style.chords = true);
        });
        // Tall enough that the chord section, near the end of the settings, is on screen.
        harness::resize(&app, &mut cx, gpui::size(px(1100.), px(1600.)));
        harness::click("video-swatch-chords", &mut cx);
        harness::paint(&app, &mut cx);
        harness::click("video-preset-chords-3", &mut cx);
        harness::paint(&app, &mut cx);
        app.update(&mut cx, |app, _| {
            assert_eq!(app.settings.video.chord_color, CAPTIONS[3]);
            app.set_video_colour_channel(HsvChannel::Value, 0.5, true);
            assert_ne!(app.settings.video.chord_color, CAPTIONS[3]);
            let drawn = app.settings.video.chord_color;
            assert_eq!(
                app.settings.video.playhead_color, original.playhead_color,
                "the playhead line keeps its own colour"
            );
            // Hiding the caption takes its picker away with it.
            app.edit_video_style(|style| style.chords = false);
            assert_eq!(app.settings.video.chord_color, drawn);
        });
        harness::paint(&app, &mut cx);
        app.read_with(&cx, |app, _| assert_eq!(app.video_export.picking, None));
        app.update(&mut cx, |app, _| {
            app.edit_video_style(|style| *style = original);
        });
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
