//! Where the loaded fonts live.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use auris_core::SoundFontId;
use rustysynth::SoundFont;

/// A font bank shared between the session that fills it and the samplers that read it.
///
/// [`crate::Sampler`] instances are built by a factory the plugin registry owns, and a factory
/// takes no arguments — so the only way sample data reaches an instrument is for the factory's
/// closure to have captured one of these.
pub type SharedSoundFonts = Arc<SoundFontBank>;

/// Every SoundFont this session has loaded, by the id the project refers to it with.
///
/// The document stores paths; this stores what those paths decoded to, exactly as
/// [`AudioSourceBank`](auris_core::AudioSourceBank) does for audio files. A font runs to
/// hundreds of megabytes, so it is loaded once and every track that plays it shares the same
/// `Arc`.
///
/// # Threads
///
/// Shared by `Arc` and mutated through `&self`, because the samplers reading it are handed out
/// by a factory that cannot be given `&mut` to anything. The lock is taken when a font is
/// imported and when a sampler is prepared — both on ordinary threads. **Nothing here is called
/// from the audio thread**: a sampler resolves its font in `prepare` and holds an `Arc` for as
/// long as it lives, so `process` never touches the bank.
#[derive(Default)]
pub struct SoundFontBank {
    fonts: RwLock<BTreeMap<SoundFontId, Arc<SoundFont>>>,
}

impl SoundFontBank {
    /// An empty bank.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty bank, ready to be shared.
    pub fn shared() -> SharedSoundFonts {
        Arc::new(Self::new())
    }

    /// Stores a font under the id the project knows it by, replacing any font already there.
    pub fn insert(&self, id: SoundFontId, font: Arc<SoundFont>) {
        self.write().insert(id, font);
    }

    /// The font with this id, if it has been loaded.
    pub fn get(&self, id: SoundFontId) -> Option<Arc<SoundFont>> {
        self.read().get(&id).cloned()
    }

    /// The lowest-numbered font, which is the first one imported.
    ///
    /// What a sampler falls back to when its track names no preset at all — a sampler that has
    /// not been told which sound to make is more useful making the first one available than
    /// making none.
    pub fn first(&self) -> Option<(SoundFontId, Arc<SoundFont>)> {
        self.read()
            .iter()
            .next()
            .map(|(id, font)| (*id, Arc::clone(font)))
    }

    /// `true` when a font with this id is loaded.
    pub fn contains(&self, id: SoundFontId) -> bool {
        self.read().contains_key(&id)
    }

    /// Forgets one font. Samplers already holding it keep playing until they are prepared again.
    pub fn remove(&self, id: SoundFontId) -> Option<Arc<SoundFont>> {
        self.write().remove(&id)
    }

    /// Forgets every font.
    pub fn clear(&self) {
        self.write().clear();
    }

    /// How many fonts are loaded.
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// `true` when nothing is loaded.
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Every loaded id, in order.
    pub fn ids(&self) -> Vec<SoundFontId> {
        self.read().keys().copied().collect()
    }

    /// Reads through the lock, ignoring poisoning.
    ///
    /// A panic elsewhere in the process should not turn every later font lookup into a second
    /// panic: what this guards is a plain map, so the worst a poisoned lock can mean here is
    /// that one insertion did not finish, and a missing font is already a case every caller
    /// handles.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<SoundFontId, Arc<SoundFont>>> {
        self.fonts.read().unwrap_or_else(|error| error.into_inner())
    }

    /// Writes through the lock, ignoring poisoning. See [`Self::read`].
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<SoundFontId, Arc<SoundFont>>> {
        self.fonts
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl std::fmt::Debug for SoundFontBank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the fonts themselves: printing a bank should not print a megabyte of sample
        // headers into somebody's log.
        f.debug_struct("SoundFontBank")
            .field("fonts", &self.ids())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_bank_has_nothing_to_fall_back_to() {
        let bank = SoundFontBank::new();
        assert!(bank.is_empty());
        assert_eq!(bank.len(), 0);
        assert!(bank.first().is_none());
        assert!(bank.get(SoundFontId(1)).is_none());
        assert!(!bank.contains(SoundFontId(1)));
        assert!(bank.ids().is_empty());
    }

    #[test]
    fn the_debug_output_names_ids_rather_than_fonts() {
        // A font holds every sample in the file. Whatever else this prints, it must not print
        // that, so the ids are the whole of it.
        let bank = SoundFontBank::new();
        assert_eq!(format!("{bank:?}"), "SoundFontBank { fonts: [] }");
    }
}
