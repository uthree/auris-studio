//! CPU-only diagnostic suite, or comparison of two source-second note JSON files.
//! Run without arguments for synthetic diagnostics, or with reference.json estimate.json.

mod support;
use auris_analysis::{
    AnalysisControl,
    audio::{ANALYSIS_RATE, AudioOptions, analyze_audio},
};
use auris_core::AudioBuffer;
use sha2::{Digest, Sha256};
use support::{Note, scores};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let report = match args.as_slice() {
        [] => synthetic()?,
        [reference, estimate] => {
            let reference_bytes = std::fs::read(reference)?;
            let estimate_bytes = std::fs::read(estimate)?;
            let reference: support::Notes = serde_json::from_slice(&reference_bytes)?;
            let estimate: support::Notes = serde_json::from_slice(&estimate_bytes)?;
            serde_json::json!({"schema": 1, "reference_sha256": format!("{:x}", Sha256::digest(reference_bytes)),
                "estimate_sha256": format!("{:x}", Sha256::digest(estimate_bytes)),
                "metrics": scores(&reference.notes, &estimate.notes)?})
        }
        _ => return Err("usage: evaluate_transcription [reference.json estimate.json]".into()),
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn synthetic() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut cases = Vec::new();
    let mut algorithm = "";
    for kind in [
        "clean",
        "vibrato",
        "weak_fundamental",
        "legato",
        "repeated",
        "dynamics",
        "dropout",
        "noise",
        "dc",
    ] {
        let negative = matches!(kind, "noise" | "dc");
        let reference: Vec<_> = if negative {
            vec![]
        } else {
            (0..8)
                .map(|i| Note {
                    pitch: if matches!(kind, "repeated" | "dropout" | "vibrato") {
                        69.0
                    } else if kind == "legato" {
                        60.0 + i as f64
                    } else {
                        [48.0, 60.0, 64.0, 67.0][i % 4]
                    },
                    start: 0.2 + i as f64 * 0.5,
                    end: 0.2 + i as f64 * 0.5 + if kind == "legato" { 0.5 } else { 0.36 },
                })
                .collect()
        };
        let mut phase = 0.0f64;
        let mut random = 79u32;
        let samples: Vec<f32> = (0..(4.5 * ANALYSIS_RATE) as usize)
            .map(|i| {
                let t = i as f64 / ANALYSIS_RATE;
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (f64::from(random) / f64::from(u32::MAX) - 0.5) * 2.0;
                if kind == "noise" {
                    return (noise * 0.2) as f32;
                }
                if kind == "dc" {
                    return 0.2;
                }
                let Some((index, note)) = reference
                    .iter()
                    .enumerate()
                    .find(|(_, n)| t >= n.start && t < n.end)
                else {
                    return 0.0;
                };
                let cents = if kind == "vibrato" {
                    65.0 * (std::f64::consts::TAU * 5.5 * t).sin()
                } else {
                    0.0
                };
                phase += std::f64::consts::TAU
                    * 440.0
                    * 2.0f64.powf((note.pitch - 69.0 + cents / 100.0) / 12.0)
                    / ANALYSIS_RATE;
                let gain = if kind == "dynamics" {
                    [0.4, 0.12, 0.04, 0.2][index % 4]
                } else {
                    0.4
                };
                let local = t - note.start;
                let envelope = if kind == "legato" {
                    1.0
                } else {
                    (local * 200.0).min(1.0) * ((note.end - t) * 200.0).min(1.0)
                };
                let signal = if kind == "weak_fundamental" {
                    0.12 * phase.sin() + 0.5 * (2.0 * phase).sin() + 0.3 * (3.0 * phase).sin()
                } else {
                    phase.sin()
                };
                let dropout = kind == "dropout" && (0.16..0.18).contains(&local);
                if dropout {
                    0.0
                } else {
                    (gain * envelope * signal) as f32
                }
            })
            .collect();
        let mut hash = Sha256::new();
        for sample in &samples {
            hash.update(sample.to_le_bytes());
        }
        let buffer = AudioBuffer::from_planar(vec![samples], ANALYSIS_RATE)?;
        let began = std::time::Instant::now();
        let result = analyze_audio(
            &buffer,
            AudioOptions { transcribe: true },
            &AnalysisControl::default(),
        )?;
        let elapsed = began.elapsed().as_secs_f64();
        algorithm = result.algorithm;
        let estimate: Vec<_> = result
            .notes
            .iter()
            .map(|n| Note {
                pitch: f64::from(n.pitch),
                start: n.start,
                end: n.end,
            })
            .collect();
        cases.push(
            serde_json::json!({"id":kind, "pcm_sha256":format!("{:x}", hash.finalize()),
            "seconds":4.5, "elapsed_seconds":elapsed, "reference":reference, "estimate":estimate,
            "metrics":scores(&reference, &estimate)?}),
        );
    }
    Ok(
        serde_json::json!({"schema":1, "suite":"synthetic-monophonic-v1", "algorithm":algorithm,
        "provenance":"Original deterministic signals generated by evaluate_transcription.rs; repository license; diagnostic cases, not held-out recordings.",
        "sample_rate":ANALYSIS_RATE, "cases":cases}),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostic_note_and_negative_control_gates() {
        let report = super::synthetic().unwrap();
        for case in report["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            match id {
                "noise" | "dc" => assert_eq!(case["metrics"]["estimated_notes"], 0, "{id}"),
                // A short dropout can resemble a genuine same-pitch reattack; keep its
                // measured result visible instead of treating the fixture as solved.
                "dropout" => {
                    assert!(case["metrics"]["onset_offset"]["f1"].as_f64().unwrap() >= 0.6)
                }
                _ => assert!(
                    case["metrics"]["onset_offset"]["f1"].as_f64().unwrap() >= 0.95,
                    "{case}"
                ),
            }
        }
    }
}
