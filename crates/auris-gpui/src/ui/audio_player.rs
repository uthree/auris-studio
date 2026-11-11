//! Shared controls for finite audio previews. The callback owns the playback clock.

use super::widgets::{ButtonState, ButtonStyle, button_with_content};
use crate::app::AurisApp;
use auris_session::OutputPreviewStatus;
use gpui::{AnyElement, Context, IntoElement, SharedString, canvas, div, prelude::*, relative};

pub(crate) struct PlayerState {
    status: Option<OutputPreviewStatus>,
    position: f64,
    volume: f32,
    muted: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: None,
            position: 0.0,
            volume: 1.0,
            muted: false,
        }
    }
}

impl Drop for PlayerState {
    fn drop(&mut self) {
        self.stop();
    }
}

impl PlayerState {
    pub(crate) fn reset_audio(&mut self) {
        self.stop();
        self.position = 0.0;
    }

    pub(crate) fn stop(&mut self) {
        self.position = self.position();
        if let Some(status) = self.status.take() {
            status.stop();
        }
    }

    pub(crate) fn start(&mut self, status: OutputPreviewStatus) {
        self.stop();
        self.status = Some(status);
    }

    pub(crate) fn options(&self, duration: f64) -> auris_session::OutputPreviewOptions {
        let position = self.position();
        auris_session::OutputPreviewOptions {
            position_seconds: if position >= duration { 0.0 } else { position },
            volume: if self.muted { 0.0 } else { self.volume },
            loop_start_seconds: None,
        }
    }

    pub(crate) fn toggle_pause(&self) -> bool {
        if let Some(status) = &self.status
            && status.is_active()
            && status.position_seconds() < status.duration_seconds()
        {
            status.set_paused(status.is_playing());
            true
        } else {
            false
        }
    }

    fn position(&self) -> f64 {
        self.status
            .as_ref()
            .map_or(self.position, OutputPreviewStatus::position_seconds)
    }

    /// Whether the current audition reports active playback.
    pub(super) fn playing(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(OutputPreviewStatus::is_playing)
    }

    fn seek(&mut self, seconds: f64) {
        self.position = seconds;
        if let Some(status) = &self.status {
            status.seek_seconds(seconds);
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.muted = false;
        self.apply_volume();
    }

    fn apply_volume(&self) {
        if let Some(status) = &self.status {
            status.set_volume(if self.muted { 0.0 } else { self.volume });
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PlayerTarget {
    Workshop(usize),
    Attachment,
    InlineAttachment(usize),
    /// A recording a tool answered with: the conversation row's view key, and the clip.
    ToolAudio(usize, usize),
}

fn time_label(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn note_rect(
    note: &auris_session::AuditionNote,
    low: u8,
    high: u8,
    duration: f64,
) -> (f32, f32, f32, f32) {
    let rows = f32::from(high - low + 1);
    (
        (note.start_seconds / duration) as f32,
        f32::from(high.saturating_sub(note.pitch)) / rows,
        ((note.end_seconds - note.start_seconds) / duration) as f32,
        1.0 / rows,
    )
}

// Keep authored colors when they identify a track, then use themed categorical fallbacks.
fn audition_track_colors(
    timeline: &auris_session::AuditionTimeline,
    theme: &crate::theme::Theme,
) -> Vec<gpui::Hsla> {
    let mut colors = Vec::with_capacity(timeline.tracks.len());
    let palette = auris_session::prelude::Color::PALETTE;
    for (index, track) in timeline.tracks.iter().enumerate() {
        let authored = theme.track_color(track.color);
        let color = if colors.contains(&authored) {
            palette
                .iter()
                .cycle()
                .skip(index % palette.len())
                .take(palette.len())
                .map(|color| theme.track_color(color.0))
                .chain(
                    (index..index + timeline.tracks.len() + 1)
                        .map(|slot| theme.atelier.categorical(slot)),
                )
                .find(|color| !colors.contains(color))
                .unwrap_or(authored)
        } else {
            authored
        };
        colors.push(color);
    }
    colors
}

impl AurisApp {
    fn player(&self, target: PlayerTarget) -> Option<&PlayerState> {
        match target {
            PlayerTarget::Workshop(index) => self.song_workshop.as_ref().map(|s| &s.players[index]),
            PlayerTarget::Attachment => self.attachment_preview.as_ref().map(|s| &s.player),
            PlayerTarget::InlineAttachment(index) => {
                self.inline_attachments.get(index).map(|s| &s.player)
            }
            PlayerTarget::ToolAudio(key, clip) => self
                .agent_chat
                .tool_players
                .get(&(key, clip))
                .map(|p| &p.player),
        }
    }

    fn player_mut(&mut self, target: PlayerTarget) -> Option<&mut PlayerState> {
        match target {
            PlayerTarget::Workshop(index) => {
                self.song_workshop.as_mut().map(|s| &mut s.players[index])
            }
            PlayerTarget::Attachment => self.attachment_preview.as_mut().map(|s| &mut s.player),
            PlayerTarget::InlineAttachment(index) => self
                .inline_attachments
                .get_mut(index)
                .map(|s| &mut s.player),
            PlayerTarget::ToolAudio(key, clip) => self
                .agent_chat
                .tool_players
                .get_mut(&(key, clip))
                .map(|p| &mut p.player),
        }
    }

    fn adjust_player(&mut self, target: PlayerTarget, volume: bool, fraction: f32, duration: f64) {
        if let Some(player) = self.player_mut(target) {
            if volume {
                player.set_volume(fraction);
            } else {
                player.seek(f64::from(fraction) * duration);
            }
        }
    }

    pub(crate) fn render_audio_player(
        &self,
        target: PlayerTarget,
        duration: f64,
        enabled: bool,
        loading: bool,
        instance: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(player) = self.player(target) else {
            return div().into_any_element();
        };
        let theme = &self.theme;
        let prefix = instance.unwrap_or_else(|| match target {
            PlayerTarget::Workshop(i) => format!("song-preview-{i}"),
            PlayerTarget::Attachment => "attachment-player".into(),
            PlayerTarget::InlineAttachment(index) => format!("inline-attachment-player-{index}"),
            PlayerTarget::ToolAudio(key, clip) => format!("agent-tool-audio-{key}-{clip}"),
        });
        let playing = player.playing();
        let timeline = match target {
            PlayerTarget::Workshop(index) => self
                .song_workshop
                .as_ref()
                .and_then(|s| s.audition_timeline(index)),
            _ => None,
        };
        // Draft auditions can update while the captured buffer keeps playing.
        let duration = timeline.as_ref().map_or_else(
            || {
                player
                    .status
                    .as_ref()
                    .filter(|s| s.is_active())
                    .map_or(duration, OutputPreviewStatus::duration_seconds)
            },
            |t| t.duration_seconds,
        );
        let position = player.position().min(duration);
        let fraction = if duration > 0.0 {
            (position / duration) as f32
        } else {
            0.0
        };
        let muted = player.muted || player.volume == 0.0;
        let native_id = match target {
            PlayerTarget::Workshop(_)
            | PlayerTarget::Attachment
            | PlayerTarget::InlineAttachment(_)
            | PlayerTarget::ToolAudio(..) => prefix.clone(),
        };
        let mut native_player = atelier::AudioPlayer::new(
            SharedString::from(native_id),
            fraction,
            time_label(position),
            time_label(duration),
        )
        .playing(playing)
        .volume(player.volume)
        .muted(muted)
        .loading(loading)
        .disabled(!enabled);
        if duration > 0.0 {
            native_player = native_player.seek_step((5.0 / duration) as f32);
        }
        if let Some(timeline) = timeline.as_ref() {
            native_player = native_player.peaks(timeline.peaks.clone());
        }
        let native_player = native_player
            .on_play(cx.listener(move |this, _, _, cx| {
                if !this.player(target).is_some_and(PlayerState::toggle_pause) {
                    match target {
                        PlayerTarget::Workshop(index) => this.preview_song_candidate(index, cx),
                        PlayerTarget::Attachment => this.play_attachment_preview(cx),
                        PlayerTarget::InlineAttachment(index) => {
                            this.play_inline_attachment(index, cx)
                        }
                        PlayerTarget::ToolAudio(key, clip) => this.play_tool_audio(key, clip, cx),
                    }
                }
                if let PlayerTarget::Workshop(_) = target
                    && let Some(workshop) = &mut this.song_workshop
                {
                    workshop.sync_idle_auditions();
                }
                cx.notify();
            }))
            .on_seek(cx.listener(move |this, value, _, cx| {
                this.adjust_player(target, false, *value, duration);
                cx.notify();
            }))
            .on_volume(cx.listener(move |this, value, _, cx| {
                this.adjust_player(target, true, *value, duration);
                cx.notify();
            }))
            .on_mute(cx.listener(move |this, _, _, cx| {
                if let Some(player) = this.player_mut(target) {
                    player.muted = !player.muted;
                    player.apply_volume();
                }
                cx.notify();
            }));
        div()
            .id(SharedString::from(prefix.clone()))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(theme.surface_raised)
            .when_some(timeline, |player, timeline| {
                player
                    .child(self.player_timeline(&prefix, target, &timeline, position, enabled, cx))
            })
            .child(native_player)
            .into_any_element()
    }

    fn player_timeline(
        &self,
        prefix: &str,
        target: PlayerTarget,
        timeline: &std::sync::Arc<auris_session::AuditionTimeline>,
        position: f64,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let duration = timeline.duration_seconds;
        let playhead = theme.atelier.colors.playhead;
        let mut sections = div()
            .relative()
            .h_8()
            .w_full()
            .border_1()
            .border_color(gpui::transparent_black());
        for (index, section) in timeline.sections.iter().enumerate() {
            let start = section.start_seconds;
            let end = section.end_seconds;
            let current = position >= start && position < end;
            let repeated = timeline
                .sections
                .iter()
                .filter(|s| s.label == section.label)
                .count()
                > 1;
            let label = if repeated {
                format!("{} {}", section.label, section.instance)
            } else {
                section.label.clone()
            };
            let detail = format!("{} · {}–{}", label, time_label(start), time_label(end));
            sections = sections.child(
                div()
                    .absolute()
                    .left(relative((start / duration) as f32))
                    .w(relative(((end - start) / duration) as f32))
                    .h_full()
                    .pr_1()
                    .child(
                        button_with_content(
                            SharedString::from(format!("{prefix}-section-{index}")),
                            div().min_w_0().truncate().child(label),
                            ButtonStyle::Ghost,
                            ButtonState::available(current, enabled),
                            theme.selection,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                if let Some(player) = this.player_mut(target) {
                                    player.seek(start);
                                }
                                cx.notify();
                            }),
                        )
                        .w_full()
                        .min_w_0()
                        .rounded_sm()
                        .px_0()
                        .tooltip(crate::ui::tooltip::text_tip(detail)),
                    ),
            );
        }
        // The audition's notes, one colour per track, on the same time axis as the scrubber
        // below: the player draws the waveform, and this strip says which part is which.
        let notes = (!timeline.tracks.is_empty() && duration > 0.0).then(|| {
            let score = std::sync::Arc::clone(timeline);
            let colors = audition_track_colors(timeline, theme);
            let overlay_id = format!("{prefix}-seek-notes");
            div()
                .relative()
                .h_10()
                .w_full()
                .rounded_sm()
                .overflow_hidden()
                .bg(theme.surface_sunken)
                .debug_selector(move || overlay_id.clone())
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let pitches =
                                || score.tracks.iter().flat_map(|t| &t.notes).map(|n| n.pitch);
                            let low = pitches().min().unwrap_or(60).saturating_sub(2);
                            let high = pitches().max().unwrap_or(72).saturating_add(2).min(127);
                            for (track, color) in score.tracks.iter().zip(&colors) {
                                let color = color.opacity(0.72);
                                for note in &track.notes {
                                    let (x, y, width, height) =
                                        note_rect(note, low, high, duration);
                                    // Keep short notes visible at the strip's raster resolution.
                                    let rect = gpui::Bounds::new(
                                        bounds.origin
                                            + gpui::point(
                                                bounds.size.width * x,
                                                bounds.size.height * y,
                                            ),
                                        gpui::size(
                                            (bounds.size.width * width).max(gpui::px(1.0)),
                                            (bounds.size.height * height).max(gpui::px(1.0)),
                                        ),
                                    );
                                    window.paint_quad(gpui::fill(rect, color));
                                }
                            }
                            // Where playback is, so the strip and the scrubber read as one.
                            let x = bounds.origin.x
                                + bounds.size.width * (position / duration).clamp(0.0, 1.0) as f32;
                            window.paint_quad(gpui::fill(
                                gpui::Bounds::new(
                                    gpui::point(x, bounds.origin.y),
                                    gpui::size(gpui::px(1.0), bounds.size.height),
                                ),
                                playhead,
                            ));
                        },
                    )
                    .size_full(),
                )
        });
        let legend = (!timeline.tracks.is_empty()).then(|| {
            div()
                .id(SharedString::from(format!("{prefix}-tracks")))
                .debug_selector({
                    let label = format!("{prefix}-tracks");
                    move || label.clone()
                })
                .flex()
                .min_w_0()
                .w_full()
                .gap_2()
                .overflow_x_scroll()
                .children(
                    timeline
                        .tracks
                        .iter()
                        .zip(audition_track_colors(timeline, theme))
                        .map(|(track, color)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .flex_shrink_0()
                                .text_xs()
                                .child(div().size_2().rounded_sm().bg(color))
                                .child(track.name.clone())
                        }),
                )
        });
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .when(!timeline.sections.is_empty(), |view| view.child(sections))
            .children(notes)
            .children(legend)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_instrument_colors_remain_distinguishable_in_the_overlay_and_legend() {
        let theme = crate::theme::Theme::default();
        let color = auris_session::prelude::Color::INSTRUMENT.0;
        let timeline = auris_session::AuditionTimeline {
            tracks: (0..10)
                .map(|id| auris_session::AuditionTrack {
                    id,
                    name: format!("Track {id}"),
                    color,
                    notes: vec![],
                })
                .collect(),
            ..Default::default()
        };
        let colors = super::audition_track_colors(&timeline, &theme);
        assert_eq!(colors[0], theme.track_color(color));
        assert_eq!(colors.len(), 10);
        for (index, color) in colors.iter().enumerate() {
            assert!(!colors[..index].contains(color));
        }
        assert!(timeline.tracks.iter().all(|track| track.color == color));
    }
    #[test]
    fn overlaid_notes_share_one_time_and_pitch_scale() {
        let note = auris_session::AuditionNote {
            pitch: 72,
            start_seconds: 2.0,
            end_seconds: 3.0,
        };
        assert_eq!(
            super::note_rect(&note, 36, 83, 8.0),
            (0.25, 11.0 / 48.0, 0.125, 1.0 / 48.0)
        );
        let bass = auris_session::AuditionNote { pitch: 36, ..note };
        let (_, y, _, _) = super::note_rect(&bass, 36, 83, 8.0);
        assert_eq!(y, 47.0 / 48.0);
    }
}
