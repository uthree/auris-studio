//! Messages the UI sends to the audio thread.
//!
//! Structural edits — adding a track, moving a note, loading a project — travel as a whole new
//! [`RenderGraph`] built off the audio thread. Everything else is a small, allocation-free
//! command, so moving a fader does not rebuild the world.

use auris_core::param::ParamId;

use crate::graph::RenderGraph;

/// One instruction for the audio thread.
///
/// Commands are applied in the order they were sent, at the start of the block that picks them
/// up. Indices refer to positions in the project's track list, which the render graph preserves
/// even for tracks whose plugin could not be instantiated.
pub enum EngineCommand {
    /// Replaces the whole graph. The old one is handed back for the UI thread to drop.
    SetGraph(Box<RenderGraph>),
    /// Starts the transport.
    Play,
    /// Stops the transport and releases every sounding note.
    Stop,
    /// Moves the playhead. Sounding notes are released so nothing hangs.
    Seek {
        /// New playhead position in frames.
        frames: u64,
    },
    /// Sets the loop region and whether it is active.
    SetLoop {
        /// Whether playback wraps at the loop end.
        enabled: bool,
        /// First frame of the loop.
        start: u64,
        /// One past the last frame of the loop.
        end: u64,
    },
    /// Moves a track fader.
    SetTrackGain {
        /// Track position in the project.
        index: usize,
        /// New fader position in decibels.
        gain_db: f32,
    },
    /// Moves a track's pan control.
    SetTrackPan {
        /// Track position in the project.
        index: usize,
        /// -1.0 hard left to 1.0 hard right.
        pan: f32,
    },
    /// Toggles a track's mute.
    SetTrackMute {
        /// Track position in the project.
        index: usize,
        /// Whether the track is silenced.
        mute: bool,
    },
    /// Moves one of a track's send levels.
    SetSendLevel {
        /// Track position in the project.
        track: usize,
        /// Position of the send in that track's send list.
        send: usize,
        /// New send level in decibels.
        level_db: f32,
    },
    /// Moves the master fader, in decibels.
    SetMasterGain(f32),
    /// Sets the master bus stereo position, -1.0 (left) to 1.0 (right).
    SetMasterPan(f32),
    /// Writes an effect parameter.
    SetEffectParam {
        /// Track position, or `None` for the master bus.
        track: Option<usize>,
        /// Position of the effect in the chain.
        slot: usize,
        /// Which parameter to write.
        param: ParamId,
        /// New value, clamped by the plugin.
        value: f32,
    },
    /// Writes a parameter on a track's instrument.
    SetInstrumentParam {
        /// Track position in the project.
        track: usize,
        /// Which parameter to write.
        param: ParamId,
        /// New value, clamped by the plugin.
        value: f32,
    },
    /// Plays a note immediately, for the piano roll and the on-screen keyboard.
    NoteOn {
        /// Track position in the project.
        track: usize,
        /// MIDI note number.
        pitch: u8,
        /// Attack strength, 0.0 to 1.0.
        velocity: f32,
    },
    /// Releases an auditioned note.
    NoteOff {
        /// Track position in the project.
        track: usize,
        /// MIDI note number.
        pitch: u8,
    },
    /// Bends every note sounding on a track, and every one that follows.
    ///
    /// Channel state rather than an event about a note: an instrument holds the last bend it was
    /// given. Zero is what puts it back, and something has to send that — a wheel let go of is a
    /// wheel sprung back, not a wheel forgotten.
    PitchBend {
        /// Track position in the project.
        track: usize,
        /// How far to bend, in semitones.
        semitones: f32,
    },
    /// Moves one of a track's controllers. Channel state, like the bend.
    Controller {
        /// Track position in the project.
        track: usize,
        /// Which controller, 0 to 127. 1 is the modulation wheel.
        number: u8,
        /// How far it is up, 0.0 to 1.0.
        value: f32,
    },
    /// Turns the click on or off.
    SetMetronome(bool),
    /// Silences everything: voices, delay lines and filter memory.
    Panic,
}

impl std::fmt::Debug for EngineCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The graph holds trait objects that cannot be formatted, so name it and stop.
            Self::SetGraph(graph) => f
                .debug_struct("SetGraph")
                .field("tracks", &graph.track_count())
                .finish(),
            Self::Play => f.write_str("Play"),
            Self::Stop => f.write_str("Stop"),
            Self::Seek { frames } => f.debug_struct("Seek").field("frames", frames).finish(),
            Self::SetLoop {
                enabled,
                start,
                end,
            } => f
                .debug_struct("SetLoop")
                .field("enabled", enabled)
                .field("start", start)
                .field("end", end)
                .finish(),
            Self::SetTrackGain { index, gain_db } => f
                .debug_struct("SetTrackGain")
                .field("index", index)
                .field("gain_db", gain_db)
                .finish(),
            Self::SetTrackPan { index, pan } => f
                .debug_struct("SetTrackPan")
                .field("index", index)
                .field("pan", pan)
                .finish(),
            Self::SetTrackMute { index, mute } => f
                .debug_struct("SetTrackMute")
                .field("index", index)
                .field("mute", mute)
                .finish(),
            Self::SetSendLevel {
                track,
                send,
                level_db,
            } => f
                .debug_struct("SetSendLevel")
                .field("track", track)
                .field("send", send)
                .field("level_db", level_db)
                .finish(),
            Self::SetMasterGain(gain_db) => f.debug_tuple("SetMasterGain").field(gain_db).finish(),
            Self::SetMasterPan(pan) => f.debug_tuple("SetMasterPan").field(pan).finish(),
            Self::SetEffectParam {
                track,
                slot,
                param,
                value,
            } => f
                .debug_struct("SetEffectParam")
                .field("track", track)
                .field("slot", slot)
                .field("param", param)
                .field("value", value)
                .finish(),
            Self::SetInstrumentParam {
                track,
                param,
                value,
            } => f
                .debug_struct("SetInstrumentParam")
                .field("track", track)
                .field("param", param)
                .field("value", value)
                .finish(),
            Self::NoteOn {
                track,
                pitch,
                velocity,
            } => f
                .debug_struct("NoteOn")
                .field("track", track)
                .field("pitch", pitch)
                .field("velocity", velocity)
                .finish(),
            Self::NoteOff { track, pitch } => f
                .debug_struct("NoteOff")
                .field("track", track)
                .field("pitch", pitch)
                .finish(),
            Self::PitchBend { track, semitones } => f
                .debug_struct("PitchBend")
                .field("track", track)
                .field("semitones", semitones)
                .finish(),
            Self::Controller {
                track,
                number,
                value,
            } => f
                .debug_struct("Controller")
                .field("track", track)
                .field("number", number)
                .field("value", value)
                .finish(),
            Self::SetMetronome(enabled) => f.debug_tuple("SetMetronome").field(enabled).finish(),
            Self::Panic => f.write_str("Panic"),
        }
    }
}
