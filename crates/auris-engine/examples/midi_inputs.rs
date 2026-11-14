//! Lists the MIDI inputs the system has, then prints what is played on them for a while.
//!
//! `cargo run -p auris-engine --example midi_inputs -- 10` listens for ten seconds (default 5).
//! A quick way to tell a keyboard the system does not see from one Auris does not hear.

use std::time::{Duration, Instant};

use auris_core::TrackId;
use auris_engine::{AudioSettings, LiveTarget, MidiInputs, start_silent};

fn main() -> Result<(), auris_engine::EngineError> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(5);
    let mut inputs = MidiInputs::new();
    let available = inputs.available()?;
    if available.is_empty() {
        println!("no MIDI inputs");
        return Ok(());
    }
    for name in &available {
        match inputs.connect(name) {
            Ok(()) => println!("listening to `{name}`"),
            Err(error) => println!("`{name}`: {error}"),
        }
    }
    let (_device, engine) = start_silent(&AudioSettings::default());
    inputs.set_target(
        &engine,
        Some(LiveTarget {
            index: 0,
            track: TrackId(1),
        }),
    );
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        for event in inputs.drain() {
            println!("{:?}", event.performed);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    inputs.close();
    Ok(())
}
