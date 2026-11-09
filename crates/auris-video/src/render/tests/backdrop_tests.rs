use super::*;
use std::hint::black_box;
use std::time::Instant;

fn serial_backdrop(style: &NoteRollStyle, background: &BackgroundImage) -> Pixmap {
    let style = style.sanitised();
    let (width, height) = (style.size.width, style.size.height);
    let mut pixmap = Pixmap::new(width, height).unwrap();
    let (br, bg, bb) = style.background.rgb();
    pixmap.fill(tiny_skia::Color::from_rgba8(br, bg, bb, 255));
    let picture = &background.0;
    let (iw, ih) = picture.dimensions();
    let scale = (f64::from(width) / f64::from(iw)).max(f64::from(height) / f64::from(ih));
    let sw = ((f64::from(iw) * scale).ceil() as u32).max(width);
    let sh = ((f64::from(ih) * scale).ceil() as u32).max(height);
    let scaled = image::imageops::resize(picture, sw, sh, image::imageops::FilterType::Triangle);
    let (ox, oy) = ((sw - width) / 2, (sh - height) / 2);
    let dim = style.background_dim;
    let back = [f32::from(br), f32::from(bg), f32::from(bb)];
    let data = pixmap.data_mut();
    for y in 0..height {
        for x in 0..width {
            let source = scaled.get_pixel(x + ox, y + oy).0;
            let alpha = f32::from(source[3]) / 255.0;
            let at = ((y * width + x) * 4) as usize;
            for channel in 0..3 {
                let over = f32::from(source[channel]) * alpha + back[channel] * (1.0 - alpha);
                let faded = over * (1.0 - dim) + back[channel] * dim;
                data[at + channel] = faded.round().clamp(0.0, 255.0) as u8;
            }
            data[at + 3] = 255;
        }
    }
    pixmap
}

fn pools() -> [rayon::ThreadPool; 3] {
    [1, 2, 4].map(|threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
    })
}

#[test]
fn large_background_compositing_matches_the_serial_reference() {
    let mut style = style();
    style.size = VideoSize::new(1024, 768);
    style.background_dim = 0.37;
    let picture = image::RgbaImage::from_fn(17, 11, |x, y| {
        image::Rgba([
            (x * 17 + y * 3) as u8,
            (y * 23 + x * 5) as u8,
            (x * 11 + y * 19) as u8,
            ((x * 13 + y * 29) % 256) as u8,
        ])
    });
    let background = BackgroundImage(picture);
    let actual = backdrop(&style, Some(&background));
    let expected = serial_backdrop(&style, &background);
    assert_eq!(actual.data().len(), expected.data().len());
    assert!(actual.data() == expected.data(), "RGBA output changed");
}

#[test]
fn backdrop_is_identical_across_pool_sizes_and_geometry_boundaries() {
    let pools = pools();
    for (width, height) in [(511, 511), (512, 512), (514, 512), (768, 1024), (1024, 768)] {
        for (source_width, source_height) in [(11, 17), (17, 11)] {
            for (dim, alpha) in [(0.0, 0), (0.37, 127), (1.0, 255)] {
                let mut style = style();
                style.size = VideoSize::new(width, height);
                style.background_dim = dim;
                let style = style.sanitised();
                let picture = image::RgbaImage::from_fn(source_width, source_height, |x, y| {
                    image::Rgba([((x * 17) % 256) as u8, ((y * 23) % 256) as u8, 91, alpha])
                });
                let background = BackgroundImage(picture);
                let expected = serial_backdrop(&style, &background);
                for (pool, threads) in pools.iter().zip([1, 2, 4]) {
                    let actual = pool.install(|| backdrop(&style, Some(&background)));
                    assert_eq!(actual.data().len(), expected.data().len());
                    assert!(
                        actual.data() == expected.data(),
                        "geometry {width}x{height}, source {source_width}x{source_height}, dim {dim}, alpha {alpha}, threads {threads}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "release-only performance check; run explicitly on a target machine"]
fn benchmark_backdrop_serial_and_parallel_medians() {
    fn median(values: &mut [u128]) -> u128 {
        values.sort_unstable();
        values[values.len() / 2]
    }

    for (width, height) in [(200, 120), (1920, 1080), (3840, 2160)] {
        let mut style = style();
        style.size = VideoSize::new(width, height);
        let picture = BackgroundImage(image::RgbaImage::from_fn(641, 359, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 239) as u8, 127, 255])
        }));
        black_box(backdrop(black_box(&style), Some(black_box(&picture))));
        let mut serial = Vec::with_capacity(9);
        let mut parallel = Vec::with_capacity(9);
        for trial in 0..9 {
            let serial_first = trial % 2 == 0;
            let run_serial = || {
                let started = Instant::now();
                black_box(serial_backdrop(black_box(&style), black_box(&picture)));
                started.elapsed().as_micros()
            };
            let run_parallel = || {
                let started = Instant::now();
                black_box(backdrop(black_box(&style), Some(black_box(&picture))));
                started.elapsed().as_micros()
            };
            if serial_first {
                serial.push(run_serial());
                parallel.push(run_parallel());
            } else {
                parallel.push(run_parallel());
                serial.push(run_serial());
            }
        }
        eprintln!(
            "backdrop {width}x{height} median: serial={}us parallel={}us",
            median(&mut serial),
            median(&mut parallel)
        );
    }
}
