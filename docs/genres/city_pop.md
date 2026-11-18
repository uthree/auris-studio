# Genre: City pop and AOR

Sevenths and ninths as the plain harmony, ii–V motion and passing chords, a sixteenth-note groove with a busy, melodic bass (often slapped), electric piano and clean guitar cutting.

## Score

- Tempo 95 to 125 BPM in 4/4.
- Write every chord as a seventh or a ninth: IVmaj7, iii7, vi7, ii7 V7, Imaj7. Typical moves are secondary dominants as passing chords (I7 into IVmaj7, III7 into vi7), ii7–V7 into each new key area, a iv7 or bVIImaj7 borrowed before the chorus, and IVmaj7 III7 vi7 as a loop. Change chords every two beats in places; the harmony moves more than in J-pop.
- A key change between sections (up a whole tone or a minor third) is common.
- The melody is smooth and syncopated, landing on chord extensions (the ninth, the major seventh) and leaving space between phrases for the band's answers.

## Arrangement

- Electric piano voicings (`electric_piano_soul`) in rootless four-note shapes, a clean guitar cutting short chords on sixteenths (`rock_rhythm_guitar` with a short gate), a busy, melodic bass on sixteenths (`finger_bass`) with octave jumps and passing notes.
- Drums: tight sixteenth hats with accents, snare on two and four, a light kick pattern that leaves room for the bass. A straight sixteenth feel, or a slight swing (52 to 55 on the sixteenth grid) for every part alike.
- Brass stabs (`brass_section`) or a string line answer the melody in the gaps; a synth or saxophone-like lead can take the hook in the intro.
- The clean guitar is `auris.physical.electric_guitar` through `auris.fx.guitar_amp` at `drive_db` 0 to 6, then `auris.fx.chorus` (`mix` 0.3 to 0.5). The electric piano may take a light `auris.fx.chorus` too; the bass stays dry.
- Look up Sounds that need an insert.

## Mix

- Warm and clean, a moderate room reverb, the bass and kick clear but not dominating. Target -14 LUFS, -1 dBTP.
- Effects: one `auris.fx.reverb` room (`room_size` 0.4 to 0.6) or `auris.fx.plate` on a return for the keys, snare and lead; a dotted-eighth `auris.fx.delay` (`sync` 6) on the lead's long notes; a gentle `auris.fx.compressor` on the bass for an even line.
- Look up Choosing a space and Insert order.
