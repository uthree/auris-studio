# Physical instruments

Choose **Physical Piano, Guitar, Bass, Bell, Mallet or Violin** in the sound library.
Each is a separate built-in plugin with its own stable ID, such as `auris.physical.guitar`.
New melodic tracks use Physical Piano. No sample files or network setup are needed.

These are compact, reduced physical models with a shared control vocabulary. They aim for
playable, adjustable instruments rather than reproductions of particular recorded instruments.

| Instrument | Model | Useful adjustments |
| --- | --- | --- |
| Piano | Finite hammer pulse and coupled unison stiff-string modes | Contact hardness, strike position, stiffness, pedal |
| Guitar | Finite-width pluck, allpass-tuned string and bridge-motion output | Pick hardness, pluck position, damping, pickup blend |
| Bass | Plucked string with lower body resonances | Finger/pick hardness, pluck position, resonance decay |
| Bell | Damped, inharmonic shell modes | Beater hardness, excitation position, decay |
| Mallet | Damped free-bar bending modes | Soft/hard beater, damping, short/long resonance |
| Violin | Travelling-wave string with static/sliding bow friction | Bow speed, pressure, response time, contact position, expression, legato |

Fitted filter cascades colour the piano, guitar and violin; guitar and violin add six causal
radiation resonances. Parallel body resonances colour the other models. The piano uses one,
two or three strings by register, up to 64 partials,
and a velocity-dependent finite hammer pulse with passive unison coupling. The
violin models one bowed string, with a bounded friction approximation rather than a full bow,
bridge and wooden-body simulation. The bell mode ratios describe a designed shell, not a specific
manufactured bell. These choices keep the instruments small and their controls predictable.

## Controls and performance

**Contact Hardness** and **Excitation Position** change the initial attack. A soft contact rejects
upper modes; moving the contact suppresses different modes rather than applying a generic EQ.
Velocity changes both excitation energy and attack hardness. **Resonance Decay** sets the nominal
time to lose 60 dB at the fundamental; higher modes decay sooner, and **Damping** adds losses.
The guitar's allpass tuning separates interpolation from physical loss. Its decay is calibrated
at the fundamental; damping shortens it by a factor of `1 + 3 × damping` and increases upper-mode
loss. Guitar and violin additionally apply smooth register-dependent contact/loss curves,
centered on G3 and A4 respectively, so the decay control is nominal at that center.
The bass and violin's linear interpolation also contributes frequency-dependent losses.
The [refinement account](physical-model-refinement.md) explains the radiation profiles and
their calibration limits.

**Body Resonance**, **Level**, **Release**, **Damping** and **Resonance Decay** affect sounding
notes. **String Stiffness** is a piano-only control that spreads upper partials while keeping the
fundamental tuned. **Bow Pressure** is violin-only and affects the sounding friction junction.
Position takes effect at the next attack on struck/plucked instruments; on violin it moves
the sounding bow junction over about 15 ms. Stiffness takes effect at the next piano attack.
Guitar **Pickup blend** moves from bridge-motion radiation through the acoustic body to a
position-dependent magnetic pickup approximation, progressively bypassing body coloration.
Violin **Bow speed** controls motion independently of **Bow Pressure**. **Bow response** sets its
2–120 ms response time (about 57 ms by default), including while a note sounds. Output expression
changes smoothly with half that time constant. **Legato** uses last-note priority: overlapping
notes reuse the same vibrating string, and releasing the newest key returns to a previous held
key. It is off by default for polyphonic playing. Changing legato mode releases held notes.
As on other instruments, the parameter editor and automation lanes save ordinary plugin state.

MIDI CC7 controls channel volume. CC11 controls expression and violin bow speed, with a
`value^1.18` response on violin before smoothing; CC1 scales bow
pressure. CC64 holds released piano strings until the pedal lifts. Note-off damps/releases a note,
all-notes-off releases the whole pool, and all-sound-off uses the shared de-click envelope.
Pitch bends act on sounding strings/modes, so the existing slide and pitch-performance stages
can play these instruments. The pool holds up to 24 voices and steals released/quiet voices first.
Excitation is deterministic and rendering is independent of block size.

## Composition

A song can select a native instrument directly:

```toml
performance = "pop-band"
form = ["verse"]

[[part]]
name = "guitar"
role = "chords"
instrument = "auris.physical.guitar"
```

The composer recognises native piano, guitar, bass and violin IDs when choosing editable stroke,
strum, mute, slide and pitch-performance stages. Existing GM family hints also resolve to these
models: piano/electric piano, guitar, bass, bells, mallets and strings. A muted-guitar hint starts
with more damping; picked/slap bass starts with harder contact; string pads start with gentler
bow pressure and a longer release. These are starting parameters for Auris instruments, not GM
patch emulation. Drum hints use the built-in Drum Kit.

Accompaniment and lyric-song backing share the same choices. Native tracks require no SoundFont
reference. An explicit SoundFont source or imported preset continues to choose that exact font.
Unsupported musical families use the session's existing reported fallback or an optional installed
GM library. Melody roles start native violin with legato; pad roles start with slower bow motion
and polyphony. Explicit parameter choices take precedence. Notes and their seeded recipes remain
unchanged by instrument selection.

## Mechanical editor

Open a physical instrument's parameter editor from its track's instrument row. The **Model
motion** region stays beside the ordinary parameter controls: strings for piano/guitar/bass/violin,
bar bending for mallet, and a shell perimeter for bell. It follows the edited track during
playback or musical typing, independently of the selected track. **Play C4** holds a test note;
**Release C4** lets it decay. Octave buttons change that test note while retaining its held state.
Closing or replacing the editor releases its test note.

Up to four recent voices show note/bend, held/releasing state and relative spatial motion. A
marker locates the excitation contact, a moving bow indicator follows actual bow motion, and
modal instruments show the first eight resonance magnitudes. Piano/struck/plucked contact
remains at that voice's excitation position until the next attack; violin contact follows its
smoothed junction while sounding. The readout includes expression, violin bow pressure and
piano pedal state.

The diagram uses live wave histories or a low-mode spatial projection, before body coloration
and output gain. It is a relative mechanical view, not measured displacement in metres. Delay
loops expose a fixed-end spatial projection; bell/bar modal bases are illustrative geometric
projections of their live mode coordinates. The display samples about 30 times per second, so
it is not a slow-motion integration of the audio-rate model. Fixed 6/24/96× display magnification
keeps dynamics and decay visible. **Freeze** retains a snapshot and stops observation;
**Resume display** continues it. Audio keeps running in both states.

Parameter controls keep their ordinary keyboard adjustment, typed values, reset, automation,
save/load and one-gesture Undo behavior. The window scrolls at smaller sizes; Tab reaches the
test-note/display buttons and sliders, Enter/Space activate a button, and Escape closes it.

## Development verification

```sh
cargo test -p auris-synth -p auris-compose -p auris-session
cargo run -p auris-synth --example physical_demo -- target/physical-demo.wav
cargo run --release -p auris-synth --example physical_bench -- --motion
cargo test -p auris-gpui --bins physical_model
```

Tests measure pitch at multiple rates, velocity response, hardness-dependent upper modes,
decay/sustain, pedal release, pitch bends, extreme settings and callback allocations. The pack's
shared tests also cover event offsets, mono/multichannel buffers and deterministic rendering.
The example writes a listening probe and reports each model's peak and RMS; it is dev tooling.
Mechanical-observation tests compare watched/unwatched PCM bit for bit, assert fixed string
endpoints and nonzero finite model motion, exercise concurrent coherent readers, retain an
attacked pluck's contact, disable retired graph readers, and count callback allocations with
observation enabled. Native-window harness tests cover preview, display magnification by
keyboard, freeze/resume, scrolling to parameters, Undo, source replacement and Escape.

The [radiation/register experiments](physical-extensions.md) compare causal resonances,
register curves and bow response against real recordings with held-out temporal log-mel loss.
The [long-note/trajectory calibration](physical-trajectories.md) measures sustained tone,
played vibrato/glissando, expression following and release against real recordings.
Factory piano, guitar and violin settings and radiation gains are calibrated against real
recordings with [temporal mel copy synthesis](physical-copy-synthesis.md). That document records
the objective, fixed training/validation split, reproducible commands and remaining limits.
The [bass/bar/drum measurements](physical-pack.md) calibrate the remaining physical models and
seven kit families against real recordings, with separate attack, body, tail and envelope errors.
The [before/after measurements](physical-instruments-evaluation.md) record the full-preset
comparison, including the lower learned scores for sustained-string and ambient arrangements.

The design follows the modal-expansion and travelling-wave principles in Julius O. Smith's
[Physical Audio Signal Processing](https://www.dsprelated.com/freebooks/pasp/), especially
[modal expansion](https://www.dsprelated.com/freebooks/pasp/Modal_Expansion.html) and
[bowed strings](https://www.dsprelated.com/freebooks/pasp/Bowed_Strings.html).
