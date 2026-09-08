# Timbre map

Open **Timbre Map** at the top of the library. Auris measures the built-in melodic instruments
and melodic presets of loaded SoundFonts in the background. The counter shows completed sounds;
Cancel, Close and Escape stop a running scan. Use **Rescan Library** after loading another font.
The prototype accepts up to 512 sounds per scan and keeps the results in memory until the
document is replaced or a new scan starts.

Each point is a sound. Hover for its name or click to hear its reference note. The **All Sounds**
list also reaches coincident points. Selecting a sound highlights its eight nearest neighbors
and lists them under **Similar Sounds**, ordered by feature-space distance (smaller is closer).
Color denotes an acoustic k-means group, not a named instrument class. **Use on Selected Track**
adopts that sound as one undoable edit; browsing and audition do not change the document.

Audition uses the selected playable track, or the first playable track. Its mute, fader, inserts
and routing apply, so choose an unmuted dry track for comparisons. Previews are matched to a
common RMS target with a peak ceiling; closing the map stops the last preview.

## Measurement and coordinates

Every source uses fresh instances for six matched triggers: MIDI 48, 60 and 72 at velocities
0.45 and 0.85, a 600 ms hold, and 400 ms after note-off. The middle-C, higher-velocity recording
is the preview. A source silent under any reference condition is omitted and counted explicitly.
Analysis always measures the raw instrument output before the project mixer.

Each trigger contributes 93 deterministic DSP features:

* Twelve MFCCs (excluding coefficient zero), each with mean and standard deviation, separately
  over attack, held body and release: 72 values. The 32 mel filters cover 40 Hz to the lesser of
  10 kHz and Nyquist. Channel powers are summed before calculating normalized log energies.
* Existing drum-analysis spectra over onset-relative attack, body and tail: low/body/high energy
  fractions, log centroid, flatness and peak concentration, for 18 values.
* Onset delay, time to 90% of captured energy, and sustained-energy ratio: three values.

The six feature blocks are concatenated, preserving corresponding pitch/velocity conditions,
then each of the 558 dimensions is standardized over this catalogue. Constant dimensions become
zero. Euclidean distance in this space drives nearest-neighbor search and deterministic k-means
(rounded square root of the sound count, bounded to 1–12 requested groups). These are exploratory
groups whose usefulness should be checked by listening.

The map displays two principal components, with the fraction of standardized variance retained.
Axes are statistical directions without fixed labels such as brightness or decay. Two-dimensional
distance is only an approximation; similarity ordering always uses all 558 dimensions. Rescanning
a changed catalogue can change both coordinates and groups. The computation needs no pretrained
weights, Python runtime, network service or GPU.

Implementation: `auris_dsp::timbre` extracts and projects features; `Session::timbre_map_job`
snapshots sources for a worker, `TimbreMap::nearest` answers similarity queries, and
`Session::use_timbre_sound` adopts a result. Numeric tests cover level/polarity invariance,
silence and invalid PCM, PCA distance preservation for rank-two fixtures, deterministic grouping,
and degenerate catalogues. A desktop harness test checks read-only browsing and undoable adoption.
