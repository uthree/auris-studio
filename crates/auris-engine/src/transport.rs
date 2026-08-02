//! Playback position and looping.

/// The playhead and its loop region, in absolute timeline frames.
///
/// The engine owns one `Transport`; the renderer advances it after every block it fills, so the
/// position always describes the *next* frame to be rendered.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Transport {
    /// `true` while the timeline is rolling. Plugins keep processing when it is `false` so
    /// reverb and delay tails ring out after a stop.
    pub playing: bool,
    /// Position of the playhead in frames from the start of the timeline.
    pub position_frames: u64,
    /// Whether playback wraps around [`Self::loop_start_frames`]..[`Self::loop_end_frames`].
    pub loop_enabled: bool,
    /// First frame of the loop region.
    pub loop_start_frames: u64,
    /// One past the last frame of the loop region.
    pub loop_end_frames: u64,
}

impl Transport {
    /// A stopped transport parked at the start of the timeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// A transport that is already rolling from `position`.
    pub fn playing_from(position: u64) -> Self {
        Self {
            playing: true,
            position_frames: position,
            ..Self::default()
        }
    }

    /// `true` when the loop region is enabled, non-empty, and still ahead of the playhead.
    ///
    /// A playhead that has been dropped past the loop end (by seeking, say) plays on rather than
    /// being yanked backwards, which matches what every DAW does.
    pub fn loop_active(&self) -> bool {
        self.loop_enabled
            && self.loop_end_frames > self.loop_start_frames
            && self.position_frames < self.loop_end_frames
    }

    /// Length of the loop region in frames, or 0 when it is empty.
    pub fn loop_length(&self) -> u64 {
        self.loop_end_frames.saturating_sub(self.loop_start_frames)
    }

    /// Frames remaining until the loop end, or `None` when looping is not active.
    pub fn frames_to_loop_end(&self) -> Option<u64> {
        self.loop_active()
            .then(|| self.loop_end_frames - self.position_frames)
    }

    /// Moves the playhead without changing the play state.
    pub fn seek(&mut self, frames: u64) {
        self.position_frames = frames;
    }

    /// Sets the loop region.
    pub fn set_loop(&mut self, enabled: bool, start: u64, end: u64) {
        self.loop_enabled = enabled;
        self.loop_start_frames = start;
        self.loop_end_frames = end;
    }

    /// Advances the playhead by `frames`, wrapping at the loop end.
    ///
    /// The modulo keeps the result correct even when `frames` covers more than one whole loop,
    /// which a large offline block or a very short loop region can do. A stopped transport does
    /// not move.
    pub fn advance(&mut self, frames: u64) {
        if !self.playing || frames == 0 {
            return;
        }
        let next = self.position_frames.saturating_add(frames);
        if self.loop_active() && next >= self.loop_end_frames {
            let length = self.loop_end_frames - self.loop_start_frames;
            let past_end = next - self.loop_end_frames;
            self.position_frames = self.loop_start_frames + past_end % length;
        } else {
            self.position_frames = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looping(position: u64) -> Transport {
        Transport {
            playing: true,
            position_frames: position,
            loop_enabled: true,
            loop_start_frames: 1_000,
            loop_end_frames: 5_000,
        }
    }

    #[test]
    fn a_stopped_transport_does_not_move() {
        let mut transport = Transport::new();
        transport.advance(512);
        assert_eq!(transport.position_frames, 0);
    }

    #[test]
    fn advancing_inside_the_loop_is_linear() {
        let mut transport = looping(1_000);
        transport.advance(512);
        assert_eq!(transport.position_frames, 1_512);
    }

    #[test]
    fn a_block_straddling_the_loop_end_wraps_by_the_overshoot() {
        let mut transport = looping(4_900);
        transport.advance(512);
        // 4900 + 512 = 5412, which is 412 past the loop end, so 1000 + 412.
        assert_eq!(transport.position_frames, 1_412);
    }

    #[test]
    fn landing_exactly_on_the_loop_end_wraps_to_the_loop_start() {
        let mut transport = looping(4_500);
        transport.advance(500);
        assert_eq!(transport.position_frames, 1_000);
    }

    #[test]
    fn a_block_longer_than_the_loop_wraps_by_the_modulo() {
        let mut transport = looping(1_000);
        // The loop is 4000 frames long; 10_500 frames is two whole loops plus 2_500.
        transport.advance(10_500);
        assert_eq!(transport.position_frames, 3_500);
    }

    #[test]
    fn a_playhead_past_the_loop_end_keeps_rolling() {
        let mut transport = looping(9_000);
        assert!(!transport.loop_active());
        transport.advance(1_000);
        assert_eq!(transport.position_frames, 10_000);
    }

    #[test]
    fn an_empty_loop_region_never_wraps() {
        let mut transport = Transport {
            playing: true,
            position_frames: 0,
            loop_enabled: true,
            loop_start_frames: 2_000,
            loop_end_frames: 2_000,
        };
        transport.advance(4_000);
        assert_eq!(transport.position_frames, 4_000);
        assert_eq!(transport.loop_length(), 0);
    }

    #[test]
    fn frames_to_loop_end_reports_the_split_point() {
        let transport = looping(4_900);
        assert_eq!(transport.frames_to_loop_end(), Some(100));
    }
}
