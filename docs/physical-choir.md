# Physical Choir

Choose **Physical Choir** in the instrument library and play notes or chords on an ordinary
instrument track. Its stable plugin ID is `auris.physical.choir`. It synthesizes sustained,
wordless ensemble vowels locally, with four singers per note and sixteen-note polyphony.

## Controls

| Control | Effect |
| --- | --- |
| Vowel (Oo / Ah / Ee) | Continuously morphs the vocal tract: 0 is rounded "oo", 1 is open "ah", 2 is front "ee" |
| Voice Size | Changes tract length from 14 to 20.5 cm; larger voices have lower resonances at the same played pitch |
| Ensemble Variation | Separates the singers' tuning, vibrato rate/phase, slow pitch drift and tract length |
| Breath | Adds independent aspiration noise to each singer's tract input |
| Tone | Adjusts the glottal source's high-frequency rolloff |
| Vibrato | Sets pitch swing in semitones; MIDI CC1 adds up to 0.3 semitones |
| Stereo Width | Spreads singers across the stereo field; zero centers the ensemble |
| Attack / Release | Shapes the onset and fade of each played note |
| Level | Sets the instrument's output gain |

All controls use the usual parameter editor, plugin-state saving and automation lanes.
Vowel and voice-size changes move sounding tract resonances over approximately 20 ms.
Width and output-gain changes are smoothed over the same time scale. MIDI CC7 controls
channel volume, CC11 controls expression, and pitch bend follows sounding notes within
±24 semitones. Note-off releases one matching held note, including overlapping unisons;
all-notes-off releases the ensemble and all-sound-off uses the shared de-click envelope.

The default patch has a 283 ms attack and a 700 ms release. Held chords around C3–C5 are a
useful starting point for an accompaniment pad. Try a lower vowel value and a longer attack
for a rounded background texture, or a higher Tone value and shorter attack for an exposed
ensemble part. A reverb insert can supply the surrounding room.

A song specification can select it directly:

```toml
form = ["verse"]

[[part]]
name = "choir"
role = "pad"
instrument = "auris.physical.choir"
```

## Model and verification

Each singer has an eight-section cylindrical vocal tract with bidirectional fractional
delay lines and passive pressure scattering at area changes. Designed cross-sectional
profiles interpolate between three vowel configurations. Glottal and lip reflections,
distributed losses and a first-order lip-radiation approximation shape the output.
The prescribed volume-flow source converts to pressure at the glottal area and back to
radiated flow at the lip area, keeping narrow-mouth vowels at playable levels.
The source is a prescribed Rosenberg-style glottal-flow pulse, reconstructed into
band-limited harmonic tables in `prepare`, plus seeded breath noise. This is a reduced
physical acoustic tract driven by a parametric source. The vowel profiles and factory
source controls are fitted to real sustained vowels, with a separate set of held-out
singers. They describe instrument timbres, rather than measured anatomy of a singer.
The [real-recording calibration account](choir-copy-synthesis.md) gives the cohort,
log-mel loss, parameter bounds, results and reproducible development commands.

The source/filter and tube principles are described in the Oxford phonetics
[tube-model practical](https://www.phon.ox.ac.uk/jcoleman/tubes_practical.html).
The [sndkit tract account](https://pbat.ch/sndkit/tract/) describes the related
Kelly–Lochbaum waveguide family. Auris uses its own pressure-scattering implementation,
area profiles, fractional section delays and ensemble renderer.

Unit tests measure physical quarter-wave resonances, vowel spectra, the played fundamental,
chord levels, stereo downmix, controller response and note release. They also compare exact
sample output across block sizes and resets, and count zero callback allocations during
automation, controller changes and voice stealing. A session test saves and reopens an
ordinary instrument track with vowel automation, then compares the rendered audio.

Generate an editable demonstration project and three stereo vowel auditions with:

```sh
cargo run -p auris-session --example choir_preview -- target/choir-preview
```

The four-chord phrase is identical in each audition. `choir-preview.auris` holds the
default "ah" version; `choir-oo.wav`, `choir-ah.wav` and `choir-ee.wav` use vowel values
0, 1 and 2 respectively. The phrase uses only the instrument and the ordinary mix output.

The calibrated four-chord auditions peak at -16.22 dBFS (oo), -8.59 dBFS (ah) and
-13.86 dBFS (ee), with no clipping. The existing nine shipped presets retain identical
WAV hashes and symbolic-ruler output before and after this addition. These regression
checks establish that the added instrument leaves those arrangements unchanged;
the vowel spectra and physical resonance tests describe the new instrument itself.
