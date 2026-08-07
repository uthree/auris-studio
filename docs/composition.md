# Automatic composition

A whole piece from a text specification, and clips that rewrite themselves when the chords
under them change. The rest of the application is in [Features](features.md).

## The song specification

**Compose → Compose a Song…** opens the song sheet, in three columns: the **song** — key, tempo,
meter, mood, groove, seed and the feel dials; the **form** — one row per playing of a section,
each with its length, how hard it is played, what progression it plays, how far it is moved from
the key and which parts play it; and the **parts** — one row each, with role, instrument, octave
and four dials.
**Write**, **Another Take** — the same dials and the next seed — and **Save as Specification…**.
The whole piece arrives as a single undo step, so a composition that is not what was wanted is one
press away from the document that was there before it.

**Style** is the first row and the one to start at. Around thirty dials is a lot to be asked for
before anything has made a sound, and the honest answer to what they should be is *depends what you
are writing* — so the shelf answers all of them at once:

| | |
| --- | --- |
| `chiptune` | The built-in voices, four to the floor |
| `pop-band` | Drums, bass, keys and a lead — the 王道進行 |
| `city-pop` | Electric piano and slap bass over 丸サ進行 |
| `rock` | Overdriven guitar, organ and a hard kit |
| `jazz-trio` | Piano, upright bass and brushes on a ii-V-I |
| `orchestral` | Strings, horns and timpani in 3/4 |
| `synthwave` | Saw lead, analogue bass and a TR-808 |
| `ambient` | Pads and a slow bell, no kit at all |

Choosing one replaces the whole sheet — tempo, key, groove, progression, form and roster — because
half a style is the arrangement of one at the tempo of another. Then change what you do not like,
which is a much better place to start than an empty form. From the command line the same list is
`auris presets`, and `auris compose --preset city-pop` writes one without a file.

Each is a `.asong` document embedded in the build rather than a structure assembled in code: a
preset is meant to be *read*, the format was designed to be the readable one, and it means the
presets are parser tests that fail loudly rather than silently.

A part's instrument cell offers the General MIDI sounds first, grouped into the sixteen families
the standard already divides them into — a hundred and twenty-eight names in one menu is a menu
nobody can read — and the built-in plugins under a rule below them. A drum part is offered the
eight kits instead, because on a drum part that number is a whole kit. Choosing a plugin clears the
program, so the row never says one thing while the piece plays another.

A row in the form is a *playing*, not a section: a name that appears twice is one section played
twice, and editing either row edits the one section, because that is what makes it recognisably
the same chorus. The section picker offers the song's own names first — choosing one of those is a
repeat — and under a rule a fresh one of each: once there is a verse, `verse 2` is one click, which
is how a song gets two verses that are not the same eight bars.

**A section can change how a part plays**, as a patch rather than a second declaration: what it
does not name it does not touch, so a busier chorus is one line. The lead an octave up in the last
chorus, the hat on sixteenths in the bridge, a rhythm written out for one section — `octave`,
`density`, `gate`, `subdivision`, `rhythm` and `note`, under `[section.chorus.part.lead]`.

What is deliberately *not* patchable: the name, the role, the instrument, the program, the level
and the pan. Those are not how a part plays, they are what its **track** is, and a part is one
track for the whole song — one row, one instrument, one fader. A chorus on strings where the verse
was on a piano is two parts and not one, and the roster button above is what brings each of them
in. The line is not a limitation waiting to be lifted: a track that changed instrument half way
through would have to be two tracks, and then it was two parts all along.

**A section that changes key is led into.** The last chord before the change becomes the dominant
seventh of the key being arrived at — the oldest device there is, and the reason a modulation can
sound like an arrival rather than an edit: a `V7` names its tonic before that tonic has sounded, so
the ear is already in the new key when the section starts. One chord, replaced in place, only in
the bars before a change somebody asked for by hand. It is the single thing in the format that
rewrites a bar of a progression quoted by name, and `lead_in = "none"` is how to say the plain jump
was meant.

**A section can play at a tempo of its own**, which is the difference between a chorus that lifts
and one that is only louder. Its button on the dial row reads `—` for a section that follows the
song and the tempo itself for one that does not, and the menu offers the song's tempo either side.
It is a **step**, in force from that section's first bar until something changes it back — a
ritardando slows *through* a passage, and neither the specification nor the document's tempo map,
which is piecewise-constant, can say that. Nothing pretends otherwise. A composed piece arrives
with the changes already on the timeline's tempo lane, where they can be dragged like any others.

**Who plays is chosen per section**, which is what stops a piece being the same six instruments
from the first bar to the last. The `7/7` button lists the roster with a tick against the parts
that come in; turning the pad off in the verse is one click. The last part left cannot be switched
off — a section that plays nothing is silence, and the specification has no way to write it, since
naming nobody is already how *everybody* is spelled. Turn the last one back on and it goes back to
saying everybody, so a part added later plays there too.

**A progression is chosen per section**, which is what lets one change partway through a song. The
picker lists the ones this song already carries, then **Write one…** and **Keep in the catalogue…**,
then the progressions this installation has been taught, then the whole built-in catalogue.
Choosing a catalogue entry the song does not carry is what adds it — there is no chart list to fill
in first.

**Write one…** takes bars of roman numerals (`| IVmaj7 | III7 | vi7 | I7 |`) and files them under
the section's own name, so a second section can reach the same chords from the same picker.
**Keep in the catalogue…** puts the one written into `~/.config/auris-studio/progressions.json`,
where every later song's picker finds it and `auris progressions` lists it. A kept progression is
*a snippet the picker offers*, not a name a file resolves: choosing one writes its chords into the
song, so an `.asong` never refers to anything outside itself and still plays for somebody who has
never seen your book. A name the built-in catalogue already uses is refused, because two `@axis`es
in one picker is a choice nobody can make.

A composed document **remembers the specification it was written from**, so the sheet reopens on
the song rather than on the defaults after a save and a reload, and **Another Take** on a piece
whose `.asong` nobody kept still gives another take of *that* song.

The sheet and the file are two faces of one `SongSpec`, so neither can drift from the other:
**Compose → Compose from Specification…**, or `auris compose song.asong`, writes a piece from the same
thing in a TOML document — key, scale, tempo, mood, chord progression, form and parts — that an
agent can write and a person can edit one line of.

Two are in [`examples/`](../examples): `hello.asong` is three lines, which is a whole song because
every field has a default, and `neon-drive.asong` is most of the vocabulary with a comment beside
each part of it.

```bash
auris compose examples/neon-drive.asong -o neon.auris
```

```toml
title  = "Neon Drive"
key    = "C minor"
tempo  = 128
mood   = "driving"
chords = "@marusa"
form   = ["intro", "verse", "chorus", "verse", "chorus", "outro"]

[section.chorus]
bars      = 8
intensity = 0.95
tempo     = 132

[[part]]
name       = "lead"
instrument = "auris.synth.chiptune"

[[part]]
name    = "strings"
role    = "pad"
program = "String Ensemble 1"
```

`instrument` names a plugin; **`program` names a General MIDI sound** out of whichever General
MIDI SoundFont is installed — by name, as above, or by number for anybody working from a font's
own listing. A part may carry both, and that is the point rather than a redundancy: the program
is played when there is a font to play it from, and the plugin is what the part falls back to when
there is not, so a specification asking for a string section on a build with no library comes out
as an oscillator rather than as silence.

On a **drum** part the same field means something else entirely, because in General MIDI it does:
percussion patches select a whole *kit* — `"Standard Kit"`, `"TR-808 Kit"`, `"Brush Kit"` — and it
is the note number that picks the drum. Which of the two readings a number gets is never guessed
at, because `role` has already said.

The syntax is TOML and the extension is `.asong`, the same way a project file is JSON inside
`.auris`. Serde reads *and writes* it, which is the point: a format that can only be read makes a
dialog that saves its settings a second implementation of the same grammar, free to disagree with
the first. Sections are a table keyed by name because their order means nothing — `form` decides
what plays when — and parts are an array because theirs is the order the tracks are created in.
Unknown fields are refused rather than ignored, so a misspelling is never a silently dropped
instruction: a syntax error says which line it is on, and every complaint about *meaning* is
reported at once.

Progressions can be quoted from a catalogue by name — `@marusa` (丸サ進行), `@royal-road`
(王道進行), `@koakuma`, `@komuro`, `@canon`, `@junjo`, `@blues`, `@andalusian` and the rest — or
written out in roman numerals (`| IVmaj7 | III7 | vi7 | I7 |`) in any key. A quoted progression is
never recoloured, because the whole point of naming one is that it comes out sounding like
itself.

That is also why a quotation follows its **chords** rather than its degrees when the key is in
the other mode. 丸サ進行 is written for a major key; asked for in C minor it is read against the
relative — Abmaj7, G7, Cm7, Eb7, the loop centred where the piece is — and not `IVmaj7 III7 vi7
I7` of an aeolian scale, which shares one chord with it and lands nowhere near. It travels the
other way just as well: `@epic`, written in minor degrees, is the loop on the relative minor when
a piece is in major. A progression you write out yourself declares no mode and is taken at face
value, because those are the degrees you meant.

Each part is built from one short figure invented per section and then restated bar after bar,
which is what gives a section something an ear can hold on to; the fourth bar of every phrase
answers it rather than repeating it again. A section played twice is the same section both times —
`variation = 0.4` buys back as much departure as you want, and `variation = 0` makes a second
chorus note for note the first. A section that another follows runs a fill into it, and every part leans
gently across a phrase rather than sitting at one level throughout.

The notes a part may reach for come from the chord under it rather than from the key alone. A
chord that borrows — a secondary dominant, a raised leading tone — does not add a note, it
*replaces* the degree that note came from, and a melody drawn from the plain scale would go on
playing the degree it replaced with both versions sounding at once. Over a G7 in C minor the
available notes are therefore `D Eb F G Ab B`: the harmonic minor, arrived at rather than named.

A composed piece arrives **mixed rather than merely written**: levels and a pan spread by role,
the kit under one drum fader, and a room the pitched parts send to — most for the pad, least for
the tune, none at all for the kick and the bass, because more room is further away. The kit sits
*above* the tune, where a kit has sat in most records made since about 1980. Every track is
coloured by what it plays rather than by the palette in order, so the kit is one family of reds,
the bass is indigo wherever it was declared, and an arrangement can be read without reading a
single name.

Everything is a pure function of the specification and its seed, so the same document always
writes the same piece and `--seed 7` writes a different one. Every decision draws from a stream
addressed by name rather than by call order, so changing the drum density does not silently
rewrite the melody. `auris progressions` lists the catalogue; `--set "field: value"` overrides any
field from the command line, with the value written the way you would say it rather than the way
TOML quotes it — `--set "key: D minor"`.

## Clips that write themselves

A single clip can be written from the chords underneath it without a specification for a whole
song: right-click an empty stretch of an instrument track and choose a preset — lead, chords, pad,
arpeggio, stab, bass, drums, or one drum voice at a time: kick, snare or hi-hat. A kit on one track
is three voices no fader can separate, so the pieces are there for when it needs to be a mix rather
than a part. The clip keeps the recipe that produced it, so **Another Take** is
the next seed, **Write It Again** follows the chords when they move, and **Keep This One** drops
the recipe when a take turns out to be the keeper. A track's own menu has **Keep Every Take Here**,
which does that to all of them at once and says how many it acted on. The dials are in the
inspector:

| | |
|---|---|
| **Subdivision** | How finely the beat divides: 1/8, 1/16, or either of them in triplets |
| **Octave** | Which register, ±2 from where the preset sits |
| **Density** | How busy the part is — for a comp, which figure it reaches for |
| **Syncopation** | How far the figure pulls off the beat, without making it busier |
| **Gate** | How long each note sounds, as a share of the gap to the next |
| **Intensity** | How hard it is played |
| **Dynamics** | How far apart the hardest and softest notes are struck |
| **Fill** | How much of the last bar the snare runs into what follows — drums only |
| **Swing** | How late the offbeats are |
| **Humanize** | How far timing and velocity wander |

Intensity and dynamics are the level and the spread, and they are separate because they are two
questions: a part can be played hard and flat, or softly with everything in it moving. At 0 the
dynamics leave every note struck alike — a sequencer, which is sometimes exactly the point — and
the level stays where the intensity put it rather than sagging with the spread. It reaches every
source of variation at once: the metric hierarchy, the accents, the lean across a phrase and the
crescendo of a drum fill.

A kit reads the density around the middle, and the middle **plays the groove**: a beat is never
thinned there, a weak sixteenth sometimes is, and a quiet section thins further. Below the middle
the groove thins from its weakest hits upward and never loses a downbeat; above it the steps the
groove left empty start taking ghost notes — which is how a drummer gets busier without playing
something else. *Which* rhythm it plays is
still the groove picker, and that is a choice from a drummer's own vocabulary rather than a
number: `basic-rock`, `eight-beat`, `sixteen-beat`, `four-on-the-floor`, `shuffle`, `breakbeat`,
`bossa-nova`, `half-time` and `sparse`.

Only the rows a preset can actually hear are drawn. A kit has no gate, subdivision, octave or
syncopation — a one-shot ignores its note-off, its pitches are drum numbers rather than notes, and
where it plays is which groove it plays. Nothing but a kit has a fill, because nothing else has a
last bar to announce. A pad has no syncopation, because it sounds each chord where the chord is. A
part on a triplet grid has no swing.

The subdivision is per part, not per song, so a stab hammering triplets over a straight kit is a
setting rather than a fight. Swing disappears on a triplet grid, because a grid already sitting
where swing is trying to push it has nothing left to be pushed — and a drum kit ignores the
subdivision entirely, since a groove is written in sixteenths and read by index.

A chord part picks its figure once for the section and restates it, the way a keyboard player picks
a feel and keeps it; only the fourth bar of a phrase is allowed to turn it over, and only
sometimes. At the top of the density dial the figure it reaches for is a rhythm rolled from the
metric hierarchy — most of the steps, with the holes that make it a rhythm rather than a tremolo.

**Chords** and **pad** read the same harmony through the same writer, and what separates them is
what a pad does at a chord change: it holds whatever the two chords have in common and moves only
the voices that have somewhere to go, where a comp restrikes every one of them. That is the
difference between a chord *changing* and a chord *drifting*, and it is the reason the two presets
are two presets rather than one with the rhythm turned off.

The **stab** preset is the settings that have to be turned up together: fast, short and hammered,
which is what most dance music has underneath it. It arrives with its own dials rather than the
middling defaults, and moving one of them keeps it moved when the preset changes — a dial somebody
set is theirs, a dial still where the last preset left it is not.

**A drum part says which note it strikes.** General MIDI is the only agreement there is about
which number is a kick, and a SoundFont is under no obligation to keep it — a kit that puts its
snare somewhere else came out silent or playing a cowbell, and nothing could say otherwise. The
song sheet shows the note where a pitched part shows its octave, which costs no room because a kit
has no octave: its pitches *are* drum numbers. The picker offers the General MIDI kit by name, and
`note = 12` in the specification reaches anything at all.

```toml
[[part]]
name = "kick"
note = 24     # this font puts its kick on C1
```

`auris` compose specifications reach the same settings, per part:

```toml
[[part]]
name        = "chords"
subdivision = "16t"
gate        = 0.25
```

