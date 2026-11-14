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
`search_documentation` returns. When the song's genre and usage are chosen
(`edit_timeline` `set_style`, stored in the document), the instructions end
with that genre's and that usage's page under `docs/genres/` and
`docs/usages/`: its summary and its section for the phase in force. The
`set_style` result itself carries both pages whole, as `style_guidance`,
because a short piece is often written without entering another phase. The checks
follow the style too (`auris_session::session::style_checks` is the table): a
loop or a cue is not asked for an intro and an outro, a genre without a
register-lifted chorus (jazz, orchestral, ambient, hip-hop and lo-fi, dance) is
not asked for one, loop-built genres may keep the verse's progression in the
chorus, and in jazz, R&B, city pop and Latin a held note outside the key is
reported as information rather than a warning. Where the style's page names a
loudness, `lint_project`'s audio pass reports a mix outside it as
`off_target_loudness` (a single level is held within a unit either way), and
`normalize_mix` without a target aims at the middle of that range, and
`verify_render` without one fails a file outside it. Going back to an earlier phase keeps the later
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

The score linter's warnings and errors are reported, not left to the prompt
alone. Every `inspect_project` counts them under `phase_facts.lint`, and the
score phase's checks list them as `lint_warnings_resolved`, which every later
phase reads too. When a workshop composer reports a candidate finished with any
left, the host sends it back with the first of them, at most twice; what remains
after that is logged in `composer-log.jsonl` and the candidate is kept. Models
that finish fast skip an instruction to lint; they do not skip a reply.
left, the host sends it back with the first of them, at most twice; what remains
after that is logged in `composer-log.jsonl` and the candidate is kept. Models
that finish fast skip an instruction to lint; they do not skip a reply.

The same send-back covers a song that stops short. It lists the structural
checks unmet in every phase the record reached, read from the live session:
sections and chords covering the song, roles, lyrics, current takes, drums,
the guide replaced, and a foreground sounding in at least half of the song's
bars (`phase_facts.tracks[].bars_with_notes`). Anything but a demo must also
have reached the mix phase. Listening coverage is left out, since every edit
makes it stale, and so is a record that never entered a phase.

The shared contract retains exact IDs, tick units, revision checks, source
preservation, limits on exploration, and the distinction between measurements,
a model's listening and human listening. Workflow instructions are model guidance;
the canonical session commands enforce validation and permissions independently.
Deferred candidate decisions retain the pending round and its IDs across turns
within its lifetime. Finishing a turn is not a reason to cancel the round.

For a controlled comparison, use the same frozen executable, model, task cohort
and tool catalog with `composition_quality.py run`. Record failures and elapsed
time alongside backend token counters. Last-reported input context is not
cumulative billable input; missing usage must not be presented as zero cost.
