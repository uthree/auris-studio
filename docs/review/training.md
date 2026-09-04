# Review findings: training/ (the voice trainer)

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 29 verified findings: 1 critical, 6 high, 15 medium, 7 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-013 | critical | `training/src/auris_singer/preprocess/pipeline.py:198` | A single corrupt WAV or non-UTF-8 transcript crashes preprocessing unhandled, discarding every already-computed .npz because the dataset manifest is written […] |
| F-028 | high | `training/src/auris_singer/host_eval.py:694` | The song render in host_eval.py sings the whole concatenated song in rows[0]'s speaker, silently mismatching every other utterance's own voice. |
| F-039 | high | `training/src/auris_singer/preprocess/pipeline.py:217` | The "too short" guard only enforces wav.numel() >= hop_length, not the (n_fft-hop_length) reflect-pad width frame_energy needs, so a short utterance can crash […] |
| F-073 | high | `training/src/auris_singer/data/datamodule.py:110` | DistributedBucketSampler.epoch is set once at construction and never advanced, because use_distributed_sampler=False (train.py:103) disables Lightning's […] |
| F-115 | high | `training/src/auris_singer/phoneme_levels.py:90` | phoneme_levels.py classes devoiced Japanese vowels as full vowels, so a whispered vowel becomes the loudness reference for the consonant before it, […] |
| F-119 | high | `training/src/auris_singer/utils/audio.py:65` | A single too-short utterance (exactly one hop of samples) crashes `training`'s whole preprocessing run via an unhandled reflect-pad RuntimeError in […] |
| F-320 | high | `training/src/auris_singer/lightning_module.py:154` | load_weights unpickles an unvalidated --init-from/--resume checkpoint via torch.load(weights_only=False), giving arbitrary code execution on a crafted file. |
| F-127 | medium | `training/doc/architecture.md:148` | architecture.md's loss table lists KL (auxiliary) default as 1.0, but code, training.md, and the doc's own later prose all agree the default is 0.2. |
| F-157 | medium | `training/src/auris_singer/losses.py:154` | EnvelopeLoss divides by the configured kernel count instead of the count actually used, silently underweighting the loss when a kernel exceeds the signal […] |
| F-175 | medium | `training/src/auris_singer/modules/generator.py:161` | NsfHifiGanGenerator's own kernel>=rate guard admits rate=1/even-kernel stages whose output_padding=1 is invalid for stride=1, crashing on first forward instead […] |
| F-182 | medium | `training/src/auris_singer/phoneme_levels.py:160` | summarize_speaker falls back to default=0.0 for an empty-levels speaker, exactly the "plateau" value its own docstring and doc/inference.md promise never […] |
| F-193 | medium | `training/doc/inference.md:433` | doc/inference.md documents an "author" card.json field that VoiceCard (metadata.rs) has no field for, so it silently vanishes on import. |
| F-201 | medium | `training/src/auris_singer/preprocess/pipeline.py:138` | A zero-length source WAV crashes the entire training preprocessing run instead of being skipped like other bad-input utterances. |
| F-202 | medium | `training/scripts/prepare_namine_ritsu.py:68` | read_mono_label/read_label crash the whole corpus-prep run on one malformed label line instead of skipping it, unlike their sibling parser which already guards […] |
| F-220 | medium | `training/src/auris_singer/host.py:156` | host.py's own MODEL_SILENCE = "<sil>" literal is a third, untested copy alongside ipa.SIL and Rust's score.rs constant. |
| F-221 | medium | `training/src/auris_singer/export.py:436` | verify_onnx's two test cases both build durations that sum short of f0 length, so the exact-duration-fit contract the wrapper's docstring declares is never […] |
| F-234 | medium | `training/src/auris_singer/data/dataset.py:147` | collate_batch crashes on batches whose n_fft-hop_length parity is odd, because spectrogram()'s frame count silently drops below the pre-truncated […] |
| F-337 | medium | `training/src/auris_singer/data/dataset.py:233` | DistributedBucketSampler silently drops any utterance outside bucket_boundaries with no log or error unless every bucket ends up empty. |
| F-384 | medium | `training/src/auris_singer/data/datamodule.py:119` | Under trainer.strategy=ddp, val_dataloader() (datamodule.py:119-126) stays unsharded because train.py:103 sets use_distributed_sampler=False Trainer-wide, so […] |
| F-405 | medium | `training/src/auris_singer/infer.py:80` | resolve_speaker() in training/src/auris_singer/infer.py:80 silently accepts a JSON bool as speaker id 0/1 because bool is a subclass of int, bypassing […] |
| F-413 | medium | `training/src/auris_singer/host_eval.py:143` | host_eval's Analyst always measures pitch with the FCPE default f0_min=40.0, ignoring the exported voice's own f0_min, silently skewing f0 metrics for voices […] |
| F-414 | medium | `training/src/auris_singer/host_eval.py:807` | evaluate_score() hardcodes n_fft/win_length=2048 in its Analyst instead of reading the voice's own exported audio config, unlike evaluate()'s corpus-based path. |
| F-194 | low | `training/src/auris_singer/data/dataset.py:95` | SingingDataset's unused hop_length/n_fft/win_length override params can silently desync f0/energy from the spectrogram, but no shipped caller ever passes them. |
| F-235 | low | `training/src/auris_singer/preprocess/pipeline.py:88` | Unsanitized dataset.sources[].name in the local preprocessing config lets output_dir/f"{utt_id}.npz" traverse outside output_dir. |
| F-260 | low | `training/pyproject.toml:43` | training/pyproject.toml:43 pins the optional `asr` extra's ReazonSpeech dep to a git branch tip, not a commit, so installs are unreproducible and unverifiable. |
| F-271 | low | `training/src/auris_singer/text/__init__.py:67` | get_frontend's docstring promises kwargs are forwarded to the front-end constructor, but the ipa/none/raw branch silently drops them instead. |
| F-366 | low | `training/src/auris_singer/model.py:288` | AurisSinger.infer() silently truncates to min(durations.sum, f0 length) instead of raising, contradicting its own docstring contract, though no current caller […] |
| F-423 | low | `training/src/auris_singer/lightning_module.py:292` | kl_scale() reads global_step after opt_d.step() has already advanced it, shifting the KL warm-up ramp by one optimizer step. |
| F-435 | low | `training/src/auris_singer/host_eval.py:487` | --speaker's docstring/help claim an unconditional "model's first" default, but corpus mode defaults per-utterance to the corpus's own recorded speaker instead. |

### F-013 · critical · A single corrupt WAV or non-UTF-8 transcript crashes preprocessing unhandled, discarding every already-computed .npz because the dataset manifest is written only after the loop completes without error.

`training/src/auris_singer/preprocess/pipeline.py:198` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `uv run python scripts/preprocess.py` over a real downloaded corpus (which the project's own recipes tell users to curl/unzip and hand-assemble) crashes with an unhandled traceback the moment one WAV is corrupt/truncated or one transcript isn't valid UTF-8. Every utterance processed before that point already has its FCPE/energy/G2P work done and its `.npz` written to disk, but `metadata.jsonl`, `speakers.json`, `phonemes.json`, and `audio_config.json` are only written after the whole loop finishes successfully — so none of that work is usable by `SingingDataset`, and the user must find/remove the one bad file and rerun preprocessing over the entire corpus from scratch.

**Trigger.** A real downloaded/unzipped corpus (this project's own docs instruct `curl`+`unzip` of third-party corpora) contains one truncated/corrupted WAV, or one transcript file that isn't valid UTF-8 (e.g. a stray Shift-JIS file mixed into a hand-assembled `text_dir`, which `generic_wav_text.yml`'s recipe explicitly invites users to build). `num_workers` (default 4-8) means this is reached partway through a run that may already have processed hundreds of files.

**Mechanism.** `stage_one` (lines 188-199) calls `utt.text_path.read_text(encoding="utf-8")` (line 192, no `errors=` fallback) and `_load_audio(utt.wav_path, ...)` (line 198, `sf.read` with no try/except) for every utterance, run inside `pool.map(stage_one, utterances)` on a `ThreadPoolExecutor` (line 208-210). Neither call is guarded. When one worker raises, `concurrent.futures.Executor.map` re-raises that exception the moment the main `for utt, wav, text_info, error in tqdm(pool.map(...))` loop (line 209) reaches that item, unwinding out of `run_preprocess` entirely. I reproduced this directly: `ThreadPoolExecutor.map` over 5 items where item 2 calls `sf.read` on a corrupt file yields results for items 0-1 and then raises `LibsndfileError`, silently dropping items 3-4. Critically, `metadata.jsonl`, `speakers.json`, `phonemes.json` and `audio_config.json` are written only *after* the whole loop finishes without error (lines 281-299) — every `.npz` already saved to disk by prior iterations (line 263) becomes orphaned, unreadable by `SingingDataset` (which reads `metadata.jsonl`), because that […]

**Expected.** doc/preprocessing.md's own 'Skipped utterances' section documents a skip-and-report contract ('An utterance is skipped when it has no transcript, produces no phonemes, is shorter than min_seconds...'); a decode failure on audio or text should be caught in `stage_one` and reported through the same `skip(reason)` mechanism already used for every other per-utterance failure, rather than aborting the run and discarding the manifest for every utterance already processed.

**Fix direction.** Wrap the two unguarded calls in `stage_one` (the `text_path.read_text` at pipeline.py:192 and `_load_audio`/`sf.read` at line 198) in a try/except that converts a decode failure into the same `(utt, None, None, reason)` skip tuple already used for missing/empty transcripts, so `skip("decode error: ...")` is recorded and the loop continues; the existing skip-and-report summary and end-of-run manifest writing then cover this case like every other per-utterance failure.

**Written rule it breaks.** doc/preprocessing.md, "Skipped utterances": "An utterance is skipped when it has no transcript, produces no phonemes, is shorter than `min_seconds`, or has fewer frames than phonemes ... The run prints a summary of skip reasons at the end." — decode failures on audio/text are not routed through this skip mechanism and instead abort the run.

### F-028 · high · The song render in host_eval.py sings the whole concatenated song in rows[0]'s speaker, silently mismatching every other utterance's own voice.

`training/src/auris_singer/host_eval.py:694` · spec-mismatch · confirmed (traced through the code; reported independently 3×)

**What a user sees.** Whenever a training-diagnostic run (`--split all` or a `--split val` draw that happens to span speakers, with no `--speaker` override) enables the song render, every utterance after the first is sung with rows[0]'s speaker instead of its own. The resulting `song − host` metric diff — which doc/evaluation.md tells the reader to interpret purely as a chunking/stitching signal — is silently contaminated by voice/timbre/pitch-range mismatch for a multi-speaker corpus, so a developer chasing a real stitching bug gets a false signal, or dismisses a real seam artifact as voice-mismatch noise.

**Trigger.** settings.speaker is None (the common, unnamed-run case) and evaluate() draws two or more utterances whose corpus records have different `speaker` values — e.g. `--split all` on a multi-speaker corpus (the five-speaker jsut_song_vocalset or three-style Namine Ritsu recipes this same file's campaign log documents), or simply a `--split val` draw that happens to span speakers, since `validation_records()` shuffles the whole corpus without grouping by speaker.

**Mechanism.** In evaluate(), the per-utterance host/reference columns correctly pick each record's own speaker: `speaker = settings.speaker or record.get("speaker")` (line 630), used at line 636. But the concatenated "song" render — built by joining every utterance's curves end to end via `concatenate_frames(frames_list, gap)` (line 684) regardless of which speaker recorded them — is sung with a single fixed speaker: `host.sing_frames(frames_path, info.path, rendered, seed=seed, acceleration=settings.acceleration, speaker=settings.speaker or rows[0].get("speaker"))` (lines 691-695). `HostFrames` carries no per-segment speaker field, and `Host.sing_frames()` (host.py) takes exactly one `--speaker` for the whole file, so the entire joined song — including utterances 2..N — is sung as `rows[0]`'s speaker whenever `settings.speaker` is None.

**Expected.** training/doc/evaluation.md line 185 (added in commit a30fda1, "Sing each corpus utterance as its own speaker", which fixed exactly this for the host/reference columns): "the corpus run sings each utterance as the speaker the corpus says it belongs to." The song column, part of the same corpus run, should do the same — or the run should refuse/skip the song column when the sampled utterances span more than one speaker, rather than silently rendering them all in `rows[0]`'s voice.

**Fix direction.** In the song block (host_eval.py ~684-695), either give HostFrames/host.sing_frames a per-segment speaker so the joined render can switch speaker at each span the way the per-utterance loop already does, or — simplest — refuse/skip the song column (with a clear log message) whenever the sampled utterances span more than one distinct speaker and settings.speaker is None, rather than silently rendering the whole song in rows[0]'s voice.

**Written rule it breaks.** training/doc/evaluation.md line 185 (added by commit a30fda1, "Sing each corpus utterance as its own speaker"): "the corpus run sings each utterance as the speaker the corpus says it belongs to."

### F-039 · high · The "too short" guard only enforces wav.numel() >= hop_length, not the (n_fft-hop_length) reflect-pad width frame_energy needs, so a short utterance can crash the whole preprocessing run instead of being skipped.

`training/src/auris_singer/preprocess/pipeline.py:217` · correctness · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** Running the auris_singer preprocess pipeline with a dataset config that omits or sets a low `audio.min_seconds` (any custom dataset config a user writes — the five shipped presets all pin min_seconds: 0.5, which happens to be safe) crashes the entire preprocessing run with an uncaught PyTorch RuntimeError ("Padding size should be less than the corresponding input dimension") the moment an utterance between hop_length (480) and 784 samples reaches frame_energy's reflect-pad. Because this runs inside ThreadPoolExecutor.map, the exception aborts the whole job for every remaining utterance instead of being skipped and logged like the other "too short" / "fewer frames than phonemes" cases the same loop already handles.

**Trigger.** A preprocessing config that omits `audio.min_seconds` (the code's own default is then 0.0, not a safe value) or sets it below ~0.0163 s at 48 kHz. All four shipped configs happen to set `min_seconds: 0.5`, but `configs/preprocess/generic_wav_text.yml`'s own comment block invites users to adapt it for a new corpus, and nothing prevents a smaller value.

**Mechanism.** `min_samples = int(float(audio_cfg.get("min_seconds", 0.0)) * sample_rate)` (line 183) defaults to 0 when a config omits `audio.min_seconds`, and the length gate at line 217 is `if wav.numel() < max(min_samples, hop_length): skip("too short")`, so it only guarantees `wav.numel() >= hop_length`. `frame_energy`/`spectrogram` reflect-pad each side by `(n_fft - hop_length) // 2` (`utils/audio.py` line 65), which for the shipped `n_fft=2048, hop_length=480` is 784 samples — almost twice `hop_length`. I confirmed the crash directly: `frame_energy(torch.zeros(480), n_fft=2048, hop_length=480, win_length=2048)` raises `RuntimeError: Argument #4: Padding size should be less than the corresponding input dimension, but got: padding (784, 784) at dimension 2 of input [1, 1, 480]`.

**Expected.** The length gate should enforce the actual downstream requirement (`wav.numel() > (n_fft - hop_length) // 2`, not just `>= hop_length`), or `min_seconds` should default to a value that satisfies it, so a short utterance is skipped and reported rather than crashing the run.

**Fix direction.** At training/src/auris_singer/preprocess/pipeline.py:217, size the guard from what frame_energy actually needs, not just hop_length: require wav.numel() > n_fft - hop_length (784 for the shipped n_fft/hop_length), i.e. `if wav.numel() < max(min_samples, hop_length, n_fft - hop_length + 1): skip(...)`, so an undersized waveform takes the existing skip("too short") path instead of raising. Consider also wrapping the per-utterance body in try/except to log-and-skip rather than abort the whole batch on any future exception.

### F-073 · high · DistributedBucketSampler.epoch is set once at construction and never advanced, because use_distributed_sampler=False (train.py:103) disables Lightning's automatic set_epoch forwarding and nothing else calls it, so every training epoch reshuffles buckets with the identical seed.

`training/src/auris_singer/data/datamodule.py:110` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A person training a voice model never sees an error, but every epoch shuffles the buckets identically: the intra-bucket sample order (and thus which utterances land in the same batch) is fixed by `torch.randperm(generator=generator.manual_seed(self.epoch))` reseeded with the same `self.epoch == 0` on every single epoch, for the entire training run. Training silently loses per-epoch data reshuffling, a well-known contributor to generalization, with no diagnostic anywhere pointing at the cause.

**Trigger.** Any training run of more than one epoch through `SingingDataModule` + `AurisSingerModule` via `scripts/train.py` -- the shipped `configs/train/base.yml` sets `trainer.max_steps: 1000000` (500k batches with manual two-optimizer stepping), which for any realistic-size corpus spans many thousands of epochs.

**Mechanism.** `DistributedBucketSampler.__iter__` (training/src/auris_singer/data/dataset.py:250-253) draws its per-epoch permutation from `generator.manual_seed(self.epoch)`, exactly mirroring `torch.utils.data.distributed.DistributedSampler`'s own contract, whose docstring warns: 'calling set_epoch() at the beginning of each epoch ... is necessary to make shuffling work properly across multiple epochs. Otherwise, the same ordering will be always used.' `SingingDataModule.train_dataloader()` passes the sampler as `batch_sampler=sampler` to `DataLoader(...)` (datamodule.py:108-115). Because a `batch_sampler` is supplied, `torch.utils.data.DataLoader.__init__` (torch/utils/data/dataloader.py ~394-401) leaves `sampler=None` so it falls into the 'give default samplers' branch and sets `self.sampler = SequentialSampler(dataset)` -- a *different* object from the `DistributedBucketSampler`. Lightning's automatic per-epoch hook, `_set_sampler_epoch` (lightning/fabric/utilities/data.py:413-436), is invoked once per epoch from `fit_loop.py:445` and only calls `.set_epoch()` on `dataloader.sampler` and on […]

**Expected.** Per the class's own `DistributedSampler`-derived contract (and standard PyTorch practice), each epoch should draw a fresh permutation, which requires `set_epoch(current_epoch)` to be called before that epoch's iteration begins -- either by wiring the sampler so Lightning's `_set_sampler_epoch` can find it (e.g. `sampler=` instead of `batch_sampler=`, or exposing a `.sampler` attribute), or by calling `sampler.set_epoch(...)` explicitly, e.g. from `on_train_epoch_start`.

**Fix direction.** In `SingingDataModule.train_dataloader()` (training/src/auris_singer/data/datamodule.py:101-115), since `use_distributed_sampler=False` is deliberate (per its own docstring at line 35) and Lightning's automatic epoch-forwarding is therefore never invoked, the code must call `.set_epoch()` itself: either implement a `on_train_epoch_start` hook (or pass a Lightning `Callback`) that calls `train_dataloader().batch_sampler.set_epoch(trainer.current_epoch)`, or store a reference to the sampler and increment `sampler.epoch` from the training loop each epoch, mirroring what `training/tests/test_dataset.py` already does manually (`sampler.set_epoch(0)`) for its own tests.

### F-115 · high · phoneme_levels.py classes devoiced Japanese vowels as full vowels, so a whispered vowel becomes the loudness reference for the consonant before it, under-correcting exactly the sibilants/plosives the table exists to fix.

`training/src/auris_singer/phoneme_levels.py:90` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Every exported voice model's `phoneme_levels` metadata table is measured with this code. For any Japanese utterance where a consonant is followed by a devoiced vowel (a common phenomenon — e.g. /k/, /s/, /t/, /ɕ/ before devoiced /i/ or /ɯ/ between voiceless neighbours, as in "sh_i_ta", "ky_u_shu"), the module (a) never records a level for the devoiced vowel itself, and (b) uses that devoiced vowel's own median RMS — quiet by definition, since it's whispered — as the loudness denominator for the preceding consonant. Because both numerator and denominator are quiet, the resulting dB ratio is pulled toward 0 dB instead of the expected -20-ish dB. This systematically under-corrects exactly the sibilants/plosives the module's own docstring says most need correction, for every speaker whose corpus contains devoicing (i.e. essentially all Japanese singing corpora). The shipped level table is therefore quietly wrong for a subset of very common phoneme contexts, and any voice trained/corrected with it will render those consonants too loud relative to what the docstring's own measured JSUT-song numbers call for.

**Trigger.** Any utterance where a measured consonant is immediately followed by a devoiced vowel, e.g. the common Japanese word です (de-su, phonemes d,e,s,ɯ̥): the fricative "s" is scored against the devoiced "ɯ̥" — which is whispered and quiet, not a full-loudness vowel — instead of skipping past it to a real vowel.

**Mechanism.** `measure()` decides whether a phoneme is a vowel (and so excluded from measurement and eligible as the loudness reference for the previous consonant) purely via `phoneme_class(symbol) in {"vowel", "special"}` (line 90) and the lookahead `next(((s, e) for q, s, e in spans[index + 1:] if phoneme_class(q) == "vowel"), None)` (lines 92-93). `auris_singer.text.ipa.PHONEME_CLASSES["vowel"]` explicitly includes the devoiced vowels (`"ḁ"`, `"i̥"`... i.e. `ḁ i̥ ɯ̥ e̥ o̥`), so a devoiced vowel is treated exactly like a full vowel here: it is never itself measured, and it is picked as the "next vowel" reference for the consonant before it. `phoneme_durations.py`'s sibling module explicitly disagrees with this: its `STRETCHED` set (line 79-85) deliberately excludes the devoiced vowels because "they are whispered between voiceless neighbours and behave like consonants, taking a slot of their own" — i.e. the durations module treats a devoiced vowel as consonant-like, but the levels module does not.

**Expected.** A devoiced vowel should be treated the way `phoneme_durations.STRETCHED`'s exclusion already treats it — as a consonant-like unit that is itself measured, and skipped over (not selected) when searching for the next real vowel to use as a loudness reference.

**Fix direction.** In `measure()` (training/src/auris_singer/phoneme_levels.py:90-93), stop treating devoiced vowels as members of the reference class: either give them their own `phoneme_class` distinct from `"vowel"` (mirroring `phoneme_durations.py`'s treatment of them as consonant-like), or explicitly exclude the devoiced set from both the skip condition and the vowel-lookahead predicate so the lookahead walks past a devoiced vowel to the next fully-voiced one. The docstring's contract ("the vowels themselves are the reference") should then only apply to genuinely voiced vowels.

**Written rule it breaks.** training/src/auris_singer/phoneme_durations.py:76-78 (sibling module, same codebase): "The devoiced vowels are deliberately *not* here: they are whispered between voiceless neighbours and behave like consonants, taking a slot of their own." — phoneme_levels.py contradicts this by classing devoiced vowels as `"vowel"` and using them as the reference denominator.

### F-119 · high · A single too-short utterance (exactly one hop of samples) crashes `training`'s whole preprocessing run via an unhandled reflect-pad RuntimeError in `frame_energy`/`_pad_for_stft`.

`training/src/auris_singer/utils/audio.py:65` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `training` preprocessing over a real corpus that contains one utterance trimmed to exactly one hop's worth of samples (480 samples at the shipped hop_length=480, e.g. after aggressive silence trimming or a mis-cut segmentation label) raises an unhandled RuntimeError from `torch.nn.functional.pad` inside `frame_energy`/`_pad_for_stft` and aborts the entire preprocessing run — losing all utterances already processed in that run, not just the offending one, since the main loop has no try/except around feature extraction.

**Trigger.** A preprocessing config whose `audio.min_seconds` is unset or set below ~0.02s (20 ms) — legitimate for a corpus with genuinely short interjections — combined with a source utterance between 480 and 959 samples long (10-20 ms at 48 kHz), producing `n_frames == 1`. `frame_energy(wav, n_fft, hop_length, win_length)` then reflect-pads a 480-sample tensor by 784 on each side and raises `RuntimeError: Padding size should be less than the corresponding input dimension`.

**Mechanism.** `_pad_for_stft` reflect-pads the waveform by `pad = (n_fft - hop_length) // 2` samples on each side (line 65-66) before framing. `torch.nn.functional.pad(..., mode="reflect")` requires the padded dimension's size to exceed the pad amount; for the shipped audio settings (n_fft=2048, hop_length=480) `pad = 784`. `run_preprocess` (preprocess/pipeline.py:183) computes `min_samples = int(float(audio_cfg.get("min_seconds", 0.0)) * sample_rate)` — defaulting to 0 when a config omits `min_seconds` — and its length filter at line 217 (`if wav.numel() < max(min_samples, hop_length): skip(...)`) only guarantees at least one hop's worth of audio (480 samples), not enough to satisfy the padder. `frame_energy` is then called unconditionally on the trimmed clip at line 249.

**Expected.** Either enforce a minimum admissible clip length tied to `(n_fft - hop_length) // 2` regardless of the configured `min_seconds`, or catch/skip a clip too short to frame instead of letting the exception abort the run.

**Fix direction.** In `run_preprocess`, raise the length floor so it always exceeds the STFT reflect-pad requirement, e.g. skip when `wav.numel() < max(min_samples, n_fft)` (or at least `> 2*pad`) instead of `max(min_samples, hop_length)`, so a clip too short for `_pad_for_stft`'s reflect padding is filtered out with a normal "too short" skip rather than crashing; alternatively, have `_pad_for_stft` fall back to constant/replicate padding when the input is shorter than `pad`.

### F-320 · high · load_weights unpickles an unvalidated --init-from/--resume checkpoint via torch.load(weights_only=False), giving arbitrary code execution on a crafted file.

`training/src/auris_singer/lightning_module.py:154` · security · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user who runs training with --init-from or --resume pointing at a checkpoint they downloaded, received from a collaborator, or fetched from a shared model repo gets arbitrary code execution on their machine the instant torch.load runs — before load_weights does any of its own validation. This is a checkpoint-as-attachment scenario, not a hypothetical: pretrained checkpoints for transfer learning are routinely shared/downloaded exactly as this flag expects.

**Trigger.** `training/scripts/train.py` passes `--init-from PATH` straight to `module.load_weights(PATH)` (line 75) and `--resume PATH` straight to `trainer.fit(..., ckpt_path=args.resume)` (line 106); a user who points either flag at a downloaded or shared `.ckpt` file that turns out to be crafted runs that file's embedded code with their own privileges the moment `torch.load` is called.

**Mechanism.** `checkpoint = torch.load(path, map_location="cpu", weights_only=False)` unpickles the entire checkpoint object with Python's pickle protocol. `weights_only=False` is exactly the flag PyTorch's own docs warn against for untrusted files: pickle can execute arbitrary code (via `__reduce__`) during deserialization, before any of this function's shape/key validation ever runs.

**Expected.** Load with `weights_only=True` (or PyTorch's `torch.serialization.add_safe_globals` for the specific Lightning classes actually needed) so a malicious checkpoint fails to deserialize instead of executing.

**Fix direction.** Load with weights_only=True (PyTorch's safe unpickler restricted to tensors/plain containers) as the default; if the checkpoint format requires non-tensor objects, register them explicitly via torch.serialization.add_safe_globals rather than disabling the safety check wholesale. Apply the same fix to the --resume path if Lightning's ckpt loading can be configured the same way.

### F-127 · medium · architecture.md's loss table lists KL (auxiliary) default as 1.0, but code, training.md, and the doc's own later prose all agree the default is 0.2.

`training/doc/architecture.md:148` · spec-mismatch · confirmed (traced through the code; reported independently 3×)

**What a user sees.** A trainer developer reading architecture.md's loss table to set `loss.kl_aux` in a config, or to sanity-check a run, sees "1.0" as the default and either leaves an explicit override at 1.0 (doubling KL pressure relative to VITS, per the doc's own explanation) or is confused when the actual run logs/config default to 0.2. The same document contradicts itself 46 lines later, so a careful reader catches it, but the table is the part most likely to be skimmed and trusted at a glance.

**Trigger.** Reading only the architecture.md loss table (the natural first place to look up a default) reports the auxiliary KL weight as 1.0.

**Mechanism.** The "Losses" table states `| KL (auxiliary) | 1.0 | against the expanded phoneme-level prior; the MAS objective |` (line 148), but the code default is 0.2 (`training/src/auris_singer/lightning_module.py:116`: `"kl_aux": float(loss.get("kl_aux", 0.2))`), and the same architecture.md file's own later prose contradicts its table: "The auxiliary KL weight also defaults to 0.2 rather than 1.0: its job is to keep the alignment statistic honest, and at full weight it doubles the total KL pressure relative to VITS" (architecture.md lines 194-196). `training/doc/training.md`'s example config likewise shows `kl_aux: 0.2 # alignment statistic only; 1.0 doubles KL pressure`.

**Expected.** The table should read 0.2, matching the code (`lightning_module.py:116`), `training.md`'s example config, and the document's own explanatory text two sections later.

**Fix direction.** Change the "KL (auxiliary)" row in training/doc/architecture.md's loss table (line 148) from "1.0" to "0.2", matching lightning_module.py's `loss.get("kl_aux", 0.2)` default, the doc's own prose at lines 194-196, and training.md's example config.

**Written rule it breaks.** CLAUDE.md: "A failure is a decision, not an edit to whichever file the assertion named. Either the export moved on and the host must learn to read it ... or the host is right and the export is wrong." (context: training/tests/test_host_contract.py rule, but the same principle of keeping doc/code claims synchronized applies — here the doc simply has a stale/wrong value contradicted by its own […]

### F-157 · medium · EnvelopeLoss divides by the configured kernel count instead of the count actually used, silently underweighting the loss when a kernel exceeds the signal length.

`training/src/auris_singer/losses.py:154` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When a project's segment_size (or a custom envelope_kernel_sizes) is smaller than one of the configured kernel windows, EnvelopeLoss silently divides by the full configured resolution count instead of the count that actually fired, scaling the loss down (e.g. by half when 1 of 2 configured resolutions is skipped). Training proceeds with no error, but the envelope/loudness-dynamics term is underweighted relative to the loss.envelope weight the training docs calibrate against, so the model gets weaker amplitude-envelope supervision than configured.

**Trigger.** Call `EnvelopeLoss(kernel_sizes=(64, 4096))(real, fake)` on 100-sample inputs (exactly the scenario `tests/test_losses.py::test_envelope_loss_skips_windows_longer_than_the_signal` exercises, but that test only asserts `torch.isfinite(...)` and never checks the magnitude). Verified empirically: with mismatched real/fake, the manual single-resolution term is 0.4643, but `EnvelopeLoss(...)` returns 0.2321 — exactly half, because it divides by `len(kernel_sizes)=2` instead of the `1` resolution that actually fired.

**Mechanism.** `EnvelopeLoss.forward` skips a `kernel_size` whenever `real.size(-1) < kernel_size` (line 149) but still normalizes by the full configured count: `return total / max(len(self.kernel_sizes), 1)` (line 154). Its sibling `MultiParamMelLoss.forward`, which has the identical skip-when-too-short pattern, instead tracks `used` and returns `total / max(used, 1)` (losses.py lines 197-214) — the correct behaviour, and the one the same class's docstring implies ("RefineGAN envelope loss" matching several window sizes).

**Expected.** Divide by the number of resolutions actually evaluated in the loop, exactly as `MultiParamMelLoss.forward` does with its `used` counter.

**Fix direction.** Track a running used counter in EnvelopeLoss.forward, incremented each time a kernel_size is not skipped (mirroring MultiParamMelLoss.forward's pattern at losses.py:197-214), and return total / max(used, 1) instead of total / max(len(self.kernel_sizes), 1).

**Written rule it breaks.** No CLAUDE.md rule is broken directly, but it diverges from the codebase's own established correct pattern: MultiParamMelLoss.forward normalizes by an actual-used counter for the identical skip-when-too-short case.

### F-175 · medium · NsfHifiGanGenerator's own kernel>=rate guard admits rate=1/even-kernel stages whose output_padding=1 is invalid for stride=1, crashing on first forward instead of at construction.

`training/src/auris_singer/modules/generator.py:161` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Anyone who configures NsfHifiGanGenerator with an upsample stage of rate=1 and an even kernel (a legal combination per the constructor's own `kernel >= rate` check) gets a generator that constructs without error but crashes with a PyTorch RuntimeError ("output padding must be smaller than either stride or dilation") on the very first forward pass, wasting setup/training time before the bug surfaces. No shipped preset in configs/train/presets.yml currently hits this (both presets use upsample_rates=[6,5,4,4], no rate=1 stage), so it is an edge-configuration path, not the default training run.

**Trigger.** Verified by construction: `NsfHifiGanGenerator(in_channels=8, hop_length=2, upsample_rates=(2, 1), upsample_kernel_sizes=(4, 2), upsample_initial_channel=16)` builds without error, but calling `.forward(...)` raises `RuntimeError: output padding must be smaller than either stride or dilation, but got output_padding_height: 0 output_padding_width: 1 stride_height: 1 stride_width: 1 dilation_height: 1 dilation_width: 1` from the second (`rate=1, kernel=2`) stage.

**Mechanism.** The constructor only rejects `kernel < rate` (`if kernel < rate: raise ValueError(f"upsample kernel {kernel} must be >= its rate {rate}")`, lines 161-164), implying any `kernel >= rate` is safe, and derives `padding = (kernel - rate + 1) // 2` / `output_padding = rate + 2 * padding - kernel` (lines 167-168) to make each `ConvTranspose1d` output exactly `rate ×` its input length. For `rate == 1` and even `kernel`, this yields `output_padding == 1`, but PyTorch requires `output_padding < stride` for `ConvTranspose1d`, and `stride == rate == 1` here, so `output_padding=1` is invalid.

**Expected.** The validation should reject (or the padding formula should handle) any `(kernel, rate)` pair that would make `output_padding >= rate`, not just `kernel < rate`; alternatively the error message's claim `kernel {kernel} must be >= its rate {rate}` should not imply that condition alone is sufficient for a working configuration.

**Fix direction.** Tighten the constructor's validation to reject any (kernel, rate) pair whose derived `output_padding = rate + 2*((kernel-rate+1)//2) - kernel` is not `< rate` (or, more simply, require kernel > rate when rate==1, and in general assert `output_padding < rate` right after computing it), raising the same ValueError style at construction time instead of letting PyTorch fail inside forward().

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-182 · medium · summarize_speaker falls back to default=0.0 for an empty-levels speaker, exactly the "plateau" value its own docstring and doc/inference.md promise never happens.

`training/src/auris_singer/phoneme_levels.py:160` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When a speaker has no measured phoneme levels at all (e.g. a VocalSet-style speaker whose transcript never puts a consonant before a vowel above ENERGY_FLOOR, so `levels` ends up empty), `summarize_speaker` silently exports `"default": 0.0` in the voice's shipped metadata. A consumer trusting the documented invariant ("never 0 dB") will apply a 0 dB gain (unity, i.e. no attenuation) to every unnamed consonant for that speaker, exactly reproducing the loudness plateau the whole table exists to suppress — silently, with no error or warning anywhere in the pipeline.

**Trigger.** A speaker whose measure() output is empty because none of their utterances contain a countable consonant-to-vowel transition (all frames are vowels/silence, or every reading is below ENERGY_FLOOR). training/doc/evaluation.md's own project history documents exactly this case: the 'five speakers in one voice' campaign notes 'The VocalSet speakers, who sang no consonant, hear none' — i.e. real exported voices in this project have had speakers with zero measured consonant levels. summarize({"that_speaker": {}}, measured_from=...) — the same shape the contract test itself exercises at training/tests/test_host_contract.py's `phoneme_levels.summarize({"x": {}}, ...)` — produces {"default": 0.0, […]

**Mechanism.** summarize_speaker()'s docstring (lines 149-152) states that `default` is 'the median over every measured reading, which is what a consonant the table does not name should be assumed to be ... rather than 0 dB, which would be the plateau this table exists to correct.' Line 160 is `default = float(round(float(np.median(pooled)), 1)) if pooled else 0.0` — when `pooled` (every raw dB reading collected for that speaker by measure()) is empty, the function falls back to literally 0.0, the exact value the docstring says the table exists to avoid. training/doc/inference.md line 377 makes the same promise to consumers of the exported format: 'speakers.*.default | the level for a consonant not named in `db` — the pooled median, a consonant's level, never 0 dB'. The Rust host's own validation in crates/auris-singer/src/metadata.rs (`broken = |db: &f64| !db.is_finite();`, around line 229) only rejects non-finite values, not 0.0, so a table with default=0.0 loads without error.

**Expected.** Per the function's own docstring and training/doc/inference.md line 377, `default` should never be 0 dB; an empty `pooled` list (no measurable consonant in the speaker's corpus) should fall back to a value that is clearly 'a consonant, quieter than a vowel' — e.g. the module's own floor, or omitting the speaker from `speakers` entirely rather than shipping a table entry that silently disables consonant attenuation while claiming to be measured data.

**Fix direction.** Replace the `else 0.0` fallback with either raising/warning (refuse to export a table with no measured data for that speaker, mirroring the width-table's existing "refuses to ship" precedent noted in doc/inference.md around FCPE/width bars) or falling back to a documented sentinel that is not the "plateau" value — e.g. reuse a project-wide or corpus-wide median instead of a per-speaker empty pool. At minimum, the docstring and inference.md promise must be made true for the empty case, not merely true for the typical case.

**Written rule it breaks.** summarize_speaker docstring: "default is the median over every measured reading, which is what a consonant the table does not name should be assumed to be — a consonant, quieter than a vowel — rather than 0 dB, which would be the plateau this table exists to correct."; doc/inference.md:377: "speakers.*.default | the level for a consonant not named in db — the pooled median, a consonant's level, […]

### F-193 · medium · doc/inference.md documents an "author" card.json field that VoiceCard (metadata.rs) has no field for, so it silently vanishes on import.

`training/doc/inference.md:433` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A voice packager follows the documented card.json convention, fills in "author", exports the .onnx, and it plays fine everywhere — but no Rust frontend (GUI, CLI, MCP, agent) ever displays the author, because VoiceCard silently drops the field on deserialize. The data survives in the file's metadata_props/sidecar but is invisible to every consumer in the workspace, with no warning that it was dropped.

**Trigger.** uv run python scripts/export_onnx.py --checkpoint last.ckpt --output voice.onnx --voice-card card.json, where card.json follows the documented convention and includes an "author" field (exactly the example doc/inference.md gives). The .onnx metadata_props and voice.json sidecar both contain voice.author, but crates/auris-singer::metadata::VoiceInfo::parse() (used by every Rust consumer, e.g. a future voice-browser UI) produces a VoiceCard with no way to read it back — the field is not merely empty, it does not exist on the type.

**Mechanism.** training/doc/inference.md lines 427-428 state: '`card.json` is a free-form JSON object; these field names are the convention a UI can rely on', followed by an example card.json (lines 430-439) that includes `"author": "..."`. The Python side (export.py's export_onnx/metadata_block, and scripts/export_onnx.py's `--voice-card` handling) does not filter the voice dict at all — it JSON-dumps whatever the card file contains verbatim into both metadata_props and the .json sidecar, so `author` really is written to disk. But crates/auris-singer/src/metadata.rs's `VoiceCard` struct (lines 138-158) only declares `name`, `description`, `version`, `license`, `credits`, `url` — no `author` field — and carries no `#[serde(deny_unknown_fields)]`, so serde's default behavior silently drops any `author` key when the host deserializes the JSON into `VoiceInfo.voice`.

**Expected.** Either crates/auris-singer/src/metadata.rs's VoiceCard struct should carry an `author` field (with #[serde(default)] to stay backward-compatible with existing exports), or training/doc/inference.md's convention table/example should not claim a field the host cannot represent.

**Fix direction.** Either add `pub author: String` (with `#[serde(default)]`) to `VoiceCard` in crates/auris-singer/src/metadata.rs so the documented convention is actually representable, or remove "author" from the card.json example in training/doc/inference.md if it's not meant to be a real convention field. Given the doc explicitly calls these "field names the convention a UI can rely on," adding the field to the struct is the correct direction.

**Written rule it breaks.** `card.json` is a free-form JSON object; these field names are the convention a UI can rely on

**Verifier's correction.** Same claim, but the "author" field in the card.json example sits on training/doc/inference.md line 434, not line 433 (line 433 is the opening `{` of the JSON block).

### F-201 · medium · A zero-length source WAV crashes the entire training preprocessing run instead of being skipped like other bad-input utterances.

`training/src/auris_singer/preprocess/pipeline.py:138` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `uv run` preprocessing over a real dataset that happens to contain one zero-length/corrupt WAV file (e.g. a failed recording or a truncated export) crashes the entire batch job with an uncaught PyTorch RuntimeError from `wav.abs().max()`, instead of skipping that one utterance and continuing — the user loses all preprocessing progress on every other utterance in the run and gets a stack trace pointing into `_load_audio` rather than a clear "skipped: zero-length audio" log line.

**Trigger.** A dataset source containing one zero-duration (but well-formed) .wav file — plausible in a large scraped/auto-trimmed corpus, e.g. a silence-trimming step that emptied a file, or a placeholder asset. `stage_one` already special-cases 'missing transcript' and 'empty transcript' for the equally-plausible empty-text case (lines 190-194) but has no equivalent guard for empty audio before it reaches `_load_audio`.

**Mechanism.** `_load_audio` (lines 132-141) does `wav, sr = sf.read(...)`; `wav = torch.from_numpy(wav.mean(axis=1))`; then unconditionally (when `peak_normalize` is true, the shipped default) `scale = wav.abs().max()` at line 138. For a syntactically valid but zero-duration WAV file, `sf.read` returns an array of shape `(0, channels)` without raising, giving a 0-length tensor, and `.max()` on a 0-element tensor raises. I reproduced this by writing a valid 0-sample WAV with soundfile and calling the repo's own `_load_audio` on it: `RuntimeError: max(): Expected reduction dim to be specified for input.numel() == 0.` `_load_audio` is called from `stage_one` (line 198), which runs inside `pool.map(stage_one, utterances)` (line 209-210); the exception surfaces when the main thread consumes that utterance's result and is not caught anywhere in `run_preprocess`, aborting the whole run.

**Expected.** An empty/zero-sample audio file should be reported and skipped like the other per-utterance failure modes `stage_one` already returns (`return utt, None, None, "<reason>"`), not allowed to raise out of the thread pool and abort the entire preprocessing run.

**Fix direction.** In `_load_audio`, check `wav.numel() == 0` (or `wav.numel() < 1`) immediately after building the tensor and either raise a clearly-named exception that `stage_one` catches and reports as a skip reason ("empty audio"), or move a minimal length check before the `peak_normalize` block so `.abs().max()` is never called on an empty tensor. This mirrors the existing "missing transcript"/"empty transcript"/"empty phoneme sequence" skip pattern already used for other bad-input cases in `stage_one`.

**Written rule it breaks.** A failure is a decision, not an edit to whichever file the assertion named [training/tests/test_host_contract.py convention] — more directly, `stage_one`'s existing pattern of returning a skip reason for "missing transcript", "empty transcript", and "empty phoneme sequence" establishes that per-utterance bad input should be skipped and logged, not allowed to crash the run.

### F-202 · medium · read_mono_label/read_label crash the whole corpus-prep run on one malformed label line instead of skipping it, unlike their sibling parser which already guards with try/except ValueError.

`training/scripts/prepare_namine_ritsu.py:68` · correctness · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Running `prepare_namine_ritsu.py` (or `prepare_jsut_song.py`) over a corpus where one `.lab` file has a malformed numeric field — a hand-edit typo, a stray token, or a `�` replacement character from the script's own `errors="replace"` decode landing in a time field — crashes the whole preprocessing run with an uncaught `ValueError` traceback partway through. Songs already written stay on disk, but the run aborts without ever printing its summary (`n_phrases`, `skipped`, `unknown`), so the user has to find and hand-fix the one bad label line before they can re-run instead of the bad phrase simply being skipped and counted.

**Trigger.** A hand-corrected or third-party `.lab`/mono-label file with one malformed line that still happens to split into exactly 3 (mono) or >=3 (HTS) tokens with a non-numeric time field — plausible in the 'hand-corrected' Namine Ritsu labels or any HTS label file with an unexpected header/comment line.

**Mechanism.** `read_mono_label` (lines 60-69) reads a label file with `errors="replace"` (anticipating encoding corruption) but then does `Phoneme(int(parts[0]) * TIME_UNIT, int(parts[1]) * TIME_UNIT, symbol)` (line 68) with no try/except: a line with exactly 3 whitespace-separated tokens whose first two aren't valid integers (e.g. a stray header/comment line, or a `�` replacement character landing in a numeric field because of the very encoding fallback this function uses) raises an uncaught `ValueError` that propagates out of `main()` (no try/except anywhere in the call chain) and aborts the whole database preparation. `read_label` in `prepare_jsut_song.py` (lines 79-92, specifically line 90) has the identical gap. `measure_phoneme_durations.py`'s `read_timed_phonemes` (lines 62-74) parses the same two label formats and explicitly wraps the same `int()` conversion in `try: ... except ValueError: continue` (lines 69-72) — proof the failure mode was identified and fixed in one of the three near-duplicate readers but never carried back to the two preparation scripts.

**Expected.** Both `read_label` and `read_mono_label` should catch `ValueError` around the `int(parts[...])` conversions and skip the line, exactly as `measure_phoneme_durations.read_timed_phonemes` already does for the same two label formats.

**Fix direction.** Wrap the `int(parts[0])`/`int(parts[1])` conversion (and the analogous one in `prepare_jsut_song.read_label`) in `try: ... except ValueError: continue`, mirroring `measure_phoneme_durations.read_timed_phonemes`, and count/report skipped malformed lines the way `skipped`/`unknown` are already tracked in `main()`.

### F-220 · medium · host.py's own MODEL_SILENCE = "<sil>" literal is a third, untested copy alongside ipa.SIL and Rust's score.rs constant.

`training/src/auris_singer/host.py:156` · test-quality · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Nothing breaks today because all three copies happen to agree, but if a future rename or dictionary change updates the Rust MODEL_SILENCE or ipa.SIL without updating host.py's own literal, frames_from_curves would silently stop folding the model's silence token into the frames' SILENCE slot, and nothing in CI would catch it since the contract suite never reads host.MODEL_SILENCE.

**Trigger.** A future change renames the model's silence spelling on either the trainer side (`text/ipa.py`'s `SIL`) or the Rust side (`score.rs`'s `MODEL_SILENCE`) without also updating the separate literal in `host.py` line 156 — the existing contract test would still pass because it never reads `host.MODEL_SILENCE`.

**Mechanism.** `host.py` defines `MODEL_SILENCE = "<sil>"` (line 156) as its own literal, used by `frames_from_curves` (line 200: `token = SILENCE if symbol == MODEL_SILENCE else symbol`) to translate the model's silence spelling into the frames' own `SILENCE` ("sil") token when building evaluation frames from a corpus. This is a third independent copy of the same string, alongside `auris_singer.text.ipa.SIL` (`"<sil>"`) and the Rust `MODEL_SILENCE` constant in `crates/auris-singer/src/score.rs:21`. `test_host_contract.py` only checks `SIL == rust_str_const(SCORE_RS, "MODEL_SILENCE")` (line 224); nothing in the contract-test suite asserts `host.MODEL_SILENCE` equals either of the other two. The module's own docstring (`host.py` lines 14-18) claims the two contract values this module needs (`SILENCE`, `energy_full_scale`) "are checked against the Rust sources as text in `tests/test_host_contract.py`", which overstates what is actually verified for this constant.

**Expected.** `host.MODEL_SILENCE` should be derived from (or asserted equal to) `auris_singer.text.ipa.SIL`, and the contract test should hold it against the Rust constant the way `SIL` already is, consistent with the module docstring's claim that the constant "is checked against the Rust sources as text".

**Fix direction.** Either delete host.py's standalone MODEL_SILENCE and import SIL from auris_singer.text.ipa instead (already checked against Rust at test_host_contract.py:224), or if host.py must stay decoupled, add an assertion in test_host_contract.py that host.MODEL_SILENCE == ipa.SIL (or directly against rust_str_const(SCORE_RS, "MODEL_SILENCE")).

**Written rule it breaks.** "A failure is a decision, not an edit to whichever file the assertion named... every check is a parser, and a parser that quietly stops matching is a test that quietly stops testing" (CLAUDE.md, "The voice trainer", describing test_host_contract.py's purpose)

### F-221 · medium · verify_onnx's two test cases both build durations that sum short of f0 length, so the exact-duration-fit contract the wrapper's docstring declares is never tested.

`training/src/auris_singer/export.py:436` · test-quality · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A trainer developer who runs `verify_onnx` and sees it pass has no evidence that the exact-duration-fit path (the one every real export actually uses, since real durations are built to sum to the frame count) works correctly through onnxruntime — a regression that only breaks the all-True-mask case would ship silently, surfacing later as audible artifacts or shape mismatches in exported voices rather than being caught by this test.

**Trigger.** Any call to verify_onnx() — invoked by scripts/export_onnx.py on every real export unless --no-verify is passed, and by test_export.py::test_onnx_export_runs_and_matches_pytorch — always hits this because the (s, t) pairs are hardcoded at the two call sites and neither divides evenly.

**Mechanism.** `_verification_inputs()` builds `"durations": torch.full((batch, s), max(1, t // s), dtype=torch.long)` (line 436), which sums to `s * (t // s)` — equal to `t` only when `s` evenly divides `t`. Its two call sites never satisfy that: `_verification_inputs(model, batch=1, s=5, t=23, ...)` (line 484) sums to 5*4=20 against t=23 (short by 3), and `_verification_inputs(model, batch=2, s=9, t=64, ...)` (line 489) sums to 9*7=63 against t=64 (short by 1). `OnnxSingerWrapper.latent()` derives `y_lengths = durations.sum(dim=1)` and masks the decoder's latent `z` via `sequence_mask(y_lengths, f0.size(-1))` (lines 132-133), so both calls verify_onnx makes always exercise a latent whose trailing 1-3 frames are masked to zero — never the fully-valid `y_lengths == T` case.

**Expected.** `_verification_inputs` should build `durations` that sum exactly to `t` (e.g. by putting the remainder on the last phoneme) for at least one of its two checks, so the one input shape the module documents as the only valid one is the one actually verified before a voice ships.

**Fix direction.** In `_verification_inputs`, build durations that sum exactly to `t` (e.g. distribute `t` evenly across the first `s-1` slots and put the remainder in the last slot, or pick `s`/`t` values where `s` divides `t`) so at least one of the two `verify_onnx` calls exercises the `y_lengths == f0.size(-1)` exact-fit, all-True-mask case that `OnnxSingerWrapper`'s docstring declares as the contract.

**Written rule it breaks.** "``sum(durations)`` must equal ``f0.size(-1)`` — the wrapper does not trim the curves the way ``infer`` does, because data-dependent slicing does not belong in a traced graph." (training/src/auris_singer/export.py OnnxSingerWrapper docstring)

### F-234 · medium · collate_batch crashes on batches whose n_fft-hop_length parity is odd, because spectrogram()'s frame count silently drops below the pre-truncated f0/energy/voiced length, contradicting audio.py's own documented L//hop_length invariant.

`training/src/auris_singer/data/dataset.py:147` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Someone retraining the singer model with a sample-rate/hop/FFT combination whose (n_fft - hop_length) is odd — e.g. adapting the 44.1 kHz pipeline with hop_length=441 while leaving n_fft=2048 — has data loading crash outright with a PyTorch shape-mismatch RuntimeError on the very first batch containing an affected utterance, instead of training proceeding. All shipped presets happen to use an even n_fft-hop_length difference, so the bug is currently latent and untested.

**Trigger.** Any `audio` config where `n_fft` and `hop_length` have different parity, e.g. adapting the pipeline to 44.1 kHz with `hop_length: 441` while leaving `n_fft: 2048` (diff = 1607, odd) -- every shipped preset (`n_fft=2048/hop=480`, and the `MultiParamMelLoss` resolutions 512/120, 1024/240, 2048/480, 4096/960) happens to keep this difference even, so the bug is latent and untested (`conftest.py`'s `audio_config` also uses the even 2048/480 pair).

**Mechanism.** `SingingDataset.__getitem__` computes `n_frames = min(wav.numel() // self.hop_length, f0.numel())` (line 95), truncates the waveform to exactly `n_frames * hop_length` samples (line 96), and truncates `f0`/`energy`/`voiced` to `n_frames` (lines 103-105), then calls `spectrogram(wav, n_fft, hop_length, win_length)` (line 97). `spectrogram()` (auris_singer/utils/audio.py) pads the waveform by `pad = (n_fft - hop_length) // 2` on each side (integer division) and runs `torch.stft(..., center=False)`, whose frame count is `floor((L + 2*pad - n_fft) / hop_length) + 1`. When `n_fft - hop_length` is even, `2*pad == n_fft - hop_length` exactly and this reduces to `n_frames`, so `item["spec"].size(-1) == n_frames`. When `n_fft - hop_length` is odd, `2*pad` is one less than `n_fft - hop_length` (floor division drops the remainder), and the resulting spectrogram has `n_frames - 1` frames -- one fewer than the already-truncated `f0`/`energy`/`voiced`. `collate_batch` then recomputes `n_frames = int(item["spec"].size(-1))` from the (now shorter) spectrogram (line 147) and assigns `f0[i, […]

**Expected.** `SingingDataset.__getitem__` should derive `n_frames` from the spectrogram it actually produced (or the frame helpers should guarantee `spectrogram()`'s frame count equals `wav.numel() // hop_length` for any `n_fft`/`hop_length` pair), so `f0`/`energy`/`voiced`/`spec` always agree in length regardless of the parity of `n_fft - hop_length`.

**Fix direction.** In SingingDataset.__getitem__, derive n_frames from the spectrogram actually produced (e.g. n_frames = min(spec.size(-1), f0.numel()) after computing spec, then truncate wav/f0/energy/voiced to that n_frames) rather than assuming spectrogram()'s frame count always equals wav.numel() // hop_length; alternatively fix _pad_for_stft to pad symmetrically regardless of the parity of n_fft - hop_length so the documented L // hop_length invariant actually holds.

**Written rule it breaks.** audio.py's own module docstring: "the waveform is reflection-padded by (n_fft - hop_length) // 2 on both sides and analysed with center=False, so a waveform of L samples yields exactly L // hop_length frames." — this is false when n_fft - hop_length is odd, which is exactly the case collate_batch's line 147/153 mismatch depends on.

### F-337 · medium · DistributedBucketSampler silently drops any utterance outside bucket_boundaries with no log or error unless every bucket ends up empty.

`training/src/auris_singer/data/dataset.py:233` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If someone editing a training config widens data.max_frames (or narrows data.min_frames) without also widening data.bucket_boundaries to match, SingingDataset still keeps the longer/shorter utterances, but DistributedBucketSampler._create_buckets computes bucket = -1 for them and simply never appends their index anywhere -- no log line, no counter, no warning. Training silently runs on a shrunken dataset; the only thing that would ever raise is if every single utterance fell outside the boundaries (RuntimeError when all buckets are empty), which a partial mismatch never triggers. This only bites when a config's bucket_boundaries and min_frames/max_frames are edited out of sync -- the two shipped configs pass test_config.py's max_frames<=bucket_boundaries[-1] check (and boundaries[0]=0 already covers the default min_frames=32), so the default training path is unaffected.

**Trigger.** A user edits `data.max_frames` (or `data.min_frames`) in a custom training config without also widening `data.bucket_boundaries` to match — e.g. raising `max_frames` from 1200 to 2000 to admit longer songs but leaving the shipped `bucket_boundaries` ending at 1200. `SingingDataset.__init__` still keeps every record with `min_frames <= n_frames <= max_frames` (dataset.py:62-64), so the longer utterances are in `self.records`/`self.lengths`, but every one of them gets `_bucket_of(length) == -1` and never appears in any batch for the entire run.

**Mechanism.** `_bucket_of` (lines 222-226) returns -1 for any length that falls outside every `(boundaries[i], boundaries[i+1]]` interval; `_create_buckets` (lines 228-248) only appends an index when `bucket >= 0` (line 233), so a -1 index is dropped with no log, counter, or warning of any kind. The only failure mode that surfaces is `_create_buckets` raising when *every* bucket ends up empty (line 238-242) — a partial loss is invisible.

**Expected.** Either clamp/raise on construction when an item falls outside the configured bucket range, or log a count of dropped utterances so a boundary/max_frames mismatch is visible instead of silently discarding data.

**Fix direction.** In DistributedBucketSampler._create_buckets, count and log (or raise) the number of utterances for which _bucket_of returns -1 instead of silently discarding them, and/or add a constructor-time assertion that every dataset length lies within (boundaries[0], boundaries[-1]] -- mirroring test_config.py's existing max_frames<=bucket_boundaries[-1] check but enforced at runtime for any config, and covering the missing min_frames>=boundaries[0] side too.

### F-384 · medium · Under trainer.strategy=ddp, val_dataloader() (datamodule.py:119-126) stays unsharded because train.py:103 sets use_distributed_sampler=False Trainer-wide, so every rank redundantly validates the full set.

`training/src/auris_singer/data/datamodule.py:119` · concurrency · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running the documented multi-GPU DDP training invocation (trainer.strategy=ddp trainer.devices=4, per doc/training.md) causes every rank to run the entire validation loop redundantly instead of splitting it, so validation takes roughly N times longer than intended on an N-GPU job. No output is wrong and single-GPU/CPU training is unaffected, but training wall-clock and cost scale up silently with no error or warning.

**Trigger.** `training/doc/training.md`'s own "Multi-GPU" section instructs `trainer.devices=4 trainer.strategy=ddp`; running that command as documented puts every one of the 4 ranks through the identical, full, unsharded validation set every epoch.

**Mechanism.** `val_dataloader()` returns a plain `DataLoader` with no `DistributedSampler`. `train.py` hardcodes `use_distributed_sampler=False` on the `Trainer` (line 103, unconditionally, not just under DDP) specifically so Lightning won't override `train_dataloader`'s own `DistributedBucketSampler` — but that same flag also stops Lightning from auto-wrapping `val_dataloader`, which has no sharding of its own. `_log_audio` (lightning_module.py:497-520) then calls `self.logger.experiment.add_audio(...)` directly for `batch_idx < log_audio_batches`, bypassing the `@rank_zero_only` guard that protects `self.log(...)`/`log_metrics`.

**Expected.** Wrap `val_dataloader`'s dataset in a `DistributedSampler` (or otherwise shard it) to match the train loader, and guard `_log_audio`'s direct `experiment.*` calls with `self.trainer.is_global_zero`, consistent with the doc's claim that the sampler "shards across replicas by rank" under `trainer.strategy=ddp`.

**Fix direction.** Shard the validation set explicitly: wrap self.val_dataset in a DistributedSampler (or a non-shuffling bucket sampler variant) inside val_dataloader() in training/src/auris_singer/data/datamodule.py, rather than relying on Trainer-wide use_distributed_sampler which train.py:103 disables for all dataloaders including validation.

### F-405 · medium · resolve_speaker() in training/src/auris_singer/infer.py:80 silently accepts a JSON bool as speaker id 0/1 because bool is a subclass of int, bypassing speaker_to_id validation.

`training/src/auris_singer/infer.py:80` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a score.json's "speaker" field is accidentally a JSON boolean (true/false) instead of a string or int index, `resolve_speaker` silently treats it as speaker id 1 or 0 rather than raising the KeyError that any other invalid speaker value would trigger. The synthesized audio comes out in the wrong speaker's voice with no error message, so the mistake is discovered only by listening to the output, if at all.

**Trigger.** `scripts/infer.py` reads `speaker=payload.get("speaker")` straight out of a user- or tool-supplied `score.json` (the documented JSON control-file format). A JSON value of `"speaker": true` (a plausible mistake from JS-style tooling using boolean sentinels for 'unset') deserializes to Python `True`, which `resolve_speaker` maps to raw id `1` with no validation.

**Mechanism.** `if isinstance(speaker, int): return speaker` (line 80) treats `speaker` as an already-resolved integer id and returns it unchanged, skipping the `speaker_to_id` membership check the `str` branch below it performs. Because Python's `bool` is a subclass of `int`, `isinstance(True, int)` and `isinstance(False, int)` are both `True`, so a boolean value is silently accepted as if it were a validated integer speaker id (`True` -> 1, `False` -> 0) instead of falling into the 'unknown speaker' `KeyError` path a bad non-int/non-str value should hit.

**Expected.** A `bool` should not be treated as a valid raw speaker id; `resolve_speaker` should reject it (or route it through the same name-lookup/validation path as any other non-`int` value) so a malformed `speaker` field fails with the same clear error a bad string gets.

**Fix direction.** Check `isinstance(speaker, bool)` before the `isinstance(speaker, int)` branch (or use `type(speaker) is int`) and either raise a clear TypeError/KeyError or fall through to the string-lookup branch so an invalid speaker value fails loudly instead of resolving to id 0/1.

### F-413 · medium · host_eval's Analyst always measures pitch with the FCPE default f0_min=40.0, ignoring the exported voice's own f0_min, silently skewing f0 metrics for voices trained with a non-default pitch floor.

`training/src/auris_singer/host_eval.py:143` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer training a voice with a non-default pitch floor (e.g. `model.generator.f0_min: 90.0` in the YAML config) exports it correctly — the metadata JSON/onnx sidecar faithfully records f0_min=90.0 — but running `scripts/evaluate_host.py` (or `--score`) against that voice silently re-analyses the rendered audio with FCPE searching [40, 1600] Hz instead of [90, 1600] Hz. The reported f0_rmse_cent, f0_accuracy, f0_corr, vuv_error and voiced_ratio_error numbers are computed with the wrong search range and no error or warning is raised, so a before/after regression comparison over such a voice is comparing numbers computed inconsistently with what the voice actually is.

**Trigger.** Train a voice with `model: {generator: {f0_min: 90.0}}` in the YAML config (a real, wired-up per-module override, not a hypothetical field) and export it — the true 90.0 is correctly written into the .onnx/.json metadata — then run `scripts/evaluate_host.py --voice ... --checkpoint ... --data ...` (or `--score`).

**Mechanism.** `export.metadata_block()` (training/src/auris_singer/export.py:287) writes `model.generator.source_generator.f0_min` into every exported voice's metadata, and `AurisSinger`'s `generator` constructor argument is documented as a per-module keyword override (training/src/auris_singer/model.py:69, forwarded to `SourceSignalGenerator` via `training/src/auris_singer/modules/generator.py:116`), so a training config can legitimately set a non-default `f0_min`. `VoiceInfo` (host_eval.py:124-132) and `voice_info()` (host_eval.py:143-163) read only `sample_rate`, `hop_length`, `symbols` and `name` out of that same JSON block and drop `f0_min` entirely. `evaluate()`'s `Analyst(...)` call (host_eval.py:588-597) and `evaluate_score()`'s (host_eval.py:806-810) never pass `f0_min`/`f0_max`, so `Analyst.__init__`'s hardcoded defaults (40.0/1600.0, host_eval.py:342-343) are used for every voice regardless of what it actually reports.

**Expected.** `VoiceInfo` should carry the exported `f0_min` (and `f0_max` where relevant), and `Analyst` should be constructed with the voice's own value rather than the FCPE extractor's generic default.

**Fix direction.** Add `f0_min: float` to `VoiceInfo` (read from `block["f0_min"]` in `voice_info()`, with a default matching `SourceSignalGenerator`'s 40.0 for older exports lacking the key), and pass `voice.f0_min` into both `Analyst(...)` call sites (host_eval.py:588 and 806) instead of relying on the constructor default.

**Written rule it breaks.** Composed audio is calibrated by measurement — render and measure before touching a level or timing constant (project memory); training/doc/evaluation.md's before/after regression comparisons assume the Analyst's pitch measurement reflects the voice under test.

**Verifier's correction.** Minor line-number correction only: `f0_min` is written into the metadata dict within `export.metadata_block()`'s dict literal around export.py:287 (confirmed at that location), and `Analyst.__init__`'s defaults are at host_eval.py:329/342-343 (f0_min default declared at 342, f0_max at 343) rather than exactly "342-343" as stated for the whole thing — negligible, the claim's substance is accurate. `Analyst`'s pitch-analysis path is only active when `pitch=True` (the default in Settings), which does not weaken the claim since that is the normal case for pitch metrics.

### F-414 · medium · evaluate_score() hardcodes n_fft/win_length=2048 in its Analyst instead of reading the voice's own exported audio config, unlike evaluate()'s corpus-based path.

`training/src/auris_singer/host_eval.py:807` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Running `evaluate_host.py --score` against a voice trained with a non-default STFT window (n_fft/win_length other than 2048) silently computes the score-column's energy and mel metrics (energy_rmse_db, energy_bias_db, energy_corr, mel_l1) using the wrong analysis window, so those numbers can mask a real regression or manufacture a fake one — with no error, warning, or failing test, since every shipped config and test fixture happens to use 2048/2048.

**Trigger.** Train with `audio: {n_fft: 1024, win_length: 1024, ...}` in the YAML config (`n_fft`/`win_length` are first-class, directly documented training-config keys — every shipped config just happens to use 2048/2048 today) and export, then run `scripts/evaluate_host.py --voice ... --score`.

**Mechanism.** `evaluate()`'s `Analyst` (host_eval.py:588-597) correctly sources `n_fft`/`win_length` from `Corpus`, which reads them from the dataset's own `audio_config.json` (host_eval.py:214-218). `evaluate_score()` has no `Corpus` (only the exported voice), so it hardcodes `Analyst(info.sample_rate, 2048, info.hop_length, 2048, ...)` (host_eval.py:807) instead — even though the true values are already embedded in the export: `scripts/train.py` merges `"audio": datamodule.audio_config` (train.py:50) into the checkpoint's `metadata`, and `metadata_block()`'s `**(metadata or {})` merge (export.py:288) carries that whole `audio` sub-object into the exported JSON, but `VoiceInfo`/`voice_info()` never read it.

**Expected.** `evaluate_score()` should use the voice's own `n_fft`/`win_length` (already present in the exported metadata's `audio` block), the way `evaluate()` uses the corpus's.

**Fix direction.** Have `VoiceInfo`/`voice_info()` also read the `audio` sub-block already embedded by `export.py`'s `metadata_block()` (n_fft, win_length, defaulting to sample_rate/hop_length-consistent values if absent for old exports), and change `evaluate_score()`'s `Analyst(...)` construction at host_eval.py:807 to use `info.n_fft`/`info.win_length` instead of the literal `2048, ... 2048`, mirroring how `evaluate()` sources them from `Corpus`.

**Written rule it breaks.** Composed audio is calibrated by measurement — render and measure before touching a level or timing constant (project convention: the evaluation tooling's numbers must be trustworthy, not silently computed on the wrong window)

### F-194 · low · SingingDataset's unused hop_length/n_fft/win_length override params can silently desync f0/energy from the spectrogram, but no shipped caller ever passes them.

`training/src/auris_singer/data/dataset.py:95` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No current training run is affected — SingingDataModule (the only production caller) never forwards n_fft/hop_length/win_length, and no config or test exercises the override path. A future caller that does pass a hop_length differing from the corpus's audio_config.json would silently get spec/wav truncated to a shorter real-world duration than the accompanying f0/energy/voiced curves, with no error or warning, corrupting that training run's pitch/energy alignment.

**Trigger.** Construct `SingingDataset(root, hop_length=240)` against a corpus preprocessed at hop_length=480 (verified: for a 1.0 s utterance, the default dataset returns 100 spec frames spanning the full 1.0 s, matched to 100 f0 frames also spanning 1.0 s; with `hop_length=240` it still returns 100 spec/f0 frames each, but the 100 spec frames now come from only the *first 0.5 s* of audio (`wav = wav[: n_frames * self.hop_length]` truncates to 24000 of the original 48000 samples) while the 100 f0 values still span the original full 1.0 s).

**Mechanism.** `SingingDataset.__init__` (lines 43-58) accepts `n_fft`/`hop_length`/`win_length` overrides that default to `audio_config[...]` but are otherwise unvalidated against it: `self.hop_length = int(hop_length if hop_length is not None else audio_config["hop_length"])`. `__getitem__` then does `n_frames = min(wav.numel() // self.hop_length, f0.numel())` (line 95) and recomputes `spec = spectrogram(wav, self.n_fft, self.hop_length, self.win_length)` (line 97) at the *overridden* hop rate, while `f0`/`energy`/`voiced` are only ever sliced (`f0[:n_frames]` etc., lines 103-105) — they are never resampled to the new hop rate. `f0.numel()` still reflects the *preprocessing* hop rate baked into the stored `.npz`, so when `self.hop_length` differs from that rate, `n_frames` frames of `spec`/`wav` cover a different real-world duration than the first `n_frames` values of `f0`.

**Expected.** Either reject a `hop_length`/`n_fft`/`win_length` override that disagrees with `audio_config.json` (the file the preprocessor stamped these values into), or resample/re-derive `f0`/`energy`/`voiced` at the same rate the spectrogram is recomputed at, so 'frame i' means the same instant in both. As shipped, neither happens and the mismatch is silent.

**Fix direction.** In SingingDataset.__init__, either reject hop_length/n_fft/win_length overrides that disagree with audio_config.json (raise, since the .npz's f0/energy/voiced were computed at the preprocessing rate and cannot be reconciled cheaply), or resample f0/energy/voiced in __getitem__ to the overridden frame rate; at minimum document the constraint in the docstring.

**Verifier's correction.** SingingDataset's public constructor (training/src/auris_singer/data/dataset.py:43-58) accepts n_fft/hop_length/win_length overrides that are never validated against audio_config.json, and __getitem__ (line 95 and 103-105) recomputes the spectrogram at the overridden hop rate while slicing f0/energy/voiced unchanged from their preprocessing-rate values — so a caller that overrides hop_length to a value that disagrees with the corpus's audio_config.json silently gets a spectrogram/waveform and a pitch/energy/voicing curve that cover different real-world durations of audio, with no error, shape […]

### F-235 · low · Unsanitized dataset.sources[].name in the local preprocessing config lets output_dir/f"{utt_id}.npz" traverse outside output_dir.

`training/src/auris_singer/preprocess/pipeline.py:88` · security · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a training config's dataset.sources[].name (or a *_suffix value) contains ".." or path separators, run_preprocess silently mkdir's and writes .npz feature files outside output_dir instead of failing loudly, and the metadata.jsonl record's relative_to(output_dir) check does not catch the escape because it compares literal segments, not resolved ones. This only fires from a config the local operator themselves authored and ran, the same trust boundary already crossed by the arbitrary-code prepare_*.py recipe scripts.

**Trigger.** A preprocessing YAML config whose `dataset.sources[].name` contains `/` or `..` segments. This project's own distribution model (`doc/datasets.md`: 'recipes can be published even where checkpoints cannot') and its recipe scripts (`prepare_jsut_song.py`, `prepare_vocalset.py`, `prepare_namine_ritsu.py`) are explicitly meant to be shared/adapted by other users, so a config is not guaranteed to be self-authored trusted input the way, say, a Rust project's own source is.

**Mechanism.** `speaker = str(source["name"])` (line 69, from `dataset.sources[].name` in the YAML config) is embedded unsanitized into `utt_id=f"{speaker}/{relative.as_posix()}"` (line 88), which is later joined as `out_path = output_dir / f"{utt.utt_id}.npz"` with `out_path.parent.mkdir(parents=True, exist_ok=True)` (lines 252-253). `Path.__truediv__` does not strip `..` components, so a `name` containing path separators or `..` walks the resulting path outside `output_dir`. I verified this concretely: with `output_dir = Path('data/processed')` and `speaker = '../../../../tmp/pwned'`, `(output_dir / f'{speaker}/utt1.npz').resolve()` resolves to `C:\Users\<user>\tmp\pwned\utt1.npz`, well outside the intended output tree, and `mkdir(parents=True)` would happily create that directory chain before `np.savez` writes into it.

**Expected.** `collect_utterances`/`run_preprocess` should reject or sanitize a `source["name"]` (and the `*_suffix` fields) containing path separators or `..` components before using it to build an output path, keeping every write inside `output_dir` the way the project's own project-folder containment rule requires elsewhere in the codebase.

**Fix direction.** In collect_utterances (pipeline.py:69-91), validate that source["name"] and the *_suffix values contain no path separators or ".." segments before building utt_id, and/or assert out_path.resolve() is still relative to output_dir.resolve() before the mkdir/savez at line ~252-253.

### F-260 · low · training/pyproject.toml:43 pins the optional `asr` extra's ReazonSpeech dep to a git branch tip, not a commit, so installs are unreproducible and unverifiable.

`training/pyproject.toml:43` · security · confirmed (traced through the code; reported independently 2×)

**What a user sees.** A developer who runs the documented `uv pip install -e '.[dev,export,asr]' --torch-backend=auto` (only needed to run `scripts/evaluate_host.py --asr`) gets whatever commit is currently HEAD of the external `reazon-research/ReazonSpeech` GitHub repo at install time, with no way to pin, reproduce, or hash-verify it — a compromised or force-pushed upstream branch would be installed silently, and two installs on different days can silently diverge.

**Trigger.** A developer follows `training/doc/evaluation.md`'s own instructions and installs the `asr` extra on two different days (or two different machines) — each install can silently pick up a different commit of ReazonSpeech, including one the developer never reviewed, and code it runs comes from a repository this project does not control.

**Mechanism.** `reazonspeech-k2-asr @ git+https://github.com/reazon-research/ReazonSpeech#subdirectory=pkg/k2-asr` names a git repository with no `@<rev>` or tag pin, so `uv pip install` resolves to whatever commit is at the tip of the default branch at install time. `training/doc/evaluation.md:87` tells a developer to run `uv pip install -e '.[dev,export,asr]' --torch-backend=auto` to get it. `training/.gitignore:18` deliberately excludes `uv.lock` from version control (confirmed: `git ls-files training/uv.lock` finds nothing tracked, even though a locally-generated `training/uv.lock` sits on disk), so there is no committed lock file to pin this reference either — CLAUDE.md's own account of that decision ("There is no `uv.lock`... it would hand one machine's answer to every other") covers only the PyTorch backend index, not this floating git ref.

**Expected.** A git dependency intended for reuse should be pinned to a specific commit or tag (`@<sha>` or `rev=`), the way every other dependency in this workspace is pinned by version or by Cargo.lock's checksum.

**Fix direction.** Pin the git dependency to an immutable ref, e.g. `reazonspeech-k2-asr @ git+https://github.com/reazon-research/ReazonSpeech@<commit-sha>#subdirectory=pkg/k2-asr`, and note in a comment why that SHA was chosen (and how to bump it). This is unrelated to the deliberate `uv.lock` exclusion (which is about the PyTorch CUDA/CPU index, per training/.gitignore's own comment) and doesn't require adding a lock file back.

### F-271 · low · get_frontend's docstring promises kwargs are forwarded to the front-end constructor, but the ipa/none/raw branch silently drops them instead.

`training/src/auris_singer/text/__init__.py:67` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A trainer config (or the ASR front-end selection) that sets `language: ipa`/`none`/`raw` together with front-end options like `add_boundary_silence` silently loses those options — the option is parsed, believed to take effect, and simply discarded with no warning or error, unlike the `ja` branch where an unrecognised kwarg raises `TypeError` immediately.

**Trigger.** Calling `get_frontend("ipa", **opts)` with a non-empty `opts` — e.g. `auris_singer.preprocess.pipeline.run_preprocess` does exactly this via `get_frontend(str(config.text.language), **dict(config.text.get("options", {})))` whenever a preprocessing config sets `text.language: ipa` together with a non-empty `text.options` block.

**Mechanism.** `get_frontend`'s docstring states "**kwargs: forwarded to the front-end constructor" (line 61), and the `"ja"/"jp"/"japanese"` branch does forward them (`JapaneseFrontend(**kwargs)`, line 65). The `"ipa"/"none"/"raw"` branch, however, calls `IpaFrontend()` with no arguments at all (line 67), so any kwargs passed by a caller are silently discarded rather than forwarded or rejected.

**Expected.** Either forward kwargs to `IpaFrontend()` as the docstring promises (which would surface a clear `TypeError` for an unsupported option), or document that `text.options` only applies to the `ja` front-end.

**Fix direction.** Either make `IpaFrontend.__init__` accept and validate the same kwargs (at minimum raise on unexpected ones, matching the `ja` branch's behavior), or narrow the docstring/signature so `get_frontend` documents that kwargs only apply to the `ja` frontend and raises if kwargs are given for `ipa`/`none`/`raw`.

**Written rule it breaks.** **kwargs: forwarded to the front-end constructor.

### F-366 · low · AurisSinger.infer() silently truncates to min(durations.sum, f0 length) instead of raising, contradicting its own docstring contract, though no current caller can trigger it.

`training/src/auris_singer/model.py:288` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No user-facing path currently reaches this: the only host-facing caller, Synthesizer.synthesize() in infer.py, already validates that f0/energy length equals durations.sum(1).max() and raises ValueError before calling AurisSinger.infer(), and every other in-repo caller derives durations from the model's own alignment output so the lengths are guaranteed equal by construction. A future direct caller of the low-level AurisSinger.infer() that passes a too-short f0/energy would get back a waveform silently shorter than requested, with no exception or warning, contradicting the method's own documented contract.

**Trigger.** Any caller (in-repo or external) that passes `f0`/`energy` shorter than `durations.sum(1).max()` — e.g. a caller that mis-sized its curves, or that mixes durations from one source with f0/energy padded to a different, shorter length.

**Mechanism.** The docstring at line 270 states the contract as `f0, energy: (B, T) with T == durations.sum(1).max()`, but line 288 computes `y_max = int(min(y_lengths.max().item(), f0.size(-1)))` — silently taking the shorter of the two instead of asserting they match. Every downstream tensor (`y_mask`, `attn`, `x_frame`, the sliced `f0`/`energy`/`voiced`) is then built at the truncated `y_max`, and the returned waveform is correspondingly shorter than the durations the caller asked for.

**Expected.** Raise (e.g. a `ValueError` naming the two lengths) when `f0.size(-1) < durations.sum(1).max()`, matching the contract stated in the docstring, rather than silently truncating.

**Fix direction.** Add the same guard infer.py's Synthesizer.synthesize() already uses: after computing y_lengths, assert or raise ValueError if f0.size(-1) (and energy.size(-1)) is less than y_lengths.max(), instead of silently taking min() at model.py:288. This is a small defense-in-depth change confined to one function.

**Written rule it breaks.** f0, energy: (B, T) with T == durations.sum(1).max() (model.py:270 docstring)

**Verifier's correction.** `AurisSinger.infer()` in training/src/auris_singer/model.py (line 288) silently truncates to `min(durations.sum(1).max(), f0.size(-1))` instead of raising when `f0`/`energy` are shorter than the documented contract (`T == durations.sum(1).max()`, docstring at line 270) requires — verified by reading and by execution. However, no reachable in-repo caller can actually trigger the mismatch: the host-facing `Synthesizer.synthesize()` in infer.py raises `ValueError` on any length mismatch before calling `model.infer()`, and every other direct caller (`lightning_module.py::validation_step`, […]

### F-423 · low · kl_scale() reads global_step after opt_d.step() has already advanced it, shifting the KL warm-up ramp by one optimizer step.

`training/src/auris_singer/lightning_module.py:292` · theory · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Training still converges normally; the only effect is that the KL warm-up ramp used in the generator loss is shifted forward by exactly one optimizer step (out of kl_warmup_steps, which itself already counts both the generator's and discriminator's steps per batch per the shipped configs), so the KL term reaches full weight one step earlier than a purely batch-indexed schedule would imply. No crash, no NaN, no wrong gradients — just a marginally different (and undocumented) warm-up curve.

**Trigger.** Every training step from step 0 onward.

**Mechanism.** `training_step` calls `opt_d.step()` at line 262 before `kl_scale = self.kl_scale()` at line 292. Under Lightning's manual optimization, `global_step` is Lightning's own `optim_step_progress.total.completed` counter, incremented by exactly one on every `LightningOptimizer.step()` call (confirmed by reading `.venv/Lib/site-packages/pytorch_lightning/loops/optimization/manual.py`'s `_on_after_step`), so by the time `kl_scale()` runs, `self.global_step` has already been bumped by the discriminator step that just happened in the same call.

**Expected.** Read `self.global_step` for the ramp before `opt_d.step()` runs (or use a step counter incremented once per batch) so the KL weight at a given batch does not depend on discriminator-step bookkeeping that happened moments earlier in the same call.

**Fix direction.** Capture kl_scale = self.kl_scale() before opt_d.step() runs (e.g. right after the forward pass, alongside the other loss computations), or switch to a counter incremented once per training_step call rather than relying on Lightning's per-optimizer-step global_step.

### F-435 · low · --speaker's docstring/help claim an unconditional "model's first" default, but corpus mode defaults per-utterance to the corpus's own recorded speaker instead.

`training/src/auris_singer/host_eval.py:487` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user running evaluate_host.py in corpus mode reads the docstring or --help text for --speaker, which says leaving it unset picks "the model's first" speaker; in fact, in corpus mode each utterance's host/reference columns are rendered with that utterance's own recorded corpus speaker (record.get("speaker")) when --speaker is left unset. The unconditional-default claim only holds in score mode. This can mislead someone debugging why corpus-mode eval output varies by speaker across utterances despite not passing --speaker.

**Trigger.** Run `evaluate_host.py --voice <multi-speaker.onnx> --checkpoint <ckpt> --data <a multi-speaker dataset>` with no `--speaker`, where the validation split includes an utterance whose recorded speaker isn't speaker id 0.

**Mechanism.** `Settings.speaker`'s docstring reads '#: Which of the voice's speakers sings, by name; ``None`` is the model's first.' (host_eval.py:487), mirrored verbatim in the CLI help at training/scripts/evaluate_host.py:68 ('(default: its first)'). But in corpus mode, host_eval.py:630 resolves it as `speaker = settings.speaker or record.get("speaker")`, with the adjoining comment (line 628-629) 'Each utterance is sung by its own speaker — the corpus says whose it is — unless the run asks for one speaker throughout.' So leaving `--speaker` unset does not default to the model's first speaker for the host/reference columns; it defaults to each validation utterance's own recorded speaker from metadata.jsonl.

**Expected.** The docstring and help text should say the corpus-mode default is each utterance's own recorded speaker, with 'the model's first' reserved for contexts that have no such per-utterance metadata (score mode) or the already-reported song-mode behavior.

**Fix direction.** Reword the Settings.speaker docstring (host_eval.py:487) and the --speaker CLI help (evaluate_host.py:68) to state the mode-dependent behavior: in score mode None means the model's first speaker; in corpus mode None means each utterance is sung by its own corpus-recorded speaker unless overridden.
