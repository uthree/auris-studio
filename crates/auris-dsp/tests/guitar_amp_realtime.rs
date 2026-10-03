//! The amplifier callback and parameter changes must allocate nothing after prepare.

use auris_core::{AudioBuffer, Effect, ParamId, Parameterized, PrepareContext, ProcessContext};
use auris_dsp::GuitarAmp;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

struct Allocator;

fn record() {
    if WATCHING.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNT.try_with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: storage operations forward unchanged to System; const thread locals allocate
// nothing, and try_with handles teardown without panicking inside the allocator.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn rendering_and_live_control_changes_allocate_nothing() {
    let mut amp = GuitarAmp::new();
    amp.prepare(&PrepareContext::new(48000.0, 256, 2));
    let mut buffer = AudioBuffer::stereo(256, 48000.0);
    let ctx = ProcessContext::realtime(48000.0, 256, 0, 120.0, true);
    COUNT.with(|count| count.set(0));
    WATCHING.with(|watching| watching.set(true));
    for index in 0..16 {
        amp.set_param(ParamId(0), index as f32 * 2.0);
        amp.set_param(ParamId(1), index as f32 - 8.0);
        amp.set_param(ParamId(4), (index % 3) as f32);
        buffer.channel_mut(0).fill(0.2);
        buffer.channel_mut(1).fill(-0.1);
        amp.process(&mut buffer, &ctx);
    }
    amp.reset();
    WATCHING.with(|watching| watching.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
}
