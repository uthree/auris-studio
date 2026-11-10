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

Factory sound, velocity 0.7, 48 kHz, read by `tools/eval/piano_stretch.py`. Every figure is in
cents. A fundamental and a heard pitch are measured from equal temperament. An octave is the
lower note's partial minus the partial of the upper note that it should meet: below zero the
two notes lie further apart than that pure octave, above zero closer.

| Key | Fundamental | Heard pitch | 2:1 octave | 4:2 octave |
|---|---|---|---|---|
| A0 | -21.0 | +14.7 | -15.5 | -12.9 |
| C1 | -13.6 | +15.0 | -8.7 | -6.4 |
| A1 | -4.7 | +11.0 | -0.9 | +0.3 |
| C2 | -4.2 | +7.7 | -0.6 | +0.4 |
| A2 | -3.3 | +3.9 | -0.3 | +0.2 |
| C3 | -3.2 | +3.4 | -0.4 | +0.1 |
| C4 | -2.3 | +2.1 | -1.6 | -0.3 |
| A4 | -0.4 | +1.4 | -2.5 | 0.0 |
| C5 | +0.4 | +1.5 | -2.9 | +0.3 |
| C6 | +6.1 | +6.5 | -5.3 | – |
| C7 | +19.6 | – | -4.7 | – |
| C8 | +46.1 | – | – | – |

A partial is the strongest frequency within 150 cents of a whole multiple of the note's
equal-tempered frequency, in the spectrum of the note from 0.3 to 2.3 s through a Hann
window. The heard pitch is the period the note repeats at, the peak of its autocorrelation
from 0.05 to 0.55 s. That is what a tuner following the waveform shows; it stands in for the
ear and is not a model of it.

The fundamentals run from 21 cents flat at A0 to 46 sharp at C8. The 0.4 cents at A4 is the
unison and not the tuning: the strings of one key are tuned a little apart, and with
Unison Detune at zero A4 reads 0.0.

From A1 to C3 an octave sits between the two pure octaves, under a cent from each. From C4
up it runs wider than the rule gives it, because of that averaging: at A4 it has reached the
pure 4:2, and the octaves on C6 and C7, which the rule tunes on the 2:1 alone, are about five
cents wider than a pure 2:1. The octaves on A0 and C1 are judged on higher partials than
these two columns read, and are wider than both pure octaves.

The heard pitch lies on the fundamental at C6, within half a cent. Downwards the overtones
weigh more in it, and a note is heard above its own fundamental: by 4 cents at C4, 7 at C3,
12 at C2 and 36 at A0. The tuning lowers the fundamentals there, which takes back 21 of
those 36 cents at A0 and 4 of the 12 at C2. By this reading the bass is therefore still heard
above equal temperament: 15 cents in the bottom octave, 11 at A1, 8 at C2 and 4 at A2.

No heard pitch is given for C7 and C8. They die away within the half second their period is
read over, and the period moves as their overtones die: read again with the middle of the
stretch weighted over its ends, C7 comes out 1.3 cents lower and C8 14, each close to its
fundamental. A heard pitch that moves by more than a cent that way is not one figure. An
octave is not given where either of its partials lies more than 60 dB under the note's
strongest, which is the fourth partial of C6 and of C7.

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/piano_stretch.py target/release/examples/physical_fit_render \
  target/piano-stretch.json
```

On Windows the executable's path ends in `.exe`. The report holds every figure unrounded,
with both readings of each heard pitch. `tools/eval/test_piano_stretch.py` checks the
readings against synthetic strings whose tuning is known.

## Tests

Two tests in `auris-synth` hold the tuning. `a_piano_is_tuned_like_a_piano` keeps A4 within
0.6 cents of 440 Hz, C1 between 6 and 30 cents flat and C7 between 6 and 30 cents sharp, and
both within 1.5 cents of equal temperament once String Stiffness is zero.
`piano_octaves_lie_between_a_pure_two_to_one_and_a_pure_four_to_two` holds the octaves on C3
and C4: the lower note's second partial at most half a cent above the upper note's
fundamental, and its fourth between half a cent below and six cents above the upper note's
second.
