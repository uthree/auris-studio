# Changelog

Auris Studio is at `0.x`. **Nothing is stable there** — the project file format, the
configuration files, the key binding ids and every public API may change in any release, without
a migration path. The version number is the promise, and `0` is the promise that there is none.

The release workflow reads the section whose heading matches the tag, so the headings are the
format rather than a convention: `## <version> — <date>`.

## Unreleased

### The voice trainer moved in

* **`training/` is now part of this repository.** The Python project that trains the singing
  voices — PyTorch and Lightning, its own `uv` environment, its own documentation — was a
  repository of its own (`uthree/auris-singer`) and is now a directory here, history and all. It
  is not part of the Rust workspace, `cargo` never sees it, and no release archive carries it.
* **The voice file format is checked across both languages.** An exported `.onnx` is a contract
  between the trainer that writes it and `auris-singer` that reads it, and several halves of that
  contract were written down twice — the metadata key, the format version, the reserved symbols,
  the phoneme table down to which symbols are voiceless. Nothing could compare them while the two
  lived apart, to the point where a comment in `auris-vocal` asserted that its voiceless list
  matched the trainer's symbol for symbol: true when written and unverified ever after.
  `training/tests/test_host_contract.py` is that comment executable, and CI runs it on every
  change to either side.
* **PyTorch is no longer pinned to a CUDA index.** The trainer named the CUDA 12.8 wheels
  outright, which is right on a machine with a card and wrong on every other;
  `uv pip install --torch-backend=auto` reads the driver and chooses.
* **A voice is measured through the host, not only through PyTorch.** Training's validation
  sings in PyTorch and export's verification compares one runtime against another on random
  inputs; neither sees what `auris-singer` does with the file — the chunking of a long
  timeline, the arrangement of frames into tokens, the energy scale, its own noise, its own
  copy of the runtime — or what `auris-vocal` decided before it. `training/scripts/evaluate_host.py`
  sings the validation set's own curves through the application and reports the trainer's
  own metrics beside a PyTorch reference and beside the same utterances sung as one song, so
  a seam or an arrangement bug shows as a number; `--score` composes from lyrics and sings
  through `auris sing`, the path a person walks. Two CLI commands are the doors: `auris
  frames` writes what a singer track will be sung as, and `auris sing-frames` sings such a
  file through a voice into a WAV, with `--report` writing the session's account of the render
  — chunks, load and render time, which processor sang.
* **The words are measured, not only the tune.** The corpus run splits its spectral distance
  by the manner class of the phoneme on each frame — vowels, consonants, the sibilants on
  their own — and measures the sibilant tilt, the energy above 4 kHz against below on /s/
  frames, render against recording. It is the consonant-width study's measurement made a
  permanent column: a voice that tracks its pitch to the cent and hums through every /s/ now
  shows it as a number.
* **A voice says how loud its consonants are, and the frames listen.** The energy a singer
  track's frames carried was the note's velocity on every phoneme, a plateau — and measured
  on JSUT-song a voiceless plosive or fricative sings twenty-odd decibels under the vowel
  after it, a voiced consonant six to nine, an approximant three. A /k/ at the vowel's level
  is a /k/ the model has never heard, and on the labelled corpus that plateau alone cost the
  phoneme error rate 0.25 → 0.56, more than the consonant widths and the note-flat pitch
  together. An export now carries `phoneme_levels`, measured by
  `training/scripts/measure_phoneme_levels.py` from a labelled dataset the way the widths are
  measured from labels; choosing the voice copies the table into the document beside the
  widths, and `auris-vocal` turns each consonant down by its measured decibels.
* **The host spells ざ and にゃ the way the voices were trained.** Both of `auris-vocal`'s
  front-ends — the kana table and the OpenJTalk map — wrote ざ行 as `z` and にゃ行 as `nʲ`,
  while the trainer's own OpenJTalk map writes `dz` and `ɲ`, so every voice was trained on
  those and the host sang ざ, ず, ぜ, ぞ, にゃ, にゅ, にょ through embeddings nothing had ever
  trained. Both symbols were in the phoneme table, which is all the contract checked; it now
  also checks that every symbol the host can emit is one the trainer's front-end writes.
* **A corpus's labels align the training, where it has them.** Training recovered every
  phoneme-to-frame alignment by monotonic alignment search, labels or no labels, and
  measured against JSUT-song's labels the search gives ɕ two thirds of its frames and ts
  under three fifths, one ɕ in three no more than two frames, and puts a boundary
  100–170 ms off on average — a consonant's worth. A preprocessing source can now name
  a `duration_dir` of labelled seconds per phoneme; the preprocessor stores them as frames,
  training expands the phonemes by them and skips the search (`data.use_durations`), the
  host evaluation measures every voice on the same labelled alignment, and
  `scripts/compare_alignment.py` prints the search against the labels by class.
  `scripts/prepare_jsut_song.py` writes the durations beside the transcripts.
* **And whether the words can be heard.** `--asr` transcribes every render with a
  recogniser in the voice's language — ReazonSpeech for Japanese, the `asr` extra — turns
  the transcript back into IPA through the trainer's own front-end and reports the phoneme
  error rate against what was asked for, with the recording's own rate beside it as the
  ceiling a speech recogniser reaches on song. Another language is one class registered
  under its code.

### The words write the tune

* **Singing synthesis can take the GPU.** Settings → General chooses where a voice model
  runs its inference: Auto (the default) sings on the platform's own GPU provider —
  DirectML on Windows, Core ML on macOS — wherever the runtime offers one, and falls back
  to the CPU where it does not, including mid-render: a provider that accepts the session
  and then refuses its shapes (DirectML does, to this model family, today) demotes the
  voice to the CPU and the render finishes. Forcing GPU makes those refusals visible
  errors instead; CPU opts out entirely. The choice applies from the next render — no
  relaunch.

* **Consonants take the width their voice measured.** A newer auris-singer export carries
  per-phoneme consonant durations measured from its training data (`phoneme_durations` in
  the model's metadata — consonant length in sung Japanese spans a factor of three by
  phoneme class, so one fixed width mistimed half the inventory). Choosing the voice copies
  the table into the document beside its name, and everything that lays phonemes out reads
  it: the sung frames, the drawn segmentation, the divider grab, the dragged-note preview.
  Old exports, and tracks with no voice, keep the fixed sixty milliseconds. A table in a
  unit this build cannot read is refused outright rather than misread by two orders of
  magnitude.

* **Compose from lyrics, the Orpheus way.** `compose_lyrics` at both model doors — and
  `Session::compose_from_lyrics` beneath it, for every frontend to come — takes Japanese
  lyrics and writes a song: phrases cut where a singer breathes, one note per mora, and a
  melody found by dynamic programming under the constraint Orpheus made famous: the tune
  must not contradict the lyric's spoken pitch accent. Chords are stamped visibly into the
  harmony lane first (王道進行 by default; a harmony already written is left alone), the
  standard band comes along behind on its usual recipes, and every note lands carrying its
  mora and phonemes, ready for `sing`. Kana lyrics compose without any dictionary — the
  accent constraint simply has nothing to say, and the answer says so — while a configured
  Japanese dictionary reads each phrase's accent nucleus and makes the melody follow the
  words. The same lyrics and seed write the same song.
* Under the hood, three seams left open on purpose: `Contour` (rise / fall / no-fall /
  free) lives in `auris-core` and names no language, so another prosody — or a learned
  model — can produce it; the accent analysis in `auris-vocal` reads jpreprocess's own
  accent phrases, from the same dictionary run that already yields phonemes; and the pitch
  search in `auris-compose` is one function with rhythm assignment as its own stage, so a
  richer rhythm scheme or a trained melody engine can stand beside either without touching
  the rest.
* **In the window: File → Compose from Lyrics…** — also findable in the command palette,
  with an empty keystroke slot for whoever wants one. Type the words — Return breaks a
  phrase, secondary-Return composes — and the piano roll opens on the song; every run draws
  a fresh seed and the status bar names it,
  so a take can be asked for again at any of the model doors. The refusals speak through
  the same field: empty words, or kanji with no dictionary anywhere, say exactly that.
* **Lyrics join the song sheet, right on the sheet.** The sheet's third column is now the
  words themselves: one multi-line box per section, in the order the form plays them, no
  button and no popup between the writer and the verse. Click a box and it is a real editor
  in place — Return breaks a phrase, Tab walks to the next section, a click lands the caret,
  the IME composes where the text is — and every keystroke is already on the sheet's dials;
  Escape just puts the keyboard down. The margin measures the words as they are typed: a
  note count per line (one note per mora), and in each box's heading the bars the sung
  rhythm needs against the bars the section has — red once the words would outrun it, and
  computed by the very reading and rhythm Write uses (`Session::measure_lyrics`), so the
  display cannot drift from what happens. The parts moved beneath to make the room: a strip
  of cards, two abreast, scrolling past its height. The `.asong` format grows the matching
  `lyrics = "..."` field, round-tripped like every other, shown in the annotated reference
  example. Writing the piece adds a Vocal track beside the band, one clip per playing of
  each lyrical section, the melody searched over that section's own harmony; every playing
  of a chorus sings the same chorus. Lyrics nobody can read cost their sections and are
  named in the report, never the piece. File → Compose from Lyrics… grew the same multi-line
  field: Return breaks a line there too, and secondary-Return composes.
* **A composed vocal arrives ornamented, by rule.** The first note of each phrase scoops
  in, notes held past half a second sway (the vibrato waits out the front of the note and
  fades in), and the line's last note falls away — the phrase-final syllable is now held a
  half note, which is both what a sung phrase does and the room the sway needs. The
  ornaments are the ordinary scoop / fall / vibrato fields, visible on the pitch curve and
  editable one by one; the rules are a starting point, never a verdict.
* **Singer voices join the library, on the instruments' own terms.** The browser grows a
  Voices section: every `.onnx` in a `Voices` folder — beside the binaries, in the
  configuration directory, or under `AURIS_VOICES` — plus any folder registered from the
  section's *Add Voice Folder…* row (remembered, never copied, exactly like plugin
  folders). One click puts a voice on the selected singer track the way one click puts a
  sound on an instrument track; the search box finds voices by name beside everything
  else; Track → Choose Voice… keeps the file dialog for the one-off file somewhere
  unusual.
* **The Japanese dictionary now ships with a release.** `naist-jdic` (jpreprocess's build,
  BSD-3-Clause) travels in a `Dictionary` directory beside the SoundFonts — inside the
  bundle on macOS — fetched at packaging time by `tools/fetch-dictionary.sh` against a
  digest recorded once, in `auris_session::library`, where the fonts' manifest already
  lives (`auris dictionary --manifest` prints it, `auris dictionary` says what is
  installed). Kanji lyrics read out of the box, and a melody composed from lyrics follows
  their accent out of the box. The settings entry becomes an **override**: choose a folder
  to swap in your own build, Clear to return to the shipped one, `AURIS_DICTIONARY` to
  point the search elsewhere entirely. The shipped archive is v0.14.0's — jpreprocess's
  v0.15.0 release ships no standalone dictionary — and the accent test proves it loads
  under the 0.15 crate rather than assuming so.

## 0.4.0 — 2026-08-31

### The singer gets a real voice

* **Singing-voice synthesis works end to end.** A singer track can now be pointed at a trained
  voice model — one self-contained `.onnx` file, chosen the way a SoundFont is and left where it
  lies — and asked to **sing**:
  notes and lyrics become frames, the frames become a waveform, and the waveform lands in
  `Audio/` as the track's *take*, an ordinary audio source that plays, exports and reopens
  with everything else. Track → Choose Voice… and Track → Sing in the window (the render runs
  off the main thread behind the export overlay, with the same stop button), and
  `auris sing <project> [--track <name>] [--voice <model.onnx>] [--seed <n>]` at the command
  line.
* **A take is kept, never silently rewritten.** Every random choice is pinned by a seed the
  document stores, so the same document, seed and voice render the same audio on any machine.
  Editing notes after a render leaves the take playing — a voice someone chose does not fall
  back to the formant preview over one edited word — and the track header says
  *behind the notes* until Sing is pressed again. Clicked notes still audition through the
  preview instrument while a take plays.
* **The take keeps up with the score by itself.** Once a track has a voice, editing the notes
  *is* the ask: shortly after an edit settles, the window re-renders the take in the
  background — no overlay, no button, just the header badge reading *… ♪ voice* while the
  CPU is spent, which is the standing rule that a cost switched on without a click must say
  so on screen. A burst of edits coalesces into one render; an edit landing mid-render
  cancels the stale work between chunks; a manual Sing or an export takes the stage and the
  background render steps off it. Refusals worth acting on — an unsaved project, a voice
  that stopped loading — are said once in the status bar rather than once per frame, and an
  empty or voiceless track is simply left alone.
* **The model doors sing too.** Three additions to the toolbox, identical over MCP and in the
  agent panel: `add_track` accepts kind `singer`, `write_lyrics` lays a phrase across a clip's
  notes one syllable each (`notes` reads the words back beside the pitches), and `sing`
  renders the track through its voice model into the project's take — so a model can write a
  song and make it sing without a window open. Twenty-nine tools in all.
* **A dragged note previews in the real voice.** Grabbing or dragging a note on a voiced
  singer track no longer strikes the formant instrument: the model sings that one note — its
  own syllable, half a second at the grabbed pitch — in the background, and the engine plays
  the render at once, transport rolling or not. Renders are cached by voice, seed, pitch and
  syllable, so a drag is instant everywhere it has already been; short sequences sit well
  inside what a desktop CPU renders faster than real time. Tracks with no voice keep the
  formant preview, and chords stay on the instrument path. Under the hood the engine grew
  one-shot playback: a pre-rendered buffer crosses the command channel whole, is mixed in by
  the callback without ever being freed there, and travels back up the retired-data channel
  when replaced — the `SetGraph` discipline in miniature.
* **The piano roll draws the sung pitch curve.** Editing a singer clip overlays the contour
  the voice will actually sing — the note's pitch plus its bend curve, in fractional
  semitones, with consonants riding their vowel and rests leaving a gap in the line — over
  the notes, the way vocal editors draw it. It is computed from the same frames the model is
  fed, cached against the document revision like the take badge, so what is drawn and what
  is sung cannot drift apart.
* **And the phoneme segmentation beside it.** The same frames say where each phoneme's time
  actually falls — the sixty milliseconds a consonant takes at a note's edge, the vowel
  stretched over the rest — and the roll now draws it: a faint divider inside the note at
  each cut, the symbol above the note at the position its frames begin. The old untimed
  phoneme list above a note yields to this wherever frames exist; zoomed far out the symbols
  step aside and the dividers stay.
* **The cuts are draggable.** Take hold of a divider and the phoneme to its left is pinned
  to that many seconds — stored on the note beside its phonemes (`Note::phoneme_seconds`),
  so the adjustment travels, saves and re-renders with the word. The timing rule lays the
  unpinned phonemes out around the pins, squeezing proportionally where pins outgrow the
  note; retyping a lyric clears its pins, the note's menu offers Reset Phoneme Timing while
  any stand, and one drag is one undo step.
* **A note can wear pitch ornaments: scoop, fall, vibrato.** しゃくり rises into the note
  from below, a fall drops away at its end, a vibrato sways around it once settled — each a
  handful of numbers stored on the note (`Note::scoop` / `fall` / `vibrato`), shaped by one
  public function (`auris_vocal::ornament_offset`) that the frames, the painter and the
  editor's grab test all read, so drawn, heard and grabbed can never disagree. The note's
  menu toggles each ornament on with defaults taken from singer measurements, and every
  ornament then carries a handle on the drawn pitch curve: the scoop's and fall's at the
  corner of the gesture (drag for span and depth at once), the vibrato's at the crest of its
  first sway (onset and depth). Scoop and fall cap at half the note so they cannot collide;
  ornaments are pitch, not phonemes, so they survive the lyric being retyped; one drag is
  one undo step, and the take re-renders itself afterwards like after any other edit.
* Under the hood: the new `auris-singer` crate runs the model on the CPU via onnxruntime,
  cutting the timeline at silences into chunks of at most twenty seconds — the model's
  attention grows with the square of the frame count, and a whole song in one inference has
  taken a machine down — and deciding each frame's voicing from the phoneme class
  (`auris_vocal::is_voiceless`), never from `f0 > 0`, which would hum through every /k/.

### The conversation moves into the window

* **The Agent panel** — View → Agent, docked on the right beside the inspector the way an
  editor's chat sidebar sits. Ask for a piece, watch each tool call land in the transcript,
  read the answer; provider, model, URL and API-key variable are set in the panel and saved
  to the shared settings, which `auris-agent` on the command line now reads as its flag
  defaults. The window spawns `auris-agent --json` beside its own binary and speaks JSON
  lines over its pipes, so the frontend boundary holds: the window never learns what an LLM
  client is.
* The two ends of one file, settled: the window **saves before every message**, so the model
  always reads the document as it stands; when a tool call writes the project back, the agent
  reports it (`changed` events, from the new `auris_toolbox::WRITES_PROJECTS` list) and the
  window reloads — automatically while it holds nothing unsaved, by an offered button when it
  does. The whole policy is a plain function with tests; the window only obeys it.
* `auris-agent --json`: newline-delimited JSON for a host program — `{"say": …}` in;
  `ready`, `call`, `result`, `changed`, `answer`, `error` out. A provider failure is an
  `error` event rather than an exit, because the host's window is still open.
* From the first live sitting: **provider and model are both picked, not typed** — the panel
  runs the new `auris-agent models` (Ollama asked in its own words, context windows included;
  OpenAI-compatible via `/models`) and fills a dropdown, refetching when the provider or URL
  changes. A **context gauge** in picocode's image sits over the input — `↑ prompt ↓ written`,
  a bar filling the chosen model's window, yellow from 60% and red from 85% — fed by token
  counts that now ride every `answer` event. **Tool rows open on a click** to the whole answer
  the model saw, which makes the transcript the loop's log. And a send with no model
  configured now says so in the transcript instead of silently doing nothing, which is what
  the first Enter ever pressed in the panel ran into. From the sittings after it: **picking a
  model applies it on the spot** — the choice writes through to the settings as the menu
  closes, and Enter in the chat treats a completed settings form as applied, so the Apply
  button now only serves the typed fields.
* **The model can edit the arrangement in place.** Six tools at both model doors, so one more
  part is an edit rather than a recomposition: `add_track` (a new track in an existing
  project, voiced by a built-in id or any General MIDI sound by name or number — the shipped
  font is adopted into the project in the same step), `add_part` (a generated part written
  onto a track from the harmony already under the song, recipe kept so `another_take` and
  `write_again` apply), `set_instrument`, `rename_track`, `remove_track`, and
  `list_instruments` for the vocabulary. Twenty-three tools in all. Behind them,
  `Session::set_track_general_midi` is new: font adoption and preset choice as one undo step.
* **The window notices another writer.** Edit an open project from outside — the MCP door
  driven by Claude Code, a sync service, anything with the file — and the window follows: it
  reloads silently while it holds nothing unsaved, and puts a Reload button in the status bar
  when it does, saying what stands to be lost either way. While that choice stands, autosave
  holds its fire — writing over another writer's version is a decision, and ⌘S is where
  decisions are made. Watching is a half-second stat of one file; the whole policy is two
  tested functions (`should_autosave` grew an `overwritten` clause, and
  `external_change_action` is the window's side).
* **The model can place notes, and derive a band from them.** Four more tools at both doors,
  for the melody-first way around: `add_clip` opens an empty clip, `edit_notes` places and
  removes notes by name and bar in one call ("F#4", bar 2, beat 3.5), `notes` reads a clip
  back numbered in time order — the numbers are the removal address — and `accompany` reads a
  melody clip and writes the key, the chords and a backing band under it without touching a
  note of the tune, `Session::accompany` finally reachable from outside the window.
  Twenty-seven tools in all.
* **The model can be handed audio.** `auris-agent --attach mix.wav "how is this?"` sends the
  file base64-encoded as an OpenAI `input_audio` content part beside the words, and the JSON
  wire takes `{"say": …, "audio": ["mix.wav"]}` per message. OpenAI-compatible servers only —
  Ollama's API has no audio field, and the agent says so up front rather than after the
  request is built. wav, mp3, flac, ogg, aac, aiff and m4a are typed by extension.
* A review of the two model doors and the panel, and the mends it asked for: a silent reload
  no longer leaves a rename sheet open over a document that is gone (the sheet comes down
  with the document that raised it); an audio attachment over 25 MB is refused by its size
  on disk before a byte is read; the `models` listing gives up after twenty seconds instead
  of parking a thread behind a server that never answers, and a second press of refresh no
  longer stacks another question behind the first; a non-UTF-8 line on the `--json` wire is
  an `error` event, as promised, rather than the end of the process; `edit_notes` refuses a
  velocity outside 0-1 the way its siblings refuse their ranges — and its description now
  names the real default, 0.75, with a test pinning the prose to the constant; absurd
  octaves and bar counts are refused rather than overflowed.

### The model gets its hands on the mixer

* Five tools at both model doors, asked for by the first model to drive the improve loop —
  it had spent an afternoon moving tenths of an LU by reseeding parts, against problems that
  were mixer-level all along. **`mixer`** reads the whole board: every fader, pan, send and
  effect parameter with its key, value and range. **`set_level`**, **`set_send`** and
  **`set_effect`** move one each — the master limiter's `input_db` is now a dial a model can
  back off when `analyze` says the climax is pinned. **`section_gain`** holds a track's (or
  the master's) gain at a level across one named section: dynamics without rewriting a note.
* Behind that last one is a new session command, **`Session::hold_automation`** — hold a
  parameter at a value across a stretch, short ramps at the edges, the surrounding lane (or
  the fader's own position) preserved, holds on different sections composing, one undo step.
  Every frontend gets it; the desktop's automation lanes draw what it writes.

### The doors meet a model where it stands

* Every tool that opens a project now resolves its path the way `compose` saves: absolute
  (relative asks are read against the host's working directory), and reaching one folder down
  when `Name.auris` really went to `Name/Name.auris` — the nesting is one-to-one, so it is
  walked rather than taught as an error. `compose` and `render` answer with absolute paths,
  because that line is what a model copies its next call from. Found live: a gemma-class
  8B model handed back exactly the relative shorthand it had asked `compose` for, stalled on
  file-not-found before the change, and completed the same loop after it.

### A fourth frontend, where Auris dials the model

* **`auris-agent`** is the mirror of `auris-mcp`: instead of waiting for a model's harness to
  connect, Auris connects to the model — a local [Ollama](https://ollama.com) server
  (`--model qwen3:8b`), or anything speaking the OpenAI chat-completions dialect behind
  `--provider openai` and `--url` (OpenAI, LM Studio, vLLM, OpenRouter) — hands it the same
  twelve tools, and runs the loop itself via [rig](https://crates.io/crates/rig). With a
  prompt it asks once and prints the answer on stdout, narrating each tool call on stderr so
  a pipe keeps the answer and a person sees the work; without one it holds a conversation,
  carrying the transcript forward — the improve loop with a person in it. An API key is only
  ever named by environment variable, never typed into a command line.
* The tools themselves moved into **`auris-toolbox`**, a presentation crate for the reader
  that is a model, the way `auris-i18n` is for the reader that is a person: one module per
  tool — name, description, argument schema, work — shared by both doors, so `compose` at the
  MCP door and `compose` at the agent door are the same text, the same schema and the same
  code by construction. (The MCP macro reads descriptions only from literals, so that door
  carries a copy — held word-for-word equal to the toolbox text by a test.) The agent's whole
  loop is itself under test against a scripted OpenAI-compatible server: the fake model calls
  a tool, the real toolbox answers, and the answer rides back over the wire.

### A third frontend, for readers that act on the answer

* **`auris-mcp`** puts the same headless session behind the
  [Model Context Protocol](https://modelcontextprotocol.io), over stdio, so a language model's
  harness can drive Auris as tools — `claude mcp add auris -- ./target/release/auris-mcp` is
  the whole installation. Seven tools cover the loop of writing a song and hearing it:
  `spec_reference` teaches the `.asong` format by example, `check_spec` validates a draft and
  answers with every default filled in, `compose` writes and saves the piece, `render` makes
  WAV files (the mix, or stems), `describe` reads a project back, and `list_presets` /
  `list_progressions` are the quotable vocabulary. Errors are tool answers rather than
  failures — a rejected spec names its lines and fields, which is exactly the loop a model
  iterates in.
* The loop that improves a piece: **`analyze`** renders a project and listens in the model's
  place — loudness and peaks for the mix, per named section (the dynamic arc as numbers), and
  on request per track alone, soloed through its buses. Behind it is a new session command,
  `Session::analyze`, so every frontend can ask the same question. Against the answer a model
  edits the spec and composes again, or aims **`another_take`** (same ask, next seed) or
  **`write_again`** (same seed, follows the harmony as it stands now) at one clip, addressed
  by the numbering `describe` now prints beside every clip — along with each clip's recipe,
  seed, and whether a hand has edited it. Every take names its seed, and `another_take` takes
  a named `seed` back — the first model to drive the loop lost a take it liked behind the
  advancing counter, and a rewrite that measured worse should never be a one-way door.
  **`teach_progression`** keeps a chord progression by name on this machine;
  **`forget_progression`** takes it back out.

### Arrangement: the joins learn a second word

* A new part role, the **riser**: General MIDI's reverse cymbal (program 119), written by the
  same writer that places the crash and answering the same question one section early — it
  swells through the last second before every arrival the crash then opens, the sample's own
  peak landing five milliseconds ahead of the downbeat (measured on the shipped font; the lead
  converts per section tempo, so the swell is the same second of cymbal at any speed). Declare
  it with `role = "riser"`; the pop-band, city-pop and synthwave presets now do.
* A piece can end by leaving the room: **`ending = "fade"`** rides the master fader from unity
  to silence across the final eight bars (or the whole final section, where it is shorter), and
  is the composer's first piece of written **automation** — the ride arrives as an ordinary
  lane on the master gain, in the same view as one drawn by hand, so it can be reshaped or
  deleted like anything a person wrote. A fading piece takes no landing bar; the fade is the
  deliberate refusal of one.

### Performance: the score does not change, the playing does

* Any note clip can now carry a **performance**: humanize, swing and gate dials in the
  inspector that change what is *heard* — playback and MIDI export both — while the piano roll
  keeps showing the notes exactly as written. Setting a dial back to its resting position
  removes it entirely; **Keep the Performance** writes what is heard into the notes, the same
  trade as freezing a generated clip's recipe. The format version moves to 18 for the new
  field.
* The humanize wander is drawn from a seed stored in the file, so a project sounds the same on
  every open — and it is drawn afresh for **every pass of a loop**, so a repeated bar is loose
  differently each time around instead of rehearsing the same accidents. Its feel is
  calibrated to the composer's own: the same milliseconds of wander, the same velocity spread,
  at every tempo.
* A transpose transform exists alongside the three dials (session API only for now), and the
  stack is ordered — the panel keeps swing in front of humanize, so the swing still finds its
  offbeats before the wander moves them off the grid.
* **The composer performs through the same stack now.** A generated clip's text lands on the
  grid — only the groove's swing is still written — and its feel arrives as transforms on the
  clip: the wander per pitched part, plus a new deterministic **lean** (the hat a touch early,
  the snare laying back, exactly as the composer always played them). One Humanize dial edits
  it, in the Performance section; the recipe's own humanize dial is gone, and regenerating or
  re-rolling a clip touches the text alone — the feel you set stays set, though another take
  re-seeds the wander so one number still names both. The kit keeps time as before and, being
  wander-exempt, no longer varies its stroke with the dial. The format version moves to 19 for
  the new transform kind.
* **A generated clip knows when it has been edited by hand.** Every write stamps a digest of
  the notes into the recipe, and a clip whose notes have drifted from it shows a standing note
  in the inspector — "edited by hand; writing it again replaces the edits" — for as long as
  they differ. Undoing the edit clears it, a resize or a split does not raise it, and nothing
  is blocked: regenerate still does exactly what it says. Files from before this field simply
  never warn.
* A saved file now records **which build saved it**. Opening a project another build saved puts
  a note on the status line (and on the command line's stderr): the piece sounds exactly as
  saved, but regenerating any clip writes in the current composer's style — so a take worth
  keeping wants freezing first. Informational only; nothing is blocked.

### Singer tracks: notes that carry words

* A new track kind for a singing-voice synthesiser. Its clips are ordinary note clips — the
  piano roll, the lanes and every clip gesture apply unchanged, and a melody moves freely
  between an instrument track and a singer track — but each note can carry a **lyric** and the
  **IPA phonemes** it is sung as. The format version moves to 17 for the new track kind.
* **Double-click a note to type its word**; Return commits and walks to the next note, so a
  verse is typed straight through. *Write Lyrics…* lays a phrase across the selection one mora
  to a note; *Edit Phonemes…* corrects a reading by hand and leaves the word as spelt.
* **Kana lyrics need nothing installed.** Kanji is read through a Japanese dictionary — a
  prebuilt `naist-jdic` folder from the jpreprocess releases, named in *Settings → General*
  and loaded where it lies, like a SoundFont. Only new kanji ever asks for it, with an error
  naming the setting.
* **File → Export Singer Frames…** writes what a voice model consumes: phoneme id, pitch in
  Hz and energy per 10 ms frame (the hop is stored on the track), as JSON. Pitch is the note
  plus its bend curve; energy is the velocity under an envelope, scaled by controller 11.
* Until a model is wired in, the track previews through **Vocal**, a new built-in instrument —
  three formants over a saw, one open vowel — that answers the modulation wheel with vibrato
  and the expression pedal with level, the same controller the frames read as energy.

### Four more colour schemes

* **One Dark, One Light, GitHub Dark and GitHub Light** join the four the application shipped
  with, so a project can be worked on in the palette the rest of the desk is already in. Each is
  the editor theme's own numbers — its background's hue and lightness, and the blue it marks a
  selection with — and the other thirty colours are derived from those, as every scheme's are.
* They are held to the same rules as the rest: the surfaces stack the same way round, body text
  clears 7:1 on the hardest of them, and a group's mark can be seen at every hue. Two of the four
  had to give a little to pass — One Dark's window is a shade deeper than the editor's, or a
  failure in the status bar would not have been readable, and GitHub Light's is a hair under white
  so the timeline still sits below the window it is cut into.

### The whole band can hear itself

* **Monitoring is a switch per track**, like the arm, and up to eight play at once — each through
  its own strip and its own armed input channels. A band therefore hears itself the way it will be
  recorded: every player through their own fader, their own effects and their own microphone.
* The ninth is refused rather than silently dropped. Every path back into the mix is a ring that
  has to exist before the input device starts, because the input callback may not make one.

### A meter for every armed track

* Each armed track's header shows **what is arriving on its own input channels**, as a thin bar
  beside the one that shows what the track puts out. Four microphones on a four-input interface
  are four readings; the transport bar's meter stays what it was, the whole device in one number.
* It appears only while the track is armed and something has the device open, and it latches a
  clip like every other meter — cleared with the rest by clicking the master block.

### Two takes can be joined without a hole in the join

* **Crossfades.** Drag one clip over another on the same track and let go: the join is shaped as
  part of the same undo step as the move, the earlier fading out across the overlap while the
  later fades in across the same stretch. How long the join is is how far you dragged — nothing
  moves to make room. A fade you drew is never written over, and **Crossfade** on either clip's
  menu makes the join on demand where one is.
* **Fade-In Shape** and **Fade-Out Shape** on the same menu choose the curve by hand, for a join
  made by dragging a fade rather than by asking for one.
* A clip's fades now carry a **shape**, one for each edge. An edge fade stays a straight line;
  a crossfade uses the equal-power curve, so the pair holds its level through the middle of the
  join instead of dipping about three decibels there. The arrangement draws the curve it plays.
* **The project format is version 16.** An older build ignores the shapes, so every crossfade in
  the piece would play as two straight ramps with a hole in the middle — and the next save would
  write the shapes away.

### The mix, taken apart

* **Stem export**: *File → Export Stems…*, or `auris render --stems <folder>`, writes one WAV per
  track into a folder, named after the track. Each one is that track as it sounds soloed — the
  buses it is routed through included, so a part sent to a reverb arrives with its reverb.
* Buses get no stem of their own and muted tracks are skipped, so the set of files is the mix
  taken apart rather than the mix plus a handful of silent ones. Two tracks of one name still make
  two files.
* Every stem carries the master chain, because a stem is what the mix sounds like with one part in
  it. That means they sum back to the mix exactly where the chain is linear, and not where it is a
  limiter — one to bypass before exporting stems that have to add up.
* One graph, played once per track: a hosted plugin is instantiated once for the whole export
  rather than once per stem.

### Four bars before you have to play anything

* **A count-in**, one to four bars, chosen by right-clicking the metronome button. Press Record
  from a standstill and the click counts those bars before the song moves: the playhead waits
  where the take will begin, and the transport bar counts the beats down where it usually shows
  the take's clock.
* Bars are counted in the meter you are in and at the tempo where the take begins — fourteen beats
  for two bars of 7/8, four for two bars of 6/8 — and the click sounds for the count whether or
  not the click itself is switched on.
* It works at bar one, which is where it is wanted most: the count is held in front of the
  playhead rather than played through a stretch of timeline that would have to exist.
* Recording begins with the count rather than after it, so nothing of the first beat is lost to a
  device opening. The count is trimmed off the clip and kept in the file.
* Pressing Record over a song that is already playing starts recording at once. The bars are going
  past already.

### A band can go down at once

* **Every armed track records**, each from its own input channel — one file and one clip per
  track, from one press of Record. Arming a second track adds a take rather than moving the first
  one, which is what the arm button did before.
* A track takes the whole device where that is a pair or less and a single channel otherwise — the
  lowest one nobody else is reading — so four armed tracks on a four-input interface land on
  inputs 1 to 4 without anything being chosen, and one track through a stereo interface still
  records both sides. **Record Input** on a track's menu offers every channel on its own and every
  pair together, and picking one arms the track as well.
* A channel the interface does not have records silence rather than its neighbour: an arm outlives
  the box it was made for, and a take holding the wrong microphone would pass for a good one.
* Monitoring follows the channels the track it plays is armed to read, so listening to a track
  armed to input 5 plays input 5. Still one track at a time — there is one ring and it carries one
  stereo pair.
* A take on several tracks is one undo step, and the report says which of them came back with
  nothing.

### An effect can listen to another track

* **A compressor can be keyed from the kick drum.** An effect slot names a track to listen to, and
  the effect hears *that* instead of the signal passing through it — the built-in compressor, and
  any hosted CLAP plugin with a sidechain input, which has been handed a silent port since the day
  ports were handed over at all. The **Sidechain** row appears on the slots whose plugin has
  somewhere to put a key and nowhere else.
* What the effect hears is what the source puts into the mix: its chain, its fader, its pan and its
  mute. Pulling the kick down ducks less; muting it stops the duck.
* A key is an edge in the routing like an output or a send, so the track that makes it is mixed
  first, and one that would leave a strip waiting for itself is refused — left off the list rather
  than offered and then rejected. Deleting a track clears the keys read from it, and a file
  carrying an impossible one is repaired on open.
* Nothing pays for it unless it is used: a buffer per track something actually listens to, and only
  where the effect in the slot has a reading for one. A project with no key in it copies nothing.
* Format version 15.

### The shipped font's pianos play

* **`rustysynth` is forked into `vendor/rustysynth`, and reads the modulator lists the published
  crate discards.** A modulator is how a SoundFont says that a controller reaches a parameter, and
  the ordinary way to make a sampled sound respond to how hard it is played is to set a filter low
  in the generators and open it with one driven by velocity. MuseScore General's acoustic pianos do
  exactly that, so with the modulators thrown away the piano played through a filter nothing ever
  opened — twenty decibels under every other program in the font, and *falling* by twenty more
  between MIDI velocity 74 and 76, where a layer boundary swapped one static filter setting for
  another that the discarded modulators were meant to override. Playing harder made it quieter.
* One note at middle C now runs -21.1, -20.2, -18.4, -17.6, -14.3, -13.4, -12.1 and -11.3 dBFS
  across velocities 70 to 115, monotonic and level with the rest of the font. The jazz trio preset,
  whose piano and lead were both 25 dB under their own bass, composes to -14.1 LUFS instead of
  -16.9 with no fader running out of travel; it was -27.0 before any of this.
* Of the 128 melodic programs, 101 are unchanged to the sample, the three acoustic pianos come up
  19.9, 19.5 and 11.7 dB, and the other 24 move by less than 3 dB.
* Deliberately partial, and the fork's README says why: only the two filter destinations are read
  through modulators, only controllers that hold still for the length of a note are modelled, and
  the specification's own default modulator list is still not implemented. What a font says about
  loudness with a modulator is left alone, because the velocity-to-attenuation curve the sampler
  already compensates for would otherwise be counted twice.

### A composed piece arrives mixed

* **Composing ends by listening to what it wrote.** Every track is rendered on its own, measured as
  programme loudness to ITU-R BS.1770, and its fader moved until the part sits where a part of that
  kind belongs; then the whole piece is lifted onto −14 LUFS. A fader position is not a level — what
  a track is worth depends on the instrument that answered, and the same number on the same fader
  is a lead at −18.6 LUFS on the built-in synth and −25.8 through the shipped font. The eight
  presets used to span ten decibels, from −17.0 to −27.0 LUFS; they now sit between −14.0 and
  −17.9.
* **Compose → Balance the Mix** does it to the open project, for a piece written before this
  existed or one whose instruments have changed since. Only a track that knows what it is gets
  moved — the composer writes down what each part is aiming at, and a hand-made track has no such
  number — so running it over a mix of your own normalises the loudness and leaves your balance
  alone. Running it twice does nothing the second time.
* It costs a render per part and two of the whole piece, which is about two and a half seconds for
  an eight-part song, and the window does not answer while it runs.
* Two limits it reports rather than hides: a fader stops at +12 dB, so a part on an instrument
  quieter than that can reach ends up short and the status line says by how much; and the master
  fader sits after the master's effects, so the limiter's ceiling now guarantees what leaves the
  chain rather than what leaves the mixer.
* `auris_dsp::loudness` is the meter behind all of it — the K-weighting, 400 ms blocks and the two
  gates, with the filter re-derived per sample rate rather than tabulated at 48 kHz. Checked
  against libebur128 on five composed renders, agreeing within a tenth of a decibel.

### The composer plays steadier

* **A composed part no longer varies how hard it strikes by half its own level.** Every difference
  in strength the composer writes — the metric hierarchy, a ghost note, the lean across a phrase,
  the rise through a fill — is now played at a quarter of what it says, so a part's strokes sit
  within about a tenth of the level it is played at instead of a fifth to a half of it. The kit is
  where it was most audible: a rock hat ran from 0.20 to 1.00, which on an instrument whose every
  stroke is the same sound reads as a drummer who cannot hit the thing evenly. The proportions are
  scaled and not replaced, so a ghost is still the quietest thing in the bar and a downbeat still
  the loudest.
* **The timing wander is 6 ms at the top of the humanize dial rather than 15.** Fifteen was chosen
  against the default setting alone, and the presets ask for more than the default: measured across
  the eight of them it moved a jazz piano by up to 27 ms and an ambient bell by 25, which is not a
  player relaxing. Two parts written on one tick still do not land on one tick, and nobody is
  waiting for anybody.
* A section's intensity is untouched and keeps the whole of its travel: how hard a passage is
  played is a different question from how much one stroke of it varies, and it was the second one
  that sounded like bad playing. Both dials still reach 0, which is still a sequencer.

### The palette answers to English too

* **A command is found by its English name whatever language the interface is in.** A window drawn
  in Japanese shows 保存 and now also matches `save`, which is what the documentation, the
  keystroke chart and every other audio program call it — the language on screen is a display
  choice, and it should not decide which alphabet somebody has to type in. Both spellings are
  matched and neither is preferred: the better of the two scores counts, so a row cannot out-rank
  another merely by being matched twice. The key search in the settings window reads the same way,
  since the two lists are supposed to mean the same thing.
* The language rows are matched from both sides, since they are the one place where what is drawn
  does not follow the setting: `language` now finds 言語 · 日本語 from an English window, as it
  already found Language · English from a Japanese one.

## 0.3.0 — 2026-08-22

### Plugins somebody else wrote

* **Auris hosts CLAP plugins**, instruments and effects alike. They are found in the platform's own
  search paths, listed in the browser under the built-ins as one branch per file, and placed on a
  track or into a chain the way anything else is. The graph cannot tell a hosted plugin from a
  biquad, so automation, presets and the parameter panel work on one exactly as they do on ours.
* **A file's branch is shut until you open it**, and that is not about size: opening one loads it,
  and loading a plugin means running somebody else's code in this process. That has to be something
  a person did, rather than something a panel did on their behalf while they were looking for a
  reverb.
* **A plugin opens its own window**, floating above the application and told which window to stay
  above. Plugins that draw into a window rather than making one — anything built on JUCE, which is
  most of them — are lent a plain native window to draw into, because a host with nothing to give
  them shows nothing at all.
* **The parameters are asked of the plugin, not guessed from the document.** A preset loaded
  inside it, a knob turned in its own window, its own MIDI mapping: all of them move parameters
  the session never hears about. The document is asked first and the plugin second, so the panel
  beside a plugin's window agrees with the window.
* **A plugin is sent the note dialect it declares** — CLAP's own or MIDI, and the better half of
  each from one that speaks both. One that speaks neither gets no notes, rather than events sent
  into a void.
* **Where else to look is in the browser**, at the foot of the plugin list: point at a `.clap` or
  at a folder holding a hundred of them, and it is remembered. A plugin built in a working tree or
  kept on an external disk was previously unreachable however plainly it could be pointed at.

### The computer keyboard is an instrument

* **`a` to `;` is a piano** — Logic and GarageBand's layout key for key, including the keys that
  look arbitrary, because a layout a hand already knows is the whole point and one that is nearly
  the same is worse than one that is plainly different. `1` and `2` bend, `3` to `8` are the
  modulation wheel, `z` and `x` move the octave, `c` and `v` the velocity, and Tab sustains.
  ⌘K switches it on, and has to carry a modifier: a bare letter could not switch off a keyboard
  that plays nearly every bare letter.
* **The keyboard is drawn while it is on**, so which octave the hands are in, how hard they are
  striking and where the wheel was left are on screen instead of in your head. Every control lights
  while it is in force, and the bar across the top says where those seventeen semitones sit on the
  whole of MIDI.

### Audio follows the tempo

* **A recording can be stretched to the piece instead of being left behind by it.** Playing a file
  faster is a resampling away and takes the pitch with it; this is WSOLA, which lays overlapping
  windows down at a new spacing so every period inside one keeps its length and only the number of
  them per second changes.
* **Per clip, from its menu.** A clip knows what tempo it was recorded at and whether it should
  follow the piece's, and everything that asks how long it is asks what it plays as rather than
  what was stored — so the arrangement and the renderer come from one number.
* **A following clip says so on its face**: a pill at the end of its name bar reading the stretch
  as a percentage of the recording. The number rather than a mark, because 100 % is a clip being
  played untouched and 150 % is one being pushed half again as long, and only the second is a
  reason to go and listen closely.
* **Cutting one does not change how it sounds.** A split or a front trim writes down the tempo the
  clip was anchored to, so two halves of a take the far side of a tempo change still play as one
  thing.

### A curve on any parameter, and a lane on any controller

* **Right-clicking a control offers automation on it** — a mixer fader, a send level, a plugin's
  row in the inspector. The document has always held those lanes and the engine has always played
  them; the track menu could only ever name a track's own fader and pan, so a lane on anything else
  could be written by hand and never edited.
* **A lane slides or steps, and either can be changed afterwards.** The shape used to be guessed
  from the parameter when the lane was made, and the guess was final — a fader meant to drop on the
  bar line rather than slide into it had no way to say so.
* **A clip carries any of the hundred and twenty-eight controllers**, not the wheel alone. The
  piano roll opens a strip for whichever ones you ask for, stacked in number order and remembered
  in the layout — an expression pedal, a breath controller or a sustain pedal now has somewhere to
  be written down.

### The export can be shaped, and called off

* **Depth, rate and dither are settings** rather than 24-bit at the project's rate every time.
  `auris export` has had the flags since the exporter was written and the window passed defaults;
  delivering a 16-bit master meant leaving the application.
* **A bounce can be stopped.** Export is the longest thing this application does and the overlay
  over it had one button, which appeared only once the render was over. A cancelled export is a
  third outcome rather than a failure: the bar stops where it got to and the line says what was
  written.

### The window opens where you left it

* **Its place and size are remembered** across launches. A DAW is arranged around a screen — pushed
  to one side of a second monitor, sized so the arrangement and the browser both fit — and doing
  that again every session is a tax on the thing the application is opened for. A remembered
  rectangle that no longer overlaps any display is ignored rather than restored somewhere no
  pointer can reach.
* **The projects opened lately are on the File menu**, ten deep, written the moment one is opened
  or saved under a name rather than on the way out: a recent list is worth most exactly when the
  last session ended badly.
* **`auris-studio Song/Song.auris` opens that song**, and so does double-clicking a `.auris` file
  with the application registered against it. The window used to ignore its argument.
* **Help → About** says which build this is.

### A meter says what already happened

* **Clipping latches.** A meter falls at 20 dB per second, so a single block over full scale is
  most of a decibel down by the next repaint and a clip nobody saw is a clip that was not reported.
  The latch is kept where every block is seen — the audio thread — and stays lit until it is
  cleared by clicking the master's meter.
* **The input meter moves whenever the device is open**, not only during a take. Setting a level is
  what somebody does *before* pressing Record, and a meter that appeared once the take began
  arrived too late to be used for it.
* **A take has a clock on it, and says while it is going wrong that it is.** How long it has run,
  frames the disk could not keep up with, a device that has disappeared — all three used to be
  visible only in the report after Stop, which is the wrong moment for every one of them.

### Finding things, and typing into them

* **The browser has a search field.** It holds every built-in instrument and effect, every `.clap`
  on the machine and every sound in every imported font — a General MIDI bank is a hundred and
  twenty-eight of them — and the only way to a name was to open the right branch and scroll.
* **Every panel that scrolls says where you are in it.** A bar appears along the edge of the
  browser, the inspector, the log, the lane column and the mixer's strips the moment there is more
  than fits, and takes no room at all while everything does. Pressing its track jumps there.
* **Shift makes a drag five times finer, and a value can simply be typed.** A full sweep of a
  slider is 220 pixels, which on a cutoff running 20 Hz to 20 kHz is about a hundred hertz a pixel
  at the bottom — fine enough to find a filter by ear and far too coarse to land on a number.
* **Double-clicking a track's name renames it**, on the header and on the mixer strip both. It was
  a menu row and nothing else, which is how a project ends up with eight tracks called Audio 1.
* The arrow keys move the selection, both keys labelled Delete delete, menus show which way their
  switches are set and which rows can do nothing, and the unlabelled buttons say what they are and
  which key also works them.
* Text fields answer the same editing keys everywhere — backspace, the caret keys, Select All, the
  clipboard — because there is now one table for them rather than four that had drifted apart. An
  IME composes into all of them.

### Effects can be dragged into order

* **Take hold of an effect's name and move it.** The chain rearranges as the pointer travels, in
  the inspector and on the mixer alike, and the whole drag is one undo step. Reordering used to be
  a chevron clicked once per position, or a menu row chosen again and again.
* **Every strip ends in an empty slot that adds an effect.** A track's strip in the mixer had no
  way to add one at all — the answer was a right-click on empty space — while the master's had a
  button above its chain instead of a slot at the end of it.

### Importing does not stop the window

* **A file is read on a worker thread.** A two-hundred-megabyte SoundFont, or an audio file being
  decoded and resampled, used to be read on the thread that draws: the status line said which file
  it was and then nothing answered until it was done, which is a freeze with a caption. Playback
  and everything else now carry on while it happens.
* Files dropped together are read one at a time, so a folder of takes does not arrive in memory all
  at once.

### Fixes worth naming

* A held note released while the pointer was over another window stops sounding. A release nobody
  was listening for used to leave it on.
* Playing the same pitch again cuts the first one, and a voice stolen for a new note is the one
  that has been sounding longest rather than the newest.
* Unison no longer stacks the chiptune past full scale, and a narrow pulse stays audible at the top
  of the keyboard.
* A cut no longer changes how a clip sounds, and quitting during a take no longer loses it.
* Changing the audio device while a take is running is refused rather than attempted.
* Undo steps through a gesture only once the gesture has finished.
* A project folder is recognised through a difference of case, and Save As points an asset back
  outside the folder when it cannot copy it in.


### One bad bar no longer costs the whole take

* **Punch recording.** Mark a stretch from the ruler's menu — or take the cycle region wholesale
  with **Punch Over Cycle Region** — switch it on with the transport's punch button or `⌘P`, and a
  take keeps only what falls inside it. The region is washed over the timeline in red while it is
  on, beside the cycle's own wash and distinguished from it by colour, because they answer two
  questions about the same bars: what is played again, and what is written down.
* **The transport rolls out of the take by itself** at the punch-out — the one part of punching
  nobody can do by hand with an instrument in both. It watches for the playhead *leaving* the
  region rather than passing the punch-out, because under a cycle it never passes it.
* **A punched take removes what it lands on**, on its own track and only where the new clip
  covers, in the same Undo step. A clip spanning the region comes back as two. Recording without
  punch cannot do this and does not: nothing knows where an ordinary take will end until it ends.
* **Record is still pressed by hand.** Punch decides what a take keeps, not when one begins.
* **The file holds the whole take**, not the trim, so a punch set to the wrong bar has not thrown
  away what was played. A take that missed the region entirely says so — a different sentence from
  an empty take, which would send somebody to check a cable that is fine.

### You can hear yourself now

* **The I button on an audio track plays the live input through it** — or `U`, or Transport → Input
  Monitoring. It joins the mix where the track's own material does: through the effects, the fader,
  the pan and wherever the track is routed. A singer hears themselves through the reverb they are
  about to be recorded into, and a muted track stays silent.
* **It works with the transport stopped and without a take running**, because setting a level is
  what somebody does before pressing Record. Recording and monitoring are independent switches on
  one device.
* **What it costs is on the status line every time it is switched on**, not once in a dialog:
  software monitoring adds latency and an interface's own direct monitoring does not. Roughly 32 ms
  at a 512-frame block, on top of what the hardware costs. Use one or the other — both at once is
  hearing yourself twice, a few milliseconds apart.
* **A monitor that breaks up says how many times.** The input and output clocks drift, and once the
  gap stops being usable the monitor jumps to the live edge rather than replaying what you have
  already heard. A handful over a session is the clocks; a stream of them is a block size that is
  too small, and Settings → Audio is where that lives.
* The input meter now moves whenever the device is open rather than only during a take — a meter
  that appeared after the take began arrived too late to set a level with.
* Track headers are twenty pixels wider by default, for the fourth button. A column you have
  already dragged keeps the width you gave it.

### The project saves itself

* **A project with a folder is written back over itself about every thirty seconds**, when
  something has changed and no drag is part way through. Nothing is announced — the unsaved mark in
  the title bar going out is the feedback, and a status line that says "saved" twice a minute is
  one that never holds anything else. A save that *fails* is reported every time.
* **It never invents a place to save.** A document that has never been saved has no folder, and
  choosing one on somebody's behalf puts their song where they did not put it. That first save is
  still a question — but pressing **Record** on an unsaved project now *asks* it, rather than
  refusing with "recording needs a project folder". The answer to a dead end is a dialog.
* What it costs is written down rather than glossed over: this writes the real file, not a recovery
  copy, so **closing without saving stops being a way to undo an afternoon**. Undo still is, while
  the window is open. **Settings → General → Autosave** turns it off.

### Audio tracks can be recorded onto

* **`TrackKind::Audio` has said "Recorded or imported audio" since the beginning, and only half of
  it was true.** Nothing in the workspace had ever opened an input device. Now: click an audio
  track, press **Record** in the transport (or `R`), play, and stop. The take lands as a clip where
  the playhead was.
* **The take goes where you are looking.** The selected audio track is the target, and its **R**
  button is outlined to say so before anything is pressed — arming first was a button press that
  said what the selection already said. Filling that button in *overrides* the selection, which is
  the one thing selecting cannot say: record the vocal while you read the drum part. Clicking it
  off hands the aim back.
* **A take is written as it happens**, straight into the project folder's `Audio/`, by a thread of
  its own — not on the UI thread, where a dialog that blocked for a second would cost the take a
  second of audio. Takes are 32-bit float, and that is not a setting: every integer depth is a
  decision about how much of a performance to throw away before anybody has heard it, and float
  cannot clip.
* **Recording needs a saved project**, and says so rather than inventing a temporary directory.
  Every other asset can sit outside the folder until a save picks it up; a take has to be written
  the moment it starts, and somewhere the machine tidies up is not that place.
* **Where the take lands is stamped by the audio callback**, not by the button. The first block to
  arrive reads the playhead, so a take begins at the frame it actually began at rather than one
  callback earlier. The input and output run on separate clocks and nothing here corrects for the
  drift between them: a take a few frames long by the end of an hour can be seen and nudged, and
  one that has been quietly resampled cannot.
* **Frames lost to a slow disk are counted and reported.** A hole moves everything after it
  earlier, which is a thing to be told about now rather than to find in a mix next week.
* The take is named after its track and numbered from the first gap — `Vocals 3.wav`, not a
  timestamp — because the folder is something people read a year later.
* The input device is chosen in **Settings → Audio**, and choosing one does *not* stop playback:
  it is opened per take, so it was never the output stream's business.
* Not in this pass: **monitoring**. You hear the song you are playing along to, not yourself
  through Auris. An interface's own direct monitoring covers it in the meantime.

### A SoundFont has an envelope now

* The sampler carries the **same four controls the built-in instruments do** — attack, decay,
  sustain, release — and the same draggable graph above them. They shape the font rather than
  replace it: a piano's hammer is still a hammer, and now you can fade it in, hold it under its own
  decay, or let it ring long after the key has gone.
* **It is off until you switch it on**, with an `Envelope` toggle and nothing else. While it is off
  the four sliders do nothing at all — the mechanism is not neutral, it is skipped, and the font
  plays exactly as it is written.
* **Switching it on costs something, and the window says so.** A shaped note needs a MIDI channel
  to itself, because channel expression is the only per-note gain the library exposes: polyphony
  drops to fifteen notes, and a drum kit's choke groups stop working — an open hi-hat is no longer
  cut off by a closed one. A caution strip sits under the graph for as long as the switch is on,
  because missing polyphony shows up as "the top note of my chord dropped out", which is a symptom
  that gets blamed on anything but the switch that caused it. Turning it back off restores
  everything at once, mid-note if need be.
* Once it is on, the envelope owns the note. A release of zero means the note stops when the key
  does, and the graph draws exactly that — a vertical drop with no tail. The default is a fifth of
  a second, which is close to what a General MIDI patch already does.
* A shaped note at full level is **exactly as loud** as an unshaped one, and the envelope is
  square-rooted on its way into the channel because the format squares it again — the same
  correction the sampler already made for velocity, and for the same reason. A release stated as
  half a second is half a second.
* The envelope generator moved from `auris-synth` to `auris-dsp`, so both instrument crates fade a
  note with the same code. Two definitions of what an attack of five milliseconds sounds like would
  have agreed right up until one of them was corrected.
* The envelope graph now counts towards the window's height the way the equalizer's curve does, so
  a plugin that has one does not lose sliders to it.

### The equalizer has a curve you can grab

* The window drew the spectrum going in and left you to aim at it with twenty-four sliders. It now
  draws **the response the equalizer is making**, over that spectrum, with a node on each band that
  is switched in. Drag a node: sideways is the frequency, up and down is the gain, and both at once
  is the pair you were thinking about when you reached for it.
* **The wheel over a node narrows the band.** Q is the third number a band has and there is nowhere
  on a two-axis graph to put it; the wheel is where every equalizer with a curve has put it, and it
  costs no screen — the graph sits outside the scrolling list, so the wheel over it meant nothing
  before.
* A **high-pass or low-pass node moves sideways only**, and its node sits on the centre line: those
  shapes have a corner, not a level, and the gain a slider would let you set is a number the audio
  never reads.
* A band that is **switched off has no node** and is not on the curve. The toggle in the list below
  is what switches it in — a handle floating over a shape it has no part in is a control that lies.
* The curve is drawn by the same crate that makes the audio, from the same band table, at the rate
  the engine is running at. A frontend that drew it itself would be a second implementation of the
  cookbook filters, and the two would agree until the day one of them was corrected.
* The sliders stay underneath. A graph is how a shape is *found* and a number is how it is *said* —
  6 dB is a value you type, not a pixel you aim at — and neither answers for the other.
* The equalizer's window is drawn wider, and the graph does not count against the height at which
  the slider list starts scrolling: the picture would otherwise have cost a third of the controls.

### A plugin window is as tall as what is in it

* **The last control was cut through the middle** — a compressor showed six and a half rows. The
  window's height was worked out by counting the rows and multiplying, which left out the four
  pixels between each pair and the padding around them, and it came out twenty-seven pixels short.
* That number is now a *ceiling* rather than a size. The body sizes itself to the rows it holds and
  stops at the ceiling, so a figure that is too small can only nudge where the window opens instead
  of cutting a control in half.

### The plugin window no longer lets the pointer through

* A press over a plugin's controls was reaching **whatever was behind the window as well**. gpui's
  hit test walks every box under the pointer until one blocks, and the floating editor blocked
  nothing — so dragging a slider over the mixer moved the slider *and* the fader underneath it.
* Dismissing a right-click menu had the same fault: the click that shut the menu also landed on the
  arrangement behind it and moved the playhead to wherever the pointer happened to be.

### A composed song is re-takeable one clip at a time

* Every clip **Compose a Song** writes now carries the recipe that describes it, so the commands a
  clip from *Write a Part Here…* has always answered to — Another Take, Write It Again, the dial
  panel, Keep This One — work on a whole composed piece. Right-click the chorus bass, ask for
  another take, and the chorus bass changes while the verse bass and the chorus drums stay where
  they are.
* That one field is the entire feature. Nothing downstream had to learn what a composed song is:
  the menu, the inspector panel and the painter's "this was written" mark all read
  `Session::clip_recipe` and nothing else, so they all started working at once.
* The recipe is read off the part **as that section played it**, not off the roster — a chorus that
  patched the bass an octave up produces a clip whose recipe says so.
* Each clip gets a seed of its own, a stream of the song's named by the part and the stretch: it is
  reproducible from the specification, different for every clip, and six digits, because a seed is a
  number a person reads off a panel and types back in.
* **A recipe describes a clip; it does not reproduce it.** A whole song is planned with things one
  clip has no room for — how far a repeated section departs from its first playing, what leads into
  what, the arch of intensity across the form — so writing a composed clip again hands it to the
  one-clip writer, which knows the document's chords and not the plan. The part, the register, the
  density, the chords and the feel all hold; the phrase moves. The same is true of dragging its
  edge, which is the same request. Keep This One is how a take stops being at the mercy of either.
* The alternative was making the composer write its clips through the one-clip writer so the two
  agreed exactly. That trades the composer's output — section variation, the joins between
  sections, the intensity arch — for a button's arithmetic, and the output is the product.

### A clip can be looped

* **Drag the right edge of a clip's name bar**, or *Loop Clip* on its right-click menu, on the Edit
  menu, or **L**. The clip goes on saying itself for as long as the edge is pulled, in faded
  repeats divided by a hairline. Dragging back over the clip's own end stops it — the same gesture
  run the other way, rather than a second thing to know about.
* The **name bar's** edge, and only it. The edge below still resizes, so how long the phrase is and
  how many times it is played stay two separate things you can change. On a clip nobody has looped
  yet the two sit on the same pixel, which is what makes the gesture findable at all.
* A loop is a **length rather than a count**, so the last repeat is cut off wherever the edge was
  let go. That is what makes the drag continuous, and it means a loop can stop half way through a
  bar because that is where the next clip starts.
* Both kinds of clip. On audio the fades stay on the clip's own two edges and the joins between
  repeats run flat: a fade-out at the end of every pass would pump once a bar.
* The repeats are **flattened when the graph is built**, so the audio thread never learns that a
  clip repeats — it plays a list it cannot tell from a song somebody duplicated by hand. Exports
  carry the repeats too, WAV and MIDI both, since a MIDI file has no notion of a region that
  repeats and the notes are the only honest way to write one down.
* Splitting a looped clip leaves neither half looping, because the repeats were of a block that no
  longer exists. Duplicating one puts the copy past the repeats rather than on top of them.
* **`FORMAT_VERSION` is 10.** The field carries backwards on a default, which normally does not move
  the number — but a version 9 build would open a song whose drum loop runs thirty-two bars, play
  the one bar, and write that back on the next save with the other thirty-one gone. Refusing at the
  door is the only honest answer.

### Notes can be quantised

* **Quantise Starts (Q), Quantise Lengths, and Quantise Both**, on the piano roll's right-click menu
  and on the Edit menu. Nothing could put a played part back on the grid after the fact; snapping
  applied while something was being dragged and not one moment later.
* Three commands rather than one with a setting, because the two numbers a note has are separately
  wrong. A part played a shade ahead of the beat wants its ragged lengths evened out and its feel
  left alone, and doing both to a take that needed one is how it stops sounding like anybody played
  it.
* They snap to the **division the grid button is showing**, which is on screen above the notes being
  moved: quantising to a value nobody can see is a jump with no explanation.
* A length never rounds down to nothing. On a sixteenth grid a clipped grace note becomes a
  sixteenth rather than silence — a note vanishing because it was played crisply is not a
  tidying-up.
* The status line says how many notes actually moved, which is the one thing worth knowing
  afterwards: four out of twenty means the other sixteen were already where they should be.

### There is a metronome

* **The button beside the cycle button, Transport → Metronome, or K.** A click on every beat while
  the transport rolls, an octave higher on the bar line. The application has had a tempo map, a
  meter map and a bar ruler since it was written, and no way at all to hear any of them.
* It clicks the beat you **feel** rather than the one the meter is written in: a bar of 6/8 gets
  two clicks, not six. Meter and tempo changes are both followed, so a piece that moves into 7/8
  at bar nine has its accents move with it.
* The click is laid **over** the mix — past the master fader, past the master mute, past the meters
  — so it cannot be turned down by accident, it is audible with every strip muted, and switching it
  on does not move a level meter. It **never reaches an export**: playback and an offline render
  take the same code path in every other respect, and this is the one line that differs.
* Stored with the project, like the cycle region, because whether a piece wants counting in is a
  fact about the piece. Not an undo step, for the reason cycling is not: a practice pass is a run
  of toggles, and those would push the edits the pass was checking off the end of the stack.

### Cut, copy and paste

* **⌘X, ⌘C and ⌘V**, meaning notes in the piano roll and clips in the arrangement — the same three
  keys scoped to wherever the keyboard is, exactly as ⌘A and ⌘D already were. On the Edit menu in
  pairs, and on the right-click menus of both surfaces. Duplicate has existed since the beginning
  and only ever laid a copy down *next to* the original; there was no way to move material
  anywhere else at all.
* What is on the clipboard is a **shape** rather than a place. Notes keep the gaps between them; a
  block of clips copied off four tracks lands on four consecutive tracks wherever you aim it, and
  goes on doing so after the tracks it came from have been reordered or deleted, because it was
  never holding those tracks' ids.
* A paste lands at the playhead; *Paste Here* on an empty lane lands it under the pointer, which is
  the one place a paste has a position of its own. What arrives becomes the selection. A paste that
  fits nowhere — the wrong kind of track, or rows running off the bottom of the list — lands what
  it can rather than failing whole.
* Its own clipboard, not the system's. Nothing here reaches another application and nothing copied
  in one arrives here.

### A melody can be given an accompaniment

* **Right-click a clip holding a tune → Accompany This Melody**, or Compose → Accompany the Melody.
  Its key is worked out and written into the harmony lane, one chord per bar is written under it,
  and bass, chords and drums are added as tracks *beside* it. The melody is not touched. One undo
  step for the lot.
* The composer could write a whole song from a specification and could write one part from chords
  that were already there. What it could not do is the thing a person actually has in front of
  them: sixteen bars they played, and no idea what goes underneath.
* The key comes from correlating what the melody plays — weighted by note length and by how hard
  each is struck — against Krumhansl and Kessler's probe-tone profiles. Each bar takes whichever of
  the key's seven triads accounts for most of it, with a thumb on the scale for what the bar
  *arrives* on and a little inertia so the progression does not change on every coin toss. Nothing
  draws a random number, so changing one note and pressing it again says what that note was doing.
* **It will be wrong sometimes, and it is built to be argued with.** A melody is one voice: a tune
  in A minor and a tune in C major play the same notes, and a bar of passing notes reads as the
  chord it passes through. So everything it guessed goes into the harmony lane where it can be seen
  and retyped, and every part it writes carries a recipe — correct a chord, press *Write It Again*,
  and the band follows.
* Each part gets a fitting General MIDI sound where the shipped font is installed, and the built-in
  oscillators where it is not, which the status line says.

### The library list is readable

* **Every row carries a colour mark**, and the marks line up into a column down the panel. A plugin
  wears its category's colour and so does the heading above it. A font's sounds are banded eight at
  a time — General MIDI's own sixteen families, Piano, Organ, Guitar, Bass — which is what turns a
  hundred and twenty-eight rows of small grey text into something an eye can find a place in. The
  percussion bank is one band rather than sixteen: its patches are kits, not programs.
* Nothing depends on the colour. Every coloured row still has its name and its number beside the
  mark, the mark is never the text, and the hues are spread as far apart as their count allows and
  then walked outwards until each clears 3:1 against the surface it sits on — in all four schemes,
  which is checked rather than eyeballed. A fixed lightness put one group in ten at 2.7:1 on
  Midnight, because lightness is not luminance and the gap between them is widest across the hues.

### A note can be placed without holding anything

* **Create can be a plain click.** ⌘-click is Logic's, and it is still the default, but holding a
  modifier to write a note is a thing you have to be told — and the first person to try this
  without being told said so. The Pointer section of Settings now offers the bare click alongside
  the three modifier gestures.
* Choosing it moves the rubber band to ⇧-drag, and a click on empty arrangement then makes a clip
  rather than moving the playhead. The settings page says both at the moment you choose it, and
  they are why the modifier is still the default. ⇧ already means *extend the selection* on every
  other press, so the gesture was there to be used rather than invented for the occasion.
* **Deleting cannot be a plain click**, and is not offered. Creating on a bare click leaves
  something you can see and undo; deleting on one would remove every note you reached for, and
  would leave no gesture anywhere meaning "just this one".

### Sixteen more commands you can put a key on

* Mute, solo and duplicate for a track; duplicate, split-at-playhead and mute for a clip; select
  all, duplicate, and transposition by a semitone or an octave for notes; add a bus. Every one of
  them was already in a right-click menu and reachable from nowhere else, which meant that
  working from the keyboard stopped at the point of actually editing anything.
* **A command can now ship with no key at all** and still be in the list. Mute wants M, solo wants
  S, and the mixer and the structure lane hold both; inventing ⌥⇧K so the row had *something*
  would take that chord from whoever wanted it and bury the commands that earned their key. The
  row is there with a dash on it, and one press puts your key on it.
* ⌘A and ⌘D mean the notes in the piano roll and the clips in the arrangement — the same key,
  scoped to where the keyboard is, which is what the panel outline has been telling you all along.
  ⌥↑ and ⌥↓ transpose by a semitone, ⇧⌥↑ and ⇧⌥↓ by an octave, ⌥X splits a clip, ⌘B adds a bus.
* The settings page groups them as **Notes** and **Clip** rather than as a second **Edit**
  section, which is what a second run of the same group would have printed.

### Composing has a menu

* **A Compose menu**, holding the song sheet and the specification file. It was one row in the
  middle of File, between Open Project and Save — and that row carried the label of the
  *specification file* route while dispatching the song sheet, so the way in that needs no file
  was announced as "Compose from Specification…" and the file route was in no menu at all.

### The tune is a line rather than a walk

* **A third of every melodic interval the composer wrote used to be a fourth or wider.** That is an
  arpeggio's interval distribution, not a tune's, and it is why a composed melody sounded unnatural
  while the accompaniment underneath it — which is a function of the chord and so is right or wrong
  locally — sounded like players. It is now one in seven, against the one in five a corpus of real
  melodies gives for leaps of *any* size.
* The measurement, the literature it is read against, what each of the five rules is for and what
  is still wrong are in **`auris_compose::melodic`**, which is a page of documentation and no code.
  The constants in the melody writer are what it argues for; neither makes sense without the other.
* What changed: the restated figure is *joined* to where the last bar left off instead of restarting
  from its structural pitch — the single worst fault, and one nothing had chosen; the interval table
  is the corpus distribution and has an entry for a repeated note, which it did not; the walk has a
  memory, so a leap is filled in and a step tends to carry on; a dissonance left by a leap resolves;
  and a phrase ends on a chord tone with a beat of air after it.
* No chord and no note count moved in any of the four fingerprint fixtures, which is the report on
  the change: the pieces are the same pieces with a singable line in them. Existing projects are
  untouched — this writes new material and does not migrate old.

### Compatibility

* `Project::FORMAT_VERSION` is 14, from 4. Every document written by 0.2.0 opens with everything
  in it; nothing written by this release opens in 0.2.0, which is what the number is for. Ten of
  the bumps happened one at a time and each one's reason is written down beside the constant — the
  short version is that automation, buses and sends, drum recipes, clip loops, per-controller
  curves and tempo-following audio are all things an older build would have ignored on the way in
  and written away on the next save.
* A clip's `modulation` curve is now an entry under controller 1 in a map of `controllers`, and an
  automation lane records the stable key of the parameter it drives beside the id that addresses
  it. Both changed shape rather than gaining a sibling.
* `Session::import_audio` and `Session::import_soundfont` still do the whole job, and each is now
  also available in halves: `decode_audio` and `read_soundfont` read a file from any thread, and
  `Session::place_audio` and `Session::install_soundfont` put what they read into the document from
  the thread that owns it.
* `settings.json` has grown the window's last rectangle, the projects opened lately, the extra
  places to look for plugins, and the export's depth, rate and dither. `layout.json` has grown the
  set of controller lanes the piano roll has open. Both are read with defaults, so an older file
  opens and is written back complete.

## 0.2.0 — 2026-08-07

### What an adversarial read of the composer's harmony found

The music theory, gone through looking for chords the composer plays that nobody wrote. Every one
of these was silent: the wrong chord sounded, the document recorded the numeral that was asked for,
and nothing anywhere said the two had stopped agreeing.

* **A numeral means the same chord wherever it is typed.** Colouring built its chords by hand
  instead of asking the numeral, so a borrowed chord and a seventh took different paths to the same
  question and answered differently. The whole of it now goes through `chord_in`, which is the one
  place that knows what a numeral means.
* **A seventh comes from the key rather than from the triad it lands on.** `vii7` in a harmonic
  minor key came out half-diminished where the key builds it fully diminished — a distinction the
  triad alone cannot make, and the leading-tone chord is where it matters most.
* **A lead-in is a fifth above the tonic it arrives at, in every mode.** It was built from the
  scale's fifth *degree*, which in phrygian and locrian is not a fifth above anything: a
  modulation into a locrian section was prepared by a chord a tritone from where it was going.
* **`ii/V` is the supertonic of V.** Everything in front of the slash was thrown away and every
  applied chord came out as the dominant seventh of its target — `ii/V`, `vii/V` and `IV/V` all
  parsed happily and all sounded as V7-of-V. An applied chord is now read in the key its target
  would be the tonic of, which is what the notation has always meant. `V/x` still takes its
  seventh: the tritone pulling into the target is the whole point of writing one.
* **A sixth leaves the fifth under it alone.** `Major6` and `Minor6` both hold a *perfect* fifth
  and the sixth was handed out on the strength of the third, so `vii6` came out with a perfect
  fifth and was no longer diminished.
* **A section ends where its progression ends.** A four-bar progression under a six-bar section
  played bars 1–4 and then 1–2, stopping in the middle of the loop; it now plays the whole thing
  and fills from the *end*, so the section lands on the chord the progression resolves to.
* **The octave figure moves an octave.** The bass folded `root + OCTAVE` back into its range, and
  the range is two octaves wide with the roots in the upper one — so for four of the seven degrees,
  the subdominant and the dominant among them, the leap was subtracted straight back and the bass
  restruck the note it was already on.
* **The bass is the bottom of the arrangement.** The pad ran from C2 and the bass from E1, sharing
  sixteen semitones, so a pad voicing could put a chord tone *under* the bass note — an inversion
  nobody wrote, decided by whichever tone happened to fold lowest. The pad now runs C3 to C5, and a
  test holds every pitched role above the bass's floor. No part may read another's notes, so the
  ranges are where this has to be settled.
* A tie between two scale degrees now rounds down rather than off the top of the scale.

### Compound time is counted in dotted beats

* **6/8 is two beats, not six.** The grid divided the note the denominator names, which made a
  "sixteenth" in 6/8 a thirty-second — the grid came out twice as fine as everything placing notes
  on it believed. A step is now a fixed note value in every meter, and the *felt* beat is derived:
  six sixteenths to a dotted quarter. Every part that asks "am I on a beat" gets the answer the
  meter actually has.
* **The metric hierarchy no longer offers a compound beat a halfway point.** A dotted quarter
  divides in three and in nothing else; its midpoint is a syncopation against the meter rather
  than a position the meter offers, and weighting it as a beat handed real weight to the one step
  in 6/8 that most needs to be heard as a departure. Swing is off in compound time for the same
  reason: the shuffle is already there.
* **A groove is mapped onto the bar rather than wrapped round it.** The built-in grooves are one
  bar of 4/4, and under a 6/8 bar the pattern restarted partway through, putting a second downbeat
  where the bar has no beat at all; under 3/4 the turnaround simply never played. The bar's first
  beat now takes the groove's first and its last takes the groove's last, which is what a drummer
  does with a pattern in a meter it was not written for.
* **`six-eight` and `slow-blues`** are grooves written *in* compound time — in eighths of a dotted
  beat rather than sixteenths of a plain one — so a song in 6/8 or 12/8 has a two-beat and a
  four-beat idea to reach for instead of borrowing a four-beat one. A groove now carries how many
  steps make one of its own beats. Nothing picks them automatically; a song names them the way it
  names any other groove.
* The bass reads the kick the same way the drummer does. It was reading the raw step index, which
  wrapped a groove shorter than the bar and truncated a longer one — so in every meter the groove
  was not written for, the bass followed a kick the kit was not striking.
* A rhythm somebody writes by hand is still a repeating cell, because that is what writing four
  steps under a 4/4 bar means.

### The panels answer the pointer

* **The song sheet's dials follow the mouse again.** The sheet is drawn on an occluding overlay,
  and gpui's hit test stops at the first hitbox that blocks — so the root's pointer handlers, which
  every drag in the application is tracked by, never saw a move over the sheet. A dial could be
  pressed and would not turn.
* **The piano roll draws the rest of the track.** The bars either side of the clip being edited
  were empty grid, so there was no way to see what the phrase before it ended on or what the next
  one starts from without closing the roll. The neighbours are now drawn behind it, flat and faint
  — no velocity in the fill and no selection outline, because a ghost that read like a note would
  be an invitation to edit something the roll will not edit.
* **The mixer scrolls, and says so.** A flex item's `min-width` is `auto`, which is the width of
  its content — so a panel holding fifteen channel strips asked the dock for the width of fifteen
  channel strips and got it. Nothing overflowed, because nothing was ever too small; the strips ran
  off the side of the window where no scroll could reach them. There is now a scrollbar under the
  strips, drawn only when there is something to scroll, with the thumb draggable and the track
  clickable to jump.
* One picker row and one way to open a menu. The song sheet, the inspector and a plugin's choice
  parameters were three copies of the same control, and twenty-eight call sites each wrote out the
  same eight lines to open a context menu.

### A section can change how a part plays

* **`[section.chorus.part.lead] octave = 6`.** A part was one setting for the whole song: whatever
  density, octave, gate and subdivision the roster gave it, it played that way from the first bar
  to the last. A section can now patch any of those, plus `rhythm` and `note` — the lead an octave
  up in the last chorus, the hat on sixteenths in the bridge.
* A **patch**, not a second declaration: what it does not name it does not touch, so a busier
  chorus is one line and adding a field to a part does not silently reset it in every section that
  tweaks one.
* The resolution happens once per part per section, and every pass reads it. That is the whole of
  the change and the only part of it with a trap: `shorten` and `humanise` run over the finished
  part *after* every section has been written, so a gate or a subdivision read off the roster there
  would be the one kind of per-section field that silently does nothing — a chorus cut to the
  verse's note lengths, or a section on triplets having its swing measured against sixteenths.
  Both are pinned by tests that fail when the resolution is taken away.
* Not patchable, by construction: the name, the role, the instrument, the program, the level and
  the pan. Those are not how a part plays, they are what its *track* is — one row, one instrument,
  one fader for the whole song. A chorus on strings where the verse was on a piano is two parts and
  the section roster is what brings each of them in. The line is not waiting to be lifted: a track
  that changed instrument half way through would have to be two tracks, and then it was two parts
  all along.

### A key change is arrived at rather than stumbled into

* **The last chord before a modulation becomes the dominant of the key being arrived at.** A
  transposed section used to begin and that was all: the piece stepped sideways and a listener
  heard the join as an edit. A `V7` names its tonic before that tonic has sounded, which is why
  every arranger reaches for it first and why nothing else does the job.
* One event, replaced in place — the section keeps its bars and its clips keep their lengths — and
  only where the key actually changes, so a piece that does not modulate is untouched whatever the
  field says. It runs *before* the melodic skeleton is chosen, or the tune would be the one part in
  the band still playing the chord that used to be there.
* The section keeps its own key and the chord is renamed against it, exactly as a borrowed chord
  is. The lane draws one key change, at the bar where it happens, with a chromatic chord leaning
  into it — not a second modulation half a bar early.
* This is the one thing in the format that rewrites a bar of a progression quoted by name. The
  trade is deliberate and `lead_in = "none"` refuses it: a modulation is a structural instruction
  asked for by hand, it outranks a chord chart, and there is no way to prepare a key change without
  changing the chord that prepares it.
* The composer's fingerprint test now compares chords rather than the text of their names. A
  numeral knows which letter its degree demands and a chord only knows whether its key leans sharp
  or flat, so B♭ and A♯ are one chord written twice — and the test flagged that as the two
  disagreeing.

### No bar takes the wheel

* **Sliders no longer answer a scroll.** Faders, sends, plugin parameters, clip dials, the song
  sheet's dials and the zoom sliders all took one. Every one of them sits inside a panel that
  scrolls, so rolling down a column of tracks changed the level of whichever fader the pointer
  crossed on the way — silently, with no drag to remember having started, and nothing on screen
  saying which one moved. A bar is swept with the pointer, and that is now the whole of how it is
  edited.
* The handler is gone from `value_slider` and `zoom_slider` themselves rather than from their
  callers, so there is no parameter left to pass one to and it cannot come back one control at a
  time. Scrolling still means scrolling everywhere it used to — the arrangement, the roll, the
  keyboard, the automation strips — and zooming by wheel still works over the timeline and the
  roll, which is where it was always reached for.
* The transport bar's tempo and signature readouts keep theirs. They are typed fields in window
  chrome rather than bars, nothing behind them scrolls, and the wheel there is a documented way to
  reach a near neighbour.

### A section can play at a tempo of its own

* **`[section.chorus] tempo = 132`.** A composed piece ran at one speed from the first bar to the
  last: the specification had a single `tempo`, and the whole thing arrived at the document as
  `set_bpm`. A section now names its own, the composer hands over a `TempoMap` rather than a
  number, and the changes are on the timeline's tempo lane where they can be dragged like any
  others. A point is written only where the tempo actually changes, on the same rule the key lane
  already followed.
* The wander follows it. Humanisation asks for a scatter in *milliseconds* and has to convert that
  into ticks, which needs a tempo — so the conversion is now per section. A chorus lifting from 60
  to 180 would otherwise have been scattered by the verse's number of ticks, which is three times
  the time the dial asked for, and that is exactly the failure the millisecond conversion was
  written to stop. `ScoreSettings` no longer carries a tempo at all: it lives on the section plan,
  in one place, so the two cannot disagree.
* It is a **step**, and that is stated rather than glossed. A ritardando slows *through* a passage;
  a section is a stretch of bars. Neither the specification nor `TempoMap`, which is
  piecewise-constant, can express a continuous change, and none of this pretends to.
* The meter is still one for the whole piece. Unlike the tempo, changing it changes the length of
  a bar, and every part is written against one grid.

### A section chooses who plays it

* **The song sheet can sit a part out.** `[section.x] parts = "…"` has been in the format the whole
  time and worked end to end, and there was no way to reach it without hand-editing a `.asong` —
  so a piece composed from the sheet was the same roster from the first bar to the last, however
  long it ran. Every section row now has a `7/7` button listing the roster with a tick against the
  parts that come in.
* The rule the button obeys is not a set toggle, and could not be. An empty list means
  *everything*, so switching the hat off in a section that names nobody has to write down the
  other six rather than remove a name from an empty list — which is what a plain toggle would do,
  and it does nothing at all. Turning the last one back on says everybody again rather than listing
  them, or the section would go on naming six when a seventh part is added and would be the one
  section that new part silently does not play in. The last part left cannot go: a section playing
  nothing is silence, and the spelling for it is already taken.

### Eight presets are eight draws

* **Every preset ships a seed of its own.** All eight left it at the default, so the shipped songs
  were eight arrangements over *one* set of random numbers — the same figure fell in the same bar
  of every piece, and hearing all eight was hearing one draw eight times. Which numbers they are
  does not matter and nothing claims it does; a test pins that no two are the same and that a
  ninth preset added without one fails rather than quietly rejoining the pile. Checked across
  seeds 0 to 8 of every preset: no draw loses a part, and none of the 72 reaches the master
  limiter, which stays where it was put — dormant.

### A cymbal marks where the form arrives

* **The kit has a crash.** A composed piece had nothing at the joins of its own form: the section
  changed and the only thing that marked it was a snare fill running into a bar that sounded like
  every other bar. `crash` is a new part role, and the writer behind it reads the *form* rather
  than a groove — it strikes the downbeat of a section that arrives at something at least as
  strong as the one before it, and stays silent where the arrangement is coming back down. The
  shipped pop form gets three: into the verse and into each chorus, and none on the verse after a
  chorus or on the outro. Six of the eight presets carry one. `DrumVoice::Crash` had existed the
  whole time and every groove returned an empty pattern for it, because a bar-long loop is the
  wrong shape for a thing that happens once a section.
* **The built-in cymbal is voiced as one.** `auris.synth.noisedrum` is a tom — noise through a
  band-pass swept down from where the note puts it — and at its defaults a part striking 49 came
  out at a spectral centroid of **342 Hz, ringing 595 ms**, which is a low tom under the name of a
  crash. It is now 3.6 kHz and 945 ms. A composed track can carry plugin parameters for the first
  time, and this is the only thing that uses it: opening the filter that far let through 13.5 dB
  more than the built-in snare across the first 300 ms, so the voicing carries the level that puts
  it back. The five General MIDI kits the presets use already place their crash within 1.4 dB of
  their own snare, and both sides are then separated by the same role gain.
* Worth knowing, and deliberately *not* changed: the rest of the built-in kit is the same
  algorithm at the same defaults, told apart only by which note each part strikes — measured, the
  kick, the snare and the hat sit at 190, 215 and 246 Hz, three thuds within 56 Hz of each other,
  and nothing about 246 Hz is a hi-hat. That is what the one preset on the built-in voices has
  always sounded like, and revoicing it is a decision about a preset rather than part of adding a
  cymbal.

### The composer keeps time, and a velocity means one thing

* **The kit does not wander.** Timing humanisation applied to every role including the drums,
  which is not what a kit does — the shipped presets scattered theirs by 4.9 to 14.0 ms, with
  single hits reaching 28.8. The kick, the snare and the hat now sit exactly on the grid. They
  keep their *lean* — the hat a little early, the snare a little late, the same whole number of
  ticks in every bar — because that is a player leaning and not a player being unreliable.
* **The dial reaches zero, and means the same thing at any tempo.** The wander was
  `6 + 19 × humanize` ticks, and the six was multiplied by nothing, so the dial was a step
  function with no setting between "quantised" and "±6 ticks". It is now **15 ms at the top of
  the dial**, converted through the tempo — so ambient at 64 BPM stops being three times looser
  than rock at 148 for no reason anybody chose. A generated clip reads the tempo underneath it
  rather than assuming 120.
* **A velocity means the same thing on every instrument.** The built-in voices were linear and
  the SoundFont sampler was squared, which is the SF2 default and what rustysynth implements —
  so the composer, which writes velocities for a linear instrument, got twice the dynamic range
  in decibels through the font. A part written MIDI 26 to 126 measured **27.4 dB through the
  sampler against 13.7 through a built-in voice**; it is now 13.8. This is a deliberate
  disagreement with other SoundFont players: a DAW where one number means two things depending
  on what is loaded is worse than one that is consistent with itself.
* **A composed piece is audible.** The sampler was voiced 11.5 dB below the rest of the
  application, so a composed mix landed 14 to 19 dB under a finished record. Composed mixes are
  now **13 to 16 dB louder**. They are still 1.5 to 8.3 dB short of a mastered piece, which is a
  crest-factor problem — arrangement and bus compression — and not something a gain constant can
  reach.
* **The shipped font's drum kits are brought level with each other.** They sit 7.95 dB apart at
  unity, which is calibration noise rather than a musical statement, and once everything got
  louder that error landed above full scale — city-pop clipped once a bar. A measured per-kit
  trim is applied where a composed part resolves to a kit, and a composed document carries a
  limiter on its master at −0.3 dB: dormant on 121 of 128 seeds of the one preset that needs it,
  and never touched by any other.
* Existing projects that use the sampler will be about 12 dB louder and half as wide in decibels.
  Nothing needs converting; the faders are where they always were.

### What a review of the whole thing found

Nineteen defects, from an adversarial read of every crate. The pattern worth naming is
that most of them are *asymmetries* — one branch of a pair doing the right thing while
its sibling does not, with a test on the correct half and none on the other.

* **A tempo change no longer erases the ones before it.** A tempo event at tick 0 in a
  later track — which format 1 files write routinely — threw away every change already
  read, and a file whose first tempo arrived partway in played its opening bars at that
  tempo instead of the default. The time-signature branch beside it had both cases right.
* **Stopping the transport lets go of the vibrato.** `Fm2::reset` zeroed the modulation
  wheel without re-deriving the depth it feeds, so a Stop, a Seek or a Panic taken mid
  curve left every later note swinging about fifty cents from a wheel nobody was holding.
  The chiptune never had this.
* **The composer writes down the chord it actually played.** Colouring rewrote the chord
  and left the numeral, and the numeral is what gets stored — so the harmony lane painted
  `Fm` over parts playing F♯ minor, and a generated Chords clip wrote the lane's version.
  Not one note moves; what changes is what the document says about them. The ambient
  preset was the same fault written by hand: `IVmaj7` in C lydian is F♯maj7, a tritone
  from the tonic, and it now uses the mode's own chords.
* **Trimming the front of a short clip does nothing instead of the wrong thing.** A clip
  shorter than the editing grid had its own floor applied as a ceiling, so touching the
  front edge moved it left, made it longer, and in the first bar drove its start negative.
* **An instrument takes its automation with it.** Swapping a track's instrument cleared
  the saved parameters and left the lanes, which bind by track and raw parameter id — so a
  curve drawn for one plugin swept an unrelated control on the next, in the exported file
  as well as in playback. An audition of a second SoundFont preset still keeps its curves.
* **A missing sample is no longer replaced by any file wearing its name.** The search
  passed no expected size, so the first match on name alone was adopted and written into
  the document. `AudioSource` now carries the fingerprint the SoundFont reference always had.
* **Save As takes a collected SoundFont with it.** A font stored inside the project folder
  was carried across as a reference to a file that was not there, and the copy opened
  elsewhere silent — with Collect Assets then answering "nothing to do".
* **A saved file carries the version of the build that wrote it**, rather than the version
  it was loaded with.
* **A muted track lets go of what was played into it.** Auditioning into a muted track
  filled a queue nothing drained, discarding note-offs once full and leaving voices
  sounding after the unmute.
* **The curve lane's grab zone is seven pixels at any scroll.** It was measured as a
  position rather than a length, so five bars along it had swollen to most of a bar: a
  press on empty strip seized a distant point, and a second point could never be added.
* **The arrangement lets go of a deleted clip and takes hold of what is drawn.**
  Alt-clicking a clip out of a swept selection left a dead id behind, which surfaced later
  as a failed Duplicate; and the rightmost column of a section's grab bar did nothing.
* **⌃⌘ chords can be bound on macOS**, and Ctrl+Win chords off it — both dropped a modifier
  and stored a chord the user had not pressed. The settings footer now shows ⌘S where it
  used to print `secondary-s`.
* **An "inside" asset path cannot escape the project folder on Windows.** A drive prefix in
  a hand-edited or shared document walked out of the folder the way a leading slash does.
* Counts in the README, the guide and `CLAUDE.md` that the code contradicted.

### A slash bass keeps its accidental

* A numeral's slash bass now carries an accidental of its own, so `v/b7` is a symbol a chord can
  be stored as. Without it, respelling a progression into a key whose scale is not the major's —
  harmonic and melodic minor, dorian, phrygian, locrian — resolved the bass as that key's own
  unaltered degree, and `@junjo` in A harmonic minor came out as `Em/G#` where it should be
  `Em/G`: a bass part sounding a minor second against the chord's own third, and the wrong numeral
  written into the document.
* **`Project::FORMAT_VERSION` is 9.** A version 8 build has no reading for the accidental — it
  falls through to the secondary-dominant branch, finds no roman numeral, and rejects the numeral,
  which fails the whole document rather than the one chord. The version is what makes that happen
  at the door rather than halfway through a harmony lane.

### A General MIDI SoundFont comes with it

* **Auris Studio now ships with MuseScore General**, 128 instruments and a percussion bank under
  the MIT licence. It is in the library panel from the moment the window opens, with nothing to
  import. Two oscillators and a noise drum were enough to hear the engine working and never enough
  to write anything, and "install a SoundFont from somewhere" is not a first five minutes anybody
  enjoys.
* Not in this repository, because the file is two hundred megabytes — more than GitHub accepts in
  one piece and far more than every clone of a source tree should carry. `tools/fetch-soundfonts.sh`
  downloads it, checks it against a SHA-256, and installs it where the application looks; the
  release workflow runs the same script before it assembles each archive. What is version
  controlled is the manifest, in `auris_session::library`, which is the part worth reviewing.
* The script asks `auris soundfonts --manifest` what to fetch rather than keeping its own copy of
  the list. A URL recorded twice is a URL that goes stale in one of the two places.
* Putting the font in the document is deliberately *not* an edit: no undo step, no dirty flag, and
  a new project nobody has touched is still unmodified. It is what this installation has, the same
  way the built-in instruments are, and neither belongs in a history of what somebody did.
* The search for an asset that has moved now covers the library directories, so a project saved on
  one machine and opened on another finds that machine's copy of the shipped font and writes the
  new path back. The reference most likely to break when a project is sent to somebody else is
  also the only one that always has an answer.
* A build with nothing installed starts, runs and composes on the built-in instruments — which is
  what CI does on every commit.

### The modulation wheel goes all the way through

* **View → Modulation** (`⌘⌥W`) puts a second strip under the piano roll, beside the bend. A clip
  carries the curve itself, the engine schedules it, the instruments answer it, and a `.mid` takes
  it out and brings it back as controller 1.
* One set of gestures and one painter for both strips. They differ in exactly two things — the bend
  goes both ways from a line across the middle, the wheel goes up from a floor — and two copies
  would have been two chances for the wheel to behave differently from the bend for no reason
  anybody could see. The same goes for the four session commands, which now take *which* curve.
* A clip's bend is now a `CurvePoint` list shared with its modulation, so the stored field is
  spelt `value` where it was `semitones`. **`Project::FORMAT_VERSION` is 8**: a version 7 document's
  bends would otherwise read as zeroes, silently, because the field has a default — and a slide
  somebody wrote would simply stop happening.
* Like the bend, a modulation curve that does not end at zero is let go before the clip ends. Both
  are channel state, and a clip finishing with the wheel up would leave everything after it
  wobbling.

### The built-in instruments have a vibrato

* **Vibrato Rate**, **Vibrato** and **Mod Depth** on the chiptune and the FM voice, and
  `NoteEvent::Modulation` — MIDI controller 1 — for a wheel to reach them by. The sampler passes
  it to the font, which is where a General MIDI set already has it wired to a vibrato of its own.
* `Vibrato` is zero by default, so a patch nobody has touched sounds exactly as it did before this
  existed and every piece already written is unchanged. `Mod Depth` is *not* zero — half a
  semitone, what a mod wheel does on almost every synthesiser ever sold — because a wheel that
  does nothing until a parameter is found is a wheel nobody discovers.
* One LFO per voice, restarted at each note on, so a chord struck together wobbles together. A
  single instrument-wide one would have every note somewhere different in its cycle, and the chord
  would arrive detuned by however far the wheel happened to be up. It keeps running while the
  depth is zero, so turning the wheel up mid-note picks the cycle up rather than jumping.
* A modulation rate now reads as `5.5 Hz` rather than `6 Hz`: below hearing the useful range is one
  decade wide, and rounded to a whole number half of it reads the same.

### The log has somewhere to go, and the release build has no terminal

* **View → Log** (`⌘⌥L`) opens a panel holding the last five hundred records the application
  wrote. Off by default, remembered in `layout.json`. A DAW is meant to fail quietly — a moved
  SoundFont costs one track its sound rather than the session — and every one of those quiet
  failures was logged to a terminal nobody was looking at. A track went silent and said nothing.
* Newest first, because the reason anybody opened it is the thing that just happened. **The log's
  status-bar icon turns amber** while there is a warning or an error nobody has read, which is the
  only part of this a person who never opens the panel will see.
* **A release build no longer opens a console window.** `windows_subsystem = "windows"`, so
  double-clicking `auris-studio.exe` gives the window and nothing else — where before it gave a
  black terminal beside the application, and closing that terminal closed the application. A debug
  build keeps its console, because `cargo run` and `RUST_LOG` are how this is worked on.
* The recorder sits in *front* of `env_logger` rather than instead of it, so the terminal and the
  panel can never disagree about what was logged.

### Eight whole songs to start from

* **Style** is the first row of the song sheet, and choosing one fills the rest of it: `chiptune`,
  `pop-band`, `city-pop`, `rock`, `jazz-trio`, `orchestral`, `synthwave`, `ambient`. Around thirty
  dials was a lot to be asked for before anything had made a sound, and knowing which of them
  matter is exactly what somebody opening a composer for the first time does not know.
* A style replaces the *whole* sheet — tempo, key, groove, progression, form and roster — because
  half a style is the arrangement of one at the tempo of another, which is not a style at all.
* `auris presets` lists them and **`auris compose --preset city-pop`** writes one with no file at
  all. Every other option means the same thing either way, because a named style and a file both
  arrive as the same text.
* Each preset is a `.asong` document embedded in the build rather than a structure assembled in
  code. A preset is meant to be read, the format was designed to be the readable one, and it makes
  the presets parser tests that fail loudly rather than silently.
* The part row's instrument picker now offers the General MIDI sounds, grouped into the sixteen
  families the standard already divides them into — a hundred and twenty-eight names in one menu
  is a menu nobody can read, and it would be taller than the screen. A drum part is offered the
  eight kits instead. Choosing a plugin clears the program, so the row never says one thing while
  the piece plays another.

### A composed part can ask for a real instrument

* **`program = "String Ensemble 1"` in a `.asong`** puts that part on the shipped SoundFont. By
  name — read case-, space- and punctuation-insensitively — or by number, for anybody working
  from a font's own listing. The composer had no way to name a SoundFont sound at all: an
  instrument was a plugin id, and a SoundFont's sounds do not have those.
* A part may carry `program` *and* `instrument`, and that is deliberate. The program is played
  where there is a font to play it from and the plugin is the fallback where there is not, so a
  specification asking for strings on a build with no library comes out as an oscillator rather
  than as silence — and the compose report names the missing library, so it is clear why.
* **On a drum part the same field is a kit**, because in General MIDI it is: the patch selects the
  whole kit and the note number picks the drum. Which of the two readings a number gets is never
  guessed at, because the role has already said — and a kit writes itself back out as
  `"TR-808 Kit"` rather than as whatever guitar shares its number.
* `auris compose --print` and the composed-track listing now name the *sound*, not the plugin the
  part would have fallen back to.

### A composed song arrives as a whole document

* **A composed piece now carries its own harmony and its own structure.** Both were computed and
  then dropped: the composer resolves a key and a full chord progression per section and names
  every stretch, and a composed song opened with an empty harmony lane and an empty structure lane
  over a piece that plainly has chords and sections. Worse than cosmetic, because a clip generated
  afterwards *reads* both — a part added by hand to a finished song had nothing to agree with.
* A key change is written only where the key changes, so a song in one key throughout has one
  point rather than one per section. Past the last bar the harmony and the structure both end,
  rather than the final chord and the outro running on for ever.
* **The drums can be asked for one voice at a time.** `Kick`, `Snare` and `Hat` join `Drums` in
  the part picker, so a hi-hat can be written onto a track of its own. A kit on one track is three
  voices no fader can separate; three tracks is a mix. `Project::FORMAT_VERSION` is 7.
* **A composed piece arrives with a rough mix.** The kit goes under one drum fader, the pitched
  parts share a room fed by sends, and the parts are spread across the stereo image instead of
  stacked in the middle. What stays centred is what a listener localises the song by — the tune,
  the bass and the kick — and nothing goes hard over, because a part at the edge of the image
  disappears on a phone. It is not a substitute for mixing; it is the ten minutes a person would
  have spent setting up before they could hear whether the piece was any good.
* More room means further away, which is the whole of the send ordering: the pad is furthest back
  because being a wash is what makes it a bed, and the tune is nearest. The bass and the kick get
  none at all — low frequencies in a reverb are mud. The reverb on the bus is set fully wet, which
  is the one setting a send/return reverb cannot be left at its default for.
* Not a note of what the composer writes moved: its tests compare whole pieces chord by chord and
  note by note, and they pass unchanged.

### Tracks can be dragged into order

* **Drag a track header up or down** to move it in the list. The arrangement reorders as the
  pointer moves rather than drawing a line and jumping on release, so what follows the hand is the
  arrangement itself. The whole drag is one undo step and one graph rebuild — a reorder is
  structural, and rebuilding on every pointer move would instantiate every plugin in the project a
  hundred times across one gesture.
* A press that does not travel is still a selection, and a press that lands on the header's fader,
  pan or mute keeps its own gesture: the header is the fallback grab, not the first claim.
* Only the list moves. Automation lanes, a routing output and a send all name a track by id, so a
  bus can end up above the tracks feeding it and nothing about the mix changes.
* **Fixed: an open automation lane pushed every header below it out of register with its track.**
  The lane column grew a row and the header column did not. The headers are now built from the same
  row walk the canvas uses, so the two cannot disagree, and the band beside an open lane carries
  the automated parameter's name.

### Buses and sends

* **A track no longer has to go to the master.** Its output is the master or a **bus**, and it can
  carry any number of **sends** — taps that feed a bus *as well as* wherever it goes itself. One
  reverb shared by six tracks is six sends; one fader over a whole drum kit is six outputs.
* A bus is a track kind rather than a thing of its own, so it has a fader, a pan, a mute, an effect
  chain, a colour and an automation lane without any of them being written twice, and every command
  that addresses a strip by track id addresses it too. What it has instead of clips is whatever is
  routed into it.
* Every mixer strip says where it goes; clicking that name offers the legal destinations, and the
  **+** beside it adds a send. A send's level is a mixer control like a fader — it drags, takes the
  wheel, resets on a double click and can be automated. Right-clicking one moves its tap before the
  fader or takes it away.
* **Solo travels both ways along the routing.** Soloing a drum track leaves the drum bus open, or
  its audio has nowhere to go; soloing the drum bus leaves the drum tracks open, or a thing with no
  material of its own plays silence.
* A route that would loop back on itself is refused, and the picker only ever offers destinations
  that would not. A file that holds one — nothing here can write one — is repaired on open with a
  line in the log, rather than refused.
* **Plugin delay compensation now follows the routing rather than the track list.** A limiter on a
  bus holds back the tracks that do *not* pass through it. Each outgoing copy of a track gets a
  delay of its own on top, so a track feeding the master dry while sending to that same bus has the
  dry and the wet arrive together instead of comb-filtering each other. Effect tails add up along a
  path the same way, so an export of a track ringing into a bus ringing into the master keeps going
  for all three.
* `Project::FORMAT_VERSION` is 6. A version 5 file opens; a version 6 file does not open in an
  older build, which is the point — the fields would be *ignored* rather than rejected, and a mix
  where six tracks feed one reverb would come up with all six routed dry and be saved back that way.

### MIDI files go in and out

* **File → Import MIDI File…**, or dropping a `.mid` on the window, reads it as a new piece: its
  tempo map, its meter, and one track per part. A new document rather than tracks added to the open
  one, because a MIDI file brings its own clock — its notes in a piece running at a different speed
  would be the right notes at the wrong lengths, with nothing on screen to say why. Unsaved work is
  asked about first, exactly as it is for Open.
* **File → Export MIDI File…** writes the other direction, at 960 ticks to the quarter note, so a
  piece that leaves and comes back has every note in the same place. A tempo does not survive
  exactly: MIDI stores whole microseconds per quarter, so 144 bpm returns as 143.999 88, while 96
  and 120 divide evenly and return exact.
* Four things real files do that a naive reader gets wrong are handled and tested: a note-on at
  zero velocity is a note-off; the same pitch struck twice before either release is two notes; a
  note nobody released is closed where its track ends; and two channels in one track are two
  instruments, which is what a format 0 file always is. A part on **channel 10** gets the
  noise-drum instrument.
* A file counted in **SMPTE frames** is refused rather than guessed at. It has no beats, so it has
  no bars, and putting it on a musical timeline would mean choosing a tempo on its behalf.
* What a `.mid` has nowhere to put, in either direction: audio tracks, the mixer, which instrument
  each track plays, and the automation.
* `MidiClip::playable_notes` is new, and the renderer now asks it too. Which notes a clip actually
  plays was written inline in the scheduler; a second copy in the exporter would have drifted into
  a file that is not the piece you can hear.

### Two things the backend could already do and nothing could ask for

* **A track's colour can be chosen.** It was picked from a palette by the track's position and
  then fixed there for good — and the order tracks were made in has nothing to do with which of
  them are drums. The track's right-click menu now offers the palette as swatches. Numbered rather
  than named, because the set holds two entries a reasonable person would call orange.
* **A whole track can be frozen.** *Keep Every Take Here* drops every recipe on it, so nothing on
  that track is written again when the chords underneath change. `Session::freeze_track` had been
  implemented and tested for some time with no way to reach it; the clip-level command was the only
  one on a menu. The status line reports how many clips it acted on, because a track reaches
  further down than the panel shows.

### Parameters move along the timeline

* The document holds **automation**: a curve per parameter, beside the tempo, the meter, the key
  and the chords. Right-click a track header for *Automate Volume* or *Automate Pan* and a lane
  opens under it; a press on empty lane writes a point and starts dragging it, the delete gesture
  takes one off, and a drag is one undo step.
* A parameter with no lane is **not automated at all** and keeps its stored value. Only an existing
  lane takes over, which is what lets a mix be automated one control at a time — and taking the
  last point off gives the parameter back.
* A lane is **not anchored at the start of the song**. It holds its nearest value flat outside the
  stretch it was written over, because it makes a claim about that stretch and none about the rest.
  A tempo has to be defined from the first sample; a filter cutoff does not.
* A lane carries how to get between its points. A fader runs in a straight line; a parameter with
  discrete positions **holds**, because interpolating a waveform chooser would sweep through every
  option between two settings and sound all of them on the way.
* Playback and export take the same path. Seeking or looping arrives at the values under the
  playhead rather than sliding to them — landing in the middle of a fade used to swell up to it
  from wherever the fader had been left.
* `Project::FORMAT_VERSION` is 5. This is a new field with a default, which normally does not move
  the version, but the direction that matters is the other one: an older build ignores a field it
  does not know, so it would open an automated mix, play it at the wrong levels, and write those
  levels back on the next save. Refusing to open is the only honest answer.
* `ParamTarget` moved from `auris-session` to `auris_core::param`, because a lane is a target and
  a shape and the document may not name a crate above it. `auris_session::param` re-exports it, so
  the old path still resolves.

### Files can be dragged into the window

* An audio file dropped on the window arrives on a **new audio track**; an `.sf2` goes on the
  library's shelf with the font opened where its sounds are chosen; a `.auris` project **opens**.
  All three were reachable only through a File menu that a person has to already know is there.
* A dropped project goes through the same unsaved-work guard the Open command does, and the guard
  carries the dropped path — answering *Save* saves what is open and then opens the one that was
  dropped, rather than saving and then asking again which file was meant.
* A project has to be dropped on its own. It is a document rather than something that goes into
  one, so a drop holding a project and three takes has no reading that does not risk the work on
  screen — import into a document about to be replaced, or replace the document the takes were
  meant for. Two projects have the same problem and no tie-break at all, so the whole drop is
  refused with a line saying why, and the border stays dark while it is still in the air.
* A drop is understood by what the file is rather than by where it was let go, so there is no
  target to aim at — the window takes it over the lanes, the mixer or the library alike. What the
  position decides is when: audio dropped on the lanes starts there, snapped to the grid the way a
  dragged clip is, and audio dropped anywhere else starts at the playhead.
* Several files at once are read in the order they were dragged, one at a time with the status
  line naming each, and a drop that only partly arrives says how many did and how many did not. A
  border lights up while a drag holding something readable is over the window, so a folder or a
  PDF says beforehand that it will not be understood.
* Importing audio now scrolls to the track it made, from the File menu as well as from a drop. On
  an arrangement taller than the window it was landing out of sight.

### Both edges of a clip are handles

* The pointer becomes a ↔ over one, so the grab can be seen before it is tried. Nothing on screen
  said the edges could be taken hold of, which made the whole gesture something you had to already
  know about. The zone the arrow lights up is the zone the press acts on, tested rather than
  trusted — including the band an audio clip gives to its fade handles, which the arrow stays out
  of because a press there takes a fade instead.
* A note's end in the piano roll gets the same arrow, and holds it back while the velocity tool is
  in hand, since that tool drags a note's velocity rather than its length.

* Dragging a clip's **left** edge trims its front instead of moving it. An audio clip's window
  walks into its source, so the material stays where it sounds and dragging back out uncovers what
  was hidden; a played clip's notes are rebased, keeping the sounding half of anything the cut runs
  through.
* A clip that **wrote itself** is written again at its new length, from either edge. It used to
  gain a tail of silence when pulled out and keep notes hanging past its own end when pulled in.
* An audio clip's edge now stops where its material does. Past the last frame it drew — and
  saved — a stretch of silence with the waveform ending part way, which the renderer clamped
  anyway: the picture and the sound disagreed.

### The time signature changes along the song

* The document's one time signature is now a map over the timeline, beside the tempo map, the
  harmony and the structure. A change is written from the ruler's right-click menu and lands on a
  bar line — a meter beginning mid-bar would leave that bar with no length and every bar number
  after it uncountable — and the ruler, the grid, the piano roll, the position readout and every
  command that counts bars follow it.
* The transport bar's centre now holds three readouts rather than two: position, tempo and
  signature. The signature shows the meter the playhead is in; clicking it drops the common
  meters, with *Other…* for anything else the notation holds.
* Editing the meter moves the bar lines and not one sample. Notes, clips, chords and sections are
  stored in ticks, so nothing under the ruler moves when the ruler is renumbered.

### The command palette does more

* Four commands that only the mouse could reach are now bindable, on the menus and in the palette:
  Tempo…, Time Signature…, Next Grid Division and Go to Position…
* The palette can set a value and not only fire a command. Type `1/16` for the editing grid, `6/8`
  for the meter, a colour scheme's name, or `日本語` to switch language — the languages listed in
  themselves, since the person opening that list is the one who cannot read the current one.

### Compatibility

* `Project::FORMAT_VERSION` is 4. A version 3 document opens with every note, clip and chord
  intact and comes up in 4/4, because the field changed shape rather than gaining a sibling. A
  document written in 3/4 by `auris compose` under 0.1.0 opens in 4/4 and wants its meter set
  again.
* `TempoMap::bar_beat_at` is gone; the arithmetic lives on `SignatureMap`, which is where bars
  were always decided. `Project::time_signature` is now `Project::signatures`, and
  `Session::harmony_grid` is now `Session::harmony_grid_at`, since which note takes the beat
  depends on where you are.

## 0.1.0 — 2026-08-05

The first release. What is here works end to end: write notes, play them through a built-in
instrument, shape them with effects, and render the result to a WAV file.

### The window

* An arrangement of instrument and audio tracks, with a bar ruler, a cycle region and a playhead
  that scrolls itself into view while the transport rolls.
* A piano roll with two tools — pointer and velocity — a mixer with per-track strips and a
  master bus, an inspector, and a library that browses instruments, effects and SoundFonts as a
  tree.
* Every panel is docked to the left, the right or the bottom, and can be moved between them from
  its icon in the status bar. Where you leave them is where they are next launch.
* A command palette, a right-click menu on every component, and a menu bar — drawn by the
  application on Windows and Linux, the system's own on macOS. Both answer to the keyboard.
* Colour schemes, chosen in the settings window and checked for contrast by a test.
* English and Japanese throughout, following the system locale unless told otherwise.

### Sound

* Chiptune, two-operator FM and noise-drum instruments, all band-limited where it matters.
* Effects: gain, pan, delay, reverb, chorus, distortion, compressor, limiter and a parametric
  equalizer with a spectrum analyser.
* SoundFont playback: `.sf2` files are imported once and referenced by font, bank and patch, so a
  project stays small and opens playing the same sound.
* A realtime engine on cpal with a bounded command channel, plugin latency compensation, effect
  tails summed along the chain, and pre-built graphs handed over so nothing is dropped on the
  audio thread.
* Audio import through Symphonia, resampled when the device disagrees with the project.

### Writing music

* A harmony lane holding a key and a chord progression, editable on the timeline and audible by
  pressing or sweeping it.
* A structure lane naming the song's sections.
* Clips that write themselves from a preset and the harmony under them, and remember the recipe
  so they can be written again after the chords change.
* Whole-song composition from a text specification, from the desktop application or the command
  line.

### Files

* A project is a folder holding `<name>.auris` and an `Audio/` directory. Imported audio is
  copied in; assets are found again by name and size when they move, and a missing one costs that
  track's sound rather than the whole document.
* Configuration lives in `~/.config/auris-studio/` on every platform, in four small JSON files a
  dotfiles repository can carry.
* WAV export at 16-bit, 24-bit or 32-bit float, for the whole project or for the cycle region.

### Frontends

* `auris-studio`, the desktop application, on macOS and Windows.
* `auris`, the command line tool, on macOS, Windows and Linux.
