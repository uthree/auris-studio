//! The modulation spectrogram as a model reads it: one picture and the numbers behind it.
use crate::audio_inspection::Presentation;
use auris_session::modulation::{Bands, COLUMNS, ModulationReport, beat_label, column_rate};
use base64::Engine;
use image::{ImageEncoder, Rgb, RgbImage};

/// Width of the data: four pixels per column.
const PLOT_WIDTH: u32 = (COLUMNS as u32) * 4;
/// Width of the band label margin, left of every panel.
const LEFT: u32 = 64;
/// Margin right of the panels, for the last rate label.
const RIGHT: u32 = 16;
/// Picture width.
const WIDTH: u32 = LEFT + PLOT_WIDTH + RIGHT;
/// Rows above each panel for its heading.
const HEADER: u32 = 20;
/// Rows between one panel and the next region's heading.
const GAP: u32 = 4;
/// Margin under the last panel, for the rate ticks and their labels.
const BOTTOM: u32 = 26;
/// Depth drawn at full brightness; deeper swings saturate.
const FULL_SCALE: f32 = 1.2;

/// How far `rate` cycles per beat sits from the left edge of the data, in pixels.
fn position(rate: f64) -> f64 {
    let lowest = column_rate(0) / 2.0_f64.powf(1.0 / 32.0);
    (rate / lowest).log2() * 16.0 * 4.0
}

/// The column of pixels, within the data, holding `rate` cycles per beat.
fn x_of(rate: f64) -> Option<u32> {
    let x = position(rate);
    (x.is_finite() && x >= 0.0 && x < f64::from(PLOT_WIDTH)).then_some(x as u32)
}

/// The unit of the rate axis, written at the right of the first panel's heading.
const RATE_UNIT: &str = "x: cycles/beat";
/// The rates labelled under the panels, as (rate, label).
const RATE_LABELS: [(f64, &str); 9] = [
    (1.0 / 16.0, "1/16"),
    (1.0 / 8.0, "1/8"),
    (1.0 / 4.0, "1/4"),
    (1.0 / 2.0, "1/2"),
    (1.0, "1"),
    (2.0, "2"),
    (4.0, "4"),
    (8.0, "8"),
    (16.0, "16"),
];

fn rhythm_line(rhythm: &std::collections::BTreeMap<String, f64>) -> String {
    rhythm
        .iter()
        .map(|(name, depth)| format!("{name} {depth:.2}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Paint every region as a panel of frequency bands and summarise the peaks in text.
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
    let row = ((720 / regions as u32).saturating_sub(GAP + HEADER) / bands as u32).clamp(2, 24);
    let panel = row * bands as u32;
    let stride = HEADER + panel + GAP;
    let height = stride * regions as u32 - GAP + BOTTOM;
    let mut pixels = RgbImage::from_pixel(WIDTH, height, crate::axes::MARGIN);
    let mut band_labels = Vec::new();
    for (index, region) in report.regions.iter().enumerate() {
        let top = index as u32 * stride + HEADER;
        crate::axes::draw_text(
            &mut pixels,
            i64::from(LEFT),
            i64::from(top - HEADER + 2),
            &format!(
                "{}: bars {}..{}",
                index + 1,
                region.start_bar,
                region.end_bar
            ),
            crate::axes::INK,
        );
        for band in 0..bands {
            let y0 = top + (bands - 1 - band) as u32 * row;
            for column in 0..COLUMNS {
                let depth = region.depth[band * COLUMNS + column];
                if !depth.is_finite() {
                    return Err("Non-finite modulation depth".into());
                }
                let pixel = crate::palette::heat(depth / FULL_SCALE);
                for dx in 0..4 {
                    for dy in 0..row {
                        pixels.put_pixel(LEFT + column as u32 * 4 + dx, y0 + dy, pixel);
                    }
                }
            }
        }
        // Dotted guides so the data underneath stays readable.
        let bar = 1.0 / f64::from(region.beats_per_bar.max(1));
        let guides = [
            (bar, [90, 220, 235]),
            (1.0, [235, 235, 235]),
            (2.0, [235, 235, 235]),
            (4.0, [235, 235, 235]),
            (3.0, [120, 225, 150]),
        ];
        for (rate, rgb) in guides {
            if let Some(x) = x_of(rate) {
                for y in (top..top + panel).step_by(3) {
                    pixels.put_pixel(LEFT + x, y, Rgb(rgb));
                }
            }
        }
        crate::axes::frame(&mut pixels, LEFT, top, LEFT + PLOT_WIDTH, top + panel);
        // Band centres beside the panel, octave bands first so a crowded panel keeps 1 kHz's.
        let centre = |band: usize| {
            f64::from(top + (bands - 1 - band) as u32 * row) + f64::from(row - 1) / 2.0
        };
        let octave = |hz: f64| ((hz / 1000.0).log2() - (hz / 1000.0).log2().round()).abs() < 0.05;
        let tier = |wanted: bool| {
            region
                .bands
                .iter()
                .enumerate()
                .filter(|(_, band)| octave(band.centre_hz) == wanted)
                .map(|(index, band)| (centre(index), crate::axes::hz_label(band.centre_hz)))
                .collect::<Vec<_>>()
        };
        let ticks = crate::axes::sparse(vec![tier(true), tier(false)], crate::axes::MIN_LABEL_GAP);
        for (y, label) in &ticks {
            crate::axes::tick_left(&mut pixels, LEFT, *y);
            crate::axes::label_left(
                &mut pixels,
                LEFT - 8,
                *y,
                top.saturating_sub(4),
                top + panel + 4,
                label,
            );
        }
        if index == 0 {
            band_labels = ticks.into_iter().map(|(_, label)| label).collect();
        }
    }
    let below = height - BOTTOM;
    for (rate, label) in RATE_LABELS {
        let x = f64::from(LEFT) + position(rate);
        crate::axes::tick_below(&mut pixels, x, below);
        crate::axes::label_below(&mut pixels, x, below + 9, 0, label);
    }
    crate::axes::draw_text(
        &mut pixels,
        i64::from(LEFT + PLOT_WIDTH - crate::axes::text_width(RATE_UNIT)),
        2,
        RATE_UNIT,
        crate::axes::INK,
    );
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            pixels.as_raw(),
            WIDTH,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    // Thirty bands of rates would be several thousand words for a whole song; the score rates
    // are read per octave instead, and the peaks stay per band.
    let group = (bands / 10).max(1);
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
            let peaks = if peaks.is_empty() {
                "none".to_string()
            } else {
                peaks.join(", ")
            };
            let mut line = format!("  {:>5} Hz {level:.1} dB | peaks {peaks}", band.centre_hz);
            if group == 1 {
                line.push_str(&format!(" | rhythm {}", rhythm_line(&band.rhythm)));
            }
            summary.push_str(&line);
            summary.push('\n');
        }
        if group > 1 {
            summary.push_str("  rhythm by octave (deepest of its bands):\n");
            for octave in region.bands.chunks(group) {
                let mut deepest = std::collections::BTreeMap::<String, f64>::new();
                for band in octave {
                    for (name, depth) in &band.rhythm {
                        let slot = deepest.entry(name.clone()).or_insert(0.0);
                        *slot = slot.max(*depth);
                    }
                }
                let (first, last) = (octave[0].centre_hz, octave[octave.len() - 1].centre_hz);
                summary.push_str(&format!(
                    "  {first}-{last} Hz | rhythm {}\n",
                    rhythm_line(&deepest)
                ));
            }
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
         Image: {WIDTH}x{height}, {regions} panel(s) top to bottom: {order}. Each panel is headed \"N: bars a..b\" with its \
         number in that list, and has {bands} {band_kind} bands, lowest ({lowest} Hz) at the bottom row and highest \
         ({highest} Hz) at the top; the left margin names band centres in Hz (k = kHz; {band_labels} on the first panel), \
         each on its row. x is modulation rate in cycles per beat on a log axis over x={LEFT}..{plot_right}, 1/16 at the left \
         to 16 at the right, 16 columns per octave, with every octave ticked and labelled under the last panel \
         (1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8, 16) and the unit written at the top right; the beat is each region's felt beat \
         (beat_ticks, 960 = quarter note). Colour is depth on a fixed scale, the ramp {ramp} \
         where 1.0 is a depth of {FULL_SCALE} or more, never normalised per image, so panels compare directly. Dotted \
         guides: cyan once per bar, white 1, 2 and 4 per beat, green 3 per beat (triplets).\n\
         Reading: depth is the band envelope's swing at that rate as a fraction of its mean; a peak at 1 per beat with \
         depth 0.5 means that register's loudness rises and falls by about half on every beat. Short separated hits \
         read above 1 and also show at whole multiples of their rate, so the slowest strong peak of a band is its pulse. \
         rhythm lists the depth at score rates (per_4_bars .. 8_per_beat); peaks are the three deepest local maxima per band. Rates below \
         lowest_resolved_cycles_per_beat cannot be told from a steady level in that region. A tempo change inside a \
         region is followed exactly, because the envelopes were resampled on the beat grid. This is measured audio, \
         not listening: depth does not say whether a groove feels good, and a band can pulse because of one loud \
         instrument.\nMeasurements, one line per band (centre, level, peaks, rhythm depths):{summary}",
        revision = report.revision,
        band_labels = band_labels.join(", "),
        plot_right = LEFT + PLOT_WIDTH,
        target = report.track.map_or_else(
            || "the mix".to_string(),
            |track| format!("track {track} soloed")
        ),
        order = order.join("; "),
        band_kind = match report.bands {
            Bands::Octave => "octave",
            Bands::ThirdOctave => "third-octave",
        },
        lowest = report.regions[0].bands[0].centre_hz,
        highest = report.regions[0].bands[bands - 1].centre_hz,
        ramp = crate::palette::DESCRIPTION,
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
            bands: Bands::Octave,
            regions: vec![region(0.0), region(FULL_SCALE)],
            omitted_regions: 1,
        };
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        // 720 / 2 - 24 = 336 rows per panel, 33 per band, clamped to 24.
        let stride = HEADER + 240 + GAP;
        assert_eq!(image.dimensions(), (WIDTH, stride * 2 - GAP + BOTTOM));
        assert_eq!(
            image.get_pixel(LEFT + 1, HEADER + 1),
            &crate::palette::heat(0.0)
        );
        assert_eq!(
            image.get_pixel(LEFT + 1, stride + HEADER + 1),
            &crate::palette::heat(1.0)
        );
        // The 1-per-beat guide sits four octaves above 1/16.
        let x = x_of(1.0).unwrap();
        assert_eq!(x, 256);
        assert_eq!(image.get_pixel(LEFT + x, HEADER).0, [235, 235, 235]);
        // Each panel is headed, and the rate under the guide is written below the last panel.
        let inked = |x: std::ops::Range<u32>, y: std::ops::Range<u32>| {
            x.flat_map(|x| y.clone().map(move |y| (x, y)))
                .any(|(x, y)| image.get_pixel(x, y) == &crate::axes::INK)
        };
        assert!(inked(LEFT..LEFT + 40, 0..HEADER));
        assert!(inked(LEFT..LEFT + 40, stride..stride + HEADER));
        let below = stride * 2 - GAP + 9;
        assert!(inked(LEFT + x - 3..LEFT + x + 3, below..below + 14));
        assert!(!inked(LEFT + x + 20..LEFT + x + 40, below..below + 14));
        assert!(presentation.text.contains("headed \"N: bars a..b\""));
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
    fn third_octave_text_reads_rhythm_per_octave_to_stay_short() {
        let mut third = region(0.3);
        third.bands = (0..30).map(|i| third.bands[i % 10].clone()).collect();
        third.depth = vec![0.3; 30 * COLUMNS];
        let report = ModulationReport {
            revision: 1,
            track: None,
            bands: Bands::ThirdOctave,
            regions: vec![third],
            omitted_regions: 0,
        };
        let presentation = present(&report).unwrap();
        assert!(presentation.text.contains("30 third-octave bands"));
        assert_eq!(presentation.text.matches("| rhythm").count(), 10);
        assert!(presentation.text.contains("rhythm by octave"));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        // (720 - 24) / 30 = 23 rows per band.
        assert_eq!(image.dimensions(), (WIDTH, HEADER + 23 * 30 + BOTTOM));
    }

    #[test]
    fn malformed_reports_are_refused() {
        let mut broken = region(0.1);
        broken.depth.pop();
        let report = ModulationReport {
            revision: 1,
            track: Some(2),
            bands: Bands::Octave,
            regions: vec![broken],
            omitted_regions: 0,
        };
        assert!(present(&report).is_err());
    }
}
