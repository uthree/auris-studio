//! Single-choice settings controls with one shared, keyboard-accessible popup.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Bounds, MouseButton, MouseDownEvent, Pixels, ScrollHandle, canvas};

use super::*;
use crate::ui::icons::icon;

type SelectChoice = Rc<dyn Fn(&mut SettingsWindow, usize, &mut Context<SettingsWindow>)>;

/// The popup keeps labels and a callback; the preference remains the source of selection truth.
pub(super) struct DropdownMenu {
    id: &'static str,
    options: Vec<(String, String)>,
    selected: usize,
    highlighted: usize,
    bounds: Bounds<Pixels>,
    scroll: ScrollHandle,
    select: SelectChoice,
    typed: String,
    typed_at: Option<Instant>,
}

impl SettingsWindow {
    /// Draws a single-choice control. The final tuple item is an optional detail line.
    pub(super) fn dropdown<V: Clone + PartialEq + 'static>(
        &mut self,
        id: &'static str,
        choices: Vec<(V, String, String)>,
        current: &V,
        assign: impl Fn(&mut Self, V, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let selected = choices
            .iter()
            .position(|(value, _, _)| value == current)
            .unwrap_or(0);
        let label = choices
            .get(selected)
            .map(|(_, label, _)| label.clone())
            .unwrap_or_default();
        let detail = choices
            .get(selected)
            .map(|(_, _, detail)| detail.clone())
            .unwrap_or_default();
        let options: Vec<_> = choices
            .iter()
            .map(|(_, label, detail)| (label.clone(), detail.clone()))
            .collect();
        let select: SelectChoice = Rc::new(move |this, index, cx| {
            if let Some((value, _, _)) = choices.get(index) {
                assign(this, value.clone(), cx);
            }
        });
        let focus = self
            .dropdown_focus
            .entry(id)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let paint_bounds = bounds.clone();
        let key_bounds = bounds.clone();
        let key_options = options.clone();
        let key_select = select.clone();
        let key_focus = focus.clone();
        let opened = self
            .dropdown_menu
            .as_ref()
            .is_some_and(|menu| menu.id == id);
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .track_focus(&focus)
            .tab_index(0)
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .min_w_0()
            .min_h(px(30.0))
            .px_2()
            .py_1()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(if opened {
                theme.accent
            } else {
                theme.border_subtle
            })
            .focus(|this| this.border_color(theme.accent))
            .cursor_pointer()
            .hover(|this| this.border_color(theme.border))
            .child(
                canvas(
                    move |bounds, _, _| paint_bounds.set(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_xs().truncate().child(label))
                    .when(!detail.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(detail),
                        )
                    }),
            )
            .child(icon(Icon::ChevronDown, px(12.0), theme.text_muted))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    window.focus(&focus);
                    this.dropdown_menu = Some(DropdownMenu {
                        id,
                        options: options.clone(),
                        selected,
                        highlighted: selected,
                        bounds: bounds.get(),
                        scroll: ScrollHandle::new(),
                        select: select.clone(),
                        typed: String::new(),
                        typed_at: None,
                    });
                    if let Some(menu) = &this.dropdown_menu {
                        menu.scroll.scroll_to_item(selected);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.dropdown_menu.is_none()
                    && matches!(
                        event.keystroke.key.as_str(),
                        "enter" | "space" | "up" | "down"
                    )
                {
                    window.focus(&key_focus);
                    this.dropdown_menu = Some(DropdownMenu {
                        id,
                        options: key_options.clone(),
                        selected,
                        highlighted: selected,
                        bounds: key_bounds.get(),
                        scroll: ScrollHandle::new(),
                        select: key_select.clone(),
                        typed: String::new(),
                        typed_at: None,
                    });
                    if let Some(menu) = &this.dropdown_menu {
                        menu.scroll.scroll_to_item(selected);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// Handles popup navigation before ordinary settings shortcuts or text input.
    pub(super) fn dropdown_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &mut self.dropdown_menu else {
            return false;
        };
        let count = menu.options.len();
        match event.keystroke.key.as_str() {
            "escape" | "tab" => {
                self.dropdown_menu = None;
            }
            "enter" | "space" => {
                let menu = self.dropdown_menu.take().unwrap();
                (menu.select)(self, menu.highlighted, cx);
            }
            "up" if count > 0 => menu.highlighted = menu.highlighted.saturating_sub(1),
            "down" if count > 0 => menu.highlighted = (menu.highlighted + 1).min(count - 1),
            "home" if count > 0 => menu.highlighted = 0,
            "end" if count > 0 => menu.highlighted = count - 1,
            _ => {
                let character = event
                    .keystroke
                    .key_char
                    .as_deref()
                    .unwrap_or(&event.keystroke.key);
                if character.chars().count() != 1
                    || event.keystroke.modifiers.secondary()
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                {
                    return true;
                }
                let now = Instant::now();
                if menu
                    .typed_at
                    .is_none_or(|last| now.duration_since(last) > Duration::from_millis(900))
                {
                    menu.typed.clear();
                }
                menu.typed.push_str(&character.to_lowercase());
                menu.typed_at = Some(now);
                let mut found = menu
                    .options
                    .iter()
                    .position(|(label, _)| label.to_lowercase().starts_with(&menu.typed));
                if found.is_none() {
                    menu.typed = character.to_lowercase();
                    found = (1..=count)
                        .map(|step| (menu.highlighted + step) % count)
                        .find(|index| {
                            menu.options[*index]
                                .0
                                .to_lowercase()
                                .starts_with(&menu.typed)
                        });
                }
                if let Some(index) = found {
                    menu.highlighted = index;
                }
            }
        }
        if let Some(menu) = &self.dropdown_menu {
            menu.scroll.scroll_to_item(menu.highlighted);
        }
        cx.notify();
        // Tab dismisses the menu and continues the platform's normal focus traversal.
        event.keystroke.key != "tab"
    }

    /// Renders above the scrolling page, constrained to the current window's dimensions.
    pub(super) fn render_dropdown(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.dropdown_menu.as_ref()?;
        let theme = self.theme.clone();
        let viewport = window.viewport_size();
        let width = menu
            .bounds
            .size
            .width
            .min((viewport.width - px(16.0)).max(px(1.0)));
        let total_height: f32 = menu
            .options
            .iter()
            .map(|(_, detail)| if detail.is_empty() { 28.0 } else { 44.0 })
            .sum();
        let height = px(total_height + 8.0)
            .min(px(280.0))
            .min((viewport.height - px(16.0)).max(px(1.0)));
        let below = menu.bounds.bottom() + px(3.0);
        let y = if below + height <= viewport.height - px(8.0) {
            below
        } else {
            (menu.bounds.top() - height - px(3.0)).max(px(8.0))
        };
        let x = menu
            .bounds
            .left()
            .min(viewport.width - width - px(8.0))
            .max(px(8.0));
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.dropdown_menu = None;
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.dropdown_menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(width)
                        .h(height)
                        .p_1()
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(Metrics::RADIUS_SM)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx: &mut App| {
                            cx.stop_propagation()
                        })
                        .child(
                            div()
                                .id("settings-dropdown-menu")
                                .debug_selector(|| "settings-dropdown-menu".to_string())
                                .flex()
                                .flex_col()
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&menu.scroll)
                                .children(menu.options.iter().enumerate().map(
                                    |(index, (label, detail))| {
                                        let id = menu.id;
                                        div()
                                            .id((id, index))
                                            .debug_selector(move || format!("{id}-option-{index}"))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .px_1()
                                            .h(px(if detail.is_empty() { 28.0 } else { 44.0 }))
                                            .flex_shrink_0()
                                            .rounded(Metrics::RADIUS_XS)
                                            .cursor_pointer()
                                            .when(menu.highlighted == index, |this| {
                                                this.bg(theme.accent_soft)
                                            })
                                            .hover(|this| this.bg(theme.accent_soft))
                                            .child(div().w(px(12.0)).flex_shrink_0().when(
                                                menu.selected == index,
                                                |this| {
                                                    this.child(icon(
                                                        Icon::Check,
                                                        px(12.0),
                                                        theme.accent,
                                                    ))
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .truncate()
                                                            .child(label.clone()),
                                                    )
                                                    .when(!detail.is_empty(), |this| {
                                                        this.child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(theme.text_muted)
                                                                .truncate()
                                                                .child(detail.clone()),
                                                        )
                                                    }),
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |this, _: &MouseDownEvent, _, cx| {
                                                        if let Some(menu) =
                                                            this.dropdown_menu.take()
                                                        {
                                                            (menu.select)(this, index, cx);
                                                        }
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    },
                                                ),
                                            )
                                    },
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RestoreSettings(auris_session::Settings);

    impl Drop for RestoreSettings {
        fn drop(&mut self) {
            self.0
                .save()
                .expect("restore isolated settings after the test");
        }
    }

    #[gpui::test]
    fn a_dropdown_applies_only_a_confirmed_choice(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let _restore = RestoreSettings(auris_session::Settings::load());
        app.update(cx, |this, cx| this.open_settings(cx));
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                this.language_preference = Some(Language::English);
                this.language = Language::English;
                cx.notify();
            })
            .unwrap();
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(800.0), px(700.0)));
        cx.run_until_parked();

        crate::harness::click("language", cx);
        cx.simulate_keystrokes("down");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.language_preference, Some(Language::English));
                assert_eq!(this.dropdown_menu.as_ref().unwrap().highlighted, 2);
            })
            .unwrap();
        cx.simulate_keystrokes("escape");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.language_preference, Some(Language::English));
                assert!(this.dropdown_menu.is_none());
            })
            .unwrap();

        // Focus remains on the trigger, so reopening and confirming needs no pointer.
        cx.simulate_keystrokes("enter down enter");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.language_preference, Some(Language::Japanese));
                assert!(this.dropdown_menu.is_none());
            })
            .unwrap();
        assert_eq!(
            app.read_with(cx, |this, _| this.language()),
            Language::Japanese
        );
    }

    #[gpui::test]
    fn a_dismissing_click_does_not_activate_the_tab_behind_it(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(800.0), px(700.0)));
        cx.run_until_parked();
        crate::harness::click("language", cx);
        crate::harness::click("tab-audio", cx);
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.tab, SettingsTab::General);
                assert!(this.dropdown_menu.is_none());
            })
            .unwrap();
        crate::harness::click("tab-audio", cx);
        handle
            .update(cx, |this, _, _| assert_eq!(this.tab, SettingsTab::Audio))
            .unwrap();
    }

    #[gpui::test]
    fn long_lists_fit_the_window_and_typeahead_reaches_hidden_choices(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(480.0), px(360.0)));
        cx.run_until_parked();
        crate::harness::click("scheme", cx);
        handle
            .update(cx, |this, _, cx| {
                let menu = this.dropdown_menu.as_mut().unwrap();
                menu.options = (0..120)
                    .map(|index| (format!("Font {index:03}"), String::new()))
                    .collect();
                menu.options.push(("Zed Sans".to_owned(), String::new()));
                menu.select = Rc::new(|this, index, _| this.status = format!("selected {index}"));
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let menu = cx.debug_bounds("settings-dropdown-menu").unwrap();
        assert!(menu.top() >= px(0.0) && menu.bottom() <= px(360.0));
        assert!(menu.left() >= px(0.0) && menu.right() <= px(480.0));
        assert!(menu.size.height <= px(280.0));
        cx.simulate_keystrokes("z e d");
        cx.run_until_parked();
        let last = cx.debug_bounds("scheme-option-120").unwrap();
        assert!(last.top() >= menu.top() && last.bottom() <= menu.bottom());
        cx.simulate_keystrokes("enter");
        handle
            .update(cx, |this, _, _| assert_eq!(this.status, "selected 120"))
            .unwrap();
    }
}
