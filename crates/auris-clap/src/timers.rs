//! The timers a plugin asks the host to run for it.
//!
//! CLAP plugins do not run their own clocks. A plugin that needs to be woken periodically — which
//! in practice means every plugin with a window, because that is how it repaints — registers a
//! timer with the host and is called back on the main thread. Auris has one main-thread tick
//! already, the one the window repaints on, and this is what hangs off it.
//!
//! The host is explicitly allowed to run a timer slower than asked. That latitude is the whole
//! reason this is not simply a `SetTimer` call: a plugin asking for one millisecond would
//! otherwise be told it was being served thirty times more often than anything here could serve
//! it.

use std::time::{Duration, Instant};

use clack_extensions::timer::TimerId;

/// The shortest period this host will run a timer at.
///
/// Sixty a second, which is the rate CLAP suggests a host should at least offer. The *real*
/// resolution is whatever the frontend calls [`ClapPlugin::tick_timers`](crate::ClapPlugin::tick_timers)
/// at, and is coarser than this; the floor is here so a plugin asking for something absurd is
/// slowed to something sane rather than fired on every tick.
pub const TIMER_FLOOR_MS: u32 = 16;

/// The period a timer will really run at, given what the plugin asked for.
///
/// A request of zero is a request for "as often as you can", which is the floor.
pub fn timer_period(requested_ms: u32) -> Duration {
    Duration::from_millis(requested_ms.max(TIMER_FLOOR_MS) as u64)
}

/// One timer a plugin has registered.
#[derive(Debug)]
pub(crate) struct Timer {
    /// What the plugin will be handed back when this fires.
    pub(crate) id: TimerId,
    /// How often it wants to fire.
    pub(crate) period: Duration,
    /// The next moment it is owed a tick.
    pub(crate) due: Instant,
}

impl Timer {
    /// A timer registered at `now`, firing first one period from then.
    pub(crate) fn new(id: TimerId, requested_ms: u32, now: Instant) -> Self {
        let period = timer_period(requested_ms);
        Self {
            id,
            period,
            due: now + period,
        }
    }
}

/// Every timer due at `now`, each moved on to its next tick.
///
/// A timer that was missed fires **once** and is then re-based on the present rather than caught
/// up. These drive repaints: a window that spent two seconds behind a modal file dialog owes
/// nobody sixty redraws of the frame it is about to draw anyway. It is also what stops a slow
/// callback from lengthening its own backlog every time it runs.
pub(crate) fn take_due(timers: &mut [Timer], now: Instant) -> Vec<TimerId> {
    let mut due = Vec::new();
    for timer in timers {
        if timer.due <= now {
            timer.due = now + timer.period;
            due.push(timer.id);
        }
    }
    due
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_is_never_shorter_than_the_floor() {
        assert_eq!(
            timer_period(0),
            Duration::from_millis(TIMER_FLOOR_MS as u64)
        );
        assert_eq!(
            timer_period(1),
            Duration::from_millis(TIMER_FLOOR_MS as u64)
        );
        assert_eq!(timer_period(1000), Duration::from_millis(1000));
    }

    #[test]
    fn only_the_timers_that_are_owed_a_tick_fire() {
        let start = Instant::now();
        let mut timers = [
            Timer::new(TimerId(0), 20, start),
            Timer::new(TimerId(1), 500, start),
        ];

        assert!(
            take_due(&mut timers, start).is_empty(),
            "neither is due yet"
        );
        assert_eq!(
            take_due(&mut timers, start + Duration::from_millis(100)),
            vec![TimerId(0)],
            "the fast one is owed a tick and the slow one is not"
        );
        assert_eq!(
            take_due(&mut timers, start + Duration::from_millis(600)),
            vec![TimerId(0), TimerId(1)]
        );
    }

    #[test]
    fn a_timer_that_was_missed_fires_once_and_starts_again_from_now() {
        // Ten seconds of a 16 ms timer is over six hundred ticks. Handing a plugin six hundred
        // repaints for a window it is about to draw once would take longer than the pause did.
        let start = Instant::now();
        let mut timers = [Timer::new(TimerId(0), 16, start)];

        let late = start + Duration::from_secs(10);
        assert_eq!(take_due(&mut timers, late), vec![TimerId(0)]);
        assert_eq!(
            timers[0].due,
            late + Duration::from_millis(16),
            "re-based on the present, not caught up from where it fell behind"
        );
        assert!(take_due(&mut timers, late).is_empty());
    }
}
