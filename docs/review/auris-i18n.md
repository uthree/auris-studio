# Review findings: auris-i18n

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 6 verified findings: 1 high, 5 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| ✅ F-033 | high | `crates/auris-i18n/src/strings.rs:57` | English Compose-from-Lyrics hint shows the raw internal token "secondary-Return" instead of a real keystroke like Ctrl/⌘-Return. |
| ✅ F-130 | low | `crates/auris-i18n/src/strings.rs:845` | NoCycleToExport's Japanese string hardcodes Mac-only "option" instead of branching by platform like PointerGesture::OptionClick.label() does. |
| ✅ F-148 | low | `crates/auris-i18n/src/strings.rs:1340` | SamplerEnvelopeOn warning string still says 15-note polyphony after SLOTS shrank to 14 in commit 1f41ec7. |
| ✅ F-268 | low | `crates/auris-i18n/src/audio.rs:83` | is_known's doc comment cites "dB" as a same-in-both-languages table entry, but no table in audio.rs contains any dB key — only Q rows exist. |
| ✅ F-289 | low | `crates/auris-i18n/src/lib.rs:97` | from_environment lets an unsupported LC_ALL value fall through to LC_MESSAGES/LANG instead of taking priority as its own doc comment says. |
| ✅ F-448 | low | `crates/auris-i18n/src/controller.rs:18` | NOTABLE's doc comment groups its 8 controllers into 4 named categories that only cover 7, leaving Pan (CC 10) uncategorized. |

### ✅ F-033 · high · English Compose-from-Lyrics hint shows the raw internal token "secondary-Return" instead of a real keystroke like Ctrl/⌘-Return.

`crates/auris-i18n/src/strings.rs:57` · ui · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** Any English-locale user opening the "Compose from Lyrics" prompt sees the hint text "...secondary-Return composes, a fresh take each run" — an internal keymap-storage token, not a real keyboard shortcut — so they cannot tell whether to press Ctrl+Return, Cmd+Return, or something else. The Japanese locale does not have this problem since it was hand-written with real key names.

**Trigger.** Open File → Compose from Lyrics (or the lyrics field it feeds) in the English interface; the hint line under the text box is shown as-is.

**Mechanism.** `HintComposeLyrics` reads `en: "One line per phrase (、！？ break too) · secondary-Return composes, a fresh take each run"`. `secondary-` is the internal *storage* spelling for the platform command modifier (CLAUDE.md: "secondary-s is stored; actions::normalise_keystroke turns it into what the keyboard reports... and actions::menu_keystroke into ⌘S or Ctrl+S, for reading"; auris-gpui/src/actions.rs's own doc on `menu_keystroke` says of the `secondary-` spelling "it is not a form anyone would put in a menu"). `Key::hint()` in crates/auris-gpui/src/ui/prompt.rs returns this Key untouched, and `render_prompt_hint` (prompt.rs:1463-1468) renders it with a bare `self.t(hint)` — no call to `menu_keystroke` or any substitution — so the literal word "secondary" reaches the screen.

**Expected.** The hint should show the platform's actual menu-form keystroke (⌘-Return on macOS, Ctrl-Return elsewhere), the same way `piano_roll_hint`/`piano_roll_empty` in messages.rs take the gesture name as an argument rather than embedding a fixed word, and the same way `crates/auris-gpui/src/actions.rs::menu_keystroke` exists specifically to turn `secondary-` into something "anyone would put in a menu".

**Fix direction.** Render the hint through `actions::menu_keystroke` (which already converts the stored `secondary-` form to platform-correct display text/glyphs, e.g. Ctrl+Return / ⌘+Return) instead of hardcoding the literal token in the English string; interpolate the resolved keystroke into `HintComposeLyrics` at render time rather than baking a static string.

**Written rule it breaks.** The keystroke a user sees is not the keystroke that is stored. `secondary-s` is stored; `actions::normalise_keystroke` turns it into what the keyboard reports, for comparing, and `actions::menu_keystroke` into ⌘S or Ctrl+S, for reading.

### ✅ F-130 · low · NoCycleToExport's Japanese string hardcodes Mac-only "option" instead of branching by platform like PointerGesture::OptionClick.label() does.

`crates/auris-i18n/src/strings.rs:845` · spec-mismatch · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** A Japanese-language user on Windows or Linux who tries "Export Cycle" without first marking a cycle region sees a status message telling them to alt-drag (英語: "option ドラッグ") the ruler — but the word used, "option", names the Mac Option key that doesn't exist on their keyboard, while the parallel English string correctly says the generic "alt-drag". The gesture itself still works (it's bound to Modifiers::alt on all platforms), so this is a wording mismatch, not a broken feature — the user must infer that "option" means their Alt key.

**Trigger.** A Japanese-language user on Windows clicks Export Cycle (or presses its shortcut) with no cycle region set — `crates/auris-gpui/src/ui/commands.rs:2193` shows this exact status text.

**Mechanism.** `Key::NoCycleToExport`'s English text says "alt-drag the ruler to mark one" (platform-neutral: `alt` reads correctly for a Windows user), but the Japanese text says "ルーラーを option ドラッグして設定してください" — it names the Mac-specific key "option" outright, in half-width Latin, with no `cfg!`/platform branching. This is a fixed `Key`, used verbatim at crates/auris-gpui/src/ui/commands.rs:2193 (`self.set_failed_status(self.t(Key::NoCycleToExport))`), so there is no per-platform substitution at the call site either. The project's own established pattern for this exact concept — the ruler's alt-drag/option-drag gesture — is `PointerGesture::OptionClick`, which is rendered through `Key::GestureOptionClick` on macOS and `Key::GestureAltClick` on Windows via `cfg!(target_os = "macos")` in crates/auris-gpui/src/gestures.rs:146-147, and CLAUDE.md states the platform rule generally: "Never name a platform key… Decide with `cfg!`, not `#[cfg]`."

**Expected.** The Japanese message should name whichever key the platform actually calls it ("Alt" on Windows, "option"/⌥ on macOS), matching the platform-aware phrasing the codebase already uses for the identical gesture elsewhere (`Key::GestureOptionClick`/`Key::GestureAltClick`), instead of a single hardcoded Mac-only term that is wrong on every Windows machine.

**Fix direction.** Split NoCycleToExport's `ja` field the way `PointerGesture::OptionClick.label()` already does at crates/auris-gpui/src/gestures.rs:146-147 (`cfg!(target_os = "macos")` choosing between an Option-Key phrase and an Alt-Key phrase), or simply reword the `ja` string to a platform-neutral term (e.g. "Alt キーを押しながら" instead of "option"), matching the English string's already-generic "alt-drag" wording.

**Written rule it breaks.** Decide with `cfg!`, not `#[cfg]`, wherever it is a choice rather than an API that only exists on one platform.

**Verifier's correction.** No correction needed; the claim's location, mechanism, trigger, and consequence all check out as described.

### ✅ F-148 · low · SamplerEnvelopeOn warning string still says 15-note polyphony after SLOTS shrank to 14 in commit 1f41ec7.

`crates/auris-i18n/src/strings.rs:1340` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user who switches on the sampler's envelope sees a warning claiming "15-note polyphony" when the actual cap, per the `SLOTS` table, is 14 shaped notes; a project relying on the stated number to plan arrangement density could be off by one voice, but nothing crashes, drops audio, or corrupts data.

**Trigger.** Open the SoundFont/sampler plugin window and switch its envelope on (`crates/auris-gpui/src/ui/plugin_window.rs:104` shows this Key as the caution text whenever `caution(SAMPLER_ID, true)` is called).

**Mechanism.** `SamplerEnvelopeOn` reads `en: "Envelope on: 15-note polyphony, and drum choke groups stop working."` / `ja: "...同時発音は15音まで..."`. The actual per-note channel slot table shrank from 15 to 14 entries in commit 1f41ec7 ("Keep the unshaped channel out of the slot table"): `crates/auris-sampler/src/sampler.rs:40` now declares `const SLOTS: [i32; 14] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];`, and that commit's own message states "Shaped polyphony drops from fifteen to fourteen, the price of a channel that is never spoken over." The i18n string was never updated to match.

**Expected.** The string should read 14, matching `SLOTS: [i32; 14]` and the commit that reduced it — i.e. the doc-vs-implementation drift the LENS calls out: behaviour changed in git history (1f41ec7) while this string did not.

**Fix direction.** Edit `SamplerEnvelopeOn` in crates/auris-i18n/src/strings.rs (en and ja) to say 14-note polyphony instead of 15, matching `SLOTS`'s 14 entries and its own doc comment ("Fourteen notes is thin...").

**Written rule it breaks.** Shaped polyphony drops from fifteen to fourteen, the price of a channel that is never spoken over. (commit 1f41ec7 message, and the doc comment above `SLOTS`: "Fourteen notes is thin next to the 128 voices the library will hold")

### ✅ F-268 · low · is_known's doc comment cites "dB" as a same-in-both-languages table entry, but no table in audio.rs contains any dB key — only Q rows exist.

`crates/auris-i18n/src/audio.rs:83` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No end user or runtime behavior is affected — is_known still correctly reports whether a term appears in any table. A developer reading the doc comment on is_known, or relying on it to understand the "considered but identical" exception, is told that "dB" is one of the terms handled this way and may assume dB-labeled parameters are covered by the completeness check, when in fact only " Q"-suffixed entries are ever wired in and tested.

**Trigger.** Read the doc comment on `is_known` (or run `cargo doc`) expecting to find where "dB" is looked up.

**Mechanism.** The doc comment on `is_known` reads: "a handful of audio terms — `Q`, `dB` — are the same word in Japanese, so a completeness check has to ask whether the term was *considered*, not whether it changed." `is_known` only searches `PLUGIN_NAMES`, `PLUGIN_DESCRIPTIONS`, `PARAMETERS`, `CHOICES` and `CATEGORIES`. None of those five tables contains an entry keyed on `"dB"` (confirmed by `grep -n 'dB' crates/auris-i18n/src/audio.rs`, which only matches the doc comment itself); the only identical-in-both-languages entries that exist are the six `" Q"`-suffixed `PARAMETERS` rows (`HP Q`, `LS Q`, `P1 Q`, `P2 Q`, `HS Q`, `LP Q`) and the CHOICES note-value rows, and the test that specifically polices this exception (`only_abbreviations_are_left_as_they_are`) only checks the `" Q"` suffix, not `"dB"`.

**Expected.** The doc comment should either name an entry that actually exists in one of the five tables, or drop the `dB` example; as written it asserts something the table contents do not back up.

**Fix direction.** Either add the missing dB-suffixed PARAMETERS entries (with the same identical-in-both-languages comment convention used for the Q rows) and extend only_abbreviations_are_left_as_they_are to also assert ends_with(" dB") where applicable, or, if dB was never meant to be a real table entry, edit the doc comment on is_known to drop the "dB" example and mention only Q.

**Written rule it breaks.** Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate) — implying doc comments are expected to be accurate, not just present.

**Verifier's correction.** No correction needed; the claim is accurate as stated.

### ✅ F-289 · low · from_environment lets an unsupported LC_ALL value fall through to LC_MESSAGES/LANG instead of taking priority as its own doc comment says.

`crates/auris-i18n/src/lib.rs:97` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user who sets LC_ALL to a locale Auris doesn't support (anything but English/Japanese) while LANG or LC_MESSAGES happens to be set to en/ja gets the UI in that lower-priority language instead of falling through to the OS locale or the English default — a wrong-but-plausible language pick, not a crash or missing text, and only reachable by someone deliberately juggling multiple locale env vars.

**Trigger.** Run the desktop app (or anything calling `Language::resolve(None)`) with `LC_ALL=fr_FR.UTF-8` and `LANG=ja_JP.UTF-8` set (LC_MESSAGES unset) — a plausible combination where a user or script has explicitly forced a third locale via LC_ALL while LANG still carries the OS default.

**Mechanism.** The doc comment states "`LC_ALL` outranks `LC_MESSAGES`, which outranks `LANG`, which is the order POSIX gives them." The implementation is `["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().filter_map(|name| std::env::var(name).ok()).find_map(|value| Language::from_locale(&value))...` — `filter_map` only drops variables that are *unset*; `find_map` then walks every *set* variable in priority order and keeps going past any that fails to name English or Japanese. So when a higher-priority variable is set but names a third language, it is silently skipped rather than treated as authoritative (with the app falling back to system locale/English), and a lower-priority variable's language is used instead.

**Expected.** Per the documented precedence, once the highest-priority *set* variable (LC_ALL) is found, its language (or the fallback, if unrecognised) should be final rather than the code continuing to lower-priority variables when the top one is set but unrecognised.

**Fix direction.** In from_environment, stop at the first *set* variable (via filter_map keeping only Ok values, then take the first regardless of whether from_locale maps it) rather than searching all three for a recognized language; only fall through to from_system_locale/default when none of LC_ALL/LC_MESSAGES/LANG is set. Concretely: find the first env var among the three that is Ok(_), and if its value doesn't map via from_locale, go straight to the fallback instead of trying the next-lower-priority variable.

**Written rule it breaks.** /// `LC_ALL` outranks `LC_MESSAGES`, which outranks `LANG`, which is the order POSIX gives them

### ✅ F-448 · low · NOTABLE's doc comment groups its 8 controllers into 4 named categories that only cover 7, leaving Pan (CC 10) uncategorized.

`crates/auris-i18n/src/controller.rs:18` · other · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer or translator reading the doc comment above `NOTABLE` in crates/auris-i18n/src/controller.rs gets a mildly inaccurate mental model of the eight menu controllers: they'd expect four clean categories (two wheels, pedals, two levels, filter) to exhaust the list, but CC 10 (Pan) fits none of them, and CC 2 (Breath) is folded into "wheels" though it's played by blowing, not turning a wheel. No user-facing string, menu order, or behavior is affected — only the prose explaining the array to a reader of the source.

**Trigger.** A maintainer reads the doc comment to reason about what NOTABLE contains -- e.g. deciding whether to add another performance controller -- and trusts the four-category breakdown as accurate.

**Mechanism.** The doc comment on NOTABLE (line 18) describes its order as "the two wheels, the pedals a foot reaches, the two levels, and the filter," but `NOTABLE = [1, 2, 4, 7, 10, 11, 64, 74]` is Modulation(wheel), Breath(a breath controller, not a wheel), Foot(pedal), Volume(level), Pan(stereo position -- not a pedal, not really a "level"), Expression(level), Sustain(a second pedal, positioned after the "levels" the prose puts it before), Brightness(filter). So CC 2 is called part of "the two wheels" though it is a breath controller, and CC 10 (Pan) matches none of the four named categories at all.

**Expected.** The doc's category breakdown should actually account for all eight entries in NOTABLE -- Breath is a breath controller, not a second wheel, and Pan needs its own mention rather than being silently absorbed into "the two levels."

**Fix direction.** Reword the doc comment at controller.rs:17-19 to either add "and pan" (or "the stereo position") as a fifth named category, or drop the precise category-counting language in favor of a looser description (e.g. "roughly the controls under a hand, plus pan") so the prose doesn't imply an exhaustive four-way partition of the eight entries.
