//! Normalized long-note volume presets and point editing in the performance inspector.

use crate::{
    app::{AurisApp, Drag},
    ui::{
        context_menu::{ContextMenu, MenuCommand},
        paint,
        pitch_performance::{pitch_settings, with_pitch_settings},
    },
};
use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{
    AnyElement, Bounds, Context, IntoElement, MouseButton, Pixels, Point, canvas, div, point,
    prelude::*, px, size,
};
use std::{cell::Cell, rc::Rc};

fn preset_key(preset: VolumeContourPreset) -> Key {
    match preset {
        VolumeContourPreset::Bowed => Key::VolumeContourBowed,
        VolumeContourPreset::Swell => Key::VolumeContourSwell,
        VolumeContourPreset::Crescendo => Key::VolumeContourCrescendo,
        VolumeContourPreset::Decrescendo => Key::VolumeContourDecrescendo,
        VolumeContourPreset::SoftAttack => Key::VolumeContourSoftAttack,
        VolumeContourPreset::Flat => Key::VolumeContourFlat,
    }
}

fn location(bounds: Bounds<Pixels>, p: CurvePoint) -> Point<Pixels> {
    point(
        bounds.left() + bounds.size.width * (p.at.raw() as f32 / VolumeContour::END.raw() as f32),
        bounds.bottom() - bounds.size.height * p.value,
    )
}

fn point_at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> CurvePoint {
    let x = (f32::from(position.x - bounds.left()) / f32::from(bounds.size.width).max(1.0))
        .clamp(0.0, 1.0);
    let y = (f32::from(bounds.bottom() - position.y) / f32::from(bounds.size.height).max(1.0))
        .clamp(0.0, 1.0);
    CurvePoint {
        at: Ticks((x * VolumeContour::END.raw() as f32).round() as i64),
        value: y,
    }
}

fn nearest(
    points: &[CurvePoint],
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let at = location(bounds, *p);
            let distance =
                f32::from(at.x - position.x).powi(2) + f32::from(at.y - position.y).powi(2);
            (distance <= 64.0).then_some((i, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

impl AurisApp {
    pub(crate) fn volume_contour_rows(
        &self,
        clip: ClipId,
        stack: &[NoteTransform],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let contour = pitch_settings(stack).volume_contour;
        let selected = contour.preset();
        let picker = self
            .picker_row(
                "volume-contour-preset",
                Key::VolumeContourPreset,
                self.t(selected.map(preset_key).unwrap_or(Key::VolumeContourCustom))
                    .to_string(),
                Self::opens_menu(cx, move |this, at| {
                    let selected =
                        pitch_settings(this.session.clip_transforms(clip).unwrap_or(&[]))
                            .volume_contour
                            .preset();
                    let mut menu = ContextMenu::new(at, this.t(Key::VolumeContourPreset));
                    for preset in VolumeContourPreset::ALL {
                        menu = menu.toggle(
                            this.t(preset_key(preset)),
                            MenuCommand::SetVolumeContour { clip, preset },
                            selected == Some(preset),
                        );
                    }
                    menu
                }),
            )
            .into_any_element();
        let bounds = Rc::new(Cell::new(None));
        let recorded = bounds.clone();
        let pressed = bounds.clone();
        let theme = self.theme.clone();
        let active = match self.drag.as_ref() {
            Some(Drag::VolumeContourPoint {
                clip: id, index, ..
            }) if *id == clip => Some(*index),
            _ => None,
        };
        let graph = div()
            .id("volume-contour-graph")
            .debug_selector(|| "volume-contour-graph".into())
            .w_full()
            .h(px(144.0))
            .flex_shrink_0()
            .p_2()
            .bg(self.theme.surface_sunken)
            .border_1()
            .border_color(self.theme.border)
            .child(
                canvas(
                    move |area, _, _| recorded.set(Some(area)),
                    move |area, _, window, _| {
                        paint::clipped(window, area.dilate(px(4.0)), |window| {
                            for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
                                paint::vline(
                                    window,
                                    area,
                                    area.left() + area.size.width * fraction,
                                    px(1.0),
                                    theme.border,
                                );
                                paint::hline(
                                    window,
                                    area,
                                    area.top() + area.size.height * fraction,
                                    theme.border,
                                );
                            }
                            let points: Vec<_> = contour
                                .points()
                                .iter()
                                .map(|p| location(area, *p))
                                .collect();
                            paint::polyline(window, &points, px(2.0), theme.accent);
                            for (i, at) in points.into_iter().enumerate() {
                                paint::rounded_rect(
                                    window,
                                    Bounds::new(
                                        at - point(px(3.0), px(3.0)),
                                        size(px(6.0), px(6.0)),
                                    ),
                                    px(2.0),
                                    if active == Some(i) {
                                        theme.text
                                    } else {
                                        theme.accent
                                    },
                                );
                            }
                        });
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(bounds) = pressed.get() {
                        this.press_volume_contour(clip, bounds, event.position);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(bounds) = bounds.get() {
                        let mut points =
                            pitch_settings(this.session.clip_transforms(clip).unwrap_or(&[]))
                                .volume_contour
                                .points()
                                .to_vec();
                        if let Some(index) = nearest(&points, bounds, event.position)
                            .filter(|i| *i > 0 && *i + 1 < points.len())
                        {
                            points.remove(index);
                            this.session
                                .begin_transaction(auris_session::Edit::SetClipTransforms(clip));
                            this.set_volume_contour(clip, VolumeContour::new(points));
                            this.session.end_transaction();
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element();
        let axes = div()
            .flex()
            .justify_between()
            .text_color(self.theme.text_muted)
            .child(self.t(Key::VolumeContourStart))
            .child(self.t(Key::VolumeContourEnd))
            .into_any_element();
        let hint = div()
            .text_color(self.theme.text_muted)
            .child(self.t(Key::VolumeContourHint))
            .into_any_element();
        vec![picker, graph, axes, hint]
    }

    pub(crate) fn set_volume_contour(&mut self, clip: ClipId, contour: VolumeContour) {
        let Ok(stack) = self.session.clip_transforms(clip) else {
            return;
        };
        let mut settings = pitch_settings(stack);
        settings.volume_contour = contour;
        let next = with_pitch_settings(stack, settings);
        let _ = self.session.set_clip_transforms(clip, next);
    }

    fn press_volume_contour(
        &mut self,
        clip: ClipId,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
    ) {
        let mut points = pitch_settings(self.session.clip_transforms(clip).unwrap_or(&[]))
            .volume_contour
            .points()
            .to_vec();
        if let Some(index) = nearest(&points, bounds, position) {
            self.begin_drag(Drag::VolumeContourPoint {
                clip,
                index,
                bounds,
            });
            return;
        }
        let point = point_at(bounds, position);
        let index = points.partition_point(|p| p.at < point.at);
        self.begin_drag(Drag::VolumeContourPoint {
            clip,
            index,
            bounds,
        });
        if points.get(index).is_some_and(|p| p.at == point.at) {
            points[index] = point;
        } else {
            points.insert(index, point);
        }
        self.set_volume_contour(clip, VolumeContour::new(points));
    }

    pub(crate) fn drag_volume_contour_point(
        &mut self,
        clip: ClipId,
        index: usize,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
    ) {
        let mut points = pitch_settings(self.session.clip_transforms(clip).unwrap_or(&[]))
            .volume_contour
            .points()
            .to_vec();
        if index >= points.len() {
            return;
        }
        let mut point = point_at(bounds, position);
        point.at = if index == 0 || index + 1 == points.len() {
            points[index].at
        } else {
            point.at.clamp(
                points[index - 1].at + Ticks(1),
                points[index + 1].at - Ticks(1),
            )
        };
        points[index] = point;
        self.set_volume_contour(clip, VolumeContour::new(points));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{
        choose, click, drag_to, paint, press, release, resize, right_press, with_a_clip,
    };
    use gpui::{Modifiers, TestAppContext};

    #[gpui::test]
    fn presets_and_point_gestures_preserve_score_and_support_undo(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(3840)))
                .unwrap();
            this.open_clip_in_editor(clip);
        });
        resize(&app, cx, size(px(1920.0), px(3000.0)));
        click("score-performed", cx);
        click("perform-pitch-details", cx);
        click("volume-contour-preset", cx);
        choose(
            &app,
            cx,
            &MenuCommand::SetVolumeContour {
                clip,
                preset: VolumeContourPreset::Crescendo,
            },
        );
        let read = |this: &AurisApp| {
            pitch_settings(this.session.clip_transforms(clip).unwrap()).volume_contour
        };
        app.read_with(cx, |this, _| {
            assert_eq!(read(this).preset(), Some(VolumeContourPreset::Crescendo))
        });
        paint(&app, cx);
        let bounds = cx.debug_bounds("volume-contour-graph").unwrap();
        let start = bounds.center();
        let end = start - point(px(0.0), px(25.0));
        press(cx, start);
        drag_to(cx, end);
        release(cx, end);
        app.read_with(cx, |this, _| {
            let contour = read(this);
            assert_eq!(contour.preset(), None);
            assert_eq!(contour.points().len(), 3);
            assert!(contour.points()[1].value > 0.65);
            let source = this.session.midi_clip(clip).unwrap();
            assert_eq!(source.notes.len(), 1);
            assert!(source.controllers.is_empty());
        });
        app.update(cx, |this, _| this.undo());
        app.read_with(cx, |this, _| {
            assert_eq!(read(this).preset(), Some(VolumeContourPreset::Crescendo))
        });
        app.update(cx, |this, _| this.redo());
        paint(&app, cx);
        press(cx, end);
        drag_to(cx, start);
        cx.simulate_keystrokes("escape");
        release(cx, start);
        app.read_with(cx, |this, _| assert!(read(this).points()[1].value > 0.65));
        paint(&app, cx);
        right_press(cx, end);
        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::none());
        app.read_with(cx, |this, _| {
            assert_eq!(read(this).preset(), Some(VolumeContourPreset::Crescendo))
        });
        app.update(cx, |this, _| this.undo());
        app.read_with(cx, |this, _| assert_eq!(read(this).points().len(), 3));
    }
}
