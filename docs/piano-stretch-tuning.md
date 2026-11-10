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

The widths come from the same stiff-string relation and the same key-by-key stiffness the
strings sound with, so the String Stiffness control moves the tuning with the inharmonicity,
and at zero stiffness the piano is exactly in equal temperament. A4 stays at 440 Hz. The
stretch is fixed when the key is struck, so a stiffness automation retunes the next note
rather than one already sounding.

## Where the fundamentals lie

Out from A4 each octave is a little wide, so the fundamentals lie flat of equal temperament in
the bass and sharp in the treble, as a real piano's do. Two tests hold them there.
`a_piano_is_tuned_like_a_piano` keeps A4 within 0.6 cents of 440 Hz, C1 between 6 and 30 cents
flat and C7 between 6 and 30 cents sharp, and both within 1.5 cents of equal temperament once
String Stiffness is zero.
`piano_octaves_lie_between_a_pure_two_to_one_and_a_pure_four_to_two` holds the octaves on C3
and C4 to that balance: at most half a cent wide at the 2:1, and between half a cent narrow
and six cents wide at the 4:2.
