//! The UI-thread side of the engine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::command::EngineCommand;
use crate::error::EngineError;
use crate::graph::RenderGraph;
use crate::meter::MeterBank;

/// Heap-owning data travelling back from the audio thread to be dropped here.
///
/// One channel for everything the callback must not free itself: a graph holds plugin
/// instances and sample buffers, and a preview buffer is a couple of hundred kilobytes of
/// audio — either one deallocated inside the callback would risk an xrun. The UI never looks
/// inside; [`EngineHandle::collect_garbage`] drops whatever arrives.
pub enum Retired {
    /// A render graph replaced by [`EngineCommand::SetGraph`].
    Graph(Box<RenderGraph>),
    /// A preview buffer replaced by [`EngineCommand::PlayOneShot`].
    Buffer(Arc<auris_core::AudioBuffer>),
}

/// Retired data travelling back from the audio thread to be dropped here.
pub(crate) type GraphReceiver = Receiver<Retired>;

/// Everything the UI needs to talk to a running engine.
///
/// Cheap to clone-free share: the handle owns the sending end of the command queue, the shared
/// playhead and the meter bank. All of its reads are lock-free, so calling them from a render
/// loop at 120 Hz costs nothing.
pub struct EngineHandle {
    pub(crate) commands: Sender<EngineCommand>,
    pub(crate) returned_graphs: GraphReceiver,
    pub(crate) playhead: Arc<AtomicU64>,
    pub(crate) count_in: Arc<AtomicU64>,
    pub(crate) meters: Arc<MeterBank>,
    pub(crate) running: Arc<AtomicBool>,
    pub(crate) playing: Arc<AtomicBool>,
    pub(crate) latency_stale: Arc<AtomicBool>,
    pub(crate) sample_rate: f64,
    pub(crate) channel_count: usize,
    pub(crate) max_block: usize,
}

impl EngineHandle {
    /// Queues a command for the audio thread.
    ///
    /// Never blocks. A full queue means the audio thread has not run since the last batch, which
    /// is normal during a window resize; the caller should retry on the next frame.
    pub fn send(&self, command: EngineCommand) -> Result<(), EngineError> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(EngineError::CommandQueueFull),
            Err(TrySendError::Disconnected(_)) => Err(EngineError::NotRunning),
        }
    }

    /// Installs a freshly built graph.
    pub fn set_graph(&self, graph: RenderGraph) -> Result<(), EngineError> {
        self.send(EngineCommand::SetGraph(Box::new(graph)))
    }

    /// Where the playhead is, in frames. Written by the audio thread every callback.
    pub fn playhead_frames(&self) -> u64 {
        self.playhead.load(Ordering::Relaxed)
    }

    /// The playhead itself, for something that has to read it from another audio callback.
    ///
    /// [`start_capture`](crate::start_capture) is the one caller: an input stream runs on its own
    /// thread and has to stamp a take with where the transport was when the first block landed.
    /// A copy of the number would be one callback out of date, which is the difference between a
    /// take that lines up and one that has to be nudged by hand every time.
    pub fn playhead_cell(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.playhead)
    }

    /// Frames of count-in left before the transport starts moving, zero when none is running.
    ///
    /// Read by a frontend to show that Record has been pressed and the song has not started yet,
    /// which is otherwise indistinguishable from a transport that is rolling and stuck.
    pub fn count_in_frames(&self) -> u64 {
        self.count_in.load(Ordering::Relaxed)
    }

    /// Declares a count-in that has been asked for but has not reached the audio thread yet.
    ///
    /// Sent commands are applied at the top of the next callback, and an input stream can hand
    /// over its first block before that happens. Whatever reads this then would find a zero and
    /// conclude the count was over — and a take trimmed by nothing is a take that begins a whole
    /// count-in early, which is the one way this can be wrong that anybody would notice.
    ///
    /// So the count is written down *before* it is sent, and the audio thread only overwrites it
    /// once it has a count of its own to report — publishing a zero every callback would put the
    /// figure back before the command it is waiting for ever arrived.
    ///
    /// Every take declares one, zero included. A take that followed one which was cancelled part
    /// way through its count would otherwise find that count still written down here.
    pub fn expect_count_in(&self, frames: u64) {
        self.count_in.store(frames, Ordering::Relaxed);
    }

    /// The count-in cell itself, for the same reader [`Self::playhead_cell`] exists for.
    ///
    /// A take that was counted in opens its file during the count, and how much of the count it
    /// caught is the difference between a clip that lands on the beat and one that lands a bar
    /// early. The input callback stamps this at the same instant it stamps the playhead, so the
    /// two are one reading rather than two.
    pub fn count_in_cell(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.count_in)
    }

    /// Playhead position in seconds.
    pub fn playhead_seconds(&self) -> f64 {
        if self.sample_rate <= 0.0 {
            0.0
        } else {
            self.playhead_frames() as f64 / self.sample_rate
        }
    }

    /// Rate the engine is rendering at. Build render graphs for this rate, not the project's.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Channel count of the output device.
    pub fn channel_count(&self) -> usize {
        self.channel_count
    }

    /// Largest block the engine renders at once; pass this to [`RenderGraph::build`].
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Lock-free level meters.
    pub fn meters(&self) -> &MeterBank {
        &self.meters
    }

    /// A shared reference to the meters, for a UI that keeps its own copy.
    pub fn meters_arc(&self) -> Arc<MeterBank> {
        Arc::clone(&self.meters)
    }

    /// Drops every graph the audio thread has handed back, returning how many there were.
    ///
    /// This is the point of the whole exchange: a graph holds plugin instances and sample
    /// buffers, and freeing those inside the audio callback would risk an xrun. Call it once per
    /// UI frame.
    pub fn collect_garbage(&self) -> usize {
        self.returned_graphs.try_iter().count()
    }

    /// `true` when the transport is rolling.
    ///
    /// Read from the audio thread's own state rather than mirrored in the caller: the transport
    /// is what actually decides, and a UI copy would drift the moment anything but a `Play` or
    /// `Stop` command changed it.
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// `true` when the running graph's delay compensation no longer matches its effect chains.
    ///
    /// Every way of changing a chain rebuilds the graph except writing a parameter, and a plugin
    /// is free to report a different latency once one has been written — a look-ahead length is
    /// a parameter like any other. The audio thread re-checks after each such write; a frontend
    /// that sees this go true should rebuild, or that track will play out of step with the rest.
    pub fn latency_is_stale(&self) -> bool {
        self.latency_stale.load(Ordering::Relaxed)
    }

    /// `true` when an output stream is actually running.
    ///
    /// `false` means the application opened without audio hardware; the handle still accepts
    /// commands so the rest of the UI works unchanged.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("sample_rate", &self.sample_rate)
            .field("channel_count", &self.channel_count)
            .field("max_block", &self.max_block)
            .field("running", &self.is_running())
            .field("playing", &self.is_playing())
            .field("playhead_frames", &self.playhead_frames())
            .finish()
    }
}
