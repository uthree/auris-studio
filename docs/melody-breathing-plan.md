# Melody breathing

[日本語](melody-breathing-plan.ja.md)

The arrangement harness measures phrase space and prepares explicit foreground note
repairs. Enable it with `AURIS_ARRANGEMENT_HARNESS=1`. Shared composition guidance
also asks agents to plan calls, continuations, answers and breathing points when writing
new music, through either manual note tools or generated composition.

The original report concerned an agent-composed lead playing continuously. Its project
has not been reproduced. The generated writer already closes phrases; it was not changed.
Tests distinguish directly authored continuous notes, detached notes without a phrase
rest, existing generated phrase endings, and notated gaps with sustain.

## Diagnose before editing

Read `inspect_project` and `read_notes` for stable IDs, original note indices and revision.
Confirm the foreground role rather than guessing from a track name:

```json
{
  "operation": {
    "kind": "inspect_breathing",
    "track": 2,
    "role": "lead",
    "start_tick": 0,
    "end_tick": 30720,
    "min_rest_ticks": 480,
    "phrase_ends": [15360, 30720]
  }
}
```

The result merges overlapping intervals before measuring authored and performed note
occupancy, exact continuous runs and `longest_without_phrase_rest`. The latter bridges
articulation gaps shorter than `min_rest_ticks`; uniformly detaching notes need not
create a phrase breath. Durations are ticks, quarter-note units and seconds through the
full tempo map. Phrase endings report their preceding space. Gaps at range boundaries
are included, and all positions are absolute ticks.

`role:"texture"` keeps the same measurements without lead-specific suggestions. Choose
the threshold and phrase endings for meter, tempo, style, section and vocal text.
There is no universal silence quota or compulsory rest every four bars. Missing space
at an explicit lead ending is a suggestion, not an artistic failure.

Clip joins and all bounded seeded loop passes use the playback performer. Muted clips
and muted tracks contribute no note occupancy. Limits are 128 quarter notes per request,
32 intersecting clips, 2,048 authored notes per clip, 4,096 potential performed instances
per clip, 128 loop passes, 32 performance stages and 8,192 intervals per representation.
Unsupported or excessive work sets `complete:false` and suppresses breathing suggestions;
partial numbers are not a verified result. At most 64 gap details are returned, alongside
their full count and a separate truncation flag. Large reports use `read_report` pages.

`audibility` separately flags sustain data and effect/send paths. Those are possible
causes, not proof of an audible tail. Instrument release, pedal state, other tracks,
solo/routing and bus effects are outside note occupancy. Compare the foreground stem
and full mix; a notated rest is not proof of audible silence.

## Prepare an explicit rest

Use `arrangement` validation with its current revision. Every edit includes one short
purpose, at most 256 UTF-8 bytes (keep Japanese purposes to a brief clause):

```json
{
  "operation": {
    "kind": "validate",
    "expected_revision": "COPY_FROM_CURRENT_INSPECTION",
    "changes": [{
      "purpose": "Let the lead release before the next phrase",
      "edit": {
        "kind": "breath",
        "clips": [12],
        "rests": [{"start_tick": 14880, "end_tick": 15360}],
        "min_note_ticks": 120,
        "protected_notes": [{"clip": 12, "index": 0}],
        "omit_notes": []
      }
    }]
  }
}
```

A note crossing a rest's start is shortened there, retaining at least `min_note_ticks`.
An attack inside a rest is refused unless its exact original `{clip,index}` is in
`omit_notes`. An omission removes only that explicitly named note; retained pitches,
attacks, velocities and order stay unchanged. Validation previews every changed original
index and its before/after duration, or the complete omitted note. All omissions must
fall inside a requested rest. Protected indices, vocal data/ornaments and touching
same-pitch continuations (possible ties, including across clips) are refused. Notes are
not split or uniformly gated.

### Held arrivals and instrument tails

`inspect_breathing.ending_guidance` distinguishes sustained winds/bowed strings,
struck/plucked instruments and patch-dependent synths. For registered native instruments,
`envelope_controls` reads attack/decay/sustain/release descriptors and saved overrides
from the actual implementation, including whether a parameter has automation. A release
value is a control setting, not a measurement of acoustic silence. Inspect its automation
and render the stem and mix through the following entry. Hosted and sampled envelopes
remain patch-dependent; no native default is invented for them.

When shortening a newly attacked ending sounds clipped, a `breath` edit can explicitly
hold an existing arrival earlier in place of named approach notes:

```json
"landings": [{
  "note": {"clip": 2, "index": 15},
  "start_tick": 13440,
  "replace_notes": [14]
}]
```

All indices refer to the original `read_notes` response. The arrival keeps its pitch and
velocity, moves to the absolute `start_tick`, and is held until the requested rest. The
named approach notes must cover exactly the interval from the new attack to the original
attack, without gaps, overlaps or another authored voice. No other notes are selected implicitly.
Protected/vocal/tied notes are refused, including replaced approach notes. Landings are
limited to sixteen and require independent clips; use independent copies for loop passes.
Preview includes every original replaced note and the arrival timing. Without `landings`,
the original release-only contract is unchanged. Curves, performance and recipes remain
intact, and the complete proposal remains one Undo.

Give the arrival enough time to settle before note-off, then budget the instrument tail
before the next entry. Choose its pitch against the local harmony and motif; the final
written note is not necessarily a resolved arrival. Let piano/guitar resonance decay
rather than forcing a generic fade.
For sustained wind/bow/synth patches, an explicitly scoped expression or native parameter
automation can shape an ending when needed. Preserve the outside curve and check for tail
reappearance when restoring gain; a gain dip alone does not release a held voice. Do not
change global instrument defaults as a substitute for phrase design.

Select 1–16 unmuted clips on one foreground instrument track and 1–16 ordered,
nonoverlapping rest windows. The selected passage is bounded to 128 quarter notes.
Singer/drum tracks are protected. A repeated clip requires explicit windows covering
the same relative rest in every audible pass; a proposal affecting an unrequested pass
is refused. Use independent clips for different phrasing between repetitions. Existing
recipes, authored curves, transforms, clip lengths, harmony and timeline maps stay intact.
Explicit recipe regeneration may later replace repaired notes, as with other score edits.

The candidate is checked against actual performed notes from the whole target track.
Another clip or a performance stage filling a rest refuses the proposal. Supported or
unknown active sustain during a rest also refuses it. Inspect/reset CC64 explicitly
before altering more notes; an instrument known to ignore CC64 does not gain pedal
support merely because a lane exists. Reverb and release remain audio-review conditions.

Review the prepared `note_changes`, then apply its `plan_id` with the original revision.
The existing arrangement contract supplies stale rejection, cancellation, exact retry
and one Undo/Redo. Failed validation changes no live score or history. An already empty
rest with no other edits adds no Undo. Independently reread notes and diagnose the same
phrase, then compare the same rendered bars. Limit local repairs to two attempts.

## Verification

Session tests cover occupancy unions, articulation versus phrase space, tempo changes,
clip/loop joins, muted clips, explicit roles, bounded work, indexed pickup omissions,
protected/vocal notes, possible ties, pedal state, overlapping clips, exact preservation
and one-Undo retries. A real PCM test measures reduced foreground energy in the repaired
rest while an added sustained texture keeps the full mix sounding. Existing generated
phrase endings are inspected without regeneration.

The ignored window test
`installed_agent_repairs_lead_breathing_and_preserves_generated_phrases` uses the
`AURIS_TOOL_SURFACE_EVAL` configuration described in
[agent tool validation](agent-tool-validation.md). A real agent diagnoses manual and
generated leads plus an intentional texture, repairs only two named lead endings in one
proposal, independently rereads the result, and numerically inspects the same foreground
stem and mix before and after. Exact state, recipe preservation and one Undo/Redo are
checked. Numeric verification alone does not establish natural phrasing or musical
improvement; use a separate listening comparison when an audio critic is available.

The [2026-10-23 verification record](../tools/eval/references/agent-breathing-2026-10-23.json)
includes real-agent and separate audio-model outcomes. The agent repaired the two exact
endings and preserved the generated reference and texture. The critic received both
stem and mix comparisons, but called the new ending abrupt/staccato and was uncertain
about intent. Measured note and PCM space increased, while natural phrasing was not established.
That is not a musical pass: consider a held arrival or an explicitly scoped dynamic taper
and compare again, rather than treating lower occupancy as a quality score.

### Follow-up ending comparison

The [held-arrival comparison record](../tools/eval/references/phrase-ending-2026-10-23.json)
keeps the follow-up attempts, the actual native envelope controls, WAV hashes, PCM
measurements and every critic response. The six-second excerpts include the following
entry and compare the previous shortening-only repair with the new explicit ending.
The continuous original is retained as a separate control. These are synthetic test
phrases, not a claim about an entire song.

The final comparison holds the same arrival pitch longer, with instrument-specific
release space. Sustained chiptune and clarinet receive a scoped track-gain taper, with
the original gain restored at the next entry after their native release. Piano keeps its
natural decay without a gain fade. Violin keeps its 1.5-second release control. Its measured
tail falls below -100 dBFS about 0.7 seconds after note-off in this fixture, so the final
interval is 0.75 seconds instead of the uniform quarter second. The conservative 1.75-second
intermediate interval is retained in the record; it left excessive quiet space. No instrument
or composer defaults change. Both foreground stems and mixes with the same sustained
background are rendered at common gain, without normalization or limiting.

Numeric assertions verify easing before release for synth/wind, reduced violin tail
immediately before the next entry, and preservation of the next attack for all four.
These checks establish the acoustic behavior, not perceived naturalness. The critic
received eight final paired uploads but repeatedly requested audio or could not identify
music. Shorter-focus and mono/16-bit controls did not establish reliable musical judgment;
one earlier isolated answer preferred the shortening-only stem, while its mix comparison
failed. No favorable model quality verdict is claimed. The comparison WAVs remain available
for human audition.

The tests that produced these records sent audio to a separate critic model, which has since
been retired; the records remain as evidence of what was measured then.
