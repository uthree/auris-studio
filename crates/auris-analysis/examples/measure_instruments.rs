//! CPU runtime diagnostic for the real optional YAMNet export; not an accuracy corpus.

use auris_analysis::{AnalysisControl, instruments::analyze_instruments};
use auris_core::AudioBuffer;
use std::{path::Path, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .ok_or("expected a prepared YAMNet ONNX path")?;
    let rate = 16_000.0;
    let samples = (0..180 * rate as usize)
        .map(|i| {
            let t = i as f64 / rate;
            let note = (t * 2.0) as usize % 4;
            let frequency = [261.6256, 329.6276, 391.9954, 523.2511][note];
            let phase = std::f64::consts::TAU * frequency * t;
            ((phase.sin() + 0.25 * (2.0 * phase).sin()) * (-12.0 * (t % 0.5)).exp() * 0.4) as f32
        })
        .collect();
    let audio = AudioBuffer::from_planar(vec![samples], rate)?;
    let start = Instant::now();
    let result = analyze_instruments(&audio, Path::new(&model), 0.2, &AnalysisControl::default())?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "fixture": "180-second synthetic decaying two-harmonic arpeggio; no instrument ground truth",
            "model_sha256": result.model_sha256, "audio_seconds": result.seconds,
            "elapsed_seconds_including_load": elapsed, "real_time_factor": elapsed / result.seconds,
            "windows": result.windows.len(), "candidates": result.candidates, "raw_top": result.raw_top,
        }))?
    );
    Ok(())
}
