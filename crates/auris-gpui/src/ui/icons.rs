//! Shared icon vocabulary backed by Atelier's monochrome SVG assets.
//!
//! Atelier keeps familiar controls in one 24px SVG family; Auris's
//! domain-specific marks live beside the provider assets, use the same stroke geometry,
//! and reach Atelier's `Icon` as an `IconSource`.

use atelier::{Icon as AtelierIcon, IconName as AtelierIconName, IconSource};
use gpui::{Hsla, IntoElement, Pixels};

/// An icon the UI can draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A semantic library sound family.
    Sound(auris_session::SoundIcon),
    /// Return the playhead to the start.
    ToStart,
    /// Start playback.
    Play,
    /// Pause playback.
    Pause,
    /// Stop playback and rewind.
    Stop,
    /// Start recording a take.
    Record,
    /// Toggle the cycle region.
    Loop,
    /// Toggle the punch region: a record disc between range brackets.
    Punch,
    /// Toggle the click: the wedge and its pendulum.
    Metronome,
    /// Add something.
    Plus,
    /// Move up in a list.
    ChevronUp,
    /// Move down in a list.
    ChevronDown,
    /// A branch of a tree that is shut. Points at what opening it would reveal.
    ChevronRight,
    /// Remove something.
    Cross,
    /// Additional commands: three horizontal dots.
    More,
    /// The application menu: three horizontal lines.
    Menu,
    /// A menu item that is currently on.
    Check,
    /// The sound library: a folder with a raised tab.
    Library,
    /// The piano roll: notes lying at different places along a grid.
    Notes,
    /// The mixer: a bank of vertical faders.
    Faders,
    /// The inspector: an information badge for the selected item's properties.
    Inspector,
    /// The log: a folded page with lines of text.
    Log,
    /// An instrument: an enclosed piano keyboard.
    Keyboard,
    /// A singing voice: a microphone in its stand.
    Microphone,
    /// An effect: a knob with its pointer.
    Knob,
    /// A sound — a font's preset, or the shelf a set of them sits on.
    Wave,
    /// The agent: Atelier's sparkle, the mark every AI entry point shares.
    Agent,
    /// Step back one edit: an arrow turning back on itself.
    Undo,
    /// Speed: a lightning bolt, for the agent's fast mode.
    Bolt,
    /// Step forward one edit: Undo's arrow, mirrored.
    Redo,
}

/// An element that draws `icon` at `size`, centred in whatever box it is given.
pub fn icon(icon: Icon, size: Pixels, color: Hsla) -> impl IntoElement + use<> {
    AtelierIcon::new(icon.source()).size(size).color(color)
}

/// Semantic tool identity, shared by chat logs and candidate activity summaries.
pub(crate) fn tool_icon(name: &str) -> Icon {
    let name = name.rsplit("__").next().unwrap_or(name);
    if name.contains("sing") || name.contains("vocal") || name.contains("lyric") {
        Icon::Microphone
    } else if name.contains("instrument") || name.contains("soundfont") {
        Icon::Keyboard
    } else if name.contains("effect") || name.contains("mix") || name.contains("volume") {
        Icon::Faders
    } else if name.contains("audio") || name.contains("render") || name.contains("export") {
        Icon::Wave
    } else if name.contains("play") || name.contains("transport") {
        Icon::Play
    } else if name.contains("note")
        || name.contains("phrase")
        || name.contains("harmon")
        || name.contains("compos")
        || name.contains("track")
        || name.contains("clip")
    {
        Icon::Notes
    } else if name.contains("inspect") || name.contains("diagnos") || name.starts_with("get_") {
        Icon::Inspector
    } else if name.contains("read") || name.contains("record") || name.contains("reference") {
        Icon::Log
    } else if name.contains("search") || name.contains("list") {
        Icon::Library
    } else {
        Icon::Knob
    }
}

impl Icon {
    /// Maps Auris's domain vocabulary onto Atelier's shared 24px icon family.
    ///
    /// Atelier deliberately keeps the set small so icon weight, optical centring and state
    /// treatment stay consistent across the editor and agent surfaces. Domain-specific names
    /// therefore resolve to established symbols; Auris's domain SVGs share that grid and reach
    /// the renderer as an [`IconSource`].
    pub fn source(self) -> IconSource {
        let name = match self {
            Self::Sound(family) => return IconSource::svg(sound_icon_path(family)),
            Self::Menu => AtelierIconName::Menu,
            Self::Punch => AtelierIconName::Punch,
            Self::Notes => AtelierIconName::Notes,
            Self::ToStart => AtelierIconName::SkipBack,
            Self::Play => AtelierIconName::Play,
            Self::Pause => AtelierIconName::Pause,
            Self::Stop => AtelierIconName::Stop,
            Self::Record => AtelierIconName::Record,
            Self::Loop => AtelierIconName::Loop,
            Self::Metronome => AtelierIconName::Metronome,
            Self::Plus => AtelierIconName::Plus,
            Self::ChevronUp => AtelierIconName::ChevronUp,
            Self::ChevronDown => AtelierIconName::ChevronDown,
            Self::ChevronRight => AtelierIconName::ChevronRight,
            Self::Cross => AtelierIconName::Close,
            Self::More => AtelierIconName::More,
            Self::Check => AtelierIconName::Check,
            Self::Library => AtelierIconName::Folder,
            Self::Faders => AtelierIconName::Sliders,
            Self::Knob => AtelierIconName::Knob,
            Self::Inspector => AtelierIconName::Info,
            Self::Log => AtelierIconName::File,
            Self::Keyboard => AtelierIconName::Keyboard,
            Self::Microphone => AtelierIconName::Mic,
            Self::Wave => AtelierIconName::Waveform,
            Self::Agent => AtelierIconName::Sparkles,
            Self::Undo => AtelierIconName::Undo,
            Self::Redo => AtelierIconName::Redo,
            Self::Bolt => AtelierIconName::Bolt,
        };
        name.into()
    }
}

/// Embedded monochrome SVG shared by library rows, map rows and the manual picker.
pub(crate) fn sound_icon_path(family: auris_session::SoundIcon) -> &'static str {
    sound_paths(family).0
}

/// The same drawing as [`sound_icon_path`], at the light weight the inspector shows it large.
///
/// The two files differ in `stroke-width` alone (2 on the icon, 1 on the artwork), so the
/// library row and the inspector show one instrument in one style; a test keeps them so.
pub(crate) fn sound_artwork_path(family: auris_session::SoundIcon) -> &'static str {
    sound_paths(family).1
}

fn sound_paths(family: auris_session::SoundIcon) -> (&'static str, &'static str) {
    use auris_session::SoundIcon;
    match family {
        SoundIcon::Sound => ("icons/sound-sound.svg", "sounds/sound-sound.svg"),
        SoundIcon::Piano => ("icons/sound-piano.svg", "sounds/sound-piano.svg"),
        SoundIcon::Organ => ("icons/sound-organ.svg", "sounds/sound-organ.svg"),
        SoundIcon::Guitar => ("icons/sound-guitar.svg", "sounds/sound-guitar.svg"),
        SoundIcon::Bass => ("icons/sound-bass.svg", "sounds/sound-bass.svg"),
        SoundIcon::Strings => ("icons/sound-strings.svg", "sounds/sound-strings.svg"),
        SoundIcon::Brass => ("icons/sound-brass.svg", "sounds/sound-brass.svg"),
        SoundIcon::Wind => ("icons/sound-wind.svg", "sounds/sound-wind.svg"),
        SoundIcon::Drum => ("icons/sound-drum.svg", "sounds/sound-drum.svg"),
        SoundIcon::Bell => ("icons/sound-bell.svg", "sounds/sound-bell.svg"),
        SoundIcon::Voice => ("icons/sound-voice.svg", "sounds/sound-voice.svg"),
        SoundIcon::Synth => ("icons/sound-synth.svg", "sounds/sound-synth.svg"),
        SoundIcon::Pad => ("icons/sound-pad.svg", "sounds/sound-pad.svg"),
        SoundIcon::Pluck => ("icons/sound-pluck.svg", "sounds/sound-pluck.svg"),
    }
}
