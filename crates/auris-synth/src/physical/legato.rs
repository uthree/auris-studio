//! Fixed-size last-note-priority bookkeeping for the optional monophonic bow performer.

#[derive(Clone, Copy, Debug, Default)]
struct Key {
    count: u16,
    velocity: f32,
    order: u64,
}

#[derive(Clone, Debug)]
pub(super) struct Legato {
    keys: [Key; 128],
    order: u64,
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

    pub(super) fn last(&self) -> Option<(u8, f32)> {
        self.keys
            .iter()
            .enumerate()
            .filter(|(_, key)| key.count > 0)
            .max_by_key(|(_, key)| key.order)
            .map(|(pitch, key)| (pitch as u8, key.velocity))
    }
}
