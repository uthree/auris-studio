//! Compact images of measured sound and authored notes; never inferred listening reports.
use auris_session::audio_inspection::Inspection;
use base64::Engine;
use image::{ImageEncoder, Rgb, RgbImage};

/// Text measurements and one base64 PNG, both referring to the same document revision.
pub struct Presentation {
    /// Numeric observations and the image's explicit axes/colour conventions.
    pub text: String,
    /// PNG encoded in memory, with no path or persistent cache.
    pub png: String,
}

/// Width of the y-axis label margin, left of both panels.
const LEFT: u32 = 64;
/// Margin right of the panels, for the last time label.
const RIGHT: u32 = 16;
/// Margin above the mel panel, for half of its top label.
const TOP: u32 = 8;
/// Width of both panels: one pixel per time column.
const PLOT_WIDTH: u32 = 512;
/// Rows per mel band.
const MEL_ROWS: u32 = 3;
/// Height of the mel panel.
const MEL_HEIGHT: u32 = 64 * MEL_ROWS;
/// Rows between the mel panel and the piano roll.
const GAP: u32 = 8;
/// Height of the piano roll panel.
const ROLL_HEIGHT: u32 = 256;
/// Margin below the roll, for the bar ticks and their numbers.
const BOTTOM: u32 = 26;
/// Picture width, axis margins included.
pub const WIDTH: u32 = LEFT + PLOT_WIDTH + RIGHT;
/// Picture height, axis margins included.
pub const HEIGHT: u32 = TOP + MEL_HEIGHT + GAP + ROLL_HEIGHT + BOTTOM;
/// First row of the piano roll.
const ROLL_TOP: u32 = TOP + MEL_HEIGHT + GAP;
/// Background of the roll where no note is.
const ROLL_BACKGROUND: Rgb<u8> = Rgb([20, 25, 34]);

fn mel(hz: f64) -> f64 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// The fractional mel band `hz` falls in, from the low-to-high band centres.
fn band_of(frequencies: &[f64], hz: f64) -> Option<f64> {
    let (first, last) = (mel(*frequencies.first()?), mel(*frequencies.last()?));
    let at = mel(hz);
    // Equal ends (or NaN) leave no scale to place a label on.
    if last.partial_cmp(&first) != Some(std::cmp::Ordering::Greater) || at < first || at > last {
        return None;
    }
    // HTK centres are evenly spaced in mel, so the band index is linear in it.
    Some((at - first) / (last - first) * (frequencies.len() - 1) as f64)
}

/// Round frequencies inside the mel panel, at their row, roundest first.
fn hz_ticks(frequencies: &[f64]) -> Vec<(f64, String)> {
    let bands = frequencies.len() as f64;
    let tier = |mantissas: &[f64]| {
        let mut ticks = Vec::new();
        for exponent in 0..5 {
            for mantissa in mantissas {
                let hz = mantissa * 10.0_f64.powi(exponent);
                if let Some(band) = band_of(frequencies, hz) {
                    // Highest band at the top; the label sits on the band's middle row.
                    let y = f64::from(TOP)
                        + (bands - 1.0 - band) * f64::from(MEL_ROWS)
                        + f64::from(MEL_ROWS - 1) / 2.0;
                    ticks.push((y, crate::axes::hz_label(hz)));
                }
            }
        }
        ticks
    };
    crate::axes::sparse(
        vec![tier(&[1.0]), tier(&[2.0, 5.0]), tier(&[3.0])],
        crate::axes::MIN_LABEL_GAP,
    )
}

/// Paint one bounded picture with a mel panel above a MIDI score panel, both with labelled axes.
pub fn present(report: &Inspection) -> Result<Presentation, String> {
    let bands = report.frequencies.len();
    if report.columns == 0
        || report.columns > 512
        || bands != 64
        || report.mel_db.len() != report.columns * bands
    {
        return Err("Invalid mel image dimensions".into());
    }
    let seconds = report.measurements["seconds"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or("Invalid inspection duration")?;
    let mut pixels = RgbImage::from_pixel(WIDTH, HEIGHT, crate::axes::MARGIN);
    for x in 0..PLOT_WIDTH {
        let column = x as usize * report.columns / PLOT_WIDTH as usize;
        for y in 0..MEL_HEIGHT {
            let band = 63 - (y / MEL_ROWS) as usize;
            let level = report.mel_db[column * bands + band];
            if !level.is_finite() {
                return Err("Non-finite mel power".into());
            }
            pixels.put_pixel(
                LEFT + x,
                TOP + y,
                crate::palette::heat((level + 90.0) / 90.0),
            );
        }
    }
    let (low, high) = (u32::from(report.low_pitch), u32::from(report.high_pitch));
    if low >= high || high > 127 {
        return Err("Invalid piano roll pitch range".into());
    }
    for x in LEFT..LEFT + PLOT_WIDTH {
        for y in ROLL_TOP..ROLL_TOP + ROLL_HEIGHT {
            pixels.put_pixel(x, y, ROLL_BACKGROUND);
        }
    }
    let span = high - low + 1;
    // Each semitone gets a band of rows; integer edges keep every band at least one row tall.
    let rows = |pitch: u32| {
        let top = ROLL_TOP + (high - pitch) * ROLL_HEIGHT / span;
        let bottom = (ROLL_TOP + (high - pitch + 1) * ROLL_HEIGHT / span).max(top + 1);
        top..bottom
    };
    let x_of = |at: f64| f64::from(LEFT) + at / seconds * f64::from(PLOT_WIDTH);
    let bar_lines: Vec<_> = report
        .bar_starts
        .iter()
        .filter(|bar| bar.seconds.is_finite() && (0.0..=seconds).contains(&bar.seconds))
        .collect();
    for bar in &bar_lines {
        let x = (x_of(bar.seconds).round() as u32).min(LEFT + PLOT_WIDTH - 1);
        for y in ROLL_TOP..ROLL_TOP + ROLL_HEIGHT {
            pixels.put_pixel(x, y, Rgb([44, 50, 62]));
        }
    }
    for pitch in (low..=high).filter(|pitch| pitch % 12 == 0) {
        let y = rows(pitch).end - 1;
        for x in LEFT..LEFT + PLOT_WIDTH {
            pixels.put_pixel(x, y, Rgb([56, 62, 76]));
        }
    }
    let colours = [
        [90, 185, 255],
        [255, 170, 80],
        [120, 225, 150],
        [220, 130, 230],
        [250, 220, 100],
        [120, 220, 225],
    ];
    for note in &report.notes {
        if note.pitch > 127 || !note.start_seconds.is_finite() || !note.end_seconds.is_finite() {
            return Err("Invalid score note".into());
        }
        let first = ((note.start_seconds / seconds * 512.0).floor() as u32).min(511);
        let last = ((note.end_seconds / seconds * 512.0).ceil() as u32).clamp(first + 1, 512);
        if !(low..=high).contains(&u32::from(note.pitch)) {
            continue;
        }
        for x in first..last {
            for y in rows(u32::from(note.pitch)) {
                pixels.put_pixel(
                    LEFT + x,
                    y,
                    Rgb(colours[note.track as usize % colours.len()]),
                );
            }
        }
    }
    // Axes: frequency beside the mel panel, pitch beside the roll, bar numbers under both.
    crate::axes::frame(&mut pixels, LEFT, TOP, LEFT + PLOT_WIDTH, TOP + MEL_HEIGHT);
    crate::axes::frame(
        &mut pixels,
        LEFT,
        ROLL_TOP,
        LEFT + PLOT_WIDTH,
        ROLL_TOP + ROLL_HEIGHT,
    );
    let hz_ticks = hz_ticks(&report.frequencies);
    for (y, label) in &hz_ticks {
        crate::axes::tick_left(&mut pixels, LEFT, *y);
        crate::axes::label_left(&mut pixels, LEFT - 8, *y, 0, ROLL_TOP - 2, label);
    }
    let pitch_tiers: Vec<Vec<(f64, String)>> =
        [&[0][..], &[7], &[4], &[2, 5, 9, 11], &[1, 3, 6, 8, 10]]
            .iter()
            .map(|classes| {
                (low..=high)
                    .filter(|pitch| classes.contains(&(pitch % 12)))
                    .map(|pitch| {
                        let band = rows(pitch);
                        let centre = f64::from(band.start + band.end - 1) / 2.0;
                        (centre, crate::axes::pitch_label(pitch as u8))
                    })
                    .collect()
            })
            .collect();
    let pitch_ticks = crate::axes::sparse(pitch_tiers, crate::axes::MIN_LABEL_GAP);
    for (y, label) in &pitch_ticks {
        crate::axes::tick_left(&mut pixels, LEFT, *y);
        crate::axes::label_left(
            &mut pixels,
            LEFT - 8,
            *y,
            ROLL_TOP - 4,
            ROLL_TOP + ROLL_HEIGHT + 4,
            label,
        );
    }
    // Every bar gets a tick; numbers go on every bar, or every 2nd, 4th... when they would crowd.
    let below = ROLL_TOP + ROLL_HEIGHT;
    let first_bar = bar_lines.first().map_or(1, |bar| bar.bar);
    let closest = bar_lines
        .windows(2)
        .map(|pair| x_of(pair[1].seconds) - x_of(pair[0].seconds))
        .fold(f64::INFINITY, f64::min);
    let every = [1, 2, 4, 8, 16, 32]
        .into_iter()
        .find(|step| closest * f64::from(*step) >= 30.0)
        .unwrap_or(32);
    let mut bar_ticks = Vec::new();
    for bar in &bar_lines {
        let x = x_of(bar.seconds);
        crate::axes::tick_below(&mut pixels, x, below);
        if (bar.bar - first_bar) % every == 0 {
            crate::axes::label_below(&mut pixels, x, below + 9, LEFT, &bar.bar.to_string());
            bar_ticks.push(bar.bar);
        }
    }
    crate::axes::draw_text(
        &mut pixels,
        8,
        i64::from(below + 9),
        "bar",
        crate::axes::INK,
    );
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            pixels.as_raw(),
            WIDTH,
            HEIGHT,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    let mut text = format!(
        "Rendered audio inspection, document revision {}.\n{}\nImage: {WIDTH}x{HEIGHT}. Both panels span x={LEFT}..{plot_right}; x is elapsed seconds 0..{seconds:.3}, linear in time. The numbers under the roll are bar numbers at bar starts ({bars}); every bar line has a tick and a faint vertical line in the roll. Left margin labels: frequency in Hz beside the mel panel (k = kHz, labels {hz}), note names beside the roll (C4 = MIDI 60, labels {names}); each label sits on its row. Top panel y={TOP}..{mel_bottom}: mel power over mel_hz_range, high frequencies at top, low at bottom; colour ramp {ramp}, where 0 is -90 dB and 1.0 is 0 dB (pale yellow), fixed reference with no per-image normalization. Bottom panel y={ROLL_TOP}..{roll_bottom}: authored piano roll, MIDI {high} at top, {low} at bottom, {px:.1} px per semitone; a grey line runs along the bottom of each C, so the row just above it is that C. Notes outside the roll's pitches are left out of the picture (notes_outside_piano_roll). Track colour cycles by numeric ID modulo 6: blue, orange, green, purple, yellow, cyan.\nMel centre frequencies (low to high, Hz): {:?}.\nThis is measured audio plus a score image, not listening. Do not infer pleasing music, exact instruments, clipping, or masking from the picture alone. Use measurements for level claims; the score is before performance transforms. Harmony is stored project metadata, not detected from the audio. Bar-start harmony samples can omit mid-bar changes.",
        report.revision,
        report.measurements,
        report.frequencies,
        plot_right = LEFT + PLOT_WIDTH,
        mel_bottom = TOP + MEL_HEIGHT,
        roll_bottom = ROLL_TOP + ROLL_HEIGHT,
        bars = if bar_ticks.is_empty() {
            "none in range".to_string()
        } else {
            bar_ticks
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        },
        hz = labels(&hz_ticks),
        names = labels(&pitch_ticks),
        ramp = crate::palette::DESCRIPTION,
        px = f64::from(ROLL_HEIGHT) / f64::from(span),
    );
    let score = serde_json::to_string(&report.notes.iter().take(64).collect::<Vec<_>>())
        .map_err(|e| e.to_string())?;
    text.push_str(&format!(
        "\nFirst {} authored notes (full score count is in measurements): {score}",
        report.notes.len().min(64)
    ));
    Ok(Presentation {
        text,
        png: base64::engine::general_purpose::STANDARD.encode(png),
    })
}

/// The labels of `ticks`, top to bottom, for the caption.
fn labels(ticks: &[(f64, String)]) -> String {
    if ticks.is_empty() {
        return "none".into();
    }
    ticks
        .iter()
        .map(|(_, label)| label.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_uses_a_fixed_scale_and_contains_the_score_without_touching_a_file() {
        let mut report = Inspection {
            revision: 7,
            measurements: serde_json::json!({"seconds":2.0}),
            columns: 2,
            frequencies: vec![1000.0; 64],
            mel_db: vec![-90.0; 128],
            notes: vec![auris_session::audio_inspection::ScoreNote {
                track: 1,
                pitch: 60,
                start_seconds: 0.5,
                end_seconds: 1.0,
            }],
            low_pitch: 0,
            high_pitch: 127,
            bar_starts: Vec::new(),
        };
        report.mel_db[64..].fill(0.0);
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(image.dimensions(), (WIDTH, HEIGHT));
        assert_eq!(image.get_pixel(LEFT, TOP), &crate::palette::heat(0.0));
        assert_eq!(image.get_pixel(LEFT + 511, TOP), &crate::palette::heat(1.0));
        // MIDI 60 of 0..127 starts 67 semitones down: 67 * 2 rows.
        assert_eq!(
            image.get_pixel(LEFT + 130, ROLL_TOP + 134).0,
            [255, 170, 80]
        );
        assert!(presentation.text.contains("revision 7"));
        assert!(!presentation.text.contains(&presentation.png));
    }

    #[test]
    fn a_narrow_pitch_range_gives_each_semitone_more_rows_and_drops_notes_outside_it() {
        let note = |pitch| auris_session::audio_inspection::ScoreNote {
            track: 0,
            pitch,
            start_seconds: 0.0,
            end_seconds: 2.0,
        };
        let report = Inspection {
            revision: 1,
            measurements: serde_json::json!({"seconds":2.0}),
            columns: 1,
            frequencies: vec![1000.0; 64],
            mel_db: vec![-90.0; 64],
            notes: vec![note(48), note(71), note(30)],
            low_pitch: 48,
            high_pitch: 71,
            bar_starts: Vec::new(),
        };
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let blue = [90, 185, 255];
        // 24 semitones over 256 rows: B4 owns the top rows, C3 the bottom ones.
        assert_eq!(image.get_pixel(LEFT + 100, ROLL_TOP).0, blue);
        assert_eq!(image.get_pixel(LEFT + 100, ROLL_TOP + 9).0, blue);
        assert_eq!(image.get_pixel(LEFT + 100, ROLL_TOP + 255).0, blue);
        assert_eq!(
            image.get_pixel(LEFT + 100, ROLL_TOP + 122),
            &ROLL_BACKGROUND
        );
        assert!(presentation.text.contains("MIDI 71 at top, 48 at bottom"));
        let reversed = Inspection {
            low_pitch: 71,
            high_pitch: 48,
            ..report
        };
        assert!(present(&reversed).is_err());
    }

    /// Ink in the rectangle, which is margin everywhere a label is not.
    fn inked(image: &RgbImage, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> bool {
        x.flat_map(|x| y.clone().map(move |y| (x, y)))
            .any(|(x, y)| image.get_pixel(x, y) == &crate::axes::INK)
    }

    #[test]
    fn axes_name_round_frequencies_note_names_and_bar_numbers_at_their_places() {
        // HTK centres evenly spaced in mel from 0 Hz to 20 kHz, as the analyser makes them.
        let top = mel(20_000.0);
        let frequencies: Vec<f64> = (1..=64)
            .map(|i| 700.0 * (10.0_f64.powf(top * f64::from(i) / 65.0 / 2595.0) - 1.0))
            .collect();
        let report = Inspection {
            revision: 1,
            measurements: serde_json::json!({"seconds":8.0}),
            columns: 1,
            frequencies: frequencies.clone(),
            mel_db: vec![-90.0; 64],
            notes: Vec::new(),
            low_pitch: 48,
            high_pitch: 71,
            bar_starts: (0..4)
                .map(|i| auris_session::audio_inspection::BarStart {
                    bar: 5 + i,
                    seconds: f64::from(i) * 2.0,
                })
                .collect(),
        };
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        // 1 kHz is labelled beside its own band.
        let band = band_of(&frequencies, 1000.0).unwrap();
        let y = TOP + ((63.0 - band) * 3.0 + 1.0).round() as u32;
        assert!(inked(&image, 0..LEFT, y - 3..y + 3));
        assert!(
            presentation
                .text
                .contains("labels 10k, 5k, 3k, 2k, 1k, 500, 100"),
            "{}",
            presentation.text
        );
        // C3 and C4 name the bottom and the middle of a two-octave roll.
        assert!(presentation.text.contains("C4"));
        let c3 = ROLL_TOP + ROLL_HEIGHT - 6;
        assert!(inked(&image, 0..LEFT, c3 - 2..c3 + 2));
        // Bar 5 starts at the left edge and bar 7 halfway along, each numbered under the roll.
        let below = ROLL_TOP + ROLL_HEIGHT + 9;
        assert!(inked(&image, LEFT..LEFT + 8, below..below + 14));
        let middle = LEFT + PLOT_WIDTH / 2;
        assert!(inked(&image, middle - 6..middle + 6, below..below + 14));
        assert_eq!(image.get_pixel(middle, ROLL_TOP + 10).0, [44, 50, 62]);
        assert!(presentation.text.contains("bar starts (5, 6, 7, 8)"));
    }
}
