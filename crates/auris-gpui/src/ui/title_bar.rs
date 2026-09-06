//! Project identity and playback controls in the themed window title bar.

use gpui::{Context, IntoElement, Pixels, Window, div, prelude::*, px};

use crate::app::AurisApp;
use crate::dock::Dock;
use crate::titlebar;
use crate::ui::prompt::PendingAction;

/// Whether there is room beside playback and the native controls for the panel switches.
///
/// Smaller windows keep every switch in the status bar. The project name truncates first;
/// playback and window controls always retain their full hit targets.
pub(crate) fn panels_in_titlebar(width: Pixels) -> bool {
    width >= px(1000.0)
}

impl AurisApp {
    /// Draws the title bar, with playback above its readouts and panel switches to the right.
    pub(crate) fn render_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = &self.theme;
        let panels = panels_in_titlebar(window.viewport_size().width);
        let controls_width = titlebar::controls_width(window);
        let controls = titlebar::controls(
            window,
            theme,
            cx.listener(|this, _, window, cx| {
                if this.confirm_discard(PendingAction::CloseWindow) {
                    this.save_window_placement();
                    window.remove_window();
                }
                cx.notify();
            }),
        );

        titlebar::titlebar(window, theme)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    // Balance the native macOS buttons so playback remains on the same centre
                    // line as the readouts below, regardless of which platform owns the frame.
                    .pr(titlebar::traffic_light_inset(window))
                    .child(
                        div().flex_1().min_w_0().h_full().child(
                            titlebar::drag_region("project-title")
                                .w_full()
                                .px_3()
                                .text_xs()
                                .child(div().truncate().child(self.window_title())),
                        ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "titlebar-transport".into())
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .child(self.render_transport_buttons(cx)),
                    )
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .items_center()
                            .child(
                                div()
                                    // Let panel switches use the balancing inset while the
                                    // two flex columns keep playback centred in the window.
                                    .absolute()
                                    .left_0()
                                    .right(-titlebar::traffic_light_inset(window))
                                    .flex()
                                    .h_full()
                                    .items_center()
                                    .pr(controls_width + px(8.0))
                                    .child(
                                        titlebar::drag_region("titlebar-space")
                                            .flex_1()
                                            .min_w(px(12.0)),
                                    )
                                    .when(panels, |bar| {
                                        bar.children(Dock::ALL.into_iter().map(|dock| {
                                            div().ml_1().child(self.dock_switches(
                                                dock,
                                                px(24.0),
                                                cx,
                                            ))
                                        }))
                                    }),
                            ),
                    ),
            )
            .child(
                controls
                    .absolute()
                    .top(titlebar::resize_inset(window))
                    .right_0()
                    .h(titlebar::HEIGHT - titlebar::resize_inset(window) - px(1.0)),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, px, size};

    use crate::dock::Panel;
    use crate::harness::{click, open, paint, resize, with_a_clip};

    #[gpui::test]
    fn titlebar_controls_still_change_the_document_and_relocate_panels(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        for width in [1360.0, 640.0, 1100.0] {
            resize(&app, cx, size(px(width), px(700.0)));
            let transport = cx.debug_bounds("titlebar-transport").unwrap();
            assert_eq!(transport.center().x, px(width / 2.0));
            if let Some(minimize) = cx.debug_bounds("window-minimize") {
                assert!(transport.right() <= minimize.left());
            }
            if super::panels_in_titlebar(px(width)) {
                // The agent is the last switch in the default right dock.
                let rightmost = cx.debug_bounds("panel-switch-5").unwrap().right();
                let controls_left = cx
                    .debug_bounds("window-minimize")
                    .map_or(px(width), |bounds| bounds.left());
                assert_eq!(rightmost, controls_left - px(8.0));
            }
            let looping = app.read_with(cx, |this, _| this.project().loop_enabled);
            click("loop", cx);
            app.read_with(cx, |this, _| {
                assert_eq!(this.project().loop_enabled, !looping)
            });

            let open = app.read_with(cx, |this, _| this.panels.is_open(Panel::Mixer));
            click("panel-switch-2", cx);
            app.read_with(cx, |this, _| {
                assert_eq!(this.panels.is_open(Panel::Mixer), !open)
            });
            paint(&app, cx);
        }
    }

    #[gpui::test]
    fn closing_from_the_titlebar_keeps_unsaved_work_until_answered(cx: &mut TestAppContext) {
        if cfg!(target_os = "macos") {
            return; // macOS draws its own traffic lights outside the headless view tree.
        }
        let (app, cx, _, clip) = with_a_clip(cx);
        click("window-close", cx);
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_some());
            assert!(this.session.midi_clip(clip).is_some());
            assert!(this.session.is_dirty());
        });
        click("prompt-cancel", cx);
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert!(this.session.midi_clip(clip).is_some());
        });
    }
}
