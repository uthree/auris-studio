//! Test support: minimal plugins plus an allocation counter.
//!
//! The engine must not depend on `auris-synth` or `auris-dsp`, so the tests bring their own
//! trivially predictable instrument and effects. Every one of them produces exact numbers, which
//! is what lets the render tests assert on levels rather than on "something happened".
//!
//! [`count_allocations`] is what turns "the audio path does not allocate" from a claim in a
//! comment into something the test suite fails on.

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};
use auris_core::registry::PluginRegistry;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Whether this thread is currently inside [`count_allocations`].
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    /// Allocations this thread has made since the count started.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    /// Blocks [`BlockCounter`] has processed on this thread.
    static PROCESS_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Counts heap allocations made by `body` **on the calling thread**.
///
/// Per-thread rather than global because `cargo test` runs cases in parallel, and a global
/// counter would pick up every other test's traffic.
pub(crate) fn count_allocations(body: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|slot| slot.set(0));
    WATCHING.with(|slot| slot.set(true));
    body();
    WATCHING.with(|slot| slot.set(false));
    ALLOCATIONS.with(Cell::get)
}

fn note_allocation() {
    // `try_with` because the allocator can run while thread-locals are being torn down.
    if WATCHING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|slot| slot.set(slot.get() + 1));
    }
}

/// A pass-through allocator that reports to [`count_allocations`].
struct CountingAllocator;

// SAFETY: every method forwards to `System` unchanged; the counter only reads and writes
// thread-local `Cell`s, which allocate nothing (both are `const`-initialised and have no
// destructor, so no lazy allocation or TLS destructor registration happens).
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Registry id of [`ConstantTone`].
pub(crate) const TONE_ID: &str = "test.tone";
/// Registry id of [`LinearGain`].
pub(crate) const GAIN_ID: &str = "test.gain";
/// Registry id of [`RingingTail`].
pub(crate) const TAIL_ID: &str = "test.tail";
/// Registry id of [`BlockCounter`].
pub(crate) const COUNTER_ID: &str = "test.counter";
/// Registry id of [`HugeTail`].
pub(crate) const HUGE_TAIL_ID: &str = "test.huge-tail";
/// Registry id of [`Lookahead`].
pub(crate) const LOOKAHEAD_ID: &str = "test.lookahead";

/// Frames [`Lookahead`] holds its input back by, and declares as its latency.
pub(crate) const LOOKAHEAD_FRAMES: usize = 64;

/// Level [`ConstantTone`] emits per held note.
pub(crate) const TONE_AMPLITUDE: f32 = 0.5;

/// Frames [`RingingTail`] claims to keep sounding after its input stops.
pub(crate) const TAIL_FRAMES: usize = 1_024;

/// A registry holding every stub in this module.
pub(crate) fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.register_instrument(|| Box::new(ConstantTone::new()));
    registry.register_effect(|| Box::new(LinearGain::new()));
    registry.register_effect(|| Box::new(RingingTail::new()));
    registry.register_effect(|| Box::new(BlockCounter));
    registry.register_effect(|| Box::new(HugeTail));
    registry.register_effect(|| Box::new(Lookahead::new()));
    registry
}

/// Emits a DC level proportional to the number of notes currently held.
///
/// Counting held notes rather than gating on "any note" is deliberate: an event delivered twice
/// shows up as double amplitude and a dropped event as silence, so event scheduling bugs are
/// visible in the rendered samples.
pub(crate) struct ConstantTone {
    params: Vec<ParamDescriptor>,
    amplitude: f32,
    held: u32,
}

impl ConstantTone {
    pub(crate) fn new() -> Self {
        Self {
            params: vec![ParamDescriptor::new(
                0u32,
                "amplitude",
                "Amplitude",
                0.0,
                1.0,
                TONE_AMPLITUDE,
            )],
            amplitude: TONE_AMPLITUDE,
            held: 0,
        }
    }

    fn apply(&mut self, event: &NoteEvent) {
        match event {
            NoteEvent::NoteOn { .. } => self.held += 1,
            NoteEvent::NoteOff { .. } => self.held = self.held.saturating_sub(1),
            NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. } => self.held = 0,
            NoteEvent::PitchBend { .. } => {}
        }
    }
}

impl Parameterized for ConstantTone {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.params
    }

    fn param(&self, id: ParamId) -> f32 {
        if id.index() == 0 { self.amplitude } else { 0.0 }
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if id.index() == 0 {
            self.amplitude = self.params[0].clamp(value);
        }
    }
}

impl Instrument for ConstantTone {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            TONE_ID,
            "Constant Tone",
            "Emits a DC level while notes are held",
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {
        self.held = 0;
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = out.frame_count().min(ctx.block_frames);
        let mut next = 0;
        for frame in 0..frames {
            while next < events.len() && events[next].frame() as usize <= frame {
                let event = events[next];
                self.apply(&event);
                next += 1;
            }
            let value = self.amplitude * self.held as f32;
            for channel in out.channels_mut() {
                channel[frame] = value;
            }
        }
    }

    fn active_voices(&self) -> usize {
        self.held as usize
    }
}

/// Multiplies its input by a linear gain.
pub(crate) struct LinearGain {
    params: Vec<ParamDescriptor>,
    gain: f32,
}

impl LinearGain {
    pub(crate) fn new() -> Self {
        Self {
            params: vec![ParamDescriptor::new(0u32, "gain", "Gain", 0.0, 4.0, 1.0)],
            gain: 1.0,
        }
    }
}

impl Parameterized for LinearGain {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.params
    }

    fn param(&self, id: ParamId) -> f32 {
        if id.index() == 0 { self.gain } else { 0.0 }
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if id.index() == 0 {
            self.gain = self.params[0].clamp(value);
        }
    }
}

impl Effect for LinearGain {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            GAIN_ID,
            "Linear Gain",
            "Multiplies by a linear gain",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        buffer.apply_gain(self.gain);
    }
}

/// Blocks [`BlockCounter`] has processed on this thread, resetting the count to zero.
///
/// Per-thread for the same reason as [`count_allocations`]: `cargo test` runs cases in parallel,
/// and a graph is only ever rendered by the thread that owns it.
pub(crate) fn take_process_calls() -> usize {
    PROCESS_CALLS.with(|slot| slot.replace(0))
}

/// Passes audio through untouched and counts the blocks it is handed.
///
/// One call per [`render_block`](crate::render_block) is the contract; every extra call is a
/// whole additional pass over the graph, which is a cost the tests can assert on.
pub(crate) struct BlockCounter;

impl Parameterized for BlockCounter {
    fn parameters(&self) -> &[ParamDescriptor] {
        &[]
    }

    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }

    fn set_param(&mut self, _id: ParamId, _value: f32) {}
}

impl Effect for BlockCounter {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            COUNTER_ID,
            "Block Counter",
            "Counts the blocks it is asked to process",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        PROCESS_CALLS.with(|slot| slot.set(slot.get() + 1));
    }
}

/// A one-pole feedback path that keeps sounding after its input stops.
///
/// `y[n] = x[n] + 0.5 * y[n-1]` halves every sample, so the tail is audible for long enough to
/// prove the offline renderer does not chop it off, and its state is two floats — no allocation.
pub(crate) struct RingingTail {
    state: [f32; 2],
}

impl RingingTail {
    pub(crate) fn new() -> Self {
        Self { state: [0.0; 2] }
    }
}

impl Parameterized for RingingTail {
    fn parameters(&self) -> &[ParamDescriptor] {
        &[]
    }

    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }

    fn set_param(&mut self, _id: ParamId, _value: f32) {}
}

impl Effect for RingingTail {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            TAIL_ID,
            "Ringing Tail",
            "A decaying feedback path with a declared tail",
            PluginCategory::Reverb,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {
        self.state = [0.0; 2];
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        for (channel, samples) in buffer.channels_mut().iter_mut().enumerate() {
            let Some(state) = self.state.get_mut(channel) else {
                continue;
            };
            for sample in samples.iter_mut() {
                *state = *sample + 0.5 * *state;
                *sample = *state;
            }
        }
    }

    fn tail_frames(&self) -> usize {
        TAIL_FRAMES
    }
}

/// Hands its input back a settable number of frames late, and declares that as its latency.
///
/// This is the shape of every look-ahead processor — the limiter is the one that ships — reduced
/// to the part the engine has to reason about: audio that comes out later than it went in, by a
/// distance the user can change while the graph is running. Declaring no tail keeps the
/// compensation tests measuring compensation and nothing else.
pub(crate) struct Lookahead {
    params: Vec<ParamDescriptor>,
    frames: usize,
    lines: Vec<Vec<f32>>,
    write: usize,
}

impl Lookahead {
    pub(crate) fn new() -> Self {
        Self {
            params: vec![ParamDescriptor::new(
                0u32,
                "frames",
                "Lookahead",
                0.0,
                LOOKAHEAD_FRAMES as f32,
                LOOKAHEAD_FRAMES as f32,
            )],
            frames: LOOKAHEAD_FRAMES,
            lines: Vec::new(),
            write: 0,
        }
    }
}

impl Parameterized for Lookahead {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.params
    }

    fn param(&self, id: ParamId) -> f32 {
        if id.index() == 0 {
            self.frames as f32
        } else {
            0.0
        }
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if id.index() == 0 {
            self.frames = self.params[0].clamp(value) as usize;
            // The ring is read modulo the new length, so start it from a position that is inside.
            self.write = 0;
        }
    }
}

impl Effect for Lookahead {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            LOOKAHEAD_ID,
            "Lookahead",
            "Returns its input a fixed number of frames late",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        // Sized for the longest look-ahead the parameter allows, so shortening it later is a
        // change of modulus rather than an allocation on the audio thread.
        self.lines = vec![vec![0.0; LOOKAHEAD_FRAMES]; ctx.channel_count.max(1)];
        self.write = 0;
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.write = 0;
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        if self.frames == 0 {
            return;
        }
        let start = self.write;
        let mut finished = start;
        for (samples, line) in buffer.channels_mut().iter_mut().zip(&mut self.lines) {
            let mut position = start;
            for sample in samples.iter_mut() {
                std::mem::swap(sample, &mut line[position]);
                position += 1;
                if position == self.frames {
                    position = 0;
                }
            }
            finished = position;
        }
        self.write = finished;
    }

    fn latency_frames(&self) -> usize {
        self.frames
    }
}

/// Passes audio through untouched and claims the longest tail expressible.
///
/// Nothing real reports this, which is the point: the numbers come from plugins, so the arithmetic
/// that adds them up has to survive one that is absurd.
pub(crate) struct HugeTail;

impl Parameterized for HugeTail {
    fn parameters(&self) -> &[ParamDescriptor] {
        &[]
    }

    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }

    fn set_param(&mut self, _id: ParamId, _value: f32) {}
}

impl Effect for HugeTail {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            HUGE_TAIL_ID,
            "Huge Tail",
            "Declares an absurd tail without producing one",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _buffer: &mut AudioBuffer, _ctx: &ProcessContext) {}

    fn tail_frames(&self) -> usize {
        usize::MAX
    }
}
