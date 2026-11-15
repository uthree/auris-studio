//! Fixed-size last-note-priority bookkeeping for a line played on one string or one pipe.

#[derive(Clone, Copy, Debug, Default)]
struct Key {
    count: u16,
    velocity: f32,
    /// Where the key's most recent note-on asked to sound, which a tuning may put anywhere.
    pitch: f32,
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

/// A key still down, as the line falls back to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HeldKey {
    /// Which key.
    pub(super) key: u8,
    /// Where it sounds.
    pub(super) pitch: f32,
    /// How hard it was pressed.
    pub(super) velocity: f32,
}

impl Legato {
    pub(super) fn clear(&mut self) {
        self.keys.fill(Key::default());
        self.order = 0;
        self.voice = None;
    }

    pub(super) fn note_on(&mut self, key: u8, pitch: f32, velocity: f32) {
        if let Some(down) = self.keys.get_mut(usize::from(key)) {
            down.count = down.count.saturating_add(1);
            down.velocity = velocity;
            down.pitch = pitch;
            down.order = self.order;
            self.order = self.order.wrapping_add(1);
        }
    }

    /// Counts one release of `key`; `false` when that key was not down.
    pub(super) fn note_off(&mut self, key: u8) -> bool {
        let Some(down) = self.keys.get_mut(usize::from(key)) else {
            return false;
        };
        if down.count == 0 {
            return false;
        }
        down.count -= 1;
        true
    }

    /// The key most recently pressed of those still down.
    pub(super) fn last(&self) -> Option<HeldKey> {
        self.keys
            .iter()
            .enumerate()
            .filter(|(_, down)| down.count > 0)
            .max_by_key(|(_, down)| down.order)
            .map(|(key, down)| HeldKey {
                key: key as u8,
                pitch: down.pitch,
                velocity: down.velocity,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(key: u8, velocity: f32) -> Option<HeldKey> {
        Some(HeldKey {
            key,
            pitch: f32::from(key),
            velocity,
        })
    }

    #[test]
    fn a_released_key_falls_back_to_the_one_pressed_before_it() {
        let mut legato = Legato::default();
        legato.note_on(60, 60.0, 0.5);
        legato.note_on(64, 64.0, 0.8);
        assert_eq!(legato.last(), held(64, 0.8));
        assert!(legato.note_off(64));
        assert_eq!(legato.last(), held(60, 0.5));
        assert!(!legato.note_off(64), "a second off for one on is stale");
        assert!(legato.note_off(60));
        assert_eq!(legato.last(), None);
    }

    #[test]
    fn a_key_pressed_twice_is_down_until_it_is_released_twice() {
        let mut legato = Legato::default();
        legato.note_on(60, 60.0, 0.5);
        legato.note_on(60, 60.0, 0.6);
        assert!(legato.note_off(60));
        assert_eq!(legato.last(), held(60, 0.6));
        assert!(legato.note_off(60));
        assert_eq!(legato.last(), None);
    }

    #[test]
    fn the_line_falls_back_to_where_the_key_underneath_sounds() {
        let mut legato = Legato::default();
        legato.note_on(61, 60.39, 0.5);
        legato.note_on(62, 60.77, 0.5);
        legato.note_off(62);
        assert_eq!(legato.last().map(|down| down.pitch), Some(60.39));
    }
}
