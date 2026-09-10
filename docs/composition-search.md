# Bounded composition search

Composition search repeatedly writes a song, measures its written notes, and retains the exact
best generated score. It is a sequential, headless command exposed through
`auris_session::composition_search::search_composition`. Running it needs no audio device,
SoundFont, model, Python environment or UI.

## Search from the desktop app

Open **Compose → Compose a Song…**, then expand **Search** in the song sheet. Choose a song
preset or edit its settings first. The search panel offers random search or hill climbing,
an attempt budget, a target number of written notes per bar, and an independent search seed.
Select a part whose density will vary, a played section whose intensity will vary, or both.
Choosing **Keep fixed** leaves that parameter unchanged; at least one parameter must be selected.
The GUI uses bounds `[0, 1]` and hill steps of `0.1`. An automatic part density starts at its
current mood default, or `0.5` for drums. The composition seed stays fixed during the run.

**Start Search** runs on a worker while the panel reports completed attempts and the best
measured density. **Cancel** stops between attempts and retains any valid partial winner.
The project stays untouched until **Apply Best & Play** adopts the exact evaluated score,
prepares its sources, balances it, and starts playback. This is one undoable composition
command. Source or preparation errors are reported by the ordinary composition workflow.

Changing the song, search settings or project makes a result unavailable for adoption.
Changing them during a run cancels that run. Closing the song sheet also cancels its task;
late results cannot replace the results of a newly opened sheet. Collapsing only the search
panel leaves its task running. A new search waits until a cancelled worker has finished its
current attempt. If editing the song removes a selected part or section from the search choices,
that selection returns to **Keep fixed**; choose another parameter before restarting.

The target is a density preference, not a musical-quality rating. Start with a short song and
a small budget, compare the measured result with the target, then listen after applying it.

## Run the example

```sh
cargo run -p auris-session --example compose_search -- chiptune random 24 42 7 search-random
cargo run -p auris-session --example compose_search -- chiptune hill 24 42 7 search-hill
cargo run -p auris-session --example compose_search -- MySong.asong hill 32 42 7 search-song 16
```

Arguments are a preset name or `.asong` file, algorithm (`random` or `hill`), attempt budget,
search seed, composition seed, new output directory, and optional target notes per bar
(default `12`). The example searches one part's density and the first played section's
intensity with bounds `[0, 1]` and hill steps of `0.1`. It prefers a melody part with density
that affects at least one played section; if no such part exists, it searches only intensity.
An implicit part density becomes an explicit `0.5` before the run. The output records this
prepared base so the starting settings can be inspected.

The new directory contains `base.asong`, the winning `best.asong` when a candidate succeeds,
and `report.json`. The report records seeds, bounds, target, ordered candidate outcomes,
failure counts, termination and the winning candidate's evaluation. Candidate entries record
the searched values against `base.asong`; all remaining fields are fixed. The terminal prints
the retained score's arrangement summary. The returned score remains in memory throughout the
run; writing `best.asong` records its inputs rather than serializing its generated notes.

## Metric and search space

The evaluator counts written note events across all tracks and divides by the score's actual
length in bars, including its ending. Chord tones and drum notes each count as separate events.
It measures the written score before performance transforms; timing humanization, source
choice, rendered audio and vocal notes later materialized by the session do not enter the
objective.

```text
notes_per_bar = written_note_count / score_length_in_bars
fitness = -abs(notes_per_bar - target_notes_per_bar)
```

Fitness is maximized: zero is an exact match, and a value closer to zero is better. The target
must be positive and finite and stays fixed throughout a run. Raw measured density and the
target accompany each evaluation. This is target matching, with no weights or normalization
to hide the units. A closer match describes arrangement density, not universal musical
quality. The target may be unreachable inside a particular space; inspect the result and
listen before deciding to keep it.

`SearchSpace` allows a `PartDensity`, a `SectionIntensity`, or both. Each names an existing
part or played section and carries `ParameterBounds { min, max, step }`. Bounds must be finite,
inside `[0, 1]`, and have `min < max`; the step must be positive, no wider than the interval,
and large enough to change an `f32` value. The base value must already lie inside its bounds.
A selected part density must be explicit.
Existing rhythm overrides, section part lists and per-section density tweaks still apply, so
choose parameters that affect the passages you intend to control.

All unselected configuration fields remain fixed. Every candidate also uses the same explicit
composition seed, which isolates parameter changes from another random take. The independent
search seed controls the sequence of proposals.

## Reusable command

```rust
use auris_session::composition_search::{
    DensityTarget, ParameterBounds, PartDensity, SearchMethod, SearchSpace,
    SongSearchRequest, search_composition,
};
use auris_session::prelude::SongSpec;

let mut base = SongSpec::default();
base.parts[0].density = Some(0.5);
let part_name = base.parts[0].name.clone();
let request = SongSearchRequest {
    base,
    space: SearchSpace {
        part_density: Some(PartDensity {
            part: part_name,
            bounds: ParameterBounds { min: 0.1, max: 0.9, step: 0.1 },
        }),
        section_intensity: None,
    },
    attempt_budget: 24,
    search_seed: 42,
    composition_seed: 7,
    algorithm: SearchMethod::HillClimb,
    evaluator: DensityTarget { target_notes_per_bar: 12.0 },
};
let result = search_composition(&request, || false).unwrap();
// Keep result.best's exact score; do not compose its recipe again to retrieve the winner.
```

The session module re-exports the search API so a frontend can depend only on `auris-session`.
The implementation lives beside the pure composer in `auris-compose`. `Composer` creates
scores, `Evaluator` measures them, and `SearchAlgorithm` proposes parameters through `ask`
and receives each outcome through `tell`. The ordinary `run_search` function owns iteration,
budget accounting, cancellation, history and best-score retention. A custom composer or
evaluator can use that same runner without depending on a concrete song search algorithm.
`run_search_with_progress` and `search_composition_with_progress` additionally call a
synchronous observer after each recorded attempt. Frontends can publish lightweight progress
from that worker callback without changing cancellation, ranking or retention semantics.

Random search samples independent bounded parameters on each attempt. Hill climbing proposes
the base first, then changes one allowed parameter at a time around its incumbent. Successful
feedback replaces that incumbent only when fitness strictly improves; ties and failures leave
it intact. A step crossing a bound clamps to that bound; an outward step from an exact bound
turns inward without drawing extra randomness. Until a successful incumbent exists, subsequent
proposals are sampled. The hill climber can settle at a local optimum; compare it with random
search at the same budget and across several search seeds.

## Results, failure and reproducibility

An invalid request, including zero budget or invalid evaluator settings, returns an error
before search begins. Every proposed candidate consumes one attempt, including validation,
composition or evaluation failures. Each outcome is returned to the algorithm and recorded
with its candidate ID, complete parameters and composition seed. Non-finite fitness or metric
diagnostics become evaluation failures before ranking or feedback.

The runner keeps the earliest candidate on a fitness tie and retains its exact generated
`Composition`, candidate inputs and evaluation. The result includes ordered history, failure
counts and an explicit termination reason. All-failed runs have no best score. Cancellation
checks occur between attempts and return partial results; the current synchronous composer
has no internal cancellation hook, so an ongoing composition completes before the next check.
A custom algorithm may also end by returning `None` from `ask`.

Identical settings and seeds produce identical ordered outcomes and scores with the current
deterministic composer in the same build. The promise does not extend across changes to the
composer, metric or RNG implementation. Preserve generated notes when keeping an arrangement:
an existing session can adopt the retained score through `Session::compose_without_balance`
and save the resulting project. This uses the evaluated score directly. The saved `.asong`
alone is a recipe that a newer build may perform differently.

The focused tests cover real composer ranking and reproducibility, feedback-dependent hill
mutation, immutable fields and bounds, ties, failure stages, non-finite results, budgets,
early exhaustion, cancellation and all-failed searches. These verify optimization behavior;
musical and acoustic evaluation remains the listening and measurement workflow in
[evaluation.md](evaluation.md).
