# Algorithmic backing for an authored melody

[日本語](agent-accompaniment.ja.md)

The live agent and saved-project MCP tools expose `accompaniment`. The agent can edit a
foreground melody with `edit_notes`, then ask Auris to write bass, chords, pads, arpeggios or
assigned drums. Requests contain roles, ranges and recipe controls; responses contain clip IDs,
note counts and recipe summaries rather than complete note arrays.

Inspect the project first. Identify the foreground by its musical role and copy its clip IDs
into both `foreground_clips` and `protected_clips`. The foreground must be a nonempty
subset of protected clips, and every foreground clip must contain pitched notes on a
non-drum track. It explicitly selects written melody notes for chord/arpeggio spacing,
including hand-written and imported notes without a recipe. Additional protected clips need
only exist: they reserve their tracks and remain untouched, but may be empty pad clips or
drum clips and do not participate in foreground adaptation. Protection preserves the entire clips, including authored notes,
curves, lyrics, performance and recipes. It does not infer a new progression from the melody.
Choose harmony deliberately with `suggest_harmony` and `edit_harmony`; existing phrase-writing and backing-adjustment
algorithms perform the score. A seventh, suspension or diminished chord is a musical choice,
not a required proportion of the chart.

The explicit foreground combines selected note windows and loop repetitions, skipping muted
tracks/clips and zero-velocity notes. It is bounded to 4096 stored and expanded notes per target.
Performance transforms, bends and solo routing are not simulated. Each returned clip includes
an `adaptation` report with source note count, before/after backing count, whether the score
changed, and a reason when adaptation is bypassed. Explicit rhythms, driving chord density,
sustained ambient textures and bass/pad/drum roles retain their pattern. No preference claim
is implied by a changed score or lower note count.

## Propose harmony from the motif

Call the read-only `suggest_harmony` tool before changing a progression:

```json
{
  "request": {
    "melody_clips": [10],
    "start_bar": 1,
    "bars": 4,
    "mood": "dreamy",
    "ending": "settled"
  }
}
```

The project key is used unless `request.key` explicitly supplies another one. Mood names are
`neutral`, `bright`, `dark`, `calm`, `driving`, `epic`, `dreamy`, `tense` and `funky`.
`ending` is `settled` (prefer a tonic arrival when compatible) or `open` (allow continuation).
Mood adjusts the cost of supported colour and harmonic changes; it never changes the key.

The result contains up to three distinct candidates, ordered by heuristic fit, always including
the strongest plain major/minor-triad alternative. Long notes and accented notes count more than
short passing notes. Sevenths need melodic support; suspensions need a fourth followed by a
third; diminished harmony needs melodic support and a following chord with small pitch-class
motion. Repeated motifs encourage related harmony. Every added tone or within-bar change has a
cost, including in a tense mood. If colour offers no meaningful improvement over the plain
alternative, the result contains only that alternative. There is no complexity target.

Each candidate reports chord names, per-bar reasons, weighted melodic fit and salient notes
outside the chord. These are symbolic heuristics, not listening results or artistic ratings.
The analysis reads authored notes within clip windows, including loops; performance transforms,
bends, timbre and audio are outside its evidence. Audition unresolved tensions in context.

The response's `revision`, `protected_clips` and `foreground_clips`, plus a chosen candidate's `harmony` object,
feed directly into `accompaniment` with `operation.kind = "regenerate"` and explicit backing
clip IDs. This applies harmony and rewrites only those clips in one Undo step. If the project
changes before acceptance, request fresh suggestions. For new backing, apply the selected
chart with `edit_harmony`, inspect the updated revision, then generate. Suggestions themselves
change neither the document nor history, including through saved-project MCP calls.

Requests cover 1–16 bars, at most 128 quarter notes, with 1–8 distinct pitched melody clips and
at most 4096 stored/expanded notes. Split at key or meter changes. The supported keys are major,
natural/harmonic/melodic minor, Dorian, Lydian, Mixolydian and Phrygian. A phrase with no melody
notes is rejected; rests within a phrase retain the preceding harmony.

## Generate and regenerate backing

Create separate backing tracks and select their sounds using the normal track tools.
For drums, inspect the track's role assignments before generating. Creating or switching to
the built-in drum kit installs its authored assignments; reselecting the current source
preserves an explicitly edited or empty map. Other sounds need their own accepted assignments.
A single drum track cannot receive
overlapping generated clips in one range: generate one `drums` clip with its configured
voices, split the ranges, or use separate drum tracks for kick, snare and hat. Copy track IDs and the current
content `revision` from `inspect_project` into the request. Example IDs below are placeholders.
Saved calls add a `project` path or project ID to the same arguments.

```json
{
  "expected_revision": "revision-from-inspect_project",
  "protected_clips": [10],
  "foreground_clips": [10],
  "operation": {
    "kind": "generate",
    "parts": [
      {
        "track": 2,
        "name": "Verse bass",
        "part": "bass",
        "start_bar": 1,
        "bars": 4,
        "seed": 23,
        "settings": { "density": 0.65 }
      },
      {
        "track": 3,
        "name": "Verse pad",
        "part": "pad",
        "start_bar": 1,
        "bars": 4,
        "seed": 42
      }
    ]
  }
}
```

Keep the returned clip IDs. To change the progression and rewrite only the chosen backing,
read a current revision and pass those IDs with a harmony replacement:

```json
{
  "expected_revision": "current-revision",
  "protected_clips": [10],
  "foreground_clips": [10],
  "operation": {
    "kind": "regenerate",
    "clips": [11],
    "harmony": {
      "start_bar": 1,
      "bars": 4,
      "key": "C major",
      "chords": "| Imaj7 | IVsus4 | viidim7 | V7 |"
    }
  }
}
```

Here clip 11 follows the new harmony, while clip 12 and the foreground retain their stored
notes. Harmony is project metadata shared by all tracks: unselected backing may therefore
still reflect the previous progression. Choose the intended targets explicitly. A separate
`edit_harmony` also preserves notes; omit `harmony` from a later `regenerate` request to use
the current project harmony. Changing recipe controls remains an explicit `edit_recipe` operation.

Recipe settings use the existing `edit_recipe` vocabulary. `octave` is an integer relative
offset from the writer's native register, limited to `-2..2`; `0` keeps the native register
and it is not an absolute MIDI octave. Regeneration preserves clip IDs, position, length, loop settings, recipe controls and seeds,
authored curves and performance stacks. A hand-edited generated score is refused rather than
overwritten, and a frozen clip has no recipe to regenerate. The protected foreground may be
authored manually or generated; neither is rewritten by this tool.

Each operation prepares an owned snapshot on a worker and publishes only if its originating
document is unchanged and the request has not been cancelled. Generation failure changes
neither the live document nor its Undo/Redo history. Harmony and all selected backing changes
form one Undo step; saved execution checkpoints the change. Retrying a completed generation
with its old revision is rejected, and occupied target ranges are refused.

Limits: 1–16 protected clips and 1–16 parts or distinct regeneration targets; at most 64 bars
and 256 quarter notes per target, 1024 quarter notes and 65,536 generated notes per operation.
Requests must produce nonempty clips. Split targets at internal meter/section changes, and
split clips that span beyond a requested harmony replacement. The snapshot is limited to
64 MiB of serialized project data. Recipe settings use the existing `edit_recipe` vocabulary;
use each generation part's `seed`, leaving `settings.seed` omitted.

Inspect the returned summaries and compare the same rendered bars with `inspect_audio` or a
preview. Preserved notes and correct regeneration are structural guarantees, not an assessment
of musical quality. Saved recipes retain their usual contract: a seed is reproducible within
a build, and freezing retains a take across changes to the composer.
