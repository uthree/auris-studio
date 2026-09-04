# RustySynth, forked for Auris Studio

This is [rustysynth](https://github.com/sinshu/rustysynth) 1.3.6 by Nobuaki Tanaka, MIT licensed,
with one thing added. Everything below this section is the upstream README.

## What was added, and why

**The published crate reads a SoundFont's modulator lists and throws them away.** `pmod` and
`imod` were `discard_data`, and only the generators reached a voice. A modulator is how a font
says "this controller reaches that parameter" — and the ordinary way to make a sampled sound
respond to how hard it is played is to set a filter low in the generators and open it with a
modulator driven by velocity.

MuseScore General's acoustic pianos do exactly that. Measured through the published crate, the
Grand Piano's filter never opened: one note at middle C peaked 20 dB under every other program in
the font, and *fell* by 20 dB between MIDI velocity 74 and 76, because the layer boundary there
swapped one static `modEnvToFilterFc` for another that the discarded modulators were meant to
override. Playing harder made it quieter. With the modulators applied it runs -21.1, -20.2, -18.4,
-17.6, -14.3, -13.4, -12.1, -11.3 dBFS across velocities 70 to 115 — monotonic, and level with the
rest of the font.

The modulator change is `src/modulator.rs` and `src/error.rs`, plus the lines that carry a modulator list from the file to a
voice: `zone.rs`, `soundfont_parameters.rs`, `preset_region.rs`, `instrument_region.rs`,
`region_pair.rs` and `voice.rs`. Every addition is marked "Added by the Auris fork".

The fork also validates every preset, instrument, zone and generator span before slicing its
tables. Upstream trusts those file-provided indices and can panic on a malformed SoundFont;
Auris treats them as ordinary `SoundFontError` values so opening a bad library file cannot take
down the application.

## What it does not do

* **Only the two filter destinations are read through modulators** — `initialFilterFc` and
  `modEnvToFilterFc`. What a font says about *loudness* with a modulator is ignored, because
  `Voice::start` already applies the specification's default velocity-to-attenuation curve by
  hand; applying both would count a font's own velocity shaping twice. Auris compensates for that
  curve in `auris_sampler::midi_velocity`, so changing it is a larger decision than this fork.
* **Only controllers that hold still for the length of a note**: note-on velocity, note-on key,
  and the constant "no controller". A voice reads its modulators once, at note-on, so a
  continuous controller, the pitch wheel or aftertouch would be frozen at whatever it was —
  worse than not answering, because it would look like support. Those modulators are skipped.
* **The specification's own default modulator list is not implemented.** Only what a font
  declares is applied. Upstream models one default by hand and none of the others; this fork adds
  nothing there.

Measured over the 128 melodic programs of MuseScore General: 101 are bit-identical, the three
acoustic pianos come up 19.9, 19.5 and 11.7 dB, and the remaining 24 move by less than 3 dB.

## Keeping it

Out of the workspace on purpose — see `exclude` in the root manifest. `cargo test` and
`cargo clippy` from *this* directory run its own suite; two upstream tests fail there because they
want SoundFont files the published crate does not ship.

---

# RustySynth

RustySynth is a SoundFont MIDI synthesizer written in pure Rust, ported from [MeltySynth](https://github.com/sinshu/meltysynth).



## Features

* Suitable for both real-time and offline synthesis.
* Supports standard MIDI files with additional features including dynamic tempo changing.
* No dependencies other than the standard library.



## Examples

An example code to synthesize a simple chord:

```rust
// Load the SoundFont.
let mut sf2 = File::open("TimGM6mb.sf2").unwrap();
let sound_font = Arc::new(SoundFont::new(&mut sf2).unwrap());

// Create the synthesizer.
let settings = SynthesizerSettings::new(44100);
let mut synthesizer = Synthesizer::new(&sound_font, &settings).unwrap();

// Play some notes (middle C, E, G).
synthesizer.note_on(0, 60, 100);
synthesizer.note_on(0, 64, 100);
synthesizer.note_on(0, 67, 100);

// The output buffer (3 seconds).
let sample_count = (3 * settings.sample_rate) as usize;
let mut left: Vec<f32> = vec![0_f32; sample_count];
let mut right: Vec<f32> = vec![0_f32; sample_count];

// Render the waveform.
synthesizer.render(&mut left[..], &mut right[..]);
```

Another example code to synthesize a MIDI file:

```rust
// Load the SoundFont.
let mut sf2 = File::open("TimGM6mb.sf2").unwrap();
let sound_font = Arc::new(SoundFont::new(&mut sf2).unwrap());

// Load the MIDI file.
let mut mid = File::open("flourish.mid").unwrap();
let midi_file = Arc::new(MidiFile::new(&mut mid).unwrap());

// Create the MIDI file sequencer.
let settings = SynthesizerSettings::new(44100);
let synthesizer = Synthesizer::new(&sound_font, &settings).unwrap();
let mut sequencer = MidiFileSequencer::new(synthesizer);

// Play the MIDI file.
sequencer.play(&midi_file, false);

// The output buffer.
let sample_count = (settings.sample_rate as f64 * midi_file.get_length()) as usize;
let mut left: Vec<f32> = vec![0_f32; sample_count];
let mut right: Vec<f32> = vec![0_f32; sample_count];

// Render the waveform.
sequencer.render(&mut left[..], &mut right[..]);
```



## Todo

* __Wave synthesis__
    - [x] SoundFont reader
    - [x] Waveform generator
    - [x] Envelope generator
    - [x] Low-pass filter
    - [x] Vibrato LFO
    - [x] Modulation LFO
* __MIDI message processing__
    - [x] Note on/off
    - [x] Bank selection
    - [x] Modulation
    - [x] Volume control
    - [x] Pan
    - [x] Expression
    - [x] Hold pedal
    - [x] Program change
    - [x] Pitch bend
    - [x] Tuning
* __Effects__
    - [x] Reverb
    - [x] Chorus
* __Other things__
    - [x] Standard MIDI file support
    - [x] MIDI file loop extension support
    - [x] Performace optimization



## License

RustySynth is available under [the MIT license](LICENSE.txt).
