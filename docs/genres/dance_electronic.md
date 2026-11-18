# Genre: Dance and electronic

Form in builds and drops rather than verses and choruses, four-on-the-floor or broken beats, the kick and bass as the centre, sidechain pumping, and a mix that is loud on purpose.

## Score

- Tempo 120 to 130 BPM for house, 140 to 150 for future bass and trance-like drops, 170 to 175 for drum and bass, in 4/4. Phrases come in eight and sixteen bars.
- Form: intro, build, drop, break, build, drop, outro. Name the sections Build and Drop; the drop is the arrival, carried by energy and the hook's rhythm rather than by a higher register.
- Harmony is a two- or four-chord loop (vi IV I V, i VI III VII, i iv), often minor, repeated through the drop. The hook is short and rhythmic: a two-bar synth figure or chord stab pattern, repeated with small changes.

## Arrangement

- Kick on every beat in the drop (four on the floor), clap or snare on two and four, offbeat open hats; the bass (`synth_bass_tight`) on the offbeats or in a rolling sixteenth pattern that leaves the kick alone.
- Chords as sidechained stabs or a pad (`synth_pad`), an arpeggio (`synth_arp_tight`) for motion, the hook on a bright lead (`synth_lead`). Keep the grid exact: swing 50, no humanize on the drums.
- Builds: a snare roll that doubles its rate, rising notes, a filter opening, the kick dropping out for the last bar. Breaks: the drums and bass out, pads and the hook's melody alone.
- A bass meant to growl takes `auris.fx.distortion` mode 0 (`drive_db` 6 to 18, `mix` 0.3 to 0.6) so its fundamental stays; a guitar, if any, goes through `auris.fx.guitar_amp`.
- Look up Sounds that need an insert.

## Mix

- Kick and bass own the low end; duck the bass and pads from the kick with a sidechain. Bright, wide, loud: target -9 to -11 LUFS, -1 dBTP.
- Effects: the sidechain is `auris.fx.compressor` on the bass and pads with `set_sidechain` from the kick (`ratio` 4 or more, `release_ms` 100 to 200 so it breathes in time); a synced `auris.fx.delay` (`sync` 6 or 7) and a long `auris.fx.reverb` on returns for the lead and pads, high-passed with `auris.fx.eq`; the kick and bass dry.
- Look up Choosing a space and Insert order.
