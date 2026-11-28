# Genre: Orchestral and film score

No drum kit: families of instruments hand material on, dynamics and orchestration build the form, and the mix keeps a wide dynamic range at a lower loudness.

## Score

- Tempo 60 to 140 BPM, in 4/4, 3/4 or 6/8 as the scene asks. Form follows the drama: a theme stated quietly, developed, and returned in full at the climax, rather than verse and chorus.
- Harmony is triads and suspensions with stepwise voice-leading: i VI III VII, i iv V, chromatic mediants (I to bVI or III) for colour, a pedal in the bass under changing chords, and Vsus4 resolving to V at cadences.
- The theme is a singable line of four or eight bars with a clear contour and one climax, given first to one family and later to another. A counter-line answers it in the gaps.
- Look up Phrase contour and climax, Motif repetition and development and Section harmony.

## Arrangement

- Strings carry the harmony (`string_legato` on `auris.physical.violin` in several registers, `string_pizzicato` for motion), woodwinds the theme (`woodwind_solo` on `auris.physical.tin_whistle` with `instrument` 1 for a flute or 2 for a piccolo, and on `auris.physical.clarinet` with 0 for a clarinet, 1 an oboe, 2 a cor anglais, 3 a bassoon), brass for weight and the climax (`brass_section` on `auris.physical.brass`, one track per instrument with its `instrument` set: trumpet 0, horn 1, trombone 2, tuba 3; a horn carries a theme as well as a woodwind does), `auris.physical.harp` for arpeggios and glissandi, `auris.physical.timpani` on the tonic and the dominant for the floor of a climax (its `technique` 1 rolls for as long as the note is held), bell or mallet for colour, low strings or a sustained bass for the floor. No drum kit.
- Build by adding families and register, not drums: the first statement on one woodwind over held strings, the second with the theme doubled in octaves and the brass entering, the climax with everything at its widest spread. Keep it human: a little humanize, swing at 50.
- Every section is complete as it is: no inserts on the instruments, whose space comes from the mix's hall.
- Look up Part roles and registers and Build-up and break.

## Mix

- A large hall reverb on everything, sections placed left to right as they would sit, no bus limiting that flattens the build. Target -18 to -21 LUFS, -1 dBTP.
- Effects: one `auris.fx.reverb` hall (`room_size` 0.8 to 0.9, `pre_delay_ms` 20 to 40) on a return that every section sends to; no distortion, chorus or sidechain.
- Look up Choosing a space and Insert order.
