//! The modulation spectrogram as a model reads it: one picture and the numbers behind it.
use crate::audio_inspection::Presentation;
use auris_session::modulation::{COLUMNS, ModulationReport, beat_label, column_rate};
use base64::Engine;
use image::{ImageEncoder, Rgb, RgbImage};

/// Picture width: four pixels per column.
const WIDTH: u32 = (COLUMNS as u32) * 4;
/// Depth drawn at full brightness; deeper swings saturate.
const FULL_SCALE: f32 = 1.2;
/// Rows between two regions' panels.
const GAP: u32 = 4;

/// A fixed colour ramp, black at no modulation through purple and orange to pale yellow.
fn colour(depth: f32) -> Rgb<u8> {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.2, [80.0, 20.0, 120.0]),
        (0.45, [200.0, 60.0, 70.0]),
        (0.75, [250.0, 160.0, 40.0]),
        (1.0, [255.0, 250.0, 190.0]),
    ];
    let value = (depth / FULL_SCALE).clamp(0.0, 1.0);
    let upper = STOPS
        .iter()
        .position(|(at, _)| *at >= value)
        .unwrap_or(4)
        .max(1);
    let (a, from) = STOPS[upper - 1];
    let (b, to) = STOPS[upper];
    let t = (value - a) / (b - a).max(f32::EPSILON);
    Rgb(std::array::from_fn(|i| {
        (from[i] + (to[i] - from[i]) * t).round() as u8
    }))
}

/// The x position of `rate` cycles per beat.
fn x_of(rate: f64) -> Option<u32> {
    let lowest = column_rate(0) / 2.0_f64.powf(1.0 / 32.0);
    let x = (rate / lowest).log2() * 16.0 * 4.0;
    (x.is_finite() && x >= 0.0 && x < f64::from(WIDTH)).then_some(x as u32)
}

/// Paint every region as a panel of octave bands and summarise the peaks in text.
pub fn present(report: &ModulationReport) -> Result<Presentation, String> {
    let regions = report.regions.len();
    if regions == 0 || regions > 16 {
        return Err("Invalid modulation report".into());
    }
    let bands = report.regions[0].bands.len();
    if bands == 0
        || report
            .regions
            .iter()
            .any(|region| region.bands.len() != bands || region.depth.len() != bands * COLUMNS)
    {
        return Err("Invalid modulation image dimensions".into());
    }
    let row = ((720 / regions as u32).saturating_sub(GAP) / bands as u32).clamp(4, 24);
    let panel = row * bands as u32;
    let height = panel * regions as u32 + GAP * (regions as u32 - 1);
    let mut pixels = RgbImage::from_pixel(WIDTH, height, Rgb([40, 45, 54]));
    for (index, region) in report.regions.iter().enumerate() {
        let top = index as u32 * (panel + GAP);
        for band in 0..bands {
            let y0 = top + (bands - 1 - band) as u32 * row;
            for column in 0..COLUMNS {
                let depth = region.depth[band * COLUMNS + column];
                if !depth.is_finite() {
                    return Err("Non-finite modulation depth".into());
                }
                let pixel = colour(depth);
                for dx in 0..4 {
                    for dy in 0..row {
                        pixels.put_pixel(column as u32 * 4 + dx, y0 + dy, pixel);
                    }
                }
            }
        }
        // Dotted guides so the data underneath stays readable.
        let bar = 1.0 / f64::from(region.beats_per_bar.max(1));
        let guides = [
            (bar, [90, 220, 235]),
            (1.0, [120, 225, 150]),
            (2.0, [120, 225, 150]),
            (4.0, [120, 225, 150]),
            (3.0, [235, 120, 230]),
        ];
        for (rate, rgb) in guides {
            if let Some(x) = x_of(rate) {
                for y in (top..top + panel).step_by(3) {
                    pixels.put_pixel(x, y, Rgb(rgb));
                }
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            pixels.as_raw(),
            WIDTH,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    let mut summary = String::new();
    for region in &report.regions {
        let mut header = serde_json::to_value(region).map_err(|e| e.to_string())?;
        if let Some(object) = header.as_object_mut() {
            object.remove("depth");
            object.remove("bands");
            object.insert("beat".into(), beat_label(region.beat_ticks).into());
        }
        summary.push_str(&format!("\nRegion {header}\n"));
        for band in &region.bands {
            let Some(level) = band.level_db else {
                summary.push_str(&format!("  {:>5} Hz: silent\n", band.centre_hz));
                continue;
            };
            let peaks: Vec<String> = band
                .peaks
                .iter()
                .map(|peak| match &peak.rhythm {
                    Some(name) => format!("{}={:.2}", name, peak.depth),
                    None => format!("{}/beat={:.2}", peak.cycles_per_beat, peak.depth),
                })
                .collect();
            let rhythm: Vec<String> = band
                .rhythm
                .iter()
                .map(|(name, depth)| format!("{name} {depth:.2}"))
                .collect();
            summary.push_str(&format!(
                "  {:>5} Hz {level:.1} dB | peaks {} | rhythm {}\n",
                band.centre_hz,
                if peaks.is_empty() {
                    "none".to_string()
                } else {
                    peaks.join(", ")
                },
                rhythm.join(", ")
            ));
        }
    }
    let order: Vec<String> = report
        .regions
        .iter()
        .map(|region| match (&region.section, region.instance) {
            (Some(label), Some(instance)) => format!(
                "{label} #{instance} (bars {}..{})",
                region.start_bar, region.end_bar
            ),
            _ => format!("bars {}..{}", region.start_bar, region.end_bar),
        })
        .collect();
    let text = format!(
        "Modulation spectrogram, document revision {revision}, {target}.\n\
         Image: {WIDTH}x{height}, {regions} panel(s) top to bottom: {order}. Each panel has {bands} octave bands, \
         lowest (31.5 Hz) at the bottom row and highest (16 kHz) at the top. x is modulation rate in cycles per beat \
         on a log axis, 1/16 at the left to 16 at the right, 16 columns per octave; the beat is each region's felt beat \
         (beat_ticks, 960 = quarter note). Colour is depth on a fixed scale: black 0, purple ~0.25, orange ~0.55, light orange ~0.9, pale \
         yellow {FULL_SCALE} or more, never normalised per image, so panels compare directly. Dotted guides: cyan once \
         per bar, green 1, 2 and 4 per beat, magenta 3 per beat (triplets).\n\
         Reading: depth is the band envelope's swing at that rate as a fraction of its mean; a peak at 1 per beat with \
         depth 0.5 means that register's loudness rises and falls by about half on every beat. Short separated hits \
         read above 1 and also show at whole multiples of their rate, so the slowest strong peak of a band is its pulse. \
         rhythm lists the depth at score rates (per_4_bars .. 8_per_beat); peaks are the three deepest local maxima per band. Rates below \
         lowest_resolved_cycles_per_beat cannot be told from a steady level in that region. A tempo change inside a \
         region is followed exactly, because the envelopes were resampled on the beat grid. This is measured audio, \
         not listening: depth does not say whether a groove feels good, and a band can pulse because of one loud \
         instrument.\nMeasurements, one line per band (centre, level, peaks, rhythm depths):{summary}",
        revision = report.revision,
        target = report.track.map_or_else(
            || "the mix".to_string(),
            |track| format!("track {track} soloed")
        ),
        order = order.join("; "),
    );
    let mut text = text;
    if report.omitted_regions > 0 {
        text.push_str(&format!(
            "\n{} further region(s) were not measured: a call covers at most 16 regions and 600 seconds. Ask again for the rest.",
            report.omitted_regions
        ));
    }
    Ok(Presentation {
        text,
        png: base64::engine::general_purpose::STANDARD.encode(png),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::modulation::{BandModulation, RegionModulation};

    fn region(depth: f32) -> RegionModulation {
        RegionModulation {
            section: Some("サビ".into()),
            instance: Some(1),
            start_bar: 9,
            end_bar: 17,
            start_tick: 30720,
            end_tick: 61440,
            seconds: 16.0,
            beats: 32.0,
            beat_ticks: 960,
            beats_per_bar: 4,
            meter: "4/4".into(),
            meter_changes: false,
            bpm_range: (120.0, 120.0),
            lowest_resolved_cycles_per_beat: 0.063,
            silent: false,
            bands: (0..10)
                .map(|_| BandModulation {
                    centre_hz: 63.0,
                    level_db: Some(-20.0),
                    peaks: Vec::new(),
                    rhythm: [("1_per_beat".to_string(), f64::from(depth))].into(),
                })
                .collect(),
            depth: vec![depth; 10 * COLUMNS],
        }
    }

    #[test]
    fn panels_stack_on_a_fixed_scale_and_the_text_omits_the_raw_image() {
        let report = ModulationReport {
            revision: 3,
            track: None,
            regions: vec![region(0.0), region(FULL_SCALE)],
            omitted_regions: 1,
        };
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        // 720 / 2 - 4 = 356 rows per panel, 35 per band, clamped to 24.
        assert_eq!(image.dimensions(), (WIDTH, 240 * 2 + GAP));
        assert_eq!(image.get_pixel(1, 1).0, [0, 0, 0]);
        assert_eq!(image.get_pixel(1, 240 + GAP + 1).0, [255, 250, 190]);
        // The 1-per-beat guide sits four octaves above 1/16.
        let x = x_of(1.0).unwrap();
        assert_eq!(x, 256);
        assert_eq!(image.get_pixel(x, 0).0, [120, 225, 150]);
        assert!(presentation.text.contains("サビ #1 (bars 9..17)"));
        assert!(presentation.text.contains("\"beat\":\"quarter\""));
        assert!(
            presentation
                .text
                .contains("63 Hz -20.0 dB | peaks none | rhythm 1_per_beat 1.20")
        );
        assert!(!presentation.text.contains("\"depth\":["));
        assert!(presentation.text.contains("1 further region"));
    }

    #[test]
    fn malformed_reports_are_refused() {
        let mut broken = region(0.1);
        broken.depth.pop();
        let report = ModulationReport {
            revision: 1,
            track: Some(2),
            regions: vec![broken],
            omitted_regions: 0,
        };
        assert!(present(&report).is_err());
    }
}
