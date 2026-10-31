//! Chord and sound-map references use the same immutable attachment path as files.

use super::*;
use crate::ui::prompt::{Prompt, PromptTarget};
use auris_session::prelude::Ticks;

/// Independent draft references for each conversation surface.
#[derive(Default)]
pub(crate) struct MusicReferences {
    pub(crate) attachments: Vec<PathBuf>,
    // Restored paths are display metadata until explicitly attached in this window.
    pub(crate) attached_here: std::collections::HashSet<PathBuf>,
    pub(crate) attachment_error: Option<String>,
    pub(crate) midi_draft: Option<super::midi::MidiDraft>,
    pub(crate) midi_loading: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MusicOwner {
    Workshop(u64),
    Agent(Option<PathBuf>, Option<String>),
}

impl AurisApp {
    pub(crate) fn attach_music_files(&mut self, paths: &[PathBuf], cx: &mut gpui::Context<Self>) {
        if self.song_workshop.is_none() {
            self.agent_attach_files(paths, cx);
            return;
        }
        if self.music_composer_locked() {
            return;
        }
        for path in paths {
            if auris_agent::attachment_kind(path).is_none() {
                self.music_references_mut().attachment_error = Some(
                    self.t(Key::AgentAttachmentUnsupported)
                        .replace("{name}", &path.to_string_lossy()),
                );
                continue;
            }
            if self.music_references().attachments.contains(path) {
                self.music_references_mut()
                    .attached_here
                    .insert(path.clone());
                self.music_references_mut().attachment_error = None;
                continue;
            }
            if !self.can_attach_agent_music() {
                self.music_references_mut().attachment_error =
                    Some(self.t(Key::AgentAttachmentLimit).into());
                break;
            }
            self.music_references_mut().attachments.push(path.clone());
            self.music_references_mut()
                .attached_here
                .insert(path.clone());
        }
        cx.notify();
    }

    pub(crate) fn music_owner(&self) -> MusicOwner {
        self.song_workshop.as_ref().map_or_else(
            || {
                MusicOwner::Agent(
                    self.session.path().map(Path::to_path_buf),
                    self.agent_chat.conversations.active_id().map(str::to_owned),
                )
            },
            |workshop| MusicOwner::Workshop(workshop.identity),
        )
    }

    pub(crate) fn music_references(&self) -> &MusicReferences {
        self.song_workshop
            .as_ref()
            .map_or(&self.agent_chat.references, |s| &s.references)
    }

    pub(crate) fn music_references_mut(&mut self) -> &mut MusicReferences {
        self.song_workshop
            .as_mut()
            .map_or(&mut self.agent_chat.references, |s| &mut s.references)
    }

    pub(crate) fn music_composer_locked(&self) -> bool {
        self.song_workshop.as_ref().map_or_else(
            || {
                self.agent_chat.pending_send.is_some()
                    || self.agent_chat.conversations.changing_conversation()
                    || self.agent_chat.conversations.previewing()
            },
            |s| s.busy(),
        )
    }

    pub(crate) fn can_attach_agent_music(&self) -> bool {
        !self.music_composer_locked()
            && self.music_references().attachments.len() < auris_agent::ATTACHMENT_COUNT_LIMIT
    }

    pub(crate) fn attach_agent_music_reference(
        &mut self,
        value: &serde_json::Value,
        label: &str,
    ) -> Result<(), String> {
        if !self.can_attach_agent_music() {
            return Err(self
                .t(
                    if self.music_references().attachments.len()
                        >= auris_agent::ATTACHMENT_COUNT_LIMIT
                    {
                        Key::AgentAttachmentLimit
                    } else {
                        Key::AgentMusicUnavailable
                    },
                )
                .to_owned());
        }
        let directory = auris_session::settings::config_dir().join("conversation-midi");
        let path = auris_session::music_reference::save_snapshot(value, &directory, label)?;
        self.music_references_mut()
            .attached_here
            .insert(path.clone());
        self.music_references_mut().attachments.push(path);
        self.music_references_mut().attachment_error = None;
        Ok(())
    }

    pub(crate) fn open_agent_chords(&mut self) {
        if !self.can_attach_agent_music() {
            return;
        }
        let key = self.session.harmony().key_at(Ticks::ZERO);
        let meter = self.project().signatures.signature_at(Ticks::ZERO);
        self.open_prompt(Prompt::new(
            format!(
                "{} — {} · {}/{}",
                self.t(Key::AgentChords),
                key.to_text(),
                meter.numerator,
                meter.denominator
            ),
            PromptTarget::AgentChordReference { key, meter },
            "",
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::harness::{click, open, paint};

    #[gpui::test]
    fn chord_prompt_attaches_timed_reference_without_editing_song(cx: &mut gpui::TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
        });
        paint(&app, cx);
        let before = app.read_with(cx, |this, _| this.project().clone());
        assert!(cx.debug_bounds("agent-midi-new").is_none());
        assert!(cx.debug_bounds("agent-chords").is_none());
        click("music-reference-menu", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let menu = this.menu.as_ref().unwrap();
            assert_eq!(menu.entries.len(), 5);
            assert!(matches!(&menu.entries[1], crate::ui::context_menu::MenuEntry::Item(item) if !item.enabled));
        });
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(this.menu.is_none()));
        click("music-reference-menu", cx);
        // Selected-clip attachment is disabled; Down skips it before reaching chords.
        cx.simulate_keystrokes("down down enter");
        paint(&app, cx);
        cx.simulate_input("| C/E G7 | vi | F |");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_none());
            assert_eq!(this.project(), &before);
            let bytes = std::fs::read(&this.agent_chat.references.attachments[0]).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["chords"][0]["chord"], "C/E");
            assert_eq!(value["chords"][2]["start_ticks"], 3840);
        });
        paint(&app, cx);
        click("music-reference-menu", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &crate::ui::context_menu::MenuCommand::ComposerChords,
        );
        paint(&app, cx);
        cx.simulate_input("| nonsense |");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            assert!(this.prompt.is_some());
            assert_eq!(this.agent_chat.references.attachments.len(), 1);
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn sound_choice_snapshots_keep_exact_preset_identity(cx: &mut gpui::TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let before = this.project().clone();
            let sound = auris_session::TimbreSound {
                name: "Warm piano".into(),
                instrument_id: "sampler".into(),
                preset: Some(auris_session::prelude::PresetRef {
                    font: auris_session::prelude::SoundFontId(17),
                    bank: 3,
                    patch: 8,
                }),
            };
            let value = auris_session::music_reference::sound_reference(&this.session, &sound);
            this.attach_agent_music_reference(&value, "sound-piano")
                .unwrap();
            let bytes = std::fs::read(&this.agent_chat.references.attachments[0]).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["preset"]["bank"], 3);
            assert_eq!(value["preset"]["patch"], 8);
            assert_eq!(value["name"], "Warm piano");
            assert_eq!(this.project(), &before);
        });
    }
}
