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
to split the difference between two pure octaves. From A2 up they are the 2:1 (the lower
note's second partial on the upper note's fundamental) and the 4:2 (its fourth on the upper
note's second). From C2 down they are the 6:3 and the 8:4, and between A2 and C2 the pair
moves from the one to the other key by key. Above C4 the 4:2 counts for less and less, and
an octave that starts at C6 or higher, where the fourth partial is too high and too
short-lived to judge by, is tuned on the 2:1 alone.

From A2 up, each key is moved by a twelfth of the width this rule gives the octave that
starts on it. An octave therefore ends as wide as the average of the widths the rule gives
the twelve keys it spans, not as wide as the rule gives the octave itself. Up to C4, where
the stiffness changes little from key to key, the two are within a cent of each other; in
the treble, where the stiffness climbs with every key, the octave comes out wider.

Below A2 the rule's width changes too much from key to key to be shared out that way: the
octaves on A1 and C2 would come out about half as wide as the rule gives them. There each
key lies an octave under the key above it, by the width the rule gives its own octave.

The widths come from the same stiff-string relation and the same key-by-key stiffness the
strings sound with, so the String Stiffness control moves the tuning with the inharmonicity,
and at zero stiffness the piano is exactly in equal temperament. A4 stays at 440 Hz. The
stretch is fixed when the key is struck, so a stiffness automation retunes the next note
rather than one already sounding.

## Measurement

`tools/eval/piano_stretch.py` reads the tuning off the sound of two pianos. One is
Physical Piano: the factory sound at velocity 0.7 and 48 kHz. The other is a recorded
Steinway model B: fifteen mf notes of the
[University of Iowa Musical Instrument Samples](https://theremin.music.uiowa.edu/MISpiano.html).
Its lowest recorded note is B0, so A0 has no recorded figure.

In each cell the first figure is Physical Piano and the second the Steinway. Every figure is
in cents. A fundamental and a heard pitch are measured from equal temperament: at A4 = 440 Hz
for Physical Piano, and for the Steinway on its own A4, which lies 3.4 cents above 440 Hz.
An octave is the lower note's partial minus the partial of the upper note that it should
meet: below zero the two notes lie further apart than that pure octave, above zero closer.

| Key | Fundamental | Heard pitch | 2:1 octave | 4:2 octave |
|---|---|---|---|---|
| A0 | -19.2 / – | +16.5 / – | -10.2 / – | -7.5 / – |
| B0 | -17.3 / -22.4 | +10.7 / +9.7 | -9.3 / -2.1 | -6.9 / -6.1 |
| C1 | -16.4 / -24.9 | +12.2 / +4.7 | -8.5 / -7.3 | -6.1 / -8.4 |
| A1 | -8.2 / -16.6 | +7.9 / +8.4 | -4.5 / -1.0 | -3.2 / -3.3 |
| C2 | -7.2 / -14.5 | +4.7 / +5.8 | -3.7 / -7.4 | -2.7 / -2.7 |
| A2 | -3.3 / -10.3 | +3.9 / +4.0 | -0.4 / +0.2 | +0.2 / +0.1 |
| C3 | -3.1 / -0.2 | +3.4 / +3.1 | -0.5 / -0.3 | +0.1 / +0.2 |
| C4 | -2.2 / -2.1 | +2.1 / +4.2 | -1.5 / -0.8 | -0.4 / +0.7 |
| A4 | -0.4 / 0.0 | +1.4 / +2.4 | -2.5 / -4.4 | 0.0 / -0.5 |
| C5 | +0.4 / +0.6 | +1.5 / +0.4 | -2.9 / -2.5 | +0.2 / +2.5 |
| C6 | +6.1 / +5.3 | +6.5 / +5.0 | -5.3 / -5.3 | – / – |
| C7 | +19.6 / +16.1 | – / +15.0 | -4.7 / – | – / – |
| C8 | +46.1 / -63.2 | – / – | – | – |

The bass octaves are tuned on higher partials than those two columns read. These are the
same octaves on the 6:3 and the 8:4:

| Key | 6:3 octave | 8:4 octave |
|---|---|---|
| A0 | -3.1 / – | +3.1 / – |
| B0 | -2.8 / +1.5 | +2.8 / 0.0 |
| C1 | -2.3 / -4.0 | +3.1 / -1.2 |
| A1 | -1.0 / -1.6 | +2.1 / -0.8 |
| C2 | -1.0 / -0.3 | +1.4 / -0.3 |
| A2 | +1.3 / +1.1 | +2.8 / +2.2 |

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
| A0 | 3.00 / – | -19.1 / – | 0.0 / – |
| B0 | 2.72 / 2.54 | -17.2 / -15.2 | 0.0 / -7.2 |
| C1 | 2.60 / 2.47 | -16.3 / -19.2 | 0.0 / -5.7 |
| A1 | 1.63 / 1.14 | -8.2 / -6.0 | +0.1 / -10.5 |
| C2 | 1.40 / 1.32 | -7.3 / -5.8 | +0.1 / -8.7 |
| A2 | 1.42 / 1.24 | -3.2 / -2.6 | -0.1 / -7.7 |
| C3 | 1.60 / 1.17 | -3.0 / -2.4 | -0.1 / +2.2 |
| C4 | 3.82 / 3.15 | -2.0 / -0.3 | -0.1 / -1.8 |
| A4 | 8.45 / 7.59 | -0.4 / 0.0 | 0.0 / 0.0 |
| C5 | 11.02 / 10.33 | +0.4 / +0.2 | 0.0 / +0.5 |
| C6 | 31.96 / – | +6.1 / – | 0.0 / – |
| C7 | 86.01 / – | +19.6 / – | 0.0 / – |
| C8 | – / – | – / – | – / – |

Physical Piano's partials stand on their lines, none of the first three more than 0.15 cents
off, and the stiffness read off its sound is the stiffness in the instrument's own table to
within one percent.

The Steinway's strings are a little less stiff: by 5 to 7 percent at B0, C1, C2 and C5, by
10 to 17 at A2, C4 and A4, and by 27 to 30 at A1 and C3.

The larger difference is in the lowest partials. From A2 down the Steinway's first partial
lies 6 to 11 cents under its string's line, and from C2 down its second lies 2 to 6 cents
under and its third 1 to 2.5. Physical Piano has nothing of the kind. By the lines
themselves, the two pianos are tuned within 3 cents of each other on every key from B0 to C5.

### Octaves

On A2 and C3 an octave of Physical Piano sits between the pure 2:1 and the pure 4:2, about
half a cent from each at most. From C4 up it runs wider than the rule gives it, because of
that averaging: at A4 it has reached the pure 4:2, and the octaves on C6 and C7, which the
rule tunes on the 2:1 alone, are about five cents wider than a pure 2:1. From C2 down it
sits between the pure 6:3 and the pure 8:4, one to three cents from each.

From A2 up, every 2:1 and 4:2 reading of the Steinway lies within two and a half cents of
Physical Piano's. Its octave on C6 is 5.3 cents wider than a pure 2:1 as well.

From B0 to C2 the Steinway's octaves lie within a cent and a quarter of its pure 8:4. An
8:4 meets on partials that stand on their strings' lines in both pianos. A 2:1 and a 4:2
meet on the upper note's first and second partials, which in the Steinway's bass lie under
theirs. So on a pure 8:4 Physical Piano's 4:2 would read one to four cents wider than the
Steinway's. Tuned between the 6:3 and the 8:4 instead, its 4:2 on B0, A1 and C2 reads within
a cent of the Steinway's, and on C1 two cents narrower. The 2:1 still differs, by up to 7
cents.

### Fundamentals

Physical Piano's fundamentals run from 19 cents flat at A0 to 46 sharp at C8. The 0.4 cents
at A4 is the unison and not the tuning: the strings of one key are tuned a little apart, and
with Unison Detune at zero A4 reads 0.0.

From C3 to C7 they lie within three and a half cents of the Steinway's. Below C3 the
Steinway's are 5 to 9 cents lower. That is its first partials lying under their strings'
lines, and not its tuning: by the lines the two pianos are within 3 cents of each other.

### Heard pitch

Physical Piano's heard pitch lies on the fundamental at C6, within half a cent. Downwards
the overtones weigh more in it, and a note is heard above its own fundamental: by 4 cents at
C4, 7 at C3, 12 at C2 and 36 at A0. The tuning lowers the fundamentals there, which takes
back 19 of those 36 cents at A0 and 7 of the 12 at C2. By this reading the bass is therefore
still heard above equal temperament: 17 cents at A0, 11 to 12 at B0 and C1, 8 at A1 and 5 at
C2.

So is the Steinway's bass, by 4 to 10 cents from B0 to A2. From B0 to C6 the two pianos are
heard within about two cents of each other, except at C1, where Physical Piano is heard 7.5
cents higher: the recorded C1 lies lower than the recorded notes either side of it.

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

`prepare` downloads the fifteen recordings, 84 MB, and checks them against the hashes in
[piano-stretch-captures.json](../tools/eval/references/piano-stretch-captures.json). `measure`
without `--recorded` reads Physical Piano alone. On Windows the executable's path ends in
`.exe`. The report holds every figure unrounded, with both readings of each heard pitch.
`tools/eval/test_piano_stretch.py` checks the readings against synthetic strings whose
tuning is known.

## Tests

Three tests in `auris-synth` hold the tuning. `a_piano_is_tuned_like_a_piano` keeps A4
within 0.6 cents of 440 Hz, C1 between 6 and 30 cents flat and C7 between 6 and 30 cents
sharp, and both within 1.5 cents of equal temperament once String Stiffness is zero.
`piano_octaves_lie_between_a_pure_two_to_one_and_a_pure_four_to_two` holds the octaves on C3
and C4: the lower note's second partial at most half a cent above the upper note's
fundamental, and its fourth between half a cent below and six cents above the upper note's
second. `piano_bass_octaves_lie_between_a_pure_six_to_three_and_a_pure_eight_to_four` holds
the octaves on A0, C1, A1 and C2: the lower note's sixth partial below the upper note's
third and its eighth above the upper note's fourth, each by 0.3 to 6 cents.
