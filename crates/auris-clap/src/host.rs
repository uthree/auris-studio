//! What Auris Studio looks like from inside a hosted plugin.
//!
//! CLAP is a two-way interface: as well as calling into the plugin, the host must answer
//! callbacks from it. Those callbacks arrive on whichever thread the plugin felt like using,
//! including the audio thread, so none of them may do real work. Every one of them here sets an
//! atomic flag and returns; the session reads the flags when it next comes round.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl};
use clack_extensions::params::{HostParams, HostParamsImplMainThread, HostParamsImplShared};
use clack_extensions::params::{ParamClearFlags, ParamRescanFlags};
use clack_extensions::state::{HostState, HostStateImpl};
use clack_extensions::timer::{HostTimer, HostTimerImpl, TimerId};
use clack_host::prelude::*;
use clack_host::utils::ClapId;

use crate::timers::{Timer, take_due};

/// Requests a hosted plugin has made and nobody has answered yet.
///
/// Read and cleared by the session on the main thread. Each flag is set-only from the plugin's
/// side, so a request can be missed only by being coalesced with an identical one — which is
/// exactly what should happen to it.
#[derive(Debug, Default)]
pub struct HostFlags {
    /// The plugin wants to be deactivated and activated again — its parameter list, latency or
    /// port layout has changed in a way it cannot apply while running.
    pub restart: AtomicBool,
    /// The plugin wants processing to start, because it has something to play.
    pub process: AtomicBool,
    /// The plugin wants its main-thread callback called.
    pub callback: AtomicBool,
    /// A parameter's *value* changed behind the host's back — reread them.
    pub rescan_values: AtomicBool,
    /// A parameter's *description* changed. Restarting is the only honest response, since the
    /// parameter list the session published may no longer be the plugin's.
    pub rescan_info: AtomicBool,
    /// The plugin's own state changed, so the project is dirty.
    pub dirty: AtomicBool,
    /// The plugin's window has gone: the user closed it, or the plugin lost its connection to it.
    pub gui_closed: AtomicBool,
    /// The window was not merely closed but *destroyed* by the plugin, and the host still owes it
    /// a `destroy` call to acknowledge that. Kept apart from [`gui_closed`](Self::gui_closed)
    /// because the difference decides whether the host may still call `hide` on the way out —
    /// hiding a window that no longer exists is a call into freed memory.
    pub gui_destroyed: AtomicBool,
}

impl HostFlags {
    /// Reads a flag and clears it in the same operation.
    pub fn take(flag: &AtomicBool) -> bool {
        flag.swap(false, Ordering::AcqRel)
    }
}

/// The `[thread-safe]` half of the host: nothing but flags, because it can be called from
/// anywhere.
#[derive(Debug, Default)]
pub struct AurisShared {
    /// Pending requests.
    pub flags: HostFlags,
}

impl SharedHandler<'_> for AurisShared {
    fn request_restart(&self) {
        self.flags.restart.store(true, Ordering::Release);
    }

    fn request_process(&self) {
        self.flags.process.store(true, Ordering::Release);
    }

    fn request_callback(&self) {
        self.flags.callback.store(true, Ordering::Release);
    }
}

impl HostGuiImpl for AurisShared {
    fn resize_hints_changed(&self) {
        // Only embedded windows are resized by the host, and none of these are.
    }

    fn request_resize(&self, _new_size: GuiSize) -> Result<(), HostError> {
        // The window belongs to the plugin — it is floating, not a rectangle of ours — so this is
        // the plugin asking the host to resize a window the host does not have. Refusing is the
        // honest answer, and CLAP defines refusal as a legal one.
        Err(HostError::Message(
            "a floating plugin window is not the host's to resize",
        ))
    }

    fn request_show(&self) -> Result<(), HostError> {
        Err(HostError::Message(
            "a floating plugin window is not the host's to show",
        ))
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Err(HostError::Message(
            "a floating plugin window is not the host's to hide",
        ))
    }

    fn closed(&self, was_destroyed: bool) {
        if was_destroyed {
            self.flags.gui_destroyed.store(true, Ordering::Release);
        }
        // Second, so a reader that sees `gui_closed` is guaranteed to see the truth about
        // `gui_destroyed` — the difference is what stops the host calling `hide` on a window that
        // is already gone.
        self.flags.gui_closed.store(true, Ordering::Release);
    }
}

impl HostParamsImplShared for AurisShared {
    fn request_flush(&self) {
        // A flush is a process call with no audio. The session already calls `process` on every
        // block, so asking for one is asking for what is about to happen anyway.
        self.flags.process.store(true, Ordering::Release);
    }
}

/// The `[main-thread]` half of the host.
pub struct AurisMainThread<'a> {
    shared: &'a AurisShared,
    /// The timers this plugin has registered, in the order it registered them.
    timers: Vec<Timer>,
    /// The next unused timer id. Never reused, so a callback for a timer the plugin has just
    /// unregistered cannot be mistaken for one it registered afterwards.
    next_timer: u32,
}

impl<'a> AurisMainThread<'a> {
    /// Builds the main-thread handler around the shared one.
    pub fn new(shared: &'a AurisShared) -> Self {
        Self {
            shared,
            timers: Vec::new(),
            next_timer: 0,
        }
    }

    /// Every timer owed a tick at `now`, each moved on to its next one.
    pub(crate) fn due_timers(&mut self, now: Instant) -> Vec<TimerId> {
        take_due(&mut self.timers, now)
    }
}

impl<'a> MainThreadHandler<'a> for AurisMainThread<'a> {}

impl HostTimerImpl for AurisMainThread<'_> {
    fn register_timer(&mut self, period_ms: u32) -> Result<TimerId, HostError> {
        let id = TimerId(self.next_timer);
        self.next_timer += 1;
        self.timers.push(Timer::new(id, period_ms, Instant::now()));
        Ok(id)
    }

    fn unregister_timer(&mut self, timer_id: TimerId) -> Result<(), HostError> {
        let before = self.timers.len();
        self.timers.retain(|timer| timer.id != timer_id);
        match self.timers.len() < before {
            true => Ok(()),
            // Saying so rather than shrugging: a plugin cancelling a timer twice, or cancelling
            // one it never had, is a plugin whose bookkeeping disagrees with the host's, and the
            // next thing it does is likely to be worse.
            false => Err(HostError::Message("no such timer")),
        }
    }
}

impl HostParamsImplMainThread for AurisMainThread<'_> {
    fn rescan(&mut self, flags: ParamRescanFlags) {
        if flags.intersects(ParamRescanFlags::VALUES) {
            self.shared
                .flags
                .rescan_values
                .store(true, Ordering::Release);
        }
        // Anything beyond the values invalidates the descriptor list the session handed the UI.
        if flags.intersects(ParamRescanFlags::INFO | ParamRescanFlags::ALL) {
            self.shared.flags.rescan_info.store(true, Ordering::Release);
        }
    }

    fn clear(&mut self, _param_id: ClapId, _flags: ParamClearFlags) {
        // Automation for one parameter should be dropped. Auris stores automation against a
        // slot and a `ParamId`, which the session owns, so this becomes a session command — and
        // until it is one, forgetting the request is better than clearing the wrong lane.
    }
}

impl HostStateImpl for AurisMainThread<'_> {
    fn mark_dirty(&mut self) {
        self.shared.flags.dirty.store(true, Ordering::Release);
    }
}

/// Auris Studio, as a CLAP host.
pub struct AurisHost;

impl HostHandlers for AurisHost {
    type Shared<'a> = AurisShared;
    type MainThread<'a> = AurisMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &AurisShared) {
        builder
            .register::<HostGui>()
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostTimer>();
    }
}

/// Identity this host reports to the plugins it loads.
pub fn host_info() -> HostInfo {
    // PANIC: none of these literals contains a NUL byte.
    HostInfo::new(
        "Auris Studio",
        "Auris Studio",
        "https://github.com/uthree/auris-studio",
        env!("CARGO_PKG_VERSION"),
    )
    .expect("host info contains no NUL bytes")
}
