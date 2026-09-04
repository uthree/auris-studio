# Review findings: auris-gpui

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 72 verified findings: 1 critical, 19 high, 31 medium, 21 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-001 | critical | `crates/auris-gpui/src/ui/compose_sheet/lyrics.rs:32` | LyricsEdit stores a raw `dials.sections` index that any form edit (add/remove/move/retarget) silently reindexes via tidy_sections, silently dropping or […] |
| F-016 | high | `crates/auris-gpui/src/ui/compose_sheet/dials.rs:501` | Renaming or removing a part on the compose sheet leaves stale names in section.parts, so Write silently drops that part from any section that named it, with no […] |
| F-021 | high | `crates/auris-gpui/src/ui/agent_chat.rs:747` | Stale agent 'Reload' button keeps a discarded project's path and can silently replace a different open project's unsaved edits. |
| F-027 | high | `crates/auris-gpui/src/ui/piano_roll.rs:1033` | Setting Delete=Double-click is unreachable on singer clips because begin_note_drag's lyric-prompt branch unconditionally returns before the delete check. |
| F-034 | high | `crates/auris-gpui/src/ui/mixer.rs:171` | Right-click on any mixer strip or fader always shows Add Track instead of the track/param menu, since the outer mixer div's handler overwrites the inner one […] |
| F-043 | high | `crates/auris-gpui/src/settings_window.rs:244` | Settings window's apply_audio sets self.audio before the apply succeeds and never rolls it back on Err, so the UI shows a rejected audio preference as active. |
| F-047 | high | `crates/auris-gpui/src/settings_window.rs:1024` | Keys-tab section headings for the same group render twice, non-adjacently, because BINDABLE's declaration order isn't grouped as found_commands assumes. |
| F-067 | high | `crates/auris-gpui/src/menu.rs:652` | macOS native Edit/View/Transport menus never disable Undo/Redo or check toggles, because menus() is built once from MenuState::default() and gpui's […] |
| F-069 | high | `crates/auris-gpui/src/ui/agent_chat.rs:846` | render_agent_chat re-runs load_preferences() every repaint while unconfigured, wiping the provider/URL/API-key-env fields on every keystroke or dropdown pick […] |
| F-070 | high | `crates/auris-gpui/src/ui/context_menu/menu.rs:329` | ContextMenu::origin's fallback clamp can place the menu directly over the anchor point in narrow viewports, contradicting its own doc comment's purpose. |
| F-071 | high | `crates/auris-gpui/src/ui/context_menu/command.rs:956` | Most clip-context-menu rows (e.g. ToggleClipMute, SplitClipAtPlayhead) act only on the right-clicked clip, ignoring the rest of a multi-clip selection the […] |
| F-072 | high | `crates/auris-gpui/src/ui/context_menu/tracks.rs:568` | Mixer's Add-Send "+" button silently does nothing in any project with zero bus tracks, since the empty menu it builds is dropped by open_menu with no feedback. |
| F-079 | high | `crates/auris-gpui/src/app.rs:1721` | A plain click on an overlapping unfaded clip unintentionally writes a crossfade and an undo step via end_drag's ungated ClipMove branch (app.rs:1721). |
| F-080 | high | `crates/auris-gpui/src/ui/piano_roll.rs:2231` | press_curve_lane hit-tests curve points against the snapped click tick, not the raw press position, so off-grid points become unclickable under coarse […] |
| F-081 | high | `crates/auris-gpui/src/ui/arrangement/geometry.rs:207` | fade_handle_at ignores loop passes, so a phantom fade-out grab hijacks resize on looped clips. |
| F-085 | high | `crates/auris-gpui/src/ui/piano_roll.rs:1146` | Piano-roll note creation snaps the clip-relative tick instead of the absolute one, so new notes miss the drawn grid whenever clip_start isn't grid-aligned. |
| F-094 | high | `crates/auris-gpui/src/ui/text_area.rs:199` | Lyrics/prompt text areas hard-clip past max_rows with no vertical scroll, hiding text and caret once content exceeds 12 lines. |
| F-095 | high | `crates/auris-gpui/src/ui/context_menu/clips.rs:295` | Note context menu titled "N notes" still applies ornament/lyric rows to only the single note under the pointer, silently dropping the rest of the selection. |
| F-110 | high | `crates/auris-gpui/src/settings_window.rs:1308` | Resetting a command's keybinding while a capture is armed leaves the capture live, so the next keystroke silently rebinds the just-reset command. |
| F-113 | high | `crates/auris-gpui/src/ui/library.rs:857` | Plugin-open state keyed by scan-list index (not file identity) lets adding/removing a plugin folder auto-load an unrelated .clap binary with no user gesture. |
| F-038 | medium | `crates/auris-gpui/src/ui/commands.rs:327` | create_clip_at names new clips from the project's track count instead of a clip count, so repeated clip creation on a track yields duplicate names like "Clip […] |
| F-044 | medium | `crates/auris-gpui/src/ui/prompt.rs:674` | Empty ClipSourceTempo field is rejected by commit_prompt's generic empty-check before it can reach the arm meant to clear the tempo to None. |
| F-045 | medium | `crates/auris-gpui/src/ui/prompt.rs:993` | commit_prompt's empty_clears guard omits ClipSourceTempo, so clearing a clip's source tempo via the prompt can never run. |
| F-054 | medium | `crates/auris-gpui/src/settings_window.rs:255` | Settings window mislabels every audio-preference error (mainly "recording in progress") as an "audio restart failed" and leaks raw English text instead of the […] |
| F-068 | medium | `crates/auris-gpui/src/ui/arrangement/headers.rs:109` | Header column reads self.lane_scroll before render_timeline clamps it, causing a one-frame header/lane misalignment right after a track deletion overflows the […] |
| F-082 | medium | `crates/auris-gpui/src/ui/context_menu/clips.rs:60` | Clip context menu titled "N clips" applies most rows (mute, gain, crossfade, fades, tempo, edit, accompany, motif) to only the single right-clicked clip, not […] |
| F-131 | medium | `crates/auris-gpui/src/ui/plugin_window.rs:219` | Closing an EQ's plugin window skips stop_watching(), so the audio thread keeps publishing that strip's spectrum every block until some other plugin window is […] |
| F-134 | medium | `crates/auris-gpui/src/i18n.rs:240` | NoSuchSpeaker renders its whole English thiserror sentence untranslated in the Japanese UI, unlike every comparable local error variant. |
| F-138 | medium | `crates/auris-gpui/src/keymap.rs:164` | discard_unusable() never re-checks survivors against defaults, so a filtered override that matches the default is kept and re-persisted as if customized. |
| F-139 | medium | `crates/auris-gpui/src/ui/transport_bar.rs:927` | toggle_monitoring hard-resets monitor_gaps to 0 even when the shared Capture device stays open, causing report_monitor_gaps to re-announce the stale cumulative […] |
| F-142 | medium | `crates/auris-gpui/src/ui/piano_roll.rs:115` | note_end_span's doc/test comment falsely claim None for notes <3px wide; the code only returns None at width <= 0. |
| F-151 | medium | `crates/auris-gpui/src/ui/arrangement/mod.rs:6` | mod.rs's doc comment falsely claims all arrangement tests live in geometry.rs, when headers.rs and gestures.rs each carry their own test modules. |
| F-156 | medium | `crates/auris-gpui/src/ui/text_field.rs:320` | apply_key returns KeyEffect::Changed for Backspace/Delete even when the caret is at a boundary and nothing was deleted, wrongly resetting palette selection / […] |
| F-169 | medium | `crates/auris-gpui/src/app.rs:1807` | select_clips's doc says primary joins the clip selection when absent from it, but the code discards primary in exactly that case instead of inserting it. |
| F-171 | medium | `crates/auris-gpui/src/ui/prompt.rs:1852` | every_target()'s doc comment falsely claims exhaustiveness is enforced; it's a plain Vec literal already missing 6 of 26 PromptTarget variants. |
| F-174 | medium | `crates/auris-gpui/src/ui/transport_bar.rs:1319` | Test named for cycling the grid only asserts a static fact about GRID_CHOICES and never calls cycle_grid or grid_label. |
| F-189 | medium | `crates/auris-gpui/src/settings_window.rs:922` | Re-clicking the already-selected output device row zeroes sample_rate and forces an unwanted audio restart. |
| F-190 | medium | `crates/auris-gpui/src/settings_window.rs:65` | An open Settings window keeps showing the old language/colour scheme when it is changed elsewhere (e.g. the command palette), until closed and reopened. |
| F-191 | medium | `crates/auris-gpui/src/ui/compose_sheet/dials.rs:244` | song_dials rebuilds SongDials::charts via BTreeMap::iter(), so reopening a project resorts extra charts alphabetically instead of preserving the order they […] |
| F-199 | medium | `crates/auris-gpui/src/gestures.rs:117` | Holding both the create and delete modifiers on empty piano-roll grid always creates a note because CommandClick/OptionClick::matches ignore each other's flag. |
| F-203 | medium | `crates/auris-gpui/src/main.rs:197` | opening_window_bounds checks only cx.primary_display(), so a window remembered on a secondary monitor is always recentred instead of restored, even though […] |
| F-204 | medium | `crates/auris-gpui/src/gestures.rs:433` | gestures.rs:433 uses `#[cfg(not(target_os = "macos"))]` instead of `cfg!`, so the non-macOS modifier assertions never compile on a macOS `cargo test` run, […] |
| F-211 | medium | `crates/auris-gpui/src/app.rs:2049` | selected_phonemes doc-comments "the grabbed note" but reads the lowest-indexed note in a BTreeSet, mismatching pitch and lyric during multi-note shift-click […] |
| F-212 | medium | `crates/auris-gpui/src/ui/compose_sheet/dials.rs:21` | TEMPO dial doc claims to cover the spec's accepted tempo range (20..400) but only covers 40..220, silently clamping/discarding legal out-of-range tempos on […] |
| F-224 | medium | `crates/auris-gpui/src/ui/piano_roll.rs:1381` | Phoneme-divider click acceptance (grabbed_boundary_at, piano_roll.rs:1381) uses a full PHONEME_GRAB radius while the drawn cursor hitbox […] |
| F-232 | medium | `crates/auris-gpui/src/ui/arrangement/geometry.rs:257` | A press in the fade band beyond ~6px of the actual (moved) fade handle silently resizes the clip with no hover cursor ever shown there. |
| F-233 | medium | `crates/auris-gpui/src/ui/agent_chat.rs:939` | Agent transcript panel never auto-scrolls on new entries, so users must manually scroll down after every turn to see the latest message. |
| F-246 | medium | `crates/auris-gpui/src/ui/agent_chat.rs:271` | AgentChat::entries (agent_chat.rs:271) is never capped or evicted and AgentEvent::Result rescans it with iter_mut().rev().find(), so a long or misbehaving […] |
| F-247 | medium | `crates/auris-gpui/src/ui/agent_chat.rs:448` | A tool-result event with no matching open call is pushed with a raw empty `line`, so chat_row's `line.is_empty()` check renders it as permanently "running" […] |
| F-251 | medium | `crates/auris-gpui/src/ui/context_menu/menu.rs:302` | ContextMenu::size() counts CJK characters at their half-width Latin cost, so the widest Japanese menu row is silently truncated by the label's .truncate(), […] |
| F-258 | medium | `crates/auris-gpui/src/ui/agent_chat.rs:501` | agent_binary() falls back to an unqualified filename when current_exe() fails, letting Command::new() resolve the agent binary via CWD/PATH search instead of […] |
| F-129 | low | `crates/auris-gpui/src/ui/commands.rs:1413` | export_singer_frames's doc comment opens with export_midi's description, leaving export_midi undocumented and cargo doc showing a false claim. |
| F-137 | low | `crates/auris-gpui/src/app.rs:2108` | Resolving an external-change conflict via Save shows "Saved to …" then blanks it within 500ms via watch_disk's unconditional Withdraw status clear […] |
| F-155 | low | `crates/auris-gpui/src/actions.rs:55` | StopPlayback action is unreachable (no keymap/menu/palette row) and its doc comment claims a return-to-start seek that Session::stop never performs. |
| F-170 | low | `crates/auris-gpui/src/ui/prompt.rs:849` | Duplicate part/progression names are rejected with the misleading "Name cannot be empty" message instead of a collision-specific one. |
| F-173 | low | `crates/auris-gpui/src/ui/transport_bar.rs:719` | Time-signature scroll handler rounds each event's delta independently with no carried remainder, so precise-scroll (trackpad) input under 8px/event never […] |
| F-213 | low | `crates/auris-gpui/src/ui/context_menu/timeline.rs:75` | Clearing the loop leaves loop_region as Some((0,0)) instead of None, so the menu still offers Punch From Cycle and arms a zero-length punch region. |
| F-245 | low | `crates/auris-gpui/src/ui/widgets.rs:873` | db_to_meter_position collapses +Infinity dB to silence (0.0) instead of saturating to 1.0 like other above-range values, but no live UI path can currently feed […] |
| F-248 | low | `crates/auris-gpui/src/ui/piano_roll.rs:170` | grabbed_phoneme_boundary picks the first in-slack boundary via .find() instead of the nearest via .min_by_key, unlike the sibling curve_point_at. |
| F-250 | low | `crates/auris-gpui/src/ui/commands.rs:1072` | sing_track/begin_export/start_export_stems don't check self.auto_sing, so a manual render can briefly run concurrently with (or silently stall behind) an […] |
| F-264 | low | `crates/auris-gpui/src/dock.rs:122` | Panel::command's doc comment says "all five" panels but Panel::ALL has held six since the Agent panel shipped. |
| F-269 | low | `crates/auris-gpui/src/menu.rs:949` | Test doc comment in menu.rs wrongly claims Open Recent/About have an empty binding id; it's actually their default keystroke that's empty. |
| F-270 | low | `crates/auris-gpui/src/ui/root.rs:1150` | Stale "Last, because..." comment sits above library_search_key though agent_key was appended after it as the true last disjunct in root.rs's on_key_down chain. |
| F-274 | low | `crates/auris-gpui/src/ui/mod.rs:3` | crates/auris-gpui/src/ui/mod.rs:3 claims every ui submodule only extends AurisApp, but tooltip.rs defines its own Render entity (Tooltip), contradicting the […] |
| F-278 | low | `crates/auris-gpui/src/dock.rs:470` | side_widths' zero-total guard at dock.rs:470 is unreachable dead code, since room's unconditional >=0 clamp forces total>0 whenever control reaches it. |
| F-283 | low | `crates/auris-gpui/src/ui/commands.rs:440` | Loop toggle on a mixed clip selection can show "Clip looped" even when most clips end up unlooped, because the status is chosen by looped > 0 rather than by […] |
| F-287 | low | `crates/auris-gpui/src/ui/plugin_window.rs:381` | Plugin window header drag lacks the pressed_at wobble guard every other pixel-based drag (ClipMove, TrackReorder, NoteMove, NoteResize) has, so a click on its […] |
| F-291 | low | `crates/auris-gpui/src/gestures.rs:122` | Shift+double-click on a singer-clip note opens the lyric prompt and silently clears the multi-selection instead of extending it. |
| F-300 | low | `crates/auris-gpui/src/ui/text_field.rs:490` | text_for_range/selected_text_range use the mutating field accessor, silently resetting an in-progress Tab-completion walk on a read-only IME query. |
| F-301 | low | `crates/auris-gpui/src/ui/plugin_window.rs:399` | Instrument plugin window's bypass button always shows "on" and is inert — InstrumentTrack has no enabled field to toggle. |
| F-306 | low | `crates/auris-gpui/src/ui/typing_panel.rs:607` | press_typed_key's audition_track guard returns before release_typed_key runs, so deleting the last instrument track mid-drag can leave a note stuck sounding. |
| F-307 | low | `crates/auris-gpui/src/ui/context_menu/menu.rs:138` | ContextMenu::step's comment says the stale-highlight fallback lands "just before the next row" (position -1) but the code hardcodes position 0, skipping the […] |

### F-001 · critical · LyricsEdit stores a raw `dials.sections` index that any form edit (add/remove/move/retarget) silently reindexes via tidy_sections, silently dropping or misdirecting typed lyrics into the wrong section.

`crates/auris-gpui/src/ui/compose_sheet/lyrics.rs:32` · correctness · confirmed (executed reproduction; reported independently 3×)

**What a user sees.** While a lyrics box is open for editing, clicking any form-editing button (remove, reorder, retarget a form entry, or add a new section before the one being edited) silently reindexes `dials.sections` under the open editor. Typed words then either vanish entirely (written to an index that no longer exists, dropped with no error) or get written into a different, wrong section — silently overwriting that section's previously-saved lyrics with no error, confirmation, or visible warning beyond an easy-to-miss heading change.

**Trigger.** Open the song sheet on its default song (sections = [intro, verse, chorus, outro], form = [intro, verse, chorus, verse, chorus, outro]). Click the outro lyrics box (`focus_section_lyrics(3)`), type some words (each keystroke calls `sync_section_lyrics`, correctly writing to `dials.sections[3]`). Then click the '✕' remove button on outro's own form row (its only placement) — `remove_from_form(dials, 5)` removes outro from the form and `tidy_sections` rebuilds `dials.sections` down to 3 entries (indices 0-2); `lyrics_edit.section` stays `3`. The lyrics box still holds keyboard focus (`taking_text_input()` is true because `lyrics_edit.is_some()`), so further typing keeps being accepted by […]

**Mechanism.** `LyricsEdit { section: usize, .. }` (lyrics.rs:29-35) is set once by `focus_section_lyrics` (lyrics.rs:60-76) and held across every subsequent keystroke; `sync_section_lyrics` (lyrics.rs:83-95) writes into `dials.sections.get_mut(edit.section)` on every change, and `lyrics_box`'s render (lyrics.rs:206-214, `edit.section == index`) decides which on-screen box shows the live editor by comparing this same raw index. But `dials.sections` is not a stable-by-position list: `tidy_sections` (dials.rs:385-396), called from `add_to_form`, `set_form_entry`, `remove_from_form` and `move_in_form` (dials.rs:318-372) after every form edit, rebuilds it from scratch by walking `dials.form` and keeping only sections still played, in first-occurrence order. Any of those four form operations that changes which name is 'first in the form' before the edited section, or drops the edited section's only placement, changes or removes its position in the rebuilt list — `LyricsEdit.section` is never updated to follow it. None of the form row buttons (view.rs:552-591, the ↑/↓/✕ handlers) clear or re-point […]

**Expected.** Per tidy_sections's own doc comment, anything that must survive a form edit has to reach into `SongDials::sections` by name, not by position — the way `section_at`/`sections_in_form_order` do on every render. `LyricsEdit` should track the section by name (or be invalidated/reattached whenever the form-editing operations in dials.rs change the section list), so an open lyrics edit always stays pointed at the section the user opened it for, and never silently redirects to — or vanishes past the […]

**Fix direction.** Change `LyricsEdit` to identify its section by name (matching how `tidy_sections`, `section_at`, and `sections_in_form_order` already address sections) instead of by raw `usize` position, or invalidate/reattach `lyrics_edit` from within `tidy_sections` (or its four callers) whenever the rebuilt list changes the edited section's position or removes it. The name-based fix is smallest: store `section: String` (or resolve the index fresh via name lookup) in `LyricsEdit`, and have `sync_section_lyrics`/`focus_section_lyrics` look up `dials.sections` by name each time.

**Written rule it breaks.** tidy_sections's own doc comment: "Reordering costs nothing on screen: the form column is drawn from `SongDials::form`, and this list is storage its rows reach into by name." (dials.rs, tidy_sections doc) — LyricsEdit reaches into dials.sections by raw index, not by name, which is exactly the case this comment says is unsafe.

### F-016 · high · Renaming or removing a part on the compose sheet leaves stale names in section.parts, so Write silently drops that part from any section that named it, with no error shown.

`crates/auris-gpui/src/ui/compose_sheet/dials.rs:501` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** After renaming a part on the compose sheet and pressing Write, sections that previously included that part silently stop playing it, with no error, warning, or visual indicator — the section's roster button still shows a count like "5/6" as if nothing changed, but the renamed part's notes are gone from the generated song. After removing a part, an orphaned stale name lingers harmlessly in section.parts (it never matches any current part again), leaving the SongSpec permanently inconsistent with the visible roster even though playback is unaffected in that case.

**Trigger.** On the default sheet: toggle off any one part in some section's roster (e.g. click the section's parts button, untick `hat`) so that section's `parts` list becomes an explicit subset that still names `bass`; then remove the `bass` part from the roster with the 'song-part-remove' button (view.rs:762-779); click 'Write' (or 'Save Song Specification'). Equivalently, rename `bass` via the part-name prompt instead of removing it — the stale `"bass"` string is left in the section's roster either way.

**Mechanism.** `toggle_part_in_section` (dials.rs:568-606) can leave a section's `parts` list holding an explicit, non-empty subset of part *names* (e.g. every part but `hat`). `remove_part` (dials.rs:501-507) only does `dials.parts.remove(index); true` — it never walks `dials.sections` to strip the removed part's name out of any section's `parts` (or `tweaks`) list. The same gap exists for a rename: `PromptTarget::SongPartName` (crates/auris-gpui/src/ui/prompt.rs:840-849) does `part.name = text;` and nothing else, so a stale copy of the *old* name is left behind in any section roster that named it. `song_spec()` (dials.rs:180-205) copies `section.parts` verbatim into the `SectionSpec`, and `SectionDoc::from_spec` (crates/auris-compose/src/spec/doc.rs:934) writes it verbatim to TOML with no cross-check against `spec.parts`. But `SpecDoc::into_spec` (doc.rs:619-638) explicitly rejects any section that names a part not present in `spec.parts` (`section \`{name}\` names the part \`{part}\`, which does not exist`), confirmed by the parser's own test `SongSpec::parse("form = […]

**Expected.** Per the module doc ('there is no second implementation of what a dial means, and the round trip through `SongSpec::to_toml()` is a test they share') and the `no_dial_can_be_turned_to_a_value_the_format_refuses` test's own premise, every mutation the sheet allows must keep `song_spec(&dials)` parseable. `remove_part` and the part-rename path must strip (or rename) the part's name out of every section's `parts` list and `tweaks` map when the part is removed or renamed, the same way […]

**Fix direction.** Have remove_part and the PromptTarget::SongPartName rename handler walk dials.sections (parts list and tweaks map) and either drop the removed part's name or rewrite the old name to the new one everywhere it appears, the same invariant toggle_part_in_section already maintains. Add a round-trip test through song_spec that asserts no section names a part absent from dials.parts after a rename or removal.

**Written rule it breaks.** remove_part's own doc comment: "A song with no parts writes no notes, and a sheet whose Write button produces an empty document is a sheet with a broken state reachable from it." — the crate already treats a broken state reachable via the Write button as something to guard against, which this gap fails to do for rename/remove.

### F-021 · high · Stale agent 'Reload' button keeps a discarded project's path and can silently replace a different open project's unsaved edits.

`crates/auris-gpui/src/ui/agent_chat.rs:747` · lifecycle · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If the agent rewrites project A while it's open and dirty, the chat panel shows a 'Reload' offer tied to A. If the user then switches to a different project B (via File > Open, a dropped file, etc.) without ever clicking that stale Reload button, the button keeps rendering and pending_reload keeps holding A's path. If the user later clicks it, agent_reload unconditionally calls open_project_at(A, cx) with no confirm_discard and no re-check that A is still open — B is silently replaced by a reload of the unrelated stale project A, discarding whatever edits B had.

**Trigger.** 1) Project A is open with unsaved edits; the agent (asked earlier) rewrites A on disk, so `absorb` sets `pending_reload = Some(A)` and shows the 'Reload' note/button. 2) Without clicking Reload, the user opens Project B through File > Open (which does go through `confirm_discard`, so A's edits are handled once) — `open_project_at(B, ...)` runs, but does not touch `agent_chat.pending_reload`, which still holds `Some(A)`. 3) The agent panel still renders the reload button (`pending_reload.is_some()` at line 871/905 is still true). 4) The user makes real edits to B (now dirty) and, out of habit or confusion, clicks the still-visible 'Reload' button.

**Mechanism.** `AgentChat::absorb`'s `Changed` arm (lines 456-467) sets `self.pending_reload = Some(project)` at line 461 only when `dirty` is true for the *currently open* file (`same_file(&project, open)` checked at line 458). Nothing ever clears `pending_reload` afterward except `agent_reload` itself taking it (line 747: `if let Some(path) = self.agent_chat.pending_reload.take() { ... self.open_project_at(path, cx); }`). `grep -rn "pending_reload"` over crates/auris-gpui/src/ shows exactly those two touch points (set at 461, taken at 747) plus the render-only read at line 871/905 for the button — nothing in `open_project_at`, `reset_view`, `open_project`, `pick_and_open_project`, `agent_apply_settings` or `agent_write_through` invalidates it. `AgentChat` state is never reset when the open document changes by any means other than the agent's own reload. `open_project_at`'s own doc comment (crates/auris-gpui/src/ui/commands.rs:706-710) says it runs 'with the document already dealt with' — i.e. it deliberately performs no dirty check or confirmation, trusting the caller to have handled that; […]

**Expected.** Per the module's own contract ('the window saves before every message... and when an event says the agent wrote the project back, the window reloads it, automatically while it has nothing unsaved and by an offered button when it does' — crates/auris-gpui/src/ui/agent_chat.rs:9-12), the offered button should only ever act on the project it was raised for, and only while that project is still the one open. Switching to a different document (by any path) should clear or invalidate a pending reload […]

**Fix direction.** Clear self.agent_chat.pending_reload whenever the open document changes by any path other than agent_reload itself (e.g. at the top of open_project_at, or in reset_view), and/or have agent_reload re-validate that pending_reload's path still equals self.session.path() before calling open_project_at, routing through confirm_discard otherwise.

**Written rule it breaks.** the window saves before every message... and when an event says the agent wrote the project back, the window reloads it, automatically while it has nothing unsaved and by an offered button when it does (crates/auris-gpui/src/ui/agent_chat.rs:9-12)

### F-027 · high · Setting Delete=Double-click is unreachable on singer clips because begin_note_drag's lyric-prompt branch unconditionally returns before the delete check.

`crates/auris-gpui/src/ui/piano_roll.rs:1033` · ui · confirmed (traced through the code; reported independently 3×)

**What a user sees.** A user who sets Delete = Double-click in Settings (a configuration the app explicitly offers and documents as intentionally supported) finds it silently does nothing on singer-track clips: double-clicking a note there always opens the lyric-edit prompt instead of deleting the note, with no way to invoke the configured delete gesture on that track type.

**Trigger.** In Settings, set the Delete gesture to Double-click (a choice the picker offers). Open a singer (vocal) track's clip in the piano roll and double-click an existing note, intending to delete it.

**Mechanism.** `begin_note_drag` checks, unconditionally on the user's configured gestures: `if let Some(index) = under_pointer && crate::gestures::PointerGesture::DoubleClick.matches(event) && self.editing_a_singer_clip() { self.open_lyric_prompt(clip_id, index); cx.notify(); return; }` (lines 1032-1039). This runs before the configurable delete check at lines 1062-1069 (`if let Some(index) = under_pointer && self.pointer.delete.matches(event) { ... remove_notes ... }`) and does not consult `self.pointer.delete` at all. `settings_window.rs`'s `pointer-delete` row explicitly offers `PointerGesture::DoubleClick` (filtered only by `PointerGesture::may_delete`, which is true for it), so assigning Delete = Double-click is a supported, reachable configuration — the test `nothing_destructive_is_bound_to_a_double_click_by_default` in gestures.rs only asserts this isn't the *default*, not that it's disallowed.

**Expected.** The hard-coded lyric-prompt branch should not pre-empt whichever gesture the user has actually assigned to Delete. It should either check `self.pointer.delete != PointerGesture::DoubleClick` before intercepting the double-click, or otherwise fall through to the delete branch when Double-click is configured as delete, so a note under the pointer is removed like it is for every other clip type.

**Fix direction.** Guard the lyric-prompt branch in begin_note_drag (piano_roll.rs:1032-1039) with a check that the matched gesture is not the user's configured delete gesture, e.g. add `&& !self.pointer.delete.matches(event)` (or equivalently check `self.pointer.delete != PointerGesture::DoubleClick`) before opening the lyric prompt, so delete still wins when so configured, matching the comment at line 1061 ("Delete first: it is the only gesture that acts on what is already there").

**Written rule it breaks.** Both remain configurable, so anyone who wants the old arrangement can say so. (PointerGestures::default doc comment, gestures.rs)

### F-034 · high · Right-click on any mixer strip or fader always shows Add Track instead of the track/param menu, since the outer mixer div's handler overwrites the inner one with no stop_propagation.

`crates/auris-gpui/src/ui/mixer.rs:171` · ui · confirmed (traced through the code; reported independently 2×)

**What a user sees.** Right-clicking any mixer channel strip, or any gain/pan/send fader inside a strip, never opens the track menu or param menu the user expects — it always shows the "Add Track" arrangement menu instead, because the outer mixer container's handler runs after (and unconditionally overwrites) the strip's or fader's own handler. Track-level actions reachable only via right-click (e.g. removing/renaming a track from the mixer, or resetting a fader) are effectively unreachable from the mixer panel.

**Trigger.** Right-click anywhere on a channel strip's own surface that is not an effect insert row or a send row -- e.g. directly on the gain fader, the pan fader, the mute/solo row, the track name, or empty strip background.

**Mechanism.** render_strip() registers `.on_mouse_down(gpui::MouseButton::Right, Self::opens_menu(cx, move |this, at| { this.select_track(track_id); this.track_menu(at, track_id) }))` (lines 169-175) on the strip div, and every fader on that strip (via `self.fader(...)` in inspector.rs:563-566) registers its own `.on_mouse_down(Right, Self::opens_menu(cx, ...this.param_menu...))`. The strip is a descendant of the mixer's own scroll wrapper, which registers `.on_mouse_down(gpui::MouseButton::Right, Self::opens_menu(cx, |this, at| this.arrangement_menu(at)))` at mixer.rs:77-81. gpui's `on_mouse_down` is a Bubble-phase listener gated only by `event.button == button && hitbox.is_hovered(window)` (gpui-0.2.2 src/elements/div.rs:121-134), and 'Event handlers propagate events by default' (gpui-0.2.2 src/app.rs:1708-1714) -- a listener must call `cx.stop_propagation()` to stop the bubble. `Self::opens_menu` (context_menu/menu.rs:387-407) never calls `stop_propagation()`, so a Right mouse-down that lands on a fader or the strip body fires the fader's `param_menu`, then the strip's `track_menu` (each […]

**Expected.** Per the code's own comment at mixer.rs:79 ('Right-clicking past the last strip is still a request to add a track'), the arrangement menu is meant to fire only when the click lands on the panel's own empty space, not when it lands on a strip's interactive controls; the strip/fader's own menu should win there, the way it correctly does for effect_row and send_row, which call `cx.stop_propagation()` after opening their menu.

**Fix direction.** Have `Self::opens_menu`'s handler call `cx.stop_propagation()` after successfully opening a non-empty menu (or have `open_menu` report whether it set the menu, and stop propagation at each call site only when it did), so a strip's or fader's own right-click handler consumes the event before it bubbles to the mixer container's arrangement-menu handler.

### F-043 · high · Settings window's apply_audio sets self.audio before the apply succeeds and never rolls it back on Err, so the UI shows a rejected audio preference as active.

`crates/auris-gpui/src/settings_window.rs:244` · ui · confirmed (traced through the code; reported independently 2×)

**What a user sees.** After an audio-preference change is rejected (e.g. clicking a device/rate/buffer-size control while a take is recording, or when device restart fails), the Settings window highlights the rejected choice as selected and shows an error status simultaneously — the UI and the real running audio backend now disagree, with no way to see the true state short of closing and reopening the window. Every further control clicked in that session keeps resubmitting the same stale/rejected preferences via `..this.audio.clone()`.

**Trigger.** Start a take (recording) so `Session.take.is_some()`, open the Settings window (nothing gates this), go to the Audio tab, and click any device row, sample-rate button, or buffer-size button. `Session::set_audio_preferences` immediately returns `Err(SessionError::RecordingInProgress)` for *any* audio-preference change while a take is in progress (crates/auris-session/src/session/mod.rs:619-621), independent of which device is picked — so this is 100% reproducible, not dependent on flaky hardware. (A disconnected/exclusive-locked output device — plausible given `AudioDevices` is explicitly documented as a stale snapshot taken once when the window opened — reaches the same `apply_audio` code […]

**Mechanism.** `apply_audio` (lines 243-264) does `self.audio = audio.clone();` unconditionally, *before* calling `app.apply_audio_preferences(audio)` and learning whether the change actually took effect: ```
fn apply_audio(&mut self, audio: AudioPreferences, cx: &mut Context<Self>) {
    self.audio = audio.clone();
    let outcome = self
        .app
        .update(cx, |app, _| app.apply_audio_preferences(audio))
        .unwrap_or_else(|_| Err("the main window has closed".to_string()));
    self.status = match outcome {
        Ok(status) => status,
        Err(error) => { crate::i18n::error_text(&SessionError::AudioRestart(error), self.language) }
    };
```. `app.apply_audio_preferences` (crates/auris-gpui/src/app.rs:2159-2171) forwards to `Session::set_audio_preferences` (crates/auris-session/src/session/mod.rs:615-660), which can fail (`SessionError::RecordingInProgress` when `self.take.is_some()`, or `SessionError::AudioRestart` when `start_audio` errors) and, on that error path, leaves `self.settings.audio`/`self.session.audio_preferences()` at their *old* values (the assignment on […]

**Expected.** Per the app.rs doc comment on `apply_audio_preferences` ("Saving is best-effort: failing to write a preferences file must not undo a device change that already worked") the intent is clearly that a *failed* device change must not be reflected as if it worked — exactly the discipline `apply_japanese_dictionary` in this same file already implements (settings_window.rs:568-585: `self.japanese_dictionary = folder` only inside `Ok(Ok(()))`). `apply_audio` should likewise only set `self.audio = […]

**Fix direction.** In `apply_audio` (settings_window.rs:243-264), move `self.audio = audio.clone()` so it only executes inside the `Ok` arm of the match on `outcome` (mirroring `apply_japanese_dictionary`'s pattern, which only updates local state inside `Ok`), leaving `self.audio` at its previous value on `Err`.

**Written rule it breaks.** Saving is best-effort: failing to write a preferences file must not undo a device change that already worked. (app.rs:2157-2158 doc comment on `apply_audio_preferences`, whose converse — a failed change must not be shown as if it worked — is the property violated here)

### F-047 · high · Keys-tab section headings for the same group render twice, non-adjacently, because BINDABLE's declaration order isn't grouped as found_commands assumes.

`crates/auris-gpui/src/settings_window.rs:1024` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** On the Settings window's Keys tab (the default, unfiltered view — a realistic path every user hits when reviewing or rebinding shortcuts), the same group heading (e.g. "View") is rendered twice, non-adjacently, with unrelated commands from other groups appearing between the two halves. The keybinding list looks broken and makes it hard to find or organize bindings for a given group.

**Trigger.** Open Settings and switch to the Keys tab with an empty search query (the default state) — no special filtering is needed since `found_commands()` returns every command unmodified when the query is empty.

**Mechanism.** `render_keys` walks `found_commands()` in table order and opens a new section heading only `if group != Some(command.group)` (lines 1022-1029): `let mut group: Option<Key> = None; for command in found.iter().copied() { if group != Some(command.group) { group = Some(command.group); rows.push(self.render_group_heading(command.group, cx)); } rows.push(self.render_key_row(command, cx)); }`. `found_commands`'s own doc comment (lines 996-999) states the precondition this depends on: "Filtered but *not* reordered: this list is arranged under section headings, and sorting by score would scatter the sections" — i.e. it assumes `BINDABLE` already groups every command of the same `Key::Group` contiguously. That assumption is false: `crates/auris-gpui/src/actions.rs` lines 433-435 read `"file.recent", GroupFile, ...; "view.about", GroupView, CmdAbout, "" => ShowAbout; "file.quit", GroupFile, ...;` — a single `GroupView` row ("About") is sandwiched inside the `GroupFile` block, and the real, large `GroupView` block does not start until much later (line 496+).

**Expected.** Every command belonging to the same `Key::Group` should render under one contiguous heading, matching the comment's own claim (`settings_window.rs:996-999`). Either `actions.rs` must declare `"view.about"` inside the `GroupView` block instead of between two `GroupFile` rows, or `render_keys` must stop relying on table order and instead group `found` by `command.group` before rendering.

**Fix direction.** Either reorder the rows inside the `bindable!{ ... }` invocation in crates/auris-gpui/src/actions.rs so all rows sharing a `Group*` are contiguous (restoring the invariant `found_commands`'s doc comment already claims), or make `render_keys` not rely on table order: bucket `found` by `command.group` (e.g. into an IndexMap/Vec-of-groups keyed by first appearance, or a fixed enumerated group order) before emitting headings, instead of detecting a change from the immediately preceding row.

**Written rule it breaks.** Filtered but *not* reordered: this list is arranged under section headings, and sorting by score would scatter the sections. (doc comment on `found_commands`, settings_window.rs:996-999)

### F-067 · high · macOS native Edit/View/Transport menus never disable Undo/Redo or check toggles, because menus() is built once from MenuState::default() and gpui's MenuItem::Action has no enabled/checked field.

`crates/auris-gpui/src/menu.rs:652` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** On macOS, opening the native Edit menu always shows Undo/Redo as clickable regardless of whether there is anything to undo, and every toggle-shaped command in the system menu (Mixer, Metronome, Loop, Punch, Recording, Monitoring, Musical Typing, etc.) is shown as a plain label with no checkmark reflecting its actual on/off state. The in-window bar on Windows/Linux renders the same MenuState correctly every frame, so this is a macOS-only regression against the app's own intended behavior.

**Trigger.** Launch the desktop app on macOS (or open a project with nothing yet to undo) and open the native Edit menu from the system menu bar: Undo and Redo are shown as normal, clickable items instead of dimmed, even though `command_if(state.can_undo, ...)` in menu.rs computed `enabled: false`. Equally, toggle the Mixer, Metronome, Loop, Punch, Recording, Monitoring, Musical Typing, or any structure/harmony/tempo/bend/modulation lane on via its keyboard shortcut, then open the corresponding system menu (View/Transport): the row never shows the ✓ tick that the very same state produces correctly in the Windows/Linux in-window bar (`ui/menu_bar.rs`, which reads live `self.menu_model()` every frame and […]

**Mechanism.** `pub fn menus(language: Language) -> Vec<Menu>` (menu.rs:651-673), which is the only function that builds the menu handed to the operating system, calls `model(language, &PanelLayout::default(), MenuState::default())` unconditionally — it has no way to receive the session's real `can_undo`/`can_redo`/`looping`/`recording`/... state or the window's real `PanelLayout`. `MenuRow::Command { enabled, checked, .. }` is computed correctly inside `model()` from that state, but `menus()` (menu.rs:663-667) converts each row to `gpui::MenuItem::Action { name, action, os_action: None }`, which — confirmed by reading gpui 0.2.2's own `platform/app_menu.rs` (`enum MenuItem::Action`) — has no field for `enabled` or `checked` at all, so both are discarded. The caller (`AurisApp::apply_language` in app.rs, the only place other than start-up that calls `cx.set_menus(menu::menus(...))`) only re-invokes this on a language change, never on any document or UI-state change, so even if `MenuItem` had room, nothing would ever push a fresh `MenuState`/`PanelLayout` into it. Separately, gpui's own default […]

**Expected.** The lens's own stated concern is 'view state that diverges from the document after undo/load/autosave/regeneration'; the project's `MenuState`/`PanelLayout` plumbing exists specifically so 'a noun with no mark beside it' (menu.rs:65-66) does not ship, and the in-window bar (menu_bar.rs) demonstrates the correct behaviour by rebuilding from live state every frame. The macOS path should either resync `cx.set_menus` whenever the state it renders changes, or gate the disabled/checked-only commands […]

**Fix direction.** Either re-invoke `cx.set_menus(menu::menus(...))` with a freshly captured `MenuState`/`PanelLayout` whenever any of the eight tracked facts change (undo stack, panel visibility, transport toggles), not just on language change, or wire gpui's `on_validate_app_menu_command`/`is_action_available` hook so the OS asks for enabled/checked state live instead of relying on a one-shot snapshot; the doc comment on `menus()` already names the second half of the problem (`MenuItem` has no field to carry `checked`/`enabled`), so a real fix likely needs both a resync trigger and, upstream, a gpui change to carry that state.

**Written rule it breaks.** MenuRow's own comment at menu.rs:65-66 (quoted in the finding) states the plumbing exists so a menu row is never "a noun with no mark beside it"; the in-window bar honors this every frame while the macOS system menu built by `menus()` does not.

### F-069 · high · render_agent_chat re-runs load_preferences() every repaint while unconfigured, wiping the provider/URL/API-key-env fields on every keystroke or dropdown pick until a model is chosen.

`crates/auris-gpui/src/ui/agent_chat.rs:846` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** On any machine where no agent model has been saved yet (the default state), the Agent settings form clobbers itself on every repaint: picking "openai" from the provider dropdown snaps back to "ollama", and every keystroke typed into the base-URL or API-key-env-variable field vanishes on the next render, because render_agent_chat unconditionally re-runs load_preferences(&self.settings.agent) whenever chosen_model is empty and no model is saved. A user whose Ollama isn't at the default localhost address, or who wants to use OpenAI, cannot get text into those fields at all — and picking a model (the only thing that would stop the reset) itself depends on the model list, which depends on the URL/provider they can't set.

**Trigger.** On a machine with no agent model saved yet (`Settings::agent.model` empty — the default state, or after `Default::default()`), open the agent panel's settings section, then either (a) click the provider dropdown and pick "openai" (handler at line ~1152: `this.agent_chat.provider_openai = chosen == 1; ... this.agent_refresh_models();`, which itself calls `cx.notify()`), or (b) click into the URL or API-key-env field and type a character, before ever picking a model from the model dropdown.

**Mechanism.** render_agent_chat (lines 838-850) runs this guard on *every* repaint, not once: `let configured = self.settings.agent.is_configured(); let configuring = self.agent_chat.configuring || !configured; if configuring && !configured && self.agent_chat.chosen_model.is_empty() { self.agent_chat.load_preferences(&self.settings.agent.clone()); }`. `AgentPreferences::is_configured()` (crates/auris-session/src/settings.rs:172-174) is `!self.model.trim().is_empty()` — i.e. `configured` tracks only the *saved* model, not anything the user is currently typing. `load_preferences` (lines 372-377) unconditionally overwrites `provider_openai`, `chosen_model`, `url_field` and `key_env_field` from the saved settings. Editing any text field in the window goes through the shared `EntityInputHandler::replace_text_in_range` (crates/auris-gpui/src/ui/text_field.rs:528-549), which calls `field.insert(text)` and then `gpui::Context::notify(cx)` after every character — forcing render_agent_chat to run again immediately. So as long as no model has ever been chosen (`chosen_model` empty) and none is saved […]

**Expected.** The load-from-saved-preferences step is documented as one-shot ("First opening on an unconfigured machine: start the form from what is saved", line 847) and should run once when the settings section is opened (as the `agent-configure` button's own handler at ~926-935 already does correctly, once per click), not be re-derived every render from state (`chosen_model.is_empty()`) that the user's own typing does not change. No existing test exercises typing into […]

**Fix direction.** Make the preferences load happen once per settings-section opening rather than being re-derived every render from `chosen_model.is_empty()` — e.g. gate it behind an explicit "already loaded" flag on AgentChat state (set once when the section opens, cleared only when settings are reloaded from disk), the way the `agent-configure` button's own handler already loads preferences once per click at line ~926-935.

### F-070 · high · ContextMenu::origin's fallback clamp can place the menu directly over the anchor point in narrow viewports, contradicting its own doc comment's purpose.

`crates/auris-gpui/src/ui/context_menu/menu.rs:329` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In a narrow or snapped/split-screen window, right-clicking to open a context menu (e.g. a track, clip, or mixer strip menu) can draw the menu directly on top of the pointer instead of beside it. The user's very next click — which they expect to land on whatever they right-clicked, or nearby — instead lands on a menu row and silently fires whichever command is under the cursor, an unintended action the user did not choose.

**Trigger.** A window (or split-screen/snapped window) narrower than about 2×the menu's width — e.g. viewport.width = 500px, and a right-click at x = 250 opening a menu whose computed `size()` is 300px wide (`MAX_WIDTH`, easily reached by an ordinary track/clip menu with several rows in the app's supported languages). `anchor.x + size.width = 550 > 500` (overflow) but `anchor.x (250) >= size.width (300)` is false, so the flip branch is skipped; the else-branch clamps x to `min(250, 500-300) = 200`, giving a menu spanning x∈[200,500] which contains the anchor at x=250. The same arithmetic pattern is duplicated for the y-axis at lines 336-342, so a tall menu (e.g. `track_menu`'s color palette rows) in a […]

**Mechanism.** `origin()` flips the menu to the other side of the pointer only when `self.anchor.x >= size.width` (line 329) / `self.anchor.y >= size.height` (line 336) — i.e. only when there is enough room on the *opposite* side to fit the whole menu. When the viewport is narrower than roughly twice the menu's width (so neither `anchor.x <= viewport.width - size.width` nor `anchor.x >= size.width` holds), both the 'fits as-is' and the 'flip' conditions are false, and the code falls through to the plain `else` clamp: `self.anchor.x.min((viewport.width - size.width).max(px(0.0)))`. That clamp does not check where the anchor sits relative to the clamped window, so the pointer's x can land strictly inside `[x, x + size.width]` — the menu is drawn on top of the point that was just right-clicked, which the function's own doc comment (lines 324-326) says is exactly what the flip exists to prevent ("pushing it back leaves it under the pointer, where it swallows the click the user is about to make").

**Expected.** Per the doc comment at lines 324-326, the menu must always be positioned so it does not cover the anchor point; when there isn't room to fit it fully on either side, `origin()` should still choose the placement that clears the anchor as far as the viewport allows (e.g. clamp to whichever edge leaves the largest gap, or clamp on the axis where a flip cannot fully fit) rather than falling into a plain window-relative clamp with no relation to the anchor.

**Fix direction.** In the else branch of each axis, don't just clamp to the viewport — clamp to whichever side of the anchor leaves it outside the menu's bounds and picks the larger available gap: if there's more room to the anchor's left than its right, place the menu's right edge at min(anchor.x, viewport.width) working leftward and clamp its left edge at 0 (and symmetrically for the right side / y-axis), so the resulting box never contains anchor.x/anchor.y even when a full flip can't fit.

**Written rule it breaks.** A menu that would overflow is flipped to the other side of the pointer rather than merely pushed back inside: pushing it back leaves it under the pointer, where it swallows the click the user is about to make. (doc comment, menu.rs:324-326)

### F-071 · high · Most clip-context-menu rows (e.g. ToggleClipMute, SplitClipAtPlayhead) act only on the right-clicked clip, ignoring the rest of a multi-clip selection the menu's own title claims to act on.

`crates/auris-gpui/src/ui/context_menu/command.rs:956` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Select several clips, right-click one of them (the selection is preserved per gestures.rs:406-410), and the context menu titles itself "N clips" — implying the row acts on all of them, per clip_menu's own comment (clips.rs:57-64: "the menu acts on all of them"). Choosing "Mute" or "Split at Playhead" (and most of the other ~12 rows besides Duplicate/Cut/Copy/Delete) silently applies only to the single clip that was right-clicked; the rest of the selection is left untouched with no error or indication anything was skipped.

**Trigger.** Select three clips on a track (so `self.selected_clips.len() > 1`), with at least one already muted and at least one not. Right-click one of the unmuted clips in the selection — the menu opens titled "3 clips" (via `messages::clip_count`). Choose "Mute".

**Mechanism.** `clip_menu` (clips.rs:57-64) titles itself with the whole selection's count specifically because "With several clips selected the menu acts on all of them, so it says so rather than naming one and quietly taking the rest with it." `clips_for_command` (clips.rs:488-498) states the general rule this depends on: "A command aimed at a clip inside the selection takes the whole selection with it, which is what selecting several of them was for; one aimed elsewhere acts alone." `toggle_clip_loop` (crates/auris-gpui/src/ui/commands.rs:425-430) repeats the same claim as a universal fact: "Every selected clip when the one asked about is inside the selection, the way every other clip command works." But `clips_for_command` is only actually called from four sites in command.rs (DuplicateClip, CutClips/CopyClips, DeleteClip) plus `toggle_clip_loop` itself. Every other row `clip_menu` offers dispatches on the bare `clip` id instead: `MenuCommand::ToggleClipMute(clip) => { let muted = self.clip_is_muted(clip); let _ = self.session.set_clip_muted(clip, !muted); }` (command.rs:956-959) reads and […]

**Expected.** Per `clips_for_command`'s own doc comment and the menu's title logic, a command chosen from a clip menu opened on a clip that is part of a multi-clip selection should act on `self.clips_for_command(clip)`, i.e. the whole selection, the same way `CutClips`, `CopyClips`, `DuplicateClip`, `DeleteClip` and `ToggleClipLoop` already do.

**Fix direction.** Route every clip-menu command handler in command.rs through `self.clips_for_command(clip)` instead of the bare `clip` id — the same pattern already used by DuplicateClip/CutClips/CopyClips/DeleteClip — starting with ToggleClipMute and SplitClipAtPlayhead, then auditing the remaining ~12 rows the menu builds for the same bypass.

**Written rule it breaks.** A command aimed at a clip inside the selection takes the whole selection with it. (clips_for_command doc comment, clips.rs:488-498); "the way every other clip command works" (toggle_clip_loop, commands.rs:425-428)

### F-072 · high · Mixer's Add-Send "+" button silently does nothing in any project with zero bus tracks, since the empty menu it builds is dropped by open_menu with no feedback.

`crates/auris-gpui/src/ui/context_menu/tracks.rs:568` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In any project with no bus tracks (the default state of a freshly created project), clicking the "+" (Add Send) button on a mixer strip produces no visible reaction whatsoever — no menu, no message, no disabled/greyed appearance. The user has no way to discover that they must first create a bus track before this button does anything; the control simply appears broken.

**Trigger.** Open a new project (or any project with no bus tracks) with at least one instrument/audio track, open the mixer, and click the "+" (Add Send) button on that track's strip.

**Mechanism.** `send_picker_menu` (tracks.rs:568-580) builds its rows purely by iterating `self.bus_names()`: `let mut menu = ContextMenu::new(anchor, self.t(Key::MenuAddSend)); for (bus, name) in self.bus_names() { menu = menu.item_greyed_unless(...); } menu`. When the project has zero bus tracks (`self.session.buses()` filters `track.kind.is_bus()`, and a fresh project has none until one is explicitly added), the loop body never runs and the returned `ContextMenu` has zero `MenuEntry::Item`s. `ContextMenu::is_empty()` (menu.rs:288-294) then reports `true`, and `AurisApp::open_menu` (menu.rs:370-376) explicitly drops any menu that `is_empty()` — the very failure mode `window_tests`'s own module doc in menu.rs warns about: "a menu whose every row turned out to be conditional does not open at all — which reads as a broken control." The track-header path that also opens this menu is guarded against exactly this (`track_menu` in tracks.rs:87-91 only offers its "Add Send" row `.item_if(self.session.buses().next().is_some(), ...)`), but the mixer strip's own "+" button […]

**Expected.** The mixer's Add-Send control should either be hidden/greyed when there is no bus to send to (matching the track-header menu's own `item_if(self.session.buses().next().is_some(), ...)` guard), or `send_picker_menu` should offer a greyed placeholder row the way `recent_menu` does for an empty Recent list, so the control never opens onto nothing.

**Fix direction.** In `output_row` (crates/auris-gpui/src/ui/mixer.rs:332-340), disable the "+" button (matching the pattern already used elsewhere for the recent-projects list, tracks.rs:234-235: "A disabled row rather than an empty menu") when `self.bus_names().is_empty()`, or alternatively have `send_picker_menu` return a single disabled/greyed placeholder item (e.g. "No buses") instead of zero entries so `open_menu` doesn't drop it silently.

**Written rule it breaks.** // A disabled row rather than an empty menu. A menu that opens with nothing in it (tracks.rs:234-235, the codebase's own established convention for exactly this situation)

### F-079 · high · A plain click on an overlapping unfaded clip unintentionally writes a crossfade and an undo step via end_drag's ungated ClipMove branch (app.rs:1721).

`crates/auris-gpui/src/app.rs:1721` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Simply clicking on an audio clip to select it, when it overlaps an unfaded neighbour, silently writes new fade-in/fade-out curves into both clips, invalidates the render graph, and records a real undo step (making the project dirty). With autosave on by default, this unrequested audible edit can be written to disk without the user ever having dragged anything.

**Trigger.** Get two audio clips on the same track into an overlapping, unfaded state without ever finishing a `ClipMove` gesture over them — e.g. drag one clip's end handle (`Drag::ClipResize`, whose `end_drag` path never calls `crossfade_landings`) past the start of its neighbour, or simply open/import a project that already has two touching/overlapping unfaded audio clips. Then single-click (mouse down, mouse up at the same point, well under the drag threshold) on either clip to select it — an action a user does routinely just to inspect a clip in the inspector.

**Mechanism.** `end_drag` (lines 1711-1751) is the sole handler for every mouse-up (`root.rs:1126` calls `self.end_drag(window, cx)` unconditionally), and it takes whatever `self.drag` currently holds. Pressing down on a clip's body always begins a `Drag::ClipMove` immediately (`ui/arrangement/gestures.rs:366-378`), with `pressed_at: Some(event.position)` guarding real movement: the pointer-move handler in `root.rs:738-745` returns early — leaving the clip's position untouched — until the drag passes `past_drag_threshold`. But the crossfade block in `end_drag` (lines 1721-1727) is reached unconditionally for any `Drag::ClipMove`, with no check of `pressed_at`/the drag threshold: `if let Drag::ClipMove { origins, .. } = &drag { let moved = ...; let joins = self.session.crossfade_landings(&moved); ... }`. `Session::crossfade_landings` (crates/auris-session/src/session/clips.rs:835) is purely geometric — it looks at each named clip's *current* overlap via `crossfade_partner` and, when `join_is_clear` (neither touching edge already has a fade), calls `shape_join` to write new fade-in/fade-out frame […]

**Expected.** `crossfade_landings` should only run for a `ClipMove` that actually crossed the drag threshold (moved the clip), the same way the `RubberBand` branch two blocks below it in the very same function gates its click-vs-drag behaviour on `!past_drag_threshold` before acting. `Session::crossfade_landings`'s own doc comment states it should be called only for "the clips the caller says have moved" — a bare click is not a move, and per this project's own rule ("the score does not change... […]

**Fix direction.** In `end_drag` (app.rs:1721), gate the `Drag::ClipMove` crossfade block on the gesture having actually crossed the drag threshold (e.g. check `past_drag_threshold` against the stored `pressed_at` before calling `crossfade_landings`), mirroring the guard already used two blocks below for the `RubberBand` click-vs-drag case.

**Written rule it breaks.** The score does not change; the performer does — "regeneration is always a command aimed at the clip", and clip edits are meant to be explicit user commands, not side effects of selection.

### F-080 · high · press_curve_lane hit-tests curve points against the snapped click tick, not the raw press position, so off-grid points become unclickable under coarse grid/zoom.

`crates/auris-gpui/src/ui/piano_roll.rs:2231` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** After creating an off-grid curve point (by holding the secondary modifier while placing it), the user cannot click back onto that same point to drag or delete it whenever the grid is coarse relative to the zoom level (e.g. a quarter-note grid, or any grid at a zoomed-in view where grid spacing exceeds roughly twice the ~7px grab radius). The click misses, and depending on where the snapped click lands it can create a brand-new point next to the one the user meant to grab, or select nothing at all — the off-grid point becomes effectively stuck and unreachable by mouse.

**Trigger.** 1) With the secondary/command modifier held, drag-place a bend or CC point at an off-grid tick (e.g. tick 500 inside a 1/4-note grid spanning 0..960). 2) Release the modifier. 3) Click again near the same screen position, without holding the modifier, to grab/move/delete that point (optionally holding the configured delete gesture instead).

**Mechanism.** `press_curve_lane` computes the hit-test position as `let at = (self.snap_unless_held(self.timeline.x_to_tick(event.position.x - bounds.origin.x), event.modifiers) - clip_start).max_zero();` (lines 2226-2230) and then searches for an existing point with `let grabbed = curve_point_at(&points, at, curve_grab_radius(&self.timeline));` (line 2231). Snapping is applied to the press *before* the existing-point search runs. `drag_curve_point` (lines 2267-2285) and the session's `set_curve_point`/`move_curve_point` (auris-session/src/session/clips.rs:42-107) never re-snap a point once it is written, so a point placed with the secondary key held (`snap_unless_held` returns the raw tick) legitimately sits off the grid, exactly where `snap_unless_held`'s own doc comment (app.rs) says it should: 'the gesture every DAW uses for put it exactly here.' `curve_grab_radius` is only ~7 pixels (`CURVE_GRAB`), far narrower than a typical grid division once converted to ticks.

**Expected.** A press near an existing curve point should find that point regardless of whether the press itself would snap elsewhere, the way `AurisApp::note_at` hit-tests a note's raw tick range rather than a snapped one. The grab search in `press_curve_lane` should test the point's proximity to the raw (unsnapped) click position — or otherwise search `points` before applying `snap_unless_held` — so grabbing and deleting an off-grid point works without also having to hold the placement modifier again.

**Fix direction.** In press_curve_lane, hit-test curve_point_at against the raw (unsnapped) tick derived from event.position, not the output of snap_unless_held; keep snap_unless_held only for the value used when writing/moving a point (the drag/create path), so an existing point at any tick can be found by proximity to the actual click regardless of grid snapping.

**Verifier's correction.** The mechanism, line numbers, ~140-tick grab radius and consequence are all accurate as stated. One refinement: the bug's trigger condition depends on the grid size relative to the (zoom-dependent) grab radius — it reproduces under the claim's own example (a coarse, e.g. quarter-note, grid) but does NOT reproduce at the application's actual default grid (a 16th note, 240 ticks), where the same off-grid point (tick 500) snaps to within the grab radius (480, 20 ticks away vs. a 140-tick radius) and is still found. The defect is real whenever grid ≳ 2×grab_radius (coarse grid and/or a zoomed-in […]

### F-081 · high · fade_handle_at ignores loop passes, so a phantom fade-out grab hijacks resize on looped clips.

`crates/auris-gpui/src/ui/arrangement/geometry.rs:207` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** On any audio clip that loops more than once, clicking near the boundary between the first and second repeat, where only a thin repeat-divider line is drawn, silently starts a fade-out drag instead of the resize the cursor promised. Dragging even slightly saturates the fade fraction to 1.0, fading the rest of a multi-repeat clip to near silence, while the fade-out handle actually drawn at the true sounding end is never clickable at all.

**Trigger.** Loop an audio clip so it repeats more than once (e.g. drag its loop handle to `loop_end = 3 * length`, exactly the gesture `ClipGrab::Loop`/`Drag::ClipLoop` exists for), then press inside the fade-band height (`y_in_clip` in `[TITLE_HEIGHT, TITLE_HEIGHT+FADE_BAND]`) at the tick boundary between the first and second repeat — a position where nothing but a thin repeat-divider line (`paint::vline`) is drawn.

**Mechanism.** `fade_handle_at` computes `let left = f32::from(view.tick_to_x(start));` (line 207) and `let out_x = left + width * (1.0 - (fade_out as f64 / frames as f64) as f32);` (line 209), where `width = view.duration_to_width(length)` (line 200) — `length` being the clip's single-pass content length (`audio_clip_length_ticks`), never the looped/sounding length. `fade_grab_at` in gestures.rs (lines 93-114) calls it with only `clip.start` and `self.audio_clip_length_ticks(clip)`; `clip.loop_end` is never read. But the painter (`lane_paint.rs`) draws the fade-out ramp only on the *last* loop pass: `for (index, (offset, span)) in loop_passes(clip.length, clip.loop_end).enumerate()` (line 175), `let pass_x = bounds.origin.x + view.tick_to_x(clip.start + offset)` (line 176), and `let last = clip.start + offset + span >= clip.start + clip.sounding_length();` (line 243) gates `fraction(*fade_out_frames)` only when `last` — i.e. at `offset = (passes-1)*length`, not `offset = 0`. For any clip with more than one pass, the drawn fade-out ramp sits near `clip.start + sounding_length()` while […]

**Expected.** geometry.rs's own module doc states the rule this violates: "A hit test measured from one number and a painter from another is how a grab bar ends up a pixel to the left of the bar it is drawn on, which is a bug nobody sees and everybody feels" (lines 10-12), and lanes.rs's `clip_edge_zones` doc: "The cursor and the press have to agree about this. An arrow promising a grab the button does not deliver is worse than no arrow at all." `fade_handle_at` should locate the fade-out ramp using the same […]

**Fix direction.** In geometry.rs fade_handle_at (and its only caller fade_grab_at in gestures.rs), compute out_x from the clip's sounding/looped length instead of the single-pass length, mirroring how lanes.rs clip_edge_zones already derives the loop handle position from sounding_length.

**Written rule it breaks.** geometry.rs module doc says a hit test measured from one number and a painter from another is a bug nobody sees and everybody feels; lanes.rs clip_edge_zones doc says the cursor and the press have to agree.

### F-085 · high · Piano-roll note creation snaps the clip-relative tick instead of the absolute one, so new notes miss the drawn grid whenever clip_start isn't grid-aligned.

`crates/auris-gpui/src/ui/piano_roll.rs:1146` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Whenever a MIDI/singer clip's start tick is not itself a multiple of the grid (i.e. most clips not dragged to a grid-aligned position, or any clip nudged after creation), clicking on a visible grid line in the piano roll to create a note places it off that line — the note lands `clip_start mod grid` ticks away from where the user clicked and from every grid line drawn on screen. The same misalignment affects the right-click "create note here" path (lines ~1559/1568). Curve-point placement in the same file snaps correctly (on the absolute tick), so the piano roll is visibly inconsistent with itself: notes drift off-grid while automation points land exactly on it.

**Trigger.** Any clip whose `start` is not an exact multiple of the current grid -- trivially reached by changing the grid resolution after the clip already exists (the transport bar's grid picker calls `Session::set_grid` at any time; existing clips are never re-snapped) or by moving a clip with the secondary/command modifier held (`snap_unless_held` explicitly permits off-grid placement). Concretely: grid = 480 ticks (quarter note), `clip.start = Ticks(100)`, user clicks near the absolute grid line at tick 480 (pointer maps to absolute tick ~700 via `x_to_tick`). `local_tick = 700-100 = 600`; `snap(600,480)` rounds to 480 (since 120 < 240); stored `note.start = 480`; the note is drawn at absolute […]

**Mechanism.** In `begin_note_drag`'s create branch: `let tick = self.timeline.x_to_tick(event.position.x - origin.x);` (line 1017, an ABSOLUTE song tick) is turned into `let local_tick = tick - clip_start;` (line 1025) BEFORE snapping, and then `let start = self.snap(local_tick).max_zero();` (line 1146) rounds that ALREADY clip-relative value to the nearest multiple of `self.project().grid`. `AurisApp::snap` (crates/auris-gpui/src/app.rs:1834) is `tick.snap_nearest(self.project().grid)`, and `Ticks::snap_nearest`/`snap_floor` (crates/auris-core/src/time.rs:58-76) round to the nearest multiple of `grid` counted from absolute tick 0 -- they know nothing about `clip_start`. But notes are rendered at `clip_start + note.start` fed through the same `TimelineView::tick_to_x` that draws the grid lines (`paint_notes` in this file, and `paint::time_grid`), so a note only lands on a drawn grid line when `clip_start` is itself an exact multiple of `grid`. The very same file does the conversion the other way for curve points: `press_curve_lane` (lines 2226-2230) and `drag_curve_point` (line 2276) snap the […]

**Expected.** Per `ui/timeline.rs`'s own module doc ("The arrangement ruler, the clip lanes and the piano roll all have to agree pixel-for-pixel about where a tick lands"), note placement should snap the absolute tick first and subtract `clip_start` afterward, exactly as `press_curve_lane`/`drag_curve_point` already do in this same file -- e.g. `(self.snap_unless_held(tick, event.modifiers) - clip_start).max_zero()`.

**Fix direction.** Snap the absolute tick before subtracting clip_start: replace `let local_tick = tick - clip_start;` ... `let start = self.snap(local_tick).max_zero();` with `let start = (self.snap(tick) - clip_start).max_zero();` (and apply the same fix to the right-click create-note path at piano_roll.rs:1559/1568), so the grid used for snapping matches the grid drawn on screen and used by `press_curve_lane`/`drag_curve_point`.

### F-094 · high · Lyrics/prompt text areas hard-clip past max_rows with no vertical scroll, hiding text and caret once content exceeds 12 lines.

`crates/auris-gpui/src/ui/text_area.rs:199` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Typing or pasting lyrics longer than max_rows (12 rows for both the compose-lyrics prompt and a song section's lyrics box) makes every line past row 12, and the caret itself once it advances past that row, render nowhere: paint_area draws all rows at an unshifted row_top and the surrounding paint::clipped hard-clips anything past the box's fixed height. The user keeps typing/editing correctly under the hood (the TextField model is intact) but is editing blind, with no visual feedback and no way to click back onto the off-screen rows.

**Trigger.** In `crates/auris-gpui/src/ui/prompt.rs`, `render_prompt_area` calls `area_height(field.content(), 4, 12)` for the `ComposeLyrics` prompt (opened via `on_compose_from_lyrics` in root.rs with an empty field). Paste or type more than 12 lines of lyrics (ordinary for 'lyrics a whole song is composed from' — verses/choruses routinely run 16-40+ lines) and press Down repeatedly or keep typing past line 12; `TextField::move_down`/`insert` have no row limit, so the caret and later lines advance into content rows that `paint_area` can no longer draw inside `bounds`. The identical pattern recurs in `crates/auris-gpui/src/ui/compose_sheet/lyrics.rs` (`MAX_ROWS = 12`) for a song section's own lyrics […]

**Mechanism.** `area_height()` (lines 47-50) caps the box's pixel height at `AREA_LINE_HEIGHT * max_rows + AREA_PADDING_Y*2` once the content has more than `max_rows` lines. `paint_area()` computes exactly one scroll offset, `scroll` (line 199: `let scroll = (caret_x - visible).max(px(0.0));`), and it is purely horizontal — derived from the caret's x-advance on its own row. `row_top` (line 203: `bounds.origin.y + AREA_PADDING_Y + AREA_LINE_HEIGHT * row as f32`) then places every row at its raw index with no vertical offset subtracted, and the loop at line 213 draws every line of `lines(text)` unconditionally, including lines whose `row_top(row)` falls below `bounds.origin.y + bounds.size.height`. Painting is hard-clipped to `bounds` via `paint::clipped` (`window.with_content_mask`), so those rows — and the caret itself when `watched_row >= max_rows`, drawn at lines 280-291 — are simply invisible. `area_offset_at()` (lines 79-97) likewise cannot place a click on those rows: mouse events only fire within the element's actual rendered bounds (capped at `max_rows` rows), so `position.y` can never […]

**Expected.** The module's own doc comment (text_area.rs lines 9-11) states the box's answer to overflow is to scroll: "A line longer than the box scrolls sideways under the caret instead, the way the one-line field always has." The same guarantee should hold vertically once the line *count* — not just one line's width — exceeds the box, mirroring the horizontal `scroll` computed at line 199 with a `scroll_y` derived from `watched_row` and `bounds.size.height`; instead there is no such mechanism at all.

**Fix direction.** Give paint_area a vertical scroll term analogous to the existing horizontal `scroll`: derive `scroll_y` from `watched_row` and the visible row count (`bounds.size.height`/`AREA_LINE_HEIGHT`) so the watched row is always kept in view, subtract it in `row_top`, and skip painting rows that fall outside the visible window; `area_offset_at` needs the same scroll_y to map a click's y position back to the correct row.

**Written rule it breaks.** A line longer than the box scrolls sideways under the caret instead, the way the one-line field always has. (text_area.rs module doc, lines 9-11) — the same overflow-handling guarantee is stated for width but not honored for row count.

### F-095 · high · Note context menu titled "N notes" still applies ornament/lyric rows to only the single note under the pointer, silently dropping the rest of the selection.

`crates/auris-gpui/src/ui/context_menu/clips.rs:295` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Selecting several notes in the piano roll and right-clicking one of the selected notes opens a menu titled "N notes" (or "ノート N 個"), but choosing Add/Remove Scoop, Fall, Vibrato, Reset Ornaments, Edit Lyric, Edit Phonemes, or Reset Phoneme Timing silently applies only to the single note under the pointer — the other selected notes are left untouched with no error or indication that anything was skipped.

**Trigger.** Select several notes in the piano roll, then right-click a note that is *not* part of that selection (or is, it doesn't matter) and already carries a scoop. The title reads "N notes"; choose "Remove Scoop".

**Mechanism.** roll_menu's title (lines 294-302) switches to `messages::note_count(self.language(), count)` whenever `selected > 1`, and that message's doc comment in crates/auris-i18n/src/messages.rs:89 says "Title of a menu acting on more than one note." But EditLyric (327-331), EditPhonemes (335-339), ResetPhonemeTiming (355-359), SetScoop (369-374), SetFall (381-386), SetVibrato (393-398) and ResetOrnaments (402-406) are all gated on `under_pointer.is_some()` alone and hard-code `index: under_pointer.unwrap_or(0)` — the single note under the pointer, which is not required to be a member of `self.selected_notes` at all. Only WriteLyrics, Cut/Copy/Duplicate/Delete, Transpose, Quantize and SetNoteVelocity actually operate on the whole `self.selected_notes` set.

**Expected.** A menu titled as acting on N notes should act on the selection for every row, or the ornament rows should not participate in the group title / should be worded per-note regardless of selection size.

**Fix direction.** Either scope the ornament/lyric rows' condition and index list to selection-aware behavior (iterate `selected_notes` when the pointer's note is part of the current selection, applying the toggle/edit to every selected note via the session APIs) or, more simply, make the title itself reflect what will actually happen: keep it singular ("Note") whenever the acted-on index is under_pointer alone, and only show the plural "N notes" title for rows that truly act on the whole selection (Cut/Copy), never let the same menu instance mix a plural title with single-note-scoped rows.

**Written rule it breaks.** Title of a menu acting on more than one note. (doc comment on messages::note_count, crates/auris-i18n/src/messages.rs:89)

**Verifier's correction.** The claim's mechanism, trigger and consequence are accurate for the sub-case "right-click a note that IS already part of the current multi-note selection." The "(or is, it doesn't matter)" phrasing overstates one sub-case: right-clicking a note NOT currently in the selection first collapses `selected_notes` to that single note (crates/auris-gpui/src/ui/piano_roll.rs:1557-1580, `open_roll_menu`), so in that sub-case the title correctly falls back to singular "Note" rather than "N notes" — the title itself is not wrong there, only the general asymmetry between selection-wide rows and […]

### F-110 · high · Resetting a command's keybinding while a capture is armed leaves the capture live, so the next keystroke silently rebinds the just-reset command.

`crates/auris-gpui/src/settings_window.rs:1308` · correctness · confirmed (traced through the code; reported independently 1×)

**What a user sees.** If a user arms a key-capture (clicks "+" to add a new binding, or otherwise triggers `arm`) for a command and then, before pressing a key, clicks that same command's "reset to default" button, the reset button's handler clears the keymap override but leaves `self.capturing` set and the window still focused for capture. The very next keystroke the user types anywhere (even unrelated, e.g. while navigating the settings window) is silently consumed by `capture_key` and bound onto the command they just reset — and because the slot index recorded before the reset may now be out of range for the (now-default) binding list, it goes through `keymap.add` rather than `keymap.set_at`, appending an unintended extra binding instead of overwriting. The user sees only the reset happen; the surprise binding shows up later with no indication why.

**Trigger.** In the Keys tab: (1) rebind a command's existing key, e.g. slot 0 of a command whose default is one key, so `is_overridden` becomes true; (2) click that chip again to re-arm it (`arm(command, 0, ...)`, `self.capturing = Some(Capture{command, 0})`), showing 'Press a key'; (3) instead of pressing a key, click the row's own reset-to-default (X) button that is still visible next to it. `keymap.clear(command)` reverts the override, but `self.capturing` is left as `Some(Capture{command, 0})`; (4) press any key anywhere while the settings window has focus — `on_key` (line 1345) still routes to `capture_key` (since `self.capturing.is_some()`), and because slot 0 still exists in the now-default […]

**Mechanism.** The individual command's reset-to-default control (the `chain_button` built at lines 1303-1316) runs `this.keymap.clear(command); this.apply_keymap(cx);` and, unlike every other keymap-mutating control in this file, never sets `this.capturing = None`. Compare: `reset-all` (lines 1054-1059) sets `this.capturing = None`; the per-group reset in `render_group_heading` (lines 1150-1154) sets it; the per-slot `drop:{id}:{slot}` control (lines 1206-1210) sets it; and `unbind:{id}` (lines 1293-1299) sets it. Only `reset:{id}` (lines 1304-1313) omits it. Since this cross button is rendered whenever `is_overridden(command)` is true regardless of whether that same command is currently armed via `arm()` (line 1328, which sets `self.capturing = Some(Capture{command, slot})`), a user can click it while a capture for that same row is in progress, leaving `self.capturing` pointing at a slot whose meaning has just changed underneath it.

**Expected.** Every other action that mutates the keymap in this file (`reset-all`, per-group reset, per-slot drop, unbind) explicitly clears `self.capturing` so a stale capture can never outlive the row state it was armed against; the per-command reset-to-default control should do the same.

**Fix direction.** Add `this.capturing = None;` to the `reset:{id}` button's listener at settings_window.rs:1308-1310, matching every other keymap-mutating handler in the file (reset-all, per-group reset, unbind, per-slot drop all already do this).

**Written rule it breaks.** this is the only one of the file's keymap-mutating handlers that omits `this.capturing = None;` (six sibling sites all set it — an implicit but consistently-followed invariant of the module, not a written doc rule)

### F-113 · high · Plugin-open state keyed by scan-list index (not file identity) lets adding/removing a plugin folder auto-load an unrelated .clap binary with no user gesture.

`crates/auris-gpui/src/ui/library.rs:857` · security · confirmed (traced through the code; reported independently 1×)

**What a user sees.** After a user opens one `.clap` plugin file in the library (a deliberate click, loading and running that plugin's native binary in-process) and later adds or removes a plugin search folder, the next render can silently load and execute a completely different, unrelated `.clap` binary that now happens to occupy the same list index — with no click, no dialog, and no indication to the user that a new plugin was just loaded and run.

**Trigger.** User has plugin folder `/plugins/B` registered, containing only `zzz.clap`; `installed_clap_files` returns `["/plugins/B/zzz.clap"]`, so it is index 0. The user deliberately expands it (a real click), setting `chosen[PluginFile(0)] = true` and triggering the intended, consented load of `zzz.clap`. The user then adds a second plugin folder `/plugins/A` (containing `aaa.clap`) via 'Add plugin folder…'. `forget_plugin_path`/`add_plugin_path` clears the `clap_files` cache; the next scan, sorted lexically (`found.sort()` in `installed_clap_files`), now returns `["/plugins/A/aaa.clap", "/plugins/B/zzz.clap"]` — `aaa.clap` is now index 0.

**Mechanism.** `installed_plugin_rows` (lines 830-865) reads `let files = self.clap_files().to_vec();` then, per row, keys the disclosure state purely by position: `let branch = Branch::PluginFile(index); let open = self.library.is_open(branch);` (lines 856-857), and if `open` is true it immediately calls `self.clap_plugins_in(file)` (line 864), which — per `Session::hosted_plugins_in`'s own doc (crates/auris-session/src/session/hosted.rs:806-813) — actually loads and runs the third-party `.clap` binary (`unsafe { self.hosted.catalog(file) }`). `clap_files()` (line ~932) is `self.session.installed_clap_files(&self.settings.plugin_paths)`, a fresh sorted-and-deduped scan every time the cache is cleared. `forget_plugin_path`/`add_plugin_path` (crates/auris-gpui/src/ui/commands.rs:1949-1990) mutate `settings.plugin_paths` and reset `self.clap_files = None`, forcing a re-scan whose order/length can change — but neither of them touches `self.library` (the `LibraryTree`), so any previously recorded `chosen[Branch::PluginFile(i)] = true` stays keyed to index `i` regardless of which file now sits there.

**Expected.** The module's own stated rule for exactly this case (crates/auris-gpui/src/ui/library.rs:80-83): 'A `.clap` file is shut for a stronger reason than size. Opening one means loading it, and loading a plugin means running somebody else's code in this process. That has to be something a person did, not something a panel did on their behalf while they were looking for a reverb.' Disclosure state for a plugin file should be keyed by a stable identity (e.g. the canonicalized path itself, the way […]

**Fix direction.** Key `Branch::PluginFile` (and the corresponding `chosen` entries) by a stable file identity — the canonicalized `PathBuf`, mirroring how `Branch::Font`/`Branch::Bank` use `SoundFontId` — instead of scan-list position; alternatively, clear/reset the relevant `chosen` entries whenever `clap_files` is invalidated in `add_plugin_path`/`forget_plugin_path` so no stale index can be reinterpreted as an open request for a different file.

**Written rule it breaks.** A `.clap` file is shut for a stronger reason than size. Opening one means loading it, and loading a plugin means running somebody else's code in this process. That has to be something a person did, not something a panel did on their behalf while they were looking for a reverb. (crates/auris-gpui/src/ui/library.rs:80-83)

### F-038 · medium · create_clip_at names new clips from the project's track count instead of a clip count, so repeated clip creation on a track yields duplicate names like "Clip 1".

`crates/auris-gpui/src/ui/commands.rs:327` · correctness · confirmed (traced through the code; reported independently 2×)

**What a user sees.** Every MIDI clip a user creates on a given track (as long as the project's track count stays fixed) is named identically, e.g. "Clip 1" — creating two clips on one track produces two clips both labeled "Clip 1" in the UI, with no numeric distinction to tell them apart when browsing or selecting clips by name.

**Trigger.** In a project with exactly one track, double-click (or use the context menu's "New Clip") twice on two different empty spots of that same instrument/singer track. Both calls see `self.project().tracks.len() == 1` and both clips are named "Clip 1" (ja: "クリップ 1"). More generally, on any track, every clip created while the project's *track count* stays fixed gets the identical name, regardless of how many clips already exist on that track or elsewhere.

**Mechanism.** `create_clip_at` computes `let count = self.project().tracks.len();` and passes it straight to `messages::new_clip_name(self.language(), count)` (line 328) to name the newly created MIDI clip. `new_clip_name`'s own doc comment (crates/auris-i18n/src/messages.rs:169) says it is the "Name given to a clip the user just created" — i.e. it wants a clip ordinal — but the value handed to it is the number of *tracks* in the whole project, which does not change when a clip is added and is never incremented (`+1` is missing too, unlike every sibling: `add_instrument_track`, `add_singer_track`, `add_audio_track` and `add_bus_track` all use `self.project().tracks.len() + 1` for the same purpose on a track). `Session::add_midi_clip` (crates/auris-session/src/session/clips.rs:150-169) performs no uniqueness check and stores the name verbatim.

**Expected.** The clip's ordinal should be derived from how many clips already exist (e.g. on that track, mirroring the `tracks.len() + 1` pattern the four track-adding commands use), so that successive clips get distinct, incrementing names the way successive tracks do.

**Fix direction.** In `create_clip_at` (crates/auris-gpui/src/ui/commands.rs:327), replace `let count = self.project().tracks.len();` with a count derived from the clips already on the target track (e.g. `self.project().track(track).map_or(1, |t| t.clips.len() + 1)`), mirroring the `tracks.len() + 1` pattern used by the sibling track-adding commands in the same file.

### F-044 · medium · Empty ClipSourceTempo field is rejected by commit_prompt's generic empty-check before it can reach the arm meant to clear the tempo to None.

`crates/auris-gpui/src/ui/prompt.rs:674` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user who types a source tempo into a clip by mistake and then tries to clear it by deleting the field's text and pressing Return gets the generic "Name cannot be empty" error and the sheet does not close; the clip keeps its (wrong) tempo. There is no way through the UI to get the clip's source-tempo field back to "unknown" (None) once it has been set to a value, except presumably other, less discoverable means (e.g. undo, or none at all).

**Trigger.** Open the clip's source-tempo prompt (PromptTarget::ClipSourceTempo) on a clip that already has a source tempo set, select all and delete so the field is empty, then press Enter (or click the primary button, which also calls commit_prompt).

**Mechanism.** commit_prompt() takes `self.prompt` (closing the sheet) at line 646 before doing anything else, then computes `text = field.content().trim().to_string()` (659) and an `empty_clears` allow-list at 663-673 that only names `Lyric`, `Phonemes`, `Lyrics`, `ComposeLyrics` and `SongMotif`. Line 674-677 then does `if text.is_empty() && !empty_clears { self.set_status(...NameCannotBeEmpty...); return; }` for every target NOT on that list. `PromptTarget::ClipSourceTempo` is missing from the list, so an empty `text` is intercepted and the function returns at line 676 -- long before the `match target { ... }` at line 678 is ever reached. The `PromptTarget::ClipSourceTempo` arm itself, at lines 993-994 (`match text.trim().is_empty() { true => self.session.set_clip_source_bpm(clip, None), ... }`), whose own comment at 991-992 says "An empty box means 'nobody knows'... the only way back from a tempo typed in by mistake", can therefore never execute its `true` branch -- `text` can never be empty by the time that match runs.

**Expected.** Per the arm's own comment (991-992), clearing the field should set the clip's source tempo to `None`. `PromptTarget::ClipSourceTempo` needs to be added to the `empty_clears` match at lines 663-673 (the same way `PromptTarget::SongMotif` already is, for the identical reason), so the arm at 993-994 is reachable.

**Fix direction.** Add `PromptTarget::ClipSourceTempo(_)` to the `empty_clears` matches! allow-list at prompt.rs:663-673 so an empty field reaches the existing `ClipSourceTempo` arm at line 991, whose `true => set_clip_source_bpm(clip, None)` then actually runs.

**Written rule it breaks.** An empty box means "nobody knows", which is a thing a clip is allowed to say and the only way back from a tempo typed in by mistake.

### F-045 · medium · commit_prompt's empty_clears guard omits ClipSourceTempo, so clearing a clip's source tempo via the prompt can never run.

`crates/auris-gpui/src/ui/prompt.rs:993` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user who opens the "Set Clip Source Tempo" prompt to undo a mistakenly-typed tempo (clear the box and press Return) sees nothing happen: commit_prompt's shared empty-text guard fires first and returns before the ClipSourceTempo match arm is reached, so `set_clip_source_bpm(clip, None)` is dead code and the clip is stuck with whatever tempo was last set, with no other UI path to clear it back to "unknown".

**Trigger.** Open the clip-source-tempo prompt (context menu → 'Set Source Tempo…'), select all (which is the field's default state per TextField::new) and delete the text, then press Return/click Rename.

**Mechanism.** commit_prompt trims the field once at line 659 (`let text = field.content().trim().to_string();`), then at lines 663-677 refuses to proceed on an empty `text` for every target except those listed in `empty_clears` (Lyric, Phonemes, Lyrics, ComposeLyrics, SongMotif) — `PromptTarget::ClipSourceTempo` is not in that list, so an empty field returns early with `set_status(NameCannotBeEmpty)` before the match on `target` is even reached. The `PromptTarget::ClipSourceTempo` arm at lines 993-994 (`match text.trim().is_empty() { true => self.session.set_clip_source_bpm(clip, None), ... }`) is only reached when `!text.is_empty()`, and since `text` was already trimmed, `text.trim().is_empty()` is identical to the already-false `text.is_empty()` — the `true` branch is unreachable.

**Expected.** The doc comment directly above the arm states the intended behaviour: "An empty box means 'nobody knows', which is a thing a clip is allowed to say and the only way back from a tempo typed in by mistake." `PromptTarget::ClipSourceTempo` needs to be included in the `empty_clears` set (line 663-673) so the match arm's `true` branch is actually reachable.

**Fix direction.** Add `PromptTarget::ClipSourceTempo(_)` to the `empty_clears` match in commit_prompt (crates/auris-gpui/src/ui/prompt.rs, around line 663) so an empty field passes through to the existing arm at line 993, which already correctly maps empty text to `set_clip_source_bpm(clip, None)`.

**Written rule it breaks.** The code's own comment at prompt.rs:990-991: "An empty box means 'nobody knows', which is a thing a clip is allowed to say and the only way back from a tempo typed in by mistake." — this documented behaviour is unreachable.

### F-054 · medium · Settings window mislabels every audio-preference error (mainly "recording in progress") as an "audio restart failed" and leaks raw English text instead of the already-translated message.

`crates/auris-gpui/src/settings_window.rs:255` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A non-English user who changes any audio preference (device, sample rate, block size) while a take is recording sees an English error message instead of their language — the settings window mislabels a "stop recording first" condition as an "audio restart failed" condition and leaks the raw English Display text of SessionError::RecordingInProgress inside it.

**Trigger.** Start recording a take in the main window, then open Settings → Audio tab and click any device row, sample-rate button, or buffer-size button (any control that calls `apply_audio`).

**Mechanism.** `apply_audio` (lines 242-264) unwraps whatever `app.apply_audio_preferences` returns into a plain `String` and always re-wraps it as `SessionError::AudioRestart`: `Err(error) => { crate::i18n::error_text(&SessionError::AudioRestart(error), self.language) }` (line 255). But `app.rs`'s `apply_audio_preferences` (lines 2159-2174) does `self.session.set_audio_preferences(audio.clone()).map_err(|error| error.to_string())?;` — converting the *original* `SessionError` to a bare `String` via `Display` before it ever reaches the settings window, discarding its variant. Tracing `Session::set_audio_preferences` (`crates/auris-session/src/session/mod.rs:615-655`), its only two possible failures are `SessionError::RecordingInProgress` (early `if self.take.is_some() { return Err(...) }` at line 620) and, in principle, `SessionError::AudioRestart` from `start_audio(&settings).map_err(...)？` at line 649 — except `auris_engine::start_audio` (`crates/auris-engine/src/device.rs:217-231`) never actually returns `Err`: both its internal failure branches log a warning and fall back to […]

**Expected.** The settings window should preserve and display the actual `SessionError` variant (or at least route recording-in-progress failures through `Key::ErrorRecordingInProgress` as `crate::i18n::error_text` already does everywhere else), not collapse every audio-preference failure into `SessionError::AudioRestart`.

**Fix direction.** In settings_window.rs::apply_audio, change apply_audio_preferences (app.rs) to return the SessionError itself instead of stringifying it with .map_err(|error| error.to_string()), and have apply_audio call crate::i18n::error_text(&error, self.language) directly on the real variant instead of always re-wrapping into SessionError::AudioRestart.

**Written rule it breaks.** auris-i18n is every word said to a person (the window and the CLI) — implying all user-facing error text must go through the translation layer with its real meaning, not a re-labelled/English-leaking one.

### F-068 · medium · Header column reads self.lane_scroll before render_timeline clamps it, causing a one-frame header/lane misalignment right after a track deletion overflows the scroll offset.

`crates/auris-gpui/src/ui/arrangement/headers.rs:109` · correctness · confirmed (traced through the code; reported independently 1×)

**What a user sees.** If a user has the track list scrolled near the bottom and deletes a track (shortening the header column), the header column is drawn with the pre-deletion, unclamped scroll offset for that render while the lane/clip column beneath it uses the freshly clamped offset. For that frame the header rows sit visibly out of line with the clip lanes they belong to (e.g. a header shifted up by roughly a track's height, lined up against the wrong lane). Because render_timeline's clamp writes back into self.lane_scroll within the same render_arrangement call, the desync self-corrects on the very next repaint, so what the user actually sees is a brief one-frame mis-paint rather than a lasting broken state.

**Trigger.** Scroll the track list to the bottom (`lane_scroll == max_lane_scroll()`) in a project with more tracks than fit on screen, then do anything that shrinks the lane column's total height while it stays scrolled there — delete the bottom-most track, drag a track's height band shorter, or close an open automation lane. The command mutates the session and calls `cx.notify()`, triggering exactly one `render_arrangement` pass.

**Mechanism.** `render_arrangement` (crates/auris-gpui/src/ui/arrangement/mod.rs:43-44) calls `self.render_track_headers(cx)` and then `self.render_timeline(cx)` in that order, within the same render pass. `render_track_headers` bakes `.top(-self.lane_scroll)` (headers.rs:109) into the header column's layout using whatever `self.lane_scroll` holds *at that moment* — i.e. the value left over from the previous frame. `render_timeline` (lanes.rs) only clamps it afterwards: `self.lane_scroll = self.lane_scroll.min(self.max_lane_scroll());` (lanes.rs:67), then captures `let lane_scroll = self.lane_scroll;` (lanes.rs:68) for the clip-lane canvas's paint closures and for `clip_edge_zones`. Because the header div's `top()` value is evaluated eagerly when the element tree is built, it is fixed at the *pre-clamp* value even though `render_timeline` mutates `self.lane_scroll` moments later in the same pass. The lanes.rs:65-66 comment ('A track deleted while the view was scrolled to the bottom leaves the offset past the end, and nothing else would ever pull it back') shows the clamp exists precisely to fix […]

**Expected.** The header column and the clip-lane column must always agree on `lane_scroll`, per headers.rs:103-104's own stated invariant ('Pushed up by the shared offset ... so a header can never drift out of line with the lane it belongs to') and lanes.rs:65-66's comment describing exactly this clamp's purpose. The clamp on lanes.rs:67 needs to run (or its result be applied) before `render_track_headers` reads `self.lane_scroll`, e.g. by clamping once at the top of `render_arrangement` before either […]

**Fix direction.** Compute the clamp once, before either sub-render runs: in render_arrangement (mod.rs), do `self.lane_scroll = self.lane_scroll.min(self.max_lane_scroll());` before calling render_track_headers and render_timeline, and remove the now-redundant clamp inside render_timeline (lanes.rs:67-68) so both columns read one already-clamped value.

**Written rule it breaks.** Pushed up by the shared offset rather than given its own scrollbar, so a header can never drift out of line with the lane it belongs to. (headers.rs, comment directly above the `.top(-self.lane_scroll)` call)

### F-082 · medium · Clip context menu titled "N clips" applies most rows (mute, gain, crossfade, fades, tempo, edit, accompany, motif) to only the single right-clicked clip, not the full selection.

`crates/auris-gpui/src/ui/context_menu/clips.rs:60` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user who selects three clips and right-clicks one sees a context menu titled "3 clips," but choosing Mute, Loop Over Clip, Clip Gain, Crossfade, fade-shape/Clear Fades, Follow Tempo, Clip Source Tempo, Edit in Piano Roll, Accompany, or Compose from Motif silently applies only to the single clip under the cursor — the other two selected clips are left untouched with no indication anything was scoped down.

**Trigger.** Select three MIDI or audio clips (so `selected_clips.len() == 3` and all three are in the set), right-click one of them so the menu opens titled "3 clips", and choose "Mute Clip" (or "Loop Clip", "Follow Tempo", etc.).

**Mechanism.** clip_menu's title logic (lines 60-68) reads: "With several clips selected the menu acts on all of them, so it says so rather than naming one and quietly taking the rest with it," and switches the title to `messages::clip_count(...)` ("N clips") whenever `clip` is part of a multi-clip `selected_clips`. `messages::clip_count`'s own doc comment in crates/auris-i18n/src/messages.rs:127 says "Title of a menu acting on more than one clip." But of the ~15 rows the menu builds, only Cut/Copy/Duplicate/Delete route through `clips_for_command(clip)` (clips.rs:492-498), which expands to the whole `selected_clips` set. Every other row — ToggleClipMute (line 89), ToggleClipLoop (line 97), LoopOverClip (line 102), ClipGain (line 109), Crossfade (line 116), the fade-shape/ClearFades rows (124-145), FollowTempo (152), ClipSourceTempo (161), EditClip (166), AccompanyClip (175), TakeClipAsMotif (184) — constructs its `MenuCommand` with the bare `clip` parameter that was right-clicked, and `run_menu_command` in command.rs (e.g. lines 956-959 for ToggleClipMute, 984 for ToggleClipLoop) acts on that […]

**Expected.** Either the title should not claim group action for rows that are single-target, or (per the code's own stated intent and `clip_count`'s doc comment) these rows should act through `clips_for_command(clip)` the same way Cut/Copy/Duplicate/Delete do.

**Fix direction.** Either narrow the title's promise (e.g. keep the per-clip name, or add a per-row indicator/tooltip distinguishing whole-selection rows from single-clip rows) or extend clips_for_command-style batching to the rows that plausibly should honor the selection (mute, follow-tempo toggles are natural batch candidates); at minimum, the misleading "N clips" title comment at clips.rs:60 should be corrected to state it only applies to Cut/Copy/Duplicate/Delete/ToggleClipLoop, not the whole menu.

**Verifier's correction.** The claim holds for ToggleClipMute, LoopOverClip, ClipGain, Crossfade, the fade-shape/ClearFades rows, FollowTempo, ClipSourceTempo, EditClip, AccompanyClip and TakeClipAsMotif: each builds its MenuCommand from the bare right-clicked clip and run_menu_command acts on that clip alone, contradicting the "N clips" title the menu shows when several clips are selected. ToggleClipLoop (clips.rs:97), however, is misclassified in the claim: although its row also carries the bare `clip`, run_menu_command dispatches it to `toggle_clip_loop` (commands.rs:430), which internally calls […]

### F-131 · medium · Closing an EQ's plugin window skips stop_watching(), so the audio thread keeps publishing that strip's spectrum every block until some other plugin window is opened.

`crates/auris-gpui/src/ui/plugin_window.rs:219` · lifecycle · confirmed (traced through the code; reported independently 2×)

**What a user sees.** After opening an EQ's editor (which starts the audio thread publishing a spectrum window for that strip every block) and then closing it via Escape or the window's close button, the analysis keeps running: `render_plugin_window` never runs again once `plugin_window` is `None`, so `stop_watching()` is never called. The audio thread keeps copying 1024 samples per block into the scope ring for a strip nobody is looking at, until the user happens to open some other plugin's editor (any plugin, on any strip), which incidentally calls `stop_watching()` through the same `render_plugin_window` code path. There is no crash, leak growth, or audible artifact — the work is bounded, lock-free and RT-safe — but it is wasted per-block work performed for no on-screen consumer, for an unbounded stretch of a session.

**Trigger.** Open a plugin editor whose id is `EQUALIZER_ID` (an EQ insert on any track or the master bus) so its curve is drawn — this calls `self.session.watch_strip(subject.strip())` every frame. Then close the window, either via Escape (`root.rs` `handle_escape` calling `self.close_plugin_window()` at line 2253) or the header's Cross button (`chain_button("pw-close", ...)` at plugin_window.rs:421-429), or let the subject stop resolving while the EQ window is open (the effect is deleted, its track is deleted, or an undo/redo crosses the edit that added it).

**Mechanism.** `close_plugin_window` (lines 218-221) is only `self.plugin_window.take().is_some()` — it never calls `self.session.stop_watching()`. The only place that call exists is inside `render_plugin_window` (lines 257-270): `let window = self.plugin_window.take()?; ... let (plugin_id, enabled) = self.resolve_plugin(subject)?; self.plugin_window = Some(window); let equalizer = self.eq_view(subject, &plugin_id); if equalizer.is_some() { self.session.watch_strip(subject.strip()); } else { self.session.stop_watching(); }`. Both `?`s return `None` *before* that watch/stop_watching block runs: the first fires every frame once `plugin_window` is `None` (i.e. after a close), the second fires the frame a subject stops resolving. So the watch/stop_watching decision is only ever re-evaluated while a window stays open and its subject keeps resolving — never on the transition to closed.

**Expected.** crates/auris-engine/src/scope.rs's own module doc states the invariant this breaks: "Analysis exists for a window that is open, and at most one plugin editor is... a graph nobody is looking at pays nothing." Closing the plugin window, by any path, should call `self.session.stop_watching()` the same way switching to a non-equalizer plugin already does inside `render_plugin_window` — e.g. `close_plugin_window` should call it directly, since the render path cannot reach it once the window is gone.

**Fix direction.** Make `close_plugin_window` call `self.session.stop_watching()` whenever the subject being closed was the equalizer being watched (or unconditionally, since stop_watching is idempotent and cheap) before taking `self.plugin_window`, so closing the window that started the watch is what ends it, symmetric with `open_plugin_window`/the watch it starts.

**Written rule it breaks.** Analysis exists for a window that is open, and at most one plugin editor is. ... the UI names the strip it is looking at and the renderer fills only that one. (crates/auris-engine/src/scope.rs module doc)

### F-134 · medium · NoSuchSpeaker renders its whole English thiserror sentence untranslated in the Japanese UI, unlike every comparable local error variant.

`crates/auris-gpui/src/i18n.rs:240` · spec-mismatch · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** A Japanese-language user who selects a nonexistent voice speaker sees an error dialog where the connective sentence is entirely in English ("the voice has no speaker called Alice; it offers Bob, Carol") instead of Japanese, breaking the localized experience on this one error path while every comparable multi-field local error (UnknownPlugin, UnknownProgression, TooManyMonitors, no_singer_named) renders correctly in the user's language.

**Trigger.** Interface language set to Japanese; a singer track's stored voice (`SingerVoice.speaker`, e.g. "Alice") no longer exists in the voice model at that path (the model was retrained/re-exported with a different speaker roster, or the project references a voice file that changed) — `Session::sing_plan` (crates/auris-session/src/session/singer.rs:721-760) calls `speaker_id(model.info(), voice.speaker.as_deref())?` which returns `Err(SessionError::NoSuchSpeaker { name: "Alice", offered: [...] })`. Pressing Sing (`sing_track` in crates/auris-gpui/src/ui/commands.rs:1066-1080) calls `self.failure(Key::CmdSing, &error)`.

**Mechanism.** `error_text` handles `SessionError::NoSuchSpeaker { .. } => with(Key::ErrorSing, error.to_string())` (line 240), where `with = |key, detail| messages::detailed(language, key.get(language), &detail)` (line 216). `error.to_string()` renders `SessionError::NoSuchSpeaker`'s thiserror template verbatim: `"the voice has no speaker called {name}; it offers {offered}"` (crates/auris-session/src/error.rs:142) — a hard-coded English sentence this crate itself wrote, not borrowed from a foreign decoder or driver. Unlike the other 'we own it' error variants (e.g. `unknown_plugin`, `unknown_progression`, `too_many_monitors`, `no_singer_named` in messages.rs, all of which have a bilingual `format!` template), NoSuchSpeaker has no `messages::` entry, so its whole English sentence — not just the speaker names inside it — rides through untranslated after a Japanese prefix.

**Expected.** NoSuchSpeaker should route through a bilingual `messages::` function (mirroring `no_singer_named`), interpolating just the speaker name and the offered list into a fully Japanese sentence, the same way every other locally-defined SessionError variant with data is handled elsewhere in this match.

**Fix direction.** Add a bilingual `messages::no_such_speaker(language, name, offered)` template (mirroring `messages::unknown_plugin`/`unknown_progression`) that translates the surrounding sentence while still interpolating `name` and the raw `offered` list verbatim, and call it from the `SessionError::NoSuchSpeaker` arm in crates/auris-gpui/src/i18n.rs:240 instead of `with(Key::ErrorSing, error.to_string())`.

**Written rule it breaks.** The half we own — which operation failed, and on what — is translated. (crates/auris-gpui/src/i18n.rs doc comment on `error_text`)

### F-138 · medium · discard_unusable() never re-checks survivors against defaults, so a filtered override that matches the default is kept and re-persisted as if customized.

`crates/auris-gpui/src/keymap.rs:164` · correctness · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** A command that has an override entry surviving `discard_unusable()` with a keystroke list identical to its shipped defaults (e.g. because an unusable alternate keystroke was filtered out, leaving only the default one behind, or because a shipped default changed after the file was written) is shown in the keybindings UI as "customized" (`is_overridden` returns true) even though it is not, and that stale, functionally-empty override is written back byte-for-byte on every subsequent save until the user happens to re-edit that exact command's binding through `set_at`/`remove_at`, which is the only path that runs the default-collapsing comparison in `Keymap::store`.

**Trigger.** A `keymap.json` containing e.g. `{"edit.undo": ["notakey-x", "secondary-z"]}` (edit.undo's own default keystroke, `secondary-z`, is exactly what `bindable("edit.undo").default` holds — the same malformed-keystroke pattern the existing test `loading_discards_bindings_this_build_cannot_use` already exercises for `edit.undo`, just with a second, non-default-equal entry removed). On `InputSettings::load()`, `discard_unusable` drops `"notakey-x"` and leaves `overrides["edit.undo"] == ["secondary-z"]`, which is exactly `defaults()` for that command, yet `keymap.is_overridden(bindable("edit.undo"))` returns `true`. Such a file is easy to produce today (a genuine two-key override where one key […]

**Mechanism.** `discard_unusable` (lines 164-179) filters each command's stored keystroke list in place with `keystrokes.retain(|keystroke| actions::is_valid_keystroke(keystroke))` and keeps the map entry regardless of what survives. It never re-runs the collapse-to-default logic that every other mutator in this file goes through via `Keymap::store` (lines 314-332), which explicitly drops an override when the written list, once normalised, equals the shipped defaults ("Writes a command's keystrokes back, dropping the override when it *is* the default ... An override equal to the default would freeze today's default into the file"). If a stored override list contains one invalid keystroke alongside a valid one that happens to equal the command's default, discarding the invalid entry leaves the surviving list textually identical to `command.defaults()`, but the entry stays in `self.overrides`, so `is_overridden` still reports `true`.

**Expected.** After filtering unusable keystrokes, `discard_unusable` should re-apply the same default-collapse rule `Keymap::store` uses, so a surviving list identical to the shipped default is removed from `overrides` rather than kept as a no-op override — consistent with every other write path in this file and with the module's own stated invariant that only genuine changes are ever persisted.

**Fix direction.** In `discard_unusable`, after filtering each entry's keystrokes, compare the surviving list (normalised) against `command.defaults()` normalised and drop the entry (`return false`) when they match, mirroring the check `Keymap::store` already performs — or simply route the survivors through `store`'s existing default-collapse logic instead of writing back to `self.overrides` directly.

**Written rule it breaks.** Only *changes* are written to disk. Storing the full set would freeze today's defaults into every existing settings file... (keymap.rs:1-5); and the `store` doc comment: "An override equal to the default would freeze today's default into the file."

### F-139 · medium · toggle_monitoring hard-resets monitor_gaps to 0 even when the shared Capture device stays open, causing report_monitor_gaps to re-announce the stale cumulative dropout count as a new event.

`crates/auris-gpui/src/ui/transport_bar.rs:927` · ui · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** While monitoring more than one track (e.g. a band monitoring several inputs at once), simply arming or disarming one more track re-displays the "the monitor has broken up N times — try a larger audio block size" status message with the old cumulative dropout count, even though no new dropout occurred since it was last reported. This misleads the user into thinking a fresh audio problem just happened and into re-checking their block size for no reason.

**Trigger.** Monitor two tracks at once (`toggle_monitoring(A)` then `toggle_monitoring(B)`, both stay on — the doc comment above `toggle_monitoring` describes exactly this 'a band monitors as a band' case). Let a handful of rebuffers occur and get reported once (e.g. status.rebuffers reaches 3). Now toggle a third track's monitoring on, or toggle A off while B stays monitored — the device does not close (`self.monitored` is not empty), so `capture.monitor_rebuffers()` still returns 3 on the next status read, but line 927 has just reset `self.monitor_gaps` to 0.

**Mechanism.** `toggle_monitoring` (lines 920-936) unconditionally runs `self.monitor_gaps = 0;` right after any successful `set_track_monitoring` call. But `Session::close_input_if_idle` (auris-session/src/session/record.rs) only drops the input `Capture` — the object `capture.monitor_rebuffers()` counts against — once `self.monitored` is empty; with more than one track monitored, turning one of them on or off leaves the same `Capture` open, so `monitor_status().rebuffers` (auris-session/src/session/monitor.rs:140) keeps accumulating from before the toggle rather than resetting. `report_monitor_gaps` (transport_bar.rs:943-952) reports whenever `status.rebuffers > self.monitor_gaps`, so zeroing `self.monitor_gaps` while the true cumulative count is still positive makes the very next frame re-report the whole stale total as a brand-new event.

**Expected.** The counter should only be rebased to the device's actual current rebuffer count at the moment of the toggle (e.g. `self.monitor_gaps = self.session.monitor_status().map_or(0, |s| s.rebuffers)`), not hard-set to 0 unless the device was actually freshly reopened — matching what `report_monitor_gaps` itself does when `monitor_status()` goes `None` (line 945, which is the one case where 0 is actually correct because the device closed).

**Fix direction.** In `toggle_monitoring`, replace the unconditional `self.monitor_gaps = 0;` with a rebase to the device's actual current count: `self.monitor_gaps = self.session.monitor_status().map_or(0, |s| s.rebuffers);`. This makes 0 correct only when the device genuinely closed (status is None), matching what `report_monitor_gaps` already does on that path, while a still-open device (other tracks still monitored) keeps its true high-water mark instead of a fabricated zero.

### F-142 · medium · note_end_span's doc/test comment falsely claim None for notes <3px wide; the code only returns None at width <= 0.

`crates/auris-gpui/src/ui/piano_roll.rs:115` · spec-mismatch · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** No functional bug reaches the user: a note between 0 and 3 pixels wide still gets a (tiny, sub-pixel) resize grab zone rather than none, so resize still works. The consequence is purely for a future maintainer or reviewer: the doc comment on `note_end_span` and the matching comment in `the_resize_cursor_covers_what_a_press_would_actually_grab` assert a "<3px returns None" behavior that the code does not have and that the test does not actually check (it only exercises widths >= 3 plus the degenerate width=0 case), so anyone trusting the docs or the test coverage believes a guarantee exists that doesn't.

**Trigger.** `note_end_span(px(100.0), px(101.0))` (a 1px-wide note) returns `Some((px(100.667), px(0.333)))` rather than `None`; any width in the open interval (0, 3) reproduces this.

**Mechanism.** The doc on `note_end_span` (lines 113-114) states: "`None` for a note too narrow to grab, which is one drawn in less than three pixels." The window-test's own comment at line 2496 repeats the same claim: "A note drawn thinner than three pixels has no room for a zone that is not the whole note, and offers none." But `resize_grab` (line 106) computes `RESIZE_HANDLE.min(f32::from(width)/3.0)`, and `note_end_span` (line 117) only returns `None` when that result is `<= 0`, i.e. only when `width <= 0`. For any width strictly between 0 and 3 pixels, `width/3.0` is a small positive number, so `note_end_span` returns `Some(...)`, not `None`. The only case the accompanying test actually exercises is a literal zero-width note (`assert_eq!(note_end_span(px(100.0), px(100.0)), None)`, line 2498) -- the "<3px" claim is asserted in prose but never checked, and is false.

**Expected.** Either `note_end_span`/`resize_grab` should return `None` below a real width floor (e.g. `width < 3.0`) as documented, or the doc comment and the test's comment should be corrected to describe the actual `width <= 0` threshold.

**Fix direction.** Either fix the doc comment to state the true guard ("None only for a zero-or-negative-width note; otherwise the grab zone shrinks toward zero but is always present down to sub-pixel widths") and reword the test comment to match, or change `note_end_span`/`resize_grab` to actually floor out at 3px and add an assertion for a width strictly between 0 and 3 (e.g. 1.0px) proving it returns None. The cheaper, more honest fix is correcting the docs/comment and adding the missing width=1.0 test case to lock in real behavior.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-151 · medium · mod.rs's doc comment falsely claims all arrangement tests live in geometry.rs, when headers.rs and gestures.rs each carry their own test modules.

`crates/auris-gpui/src/ui/arrangement/mod.rs:6` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer reading crates/auris-gpui/src/ui/arrangement/mod.rs's module doc is told "every test in the module is in [geometry.rs]" and that geometry.rs is "the only part of the arrangement that can be exercised without a window." Trusting this, they would look only in geometry.rs when auditing or extending arrangement test coverage and miss the window-driven gpui::test suite in gestures.rs (8 tests exercising drag/press/release through the harness) and the plain #[test] functions in headers.rs (2 tests) — both of which also falsify the "only geometry can run without a window" premise, since headers.rs's tests are plain #[test] too.

**Trigger.** Read mod.rs's module doc, then `grep -n "mod tests" crates/auris-gpui/src/ui/arrangement/*.rs` — three files match, not one.

**Mechanism.** Lines 5-6 read: "...That layer is `geometry`, and it is the only part of the arrangement that can be exercised without a window, which is why every test in the module is in that file." This is false: `headers.rs` has its own `#[cfg(test)] mod tests` (line 624) with `the_resize_strip_leaves_a_header_worth_pressing` and `the_arm_button_shows_where_a_take_would_land_as_well_as_what_was_armed`, and `gestures.rs` has its own `#[cfg(test)] mod tests` (line 497) with six `#[gpui::test]` window-driven tests (`placing_an_automation_point_with_a_wobble_is_one_undo_step`, drag tests, etc.) that explicitly require a `TestAppContext`/window harness — the opposite of "exercised without a window". `git log -S"mod tests"` shows both test modules were added by commits `928df13` (headers.rs) and `a23792e` (gestures.rs), both dated after `577a7dd` ("Put the arrangement's geometry where a test can reach it"), the very commit that introduced this doc comment on the premise that geometry.rs held "twelve tests... all twelve of them exercising the same handful of free functions... the whole test module".

**Expected.** The doc should describe the current split (geometry.rs holds the pure-function tests; gestures.rs and headers.rs each carry their own, added later for behaviour that needs a window or app state) rather than asserting a single-file invariant that two later commits already broke.

**Fix direction.** Update the doc comment at crates/auris-gpui/src/ui/arrangement/mod.rs:5-6 to drop the false "every test in the module is in that file" claim — e.g. state that geometry holds the arrangement's pure/window-free tests while headers and gestures carry their own (headers's plain, gestures's window-driven via the harness).

**Written rule it breaks.** Every public item carries a doc comment (#![warn(missing_docs)] is on in each crate) — implying doc comments are meant to be accurate/maintained; the doc comment here is stale relative to the code it describes.

### F-156 · medium · apply_key returns KeyEffect::Changed for Backspace/Delete even when the caret is at a boundary and nothing was deleted, wrongly resetting palette selection / re-syncing on a no-op keypress.

`crates/auris-gpui/src/ui/text_field.rs:320` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Pressing Backspace with the caret already at the very start of a text field (or Delete at the very end) does nothing to the text, but the field still reports "Changed." In the command palette this resets the highlighted row back to the top of the results even though the query text and caret didn't move; in the lyrics editor it triggers a redundant re-sync. Nothing is lost or corrupted — the annoyance is losing your place in a list on a keypress that visibly typed nothing.

**Trigger.** In the command palette (crates/auris-gpui/src/ui/palette.rs:401-410, which reacts to `KeyEffect::Changed` by resetting `palette.selected = 0`): type a query that matches several rows, press Down to highlight a row other than the first, press Home to put the caret at position 0, then press Backspace once (nothing precedes the caret, so nothing is deleted).

**Mechanism.** `apply_key` (lines 294-321) unconditionally falls through to `KeyEffect::Changed` after calling `self.backspace()` or `self.delete_forward()`, without checking whether content actually changed. `backspace()` (lines 133-141) and `delete_forward()` (lines 143-152) are no-ops when the caret sits at an edge with nothing to consume (`previous_boundary`/`next_boundary` returns the same offset, producing an empty replace range), yet the caller still gets `KeyEffect::Changed` back.

**Expected.** `apply_key` should only return `KeyEffect::Changed` when the content actually changed (e.g. compare `self.content` before/after, or have `backspace`/`delete_forward` report whether their replace range was non-empty) and `KeyEffect::Moved` or `Ignored` otherwise, consistent with the documented contract.

**Fix direction.** In apply_key, capture content.len() (or selection) before calling backspace()/delete_forward() and compare after; return KeyEffect::Moved (or a new Ignored-like no-op) when it didn't change, Changed only when it did. Equivalently, have backspace()/delete_forward() return a bool indicating whether they mutated anything, and match on that in apply_key.

### F-169 · medium · select_clips's doc says primary joins the clip selection when absent from it, but the code discards primary in exactly that case instead of inserting it.

`crates/auris-gpui/src/app.rs:1807` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When a user rubber-band-selects clips while a different clip was previously focused (crates/auris-gpui/src/ui/selection.rs apply_rubber_band, primary = self.selected_clip), select_clips silently drops that previous clip from both the pointed-at editor and the resulting selection set if it isn't inside the new band. The editor jumps to an arbitrary clip (the BTreeSet's lowest ClipId) instead of keeping the previously-focused clip in the selection as the doc comment promises. No crash or corruption, but the doc comment actively misdescribes select_clips's behavior to future maintainers/reviewers relying on it.

**Trigger.** Call `select_clips(clips, Some(primary))` with `primary` not a member of a non-empty `clips`.

**Mechanism.** The doc reads: '`primary` joins the selection if it is not already in it, and is dropped when it is not one of them and the set is not empty.' (lines 1807-1808). The implementation (lines 1809-1815) is `self.selected_clip = match primary { Some(id) if clips.contains(&id) => Some(id), _ => clips.iter().next().copied() }; self.selected_clips = clips;`. `primary` is never inserted into `clips`/`self.selected_clips` under any branch — it is used as the pointed-at clip only when it is *already* a member of the given set, and is discarded (falling back to the set's first element) in every case where it is not already a member. The doc's first clause ('joins the selection if it is not already in it') describes the opposite of what happens, and its second clause restates the same 'not already in it' condition with the contradictory outcome 'dropped', making the comment self-contradictory as well as inaccurate. Confirmed via `git log -p` that this doc text was introduced verbatim alongside this exact code, so it is not stale drift from a later change — it never matched.

**Expected.** The comment should describe the actual rule — primary points the editors only when it is already a member of the given set; otherwise it is dropped and the first member of the set (if any) is used — or, if 'joins' was the intended behaviour, the code should insert primary into the set.

**Fix direction.** Either fix the doc to say primary is dropped (never inserted) whenever it's absent from clips, matching the code, or fix the code to match the doc by inserting primary into selected_clips when it's Some and not already a member. Given apply_rubber_band's real intent (keep the previously-focused clip visible during a sweep), the code fix — `if let Some(id) = primary { clips.insert(id); }` before assigning selected_clip/selected_clips — is likely the correct direction, not just the doc.

**Written rule it breaks.** select_clips doc comment (app.rs:1806-1808): "`primary` joins the selection if it is not already in it, and is dropped when it is not one of them and the set is not empty."

### F-171 · medium · every_target()'s doc comment falsely claims exhaustiveness is enforced; it's a plain Vec literal already missing 6 of 26 PromptTarget variants.

`crates/auris-gpui/src/ui/prompt.rs:1852` · test-quality · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No end user ever sees this — it's a false guarantee in a test-only doc comment. The practical harm falls on a future contributor/reviewer at crates/auris-gpui/src/ui/prompt.rs:1852: they trust the comment's claim that every_target() is kept exhaustive by construction, so they skip auditing it when adding a PromptTarget variant. The test everything_that_is_a_notation_rather_than_a_name_says_so (line ~1894) then silently exercises only the 20 listed variants and passes even though 6 real variants (Lyric, Phonemes, Lyrics, ComposeLyrics, Param, ClipSourceTempo) — confirmed already missing today — get zero hint-behavior coverage.

**Trigger.** Add a new `PromptTarget` variant, or simply note that `ClipSourceTempo`/`Param`/the three lyric variants already exist and are silently absent — no build or test failure results.

**Mechanism.** The doc comment on `every_target()` (line 1852) asserts "Every target, so a new one cannot be added without this file being opened." The hand-written `Vec` literal (lines 1853-1876) lists only 20 of the enum's 26 variants, omitting `Lyric{..}`, `Phonemes{..}`, `Lyrics{..}`, `ComposeLyrics`, `Param(_)` and `ClipSourceTempo`. Because it is a plain function returning a `Vec` (not an exhaustive `match`), the compiler enforces nothing — a variant can be added to `PromptTarget` (or, as already happened, six already were) without ever touching this function or its test.

**Expected.** Either make the enumeration compiler-enforced (e.g. drive the test from an exhaustive `match target { ... }` so a new variant is a compile error until handled) or update the doc comment to say which variants are deliberately excluded and why, so the claim matches what the function actually does.

**Fix direction.** Replace the hand-written Vec with something the compiler actually ties to the enum: either build every_target() from a `match target { PromptTarget::Track(_) => ..., ... }`-style exhaustive helper (so a new variant is a compile error until added), or add a `#[test]` that pattern-matches a dummy value through a `match` with no wildcard arm to force enumeration, or at minimum rewrite the doc comment to say what's true ("a partial list; add new variants here manually — nothing enforces completeness") and add the 6 missing variants now.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs". (general project ethos that tests should verify real coverage, not just look like they do — the doc comment here overstates what the test structurally guarantees)

### F-174 · medium · Test named for cycling the grid only asserts a static fact about GRID_CHOICES and never calls cycle_grid or grid_label.

`crates/auris-gpui/src/ui/transport_bar.rs:1319` · test-quality · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A developer reading the test suite sees a passing test named `the_grid_can_be_cycled_all_the_way_off_and_round_again` and reasonably concludes grid cycling (including wrap-around back to 1/1 and the off state) is verified. It is not: a regression that breaks the `(index + 1) % GRID_CHOICES.len()` wraparound, or corrupts the `unwrap_or(2)` fallback used when the current grid value isn't found in the table, would ship silently — `cycle_grid` and `grid_label` are called by zero tests anywhere in the crate, only from production click/keyboard handlers.

**Trigger.** Any regression in `cycle_grid`'s own logic — e.g. changing `(index + 1) % GRID_CHOICES.len()` to something that skips the off state, or breaking the `unwrap_or(2)` fallback — would leave this test green, because the test body cannot observe `cycle_grid`'s behavior at all.

**Mechanism.** The test `the_grid_can_be_cycled_all_the_way_off_and_round_again` (lines 1319-1328) is: `let ticks: Vec<i64> = GRID_CHOICES.iter().map(|(_, ticks)| *ticks).collect(); assert!(ticks.contains(&1), "one tick is as fine as the document gets");`. It never constructs an `AurisApp` and never calls `AurisApp::cycle_grid` (lines 1017-1025) or `grid_label` (lines 1032-1039) — the only two functions in the file that actually implement cycling. It only checks a static property of the `GRID_CHOICES` table.

**Expected.** The test's own name promises to verify that cycling reaches every choice and wraps at the end; it should call `cycle_grid` (via a constructed `AurisApp`/session) or, if that is impractical without a window, its assertion should be renamed to describe what it actually checks (that `GRID_CHOICES` contains the finest division) rather than claiming to test the cycle.

**Fix direction.** Rewrite the test to actually exercise `AurisApp::cycle_grid` — construct or drive it via `auris_gpui::harness` (the mechanism CLAUDE.md prescribes for exactly this: "A gesture is made as a gesture ... and the document is asked what happened"), call `cycle_grid` GRID_CHOICES.len() times, and assert the grid value returns to its start and that `grid_label` reports the off state correctly partway through. At minimum, rename the current test to reflect what it actually checks (that 1 tick is present in the table) and add a separate test that calls `cycle_grid`.

**Written rule it breaks.** The window is testable. `auris_gpui::harness` opens the whole application ... and drives it from `cargo test` — real keymap, real view tree, real session. A gesture is made as a gesture (press, move, release) and the document is asked what happened.

### F-189 · medium · Re-clicking the already-selected output device row zeroes sample_rate and forces an unwanted audio restart.

`crates/auris-gpui/src/settings_window.rs:922` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a user has picked a non-default output sample rate and then re-clicks the output device row that is already selected (e.g. to re-confirm it, or an accidental double click), the sample rate silently reverts to auto/default and the audio output stream is torn down and reopened — an audible interruption/restart the user did not ask for, with no dialog or visible change to justify it since the device itself did not change.

**Trigger.** In Settings' Audio tab, pick an output device and an explicit non-default sample rate (e.g. 96 kHz) for it, then click that same, already-selected device row again (e.g. a mis-click, or clicking to re-check its detail line).

**Mechanism.** `device_row`'s click handler always forces `sample_rate: None` for an output row, with no check for whether the clicked device is already the selected one: `.on_click(cx.listener(move |this, _, _, cx| { let audio = match slot { DeviceSlot::Output => AudioPreferences { device: device.clone(), sample_rate: None, ..this.audio.clone() }, ... }; this.apply_audio(audio, cx); }))` (lines 917-933). `selected` (line 872-875) is computed only for styling, never consulted before firing the update. Downstream, `output_changed` (`crates/auris-session/src/session/mod.rs:1103-1107`) compares `before.sample_rate != after.sample_rate`; if the previously chosen device already had an explicit non-default rate (e.g. 96 kHz), sending `sample_rate: None` makes `output_changed` true even though `device` did not change, so `Session::set_audio_preferences` takes the full teardown/reopen branch (drops the current device, calls `start_audio` again) purely because of a re-click.

**Expected.** The click handler should compare against the currently selected device (`selected`) and skip resetting `sample_rate` — or skip calling `apply_audio` entirely — when the clicked device is already the one in `self.audio.device`, matching the code's own stated rationale ("The old rate may not exist on the *new* device", line 921) which only applies when the device is actually changing.

**Fix direction.** In device_row's on_click closure for DeviceSlot::Output, don't unconditionally null out sample_rate: only clear it when the clicked device actually differs from this.audio.device (mirror the `selected` check already computed above the closure), otherwise keep this.audio.sample_rate so re-clicking the current device is a no-op.

### F-190 · medium · An open Settings window keeps showing the old language/colour scheme when it is changed elsewhere (e.g. the command palette), until closed and reopened.

`crates/auris-gpui/src/settings_window.rs:65` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** If the Settings window is already open and the user changes the interface language or colour scheme through another surface (the command palette calls `AurisApp::apply_language`/`apply_scheme` directly at crates/auris-gpui/src/ui/palette.rs:336,349), the rest of the app repaints in the new language/theme but the open Settings window keeps showing its stale snapshot — old-language labels and old-theme colours — until the user closes and reopens it. No data is lost or corrupted; it is a stale-display bug confined to one already-open window.

**Trigger.** Open Settings (General tab stays visible on screen). Switch focus to the main window, open the command palette, and choose a different interface language or colour scheme from it. Switch back to the still-open Settings window.

**Mechanism.** `SettingsWindow` snapshots `language_preference`/`language` (lines 64-67) and `theme` (line 58) exactly once, in `new()` (lines 116-151, e.g. `language: Language::resolve(language_preference)` at line 140), and never re-reads `AurisApp`'s live state afterward — it only ever pushes its own edits outward (`apply_language`/`apply_scheme`, lines 219-240). The window's own comment at lines 230-234 shows the author is aware of exactly this class of bug for one direction only: "This window holds its own copy of the palette rather than reading the application's, so it has to be told as well — otherwise the change would be visible everywhere except in the window where it was made." But the same two preferences are also editable from a second, independent surface: the main window's command palette dispatches `PaletteCommand::Language(language) => self.apply_language(Some(language), cx)` and `PaletteCommand::Scheme(scheme) => self.apply_scheme(scheme.id)` directly on `AurisApp` (`crates/auris-gpui/src/ui/palette.rs:349` and `:336`), with no notification path back into an already-open […]

**Expected.** The Settings window should either subscribe to `AurisApp`'s language/theme (so external changes propagate in), or `open_settings` should refresh the state of an already-open window (not just call `activate_window()`) when it is brought forward, so the window it shows is never allowed to diverge from the value it is representing.

**Fix direction.** Give `SettingsWindow` a way to be told about externally-originated language/theme changes — e.g. have `AurisApp::apply_language`/`apply_scheme` also push the new value into `self.settings_window` (via the `WindowHandle`, mirroring how `SettingsWindow::apply_language`/`apply_scheme` already push back into `AurisApp`) so both directions of the link are covered, not just settings-window-as-origin.

**Written rule it breaks.** This window holds its own copy of the palette rather than reading the application's, so it has to be told as well — otherwise the change would be visible everywhere except in the window where it was made. (crates/auris-gpui/src/settings_window.rs, doc comment on `apply_scheme`)

### F-191 · medium · song_dials rebuilds SongDials::charts via BTreeMap::iter(), so reopening a project resorts extra charts alphabetically instead of preserving the order they were added in.

`crates/auris-gpui/src/ui/compose_sheet/dials.rs:244` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user who added multiple non-main chord progressions to a song sees them listed in a different (alphabetical) order after saving and reopening the project than the order they originally added them in the compose sheet's chart picker/list. No chord data, notes, or audio is corrupted -- only the display order of entries in the UI changes.

**Trigger.** In the sheet, give one section a catalogue progression whose name sorts alphabetically *after* one given to another section later — e.g. choose 'marusa' for the chorus first, then choose 'circle-of-fifths' for the bridge second, so `dials.charts == [main, marusa, circle-of-fifths]`. Press Write (stores the spec) and reopen the song sheet (`open_song_sheet` re-parses the stored spec and calls `song_dials`).

**Mechanism.** `SongDials::charts` is documented as an ordered `Vec<(String, Chart)>` specifically so the sheet is a 'list ... a person edits' rather than a map whose rows 'slide ... somewhere else in the panel' (dials.rs:106-111), and `give_section_chart`/`set_section_chart` (dials.rs:403-436) always push a newly-chosen catalogue chart onto the *end* of that Vec, preserving the order the user picked them in. But `song_dials` — the function that rebuilds `SongDials` from a `SongSpec`, used on every reopen of the sheet (`opening_dials`, dials.rs:220-231, called from `open_song_sheet` in view.rs:42-53) — reconstructs `charts` via `spec.charts.iter()` (dials.rs:244-249) where `spec.charts` is a `BTreeMap<String, Chart>` (auris-compose/src/spec/mod.rs:463). `BTreeMap::iter()` yields entries in ascending key (alphabetical) order, which is unrelated to the order the user added them in. Unlike `sections`, which is correctly re-derived from the ordered `form` Vec (dials.rs:251-262), `charts` has no such ordered backing field in the file format, so its original order is lost on every round trip through a […]

**Expected.** Reopening a saved sheet should reproduce the chart list in the order the user built it, the same way `sections` is reconstructed from the ordered `form` field rather than from a map — or the doc comment's stated reason for keeping `charts` a `Vec` (dials.rs:106-111) no longer holds once the sheet is reloaded from a file.

**Fix direction.** Give SongSpec an ordered backing field for chart order (e.g. a Vec<String> naming chart order, analogous to `form` for sections) and have song_dials rebuild the extra-charts portion from that order instead of BTreeMap::iter(), falling back to insertion/alphabetical order only for specs saved before the field existed.

**Written rule it breaks.** dials.rs:106-111: sections and charts "are ordered lists here and maps there, because a list is what a person edits"; and song_dials's own doc comment (dials.rs:234-238): "What makes the round trip hold is that this normalises... Every list the sheet can produce is already in that shape, because every gesture it offers preserves it."

### F-199 · medium · Holding both the create and delete modifiers on empty piano-roll grid always creates a note because CommandClick/OptionClick::matches ignore each other's flag.

`crates/auris-gpui/src/gestures.rs:117` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In the piano roll, pressing on empty grid while holding both the create modifier (Cmd/Ctrl by default) and the delete modifier (Alt/Option by default) — e.g. from mis-timed key release or a thumb resting on the wrong key — always creates a new note instead of doing nothing or starting a rubber-band selection, because CommandClick.matches() and OptionClick.matches() each look at only their own modifier flag and both report true simultaneously.

**Trigger.** Under the default gesture configuration (create = CommandClick, delete = OptionClick), hold both the secondary/command modifier and the option/alt modifier at once and left-click empty space in the piano roll's note grid.

**Mechanism.** `PointerGesture::matches` guards `Click` against every other modifier (`event.click_count == 1 && !event.modifiers.secondary() && !event.modifiers.alt && !event.modifiers.shift`, lines 111-116) but the two modifier gestures do not guard against each other: `PointerGesture::CommandClick => event.modifiers.secondary(),` (line 117) and `PointerGesture::OptionClick => event.modifiers.alt,` (line 118) each look at only their own modifier. `empty_press` (gestures.rs lines 176-184) then does `if gestures.create.matches(event) { EmptyPress::Create } else { EmptyPress::Band { extend: event.modifiers.shift } }` — it never checks `gestures.delete.matches(event)`. In `AurisApp::begin_note_drag` (piano_roll.rs), the delete check at lines 1062-1069 only runs when `under_pointer` is `Some`, i.e. never for a press on empty grid, so nothing there catches the conflict either.

**Expected.** Consistent with `PointerGesture::Click`'s own handling of the other modifiers, `CommandClick` and `OptionClick` should not both answer true for the same press — at minimum, `empty_press` should not report `Create` when the *delete* gesture's modifier is also held, the way `Click` already refuses a press carrying any other gesture's modifier.

**Fix direction.** Make CommandClick and OptionClick mutually exclusive in PointerGesture::matches, the same way Click already refuses every other modifier: `CommandClick => event.modifiers.secondary() && !event.modifiers.alt` and `OptionClick => event.modifiers.alt && !event.modifiers.secondary()`. That alone fixes empty_press (which only calls gestures.create.matches) without needing a separate delete-aware check.

**Written rule it breaks.** Test comment in crates/auris-gpui/src/gestures.rs: "one press must not be both gestures at once" (a_modified_double_click_belongs_to_the_modifier), and the doc note above PointerGesture::Click's match arm: "Every modifier is refused rather than only the two that name gestures."

### F-203 · medium · opening_window_bounds checks only cx.primary_display(), so a window remembered on a secondary monitor is always recentred instead of restored, even though gpui's cx.displays() enumerates every connected display.

`crates/auris-gpui/src/main.rs:197` · platform · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user with two or more monitors who moves the Auris Studio window onto a secondary display and closes the app there gets it recentred onto the primary display on next launch, instead of reopening where they left it — every time, on any multi-monitor setup, until they move it back. The window is never lost or unreachable (recentring onto the primary display is a safe fallback), so this is an annoyance, not data loss or an unusable app.

**Trigger.** A dual/multi-monitor setup (very common on Windows desktops) where the application window was moved to and closed on a non-primary monitor. On the next launch the remembered rectangle sits entirely outside the primary display's bounds even though the monitor it was really on is still plugged in and unchanged.

**Mechanism.** `opening_window_bounds` (line 196) and `restorable` (line 210) test the remembered `WindowPlacement` for overlap against `cx.primary_display()` only (lines 197, 235), never against `cx.displays()` (which gpui exposes — `~/.cargo/registry/src/.../gpui-0.2.2/src/app.rs:999`, returning every connected display). `restorable` returns `None` — meaning "recentre on the primary screen" — whenever the remembered rectangle does not overlap the primary display's bounds by at least `RESCUABLE`, regardless of whether it overlaps a still-connected secondary display.

**Expected.** Per the doc comment's own framing ("the rectangle has to still overlap **the desktop**"), overlap should be checked against the union of `cx.displays()`, not just `cx.primary_display()`, so a window left on any still-connected monitor reopens there.

**Fix direction.** In `opening_window_bounds`, iterate `cx.displays()` and test `restorable` against each display's bounds (or the display whose bounds actually contain/overlap the remembered rectangle), falling back to the primary display recentre only if no connected display overlaps. `restorable` already takes a single `Option<Bounds<Pixels>>`, so the caller just needs to try multiple displays.

### F-204 · medium · gestures.rs:433 uses `#[cfg(not(target_os = "macos"))]` instead of `cfg!`, so the non-macOS modifier assertions never compile on a macOS `cargo test` run, violating CLAUDE.md's explicit cfg!/#[cfg] rule.

`crates/auris-gpui/src/gestures.rs:433` · architecture · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No end user impact — this is a test-only style violation. A developer running `cargo test -p auris-gpui --bins` on macOS never compiles or executes the Windows/Linux modifier assertions (and vice versa on CI), so a regression in how `PointerGesture::CommandClick` reads Ctrl/Windows-key modifiers on non-macOS platforms could land without any test catching it on the macOS dev machine, silently rotting until Windows CI (or a real user on Windows) hits it.

**Trigger.** Run `cargo test -p auris-gpui --bins` on macOS (the project's stated development platform): the assertions that `PointerGesture::CommandClick` matches `Modifiers::control()` and does not match `Modifiers::windows()` never compile or execute on that machine. The reverse happens on Windows/Linux CI, where the `Modifiers::command()` assertion never runs there either.

**Mechanism.** The test `the_create_gesture_uses_a_key_this_platform_actually_has` guards its Windows/Linux assertions with `#[cfg(not(target_os = "macos"))]` (line 433) and its macOS assertion with `#[cfg(target_os = "macos")]` (line 438), so only one branch is ever *compiled* for a given build target. CLAUDE.md's Platforms section states the rule in exactly this scenario: "Decide with `cfg!`, not `#[cfg]`, wherever it is a choice rather than an API that only exists on one platform. Both arms then compile and their tests run everywhere, which is the only reason the Windows menu bar can be checked from a Mac." `Modifiers::control()`, `Modifiers::windows()` and `Modifiers::command()` are just plain struct values (not platform-gated APIs), so there is no technical reason this couldn't use `cfg!()` like the very next test in the same file (`the_labels_name_keys_this_platform_has`, which does use `cfg!(target_os = "macos")` correctly).

**Expected.** Per CLAUDE.md ("Platforms" section) and the sibling test in the same file, the assertions should be unconditional or guarded with `if cfg!(target_os = "macos") { ... } else { ... }` so every branch is compiled and exercised on every platform's test run.

**Fix direction.** Rewrite the guarded block in `the_create_gesture_uses_a_key_this_platform_actually_has` (gestures.rs:428-440) to use `if cfg!(target_os = "macos") { ... } else { ... }` instead of `#[cfg(target_os = "macos")]`/`#[cfg(not(target_os = "macos"))]`, matching the pattern already used two tests below it (`the_labels_name_keys_this_platform_has`) — this makes both branches compile and type-check on every platform, catching API-surface breaks regardless of which OS runs `cargo test`.

**Written rule it breaks.** Decide with `cfg!`, not `#[cfg]`, wherever it is a choice rather than an API that only exists on one platform. Both arms then compile and their tests run everywhere, which is the only reason the Windows menu bar can be checked from a Mac.

### F-211 · medium · selected_phonemes doc-comments "the grabbed note" but reads the lowest-indexed note in a BTreeSet, mismatching pitch and lyric during multi-note shift-click drags.

`crates/auris-gpui/src/app.rs:2049` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When a user shift-clicks (adds) an unselected note to a multi-note selection and drags it, the audible pitch-preview plays that note's pitch but sings the syllable belonging to whichever selected note has the lowest index in the clip — not the syllable of the note the user is actually grabbing/dragging. The preview can therefore say the wrong word for the pitch being auditioned, misleading the user about what lyric is attached to the note they're moving.

**Trigger.** In a singer track's piano roll: click note A (lower on-screen index, e.g. index 2) to select it, then shift-click note B at a higher index (e.g. index 5) to add it to the selection and grab it for a move. `selected_notes` is now `{2, 5}`; the drag starts on note B's pitch, but `selected_phonemes` returns index 2's (note A's) phoneme list because it is the smaller of the two.

**Mechanism.** `selected_phonemes` is documented as "The syllable the grabbed note carries, read off the primary selection" (app.rs:2033), but it implements that as `self.selected_notes.iter().next()` (line 2049) — the smallest index in the `BTreeSet<usize>`, not the note that was actually pressed. `begin_note_drag` in ui/piano_roll.rs (lines 1092-1142) adds the just-pressed note's index into `self.selected_notes` with `.insert(index)` on a shift-click (line 1103) without clearing the rest of the set, then calls `self.audition(pitch)` (line 1141) for the just-grabbed note's pitch, which reaches `wish_sung_preview` → `self.selected_phonemes(track)`.

**Expected.** The function should read phonemes off the note actually being dragged (the index passed into the surrounding `Drag::NoteMove`/audition call), not off `BTreeSet::iter().next()`, to match its own doc comment's claim of reading "the grabbed note."

**Fix direction.** Either track which note was last grabbed (e.g. a `primary: Option<usize>` field alongside `selected_notes`, mirroring the existing `primary: Option<ClipId>` pattern used for clips) and have `selected_phonemes` read that note's phonemes, or narrow the doc comment to say it reads the lowest-indexed selected note rather than the grabbed one — the fix should make behavior and documentation agree, and matching the doc's stated intent (grabbed note) is the more correct fix given `audition` already uses the grabbed note's pitch.

**Written rule it breaks.** /// The syllable the grabbed note carries, read off the primary selection.

### F-212 · medium · TEMPO dial doc claims to cover the spec's accepted tempo range (20..400) but only covers 40..220, silently clamping/discarding legal out-of-range tempos on first drag.

`crates/auris-gpui/src/ui/compose_sheet/dials.rs:21` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A song whose tempo is outside 40..220 BPM (legal per the spec's 20..400 range, and per the transport's own 10..999 bound) opens the compose sheet with the tempo dial pinned to one end and showing a value that doesn't match the document; the first drag then snaps the tempo into the 40..220 window, silently discarding the original out-of-range value instead of letting the user fine-tune from where it actually is.

**Trigger.** Set the project's transport tempo to something outside 40-220 (e.g. 30 BPM, reachable via the timeline's own tempo drag, which the engine accepts down to 10 BPM), open the compose sheet, and drag the Tempo bar by even one pixel.

**Mechanism.** The doc comment on `TEMPO` (dials.rs:20) reads '/// The tempo range the specification accepts, which is what the dial has to cover,' backing `pub const TEMPO: std::ops::RangeInclusive<f64> = 40.0..=220.0;` (line 21). But `SpecDoc::into_spec` accepts `(20.0..=400.0).contains(&tempo)` for the song's own tempo and for a section's pinned tempo (crates/auris-compose/src/spec/doc.rs:456-461, 700-707), and the live transport (`TempoMap::MIN_BPM = 10.0`, `MAX_BPM = 999.0`, crates/auris-core/src/time.rs:743-791) can already have the project at a tempo outside 40-220 before the sheet is even opened — `opening_dials` (dials.rs:220-231) copies that tempo straight into `dials.tempo` unclamped. `SongDial::Tempo::fraction` (dials.rs:713-717) clamps the bar's position via `between()` (which itself `.clamp(0.0, 1.0)`s), and `SongDial::Tempo::set` (dials.rs:738-743) always resolves a drag through `lerp(fraction, 40.0, 220.0)`, so the very first pixel of any drag recomputes `dials.tempo` from a fraction that was already pinned to 0 or 1.

**Expected.** Either the doc comment's claim should be true (TEMPO widened to match what `SpecDoc::into_spec` actually accepts, i.e. 20.0..=400.0), or, if 40-220 is a deliberate UI restriction, the comment should say so rather than asserting equivalence with the specification's own accepted range, and a typed-tempo escape hatch (mirroring the meter field) should exist so an out-of-range document tempo can be read and edited without the dial silently renormalizing it.

**Fix direction.** Either widen `TEMPO` to `20.0..=400.0` to match `SpecDoc::into_spec`'s actual accepted range, or, if 40..220 is intentionally a narrower UI-friendly window, correct the doc comment to say so and make `opening_dials`/`set` clamp-and-preserve (or otherwise not silently discard) an incoming tempo outside that window.

**Written rule it breaks.** /// The tempo range the specification accepts, which is what the dial has to cover.

**Verifier's correction.** No correction needed; the claim's mechanism, location, and numeric comparison (dial covers 180 of the specification's 380-wide accepted range, i.e. less than half) are accurate as stated.

### F-224 · medium · Phoneme-divider click acceptance (grabbed_boundary_at, piano_roll.rs:1381) uses a full PHONEME_GRAB radius while the drawn cursor hitbox (phoneme_divider_zones, lines 1309-1319) uses only half that, so presses 2.5-5px from a cut retime a phoneme with no cursor cue.

`crates/auris-gpui/src/ui/piano_roll.rs:1381` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Clicking and dragging inside a singer-clip note, 2.5-5px from a phoneme divider, shows the ordinary note cursor (implying a whole-note move) but the press actually grabs the phoneme boundary and silently re-times that phoneme via set_phoneme_duration — changing what the voice sings at that syllable with no visual cue that a different gesture was performed.

**Trigger.** On a singer clip, open a note with 2+ phonemes so a divider is drawn inside it (e.g. the `dragging_a_phoneme_divider_pins_its_length` fixture: a note singing "か"). Press and drag starting 3-4 pixels from the phoneme cut but still inside the note body, at a point where `note_end_span`'s resize zone does not also claim it.

**Mechanism.** `phoneme_divider_zones` (lines 1309-1319) draws the resize-arrow hitbox as a box `PHONEME_GRAB` (5px) wide centred on the cut: `origin: point(x - px(PHONEME_GRAB / 2.0), ...), size: size(px(PHONEME_GRAB), ...)` — i.e. the cursor only changes within ±2.5px of the divider. But the code that actually decides whether a press takes hold of the divider, `grabbed_boundary_at` (lines 1367-1388), computes `slack` as the seconds spanned by a full `PHONEME_GRAB`-pixel offset: `let slack = (tempo.ticks_to_seconds(self.timeline.x_to_tick(x + px(PHONEME_GRAB))).0 - at).abs();` and `grabbed_phoneme_boundary` (called at line 1386) accepts any press within `slack_seconds` on either side of the cut — a symmetric ±5px window, twice the width of the zone that lit up the arrow. So a press landing 2.5-5px from a cut shows the ordinary note cursor (implying 'this will move the note') but actually begins `Drag::PhonemeDuration`, re-timing the phoneme boundary instead.

**Expected.** The doc comment for `note_end_zones`/the paired test `the_resize_cursor_covers_what_a_press_would_actually_grab` establishes the project's own rule for the analogous resize handle: 'every pixel the arrow lights up has to be one where the press takes the resize branch... A zone that ran past the end would promise a grab on empty grid.' The phoneme-divider hitbox should use the same radius the click logic actually accepts (or the click logic should use the same width as the drawn zone) so the […]

**Fix direction.** Make grabbed_boundary_at's slack match the drawn zone's half-width (PHONEME_GRAB/2 each side, i.e. use x + px(PHONEME_GRAB/2.0) when computing slack) instead of a full PHONEME_GRAB offset, so the accepted press window is exactly as wide as the box that lights the resize cursor.

**Written rule it breaks.** every pixel the arrow lights up has to be one where the press takes the resize branch... A zone that ran past the end would promise a grab on empty grid. (project's own stated rule for the analogous note-end resize handle, via note_end_zones' doc comment and the paired test the_resize_cursor_covers_what_a_press_would_actually_grab)

### F-232 · medium · A press in the fade band beyond ~6px of the actual (moved) fade handle silently resizes the clip with no hover cursor ever shown there.

`crates/auris-gpui/src/ui/arrangement/geometry.rs:257` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** When a clip's fade-in or fade-out has been dragged more than ~6px away from the clip's corner (an ordinary fade on any clip wider than ~40px), the strip of the fade band nearest the clip edge shows no cursor affordance at all on hover — edge_zone_rows deliberately excludes that whole band from every resize hitbox — yet a mouse press there is not caught by fade_handle_at (it only claims a 6px window around the real handle) and falls through to clip_grab_at, which (per its own doc, "at any height") grants a resize regardless of y. The user clicks on what looks like dead space between the fade handle and the clip edge and the clip silently trims its start or end, an undiscoverable mutation with no visual cue that a press there would do anything.

**Trigger.** An audio clip drawn wider than `FADE_HANDLE_MIN_WIDTH` (40px) with a fade-in covering more than roughly `FADE_GRAB / width` of its frames (e.g. a 4-beat clip at 48px/beat = 192px wide, 48000 frames, fade_in_frames > ~1500 — a very ordinary fade). The user presses at the clip's left edge (`x` within ~7px of `start_x`) at a `y` inside the fade band (roughly 14-26px below the clip's top, i.e. `TITLE_HEIGHT..TITLE_HEIGHT+FADE_BAND`), away from where the now-relocated fade-in handle actually is.

**Mechanism.** `edge_zone_rows` (geometry.rs:150-176) deliberately omits the fade band `[top+TITLE_HEIGHT, top+TITLE_HEIGHT+FADE_BAND)` from the resize-cursor rows it returns, on the stated assumption (line 153: "a press there falls past the fade check, which wants a `y` inside the band") that any press in that band is claimed by a fade handle. But `fade_handle_at` (geometry.rs:187-221) only claims a narrow slice: it requires the press to land within `FADE_GRAB` (6px, line 78) of the *actual* fade-in/out x position, which moves away from the clip's corners as `fade_in_frames`/`fade_out_frames` grow (line 208-209: `in_x = left + width * fade_in/frames`, `out_x = left + width * (1 - fade_out/frames)`). Meanwhile `clip_grab_at` (geometry.rs:250-269), which is checked next in `begin_lane_drag` (gestures.rs:322-362) whenever `fade_grab_at` (gestures.rs:93-114, called at gestures.rs:322) returns `None`, applies its resize check `within(start_x)`/`within(end_x)` (geometry.rs:262-267) with **no y restriction at all** — its own doc (geometry.rs:240-243) says resize works "at any height", confining only the […]

**Expected.** Per the module's own stated goal (geometry.rs comment on `clip_edge_zones`/`edge_zone_rows`, and the file-level doc's "a hit test measured from one number and a painter from another is... a bug nobody sees and everybody feels"), the cursor a hover shows and the gesture a press performs should agree. Either `edge_zone_rows` should include a resize row for the parts of the band a real fade handle does not currently occupy, or `clip_grab_at`'s resize check should stay clear of the band the way its […]

**Fix direction.** Make edge_zone_rows and fade_handle_at agree by construction: either have edge_zone_rows compute the actual fade-handle x position (mirroring fade_handle_at's in_x/out_x math) and only carve out the ±FADE_GRAB slice around it, showing a resize cursor over the rest of the band; or have clip_grab_at's Resize checks exclude the fade band's y-range (mirroring how its Loop check is confined to TITLE_HEIGHT) so a press there is a no-op unless it actually lands on the fade handle. The first option preserves more resize surface; either restores the file's own stated invariant that the hover cursor and the press gesture agree.

**Written rule it breaks.** "The constants are shared rather than merely convenient. A hit test measured from one number and a painter from another is how a grab bar ends up a pixel to the left of the bar it is drawn on, which is a bug nobody sees and everybody feels." (geometry.rs file doc) — and edge_zone_rows's own comment claims "a press there falls past the fade check, which wants a `y` inside the band," which is false […]

### F-233 · medium · Agent transcript panel never auto-scrolls on new entries, so users must manually scroll down after every turn to see the latest message.

`crates/auris-gpui/src/ui/agent_chat.rs:939` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** During any agent-panel conversation longer than the visible viewport, new messages (including the user's own just-sent text and the model's reply) are appended below the fold and the panel does not scroll to show them; the user must notice the panel is scrollable and manually drag the scrollbar down after every turn to read the latest exchange.

**Trigger.** Have any conversation with the agent panel long enough that the transcript's total height exceeds the panel's viewport (a handful of tool-call rows and replies), while the scrollbar is at its default top offset (or anywhere short of the very bottom).

**Mechanism.** The transcript is rendered oldest-first, growing downward (`self.agent_chat.entries.iter().enumerate()` at line 862-868, laid out top-to-bottom in a `flex_col` at lines 939-973), and scrolled through `self.scrolling(ScrollPanel::Agent, ..., cx)`, which in crates/auris-gpui/src/ui/scrollbars.rs:188-209 only calls `body.track_scroll(handle)` — it keeps whatever offset `self.agent_chat.scroll` (a bare `gpui::ScrollHandle`) already has. Nothing in agent_chat.rs, commands.rs or scrollbars.rs ever moves that offset when a new entry is pushed (`entries.push` sites: lines 413, 448, 453/462/464, 481, 485, 490, 679). Contrast with the arrangement's own "follow the playhead" behaviour, which explicitly calls `self.timeline.scroll_to_reveal(playhead, width)` (root.rs:56) to keep something new in view — no equivalent call exists for the agent scroll handle. The log panel, by contrast, deliberately avoids the whole problem by putting new entries at the *top* (log_panel.rs:31-36, comment at lines 84-86: "Newest first ... scrolling to the bottom of five hundred lines to find it is the terminal's […]

**Expected.** A chat transcript that grows should keep the newest entry in view (or at least move there when a new entry is appended while the view was already at the bottom), the way the arrangement's playhead-follow or the log panel's newest-first ordering already solve the same class of problem elsewhere in this codebase. This class of bug is also outside what the project's own headless window harness can catch, since CLAUDE.md documents that harness as unable to assert on pixels or scroll position, which […]

**Fix direction.** In AgentChat::absorb (and the direct entries.push in the send path), after appending a new entry call self.scroll.scroll_to_bottom() (gpui 0.2.2 exposes this on ScrollHandle) — or track whether the view was already at the bottom before the push and only call it then, to avoid yanking the view away from a user who scrolled up to read history.

### F-246 · medium · AgentChat::entries (agent_chat.rs:271) is never capped or evicted and AgentEvent::Result rescans it with iter_mut().rev().find(), so a long or misbehaving agent session grows memory unboundedly and slows/stalls the UI thread, unlike the crate's own capped Logbook.

`crates/auris-gpui/src/ui/agent_chat.rs:271` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In a long agent-panel session (or one where an OpenAI-compatible endpoint the user pointed the panel at streams stray/unmatched "result" events), the chat transcript vector grows forever and each unmatched Result event triggers a reverse linear scan of the whole history, so memory use climbs without limit and the DAW's UI thread (not the audio thread) becomes progressively slower and can stall while absorbing a backlog of tool events.

**Trigger.** A single agent turn — or, more simply, an ordinary long-running session in the agent panel — that involves many tool calls (an agentic edit/compose loop calling a tool per note or per track is exactly the feature's own use case), or a misbehaving/malicious OpenAI-compatible endpoint the user pointed the panel at that streams a very long sequence of `{"event":"call",...}`/`{"event":"result",...}` lines. Each `Tool` entry's `detail` field is also unbounded in size (the whole tool answer, untruncated), so even a modest number of large tool outputs adds up.

**Mechanism.** `AgentChat::entries: Vec<ChatEntry>` (line 271) is only ever pushed to (grep of the file shows 9 push sites at lines 413, 448, 462, 464, 481, 485, 490, 662, 679) and is never truncated, capped or cleared anywhere in this file or its `Default` impl — unlike the sibling `Logbook` in the same crate, which explicitly bounds itself with `pub const CAPACITY: usize = 500;` and evicts the oldest entry once full (crates/auris-gpui/src/logbook.rs:31,81-84) for exactly this reason. Worse, every `AgentEvent::Result` triggers `self.entries.iter_mut().rev().find(...)` (line 429) — a full reverse linear scan of the whole transcript to find the matching open `Tool` row — so a conversation with `n` tool calls does O(n) work per call, O(n²) total. `drain_agent` (line 688) drains the *entire* currently-buffered backlog of wire events synchronously in a `loop` on the render/repaint tick (`link.from_child.try_recv()` at line 716) with no cap on how many events it processes per call, so all of this cost lands on the UI thread. The wire this reads from is a subprocess (`auris-agent`) that itself talks to […]

**Expected.** The transcript should be bounded the same way the crate's own `Logbook` is bounded (a capacity with oldest-entry eviction), and/or the open-tool-row lookup should not require a full rescan of the whole history once it is large — e.g. by tracking the open row's index directly instead of searching for it.

**Fix direction.** Bound AgentChat::entries the same way the crate's own Logbook already does (a fixed CAPACITY with pop_front/VecDeque eviction of the oldest rows), and replace the reverse iter_mut().rev().find() scan in the AgentEvent::Result arm with a tracked index/handle to the just-pushed open Tool row (e.g. store the index of the last pushed Call row) so filling it in is O(1) instead of O(n).

**Verifier's correction.** Substantially accurate; one detail overstates it. `entries: Vec<ChatEntry>` (line 271) is indeed never capped or evicted anywhere in the crate (verified by grep, contrasted with `Logbook`'s explicit `CAPACITY`/`pop_front`), so unbounded memory growth over a long session is unconditionally correct. The `iter_mut().rev().find(...)` scan at line 429 is real, but it is *not* O(n) on every single `AgentEvent::Result` as stated — when a Result immediately follows its own matching Call (the ordinary single-tool-in-flight pattern), the match is the last-pushed element and the scan is O(1) (measured […]

### F-247 · medium · A tool-result event with no matching open call is pushed with a raw empty `line`, so chat_row's `line.is_empty()` check renders it as permanently "running" instead of showing its known ok/fail state.

`crates/auris-gpui/src/ui/agent_chat.rs:448` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In the agent-chat panel, a tool-result row that had no matching open call (e.g. an out-of-order or build-mismatched event) permanently shows the "…" (running) marker even though its ok/failure state is already known, instead of ever settling to "✓" or "✗". The row is stuck this way for the rest of the session.

**Trigger.** A `call` event for some tool is lost before reaching `absorb` — e.g. `parse_event` (lines 104-141) returns `None` for a line the reader thread couldn't parse as JSON (a partial/interleaved write on the child's stdout pipe, or a future agent build emitting a call shape this build doesn't recognise) — while the corresponding `result` event for that same tool later arrives intact and reports success with empty text (`"text": ""`), e.g. a tool whose successful outcome has nothing to say.

**Mechanism.** In `AgentChat::absorb`'s `Result` handling (lines 420-453), when a matching still-running `Tool` row is found, its `line` is deliberately normalized: `*row_line = if line.is_empty() { "done".to_string() } else { line };` (lines 440-444) — specifically so an empty result text doesn't leave the row looking unfinished, since `chat_row` (lines 1027-1032) treats `line.is_empty()` as the sole signal that a row is still pending: `let mark = match (*ok, line.is_empty()) { (_, true) => "…", (true, false) => "✓", (false, false) => "✗" };`. But the fallback branch for a `Result` with no matching open row — explicitly anticipated by the comment at lines 425-427 ('A result with no matching call — a build mismatch, a dropped line — becomes its own row rather than being lost') — pushes the event's fields straight through unnormalized: `_ => self.entries.push(ChatEntry::Tool { name: tool, ok, line, detail })` (lines 448-453), skipping the same empty-line substitution.

**Expected.** The same-name empty-line normalization applied in the matched-row branch ('done' in place of an empty `line`) should also apply to the no-match fallback row, so a result that arrives detached from its call still renders as finished rather than as an indefinitely pending call.

**Fix direction.** In the `_ => self.entries.push(...)` fallback arm of the `AgentEvent::Result` match (agent_chat.rs:448), apply the same empty-line normalization used in the matched-row arm — `line: if line.is_empty() { "done".to_string() } else { line }` — so `chat_row`'s `line.is_empty()` "still running" check can never be fooled by a freshly-pushed, already-resolved row.

**Written rule it breaks.** // The call pushed a running row; this fills it in. A result with no matching call — a build mismatch, a dropped line — becomes its own row rather than being lost.

### F-251 · medium · ContextMenu::size() counts CJK characters at their half-width Latin cost, so the widest Japanese menu row is silently truncated by the label's .truncate(), breaking the doc comment's own guarantee that a bad width estimate never clips a word.

`crates/auris-gpui/src/ui/context_menu/menu.rs:302` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A Japanese-locale user opening a context menu whose widest row is a CJK label (e.g. the track menu's "ボリュームをオートメーション" for Automate Volume) sees that label silently ellipsized mid-word, because the allocated menu width is computed from a Latin-tuned per-character estimate that undercounts full-width glyphs by roughly half.

**Trigger.** Run the interface in Japanese (`Language::Japanese`) and open a menu whose longest row uses a moderately long Japanese label (e.g. the track menu's "ボリュームをオートメーション" / "ボリュームをオートメーション" automation row, or any clip/roll menu row with a similarly long ja string) where the row also carries the always-present mark/tick column.

**Mechanism.** `ContextMenu::size()` picks the menu width from `item.label.chars().count() * CHARACTER_WIDTH` (menu.rs:298-308), where `CHARACTER_WIDTH = 6.6` px/char (line 43) is described only as "Rough advance width of one character" and the comment above it (lines 41-42) asserts "an over- or under-estimate costs a little whitespace rather than a clipped word" because the label is truncated with an ellipsis (render_context_menu, lines 483-489). `.chars().count()` counts Unicode scalar values, giving a full-width Japanese kana/kanji the same weight as a half-width Latin letter, even though at the menu's `text_xs` size (0.75rem ≈ 12px, gpui styled.rs `text_xs`) a full-width glyph's real advance is close to 12px — roughly double the 6.6px assumed. Japanese strings used in these very menus (e.g. `MenuAutomateVolume`'s ja text "ボリュームをオートメーション", 14 characters, in strings.rs:962) are sized as if they needed ~92px of glyph space when they need closer to 168px, so the row's `flex_1`/`truncate()` label area (menu.rs:483-489) is handed materially less room than the localized text needs.

**Expected.** Width estimation should weight full-width/wide Unicode characters (or CJK scripts generally) more heavily than half-width Latin ones, so the stated guarantee (estimate error costs whitespace, not truncation) actually holds for every language the interface ships, per `auris-i18n`'s parity with English.

**Fix direction.** In `ContextMenu::size()` (menu.rs:302), weight each character by its real advance instead of a flat `CHARACTER_WIDTH`: use a wider per-glyph constant (or double-count) for characters outside the ASCII/half-width range, e.g. via `unicode_width::UnicodeWidthChar` or a simple codepoint-range check, so CJK labels get roughly the correct estimated width.

**Written rule it breaks.** // Only used to pick a width — the labels themselves are truncated, so an over- or under-estimate costs a little whitespace rather than a clipped word. (menu.rs:40-42)

### F-258 · medium · agent_binary() falls back to an unqualified filename when current_exe() fails, letting Command::new() resolve the agent binary via CWD/PATH search instead of erroring.

`crates/auris-gpui/src/ui/agent_chat.rs:501` · security · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If std::env::current_exe() ever fails (deleted/moved binary, certain sandboxed or restricted-permission contexts) when the user opens the Agent Chat panel or the model-list dropdown, agent_binary() silently falls back to the bare filename "auris-agent.exe"/"auris-agent" with no directory component. Command::new() then hands that unqualified name to CreateProcess/exec, which searches the current working directory and PATH; a malicious executable placed there (e.g. dropped alongside a project file, since spawn_link sets current_dir to that project folder) can be launched instead of the real agent binary, with no error distinguishing this from an ordinary launch failure.

**Trigger.** Any environment where `std::env::current_exe()` returns `Err` (or where the beside-the-exe candidate does not exist) while the process's current working directory, or some earlier `PATH` entry, contains a file literally named `auris-agent.exe` (Windows) or `auris-agent` (elsewhere) placed there by another program or user with local write access to one of those locations.

**Mechanism.** `agent_binary()` (lines 501-510) tries to locate `auris-agent[.exe]` beside the running executable via `std::env::current_exe()`, but on failure it falls back to `PathBuf::from(name)` — a bare filename with no directory component. Both call sites, `spawn_link` (line 519: `Command::new(agent_binary())`) and `spawn_model_listing` (line 570), then hand this straight to `std::process::Command::new`, which — for an unqualified name — resolves the executable via the OS's standard search order (on Windows: the launching app's own directory, then the *current* process's current working directory, then the Windows system directories, then `PATH`; similarly `PATH`-based on Unix). `current_exe()` can fail (the docs note platform-specific mechanisms, e.g. a deleted/moved-and-relaunched binary, certain sandboxes/containers, or restricted permissions), and there is no check here that a resolved candidate is actually the file the application shipped, nor any fallback to a hard error instead of a bare-name search.

**Expected.** When `current_exe()` fails (or the expected sibling file does not exist), the spawn should fail with an explicit error rather than silently falling back to an unqualified name that lets the OS's directory/PATH search pick the binary.

**Fix direction.** Make agent_binary() return a Result instead of silently degrading: when current_exe() or .parent() fails, surface a clear error to the caller (the same way spawn_link/spawn_model_listing already surface Command::spawn() failures) instead of returning a bare filename for Command::new to resolve via CWD/PATH search.

### F-129 · low · export_singer_frames's doc comment opens with export_midi's description, leaving export_midi undocumented and cargo doc showing a false claim.

`crates/auris-gpui/src/ui/commands.rs:1413` · spec-mismatch · confirmed (traced through the code; reported independently 2×)

**What a user sees.** Someone reading the generated rustdoc (`cargo doc --workspace --no-deps --open`) or just the source for `export_singer_frames` sees "writes the document out as a MIDI file" as its first line, which is false — the function actually writes JSON frame data. `export_midi` itself is left with no doc comment, silently escaping `#![warn(missing_docs)]` only because the mislabeled line satisfies the lint on the wrong function. No runtime behavior is affected — both functions' bodies are correct and do what their names say.

**Trigger.** Open crates/auris-gpui/src/ui/commands.rs and read the doc comment immediately above `export_singer_frames` (or run `cargo doc` and view that item), or look at `export_midi` and note it has no doc at all.

**Mechanism.** Lines 1413-1414 read:
    /// Prompts for a destination and writes the document out as a MIDI file.
    /// Writes the singer track's frame features — phonemes, pitch, energy — to a JSON file.
    pub(crate) fn export_singer_frames(...)
The first line is false for this function (it writes a JSON frame-features file, not a MIDI file) and is a verbatim copy of the doc comment that originally belonged to `export_midi` (added in commit b4ae38a). `git show 33b91f2` (the commit that introduced `export_singer_frames`) shows the new function was inserted directly above the pre-existing `export_midi` doc comment, a second line was appended to describe the new function, but the original line was never moved down to the function it actually describes. As a result `export_midi`, at line 1459, now has no doc comment at all — confirmed by reading the current file, which shows `pub(crate) fn export_midi(...)` immediately preceded only by a blank line and the closing brace of `export_singer_frames`.

**Expected.** Per CLAUDE.md's documentation convention ("Every public item carries a doc comment") and the project's general practice of one accurate doc comment per function, `export_midi` should carry its own "Prompts for a destination and writes the document out as a MIDI file." doc comment, and `export_singer_frames` should carry only the frame-features description that already follows it.

**Fix direction.** Delete the stray first doc line from `export_singer_frames` (keep only the "Writes the singer track's frame features…" line), and add a correct one-line doc comment to `export_midi`, e.g. "/// Prompts for a destination and writes the document out as a MIDI file."

**Written rule it breaks.** Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate). CI builds the docs with warnings denied...

### F-137 · low · Resolving an external-change conflict via Save shows "Saved to …" then blanks it within 500ms via watch_disk's unconditional Withdraw status clear (app.rs:2107-2108).

`crates/auris-gpui/src/app.rs:2108` · ui · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** After resolving a "file changed on disk" conflict by pressing Save (secondary-s) instead of clicking Reload, the "Saved to /path" confirmation appears and then, within up to 500ms, is silently replaced by a blank status line — even though the save succeeded and the document is safely written. The user has no lasting on-screen confirmation that the save happened and may wonder if it worked.

**Trigger.** 1) An external writer touches the open, dirty project file so `watch_disk` sets `ExternalChange::Offer` and shows the conflict status. 2) The user resolves it the obvious way, by pressing Save (`secondary-s`) rather than clicking Reload. `save()` sets the status to "Saved to <path>". 3) Within at most 500ms the next `watch_disk` tick sees `modified=false, dirty=false, offered=true` → `Withdraw`, and blanks the status line to an empty string.

**Mechanism.** `watch_disk` (app.rs:2091-2126) drives `external_change_action` and, on `ExternalChange::Withdraw`, runs `self.external_change = None; self.set_status(String::new());` (lines 2107-2108) — it unconditionally blanks the status line. `Withdraw` fires when `externally_modified()` has gone back to false while an offer is still standing (`external_change.is_some()`), and `externally_modified()` (auris-session/src/session/autosave.rs:138-146) only turns false again once `disk_stamp` is refreshed by `mark_saved()`, which every save path calls (auris-session/src/session/files.rs:375, called from `save_in_place`). So the realistic — essentially only — way `Withdraw` fires is a manual save while the conflict banner is up. But `AurisApp::save` (ui/commands.rs:532-544) sets `self.status` to `messages::saved(...)` ("Saved to …") synchronously in the same call, immediately before `watch_disk`'s next tick (throttled to 500ms, often much sooner since the offer may already have been standing past the interval) overwrites that with an empty string.

**Expected.** Withdrawing the stale-offer banner should not stomp a status message that a concurrent command (here, the very save that caused the withdrawal) just set. Per the file's own doc comment the Withdraw path exists specifically for "a manual save takes it back" — the blanking should either be skipped when the status already reflects a fresher event, or Withdraw should leave `self.status` alone rather than clearing it to empty.

**Fix direction.** In watch_disk's ExternalChange::Withdraw arm (app.rs:2107-2108), drop the self.set_status(String::new()) call — clear only self.external_change and call cx.notify(); leave whatever status a concurrent command (the save) already set alone, since Withdraw's own doc comment says a manual save "takes it back" transparently rather than needing its own status wipe.

**Written rule it breaks.** watch_disk's doc comment: "withdraw the offer once the file is ours again (a manual save takes it back)" — implies transparent withdrawal, not overwriting the save's own confirmation message.

### F-155 · low · StopPlayback action is unreachable (no keymap/menu/palette row) and its doc comment claims a return-to-start seek that Session::stop never performs.

`crates/auris-gpui/src/actions.rs:55` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No user is affected today: StopPlayback has no keybinding and no palette/menu entry (BINDABLE generates both), so it can never actually be invoked through the UI. The only latent risk is for a future contributor who adds a keymap or menu row for it expecting it to "stop and return to the start" per its doc comment, when it actually only stops (Session::stop leaves the playhead where it is, per that method's own doc) — the return-to-start behavior lives in the separate ReturnToZero action.

**Trigger.** A user tries to find a keyboard shortcut, menu item, or palette entry for "stop" as distinct from the Play/Pause toggle; none exists anywhere in the shipped keymap, menu bar, or palette, even though the action, its doc comment, and its handler all exist in the source. If a future change bound a key or menu row to `StopPlayback` (the natural-looking fix, since the row is easy to assume is just missing), it would run `on_stop` and leave the playhead exactly where playback stopped rather than returning to the start.

**Mechanism.** `StopPlayback` is declared in the `actions!` macro (line 55, doc: "Stop playback and return to the start.") and root.rs registers a live handler for it (`ui/root.rs:129` `.on_action(cx.listener(Self::on_stop))`, handler at `ui/root.rs:1412`). But `StopPlayback` never appears as an `=>` target in the `bindable!` table in this file (grep over every `=> ActionName;` in actions.rs confirms it is the only one of the 96 declared actions with no BINDABLE entry), it has no row in `menu::model` (menu.rs's Transport section lists Play/Return/StepBack/StepForward/Record/Monitor/Punch/Loop/Metronome/MusicalTyping/GoTo/Panic but no Stop), and the command palette in ui/palette.rs is explicitly built only from BINDABLE ("a command added to [BINDABLE] appears here for free" — implying commands absent from BINDABLE are absent from the palette too). So there is no keystroke, no menu item and no palette row that dispatches this action. Separately, even if it were dispatched, `on_stop` only calls `self.session.stop()`, and `Session::stop` (crates/auris-session/src/session/transport.rs:38-40) is […]

**Expected.** Per the module doc (actions.rs:1-4) every declared action should be bound once and reachable through keymap, menu and (via BINDABLE) the palette; per the action's own doc comment, invoking it should stop playback *and* return to the start. Either `StopPlayback` should be removed (the working Stop button already covers this via direct session calls) or it should get a BINDABLE entry / menu row and its handler should also call `seek(Ticks::ZERO)` to match its doc comment and the reference […]

**Fix direction.** Either add a `bindable!` row for StopPlayback (giving it a keystroke, and thus a palette row automatically per palette.rs's "a command added to BINDABLE appears here for free") and a Transport menu row, or delete the action and its handler if TogglePlay/ReturnToZero already cover the need. Separately, fix the doc comment on StopPlayback (or on_stop) so it no longer claims "return to the start" when Session::stop explicitly does not seek.

**Written rule it breaks.** Every public item carries a doc comment (project doc-comment convention) — implicitly, the doc comment is expected to describe actual behavior, but StopPlayback's doc says "Stop playback and return to the start" while Session::stop is doc'd "Stops playback, leaving the playhead where it is."

### F-170 · low · Duplicate part/progression names are rejected with the misleading "Name cannot be empty" message instead of a collision-specific one.

`crates/auris-gpui/src/ui/prompt.rs:849` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** When a user renames a song part or names a kept chord progression to a value that's already taken by another part/progression, the app shows "Name cannot be empty" even though they typed real, non-empty text — leaving them unable to tell why the rename is being rejected since the message describes a condition (emptiness) that isn't true.

**Trigger.** On the song sheet, rename a part to a name another part already has (e.g. renaming Part 2 to "Verse" when Part 1 is already named "Verse"), or try to keep a progression under a name the built-in catalogue/book already uses, then press Enter.

**Mechanism.** For `PromptTarget::SongPartName(index)`, `taken` (841-847) is true when another part already has the typed name. Line 848-851 folds that together with the true empty case: `if taken || text.trim().is_empty() { self.set_failed_status(self.t(Key::NameCannotBeEmpty).to_string()); return; }` -- so a non-empty, duplicate name is reported with the exact same string as an empty one. The same pattern recurs at `PromptTarget::KeepProgression` (903-917): `if !self.progressions.keep(&text, &chart, chart.mode) { ... self.t(Key::NameCannotBeEmpty) ... }` (913-916), whose own comment ("A name the built-in catalogue already uses, or none at all") acknowledges the collision case but still reuses the empty-specific string. `Key::NameCannotBeEmpty` is defined in auris-i18n/src/strings.rs:749 as literally "Name cannot be empty" / "名前を空にはできません", which is a false statement when the rejection reason is a naming collision.

**Expected.** The collision case should surface a distinct, accurate message (e.g. "name already in use") rather than reusing NameCannotBeEmpty, whose text directly contradicts what just happened.

**Fix direction.** Add a distinct i18n string (e.g. Key::NameAlreadyUsed) for the collision case and branch on `taken` (SongPartName) and the collision-vs-empty cause inside ProgressionBook::keep's failure (KeepProgression) to show it instead of reusing NameCannotBeEmpty.

### F-173 · low · Time-signature scroll handler rounds each event's delta independently with no carried remainder, so precise-scroll (trackpad) input under 8px/event never changes the numerator.

`crates/auris-gpui/src/ui/transport_bar.rs:719` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user scrolling on the time-signature LCD field with a trackpad or a precise-scroll mouse sees the numerator never change: each event's fractional notch is discarded since only `notches.round()` is added and `current.numerator` is re-read fresh from the session every time, so any single event whose pixel delta is under 8px is a complete no-op, and a long steady low-velocity gesture built entirely of sub-8px events never advances the value at all. A regular mouse wheel (which reports full-notch, high-magnitude deltas) is unaffected.

**Trigger.** Hover the time-signature LCD field and scroll with a trackpad or any mouse with smooth/high-resolution scrolling enabled (delivering `ScrollDelta::Pixels` events smaller than 8px each), for as long a gesture as desired.

**Mechanism.** render_signature_control's `.on_scroll_wheel` handler (lines 713-726) computes `let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;` then `let beats = (current.numerator as i64 + notches.round() as i64).clamp(...)` and writes `beats` straight back with `set_signature_at`, re-deriving from the live `current.numerator` on every single event. `event.delta` is `ScrollDelta::Pixels` for any 'precise' scroll source (gpui-0.2.2 src/interactive.rs:395-423 documents `Pixels` as 'An exact scroll delta in pixels', which is what trackpads and smooth-scrolling mice deliver as many small per-event deltas rather than one large notch). Any event whose pixel delta is under 8px rounds to zero notches and changes nothing, and no fractional remainder is ever carried to the next event, so a long, steady scroll gesture never accumulates. The adjacent render_tempo_control's `.on_scroll_wheel` (lines 674-680) uses the exact same `notches` computation but adds it directly to a float: `let bpm = this.project().tempo_map.bpm_at(at) + f64::from(notches); this.session.set_tempo_at(at, bpm);` -- […]

**Expected.** The doc comment on render_signature_control (transport_bar.rs:691, 'The wheel still steps the beat count') states the control is meant to step on scroll input generally; it should accumulate sub-notch scroll progress the way the tempo field's float accumulator does, rather than silently discarding it every event.

**Fix direction.** Carry a fractional remainder across events, the way the adjacent tempo control does with its unrounded f64 accumulation: add an `f32` accumulator field on `AurisApp` (e.g. `signature_scroll_remainder`), add each event's raw `notches` to it, apply `remainder.trunc()` whole notches to the numerator and clamp, then keep only `remainder.fract()` for the next event.

### F-213 · low · Clearing the loop leaves loop_region as Some((0,0)) instead of None, so the menu still offers Punch From Cycle and arms a zero-length punch region.

`crates/auris-gpui/src/ui/context_menu/timeline.rs:75` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** After clearing the loop/cycle region, the ruler context menu still shows "Clear Cycle" (a no-op re-clear) and "Punch From Cycle", which is now offered even though there is no meaningful cycle. Choosing it sets the punch region to (0,0) and enables punch, silently arming a punch-in/out window with zero length instead of leaving punch untouched or informing the user there is no cycle to copy from.

**Trigger.** 1) Set a cycle region (e.g. via SetLoopStart/SetLoopEnd or ToggleLoop's auto-seed). 2) Right-click the ruler and choose Clear Cycle (`MenuCommand::ClearLoop`). 3) Right-click the ruler again — 'Punch From Cycle' is still offered because `loop_region` is `Some((0,0))`, not `None`. 4) Choose it.

**Mechanism.** `ruler_menu` gates the 'Punch From Cycle' row on `self.project().loop_region.is_some()` (timeline.rs:75-79). `MenuCommand::ClearLoop`'s handler (command.rs:1293-1296) does not clear `loop_region` back to `None`; it calls `self.session.set_loop_region(Ticks::ZERO, Ticks::ZERO)`, and `Session::set_loop_region` (crates/auris-session/src/session/transport.rs:97-105) unconditionally stores `Some((start.max_zero(), end.max_zero()))` — there is no session API that ever puts `project.loop_region` back to `None` once it has been touched (`Project::default()` is the only place it is `None`, in project/mod.rs:447). So after any 'Clear Cycle', `loop_region` is `Some((0,0))`, which still satisfies `.is_some()`. Choosing 'Punch From Cycle' then runs its handler (command.rs:1317-1322), which copies `(start, end) = (0,0)` verbatim into `set_punch_region` and sets `punch_enabled = true`.

**Expected.** The row (and the 'Clear Cycle' row beside it, same `is_some()` gate) should reflect whether there is an actual, positive-length cycle region — e.g. gated on `loop_region.is_some_and(|(s,e)| e > s)`, matching the `end > start` test the engine itself already uses in `publish_loop`/`punch_frames_at` — rather than on whether the field has ever been touched.

**Fix direction.** Make `Session::set_loop_region` (or `ClearLoop`'s handler in command.rs) set `self.project.loop_region = None` when clearing, instead of writing `Some((0,0))`; then the existing `loop_region.is_some()` gates in timeline.rs correctly hide both "Clear Cycle" and "Punch From Cycle" once the cycle is empty.

**Verifier's correction.** Same defect, correct line for the underlying mechanism; the gate line is 76, not 75 — `self.project().loop_region.is_some()` at timeline.rs:76 (the `MenuPunchFromCycle` item_if), one line below where the claim cites it. The identical gate for "Clear Cycle" itself is at timeline.rs:59.

### F-245 · low · db_to_meter_position collapses +Infinity dB to silence (0.0) instead of saturating to 1.0 like other above-range values, but no live UI path can currently feed it +Infinity.

`crates/auris-gpui/src/ui/widgets.rs:873` · correctness · plausible (executed reproduction; reported independently 1×)

**What a user sees.** No user-visible effect today: every real caller of db_to_meter_position feeds it a value derived from MeterBank::track_peak/master_peak, and MeterBank::report zeroes any non-finite peak before storing it, so +Infinity never actually reaches this function through the UI. It is a latent inconsistency in a public pure function's edge-case handling, not a reproducible on-screen bug.

**Trigger.** Any caller passing `db = f32::INFINITY` (e.g. `gain_to_db` in auris-core returns `20.0 * gain.abs().log10()`, which is `+inf` for `gain == f32::INFINITY` — reachable if an unstable filter or runaway gain automation ever produces an infinite sample peak upstream).

**Mechanism.** `db_to_meter_position` starts with `if !db.is_finite() || db <= METER_FLOOR_DB { return 0.0; }`. `f32::is_finite()` is false for both +Infinity and -Infinity (and NaN), so a positive-infinity dB value takes the exact same 'return 0.0' path as a negative-infinity (i.e. genuinely silent) one. The function's own doc says it 'maps a level in dBFS onto a 0..1 meter position', and the existing test only asserts the -Infinity case (`db_to_meter_position(f32::NEG_INFINITY) == 0.0`); there is no symmetric assertion, and none is possible, because +Infinity is handled identically to -Infinity rather than to the loud end of the range.

**Expected.** A value above the meter's usable range, finite or not, should saturate to the loud end (1.0) the way `db.min(6.0)` already does for large finite values; only values at or below `METER_FLOOR_DB` (and NaN, which has no direction) should read as 0.0.

**Fix direction.** Special-case db.is_infinite() && db.is_sign_positive() (or reorder the check to db == f32::INFINITY) to fall through to the normal clamp/saturate path instead of the early return, and add a db_to_meter_position(f32::INFINITY) == 1.0 test case alongside the existing NEG_INFINITY test at widgets.rs:897.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

**Verifier's correction.** `db_to_meter_position` (crates/auris-gpui/src/ui/widgets.rs:872-873) does collapse a directly-supplied `+Infinity` dB to 0.0 (silence) instead of saturating to 1.0 like other above-range values, which is inconsistent with the project's established convention elsewhere (compressor.rs `peak_at`) that an infinite sample should read as "as loud as a number gets," not as silence. However, this is not currently reachable from any real UI code path: every caller passes `db = gain_to_db(x)` where `x` comes from `MeterBank::track_peak`/`master_peak` (both go through `MeterBank::report`, which zeroes […]

### F-248 · low · grabbed_phoneme_boundary picks the first in-slack boundary via .find() instead of the nearest via .min_by_key, unlike the sibling curve_point_at.

`crates/auris-gpui/src/ui/piano_roll.rs:170` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When two phoneme boundaries on a note both fall within the 5px grab radius of a mouse press (only possible when phonemes are very narrow at the current zoom level), dragging grabs the earlier boundary instead of the one visually closer to the cursor, so the wrong phoneme split moves. At any normal zoom this never triggers since boundaries are pixels apart.

**Trigger.** A note with 3+ phonemes whose middle consonant is short enough (or the view zoomed out enough, since `slack` scales with pixels-per-tick) that two adjacent boundaries both land within `slack_seconds` of the press — e.g. a note singing a consonant cluster or a small-y mora (きゃ = k/y/a, two internal cuts), viewed at a zoom where 5 screen pixels cover tens of milliseconds. Pressing nearer the second cut than the first still grabs the first.

**Mechanism.** `grabbed_phoneme_boundary` (lines 152-172) filters candidate boundaries with `.filter(|(_, (_, to))| *to > 0.0 && *to < length)` and then takes the first one within slack with `.find(|(_, (_, to))| (start_seconds + to - at_seconds).abs() <= slack_seconds)`. Layout boundaries are strictly increasing (from `phoneme_layout` in `auris-vocal/src/frames.rs`, which accumulates `at += width` with all widths positive), so when two boundaries both fall inside the same ±slack window, `.find()` always returns the earlier one even if the later one is closer to the press. This is the opposite convention from the sibling curve-point picker in the same file, `curve_point_at` (line 2088-2095), which explicitly does `.min_by_key(|(_, distance)| *distance)` and is documented 'Nearest rather than first, so two points dragged close together still resolve to the one under the pointer.'

**Expected.** Pick the boundary with the smallest `(start_seconds + to - at_seconds).abs()` among those within slack, the same nearest-wins rule `curve_point_at` already uses and documents for exactly this kind of closely-spaced-target ambiguity.

**Fix direction.** Change the `.find()` to compute distance for each filtered candidate and select via `.min_by_key` on that distance, mirroring `curve_point_at`'s existing nearest-point pattern and its documented rationale ("Nearest rather than first, so two points dragged close together still resolve to the one under the pointer").

### F-250 · low · sing_track/begin_export/start_export_stems don't check self.auto_sing, so a manual render can briefly run concurrently with (or silently stall behind) an in-flight auto-sing render, though the final progress(total,total) recheck in Model::sing_with prevents any stale take from landing.

`crates/auris-gpui/src/ui/commands.rs:1072` · concurrency · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user who edits a singer clip, then immediately presses Sing/Export/Export Stems before the automatic re-sing has caught up, briefly runs two ONNX inference renders at once (extra CPU/GPU load and a momentarily confusing status line), or — if the manual render targets the same voice — the manual action silently stalls until the auto-sing render's current chunk finishes, since `cancel_auto_sing` only sets a flag rather than waiting. No stale/wrong take is ever landed: `Model::sing_with`'s final `progress(total, total)` check (model.rs:249) catches a cancellation issued up to the very last chunk and routes it to `SingError::Cancelled`, which `poll_auto_sing` treats as a no-op.

**Trigger.** While `self.auto_sing` is `Some(..)` (an automatic re-sing is actively running in the background, e.g. right after an edit crosses the debounce window on a longer singer clip), press Sing (or Export / Export Stems) before that render reaches its next cancellation checkpoint. `sing_track`/`begin_export`/`start_export_stems` do not block on or wait for the outstanding `auto_sing` task; a second render is started concurrently with the still-running first one, and the first can still complete and call `land_singer_take` afterward.

**Mechanism.** `sing_track`'s own doc comment (lines 1062-1064) claims the render "reuses the export overlay ... and the same one-at-a-time rule, because two long renders at once would fight over the machine". The guard at lines 1068-1071 only checks `self.choosing_export` and `self.export`; it says nothing about `self.auto_sing`. Line 1072 then calls `self.cancel_auto_sing()`, which only stores `true` into an `AtomicBool` (lines 1174-1178) — it does not block, and does not wait for `self.auto_sing` to become `None`. Execution falls straight through into building and spawning a brand-new background render (lines 1106-1166). The same pattern repeats in `begin_export` (guard at 2169-2172, `cancel_auto_sing()` at 2173) and `start_export_stems` (2064-2068). Meanwhile `auris_singer::Model::sing_with` (crates/auris-singer/src/model.rs:197-247) only re-checks the cancel flag via the `progress` callback between chunks, so a render close to completion can still return `Ok` and be landed by `poll_auto_sing`'s completion handler (commands.rs:1307-1330) after the cancel request was issued.

**Expected.** Starting a manual Sing or Export should either block until any in-flight `auto_sing` has actually stopped (not just requested to stop), or refuse/queue the new request the same way the `choosing_export`/`export` guard already refuses a second concurrent export, so at most one singer render is ever in flight at a time as the doc comments claim.

**Fix direction.** In `sing_track`/`begin_export`/`start_export_stems`, extend the existing guard to also check `self.auto_sing.is_some()` (or have `cancel_auto_sing()` return a future/flag the caller awaits before spawning the new render) so the code enforces the "one-at-a-time rule" its own doc comment claims, instead of only guarding against a concurrent manual export/sing.

**Written rule it breaks.** sing_track's doc comment: "the same one-at-a-time rule, because two long renders at once would fight over the machine and the status line."

**Verifier's correction.** The core defect is real for `sing_track` (commands.rs:1068-1072): the guard never checks `self.auto_sing`, and `cancel_auto_sing()` only stores a flag without waiting, so a manual Sing on a *different* singer track than the one auto-sing is currently rendering can run its `Model::sing_with` genuinely concurrently with the still-running auto-sing render (different voice files → different `Arc<Mutex<VoiceModel>>`, no lock serializes them). When the manual Sing targets the *same* track/voice as the in-flight auto-sing render, the shared per-voice `Arc<Mutex<VoiceModel>>` cached in […]

### F-264 · low · Panel::command's doc comment says "all five" panels but Panel::ALL has held six since the Agent panel shipped.

`crates/auris-gpui/src/dock.rs:122` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer reading the doc comment on Panel::command is told the dock has five panels learned "by clicking all five," but there are six (Library, PianoRoll, Mixer, Inspector, Log, Agent); this is a source-level documentation staleness with no runtime effect on the app, CLI, MCP server, or trainer.

**Trigger.** Read the current doc comment on `Panel::command` (or `Panel::ALL`) alongside the current panel list.

**Mechanism.** The doc comment on `Panel::command` reads: '...The switch is a mark and nothing else — the panel it opens is a thing learned by clicking all five — and the key is exactly what somebody who has just learned it would rather not have to click for again.' (line 122). `Panel::ALL` (lines 86-93) currently lists six variants: Library, PianoRoll, Mixer, Inspector, Log, Agent. `git log -p` shows the comment was introduced verbatim by commit 038c20e ('Say what the unlabelled buttons are...') when `Panel::ALL` indeed had five entries (no Agent); a later commit, aa3a052 ('Sit the agent down beside the song'), grew `Panel::ALL` from `[Panel; 5]` to `[Panel; 6]` by adding `Panel::Agent`, but did not touch this doc comment, leaving the count stale.

**Expected.** The comment should read 'all six' (or be phrased independent of the count) to match the current six-element `Panel::ALL`.

**Fix direction.** Update the doc comment at crates/auris-gpui/src/dock.rs:122 to say "all six" instead of "all five" (or drop the specific count and say "all of them"), matching Panel::ALL's six variants.

**Written rule it breaks.** Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate) — implying doc comments are expected to stay accurate; CLAUDE.md also calls out guide-vs-code staleness (e.g. "when the two disagree, the guide is right and this is stale") as a class of problem the project tracks.

### F-269 · low · Test doc comment in menu.rs wrongly claims Open Recent/About have an empty binding id; it's actually their default keystroke that's empty.

`crates/auris-gpui/src/menu.rs:949` · test-quality · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No runtime or end-user impact at all — this only misleads a future contributor reading the test's doc comment in crates/auris-gpui/src/menu.rs, who would incorrectly believe Open Recent and About rows carry an empty `MenuRow::Command.binding` id, when in fact every menu row always passes a non-empty id string and it is `Bindable::default` (the default keystroke) that is empty for those two commands.

**Trigger.** Reading the comment and the `binding.is_empty()` clause suggests the test knowingly tolerates rows with no binding id, and that behaviour is exercised by two real rows in the menu. In fact no row in the current (or original) code ever has an empty `binding`, so that half of the `||` is dead for every row this test walks; the assertion is effectively just `bindable(binding).is_some()`, and the comment's claim about which rows hit the other branch is false.

**Mechanism.** The doc comment above `every_binding_a_menu_row_names_is_one_the_table_has` (lines 944-950) says: "An empty id is the deliberate case: Open Recent and About have no keystroke... ", and the assertion at line 960 is `binding.is_empty() || crate::actions::bindable(binding).is_some()`. But every `MenuRow::Command` built in `model()` — including the Open Recent row (`command(t(Key::CmdOpenRecent), actions::OpenRecent, "file.recent")`) and the About row (`command(t(Key::CmdAbout), actions::ShowAbout, "view.about")`) — is constructed with a real, non-empty binding *id* (`"file.recent"`, `"view.about"`). What is actually empty for those two commands is their *default keystroke* in the `BINDABLE` table (the `""` column in actions.rs), a completely different field on a different type (`Bindable::default`, not `MenuRow::binding`). A grep across menu.rs for any `command(..., "")` call site confirms no row ever passes an empty binding id; this was true even in the commit (92cae656) that introduced the comment.

**Expected.** Either the comment should be corrected to describe what actually varies (empty *default keystroke*, not empty *binding id*), or the `binding.is_empty()` branch should be removed from the assertion since, per the file's own menu rows, every `MenuRow::Command` always names a real BINDABLE id.

**Fix direction.** Rewrite the doc comment on `every_binding_a_menu_row_names_is_one_the_table_has` (menu.rs:944-950) to correctly say that Open Recent and About have no default keystroke (`Bindable::default` is empty in the BINDABLE table), not that their menu row `binding` id is empty; optionally note that the `binding.is_empty()` disjunct in the assertion is defensive/dead code today since no call site passes an empty id.

### F-270 · low · Stale "Last, because..." comment sits above library_search_key though agent_key was appended after it as the true last disjunct in root.rs's on_key_down chain.

`crates/auris-gpui/src/ui/root.rs:1150` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No functional effect for any user — the key dispatch order and behavior are unchanged. A future contributor reading the comment while adding or reordering a handler near agent_key could be misled into thinking library_search_key is the last disjunct, but nothing currently observable is wrong.

**Trigger.** No specific runtime input is needed to observe the mismatch; it is visible by reading lines 1144-1158. (Functionally, `focus_agent_field` (agent_chat.rs) clears `library_search_focused` and the library search box's focus setter clears `agent_chat.focused`, so the two handlers happen to be mutually exclusive today and no user-visible bug currently follows from the wrong ordering — but that exclusivity is not stated or enforced anywhere near this dispatch chain.)

**Mechanism.** In `on_key_down`'s dispatch chain (lines 1144-1153), the comment "// Last, because everything above it is in front of the browser on the screen and\n// has to answer for a key first." sits directly above `|| self.library_search_key(event)` (line 1152), asserting that handler is evaluated last. But `|| self.agent_key(event)` (line 1153) follows it, so library_search_key is not actually last. `git log -p -- crates/auris-gpui/src/ui/root.rs` shows the comment plus the `library_search_key` disjunct were introduced together as the chain's final line, and a later commit's diff adds `+ || self.agent_key(event)` right after it — the comment was never updated when the new handler was appended.

**Expected.** The comment describing "last, because it's in front of everything else" should sit on the actual last disjunct (`agent_key`), or explain why agent_key is exempt/placed after it, so the prose again matches the evaluation order it claims to describe.

**Fix direction.** Move the "Last, because..." comment down to sit directly above `|| self.agent_key(event)` (the actual last disjunct), or reword it to describe library_search_key's actual position ("second-to-last") rather than claiming finality.

### F-274 · low · crates/auris-gpui/src/ui/mod.rs:3 claims every ui submodule only extends AurisApp, but tooltip.rs defines its own Render entity (Tooltip), contradicting the doc.

`crates/auris-gpui/src/ui/mod.rs:3` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer reading crates/auris-gpui/src/ui/mod.rs's module doc gets a wrong mental model of the ui module's architecture (that every submodule only extends AurisApp), then hits tooltip.rs's own well-reasoned "Why this is a view rather than an element" section and has to reconcile the contradiction themselves; no runtime behavior, build, or test is affected.

**Trigger.** No runtime input needed — the mismatch is between the doc text in mod.rs and the code in the sibling module tooltip.rs that mod.rs itself declares.

**Mechanism.** mod.rs's module doc (lines 1-5) states: "Every module here adds `impl` blocks to [`crate::app::AurisApp`] rather than defining its own gpui entity: the panels all read the same project, selection and engine handle, and one owner of that state is simpler than synchronising several." But `pub mod tooltip;` (mod.rs line 37) is one of "every module here", and tooltip.rs defines `pub struct Tooltip` (tooltip.rs line 26) with `impl Render for Tooltip` (tooltip.rs line 37) — a second, independent gpui view/entity, not an impl block on AurisApp. `grep -rn "impl Render for" crates/auris-gpui/src/ui/*.rs` finds exactly two hits under ui/: AurisApp (root.rs:35) and Tooltip (tooltip.rs:37), confirming tooltip.rs is a real, singular exception to the blanket claim.

**Expected.** The mod.rs doc should carve out the tooltip exception (mirroring tooltip.rs's own "Why this is a view rather than an element" section) instead of asserting an absolute rule a sibling module in the same list visibly and intentionally breaks.

**Fix direction.** Qualify the claim in crates/auris-gpui/src/ui/mod.rs's module doc, e.g. "Every module here adds `impl` blocks to [`crate::app::AurisApp`] rather than defining its own gpui entity, with one exception — [`tooltip`] needs a standalone `AnyView`, see its doc comment for why" — a one-sentence edit, no code change.

**Written rule it breaks.** Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate) — implying doc comments are expected to be accurate; more directly, the doc text itself: "Every module here adds `impl` blocks to [`crate::app::AurisApp`] rather than defining its own gpui entity"

### F-278 · low · side_widths' zero-total guard at dock.rs:470 is unreachable dead code, since room's unconditional >=0 clamp forces total>0 whenever control reaches it.

`crates/auris-gpui/src/dock.rs:470` · other · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No observable effect today: the case the second guard appears to cover (total <= 0) is already handled by the first branch, since room is unconditionally clamped to >= 0 on line 466. A reader or future editor, however, sees a guard that looks like it protects the division on the following lines and may rely on it, or may safely delete/narrow the first branch believing this one still covers the zero-total case — reintroducing a real division-by-zero risk that this artifact currently masks.

**Trigger.** Any call to `PanelLayout::side_widths`.

**Mechanism.** `side_widths` (lines 460-477): `let total = asked.0 + asked.1; let room = (viewport - keep).max(px(0.0)); if total <= room { return asked; } if total <= px(0.0) { return (px(0.0), px(0.0)); } ...`. Every caller passes non-negative dock sizes for `asked` (0 for a closed dock, otherwise >= MIN_SIDE/MIN_BOTTOM, per the doc's own 'asked is what each would like, which is zero for a dock that is shut'), so `total >= 0` always, and `room >= 0` always because of the explicit `.max(px(0.0))`. Reaching the second `if` requires having already failed `total <= room` on the line above, which — since `room >= 0` — forces `total > room >= 0`, i.e. `total > 0` strictly. So `total <= px(0.0)` can never be true at that point; the branch is unreachable for any input the type can represent. The test that exercises the all-zero case, `shut_docks_ask_for_nothing_and_are_given_nothing` (asked=(0,0)), actually satisfies `total(0) <= room(0)` and returns via the *first* branch with the comment 'no division by a total of zero' attached to that assertion, not the second branch — confirming the second guard is […]

**Expected.** Either remove the unreachable second guard, or restructure so the zero-total case is visibly the one it protects (e.g. check `total <= px(0.0)` before the `total <= room` early return).

**Fix direction.** Remove the unreachable `if total <= px(0.0) { return (px(0.0), px(0.0)); }` block, or replace it with a comment/debug_assert noting that total > 0 is guaranteed here (since room >= 0 and total > room at this point) so the subsequent division by `total` is always safe.

### F-283 · low · Loop toggle on a mixed clip selection can show "Clip looped" even when most clips end up unlooped, because the status is chosen by looped > 0 rather than by net outcome.

`crates/auris-gpui/src/ui/commands.rs:440` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** After multi-selecting several clips and toggling loop, the status bar can say "Clip looped" even though a majority of the selected clips ended up unlooped (each clip flips from its own prior state, but the message is chosen only by whether the count of now-looped clips is nonzero, not by comparing it to the count of now-unlooped clips). The actual per-clip loop state is set correctly — only the summary message is misleading.

**Trigger.** Select three clips where two are currently looped and one is not, then invoke Toggle Loop on the selection. Each clip flips independently: the two looped ones become unlooped, the one unlooped one becomes looped, so `looped == 1 > 0`. The status bar reports the "Clip Looped" message even though the net effect on the selection was mostly to turn looping off.

**Mechanism.** `toggle_clip_loop` (lines 430-444) flips each selected clip from *its own* current state (per its own doc comment, because "a mixed selection has no single answer to flip"), counting how many ended up looped in `looped`. The final status line is chosen purely by `looped > 0` (line 440): as soon as at least one clip in the selection became looped, the message is `Key::ClipLooped` — even if the rest of the (larger) selection just became *unlooped*.

**Expected.** The summary should reflect the selection's net outcome (e.g. compare `looped` against `chosen.len() - looped`, or report a genuinely mixed result) rather than treating any nonzero count of newly-looped clips as the whole story.

**Fix direction.** In toggle_clip_loop, track both looped and total chosen.len() (or unlooped = chosen.len() - looped) and pick the status key by comparing looped vs unlooped (majority), or add a distinct mixed-outcome status message when neither is unanimous, rather than using looped > 0 as the sole test.

### F-287 · low · Plugin window header drag lacks the pressed_at wobble guard every other pixel-based drag (ClipMove, TrackReorder, NoteMove, NoteResize) has, so a click on its header buttons visibly nudges the window before the click fires.

`crates/auris-gpui/src/ui/plugin_window.rs:381` · ui · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Clicking the plugin window header's bypass/own-window/close button with the ordinary sub-pixel-to-few-pixel jitter of a real click causes the floating plugin window to visibly nudge before the click's own action (toggle bypass, open hosted window, close) fires on release. No data or document state is affected — Drag::MovePluginWindow::edit() returns None — it is purely a visual jitter/inconsistency.

**Trigger.** Press the left mouse button on the plugin window's bypass/own-window/close button and release with even one pixel of pointer movement in between (ordinary mouse/trackpad jitter during a click) — before the button's own click action fires on mouse-up.

**Mechanism.** The title bar's `on_mouse_down` handler (lines 381-390) calls `this.begin_drag(Drag::MovePluginWindow { grab_offset: ... })` unconditionally for any left mouse-down anywhere in the header row, including on the child buttons (`plugin_header`'s bypass button, the "own window" `chain_button`, and the close `chain_button`), because gpui's `.on_click()` (used by all three, per crates/auris-gpui/src/ui/widgets.rs) only records `pending_mouse_down` on `MouseDownEvent` without calling `cx.stop_propagation()` (confirmed in gpui 0.2.2 src/elements/div.rs around line 2137-2145) — so the down event keeps bubbling into the header's own raw `on_mouse_down`. Once `self.drag = Some(Drag::MovePluginWindow{..})` is set, `root.rs`'s `on_mouse_move` (lines 1067-1073) applies `window.anchor = event.position - grab_offset` on the very first pointer-move event, with no threshold check. Every other pixel-measured drag in the `Drag` enum that shares a hitbox with clickable children (`ClipMove`, `TrackReorder`, `NoteMove`, `NoteResize`) carries a `pressed_at: Option<Point<Pixels>>` guard specifically so a […]

**Expected.** Following the same pattern as `ClipMove`/`TrackReorder`/`NoteMove`/`NoteResize`, `Drag::MovePluginWindow` should carry a `pressed_at` guard (or the header's mouse-down should exclude the button hitboxes) so a click on the header's buttons cannot begin a window move before the click completes.

**Fix direction.** Give Drag::MovePluginWindow a pressed_at: Option<Point<Pixels>> field (mirroring ClipMove/TrackReorder/NoteMove/NoteResize) and gate root.rs's on_mouse_move application of window.anchor behind gestures::past_drag_threshold, the same way the other pixel-measured drags are guarded.

**Written rule it breaks.** The code's own adjacent comment: "The whole bar is the grab handle, so the window moves from anywhere that is not one of its two buttons." (plugin_window.rs:379-380) — not enforced, since the header's on_mouse_down fires unconditionally and gpui's on_click does not stop_propagation on mouse-down.

### F-291 · low · Shift+double-click on a singer-clip note opens the lyric prompt and silently clears the multi-selection instead of extending it.

`crates/auris-gpui/src/gestures.rs:122` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** On a singer track, if a user has multiple notes selected and then shift-double-clicks one of them (intending to extend the selection, per the app's own rule), the lyric-edit prompt opens for that single note and the rest of the multi-selection is silently cleared instead of extended. No notes or data are lost — only the in-memory selection set is reset — and the bug is reachable only on singer clips, only via double-click, so it is a narrow, easily-worked-around UI inconsistency.

**Trigger.** On a singer clip, shift-click two or more notes to build a multi-note selection, then shift-double-click one of the already-selected notes (e.g. accidentally, or attempting to toggle it) within the double-click time window.

**Mechanism.** `PointerGesture::Click::matches` (lines 111-116) explicitly refuses a shifted press: 'Every modifier is refused rather than only the two that name gestures... a plain-click *create* that also claimed ⇧-click would leave no way at all to sweep a rubber band.' `PointerGesture::DoubleClick::matches` (lines 121-123) has no such exclusion — `event.click_count >= 2 && !event.modifiers.secondary() && !event.modifiers.alt` — so a shift-held double-click still matches. In `piano_roll.rs::begin_note_drag` (lines 1032-1039), this check runs first, ahead of the shift-click selection-toggle logic at lines 1092-1104, and on a match calls `open_lyric_prompt` (lines 1480-1496), whose first act is `self.selected_notes.clear(); self.selected_notes.insert(index);` — discarding whatever multi-note selection shift-click had built.

**Expected.** `empty_press`'s own doc comment states the project-wide invariant: '⇧ already means "extend the selection" on every other press in the application.' `PointerGesture::DoubleClick::matches` should exclude a held shift the same way `Click` does, so a shift-double-click on a note either does nothing special or falls through to the ordinary shift-click toggle instead of clobbering the selection and switching into lyric editing.

**Fix direction.** Add `&& !event.modifiers.shift` to `PointerGesture::DoubleClick::matches` in crates/auris-gpui/src/gestures.rs (mirroring `Click::matches`), so a shift-held double-click falls through to the normal shift-toggle selection logic in `begin_note_drag` instead of being claimed by the lyric-prompt branch.

**Written rule it breaks.** // Every modifier is refused rather than only the two that name gestures. ⇧ is the extend-a-selection key everywhere in the application

### F-300 · low · text_for_range/selected_text_range use the mutating field accessor, silently resetting an in-progress Tab-completion walk on a read-only IME query.

`crates/auris-gpui/src/ui/text_field.rs:490` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If the platform's text-input system issues a read-only IME query (text_for_range or selected_text_range) while the user is mid Tab-walk through completions in a prompt field, the walk silently restarts from the field's current text instead of continuing, so a subsequent Tab can appear to do nothing. Tab presses themselves never trigger this (they're intercepted before reaching field_mut), and on Windows the only call site for selected_text_range fires on WM_IME_STARTCOMPOSITION, not a passive read, so the practical reachability of this bug is unconfirmed and likely rare.

**Trigger.** Start a Tab-walk on a notation field (e.g. type 'b' in the Chord prompt, press Tab once to reach 'bIII'), then have the platform's text-input system invoke `EntityInputHandler::text_for_range` or `selected_text_range` on the window before the next Tab press (these are standard NSTextInputClient/TSF queries a platform can issue outside of active IME composition, e.g. for accessibility or focus bookkeeping) — the next Tab then starts a fresh walk from the field's current content ('bIII') instead of continuing the original walk from 'b'.

**Mechanism.** `entity_input_handler!`'s `text_for_range` (line 490) and `selected_text_range` (line 502) both fetch the field via `HasTextField::field(self)`, which for `AurisApp` resolves to `writable_field()` → `self.prompt.as_mut().and_then(Prompt::field_mut)` (prompt.rs:1634). `Prompt::field_mut` (prompt.rs:578-584) unconditionally does `self.completing = None;` before returning the field — by its own doc comment, that reset is meant to fire only on 'every path that changes the text.' `text_for_range`/`selected_text_range` are pure reads (they never call `replace`/`insert`), yet they go through the same mutating accessor instead of the read-only `readable_field()` that `marked_text_range` correctly uses (text_field.rs:514).

**Expected.** Read-only `EntityInputHandler` methods should use `HasTextField::readable_field` (as `marked_text_range` already does), leaving `self.completing` untouched, matching the stated invariant that only text-changing paths take the resetting route.

**Fix direction.** Change text_for_range and selected_text_range in the entity_input_handler! macro (text_field.rs) to fetch the field via HasTextField::readable_field instead of the mutating HasTextField::field, matching what marked_text_range already does, so these read-only accessors no longer route through Prompt::field_mut and reset self.completing.

**Written rule it breaks.** Prompt::field_mut's own doc comment: the completing reset is meant for "the key handler and the platform's input handler both" changing text — i.e. paths that change the text, not pure reads.

### F-301 · low · Instrument plugin window's bypass button always shows "on" and is inert — InstrumentTrack has no enabled field to toggle.

`crates/auris-gpui/src/ui/plugin_window.rs:399` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Opening an instrument's plugin window shows a bypass button that always reads "on" and does nothing when clicked — no crash, no wrong audio, just a control that silently ignores input because instruments have no bypass concept in the data model yet.

**Trigger.** Open an instrument's plugin window (from the inspector, `PluginSubject::Instrument(track)`) and click the On/Off button in its header, the same button that bypasses an effect insert.

**Mechanism.** `resolve_plugin` hardcodes `enabled = true` for `PluginSubject::Instrument` (line 508: `Some((inner.instrument_id.clone(), true))`) — there is no `enabled` field on an instrument slot in auris-core to read. The header's `on_toggle` handler passed to `plugin_header` (lines 398-403) only calls `this.toggle_effect(track, slot)` when `subject` matches `PluginSubject::Insert`; for `PluginSubject::Instrument` the `if let` does not match, so the click does nothing but `cx.notify()`.

**Expected.** Either omit the toggle for a `PluginSubject::Instrument` (there being no bypass concept for the primary instrument in the session model), or give it an effect; every other clickable control in this window changes something when pressed.

**Fix direction.** Either hide/disable the bypass button for PluginSubject::Instrument in plugin_header's call site until InstrumentTrack gains a real enabled field, or add that field to InstrumentTrack and wire resolve_plugin/toggle to read and mutate it, matching the Insert arm's pattern (entry.enabled).

**Written rule it breaks.** Only for a plugin that has one. A button that did nothing on every built-in in the application would be a button nobody pressed on the one plugin where it works. (comment in plugin_window.rs, stating the project's own principle against dead buttons — applied to the "own window" button but the same reasoning applies here)

### F-306 · low · press_typed_key's audition_track guard returns before release_typed_key runs, so deleting the last instrument track mid-drag can leave a note stuck sounding.

`crates/auris-gpui/src/ui/typing_panel.rs:607` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** On the musical-typing panel, if the only remaining instrument track is deleted (via Ctrl/Cmd+Backspace) while the mouse button is held down on a drawn key, then the pointer slides onto another key without releasing, the previously held note keeps sounding and its key stays lit — until the pointer eventually comes up over the root, at which point the unconditional mouse-up handler releases it. It self-heals on mouse-up and requires deleting the last instrument track mid-gesture, so it is a narrow, transient edge case.

**Trigger.** Press and hold a drawn key (mouse down on the typing keyboard while a track with an instrument is selected/auditionable), then — without releasing the mouse button — change track selection via a keyboard shortcut to a track `audition_track` returns None for (e.g. an empty/uninstrumented track), then slide the pointer onto a different drawn key.

**Mechanism.** `press_typed_key`'s doc comment states "Whatever the pointer was holding is let go of first," but the early `let Some(track) = self.session.audition_track(self.selected_track) else { return; }` guard (lines 607-609) sits before `self.release_typed_key()` (line 610). If the audition target becomes unavailable between two key presses of the same pointer gesture, the function returns before releasing the key recorded in `self.clicked_key`.

**Expected.** Per the doc comment's own stated guarantee, `release_typed_key()` should run unconditionally at the start of `press_typed_key`, before the `audition_track` check gates whether a *new* note is struck.

**Fix direction.** Move `self.release_typed_key();` before the `audition_track` guard in `press_typed_key` (or restructure so the early return only skips the press, not the release), so the release always runs regardless of whether a playable track exists.

**Written rule it breaks.** Whatever the pointer was holding is let go of first. One pointer holds one key, and a slide that took the next one without putting the last one down would leave a note sounding with nothing left that knows about it — which is the whole of the bug this guards.

**Verifier's correction.** The core defect is exactly as claimed: press_typed_key's `let Some(track) = self.session.audition_track(self.selected_track) else { return; }` guard (typing_panel.rs:607-609) precedes `self.release_typed_key()` (line 610), so a call that hits the early return skips releasing the key/note the pointer was already holding, contradicting the doc comment's "Whatever the pointer was holding is let go of first." However, the claim's example trigger is imprecise: `Session::audition_track` (auris-session/src/session/harmony.rs:340-353) falls back to any instrument track in the project when the […]

### F-307 · low · ContextMenu::step's comment says the stale-highlight fallback lands "just before the next row" (position -1) but the code hardcodes position 0, skipping the first choosable row.

`crates/auris-gpui/src/ui/context_menu/menu.rs:138` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No end-user-visible effect on menu navigation since `highlighted` is only ever set to entries drawn from `choosable` (or None), so the fallback path (`unwrap_or(0)`) is dead code in practice; the only harm is to a future maintainer reading the comment at menu.rs:133-134, who is told the fallback lands "just before the next one down" (implying position -1, landing on the first choosable row after `step(1)`) when the code actually hardcodes position 0, landing on the second choosable row and skipping the first — misleading anyone who edits `step` or relies on the comment to reason about the fallback's behavior.

**Trigger.** Only reachable if `ContextMenu::highlighted` is ever `Some(index)` where `index` is not present in the freshly-recomputed `choosable` list for the current `entries`. Every current call site (`menu.rs` step()'s own two assignments, and root.rs's `Home`/`End` handling which resets `highlighted = None` before stepping) only ever sets `highlighted` to a value taken from `choosable`, or to `None`, so this branch is not exercised by any code path found in the crate today.

**Mechanism.** The comment at lines 133-134 says the fallback is for when the current highlight is 'just before the next one down' — i.e. equivalent to position -1, so that `step(1)` lands on the first choosable row. The code instead does `.unwrap_or(0)` (line 138), i.e. position 0 (the first choosable row itself). With that fallback, `step(1)` computes `next = (0+1) % count = 1`, landing on the *second* choosable row and silently skipping the first, which contradicts the documented intent.

**Expected.** Either the fallback should be `-1` (so `step(1)` lands on the first choosable row, matching the doc), or the comment should be corrected to describe the actual position-0 behaviour.

**Fix direction.** Either change the comment to accurately describe the `unwrap_or(0)` fallback (e.g. "or the first choosable row's position if the stored index is stale, which then steps from there"), or change the fallback to `.unwrap_or(-1)` so `step`'s documented contract ("the first Down lands on the first row") actually holds for this recovery path; the two-line fix is to pick one of these and make code and comment agree.

**Written rule it breaks.** Every public item carries a doc comment (#![warn(missing_docs)] is on in each crate).

**Verifier's correction.** None needed — the claim's location (line 138), mechanism, and consequence are accurate as stated.
