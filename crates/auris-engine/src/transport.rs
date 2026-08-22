//! Playback position and looping.

/// A count-in the transport is part way through.
///
/// The playhead does not move while one is running and nothing in the arrangement sounds, so a
/// count-in is not a stretch of the timeline: it is a pause with a pulse in it, held in front of
/// the position the transport is about to roll from. That is what lets bar one be counted in at
/// all — there is no timeline before it to roll through, and a count-in that needed some would be
/// a count-in that worked everywhere except at the start of a song.
///
/// The beat is a whole number of frames rather than a rate, so [`Self::total_frames`] and the
/// beats inside it are counted against the same number and cannot drift apart. Rounding a beat to
/// the nearest frame costs a fraction of a millisecond over a two-bar count.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct CountIn {
    /// Frames left before the playhead starts moving.
    pub remaining_frames: u64,
    /// How long one beat of the count lasts, in frames.
    pub beat_frames: u64,
    /// How many beats are counted.
    pub beats: u32,
    /// How many beats make a bar, so that the first of each is accented.
    pub per_bar: u32,
}

impl CountIn {
    /// A count of `beats`, each `beat_frames` long, accented every `per_bar`.
    pub fn new(beats: u32, beat_frames: u64, per_bar: u32) -> Self {
        Self {
            remaining_frames: beats as u64 * beat_frames,
            beat_frames,
            beats,
            per_bar: per_bar.max(1),
        }
    }

    /// How long the whole count lasts, in frames.
    pub fn total_frames(&self) -> u64 {
        self.beats as u64 * self.beat_frames
    }

    /// How far into the count the transport has got, in frames.
    pub fn elapsed_frames(&self) -> u64 {
        self.total_frames().saturating_sub(self.remaining_frames)
    }
}

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
    /// The count-in being played, while one is.
    ///
    /// A rolling transport with a count-in in front of it is *playing* — the button is down, the
    /// take is being written — and merely not moving yet. Everything that asks whether the
    /// arrangement should be heard asks [`Self::rolling`] instead.
    pub count_in: Option<CountIn>,
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

    /// `true` while a count-in is being played and the playhead is therefore held still.
    pub fn counting_in(&self) -> bool {
        self.count_in.is_some()
    }

    /// `true` when the arrangement should be heard: playing, and past any count-in.
    ///
    /// What every source asks. The distinction from [`Self::playing`] is the whole of the
    /// count-in feature: a count is played *by* a transport that is running, over an arrangement
    /// that is not yet sounding.
    pub fn rolling(&self) -> bool {
        self.playing && self.count_in.is_none()
    }

    /// Begins a count-in, or ends one when `count` counts no frames.
    pub fn set_count_in(&mut self, count: CountIn) {
        self.count_in = (count.remaining_frames > 0).then_some(count);
    }

    /// Frames remaining until the count-in ends, or `None` when none is running.
    pub fn frames_to_count_in_end(&self) -> Option<u64> {
        self.count_in.map(|count| count.remaining_frames)
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
    ///
    /// A count-in is spent instead of the playhead, and the caller is expected not to hand over a
    /// block that straddles the end of one — the renderer splits its segments there, exactly as it
    /// does at the loop end, so that the first beat of the song lands on the frame it should.
    /// Frames past the end of a count are dropped rather than credited to the playhead, which is
    /// the safe direction to be wrong in: a block late is a block, a block early is a take that
    /// begins before its own downbeat.
    pub fn advance(&mut self, frames: u64) {
        if !self.playing || frames == 0 {
            return;
        }
        if let Some(count) = &mut self.count_in {
            count.remaining_frames = count.remaining_frames.saturating_sub(frames);
            if count.remaining_frames == 0 {
                self.count_in = None;
            }
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
            count_in: None,
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
            count_in: None,
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

    #[test]
    fn a_count_in_holds_the_playhead_where_the_take_will_begin() {
        let mut transport = Transport::playing_from(48_000);
        transport.set_count_in(CountIn::new(4, 1_000, 4));
        assert!(transport.counting_in());
        assert!(!transport.rolling(), "the arrangement must not sound yet");

        transport.advance(3_000);
        assert_eq!(transport.position_frames, 48_000, "the playhead moved");
        assert_eq!(transport.frames_to_count_in_end(), Some(1_000));

        transport.advance(1_000);
        assert!(!transport.counting_in());
        assert!(transport.rolling());
        assert_eq!(
            transport.position_frames, 48_000,
            "the take starts where it was"
        );

        // And from there the transport is an ordinary one again.
        transport.advance(512);
        assert_eq!(transport.position_frames, 48_512);
    }

    #[test]
    fn a_count_of_nothing_is_no_count_at_all() {
        let mut transport = Transport::playing_from(0);
        transport.set_count_in(CountIn::new(0, 24_000, 4));
        assert!(!transport.counting_in());
        assert!(transport.rolling());
    }

    #[test]
    fn a_count_in_is_measured_from_the_beat_it_was_built_with() {
        let count = CountIn::new(8, 24_000, 4);
        assert_eq!(count.total_frames(), 192_000);
        assert_eq!(count.elapsed_frames(), 0);

        let mut transport = Transport::playing_from(0);
        transport.set_count_in(count);
        transport.advance(24_000);
        let counting = transport.count_in.expect("still counting");
        assert_eq!(counting.elapsed_frames(), 24_000);
        assert_eq!(counting.remaining_frames, 168_000);
    }

    #[test]
    fn a_stopped_transport_does_not_spend_its_count_in() {
        let mut transport = Transport::new();
        transport.set_count_in(CountIn::new(4, 1_000, 4));
        transport.advance(4_000);
        assert_eq!(transport.frames_to_count_in_end(), Some(4_000));
    }
}
