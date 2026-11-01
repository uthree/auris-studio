# Historical project-format fixtures

These files were written by each historical `auris-core::Project` serializer using
the common [fixture author](generate.rs). They are synthetic regression documents,
not user songs. Their external plugins, SoundFont and audio paths are deliberate
placeholders; loading the document must not require those resources.

| File | Historical core revision | Distinguishing stored behavior |
| --- | --- | --- |
| `v27.auris` | `85e77d27` | Pitch volume swell without an editable volume contour; drum role map; single output |
| `v28.auris` | `83c2affe` | Explicit volume contour; drum role map without named lanes |
| `v29.auris` | `909f2bc8` | Drum editor lanes alongside role assignments |
| `v30.auris` | `c828ab0d` | Eleventh harmony (`V11`) |
| `v31.auris` | `61121f12` | Thirteenth harmony (`V13`); single output |
| `v32.auris` | `730493ad` | Multiple lead outputs and deliberately disconnected audio output |

The version is supplied by the historical crate, never rewritten after serialization.
The fixture author uses the historical Rust output field selected at compilation.
All other score and mix settings are shared, allowing comparisons across versions.
The early historical decoder could not reload nonempty controller maps nested in
internally tagged tracks. Generation therefore uses its serializer directly; the
current loader must preserve those historically writable files, including sustain.

The fixed semantic expectations in `../../project_format_baselines.rs` include:

- Lead track 1 / clip 2: authored notes 60 and 67, a seeded recipe, humanize then
  octave transpose then pitch performance, bend and sustain beyond the final note.
- Drum track 3 / clip 4: kick at note 73, preserved role and derived/named lane.
- Audio track 5 / clip 7: source 6 at `Audio/loop.wav`; SoundFont ID 8.
- Singer track 9 / clip 10: Japanese lyric, phoneme timing, project-local voice
  configuration, source 11 take with fingerprint 987654321 and seed 123.
- Return bus 12, lead inserts 13/14, return insert 15, master insert 16 and sends
  17/18. Insert 14 is bypassed and keyed from drum track 3. Send 17 is pre-fader,
  send 18 post-fader, with independent levels.
- Gain, instrument, effect and send automation retain targets, keys, interpolation
  and endpoint holds. Normalized cutoff values are not converted into display units.
- Tempo changes at tick 3840, a project loop and a repeated source clip.
- Hosted state byte strings, asset paths and next-ID allocation.

Run the regression suite with:

```powershell
cargo test -p auris-io --test project_format_baselines
```

Do not update fixtures merely to make a changed loader pass. Test loading does not
rewrite them; current-writer round trips go into temporary directories.

## Reproducing the historical writes

From the repository root, with the listed commits available locally:

```powershell
./tools/fixtures/generate-project-formats.ps1
```

The script archives only the historical core source into a fresh temporary directory
and builds a small standalone author with offline cached dependencies. It never
changes the active checkout or adds historical code to the application. Each revision
gets a separate build target: archives preserve old mtimes, so sharing build outputs
could reuse an executable from a different revision. The script verifies the stored
format after every generation and fails rather than accepting that mismatch.

The isolated manifests pin the cached serde/base64/thiserror versions used to generate
these fixtures; the script also pins full Git revision IDs. This is
historical model serialization, not a reconstruction of released installers
or their complete dependency locks. Exact historical audio rendering is outside this
fixture set. Temporary source/build directories are reported for inspection.

Review the generated diff and run the regression suite before accepting any change.
