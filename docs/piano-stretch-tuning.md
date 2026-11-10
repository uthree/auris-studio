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

The piano is tuned the way a technician tunes one, outward from A4. The octave from A3 to A4
is laid first, its width shared evenly among its twelve keys. Every other key then lies an
octave from a key already tuned, by a width that splits the difference between two pure
octaves. From A2 up these are the 2:1 (the lower note's second partial on the upper note's
fundamental) and the 4:2 (its fourth on the upper note's second). From C2 down they are the
6:3 and the 8:4, and between A2 and C2 the pair moves from the one to the other key by key.

The widths come from the same stiff-string relation and the same key-by-key stiffness the
strings sound with, so the String Stiffness control moves the tuning with the inharmonicity,
and at zero stiffness the piano is exactly in equal temperament. A4 stays at 440 Hz. The
stretch is fixed when the key is struck, so a stiffness automation retunes the next note
rather than one already sounding.

Both the stiffness and the rule are held against a recorded Steinway, read below. The
stiffness of Physical Piano's strings is drawn through that Steinway's, and with it the rule
lays the strings within a cent of where the Steinway's lie.

## Measurement

`tools/eval/piano_stretch.py` reads the tuning off the sound of two pianos. One is
Physical Piano: the factory sound at velocity 0.7 and 48 kHz. The other is a recorded
Steinway model B: twenty-eight mf notes of the
[University of Iowa Musical Instrument Samples](https://theremin.music.uiowa.edu/MISpiano.html).
Its lowest recorded note is B0, so A0 has no recorded figure.

In each cell the first figure is Physical Piano and the second the Steinway. Every figure is
in cents. A fundamental and a heard pitch are measured from equal temperament: at A4 = 440 Hz
for Physical Piano, and for the Steinway on its own A4, which lies 3.4 cents above 440 Hz.
An octave is the lower note's partial minus the partial of the upper note that it should
meet: below zero the two notes lie further apart than that pure octave, above zero closer.

| Key | Fundamental | Heard pitch | 2:1 octave | 4:2 octave |
|---|---|---|---|---|
| A0 | -15.0 / – | +15.0 / – | -9.1 / – | -6.7 / – |
| B0 | -14.5 / -22.4 | +10.8 / +9.7 | -8.7 / -2.1 | -6.5 / -6.1 |
| C1 | -14.2 / -24.9 | +12.8 / +4.7 | -8.1 / -7.3 | -5.9 / -8.4 |
| A1 | -5.3 / -16.6 | +7.9 / +8.4 | -3.0 / -1.0 | -2.1 / -3.3 |
| C2 | -5.4 / -14.5 | +5.3 / +5.8 | -3.3 / -7.4 | -2.4 / -2.7 |
| A2 | -2.0 / -10.3 | +4.2 / +4.0 | -0.2 / +0.2 | +0.4 / +0.1 |
| C3 | -1.7 / -0.2 | +3.4 / +3.1 | -0.1 / -0.3 | +0.3 / +0.2 |
| C4 | -1.3 / -2.1 | +2.3 / +4.2 | -0.5 / -0.8 | +0.2 / +0.7 |
| A4 | -0.4 / 0.0 | +1.2 / +2.4 | -1.8 / -4.4 | +1.5 / -0.5 |
| C5 | +0.1 / +0.6 | +1.1 / +0.4 | -2.7 / -2.5 | +2.3 / +2.5 |
| C6 | +5.4 / +5.3 | +5.7 / +5.0 | -4.9 / -5.3 | – / – |
| C7 | +15.9 / +16.1 | – / +15.0 | -10.9 / – | – / – |
| C8 | +39.6 / -63.2 | – / – | – | – |

The bass octaves are tuned on higher partials than those two columns read. These are the
same octaves on the 6:3 and the 8:4:

| Key | 6:3 octave | 8:4 octave |
|---|---|---|
| A0 | -2.8 / – | +2.7 / – |
| B0 | -2.6 / +1.5 | +2.6 / 0.0 |
| C1 | -2.2 / -4.0 | +3.0 / -1.2 |
| A1 | -0.5 / -1.6 | +1.6 / -0.8 |
| C2 | -0.9 / -0.3 | +1.2 / -0.3 |
| A2 | +1.3 / +1.1 | +2.7 / +2.2 |

A partial is looked for as the strongest frequency within 150 cents of a whole multiple of
the note's equal-tempered frequency, in the spectrum of the two seconds that start 0.3 s
after the strike, through a Hann window. It is read at the middle of the power within 25
cents of that frequency, because the strings of one key are not tuned exactly together: the
three of the recorded C7 lie 12 cents apart. The two microphones of a recording are added in
power. The heard pitch is the period the note repeats at, the peak of its autocorrelation
from 0.05 to 0.55 s after the strike. That is what a tuner following the waveform shows; it
stands in for the ear and is not a model of it.

### The strings

How the octaves and the fundamentals read depends on the strings as well as on the tuning,
so the tool reads the strings too. Partial *n* of a stiff string lies at *n f* √(1 + *B n*²),
which makes (*f*ₙ / *n*)² a straight line against *n*². The stiffness *B* is read off that
line, drawn through up to forty partials below 10 kHz, and the same line says where the first
partial would lie if it stood on it. The last column is how far the first partial lies from
there.

| Key | Stiffness B × 10⁴ | First partial by the line | First partial from the line |
|---|---|---|---|
| A0 | 2.60 / – | -15.0 / – | 0.0 / – |
| B0 | 2.51 / 2.54 | -14.4 / -15.2 | 0.0 / -7.2 |
| C1 | 2.47 / 2.47 | -14.2 / -19.2 | 0.0 / -5.7 |
| A1 | 1.20 / 1.14 | -5.3 / -6.0 | +0.1 / -10.5 |
| C2 | 1.20 / 1.32 | -5.5 / -5.8 | +0.1 / -8.7 |
| A2 | 1.20 / 1.24 | -1.9 / -2.6 | -0.1 / -7.7 |
| C3 | 1.20 / 1.17 | -1.6 / -2.4 | -0.1 / +2.2 |
| C4 | 3.17 / 3.15 | -1.2 / -0.3 | -0.1 / -1.8 |
| A4 | 7.68 / 7.59 | -0.4 / 0.0 | 0.0 / 0.0 |
| C5 | 10.32 / 10.33 | 0.0 / +0.2 | 0.0 / +0.5 |
| C6 | 21.96 / 21.60 | +5.4 / +5.3 | 0.0 / 0.0 |
| C7 | 50.01 / – | +15.9 / – | 0.0 / – |
| C8 | 110.03 / – | +39.6 / – | 0.0 / – |

Physical Piano's stiffness is the Steinway's. The instrument's own table of it,
`grand::INHARMONICITY`, is drawn through what the tool reads off the Steinway at twenty-six
keys:

| Key | Physical Piano, B × 10⁴ | Steinway, B × 10⁴ | Partials the Steinway's is read from |
|---|---|---|---|
| B0 | 2.51 | 2.54 | 36 |
| C1 | 2.47 | 2.47 | 35 |
| E1 | 1.38 | 1.38 | 35 |
| G1 | 1.20 | 1.09 | 33 |
| A1 | 1.20 | 1.14 | 36 |
| B1 | 1.20 | 1.27 | 31 |
| C2 | 1.20 | 1.32 | 32 |
| E2 | 1.20 | 1.11 | 36 |
| G2 | 1.20 | 1.27 | 33 |
| A2 | 1.20 | 1.24 | 32 |
| C3 | 1.20 | 1.17 | 29 |
| E3 | 1.66 | 1.55 | 21 |
| G3 | 2.11 | 2.08 | 17 |
| A3 | 2.49 | 2.28 | 16 |
| C4 | 3.17 | 3.15 | 14 |
| E4 | 4.70 | 4.92 | 11 |
| G4 | 6.31 | 6.60 | 11 |
| A4 | 7.68 | 7.59 | 8 |
| C5 | 10.32 | 10.33 | 5 |
| E5 | 13.29 | 12.59 | 7 |
| G5 | 16.03 | 15.95 | 5 |
| A5 | 18.18 | 18.53 | 4 |
| C6 | 21.96 | 21.60 | 2 |
| E6 | 28.85 | 35.29 | 2 |
| G6 | 35.48 | 37.01 | 2 |
| A6 | 40.72 | 40.18 | 2 |
| C7 | 50.01 | – | – |
| C8 | 110.03 | – | – |

From B0 to A5, where three or more partials can be read, the table is within 10 percent of
every reading. From C6 to A6 the recording shows two partials, which make a line with
nothing to check it by, and those readings are good to a third. Above A6 it shows no second
partial at all, and the table goes on as it rose through the two octaves below.

What the two pianos do not share is in the lowest partials. Physical Piano's stand on their
lines, none of the first three more than 0.15 cents off. From A2 down the Steinway's first
partial lies 6 to 11 cents under its string's line, and from C2 down its second lies 2 to 6
cents under and its third 1 to 2.5.

By the lines themselves, the two pianos are tuned within a cent of each other on every key
from B0 to C6 but one: the recorded C1 lies 5 cents lower.

### Octaves

From A2 up an octave of Physical Piano sits midway between its pure 2:1 and its pure 4:2,
and from C2 down midway between its pure 6:3 and its pure 8:4: the two readings are equal
and opposite to within about a cent.

The Steinway's octaves agree. From A2 to C6 its 2:1 and its 4:2 read within half a cent of
Physical Piano's, except on A4, where its octave is two to two and a half cents wider.

From B0 to C2 the Steinway's octaves lie within a cent and a quarter of its pure 8:4, where
Physical Piano's lie between the 6:3 and the 8:4. An 8:4 meets on partials that stand on
their strings' lines in both pianos. A 2:1 and a 4:2 meet on the upper note's first and
second partials, which in the Steinway's bass lie under theirs. On the 4:2 the two pianos'
bass octaves read alike all the same: within a cent and a quarter on B0, A1 and C2, and two
and a half cents apart on C1.

### Fundamentals

Physical Piano's fundamentals run from 15 cents flat at A0 to 40 sharp at C8. The 0.4 cents
at A4 is the unison and not the tuning: the strings of one key are tuned a little apart, and
with Unison Detune at zero A4 reads 0.0.

From C3 to C7 they lie within a cent and a half of the Steinway's. Below C3 the Steinway's
are 8 to 11 cents lower. That is its first partials lying under their strings' lines, and
not its tuning.

### Heard pitch

Physical Piano's heard pitch lies on the fundamental at C6, within half a cent. Downwards
the overtones weigh more in it, and a note is heard above its own fundamental: by 4 cents at
C4, 5 at C3, 11 at C2 and 30 at A0. The tuning lowers the fundamentals there, which takes
back 15 of those 30 cents at A0 and 5 of the 11 at C2. By this reading the bass is therefore
still heard above equal temperament: 15 cents at A0, 11 to 13 at B0 and C1, 8 at A1 and 5 at
C2.

So is the Steinway's bass, by 4 to 10 cents from B0 to A2. From B0 to C6 the two pianos are
heard within two cents of each other, except at C1, where Physical Piano is heard 8 cents
higher: the recorded C1 is the one whose string lies low.

### What is not given, and how far to trust the rest

No heard pitch is given where the period does not hold still. Physical Piano's C7 and C8 die
away within the half second their period is read over, and the period moves as their
overtones die: read again with the middle of the stretch weighted over its ends, C7 comes out
1.3 cents lower and C8 13, each close to its fundamental. A heard pitch that moves by more
than a cent that way is not one figure.

An octave is not given where either of its partials lies more than 60 dB under the note's
strongest, which is the fourth partial of Physical Piano's C6 and C7. In a recording a
partial must also stand more than 10 dB above what the same file holds in its last two
seconds, because the recordings carry a steady line near 8 kHz. That leaves out the
Steinway's fourth partials of C6 and C7 and the second of C7.

The recorded C8 is out of tune: it lies 79 cents short of an octave above the recorded C7,
and says nothing of how a C8 is tuned.

Read over other stretches, Physical Piano's figures stay within about half a cent. The
Steinway's stay within two cents, except its C7 fundamental and its 2:1 on C6, which move by
two to three. Its stiffness moves by up to 3 percent with the stretch read and the number of
partials followed, and by 8 at A1 and C2; its strings' lines move by up to 2 cents from B0 to
C2 and by about half a cent above. One recorded piano, tuned once, is all the comparison
rests on.

### Reading it again

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/piano_stretch.py prepare target/piano-stretch
uv run tools/eval/piano_stretch.py measure target/release/examples/physical_fit_render \
  target/piano-stretch.json --recorded target/piano-stretch
```

`prepare` downloads the twenty-eight recordings, 157 MB, and checks them against the hashes
in [piano-stretch-captures.json](../tools/eval/references/piano-stretch-captures.json).
`measure` without `--recorded` reads Physical Piano alone. On Windows the executable's path
ends in `.exe`. The report holds every figure unrounded, with both readings of each heard
pitch. `tools/eval/test_piano_stretch.py` checks the readings against synthetic strings
whose tuning is known.

## Tests

Three tests in `auris-synth` hold the tuning. `a_piano_is_tuned_like_a_piano` keeps A4
within 0.6 cents of 440 Hz, C1 between 6 and 30 cents flat and C7 between 6 and 30 cents
sharp, and both within 1.5 cents of equal temperament once String Stiffness is zero.
`piano_octaves_lie_between_a_pure_two_to_one_and_a_pure_four_to_two` holds the octaves on
C3, C4, C5 and C6: the lower note's second partial no more than half a cent above the upper
note's fundamental, and its fourth no more than half a cent below the upper note's second.
`piano_bass_octaves_lie_between_a_pure_six_to_three_and_a_pure_eight_to_four` holds the
octaves on A0, C1, A1 and C2 the same way, on the sixth partial with the third and on the
eighth with the fourth. Neither lets a partial lie more than six cents the other way.
