//! Shared icon vocabulary backed by Atelier's monochrome SVG assets.
//!
//! Atelier keeps familiar controls in one 24px SVG family; Auris's
//! few domain-specific marks live beside the provider assets and use the same stroke geometry.

use atelier::{Icon as AtelierIcon, IconName as AtelierIconName};
use gpui::{Hsla, IntoElement, Pixels, div, prelude::*, svg};

/// An icon the UI can draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Return the playhead to the start.
    ToStart,
    /// Start playback.
    Play,
    /// Pause playback.
    Pause,
    /// Audible preview output.
    Volume,
    /// Muted preview output.
    VolumeMuted,
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
    /// Step forward one edit: Undo's arrow, mirrored. Atelier has no redo glyph, so this one is
    /// drawn here with the same stroke.
    Redo,
}

/// An element that draws `icon` at `size`, centred in whatever box it is given.
pub fn icon(icon: Icon, size: Pixels, color: Hsla) -> impl IntoElement + use<> {
    if let Some(path) = icon.extension_path() {
        div()
            .size(size)
            .text_color(color)
            .child(svg().path(path).size_full().text_color(color))
            .into_any_element()
    } else {
        AtelierIcon::new(icon.atelier_name())
            .size(size)
            .color(color)
            .into_any_element()
    }
}

impl Icon {
    fn extension_path(self) -> Option<&'static str> {
        match self {
            Self::Volume => Some("icons/auris-volume.svg"),
            Self::VolumeMuted => Some("icons/auris-volume-muted.svg"),
            Self::Punch => Some("icons/auris-punch.svg"),
            Self::Notes => Some("icons/auris-notes.svg"),
            Self::Keyboard => Some("icons/auris-keyboard.svg"),
            Self::Knob => Some("icons/auris-knob.svg"),
            Self::Redo => Some("icons/auris-redo.svg"),
            Self::Bolt => Some("icons/auris-bolt.svg"),
            _ => None,
        }
    }

    /// Maps Auris's domain vocabulary onto Atelier's shared 24px icon family.
    ///
    /// Atelier deliberately keeps the set small so icon weight, optical centring and state
    /// treatment stay consistent across the editor and agent surfaces. Domain-specific names
    /// therefore resolve to the nearest established symbol instead of introducing another drawn
    /// glyph with a different visual grammar.
    fn atelier_name(self) -> AtelierIconName {
        match self {
            Self::ToStart => AtelierIconName::SkipBack,
            Self::Play => AtelierIconName::Play,
            Self::Pause => AtelierIconName::Pause,
            Self::Volume | Self::VolumeMuted => AtelierIconName::Music,
            Self::Stop => AtelierIconName::Stop,
            Self::Record => AtelierIconName::Record,
            Self::Loop => AtelierIconName::Loop,
            Self::Punch => AtelierIconName::Sliders,
            Self::Metronome => AtelierIconName::Metronome,
            Self::Plus => AtelierIconName::Plus,
            Self::ChevronUp => AtelierIconName::ChevronUp,
            Self::ChevronDown => AtelierIconName::ChevronDown,
            Self::ChevronRight => AtelierIconName::ChevronRight,
            Self::Cross => AtelierIconName::Close,
            Self::More => AtelierIconName::More,
            Self::Check => AtelierIconName::Check,
            Self::Library => AtelierIconName::Folder,
            Self::Notes => AtelierIconName::Music,
            Self::Faders | Self::Knob => AtelierIconName::Sliders,
            Self::Inspector => AtelierIconName::Info,
            Self::Log => AtelierIconName::File,
            Self::Keyboard => AtelierIconName::Music,
            Self::Microphone => AtelierIconName::Mic,
            Self::Wave => AtelierIconName::Waveform,
            Self::Agent => AtelierIconName::Sparkles,
            Self::Undo => AtelierIconName::Undo,
            Self::Redo => AtelierIconName::Undo,
            Self::Bolt => AtelierIconName::Sparkles,
        }
    }
}
