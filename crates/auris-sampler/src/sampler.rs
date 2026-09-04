//! The instrument that plays a SoundFont.

use std::sync::Arc;

use auris_core::param::{ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain};
use auris_core::plugin::{
    Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginState,
    PrepareContext, ProcessContext,
};
use auris_core::registry::PluginRegistry;
use auris_core::{AudioBuffer, PresetRef};
use auris_dsp::Adsr;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::bank::SharedSoundFonts;

/// The sampler's plugin id, as a project file stores it.
pub const SAMPLER_ID: &str = "auris.sampler.soundfont";

/// The channel a note is played on when the envelope is leaving the font alone.
///
/// A track is one sound, and the bank and patch that choose it are set on the channel rather
/// than carried by each note, so one channel is all an unshaped sampler needs.
const CHANNEL: i32 = 0;

/// The channels a *shaped* note may be given, one note to a channel.
///
/// Channel expression is the only per-note gain rustysynth exposes, and it applies to every
/// voice on its channel — so a note that is to be faded on its own has to have a channel to
/// itself, and the number of channels is the polyphony. Fourteen notes is thin next to the 128
/// voices the library will hold, which is the price of shaping and the reason an untouched
/// envelope does not pay it.
///
/// Channel 9 is missing on purpose: rustysynth treats it as the percussion channel and adds 128
/// to whatever bank is selected there, so a slot on it would play a different sound from the
/// other channels. [`CHANNEL`] is missing for a subtler reason: an unshaped note plays there
/// without ever appearing in the slot table, so a slot on the same channel would look free to
/// [`claim`] while a note is sounding on it — and the first shaped note after an off→on flip of
/// the envelope would write its own fade over that held note's loudness.
const SLOTS: [i32; 14] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];

/// Every channel the sampler speaks on: the unshaped one, then the shaped slots.
///
/// The iterator a channel-wide message loops over — bank and patch selection, pitch bend, a
/// forwarded controller. Looping over [`SLOTS`] alone would leave the unshaped channel behind
/// on all three, now that it is no longer one of them.
fn all_channels() -> impl Iterator<Item = i32> {
    std::iter::once(CHANNEL).chain(SLOTS)
}

/// Expression the library's own channel reset leaves behind, and so the number that means "as
/// loud as this sampler is with no envelope over it".
///
/// The envelope is scaled into this rather than into the controller's full 16383, so a shaped
/// note at full level is *exactly* as loud as an unshaped one. Everything measured for
/// [`NOMINAL_VOLUME`] was measured through this value, and a fifteenth of a decibel that only
/// appears once you touch a control is the kind of difference nobody ever tracks down.
const FULL_EXPRESSION: i32 = 127 << 7;

/// Frames the synthesiser computes at a time, internally.
///
/// Note events land on the next multiple of this, so it sets the timing resolution: 32 frames is
/// two thirds of a millisecond at 48 kHz, comfortably below where a listener hears a drum hit
/// arrive late. The library's own default is 64, which is twice that and audible on tight
/// percussion; going below 32 buys accuracy nobody can hear and pays per-voice overhead for it.
const INTERNAL_BLOCK: usize = 32;

/// Voices one instance may sound at once.
///
/// A voice is a few hundred bytes, and an orchestral font can spend three or four of them on a
/// single key, so the library's default of 64 runs out on a sustained chord long before anything
/// else does.
const MAX_VOICES: usize = 128;

/// Semitones of pitch bend at full deflection.
///
/// [`NoteEvent::PitchBend`] carries semitones, so the range only has to be wide enough to hold
/// what is asked for; an octave covers every bend a person writes and still leaves the 14-bit
/// wheel resolving under a thousandth of a semitone.
const BEND_RANGE: i32 = 12;

/// The gain that puts a General MIDI program where a built-in instrument sits.
///
/// Four times what the library calls unity, which is +12.04 dB. The number is measured rather
/// than chosen: one note at velocity 1.0 with the gain flat peaks at -6.0 dBFS through the
/// built-in `Chiptune` at its defaults, and the median of the 128 melodic programs of MuseScore
/// General peaked 11.45 dB below that at the library's own unity — 11.87 dB below over a played
/// phrase rather than one held note. Rounding that to a whole factor of four leaves the median
/// program within 0.6 dB of the built-ins.
///
/// The `level` parameter is measured from here, so 0 dB is *the level the rest of the
/// application answers a note with*, which is not the same promise the old constant made: it
/// used to mean "the sound the font was voiced for", and a font voiced for a player that reads
/// the format's own attenuation conventions came out far quieter than everything around it.
///
/// This leaves no headroom, and is not supposed to: a built-in instrument does not limit itself
/// either, and its own triad already reaches +3.0 dBFS. See [`Sampler`] for what a chord peaks
/// at and where that bites.
const NOMINAL_VOLUME: f32 = 2.0;

/// Turns a note's velocity into the MIDI velocity that makes the synthesiser answer it linearly.
///
/// A SoundFont synthesiser makes amplitude proportional to the *square* of MIDI velocity — the
/// format's default velocity-to-attenuation curve, which rustysynth implements as a fixed
/// `2 * 20 * log10(velocity / 127)` decibels and which no font can override here: the fork in
/// `vendor/rustysynth` reads the modulator lists the published crate discards, but deliberately
/// applies only the two that reach the *filter*, so that a font's own velocity-to-loudness
/// shaping is not counted twice on top of this curve. Everything else in Auris is
/// linear: the built-in instruments' voice allocator assigns `level = velocity`, and a built-in
/// part rendered at 0.56 instead of 1.0 measures -5.04 dB against the sampler's -10.10 dB for
/// the same part.
///
/// Two laws for one word is the bug. Taking the square root here cancels the squaring, so a
/// velocity means the same thing on both paths and a composed part keeps the dynamic range it
/// was written with rather than twice it. The cost is that Auris now disagrees with other
/// SoundFont players about what a velocity sounds like, which is the honest trade: the
/// application's own consistency is worth more than agreeing with a convention no part of it
/// follows anywhere else.
///
/// The correction is exact up to the integer the synthesiser takes: the rounding leaves at most
/// `40 * log10(1 + 0.5 / midi)` decibels of error, which is 0.22 dB at velocity 0.1 and less
/// above it. Quiet notes are actually served *better* than before, because a velocity of 0.01
/// now arrives as MIDI 13 rather than MIDI 1, where a single step is the whole signal.
///
/// Velocity 0 is a note-off in MIDI, so a note written silently rounds up to 1 rather than
/// releasing itself.
fn midi_velocity(velocity: f32) -> i32 {
    let compensated = velocity.clamp(0.0, 1.0).sqrt();
    ((compensated * 127.0).round() as i32).max(1)
}

// MIDI messages, by their wire values, so the calls below read as what they are.
const CONTROL_CHANGE: i32 = 0xB0;
const PROGRAM_CHANGE: i32 = 0xC0;
const PITCH_BEND: i32 = 0xE0;
const CC_BANK_SELECT: i32 = 0x00;
const CC_DATA_ENTRY: i32 = 0x06;
const CC_RPN_LSB: i32 = 0x64;
const CC_RPN_MSB: i32 = 0x65;
const CC_EXPRESSION: i32 = 0x0B;
const CC_EXPRESSION_FINE: i32 = 0x2B;

/// Whether a controller is one the sampler spends on itself.
///
/// The expression pair carries the per-note envelope — see `write_expression` — so those two are
/// the sampler's, and a clip that writes a lane on either is answered with silence rather than
/// with a fade that fights it.
fn is_reserved(number: u8) -> bool {
    let number = i32::from(number);
    matches!(
        number,
        CC_BANK_SELECT
            | CC_DATA_ENTRY
            | CC_RPN_LSB
            | CC_RPN_MSB
            | CC_EXPRESSION
            | CC_EXPRESSION_FINE
    )
}

/// Where a preset choice lives inside [`PluginState::extra`].
///
/// Nested under a key rather than being the whole of `extra`, so this is not the only thing the
/// sampler can ever keep there.
const PRESET_KEY: &str = "preset";

const LEVEL: usize = 0;
const ENVELOPE: usize = 1;
const ATTACK: usize = 2;
const DECAY: usize = 3;
const SUSTAIN: usize = 4;
const RELEASE: usize = 5;
const PARAM_COUNT: usize = 6;

/// The key of the switch, so a frontend can find it without knowing where it sits.
pub const ENVELOPE_KEY: &str = "envelope";

/// Whether these parameter values ask the sampler to shape a note at all.
///
/// One switch, and nothing else. It could have been inferred — an attack of zero holding at full
/// and letting go at once multiplies every note by one — but a feature that turns itself on when
/// a slider moves is a feature nobody can turn off with confidence, and this one has a price to
/// warn about. Off means the sampler skips the mechanism entirely and plays the font exactly as
/// it is written, choke groups and all 128 voices included; on means it does not.
fn shaping(values: &[f32; PARAM_COUNT]) -> bool {
    values[ENVELOPE] >= 0.5
}

/// Writes a preset choice into the state a project stores for a track's instrument.
///
/// Public because choosing a preset is a *session* command — the document has to record the
/// choice before a graph is built — and the encoding should be written down once.
pub fn store_preset(state: &mut PluginState, preset: PresetRef) {
    let encoded = serde_json::to_value(preset).unwrap_or(serde_json::Value::Null);
    match state.extra.as_object_mut() {
        Some(map) => {
            map.insert(PRESET_KEY.to_string(), encoded);
        }
        None => {
            let mut map = serde_json::Map::new();
            map.insert(PRESET_KEY.to_string(), encoded);
            state.extra = serde_json::Value::Object(map);
        }
    }
}

/// Reads a preset choice back, or `None` when the state names none.
pub fn stored_preset(state: &PluginState) -> Option<PresetRef> {
    let stored = state.extra.get(PRESET_KEY)?;
    serde_json::from_value(stored.clone()).ok()
}

/// Registers the sampler, giving it the bank it will read fonts from.
///
/// Not a [`PluginPack`](auris_core::registry::PluginPack) like the other built-ins: a pack
/// registers itself from a static method and so has nothing to hand its plugins, whereas every
/// sampler needs the same bank. The closure captures it, which is the whole reason this function
/// exists.
pub fn register_sampler(registry: &mut PluginRegistry, fonts: SharedSoundFonts) {
    registry.register_instrument(move || Box::new(Sampler::new(Arc::clone(&fonts))));
}

/// Plays one preset of one SoundFont.
///
/// # Realtime behaviour
///
/// The synthesiser is built in [`Instrument::prepare`], which is where the font is looked up and
/// every buffer it needs is allocated. [`Instrument::process`] renders into buffers that already
/// exist and touches neither the bank nor the filesystem, and [`Instrument::reset`] — reachable
/// from the callback through every transport stop and seek — re-selects the preset `prepare`
/// resolved rather than going back to the bank, whose lock is not one to take there.
///
/// If the library panics while rendering — a degenerate font can push its oscillator off the end
/// of the sample data — the panic is caught and the instance goes silent until the next
/// `prepare` builds a fresh synthesiser. One track's sound is the most a bad font may cost.
///
/// Note events are honoured by splitting the block at each event's frame, so timing does not
/// depend on the host's buffer size — down to the synthesiser's own internal block, which is
/// what actually bounds it.
///
/// # What a velocity means, and what it costs
///
/// Velocity is linear here, as it is everywhere else in Auris: doubling a note's velocity
/// doubles its amplitude, which is not what the SoundFont format says and is what every other
/// instrument in the application does. Two consequences are worth knowing before reaching for
/// the mixer.
///
/// A velocity chooses a *sample*, not only a level. Raising the number handed to the
/// synthesiser moves a multi-sampled program up its velocity layers, so a kit or a piano
/// answers a mid-strength note with a harder recorded hit than it did before — a change of
/// timbre and not only of level. Swept every third velocity from 40 to 127 across the 128
/// melodic programs of MuseScore General, one program departs from the smooth law by more than
/// 3 dB: the jazz guitar, by 4.8 dB, where its layers change. What linearising moves is the
/// written velocity each discontinuity sits at, not how large it is.
///
/// It used to be three programs and 21 dB. The two acoustic pianos and the honky-tonk set a low
/// filter in their generators and open it with a modulator, and the published rustysynth
/// discards modulators — so the filter stayed shut and a layer boundary that swapped one static
/// `modEnvToFilterFc` for another read as a twenty-decibel cliff that *fell* as the note was
/// struck harder. `vendor/rustysynth` reads them; its README carries the measurement.
///
/// A chord can exceed full scale, and is meant to be able to. One note at velocity 1.0 with the
/// gain flat peaks around -6 dBFS on a typical program, a triad around -1, and ten fingers
/// around +2 — against a built-in instrument's +3 for the same triad and +10.5 for the same ten.
/// Percussion is the exception that stays uncomfortable: the font's kits are voiced hotter than
/// its pitched programs by 7 dB or so, and the loudest single kick in the shipped font peaks at
/// +6.5 dBFS on its own. That is the font's own program-to-program balance, which one constant
/// cannot correct and should not try to; it belongs to the mixer.
pub struct Sampler {
    fonts: SharedSoundFonts,
    params: Vec<ParamDescriptor>,
    values: [f32; PARAM_COUNT],
    /// Which sound to play, from the saved state. `None` means nothing has chosen one.
    preset: Option<PresetRef>,
    /// What [`Self::resolve`] answered when the synthesiser was built.
    ///
    /// `reset` re-selects from this rather than resolving again: it runs on the audio thread,
    /// where taking the bank's lock — let alone logging a missing font — is not allowed.
    selected: Option<PresetRef>,
    /// Built in `prepare`; `None` when there is no font to play.
    synth: Option<Synthesizer>,
    /// Set when the synthesiser panicked while rendering.
    ///
    /// A poisoned synthesiser is never called again — whatever state the panic left it in is
    /// not one to play — but it is also not dropped here, because dropping it would free its
    /// buffers on the audio thread. It waits for the next `prepare`.
    poisoned: bool,
    /// Only used to fold stereo into a host that asked for one channel.
    scratch_left: Vec<f32>,
    scratch_right: Vec<f32>,
    /// One per entry of [`SLOTS`], holding the envelope of whatever note is on that channel.
    ///
    /// Built once in [`Sampler::new`] and never resized, because it is indexed from the audio
    /// thread.
    slots: Vec<Slot>,
    /// Counts up as slots are taken, so the smallest number is the one least recently used.
    clock: u64,
    /// Keys held, for the activity indicator.
    held: u32,
}

/// One channel's worth of shaped note.
#[derive(Debug)]
struct Slot {
    envelope: Adsr,
    /// The key sounding here, until the envelope has taken it to silence and handed the
    /// note-off to the font. `None` means the channel is free, tail or no tail.
    key: Option<u8>,
    /// [`Sampler::clock`] when this slot was last taken.
    used: u64,
    /// The expression last written to this channel, so an unchanged level costs nothing.
    expression: i32,
}

/// The slot a new note should take.
///
/// A channel nobody is holding is preferred, least recently taken first: a font's own release
/// tail keeps sounding on a channel after the note-off has gone, and going round the houses gives
/// it fourteen other notes of grace before anything lands on top of it. When every channel is
/// held, the quietest note is the one whose loss is least heard.
fn claim(slots: &[Slot]) -> usize {
    slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.key.is_none())
        .min_by_key(|(_, slot)| slot.used)
        .or_else(|| {
            slots
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.envelope.level().total_cmp(&b.envelope.level()))
        })
        .map_or(0, |(index, _)| index)
}

impl Sampler {
    /// A sampler reading fonts from `fonts`.
    pub fn new(fonts: SharedSoundFonts) -> Self {
        // A level and an envelope, because the font is the sound. Anything else a track wants
        // doing to it — ambience above all — belongs in its effect chain, where the offline
        // renderer knows to keep rendering until the tail has fallen silent. An instrument
        // reports no tail at all, so a reverb built into this one would be cut off at the end of
        // every export.
        //
        // The four envelope controls carry the same keys, ranges and curves as the built-in
        // instruments', so the same picture is drawn over them and dragging a corner means the
        // same thing wherever it is done.
        //
        // Their defaults are what turning the switch on sounds like, and they are chosen to be a
        // small step rather than none: flat while the key is down, with a fifth of a second of
        // release so a note is let go rather than chopped. A release of zero would be the closest
        // thing to "the font untouched", and it is also the one setting that would make the first
        // thing anybody hears after finding the switch sound broken.
        let params = vec![
            ParamDescriptor::decibels(LEVEL as u32, "level", "Level", -60.0, 12.0, 0.0),
            ParamDescriptor::toggle(ENVELOPE as u32, ENVELOPE_KEY, "Envelope", false),
            ParamDescriptor::new(ATTACK as u32, "attack", "Attack", 0.0, 2.0, 0.0)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Power(3.0)),
            ParamDescriptor::new(DECAY as u32, "decay", "Decay", 0.0, 4.0, 0.0)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Power(3.0)),
            ParamDescriptor::percent(SUSTAIN as u32, "sustain", "Sustain", 1.0),
            ParamDescriptor::new(RELEASE as u32, "release", "Release", 0.0, 4.0, 0.2)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Power(3.0)),
        ];
        let mut values = [0.0; PARAM_COUNT];
        for (slot, descriptor) in values.iter_mut().zip(&params) {
            *slot = descriptor.default;
        }
        let slots = SLOTS
            .iter()
            .map(|_| Slot {
                envelope: Adsr::new(),
                key: None,
                used: 0,
                expression: FULL_EXPRESSION,
            })
            .collect();
        let mut sampler = Self {
            fonts,
            params,
            values,
            preset: None,
            selected: None,
            synth: None,
            poisoned: false,
            scratch_left: Vec::new(),
            scratch_right: Vec::new(),
            slots,
            clock: 0,
            held: 0,
        };
        sampler.reshape();
        sampler
    }

    /// Restates the envelope's four times on every slot, from the current parameter values.
    fn reshape(&mut self) {
        let (attack, decay, sustain, release) = (
            self.values[ATTACK],
            self.values[DECAY],
            self.values[SUSTAIN],
            self.values[RELEASE],
        );
        for slot in &mut self.slots {
            slot.envelope.set_adsr(attack, decay, sustain, release);
        }
    }

    /// Whether the envelope is doing anything to the font's own sound. See [`shaping`].
    fn shaped(&self) -> bool {
        shaping(&self.values)
    }

    /// Which sound this sampler has been told to play.
    pub fn preset(&self) -> Option<PresetRef> {
        self.preset
    }

    /// `true` once a font has been found and a synthesiser built for it.
    pub fn has_voice(&self) -> bool {
        self.synth.is_some()
    }

    /// The font and preset to play, or `None` when there is nothing to play.
    ///
    /// A track that names a font the bank does not hold stays silent rather than falling back:
    /// the file has gone missing, and quietly playing some other font's piano would hide that.
    /// A track that names *nothing* is a different case — a sampler just dropped onto a track —
    /// and takes the first font loaded, so it makes a sound the moment it is used.
    fn resolve(&self) -> Option<(Arc<SoundFont>, PresetRef)> {
        match self.preset {
            Some(preset) => match self.fonts.get(preset.font) {
                Some(font) => Some((font, preset)),
                None => {
                    log::warn!(
                        "soundfont {} is not loaded, so this track stays silent",
                        preset.font.0
                    );
                    None
                }
            },
            None => self.fonts.first().map(|(id, font)| {
                // Bank 0, patch 0 is the general-MIDI piano. A font without it falls through to
                // the library's own default, which is the preset with the lowest number.
                (
                    font,
                    PresetRef {
                        font: id,
                        bank: 0,
                        patch: 0,
                    },
                )
            }),
        }
    }

    /// Points the channel at a preset and sets up the pitch-bend range.
    ///
    /// Called after every reset as well as after building: resetting the synthesiser resets its
    /// channels too, which would otherwise leave the track playing bank 0 patch 0.
    /// Every slot channel is set up, not only the one in use, so that turning the envelope on
    /// halfway through a take does not need a channel configured from the audio thread.
    fn select(&mut self, preset: PresetRef) {
        let Some(synth) = self.synth.as_mut() else {
            return;
        };
        for channel in all_channels() {
            synth.process_midi_message(channel, CONTROL_CHANGE, CC_BANK_SELECT, preset.bank);
            synth.process_midi_message(channel, PROGRAM_CHANGE, preset.patch, 0);
            // Registered parameter 0 is the bend range; it takes both halves of the selection.
            synth.process_midi_message(channel, CONTROL_CHANGE, CC_RPN_MSB, 0);
            synth.process_midi_message(channel, CONTROL_CHANGE, CC_RPN_LSB, 0);
            synth.process_midi_message(channel, CONTROL_CHANGE, CC_DATA_ENTRY, BEND_RANGE);
        }
    }

    /// Sends one slot's envelope level to its channel, if it has moved since the last write.
    ///
    /// Square-rooted on the way out, for the reason [`midi_velocity`] is: the library follows the
    /// General MIDI spec and *squares* the channel's volume and expression before a voice is
    /// multiplied by it. Writing the level straight through would make a fade that says it is at
    /// half fall to a quarter, and a release stated in seconds arrive twice as fast as it reads.
    fn write_expression(&mut self, index: usize) {
        let level = self.slots[index].envelope.level().clamp(0.0, 1.0);
        self.write_raw_expression(
            index,
            (level.sqrt() * FULL_EXPRESSION as f32).round() as i32,
        );
    }

    /// Writes a 14-bit expression value to a slot's channel, unless it is already there.
    fn write_raw_expression(&mut self, index: usize, raw: i32) {
        if self.slots[index].expression == raw {
            return;
        }
        self.slots[index].expression = raw;
        let channel = SLOTS[index];
        if let Some(synth) = self.synth.as_mut() {
            // Coarse first: the library keeps the two halves of the controller in one 14-bit
            // number, and writing the low half second is what leaves the whole of it set.
            synth.process_midi_message(channel, CONTROL_CHANGE, CC_EXPRESSION, raw >> 7);
            synth.process_midi_message(channel, CONTROL_CHANGE, CC_EXPRESSION_FINE, raw & 0x7F);
        }
    }

    /// Hands every shaped note back to the font and puts every channel to full.
    ///
    /// Run when the envelope stops shaping. Without it, the controls going back to their
    /// defaults would leave whatever gain the last fade ended on written into the channel — and
    /// worse, a note the envelope was holding open would have nothing left to close it.
    fn release_the_slots(&mut self) {
        for index in 0..self.slots.len() {
            self.slots[index].envelope.silence();
            self.hand_back(index);
            self.write_raw_expression(index, FULL_EXPRESSION);
        }
    }

    /// Tells the font the note on a slot is over, if the slot is holding one.
    ///
    /// Every path that ends a shaped note goes through here, because a slot whose key is cleared
    /// without the note-off being sent is a voice the library will hold open forever.
    fn hand_back(&mut self, index: usize) {
        if let Some(key) = self.slots[index].key.take()
            && let Some(synth) = self.synth.as_mut()
        {
            synth.note_off(SLOTS[index], key as i32);
        }
    }

    /// Sends one parameter's current value wherever it has to go.
    fn push(&mut self, index: usize) {
        match index {
            LEVEL => {
                let value = self.values[LEVEL];
                if let Some(synth) = self.synth.as_mut() {
                    synth.set_master_volume(NOMINAL_VOLUME * db_to_gain(value));
                }
            }
            ENVELOPE | ATTACK | DECAY | SUSTAIN | RELEASE => {
                self.reshape();
                if !self.shaped() {
                    // Switched off, possibly mid-fade. Every note the envelope was holding open
                    // goes back to the font and every channel goes back to full, or the sampler
                    // would keep playing under a gain nothing is moving any more.
                    self.release_the_slots();
                }
            }
            _ => {}
        }
    }

    /// Sends every parameter, for when a fresh synthesiser knows none of them.
    fn push_all(&mut self) {
        for index in 0..PARAM_COUNT {
            self.push(index);
        }
    }

    /// Applies one event.
    fn dispatch(&mut self, event: NoteEvent) {
        if self.synth.is_none() || self.poisoned {
            return;
        }
        match event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.start(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => self.let_go(pitch),
            NoteEvent::AllNotesOff { .. } => self.stop_everything(false),
            NoteEvent::AllSoundOff { .. } => self.stop_everything(true),
            NoteEvent::PitchBend { semitones, .. } => {
                let travel = (semitones / BEND_RANGE as f32).clamp(-1.0, 1.0);
                let raw = (8192.0 + travel * 8191.0).round().clamp(0.0, 16383.0) as i32;
                if let Some(synth) = self.synth.as_mut() {
                    for channel in all_channels() {
                        synth.process_midi_message(
                            channel,
                            PITCH_BEND,
                            raw & 0x7F,
                            (raw >> 7) & 0x7F,
                        );
                    }
                }
            }
            NoteEvent::Controller { number, value, .. } if !is_reserved(number) => {
                // Handed straight to the font rather than interpreted, whichever controller it is.
                // A SoundFont declares its own modulators — controller 1 is conventionally wired
                // to a vibrato depth in every General MIDI set — but a font is free to have wired
                // it to a filter, and what it was authored to do is what it should do.
                let raw = (value.clamp(0.0, 1.0) * 127.0).round() as i32;
                if let Some(synth) = self.synth.as_mut() {
                    for channel in all_channels() {
                        synth.process_midi_message(channel, CONTROL_CHANGE, i32::from(number), raw);
                    }
                }
            }
            // The expression pair is the sampler's own: `write_expression` spends it on the
            // per-note fade, several times a block. A lane written on it would be overwritten
            // before it was heard and would flatten the fade in between, so it is dropped here
            // rather than the two of them fighting over one controller.
            NoteEvent::Controller { .. } => {}
        }
    }

    /// Starts a note, on its own channel when the envelope has something to say about it.
    fn start(&mut self, pitch: u8, velocity: f32) {
        self.held = self.held.saturating_add(1);
        // See `midi_velocity`: the number handed over is pre-compensated, because the
        // synthesiser squares it and the rest of the application does not.
        let velocity = midi_velocity(velocity);
        if !self.shaped() {
            if let Some(synth) = self.synth.as_mut() {
                synth.note_on(CHANNEL, pitch as i32, velocity);
            }
            return;
        }
        let index = claim(&self.slots);
        // Whatever was here is handed back before the channel changes hands, so the note being
        // displaced ends rather than being re-shaped by the note that displaced it.
        self.hand_back(index);
        self.slots[index].envelope.silence();
        self.slots[index].envelope.trigger();
        self.slots[index].key = Some(pitch);
        self.slots[index].used = self.clock;
        self.clock = self.clock.wrapping_add(1);
        // Before the note, so the first block it is heard in is already at the level the
        // envelope says rather than at whatever the last note left behind.
        self.write_expression(index);
        if let Some(synth) = self.synth.as_mut() {
            synth.note_on(SLOTS[index], pitch as i32, velocity);
        }
    }

    /// Lets a note go: the envelope's release if it has one, the font's own otherwise.
    fn let_go(&mut self, pitch: u8) {
        self.held = self.held.saturating_sub(1);
        // The *oldest* note with this key. The same key struck twice while the first is still
        // sounding is two notes, and a note-off names a pitch rather than a note, so which of the
        // two it ends is this function's decision and not the caller's — but only one answer lets
        // both notes sound for the length they were written. Taking the newest ends the note that
        // has just begun, leaves the older one to be ended by the *next* off, and so plays the
        // first note over the second one's span and the second as a click.
        //
        // It is also the answer `auris_synth::VoiceAllocator::note_off` gives every built-in
        // voice, and two note-off policies in one workspace is a project that sounds different
        // depending on whether a part landed on a font.
        let held = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.key == Some(pitch) && !slot.envelope.is_releasing())
            .min_by_key(|(_, slot)| slot.used)
            .map(|(index, _)| index);
        match held {
            // The font is not told yet: the envelope owns the fade, and telling the font now
            // would start its own release underneath and cut the tail short. `step_envelopes`
            // hands the note back when the level reaches silence.
            Some(index) => self.slots[index].envelope.release(),
            // No slot holds it, so it is a note from before the envelope was turned on.
            None => {
                if let Some(synth) = self.synth.as_mut() {
                    synth.note_off(CHANNEL, pitch as i32);
                }
            }
        }
    }

    /// Ends everything: `immediate` cuts, otherwise every note is let go at once.
    fn stop_everything(&mut self, immediate: bool) {
        self.held = 0;
        for slot in &mut self.slots {
            if immediate {
                slot.key = None;
                slot.envelope.kill();
            } else if slot.key.is_some() {
                slot.envelope.release();
            }
        }
        if immediate {
            if let Some(synth) = self.synth.as_mut() {
                synth.note_off_all(true);
            }
            // Nothing is sounding, so the channels go back to full rather than staying wherever
            // the envelopes were cut off.
            for index in 0..self.slots.len() {
                self.write_raw_expression(index, FULL_EXPRESSION);
            }
        }
    }

    /// Advances every sounding envelope by `frames` and writes the levels it arrives at.
    ///
    /// Called immediately before the frames it describes are rendered: the library ramps a
    /// voice's gain from the previous block's value to this one across the block, so writing the
    /// level the envelope *ends* at is what draws the segment rather than a staircase.
    fn step_envelopes(&mut self, frames: usize) {
        for index in 0..self.slots.len() {
            if !self.slots[index].envelope.is_active() {
                continue;
            }
            for _ in 0..frames {
                self.slots[index].envelope.process();
            }
            self.write_expression(index);
            if self.slots[index].envelope.is_finished() {
                // The envelope has taken this note to silence, so the font can have its voices
                // back. Its own release runs from here, under a channel gain of zero.
                self.hand_back(index);
            }
        }
    }

    /// Renders `start..end` of the block, stepping the envelopes through it.
    ///
    /// An unshaped sampler renders the whole run in one call, exactly as it did before there was
    /// an envelope. A shaped one walks it in [`INTERNAL_BLOCK`] steps, which is the length the
    /// library renders and interpolates over anyway: shorter would buy resolution the voices
    /// cannot use, and longer would turn a fast attack into a staircase.
    fn render_range(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        if !self.shaped() {
            self.render_run(out, start, end);
            return;
        }
        let mut cursor = start;
        while cursor < end {
            let stop = (cursor + INTERNAL_BLOCK).min(end);
            self.step_envelopes(stop - cursor);
            self.render_run(out, cursor, stop);
            cursor = stop;
        }
    }

    /// Renders `start..end` in one go, whatever the envelopes are doing.
    fn render_run(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Sampler {
            synth,
            poisoned,
            scratch_left,
            scratch_right,
            held,
            ..
        } = self;
        let Some(synth) = synth.as_mut() else {
            return;
        };
        if *poisoned {
            // Poisoned mid-block: the rest of the block is silence, not another attempt.
            for channel in out.channels_mut() {
                channel[start..end].fill(0.0);
            }
            return;
        }
        match out.channels_mut() {
            [left, right, rest @ ..] => {
                if !rendered_safely(synth, &mut left[start..end], &mut right[start..end]) {
                    *poisoned = true;
                    *held = 0;
                    left[start..end].fill(0.0);
                    right[start..end].fill(0.0);
                    for extra in rest.iter_mut() {
                        extra[start..end].fill(0.0);
                    }
                    return;
                }
                // More than two channels is not something the engine asks for, but a buffer
                // that arrives with them should not come back with silence in the extras.
                for (index, extra) in rest.iter_mut().enumerate() {
                    let source: &[f32] = if index % 2 == 0 { left } else { right };
                    extra[start..end].copy_from_slice(&source[start..end]);
                }
            }
            [mono] => {
                let available = scratch_left.len().min(scratch_right.len());
                let frames = (end - start).min(available);
                if !rendered_safely(
                    synth,
                    &mut scratch_left[..frames],
                    &mut scratch_right[..frames],
                ) {
                    *poisoned = true;
                    *held = 0;
                    mono[start..end].fill(0.0);
                    return;
                }
                for (index, sample) in mono[start..start + frames].iter_mut().enumerate() {
                    *sample = 0.5 * (scratch_left[index] + scratch_right[index]);
                }
                // Prepared for a stereo host and handed a mono buffer anyway: there is no
                // scratch to render through, and silence beats whatever was in the buffer.
                mono[start + frames..end].fill(0.0);
            }
            [] => {}
        }
    }
}

/// Lets the synthesiser render, and answers whether it survived.
///
/// The library indexes its sample data with arithmetic a degenerate font can push out of range,
/// and this is the audio callback thread: an unwind escaping into the C callback would abort the
/// whole process. Catching costs nothing until something actually panics; the price of that —
/// the hook's message, an allocation for the payload — is paid once, on the way to silence.
///
/// `AssertUnwindSafe` is honest here because the synthesiser is never called again once
/// poisoned: whatever invariants the panic broke are invariants nobody will read.
fn rendered_safely(synth: &mut Synthesizer, left: &mut [f32], right: &mut [f32]) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| synth.render(left, right))).is_ok()
}

impl Parameterized for Sampler {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.params
    }

    fn param(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or(0.0)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        let index = id.index();
        let Some(descriptor) = self.params.get(index) else {
            return;
        };
        self.values[index] = descriptor.clamp(value);
        self.push(index);
    }

    fn save_state(&self) -> PluginState {
        let mut state = PluginState {
            params: self
                .params
                .iter()
                .map(|p| (p.key.to_string(), self.param(p.id)))
                .collect(),
            extra: serde_json::Value::Null,
        };
        if let Some(preset) = self.preset {
            store_preset(&mut state, preset);
        }
        state
    }

    fn load_state(&mut self, state: &PluginState) {
        for (key, value) in &state.params {
            self.set_param_by_key(key, *value);
        }
        // Which font to play cannot be an `f32` parameter, so it travels in `extra` and is
        // resolved by the next `prepare` — which is always the very next thing the graph does.
        self.preset = stored_preset(state);
    }
}

impl Instrument for Sampler {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            SAMPLER_ID,
            "SoundFont",
            "Plays an imported SoundFont",
            PluginCategory::Sampler,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        if self.poisoned {
            // Deferred from the audio thread, which had no business saying it there.
            log::warn!("the synthesiser panicked while rendering and is being rebuilt");
        }
        self.poisoned = false;
        self.synth = None;
        self.selected = None;
        self.held = 0;
        for slot in &mut self.slots {
            slot.envelope.set_sample_rate(ctx.sample_rate as f32);
            slot.envelope.silence();
            slot.key = None;
            // A fresh synthesiser knows nothing about what was written to the old one, and its
            // channels come up at full.
            slot.expression = FULL_EXPRESSION;
        }

        // Only a host asking for fewer than two channels needs anywhere to fold stereo down.
        let scratch = if ctx.channel_count >= 2 {
            0
        } else {
            ctx.max_block_frames
        };
        self.scratch_left.clear();
        self.scratch_left.resize(scratch, 0.0);
        self.scratch_right.clear();
        self.scratch_right.resize(scratch, 0.0);

        let Some((font, preset)) = self.resolve() else {
            return;
        };

        let rate = if ctx.sample_rate.is_finite() {
            ctx.sample_rate.round()
        } else {
            0.0
        };
        let mut settings = SynthesizerSettings::new(rate as i32);
        settings.block_size = INTERNAL_BLOCK;
        settings.maximum_polyphony = MAX_VOICES;
        // See `Sampler::new`: ambience is the effect chain's job, so the reverb and chorus
        // network is not built at all rather than being built and fed nothing.
        settings.enable_reverb_and_chorus = false;

        match Synthesizer::new(&font, &settings) {
            Ok(synth) => self.synth = Some(synth),
            Err(error) => {
                log::warn!("could not play soundfont {}: {error}", preset.font.0);
                return;
            }
        }
        self.selected = Some(preset);
        self.select(preset);
        self.push_all();
    }

    fn reset(&mut self) {
        self.held = 0;
        for slot in &mut self.slots {
            slot.envelope.silence();
            slot.key = None;
            // `Synthesizer::reset` puts every channel's controllers back, expression included.
            slot.expression = FULL_EXPRESSION;
        }
        if self.poisoned {
            // The synthesiser died rendering; whatever state it was left in is not one to
            // call back into. `prepare` will build a fresh one.
            return;
        }
        if let Some(synth) = self.synth.as_mut() {
            synth.reset();
        }
        // A reset returns the channel to bank 0 patch 0 with the library's own controller
        // defaults, so everything the track chose has to be said again — from what `prepare`
        // resolved, because this runs on the audio thread and the bank's lock does not.
        if let Some(preset) = self.selected {
            self.select(preset);
        }
        self.push_all();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.synth.is_none() || self.poisoned {
            out.clear();
            return;
        }
        let frames = out.frame_count().min(ctx.block_frames);

        let mut cursor = 0;
        let mut next_event = 0;
        while cursor < frames {
            while let Some(event) = events.get(next_event) {
                if event.frame() as usize > cursor {
                    break;
                }
                self.dispatch(*event);
                next_event += 1;
            }
            // The next event is where this run has to stop; the inner loop leaves it strictly
            // past the cursor, so the run is never empty and the outer loop always advances.
            let end = events
                .get(next_event)
                .map_or(frames, |event| (event.frame() as usize).min(frames));
            self.render_range(out, cursor, end);
            cursor = end;
        }
        // An event on the block's last frame has nothing left to render but still has to be
        // heard — a note-off there is what stops the note in the block after this one.
        while let Some(event) = events.get(next_event) {
            self.dispatch(*event);
            next_event += 1;
        }
    }

    fn active_voices(&self) -> usize {
        // Keys held, not voices sounding: the library does not publish its voice count, and one
        // key of a layered font is several voices. Close enough for an activity indicator, and
        // it is at least never wrong about whether anything is playing.
        self.held as usize
    }
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler")
            .field("preset", &self.preset)
            .field("playing", &self.synth.is_some())
            .field("held", &self.held)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::SoundFontBank;
    use auris_core::SoundFontId;

    fn sampler() -> Sampler {
        Sampler::new(SoundFontBank::shared())
    }

    fn preset() -> PresetRef {
        PresetRef {
            font: SoundFontId(7),
            bank: 128,
            patch: 42,
        }
    }

    #[test]
    fn a_preset_survives_a_round_trip_through_the_saved_state() {
        // The document is where a preset choice lives between sessions, so this is the join
        // that decides whether a project opens playing what it was saved playing.
        let mut state = PluginState::empty();
        store_preset(&mut state, preset());
        assert_eq!(stored_preset(&state), Some(preset()));
    }

    #[test]
    fn storing_a_preset_leaves_the_rest_of_the_state_alone() {
        let mut state = PluginState::empty();
        state.extra = serde_json::json!({ "something-else": 3 });
        store_preset(&mut state, preset());
        assert_eq!(stored_preset(&state), Some(preset()));
        assert_eq!(
            state.extra.get("something-else"),
            Some(&serde_json::json!(3))
        );
    }

    #[test]
    fn a_state_naming_no_preset_reads_back_as_none() {
        assert_eq!(stored_preset(&PluginState::empty()), None);

        // And so does one holding something that is not a preset at all, which is what a file
        // written by a future version looks like from here.
        let mut state = PluginState::empty();
        state.extra = serde_json::json!({ "preset": "the loud one" });
        assert_eq!(stored_preset(&state), None);
    }

    #[test]
    fn parameters_and_preset_both_come_back_from_a_saved_state() {
        let mut before = sampler();
        before.set_param_by_key("level", -6.0);
        before.preset = Some(preset());
        let state = before.save_state();

        let mut after = sampler();
        after.load_state(&state);
        assert_eq!(after.param_by_key("level"), Some(-6.0));
        assert_eq!(after.preset(), Some(preset()));
    }

    #[test]
    fn a_sampler_with_no_font_renders_silence_rather_than_refusing_to_run() {
        // Every path here has to survive an empty bank: a project can name a font whose file
        // has moved, and that must cost one track's sound, not the session.
        let mut sampler = sampler();
        sampler.prepare(&PrepareContext::new(48_000.0, 512, 2));
        assert!(!sampler.has_voice());

        let mut out = AudioBuffer::stereo(512, 48_000.0);
        for sample in out.channel_mut(0) {
            *sample = 0.5;
        }
        let events = [
            NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 1.0,
            },
            NoteEvent::NoteOff {
                frame: 200,
                pitch: 60,
            },
        ];
        let ctx = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
        sampler.process(&events, &mut out, &ctx);

        assert!(
            out.channel(0).iter().all(|s| *s == 0.0),
            "a silent sampler must still overwrite the buffer it was handed"
        );
        assert_eq!(sampler.active_voices(), 0);
        sampler.reset();
    }

    #[test]
    fn the_descriptor_says_what_a_project_file_will_store() {
        let sampler = sampler();
        let descriptor = sampler.descriptor();
        assert_eq!(descriptor.id, SAMPLER_ID);
        assert_eq!(descriptor.category, PluginCategory::Sampler);
        assert_eq!(sampler.parameters().len(), PARAM_COUNT);
        for (index, param) in sampler.parameters().iter().enumerate() {
            assert_eq!(
                param.id.index(),
                index,
                "descriptor {index} is out of place"
            );
        }
    }

    #[test]
    fn the_sampler_registers_itself_under_the_id_projects_store() {
        let mut registry = PluginRegistry::new();
        register_sampler(&mut registry, SoundFontBank::shared());
        assert!(registry.has_instrument(SAMPLER_ID));
    }

    // ------------------------------------------------------------ playing one

    const RATE: f64 = 48_000.0;
    const FONT: SoundFontId = SoundFontId(1);

    /// A bank holding the two-tone test font under [`FONT`].
    fn stocked() -> crate::bank::SharedSoundFonts {
        let bank = SoundFontBank::shared();
        bank.insert(FONT, crate::test_support::two_tone_font(RATE as i32));
        bank
    }

    /// A prepared sampler playing `patch` of the test font.
    fn playing(bank: crate::bank::SharedSoundFonts, patch: i32, frames: usize) -> Sampler {
        let mut sampler = Sampler::new(bank);
        sampler.preset = Some(PresetRef {
            font: FONT,
            bank: 0,
            patch,
        });
        sampler.prepare(&PrepareContext::new(RATE, frames, 2));
        assert!(sampler.has_voice(), "the font is in the bank");
        sampler
    }

    fn note_on(frame: u32) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch: crate::test_support::ROOT_KEY,
            velocity: 1.0,
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    #[test]
    fn a_note_turns_into_audio() {
        // The one test here that can fail for a real reason: everything else is arithmetic on
        // state, and this is the path that runs through the file format, the preset lookup and
        // the synthesiser.
        let mut sampler = playing(stocked(), 0, 512);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(200)], &mut out, &ctx);

        assert_eq!(
            rms(&out.channel(0)[..200]),
            0.0,
            "the sound started before the note did"
        );
        assert!(
            rms(&out.channel(0)[200..]) > 0.01,
            "the note made no sound at all"
        );
        assert_eq!(sampler.active_voices(), 1);
    }

    #[test]
    fn each_preset_plays_its_own_sound() {
        // Without this, a sampler that ignored bank and patch entirely and always played the
        // font's first preset would pass every other test in this file.
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let measure = |patch: i32| {
            let mut sampler = playing(bank.clone(), patch, frames);
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            // Past the attack, so both are measured at their sustained level.
            rms(&out.channel(0)[frames / 2..])
        };

        // The two tones are recorded four to one apart.
        let ratio = measure(0) / measure(42);
        assert!(
            (ratio - 4.0).abs() < 0.4,
            "the two presets should differ by four to one, not {ratio}"
        );
    }

    #[test]
    fn the_block_size_does_not_change_what_is_heard() {
        // A note lands on the frame it names, so splitting the same span into two calls has to
        // produce the same samples. This is what the event-splitting loop in `process` is for.
        let bank = stocked();
        let ctx_full = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let ctx_half = ProcessContext::realtime(RATE, 256, 0, 120.0, true);

        let mut whole = playing(bank.clone(), 0, 512);
        let mut one_go = AudioBuffer::stereo(512, RATE);
        whole.process(&[note_on(300)], &mut one_go, &ctx_full);

        let mut split = playing(bank, 0, 512);
        let mut first = AudioBuffer::stereo(256, RATE);
        let mut second = AudioBuffer::stereo(256, RATE);
        split.process(&[], &mut first, &ctx_half);
        // Frame 300 of the whole block is frame 44 of the second half.
        split.process(&[note_on(44)], &mut second, &ctx_half);

        assert_eq!(&one_go.channel(0)[..256], first.channel(0));
        assert_eq!(&one_go.channel(0)[256..], second.channel(0));
    }

    #[test]
    fn a_released_note_stops() {
        let mut sampler = playing(stocked(), 0, 512);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let mut out = AudioBuffer::stereo(512, RATE);

        sampler.process(&[note_on(0)], &mut out, &ctx);
        let sounding = rms(out.channel(0));
        assert!(sounding > 0.01);

        sampler.process(
            &[NoteEvent::NoteOff {
                frame: 0,
                pitch: crate::test_support::ROOT_KEY,
            }],
            &mut out,
            &ctx,
        );
        assert_eq!(sampler.active_voices(), 0);

        // The release is short but not instant, so let a few blocks go by.
        for _ in 0..8 {
            sampler.process(&[], &mut out, &ctx);
        }
        assert!(
            rms(out.channel(0)) < sounding / 100.0,
            "the note went on sounding after it was released"
        );
    }

    #[test]
    fn an_event_on_the_last_frame_is_still_heard() {
        // There is nothing left to render at that point, but a note-off there is what stops the
        // note in the block after this one — dropping it would leave it sounding forever.
        let mut sampler = playing(stocked(), 0, 512);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let mut out = AudioBuffer::stereo(512, RATE);

        sampler.process(&[note_on(511)], &mut out, &ctx);
        assert_eq!(sampler.active_voices(), 1);
        sampler.process(&[], &mut out, &ctx);
        assert!(rms(out.channel(0)) > 0.01);
    }

    #[test]
    fn a_sampler_that_was_told_nothing_plays_the_first_font_loaded() {
        // Dropping the sampler onto a track and hearing nothing at all would read as broken, so
        // an unchosen preset takes the first font rather than silence.
        let mut sampler = Sampler::new(stocked());
        assert_eq!(sampler.preset(), None);
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        assert!(sampler.has_voice());

        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert!(rms(out.channel(0)) > 0.01);
    }

    #[test]
    fn a_font_the_bank_does_not_hold_stays_silent_rather_than_playing_another() {
        // The file has moved. Quietly playing some other font's piano would hide that, and the
        // project would come back subtly wrong instead of obviously incomplete.
        let bank = stocked();
        let mut sampler = Sampler::new(bank);
        sampler.preset = Some(PresetRef {
            font: SoundFontId(99),
            bank: 0,
            patch: 0,
        });
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        assert!(!sampler.has_voice());

        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert_eq!(rms(out.channel(0)), 0.0);
    }

    #[test]
    fn a_font_that_breaks_the_synthesiser_costs_its_sound_and_not_the_process() {
        // The library walks a looping voice by folding the position back one loop length per
        // frame; this font's one-frame loop at a preposterous claimed rate outruns the fold,
        // runs off the end of the sample data and hits an unchecked index. Uncontained, that
        // panic unwinds into the C audio callback and aborts the whole application.
        let bank = SoundFontBank::shared();
        bank.insert(FONT, crate::test_support::runaway_font(RATE as i32 * 512));
        let mut sampler = playing(bank, 0, 512);

        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let mut out = AudioBuffer::stereo(512, RATE);
        for sample in out.channel_mut(0) {
            *sample = 0.5;
        }
        sampler.process(&[note_on(0)], &mut out, &ctx);

        assert!(
            sampler.poisoned,
            "the runaway loop should have taken the synthesiser down"
        );
        assert!(
            out.channel(0).iter().all(|s| *s == 0.0),
            "a poisoned block must come back as silence, not as what was in the buffer"
        );
        assert_eq!(sampler.active_voices(), 0);

        // Every later block is silence rather than another attempt, and a reset must not call
        // back into whatever state the panic left behind.
        sampler.reset();
        let mut later = AudioBuffer::stereo(512, RATE);
        for sample in later.channel_mut(0) {
            *sample = 0.5;
        }
        sampler.process(&[note_on(0)], &mut later, &ctx);
        assert!(later.channel(0).iter().all(|s| *s == 0.0));

        // `prepare` is the way back to life.
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        assert!(!sampler.poisoned);
        assert!(sampler.has_voice());
    }

    #[test]
    fn a_reset_never_goes_back_to_the_bank() {
        // `reset` runs on the audio callback thread — every transport stop and seek reaches it
        // — so it must play from what `prepare` resolved rather than take the bank's lock.
        // Observable from outside: a font that leaves the bank after `prepare` keeps sounding,
        // and keeps sounding as the sound the track chose rather than the library's default.
        let bank = stocked();
        let frames = 1_024;
        let mut sampler = playing(bank.clone(), 42, frames);
        bank.remove(FONT);

        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);
        let level = |sampler: &mut Sampler| {
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };
        let before = level(&mut sampler);
        sampler.reset();
        let after = level(&mut sampler);
        assert!(
            (before - after).abs() < before * 0.05,
            "the chosen sound should survive a reset with the bank emptied: {before} then {after}"
        );
    }

    #[test]
    fn a_reset_keeps_playing_the_sound_the_track_chose() {
        // Resetting the synthesiser resets its channels too, which would otherwise silently
        // return the track to bank 0 patch 0 — a different instrument, from the same font.
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let level = |sampler: &mut Sampler| {
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };

        let mut sampler = playing(bank, 42, frames);
        let before = level(&mut sampler);
        sampler.reset();
        let after = level(&mut sampler);
        assert!(
            (before - after).abs() < before * 0.05,
            "the quiet preset came back at a different level: {before} then {after}"
        );
    }

    #[test]
    fn a_mono_host_gets_the_two_channels_folded_together() {
        let mut sampler = Sampler::new(stocked());
        sampler.preset = Some(PresetRef {
            font: FONT,
            bank: 0,
            patch: 0,
        });
        sampler.prepare(&PrepareContext::new(RATE, 512, 1));

        let mut out = AudioBuffer::new(1, 512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert!(rms(out.channel(0)) > 0.01);
    }

    #[test]
    fn nothing_on_the_audio_path_allocates() {
        // The realtime contract, as a test rather than as a sentence in a doc comment. It covers
        // `process`, `set_param` and `reset`, which are the three things the audio thread calls
        // — `reset` is how this file once ended up taking a lock and logging on the callback.
        let mut sampler = playing(stocked(), 0, 512);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let events = [
            note_on(0),
            NoteEvent::NoteOff {
                frame: 128,
                pitch: crate::test_support::ROOT_KEY,
            },
            NoteEvent::PitchBend {
                frame: 200,
                semitones: 2.0,
            },
            NoteEvent::AllNotesOff { frame: 400 },
        ];
        // Warm up outside the watched region, so first-touch growth is not counted.
        sampler.process(&events, &mut out, &ctx);

        let allocations = crate::test_support::count_allocations(|| {
            for _ in 0..64 {
                sampler.process(&events, &mut out, &ctx);
                sampler.set_param_by_key("level", -3.0);
                sampler.reset();
            }
        });
        assert_eq!(allocations, 0, "the sampler allocated while rendering");
    }

    // ------------------------------------------------------------ shaping the font

    fn defaults() -> [f32; PARAM_COUNT] {
        let mut values = [0.0; PARAM_COUNT];
        for (slot, descriptor) in values.iter_mut().zip(&sampler().params) {
            *slot = descriptor.default;
        }
        values
    }

    #[test]
    fn the_sampler_starts_with_the_envelope_switched_out() {
        // The whole design rests on this: the shaped path trades a drum kit's choke groups and 113
        // voices of polyphony for the fade, and nobody who has not asked for it should pay that.
        assert!(!shaping(&defaults()));
    }

    #[test]
    fn nothing_but_the_switch_switches_the_envelope_on() {
        // The reason this is a test and not an implementation detail: a version of `shaping` that
        // read the four sliders would turn the mechanism on the moment somebody nudged one to see
        // what it did, and there would be no way to turn it off again with any confidence.
        for index in [ATTACK, DECAY, SUSTAIN, RELEASE] {
            let mut values = defaults();
            values[index] = if index == SUSTAIN { 0.0 } else { 2.0 };
            assert!(
                !shaping(&values),
                "parameter {index} turned the envelope on by itself"
            );
        }

        let mut values = defaults();
        values[ENVELOPE] = 1.0;
        assert!(shaping(&values));
    }

    #[test]
    fn the_switch_is_a_parameter_a_frontend_can_find_by_name() {
        // The warning strip in the plugin window is drawn from this, so a rename that only
        // happened here would silently stop it appearing.
        let sampler = sampler();
        let switch = sampler
            .parameters()
            .iter()
            .find(|param| param.key == ENVELOPE_KEY)
            .expect("the envelope switch");
        assert_eq!(switch.unit, ParamUnit::Toggle);
        assert_eq!(switch.default, 0.0);
    }

    /// Slots in a stated order of use, none of them holding a key.
    fn free_slots(used: &[u64]) -> Vec<Slot> {
        used.iter()
            .map(|used| Slot {
                envelope: Adsr::new(),
                key: None,
                used: *used,
                expression: FULL_EXPRESSION,
            })
            .collect()
    }

    #[test]
    fn a_new_note_takes_the_channel_that_has_been_idle_longest() {
        // Round the houses rather than back to the front: the font's own release tail is still
        // sounding on a channel after its note has gone, and every slot passed over before
        // coming back to it is a note's worth of grace for that tail.
        let slots = free_slots(&[7, 2, 9, 4]);
        assert_eq!(claim(&slots), 1);
    }

    #[test]
    fn a_note_that_finds_every_channel_held_displaces_the_quietest() {
        let mut slots = free_slots(&[0, 1, 2, 3]);
        for (index, slot) in slots.iter_mut().enumerate() {
            slot.key = Some(60 + index as u8);
            slot.envelope.trigger();
            // Level 1, 2, 3, 4 attack steps in: the first is the quietest.
            for _ in 0..=index {
                slot.envelope.process();
            }
        }
        assert_eq!(claim(&slots), 0);

        // And once one is let go it is taken instead, however loud it still is.
        slots[3].key = None;
        assert_eq!(claim(&slots), 3);
    }

    #[test]
    fn the_first_of_two_notes_at_one_pitch_is_the_first_let_go() {
        // A note-off names a pitch and not a note, so which of two sounding notes it ends is this
        // sampler's decision — and only one answer lets both sound for the length they were
        // written. Ending the newest leaves the older one for the *next* off, so the first note
        // plays over the second one's span and the second is a click; it used to do exactly that.
        //
        // It is also what `auris_synth::VoiceAllocator::note_off` answers for every built-in
        // voice, and two note-off policies in one workspace is a project that plays differently
        // depending on whether a part landed on a font.
        let mut sampler = sampler();
        sampler.slots = free_slots(&[3, 1, 2, 0]);
        // Two notes at one pitch — the older of them on slot 1 — and one at another, which must
        // not be touched by either off.
        for (slot, key) in [(0usize, 62u8), (1, 60), (2, 60)] {
            sampler.slots[slot].key = Some(key);
            sampler.slots[slot].envelope.trigger();
        }

        sampler.let_go(60);
        assert!(
            sampler.slots[1].envelope.is_releasing(),
            "the note that had been sounding longest was left holding"
        );
        assert!(
            !sampler.slots[2].envelope.is_releasing(),
            "the note that had just begun was the one ended"
        );
        assert!(
            !sampler.slots[0].envelope.is_releasing(),
            "another pitch is another note"
        );

        // And the second off takes the one that is left, so neither is stranded.
        sampler.let_go(60);
        assert!(sampler.slots[2].envelope.is_releasing());
        assert!(!sampler.slots[0].envelope.is_releasing());
    }

    #[test]
    fn an_attack_fades_a_note_the_font_starts_at_once() {
        // The test that would fail if the envelope were computed and never reached the audio:
        // expression is the only per-note gain the library exposes, and this is the proof that
        // writing it does something.
        let bank = stocked();
        let opening = |attack: f32| {
            let mut sampler = playing(bank.clone(), 0, 512);
            sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
            sampler.set_param_by_key("attack", attack);
            let mut out = AudioBuffer::stereo(512, RATE);
            let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            (rms(&out.channel(0)[..64]), rms(&out.channel(0)[448..]))
        };

        // The font has an opening of its own — two milliseconds of delay and attack, which is
        // what the defaults in a SoundFont's envelope generators come to — so a switched-on
        // envelope with no attack is the reference rather than a flat line.
        let (flat_early, flat_late) = opening(0.0);
        let (slow_early, slow_late) = opening(0.2);
        assert!(
            slow_early < flat_early / 5.0,
            "the attack did not fade the note in: {slow_early} against the font's own {flat_early}"
        );
        assert!(
            slow_late < flat_late / 2.0,
            "a fifth of a second in, the note should still be climbing, not at {slow_late}"
        );
    }

    #[test]
    fn a_release_holds_a_note_the_font_would_have_dropped() {
        // The font's own release is a millisecond, so anything still sounding thirty
        // milliseconds after the key is let go is the envelope's doing and nothing else.
        let bank = stocked();
        let ringing = |release: f32| {
            let mut sampler = playing(bank.clone(), 0, 512);
            sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
            sampler.set_param_by_key("release", release);
            let mut out = AudioBuffer::stereo(512, RATE);
            let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            let sounding = rms(out.channel(0));
            sampler.process(
                &[NoteEvent::NoteOff {
                    frame: 0,
                    pitch: crate::test_support::ROOT_KEY,
                }],
                &mut out,
                &ctx,
            );
            sampler.process(&[], &mut out, &ctx);
            (sounding, rms(out.channel(0)))
        };

        let (sounding, after) = ringing(0.0);
        assert!(sounding > 0.01, "the reference note made no sound");
        assert!(
            after < sounding / 100.0,
            "with no release the font should have dropped the note already, not {after}"
        );

        let (sounding, after) = ringing(0.3);
        assert!(
            after > sounding / 2.0,
            "the release let the note go early: {after} against {sounding}"
        );
    }

    #[test]
    fn all_notes_off_waits_for_the_shaped_release_before_telling_the_font() {
        let mut sampler = playing(stocked(), 0, 512);
        sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
        sampler.set_param_by_key("release", 0.3);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        let sounding = rms(out.channel(0));

        sampler.process(&[NoteEvent::AllNotesOff { frame: 0 }], &mut out, &ctx);
        sampler.process(&[], &mut out, &ctx);

        assert!(
            rms(out.channel(0)) > sounding / 2.0,
            "the SoundFont release cut off the sampler envelope"
        );
    }

    #[test]
    fn turning_the_envelope_back_off_does_not_leave_the_sampler_quiet() {
        // The channels carry the fade, so a sampler switched off halfway through one would
        // otherwise go on playing at whatever gain that fade had reached. This is the whole
        // reason the switch has to be able to undo itself rather than only stop being applied.
        let bank = stocked();
        let mut sampler = playing(bank, 0, 512);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);

        sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
        sampler.set_param_by_key("attack", 2.0);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        let fading = rms(out.channel(0));

        sampler.set_param_by_key(ENVELOPE_KEY, 0.0);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert!(
            rms(out.channel(0)) > fading * 10.0,
            "the sampler stayed under the fade it was told to stop applying"
        );
    }

    #[test]
    fn a_held_unshaped_note_is_not_faded_by_a_later_shaped_one() {
        // An unshaped note plays on `CHANNEL` without ever entering the slot table. Flip the
        // envelope on while it is held and strike another note: the new note's fade has to land
        // on a slot channel of its own. Slot 0 used to share channel 0, look free to `claim`,
        // and write the new note's envelope — and eventually its note-off — over the held
        // note's loudness.
        let bank = stocked();
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let held_beside_a_shaped_note = |with_shaped: bool| {
            let mut sampler = playing(bank.clone(), 0, 512);
            let mut out = AudioBuffer::stereo(512, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);

            sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
            let strike: Vec<NoteEvent> = with_shaped.then(|| note_on(0)).into_iter().collect();
            sampler.process(&strike, &mut out, &ctx);
            let off: Vec<NoteEvent> = with_shaped
                .then_some(NoteEvent::NoteOff {
                    frame: 0,
                    pitch: crate::test_support::ROOT_KEY,
                })
                .into_iter()
                .collect();
            sampler.process(&off, &mut out, &ctx);

            // Past the shaped note's release, so only the held note is left to measure. The
            // block count is the same on both arms — the tone is compared at the same age.
            for _ in 0..24 {
                sampler.process(&[], &mut out, &ctx);
            }
            rms(out.channel(0))
        };

        let alone = held_beside_a_shaped_note(false);
        let beside = held_beside_a_shaped_note(true);
        assert!(alone > 0.005, "the held note fell silent on its own");
        let ratio = beside / alone;
        assert!(
            (ratio - 1.0).abs() < 0.25,
            "the shaped note dragged the held one to {ratio} of its level"
        );
    }

    #[test]
    fn a_shaped_note_at_full_level_is_as_loud_as_an_unshaped_one() {
        // The envelope is scaled into the expression the library's own reset leaves behind, so
        // that reaching for a control never quietly moves the whole track. Everything measured
        // for `NOMINAL_VOLUME` was measured at that value.
        let bank = stocked();
        let sustained = |shape: bool| {
            let frames = 2_048;
            let mut sampler = playing(bank.clone(), 0, frames);
            if shape {
                // Shaping, but arriving at full level well inside the window measured below.
                sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
                sampler.set_param_by_key("attack", 0.001);
            }
            let mut out = AudioBuffer::stereo(frames, RATE);
            let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };

        let ratio = sustained(true) / sustained(false);
        assert!(
            (ratio - 1.0).abs() < 0.01,
            "shaping moved the level by a factor of {ratio}"
        );
    }

    #[test]
    fn the_shaped_path_does_not_allocate_either() {
        // `nothing_on_the_audio_path_allocates` covers the plain path; this is the same contract
        // for the one that steps fifteen envelopes and writes controllers while it renders.
        let mut sampler = playing(stocked(), 0, 512);
        sampler.set_param_by_key(ENVELOPE_KEY, 1.0);
        sampler.set_param_by_key("attack", 0.05);
        sampler.set_param_by_key("sustain", 0.6);
        sampler.set_param_by_key("release", 0.4);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let events = [
            note_on(0),
            NoteEvent::NoteOff {
                frame: 128,
                pitch: crate::test_support::ROOT_KEY,
            },
            NoteEvent::PitchBend {
                frame: 200,
                semitones: 2.0,
            },
            NoteEvent::AllNotesOff { frame: 400 },
        ];
        sampler.process(&events, &mut out, &ctx);

        let allocations = crate::test_support::count_allocations(|| {
            for _ in 0..64 {
                sampler.process(&events, &mut out, &ctx);
                sampler.set_param_by_key("release", 0.3);
                sampler.reset();
            }
        });
        assert_eq!(allocations, 0, "the sampler allocated while shaping");
    }

    // ------------------------------------------------------------ what a velocity means

    /// The sustained level of one note at `velocity`, on the test font's loud preset.
    ///
    /// Measured past the halfway point so the attack is behind it and the tone is steady, which
    /// is the only part of the note whose level is a property of the velocity rather than of the
    /// envelope.
    fn level_at(bank: &crate::bank::SharedSoundFonts, velocity: f32) -> f32 {
        let frames = 1_024;
        let mut sampler = playing(bank.clone(), 0, frames);
        let mut out = AudioBuffer::stereo(frames, RATE);
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);
        sampler.process(
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: crate::test_support::ROOT_KEY,
                velocity,
            }],
            &mut out,
            &ctx,
        );
        rms(&out.channel(0)[frames / 2..])
    }

    #[test]
    fn a_velocity_is_linear_in_amplitude() {
        // The test that would have caught the whole thing. A SoundFont synthesiser squares the
        // velocity it is given, so without the correction in `midi_velocity` this measures the
        // square of every ratio below — 0.25 where it wants 0.5 — and a part written for a
        // built-in instrument comes out with twice the dynamic range through the sampler.
        //
        // The tolerance is the quantisation, not slop. The synthesiser takes an integer, so the
        // best any correction can do is `40 * log10(1 + 0.5 / midi)` decibels: 0.22 dB at
        // velocity 0.1, less above it. 0.5 dB leaves room for that and for the tone's own
        // measurement noise without leaving room for the square law, which is 6 dB out at 0.5.
        let bank = stocked();
        let full = level_at(&bank, 1.0);
        assert!(full > 0.01, "the reference note made no sound");

        for velocity in [0.9f32, 0.75, 0.5, 0.35, 0.2, 0.1] {
            let measured = level_at(&bank, velocity) / full;
            let error = 20.0 * (measured / velocity).log10();
            assert!(
                error.abs() < 0.5,
                "velocity {velocity} should scale the amplitude by {velocity}, \
                 not by {measured} — {error:.2} dB out"
            );
        }
    }

    #[test]
    fn the_sampler_answers_a_velocity_the_way_a_built_in_instrument_does() {
        // `auris-sampler` and `auris-synth` are siblings and neither may depend on the other, so
        // this states the built-in law rather than importing it: `VoiceAllocator::note_on` sets
        // `slot.level = velocity` and the level multiplies the voice, so a built-in part at 0.56
        // measures 20*log10(0.56) = -5.04 dB. The number below is that law written out, and the
        // assertion is that the sampler answers the same velocity with the same relative level.
        //
        // The two crates meeting for real wants a test in `auris-session`, which is the layer
        // that owns both through the registry; this one cannot see far enough to be that test,
        // only far enough to fail when the sampler is the half that drifts.
        let bank = stocked();
        let full = level_at(&bank, 1.0);

        let velocity = 0.56f32;
        let built_in_db = 20.0 * velocity.log10();
        let measured_db = 20.0 * (level_at(&bank, velocity) / full).log10();
        assert!(
            (measured_db - built_in_db).abs() < 0.5,
            "a built-in instrument answers velocity {velocity} with {built_in_db:.2} dB \
             and the sampler answered {measured_db:.2} dB"
        );
    }

    #[test]
    fn a_written_part_keeps_the_dynamic_range_it_was_written_with() {
        // The complaint, as a number. A composed part spans MIDI 26 to 126, which a built-in
        // instrument renders as 20*log10(126/26) = 13.7 dB. Through the squared law the same
        // part measured 27.4 dB, and "the velocity variation is severe" is what that sounds like.
        let bank = stocked();
        let quiet = level_at(&bank, 26.0 / 127.0);
        let loud = level_at(&bank, 126.0 / 127.0);
        let span = 20.0 * (loud / quiet).log10();
        let want = 20.0 * (126.0f32 / 26.0).log10();
        assert!(
            (span - want).abs() < 0.5,
            "the part should span {want:.2} dB as it does on a built-in instrument, not {span:.2}"
        );
    }

    #[test]
    fn the_quietest_note_still_sounds_rather_than_releasing_itself() {
        // Velocity 0 is a note-off in MIDI, so the floor of 1 is what keeps a note written
        // silently from cancelling itself. Worth its own case because the square root moves
        // every other velocity and could have taken the floor with it.
        assert_eq!(midi_velocity(0.0), 1);
        assert_eq!(midi_velocity(-1.0), 1);
        assert_eq!(midi_velocity(1.0), 127);
        assert_eq!(midi_velocity(2.0), 127);
        // And the correction is what it claims: the square of the fraction handed over is the
        // velocity asked for, which is the whole of the arithmetic.
        for velocity in [0.1f32, 0.25, 0.5, 0.75] {
            let fraction = midi_velocity(velocity) as f32 / 127.0;
            let error = 20.0 * (fraction * fraction / velocity).log10();
            assert!(
                error.abs() < 0.5,
                "velocity {velocity} became MIDI {}, which squares to {}",
                midi_velocity(velocity),
                fraction * fraction
            );
        }
    }

    #[test]
    fn automation_cannot_overwrite_the_samplers_channel_setup() {
        for number in [0, 6, 11, 43, 100, 101] {
            assert!(is_reserved(number), "controller {number}");
        }
        assert!(!is_reserved(1), "the modulation wheel remains available");
    }

    #[test]
    fn nominal_is_four_times_what_the_library_calls_unity() {
        // What the doc comment on `NOMINAL_VOLUME` now promises, measured against the thing it
        // is a multiple of rather than asserted against itself: the library's own master volume
        // default is 0.5, and 0 dB on the `level` parameter has to come out four times that.
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let mut sampler = playing(bank, 0, frames);
        let mut out = AudioBuffer::stereo(frames, RATE);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        let ours = rms(&out.channel(0)[frames / 2..]);

        // The same note through a synthesiser left at the library's defaults.
        let font = crate::test_support::two_tone_font(RATE as i32);
        let mut settings = SynthesizerSettings::new(RATE as i32);
        settings.block_size = INTERNAL_BLOCK;
        settings.maximum_polyphony = MAX_VOICES;
        settings.enable_reverb_and_chorus = false;
        let mut theirs = Synthesizer::new(&font, &settings).expect("the font this crate writes");
        let library_unity = theirs.get_master_volume();
        theirs.note_on(CHANNEL, crate::test_support::ROOT_KEY as i32, 127);
        let mut left = vec![0.0; frames];
        let mut right = vec![0.0; frames];
        theirs.render(&mut left, &mut right);
        let reference = rms(&left[frames / 2..]);

        assert_eq!(library_unity, 0.5, "the library moved its own unity");
        let ratio = ours / reference;
        assert!(
            (ratio - 4.0).abs() < 0.05,
            "0 dB should be four times the library's unity, not {ratio}"
        );
        assert_eq!(NOMINAL_VOLUME, 4.0 * library_unity);
    }

    #[test]
    fn the_level_parameter_moves_the_output() {
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let measure = |level: f32| {
            let mut sampler = playing(bank.clone(), 0, frames);
            sampler.set_param_by_key("level", level);
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };

        // Six decibels down is half the amplitude.
        let ratio = measure(0.0) / measure(-6.0206);
        assert!(
            (ratio - 2.0).abs() < 0.1,
            "-6 dB should halve the output, not scale it by {ratio}"
        );
    }
}
