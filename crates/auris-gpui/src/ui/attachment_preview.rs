//! Shared file picking and local previews for both conversation surfaces.

use super::agent_components;
use super::audio_player::{PlayerState, PlayerTarget};
use crate::app::AurisApp;
use auris_i18n::Key;
use gpui::{Context, IntoElement, Window, div, prelude::*};
use std::{io::Read, path::PathBuf, sync::Arc};

enum Content {
    Text(String),
    Image(Arc<gpui::RenderImage>),
    Audio(Arc<auris_session::prelude::AudioBuffer>),
    Midi(auris_session::midi_reference::MidiReference),
}

pub(crate) struct AttachmentPreview {
    serial: u64,
    workshop: bool,
    pub(super) path: PathBuf,
    content: Option<Content>,
    pub(super) error: Option<String>,
    pub(crate) player: PlayerState,
}

fn load(path: &std::path::Path, rate: f64) -> Result<Content, String> {
    if !path.is_absolute() {
        return Err("Attachment path is not absolute".into());
    }
    let kind = auris_agent::attachment_kind(path).ok_or("Unsupported file type")?;
    match kind {
        auris_agent::AttachmentKind::Midi => {
            let score = auris_session::midi_reference::MidiReference::read(path)?;
            Ok(Content::Midi(score))
        }
        auris_agent::AttachmentKind::Text => {
            let bytes = read_bounded(path, 256 * 1024)?;
            Ok(Content::Text(
                String::from_utf8(bytes).map_err(|e| e.to_string())?,
            ))
        }
        auris_agent::AttachmentKind::Image => {
            let bytes = read_bounded(path, 32 * 1024 * 1024)?;
            let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .map_err(|e| e.to_string())?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            use image::ImageDecoder;
            let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
            let orientation = decoder.orientation().map_err(|e| e.to_string())?;
            let mut image =
                image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
            image.apply_orientation(orientation);
            let mut decoded = image.thumbnail(1024, 1024).into_rgba8();
            for pixel in decoded.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            Ok(Content::Image(Arc::new(gpui::RenderImage::new(vec![
                image::Frame::new(decoded),
            ]))))
        }
        auris_agent::AttachmentKind::Audio => {
            if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
                return Err("Audio exceeds the 512 MiB file limit".into());
            }
            auris_session::decode_audio(path, rate)
                .map(|audio| Content::Audio(Arc::new(audio)))
                .map_err(|e| e.to_string())
        }
    }
}

fn read_bounded(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("File is too large to preview".into());
    }
    Ok(bytes)
}

/// A MIDI reference at a glance: Atelier's `NoteRoll`, a part per track in its categorical
/// colour, on the sunken surface every visualisation sits on.
fn midi_preview(
    score: &auris_session::midi_reference::MidiReference,
    theme: &crate::theme::Theme,
) -> gpui::AnyElement {
    let notes = score
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(part, track)| {
            track.notes.iter().map(move |note| {
                atelier::viz::RollNote::new(note.pitch, note.start.0 as f32, note.length.0 as f32)
                    .part(part)
            })
        })
        .collect();
    div()
        .w_full()
        .h(gpui::px(120.0))
        .overflow_hidden()
        .rounded(theme.atelier.shape.radius_md)
        .bg(theme.atelier.colors.sunken)
        .child(atelier::viz::NoteRoll::new(notes))
        .into_any_element()
}

impl AurisApp {
    /// Load inline references once, off the UI thread, without opening a preview pane.
    pub(crate) fn load_inline_attachments(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if self
                .inline_attachments
                .iter()
                .any(|preview| preview.path == path)
            {
                continue;
            }
            let index = self.inline_attachments.len();
            self.inline_attachments.push(AttachmentPreview {
                serial: 0,
                workshop: false,
                path: path.clone(),
                content: None,
                error: None,
                player: PlayerState::default(),
            });
            let rate = self.session.sample_rate();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { load(&path, rate) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    let preview = &mut this.inline_attachments[index];
                    match result {
                        Ok(content) => preview.content = Some(content),
                        Err(error) => preview.error = Some(error),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Embed a reference directly beside its message or draft attachment controls.
    pub(crate) fn inline_attachment(
        &self,
        path: &std::path::Path,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some((index, preview)) = self
            .inline_attachments
            .iter()
            .enumerate()
            .find(|(_, preview)| preview.path == path)
        else {
            return div().into_any_element();
        };
        let content = match &preview.content {
            Some(Content::Image(image)) => div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-image")
                })
                .w_full()
                .child(
                    gpui::img(Arc::clone(image))
                        .w_full()
                        .h(gpui::px(240.0))
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            Some(Content::Text(text)) => div()
                .id(gpui::SharedString::from(format!("{id}-text")))
                .max_h(gpui::px(240.0))
                .overflow_y_scroll()
                .child(agent_components::selectable_text(
                    gpui::SharedString::from(format!("{id}-content")),
                    text.clone(),
                    window,
                    cx,
                ))
                .into_any_element(),
            Some(Content::Audio(audio)) => self.render_audio_player(
                PlayerTarget::InlineAttachment(index),
                audio.duration_seconds(),
                true,
                false,
                Some(format!("{id}-player")),
                cx,
            ),
            Some(Content::Midi(score)) => midi_preview(score, &self.theme),
            None => preview.error.as_ref().map_or_else(
                || {
                    div()
                        .child(self.t(Key::AttachmentLoading))
                        .into_any_element()
                },
                |error| agent_components::error_notice(error.clone()),
            ),
        };
        div()
            .id(gpui::SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .text_color(self.theme.text)
            .child(content)
            .children(
                preview
                    .error
                    .as_ref()
                    .filter(|_| preview.content.is_some())
                    .map(|error| agent_components::error_notice(error.clone())),
            )
            .into_any_element()
    }

    /// Play an embedded audio reference through its own transport state.
    pub(crate) fn play_inline_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.acquire_audio(cx) {
            return;
        }
        let Some(preview) = self.inline_attachments.get_mut(index) else {
            return;
        };
        let Some(Content::Audio(audio)) = &preview.content else {
            return;
        };
        match self.session.play_output_preview_with(
            Arc::clone(audio),
            preview.player.options(audio.duration_seconds()),
        ) {
            Ok(status) => preview.player.start(status),
            Err(error) => preview.error = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn pick_conversation_files(&mut self, workshop: bool, cx: &mut Context<Self>) {
        let document = self
            .session
            .project_folder()
            .map(std::path::Path::to_path_buf);
        let identity = self.song_workshop.as_ref().map(|s| s.identity);
        cx.spawn(async move |this, cx| {
            let Some(files) = rfd::AsyncFileDialog::new().pick_files().await else {
                return;
            };
            let paths: Vec<_> = files.iter().map(|f| f.path().to_path_buf()).collect();
            let _ = this.update(cx, |this, cx| {
                if this.session.project_folder() != document.as_deref() {
                    return;
                }
                if workshop {
                    if this.song_workshop.as_ref().map(|s| s.identity) == identity {
                        this.workshop_attach_files(&paths, cx);
                    }
                } else {
                    this.agent_attach_files(&paths, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn open_attachment_preview(
        &mut self,
        path: PathBuf,
        workshop: bool,
        cx: &mut Context<Self>,
    ) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.attachment_preview = Some(AttachmentPreview {
            serial,
            workshop,
            path: path.clone(),
            content: None,
            error: None,
            player: PlayerState::default(),
        });
        let rate = self.session.sample_rate();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&path, rate) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(preview) = &mut this.attachment_preview
                    && preview.serial == serial
                {
                    match result {
                        Ok(content) => preview.content = Some(content),
                        Err(error) => preview.error = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn play_attachment_preview(&mut self, cx: &mut Context<Self>) {
        if !self.acquire_audio(cx) {
            return;
        }
        let Some(preview) = &mut self.attachment_preview else {
            return;
        };
        let Some(Content::Audio(audio)) = &preview.content else {
            return;
        };
        match self.session.play_output_preview_with(
            Arc::clone(audio),
            preview.player.options(audio.duration_seconds()),
        ) {
            Ok(status) => preview.player.start(status),
            Err(error) => preview.error = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn render_attachment_preview(
        &self,
        workshop: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(preview) = self
            .attachment_preview
            .as_ref()
            .filter(|p| p.workshop == workshop)
        else {
            return div().into_any_element();
        };
        let title = preview
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let theme = &self.theme;
        div()
            .id("attachment-preview")
            .debug_selector(|| "attachment-preview".into())
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.agent_chat.focused = None;
                }),
            )
            .flex()
            .flex_col()
            .gap(theme.atelier.spacing.sm)
            .p(theme.atelier.spacing.md)
            .min_w_0()
            .max_h(gpui::px(320.0))
            .overflow_y_scroll()
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.atelier.spacing.sm)
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        atelier::Button::new(
                            "close-attachment-preview",
                            self.t(Key::AttachmentClosePreview),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.attachment_preview = None;
                            cx.notify();
                        })),
                    ),
            )
            .child(match &preview.content {
                Some(Content::Text(text)) => {
                    agent_components::selectable_text("attachment-text", text.clone(), window, cx)
                        .into_any_element()
                }
                Some(Content::Image(image)) => gpui::img(Arc::clone(image))
                    .w_full()
                    .max_h(gpui::px(240.0))
                    .object_fit(gpui::ObjectFit::Contain)
                    .into_any_element(),
                Some(Content::Audio(audio)) => self.render_audio_player(
                    PlayerTarget::Attachment,
                    audio.duration_seconds(),
                    true,
                    false,
                    None,
                    cx,
                ),
                Some(Content::Midi(score)) => midi_preview(score, theme),
                None => div()
                    .child(if preview.error.is_some() {
                        ""
                    } else {
                        self.t(Key::AttachmentLoading)
                    })
                    .into_any_element(),
            })
            .when_some(preview.error.clone(), |this, error| {
                this.child(agent_components::error_notice(error))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_text_images_midi_and_audio_without_editing_a_project() {
        let folder = tempfile::tempdir().unwrap();
        let text = folder.path().join("lyrics.md");
        std::fs::write(&text, "# Lyrics\n\n  夜の雨").unwrap();
        assert!(
            matches!(load(&text, 48000.0).unwrap(), Content::Text(text) if text == "# Lyrics\n\n  夜の雨")
        );
        let midi = folder.path().join("melody.auris-midi");
        let score = auris_session::midi_reference::MidiReference::new(
            "Melody".into(),
            Default::default(),
            Default::default(),
        );
        std::fs::write(&midi, score.encode().unwrap()).unwrap();
        assert!(
            matches!(load(&midi, 48000.0).unwrap(), Content::Midi(reference) if reference == score)
        );
        for (extension, format) in [
            ("png", image::ImageFormat::Png),
            ("jpg", image::ImageFormat::Jpeg),
            ("gif", image::ImageFormat::Gif),
            ("webp", image::ImageFormat::WebP),
        ] {
            let path = folder.path().join(format!("reference.{extension}"));
            image::DynamicImage::new_rgb8(8, 8)
                .save_with_format(&path, format)
                .unwrap();
            assert!(matches!(load(&path, 48000.0).unwrap(), Content::Image(_)));
        }
        let path = folder.path().join("reference.wav");
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&96000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0, 0, 0]);
        std::fs::write(&path, wav).unwrap();
        assert!(
            matches!(load(&path, 48000.0).unwrap(), Content::Audio(audio) if audio.frame_count() == 2)
        );
        assert!(load(std::path::Path::new("relative.txt"), 48000.0).is_err());
    }

    #[gpui::test]
    fn previews_can_be_replaced_and_closed_on_both_surfaces(cx: &mut gpui::TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("lyrics.txt");
        std::fs::write(&path, "夜の雨").unwrap();
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.panels.show(crate::dock::Panel::Agent);
            this.open_attachment_preview(path.clone(), false, cx);
        });
        cx.run_until_parked();
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("attachment-preview").is_some());
        app.read_with(cx, |this, _| {
            assert!(matches!(&this.attachment_preview.as_ref().unwrap().content, Some(Content::Text(text)) if text == "夜の雨"));
        });
        crate::harness::click("agent-input", cx);
        crate::harness::select_text(&app, cx, "attachment-text");
        cx.simulate_keystrokes("secondary-c");
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("夜の雨")
            )
        });
        crate::harness::click("close-attachment-preview", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.attachment_preview.is_none(),
                "close button must dismiss the preview"
            )
        });
        app.update(cx, |this, cx| {
            this.open_attachment_preview(path.clone(), false, cx);
            this.open_attachment_preview(folder.path().join("missing.txt"), false, cx);
        });
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(this.attachment_preview.as_ref().unwrap().error.is_some())
        });
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
                this.open_attachment_preview(path, true, cx);
            })
        });
        cx.run_until_parked();
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("attachment-preview").is_some());
        crate::harness::click("close-attachment-preview", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.attachment_preview.is_none()));
    }
}
