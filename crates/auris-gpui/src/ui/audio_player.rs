//! Shared controls for finite audio previews. The callback owns the playback clock.

use super::icons::{Icon, icon};
use super::tooltip::keyed_tip;
use super::widgets::{
    AdjustSlider, ButtonState, ButtonStyle, button_with_content, slider_fraction_after_key,
};
use crate::app::AurisApp;
use auris_i18n::Key;
use auris_session::OutputPreviewStatus;
use gpui::{
    AnyElement, Bounds, Context, IntoElement, MouseButton, MouseMoveEvent, Pixels, SharedString,
    canvas, div, prelude::*, relative, rems,
};
use std::{cell::Cell, rc::Rc};

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

    fn playing(&self) -> bool {
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
    Chords,
}

#[derive(Clone, Copy)]
pub(crate) struct PlayerDrag {
    target: PlayerTarget,
    volume: bool,
    bounds: Bounds<Pixels>,
    duration: f64,
}

fn time_label(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl AurisApp {
    fn player(&self, target: PlayerTarget) -> Option<&PlayerState> {
        match target {
            PlayerTarget::Workshop(index) => self.song_workshop.as_ref().map(|s| &s.players[index]),
            PlayerTarget::Chords => Some(&self.song_preview.player),
        }
    }

    fn player_mut(&mut self, target: PlayerTarget) -> Option<&mut PlayerState> {
        match target {
            PlayerTarget::Workshop(index) => {
                self.song_workshop.as_mut().map(|s| &mut s.players[index])
            }
            PlayerTarget::Chords => Some(&mut self.song_preview.player),
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

    pub(crate) fn move_audio_player(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.audio_player_drag else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.audio_player_drag = None;
            return false;
        }
        let fraction =
            ((event.position.x - drag.bounds.left()) / drag.bounds.size.width).clamp(0.0, 1.0);
        self.adjust_player(drag.target, drag.volume, fraction, drag.duration);
        cx.notify();
        true
    }

    pub(crate) fn render_audio_player(
        &self,
        target: PlayerTarget,
        duration: f64,
        enabled: bool,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(player) = self.player(target) else {
            return div().into_any_element();
        };
        let theme = &self.theme;
        let prefix = match target {
            PlayerTarget::Workshop(i) => format!("song-preview-{i}"),
            PlayerTarget::Chords => "chord-player".into(),
        };
        let play_id: SharedString = match target {
            PlayerTarget::Chords => "song-preview-play".into(),
            _ => format!("{prefix}-play").into(),
        };
        let playing = player.playing();
        // Draft auditions can update while the captured buffer keeps playing.
        let duration = player
            .status
            .as_ref()
            .filter(|s| s.is_active())
            .map_or(duration, OutputPreviewStatus::duration_seconds);
        let position = player.position().min(duration);
        let fraction = if duration > 0.0 {
            (position / duration) as f32
        } else {
            0.0
        };
        let play_label = self.t(if loading {
            Key::WorkshopPreparing
        } else if playing {
            Key::PreviewPause
        } else {
            Key::PreviewPlay
        });
        let muted = player.muted || player.volume == 0.0;
        let mute_label = self.t(if muted {
            Key::PreviewUnmute
        } else {
            Key::PreviewMute
        });
        div()
            .id(SharedString::from(prefix.clone()))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(theme.surface_raised)
            .child(
                button_with_content(
                    play_id,
                    icon(
                        if playing { Icon::Pause } else { Icon::Play },
                        crate::theme::Metrics::CONTROL_HEIGHT * 0.6,
                        theme.text,
                    ),
                    ButtonStyle::Ghost,
                    ButtonState::available(false, enabled && !loading),
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        if !this.player(target).is_some_and(PlayerState::toggle_pause) {
                            match target {
                                PlayerTarget::Workshop(index) => {
                                    this.preview_song_candidate(index, cx)
                                }
                                PlayerTarget::Chords => this.preview_song_chords(false, cx),
                            }
                        }
                        cx.notify();
                    }),
                )
                .size_8()
                .tooltip(keyed_tip(play_label, "", theme)),
            )
            .child(self.player_slider(
                format!("{prefix}-seek"),
                target,
                false,
                fraction,
                duration,
                enabled && duration > 0.0,
                cx,
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .child(time_label(position))
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .child(format!(" / {}", time_label(duration))),
                    )
                    .flex()
                    .items_center(),
            )
            .child(
                button_with_content(
                    SharedString::from(format!("{prefix}-mute")),
                    icon(
                        if muted {
                            Icon::VolumeMuted
                        } else {
                            Icon::Volume
                        },
                        crate::theme::Metrics::CONTROL_HEIGHT * 0.6,
                        theme.text,
                    ),
                    ButtonStyle::Ghost,
                    ButtonState::available(false, enabled),
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(player) = this.player_mut(target) {
                            if player.volume == 0.0 {
                                player.volume = 1.0;
                                player.muted = false;
                            } else {
                                player.muted = !player.muted;
                            }
                            player.apply_volume();
                        }
                        cx.notify();
                    }),
                )
                .size_8()
                .tooltip(keyed_tip(mute_label, "", theme)),
            )
            .child(self.player_slider(
                format!("{prefix}-volume"),
                target,
                true,
                if muted { 0.0 } else { player.volume },
                duration,
                enabled,
                cx,
            ))
            .into_any_element()
    }

    // The two sliders share interaction policy while their geometry and availability differ.
    #[expect(clippy::too_many_arguments)]
    fn player_slider(
        &self,
        id: String,
        target: PlayerTarget,
        volume: bool,
        fraction: f32,
        duration: f64,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let painted = bounds.clone();
        let selector = id.clone();
        let label = self.t(if volume {
            Key::PreviewVolume
        } else {
            Key::PreviewSeek
        });
        let slider = div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .relative()
            .h_8()
            .min_w_0()
            .rounded_sm()
            .border_1()
            .border_color(gpui::transparent_black())
            .when(volume, |s| s.w_12().flex_shrink_0())
            .when(!volume, |s| s.flex_1())
            .flex()
            .items_center()
            .px_1()
            .child(
                div()
                    .relative()
                    .h_1()
                    .w_full()
                    .rounded_full()
                    .bg(theme.surface_sunken)
                    .child(
                        canvas(move |bounds, _, _| painted.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(theme.text_muted),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(fraction))
                            .top(rems(-0.25))
                            .ml(rems(-0.375))
                            .size_3()
                            .rounded_full()
                            .bg(if enabled {
                                theme.text
                            } else {
                                theme.text_faint
                            }),
                    ),
            )
            .when(enabled, |s| {
                s.tab_index(0)
                    .key_context("AurisSlider")
                    .focus(|s| s.border_color(theme.selection))
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            let bounds = bounds.get();
                            if bounds.size.width <= gpui::px(0.0) {
                                return;
                            }
                            let fraction = ((event.position.x - bounds.left()) / bounds.size.width)
                                .clamp(0.0, 1.0);
                            this.audio_player_drag = Some(PlayerDrag {
                                target,
                                volume,
                                bounds,
                                duration,
                            });
                            this.adjust_player(target, volume, fraction, duration);
                            // Let GPUI's built-in mouse-down listener focus this Tab stop.
                            cx.notify();
                        }),
                    )
                    .on_action(cx.listener(move |this, event: &AdjustSlider, _, cx| {
                        if let Some(next) = slider_fraction_after_key(fraction, event.key) {
                            this.adjust_player(target, volume, next, duration);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
            })
            .when(!enabled, |s| s.opacity(0.5))
            .tooltip(keyed_tip(label, "← → · Home · End", theme));
        slider.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::harness::{click, drag, open, paint};
    use auris_session::prelude::SongSpec;

    #[gpui::test]
    fn chord_player_seeks_by_pointer_and_keyboard_and_restores_muted_volume(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            let spec = SongSpec::parse(
                "key = 'D dorian'\ntempo = 240\nform = 'verse'\n[section.verse]\nbars = 4",
            )
            .unwrap();
            this.song_sheet = Some(crate::ui::compose_sheet::song_dials(&spec));
            this.project().clone()
        });
        paint(&app, cx);
        click("song-preview-play", cx);
        paint(&app, cx);
        click("chord-player-seek", cx);
        cx.simulate_keystrokes("home");
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_preview.player.position(), 0.0)
        });
        cx.simulate_keystrokes("right");
        app.read_with(cx, |this, _| {
            assert!(this.song_preview.player.position() > 0.0);
        });
        let slider = cx.debug_bounds("chord-player-seek").unwrap();
        drag(
            cx,
            slider.center(),
            gpui::point(slider.right(), slider.center().y),
        );
        app.read_with(cx, |this, _| {
            let player = &this.song_preview.player;
            assert_eq!(
                player.options(player.position()).position_seconds,
                0.0,
                "play at the end restarts"
            );
            assert!(this.audio_player_drag.is_none());
        });
        click("chord-player-volume", cx);
        cx.simulate_keystrokes("home");
        cx.simulate_keystrokes("pageup");
        app.read_with(cx, |this, _| {
            assert!(
                (this.song_preview.player.volume - 0.1).abs() < 1e-6,
                "volume after Home/PageUp: {}",
                this.song_preview.player.volume
            );
        });
        click("chord-player-mute", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_preview.player.options(10.0).volume, 0.0);
        });
        click("song-preview-another", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_preview.player.options(10.0).volume,
                0.0,
                "changing the audio must preserve mute"
            );
        });
        click("chord-player-mute", cx);
        app.read_with(cx, |this, _| {
            assert!((this.song_preview.player.options(10.0).volume - 0.1).abs() < 1e-6);
            assert_eq!(this.project(), &before);
            assert!(!this.session.can_undo());
        });
    }
}
