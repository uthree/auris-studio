//! Voice bookkeeping.
//!
//! [`VoiceAllocator`] owns nothing but the *state* of a fixed pool of voices: which note each
//! one is playing, whether the key is still down, how loud it is and how long ago it started.
//! The DSP for a voice lives in the instrument, in a parallel array of the same length. Keeping
//! the two apart means all three built-in instruments share one stealing policy without the
//! allocator knowing anything about oscillators or filters.
//!
//! The pool is sized once in `prepare`; nothing here allocates afterwards. Results that would
//! naturally be a list of indices are returned as a [`VoiceMask`] bitset instead of a `Vec`,
//! which is what keeps `note_off` and friends usable from the audio thread.

/// Largest pool the allocator supports, set by the width of [`VoiceMask`].
pub const MAX_VOICES: usize = 64;

/// Lifecycle state of one voice.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum VoiceState {
    /// Silent and available.
    #[default]
    Free,
    /// Sounding with the key still down.
    Held,
    /// Sounding out its release; still audible, but first in line to be stolen.
    Released,
}

/// Bookkeeping for one voice in the pool.
#[derive(Copy, Clone, Debug, Default)]
pub struct VoiceSlot {
    /// What the voice is doing.
    pub state: VoiceState,
    /// MIDI note the voice was started with.
    pub pitch: u8,
    /// Velocity the voice was started with, 0..1.
    pub velocity: f32,
    /// Most recent output level, published by the instrument and used to pick which voice to
    /// steal.
    pub level: f32,
    /// Monotonic counter assigned at note on; smaller means older.
    pub age: u64,
}

/// A set of voice indices, one bit each.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceMask(u64);

impl VoiceMask {
    /// The empty set.
    pub const fn empty() -> Self {
        VoiceMask(0)
    }

    /// Adds an index. Indices at or above [`MAX_VOICES`] are ignored.
    pub fn insert(&mut self, index: usize) {
        if index < MAX_VOICES {
            self.0 |= 1u64 << index;
        }
    }

    /// `true` when `index` is in the set.
    pub fn contains(self, index: usize) -> bool {
        index < MAX_VOICES && self.0 & (1u64 << index) != 0
    }

    /// Number of indices in the set.
    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// `true` when the set holds no indices.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Iterates the indices in ascending order.
    pub fn iter(self) -> VoiceMaskIter {
        VoiceMaskIter(self.0)
    }
}

impl IntoIterator for VoiceMask {
    type Item = usize;
    type IntoIter = VoiceMaskIter;

    fn into_iter(self) -> VoiceMaskIter {
        self.iter()
    }
}

/// Ascending iterator over the indices in a [`VoiceMask`].
#[derive(Copy, Clone, Debug)]
pub struct VoiceMaskIter(u64);

impl Iterator for VoiceMaskIter {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.0 == 0 {
            return None;
        }
        let index = self.0.trailing_zeros() as usize;
        self.0 &= self.0 - 1;
        Some(index)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.0.count_ones() as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for VoiceMaskIter {}

/// Which voice a note on landed on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VoiceAssignment {
    /// Index into the pool, and into the instrument's parallel voice array.
    pub index: usize,
    /// `true` when the voice was still sounding and had to be taken over. Instruments use this
    /// to decide whether they may reset oscillator phase: doing so on a sounding voice would
    /// click.
    pub stolen: bool,
}

/// A fixed pool of voices with a steal-the-quietest policy.
#[derive(Clone, Debug, Default)]
pub struct VoiceAllocator {
    slots: Vec<VoiceSlot>,
    next_age: u64,
}

impl VoiceAllocator {
    /// An allocator with no voices yet; call [`VoiceAllocator::prepare`] before use.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sizes the pool. This is the only method that allocates, so it belongs in the
    /// instrument's `prepare`. The count is clamped to `1..=MAX_VOICES`.
    pub fn prepare(&mut self, voices: usize) {
        let voices = voices.clamp(1, MAX_VOICES);
        self.slots.clear();
        self.slots.resize(voices, VoiceSlot::default());
        self.next_age = 0;
    }

    /// Number of voices in the pool.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Every slot, for instruments that want to inspect the pool.
    pub fn slots(&self) -> &[VoiceSlot] {
        &self.slots
    }

    /// One slot, or `None` when out of range.
    pub fn slot(&self, index: usize) -> Option<&VoiceSlot> {
        self.slots.get(index)
    }

    /// Voices that are not free.
    pub fn active_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state != VoiceState::Free)
            .count()
    }

    /// Indices of every voice that is not free.
    pub fn active_mask(&self) -> VoiceMask {
        let mut mask = VoiceMask::empty();
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.state != VoiceState::Free {
                mask.insert(index);
            }
        }
        mask
    }

    /// Claims a voice for a new note.
    ///
    /// A free voice is used when there is one. Otherwise the quietest voice is taken, with
    /// released voices preferred over held ones and the oldest breaking ties — stealing what is
    /// least audible is what makes running out of voices sound like a missing tail rather than
    /// a dropped note. Returns `None` only when the pool is empty.
    pub fn note_on(&mut self, pitch: u8, velocity: f32) -> Option<VoiceAssignment> {
        if self.slots.is_empty() {
            return None;
        }

        let free = self
            .slots
            .iter()
            .position(|slot| slot.state == VoiceState::Free);
        let (index, stolen) = match free {
            Some(index) => (index, false),
            None => (self.steal_candidate(), true),
        };

        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        if let Some(slot) = self.slots.get_mut(index) {
            slot.state = VoiceState::Held;
            slot.pitch = pitch;
            slot.velocity = velocity;
            slot.level = velocity;
            slot.age = age;
        }
        Some(VoiceAssignment { index, stolen })
    }

    /// Releases the oldest held voice playing `pitch`, returning which one it was.
    ///
    /// One voice, not every voice at that pitch: two overlapping notes on the same pitch are two
    /// notes, and the arrangement schedules an on and an off for each of them. Releasing them all
    /// would let the first note's off cut the second note short, which is audible whenever a part
    /// holds a pedal note under a line that touches it. Pairing each off with a single voice, and
    /// taking the oldest first, matches the offs to the ons in the order they arrived.
    ///
    /// The mask is empty when nothing at that pitch is held, which is what an off with no
    /// matching on looks like.
    pub fn note_off(&mut self, pitch: u8) -> VoiceMask {
        let mut oldest: Option<usize> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.state != VoiceState::Held || slot.pitch != pitch {
                continue;
            }
            if oldest.is_none_or(|best| slot.age < self.slots[best].age) {
                oldest = Some(index);
            }
        }
        let mut mask = VoiceMask::empty();
        if let Some(index) = oldest {
            self.slots[index].state = VoiceState::Released;
            mask.insert(index);
        }
        mask
    }

    /// Releases every held voice, returning every voice that is still sounding.
    ///
    /// The returned mask covers released voices too, so an all-sound-off can silence them all
    /// in one pass while an all-notes-off simply ignores the ones already fading.
    pub fn release_all(&mut self) -> VoiceMask {
        let mut mask = VoiceMask::empty();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == VoiceState::Held {
                slot.state = VoiceState::Released;
            }
            if slot.state != VoiceState::Free {
                mask.insert(index);
            }
        }
        mask
    }

    /// Marks a voice free once its envelope has finished.
    pub fn retire(&mut self, index: usize) {
        if let Some(slot) = self.slots.get_mut(index) {
            slot.state = VoiceState::Free;
            slot.level = 0.0;
        }
    }

    /// Publishes a voice's current output level for the stealing policy.
    pub fn set_level(&mut self, index: usize, level: f32) {
        if let Some(slot) = self.slots.get_mut(index) {
            slot.level = if level.is_finite() { level.abs() } else { 0.0 };
        }
    }

    /// Frees every voice without touching the pool size.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = VoiceSlot::default();
        }
        self.next_age = 0;
    }

    fn steal_candidate(&self) -> usize {
        let mut best = 0usize;
        for index in 1..self.slots.len() {
            let (Some(candidate), Some(current)) = (self.slots.get(index), self.slots.get(best))
            else {
                break;
            };
            let candidate_rank = steal_rank(candidate.state);
            let current_rank = steal_rank(current.state);
            let better = if candidate_rank != current_rank {
                candidate_rank < current_rank
            } else if candidate.level < current.level {
                true
            } else if candidate.level > current.level {
                false
            } else {
                candidate.age < current.age
            };
            if better {
                best = index;
            }
        }
        best
    }
}

/// Lower ranks are stolen first.
fn steal_rank(state: VoiceState) -> u8 {
    match state {
        VoiceState::Free => 0,
        VoiceState::Released => 1,
        VoiceState::Held => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_is_sized_once_and_clamped() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(0);
        assert_eq!(allocator.capacity(), 1);
        allocator.prepare(1_000);
        assert_eq!(allocator.capacity(), MAX_VOICES);
        allocator.prepare(8);
        assert_eq!(allocator.capacity(), 8);
        assert_eq!(allocator.active_count(), 0);
    }

    #[test]
    fn free_voices_are_handed_out_in_order() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(4);
        for expected in 0..4 {
            let assignment = allocator.note_on(60 + expected as u8, 1.0).unwrap();
            assert_eq!(assignment.index, expected);
            assert!(!assignment.stolen);
        }
        assert_eq!(allocator.active_count(), 4);
    }

    #[test]
    fn note_off_releases_only_the_matching_pitch() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(4);
        allocator.note_on(60, 1.0);
        allocator.note_on(64, 1.0);

        let released = allocator.note_off(60);
        assert_eq!(released.len(), 1);
        assert!(released.contains(0));
        assert_eq!(allocator.slot(1).unwrap().state, VoiceState::Held);
        // Still audible until the instrument retires them.
        assert_eq!(allocator.active_count(), 2);
        assert!(allocator.note_off(60).is_empty());
    }

    #[test]
    fn each_note_off_releases_one_voice_oldest_first() {
        // Two overlapping notes on one pitch are two notes. The first off must release only the
        // first of them, or a pedal note would be cut short by the line that crosses it.
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(4);
        allocator.note_on(60, 1.0);
        allocator.note_on(64, 1.0);
        allocator.note_on(60, 1.0);

        let first = allocator.note_off(60);
        assert_eq!(first.len(), 1);
        assert!(first.contains(0), "the older of the two should go first");
        assert_eq!(allocator.slot(2).unwrap().state, VoiceState::Held);

        let second = allocator.note_off(60);
        assert!(second.contains(2));
        assert!(
            allocator.note_off(60).is_empty(),
            "a third off has nothing left to pair with"
        );
    }

    #[test]
    fn retiring_a_voice_makes_it_reusable() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(2);
        allocator.note_on(60, 1.0);
        allocator.note_on(62, 1.0);
        allocator.note_off(60);
        allocator.retire(0);
        assert_eq!(allocator.active_count(), 1);
        let assignment = allocator.note_on(67, 1.0).unwrap();
        assert_eq!(assignment.index, 0);
        assert!(!assignment.stolen);
    }

    #[test]
    fn a_full_pool_steals_the_quietest_released_voice() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(3);
        allocator.note_on(60, 1.0);
        allocator.note_on(62, 1.0);
        allocator.note_on(64, 1.0);
        allocator.note_off(62);
        allocator.set_level(0, 0.05);
        allocator.set_level(1, 0.4);
        allocator.set_level(2, 0.9);

        // Voice 0 is quieter, but voice 1 has been let go, so it goes first.
        let assignment = allocator.note_on(67, 1.0).unwrap();
        assert_eq!(assignment.index, 1);
        assert!(assignment.stolen);
        assert_eq!(allocator.slot(1).unwrap().pitch, 67);
    }

    #[test]
    fn with_everything_held_the_quietest_is_stolen() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(3);
        for pitch in [60u8, 62, 64] {
            allocator.note_on(pitch, 1.0);
        }
        allocator.set_level(0, 0.8);
        allocator.set_level(1, 0.9);
        allocator.set_level(2, 0.02);
        let assignment = allocator.note_on(67, 1.0).unwrap();
        assert_eq!(assignment.index, 2);
        assert!(assignment.stolen);
    }

    #[test]
    fn equally_loud_voices_are_stolen_oldest_first() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(3);
        for pitch in [60u8, 62, 64] {
            allocator.note_on(pitch, 0.5);
        }
        let first = allocator.note_on(67, 0.5).unwrap();
        assert_eq!(first.index, 0);
        let second = allocator.note_on(69, 0.5).unwrap();
        assert_eq!(second.index, 1);
    }

    #[test]
    fn release_all_reports_every_sounding_voice() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(4);
        allocator.note_on(60, 1.0);
        allocator.note_on(62, 1.0);
        allocator.note_off(62);
        let mask = allocator.release_all();
        assert_eq!(mask.len(), 2);
        assert!(
            allocator
                .slots()
                .iter()
                .take(2)
                .all(|s| s.state == VoiceState::Released)
        );
        allocator.clear();
        assert_eq!(allocator.active_count(), 0);
        assert!(allocator.active_mask().is_empty());
    }

    #[test]
    fn a_mask_iterates_in_ascending_order() {
        let mut mask = VoiceMask::empty();
        for index in [7usize, 0, 63, 31] {
            mask.insert(index);
        }
        mask.insert(MAX_VOICES);
        assert_eq!(mask.iter().collect::<Vec<_>>(), vec![0, 7, 31, 63]);
        assert_eq!(mask.len(), 4);
        assert!(!mask.contains(MAX_VOICES));
    }

    #[test]
    fn a_non_finite_level_does_not_poison_the_policy() {
        let mut allocator = VoiceAllocator::new();
        allocator.prepare(2);
        allocator.note_on(60, 1.0);
        allocator.note_on(62, 1.0);
        allocator.set_level(0, f32::NAN);
        allocator.set_level(1, 0.5);
        assert_eq!(allocator.slot(0).unwrap().level, 0.0);
        assert_eq!(allocator.note_on(64, 1.0).unwrap().index, 0);
    }
}
