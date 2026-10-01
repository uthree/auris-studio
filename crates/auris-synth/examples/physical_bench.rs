//! Measures callback cost with all 24 physical voices held; no audio device is needed.

use std::time::Instant;

use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PrepareContext, ProcessContext,
};
use auris_synth::{Model, Physical};

fn main() {
    let observe = std::env::args().any(|arg| arg == "--motion");
    let rate = 48_000.0;
    let frames = 256;
    for model in Model::ALL {
        let mut instrument = Physical::new(model);
        instrument.set_param_by_key("decay", 12.0);
        instrument.prepare(&PrepareContext::new(rate, frames, 2));
        let monitor = instrument.motion_monitor();
        if let Some(monitor) = &monitor {
            monitor.watch(observe);
        }
        let mut buffer = AudioBuffer::stereo(frames, rate);
        let events: Vec<_> = (0..24)
            .map(|index| NoteEvent::NoteOn {
                frame: 0,
                pitch: 48 + index % 12,
                velocity: 0.8,
            })
            .collect();
        let context = ProcessContext::realtime(rate, frames, 0, 120.0, true);
        instrument.process(&events, &mut buffer, &context);
        let mut times = Vec::with_capacity(512);
        for _ in 0..512 {
            let started = Instant::now();
            instrument.process(&[], &mut buffer, &context);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&buffer);
        }
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        times.sort_by(f64::total_cmp);
        println!(
            "{model:?}: mean {mean:.3} ms, p99 {:.3} ms, {:.1}% of 5.333 ms callback budget",
            times[506],
            mean / (frames as f64 / rate * 1000.0) * 100.0
        );
    }
}
