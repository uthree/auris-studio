//! The editing session: one document, one engine, one command per user action.
//!
//! [`Session`] owns the document, the plugin registry, the audio engine and the undo history, and
//! exposes one method per user-level command. Every frontend drives the application through this
//! module and through nothing else.
//!
//! # Where things are
//!
//! Here: [`Session`] itself, everything it hands back — [`SessionOptions`], [`AudioStatus`],
//! [`SaveReport`], [`MidiReport`], [`ComposeReport`] — and the spine the rest of the module leans
//! on. The spine is four groups and nothing more: opening a session, reading the document,
//! recording an undo step, and rebuilding the render graph. Nearly every command in the files
//! beside this one ends by calling into two of them, which is what keeps the document, the
//! history and the audio thread in step without each command remembering to.
//!
//! The rest is a file per thing a user could ask for. `transport` runs the clock and moves the
//! tempo and the meter under it; `harmony` is the key, the chords, the sections, and hearing any
//! of them; `tracks` is what plays and where its audio goes; `clips` is what sits on a track and
//! `generated` is the half of those that write themselves; `notes` is what is inside one;
//! `mixer` is the strip, its parameters and the lanes that drive them; `files` is everything that
//! reaches the disk and `assets` is how a saved document finds the files it only names;
//! `compose` replaces the whole document with a written piece and `accompany` writes parts around
//! one clip of the document there already; `clipboard` is cut, copy and paste over both of the
//! things a user can select. `record` and `monitor` are the two halves of the input device — what
//! is kept, and what is merely heard — and share it rather than each opening one. `typing` is the
//! computer keyboard played as an instrument, for a desk with no MIDI keyboard on it.
//!
//! Every one of them is `impl Session`, so no path a caller writes changes. And because they are
//! *children* of this module rather than neighbours of it, they read [`Session`]'s private fields
//! as they always did — the split opened up nothing except a handful of helpers that two files
//! now share.

mod accompany;
mod analysis;
mod assets;
mod autosave;
mod clipboard;
mod clips;
mod compose;
mod files;
mod generated;
mod harmony;
mod hosted;
mod levels;
mod lyrics;
mod mixer;
mod monitor;
mod notes;
mod perform;
mod punch;
mod record;
mod singer;
mod tracks;
mod transport;
mod typing;
mod vst3;

#[cfg(test)]
mod fixtures;

pub use accompany::{AccompanyReport, DEFAULT_PARTS};
pub use analysis::{MixAnalysis, SectionLoudness, TrackLoudness};
pub use autosave::{AUTOSAVE_INTERVAL, AutosaveState, should_autosave};
pub use clipboard::{Clipboard, CopiedClip, CopiedContent};
pub use compose::{composed_gain_db, kit_trim_db};
pub use files::{LoadedFont, decode_audio, read_soundfont};
pub use hosted::PluginWindow;
pub use levels::{
    BalanceReport, CEILING_DB, LIMITER_ALLOWANCE_DB, TARGET_LUFS, TrackLevel, fader_for,
    faders_lift_db, master_gain_db,
};
pub use lyrics::{DEFAULT_LYRIC_PROGRESSION, LyricSongReport, LyricsMeasure};
pub use monitor::MonitorStatus;
pub use notes::{Quantize, quantized};
pub use singer::{
    LYRIC_CONTINUATION, MIN_PHONEME_SECONDS, PREVIEW_NOTE_SECONDS, SingPlan, SingerTakeState,
    SungFrames, take_fingerprint,
};

pub use record::{
    Arm, InputChannels, RecordingReport, RecordingStatus, TakeReport, input_level_of,
};
pub use tracks::{MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT};
pub use typing::{
    DEFAULT_OCTAVE, DEFAULT_VELOCITY, LAYOUT, MusicalTyping, OCTAVE_RANGE, Played, Release, Struck,
    TYPING_BEND, TypingRole, VELOCITY_STEP, WHEEL_STEPS, shadows_musical_typing,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auris_core::param::ParamDescriptor;
use auris_core::time::{Seconds, Ticks};
use auris_core::{AudioSourceBank, PluginRegistry, Project, SourceId, TrackId};
use auris_engine::{
    AudioDevice, AudioDeviceInfo, AudioSettings, EngineCommand, EngineHandle, MeterBank,
    RenderGraph, start_audio,
};
use auris_gpu::{GpuContext, WaveformPeaks};
use auris_io::SoundFont;
use auris_sampler::{SharedSoundFonts, SoundFontBank};

use crate::error::SessionError;
use crate::history::{Edit, History};
use crate::registry::default_registry;
use crate::render::{RenderJob, bank_at_rate, fill_stretches};
use crate::settings::AudioPreferences;

/// How to start a session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionOptions {
    /// Open an audio output device. `false` gives a silent engine that still accepts commands,
    /// which is what a CLI or a test wants.
    pub audio: bool,
    /// Try to use the GPU for waveform and loudness analysis.
    pub gpu: bool,
    /// Which device to open and how, loaded from the user's settings.
    pub audio_preferences: AudioPreferences,
    /// Sample rate for a new project.
    pub sample_rate: f64,
    /// Read the SoundFonts the application ships with, so their sounds are in the library.
    ///
    /// `false` in a test, and for one reason: whether the library is installed is a fact about
    /// the machine, and a document that holds a font on a developer's laptop and none on a CI
    /// runner is a document two test runs would disagree about. It also saves reading two hundred
    /// megabytes per session in a suite that opens hundreds of them.
    pub shipped_fonts: bool,
    /// Set a composed piece's levels by rendering it and measuring what came out.
    ///
    /// On, because a level the composer guessed is a guess about an instrument nobody had heard
    /// yet — see [`Session::balance_levels`]. Off is for a caller that wants the numbers the
    /// composer wrote and nothing else: the tests that are *about* those numbers, and anything
    /// that cannot afford a render per part.
    pub balance_composed: bool,
    /// Write the document back over itself as it changes, once it has somewhere to be written.
    ///
    /// On by default, and off for a headless session: a batch tool holds a document for a few
    /// hundred milliseconds and saves it once at the end on purpose, and a background write in
    /// the middle of that would be a file appearing that nobody asked for. See
    /// [`should_autosave`] for what the feature costs when it is on.
    pub autosave: bool,
    /// Load the Japanese dictionary the application ships with, when one is installed.
    ///
    /// `false` in a test for [`Self::shipped_fonts`]'s reason: whether the dictionary is
    /// installed is a fact about the machine, and a suite whose accent analysis appears and
    /// disappears with it would disagree with itself between two runners. An explicit folder
    /// handed to [`Session::set_japanese_dictionary`](crate::Session::set_japanese_dictionary)
    /// always wins over the shipped one.
    pub shipped_dictionary: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            audio: true,
            gpu: true,
            audio_preferences: AudioPreferences::default(),
            sample_rate: 48_000.0,
            shipped_fonts: true,
            balance_composed: true,
            autosave: true,
            shipped_dictionary: true,
        }
    }
}

impl SessionOptions {
    /// No audio device, no GPU and no shipped library — for tests and for anything that wants a
    /// session which behaves identically on every machine.
    ///
    /// A headless tool that is making *music* rather than checking a document — `auris compose`
    /// is the one — wants the library back, and asks for it with [`Self::with_shipped_fonts`].
    pub fn headless() -> Self {
        Self {
            audio: false,
            gpu: false,
            shipped_fonts: false,
            autosave: false,
            shipped_dictionary: false,
            ..Self::default()
        }
    }

    /// Whether to read the SoundFonts the application ships with.
    pub fn with_shipped_fonts(mut self, shipped_fonts: bool) -> Self {
        self.shipped_fonts = shipped_fonts;
        self
    }

    /// Whether to load the Japanese dictionary the application ships with.
    pub fn with_shipped_dictionary(mut self, shipped_dictionary: bool) -> Self {
        self.shipped_dictionary = shipped_dictionary;
        self
    }

    /// Whether composing ends by measuring the piece and setting its levels from what it heard.
    pub fn with_balance(mut self, balance_composed: bool) -> Self {
        self.balance_composed = balance_composed;
        self
    }

    /// Sets the sample rate a new project is created at.
    pub fn with_sample_rate(mut self, sample_rate: f64) -> Self {
        self.sample_rate = sample_rate;
        self
    }
}

/// What the audio backend ended up doing.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioStatus {
    /// Device name, or a placeholder when running silently.
    pub device: String,
    /// `true` when a real output stream is open.
    pub running: bool,
    /// Rate the engine renders at, which is the device's rate and not necessarily the project's.
    pub sample_rate: f64,
    /// Output channel count.
    pub channels: usize,
    /// Name of the GPU adapter in use, when there is one.
    pub gpu: Option<String>,
}

/// An open editing session.
///
/// Mutators keep the document, the undo history and the audio thread in step. Structural
/// changes rebuild the render graph; cheap ones send a command instead. Wrap a burst of related
/// edits — a pointer drag, a scripted batch — in [`Session::begin_transaction`] /
/// [`Session::end_transaction`] so they become one undo step and one rebuild.
pub struct Session {
    project: Project,
    /// Decoded audio at the project's own rate, which is what the document and the waveform
    /// drawing are expressed in.
    bank: AudioSourceBank,
    /// The same audio at the rate the render graph is built for.
    ///
    /// The engine renders at whatever rate the output device runs at, and a device is free to
    /// disagree with the project — 44.1 kHz hardware under a 48 kHz project is ordinary. The
    /// graph reads a clip's samples one for one against the timeline, so it needs the sources at
    /// its own rate or every audio clip plays at the wrong speed. Converting once, when the rate
    /// changes, is what keeps that off the rebuild path: resampling a long file is expensive and
    /// a rebuild happens on every structural edit.
    ///
    /// When the two rates agree — the usual case — this shares the same buffers and costs
    /// nothing but the map holding them.
    render_bank: AudioSourceBank,
    /// Rate [`Self::render_bank`] currently holds.
    render_bank_rate: f64,
    /// The SoundFonts behind [`Project::soundfonts`], for the same reason [`Self::bank`] exists:
    /// the document keeps paths, the samples live beside it.
    ///
    /// Shared with the registry, whose sampler factory captured it — which is the only way sample
    /// data reaches an instrument the registry builds.
    fonts: SharedSoundFonts,
    /// The fonts that came with the application, kept by the path they were read from.
    ///
    /// [`Self::fonts`] is emptied whenever the document is replaced, because it is keyed by ids
    /// that belong to a document. These are not: the same file is the same samples whichever
    /// project is open, and re-reading two hundred megabytes on every **File → New** is a stall
    /// nobody would understand.
    shipped: HashMap<PathBuf, Arc<SoundFont>>,
    /// Whether this session reads the shipped library at all — see
    /// [`SessionOptions::shipped_fonts`].
    shipped_library: bool,
    /// Whether composing ends by measuring the piece — see [`SessionOptions::balance_composed`].
    balance_composed: bool,
    registry: Arc<PluginRegistry>,
    device: Option<AudioDevice>,
    engine: EngineHandle,
    gpu: Option<Arc<GpuContext>>,
    /// What the audio backend was asked for, so a settings panel can show it back.
    audio: AudioPreferences,
    /// Whether this session must never claim a real device, however it is reconfigured.
    headless: bool,

    history: History,
    transaction: Option<Transaction>,
    /// Counts every change to the document: edits, undo, redo, another document entirely.
    ///
    /// For frontends that cache something derived from the document — a repaint loop that asked
    /// an expensive question thirty times a second would otherwise have no cheap way to know the
    /// old answer still stands. Monotonic within a session, meaningless across two.
    revision: u64,
    needs_rebuild: bool,
    /// Whether the meter has moved since the render graph was built.
    ///
    /// The engine holds a copy of the signature map for one purpose — accenting the metronome's
    /// bar lines — and a meter change is otherwise none of its business. So a change made while
    /// the click is off is remembered here instead of costing a rebuild nobody would hear, and
    /// paid for the moment the click is switched on. See [`Session::set_metronome`].
    meter_is_stale: bool,
    /// The last edit recorded outside a transaction, and when, for [`Session::record_repeating`].
    last_record: Option<(Edit, Instant)>,

    path: Option<PathBuf>,
    dirty: bool,
    /// The exact document state most recently read from or written to disk.
    saved_project: Project,
    /// Whether the document is written back over itself as it changes. See [`should_autosave`].
    autosave: bool,
    /// When the document was last written, by any means. The autosave clock runs from here.
    last_save: Instant,
    /// The file's modification time as of this session's last read or write of it — what
    /// [`Session::externally_modified`] compares against to notice another writer.
    disk_stamp: Option<std::time::SystemTime>,
    /// Hash of the exact file bytes at [`Self::disk_stamp`].
    disk_fingerprint: Option<u64>,

    /// Where a spectrum display reads its samples.
    ///
    /// Owned by the session and handed to each graph rather than created with one, because a
    /// rebuild happens on every structural edit and an open display must not be left reading a
    /// scope that nothing writes to any more.
    scope: Arc<auris_engine::Scope>,
    /// Turns the window the engine publishes into a spectrum.
    ///
    /// Here rather than in a frontend because a frontend may not name `auris-dsp` — and because
    /// two of them wanting a spectrum should not each grow their own FFT.
    analyzer: auris_dsp::SpectrumAnalyzer,

    param_cache: HashMap<String, Arc<Vec<ParamDescriptor>>>,
    /// Which built-in effects listen to a key, by plugin id.
    ///
    /// The only way to ask is to build one, and the frontend asks while it is drawing — every
    /// frame, for every slot on screen. The answer cannot change for a given id, so it is worth
    /// exactly one instantiation each. A hosted plugin is not here: it is asked about by slot,
    /// because two slots can hold the same plugin out of two different files.
    keyed_cache: HashMap<String, bool>,
    waveforms: HashMap<SourceId, Arc<WaveformPeaks>>,

    /// What was last cut or copied.
    ///
    /// Outside the document deliberately, and so outside undo: a clipboard that a document swap
    /// emptied would lose its contents on every Undo, and taking a step back is exactly when
    /// somebody is about to paste. See [`clipboard`].
    clipboard: Clipboard,

    /// The audio tracks a take would be recorded onto, each with the input channels it reads.
    ///
    /// A list because a take can land on several tracks at once, one device channel to each. In
    /// the order they were armed, which is the order the files come out in.
    ///
    /// Not in the document, for the same reason the clipboard is not: arming is how somebody
    /// prepares to play rather than something they wrote, and an Undo that disarmed the track
    /// they were about to record onto would be a surprise with a microphone already live.
    armed: Vec<record::Arm>,
    /// Channels the input device has, remembered from the last time anybody found out.
    ///
    /// Asking means talking to the OS audio server, and a channel picker asks while it draws.
    /// Cleared when the audio settings change, which is the only thing that can make it stale.
    input_channels: Option<usize>,
    /// The count-in last sent to the audio thread, for reading the one running.
    ///
    /// The engine publishes how many frames of it are left and nothing else, which is all the
    /// transport needs and one number short of what a readout does: turning frames back into
    /// "three beats to go" needs the length of a beat, and this side is where that was worked
    /// out. Stale between takes, and harmless — nothing reads it while the count is over.
    counting: Option<auris_engine::CountIn>,
    /// The input device, open while a take is running or somebody is monitoring.
    ///
    /// One device serving both, because it is one device: a take that closed it on stopping would
    /// take the monitor down with it, and two streams onto one interface is a thing drivers
    /// refuse. See [`monitor`] for what that costs and what it buys.
    input: Option<auris_engine::Capture>,
    /// The take that is running, if one is. See [`record`].
    take: Option<record::Take>,
    /// The Japanese text frontend: the folder the settings name, or the shipped dictionary.
    ///
    /// `None` only where neither exists, and the session sings anyway: kana lyrics go
    /// through the built-in table. Owned here rather than loaded per lyric because opening
    /// the folder parses a compiled dictionary — work worth doing once. See [`singer`].
    japanese: Option<auris_vocal::JapaneseDictionary>,
    /// Whether the shipped dictionary stands in while no folder is named — the option, kept
    /// so that clearing the setting returns to the shipped one instead of to nothing.
    shipped_dictionary: bool,
    /// The voice models behind singer tracks, by the file each was read from.
    ///
    /// Keyed by path rather than by track or document id, like [`Self::shipped`] and for the
    /// same reason: the same file is the same voice whichever project is open, and a model is a
    /// couple of hundred megabytes that takes a third of a second to load — worth doing once a
    /// session, not once a song. Behind `Arc<Mutex<_>>` because a frontend renders takes on a
    /// worker thread while the session keeps answering commands; see [`singer`].
    voices: HashMap<PathBuf, (singer::VoiceStamp, Arc<Mutex<auris_singer::VoiceModel>>)>,
    /// Where those models run their inference — the settings' choice, applied to every load.
    ///
    /// Kept beside the cache it governs: changing it empties [`Self::voices`], which is what
    /// makes the change take effect at the very next render rather than the next launch.
    acceleration: auris_singer::Acceleration,
    /// The track the live input is being played through, if anybody asked for that. See
    /// [`monitor`].
    monitored: Vec<TrackId>,
    /// The computer keyboard, when it is being played as one. See [`typing`].
    ///
    /// Outside the document for the same reason [`Self::armed`] is: which octave somebody's hands
    /// are on is how they are playing, not something they wrote, and an Undo that moved the
    /// keyboard under them would be a surprise mid-phrase.
    typing: MusicalTyping,

    /// Third-party CLAP plugins the document names.
    ///
    /// Here rather than in the graph because only half of a hosted plugin belongs in a graph:
    /// the half that answers questions has to stay on this thread and outlive any number of
    /// rebuilds. See [`hosted`] for what that costs and how it is paid.
    hosted: hosted::HostedPlugins,
    /// Third-party VST3 instances, parallel to the CLAP host above.
    vst3: vst3::Vst3Plugins,
}

/// What a Save As produced.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveReport {
    /// The document that was written.
    pub document: PathBuf,
    /// Audio the project refers to that could not be copied into the folder.
    ///
    /// The project saved, and it opens on this machine. It is the *folder* that is incomplete:
    /// carried to another machine, the clips these belong to would be silent. Reported rather
    /// than logged because a save that says nothing is a save the user believes was complete.
    pub uncollected: Vec<PathBuf>,
}

/// What reading a Standard MIDI File produced.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MidiReport {
    /// How many tracks the file turned into.
    pub tracks: usize,
    /// How many notes, across all of them.
    pub notes: usize,
    /// Position just past the last note.
    pub length: Ticks,
}

/// What composing produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeReport {
    /// How many tracks of *music* were created.
    ///
    /// The buses the mix routes through are not counted. They are plumbing rather than parts, and
    /// a report saying eight over a piece with six things playing in it would be answering a
    /// question nobody asked.
    pub tracks: usize,
    /// How many clips.
    pub clips: usize,
    /// How many notes.
    pub notes: usize,
    /// How long the piece is.
    pub length: Ticks,
    /// Instruments a part asked for that this build does not have.
    pub substituted: Vec<String>,
    /// What setting the mix by measurement did, or `None` if the piece could not be rendered.
    ///
    /// Composing ends by listening to what it wrote — see [`Session::balance_levels`] — because
    /// the level a part wants depends on the instrument that answered, and the composer chooses
    /// the part while the session finds the instrument.
    pub balance: Option<BalanceReport>,
    /// How many sung notes the sections' lyrics became — zero for an instrumental piece.
    pub sung: usize,
    /// Sections whose lyrics could not be read — kanji with no Japanese dictionary anywhere —
    /// and so play instrumentally. Their words cost themselves, never the piece.
    pub unsung: Vec<String>,
}

struct Transaction {
    edit: Edit,
    before: Project,
    /// Whether the document had unsaved changes when the gesture opened.
    ///
    /// Kept so [`Session::revert_transaction`] can put that back too: a drag on a freshly saved
    /// document that is then abandoned has changed nothing, and the window should not claim it
    /// has.
    dirty_before: bool,
}

impl Session {
    /// The longest count-in that can be asked for, in bars.
    ///
    /// Four, which is a bar more than anybody counts and two more than anybody should have to sit
    /// through twice. The cap is here because the count is time a musician spends waiting: a
    /// setting that allowed sixteen bars would be a setting somebody hit by accident once and
    /// then had to stop and stare at.
    pub const MAX_COUNT_IN_BARS: u32 = 4;

    /// Opens a session.
    ///
    /// Never fails for want of audio hardware: the engine falls back to running silently, so a
    /// machine with no output device still edits and exports.
    pub fn new(options: SessionOptions) -> Result<Self, SessionError> {
        let fonts = SoundFontBank::shared();
        let registry = default_registry(Arc::clone(&fonts));
        let project = Project::new("Untitled", options.sample_rate);
        let audio = options.audio_preferences.clone();

        let settings = AudioSettings {
            device: audio.device.clone(),
            sample_rate: audio.sample_rate.or_else(|| {
                // A headless engine has no device to ask, so it needs to be told a rate.
                (!options.audio).then_some(options.sample_rate.round().max(1.0) as u32)
            }),
            block_frames: Some(audio.block_frames.max(16)),
            ..AudioSettings::default()
        };
        let (device, engine) = if options.audio {
            start_audio(&settings)?
        } else {
            // `start_audio` opens the default device, which a headless tool must not do — it
            // would claim the hardware and spin an audio thread for nothing.
            auris_engine::start_silent(&settings)
        };

        let gpu = if options.gpu {
            GpuContext::new().map(Arc::new)
        } else {
            None
        };

        let render_bank_rate = engine.sample_rate();
        let mut session = Self {
            saved_project: project.clone(),
            project,
            bank: AudioSourceBank::new(),
            render_bank: AudioSourceBank::new(),
            render_bank_rate,
            fonts,
            shipped: HashMap::new(),
            shipped_library: options.shipped_fonts,
            balance_composed: options.balance_composed,
            registry,
            device: Some(device),
            engine,
            gpu,
            audio,
            headless: !options.audio,
            history: History::default(),
            transaction: None,
            revision: 0,
            needs_rebuild: false,
            meter_is_stale: false,
            last_record: None,
            path: None,
            dirty: false,
            autosave: options.autosave,
            last_save: Instant::now(),
            disk_stamp: None,
            disk_fingerprint: None,
            scope: Arc::new(auris_engine::Scope::new()),
            analyzer: auris_dsp::SpectrumAnalyzer::new(auris_engine::SCOPE_WINDOW),
            param_cache: HashMap::new(),
            keyed_cache: HashMap::new(),
            waveforms: HashMap::new(),
            clipboard: Clipboard::default(),
            armed: Vec::new(),
            input_channels: None,
            counting: None,
            input: None,
            monitored: Vec::new(),
            typing: MusicalTyping::default(),
            take: None,
            japanese: None,
            shipped_dictionary: options.shipped_dictionary,
            voices: HashMap::new(),
            acceleration: auris_singer::Acceleration::default(),
            hosted: hosted::HostedPlugins::default(),
            vst3: vst3::Vst3Plugins::default(),
        };
        session.install_shipped_fonts();
        session.install_shipped_dictionary();
        session.rebuild_graph();
        Ok(session)
    }

    /// The rate the engine is running at.
    ///
    /// A filter's response depends on it — the cookbook designs against the sample rate, and the
    /// top of the spectrum is where the difference between 44.1 and 48 kHz shows — so a display
    /// drawing an equalizer's curve has to draw it at the rate the audio is actually made at.
    pub fn sample_rate(&self) -> f64 {
        self.engine.sample_rate()
    }

    /// What the audio backend ended up doing, for a status line.
    pub fn audio_status(&self) -> AudioStatus {
        AudioStatus {
            device: self
                .device
                .as_ref()
                .map_or_else(|| "none".to_string(), |d| d.name().to_string()),
            running: self.engine.is_running(),
            sample_rate: self.engine.sample_rate(),
            channels: self.engine.channel_count(),
            gpu: self
                .gpu
                .as_ref()
                .map(|context| format!("{} ({})", context.adapter_name(), context.backend())),
        }
    }

    /// Every output device the host can see, and what each can do.
    ///
    /// Queried on demand rather than cached: devices come and go while the application runs,
    /// and this is only called when a settings panel opens.
    pub fn output_devices(&self) -> Vec<AudioDeviceInfo> {
        auris_engine::output_devices()
    }

    /// The audio preferences this session was opened with.
    pub fn audio_preferences(&self) -> &AudioPreferences {
        &self.audio
    }

    /// Reopens the audio output with new preferences.
    ///
    /// The old device is dropped first: two streams on the same hardware is at best a glitch
    /// and at worst a refusal to open. Transport position is deliberately *not* carried over —
    /// the new device starts from where the playhead was, but stopped, because a device swap
    /// mid-playback produces a discontinuity nobody wants to hear.
    ///
    /// Refused outright during a take. A take is stamped with the engine frame it began on, and
    /// the engine that counted those frames is exactly what this replaces — so the clip would be
    /// placed by dividing a count taken from one clock by the rate of another, and land somewhere
    /// on the timeline that has nothing to do with where it was played. `restart_input` declines
    /// to swap the microphone mid-take for its own reasons, which left the take running against a
    /// stale engine either way; this is the enforcement its doc comment already assumed.
    pub fn set_audio_preferences(
        &mut self,
        preferences: AudioPreferences,
    ) -> Result<(), SessionError> {
        if self.take.is_some() {
            return Err(SessionError::RecordingInProgress);
        }
        let input_changed = self.audio.input_device != preferences.input_device;
        if !output_changed(&self.audio, &preferences) {
            self.audio = preferences;
            // Only the input moved, so the output is left alone — but a monitor that is running is
            // running through the *old* microphone and would go on doing so.
            if input_changed {
                self.restart_input();
                if self.monitoring() {
                    self.rebuild_graph();
                }
            }
            return Ok(());
        }
        // Capture the position in seconds, not frames: the new device may run at a different
        // rate, and frames counted at the old one would land somewhere else entirely.
        let playhead = self.engine.playhead_seconds();
        let settings = AudioSettings {
            device: preferences.device.clone(),
            sample_rate: preferences.sample_rate,
            block_frames: Some(preferences.block_frames.max(16)),
            ..AudioSettings::default()
        };

        self.device = None;
        let (device, engine) = if self.headless {
            auris_engine::start_silent(&settings)
        } else {
            start_audio(&settings).map_err(|error| SessionError::AudioRestart(error.to_string()))?
        };

        self.device = Some(device);
        self.engine = engine;
        self.audio = preferences;

        // The capture reads the *engine's* playhead atomic to stamp where a take begins, and this
        // is a different engine. An input left open across the swap would be stamping takes
        // against a clock nothing moves any more.
        self.restart_input();
        // The new engine starts with no graph, no loop and a playhead at zero.
        self.rebuild_graph();
        self.publish_loop();
        self.seek(self.project.tempo_map.seconds_to_ticks(Seconds(playhead)));
        Ok(())
    }

    /// Housekeeping a frontend should call periodically — once per rendered frame is plenty.
    ///
    /// Frees graphs the audio thread has retired, drains the command queue when running silently
    /// where nothing else would, and rebuilds the graph when a parameter has moved a plugin's
    /// latency out from under the delay compensation.
    pub fn poll(&mut self) {
        self.engine.collect_garbage();
        if let Some(device) = &mut self.device
            && !device.is_running()
        {
            device.discard_pending();
        }
        // Writing a parameter is the one edit that reaches a plugin without rebuilding, so it is
        // also the one that can leave the tracks compensated for the wrong number of frames.
        // Rebuilding re-measures every chain and re-sizes the delay lines to match — and going
        // through `invalidate_graph` is what keeps a drag on such a control to one rebuild at the
        // end of the gesture instead of one per pointer move.
        if self.engine.latency_is_stale() {
            self.invalidate_graph();
        }
        // A hosted plugin keeps no clock of its own and its window does not repaint without one.
        // This is also where a window the user closed is noticed, and where a plugin that changed
        // itself — a preset loaded in its own window — puts the unsaved mark back on the title
        // bar. All three are things the plugin says by setting a flag and nothing else.
        if self.hosted.service() {
            self.dirty = true;
        }
        self.vst3.sweep();
    }

    /// The document.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Everything the registry knows how to build.
    pub fn registry(&self) -> &Arc<PluginRegistry> {
        &self.registry
    }

    /// Decoded audio for the project's sources.
    pub fn bank(&self) -> &AudioSourceBank {
        &self.bank
    }

    /// The audio engine's UI-side handle.
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }

    /// Fills `bands` with the spectrum of whatever strip is being watched, in dBFS.
    ///
    /// Bands rather than bins, spaced by octave between `low_hz` and `high_hz`, because that is
    /// what a display can show and what a musician reads. Silence when nothing is being watched
    /// or the window was written through mid-copy — the next call is one repaint away and will
    /// find a settled one, which is cheaper than making the audio thread wait.
    pub fn spectrum(&mut self, low_hz: f64, high_hz: f64, bands: &mut [f32]) {
        bands.fill(auris_dsp::SILENCE_DB);
        let mut samples = vec![0.0f32; self.analyzer.size()];
        if !self.scope.read(&mut samples) {
            return;
        }
        let rate = self.scope.sample_rate();
        self.analyzer.reset();
        self.analyzer.push(&samples);
        let mut bins = vec![0.0f32; self.analyzer.bin_count()];
        self.analyzer.magnitudes(&mut bins);
        auris_dsp::bands_from_bins(&bins, rate, low_hz, high_hz, bands);
    }

    /// Level a band with nothing in it reports, so a display knows where its floor is.
    pub fn spectrum_silence() -> f32 {
        auris_dsp::SILENCE_DB
    }

    /// Which strip the scope should follow, given what a frontend currently has open.
    ///
    /// Here rather than in the frontend because it is the mapping from *a plugin's place in the
    /// document* to *a position in the render graph*, and positions in the graph are this layer's
    /// business: a frontend that worked it out itself would be reading the track order to do it.
    pub fn watch_strip(&self, track: Option<TrackId>) {
        let source = match track {
            None => auris_engine::ScopeSource::Master,
            Some(id) => match self.project.track_index(id) {
                Some(index) => auris_engine::ScopeSource::Track(index),
                None => auris_engine::ScopeSource::Off,
            },
        };
        self.scope.watch(source);
    }

    /// Stops the analysis, for when nothing is looking at it.
    pub fn stop_watching(&self) {
        self.scope.watch(auris_engine::ScopeSource::Off);
    }

    /// Lock-free level meters.
    pub fn meters(&self) -> &MeterBank {
        self.engine.meters()
    }

    /// Where the document was last saved or loaded from.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// `true` when there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// A number that moves whenever the document does — edits, undo, redo, another document.
    ///
    /// For caching answers derived from the document: a frontend that repaints on a timer
    /// compares this against the value it computed under, and only a change makes it ask an
    /// expensive question again. Monotonic within a session, meaningless across two.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Waveform peaks for an imported source, once it has been analysed.
    pub fn waveform(&self, source: SourceId) -> Option<&Arc<WaveformPeaks>> {
        self.waveforms.get(&source)
    }

    /// A `Send` snapshot that can be rendered on another thread.
    ///
    /// Takes `&mut self` for one reason: a hosted plugin has to be *built here*, on this thread,
    /// because the half that can build one may not leave it. An export that did not do this would
    /// bounce the mix minus every plugin the user loaded.
    pub fn render_job(&mut self) -> RenderJob {
        self.job_for(self.project.clone())
    }

    /// A job that renders `project` rather than the document, with this session's sounds.
    ///
    /// What the balance pass measures is a *variation* on the open document — one track soloed,
    /// or the same piece with different faders — and it must be rendered through the same bank,
    /// the same registry and the same hosted plugins as the real thing, or the measurement would
    /// be of something else.
    pub(crate) fn job_for(&mut self, project: Project) -> RenderJob {
        // The export's own instances, at the project's rate rather than the device's. A plugin
        // that is already rendering keeps doing so: `place` hands out a second instance and takes
        // the first back when the export drops it.
        let prepare = auris_core::plugin::PrepareContext::new(
            self.project.sample_rate,
            self.engine.max_block(),
            auris_engine::RENDER_CHANNELS,
        );
        let mut placed = self.hosted.place(&project, &prepare);
        placed.extend(self.vst3.place(&project, &prepare));
        let mut instruments = self.hosted.place_instruments(&project, &prepare);
        instruments.extend(self.vst3.place_instruments(&project, &prepare));
        RenderJob::new(
            project,
            self.bank.clone(),
            Arc::clone(&self.registry),
            placed,
            instruments,
        )
    }

    /// Starts a gesture. Mutations until [`Self::end_transaction`] become one undo step.
    ///
    /// Nesting is not supported; a second call finishes the first before starting the next,
    /// which preserves an interrupted gesture as its own undoable, rendered edit.
    pub fn begin_transaction(&mut self, edit: Edit) {
        if self.transaction.is_some() {
            self.end_transaction();
        }
        self.transaction = Some(Transaction {
            edit,
            before: self.project.clone(),
            dirty_before: self.dirty,
        });
    }

    /// Ends a gesture, returning whether it changed anything.
    ///
    /// A transaction that made no difference records no undo step and triggers no rebuild — so
    /// a click that only selected something cannot push real history off the end of the stack.
    pub fn end_transaction(&mut self) -> bool {
        let Some(transaction) = self.transaction.take() else {
            return false;
        };
        if transaction.before == self.project {
            self.needs_rebuild = false;
            self.dirty = transaction.dirty_before;
            return false;
        }
        self.history.push(transaction.edit, &transaction.before);
        // A finished gesture breaks a run of repeats, the same way any ordinary edit does. Without
        // this a drag would leave `last_record` set to the edit it made, and a nudge of the same
        // kind a moment later would fold into the step the drag had already pushed — one Undo
        // taking back both.
        self.last_record = None;
        self.dirty = true;
        if std::mem::take(&mut self.needs_rebuild) {
            self.rebuild_graph();
        }
        true
    }

    /// Abandons a gesture without recording it. The document keeps whatever it currently holds.
    pub fn cancel_transaction(&mut self) {
        self.transaction = None;
        if std::mem::take(&mut self.needs_rebuild) {
            self.rebuild_graph();
        }
    }

    /// Abandons a gesture and puts the document back the way it was when it opened.
    ///
    /// What Escape means during a drag: the clip goes back where it was picked up from, and
    /// nothing lands on the undo stack. [`Self::cancel_transaction`] keeps the half-finished
    /// result instead, for a host that has already told the user the edit took.
    ///
    /// Returns whether anything was put back.
    pub fn revert_transaction(&mut self) -> bool {
        let Some(transaction) = self.transaction.take() else {
            return false;
        };
        if transaction.before == self.project {
            // Nothing moved, so there is nothing to restore — but a mutation that cancelled
            // itself out may still have marked the graph stale.
            if std::mem::take(&mut self.needs_rebuild) {
                self.rebuild_graph();
            }
            return false;
        }
        self.replace_project(transaction.before);
        self.dirty = transaction.dirty_before;
        true
    }

    /// Steps back one edit, returning what it reversed.
    ///
    /// Nothing while a gesture is open — see [`Self::can_undo`], which is the same rule stated as
    /// a question.
    pub fn undo(&mut self) -> Option<Edit> {
        if self.transaction.is_some() {
            return None;
        }
        let edit = self.history.undo_edit()?;
        let project = self.history.undo(&self.project)?;
        self.replace_project(project);
        self.dirty = self.project != self.saved_project;
        Some(edit)
    }

    /// Steps forward one edit, returning what it reapplied.
    ///
    /// Nothing while a gesture is open, for the reasons under [`Self::can_undo`].
    pub fn redo(&mut self) -> Option<Edit> {
        if self.transaction.is_some() {
            return None;
        }
        let edit = self.history.redo_edit()?;
        let project = self.history.redo(&self.project)?;
        self.replace_project(project);
        self.dirty = self.project != self.saved_project;
        Some(edit)
    }

    /// `true` when there is something to undo.
    ///
    /// Never during a gesture. A pointer that is still down owns the document: the mutations it
    /// has made so far are deliberately not on the history yet, and stepping through history from
    /// underneath it goes wrong in three ways at once. `replace_project` would drop the open
    /// transaction on the floor — not commit it, not revert it, drop it, so Escape could no longer
    /// put the clip back where it was picked up from. The drag would still be physically running,
    /// so every further pointer move would land its own history entry instead of joining the one
    /// step the gesture was meant to be, and enough of them evict unrelated work from the other
    /// end of a stack that holds sixty-four. And the close at mouse-up would find nothing to close
    /// and quietly do nothing.
    ///
    /// So the answer is no, and the gesture finishes first. It is one entry on the stack a moment
    /// later, and undoing it then does what pressing undo during it looked like it would.
    pub fn can_undo(&self) -> bool {
        self.transaction.is_none() && self.history.can_undo()
    }

    /// `true` when there is something to redo, and no gesture is open. See [`Self::can_undo`].
    pub fn can_redo(&self) -> bool {
        self.transaction.is_none() && self.history.can_redo()
    }

    /// Drops the undo history and marks the document as unmodified.
    ///
    /// For scaffolding a host writes itself — a demo project, a template — which should not be
    /// undoable and should not make a freshly opened document look edited.
    pub fn forget_history(&mut self) {
        if self.transaction.is_some() {
            return;
        }
        self.history.clear();
        self.last_record = None;
        self.dirty = false;
        self.saved_project = self.project.clone();
    }

    /// Records an undo step for the edit about to be made.
    ///
    /// Inside a transaction this does nothing: the snapshot was taken when the transaction
    /// opened, and one gesture is one step.
    ///
    /// Validate *before* calling this. Pushing a step clears the redo stack and marks the
    /// document dirty, so a command that records first and refuses after has already cost the
    /// user both — a phantom step that undoes nothing, and a redo branch that is simply gone.
    fn record(&mut self, edit: Edit) {
        if self.transaction.is_none() {
            self.history.push(edit, &self.project);
        }
        // Any ordinary edit breaks a run of repeats: a tempo nudge, a note, another tempo nudge
        // must be three steps, or undoing the second nudge would take the note with it.
        self.last_record = None;
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// How long a repeated edit keeps folding into the step before it.
    ///
    /// Long enough to cover the gap between two notches of a wheel a user is still turning,
    /// short enough that coming back to the same control a moment later is a step of its own.
    const COALESCE: Duration = Duration::from_millis(600);

    /// Records an edit that arrives as a stream of small steps — a wheel notch, a held arrow key
    /// — folding it into the previous step when it is the same edit made a moment ago.
    ///
    /// A gesture with a beginning and an end wants [`Self::begin_transaction`] instead. This is
    /// for the ones that have neither: without it a flick of the wheel over the tempo readout
    /// pushes one undo step per event and shoves the real history off the end of the stack.
    fn record_repeating(&mut self, edit: Edit) {
        self.record_repeating_at(edit, Instant::now());
    }

    /// [`Self::record_repeating`] against a clock a test can hold still.
    fn record_repeating_at(&mut self, edit: Edit, now: Instant) {
        let folds = self
            .last_record
            .is_some_and(|(last, at)| last == edit && now.duration_since(at) < Self::COALESCE);
        if folds {
            self.dirty = true;
            self.revision = self.revision.wrapping_add(1);
        } else {
            self.record(edit);
        }
        self.last_record = Some((edit, now));
    }

    /// Marks the render graph as stale, rebuilding immediately outside a transaction.
    fn invalidate_graph(&mut self) {
        if self.transaction.is_some() {
            self.needs_rebuild = true;
        } else {
            self.rebuild_graph();
        }
    }

    /// Rebuilds the render graph and hands it to the audio thread.
    pub fn rebuild_graph(&mut self) {
        // Whatever the meter now is, this is the copy the engine will be holding.
        self.meter_is_stale = false;
        let rate = self.engine.sample_rate();
        // Only ever true just after the output device changed, which is also the only time
        // resampling every source is worth what it costs.
        if self.render_bank_rate != rate {
            self.render_bank = bank_at_rate(&self.bank, rate);
            self.render_bank_rate = rate;
        }
        // Every rebuild, not only after a rate change: what a clip's stretch is depends on the
        // tempo and on the clip, and both of those are edits that land here.
        fill_stretches(&self.project, &mut self.render_bank);
        // Whatever the audio thread has handed back is dropped before the hosted plugins are
        // asked for their effects, so an instance whose graph has already been retired is reused
        // rather than replaced. Without this every rebuild would find its plugin still busy and
        // build a second one. See [`hosted`].
        self.engine.collect_garbage();
        let prepare = auris_core::plugin::PrepareContext::new(
            rate,
            self.engine.max_block(),
            auris_engine::RENDER_CHANNELS,
        );
        let mut placed = self.hosted.place(&self.project, &prepare);
        placed.extend(self.vst3.place(&self.project, &prepare));
        let mut instruments = self.hosted.place_instruments(&self.project, &prepare);
        instruments.extend(self.vst3.place_instruments(&self.project, &prepare));

        let mut graph = RenderGraph::build_with(
            &self.project,
            &self.render_bank,
            &self.registry,
            &mut placed,
            &mut instruments,
            self.engine.max_block(),
            rate,
        );
        graph.set_scope(Arc::clone(&self.scope));
        // Re-attached rather than remembered by the graph, for the same reason the scope is: this
        // runs on every structural edit, and a monitor that did not survive one would go quiet the
        // moment somebody added a track while playing.
        graph.set_monitors(&self.monitor_taps());
        if let Err(error) = self.engine.set_graph(graph) {
            log::warn!("could not update the render graph: {error}");
        }
    }

    fn send(&self, command: EngineCommand) {
        if let Err(error) = self.engine.send(command) {
            log::debug!("engine command dropped: {error}");
        }
    }

    /// Replaces the whole document, keeping the engine in step.
    fn replace_project(&mut self, project: Project) {
        self.adopt_project(project);
        self.rebuild_graph();
        // The loop lives in the audio thread's transport and only `SetLoop` moves it, so a
        // document swap that does not republish leaves playback wrapping the old range.
        self.publish_loop();
    }

    /// Replaces the document *without* telling the engine, for a caller that has more to do first.
    ///
    /// [`Self::open`] is the one: its document names files that have not been read yet, and a
    /// graph built over those would be a graph of silent tracks — logged, one warning per track,
    /// about assets that are about to arrive — and then thrown away and built again. Every other
    /// caller wants [`Self::replace_project`].
    fn adopt_project(&mut self, project: Project) {
        // The hosted slots are deliberately *not* cleared here. Undo, redo, a cancelled drag and
        // a compose all arrive as a variant of the same document, whose slot ids still name the
        // same plugins — keeping the instances is what lets a preset loaded inside a plugin
        // survive an unrelated undo, and what keeps that undo from paying an instantiation. It
        // is `Session::open`, where a *different* document takes over and could reuse an id for
        // a different plugin, that clears them.
        self.project = project;
        self.armed
            .retain(|arm| self.project.track(arm.track).is_some());
        let monitors_before = self.monitored.len();
        self.monitored
            .retain(|track| self.project.track(*track).is_some());
        if self.monitored.len() != monitors_before {
            self.publish_monitors();
            self.close_input_if_idle();
        }
        self.transaction = None;
        self.needs_rebuild = false;
        self.last_record = None;
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Whether a change of preferences means the output stream has to be torn down and reopened.
///
/// The input device is not the output stream's business. It is opened per take and read when one
/// starts, so changing it costs nothing that is already running — and restarting for it would
/// mean the settings window silenced the song to change a microphone.
fn output_changed(before: &AudioPreferences, after: &AudioPreferences) -> bool {
    before.device != after.device
        || before.sample_rate != after.sample_rate
        || before.block_frames != after.block_frames
}

impl Drop for Session {
    /// Closes a take that is still being written.
    ///
    /// The last line of defence rather than the usual route: a frontend stops the take itself
    /// before it asks about unsaved work, because a stopped take becomes a clip and a clip is
    /// something the ordinary question already covers. This catches the paths that never get
    /// there — a window closed by the system, a panic unwinding out of the interface — where the
    /// alternative is a file on disk whose header never learned how long it is. See
    /// `Session::abandon_take`.
    fn drop(&mut self) {
        self.abandon_take();
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("project", &self.project.name)
            .field("tracks", &self.project.tracks.len())
            .field("dirty", &self.dirty)
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::ParamTarget;
    use crate::session::fixtures::session;
    use auris_core::param::ParamId;
    use auris_core::{ClipId, EffectSlotId, Note};

    #[test]
    fn choosing_a_microphone_does_not_reopen_the_speakers() {
        // The settings window applies the whole preferences object for any change in it, so
        // without this a user picking an input device would have the song stop, the graph rebuild
        // and the playhead jump — for a device the output stream has never heard of.
        let base = AudioPreferences::default();
        let mut input = base.clone();
        input.input_device = Some("Scarlett 2i2".to_string());
        assert!(!output_changed(&base, &input));

        for changed in [
            AudioPreferences {
                device: Some("Speakers".to_string()),
                ..input.clone()
            },
            AudioPreferences {
                sample_rate: Some(96_000),
                ..input.clone()
            },
            AudioPreferences {
                block_frames: 128,
                ..input.clone()
            },
        ] {
            assert!(
                output_changed(&input, &changed),
                "{changed:?} should have reopened the output"
            );
        }
    }

    /// A moment `ms` after whichever `Instant` it is added to.
    fn tick(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn a_refused_edit_leaves_no_step_and_no_dirt_behind_anywhere() {
        // Every mutator that once recorded before validating, exercised with ids nothing owns.
        // What this pins: a record clears the redo stack and marks the document dirty, so a
        // command that refuses must refuse *before* it records — the same invariant the
        // preset and instrument commands already keep.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();
        let ghost = ClipId(9_999);
        let slot = EffectSlotId(9_999);

        assert!(session.move_clip(ghost, Ticks::ZERO).is_err());
        assert!(session.resize_clip(ghost, Ticks(960)).is_err());
        assert!(
            session
                .add_note(ghost, Note::new(60, Ticks::ZERO, Ticks(960)))
                .is_err()
        );
        assert!(
            session
                .move_notes(ghost, &[(0, Ticks::ZERO, 60)], Ticks::ZERO, 0)
                .is_err()
        );
        assert!(session.resize_note(ghost, 0, Ticks(960)).is_err());
        session.move_clips(&[(ghost, Ticks::ZERO)], Ticks(960));
        session.remove_effect(slot);
        session.set_effect_enabled(Some(track), slot, false);
        session.move_effect(Some(track), slot, 1);
        session.set_param(ParamTarget::TrackGain(TrackId(9_999)), -3.0);

        assert!(!session.can_undo(), "a refused edit left a step behind");
        assert!(
            !session.is_dirty(),
            "a refused edit marked the document dirty"
        );

        // And the redo branch survives a stale gesture, which is where the cost actually
        // landed: the piano roll can hold a clip id that undo just removed.
        session
            .add_midi_clip(track, "A", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session.undo();
        assert!(session.can_redo());
        assert!(session.move_clip(ghost, Ticks::ZERO).is_err());
        assert!(
            session.can_redo(),
            "a refused edit destroyed the redo stack"
        );
    }

    #[test]
    fn a_clamped_or_identical_edit_leaves_no_step_and_no_dirt() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, Note::new(0, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.forget_history();

        session.move_clips(&[(clip, Ticks::ZERO)], -Ticks::QUARTER);
        session
            .move_notes(clip, &[(0, Ticks::ZERO, 0)], -Ticks::QUARTER, -100)
            .unwrap();
        session.rename_clip(clip, "Riff").unwrap();
        session.set_track_mute(track, false).unwrap();
        session.set_track_solo(track, false).unwrap();
        session.clear_harmony(Ticks::ZERO, Ticks::ZERO);

        assert!(!session.can_undo());
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_headless_session_opens_without_audio() {
        let session = session();
        let status = session.audio_status();
        assert!(!status.running);
        assert!(!session.is_playing());
        assert!(session.project().tracks.is_empty());
    }

    #[test]
    fn a_transaction_that_changes_nothing_records_no_undo_step() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        let steps_before = session.can_undo();

        session.begin_transaction(Edit::MoveClip);
        let changed = session.end_transaction();

        assert!(!changed);
        assert_eq!(session.can_undo(), steps_before);
        // The no-op transaction must not have discarded anything either.
        assert!(session.undo().is_some());
    }

    #[test]
    fn history_cannot_be_forgotten_halfway_through_a_transaction() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.begin_transaction(Edit::MoveClip);
        session.forget_history();

        assert!(!session.can_undo(), "the open transaction hides history");
        session.end_transaction();
        assert!(
            session.can_undo(),
            "forget_history must not erase an open edit"
        );
    }

    #[test]
    fn a_transaction_that_moves_back_to_its_origin_restores_the_saved_state() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();

        session.begin_transaction(Edit::MoveClip);
        session.move_clip(clip, Ticks::QUARTER).unwrap();
        session.move_clip(clip, Ticks::ZERO).unwrap();

        assert!(!session.end_transaction());
        assert!(!session.is_dirty());
        assert!(!session.can_undo());
    }

    #[test]
    fn a_transaction_collapses_many_edits_into_one_undo_step() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();

        session.begin_transaction(Edit::MoveClip);
        for beat in 1..=8 {
            session
                .move_clip(clip, Ticks::from_beats(beat as f64))
                .unwrap();
        }
        assert!(session.end_transaction());

        assert_eq!(
            session.midi_clip(clip).unwrap().start,
            Ticks::from_beats(8.0)
        );
        session.undo().unwrap();
        // One step takes the clip all the way back, not one beat back.
        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
    }

    #[test]
    fn starting_a_second_transaction_finishes_the_interrupted_edit() {
        let mut session = session();
        session.forget_history();

        session.begin_transaction(Edit::AddInstrumentTrack);
        session.add_default_instrument_track("Lead").unwrap();
        session.begin_transaction(Edit::MoveClip);

        assert_eq!(session.project().tracks.len(), 1);
        assert!(
            !session.end_transaction(),
            "the second gesture changed nothing"
        );
        assert_eq!(session.undo(), Some(Edit::AddInstrumentTrack));
        assert!(session.project().tracks.is_empty());
    }

    #[test]
    fn undo_and_redo_walk_the_document() {
        let mut session = session();
        session.add_default_instrument_track("One").unwrap();
        session.add_default_instrument_track("Two").unwrap();
        assert_eq!(session.project().tracks.len(), 2);

        assert_eq!(session.undo(), Some(Edit::AddInstrumentTrack));
        assert_eq!(session.project().tracks.len(), 1);
        assert_eq!(session.redo(), Some(Edit::AddInstrumentTrack));
        assert_eq!(session.project().tracks.len(), 2);
    }

    #[test]
    fn undo_back_to_the_saved_document_clears_dirty() {
        let mut session = session();
        session.add_default_instrument_track("Saved").unwrap();
        session.forget_history();
        session.add_default_instrument_track("Later").unwrap();
        assert!(session.is_dirty());

        session.undo();
        assert!(!session.is_dirty());
        session.redo();
        assert!(session.is_dirty());
    }

    #[test]
    fn an_abandoned_gesture_puts_the_document_back_and_leaves_no_trace() {
        // Escape during a drag. The clip goes back where it was picked up from, the undo stack
        // is untouched, and a document that was saved a moment ago is still saved.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.forget_history();
        let steps = session.history.can_undo();

        session.begin_transaction(Edit::MoveClip);
        session.move_clips(&[(clip, Ticks::ZERO)], Ticks::QUARTER);
        assert_ne!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);

        assert!(session.revert_transaction());
        assert_eq!(session.midi_clip(clip).unwrap().start, Ticks::ZERO);
        assert_eq!(session.history.can_undo(), steps);
        assert!(!session.is_dirty(), "an abandoned gesture edited nothing");
    }

    #[test]
    fn a_gesture_that_moved_nothing_is_not_worth_reverting() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.begin_transaction(Edit::MoveClip);
        assert!(!session.revert_transaction());
        assert!(
            session.transaction.is_none(),
            "the gesture is over either way"
        );
    }

    #[test]
    fn a_parameter_changed_without_a_drag_can_still_be_undone() {
        // A menu choice, a toggle or the wheel reaches `set_param` with no gesture around it.
        // Unrecorded, Undo took back whatever the user did *before* touching the knob instead.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let descriptor = session.descriptor_for(target).unwrap();
        let before = session.param_value(target, &descriptor);
        let after = descriptor.clamp(descriptor.max);
        assert_ne!(before, after, "the test needs a parameter it can move");

        session.set_param(target, after);
        assert_eq!(session.undo(), Some(Edit::AdjustParameter(target)));
        assert_eq!(session.param_value(target, &descriptor), before);
    }

    #[test]
    fn notches_on_two_different_controls_are_two_undo_steps() {
        // Coalescing compares edits, and `AdjustParameter` used to carry no target — so a
        // cutoff notch and a fader notch within the window folded into one step, and undo
        // silently took back the first alongside the second.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let cutoff = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let fader = ParamTarget::TrackGain(track);
        session.forget_history();

        let start = Instant::now();
        session.record_repeating_at(Edit::AdjustParameter(cutoff), start);
        session.record_repeating_at(Edit::AdjustParameter(fader), start + tick(50));
        assert_eq!(session.undo(), Some(Edit::AdjustParameter(fader)));
        assert_eq!(
            session.undo(),
            Some(Edit::AdjustParameter(cutoff)),
            "two controls inside the window folded into one step"
        );
    }

    #[test]
    fn a_stream_of_notches_is_one_undo_step_and_a_later_one_is_its_own() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();

        let start = Instant::now();
        for notch in 1..=8i32 {
            session.record_repeating_at(
                Edit::ChangeTempo(Ticks::ZERO),
                start + tick(notch as u64 * 50),
            );
            session.project.set_bpm(120.0 + f64::from(notch));
        }
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 120.0);
        assert!(!session.can_undo(), "eight notches were one step");

        // Coming back to the control after the window has closed is a step of its own.
        session.redo();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start + tick(5_000));
        session.project.set_bpm(140.0);
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 128.0);
    }

    #[test]
    fn a_run_of_nudges_is_one_step_and_the_drag_before_it_is_another() {
        // A held arrow key arrives as one call per repeat. Without folding, a second of it is
        // thirty steps out of a stack that holds sixty-four, and the afternoon's real history is
        // pushed off the end by a key nobody meant to lean on.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Nudged", Ticks::ZERO, Ticks(1920))
            .unwrap();

        // A drag first, which is what makes the second half of this test worth writing.
        session.begin_transaction(Edit::MoveClip);
        session.move_clips(&[(clip, Ticks::ZERO)], Ticks(480));
        assert!(session.end_transaction());
        session.forget_history();
        session.begin_transaction(Edit::MoveClip);
        session.move_clips(&[(clip, Ticks(480))], Ticks(480));
        assert!(session.end_transaction());

        // Then a run of nudges, close together, all of them one step.
        for _ in 0..6 {
            let at = session.clip_start(clip).unwrap();
            session.move_clips(&[(clip, at)], Ticks(240));
        }
        assert_eq!(session.clip_start(clip), Some(Ticks(960 + 6 * 240)));

        // One Undo takes the whole run back to where the drag left it — and the drag is still
        // there underneath, rather than having been folded in with it.
        assert_eq!(session.undo(), Some(Edit::MoveClip));
        assert_eq!(
            session.clip_start(clip),
            Some(Ticks(960)),
            "the nudges were one step"
        );
        assert_eq!(session.undo(), Some(Edit::MoveClip));
        assert_eq!(
            session.clip_start(clip),
            Some(Ticks(480)),
            "and the drag was its own"
        );
    }

    #[test]
    fn an_edit_in_between_keeps_two_repeats_apart() {
        // Nudge, write a note, nudge again. Folding the second nudge into the first would make
        // one Undo take the note with it.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();

        let start = Instant::now();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start);
        session.project.set_bpm(130.0);
        session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start + tick(20));
        session.project.set_bpm(140.0);

        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 130.0);
        assert_eq!(session.undo(), Some(Edit::AddClip));
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 120.0);
    }

    #[test]
    fn a_split_in_between_keeps_two_repeats_apart() {
        // `split_clip` pushes its own history entry rather than going through `record`, so it was
        // the one edit that did not close the coalescing window: the nudge after the split folded
        // into the one before it, and a single Undo took the split and the second nudge together.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session.forget_history();

        let start = Instant::now();
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start);
        session.project.set_bpm(130.0);
        session
            .split_clip(clip, Ticks::from_beats(2.0))
            .expect("splits");
        session.record_repeating_at(Edit::ChangeTempo(Ticks::ZERO), start + tick(20));
        session.project.set_bpm(140.0);

        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(
            session.project().bpm(),
            130.0,
            "the nudge after the split came back on its own"
        );
        assert_eq!(session.undo(), Some(Edit::SplitClip));
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.project().bpm(), 120.0);
    }

    #[test]
    fn a_render_job_is_independent_of_later_edits() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();

        let mut job = session.render_job();
        session.remove_track(track).unwrap();

        // The job kept its own copy, so the render still contains the note.
        let rendered = job
            .render(
                &auris_engine::OfflineOptions::whole_project(),
                &mut auris_engine::RenderProgress::default(),
            )
            .unwrap();
        assert!(rendered.peak() > 0.01);
        assert!(session.project().tracks.is_empty());
    }
}
