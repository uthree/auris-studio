# Experimental production phases

[日本語](composition-workflows.ja.md)

Set `AURIS_COMPOSITION_WORKFLOWS=1` before starting Auris to test production in
three phases: score, arrangement and mix. The complete tool catalog stays the
same, so prompt experiments do not silently compare different tools. The default
continues to use the existing prompt until listening evidence supports a change.
Use `AURIS_COMPOSITION_CANDIDATES=1` independently to advertise candidate tools;
keep that flag equal in both prompt conditions. The evaluation runner enables it
in both conditions; use `--no-candidate-tools` to test the default application
catalog instead. Its local-model context defaults to 64k and can be recorded
explicitly with `--context-tokens`; a larger budget in an experiment does not
change application defaults.

Both profiles share the full vocal-song form guidance: unspecified new songs
include a second verse and later chorus/ending, while explicit short forms and
local edits keep their scope. The default profile therefore gets this behavior
without enabling experimental workflows. See [live vocal production](agent-workflows.md#live-vocal-production)
for section coverage and score readback.

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
In modular mode the response and later project inspections carry only the
current phase's instructions. Going back to an earlier phase keeps the later
phases' work and lists them under `phase.revisit`. `phase.checks` reports the
unmet structural conditions of the current and earlier phases, read from
`inspect_project`'s `phase_facts`: sections covering the song, chords in every
section, registered roles, annotated lyrics and current vocal takes for the
score; assigned drums and a replaced guide for the arrangement; a measured full
mix for the mix. They are warnings, not refusals.

The shared contract retains exact IDs, tick units, revision checks, source
preservation, limits on exploration, and the distinction between measurements,
critic reports and human listening. Workflow instructions are model guidance;
the canonical session commands enforce validation and permissions independently.
Deferred candidate decisions retain the pending round and its IDs across turns
within its lifetime. Finishing a turn is not a reason to cancel the round.

For a controlled comparison, use the same frozen executable, model, task cohort
and tool catalog with `composition_quality.py run --prompt-profile legacy` and
`--prompt-profile modular`. Record failures and elapsed time alongside backend
token counters. Last-reported input context is not cumulative billable input;
missing usage must not be presented as zero cost. Use blinded human preference
and request adherence before adopting the modular profile as the default.
