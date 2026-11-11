# Lyric analysis, diagnosis and local repair

[日本語](lyric-harness.ja.md)

The lyric harness checks structural fit before composing or editing. It runs locally,
without a singing model, inference server, audio device or document mutation during reads.
It does not rate artistic quality or prove that a changed phrase retains its meaning.

## Establish a reading

`analyze_lyrics` accepts original Japanese `text` and optional `readings`:

```json
{
  "text": "ずっと君と、珈琲",
  "readings": [{"phrase": 2, "kana": "コーヒー"}]
}
```

Musical phrases are the nonempty segments separated by line breaks or Japanese sentence
punctuation (`、。！？!?`). Phrase numbers and line numbers start at one. `start_char` and
`end_char` are zero-based Unicode scalar offsets in the original text, with an exclusive
end. They describe a whole phrase, not an inferred alignment between kanji and moras.

Each phrase includes its original text, reading, reading source, mora spellings, IPA
phonemes and closure/nasal/prolonged-sound-mark flags. A prolonged-sound-mark flag describes
an explicit `ー`; it is not an inference that every other vowel sequence is short.
The dictionary reads whole phrases when installed; the kana table works without it.
Unreadable phrases retain their text and error and make `complete` false. A reading
override is kana-only, addresses one existing phrase and lasts only for that request.
It does not change a subsequent composition's pronunciation.

Kana spells a long o as おう and a long e as えい (りょうて, せんせい), and the kana table sings
what is spelt. When a dictionary is installed and reads the written text to the same moras
as the reading, differing only where the reading spells う or い after that vowel, the
dictionary's held vowel is sung and the reading's spelling is kept: 両手 with the reading
りょうて sings *ryo-o-te*. Any other difference makes the reading a deliberate alternative,
sung as spelt. Without a dictionary, spell the long vowel with ー (りょーて). `write_lyrics`
reads its `reading` the same way.

## Fit a phrase to its notes

A mora is a unit of the language, not of the melody. Each mora's `slot` says what it asks
of the notes:

- `note` needs a note of its own. That includes `ん`.
- `hold` is a held long vowel: `ー`, or a vowel repeating the one before it inside a word,
  as in `さあ`. It may take its own note or be sung as the end of the previous one.
- `rest` is `っ`. It takes no note; it is the rest after the previous note.

So a phrase can be sung on any count from `min_notes` (every long vowel held) to
`max_notes` (every long vowel on its own note). The dictionary reads 東京 as トーキョー, so a
dictionary long vowel is a hold; a kana spelling such as `ゆこう` is read as written, and
`ゆこー` is how a reading asks for the hold.

Pass `fit` with the note count each phrase must be sung on, for example the notes of the
motif it repeats:

```json
{
  "text": "さあ いっしょに ゆこー",
  "fit": [{"phrase": 1, "notes": 6}]
}
```

Each fitted phrase gets `fit.verdict` (`fits`, `too_many_moras` for 字余り or
`too_few_moras` for 字足らず) and `fit.excess`, the moras to remove (positive) or add
(negative). Nothing is merged or split to force a fit. `note_reading` is the reading cut
into notes with `|`, for the requested count when it fits and for `max_notes` otherwise.
When long vowels must be held, the ones nearest the end of the phrase are held first.

Pass `note_reading` to `write_lyrics` as its `reading`. A reading without `|` still puts
one mora on each note. A grouped reading may put a held long vowel, a final `っ`, or one `ん` or bare
vowel after the first mora (`さん`, `あい`) on a note with the mora before it; anything
else is refused. VOICEVOX, which takes one mora per note, sings such a note as two, the
first mora keeping three fifths of it. A group ending in `っ` sings no
closure phoneme, so the next note must start at least a thirty-second note (120 ticks) after
that note ends.

## Feet, and the rhythm drawn on the beat

Each phrase also reports `feet`: its moras paired into two-mora feet (拍), the unit Japanese
verse and song are timed in. Pairing starts again at each word group and never splits a heavy
syllable, so `ふるいけや` is `ふる / いけ / や` and `かたかった` is `かた / かっ / た`. Sung two
moras a beat in eighths, each foot fills one beat. A one-mora foot is where the melody puts a
pickup, a held note or a rest.

`lyric_rhythm` reads a singer track after its lyrics are written and changes nothing. It draws
every sung line on its beat grid, choosing the coarsest grid that holds every onset:

```text
さ く | ら が | さ い | た - ‖ - - | _ _ | _ _ | _ _
```

`‖` divides bars and `|` divides beats. Each token is one slot of `grid_unit` (here an
eighth): kana where a note starts, with the moras it sings, `-` while it still sounds, `_`
for silence, `~` for a melisma and `っ` for a closure. Each line and section also reports moras
per second, the shortest mora in milliseconds and whether the line ends on a held mora.

Spots, each with clip and note index for `edit_notes`:

- `outlasting_syllable`: ん or the い of あい/おい lasts at least twice as long as the mora it
  belongs to, inside a line. At the end of a line it is a held ending, not a spot. A note
  only: in the 100 PJS songs this happens on 11% of ん notes and 8% of diphthong い notes,
  against 20% for other moras, so it is a loose tendency rather than a rule.
- `short_mora`: a mora shorter than 90 ms, which may lose its vowel to the next consonant in
  synthesis. Any short mora is a warning.
- `rest_in_word`: silence of a sixteenth or more between two moras of one word, shown as
  `まふ・らー` with the rest's length in `ms`. Words come from the Japanese dictionary and are
  read a clip at a time, so a word split by a rest long enough to end a line is still found.
  One in a section is a note and two are a warning: in 28 PJS songs it happened on 6 of 414
  steps inside a word, against 39 of 693 between words. Without a dictionary, or where a
  clip's written words do not read to the moras its notes sing, a note says it was not
  checked.

A section of three or more lines with no held ending, or slower than two moras a second, gets
a note only, since a rest can close a line and a slow section may be deliberate. In PJS, 81%
of lines end held and the median pace is 3.7 moras a second. A diphthong い written as the
first character of a word on its own note (今) is not counted. Where ん falls in the beat is
not checked: in PJS it arrives on a stronger position than its head as often as any mora does.

## Diagnose existing phrases

`diagnose_song` compares sung phrases that should share a melody. The diagnostic checks each
musical phrase, not the section's total mora count.

The result includes source-ordered analyses and independent issues with stable `code`,
`severity`, section/phrase addresses, reference, expected/actual values and repair scope.
`status` is `pass`, `fail`, or `incomplete`. Known violations take precedence over incomplete
input, while `complete` independently records whether all checks could run. An unreadable
phrase prevents dependent comparisons, but does not hide errors in other readable phrases.

For an existing project, use exact singer clip IDs and zero-based storage note indices
returned by `read_notes`:

```json
{
  "comparisons": [{
    "reference": {"clip": 10, "notes": [0, 1, 2, 3, 4, 5]},
    "target": {"clip": 11, "notes": [0, 1, 2, 3, 4, 5]}
  }]
}
```

Pass nonempty `comparisons`. Live comparisons read stored phonemes
and verify pitch, relative onset, duration and ornaments as well as mora/closure positions.
Selected indices must form a contiguous, nonoverlapping phrase. Original phrase labels
followed by `+` are supported when stored phonemes establish one mora on every note, as
`write_lyrics` does for kanji. Empty lyrics or missing/multi-mora phonemes are reported as
incomplete. Every diagnosis returns the current document revision.

Saved-file tools add `project` with a project ID or path. It is optional for `analyze_lyrics`,
so a project need not be created just to check a draft.
Live tools use the open document and omit `project`. Large results use the existing
immutable `read_report` pages; retain the returned report ID and JSON pointers.

## Preview and apply a local repair

Keep the target, comparison reference and required keywords fixed for one repair round.
`repair_lyrics` accepts one to three alternatives and checks all of them before editing:

```json
{
  "target": {"clip": 11, "notes": [0, 1, 2, 3, 4, 5]},
  "reference": {"clip": 10, "notes": [0, 1, 2, 3, 4, 5]},
  "expected_revision": "COPY_FROM_READ_NOTES_OR_DIAGNOSIS",
  "candidates": [
    {"text": "きみとあえる"},
    {"text": "きっとあえる"}
  ],
  "required_keywords": ["あえる"],
  "apply": false
}
```

If the reference sings `ずっときみと`, the second candidate matches its six moras and
second-position closure. The response selects the first passing candidate using a
one-based `selected_candidate`, and retains diagnostics for every candidate. A candidate
must be one complete musical phrase. Keywords are literal checks of its original text.
An optional candidate `reading` supplies explicit kana that is actually applied.

Review the candidate's meaning, then repeat the chosen candidate with `apply: true` and
the same revision. Omitting `reference` retains the target's established closure pattern.
Reference and target must have matching musical slots and must not overlap. A stale
revision, invalid selection or unavailable template leaves the document unchanged. If no
candidate passes, `applied` is false and all candidate diagnostics remain available.

Application writes lyrics and per-mora phonemes only in the selected target notes, clearing
duration pins only where the words or pronunciation changed. Pitches, timing, velocity,
ornaments and every other note are retained. One Undo reverses the edit; preview and
no-op application add no history. Kana-only candidates use per-mora note labels. Non-kana
text or a candidate with an explicit reading retains the original text on the first note
and uses `+` on the remaining notes, matching `write_lyrics`. Every note retains its own
pronunciation, so original spelling and an explicit reading both survive saving.

The live agent enforces at most three distinct alternatives per target/revision in a user
turn across tool calls and transports. Reusing a previewed candidate for application does
not consume another alternative. Changing the reference or keywords in that round is
rejected, as are unchanged repeated previews or application attempts. Standalone saved-file
callers receive the same three-candidate bound per call;
their orchestration must retain the round and stop rather than submitting endless batches.

After applying, read back the notes, diagnose every related phrase, and review semantic
preservation. Inspect the singer state and rerender stale takes with `sing`. Do not claim
completed vocals from a structural pass alone. When three alternatives fail, report the
remaining diagnostics and which lyric or musical constraint needs to change.
