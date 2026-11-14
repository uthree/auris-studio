# Genre: Chiptune

The sound of game consoles: few voices at once, chords played as fast arpeggios, pulse and triangle waves, noise drums, no reverb, and every note exact on the grid.

## Score

- Tempo 120 to 180 BPM in 4/4. Form is an eight- or sixteen-bar loop with an A and a B section, built to repeat; a title theme may take a short intro.
- Harmony is bright and quick: I V vi IV, vi IV V I, i bVI bVII i, a secondary dominant into each new phrase, a key change up a step for the second time round. Chords change every bar or half bar.
- The melody is the identity: short motifs repeated and sequenced up a step, fast runs and grace notes, wide leaps no singer would take. Only three or four voices sound at once, so every voice is a line.
- Look up Motif repetition and development and The chorus hook in the intro.

## Arrangement

- Voices on `auris.synth.chiptune`: a pulse lead (12.5 or 25 percent duty) for the melody, a second pulse for the harmony or an echo of the lead, a triangle bass (`synth_bass_tight`) in eighths or octave jumps, noise drums on `auris.synth.noisedrum` for kick, snare and hats. Chords as fast arpeggios (`synth_arp_tight`) rather than held stacks.
- Keep it exact: swing 50, no humanize, short gates on the bass and the arpeggio. Build by adding the second pulse and doubling the drum pattern in the B section.
- Look up Loop return and Copy, then vary.

## Mix

- Dry and direct: no reverb, a short echo on the lead at most, the lead clearly on top. Target -12 to -14 LUFS, -1 dBTP.
