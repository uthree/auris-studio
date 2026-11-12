# Physical Choir

[日本語](physical-choir.ja.md)

Choose **Physical Choir** in the instrument library and play notes or chords on an ordinary
instrument track. Its stable plugin ID is `auris.physical.choir`. It synthesizes sustained,
wordless ensemble vowels locally, with four singers per note and sixteen-note polyphony.

## Controls

| Control | Effect |
| --- | --- |
| Vowel (Oo / Ah / Ee) | Continuously morphs the vocal tract: 0 is rounded "oo", 1 is open "ah", 2 is front "ee" |
| Voice Size | Changes tract length from 14 to 20.5 cm; larger voices have lower resonances at the same played pitch |
| Ensemble Variation | Separates the singers' tuning, vibrato rate/phase, slow pitch drift and tract length |
| Breath | Leaves a gap at the back of the glottis that never closes, and turbulence on the air jet through it |
| Tone | How sharply the folds' flow reaches the tract: low is soft and dark, high is bright |
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

The default patch has a 389 ms attack and a 700 ms release. The attack is the singer's breath: lung pressure is full a third of the way through it, and the folds start on their own once it is enough to set them going. Held chords around C3–C5 are a
useful starting point for an accompaniment pad. Try a lower vowel value and a longer attack
for a rounded background texture, or a higher Tone value and shorter attack for an exposed
ensemble part. A reverb insert can supply the surrounding room.

## Model and verification

No waveform is written anywhere. Each singer has a pair of two-mass vocal folds (after
Steinecke and Herzel): a lower and an upper mass on springs, which the lungs' pressure, the
Bernoulli pressure of the air through them and their collision set swinging on their own. The
flow they let through is solved every step against the pressure wave coming back down the
vocal tract, so the tract pushes back on the source as it does in a throat.

What a singer controls is tension, not frequency. Tension scales the folds' masses down and
their springs up; what actually sounds also depends on breath and vowel, so each period is
measured and the tension corrected towards the note, the way a singer finds a pitch by ear.
A note is sung on folds of the size that sings it comfortably, from a bass's to a soprano's,
and a note bent far from where it started changes register. Every note from C2 to A5 in
every vowel is held within eight cents.

The tract is a short narrow epilarynx followed by eight cylindrical sections with
bidirectional fractional delay lines and passive pressure scattering at area changes. The
epilarynx is what lets the folds start against a wide pharynx. Three fitted area profiles
interpolate between the vowels; lip reflection, distributed losses and a first-order lip
radiation shape the output. A note rising past a vowel's first resonance would stall the
folds, so it is sung with the vowel opened towards "ah" and then the tract shortened, as
sopranos do on top notes.

The vowel profiles and factory source controls are fitted to real sustained vowels from
VocalSet, through these folds, with a separate set of held-out singers. On the training
singers the mel distance is 0.074 (the earlier prescribed-pulse source read 0.072); on the
held-out singers it is 0.102 (0.086), furthest on low male notes. They describe instrument
timbres, rather than measured anatomy of a singer.

The source/filter and tube principles are described in the Oxford phonetics
[tube-model practical](https://www.phon.ox.ac.uk/jcoleman/tubes_practical.html).
The [sndkit tract account](https://pbat.ch/sndkit/tract/) describes the related
Kelly–Lochbaum waveguide family. Auris uses its own pressure-scattering implementation,
area profiles, fractional section delays and ensemble renderer. The fold model, the pitch
held by ear and the formant tuning follow kishida's physically modelled singing demo.

Unit tests measure physical quarter-wave resonances, vowel spectra, the first-resonance
table, every note's pitch across the choir's range, the played fundamental,
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

The four-chord auditions peak at -9.30 dBFS (oo), -8.12 dBFS (ah) and -8.55 dBFS (ee),
with no clipping: every vowel is levelled to the same loudness from C3 to G4, so automating
the vowel does not ride the level.
