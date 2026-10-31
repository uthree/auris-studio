//! Inline score drafts; only immutable, explicitly attached snapshots reach a worker.

use super::*;
use crate::ui::reference_piano_roll::ReferencePianoRoll;
use auris_session::midi_reference::MidiReference;

pub(crate) struct MidiDraft {
    score: MidiReference,
    replacing: Option<PathBuf>,
    track: usize,
    editing: bool,
    claims_keys: bool,
    pub(crate) editor: Option<gpui::Entity<ReferencePianoRoll>>,
    editor_subscription: Option<gpui::Subscription>,
}

impl MidiDraft {
    fn new(score: MidiReference, replacing: Option<PathBuf>) -> Self {
        Self {
            score,
            replacing,
            track: 0,
            editing: false,
            claims_keys: false,
            editor: None,
            editor_subscription: None,
        }
    }

    fn read_editor(&mut self, cx: &gpui::App) {
        let Some(editor) = &self.editor else {
            return;
        };
        let editor = editor.read(cx);
        let app = editor.app.read(cx);
        // Keep the immutable attachment snapshot current even when teardown has no GPUI context.
        for (clip, track) in editor.clips.iter().zip(&mut self.score.tracks) {
            if let Some(clip) = app.session.midi_clip(*clip) {
                track.notes.clone_from(&clip.notes);
            }
        }
        self.track = editor
            .clips
            .iter()
            .position(|id| Some(*id) == app.selected_clip)
            .unwrap_or(self.track);
        self.editing = app.prompt.is_some() || app.drag.is_some();
        self.claims_keys = app.keys_are_claimed();
    }
}

impl AurisApp {
    /// Prevent ancestor key bindings from intercepting a reference editor's text or menu keys.
    pub(crate) fn reference_score_claims_keys(&self) -> bool {
        self.music_references()
            .midi_draft
            .as_ref()
            .is_some_and(|draft| draft.claims_keys)
    }

    fn ensure_midi_piano_roll(&mut self, cx: &mut gpui::Context<Self>) {
        let grid = self.settings.piano_roll_grid;
        let relative_snap = self.settings.relative_note_snap;
        let division = self.project().grid;
        let Some(draft) = self.music_references_mut().midi_draft.as_mut() else {
            return;
        };
        if draft.editor.is_some() {
            return;
        }
        match ReferencePianoRoll::new(&draft.score, cx) {
            Ok(editor) => {
                let child = editor.read(cx).app.clone();
                child.update(cx, |app, _| {
                    app.settings.piano_roll_grid = grid;
                    app.settings.relative_note_snap = relative_snap;
                    app.session.set_grid(division);
                    app.session.forget_history();
                });
                draft.editor = Some(editor);
                draft.editor_subscription = Some(cx.observe(&child, |this, child, cx| {
                    let Some(draft) = this.music_references_mut().midi_draft.as_mut() else {
                        return;
                    };
                    if !draft
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.read(cx).app == child)
                    {
                        return;
                    }
                    draft.read_editor(cx);
                    cx.notify();
                }));
            }
            Err(error) => self.music_references_mut().attachment_error = Some(error.to_string()),
        }
    }

    pub(crate) fn cancel_midi_for_attachment(&mut self, path: &Path) {
        if self
            .music_references()
            .midi_draft
            .as_ref()
            .is_some_and(|d| d.replacing.as_deref() == Some(path))
        {
            self.music_references_mut().midi_draft = None;
            if let Some(s) = &mut self.song_workshop {
                s.active_field = Some(0);
            } else {
                self.agent_chat.focused = Some(AgentField::Chat);
            }
        }
    }
    pub(crate) fn start_midi_draft(&mut self, from_selection: bool) {
        if self.music_references().midi_draft.is_some()
            || self.music_composer_locked()
            || self.music_references().midi_loading
        {
            return;
        }
        let project = self.session.project();
        let score = if from_selection {
            let Some(clip) = self.selected_clip.and_then(|id| self.session.midi_clip(id)) else {
                return;
            };
            MidiReference::from_clip(clip, project.tempo_map.clone(), project.signatures.clone())
        } else {
            Ok(MidiReference::new(
                self.t(Key::AgentMidiMelody).to_owned(),
                project.tempo_map.clone(),
                project.signatures.clone(),
            ))
        };
        match score {
            Ok(score) => self.music_references_mut().midi_draft = Some(MidiDraft::new(score, None)),
            Err(error) => self.music_references_mut().attachment_error = Some(error),
        }
    }

    pub(crate) fn edit_midi_attachment(&mut self, path: PathBuf, cx: &mut gpui::Context<Self>) {
        if self.music_references().midi_draft.is_some()
            || self.music_references().midi_loading
            || self.music_composer_locked()
        {
            return;
        }
        if !path.is_absolute() {
            self.music_references_mut().attachment_error = Some(
                self.t(Key::AgentAttachmentUnavailable)
                    .replace("{path}", &path.to_string_lossy()),
            );
            cx.notify();
            return;
        }
        let from_draft = self.music_references().attachments.contains(&path);
        self.music_references_mut().midi_loading = true;
        let owner = self.music_owner();
        cx.spawn(async move |this, cx| {
            let source = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { MidiReference::read(&source) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.music_owner() != owner {
                    if let super::music::MusicOwner::Agent(project, conversation) = &owner
                        && this.session.path() == project.as_deref()
                        && this.agent_chat.conversations.active_id() == conversation.as_deref()
                    {
                        this.agent_chat.references.midi_loading = false;
                    }
                    cx.notify();
                    return;
                }
                this.music_references_mut().midi_loading = false;
                if this.music_composer_locked()
                    || this.music_references().midi_draft.is_some()
                    || (from_draft && !this.music_references().attachments.contains(&path))
                {
                    return;
                }
                match result {
                    Ok(score) => {
                        this.music_references_mut().midi_draft =
                            Some(MidiDraft::new(score, from_draft.then_some(path)))
                    }
                    Err(error) => this.music_references_mut().attachment_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn choose_midi_attachment(&mut self, cx: &mut gpui::Context<Self>) {
        let owner = self.music_owner();
        cx.spawn(async move |this, cx| {
            let Some(file) = rfd::AsyncFileDialog::new()
                .add_filter("MIDI", &["mid", "midi"])
                .pick_file()
                .await
            else {
                return;
            };
            let path = file.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                if this.music_owner() != owner {
                    return;
                }
                this.attach_music_files(std::slice::from_ref(&path), cx);
                this.edit_midi_attachment(path, cx);
            });
        })
        .detach();
    }

    /// Commit the visible surface's independent draft before submitting or leaving it.
    pub(crate) fn commit_midi_draft(&mut self) -> bool {
        let limit_error = self.t(Key::AgentAttachmentLimit).to_owned();
        let refs = self.music_references_mut();
        let Some(draft) = refs.midi_draft.as_mut() else {
            return true;
        };
        if draft.editing {
            return false;
        }
        let replacing = draft
            .replacing
            .as_ref()
            .and_then(|path| refs.attachments.iter().position(|p| p == path));
        if replacing.is_none() && refs.attachments.len() >= auris_agent::ATTACHMENT_COUNT_LIMIT {
            refs.attachment_error = Some(limit_error);
            return false;
        }
        let directory = auris_session::settings::config_dir().join("conversation-midi");
        match draft.score.save_snapshot(&directory) {
            Ok(path) => {
                refs.attached_here.insert(path.clone());
                if let Some(index) = replacing {
                    refs.attachments[index] = path;
                } else {
                    refs.attachments.push(path);
                }
                refs.midi_draft = None;
                refs.attachment_error = None;
                if let Some(s) = &mut self.song_workshop {
                    s.active_field = Some(0);
                } else {
                    self.agent_chat.focused = Some(AgentField::Chat);
                }
                true
            }
            Err(error) => {
                refs.attachment_error = Some(error);
                false
            }
        }
    }

    pub(super) fn commit_agent_midi_draft(&mut self) -> bool {
        self.song_workshop.is_some() || self.commit_midi_draft()
    }

    pub(crate) fn midi_composer_controls(
        &mut self,
        locked: bool,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let menu_open = self.menu.as_ref().is_some_and(|menu| {
            menu.entries.iter().any(|entry| {
                matches!(entry,
                super::super::context_menu::MenuEntry::Item(item)
                    if matches!(item.command, MenuCommand::ComposerMelody(_)))
            })
        });
        div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .child(
                atelier::Button::new("music-reference-menu", self.t(Key::AgentMusicReferences))
                    .ghost()
                    .size(atelier::ControlSize::Small)
                    .icon(atelier::IconName::More)
                    .icon_after(atelier::IconName::ChevronDown)
                    .selected(menu_open)
                    .disabled(locked)
                    .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                        if this.music_composer_locked() {
                            return;
                        }
                        let available = this.can_attach_agent_music();
                        let midi_available = available
                            && !this.music_references().midi_loading
                            && this.music_references().midi_draft.is_none();
                        let has_clip = this
                            .selected_clip
                            .and_then(|id| this.session.midi_clip(id))
                            .is_some();
                        let mut menu =
                            ContextMenu::new(event.position(), this.t(Key::AgentMusicReferences))
                                .item_greyed_unless(
                                    midi_available,
                                    this.t(Key::AgentMidiNew),
                                    MenuCommand::ComposerMelody(false),
                                )
                                .item_greyed_unless(
                                    midi_available && has_clip,
                                    this.t(Key::AgentMidiClip),
                                    MenuCommand::ComposerMelody(true),
                                )
                                .item_greyed_unless(
                                    midi_available,
                                    this.t(Key::AgentMidiFile),
                                    MenuCommand::ComposerMidi,
                                )
                                .item_greyed_unless(
                                    available,
                                    this.t(Key::AgentChords),
                                    MenuCommand::ComposerChords,
                                )
                                .item_greyed_unless(
                                    available,
                                    this.t(Key::AgentSound),
                                    MenuCommand::ComposerSound,
                                );
                        menu.step(1);
                        this.open_menu(menu);
                        cx.notify();
                    })),
            )
            .when(self.music_references().midi_loading, |row| {
                row.child(self.t(Key::AgentMidiLoading))
            })
            .into_any_element()
    }

    pub(crate) fn midi_editor(
        &mut self,
        max_height: gpui::Pixels,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        self.ensure_midi_piano_roll(cx);
        let Some(draft) = &self.music_references().midi_draft else {
            return div().into_any_element();
        };
        let theme = self.theme.clone();
        let editor = draft.editor.clone();
        if let Some(editor) = &editor {
            let child = editor.read(cx).app.clone();
            child.update(cx, |app, _| {
                app.theme = theme.clone();
                app.language = self.language();
                app.pointer = self.pointer;
            });
        }
        let track = draft.track;
        let track_count = draft.score.tracks.len();
        let track_name = draft.score.tracks[track].name.clone();
        div()
            .id("agent-midi-editor")
            .debug_selector(|| "agent-midi-editor".into())
            .flex()
            .flex_col()
            .min_w_0()
            .flex_shrink()
            .h(max_height)
            .min_h(gpui::rems(6.0))
            .border_1()
            .border_color(theme.border)
            .rounded(Metrics::RADIUS_SM)
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .min_w_0()
                    .gap_1()
                    .p_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(draft.score.name.clone()),
                    )
                    .child(bounded_button_enabled(
                        "agent-midi-cancel",
                        self.t(Key::Cancel),
                        ButtonStyle::Ghost,
                        ButtonState::available(false, true),
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, window, cx| {
                            this.close_midi_editor(window);
                            cx.notify();
                        }),
                    ))
                    .child(bounded_button_enabled(
                        "agent-midi-apply",
                        self.t(Key::AgentMidiApply),
                        ButtonStyle::Normal,
                        ButtonState::available(false, !draft.editing),
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, window, cx| {
                            if this.commit_midi_draft() {
                                this.close_midi_editor(window);
                            }
                            cx.notify();
                        }),
                    )),
            )
            .when(track_count > 1, |column| {
                column.child(
                    div()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .gap_1()
                        .child(self.midi_track_button(
                            "agent-midi-track-prev",
                            "◀",
                            track.checked_sub(1),
                            cx,
                        ))
                        .child(div().flex_1().min_w_0().truncate().text_xs().child(format!(
                            "{} / {} · {}",
                            track + 1,
                            track_count,
                            track_name
                        )))
                        .child(self.midi_track_button(
                            "agent-midi-track-next",
                            "▶",
                            (track + 1 < track_count).then_some(track + 1),
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .capture_any_mouse_down(cx.listener(|this, _, _, _| {
                        if let Some(s) = &mut this.song_workshop {
                            s.active_field = None;
                        } else {
                            this.agent_chat.focused = None;
                        }
                    }))
                    .children(editor),
            )
            .into_any_element()
    }

    fn midi_track_button(
        &self,
        id: &'static str,
        label: &str,
        track: Option<usize>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        bounded_button_enabled(
            id,
            label.to_owned(),
            ButtonStyle::Ghost,
            ButtonState::available(false, track.is_some()),
            self.theme.accent,
            &self.theme,
            cx.listener(move |this, _, window, cx| {
                if let Some(track) = track
                    && let Some(draft) = this.music_references_mut().midi_draft.as_mut()
                    && let Some(editor) = &draft.editor
                {
                    let editor = editor.read(cx);
                    let clip = editor.clips[track];
                    editor.app.clone().update(cx, |app, cx| {
                        app.selected_clip = Some(clip);
                        app.selected_track = app.project().track_of_clip(clip);
                        app.selected_clips = [clip].into_iter().collect();
                        app.selected_notes.clear();
                        app.focus_pane(crate::app::Pane::PianoRoll, window);
                        cx.notify();
                    });
                    draft.track = track;
                }
                cx.notify();
            }),
        )
        .into_any_element()
    }

    pub(crate) fn close_midi_editor(&mut self, window: &mut Window) {
        self.music_references_mut().midi_draft = None;
        if let Some(s) = &mut self.song_workshop {
            s.active_field = Some(0);
            window.focus(&s.foci[0]);
        } else {
            self.focus_agent_field(AgentField::Chat);
            if let Some(focus) = &self.agent_chat.input_focus {
                window.focus(focus);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn midi_editor_stays_inside_a_narrow_panel_at_large_text_sizes(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.panels
                .set_size(crate::dock::Dock::Right, crate::dock::PanelLayout::MIN_SIDE);
            this.agent_chat.models_loaded = true;
            this.start_midi_draft(false);
        });
        for (scheme, language, font_size) in [
            ("midnight", auris_i18n::Language::English, 16.0),
            ("daylight", auris_i18n::Language::Japanese, 24.0),
        ] {
            app.update(cx, |this, _| {
                this.theme = Theme::named(scheme);
                this.language = language;
            });
            cx.update(|window, _| window.set_rem_size(px(font_size)));
            crate::harness::resize(&app, cx, gpui::size(px(720.0), px(720.0)));
            let panel = cx.debug_bounds("agent-panel").unwrap();
            for selector in ["agent-midi-editor", "agent-midi-apply", "agent-input"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(
                    bounds.left() >= panel.left() && bounds.right() <= panel.right(),
                    "{selector}: {bounds:?} outside {panel:?}"
                );
                assert!(
                    bounds.top() >= panel.top() && bounds.bottom() <= panel.bottom(),
                    "{selector}: {bounds:?} outside {panel:?}"
                );
            }
        }
    }

    #[gpui::test]
    fn midi_queued_messages_hold_independent_score_snapshots(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.model = "test-model".into();
            this.agent_chat.busy = true;
            for pitch in [60, 67] {
                this.start_midi_draft(false);
                this.agent_chat
                    .references
                    .midi_draft
                    .as_mut()
                    .unwrap()
                    .score
                    .tracks[0]
                    .notes
                    .push(auris_session::prelude::Note::new(
                        pitch,
                        auris_session::prelude::Ticks::ZERO,
                        auris_session::prelude::Ticks(480),
                    ));
                this.agent_chat.input = TextField::new("Use this melody");
                assert!(this.enqueue_agent_message());
            }
            let first = &this.agent_chat.queued[0].attachments[0];
            let second = &this.agent_chat.queued[1].attachments[0];
            assert_ne!(first, second);
            assert_eq!(
                MidiReference::read(first).unwrap().tracks[0].notes[0].pitch,
                60
            );
            assert_eq!(
                MidiReference::read(second).unwrap().tracks[0].notes[0].pitch,
                67
            );
        });
    }

    #[gpui::test]
    fn midi_clip_copy_uses_shared_gestures_and_lyrics_without_editing_the_song(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::*;
        use auris_session::prelude::*;
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.selected_clip = Some(clip);
            this.panels.show(crate::dock::Panel::Agent);
            this.panels
                .set_size(crate::dock::Dock::Right, gpui::px(640.0));
            this.agent_chat.models_loaded = true;
            this.pointer = crate::gestures::PointerGestures::default();
            this.settings.piano_roll_grid = auris_session::PianoRollGrid::Sixteenth;
            this.settings.relative_note_snap = false;
            this.start_midi_draft(true);
        });
        paint(&app, cx);
        let editor = reference_editor(&app, cx);
        show_pitch(&editor, cx, 60);
        let at = roll_point(&editor, cx, Ticks(240), 60);
        click_at(cx, at, creating());
        paint(&app, cx);
        let from = roll_point(&editor, cx, Ticks(240), 60);
        let to = roll_point(&editor, cx, Ticks(720), 62);
        drag(cx, from, to);
        paint(&app, cx);
        let at = roll_point(&editor, cx, Ticks(720), 62);
        double_click(cx, at);
        paint(&app, cx);
        assert!(editor.read_with(cx, |app, _| app.prompt.is_some()));
        assert!(
            cx.debug_bounds("prompt-text-field").is_some(),
            "shared prompt was not painted"
        );
        cx.update(|window, cx| {
            assert!(
                editor.read(cx).focus.is_focused(window),
                "reference focus lost; parent field: {:?}; parent prompt: {:?}",
                app.read(cx).agent_chat.focused,
                app.read(cx).prompt.as_ref()
            );
        });
        cx.simulate_input("きみ");
        editor.read_with(cx, |app, _| {
            assert_eq!(
                app.prompt
                    .as_ref()
                    .and_then(|p| p.field())
                    .map(|f| f.content()),
                Some("きみ"),
                "native field did not receive text"
            )
        });
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        editor.read_with(cx, |app, _| {
            assert!(
                app.prompt.is_none(),
                "lyric prompt remains: {:?}; notes: {:?}",
                app.prompt.as_ref(),
                app.selected_midi_clip().unwrap().notes
            );
            assert!(app.drag.is_none(), "drag remains after release");
        });
        click("agent-midi-apply", cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.agent_chat.references.midi_draft.is_none(),
                "editing: {:?}; error: {:?}",
                this.agent_chat
                    .references
                    .midi_draft
                    .as_ref()
                    .map(|d| d.editing),
                this.agent_chat.references.attachment_error
            );
            let score = MidiReference::read(&this.agent_chat.references.attachments[0]).unwrap();
            assert_eq!(score.tracks[0].notes[0].pitch, 62);
            assert_eq!(score.tracks[0].notes[0].lyric, "きみ");
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty());
        });
    }

    #[gpui::test]
    fn reference_undo_and_cancel_leave_the_song_and_composer_intact(cx: &mut gpui::TestAppContext) {
        use crate::harness::*;
        use auris_session::prelude::*;
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.agent_chat.input = TextField::new("Keep this draft");
            this.pointer = crate::gestures::PointerGestures::default();
            this.settings.piano_roll_grid = auris_session::PianoRollGrid::Sixteenth;
            this.settings.relative_note_snap = false;
            this.start_midi_draft(false);
        });
        let before = app.read_with(cx, |app, _| app.project().clone());
        paint(&app, cx);
        let editor = reference_editor(&app, cx);
        show_pitch(&editor, cx, 60);
        let at = roll_point(&editor, cx, Ticks(240), 60);
        click_at(cx, at, creating());
        paint(&app, cx);
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        editor.read_with(cx, |app, _| {
            assert!(app.selected_midi_clip().unwrap().notes.is_empty())
        });
        cx.dispatch_action(crate::actions::Redo);
        paint(&app, cx);
        editor.read_with(cx, |app, _| {
            assert_eq!(app.selected_midi_clip().unwrap().notes.len(), 1)
        });
        click("agent-midi-cancel", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.references.midi_draft.is_none());
            assert!(this.agent_chat.references.attachments.is_empty());
            assert_eq!(this.agent_chat.focused, Some(AgentField::Chat));
            assert_eq!(this.agent_chat.input.content(), "Keep this draft");
            assert_eq!(this.project(), &before);
        });
    }
    #[gpui::test]
    fn midi_track_switching_preserves_channels_and_unedited_parts(cx: &mut gpui::TestAppContext) {
        use crate::harness::*;
        use auris_session::{midi_reference::ReferenceTrack, prelude::*};
        let (app, cx) = open(cx);
        app.update(cx, |app, _| {
            app.panels.show(crate::dock::Panel::Agent);
            app.agent_chat.models_loaded = true;
            app.pointer = crate::gestures::PointerGestures::default();
            app.settings.piano_roll_grid = auris_session::PianoRollGrid::Sixteenth;
            app.settings.relative_note_snap = false;
            app.start_midi_draft(false);
            let draft = app.agent_chat.references.midi_draft.as_mut().unwrap();
            draft.score.tracks[0].channel = 9;
            draft.score.tracks[0]
                .notes
                .push(Note::new(36, Ticks::ZERO, Ticks(480)));
            draft.score.tracks.push(ReferenceTrack {
                name: "Lead".into(),
                channel: 2,
                notes: vec![],
            });
        });
        paint(&app, cx);
        click("agent-midi-track-next", cx);
        paint(&app, cx);
        let editor = reference_editor(&app, cx);
        show_pitch(&editor, cx, 72);
        let at = roll_point(&editor, cx, Ticks(240), 72);
        click_at(cx, at, creating());
        paint(&app, cx);
        click("agent-midi-apply", cx);
        app.read_with(cx, |app, _| {
            let score = MidiReference::read(&app.agent_chat.references.attachments[0]).unwrap();
            assert_eq!(score.tracks[0].channel, 9);
            assert_eq!(
                score.tracks[0].notes[0],
                Note::new(36, Ticks::ZERO, Ticks(480))
            );
            assert_eq!(score.tracks[1].channel, 2);
            assert_eq!(score.tracks[1].notes[0].pitch, 72);
        });
    }
}
