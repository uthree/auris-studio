# Claims the review could not settle

Part of the [whole-repository adversarial review](README.md). For these 7 claims the verification agents repeatedly returned unusable output, so they carry only the first-pass reviewer's argument. Treat them as leads, not findings.

### F-052 · high (unverified) · training/README.md Quick Start omits the `export` extra its own next step needs

`training/README.md:32` · spec-mismatch

**Trigger.** A new contributor copies the three Quick Start commands verbatim from training/README.md: `uv venv --python 3.11`, `uv pip install -e '.[dev]' --torch-backend=auto`, then the preprocess/train/export block.

**Mechanism.** Line 32 reads `uv pip install -e '.[dev]' --torch-backend=auto`, installing only the `dev` extra from `training/pyproject.toml`. Ten lines later, line 42 of the same Quick Start block runs `uv run python scripts/export_onnx.py --checkpoint runs/base/checkpoints/last.ckpt --output voice.onnx`. `scripts/export_onnx.py` imports `auris_singer.export`, whose `export_onnx()` does `import onnx` (training/src/auris_singer/export.py:407) and whose `verify_onnx()` does `import onnxruntime` (training/src/auris_singer/export.py:469) — both packages live only in the `export` optional-dependency group in `training/pyproject.toml` (`onnx`, `onnxscript`, `onnxruntime`), not in `dev`. CLAUDE.md's own Commands section confirms the intended install line is `uv pip install -e '.[dev,export]' --torch-backend=auto`, and `.github/workflows/ci.yml`'s training job likewise installs `'.[dev,export]'`.

### F-162 · medium (unverified) · save_project leaves the scratch file behind when the write step itself fails

`crates/auris-io/src/project_file.rs:167` · correctness

**Trigger.** Any real I/O failure during `std::fs::write(&in_progress, json)` — most concretely a disk that fills up while Auris Studio writes the scratch JSON for a save (autosave or manual save). The project's own test for this function, `a_failed_save_leaves_the_previous_version_intact` (project_file.rs lines 388-406), has to call `std::fs::remove_dir(&blocker).unwrap();` itself right after asserting the save failed, because `save_project` never removes the entry on this path — that manual cleanup line […]

**Mechanism.** `save_project` writes JSON to the sibling scratch path with `std::fs::write(&in_progress, json).map_err(...)?;` (line 167) and propagates any failure immediately via `?`. Only the *second* failure point — `std::fs::rename` at lines 168-171 — calls `let _ = std::fs::remove_file(&in_progress);` before returning. If the write to `in_progress` itself fails (the realistic case this crate repeatedly calls out: 'a full disk, a dropped network share'), the function returns `Err` without ever attempting to delete the scratch file it may have just created or partially filled. `write_wav` in export.rs (lines 182-186), which shares this exact `in_progress_path` scheme, cleans up on *both* of its failure branches, so this is an asymmetry specific to `save_project`.

### F-178 · medium (unverified) · A non-finite sample makes rms/rms_db() return NaN, not the documented -120 dB floor

`crates/auris-gpu/src/analysis.rs:84` · dsp

**Trigger.** Call `analyze_loudness_cpu` (or the GPU path) on any `AudioBuffer` containing a single non-finite (NaN or infinite) sample, e.g. `AudioBuffer::from_planar(vec![vec![0.5, f32::NAN, 0.5]], 48_000.0)`.

**Mechanism.** `peak` and `true_peak` are accumulated exclusively through Rust's NaN-ignoring `f32::max` (`peak = peak.max(value.abs());` line 134, and `true_peak = true_peak.max(catmull_rom(...).abs());` line 141, then `true_peak_estimate: true_peak.max(peak)` at line 89) — a NaN operand is always discarded by `.max()`, so those two fields can never become NaN from a NaN sample. `sum_squares`, by contrast, is accumulated with plain addition — `sum_squares += (value * value) as f64;` (line 135) in the CPU reference, and identically on the GPU path via `sum_squares = sum_squares + value * value;` (loudness.wgsl:58) folded into the host total with `sum_squares += slot[2] as f64;` (analysis.rs:219) — and IEEE-754 addition propagates NaN unconditionally, so one non-finite sample anywhere in the buffer poisons `sum_squares` for every channel processed afterward (`sum_squares` is one running total across […]

### F-192 · medium (unverified) · Plugin window's height ceiling omits the title bar, capping the list short

`crates/auris-gpui/src/ui/plugin_window.rs:173` · ui

**Trigger.** Open the plugin window for the default instrument (`auris.synth.chiptune`, `auris_session::registry::DEFAULT_INSTRUMENT`), which has 16 parameters (crates/auris-synth/src/chiptune.rs) and an ADSR envelope (attack/decay/sustain/release keys, so `envelope_of` returns `Some`, adding `GRAPH_HEIGHT` = 84px to `above`). Needed height = PANEL_HEADER_HEIGHT(22) + body(16*22 + 15*4 + 2*8 = 428) + above(84) = 534px, which exceeds the ceiling `frame.height` = MAX_LIST_HEIGHT(420) + above(84) = 504px, so […]

**Mechanism.** `PluginWindow::frame` computes the window's height ceiling as `Self::MAX_LIST_HEIGHT + above` (line 173), where `above` sums every non-scrolling element drawn *between the title bar and the list* — the analyser curve, the envelope graph, the caution strip, the sidechain row (lines 315-327). The title bar itself (`Metrics::PANEL_HEADER_HEIGHT`, 22px) is never added to this ceiling, even though it is likewise a `flex_shrink_0` element drawn above the scrolling list (rendered at line ~374 with `.h(Metrics::PANEL_HEADER_HEIGHT)`) and is present on every window. The doc comment on `frame()` explains exactly why `above` content must be *added* to the ceiling rather than taken out of it ("otherwise giving a plugin a picture would have taken a third of its sliders away with it") — the identical reasoning applies to the header, which is drawn even higher up, but the header is not included. The […]

### F-218 · medium (unverified) · vocal_rhythm drops empty-syllable phrases; write_vocal zips positionally and silently misaligns everything after one

`crates/auris-compose/src/vocal.rs:91` · correctness

**Trigger.** Call `let rhythm = vocal_rhythm(&[0, 3], meter);` (rhythm.phrases has length 1: just the 3-syllable phrase's slots) and `write_vocal(&harmony, Ticks::ZERO, &rhythm, &[vec![], vec![Contour::Free, Contour::Free, Contour::Free]], VocalRange::default(), seed)` — the natural way to build the second argument from the same 2-phrase source (an empty-contour phrase followed by a 3-syllable one). The zip pairs `rhythm.phrases[0]` (the real 3-syllable slots) with `phrases[0]` (the empty Vec meant for the […]

**Mechanism.** `vocal_rhythm` (line 83) iterates `counts.iter().copied().filter(|count| *count > 0)` (line 91), so any phrase with a syllable count of 0 contributes NO entry to `VocalRhythm.phrases` — the returned `phrases` vector is shorter than `counts` whenever any count is 0, and the surviving entries no longer correspond by index to the original per-phrase list. `write_vocal`'s own doc (lines 225-227) says `phrases` (the per-phrase `Vec<Contour>` argument) 'must line up with rhythm — both come from the same lyric', and it then pairs them purely positionally: `rhythm.phrases.iter().zip(phrases).enumerate()` (line 245). If a caller builds its `contours: Vec<Vec<Contour>>` with one entry per ORIGINAL phrase (the natural, 1:1 way to build it from the same source list `counts` was derived from, and exactly the shape produced by an empty phrase yielding an empty `Vec<Contour>`), the zip pairs the wrong […]

### F-279 · low (unverified) · Vendored Cargo.toml points to a Cargo.toml.orig that was never committed

`vendor/rustysynth/Cargo.toml:10` · other

**Trigger.** Anyone reading this vendored, forked crate's manifest — which CLAUDE.md explicitly flags as noteworthy ("kept in vendor/rustysynth ... see vendor/rustysynth/README.md") — and following the pointer to see what upstream's manifest actually declared.

**Mechanism.** The auto-generated header (lines 1-10) tells a reader to consult `Cargo.toml.orig` for "the original contents" of the manifest before cargo normalized it. `git log --all` for `vendor/rustysynth/Cargo.toml.orig` returns nothing — the file has never existed in this repository's history — and `ls vendor/rustysynth/` confirms it is absent today (only Cargo.lock, Cargo.toml, LICENSE.txt, README.md, src). Whoever committed this file ran it through cargo's publish/package normalization and kept only the generated output, not the original hand-written manifest the comment refers to.

### F-282 · low (unverified) · Attachment-ceiling error message rounds the reported file size down to match the ceiling

`crates/auris-agent/src/main.rs:752` · correctness

**Trigger.** `auris-agent --model m --attach clip.wav "..."` where `clip.wav` is, say, 25.5 MB (26,738,688 bytes) — an entirely ordinary audio file just over the stated limit.

**Mechanism.** `size / (1024 * 1024)` (line 752) is integer division, truncating toward zero, used to report the offending file's size in the refusal message at lines 747-755. For any file between `ATTACHMENT_CEILING` (25 MB) and just under 26 MB, `size / (1024*1024)` truncates to 25 — the same number printed for the ceiling itself (`ATTACHMENT_CEILING / (1024*1024)` = 25, line 753) — producing the self-contradicting sentence "`<path>` is 25 MB; audio over 25 MB is refused".

