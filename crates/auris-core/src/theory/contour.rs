//! What a sung syllable asks of the melodic line.
//!
//! A lyric spoken aloud already has a shape — syllables the voice rises into, syllables it
//! falls away from — and a melody that contradicts that shape sings the words against their
//! own meaning. [`Contour`] is that shape reduced to the one question a melody writer asks at
//! every note: *which way may the line move to arrive here?*
//!
//! Deliberately not Japanese. Today the only producer is Japanese pitch accent
//! (`auris-vocal`'s accent analysis, following Orpheus's reading of the Tokyo dialect), and
//! the only consumer is the vocal melody writer (`auris-compose`). But the vocabulary between
//! them names no language and no algorithm, so another language's prosody — English stress,
//! Mandarin tone — or another writer entirely can join either end without the other noticing.
//! That is why it lives here, beside the keys and chords the same two crates already share.

/// The constraint a syllable places on the melodic step that *arrives* at it.
///
/// A constraint on the step rather than on the note: prosody is relative — an accent is heard
/// as a fall from the mora before, wherever both sit — and a melody writer deciding pitch `n`
/// against pitch `n - 1` wants the rule in exactly those terms.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Contour {
    /// The line must move up onto this syllable.
    Rise,
    /// The line must move down onto this syllable.
    Fall,
    /// The line may rise or stay, but must not fall — a fall here would be heard as an
    /// accent the word does not have.
    NoFall,
    /// The prosody says nothing; the melody is free.
    Free,
}
