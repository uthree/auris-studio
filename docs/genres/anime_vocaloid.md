# Genre: Anime song and Vocaloid

Fast (often 170 to 200 BPM), dense and wide-ranged melodies, rhythmic hits (キメ) the whole band plays together, quick section changes and key changes. A song of this kind is busier than J-pop in every part.

## Score

- Tempo 160 to 200 BPM in 4/4, or a half-time feel over a fast pulse. Sections are short (often eight bars) and change quickly; an instrumental hook opens the song before the voice.
- The J-pop vocabulary (Aメロ, Bメロ, サビ, royal road progression) applies, played faster and with more chromatic motion: a passing diminished chord, a sudden modulation up a semitone or a third between sections, a bVI–bVII–I rise into the chorus.
- The melody packs sixteenths in the verse and opens into long, high notes in the chorus; leaps of a sixth or an octave are normal. Keep each line breathable: a sixteenth run needs a rest or a held note after it.
- Plan the キメ in the score: two to four hits the whole band plays together, usually in the bar before a chorus or at the end of a phrase.

## Arrangement

- Drums drive: a busy kick, sixteenth hats, a two-beat (kick and snare alternating on eighths) or four-on-the-floor chorus. Bass in eighths or sixteenths, locking with the kick. Distorted or bright guitars (`rock_rhythm_guitar`, `lead_guitar`), a synth lead or arpeggio (`synth_lead`, `synth_arp_tight`), strings for the chorus lift.
- Every part plays the キメ: write the hits on the same beats in drums, bass and chords, with rests around them.
- Fills into every section, a riser or a snare roll before the chorus, and a full stop for a beat before the last chorus.
- Distorted guitars are `auris.physical.electric_guitar` through `auris.fx.guitar_amp` (`drive_db` 24 to 34, `cabinet` 2). A growling synth bass takes `auris.fx.distortion` mode 0 with `mix` about 0.4, so its fundamental stays.
- Look up Sounds that need an insert.

## Mix

- Bright and dense, the lead on top; tight low end with the bass and kick sidechained. Target -12 to -14 LUFS, -1 dBTP.
- Effects: `auris.fx.compressor` on the bass keyed from the kick with `set_sidechain`; the synth lead or arpeggio through a synced `auris.fx.delay` (`sync` 6 or 7, `ping_pong` 1, `mix` 0.15 to 0.25); a bright `auris.fx.plate` return for the voice and the lead; the drums on a compressed bus.
- Look up Choosing a space and Insert order.
