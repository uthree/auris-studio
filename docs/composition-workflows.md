# Production phases

[日本語](composition-workflows.ja.md)

The live agent and the workshop composers produce a song in three phases: score,
arrangement and mix. The complete tool catalog is offered in every phase, so a
model that needs something another phase owns can see it and step back.
Use `AURIS_COMPOSITION_CANDIDATES=1` to advertise candidate tools. The evaluation
runner enables it; use `--no-candidate-tools` to test the default application
catalog instead. Its local-model context defaults to 64k and can be recorded
explicitly with `--context-tokens`; a larger budget in an experiment does not
change application defaults.

Unspecified new vocal songs include a second verse and later chorus/ending,
while explicit short forms and local edits keep their scope. See
[live vocal production](agent-workflows.md#live-vocal-production) for section
coverage and score readback.

`production_record` accepts `enter_phase` with `phase` (`score`, `arrangement`,
`mix`), a `reason` and, for the score phase, a `writing_order` (`harmony_first`,
`melody_first`, `rhythm_first`). The score phase decides form, tempo, chords,
lyrics, the foreground melody and its voice, and ends in a demo with a plain
guide part; the arrangement phase chooses sounds and writes backing, drums,
performance and transitions around that fixed score; the mix phase balances,
processes and delivers. `set_roles` registers the foreground tracks and the
guide tracks (or `no_foreground` with a reason), and is required before
arrangement. A local fix enters the phase that owns what is being fixed.

The phase is conversation metadata, subject to the same write permission as
other record updates, and survives context compaction and conversation restart.
The system prompt is only the common core: the rules that hold in every phase
(addresses, revisions and units, failure recovery, scope and honesty). Each
phase's own guidance, tool essentials and technique index come back as
`phase_instructions` when a phase is entered and when the record is read,
beside the record rather than inside its `read_report` pages; other record
updates and project inspections leave them out. The technique index names
headings of [composition techniques](composition-techniques.md), which
`search_documentation` returns. Going back to an earlier phase keeps the later
phases' work and lists them under `phase.revisit`. `phase.checks` reports the
unmet structural conditions of the current and earlier phases, read from
`inspect_project`'s `phase_facts`: sections covering the song, chords in every
section, registered roles, annotated lyrics and current vocal takes for the
score; assigned drums and a replaced guide for the arrangement; a measured full
mix for the mix. They are warnings, not refusals.

What a phase keeps fixed is enforced by the host, not by the prompt. In the
arrangement phase the foreground tracks' notes and lyrics, the harmony, the
sections, the meter and the tempo map are locked; in the mix phase every note,
sound and performance is too. The host compares the locked material before and
after each agent edit, by where it sounds on the timeline, and withdraws an edit
that changed it: the document and the undo history are left as they were, and
the model is told what changed and which phase to enter to change it. Long jobs
are compared when their result is written. Returning to the score and coming
forward again lists, under `phase.review`, the bars that changed and the tracks
that play over them, so the arrangement can be fixed there first.

The workshop runs in rounds. For a new song the director first dispatches two
or three demos (`"phase": "score"`), which stop at the score with roles
registered and are marked as demos on their cards. After the user selects one,
a `"phase": "finish"` assignment continues it in the arrangement phase, with its
melody, lyrics and harmony locked, and finishes the mix. Each candidate keeps
its production record beside its song, so a finish starts from its demo's roles.

The shared contract retains exact IDs, tick units, revision checks, source
preservation, limits on exploration, and the distinction between measurements,
critic reports and human listening. Workflow instructions are model guidance;
the canonical session commands enforce validation and permissions independently.
Deferred candidate decisions retain the pending round and its IDs across turns
within its lifetime. Finishing a turn is not a reason to cancel the round.

For a controlled comparison, use the same frozen executable, model, task cohort
and tool catalog with `composition_quality.py run`. Record failures and elapsed
time alongside backend token counters. Last-reported input context is not
cumulative billable input; missing usage must not be presented as zero cost.
