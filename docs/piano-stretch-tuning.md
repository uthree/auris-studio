# Stretch tuning of the physical piano

[日本語](piano-stretch-tuning.ja.md)

Physical Piano used to put every fundamental exactly on equal temperament (A4 = 440 Hz).
Its strings are stiff, so their overtones run sharp of the harmonics: at the factory
stiffness the eighth partial of a tenor note sits about 40 cents above eight times the
fundamental. With the fundamentals on equal temperament, two things went wrong together.

* **The bass sounded sharp.** The soundboard radiates little below 200 Hz, so a low note's
  pitch is heard in its overtones. A tuner reading of the rendered note (the peak of its
  autocorrelation) was 7–10 cents sharp from C2 to C4 and 25–33 cents sharp at C1 and A0,
  against 0 in the treble.
* **Every octave beat.** The lower note's fourth partial sat 7.6 cents above the upper note's
  second across the whole keyboard, and its second partial 1.8 cents above the upper
  fundamental.

The fundamentals themselves were accurate to within a cent at 44.1, 48 and 96 kHz and at
every velocity; the modal oscillators have no fractional delay to get wrong. The fault was the
tuning, not the oscillators.

## What changed

The piano is now tuned the way a technician tunes one: outward from A4, each octave is widened
to split the difference between a pure 2:1 (the lower note's second partial on the upper
note's fundamental) and a pure 4:2 (its fourth on the upper note's second). Below A2 the
fundamentals thin out, and the octaves are judged on proportionally higher partials, 4:2 and
8:4 at A1 and 8:4 and 16:8 at A0. The widths come from the same stiff-string relation and the
same register-scaled stiffness the strings sound with, so the String Stiffness control moves
the tuning with the inharmonicity, and at zero stiffness the piano is exactly in equal
temperament. A4 stays at 440 Hz. `calibration::piano_stretch` is the implementation; the
stretch is fixed when the key is struck, so a stiffness automation retunes the next note
rather than one already sounding.

## Measurement

Factory sound with Body Resonance at its default, velocity 0.7, 48 kHz. Cents from equal
temperament; an octave is the lower note's partial minus the upper note's, so a positive value
is a wide, beating octave. Partials are Goertzel peaks over 0.3–2.3 s; heard pitch is the
autocorrelation peak over 0.05–0.55 s.

| Key | Fundamental | Heard pitch | 2:1 octave | 4:2 octave |
|---|---|---|---|---|
| A0 | +0.1 → -25.3 | +33.1 → +7.8 | +0.2 → -13.7 | +2.6 → -12.3 |
| C1 | +0.3 → -18.9 | +25.2 → +7.1 | +0.9 → -7.5 | +3.4 → -4.5 |
| A1 | +0.5 → -11.1 | +10.2 → -1.2 | +0.9 → +0.0 | +1.0 → -0.5 |
| C2 | +0.0 → -10.5 | +8.7 → -1.7 | +0.5 → -1.6 | +0.2 → -2.0 |
| A2 | -0.5 → -10.4 | +10.4 → +0.2 | +1.9 → -2.8 | +6.5 → +1.9 |
| C3 | -0.5 → -9.0 | +10.0 → +1.3 | +2.2 → -2.7 | +7.8 → +3.0 |
| C4 | -0.7 → -4.3 | +7.3 → +3.4 | +1.8 → -3.0 | +7.7 → +2.9 |
| A4 | -0.4 → -0.4 | +2.4 → +2.4 | +1.8 → -3.0 | +7.6 → +2.8 |
| C5 | -0.4 → +0.8 | +1.5 → +2.7 | +1.8 → -3.1 | +7.5 → +2.7 |
| C6 | -0.2 → +5.9 | +0.0 → +6.1 | +1.9 → -3.0 | +7.6 → +2.8 |
| C7 | -0.1 → +10.8 | +0.0 → +10.6 | +1.9 → -3.0 | +7.7 → +2.9 |
| C8 | +0.0 → +15.7 | +5.8 → +23.2 | – | – |

The treble now reads sharp on a tuner, as a real piano's does: a concert grand's C8 is
typically 25–35 cents above equal temperament. Here the stretch is smaller, because the
model's stiffness stays at its tenor value in the treble where a real string's rises. The
heard pitch at C8 is unreliable, since its period is only eleven samples. The remaining
-0.4 cents at A4 is the uneven hammer favouring the flat string of the detuned unison.

`piano_octaves_are_stretched_between_pure_two_one_and_four_two` holds the tenor octaves to
that balance, and failed on the previous tuning with a 4:2 octave of +7.8 cents at C3.
