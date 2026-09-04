# Review findings: auris-vocal

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 9 verified findings: 2 critical, 3 high, 4 medium.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| ✅ F-009 | critical | `crates/auris-vocal/src/g2p.rs:95` | JapaneseDictionary::phonemes() misparses jpreprocess's NJD output as HTS labels, so any kanji lyric errors instead of singing on the live singer.rs render path. |
| ✅ F-015 | critical | `crates/auris-vocal/src/frames.rs:189` | Unbounded frame_hop from a project file lets render_frames allocate unboundedly just from viewing a clip in the piano roll, hanging the GUI. |
| ✅ F-036 | high | `crates/auris-vocal/src/frames.rs:340` | phoneme_at's segments.last() fallback keeps release=true forever past the last segment, forcing full gain on a pinned-short trailing consonant. |
| ✅ F-058 | high | `crates/auris-vocal/src/openjtalk.rs:14` | openjtalk_phoneme has no arms for OpenJTalk's uppercase devoiced-vowel labels A/I/U/E/O, so ordinary Japanese text (です, ます, し...) fails g2p with an […] |
| ✅ F-097 | high | `crates/auris-vocal/src/openjtalk.rs:23` | `openjtalk_phoneme` has no `kw`/`gw` arms, so any lyric OpenJTalk analyzes with a labialized velar mora hard-errors the whole line instead of singing it. |
| ✅ F-163 | medium | `crates/auris-vocal/src/frames.rs:303` | Two vocal notes sharing an identical start tick cause the shorter one to be silently dropped from the rendered performance with no warning. |
| ✅ F-216 | medium | `crates/auris-vocal/src/frames.rs:165` | render_frames only checks frame_hop is finite and positive, so a project file with a near-zero hop bypasses the session's documented [1ms,100ms] clamp and can […] |
| ✅ F-237 | medium | `crates/auris-vocal/src/phoneme.rs:20` | is_syllabic only recognizes the 5 Japanese-core vowels, so any hand-edited non-Japanese IPA vowel is timed and gained as a consonant instead of stretching to […] |
| ✅ F-391 | medium | `crates/auris-vocal/src/ornament.rs:41` | ornament_offset validates t/length/seconds/rate for finiteness but uses scoop/fall/vibrato depth raw, letting a corrupted project file's Infinity depth flood […] |

### ✅ F-009 · critical · JapaneseDictionary::phonemes() misparses jpreprocess's NJD output as HTS labels, so any kanji lyric errors instead of singing on the live singer.rs render path.

`crates/auris-vocal/src/g2p.rs:95` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Any note lyric containing kanji or other non-kana text (the entire reason a Japanese dictionary folder is configured at all) fails to sing: JapaneseDictionary::phonemes() is on the live production path (auris-session/src/session/singer.rs calls lyric_phonemes, which calls dictionary.phonemes(lyric) whenever the lyric is not plain kana), and it raises VocalError::Text{"unreadable label ..."} on essentially every real call. A user who sets up the dictionary specifically to sing kanji lyrics gets an error on the very first non-kana note instead of audio.

**Trigger.** Call `JapaneseDictionary::phonemes("歌")` (or any kanji/mixed text) with any loaded dictionary — e.g. via `lyric_phonemes("歌", Some(&dictionary))`.

**Mechanism.** `phonemes()` calls `self.inner.run_frontend(text)` (line 95) and then treats each returned string as one phoneme's HTS-style full-context label, extracting the current phoneme with `label_phoneme(label)` (line 102, which looks for the substring between `-` and `+`). But `jpreprocess::JPreprocess::run_frontend` (jpreprocess 0.15.0, the exact version pinned in Cargo.lock) does NOT return full-context phoneme labels — its own doc says 'Tokenize a text, preprocess, and return NJD converted to string... The returned string does not match that of openjtalk', and its implementation is `Ok(njd.into())` where `impl From<NJD> for Vec<String>` maps `node.to_string()` — one comma-separated NJD feature string per morpheme (e.g. `"これ,名詞,代名詞,一般,*,*,*,これ,コレ,コレ,0/2,C3,-1"`, confirmed against jpreprocess-njd 0.15.0's `NJDNode::Display` impl and the crate's own test fixtures). That format never contains the `+` character `label_phoneme` requires (only phoneme labels built from `p1^p2-p3+p4=p5` do, per jlabel's `Serializer::p`), so `label_phoneme` returns `None` on the very first node of any non-empty […]

**Expected.** `phonemes()` should obtain actual full-context labels — e.g. `self.inner.make_label(self.inner.run_frontend(text)?)` (or `extract_fullcontext`) and read the current phoneme off each `jlabel::Label`'s `Display` string — so that real Japanese text (as `lib.rs`'s own module doc promises) is actually converted to phonemes rather than always refusing.

**Fix direction.** Stop treating run_frontend's output as HTS full-context labels. Either call jpreprocess's actual label-generation API (if the crate/version exposes one) and keep label_phoneme for that format, or switch to the same NJD-based path accent_phrases already uses (text_to_njd + njd.preprocess() + reading each node's pronunciation/mora string) and derive phonemes from that instead of parsing "-...+" out of a comma-separated NJD feature string.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers ... rather than on "it runs" (CLAUDE.md, Conventions) — more directly, phonemes() has zero test coverage anywhere in g2p.rs's test module while accent_phrases() (the sibling method using the correct NJD API) does, so the broken path went unverified.

### ✅ F-015 · critical · Unbounded frame_hop from a project file lets render_frames allocate unboundedly just from viewing a clip in the piano roll, hanging the GUI.

`crates/auris-vocal/src/frames.rs:189` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Opening a project file with a maliciously or accidentally corrupted `frame_hop` (e.g. 1e-9) and simply selecting the singer clip in the piano roll — no render or export action taken — causes the GUI to hang or OOM, because `singer_sung_geometry` calls `render_frames` on every paint of the selected track and `render_frames` allocates three `Vec`s sized by `(end / hop).ceil() as usize + 1` with no upper bound.

**Trigger.** Open a project (or a project a hostile/corrupted tool produced) whose singer track has `"frame_hop": 1e-9` (finite, positive, so it passes the guard) and any note with nonzero length, then simply select that clip in the piano roll. `auris_gpui::ui::piano_roll::singer_sung_geometry` (piano_roll.rs:437) calls `Session::singer_frames` → `render_frames` on that path with no further validation, so no explicit export/render command is even needed — viewing the clip is enough.

**Mechanism.** `SingerTrack::frame_hop: f64` (`crates/auris-core/src/project/track.rs:116`, `#[serde(default = "default_frame_hop")]`) is deserialized directly from the `.auris` project JSON with no range check. `render_frames` (frames.rs:165-169) only guards `is_finite() && > 0.0` before using it as `hop`; any tiny-but-positive-finite value passes. Line 189, `let count = (end / hop).ceil() as usize + 1;`, then computes a frame count inversely proportional to `hop`, and the loop at line 191 pushes into `phonemes`/`f0_hz`/`energy` that many times.

**Expected.** `Session::set_frame_hop` (crates/auris-session/src/session/singer.rs:435-440) already states the intended contract: "Clamped into 1–100 ms rather than refused: every value in that range is a hop some model somewhere uses, and outside it is either a frame per sample or a frame per phrase, neither of which anybody means." `render_frames` (or `SingerTrack` deserialization) should enforce that same range for a value coming straight off disk, not just `is_finite() && > 0.0`.

**Fix direction.** Clamp `frame_hop` to a sane minimum (e.g. via a `deserialize_with` or a floor check alongside the existing `is_finite() && > 0.0` guard in `render_frames`, using something like `track.frame_hop.max(MIN_FRAME_HOP)`), and/or cap `count` in `render_frames` before allocating, so a malformed project file cannot force unbounded work merely by being viewed.

**Written rule it breaks.** Session::singer_frames doc comment: "A question, not a command: nothing is recorded and nothing changes." — a pure read path should not be able to hang/OOM the app.

### ✅ F-036 · high · phoneme_at's segments.last() fallback keeps release=true forever past the last segment, forcing full gain on a pinned-short trailing consonant.

`crates/auris-vocal/src/frames.rs:340` · correctness · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** When a trailing consonant phoneme is pinned to a short duration on a note that legitimately ends in a non-syllabic phoneme, exported/sung audio plays that consonant at full vowel-level gain for the rest of the note instead of the documented attenuated level, producing an unnaturally loud, unnatural-sounding tail.

**Trigger.** Via the documented boundary-drag feature (`Session::set_phoneme_duration`, crates/auris-session/src/session/singer.rs:236-274, which clamps each pin only to `[MIN_PHONEME_SECONDS=0.01, 10.0]` with no cross-check against the note length or the other phonemes' pins): take a 2-second note with phonemes `["a", "s"]` and pin both to their minimum, `phoneme_seconds = [0.01, 0.01]`. phoneme_layout produces segments `[(0.0,0.01,"a"), (0.01,0.02,"s")]` covering only 20 ms of the 2 s note; render_frames at default 10 ms hop then labels frame 0 as "a" and *every* frame from index 1 through the note's end (≈199 frames, ~1.98 s) as "s", with `release` true (hence full gain) from frame 2 onward.

**Mechanism.** phoneme_at() computes `let release = CONSONANT_RELEASE_SECONDS.min((to - from) / 2.0); (token.as_str(), !is_syllabic(token) && to - into <= release)`. When `phoneme_layout()`'s pinned widths sum to less than the note length and no phoneme is left unpinned to receive the stretchy remainder (`stretchy == 0` at frames.rs:417-421, so the `shared` fill computed at line 418 is never applied to anything), the returned segments cover only a small prefix of the note. For any `t` past that prefix, `segments.iter().find(...)` fails and the code falls back to `segments.last()` (line 335). If that last phoneme is a *consonant* (non-syllabic), `to - into` becomes increasingly negative as `into` grows, which is always `<= release`, so the boolean stays `true` forever. In render_frames (line 237-241) `release == true` forces `gain = 1.0` unconditionally, bypassing any consonant-level attenuation the voice's ConsonantLevels table would otherwise apply.

**Expected.** Per the module doc (frames.rs:11-16, 34-37) a consonant should take only its (measured or default) width and its `release` flag should only be true within the last `CONSONANT_RELEASE_SECONDS` of that consonant's own segment; a note whose allocated phoneme widths under-run its length should hold its *final phoneme* over the tail the way a vowel is held (frames.rs:360-361), not spuriously keep the release flag (and hence full gain) asserted indefinitely for a consonant far past its nominal […]

**Fix direction.** In phoneme_at, only treat the last-segment fallback as within the release window when to - into is non-negative and <= release; once into passes the segment's to, return release = false so full-gain no longer applies for the rest of the note.

**Written rule it breaks.** Consonants are short; syllabics stretch... The last CONSONANT_RELEASE_SECONDS of a consonant are the exception and come back up to the vowel's level... a /k/ held at its closure's level to the end is a /k/ that never opens. (frames.rs module doc)

### ✅ F-058 · high · openjtalk_phoneme has no arms for OpenJTalk's uppercase devoiced-vowel labels A/I/U/E/O, so ordinary Japanese text (です, ます, し...) fails g2p with an unknown-phoneme error.

`crates/auris-vocal/src/openjtalk.rs:14` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Singing or synthesizing any Japanese lyric/text that contains a devoiced vowel (extremely common — です, ます, し, す, つ, く, ひ between voiceless consonants) fails outright with a "unknown phoneme `U`"-style VocalError::Text instead of producing audio, once the OpenJTalk dictionary path is exercised.

**Trigger.** Once the `run_frontend`/`label_phoneme` bug above is fixed (or by unit-testing `openjtalk_phoneme("U")` directly), read any Japanese text containing a devoiced high vowel through the dictionary — です, ます, し, す, つ, く, ひ between voiceless sounds are all common.

**Mechanism.** `openjtalk_phoneme` only has arms for lowercase `"a","i","u","e","o"` (lines 16-20); it has no arms for OpenJTalk's uppercase devoiced-vowel phoneme names `"A","I","U","E","O"`, so any of them fall to `_ => return None`. That these uppercase symbols are real, expected output is independently confirmed two ways: (1) `jpreprocess-jpcommon` 0.15.0's own unit tests (`src/feature/mod.rs`) show です romanising to `..., "d", "e", "s", "U", ...` — the exact phoneme sequence `label_phoneme`/`openjtalk_phoneme` would see for one of the commonest Japanese words; (2) the training pipeline's own `OPENJTALK_TO_IPA` table (`training/src/auris_singer/text/japanese.py`, lines 27-32) explicitly maps `"A"->"ḁ"`, `"I"->"i̥"`, `"U"->"ɯ̥"`, `"E"->"e̥"`, `"O"->"o̥"` with the comment 'Uppercase vowels are OpenJTalk's devoiced vowels' — the very tokens `phoneme.rs`'s `VOICELESS` array already lists (`"ḁ", "i̥", "ɯ̥", "e̥", "o̥"`) as recognised. `training/tests/test_host_contract.py`'s subset check (`ours == host` over the *voiceless* table, and `produced ⊆ phoneme_table` over what the host *does* emit) never […]

**Expected.** `openjtalk_phoneme` should translate `"A","I","U","E","O"` to the devoiced-vowel IPA tokens the same way the trainer's `OPENJTALK_TO_IPA` does, so a devoiced vowel reads identically through both front-ends — the crate's own stated contract ('the test at the bottom is the contract: a syllable read either way must produce the same tokens').

**Fix direction.** Add match arms in openjtalk_phoneme (crates/auris-vocal/src/openjtalk.rs) for "A","I","U","E","O" mapping to the existing VOICELESS IPA tokens ("ḁ","i̥","ɯ̥","e̥","o̥"), mirroring training/src/auris_singer/text/japanese.py's OPENJTALK_TO_IPA table exactly, and extend the openjtalk.rs unit test to cover a devoiced-vowel syllable so a future drift is caught.

**Written rule it breaks.** the test at the bottom is the contract: a syllable read either way must produce the same tokens (crates/auris-vocal/src/openjtalk.rs doc comment)

### ✅ F-097 · high · `openjtalk_phoneme` has no `kw`/`gw` arms, so any lyric OpenJTalk analyzes with a labialized velar mora hard-errors the whole line instead of singing it.

`crates/auris-vocal/src/openjtalk.rs:23` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Singing lyrics that OpenJTalk analyzes to include a labialized velar mora (e.g. クヮ/グヮ, used in some loanwords and dialectal/historical readings) fails the entire phrase with `unknown phoneme \`kw\`` (or `gw`), even though every other mora in the line was readable — the user gets no audio and an opaque text error instead of a sung line.

**Trigger.** A note's lyric is Japanese text containing a labial-velar mora that OpenJTalk/jpreprocess would label "kw" or "gw" (e.g. a word written in kanji/mixed text spelled クァ or グァ), routed through `JapaneseDictionary::phonemes` because it is not representable via the kana table alone.

**Mechanism.** `openjtalk_phoneme` (lines 14-56) matches every OpenJTalk phoneme label the dictionary path can emit and falls through to `_ => return None` (line 55) for anything unrecognized. The match arms cover `"k" => &["k"]`, `"ky" => &["kʲ"]`, `"g" => &["g"]`, `"gy" => &["gʲ"]` (lines 23-26) but there is no arm for `"kw"` or `"gw"` — the labialized-velar phonemes OpenJTalk emits for loanword/dialect readings such as クァ (kwa) or グァ (gwa). `JapaneseDictionary::phonemes` (crates/auris-vocal/src/g2p.rs:106-109) turns a `None` from `openjtalk_phoneme` into `Err(VocalError::Text{ detail: format!("unknown phoneme `{name}`") })`, which fails the *whole* lyric, not just the offending syllable. The trainer's own mapping (`training/src/auris_singer/text/japanese.py` lines 50 and 53) explicitly has `"kw": "kʷ"` and `"gw": "gʷ"`, and both `kʷ`/`gʷ` are first-class entries in the shared IPA table (`training/src/auris_singer/text/ipa.py` line 53-54), so the model can be trained on and expects these symbols, but the host's dictionary front-end cannot produce them at all.

**Expected.** Every OpenJTalk phoneme label the trainer's `OPENJTALK_TO_IPA` recognizes (including "kw"→"kʷ" and "gw"→"gʷ") should have a matching arm in `openjtalk_phoneme`, per lib.rs's own contract that "a syllable read either way must produce the same tokens" and the file's own test comment stating that the bottom test "is the contract".

**Fix direction.** Add `"kw" => &["kʷ"]` and `"gw" => &["gʷ"]` arms to `openjtalk_phoneme` in crates/auris-vocal/src/openjtalk.rs (or map to the trainer's actual spelling for labialized velars, matching the `dz`/`ɲ` precedent of following the trainer's own OpenJTalk map rather than "textbook" IPA), and add both symbols to the phoneme table/VOICELESS list and `training/tests/test_host_contract.py` coverage so the two sides stay pinned together.

**Written rule it breaks.** A voice file is a contract between two languages, and several halves of it were written down twice — the `metadata_props` key, the format version, the reserved `<sil>` and `<unk>`, and the phoneme table down to which symbols are voiceless. ... `training/tests/test_host_contract.py` is that assertion executable, and it is the thing to keep alive.

### ✅ F-163 · medium · Two vocal notes sharing an identical start tick cause the shorter one to be silently dropped from the rendered performance with no warning.

`crates/auris-vocal/src/frames.rs:303` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When a user places two notes at the exact same start tick on a vocal track (e.g. two notes typed to start at beat 0, or two overlapping-loop passes landing on the same tick), the shorter of the two is silently deleted from the rendered performance: its pitch, lyric, and phonemes never appear in frames.inventory and no audio is produced for it, with no warning or diagnostic. Which note survives depends only on an incidental sort tie-break (end tick ascending) rather than any documented rule.

**Trigger.** render_frames on a track built from `[sung(60, 0.0, 2.0, &["a"]), sung(64, 0.0, 0.1, &["i"])]` (both notes start at beat 0.0): the shorter "i" note sorts first (end 0.1 beat < end 2.0 beat), gets `end = min(0.05s, 0.0s) = 0.0s`, and is filtered out; only the "a" note survives, playing in full 0..1.0 s. `frames.inventory` never contains "i", and no frame carries pitch 64 anywhere in the output.

**Mechanism.** `timed_notes()` sorts by `(start.raw(), end.raw())` (line 303) and then computes each note's forced end as `(*end).min(*next_start)` against only the immediately-following entry (lines 304-312). When two notes share the exact same start tick, the sort tie-breaks by end ascending, so the shorter note is placed immediately before the longer one. Its forced end becomes `min(short_end, long_start) == long_start == its own start`, and the subsequent `filter(|((start,_,_), end)| *end > *start)` (line 317) then drops it completely — not truncated, entirely absent from the returned Vec.

**Expected.** The module's own stated rule is "A singer sings one note at a time. Where notes overlap, the later-starting note cuts the earlier one off at its own start" (frames.rs:8-10) -- a rule that presumes one note starts later than the other. For two notes tied at the same start neither is "later", so cutting one to zero length and discarding it outright is not what the documented rule describes; at minimum the note should not vanish without a trace.

**Fix direction.** In timed_notes (frames.rs:303-317), only force-cut a note's end against next_start when next_start is strictly greater than the note's own start; when next_start == start, treat it as a true tie (do not collapse the forced end to the note's own start) and instead resolve the collision deterministically without silently zeroing and dropping the loser via the min()+filter side effect.

**Written rule it breaks.** A singer sings one note at a time. Where notes overlap, the later-starting note cuts the earlier one off at its own start.

### ✅ F-216 · medium · render_frames only checks frame_hop is finite and positive, so a project file with a near-zero hop bypasses the session's documented [1ms,100ms] clamp and can allocate huge Vecs, hanging or crashing the app.

`crates/auris-vocal/src/frames.rs:165` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Opening a project file whose SingerTrack.frame_hop was hand-edited or written by a foreign tool to an extremely small positive value (e.g. 1e-9) loads it untouched, and any subsequent singing render (sing, export, or the per-edit take-state refresh) computes frame count as end/hop and allocates three Vec<_> of that length, which can hang or OOM/crash the app instead of being rejected or clamped like every other entry point.

**Trigger.** A project file (hand-edited, produced by another tool, or corrupted in a way that still parses as valid JSON -- e.g. `"frame_hop": 1e-9`, a perfectly valid JSON number) loaded and then rendered/exported: for a track with even a few seconds of notes, `count` becomes billions, and the three `Vec::push` loops attempt to allocate and fill gigabytes-to-terabytes of memory.

**Mechanism.** `let hop = if track.frame_hop.is_finite() && track.frame_hop > 0.0 { track.frame_hop } else { default_frame_hop() };` (lines 165-169) accepts any finite positive value with no lower bound, and `count = (end / hop).ceil() as usize + 1` (line 189) then allocates three `Vec`s of that length. `SingerTrack::frame_hop` (crates/auris-core/src/project/track.rs:115-116) deserializes straight from the project JSON via `#[serde(default = ...)]` with no range validation; only the session command `Session::set_frame_hop` (crates/auris-session/src/session/singer.rs:435-450) clamps it into `[0.001, 0.1]`, and nothing enforces that clamp on project load or on `render_frames`'s own input.

**Expected.** The lens's own concern applies directly here (an absurd "sample rate"-like clock value should not be allowed to reach allocation-sized arithmetic unchecked); the session's own `set_frame_hop` already documents the correct bound ("Clamped into 1-100 ms ... outside it is either a frame per sample or a frame per phrase, neither of which anybody means", crates/auris-session/src/session/singer.rs:432-434) but `render_frames` does not apply the same floor to whatever value actually arrives on the […]

**Fix direction.** Clamp frame_hop into the same [0.001, 0.1] range render_frames already treats as the valid domain (mirroring Session::set_frame_hop's clamp) directly in render_frames's guard, e.g. track.frame_hop.clamp(0.001, 0.1) when finite and positive, else default_frame_hop(); optionally also validate/clamp SingerTrack::frame_hop on deserialization in load_project for defense in depth.

**Written rule it breaks.** Session::set_frame_hop doc comment: "Clamped into 1–100 ms rather than refused: every value in that range is a hop some model somewhere uses, and outside it is either a frame per sample or a frame per phrase, neither of which anybody means."

### ✅ F-237 · medium · is_syllabic only recognizes the 5 Japanese-core vowels, so any hand-edited non-Japanese IPA vowel is timed and gained as a consonant instead of stretching to fill its note.

`crates/auris-vocal/src/phoneme.rs:20` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A note whose phonemes were hand-edited to a non-Japanese IPA vowel (e.g. ɛ, ɪ, ʊ, y, ø, œ, ɐ, or any of the ~15 vowels beyond the Japanese-core 5) is sung wrong: phoneme_layout treats it as an edge consonant and squeezes it into a fixed ~60ms slot instead of stretching to fill the note, and on a voice that ships a measured consonant-level table, level_gain applies the consonant dB attenuation to it too. A held non-Japanese vowel note comes out clipped short and, on some voices, quieter than it should be, with no error or warning.

**Trigger.** A user (or a future non-Japanese g2p front-end) writes a note's phonemes directly with a vowel symbol from the shared table that is not one of the 5 Japanese-core vowels — e.g. `["h", "ɛ", "l", "o"]` for an English-style syllable — on a track whose voice carries a `ConsonantWidths`/`ConsonantLevels` table (any format-2 export).

**Mechanism.** `VOWELS` (line 20) is `["a", "i", "ɯ", "e", "o"]` and `is_syllabic` (lines 28-30) answers `true` only for those five plus `ɴ`/`ʔ`; every other symbol — including the ~15 additional vowels the shared IPA table reserves "for other languages" (`training/src/auris_singer/text/ipa.py` lines 44-45: `u, ɨ, ə, ɛ, ɔ, æ, ʌ, ɑ, ɒ, ʊ, ɪ, y, ø, œ, ɐ`) — is treated as a fixed-width consonant. Training's own `STRETCHED` set (`training/src/auris_singer/phoneme_durations.py` lines 79-85) explicitly includes all of these as syllabic (excluded from consonant-duration measurement), and `ipa.py`'s `PHONEME_CLASSES["vowel"]` (lines 89-92) lists the same full set — so the trainer treats them as vowels. On the host side, `phoneme_layout` (crates/auris-vocal/src/frames.rs, `is_syllabic` calls at ~377-378) uses `is_syllabic` to decide which phonemes stretch to fill a note versus which get a short fixed/measured-consonant slot, and `level_gain` (frames.rs ~446) uses `!is_syllabic(phoneme)` to decide whether to apply a consonant-attenuation dB gain. `crates/auris-vocal/src/lib.rs` (lines 11-13) documents that […]

**Expected.** `is_syllabic` (or the note-timing/level code that calls it) should recognize the full vowel set the shared IPA table and the trainer's `STRETCHED`/`PHONEME_CLASSES["vowel"]` already agree on, so that lib.rs's documented promise — that hand-edited IPA phonemes work for other languages without rebuilding the crate — actually holds for timing and level, not just for token lookup.

**Fix direction.** Extend VOWELS in crates/auris-vocal/src/phoneme.rs (or is_syllabic's logic) to include the full vowel set the trainer already reserves and classifies as syllabic in training/src/auris_singer/text/ipa.py and phoneme_durations.py::STRETCHED, keeping the two tables in sync the way VOICELESS already is; alternatively, explicitly document that only the Japanese-core 5 are supported for timing/level purposes today and add a host-contract test asserting that boundary in both directions.

**Written rule it breaks.** auris-vocal/src/lib.rs: "Other languages are written by editing a note's phonemes directly; the phoneme vocabulary is IPA precisely so that nothing here has to be rebuilt when a voice model [learns another language]"

**Verifier's correction.** `is_syllabic` in crates/auris-vocal/src/phoneme.rs (VOWELS at line 20, is_syllabic lines 28-30) recognizes only the 5 Japanese-core vowels, not the ~15 additional vowels (ɛ, ɪ, ʊ, y, ø, œ, ɐ, etc.) that the shared trainer-side IPA table (training/src/auris_singer/text/ipa.py's IPA_SYMBOLS/PHONEME_CLASSES["vowel"]) and STRETCHED set (training/src/auris_singer/phoneme_durations.py) already reserve and treat as syllabic for exactly this purpose — supporting future non-Japanese language front-ends without changing the shared phoneme table. This contradicts the project's own documented design […]

### ✅ F-391 · medium · ornament_offset validates t/length/seconds/rate for finiteness but uses scoop/fall/vibrato depth raw, letting a corrupted project file's Infinity depth flood the f0 curve.

`crates/auris-vocal/src/ornament.rs:41` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Opening a corrupted or hand-edited .auris project whose note ornament carries an out-of-range depth (e.g. JSON literal 1e400, which serde_json parses as Infinity) produces Infinity/NaN semitone offsets from ornament_offset that flow straight into the f0 curve fed to the singing-voice model, so that note's pitch output is corrupted (garbage or silent) rather than the ornament being switched off as documented.

**Trigger.** Hand-edit (or have any external tool write) a `.auris` project file so a note's `scoop` (or `fall`/`vibrato`) object has `"depth": 1e40`, then open the project and view/render the singer clip.

**Mechanism.** `ornament_offset`'s guard at line 41 (`if !t.is_finite() || !length.is_finite() || length <= 0.0 || t < 0.0 || t >= length`) and `ornament_reach` (lines 83-88) validate `t`, `length`, `seconds`, `rate`, `delay` and `fade_in`, but never `scoop.depth` / `fall.depth` / `vibrato.depth` before they are used unguarded at lines 50, 59 and 71 (e.g. `offset -= f64::from(scoop.depth) * ease;`). The doc comment at lines 32-33 promises 'Degenerate numbers (a non-positive or non-finite span or rate) switch that ornament off rather than propagating' -- depth is conspicuously not in that list. The only place that clamps depth to a finite, bounded range is `ornament_depth()` in crates/auris-session/src/session/singer.rs:1118, which runs solely inside session *commands* (creating/editing an ornament from the UI or toolbox). `crates/auris-io/src/project_file.rs::load_project` (~lines 180-199) deserializes `Project` with plain serde and only repairs id counters and routing afterward -- it never re-validates ornament fields. `Scoop`/`Fall`/`Vibrato` (crates/auris-core/src/project/ornament.rs) derive […]

**Expected.** The module's own doc comment says degenerate ornament numbers 'switch that ornament off rather than propagating'; `depth` should be validated the same way `seconds`/`rate`/`delay`/`fade_in` already are, or `load_project` should re-validate ornament fields the way session commands do.

**Fix direction.** In ornament_offset (crates/auris-vocal/src/ornament.rs), check scoop.depth.is_finite(), fall.depth.is_finite(), and vibrato.depth.is_finite() alongside the existing t/length/seconds/rate checks, skipping that ornament's contribution (treating it as 0) when depth is non-finite — matching the pattern already used for seconds/rate/delay/fade_in.

**Written rule it breaks.** Degenerate numbers (a non-positive or non-finite span or rate) switch that ornament off rather than propagating. (crates/auris-vocal/src/ornament.rs:32-33 doc comment)
