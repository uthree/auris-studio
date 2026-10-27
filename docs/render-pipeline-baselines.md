# Render-pipeline compatibility baselines

The M0 reference is the current document and renderer, before native pipeline
storage. These checks characterize preservation of authored data and execution
semantics. They do not establish musical quality or listening preferences.

## Persistence

`crates/auris-io/tests/project_format_baselines.rs` reads fixed files written by
the actual format 27–32 serializers. The [fixture provenance](../crates/auris-io/tests/fixtures/project_formats/provenance.md)
records their source revisions and reproduction command. Twelve tests cover
authored notes and sustain extents, transform order and seeds, recipes without
regeneration, singer score/take references, imported audio, SoundFonts, opaque
plugin bytes, automation keys/curves, routing taps, tempo/loop maps, IDs and
current-format round trips. Loading also leaves the original file bytes intact.

## Scheduling and rendering

Use the existing numerical tests as independent compatibility oracles. They
assert samples, absolute sample positions, lengths and route relationships;
copying today's complete rendered output would weaken these contracts.

| Contract | Existing executable reference |
| --- | --- |
| Absolute note positions and tempo changes | `graph::schedule::tests::notes_land_on_absolute_sample_positions`, `a_tempo_change_moves_later_notes` |
| Clip repetition | `graph::schedule::tests::a_looped_midi_clip_is_expanded_into_its_repeats` |
| Bus sums and independent output branches | `renderer::tests::a_bus_sums_what_is_routed_to_it_and_its_fader_moves_all_of_it`, `multiple_outputs_sum_independently_and_compensate_each_branch` |
| Sidechain dependency order | `renderer::tests::a_track_that_keys_another_is_rendered_before_it_wherever_it_is_stored` |
| Seek into sounding/stacked notes | `renderer::tests::starting_playback_inside_a_held_note_chases_it_back`, `seeking_into_stacked_notes_matches_playing_through_them` |
| Loop splitting and note chase | `renderer::tests::a_loop_wraps_inside_a_block_without_a_discontinuity`, `a_note_spanning_the_loop_start_retriggers_on_every_wrap` |
| Delay compensation across block sizes | `renderer::tests::compensation_holds_at_any_block_size` |
| Serial and parallel tails | `offline::tests::the_longest_effect_tail_is_appended`, `a_tail_behind_another_tail_lengthens_the_export_by_both` |
| Offline/realtime agreement | `offline::tests::the_export_matches_block_by_block_realtime_rendering`, `an_explicit_range_matches_realtime_seeking_to_the_same_place` |
| Repeatable execution in one build | `offline::tests::rendering_twice_produces_identical_samples` |

These tests live in `crates/auris-engine/src/graph/schedule.rs`,
`crates/auris-engine/src/renderer.rs` and `crates/auris-engine/src/offline.rs`.
The engine's test plugins deliberately make exact frame and gain assertions
possible without depending on an instrument implementation.

Sustain has separate score/import and performer contracts. In `auris-session`,
`midi_import_preserves_the_pedal_tail_after_the_last_key_release` checks retained
clip extents. In `auris-synth`,
`physical::tests::pedal_defers_piano_release_and_all_sound_off_overrides_it`
checks the built-in piano. In `auris-sampler`,
`overlapping_layered_notes_release_in_order_even_under_the_hold_pedal` checks
note ownership under overlapping sustained notes. A held note without a pedal
is not a substitute for these checks.

```sh
cargo test -p auris-io --test project_format_baselines
cargo test -p auris-engine
cargo test -p auris-session midi_import_preserves_the_pedal_tail
cargo test -p auris-synth pedal_defers_piano_release
cargo test -p auris-sampler overlapping_layered_notes_release
```

The standalone `auris-session` example `pipeline_baseline` complements these
exact tests with same-build built-in rendering and release performance
measurements. It does not create a legacy audio algorithm contract: authored
score data remains the archival guarantee described in `auris_session::guide`.

## Workflow and performance records

The [workflow baseline](render-pipeline-workflow-baseline.md) records conventional
tool operations and the existing GPUI gesture tests. The
[performance baseline](render-pipeline-performance-baseline.md) records workload,
build environment, measurements and reproduction. Timing observations belong to
that machine and build; they are comparison data, not portable CI thresholds.

Continue to run `cargo test --workspace` and
`cargo clippy --workspace --all-targets` before each implementation unit lands.
New pipeline behavior needs its own assertions alongside these references;
passing a migration round trip alone does not prove unchanged playback.
