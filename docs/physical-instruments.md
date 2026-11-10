# Physical instruments

[日本語](physical-instruments.ja.md)

Choose **Physical Piano, Hammered Strings, Guitar, Electric Guitar, Bass, Strings, Mallet,
Bell, Reeds or Whistles** in the sound library. Each is a built-in plugin with its own stable
ID, such as `auris.physical.guitar`. New melodic tracks use Physical Piano. There are no sample
files to install. **[Physical Choir](physical-choir.md)** is a separate instrument with its
own page.

Every one of them is a model of the thing itself, not a recording of it: what you turn is a
hammer's felt, a bow's pressure, a reed's stiffness, and the sound follows from that.

## How they are built

Two kinds of thing make sound in these instruments, and each is computed as what it is.

**What rings on its own** — a string's partials, a bar's or a bell's modes, the soundboard or
box they drive — is a bank of damped oscillators, one per partial, each with its own
frequency and decay. A piano note is up to ninety-six partials of each of its three strings.

**What only sounds while it is fed** — a string under a bow, the air in a pipe — is the wave
going round it, and the hammer, the bow's rosin, the reed and the air jet are solved against
that wave sample by sample.

| Instrument | What is modelled | What to turn |
| --- | --- | --- |
| Piano | A felt hammer that stiffens as it is driven in, thrown at one, two or three strings that share a bridge; dampers; every other string answering; a soundboard; the knock of the case | Contact hardness, excitation position, unison detune, sympathetic resonance, string stiffness, release (the dampers) |
| Hammered Strings | A light hammer on courses of thin wire over a shallow box | Hammer hardness, strike position, course detune, bridge damping |
| Guitar | Plucked strings moving in two directions at once, on a box with an air resonance and a top plate | Pick hardness, pluck position, damping, body, pickup blend (box to saddle) |
| Electric Guitar | Steel strings under a magnet, before the amplifier | Pick and pickup position, tone, pickup Q, decay per octave |
| Bass | Plucked bass strings under a pickup, or on an upright's body | Hardness, position, body (zero is electric, one is upright), tone |
| Strings | Each player a string under a bow whose rosin grips and lets go; a section of one to four players; violin, viola, cello and bass bodies | Bow pressure, speed, position and response, players, instrument, vibrato, legato |
| Mallet | A bar over a resonator tube: marimba, vibraphone, xylophone, glockenspiel, or a tine held at one end | Bar, mallet hardness, strike position, tubes, tremolo |
| Bell | Paired modes that beat as they ring: a cast bell, a tubular bell, a handbell, a singing bowl | Bell, hardness, where round the rim it is struck, warble |
| Reeds | A reed valve on a cylinder of air closed at the reed: a clarinet | Breath pressure, reed stiffness, breath noise, attack, vibrato, legato |
| Whistles | An air jet across the window of an open pipe | Breath pressure, jet shape, breath noise, attack, vibrato, legato |

Every model also has **Level**, **Stereo Width**, **Live Quality** and **Render Quality**.

Some of what these do is worth knowing before you hear it.

- **A piano's registers are as loud as its strings make them, and its hammers are voiced.**
  A string can only give the bridge the energy it loses, so how far each register moves the
  soundboard comes from how fast its notes die away — measured on a Steinway B from the
  University of Iowa Musical Instrument Samples, one value for each C. The hammers' felt is
  voiced against the same recordings, as a technician needles a grand's: softer than the
  published hammers everywhere, and softest in the fifth octave. A note falls fast and then
  slowly, because the bridge takes what a unison's strings share and leaves what they do not.
- **Every key is as loud as the next.** A soundboard or a box is a crowd of resonances, and a
  crowd heard carelessly has holes in it: notes that fall between two resonances and all but
  vanish. Here every resonance is heard in the same sense, over what the bridge radiates by
  itself, and nothing cancels. What is left is the instrument's own — a guitar's air and top
  plate, a violin's body — and the notes those favour are plucked or bowed a little more
  gently, as a player learns to.
- **A blow knocks the case.** Under every piano note is a third of a second of pitchless knock
  from the keybed and the case, the same whichever key it is: nothing beside a bass note, and
  a good part of what is heard of the top octave.
- **A key pressed very gently makes no sound and lifts its damper.** Take a low C at velocity
  1, play the C above and let it go: the low strings ring on at the higher note's pitch. The
  sustain pedal does that for every string at once.
- **Unison Detune** at twelve cents is a honky-tonk; at its default it is a piano a tuner has
  just left, whose trebles beat no faster than its tenors.
- **A bowed string does not have to play a note.** Pressed too lightly it slips twice a
  period and whistles an octave up; pressed too hard it will not let go. The pressure control
  runs between those two, measured against the most the string takes at that bow speed and
  place. Each note is caught firmly and eased to the pressure you set, and a player whose
  string slips leans in until it stops — which is why a low **Bow Pressure** on a low note is
  less light than it says. A firm bow also pulls a note flat, and each player moves the finger
  until it is in tune again, over the first few tenths of a second.
- **Pressing harder is brighter, not louder.** Bow speed is loudness. Pressure sharpens the
  corner that travels round the string, and the top of the spectrum comes up by several
  decibels while the bottom stays where it was.
- **A section is several players**, each a string of their own, a few cents and a few
  milliseconds from the next and a seat apart. With **Instrument** on Section, a low note is
  played on a cello or a bass and sounds through that body.
- **A clarinet is quiet until it speaks.** Breath is measured from the pressure at which the
  note begins to sound at all, so the softest breath still plays every note, nearly as a
  sine; half-way to shutting the reed it is nearly a square wave. A tongued note starts from
  nearly nothing and is there in a few hundredths of a second, as a recorded clarinet's is.
- **A bell's warble depends on where you strike it.** Each partial is two shapes a
  fraction of a hertz apart, and the place round the rim shares the blow between them.

## Controls and performance

Velocity changes what excites the instrument, not only how loud the result is: a harder
hammer blow is shorter and so brighter, a faster bow presses harder, a harder breath opens
the reed further.

Changes to **Decay**, **Damping**, **Release** and **Unison Detune** reach notes that are
already sounding. Hardness, position and stiffness belong to the blow or the pluck, and wait
for the next one. On the strings, **Bow Position** moves the bow at once.

MIDI CC7 is channel volume. CC11 is expression: level, and also bow speed and breath. CC1
deepens the vibrato of the bowed and blown instruments. CC64 lifts the piano's dampers and the
vibraphone's; pressing it after a key has come up catches a note that is already fading.
Pitch bend reaches sounding notes, up to two octaves either way. On the piano **Release** is
the damper: the time it takes to remove 80 dB, longer in the bass, and the keys above F6 have
none.

With **Legato** on, the strings, reeds and whistles play one line: overlapping notes reuse the
sounding string or pipe without a new attack, and letting the newest key go returns to the one
still held.

**Live Quality** is what plays as you work and **Render Quality** what an export, a freeze or
a bounce uses. Draft keeps about a third of the partials and runs the bowed and blown
instruments at the project's sample rate; Standard and High run them at twice that. Render
defaults to High and live to Standard.

Rendering is the same whatever block size the audio device asks for, and the same every time.
The piano holds thirty-two notes at once and the others twelve to twenty-four; past that the
quietest released note is taken for the next.

## Composition

A song can name a native instrument directly:

```toml
performance = "pop-band"
form = ["verse"]

[[part]]
name = "guitar"
role = "chords"
instrument = "auris.physical.guitar"
```

General MIDI programs resolve to these instruments, with a patch that makes the model the
instrument named:

| Programs | Instrument |
| --- | --- |
| 0–3, 6–7 | Piano; 3 is the honky-tonk, 6–7 a hard short attack |
| 8–13, 108 | Mallet: celesta and glockenspiel on steel bars, music box and kalimba on tines, vibraphone, marimba, xylophone |
| 14, 112 | Bell: tubular bells, handbell |
| 15 | Hammered Strings |
| 24–25, 45 | Guitar: nylon, steel, and pizzicato strings as a short pluck |
| 26–28, 31 | Electric Guitar; 28 is muted |
| 32–35 | Bass: 32 is the upright |
| 40–44, 48–49 | Strings: solo violin, viola, cello and contrabass, and sections of four |
| 71 | Reeds |
| 74, 78 | Whistles: recorder, whistle |

These are starting parameters for Auris's own instruments, not General MIDI patch emulation.
A program none of them is a model of — a trumpet, a harp, a flute — keeps its SoundFont sound
or the session's reported fallback. An explicit SoundFont source always chooses that font.

## Looking inside

Open an instrument's parameter editor from its track's instrument row. Beside the parameters
are the bodies themselves — up to four sounding notes, each drawn as what it is: a string
between its ends with the place it was struck, plucked or bowed marked; a bar on its supports;
a bell's rim; a pipe's air from the mouthpiece to the open end. **Every body can be touched.**
Pull a guitar string aside and let go to pluck it there; press a piano string to strike it and
let go to lower its damper; hold on a violin string to bow it at that place for as long as
you hold.

Under the audition keyboard is **Inside the instrument**:

- **Body** is the soundboard or box the strings drive, as a surface: where each of its lowest
  modes is in its swing, laid out the way a plate's modes lie. Piano, hammered strings, guitar,
  upright bass and the bowed strings have one.
- **Decay** is the newest note's first twenty-four partials over the last second and a third,
  height in decibels. A piano's two decays, a honky-tonk's beating and a bell's warble are
  seen here rather than heard.
- **Partials** is the same spectrum now, for every note showing.
- The last plot is what excites the instrument: a hammer's or a mallet's force through the six
  milliseconds of its blow; a string's speed under the bow through one period, flat at the
  bow's own speed while the rosin grips and flying back when it lets go; a reed's opening
  through one period; where an air jet meets the edge.

Drag the surface to turn it. It is shaded on the graphics processor where there is one, and
drawn the same without.

The pictures are read from the state the instrument is already keeping, about thirty times a
second, and only while the editor is open. They are relative, not measured in metres, and a
string's shape is a snapshot, not a slow-motion film. **Freeze** holds a picture while the
sound goes on. Watching changes nothing that is heard.

## Measured against recordings

The instruments are checked against recordings of real ones with the tools under `tools/eval`,
which render through the code the application runs. These are what they read on 2026-11-11;
smaller is closer.

| Measure | Recorded | Reads | A model fitted to the same recordings |
| --- | --- | --- | --- |
| Piano, mel distance | Steinway B, C3–G4, 12 held-out notes | 0.162 | 0.152 |
| Piano, partial decay and envelope | Steinway B, C1–C8 | 4.8 dB, 1.5 dB | 5.4 dB, 2.0 dB |
| Piano, partial balance, register level, knock | the same | 4.0 dB, 1.8 dB, 5.1 dB | 4.2 dB, 1.7 dB, 3.4 dB |
| Guitar, mel distance | classical guitar, 44 held-out notes | 0.167, and 0.157 as Nylon guitar | 0.144 |
| Strings, mel distance | solo violin, 39 held-out notes | 0.111 | 0.080 |
| Reeds, mel distance | B♭ clarinet, held-out pitches | 0.043 | 0.054 |
| Whistles, mel distance of the settled tone | tin whistle scale | 0.017 | 0.018 |
| Hammered Strings, mel distance | hammered dulcimer | 0.234 | 0.241 |

The last column is the instruments these replaced, which were fitted to the recordings in the
second column. It is the mark a fitted model reaches, and the piano, the guitar and the
strings are still short of it. The mel distance is a spectrogram
distance over a note's first second, defined in `tools/eval/physical_mel.py`; it says how
close a note is to one recording, not how it sounds. The recordings, what each measure
compares and the commands behind every row are in
[physical instruments against real recordings](physical-copy-synthesis.md).

From C3 to G4 the piano's pianissimo lies 12.7 dB under its mezzo-forte and its fortissimo
7.0 dB over, at velocities of 0.35, 0.65 and 0.95; the recorded Steinway's lie 13.3 dB under
and 9.6 dB over.

Key to key, a piano's notes are within 0.8 dB rms of the keys around them, and a guitar's and
the strings' within 1.5 dB.

## Development verification

```sh
cargo test -p auris-synth -p auris-session
cargo test -p auris-gpui --bins physical
cargo run --release -p auris-synth --example physical_probe
cargo run --release -p auris-synth --example physical_bench

cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/fetch_physical_reference.py target/physical-reference
uv run tools/eval/prepare_physical_reference.py target/physical-reference \
  --lock tools/eval/references/iowa-notes.json
uv run tools/eval/evaluate_physical_mel.py target/physical-reference \
  <a worker frozen earlier> target/release/examples/physical_fit_render target/physical-eval
uv run tools/eval/piano_register.py target/piano-register prepare
uv run tools/eval/piano_register.py target/piano-register evaluate \
  <a worker frozen earlier> target/release/examples/physical_fit_render target/piano-register/evaluation.json
```

The tests assert on numbers: that every model is in tune across its range at three sample
rates, that a render does not depend on the host's block size, that nothing on the audio thread
allocates; that a piano is stretched the way a tuner stretches one and falls fast and then
slowly; that a bowed string moves as Helmholtz said at every pressure; that a clarinet has no
even harmonics and speaks at the softest breath. `physical_probe` plays single notes and prints
each one's level, pitch and decay; `physical_bench` prints the share of a processor each model
takes. `physical_fit_render` is the worker the measuring scripts under `tools/eval` drive:
freeze a copy of it before changing anything that sounds, and compare the two against the
recordings afterwards. The recordings are downloaded into `target/` and never committed.

A search over a voicing turns the dials in `auris_synth::physical::calibration` through the
worker's `dials` field, without rebuilding for each candidate. That needs
`--features physical-calibration`; an application build has no dials.
