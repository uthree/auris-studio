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
| Guitar | Plucked strings moving in two directions at once, on a box with an air resonance and a top plate; the open strings nobody is playing answer through the bridge | Pick hardness, pluck position, damping, body, pickup blend (box to saddle), technique |
| Electric Guitar | Steel strings under a magnet, before the amplifier | Pick and pickup position, tone, pickup Q, decay per octave, technique |
| Bass | Plucked bass strings under a pickup, or on an upright's body; slapped, a thumb thrown at the string and the frets the string comes down on | Hardness, position, body (zero is electric, one is upright), tone, technique |
| Strings | Each player a string under a bow whose rosin grips and lets go; a section of one to four players; violin, viola, cello and bass bodies | Bow pressure, speed, position and response, players, instrument, vibrato, legato, technique |
| Mallet | A bar over a resonator tube: marimba, vibraphone, xylophone, glockenspiel, or a tine held at one end | Bar, mallet hardness, strike position, tubes, tremolo |
| Bell | Paired modes that beat as they ring: a cast bell, a tubular bell, a handbell, a singing bowl | Bell, hardness, where round the rim it is struck, warble |
| Reeds | A reed valve on a cylinder of air closed at the reed: a clarinet | Breath pressure, reed stiffness, breath noise, attack, vibrato, legato, technique |
| Whistles | An air jet across the window of an open pipe | Breath pressure, jet shape, breath noise, attack, vibrato, legato, technique |

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
- **A guitar's open strings answer.** Play A3 and let it go: the open fifth string, whose
  second harmonic it is, goes on ringing about thirty-five decibels down. B♭3, which falls
  between every open string's partials, leaves nothing. A string being fretted is the note's
  own and does not ring open; turning **Body** down, or the pickup blend over to the saddle,
  takes the bridge's coupling away with it.
- **A slapped bass meets its frets.** With **Technique** on Slap, the notes below E2 are
  struck with the side of the thumb and the ones from E2 up are popped: hooked by a finger,
  pulled away from the board and let go. Either way the string is driven at the fingerboard
  and comes down on the frets, and that is the sound — a clank of metal for a few hundredths
  of a second, and a rattle for as long as the string swings further than the frets allow.
  The frets are as far under the string as they are however hard it is slapped, so a harder
  slap is mostly a harder clank: the note it leaves grows less than the blow does. **Excitation Position** is
  where the hand is, and a slapping hand is over the last fret, at 32%: there the thumb has
  the board under it, and nearer the bridge it comes down past the end of the neck. **Contact
  Hardness** runs from the pad of the thumb to its bone. Thumb and Pop play every note one
  way. The **Slap bass** sound sets all of this.
- **Technique chooses how the other instruments are played, too.** The strings are bowed
  (Arco), plucked (Pizzicato) or bowed in a tremolo. A plucked string is held aside by a
  finger over the end of the fingerboard and let go: it rings and dies away as a bowed
  string does once the bow has left it, a low string for seconds and the top one for a
  fraction of one, and the note is over when it has died away whether or not its key is
  still down. In a tremolo each player's bow goes back and forth about eleven times a
  second, in their own time, and each tunes by ear from the whole periods between one turn
  of the bow and the next; the note stays within a few cents of the bowed one. The guitars and
  the bass play **Harmonics**: the note is plucked on a string an octave under it with a
  finger lying on the string's middle, which stops every partial that moves there within a
  few hundredths of a second. What rings on is purer than the fretted note and lasts
  longer. The reeds and whistles **Flutter-tongue**: the tongue is rolled against the
  breath two dozen times a second. A low note, whose pipe is slow to answer, flutters less
  deeply than a high one.
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
  nearly nothing and is there in about a tenth of a second, as a recorded clarinet's is: a
  soft one is started with a little more breath than it is held with, or it would take most
  of a second to speak. The breath in the tone is rough the way a recorded clarinet's is,
  about 33 dB under the tone at every dynamic and falling away above a kilohertz.
- **A bell's warble depends on where you strike it.** Each partial is two shapes a
  fraction of a hertz apart, and the place round the rim shares the blow between them.

## Controls and performance

Velocity changes what excites the instrument, not only how loud the result is: a harder
hammer blow is shorter and so brighter, a faster bow presses harder, a harder breath opens
the reed further.

Changes to **Decay**, **Damping**, **Release** and **Unison Detune** reach notes that are
already sounding. Hardness, position, stiffness and the **Technique** of the strings, the
guitars and the bass belong to the blow, the pluck or the stroke, and wait for the next one.
On the reeds and whistles the tongue starts and stops rolling at once. On the strings, **Bow Position** moves the bow at once.

MIDI CC7 is channel volume. CC11 is expression: level, and also bow speed and breath. CC1
deepens the vibrato of the bowed and blown instruments. CC64 lifts the piano's dampers and the
vibraphone's; pressing it after a key has come up catches a note that is already fading. It is
read as a depth, not a switch: the first quarter of its travel is free play, and between a
quarter and three quarters the dampers rise off the strings, so a half pedal shortens a released
note without stopping it. CC66 is the piano's sostenuto, holding up only the dampers that were up
as it went down. CC67 is its soft pedal: a note struck with it down meets two strings of three,
with softer felt, and comes out quieter and darker. CC68 is the legato footswitch: on the
strings, reeds and whistles it slurs as the **Legato** control does, for as long as it is down.
CC75, the General MIDI 2 decay-time controller, rests at its middle; below it the guitars and
the bass hear a palm laid on the strings at the bridge. It shortens notes that are held, the
upper partials first, so at the bottom a note is its pick's click and a short, dark thump. It
reaches notes already sounding, and the open strings too. Above the middle it does nothing.
Letting a key go is the fretting hand easing off: the note stops within **Release**.
CC74, brightness, also rests at its middle; on the strings it is where the bow is drawn. Above
the middle the bow is asked nearer the bridge, as near as a third of **Bow Position** (sul
ponticello). It goes as near as the string still holds its note, which is about where a bow is
usually drawn, and leans harder for the rest of the way: the note is brighter, as loud as it
was and on its pitch. A **Bow Position** set nearer the bridge than that does the same. Below
the middle the bow goes out over the end of the fingerboard, up to twice as far, with a lighter
stroke (sul tasto, veiled). CC65 turns portamento on for the strings, reeds
and whistles: a slurred note slides to the next instead of moving the finger at once, taking the
time CC5 gives, from a fiftieth of a second to a second and a half. Pitch bend is not slowed.
On everything struck or plucked (the pianos, guitars, bass, mallets and bells), CC74 is how hard
the next hammer, pick, mallet or beater is: harder above its middle and softer below it, by up
to half of the instrument's own hardness control. A soft mallet high on a xylophone is already
off the bar as fast as the bar throws it back, so there it can only go harder. CC2, the breath
controller, blows the reeds and whistles beneath the expression pedal. It rests all the way up;
a wind controller sends it, and in the bottom tenth of its travel the pipe stops speaking. On the
vibraphone the modulation wheel turns the motor, wherever it is further up than **Tremolo**.
CC64 is also the damper pedal of the tubular chimes and of the hammered strings, like a
cimbalom's.
A technique is not a controller. MIDI has no message for a slap, a plucked violin or a
harmonic, and General MIDI gives each of them a program of its own, so here each is the
instrument's **Technique** control, automated like any other where a part changes hands.
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
| 24–25 | Guitar: nylon, steel |
| 26–28, 31 | Electric Guitar; 28 is muted, 31 is harmonics |
| 32–37 | Bass: 32 is the upright, 36–37 are slapped |
| 40–45, 48–49 | Strings: solo violin, viola, cello and contrabass, and sections of four; 44 is the tremolo, 45 pizzicato |
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
| Piano, mel distance | Steinway B, C3–G4, 12 held-out notes | 0.171 | 0.152 |
| Piano, partial decay and envelope | Steinway B, C1–C8 | 4.9 dB, 1.4 dB | 5.4 dB, 2.0 dB |
| Piano, partial balance, register level, knock | the same | 4.5 dB, 1.8 dB, 5.2 dB | 4.2 dB, 1.7 dB, 3.4 dB |
| Guitar, mel distance | classical guitar, 44 held-out notes | 0.160, and 0.154 as Nylon guitar | 0.144 |
| Strings, mel distance | solo violin, 39 held-out notes | 0.111 | 0.080 |
| Reeds, mel distance | B♭ clarinet, held-out pitches | 0.041 | 0.054 |
| Whistles, mel distance of the settled tone | tin whistle scale | 0.017 | 0.018 |
| Hammered Strings, mel distance | hammered dulcimer | 0.234 | 0.241 |

The last column is the instruments these replaced, which were fitted to the recordings in the
second column. It is the mark a fitted model reaches, and the piano, the guitar and the
strings are still short of it. The mel distance is a spectrogram
distance over a note's first second, defined in `tools/eval/physical_mel.py`; it says how
close a note is to one recording, not how it sounds. The recordings, what each measure
compares and the commands behind every row are in
[physical instruments against real recordings](physical-copy-synthesis.md).

From C3 to G4 the piano's pianissimo lies 11.3 dB under its mezzo-forte and its fortissimo
10.3 dB over, at velocities of 0.35, 0.65 and 0.95; the recorded Steinway's lie 13.3 dB under
and 9.6 dB over.

Key to key, a piano's notes are within 0.8 dB rms of the keys around them, and a guitar's and
the strings' within 1.5 dB.

The piano's bass was voiced again on 2026-11-24, against the same Steinway's G1, C2 and G2 as
well as its Cs. The blow of a low note had 6 to 20 dB less than the recording's between one
and eight kilohertz, and has within 11 dB of it now: the two lowest octaves' hammers are
harder, and a hammer that strikes one string or two leaves room for 144 partials of each
where there were 96, which had ended the lowest C at three kilohertz. The two planes of a
wound string, and the strings of a low pair, lie a few hundredths of a hertz further apart
than a treble unison's. Of the measures in the table the envelope moved from 1.5 to 1.4 dB
and each partial's decay from 4.8 to 4.9 dB; the balance, the level and the knock are where
they were, and no key above C3 changed.

The middle was voiced again on 2026-11-26, against fifty-five of the same Steinway's notes
from C3 to E6 at three dynamics rather than its Cs alone. Its C4 and C5 turn out to be the two
dullest keys of that stretch, and hammers needled to them had left every note around them
dull: the partials above two kilohertz stood 13 dB short of the recording on average, and from
eight to twelve kilohertz the model had almost nothing, because the strings' internal
friction, fitted below four kilohertz as the square of frequency, was carried on up and had
that band dying at 55 to 90 dB a second where the recording's dies at 10 to 30. The hammers
from C3 to C6 are harder now, the friction levels off above five kilohertz, and the registers
there were brought down by what the harder hammers added at mezzo-forte. The partials above
two kilohertz stand within 4 dB of the recording on average, and the second to eighth partials
of a pianissimo note are 10.5 dB from it where they were 14.2. What it cost is in the table:
the balance read at the Cs alone went from 4.0 to 4.3 dB, and the mel distance from 0.162 to
0.170, all of it in the pianissimo notes. Decay, envelope and key-to-key evenness did not
move.

The same day a key's velocity was given the recorded grand's range of touch. A hammer's speed
had followed a power of velocity, fitted to the pianissimo, and it left the fortissimo 7 dB
over the mezzo-forte where the Steinway's stands 11 dB over, with the bands from two to eight
kilohertz rising 6 dB less than the recording's as the blow hardened. The speed now rises by
the same number of decibels for each step of velocity, 32 across the whole range, and over
twenty-two notes from C2 to C7 a fortissimo stands 11.2 dB over a mezzo-forte, the recording's
11.3, and a pianissimo 13.6 dB under it, the recording's 12.7.

The lowest notes, those of one wound string, were given their two decays on 2026-11-26. The
recorded Steinway's A0 to A1 fall 4 to 10 dB in their first second and then 2 or 3 dB a
second for as long as they are measured; the piano's fell 4 or 5 dB a second from the start
and lost 9 to 12 dB between the first second and the third, where the recording loses 3 to 7.
A wound string moves in two planes, and they had shared one direction of the bridge, so the
second only pulled against the first. They meet the bridge in two directions now: the first is
drained four and a half times as fast as a unison's shared motion, the second at a twentieth of
that, and each is heard by itself. Over those notes each partial's decay reads 6.9 dB from the
recording where it read 7.4, and the envelope 1.6 dB where it read 2.3. From C2 up the strings
come in pairs and nothing changed.

A clarinet's breath, read between the harmonics of the same recordings' settled notes, lies
31 to 35 dB under the tone from pianissimo to fortissimo, and 33 to 35 dB here; it had been 21
to 31 dB, an even hiss that stood over the harmonics above two kilohertz. A pianissimo note
takes 134 ms to rise from a tenth of its level to nine tenths in the recordings and 131 ms
here, where it had taken 277 ms. The reeds' row above was read again on 2026-11-24.

Every instrument was played across its range on 2026-11-24, at three dynamics and at the ends
of its controls, and read for its pitch, for what lies between its harmonics and for how
evenly it speaks. Four things were out and were put right.

With the bow asked three times nearer the bridge than it is usually drawn, which is the top of
controller 74, 172 of 185 bowed notes were hoarse and 148 were more than five cents off their
pitch, by as much as 88. The least force that holds a bowed string's motion grows as the
square of how near the bridge the bow is, and the most this string takes was found not to
grow at all, so there no force does both; and a player who pressed until the string stopped
whistling crushed the note. The bow now stops where the string holds and leans harder for the
rest. Four of the 185 are hoarse and 13 are off their pitch, nearly all of those from A♯6 up,
where a leaning bow still pulls the note flat. With the bow where it is usually drawn nothing
changed.

A clarinet blown at nine tenths of its breath pressure sounded up to 8 cents sharp, and a
whistle's top notes 4 cents flat; both are within a cent and a half now at every pressure. A
soft note under a flutter-tongue had kept little but its breath, 13 to 28 dB under the same
note played plainly, and lies about 6 dB under it. A guitar's harmonics were evened against
the note of the string they are played on and not the note heard: they are within 1.8 dB rms
of the harmonics beside them where they were within 2.9.

One thing was found and left: the lowest notes of a bowed bass, E1 and F1, are rougher than a
recorded one's a third above them.

A recorded classical guitar's first string was found then to have about 17 dB less between two
and four kilohertz than at its fundamental in a note's first half second, where these had
35 dB less, and letting the plain strings go more quickly made the mel distance of the table
worse. The guitar was read again the same day, partial by partial against that recording: the
level of each of a note's first sixteen partials over its first tenth of a second, and how
fast each falls. Three things were out, and were put right together.

- **The pluck.** Every string had been let go alike. The recorded sixth string agrees with
  that; the first, whose wave runs four times as fast, had 15 dB more above its eighth partial
  played softly, 22 dB at a middling pluck and 34 dB played hard. A string is now let go
  sooner the faster it is, and sooner still the harder it is plucked.
- **The strings' losses.** The recorded partials fall faster the higher they are, the first
  string's sixteenth at about 200 dB a second; these fell at under 50. The guitar's
  **Damping** rests at a half now, where its strings lose what that guitar's lose, and its
  **Decay** at 12.5 s so that the fundamental lasts as it did.
- **The box.** Against its level from 150 to 350 Hz the recorded guitar is about 5 dB quieter
  from 400 Hz to 1.5 kHz than this box was, which is what had made a high note's second
  partial too strong; and it has no trough between its air resonance and its top plate's.
  The bridge radiates by itself only up to 500 Hz, the plates' modes above 350 Hz are lower,
  and the air resonance is broader and weaker.

Over every string and dynamic the partials' levels are 3.6 dB from the recording's where they
were 6.3 dB, and their falls 10 dB a second from it where they were 21. The mel distance of
the table went from 0.167 to 0.160, and from 0.157 to 0.154 as Nylon guitar. The first string
has 8 dB less than the recording between two and four kilohertz in a note's first half second
where it had 18 dB less, and the open sixth string's fundamental is still 12 dB stronger than
the recording's. The electric guitar and the bass were set against recordings of their own and
did not change. A guitar saved with its controls written out keeps the damping and decay it
had, and with them a brighter top that rings as long as it did. The Body effect's guitar is
this box and changed with it.

The bass was read the same day against the three recorded electric basses its slap was set
on: sixty finger-style notes, each left to ring for two or three seconds. Their level falls
at 3.7 dB a second and their fundamental at 3.5; this bass fell at 12.6 and 12.0, three times
as fast. A recorded fundamental loses about 2 dB a second open or at the third fret on every
string, the low ones as the high, and nearly twice that an octave up the same string; here a
note decayed faster the higher it was, whatever string it was on.

The bass's **Decay** rests at 35 s now, where its open strings ring as the recorded ones do.
A stopped string decays faster by how much shorter it sounds, and pitch by itself no longer
shortens a note. A wound string's two losses are set apart, more to its friction and less to
its bending: the E string's partials between one and two kilohertz had died at 74 dB a second
where the recorded ones die at 30. Note for note the level's fall is 1.1 dB a second from the
recordings' where it was 8.8, and the partials' falls 2.5 to 5 from theirs where they were 6
to 8. A full pluck had also pulled a note 12 cents sharp, which a string that rings this long
would carry for seconds; the recorded notes are 0.7 cents sharper over their first half second
than two seconds on, and a full pluck is 2 cents sharp now. The Upright bass keeps the 7 s it
had. What slapping does to a note moved by half a decibel or less, and the table below is
read on this bass.

Whether a bass's or an electric guitar's high strings are let go sooner than its low ones, as
the guitar's are, was looked for on both. A magnet hears a comb of each string's partials,
with its teeth at different frequencies on each string and on each recorded instrument, which
moves single bands by 20 dB, so the strings could not be compared band by band as the guitar's
were. On the bass, with the guitar's law and without it, the model lies 7.8 dB rms from the
recordings between strings, and the bass keeps one corner.

The electric guitar was settled by fitting. All of its controls were fitted to 48 clean notes
of a recorded Stratocaster, on both pickups, once with one corner for every string and once
with the law's power free, and each fit was read on 46 notes at other pitches. Without the law
the fit ends at the hardest pick there is; with it, at a soft pick and a power of 1.3, and the
error on the held-out notes is 5% lower, 0.127 for 0.134, with the interval of the difference
clear of zero. Both fits put the pickups at 24% and 7% of the string, where a Stratocaster has
them and where the combs alone put them. The electric guitar has the law now, at a power of
1.4, and a softer pick: **Contact Hardness** rests at 0.3 where it stood at 0.6. With its
other controls as they were the held-out error is 0.145 where it was 0.150, and 33 of the 46
notes are closer. Those notes were played at one dynamic, so a harder stroke adds nothing to
the power here as it does on the guitar.

Two things are left open on it. Read from the combs alone, the same recordings can also be
told as a pick 3.5% of the string from the bridge and no law, 4.3 dB from the recorded
partials where a pick further up with a law is 5.1 dB; the instrument puts a pick no nearer
than 5%, and with it there the low strings are much too bright. And the same fits ask for
more than the pluck: with **Damping** near 0.3, where it rests at 0.03, the held-out error is
0.127, which is not taken up here.

The slapped bass is read beside three recorded electric basses, each note played finger-style,
with the thumb and popped. What is compared is what slapping does to a note: each measure less
the same measure of the same note plucked. These are means over notes that had no part in
setting the model, read on 2026-11-24.

| Slapped less plucked | Recorded, thumb | Here | Recorded, pop | Here |
| --- | --- | --- | --- | --- |
| Level | −3.3 dB | −2.2 dB | −2.1 dB | −3.9 dB |
| The blow over the note it leaves | 6.0 dB | 3.1 dB | 2.1 dB | 4.1 dB |
| The peak over the note | 6.0 dB | 5.1 dB | 2.6 dB | 3.8 dB |
| The fundamental's share | −4.7 dB | −3.8 dB | −0.6 dB | −6.2 dB |
| Fall from 0.1 s to 0.5 s | 0.5 dB | 1.3 dB | 0.7 dB | 1.6 dB |

A thumbed note's blow is the measure furthest out: it stands 3 dB less over the note it leaves
than a recorded one's.

A plucked violin's notes on its D and E strings fall by 17 to 310 dB a second in the Iowa
recordings, read from 5 dB under their loudest to 25 dB under, and by 66 to 450 dB a second
here; the median of one over the other is 1.25. The recorded open string rings four times as
long as a stopped one beside it, which the model does not tell apart. The tremolo, the
harmonics and the flutter-tongue are held to what each of them is by the tests, not to a
recording.

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
uv run tools/eval/bass_slap.py fetch target/slap-reference
uv run tools/eval/bass_slap.py measure target/slap-reference \
  target/release/examples/physical_fit_render target/slap-reference/measure.json
```

The tests assert on numbers: that every model is in tune across its range at three sample
rates, that a render does not depend on the host's block size, that nothing on the audio thread
allocates; that a piano is stretched the way a tuner stretches one and falls fast and then
slowly; that a bowed string moves as Helmholtz said at every pressure; that a clarinet has no
even harmonics and speaks at the softest breath; that a slapped string never gains energy from
its frets, and a thumb and a string never hold more between them than the thumb came with;
that a plucked string ends with its key down, a tremolo's bow turns a dozen times a second, a
harmonic has lost the octave under it within a tenth of a second, and a fluttered note stays
on its pitch. `physical_probe` plays single notes and prints
each one's level, pitch and decay; `physical_bench` prints the share of a processor each model
takes. `physical_fit_render` is the worker the measuring scripts under `tools/eval` drive:
freeze a copy of it before changing anything that sounds, and compare the two against the
recordings afterwards. The recordings are downloaded into `target/` and never committed.

A search over a voicing turns the dials in `auris_synth::physical::calibration` through the
worker's `dials` field, without rebuilding for each candidate. That needs
`--features physical-calibration`; an application build has no dials.
