//! Writes takes of the accompaniment writers as JSON lines, for
//! `tools/eval/accompaniment_parts.py` to count.
//!
//! ```text
//! cargo run --release -p auris-compose --example dump_parts > takes.jsonl
//! cargo run --release -p auris-compose --example dump_parts -- --seeds 1000 > takes.jsonl
//! cargo run --release -p auris-compose --example dump_parts -- --sweep > sweep.jsonl
//! ```
//!
//! Sixteen bars of every take: the bass, the chords, the pad and the arpeggio over six charts in
//! six idioms at three densities, and the kit in eight grooves at three densities and three
//! intensities — 24 seeds of each unless `--seeds` says otherwise. A chart is written once as a
//! `chart` line, with its chords in ticks and semitones, so that the reader can tell which chord
//! a note was written over; every take after it is a `take` line.
//!
//! `--sweep` writes the two dials that are read as a curve instead: the chords' density from
//! 0.6 to 1.0, where the upper quarter of the dial fills the bar in, and the kit's from 0.3 to
//! 0.8 round its neutral middle.
//!
//! Development tooling: `docs/evaluation.md` has the account, and no release build carries it.

use auris_compose::write_phrase;
use auris_core::harmony::Harmony;
use auris_core::theory::chart::Chart;
use auris_core::theory::key::Key;
use auris_core::time::{SignatureMap, Ticks, TimeSignature};
use auris_core::{ClipPreset, ClipRecipe, PerformanceStyle};

/// How many bars every take is.
const BARS: usize = 16;

/// What one take was written with, beside its notes.
struct Take<'a> {
    preset: ClipPreset,
    chart: &'a str,
    style: &'a str,
    groove: &'a str,
    recipe: &'a ClipRecipe,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let sweep = arguments.iter().any(|argument| argument == "--sweep");
    let seeds = arguments
        .iter()
        .position(|argument| argument == "--seeds")
        .and_then(|at| arguments.get(at + 1))
        .and_then(|count| count.parse::<u64>().ok())
        .unwrap_or(24);
    let meter = TimeSignature::new(4, 4);
    let length = meter.ticks_per_bar() * BARS as i64;
    let harmony_of = |chart: &str| {
        let mut harmony = Harmony::in_key(Key::parse("C major").expect("a key"));
        harmony.stamp(
            &Chart::parse(chart).expect("a chart"),
            Ticks::ZERO,
            BARS,
            meter,
        );
        harmony
    };
    let write = |harmony: &Harmony, take: Take<'_>| {
        let notes = write_phrase(harmony, Ticks::ZERO, length, meter, take.recipe, None);
        emit(&take, &notes);
    };

    if sweep {
        let harmony = harmony_of("@axis");
        for (style_name, style) in [
            ("none", None),
            ("jazz-trio", Some(PerformanceStyle::JazzTrio)),
        ] {
            for density in [0.6f32, 0.7, 0.75, 0.76, 0.8, 0.9, 1.0] {
                for seed in 0..seeds {
                    let mut recipe = ClipRecipe::new(ClipPreset::Chords, seed);
                    recipe.style = style;
                    recipe.density = density;
                    write(
                        &harmony,
                        Take {
                            preset: ClipPreset::Chords,
                            chart: "@axis",
                            style: style_name,
                            groove: "basic-rock",
                            recipe: &recipe,
                        },
                    );
                }
            }
        }
        for density in [0.3f32, 0.4, 0.45, 0.5, 0.55, 0.6, 0.7, 0.8] {
            for intensity in [0.55f32, 0.7, 0.9] {
                for seed in 0..seeds {
                    let mut recipe = ClipRecipe::new(ClipPreset::Drums, seed);
                    recipe.density = density;
                    recipe.intensity = intensity;
                    write(
                        &harmony,
                        Take {
                            preset: ClipPreset::Drums,
                            chart: "@axis",
                            style: "none",
                            groove: "basic-rock",
                            recipe: &recipe,
                        },
                    );
                }
            }
        }
        return;
    }

    let charts = [
        "@axis",
        "@royal-road",
        "@marusa",
        "@koakuma",
        "@canon",
        "@komuro",
    ];
    let styles: [(&str, Option<PerformanceStyle>); 6] = [
        ("none", None),
        ("pop-band", Some(PerformanceStyle::PopBand)),
        ("rock", Some(PerformanceStyle::Rock)),
        ("city-pop", Some(PerformanceStyle::CityPop)),
        ("jazz-trio", Some(PerformanceStyle::JazzTrio)),
        ("synthwave", Some(PerformanceStyle::Synthwave)),
    ];
    let grooves = [
        "basic-rock",
        "eight-beat",
        "sixteen-beat",
        "four-on-the-floor",
        "shuffle",
        "breakbeat",
        "bossa-nova",
        "half-time",
    ];
    let densities = [0.3f32, 0.5, 0.8];

    for chart in charts {
        let harmony = harmony_of(chart);
        let chords: Vec<serde_json::Value> = harmony
            .events_in(Ticks::ZERO, length, &SignatureMap::constant(meter))
            .iter()
            .map(|event| {
                serde_json::json!([
                    event.start.raw(),
                    event.length.raw(),
                    event.chord.root.semitones(),
                    event.chord.bass_class().semitones(),
                    event
                        .chord
                        .classes()
                        .iter()
                        .map(|class| class.semitones())
                        .collect::<Vec<_>>(),
                ])
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({"kind": "chart", "chart": chart, "chords": chords})
        );

        for preset in [
            ClipPreset::Bass,
            ClipPreset::Chords,
            ClipPreset::Pad,
            ClipPreset::Arp,
        ] {
            for (style_name, style) in styles {
                for density in densities {
                    for seed in 0..seeds {
                        let mut recipe = ClipRecipe::new(preset, seed);
                        recipe.style = style;
                        recipe.density = density;
                        write(
                            &harmony,
                            Take {
                                preset,
                                chart,
                                style: style_name,
                                groove: "basic-rock",
                                recipe: &recipe,
                            },
                        );
                    }
                }
            }
        }
        // The kit does not read the chart, so it is written once.
        if chart != charts[0] {
            continue;
        }
        for groove in grooves {
            for density in densities {
                for intensity in [0.45f32, 0.7, 0.9] {
                    for seed in 0..seeds {
                        let mut recipe = ClipRecipe::new(ClipPreset::Drums, seed);
                        recipe.density = density;
                        recipe.intensity = intensity;
                        recipe.groove = groove.to_string();
                        write(
                            &harmony,
                            Take {
                                preset: ClipPreset::Drums,
                                chart,
                                style: "none",
                                groove,
                                recipe: &recipe,
                            },
                        );
                    }
                }
            }
        }
    }
}

/// One take as a line: what it was written with, and its notes as
/// `[start, length, pitch, velocity, drum voice]`.
fn emit(take: &Take<'_>, notes: &[auris_core::Note]) {
    let notes: Vec<serde_json::Value> = notes
        .iter()
        .map(|note| {
            serde_json::json!([
                note.start.raw(),
                note.length.raw(),
                note.pitch,
                (note.velocity * 1000.0).round() / 1000.0,
                note.drum_voice,
            ])
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "kind": "take",
            "preset": take.preset.name(),
            "chart": take.chart,
            "style": take.style,
            "density": take.recipe.density,
            "seed": take.recipe.seed,
            "groove": take.groove,
            "intensity": take.recipe.intensity,
            "notes": notes,
        })
    );
}
