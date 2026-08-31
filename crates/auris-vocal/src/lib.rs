//! The singer's language: lyrics to IPA phonemes, and notes to the frames a voice model is fed.
//!
//! A [singer track](auris_core::SingerTrack) stores what a person wrote — a lyric on each note —
//! and what the voice model is given: a phoneme list per note, and ultimately three sequences
//! sampled on a fixed clock. This crate is the whole of the translation, in two halves:
//!
//! * **Text to phonemes.** [`lyric_phonemes`] turns one note's lyric into IPA. Lyrics written in
//!   kana go through a built-in table ([`kana`]) and need nothing installed; anything else
//!   Japanese — kanji, digits, mixed text — goes through [`JapaneseDictionary`], which wraps
//!   [jpreprocess](https://github.com/jpreprocess/jpreprocess) over a dictionary folder the user
//!   provides at run time. Other languages are written by editing a note's phonemes directly;
//!   the phoneme vocabulary is IPA precisely so that nothing here has to be rebuilt when a
//!   voice model learns another language.
//! * **Notes to frames.** [`render_frames`] samples a track's notes, bends and expression onto
//!   the model's clock: one phoneme id, one pitch and one energy per
//!   [`frame_hop`](auris_core::SingerTrack::frame_hop) seconds.
//!
//! The phoneme timing rules are deliberately simple and live in [`frames`]; the measurements
//! that justify them are in that module's tests. What a phoneme *is* — a token, its class, the
//! silence token — is [`phoneme`]'s business, shared by both halves.

#![warn(missing_docs)]

pub mod frames;
pub mod g2p;
pub mod kana;
pub mod openjtalk;
pub mod phoneme;

pub use frames::{SingerFrames, phoneme_layout, render_frames};
pub use g2p::{JapaneseDictionary, VocalError, lyric_phonemes};
pub use kana::{kana_phonemes, split_kana_lyric, split_kana_moras};
pub use phoneme::{SILENCE, is_syllabic, is_voiceless, phoneme_moras};
