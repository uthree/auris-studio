# Stretch tuning of the physical piano

[日本語](piano-stretch-tuning.ja.md)

Physical Piano does not put its fundamentals on equal temperament. Its strings are stiff, so
their overtones run sharp of the harmonics, and a piano with every fundamental on equal
temperament (A4 = 440 Hz) goes wrong in two ways together.

* **The bass sounds sharp.** The ear hears a low note's pitch in its overtones, so the note
  is heard above its fundamental.
* **Every octave beats.** The lower note's second partial lies above the upper note's
  fundamental, and its fourth further above the upper note's second.

## How it is tuned

The piano is tuned the way a technician tunes one: outward from A4, each octave is widened
to split the difference between a pure 2:1 (the lower note's second partial on the upper
note's fundamental) and a pure 4:2 (its fourth on the upper note's second). Below A2 the
fundamentals thin out, and the octaves are judged on proportionally higher partials, 4:2 and
8:4 at A1 and 8:4 and 16:8 at A0. Above C4 the 4:2 counts for less and less, and an octave
that starts at C6 or higher, where the fourth partial is too high and too short-lived to
judge by, is tuned on the 2:1 alone.

Each key is moved by a twelfth of the width this rule gives the octave that starts on it. An
octave therefore ends as wide as the average of the widths the rule gives the twelve keys it
spans, not as wide as the rule gives the octave itself. From A2 to C4, where the stiffness
changes little from key to key, the two are within a cent of each other. In the bottom
octave, where the rule's width falls with every key, an octave comes out narrower than the
rule gives it; in the treble, where the stiffness climbs with every key, wider.

The widths come from the same stiff-string relation and the same key-by-key stiffness the
strings sound with, so the String Stiffness control moves the tuning with the inharmonicity,
and at zero stiffness the piano is exactly in equal temperament. A4 stays at 440 Hz. The
stretch is fixed when the key is struck, so a stiffness automation retunes the next note
rather than one already sounding.

## Measurement

`tools/eval/piano_stretch.py` reads the tuning off the sound of two pianos. One is
Physical Piano: the factory sound at velocity 0.7 and 48 kHz. The other is a recorded
Steinway model B: thirteen mf notes of the
[University of Iowa Musical Instrument Samples](https://theremin.music.uiowa.edu/MISpiano.html).
Its lowest recorded note is B0, so A0 has no recorded figure.

In each cell the first figure is Physical Piano and the second the Steinway. Every figure is
in cents. A fundamental and a heard pitch are measured from equal temperament: at A4 = 440 Hz
for Physical Piano, and for the Steinway on its own A4, which lies 3.4 cents above 440 Hz.
An octave is the lower note's partial minus the partial of the upper note that it should
meet: below zero the two notes lie further apart than that pure octave, above zero closer.

| Key | Fundamental | Heard pitch | 2:1 octave | 4:2 octave |
|---|---|---|---|---|
| A0 | -21.0 / – | +14.7 / – | -15.5 / – | -12.9 / – |
| C1 | -13.6 / -24.9 | +15.0 / +4.7 | -8.7 / -7.3 | -6.4 / -8.4 |
| A1 | -4.7 / -16.6 | +11.0 / +8.4 | -1.0 / -1.0 | +0.3 / -3.3 |
| C2 | -4.2 / -14.5 | +7.7 / +5.8 | -0.6 / -7.4 | +0.4 / -2.7 |
| A2 | -3.3 / -10.3 | +3.9 / +4.0 | -0.4 / +0.2 | +0.2 / +0.1 |
| C3 | -3.1 / -0.2 | +3.4 / +3.1 | -0.5 / -0.3 | +0.1 / +0.2 |
| C4 | -2.2 / -2.1 | +2.1 / +4.2 | -1.5 / -0.8 | -0.4 / +0.7 |
| A4 | -0.4 / 0.0 | +1.4 / +2.4 | -2.5 / -4.4 | 0.0 / -0.5 |
| C5 | +0.4 / +0.6 | +1.5 / +0.4 | -2.9 / -2.5 | +0.2 / +2.5 |
| C6 | +6.1 / +5.3 | +6.5 / +5.0 | -5.3 / -5.3 | – / – |
| C7 | +19.6 / +16.1 | – / +15.0 | -4.7 / – | – / – |
| C8 | +46.1 / -63.2 | – / – | – | – |

A partial is looked for as the strongest frequency within 150 cents of a whole multiple of
the note's equal-tempered frequency, in the spectrum of the two seconds that start 0.3 s
after the strike, through a Hann window. It is read at the middle of the power within 25
cents of that frequency, because the strings of one key are not tuned exactly together: the
three of the recorded C7 lie 12 cents apart. The two microphones of a recording are added in
power. The heard pitch is the period the note repeats at, the peak of its autocorrelation
from 0.05 to 0.55 s after the strike. That is what a tuner following the waveform shows; it
stands in for the ear and is not a model of it.

### Fundamentals

Physical Piano's fundamentals run from 21 cents flat at A0 to 46 sharp at C8. The 0.4 cents
at A4 is the unison and not the tuning: the strings of one key are tuned a little apart, and
with Unison Detune at zero A4 reads 0.0.

From C3 to C7 they lie within three and a half cents of the Steinway's. Below C3 the
Steinway's are 7 to 12 cents lower, and part of that is not tuning. The recorded C2's second
partial lies 6.9 cents above twice its fundamental, where Physical Piano's lies 0.4 above,
so the recorded fundamental sits lower beneath its own overtones. Read by their second
partials, the recorded C1 and C2 are 9 and 4 cents lower than Physical Piano's.

### Octaves

From A1 to C3 an octave of Physical Piano sits between the two pure octaves, within a cent
of each. From C4 up it runs wider than the rule gives it, because of that averaging: at A4
it has reached the pure 4:2, and the octaves on C6 and C7, which the rule tunes on the 2:1
alone, are about five cents wider than a pure 2:1. The octaves on A0 and C1 are judged on
higher partials than these two columns read, and are wider than both pure octaves.

From A2 up, every octave reading of the Steinway lies within two and a half cents of
Physical Piano's. Its octave on C6 is 5.3 cents wider than a pure 2:1 as well. Below A2 the
two differ more: the Steinway's 2:1 on C2 is 7 cents wider than Physical Piano's, and its
4:2 on A1 and on C2 3 to 4 cents wider.

### Heard pitch

Physical Piano's heard pitch lies on the fundamental at C6, within half a cent. Downwards
the overtones weigh more in it, and a note is heard above its own fundamental: by 4 cents at
C4, 7 at C3, 12 at C2 and 36 at A0. The tuning lowers the fundamentals there, which takes
back 21 of those 36 cents at A0 and 4 of the 12 at C2. By this reading the bass is therefore
still heard above equal temperament: 15 cents in the bottom octave, 11 at A1, 8 at C2 and 4
at A2.

So is the Steinway's bass, by 4 to 8 cents from C1 to A2. From A2 to C6 the two pianos are
heard within about two cents of each other. Below A2 Physical Piano is heard higher: by 2 to
3 cents at A1 and C2, and by 10 at C1.

### What is not given, and how far to trust the rest

No heard pitch is given where the period does not hold still. Physical Piano's C7 and C8 die
away within the half second their period is read over, and the period moves as their
overtones die: read again with the middle of the stretch weighted over its ends, C7 comes out
1.3 cents lower and C8 14, each close to its fundamental. A heard pitch that moves by more
than a cent that way is not one figure.

An octave is not given where either of its partials lies more than 60 dB under the note's
strongest, which is the fourth partial of Physical Piano's C6 and C7. In a recording a
partial must also stand more than 10 dB above what the same file holds in its last two
seconds, because the recordings carry a steady line near 8 kHz. That leaves out the
Steinway's fourth partials of C6 and C7 and the second of C7.

The recorded C8 is out of tune: it lies 79 cents short of an octave above the recorded C7,
and says nothing of how a C8 is tuned.

Read over other stretches, Physical Piano's figures stay within about half a cent. The
Steinway's stay within a cent and a half, except its C7 fundamental and its 2:1 on C6, which
move by two to three.

### Reading it again

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/piano_stretch.py prepare target/piano-stretch
uv run tools/eval/piano_stretch.py measure target/release/examples/physical_fit_render \
  target/piano-stretch.json --recorded target/piano-stretch
```

`prepare` downloads the thirteen recordings, 70 MB, and checks them against the hashes in
[piano-stretch-captures.json](../tools/eval/references/piano-stretch-captures.json). `measure`
without `--recorded` reads Physical Piano alone. On Windows the executable's path ends in
`.exe`. The report holds every figure unrounded, with both readings of each heard pitch.
`tools/eval/test_piano_stretch.py` checks the readings against synthetic strings whose
tuning is known.

## Tests

Two tests in `auris-synth` hold the tuning. `a_piano_is_tuned_like_a_piano` keeps A4 within
0.6 cents of 440 Hz, C1 between 6 and 30 cents flat and C7 between 6 and 30 cents sharp, and
both within 1.5 cents of equal temperament once String Stiffness is zero.
`piano_octaves_lie_between_a_pure_two_to_one_and_a_pure_four_to_two` holds the octaves on C3
and C4: the lower note's second partial at most half a cent above the upper note's
fundamental, and its fourth between half a cent below and six cents above the upper note's
second.
