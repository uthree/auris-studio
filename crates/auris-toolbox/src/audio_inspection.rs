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
            let value = (((level + 90.0) / 90.0).clamp(0.0, 1.0) * 255.0).round() as u8;
            pixels.put_pixel(x, y, Rgb([value, value, value]));
        }
    }
    for pitch in (0..128).step_by(12) {
        let y = 128 + (127 - pitch) * 2;
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
        let y = 128 + u32::from(127 - note.pitch) * 2;
        for x in first..last {
            for dy in 0..2 {
                pixels.put_pixel(x, y + dy, Rgb(colours[note.track as usize % colours.len()]));
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(pixels.as_raw(), 512, 384, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    let mut text = format!(
        "Rendered audio inspection, document revision {}.\n{}\nImage: 512x384. Shared x axis is elapsed seconds 0..{seconds:.3}. Top y=0..127: mel power, high frequencies at top, low at bottom; black=-90 dB, white=0 dB, fixed reference with no per-image normalization. Bottom y=128..383: authored piano roll, MIDI 127 at top, 0 at bottom; grey horizontal lines mark octave C notes. Track colour cycles by numeric ID modulo 6: blue, orange, green, purple, yellow, cyan.\nMel centre frequencies (low to high, Hz): {:?}.\nThis is measured audio plus a score image, not listening. Do not infer pleasing music, exact instruments, clipping, or masking from the picture alone. Use measurements for level claims; the score is before performance transforms. Harmony is stored project metadata, not detected from the audio. Bar-start harmony samples can omit mid-bar changes.",
        report.revision, report.measurements, report.frequencies
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
        };
        report.mel_db[64..].fill(0.0);
        let presentation = present(&report).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&presentation.png)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(image.dimensions(), (512, 384));
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(image.get_pixel(511, 0).0, [255, 255, 255]);
        assert_eq!(image.get_pixel(130, 262).0, [255, 170, 80]);
        assert!(presentation.text.contains("revision 7"));
        assert!(!presentation.text.contains(&presentation.png));
    }
}
