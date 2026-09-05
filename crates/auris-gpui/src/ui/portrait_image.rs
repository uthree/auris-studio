//! Bounded, off-thread decoding for the singer inspector's artwork.

use gpui::RenderImage;
use image::{DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader, Limits, imageops};
use std::{io::Cursor, sync::Arc};

const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
const DISPLAY_DIMENSION: u32 = 512;

/// Decode a single still portrait for GPUI; call this only from a background worker.
///
/// The encoded size, dimensions and pixel count are checked before decoding pixel data. The
/// decoder also gets an allocation budget, although its memory limit is best effort. Large
/// portraits are reduced without cropping or enlarging, leaving at most 1 MiB of cached pixels.
pub(super) fn decode_portrait(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err("Portrait exceeds the 8 MiB image limit".into());
    }
    let format = image::guess_format(bytes).map_err(|error| error.to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err("Portrait must be a PNG, JPEG or WebP image".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("Portrait exceeds the 16 megapixel image limit".into());
    }
    if decoder.total_bytes() > MAX_DECODE_BYTES {
        return Err("Portrait exceeds the decoded image limit".into());
    }
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    decoded.apply_orientation(orientation);
    let mut rgba = decoded.into_rgba8();
    let (width, height) = rgba.dimensions();
    let longest = width.max(height);
    if longest > DISPLAY_DIMENSION {
        // Resize premultiplied colors so transparent pixels cannot add a colored fringe. GPUI
        // expects straight alpha, so undo the multiplication on the much smaller result.
        for pixel in rgba.pixels_mut() {
            let alpha = u16::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
        let scaled = |dimension: u32| {
            ((u64::from(dimension) * u64::from(DISPLAY_DIMENSION) + u64::from(longest / 2))
                / u64::from(longest))
            .max(1) as u32
        };
        rgba = imageops::resize(
            &rgba,
            scaled(width),
            scaled(height),
            imageops::FilterType::Triangle,
        );
        for pixel in rgba.pixels_mut() {
            let alpha = u16::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = (u16::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
    }
    // RenderImage's Frame storage uses image::RgbaImage, but the GPU upload expects BGRA.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![Frame::new(rgba)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn encode(image: RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        let image = DynamicImage::ImageRgba8(image);
        if format == ImageFormat::Jpeg {
            image.to_rgb8().write_to(&mut bytes, format).unwrap();
        } else {
            image.write_to(&mut bytes, format).unwrap();
        }
        bytes.into_inner()
    }

    #[test]
    fn png_and_webp_keep_straight_alpha_and_use_bgra_channels() {
        let rgba = RgbaImage::from_raw(2, 1, vec![241, 37, 19, 128, 3, 7, 11, 255]).unwrap();
        for format in [ImageFormat::Png, ImageFormat::WebP] {
            let image = decode_portrait(&encode(rgba.clone(), format)).unwrap();
            assert_eq!(image.size(0).width.0, 2);
            assert_eq!(image.size(0).height.0, 1);
            assert_eq!(image.frame_count(), 1);
            assert_eq!(
                image.as_bytes(0).unwrap(),
                [19, 37, 241, 128, 11, 7, 3, 255]
            );
        }
    }

    #[test]
    fn jpeg_is_opaque_and_keeps_channel_order() {
        let rgba = RgbaImage::from_pixel(8, 8, Rgba([230, 80, 30, 255]));
        let image = decode_portrait(&encode(rgba, ImageFormat::Jpeg)).unwrap();
        for pixel in image.as_bytes(0).unwrap().as_chunks::<4>().0 {
            assert!(pixel[2].abs_diff(230) <= 3, "red: {pixel:?}");
            assert!(pixel[1].abs_diff(80) <= 3, "green: {pixel:?}");
            assert!(pixel[0].abs_diff(30) <= 3, "blue: {pixel:?}");
            assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn large_artwork_fits_both_orientations_without_cropping_or_enlarging() {
        for (width, height, expected) in [
            (600, 1200, (256, 512)),
            (1200, 600, (512, 256)),
            (35, 71, (35, 71)),
            (1, 600, (1, 512)),
        ] {
            let rgba = RgbaImage::from_pixel(width, height, Rgba([40, 90, 170, 255]));
            let image = decode_portrait(&encode(rgba, ImageFormat::Png)).unwrap();
            assert_eq!((image.size(0).width.0, image.size(0).height.0), expected);
            assert!(image.as_bytes(0).unwrap().len() <= 512 * 512 * 4);
            assert_eq!(&image.as_bytes(0).unwrap()[..4], &[170, 90, 40, 255]);
        }
    }

    #[test]
    fn downsampling_does_not_bleed_hidden_colors_into_transparent_edges() {
        let rgba = RgbaImage::from_fn(1024, 2, |x, _| {
            if x % 2 == 0 {
                Rgba([255, 0, 0, 0])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let image = decode_portrait(&encode(rgba, ImageFormat::Png)).unwrap();
        assert_eq!(image.size(0).width.0, 512);
        for pixel in image.as_bytes(0).unwrap().as_chunks::<4>().0 {
            assert_eq!(pixel[0], 255, "visible blue stays blue: {pixel:?}");
            assert_eq!(pixel[1], 0);
            assert_eq!(pixel[2], 0, "hidden red must not bleed: {pixel:?}");
            assert!((1..255).contains(&pixel[3]));
        }
    }

    #[test]
    fn oversized_headers_are_rejected_before_any_pixels_are_decoded() {
        // Keep a 1x1 IDAT while changing only the IHDR dimensions and its checksum. If the
        // guard fails, decoding would report a truncated pixel stream instead of our limit.
        let original = encode(RgbaImage::new(1, 1), ImageFormat::Png);
        for (width, height) in [(8193_u32, 1_u32), (4097, 4097)] {
            let mut bytes = original.clone();
            bytes[16..20].copy_from_slice(&width.to_be_bytes());
            bytes[20..24].copy_from_slice(&height.to_be_bytes());
            let mut crc = !0_u32;
            for byte in &bytes[12..29] {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
                }
            }
            bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
            let error = decode_portrait(&bytes).unwrap_err();
            assert!(
                error.contains("limit") || error.contains("Limits"),
                "{error}"
            );
        }
    }

    #[test]
    fn invalid_unsupported_and_oversized_encoded_images_are_rejected() {
        assert!(decode_portrait(b"corrupt image").is_err());
        let png = encode(RgbaImage::new(2, 2), ImageFormat::Png);
        assert!(decode_portrait(&png[..png.len() / 2]).is_err());
        // A recognizable GIF header must be rejected even when GPUI enables that decoder.
        let error = decode_portrait(b"GIF89a\x01\0\x01\0\0\0\0").unwrap_err();
        assert!(error.contains("PNG, JPEG or WebP"));
        let error = decode_portrait(&vec![0; MAX_ENCODED_BYTES + 1]).unwrap_err();
        assert!(error.contains("8 MiB"));
    }
}
