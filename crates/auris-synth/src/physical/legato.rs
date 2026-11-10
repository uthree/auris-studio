//! Fixed-size last-note-priority bookkeeping for a line played on one string or one pipe.

#[derive(Clone, Copy, Debug, Default)]
struct Key {
    count: u16,
    velocity: f32,
    order: u64,
}

/// Which keys are down and in what order they went down, so a released key can fall back to
/// the one still held under it.
#[derive(Clone, Debug)]
pub(super) struct Legato {
    keys: [Key; 128],
    order: u64,
    /// The voice carrying the line.
    pub(super) voice: Option<usize>,
}

impl Default for Legato {
    fn default() -> Self {
        Self {
            keys: [Key::default(); 128],
            order: 0,
            voice: None,
        }
    }
}

impl Legato {
    pub(super) fn clear(&mut self) {
        self.keys.fill(Key::default());
        self.order = 0;
        self.voice = None;
    }

    pub(super) fn note_on(&mut self, pitch: u8, velocity: f32) {
        if let Some(key) = self.keys.get_mut(usize::from(pitch)) {
            key.count = key.count.saturating_add(1);
            key.velocity = velocity;
            key.order = self.order;
            self.order = self.order.wrapping_add(1);
        }
    }

    /// Counts one release of `pitch`; `false` when that key was not down.
    pub(super) fn note_off(&mut self, pitch: u8) -> bool {
        let Some(key) = self.keys.get_mut(usize::from(pitch)) else {
            return false;
        };
        if key.count == 0 {
            return false;
        }
        key.count -= 1;
        true
    }

    /// The key most recently pressed of those still down.
    pub(super) fn last(&self) -> Option<(u8, f32)> {
        self.keys
            .iter()
            .enumerate()
            .filter(|(_, key)| key.count > 0)
            .max_by_key(|(_, key)| key.order)
            .map(|(pitch, key)| (pitch as u8, key.velocity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_released_key_falls_back_to_the_one_pressed_before_it() {
        let mut legato = Legato::default();
        legato.note_on(60, 0.5);
        legato.note_on(64, 0.8);
        assert_eq!(legato.last(), Some((64, 0.8)));
        assert!(legato.note_off(64));
        assert_eq!(legato.last(), Some((60, 0.5)));
        assert!(!legato.note_off(64), "a second off for one on is stale");
        assert!(legato.note_off(60));
        assert_eq!(legato.last(), None);
    }

    #[test]
    fn a_key_pressed_twice_is_down_until_it_is_released_twice() {
        let mut legato = Legato::default();
        legato.note_on(60, 0.5);
        legato.note_on(60, 0.6);
        assert!(legato.note_off(60));
        assert_eq!(legato.last(), Some((60, 0.6)));
        assert!(legato.note_off(60));
        assert_eq!(legato.last(), None);
    }
}
