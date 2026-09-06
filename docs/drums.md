# Drum tracks

Drums have their own track kind, separate from melodic software instruments. Create a drum
track to start with the built-in kit, or select a SoundFont or hosted instrument for that track.
Changing its sound preserves its drum identity. Composed percussion and imported MIDI channel
10 parts use drum tracks. Older projects migrate explicit percussion metadata while preserving
their stored notes and performance.

## Editing and generation

Opening a drum clip shows the drum editor. Its rows use the track's authored assignments and
the MIDI addresses present in the clip. Drum clips move and paste between drum tracks; melodic
clips remain on melodic or singer tracks.

Generated drum clips have a **Drummer** inspector. Move the performance pad to the right for
greater complexity and upward for greater intensity; a drag is one undoable edit and keeps the
take's seed. Choose a groove, adjust fills, swing and dynamics, or select another take. A kit
with independent writers also exposes each rhythmic voice's complexity, intensity and take.
Editing one writer preserves the other voices and their saved assignments. Authored fixed
rhythms retain their rhythm while intensity remains adjustable. Freezing keeps the current
notes and the drum track while removing the generation recipe.

## Drum sound measurement

Select a drum track and choose **Analyze Drum Sounds** in its inspector or context menu.
The worker triggers every MIDI key at three velocities, twice each, and measures the resulting
stereo audio. Built-in instruments, an explicitly selected SoundFont preset, and stateful hosted
CLAP or VST3 instruments use the same analysis. The inspector shows proposed roles, numeric
fitness, spectral centroid and energy duration. **Use Mapping for New Clips Only** saves the
proposal for future generation, including an empty or partial assignment. **Use Measured Drum
Mapping** also retargets existing generated drum notes and their recipes as one undoable edit.

Analysis itself does not edit the project. The selected instrument's current state is snapshotted
on its owning thread, then restored in an independent worker process. The worker renders the
instrument directly, before track inserts, sends and mixer gain. The parent can cancel a scan or
terminate it at its deadline. Results are rejected when the source has changed since measurement.
Replacing the document cancels its pending scan and clears the previous result.

The command line provides the same operation:

```sh
auris analyze-drums /path/to/Song.auris --track Drums
auris analyze-drums /path/to/Song.auris --track id:5 --first-note 0 --last-note 127 --apply
auris analyze-drums /path/to/Song.auris --track Drums --apply --remap-clips
```

The first command prints a JSON report. `--apply` saves the computed map for future generation;
`--remap-clips` also retargets existing generated drum notes, with a checkpoint before saving.
Untagged manually authored notes keep their pitches. If a generated voice requires a role the
scan did not find, remapping fails without changing the document. MCP and the agent expose
`analyze_drum_kit` with the same options. Its default report summarizes each audible note's
measurement ranges; `include_triggers` returns every recorded trigger. `inspect_composition`
reports individual drum writers; `edit_recipe` accepts `drum_voice` to change one writer's
musical controls.

## Evidence and assignment

The DSP classifier receives only PCM and its sample rate. MIDI addresses identify which key was
triggered; note names, General MIDI assignments, instrument names and sample labels do not enter
its calculations. An authored drum map records a musical choice independently of these measurements.

Channel powers are measured separately, preserving opposite-phase stereo sounds. Measurements
include peak and RMS level, onset delay, time to 90% of the captured energy, sustained energy,
and windowed spectra of the attack, body and tail. Spectral features include band energy,
centroid, flatness, peak concentration and the movement of the low-frequency spectral peak.

Explicit acoustic criteria produce six independent fitness scores: kick, snare, closed hat,
open hat, crash and tom. They describe suitability for a musical role, not probabilities of an
instrument's identity. A bright long sound can fit more than one role. Each note's aggregate
score is its lowest score across the measured velocities and repetitions; the report also keeps
every trigger and its variation. Automatic proposals require a minimum fitness of 0.55, and
roles without an eligible sound remain unassigned. These criteria are engineering hypotheses
that can be evaluated and revised without introducing name-based priors.

Notes are held through the recording window so a short note-off cannot make a sustained bass
appear percussive. Between triggers the worker stops voices and waits for quiet, preserving
round-robin variation instead of resetting the source before every hit. A tail that fails to
settle, invalid PCM, unavailable state restoration or failed processing invalidates the scan.

## Score and rendering

One kit track contains one instrument instance and one clip per section. Each note keeps a
stable voice tag. Composite recipes retain independent writers and fixed form accents, and
scoped performance transforms retain each voice's timing and velocity variation. Whole-kit
regeneration and individual-voice regeneration use the saved assignment. Accepting a partial
map does not invent sounds for absent roles when new clips are generated.

The built-in `auris.synth.drumkit` has a prepared voice pool, tonal kicks and toms, noisy snares,
high-frequency hats and cymbals, and shared open/closed hat choking. Render-time processing
allocates no memory. Its numerical measurements can be reproduced with:

```sh
cargo run -p auris-synth --example measure_drums
```

The session guide describes the crate boundaries. DSP tests check levels and spectral behavior;
session tests cover source isolation, map application, undo and regeneration. CLI integration
tests run the real child-process protocol, save the accepted map, reload it and regenerate.

Installed-plugin smoke tests live in `crates/auris-cli/tests/external_drum_probe.rs`. Set
`AURIS_CLAP_DRUM_SMOKE_PLUGIN` to a local CLAP instrument with an audible initial patch, then run
`cargo test -p auris-cli --test external_drum_probe an_external_clap_snapshot -- --ignored`.
The corresponding VST3 test is `an_external_vst3_snapshot`, using `AURIS_VST3_SMOKE_PLUGIN`.
These checks verify state restoration, real output and isolation; the initial patch need not
receive any drum assignment.
