# Genre: R&B and neo-soul

Swung sixteenths played behind the beat, extended and altered chords, smooth voice-leading in electric piano and pads, a melody that bends around the beat rather than sitting on it.

## Score

- Tempo 65 to 95 BPM in 4/4.
- Harmony in ninths and elevenths: ii9 V13 Imaj9, IVmaj9 iii7 vi9, chromatic planing (the same chord shape moved by a semitone), a minor iv9 for colour. Two- or four-bar progressions that loop are normal; the movement is in the voicings, not in the form.
- The melody is conversational: short phrases that start after the beat, slides between neighbouring notes, held notes on the ninth or the eleventh. Leave it room; less is more than in pop.

## Arrangement

- Every part swings alike on the sixteenth grid: swing_percent 56 to 62, swing_grid sixteenth, set on drums, bass, keys and lead.
- Electric piano (`electric_piano_soul`) or a soft pad (`synth_pad`) with close voicings that move by step; a round bass (`finger_bass`) with ghost notes and slides, often playing between the kick hits.
- Drums: a lazy kick, a soft snare on three or on two and four with ghost notes, hats with velocity accents; humanize a little so the drums sit behind the beat.
- The electric piano may take `auris.fx.tube` (`drive_db` 6 to 12) for warmth and a slow `auris.fx.chorus`; a guitar is clean, through `auris.fx.guitar_amp` at a low drive.
- Look up Sounds that need an insert.

## Mix

- Warm and soft: low mids full, highs gentle, a lush reverb on the keys and the lead. Target -14 LUFS, -1 dBTP; no hard limiting.
- Effects: `auris.fx.plate` or a long `auris.fx.reverb` (`room_size` 0.6 to 0.8) on a return for the keys and lead; a slow `auris.fx.compressor` (`attack_ms` 20 to 30, `ratio` 2 to 3) on the bass and the drum bus; tame the highs with an `auris.fx.eq` high shelf rather than boosting brightness.
- Look up Choosing a space and Insert order.
