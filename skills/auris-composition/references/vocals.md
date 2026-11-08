# Songs with lyrics and synthesized vocals

## Musical decisions

Develop a concrete scene and a change in perspective across verses. Give the chorus a concise central thought and a recognizable verbal and melodic hook. Establish a short identity-bearing phrase early, then develop the requested form before repeatedly polishing one passage. For a full song, include a second verse and later chorus/ending; respect requests for a one-chorus demo, excerpt, lyric-only work, duration limit, or local edit. A bridge is optional. Give later verses new words that advance the story, with deliberate phrasing or backing changes. Repeating the first verse's clip does not create a second verse with different lyrics.

Compose words and melody together. Place important syllables or Japanese mora on meaningful beats and sustained notes, preserve word boundaries, and leave breaths and singable phrase lengths. Do not fill spare notes with filler syllables or force all words onto an identical grid. Check the selected singer's useful range, leaps, dense consonants, long vowels, readings, and actual note assignments. Passing a fit diagnostic does not establish natural pronunciation or expression.

Keep the singer intelligible through accompaniment register, voicing, timing, and space. Prepare section transitions and a real ending unless a loop is requested. Preserve existing words and melody during a local repair. Make targeted corrections supported by evidence; stop after at most two local revision attempts per issue unless more exploration is requested, and report unresolved issues.

When repairing intelligibility, keep word groups, mora assignments and rhythmic identity together. Uniformly widening every gap or slowing every syllable can remove the intended groove. Compare a local reading, consonant, note-length or phrase-boundary edit with the current take before changing the whole line. Preserve the user's chosen grid and held endings; a sustained particle can be an explicit preference for one lyric, not a rule for Japanese singing. Treat a user-edited project as the new baseline and protect its melody during backing-only changes.

## Vowel, register and sustained words

When authoring a new lyric and melody with an exposed high or powerful arrival,
try a meaningful word sustained on あ /a/ or お /o/ as preferred candidates.
Identify the vowel actually held in the word, its pitch and duration, and the
consonant entering it; a kana label alone is not a complete vocal gesture.
Do not sacrifice the lyric's meaning or natural word boundary for a vowel match.

For ん, prefer a less exposed lower/middle placement or a brief phrase ending
when that preserves the melodic contour and reading. Keep its mora slot and
test the following consonant and preceding vowel. Do not assume every ん is
quiet, forbid a nasal sustain, move every ん downward, or erase it to lengthen
the preceding vowel. Fixed lyrics and a protected melody take precedence;
try local timing, articulation or an allowed transposition before proposing a
lyric change, and identify any remaining conflict.

These are listener-informed authoring preferences, not a universal ranking of
vowels. Voice, register, intensity and style affect vowel modification:
[Titze's preliminary classical/belt study](https://vocology.utah.edu/_resources/documents/formant_frequency_shifts_titze.pdf)
describes style-dependent changes, not a rule for Japanese lyric placement.
[TUFS's mora-nasal lesson](https://www.coelang.tufs.ac.jp/ja/en/pmod/practical/02-03-01.php)
explains that ん has its own mora and varies with the following sound; it does
not prescribe a low pitch. Human singing findings are not validation of a
synthesis model. Render the actual selected voice, compare the exposed word
alone and in its phrase/mix, and keep pronunciation, effort and musical
preference distinct from note-range diagnostics.

## Voice prerequisites

Discover an installed voice before promising sung audio. MCP provides `list_voices`; with CLI, use a supplied voice path or inspect the configured Auris Voices directories. `auris help` is authoritative for the installed CLI; do not invent an `auris list_voices` command. Voice choices include a DiffSinger `dsconfig.yaml`, a `*.leapsinger.json` manifest, or a `*.voicevox.json` connection. Keep the model's supporting files together. Record the selected voice, speaker, and synthesis seed where supported. A seed does not guarantee identical audio from every backend; retain the actual generated take.

A VOICEVOX connection is metadata, not a voicebank. Its configured Engine must be running. Discover singing styles from that Engine's `/singers`: the query style must support `sing` or `singing_teacher`, and the decode style must support `frame_decode`. Do not guess IDs or use a talk-only style. Standard 93.75 fps connections use 24000 or 48000 Hz, not 44100 Hz; this is separate from the project output rate. Do not silently download models, replace binaries, or switch singers when a backend is unavailable. Preserve the score and identify the missing prerequisite.

A single short health-check timeout does not prove that the Engine is unavailable. If the configured process is running, retry the read-only version/style discovery once with a longer bounded timeout (for example, 15 seconds). If it still fails, report the endpoint/error; do not automatically restart the Engine or rewrite its connection. Keep this separate from a synthesis request, which may take longer and must not be duplicated while pending.

## CLI: specification to sung mix

For a new song, put lyrics in each sung section of an `.asong`. Use distinct section names for different words, and `melody_from` for intentional melodic reuse. Shared sections must match phrase counts, mora counts per phrase, and closure positions (Japanese sokuon, small `っ`); equal total counts alone are insufficient. Newlines and sentence punctuation divide lyric phrases. A minimal short demo is:

```toml
title = "Morning Song"
style = "pop-band"
key = "C major"
tempo = 108
seed = 42
chords = "@axis"
form = ["verse", "chorus"]

[section.verse]
bars = 4
lyrics = "あさの ひかりが\nまどを たたくよ"

[section.chorus]
bars = 4
lyrics = "きみの こえが\nそらへ とどくよ"
```

Top-level `singer` and `singer_speaker` can record an existing voice path and speaker in the specification. Without a selected voice, the generated score may use a preview voice; that is not evidence of finished singing. Section lyrics create the singer track beside the accompaniment. Do not model a singer as an ordinary instrument part.

Diagnose the original and reused sections together before composing. For `closure_position_mismatch`, inspect the affected phrase's actual reading and closure indices; revise new lyrics while preserving their meaning, or choose an independent melody when reuse is unnecessary. Do not remove all sokuon or impose one mora count on every song. Keep readable display lyrics beside the synthesis reading when they differ, and verify that both still express the same words.

```text
auris compose song.asong --print
auris compose song.asong -o Song.auris
auris info <actual-saved-project.auris>
auris sing <actual-saved-project.auris> --track <actual-singer-track-name> --voice <existing-voice-path> --seed 42
auris render <actual-saved-project.auris> -o Song-mix.wav
auris render <actual-saved-project.auris> --stems Song-stems
```

Use the saved path printed by `compose` (often `Song/Song.auris`) and the singer name reported by `info`. Omit `--track` only when there is exactly one singer. Add `--speaker <actual-speaker-name>` when needed. `sing` selects the voice, synthesizes a take, and saves the project in place; it is not a standalone mix export and has no `-o` option. Synthesize each requested singer before rendering. Re-synthesize after score, lyrics, or relevant performance changes; an older take cannot verify a new score. Do not run `compose` over an existing authored project to repair one phrase.

`--print` validates specification syntax/settings, not pronunciation or full vocal coverage. The CLI has no equivalent of all the MCP lyric diagnostics. Inspect the saved notes/lyrics read-only or use MCP when available, and check the rendered vocal stem through the last sung phrase. Keep any remaining uncertainty explicit. `frames`/`sing-frames` are not a substitute for this workflow: raw frames lack lyrics, so VOICEVOX cannot synthesize them.

## MCP: discover, author, synthesize, verify

Use the advertised catalog and `tool_help`; saved-project MCP and the desktop live agent differ. If `AURIS_MCP_TOOL_GROUPS` restricts the server, enable `vocals`, plus `composition` for specification generation, `manual` for note edits, and `mix` for mixing as needed. Set groups in the server process environment and restart it after a change.

1. Inspect the project and planned section ranges. Use `production_record` and `edit_harmony` for the plan when available. Discover voices with `list_voices`; select with `set_singer_voice` and inspect real speaker names/capabilities with `inspect_singer`.
2. For saved-project generation, fetch `spec_reference`, validate with `check_spec`, check lyric fit with `diagnose_song`, then `compose`. For live or targeted edits, create a `singer` track when needed, write the score with advertised clip/note tools, then use `write_lyrics` to fill existing slots. Read notes before changing them and read assigned lyrics back afterward.
3. Use `analyze_lyrics` for readings and `diagnose_song` for phrase fit. Diagnostic readings do not persist as lyric corrections. If using `repair_lyrics`, fix the target, reference/template, and required keywords; preview at most three alternatives, apply only a passing candidate at the unchanged revision, and recheck meaning and related phrases. Use the revision fields required by the actual tool schema.
4. Finish notes, lyrics, and backing in every planned vocal section, including later verses. Call `sing` for each singer using the actual schema. Wait for completion if the host returns an asynchronous job; failed, cancelled, or pending synthesis is not success.
5. Read `inspect_singer`: require `take_state` to be `current`, then inspect `section_coverage`, following `section_coverage.next_section_offset` with `section_offset` until complete. Compare with the plan and read later sections' actual notes and lyrics. Counts, labels, and a current take alone do not prove completeness; intentional instrumental sections may be empty. Check mute/routing and render the mix and requested stems. Save through the host's supported project-save operation; live edits may still be unsaved.

### Time order and bounded synthesis

Read every required note page at one revision before grouping phrases. Storage indices are edit addresses, not chronological positions. When returned, `timeline_index` is the note's rank across the complete clip; sort all collected pages by that rank. On older hosts, order by `(start_tick, original index)`. Retain each original `index` for patches. `write_lyrics` assigns in time order; its `from` remains a storage index. After writing, re-read the affected time-ordered notes and compare onset, pitch, lyric/readings and mora assignments before synthesis. Use `analyze_lyrics`, not character count, for small kana, sokuon and long vowels. Pass `fit` with each phrase's note count: a held long vowel (ー, さあ) may share the previous note, and っ is a rest after its note rather than a note. Without a dictionary (`dictionary_available` false), spell long vowels in a reading as sung with ー (りょーて), because おう and えい are otherwise sung as spelt; with one, a reading that matches the written text takes its long vowels from the dictionary. When a phrase fits, pass its `note_reading` (notes separated by `|`) as the `write_lyrics` reading. Where a hook repeats exactly, every line sung to it must fit the same count; adjust words first (drop or add a particle, change a word). Elsewhere let line lengths differ and let the melody adapt (add or drop a pickup, split a long note, merge two) rather than writing every line to one count: lines all in 7-5 meter sung one even note per mora sound like a school song (唱歌).

Before a long or late-entering vocal synthesis, inspect first/last sounding time, leading silence and expected frame count. A bar limit alone does not bound memory at slow tempos. Use the backend's advertised duration/frame budget. For a backend without bounded full-song synthesis, use isolated phrase projects with local timing and a short explicit pre-roll, synthesize sequentially, and retain the offset/source mapping when placing audio back on the original timeline. Preserve the editable score and generated takes; never silently truncate a requested song to fit a limit. Check joins, consonant lead-in and the final phrase after assembly.

Do not overlap heavy requests to the same Engine. A client timeout or stopped CLI does not prove server inference stopped or memory was released. Keep an unresolved request pending until its state is known; do not automatically retry it or terminate an Engine that may serve other work. A successful short probe does not establish a safe duration for an arbitrary full song.

## Delivery checks

After the composition is established, complete [the arrangement pass](arrangement.md) around the actual vocal line. Check backing in sung passages and gaps, including later verses. Refresh any affected singer takes before final export; arrangement review does not authorize replacing the lyrics or vocal melody.

Verify the rendered mix and vocal stem across the opening, later verses, joins, and final sung phrase. Check missing/truncated singing, vocal audibility, duration, peak/true peak, and loudness using `verify_render` or equivalent measurements. A short successful voice probe does not verify a complete song. Preserve the editable project, lyrics/specification, voice/speaker selection, seed, and saved audio take; report unsung or stale sections explicitly.

### Balance and verify the synthesized take

Automatic mixing may have balanced a preview voice whose level differs substantially from the synthesized take. Render with the actual voice, inspect vocal and backing levels over matching sung passages, and revisit the vocal fader, backing, and master headroom. Use listening when available; stem RMS/LUFS differences are supporting evidence, not a universal vocal-to-backing target. Do not reuse a fixed gain reduction from another song or normalize a dominating vocal away with only a master adjustment. Save mix changes in the project and re-render affected deliverables so the editable project and final WAV agree.

Use actual section/tempo boundaries to measure each planned sung section and the final phrase separately. At changing tempos, derive seconds from the tempo map rather than a single ticks-per-second constant. Inspect the lyric-bearing notes, phonemes, continuation markers, and intentional rests together; one initial lyric string plus continuation notes need not mean missing lyrics. Nonzero section energy only rules out total silence, not missing syllables. Follow coverage/report pagination, verify take freshness after final edits, and measure the ending tail as well as the main song. Keep these three conclusions separate: score assignments, synthesized audio coverage, and perceptual quality.

Distinguish score inspection, audio measurements and human listening. Neither waveform inspection nor a passing diagnostic proves intelligibility, emotional delivery, or musical quality. Deliver the requested audio and editable score with the completed checks and any unverified listening judgments. Use the user's later listening feedback for targeted revisions.

Keep human comments verbatim and do not expand a rhythm preference into unreported claims about breathing or memorability.
