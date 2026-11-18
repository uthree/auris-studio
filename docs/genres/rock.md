# Genre: Rock and band

Guitars, bass and drums as a band: eight-beat drums, power chords, a riff that carries the identity as much as the vocal does, and a two-beat drive in the chorus at fast tempos.

## Score

- Tempo 110 to 180 BPM in 4/4; power ballads 66 to 80. Form: intro riff, verse, pre-chorus, chorus twice, a solo or bridge, a last chorus, the riff again as the outro.
- Harmony is mostly diatonic triads and power chords (`5`): I bVII IV, i bVI bVII, vi IV I V, with borrowed bVI and bVII in major keys. Chords change on the bar or the half bar; a chorus moves faster than the verse.
- Write the riff first: a one- or two-bar figure on the low strings, mostly roots and fifths with a passing note, repeated under the verse and returning between sections. The melody sits on chord tones with eighth-note drive; the chorus lifts and holds long notes over the band's eighths.
- Look up Motif repetition and development, Chorus lift and Section roles.

## Arrangement

- Two guitars on `auris.physical.electric_guitar`: one rhythm (`rock_rhythm_guitar`, power chords and palm-muted eighths), one for the riff, fills and the solo (`lead_guitar`). Bass (`finger_bass`) locks to the kick in root eighths. Drums (`pop_drums`, groove `basic-rock` or `eight-beat`): kick on one and three, snare on two and four, eighth hats in the verse, crash and open hats in the chorus.
- Build by register and density: verse guitars muted and low, chorus open chords and the second guitar an octave up. A fill on the last beat into every new section. Straight feel: swing stays at 50.
- Each guitar gets `auris.fx.guitar_amp` when it is chosen: rhythm `drive_db` about 18 to 24 with `cabinet` 2 (closed), the lead 28 to 34, a clean verse part 0 to 6. A dry pickup is not a rock guitar. The bass stays dry or takes a little `auris.fx.tube`.
- Look up Sounds that need an insert, Kick and bass together and Fills and crashes at transitions.

## Mix

- Guitars wide (one each side), bass and kick centred, the lead or riff on top. Bright and dense: target -10 to -12 LUFS, -1 dBTP.
- Effects: the guitar amps stay. High-pass the guitars (`auris.fx.eq` `hp_freq` 80 to 120 Hz) so the bass owns the low end; route the drums to a bus with `auris.fx.compressor` (`ratio` 3 to 4, `attack_ms` 10 to 30) for glue; one `auris.fx.reverb` room (`room_size` 0.3 to 0.5) on a return for drums and guitars, and `auris.fx.plate` or a quarter-note `auris.fx.delay` (`sync` 5) for the lead or the voice.
- Look up Choosing a space and Insert order.
