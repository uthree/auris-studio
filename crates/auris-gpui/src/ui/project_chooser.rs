//! Startup project selection, before the editor claims realtime audio.
//!
//! Drawn with Atelier's own pieces, as in its `auris_screens` example: a navigation [`Sidebar`]
//! of [`ListItem`]s, [`OptionCard`]s for how to start, and a footer that holds the one primary
//! commit. The panel floats over the liquid-metal backdrop, and the setup guide is the panel's
//! third page rather than a window of its own.

use std::time::{Duration, Instant};

use atelier::{
    Button, Edge, EmptyState, IconName, Label, ListItem, Morph, OptionCard, SectionHeader, Sidebar,
    TextColor, TextSize,
};
use auris_i18n::Key;
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, Context, IntoElement, KeyDownEvent, Pixels, Size,
    Window, div, point, prelude::*, px, size,
};

use crate::app::{AurisApp, Pane};

/// The start panel's largest size.
const PANEL_WIDTH: Pixels = px(880.0);
const PANEL_HEIGHT: Pixels = px(480.0);
/// The size below which the panel stops yielding to the metal and takes the window instead, so
/// its two cards and footer still fit side by side.
const PANEL_MIN_WIDTH: Pixels = px(640.0);
const PANEL_MIN_HEIGHT: Pixels = px(400.0);
/// How much of the body the panel takes between those sizes. The rest is the liquid metal,
/// which is the screen's welcome and should be seen rather than glimpsed at the edges.
const PANEL_SHARE: f32 = 0.56;

/// Which page of the start panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChooserPage {
    /// Ways to start a new project.
    #[default]
    New,
    /// Recently opened projects.
    Recent,
    /// The first-run setup guide.
    Setup,
}

#[derive(Default)]
pub(crate) struct ProjectChooser {
    return_to_editor: bool,
    pub(crate) page: ChooserPage,
    selected: usize,
    ai: bool,
    pub(crate) setup_step: usize,
    pub(crate) setup_error: Option<String>,
}

/// The start panel growing into the song workshop, or the workshop settling back into it.
///
/// gpui has no implicit transitions, so the hand-over is drawn: for one `motion.normal` the
/// liquid metal stays mounted and a panel-coloured shape travels between the start panel's
/// rectangle and the whole body, while the arriving screen fades in on top of it. Only the
/// shape moves; the arriving content is laid out at its final size from the first frame, so
/// nothing reflows — Atelier's rule against animating layout holds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WelcomeTransition {
    serial: u64,
    started: Instant,
    pub(crate) to_workshop: bool,
    /// How opaque the shape is once it covers the whole body: the workshop's veil over the
    /// metal, as it is on arrival or as it was when Back was pressed.
    settle: f32,
}

impl WelcomeTransition {
    fn running(&self, duration: Duration) -> bool {
        self.started.elapsed() < duration
    }
}

/// The start panel's size inside a body of `area` with at least `margin` around it.
///
/// A share of the body, so a bigger window shows more metal rather than a bigger panel, between
/// a floor that keeps the panel usable and a ceiling past which it would only hold empty space.
fn panel_size(area: Size<Pixels>, margin: Pixels) -> Size<Pixels> {
    let fit = |extent: Pixels, least: Pixels, most: Pixels| {
        (extent * PANEL_SHARE)
            .max(least)
            .min(most)
            .min(extent - margin * 2.0)
            .max(px(0.0))
    };
    size(
        fit(area.width, PANEL_MIN_WIDTH, PANEL_WIDTH),
        fit(area.height, PANEL_MIN_HEIGHT, PANEL_HEIGHT),
    )
}

/// Where the start panel sits inside a body of `area`: its horizontal and vertical insets.
fn panel_insets(area: Size<Pixels>, margin: Pixels) -> (Pixels, Pixels) {
    let panel = panel_size(area, margin);
    (
        (area.width - panel.width) / 2.0,
        (area.height - panel.height) / 2.0,
    )
}

/// The body below the title bar, which the backdrop fills and the panel is centred in.
fn body_area(window: &Window, tabs: Pixels) -> Size<Pixels> {
    let viewport = window.viewport_size();
    size(
        viewport.width,
        viewport.height - crate::titlebar::HEIGHT - tabs,
    )
}

/// How opaque the arriving screen is at `delta`: it waits for the shape to have mostly arrived,
/// so its text never shows over the metal outside the shape.
fn arriving_opacity(delta: f32) -> f32 {
    ((delta - 0.4) / 0.6).clamp(0.0, 1.0)
}

impl AurisApp {
    pub(crate) fn show_editor_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.release_typed_notes();
        self.stop_audition();
        self.close_menu();
        self.close_menu_bar();
        self.show_project_chooser(window, cx);
        if let Some(chooser) = &mut self.project_chooser {
            chooser.return_to_editor = true;
        }
    }

    fn resume_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_chooser = None;
        self.focus_pane(self.last_pane, window);
        cx.notify();
    }

    pub(crate) fn show_project_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_chooser = Some(ProjectChooser::default());
        self.set_status("");
        self.focus.focus(window);
        cx.notify();
    }

    /// Starts the hand-over between the start panel and the workshop, unless motion is off.
    pub(crate) fn begin_welcome_transition(
        &mut self,
        to_workshop: bool,
        settle: f32,
        cx: &mut Context<Self>,
    ) {
        let motion = self.theme.atelier.motion;
        if !motion.enabled {
            self.welcome_transition = None;
            return;
        }
        let serial = self
            .welcome_transition
            .map_or(0, |transition| transition.serial + 1);
        self.welcome_transition = Some(WelcomeTransition {
            serial,
            started: Instant::now(),
            to_workshop,
            settle,
        });
        // The animation asks for frames only while it runs. Wake once after it, so the backdrop
        // is unmounted even when nothing else is drawing — a paused metal frame, for one.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(motion.normal + Duration::from_millis(32))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The hand-over in progress, if any; forgets one that has finished.
    pub(crate) fn welcome_transition(&mut self) -> Option<WelcomeTransition> {
        let duration = self.theme.atelier.motion.normal;
        if self
            .welcome_transition
            .is_some_and(|transition| !transition.running(duration))
        {
            self.welcome_transition = None;
        }
        self.welcome_transition
    }

    /// The liquid metal behind the start panel, filling the body below the title bar.
    ///
    /// The backdrop owns its GPU cache and animation lifetime under this one id, so the workshop
    /// can keep it mounted through the hand-over without a second shader warm-up. Inactive
    /// windows hold a still frame; Atelier's motion policy stills it entirely.
    pub(crate) fn welcome_backdrop(&self, window: &Window) -> Option<AnyElement> {
        if cfg!(test) {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    atelier::MetalBackdrop::new("chooser-metal")
                        .when(!window.is_window_active(), |backdrop| backdrop.phase(0.12)),
                )
                .into_any_element(),
        )
    }

    /// The panel-coloured shape that travels between the start panel and the whole body.
    ///
    /// Atelier's `Morph` owns the transition fill and border, including the translucent veil
    /// that remains behind the workshop. Keeping those values on the morph avoids a second
    /// wrapper animation whose opacity could drift from the shape's geometry.
    pub(crate) fn welcome_morph(
        &self,
        transition: WelcomeTransition,
        window: &Window,
    ) -> AnyElement {
        let settle = transition.settle;
        let theme = &self.theme;
        let area = body_area(window, self.project_tabs_height());
        let (inset_x, inset_y) = panel_insets(area, theme.atelier.spacing.xl);
        let top = crate::titlebar::HEIGHT + self.project_tabs_height();
        let body = Bounds::new(point(px(0.0), top), area);
        let panel = Bounds::new(
            point(inset_x, top + inset_y),
            size(area.width - inset_x * 2.0, area.height - inset_y * 2.0),
        );
        let radius = theme.atelier.shape.radius_xl;
        let to_workshop = transition.to_workshop;
        let id = ("welcome-morph", transition.serial as usize);
        let shape = if to_workshop {
            Morph::new(id, panel).to(body).radius(radius, px(0.0))
        } else {
            Morph::new(id, body).to(panel).radius(px(0.0), radius)
        };
        let veil = gpui::Hsla {
            a: settle,
            ..theme.surface
        };
        div()
            .absolute()
            .inset_0()
            .child(
                shape
                    .fill(theme.surface)
                    .fill_to(veil)
                    .border(theme.border_subtle, None),
            )
            .into_any_element()
    }

    /// Fades the arriving screen in over [`Self::welcome_morph`].
    pub(crate) fn welcome_arrival<E: IntoElement + Styled + 'static>(
        &self,
        transition: WelcomeTransition,
        element: E,
    ) -> AnyElement {
        element
            .with_animation(
                ("welcome-arrival", transition.serial as usize),
                Animation::new(self.theme.atelier.motion.normal),
                |element, delta| element.opacity(arriving_opacity(delta)),
            )
            .into_any_element()
    }

    fn choose_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_project.is_some() {
            return;
        }
        let Some(chooser) = &self.project_chooser else {
            return;
        };
        match chooser.page {
            ChooserPage::Recent => {
                if let Some(path) = self.settings.recent.get(chooser.selected).cloned() {
                    self.open_project_window(path, cx);
                }
            }
            ChooserPage::New if chooser.ai => {
                let arriving = super::song_workshop::workshop_veil(false, px(0.0));
                self.begin_welcome_transition(true, arriving, cx);
                self.open_song_workshop(window, cx);
            }
            ChooserPage::New => {
                let return_to_editor = chooser.return_to_editor;
                self.project_chooser = None;
                self.focus_pane(Pane::Arrangement, window);
                if return_to_editor {
                    self.new_project_asking();
                }
            }
            ChooserPage::Setup => self.step_setup_guide(true, cx),
        }
        cx.notify();
    }

    fn set_chooser_page(&mut self, page: ChooserPage, cx: &mut Context<Self>) {
        if let Some(chooser) = &mut self.project_chooser {
            chooser.page = page;
        }
        cx.notify();
    }

    fn chooser_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.startup_update_prompt_open() {
            if self.prompt_key(event, window, cx) {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let Some(chooser) = self.project_chooser.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "left" if chooser.page == ChooserPage::Setup => {
                chooser.setup_step = chooser.setup_step.saturating_sub(1)
            }
            "right" if chooser.page == ChooserPage::Setup => {
                chooser.setup_step =
                    (chooser.setup_step + 1).min(crate::setup_guide::STEPS.len() - 1)
            }
            "left" => chooser.page = ChooserPage::New,
            "right" => chooser.page = ChooserPage::Recent,
            "up" => chooser.selected = chooser.selected.saturating_sub(1),
            "down" => {
                chooser.selected =
                    (chooser.selected + 1).min(self.settings.recent.len().saturating_sub(1))
            }
            "enter" => self.choose_project(window, cx),
            "escape" if chooser.page == ChooserPage::Setup => self.setup_escape(window, cx),
            "escape" if chooser.return_to_editor && self.opening_project.is_none() => {
                self.resume_editor(window, cx)
            }
            "escape" if self.opening_project.is_none() => {
                if self.project_tabs.is_some() {
                    self.close_project_tab(cx);
                } else {
                    window.remove_window();
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn chooser_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let chooser = self.project_chooser.as_ref().expect("chooser is visible");
        let page = chooser.page;
        let busy = self.opening_project.is_some();
        let recent_count = self.settings.recent.len();
        let item = |id: &'static str, label: Key, icon: IconName, target: ChooserPage| {
            ListItem::new(id, self.t(label))
                .icon(icon)
                .selected(page == target)
                .when(target == ChooserPage::Recent && recent_count > 0, |item| {
                    item.trailing(
                        Label::new(recent_count.to_string())
                            .size(TextSize::XSmall)
                            .color(TextColor::Faint),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_chooser_page(target, cx)))
        };
        Sidebar::new(Edge::Left)
            // Narrow windows give the cards the room; the sidebar is what yields first.
            .w(if window.viewport_size().width < px(1200.0) {
                px(176.0)
            } else {
                px(208.0)
            })
            .p(theme.atelier.spacing.sm)
            .gap(theme.atelier.spacing.xxs)
            .child(item(
                "chooser-new",
                Key::ChooseNew,
                IconName::Plus,
                ChooserPage::New,
            ))
            .child(item(
                "chooser-recents",
                Key::ChooseRecent,
                IconName::History,
                ChooserPage::Recent,
            ))
            .child(div().flex_1())
            // Settings and setup are help, not ways to start: they live at the sidebar's foot
            // with navigation, not with the commit. Settings is the same window the editor opens.
            // Atelier's button tooltip is plain text; the wrapper carries the keyed card, so the
            // shortcut shown is whatever the keymap currently says.
            .child(
                div()
                    .id("chooser-settings-tip")
                    .w_full()
                    .tooltip(self.tip(Key::Settings, "view.settings"))
                    .child(
                        Button::new("chooser-settings", self.t(Key::MenuSettingsItem))
                            .ghost()
                            .icon(IconName::Settings)
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_settings(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Button::new("chooser-setup", self.t(Key::SetupGuide))
                    .ghost()
                    .icon(IconName::Info)
                    .selected(page == ChooserPage::Setup)
                    .disabled(busy)
                    .full_width()
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_chooser_page(ChooserPage::Setup, cx)),
                    ),
            )
    }

    fn chooser_new_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let chooser = self.project_chooser.as_ref().expect("chooser is visible");
        let ai_selected = chooser.ai;
        let card = |id: &'static str, ai: bool| {
            let selected = ai == ai_selected;
            let on_click = cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if this.opening_project.is_some() {
                    return;
                }
                if let Some(chooser) = &mut this.project_chooser {
                    chooser.ai = ai;
                }
                if event.click_count() == 2 {
                    this.choose_project(window, cx);
                }
                cx.notify();
            });
            let card = if ai {
                // Only the welcome entry keeps the brand pink; conversation surfaces stay quiet.
                let colour = atelier::ensure_contrast(
                    theme.atelier.colors.agent_gradient[1],
                    theme.surface,
                    4.5,
                );
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .tab_index(0)
                    .key_context(atelier::CONTROL_CONTEXT)
                    .flex()
                    .flex_col()
                    .gap(theme.atelier.spacing.md)
                    .p(theme.atelier.spacing.lg)
                    .rounded(theme.atelier.shape.radius_lg)
                    .bg(theme.atelier.colors.elevated)
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .focus(|style| {
                        style
                            .border_color(theme.accent)
                            .shadow(theme.atelier.focus_ring())
                    })
                    .when(selected, |card| {
                        card.shadow(vec![gpui::BoxShadow {
                            color: theme.atelier.colors.focus_ring.opacity(0.35),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(1.),
                            spread_radius: px(3.),
                        }])
                    })
                    .when(!selected, |card| {
                        card.hover(|style| style.bg(theme.surface_hover))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(56.))
                            .rounded(
                                atelier::Theme::concentric(
                                    theme.atelier.shape.radius_lg,
                                    theme.atelier.spacing.lg,
                                )
                                .max(theme.atelier.shape.radius_md),
                            )
                            .bg(colour.opacity(0.10))
                            .child(
                                atelier::Icon::new(IconName::Sparkles)
                                    .size(px(28.))
                                    .color(colour),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.atelier.spacing.xs)
                            .child(
                                Label::new(self.t(Key::WorkshopTitle))
                                    .size(TextSize::Large)
                                    .heading_weight(),
                            )
                            .child(
                                Label::new(self.t(Key::ChooseAiHelp))
                                    .size(TextSize::Small)
                                    .color(TextColor::Muted),
                            ),
                    )
                    .on_click(on_click)
                    .into_any_element()
            } else {
                OptionCard::new(id, IconName::Music, self.t(Key::ChooseEmpty))
                    .description(self.t(Key::ChooseEmptyHelp))
                    .selected(selected)
                    .on_click(on_click)
                    .into_any_element()
            };
            div()
                .debug_selector(move || id.into())
                .flex_1()
                .min_w(px(220.))
                .max_w(px(340.))
                .child(card)
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.atelier.spacing.xl)
            .child(
                SectionHeader::new(self.t(Key::ChooseNew))
                    .description(self.t(Key::ChooseStartHelp)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.atelier.spacing.lg)
                    .child(card("chooser-empty", false))
                    .child(card("chooser-ai", true)),
            )
            .into_any_element()
    }

    fn chooser_recent_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let chooser = self.project_chooser.as_ref().expect("chooser is visible");
        let selected = chooser.selected;
        if self.settings.recent.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .justify_center()
                .child(
                    EmptyState::new(IconName::History, self.t(Key::ChooseRecent))
                        .description(self.t(Key::ChooseRecentEmpty)),
                )
                .into_any_element();
        }
        let rows = self
            .settings
            .recent
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let title = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                ListItem::new(("chooser-recent", index), title)
                    .icon(IconName::File)
                    .description(path.display().to_string())
                    .selected(selected == index)
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            if let Some(chooser) = &mut this.project_chooser {
                                chooser.selected = index;
                            }
                            if event.click_count() == 2 {
                                this.choose_project(window, cx);
                            }
                            cx.notify();
                        }),
                    )
            });
        div()
            .flex()
            .flex_col()
            .gap(theme.atelier.spacing.md)
            .child(SectionHeader::new(self.t(Key::ChooseRecent)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.atelier.spacing.xxs)
                    .children(rows),
            )
            .into_any_element()
    }

    fn chooser_footer(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let chooser = self.project_chooser.as_ref().expect("chooser is visible");
        let return_to_editor = chooser.return_to_editor;
        if chooser.page == ChooserPage::Setup {
            return self.render_setup_footer(chooser.setup_step, cx);
        }
        let busy = self.opening_project.is_some();
        let recent = chooser.page == ChooserPage::Recent;
        let can_choose = !busy && (!recent || chooser.selected < self.settings.recent.len());
        let commit = self.t(if recent {
            Key::ChooseOpen
        } else if chooser.ai {
            Key::ChooseStartAi
        } else {
            Key::ChooseCreate
        });
        vec![
            Button::new("chooser-browse", self.t(Key::ChooseExisting))
                .icon(IconName::Folder)
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.pick_and_open_project(cx)))
                .into_any_element(),
            div().flex_1().into_any_element(),
            Button::new(
                "chooser-close",
                self.t(if return_to_editor {
                    Key::ResumeProject
                } else {
                    Key::ChooseClose
                }),
            )
            .disabled(busy)
            .on_click(cx.listener(move |this, _, window, cx| {
                if return_to_editor {
                    this.resume_editor(window, cx);
                } else if this.project_tabs.is_some() {
                    this.close_project_tab(cx);
                } else {
                    window.remove_window();
                }
            }))
            .into_any_element(),
            Button::new("chooser-confirm", commit)
                .primary()
                .disabled(!can_choose)
                .on_click(cx.listener(|this, _, window, cx| this.choose_project(window, cx)))
                .into_any_element(),
        ]
    }

    pub(crate) fn render_project_chooser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transition = self.welcome_transition();
        if self.song_workshop.is_some() {
            return self.render_song_workshop(transition.filter(|t| t.to_workshop), window, cx);
        }
        let prompt = if self.startup_update_prompt_open() {
            self.reconcile_focus(window, cx);
            self.render_prompt(cx)
        } else {
            if self.modal_focus.prompt_contains_focused(window) {
                window.focus(&self.focus);
            }
            None
        };
        let update = self.render_startup_update(cx);
        let project_tabs = self.render_project_tabs(window, cx);
        let transition = transition.filter(|t| !t.to_workshop);
        let theme = self.theme.clone();
        let spacing = theme.atelier.spacing;
        let chooser = self.project_chooser.as_ref().expect("chooser is visible");
        let page = chooser.page;
        let setup_step = chooser.setup_step;
        let body = match page {
            ChooserPage::New => self.chooser_new_page(cx),
            ChooserPage::Recent => self.chooser_recent_page(cx),
            ChooserPage::Setup => self.render_setup_page(setup_step, cx),
        };
        let content = div()
            .id((
                "chooser-content",
                match page {
                    ChooserPage::New => 0,
                    ChooserPage::Recent => 1,
                    ChooserPage::Setup => setup_step + 2,
                },
            ))
            .debug_selector(|| "chooser-content".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(spacing.xxl)
            .flex()
            .flex_col()
            .child(body);
        let status = (!self.status.is_empty()
            && !update
                .as_ref()
                .is_some_and(|(_, message)| message == &self.status))
        .then(|| {
            div()
                .px(spacing.xxl)
                .pb(spacing.md)
                .text_size(theme.atelier.typography.size_sm)
                .text_color(if self.status_failed {
                    theme.danger
                } else {
                    theme.text_muted
                })
                .child(self.status.clone())
        });
        let footer = div()
            .flex()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap(spacing.sm)
            .min_h(theme.atelier.density.toolbar)
            .h_auto()
            .px(spacing.lg)
            .py(spacing.sm)
            .border_t_1()
            .border_color(theme.border_subtle)
            .children(self.chooser_footer(cx));
        let panel_size = panel_size(body_area(window, self.project_tabs_height()), spacing.xl);
        let panel = div()
            .occlude()
            .relative()
            .flex()
            .w(panel_size.width)
            .h(panel_size.height)
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .rounded(theme.atelier.shape.radius_xl)
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border_subtle)
            .shadow(theme.atelier.elevation.high.clone())
            .child(self.chooser_sidebar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(content)
                    .children(update.map(|(notice, _)| notice))
                    .children(status)
                    .child(footer),
            );
        let panel = match transition {
            Some(transition) => self.welcome_arrival(transition, panel),
            None => panel.into_any_element(),
        };
        div()
            .id("project-chooser")
            .debug_selector(|| "project-chooser".into())
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.atelier.window_fill())
            .text_color(theme.text)
            .font(theme.font.clone())
            .text_sm()
            .track_focus(&self.focus)
            .key_context(crate::actions::KEY_CONTEXT)
            .on_action(
                cx.listener(|_, _: &crate::actions::FocusNextPane, window, _| window.focus_next()),
            )
            .on_action(
                cx.listener(|_, _: &crate::actions::FocusPreviousPane, window, _| {
                    window.focus_prev()
                }),
            )
            .on_key_down(cx.listener(Self::chooser_key))
            .on_action(cx.listener(|this, _: &crate::actions::OpenProject, _, cx| {
                if this.opening_project.is_none() {
                    this.pick_and_open_project(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &crate::actions::OpenSettings, _, cx| {
                    this.open_settings(cx);
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &crate::actions::Quit, window, cx| {
                this.quit_asking(window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::actions::NewProject, _, cx| {
                crate::project_windows::open_in(None, this.project_window, cx);
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &crate::actions::NextProjectTab, _, cx| {
                    this.cycle_project_tab(true, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::PreviousProjectTab, _, cx| {
                    this.cycle_project_tab(false, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::CloseProjectTab, _, cx| {
                    this.close_project_tab(cx)
                }),
            )
            .child(crate::titlebar::bar(
                window,
                &theme,
                crate::titlebar::Parts::new("chooser-title", self.t(Key::ChooseProject)),
                crate::titlebar::controls(
                    window,
                    &theme,
                    cx.listener(|this, _, window, cx| this.close_project_window(window, cx)),
                ),
            ))
            .children(project_tabs)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .p(spacing.xl)
                    .children(self.welcome_backdrop(window))
                    .children(transition.map(|transition| self.welcome_morph(transition, window)))
                    .child(panel),
            )
            .children(crate::titlebar::resize_handles(window))
            .children(prompt)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use auris_i18n::Language;
    use gpui::{px, size};

    #[test]
    fn the_panel_leaves_most_of_a_large_window_to_the_metal_and_still_fits_a_small_one() {
        let margin = px(24.0);
        let large = super::panel_size(size(px(1600.0), px(1000.0)), margin);
        assert_eq!(large, size(super::PANEL_WIDTH, super::PANEL_HEIGHT));
        assert!(f32::from(large.width) * f32::from(large.height) < 1600.0 * 1000.0 * 0.3);
        let medium = super::panel_size(size(px(1300.0), px(820.0)), margin);
        assert!(medium.width < px(1300.0) * 0.6 && medium.height < px(820.0) * 0.6);
        let small = super::panel_size(size(px(600.0), px(380.0)), margin);
        assert_eq!(small, size(px(552.0), px(332.0)), "never past the margin");
    }

    #[gpui::test]
    fn japanese_chooser_footer_stays_inside_a_short_narrow_panel(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.language = Language::Japanese;
                this.show_project_chooser(window, cx);
            });
        });
        crate::harness::resize(&app, cx, size(px(600.0), px(380.0)));
        crate::harness::paint(&app, cx);

        let chooser = cx
            .debug_bounds("project-chooser")
            .expect("chooser is visible");
        for action in ["chooser-browse", "chooser-close", "chooser-confirm"] {
            let bounds = cx
                .debug_bounds(action)
                .unwrap_or_else(|| panic!("{action} remains rendered"));
            assert!(
                bounds.left() >= chooser.left(),
                "{action} escaped left edge"
            );
            assert!(
                bounds.right() <= chooser.right(),
                "{action} escaped right edge"
            );
            assert!(
                bounds.bottom() <= chooser.bottom(),
                "{action} escaped bottom edge"
            );
        }
    }

    #[gpui::test]
    fn choosing_a_start_card_waits_for_the_explicit_commit(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.show_project_chooser(window, cx));
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("chooser-ai", cx);
        app.read_with(cx, |app, _| {
            assert!(app.project_chooser.as_ref().unwrap().ai);
            assert!(app.song_workshop.is_none());
        });
        crate::harness::click("chooser-empty", cx);
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |app, _| window.focus(&app.focus));
        });
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        app.read_with(cx, |app, _| {
            assert!(app.project_chooser.is_none());
            assert!(app.song_workshop.is_none());
        });
    }

    #[gpui::test]
    fn chooser_creates_in_place_and_keeps_audio_suspended(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.session.set_audio_suspended(true).unwrap();
                app.show_project_chooser(window, cx);
            })
        });
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |app, _| {
            assert!(app.project_chooser.is_none());
            assert!(app.session.audio_suspended());
            assert!(!app.session.is_dirty());
        });
    }

    #[gpui::test]
    fn the_chooser_sidebar_opens_settings(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.show_project_chooser(window, cx));
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |app, _| assert!(app.settings_window.is_none()));
        crate::harness::click("chooser-settings", cx);
        app.read_with(cx, |app, _| {
            assert!(app.settings_window.is_some(), "the button opens Settings");
            assert!(app.project_chooser.is_some(), "and the chooser stays");
        });
    }

    #[gpui::test]
    fn the_settings_shortcut_works_on_the_chooser(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.show_project_chooser(window, cx));
        });
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |app, _| window.focus(&app.focus));
        });
        cx.simulate_keystrokes("secondary-,");
        app.read_with(cx, |app, _| {
            assert!(app.settings_window.is_some(), "the shortcut opens Settings");
            assert!(app.project_chooser.is_some());
        });
    }

    #[gpui::test]
    fn empty_recents_keep_the_chooser_open(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.settings.recent.clear();
                app.show_project_chooser(window, cx);
            })
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("chooser-recents", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("chooser-confirm", cx);
        app.read_with(cx, |app, _| assert!(app.project_chooser.is_some()));
    }
}
