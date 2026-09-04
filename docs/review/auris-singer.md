# Review findings: auris-singer

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 3 verified findings: 2 high, 1 medium.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-011 | high | `crates/auris-singer/src/metadata.rs:284` | VoiceInfo::parse trusts n_speakers:u32 from voice metadata unchecked and calls speakers(), letting a crafted/corrupt value drive a multi-gigabyte Vec<String> […] |
| F-078 | high | `crates/auris-singer/src/score.rs:206` | SingerFrames.f0_hz/energy are indexed without bounds checks in chunk_ranges/arrange, panicking on a hand-edited or externally-written file whose curve arrays […] |
| F-217 | medium | `crates/auris-singer/src/model.rs:137` | VoiceModel::load's doc promises a CPU fallback on GPU session-build failure under Acceleration::Auto, but open_session has no such retry — only the later […] |

### F-011 · high · VoiceInfo::parse trusts n_speakers:u32 from voice metadata unchecked and calls speakers(), letting a crafted/corrupt value drive a multi-gigabyte Vec<String> allocation that aborts the process on load.

`crates/auris-singer/src/metadata.rs:284` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Loading a voice model whose embedded metadata (or its .json sidecar) has been corrupted or crafted with a huge n_speakers value crashes the whole process on file load — VoiceInfo::parse calls info.speakers() unconditionally, which allocates a Vec<String> sized directly by the untrusted u32 (up to ~4.29 billion entries), causing an allocation failure/abort or an extreme multi-gigabyte allocation before the user ever sees a speaker list or error message.

**Trigger.** Load any `.onnx` voice file (via `VoiceModel::load`, the normal "choose a voice" flow) whose embedded metadata JSON sets `"n_speakers": 4000000000` (or any value in the billions) while everything else is well-formed. `VoiceCard`/voice files are explicitly documented as shared, third-party-distributable assets ("a library shared by every project", like a SoundFont), so a corrupted or malicious export is a realistic input, not just a contrived one.

**Mechanism.** `VoiceInfo::n_speakers: u32` (line 45) is read straight off the JSON metadata embedded in an ONNX file's `metadata_props["auris_singer"]` (or its `.json` sidecar) with no upper bound. `VoiceInfo::parse` validates `sample_rate`, `hop_length` and `inter_channels` against zero (lines 177-181) but never bounds `n_speakers`. `parse` then unconditionally calls `let known = info.speakers();` at line 189, and `speakers()` (lines 283-293) does `(0..self.n_speakers).map(|id| ... .unwrap_or_else(|| id.to_string())).collect()` — an allocation and iteration count directly equal to the attacker/corruption-controlled `n_speakers`.

**Expected.** Matches the lens's own example of the class of bug to catch — "absurd values (sample rate 0, a thousand channels)" — and the file's own validation block (lines 177-181) already treats `sample_rate`/`hop_length`/`inter_channels` as untrusted header counts that must be sanity-checked before being used; `n_speakers` should get the same treatment (e.g. reject anything above a small sane ceiling, or at minimum cap the iteration in `speakers()`) before it is used to size an allocation.

**Fix direction.** In VoiceInfo::parse, add an n_speakers bound check alongside the existing zero checks on sample_rate/hop_length/inter_channels (crates/auris-singer/src/metadata.rs:177-181) — reject metadata whose n_speakers exceeds a small sane ceiling (e.g. a few thousand, or bounded relative to speaker_to_id.len()) with a SingError::Metadata before info.speakers() is ever called at line 189.

**Written rule it breaks.** /// Reads the metadata JSON and refuses anything a later `sing` would trip over.

### F-078 · high · SingerFrames.f0_hz/energy are indexed without bounds checks in chunk_ranges/arrange, panicking on a hand-edited or externally-written file whose curve arrays are shorter than phonemes.

`crates/auris-singer/src/score.rs:206` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `auris sing-frames` (or any caller of `Session::sing_frames`) on a `SingerFrames` JSON file whose `f0_hz` or `energy` array is shorter than `phonemes` — trivially produced by hand-editing the exported file, or by any external tool writing to the documented shape — crashes the process with an index-out-of-bounds panic instead of surfacing a `SingError`, mid-render.

**Trigger.** Produce a `SingerFrames` JSON where `phonemes.len() != f0_hz.len()` (or `!= energy.len()`), e.g. by hand-editing the file `Session::export_singer_frames` writes and truncating `f0_hz`/`energy`, or by any external tool building the file to the documented shape but with a shorter curve array. Feed it through `Session::read_singer_frames` → `Session::sing_frames` (or the `auris sing-frames` CLI subcommand, crates/auris-cli/src/main.rs:854/863) with at least one sung frame at or beyond the shorter array's length.

**Mechanism.** `SingerFrames` derives `Deserialize` with four independent `Vec`s and no length-consistency check anywhere in the crate (confirmed by grep: no custom `Deserialize`, `TryFrom`, or `validate` for the type). `chunk_ranges` and `arrange` both size their iteration range off `frames.len()` (== `phonemes.len()`, score.rs:78, 89, 136) but then index `frames.f0_hz`/`frames.energy` directly by that same position — `let f0 = frames.f0_hz[at];` (line 206), `frames.energy[at]` (line 208), and the quietest-frame search `frames.energy[*a]` inside `chunk_ranges` (line 100). None of these use `.get()`, unlike the deliberately defensive `frames.phonemes[at]` → `ids.get(entry).copied().unwrap_or(unk)` pattern the same function uses two lines above.

**Expected.** The module's own doc comment for `arrange` (score.rs:165) states the design intent directly: "A file edited by hand can hold an index past its own inventory; sing it as unknown rather than panicking over it." The same tolerance should extend to array-length mismatches between `phonemes`, `f0_hz` and `energy` — either validated once when `SingerFrames` is read from disk, or indexed with `.get()`/a fallback the way `phonemes[at]` already is, rather than trusting the three arrays to stay in […]

**Fix direction.** Validate array-length consistency once in `Session::read_singer_frames` (or add a checked constructor/`TryFrom` on `SingerFrames`) and return a `SessionError`/`SingError` on mismatch; alternatively, index `frames.f0_hz`/`frames.energy` in `chunk_ranges` and `arrange` with `.get(at).copied().unwrap_or(0.0)`, matching the defensive pattern already used for `frames.phonemes[at]` two lines above.

**Written rule it breaks.** arrange()'s own doc comment: "A file edited by hand can hold an index past its own inventory; sing it as unknown rather than panicking over it." (crates/auris-singer/src/score.rs, above `arrange`)

### F-217 · medium · VoiceModel::load's doc promises a CPU fallback on GPU session-build failure under Acceleration::Auto, but open_session has no such retry — only the later mid-render demotion in sing_with is implemented.

`crates/auris-singer/src/model.rs:137` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** On a machine where the platform's GPU execution provider registers successfully but fails to compile this specific voice model's graph at session-build time, loading a voice under Acceleration::Auto (the default) fails outright with SingError::Load instead of silently falling back to the CPU as the doc for VoiceModel::load promises. The user sees a load error and has to manually switch acceleration to CPU, rather than getting the transparent CPU render the documented "Auto" contract describes.

**Trigger.** Load a voice with `Acceleration::Auto` on a machine where the platform's GPU execution provider reports `is_available() == true` but fails to build a session for this specific model (a real, ort-documented possibility — a version mismatch, missing dependency DLL, or a graph the EP can't compile at load time).

**Mechanism.** The doc on `VoiceModel::load` (lines 131-135) promises `Acceleration::Auto` 'falls back to the CPU when the device refuses the session, and demotes itself to the CPU mid-render if the provider accepts the session and then refuses its shapes' — two distinct failure points. Only the second (mid-render) is implemented: `sing_with` catches a `sing_chunk` error and rebuilds on the CPU when `self.on_gpu && self.acceleration == Auto` (lines 237-243). The first — `open_session`'s own `builder.commit_from_file(path).map_err(refused)?` (line 120) — has no such retry; under `Auto`, if the GPU provider is engaged (`carried == true`) but session *creation* itself fails, `load` (line 137: `let (session, on_gpu) = open_session(path, acceleration)?;`) propagates the error straight out as `SingError::Load`, with the CPU never tried. The `ort` crate's own doc on `ExecutionProvider::is_available` (the `carried` check) states exactly this gap is possible: 'this does not always mean the execution provider is usable for a specific session... the EP may encounter an error while attempting to load […]

**Expected.** Under `Acceleration::Auto`, a `commit_from_file` failure with the GPU provider engaged should retry with a CPU-only session (the same recovery `sing_with` already performs for the later, mid-render failure), matching the crate's own documented contract for `load`.

**Fix direction.** In open_session (crates/auris-singer/src/model.rs), when acceleration is Auto and the GPU provider was engaged, catch a commit_from_file failure and retry once with a CPU-only builder before propagating the error — mirroring the CPU-demotion retry sing_with already performs on a mid-render failure. Alternatively, narrow the doc comment on VoiceModel::load to state only the mid-render fallback is implemented, since load-time commit failures currently propagate.

**Written rule it breaks.** The doc comment on VoiceModel::load (crates/auris-singer/src/model.rs:131-135): "falls back to the CPU when the device refuses the session, and demotes itself to the CPU mid-render if the provider accepts the session and then refuses its shapes"
