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

/// Paint one bounded picture with a mel panel above a MIDI score panel.
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
    let mut pixels = RgbImage::from_pixel(512, 384, Rgb([12, 16, 24]));
    for x in 0..512 {
        let column = x as usize * report.columns / 512;
        for y in 0..128 {
            let band = 63 - y as usize / 2;
            let level = report.mel_db[column * bands + band];
            if !level.is_finite() {
                return Err("Non-finite mel power".into());
            }
            pixels.put_pixel(x, y, crate::palette::heat((level + 90.0) / 90.0));
        }
    }
    let (low, high) = (u32::from(report.low_pitch), u32::from(report.high_pitch));
    if low >= high || high > 127 {
        return Err("Invalid piano roll pitch range".into());
    }
    let span = high - low + 1;
    // Each semitone gets a band of rows; integer edges keep every band at least one row tall.
    let rows = |pitch: u32| {
        let top = 128 + (high - pitch) * 256 / span;
        let bottom = (128 + (high - pitch + 1) * 256 / span).max(top + 1);
        top..bottom
    };
    for pitch in (low..=high).filter(|pitch| pitch % 12 == 0) {
        let y = rows(pitch).end - 1;
        for x in 0..512 {
            pixels.put_pixel(x, y, Rgb([40, 45, 54]));
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
                pixels.put_pixel(x, y, Rgb(colours[note.track as usize % colours.len()]));
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(pixels.as_raw(), 512, 384, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    let mut text = format!(
        "Rendered audio inspection, document revision {}.\n{}\nImage: 512x384. Shared x axis is elapsed seconds 0..{seconds:.3}. Top y=0..127: mel power over mel_hz_range, high frequencies at top, low at bottom; colour ramp {ramp}, where 0 is -90 dB and 1.0 is 0 dB (pale yellow), fixed reference with no per-image normalization. Bottom y=128..383: authored piano roll, MIDI {high} at top, {low} at bottom, {px:.1} px per semitone; a grey line runs along the bottom of each C, so the row just above it is that C. Notes outside the roll's pitches are left out of the picture (notes_outside_piano_roll). Track colour cycles by numeric ID modulo 6: blue, orange, green, purple, yellow, cyan.\nMel centre frequencies (low to high, Hz): {:?}.\nThis is measured audio plus a score image, not listening. Do not infer pleasing music, exact instruments, clipping, or masking from the picture alone. Use measurements for level claims; the score is before performance transforms. Harmony is stored project metadata, not detected from the audio. Bar-start harmony samples can omit mid-bar changes.",
        report.revision,
        report.measurements,
        report.frequencies,
        ramp = crate::palette::DESCRIPTION,
        px = 256.0 / f64::from(span),
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
        };
        report.mel_db[64..].fill(0.0);
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(image.dimensions(), (512, 384));
        assert_eq!(image.get_pixel(0, 0), &crate::palette::heat(0.0));
        assert_eq!(image.get_pixel(511, 0), &crate::palette::heat(1.0));
        assert_eq!(image.get_pixel(130, 262).0, [255, 170, 80]);
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
        };
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let blue = [90, 185, 255];
        // 24 semitones over 256 rows: B4 owns the top rows, C3 the bottom ones.
        assert_eq!(image.get_pixel(100, 128).0, blue);
        assert_eq!(image.get_pixel(100, 137).0, blue);
        assert_eq!(image.get_pixel(100, 383).0, blue);
        assert_eq!(image.get_pixel(100, 250).0, [12, 16, 24]);
        assert!(presentation.text.contains("MIDI 71 at top, 48 at bottom"));
        let reversed = Inspection {
            low_pitch: 71,
            high_pitch: 48,
            ..report
        };
        assert!(present(&reversed).is_err());
    }
}
