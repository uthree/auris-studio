//! What kind of music a song is, and what it is for.
//!
//! Two independent axes. A genre says how the music speaks (its form, groove, harmony and
//! sounds); a usage says what the music has to do (be a song, loop under a game, sit under a
//! scene). A chiptune can be a song or a loop, and a loop can be orchestral or chiptune, so
//! neither is folded into the other.
//!
//! The document holds the pair because the checks that read a score need it: what a chorus
//! must do, whether an intro is required and how loud a mix should be all depend on it, and
//! those checks see the document, not the conversation that wrote it.

use serde::{Deserialize, Serialize};

/// The genre a song is written in, from a list drawn from streaming services' top level.
///
/// A genre is here only when its writing differs from every other one's: in form, groove,
/// harmony, melody or mix. Two genres that would be written the same way are one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Genre {
    /// J-pop: verse, pre-chorus and chorus (Aメロ, Bメロ, サビ), a chorus that lifts.
    JPop,
    /// Anime songs and Vocaloid-style songs: fast, dense, wide-ranged, with hits and key changes.
    AnimeVocaloid,
    /// Rock and band music: eight-beat drums, power chords, a riff carrying the identity.
    Rock,
    /// City pop and AOR: sevenths and ninths, ii–V motion, a sixteenth-note groove.
    CityPop,
    /// R&B and neo-soul: swung sixteenths, extended harmony, a laid-back feel.
    RnbNeoSoul,
    /// Hip-hop and lo-fi: a form built on a four- or eight-bar loop, boom-bap swing.
    HipHopLofi,
    /// Dance and electronic: build and drop instead of verse and chorus, four on the floor.
    DanceElectronic,
    /// Jazz: swing, ii–V progressions, walking bass, head–solos–head.
    Jazz,
    /// Orchestral and film scoring: no drum kit, families handing material on, dynamics as form.
    Orchestral,
    /// Ambient and healing music: little or no pulse, slow harmony, texture as the subject.
    Ambient,
    /// Folk and acoustic: strummed guitars, plain harmony, a small band, a story told.
    FolkAcoustic,
    /// Latin and bossa nova: clave and bossa rhythms, seventh chords.
    Latin,
    /// Chiptune: few voices at once, chords as arpeggios, no reverb, exact on the grid.
    Chiptune,
}

impl Genre {
    /// Every genre, in the order a list presents them.
    pub const ALL: [Self; 13] = [
        Self::JPop,
        Self::AnimeVocaloid,
        Self::Rock,
        Self::CityPop,
        Self::RnbNeoSoul,
        Self::HipHopLofi,
        Self::DanceElectronic,
        Self::Jazz,
        Self::Orchestral,
        Self::Ambient,
        Self::FolkAcoustic,
        Self::Latin,
        Self::Chiptune,
    ];

    /// The stored identifier, as it appears in a saved file and a tool argument.
    pub fn id(self) -> &'static str {
        match self {
            Self::JPop => "j_pop",
            Self::AnimeVocaloid => "anime_vocaloid",
            Self::Rock => "rock",
            Self::CityPop => "city_pop",
            Self::RnbNeoSoul => "rnb_neo_soul",
            Self::HipHopLofi => "hip_hop_lofi",
            Self::DanceElectronic => "dance_electronic",
            Self::Jazz => "jazz",
            Self::Orchestral => "orchestral",
            Self::Ambient => "ambient",
            Self::FolkAcoustic => "folk_acoustic",
            Self::Latin => "latin",
            Self::Chiptune => "chiptune",
        }
    }
}

/// What the music has to do, whatever its genre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Usage {
    /// A song heard for itself, start to finish, with an intro and an ending.
    Song,
    /// Background music that loops, as under a game: the end runs straight into the start.
    LoopBgm,
    /// A cue under a scene or a video: fitted to a length, entering and leaving with it.
    Cue,
}

impl Usage {
    /// Every usage, in the order a list presents them.
    pub const ALL: [Self; 3] = [Self::Song, Self::LoopBgm, Self::Cue];

    /// The stored identifier, as it appears in a saved file and a tool argument.
    pub fn id(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::LoopBgm => "loop_bgm",
            Self::Cue => "cue",
        }
    }
}

/// A song's genre and usage, either of which may be left undecided.
///
/// Undecided means the general guidance and today's checks apply, so a song written before
/// either was chosen is read exactly as it was.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Style {
    /// The genre, when one has been chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<Genre>,
    /// The usage, when one has been chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

impl Style {
    /// Whether neither axis has been chosen.
    pub fn is_unset(&self) -> bool {
        self.genre.is_none() && self.usage.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_is_the_serialized_name() {
        for genre in Genre::ALL {
            assert_eq!(serde_json::to_value(genre).unwrap(), genre.id());
        }
        for usage in Usage::ALL {
            assert_eq!(serde_json::to_value(usage).unwrap(), usage.id());
        }
    }

    #[test]
    fn an_unset_style_writes_nothing_and_reads_back_unset() {
        assert_eq!(serde_json::to_string(&Style::default()).unwrap(), "{}");
        let style: Style = serde_json::from_str("{}").unwrap();
        assert!(style.is_unset());
    }
}
