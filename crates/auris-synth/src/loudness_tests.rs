//! Every instrument in the pack at its default settings, held to the level reference.

use auris_core::PluginRegistry;
use auris_dsp::loudness::integrated_lufs;

use crate::SynthPack;
use crate::level_reference::{REFERENCE_LUFS, phrase, render};

/// How far from [`REFERENCE_LUFS`] an instrument may sit.
const TOLERANCE_DB: f32 = 1.0;

#[test]
fn every_instrument_plays_its_phrase_at_the_same_loudness() {
    let mut registry = PluginRegistry::new();
    registry.install::<SynthPack>();
    let ids: Vec<String> = registry.instruments().map(|d| d.id.to_string()).collect();
    let measured: Vec<(String, f32, f32)> = ids
        .into_iter()
        .map(|id| {
            let mut instrument = registry.create_instrument(&id).unwrap();
            let rendered = render(instrument.as_mut(), &phrase(&id));
            let lufs = integrated_lufs(&rendered).unwrap_or(f32::NEG_INFINITY);
            let peak = (0..2)
                .flat_map(|channel| rendered.channel(channel).iter())
                .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
            (id, lufs, 20.0 * peak.log10())
        })
        .collect();
    let table: String = measured
        .iter()
        .map(|(id, lufs, peak)| {
            format!(
                "{id:32} {lufs:7.2} LUFS ({:+.2})  peak {peak:6.2} dBFS\n",
                lufs - REFERENCE_LUFS
            )
        })
        .collect();
    println!("{table}");
    let outliers: Vec<&(String, f32, f32)> = measured
        .iter()
        .filter(|(_, lufs, peak)| (lufs - REFERENCE_LUFS).abs() > TOLERANCE_DB || *peak > -1.0)
        .collect();
    assert!(
        outliers.is_empty(),
        "outside {REFERENCE_LUFS} ± {TOLERANCE_DB} LUFS or above -1 dBFS: {outliers:?}\n{table}"
    );
}
