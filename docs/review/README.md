# Whole-repository adversarial review

Reviewed: commit `52d1702` on `dev`, 2026-09-05. Scope: every crate under `crates/`, the vendored `rustysynth` fork, `training/`, the tooling scripts, CI and the top-level documentation.

## How the review was run

The review was a multi-agent audit built to be adversarial at every stage, so that what survives is a defect and not an opinion:

1. **Discovery.** The tree was split into 50 units (2–6k lines each). Each unit was read in full by three independent reviewers, each under a different lens (logic correctness, realtime and concurrency, persistence and hostile input, contract versus implementation, frontend behaviour, trust boundaries, numeric DSP, music theory, Python, interface text). Sixteen more reviewers each swept the whole tree along one concern (the realtime path end to end, save/load round trips, undo/redo consistency, the Windows-only paths, thread ownership, the seed contract, toolbox parity and prompt injection, the voice-file host contract, test quality, the panic surface, time arithmetic, the architecture rules in CLAUDE.md, resource lifecycle, DSP numeric claims, repository archaeology, the supply chain). A completeness critic then named ten gaps, which ten more reviewers covered.
2. **Second pass.** Every suspicion a first-pass reviewer could not pin down (1,110 of them) was handed to a fresh reviewer for its unit with orders to settle it or discard it.
3. **Verification.** Every claim was given to two independent verifiers: a skeptic told to refute it, and a reproducer told to execute it (existing tests, the built binaries against crafted input, or a scratch program) or trace it with concrete values. When they disagreed a tie-breaker read the code and decided. Survivors were then rated by a consequence judge. Claims that both verifiers refuted are listed in [refuted.md](refuted.md); claims whose verification kept failing are listed in [unverified.md](unverified.md) as leads.

Only the verified findings below are the review's result. Titles are the verifiers' one-line restatements; the mechanism, trigger and expected behaviour are the original reviewer's, edited only for length.

## Totals

| | Count |
|---|---|
| Verified findings | 381 |
| … critical | 16 |
| … high | 101 |
| … medium | 165 |
| … low | 99 |
| … confirmed by executing a reproduction | 285 |
| … contradicting CLAUDE.md, the guide or a README | 32 |
| Claims refuted by verification | 40 |
| Claims left unverified | 34 |

Severity, as the judges used it: **critical** = data loss or corruption, a crash on a common path, audio dropouts from a realtime violation, or memory unsafety; **high** = a wrong result on a realistic path; **medium** = a wrong result on an edge path, or a false claim in docs or tests that hides a real gap; **low** = minor.

`✅` marks a finding whose fix and regression coverage have been completed. Findings without a check mark remain open.

## By area

| Area | Findings | critical | high | medium | low |
|---|---|---|---|---|---|
| [auris-gpui](auris-gpui.md) | 98 | 1 | 22 | 42 | 33 |
| [auris-session](auris-session.md) | 68 | 3 | 19 | 29 | 17 |
| [auris-core](auris-core.md) | 32 | 3 | 10 | 13 | 6 |
| [training](training.md) | 29 | 1 | 6 | 15 | 7 |
| [auris-compose](auris-compose.md) | 25 | 0 | 6 | 16 | 3 |
| [auris-engine](auris-engine.md) | 19 | 0 | 8 | 4 | 7 |
| [auris-dsp](auris-dsp.md) | 18 | 0 | 6 | 8 | 4 |
| [auris-toolbox](auris-toolbox.md) | 16 | 1 | 7 | 7 | 1 |
| [auris-clap](auris-clap.md) | 10 | 2 | 4 | 2 | 2 |
| [auris-vocal](auris-vocal.md) | 9 | 2 | 3 | 4 | 0 |
| [vendor/rustysynth](vendor-rustysynth.md) | 9 | 1 | 1 | 4 | 3 |
| [repo/ci/docs](repo-ci-docs.md) | 8 | 0 | 0 | 3 | 5 |
| [auris-sampler](auris-sampler.md) | 7 | 0 | 4 | 3 | 0 |
| [auris-cli](auris-cli.md) | 6 | 0 | 0 | 2 | 4 |
| [auris-i18n](auris-i18n.md) | 6 | 0 | 1 | 0 | 5 |
| [auris-gpu](auris-gpu.md) | 5 | 0 | 0 | 5 | 0 |
| [auris-io](auris-io.md) | 5 | 2 | 0 | 2 | 1 |
| [auris-synth](auris-synth.md) | 4 | 0 | 1 | 3 | 0 |
| [auris-agent](auris-agent.md) | 3 | 0 | 1 | 2 | 0 |
| [auris-singer](auris-singer.md) | 3 | 0 | 2 | 1 | 0 |
| [auris-mcp](auris-mcp.md) | 1 | 0 | 0 | 0 | 1 |

## By category

| Category | Findings |
|---|---|
| correctness | 140 |
| spec-mismatch | 83 |
| ui | 48 |
| dsp | 22 |
| security | 15 |
| persistence | 15 |
| realtime | 10 |
| theory | 10 |
| test-quality | 9 |
| concurrency | 8 |
| platform | 7 |
| other | 7 |
| lifecycle | 5 |
| architecture | 2 |

## Critical and high findings

The full entries, with trigger, mechanism and fix direction, are in the per-area files.

| ID | Severity | Area | Location | Finding |
|---|---|---|---|---|
| ✅ F-001 | critical | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/compose_sheet/lyrics.rs:32` | LyricsEdit stores a raw `dials.sections` index that any form edit (add/remove/move/retarget) silently reindexes via tidy_sections, silently dropping or misdirecting […] |
| ✅ F-005 | critical | [auris-clap](auris-clap.md) | `crates/auris-clap/src/bridge.rs:214` | Offline export activates hosted CLAP plugins at the live 512-frame block size while the offline renderer defaults to 1024-frame blocks, silently leaving roughly half of […] |
| ✅ F-008 | critical | [auris-clap](auris-clap.md) | `crates/auris-clap/src/plugin.rs:378` | Embedded CLAP GUI: if show() fails after set_parent(), HostWindow's Drop destroys the plugin's child HWND before gui.destroy() is called, risking a […] |
| ✅ F-009 | critical | [auris-vocal](auris-vocal.md) | `crates/auris-vocal/src/g2p.rs:95` | JapaneseDictionary::phonemes() misparses jpreprocess's NJD output as HTS labels, so any kanji lyric errors instead of singing on the live singer.rs render path. |
| ✅ F-013 | critical | [training](training.md) | `training/src/auris_singer/preprocess/pipeline.py:198` | A single corrupt WAV or non-UTF-8 transcript crashes preprocessing unhandled, discarding every already-computed .npz because the dataset manifest is written only after […] |
| ✅ F-014 | critical | [auris-core](auris-core.md) | `crates/auris-core/src/project/curve.rs:28` | CurvePoint lacks the finite-value guard AutomationLane and set_param already enforce, so a NaN bend/controller value round-trips to JSON `null` and makes the whole […] |
| ✅ F-015 | critical | [auris-vocal](auris-vocal.md) | `crates/auris-vocal/src/frames.rs:189` | Unbounded frame_hop from a project file lets render_frames allocate unboundedly just from viewing a clip in the piano roll, hanging the GUI. |
| ✅ F-017 | critical | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:1944` | edit_notes' `beats`/`beat` fields have no upper bound, letting a single tool call overflow Ticks arithmetic in fit_length_to_notes and crash or corrupt the session. |
| ✅ F-018 | critical | [vendor/rustysynth](vendor-rustysynth.md) | `vendor/rustysynth/src/instrument.rs:33` | A crafted/corrupt SF2 file's zone-index fields cause an unchecked out-of-bounds slice panic in vendor/rustysynth's Instrument/Preset construction, crashing the whole app […] |
| ✅ F-019 | critical | [auris-core](auris-core.md) | `crates/auris-core/src/project/mod.rs:579` | repair_id_counter's unchecked `highest + 1` panics (debug) or silently wraps to 0 (release) on a u64::MAX id in a loaded project file. |
| ✅ F-022 | critical | [auris-io](auris-io.md) | `crates/auris-io/src/soundfont.rs:104` | check_chunk's LIST-descent doesn't clamp to the enclosing LIST's end, and its generic leaf handler mis-tracks ifil/iver's true byte length, letting a crafted .sf2 desync […] |
| ✅ F-023 | critical | [auris-core](auris-core.md) | `crates/auris-core/src/time.rs:668` | align_to_bars in crates/auris-core/src/time.rs can silently drop a time-signature change and relocate another whenever an earlier point's bar-rounding overshoots past […] |
| ✅ F-024 | critical | [auris-session](auris-session.md) | `crates/auris-session/src/session/hosted.rs:583` | HostedSlot::incoming silently force-loads stale document state onto a reused plugin instance when reclaim has already moved the live instance into spare, discarding […] |
| ✅ F-025 | critical | [auris-session](auris-session.md) | `crates/auris-session/src/session/record.rs:966` | Recording with a count-in while the output falls back to `start_silent` silently trims real captured audio, since the shared count-in atomic is never decremented and is […] |
| ✅ F-026 | critical | [auris-session](auris-session.md) | `crates/auris-session/src/session/mod.rs:269` | Session's field order drops the retired-graph channel before the live cpal stream, letting the audio callback free a RenderGraph on the realtime thread during shutdown. |
| ✅ F-313 | critical | [auris-io](auris-io.md) | `crates/auris-io/src/soundfont.rs:62` | load_soundfont has no catch_unwind around SoundFont::new, so a malformed .sf2 with honest chunk sizes but bad pdta indices panics rustysynth and crashes the whole app on […] |
| ✅ F-002 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/hosted.rs:611` | `HostedSlot::reclaim` overwrites `self.spare` via plain `Option` assignment without checking if it still holds a live effect, permanently leaking that CLAP plugin […] |
| ✅ F-003 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/clip.rs:122` | A saved audio clip with an out-of-range loop_end hangs graph build on project open, with no cap in loop_passes and no validation on deserialize. |
| ✅ F-004 | high | [vendor/rustysynth](vendor-rustysynth.md) | `vendor/rustysynth/src/zone.rs:25` | Zone::new panics on out-of-range slice index when a SoundFont's bag/generator chunk counts disagree, crashing Auris Studio instead of rejecting the broken file. |
| ✅ F-006 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:2515` | edit_notes' placed_at() only lower-bounds `beat`, so a large finite beat overflows Ticks arithmetic (panic in dev, silent wraparound corruption in release). |
| ✅ F-011 | high | [auris-singer](auris-singer.md) | `crates/auris-singer/src/metadata.rs:284` | VoiceInfo::parse trusts n_speakers:u32 from voice metadata unchecked and calls speakers(), letting a crafted/corrupt value drive a multi-gigabyte Vec<String> allocation […] |
| ✅ F-012 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/singer.rs:531` | `arrange()` in score.rs indexes `frames.f0_hz[at]`/`frames.energy[at]` unchecked, so a hand-edited SingerFrames JSON with mismatched array lengths panics `auris […] |
| ✅ F-016 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/compose_sheet/dials.rs:501` | Renaming or removing a part on the compose sheet leaves stale names in section.parts, so Write silently drops that part from any section that named it, with no error […] |
| ✅ F-021 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/agent_chat.rs:747` | Stale agent 'Reload' button keeps a discarded project's path and can silently replace a different open project's unsaved edits. |
| ✅ F-027 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/piano_roll.rs:1033` | Setting Delete=Double-click is unreachable on singer clips because begin_note_drag's lyric-prompt branch unconditionally returns before the delete check. |
| ✅ F-028 | high | [training](training.md) | `training/src/auris_singer/host_eval.py:694` | The song render in host_eval.py sings the whole concatenated song in rows[0]'s speaker, silently mismatching every other utterance's own voice. |
| ✅ F-029 | high | [auris-compose](auris-compose.md) | `crates/auris-compose/src/rhythm.rs:607` | swing_offset returns a negative (early) shift for percent < 50, rushing offbeats instead of delaying them as documented and tested. |
| ✅ F-032 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/tracks.rs:347` | set_track_instrument wipes instrument_state and file unconditionally, even when instrument_id is unchanged, losing user parameter values on a no-op call. |
| ✅ F-033 | high | [auris-i18n](auris-i18n.md) | `crates/auris-i18n/src/strings.rs:57` | English Compose-from-Lyrics hint shows the raw internal token "secondary-Return" instead of a real keystroke like Ctrl/⌘-Return. |
| ✅ F-034 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/mixer.rs:171` | Right-click on any mixer strip or fader always shows Add Track instead of the track/param menu, since the outer mixer div's handler overwrites the inner one with no […] |
| ✅ F-035 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/clip.rs:988` | Project::split_clip clones bend/controller curves verbatim into both MIDI clip halves instead of rebasing/trimming them like notes, corrupting automation on split. |
| ✅ F-036 | high | [auris-vocal](auris-vocal.md) | `crates/auris-vocal/src/frames.rs:340` | phoneme_at's segments.last() fallback keeps release=true forever past the last segment, forcing full gain on a pinned-short trailing consonant. |
| ✅ F-039 | high | [training](training.md) | `training/src/auris_singer/preprocess/pipeline.py:217` | The "too short" guard only enforces wav.numel() >= hop_length, not the (n_fft-hop_length) reflect-pad width frame_energy needs, so a short utterance can crash the whole […] |
| ✅ F-042 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/typing.rs:421` | MusicalTyping::bend() is computed globally across all held keys but attributed to one TrackId at both press and release, misdirecting pitch-bend to the wrong track when […] |
| ✅ F-043 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/settings_window.rs:244` | Settings window's apply_audio sets self.audio before the apply succeeds and never rolls it back on Err, so the UI shows a rejected audio preference as active. |
| ✅ F-046 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/generated.rs:47` | generate_clip's WrongTrackKind error hardcodes actual: "an audio track" even when the rejected track is actually a Singer or Bus track, unlike the sibling add_midi_clip […] |
| ✅ F-047 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/settings_window.rs:1024` | Keys-tab section headings for the same group render twice, non-adjacently, because BINDABLE's declaration order isn't grouped as found_commands assumes. |
| ✅ F-049 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/curve.rs:157` | curve_events omits the leading (0, first.value) event, so a curve's held-flat lead-in before its first point is silently dropped from playback scheduling. |
| ✅ F-050 | high | [auris-core](auris-core.md) | `crates/auris-core/src/time.rs:169` | Seconds::format_clock derives minutes and seconds from uncoordinated roundings, printing invalid strings like "2:60.000" instead of carrying to "3:00.000". |
| ✅ F-055 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/track.rs:508` | Project::set_hosted_instrument skips remove_instrument_automation, so old lanes keep driving the newly hosted CLAP plugin's parameters by stale index. |
| ✅ F-056 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/track.rs:496` | set_hosted_instrument swaps a track's plugin without calling remove_instrument_automation, so stale automation lanes drive the new plugin's unrelated parameters. |
| ✅ F-057 | high | [auris-dsp](auris-dsp.md) | `crates/auris-dsp/src/gain.rs:112` | GainPan ramps gain/pan via SmoothedValue but applies phase-invert and width raw per-block, causing an audible step/pop on toggle. |
| ✅ F-058 | high | [auris-vocal](auris-vocal.md) | `crates/auris-vocal/src/openjtalk.rs:14` | openjtalk_phoneme has no arms for OpenJTalk's uppercase devoiced-vowel labels A/I/U/E/O, so ordinary Japanese text (です, ます, し...) fails g2p with an unknown-phoneme error. |
| ✅ F-060 | high | [auris-compose](auris-compose.md) | `crates/auris-compose/src/gm.rs:248` | Drum `program` values between GM kit boundaries are silently corrupted to the nearest lower kit's number on TOML save/reparse, with no validation or error. |
| ✅ F-061 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/levels.rs:195` | balance_levels() uses cancel_transaction() on failure, leaving partially-written faders (already sent to the audio engine) applied and unrecorded instead of reverted. |
| ✅ F-062 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/lyrics.rs:368` | measure_lyrics leaks a partially-unreadable line's mora counts into the totals even though that line itself reports as None. |
| ✅ F-063 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/record.rs:1057` | Undoing a track-add (or any undo removing an armed track) leaves a stale entry in Session::armed that adopt_project never clears, so take_tracks() returns only that dead […] |
| ✅ F-064 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/monitor.rs:90` | remove_track never clears self.monitored, so deleting a monitored track permanently holds the input device open (only fixable by stop_monitoring(), which kills every […] |
| ✅ F-065 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/harmony.rs:227` | stamp_named_progression picks the respelling key from the caller's raw tick but writes at a separately re-snapped tick, so charts near a mode boundary can be spelled for […] |
| ✅ F-066 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:2519` | strip_by_name treats any track named "master" as the master bus, silently misrouting set_level/set_effect/section_gain to the wrong target. |
| ✅ F-067 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/menu.rs:652` | macOS native Edit/View/Transport menus never disable Undo/Redo or check toggles, because menus() is built once from MenuState::default() and gpui's MenuItem::Action has […] |
| ✅ F-069 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/agent_chat.rs:846` | render_agent_chat re-runs load_preferences() every repaint while unconfigured, wiping the provider/URL/API-key-env fields on every keystroke or dropdown pick until a […] |
| ✅ F-070 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/context_menu/menu.rs:329` | ContextMenu::origin's fallback clamp can place the menu directly over the anchor point in narrow viewports, contradicting its own doc comment's purpose. |
| ✅ F-071 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/context_menu/command.rs:956` | Most clip-context-menu rows (e.g. ToggleClipMute, SplitClipAtPlayhead) act only on the right-clicked clip, ignoring the rest of a multi-clip selection the menu's own […] |
| ✅ F-072 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/context_menu/tracks.rs:568` | Mixer's Add-Send "+" button silently does nothing in any project with zero bus tracks, since the empty menu it builds is dropped by open_menu with no feedback. |
| ✅ F-073 | high | [training](training.md) | `training/src/auris_singer/data/datamodule.py:110` | DistributedBucketSampler.epoch is set once at construction and never advanced, because use_distributed_sampler=False (train.py:103) disables Lightning's automatic […] |
| ✅ F-075 | high | [auris-dsp](auris-dsp.md) | `crates/auris-dsp/src/reverb.rs:324` | Reverb reads mix/width/damping/room-size/pre-delay once per block instead of ramping them, causing zipper noise and pre-delay read-head jumps on parameter changes, […] |
| ✅ F-076 | high | [auris-core](auris-core.md) | `crates/auris-core/src/time.rs:684` | SignatureMap::from_points/align_to_bars does unchecked i64 multiply-add on tick values from deserialized project files, panicking (debug) or silently corrupting bar […] |
| ✅ F-077 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/device.rs:800` | Automation writes to a hosted CLAP effect's parameter can change its reported latency without ever marking `latency_stale`, unlike the discrete SetEffectParam command […] |
| ✅ F-078 | high | [auris-singer](auris-singer.md) | `crates/auris-singer/src/score.rs:206` | SingerFrames.f0_hz/energy are indexed without bounds checks in chunk_ranges/arrange, panicking on a hand-edited or externally-written file whose curve arrays are shorter […] |
| ✅ F-079 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/app.rs:1721` | A plain click on an overlapping unfaded clip unintentionally writes a crossfade and an undo step via end_drag's ungated ClipMove branch (app.rs:1721). |
| ✅ F-080 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/piano_roll.rs:2231` | press_curve_lane hit-tests curve points against the snapped click tick, not the raw press position, so off-grid points become unclickable under coarse grid/zoom. |
| ✅ F-081 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/arrangement/geometry.rs:207` | fade_handle_at ignores loop passes, so a phantom fade-out grab hijacks resize on looped clips. |
| ✅ F-083 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/clip.rs:1349` | notes_digest omits phoneme_seconds/scoop/fall/vibrato despite claiming "every field", so ornament-only hand edits are silently discarded on resize/regenerate. |
| ✅ F-084 | high | [auris-sampler](auris-sampler.md) | `crates/auris-sampler/src/sampler.rs:676` | stop_everything(false) calls synth.note_off_all(false) immediately, starting the font's ~1ms release under the user's configured Release stage instead of deferring it […] |
| ✅ F-085 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/piano_roll.rs:1146` | Piano-roll note creation snaps the clip-relative tick instead of the absolute one, so new notes miss the drawn grid whenever clip_start isn't grid-aligned. |
| ✅ F-086 | high | [auris-dsp](auris-dsp.md) | `crates/auris-dsp/src/compressor.rs:241` | Compressor makeup gain is added unsmoothed to the envelope-filtered gain each frame, causing a zipper-noise click whenever makeup_db changes between blocks. |
| ✅ F-087 | high | [auris-dsp](auris-dsp.md) | `crates/auris-dsp/src/eq.rs:335` | Re-enabling a disabled EQ band resumes its biquad from stale, frozen s1/s2 state, producing an audible click/thump instead of a clean rejoin. |
| ✅ F-088 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/graph/strip.rs:278` | Soloing a track during playback rebuilds the graph and settles MuteFade instead of sliding it, producing an audible click, contradicting strip.rs:251's own doc comment. |
| ✅ F-089 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/capture.rs:702` | Input stream setup lacks output's Fixed-buffer-size retry fallback, so WASAPI-class devices fail to open for recording instead of falling back like output does. |
| ✅ F-090 | high | [auris-clap](auris-clap.md) | `crates/auris-clap/src/bridge.rs:306` | Bridge::render hard-codes `None` for CLAP's transport argument, so hosted plugins never see host tempo, playhead, or transport state. |
| ✅ F-091 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/monitor.rs:160` | publish_monitors drops InputChannels::count, so a mono-armed track's monitor always plays a stereo pair and bleeds in the next device channel. |
| ✅ F-092 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/files.rs:342` | save_as_replacing repoints self.path before the disk write but leaves self.dirty unset on write failure, silently disarming autosave for the unsaved new location. |
| ✅ F-093 | high | [auris-agent](auris-agent.md) | `crates/auris-agent/src/main.rs:839` | converse() in auris-agent has no request timeout, so a black-holed LLM host hangs the agent process (and the panel's parked thread) forever, unlike list_models which is […] |
| ✅ F-094 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/text_area.rs:199` | Lyrics/prompt text areas hard-clip past max_rows with no vertical scroll, hiding text and caret once content exceeds 12 lines. |
| ✅ F-095 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/context_menu/clips.rs:295` | Note context menu titled "N notes" still applies ornament/lyric rows to only the single note under the pointer, silently dropping the rest of the selection. |
| ✅ F-096 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/device.rs:516` | Windows output devices whose default WASAPI mix format is I24/I32/I64/F64/U8 (e.g. a common "24-bit" device default) silently fall back to a fully silent audio engine […] |
| ✅ F-097 | high | [auris-vocal](auris-vocal.md) | `crates/auris-vocal/src/openjtalk.rs:23` | `openjtalk_phoneme` has no `kw`/`gw` arms, so any lyric OpenJTalk analyzes with a labialized velar mora hard-errors the whole line instead of singing it. |
| ✅ F-099 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/graph/strip.rs:70` | SmoothedGain::advance() ignores segment length, so loop/count-in edges collapse fader, pan and send ramps into an audible hard step instead of a smooth glide. |
| ✅ F-100 | high | [auris-clap](auris-clap.md) | `crates/auris-clap/src/bridge.rs:244` | A parameter write's `changed` flag is cleared via mem::take before delivery is confirmed, so it is lost forever if `ensure_processing_started()` fails that block. |
| ✅ F-101 | high | [auris-compose](auris-compose.md) | `crates/auris-compose/src/parts/comp.rs:305` | Pushed-chord anticipation in auris-compose's comp() deletes (not trims) a prior close chord's onset when chords are spaced under half a beat apart. |
| ✅ F-102 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/clips.rs:51` | set_curve_point/move_curve_point clamp NaN-through instead of using the codebase's own finite_unit pattern, letting a NaN curve value get saved as JSON null that then […] |
| ✅ F-103 | high | [auris-synth](auris-synth.md) | `crates/auris-synth/src/chiptune.rs:278` | Chiptune::note_on stores the new note's target pitch into last_frequency instead of the previous voice's live gliding frequency, so rapid portamento runs jump-start from […] |
| ✅ F-104 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/device.rs:423` | device.rs:423 leaves block_frames unclamped when SupportedBufferSize::Unknown, letting an oversized/corrupted value drive a huge eager allocation in AudioEngine::new […] |
| ✅ F-106 | high | [auris-clap](auris-clap.md) | `crates/auris-clap/src/ports.rs:110` | Unbounded plugin-reported port/channel/parameter counts size Vec allocations with no cap, letting a buggy or malicious CLAP plugin crash the whole DAW via an OOM-abort […] |
| ✅ F-108 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/assets.rs:154` | collect_assets copies arbitrary local files named by an untrusted .auris project's External asset paths into Audio/ without ever validating them as audio or SoundFont […] |
| ✅ F-110 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/settings_window.rs:1308` | Resetting a command's keybinding while a capture is armed leaves the capture live, so the next keystroke silently rebinds the just-reset command. |
| ✅ F-112 | high | [auris-compose](auris-compose.md) | `crates/auris-compose/src/phrase.rs:207` | write_phrase floor-divides length into bars, so resizing a Lead/Kick/Snare/Hat/Drums clip to a non-bar-aligned length silently leaves its fractional tail with no notes. |
| ✅ F-113 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/library.rs:857` | Plugin-open state keyed by scan-list index (not file identity) lets adding/removing a plugin folder auto-load an unrelated .clap binary with no user gesture. |
| ✅ F-115 | high | [training](training.md) | `training/src/auris_singer/phoneme_levels.py:90` | phoneme_levels.py classes devoiced Japanese vowels as full vowels, so a whispered vowel becomes the loudness reference for the consonant before it, under-correcting […] |
| ✅ F-116 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:2330` | auris-toolbox's `sing` tool result splices unsanitized voice-card name/speaker text from an untrusted .onnx file verbatim into agent-facing output — an indirect […] |
| ✅ F-118 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/mod.rs:830` | Re-entrant begin_transaction overwrites an open transaction, so its already-applied edit can become permanently unrecorded and unrendered. |
| ✅ F-119 | high | [training](training.md) | `training/src/auris_singer/utils/audio.py:65` | A single too-short utterance (exactly one hop of samples) crashes `training`'s whole preprocessing run via an unhandled reflect-pad RuntimeError in […] |
| ✅ F-121 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/device.rs:703` | Session's field-order drop disconnects EngineHandle before the cpal stream stops, so retired graphs/buffers can be freed on the audio callback thread at shutdown. |
| ✅ F-123 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:326` | `render`'s stems/output path has no containment check, so it can silently overwrite the open project's own Audio/ assets via `write_wav`'s unconditional rename. |
| ✅ F-314 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:1558` | `add_part`'s unbounded `bars` argument lets one MCP/CLI call drive billions of generated notes, OOM-crashing the shared toolbox process. |
| ✅ F-316 | high | [auris-compose](auris-compose.md) | `crates/auris-compose/src/parts/drums.rs:115` | drums.rs:115 gates the snare's ending fill on the snare's own pattern having hits, so the shipped "sparse" groove (empty snare row) permanently silences the fill despite […] |
| ✅ F-317 | high | [auris-core](auris-core.md) | `crates/auris-core/src/theory/numeral.rs:519` | degree_of never checks accidental 0 against the major reference scale, so borrowed major-scale degrees in minor/modal keys are mislabeled with double accidentals instead […] |
| ✅ F-320 | high | [training](training.md) | `training/src/auris_singer/lightning_module.py:154` | load_weights unpickles an unvalidated --init-from/--resume checkpoint via torch.load(weights_only=False), giving arbitrary code execution on a crafted file. |
| ✅ F-322 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/agent_chat.rs:524` | spawn_link discards auris-agent's stderr, so a startup failure (e.g. a misconfigured api_key_env) surfaces in chat only as the uninformative "The agent process ended." |
| ✅ F-326 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:2416` | track_by_name in auris-toolbox silently resolves to the first of two same-named tracks, so by-name tools can act on the wrong one. |
| ✅ F-327 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/clips.rs:566` | trim_clip_start rebases MIDI notes on front-trim but leaves bend/controller CurvePoints at stale offsets, misaligning automation with the trimmed clip's notes. |
| ✅ F-328 | high | [auris-toolbox](auris-toolbox.md) | `crates/auris-toolbox/src/lib.rs:1933` | edit_notes validates a new note's start against the clip but not its end, letting a long `beats` value silently grow the clip via fit_length_to_notes with no mention in […] |
| ✅ F-329 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/hosted.rs:853` | set_hosted_instrument swaps a CLAP plugin's id/state but skips remove_instrument_automation, leaving old-plugin automation lanes driving the new plugin's unrelated […] |
| ✅ F-330 | high | [auris-clap](auris-clap.md) | `crates/auris-clap/src/plugin.rs:796` | Hosted CLAP stepped/enum parameters get ParamUnit::Choice with an empty `choices` list, so their picker menu renders with zero selectable options. |
| ✅ F-331 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/accompany.rs:165` | accompany() snaps the chord/key write and the generated clip's start through two different grids, so parts can be composed against the wrong harmony. |
| ✅ F-333 | high | [auris-dsp](auris-dsp.md) | `crates/auris-dsp/src/distortion.rs:146` | Distortion applies drive/output/mix as block-constant steps with no SmoothedValue ramp, causing zipper-noise clicks when those parameters are automated, unlike Delay and […] |
| ✅ F-339 | high | [auris-dsp](auris-dsp.md) | `crates/auris-dsp/src/limiter.rs:115` | Limiter::prepare has no upper bound on sample_rate, so a corrupted .auris file's sample_rate can abort the render/export process via a multi-GB allocation. |
| ✅ F-341 | high | [auris-session](auris-session.md) | `crates/auris-session/src/session/mod.rs:1083` | Session::open clears self.hosted for id-reuse safety but never clears self.armed/self.monitored, so a new project can inherit stale arm/monitor state via colliding […] |
| ✅ F-343 | high | [auris-sampler](auris-sampler.md) | `crates/auris-sampler/src/sampler.rs:148` | is_reserved() only shields CC11/43 (expression), letting automation on CC0/6/0x64/0x65 silently hijack the sampler's own bank-select and pitch-bend-range RPN state. |
| ✅ F-345 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/arrangement/lanes.rs:339` | clip_grab_at's symmetric end-edge check lets a press past a looped clip's raw end start a resize drag with no resize cursor shown there. |
| ✅ F-346 | high | [auris-gpui](auris-gpui.md) | `crates/auris-gpui/src/ui/compose_sheet/dials.rs:878` | Gain dial's clamped display fraction is reused as drag start, so touching a part with legally out-of-range gain (-60..12 dB) silently snaps it into -30..0 dB on first […] |
| ✅ F-347 | high | [auris-sampler](auris-sampler.md) | `crates/auris-sampler/src/sampler.rs:510` | Turning ADSR shaping off on a held sampler note snaps channel expression to full before the font's own release begins, producing an audible gain-jump click. |
| ✅ F-350 | high | [auris-sampler](auris-sampler.md) | `crates/auris-sampler/src/sampler.rs:658` | let_go() misattributes a stolen shaped note's slot-less state to "never shaped", letting its note-off silence an unrelated held note of the same pitch. |
| ✅ F-351 | high | [auris-engine](auris-engine.md) | `crates/auris-engine/src/device.rs:190` | discard_pending can race the still-live CoreAudio callback thread and silently steal a queued engine command during a device disconnect. |
| ✅ F-352 | high | [auris-core](auris-core.md) | `crates/auris-core/src/project/routing.rs:390` | repair_routing is O(n^3) in track count and runs unconditionally, synchronously, on every project open, even when routing is already valid. |
| ✅ F-375 | high | [auris-compose](auris-compose.md) | `crates/auris-compose/src/rhythm.rs:279` | Pattern::at_in_bar's middle==0 branch hard-codes every interior beat to the pattern's first beat instead of cycling, silencing the six-eight groove's snare backbeat […] |

## Rules the project wrote down that the code breaks

Each of these findings was judged to contradict a rule stated in `CLAUDE.md`, `auris_session::guide` or a README (findings that contradict only a local doc comment carry the quote in their entry instead). The quoted rule is in the entry.

- ✅ F-009 (critical, [auris-vocal](auris-vocal.md)): JapaneseDictionary::phonemes() misparses jpreprocess's NJD output as HTS labels, so any kanji lyric errors instead of singing on the live singer.rs […]
- ✅ F-026 (critical, [auris-session](auris-session.md)): Session's field order drops the retired-graph channel before the live cpal stream, letting the audio callback free a RenderGraph on the realtime […]
- ✅ F-060 (high, [auris-compose](auris-compose.md)): Drum `program` values between GM kit boundaries are silently corrupted to the nearest lower kit's number on TOML save/reparse, with no validation or […]
- ✅ F-096 (high, [auris-engine](auris-engine.md)): Windows output devices whose default WASAPI mix format is I24/I32/I64/F64/U8 (e.g. a common "24-bit" device default) silently fall back to a fully […]
- ✅ F-103 (high, [auris-synth](auris-synth.md)): Chiptune::note_on stores the new note's target pitch into last_frequency instead of the previous voice's live gliding frequency, so rapid portamento […]
- ✅ F-116 (high, [auris-toolbox](auris-toolbox.md)): auris-toolbox's `sing` tool result splices unsanitized voice-card name/speaker text from an untrusted .onnx file verbatim into agent-facing output — […]
- F-326 (high, [auris-toolbox](auris-toolbox.md)): track_by_name in auris-toolbox silently resolves to the first of two same-named tracks, so by-name tools can act on the wrong one.
- F-339 (high, [auris-dsp](auris-dsp.md)): Limiter::prepare has no upper bound on sample_rate, so a corrupted .auris file's sample_rate can abort the render/export process via a multi-GB […]
- F-127 (medium, [training](training.md)): architecture.md's loss table lists KL (auxiliary) default as 1.0, but code, training.md, and the doc's own later prose all agree the default is 0.2.
- F-146 (medium, [repo/ci/docs](repo-ci-docs.md)): docs/features.md:1270 says 29 tools; auris-toolbox declares 30 pub mod tool modules, confirmed by both frontends' own count-assertion tests.
- F-154 (medium, [auris-session](auris-session.md)): record.rs's module doc still describes the pre-f0c836e single shared monitor ring, contradicting the current per-track monitor rings in monitor.rs […]
- F-157 (medium, [training](training.md)): EnvelopeLoss divides by the configured kernel count instead of the count actually used, silently underweighting the loss when a kernel exceeds the […]
- F-158 (medium, [auris-session](auris-session.md)): guide.rs and README.md claim 13 workspace crates and omit auris-singer from the architecture diagram; there are 18 crates.
- F-167 (medium, [auris-cli](auris-cli.md)): CLI `compose` silently drops --preset when a spec file is also given, unlike auris-toolbox's resolve_spec which rejects the combination outright.
- F-168 (medium, [auris-gpu](auris-gpu.md)): auris-gpu's crate/module docs still claim no shipped code reports the true peak, but Session::analyze has surfaced it via the MCP analyze tool since […]
- F-180 (medium, [auris-session](auris-session.md)): resize_clip's `available.max(1)` clamp forces length_frames=1 on an audio clip whose source has zero frames left, unlike trim_clip_start's explicit […]
- F-188 (medium, [auris-session](auris-session.md)): guide.rs (lines ~88 and 638) wrongly claims frontend binaries call default_registry; only Session::new does, contradicting the guide's own later […]
- F-205 (medium, [vendor/rustysynth](vendor-rustysynth.md)): rustysynth fork README's closed list of touched files (README.md:22-23) omits src/error.rs, which adds the InvalidModulatorList variant actually […]
- F-220 (medium, [training](training.md)): host.py's own MODEL_SILENCE = "<sil>" literal is a third, untested copy alongside ipa.SIL and Rust's score.rs constant.
- F-236 (medium, [repo/ci/docs](repo-ci-docs.md)): aesthetics.py keys per-file scores by bare filename stem, so same-named WAVs in different subdirectories silently overwrite each other in the […]
- F-340 (medium, [auris-session](auris-session.md)): A clip with start=i64::MIN, only reachable via a hand-edited/corrupt .auris file since load_project never validates clip starts, panics on drag via […]
- F-361 (medium, [auris-gpu](auris-gpu.md)): auris-gpu's crate and module docs falsely claim compute_peaks reruns on every zoom/scroll, when it actually runs once per source and is cached in […]
- F-367 (medium, [auris-toolbox](auris-toolbox.md)): mixer/set_send in auris-toolbox never report send automation, unlike the parallel gain/pan/effect handling.
- F-373 (medium, [auris-session](auris-session.md)): guide.rs:1238 wrongly claims an old build would silently misread a post-AssetPath path as absolute; it actually hard-fails to deserialize the whole […]
- F-387 (medium, [auris-compose](auris-compose.md)): A pushed Held-figure chord is struck at 0.9x velocity instead of the intended 0.7x held multiplier, an unintended ~29% loudness jump.
- F-135 (low, [repo/ci/docs](repo-ci-docs.md)): release.yml grants contents:write to all four jobs via workflow-root permissions, though only publish's release-creation step needs it.
- F-264 (low, [auris-gpui](auris-gpui.md)): Panel::command's doc comment says "all five" panels but Panel::ALL has held six since the Agent panel shipped.
- F-266 (low, [vendor/rustysynth](vendor-rustysynth.md)): PresetRegion::get_initial_filter_cutoff_frequency returns a raw multiplying factor instead of Hz, but the method is dead code never called on any […]
- F-285 (low, [auris-session](auris-session.md)): guide.rs:638 wrongly claims frontends call default_registry directly, duplicating the same misattribution already at line 89-90.
- F-430 (low, [auris-core](auris-core.md)): TimeSignature::COMMON doc claims the full meter menu is 400 rows; it's actually 32x5=160.
- F-443 (low, [vendor/rustysynth](vendor-rustysynth.md)): A malformed SF2 bag with non-monotonic generator_index silently empties a zone instead of raising a parse error.
- F-454 (low, [auris-gpui](auris-gpui.md)): stepped()'s unwrap_or(0) fallback can jump menu keyboard highlight to the wrong row if a row's enabled state changes while the menu is open.

## Reading a finding

Every entry has the same shape: the location and category; what a user of the DAW, the CLI, the MCP server or the trainer observes; the concrete trigger; the mechanism in the code; what correct behaviour is; the smallest fix the judge could see; and, where one applies, the written rule it breaks. IDs are stable across the files, so a fix commit can cite `F-014`.
