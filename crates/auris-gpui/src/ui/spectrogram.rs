//! Audio-source spectrograms, analysed and coloured away from the UI and audio threads.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use auris_session::prelude::*;
use gpui::{Bounds, Context, Corners, Pixels, RenderImage, Window, point, px, size};
use image::{Frame, Rgba, RgbaImage};

use crate::app::AurisApp;
use crate::theme::Theme;
use crate::ui::paint;

/// Retain recently viewed sources, with room for every source currently visible.
const CACHE_LIMIT: usize = 32;
const DISPLAY_FLOOR_DB: f32 = -90.0;

/// One source texture and the coordinates needed to crop it to an audio clip.
pub(crate) struct SpectrogramImage {
    image: Arc<RenderImage>,
    frames: u64,
    low_hz: f64,
    high_hz: f64,
}

struct Entry {
    job: SpectrogramJob,
    image: Option<Arc<SpectrogramImage>>,
}

/// One worker at a time; source identity prevents an old result entering a new document.
#[derive(Default)]
pub(crate) struct SpectrogramCache {
    entries: VecDeque<Entry>,
    pending: Option<(SourceId, u64)>,
    generation: u64,
    retired: Vec<Arc<RenderImage>>,
}

impl SpectrogramCache {
    fn entry(&self, source: SourceId, session: &Session) -> Option<&Entry> {
        self.entries.iter().find(|entry| {
            entry.job.source() == source && session.spectrogram_job_is_current(&entry.job)
        })
    }

    pub(crate) fn get(&self, source: SourceId, session: &Session) -> Option<Arc<SpectrogramImage>> {
        self.entry(source, session)
            .and_then(|entry| entry.image.clone())
    }

    pub(crate) fn is_complete(&self, source: SourceId, session: &Session) -> bool {
        self.entry(source, session).is_some()
    }

    fn retire(&mut self, entry: Entry) {
        if let Some(image) = entry.image {
            self.retired.push(Arc::clone(&image.image));
        }
    }

    pub(crate) fn clear(&mut self) {
        while let Some(entry) = self.entries.pop_front() {
            self.retire(entry);
        }
        self.generation = self.generation.wrapping_add(1);
        // A running job keeps its slot until it finishes, even if the document changes.
    }

    fn prune(&mut self, session: &Session, visible: &HashSet<SourceId>) {
        let mut index = 0;
        while index < self.entries.len() {
            let entry = &self.entries[index];
            if !session.spectrogram_job_is_current(&entry.job)
                || (self.entries.len() > CACHE_LIMIT && !visible.contains(&entry.job.source()))
            {
                if let Some(entry) = self.entries.remove(index) {
                    self.retire(entry);
                }
            } else {
                index += 1;
            }
        }
    }

    /// GPUI's atlas owns uploaded textures independently of the cached Arcs.
    pub(crate) fn release_images(&mut self, window: &mut Window) {
        for image in self.retired.drain(..) {
            if let Err(error) = window.drop_image(image) {
                log::debug!("could not release spectrogram: {error}");
            }
        }
    }
}

impl AurisApp {
    /// Changes presentation only; editing, playback and project history are unaffected.
    pub(crate) fn set_track_spectrogram(
        &mut self,
        track: TrackId,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !self
            .project()
            .track(track)
            .is_some_and(|track| matches!(track.kind, TrackKind::Audio(_)))
        {
            return;
        }
        if enabled {
            self.spectrogram_tracks.insert(track);
        } else {
            self.spectrogram_tracks.remove(&track);
        }
        self.poll_spectrograms(cx);
        cx.notify();
    }

    /// Schedules visible sources, so zooming and scrolling do not run FFTs during paint.
    pub(crate) fn poll_spectrograms(&mut self, cx: &mut Context<Self>) {
        let tracks = &self.session.project().tracks;
        self.spectrogram_tracks.retain(|id| {
            tracks
                .iter()
                .any(|track| track.id == *id && matches!(track.kind, TrackKind::Audio(_)))
        });
        let mut visible = Vec::new();
        if let Some(bounds) = self.canvas.lanes.get() {
            let (start, end) = self.timeline.visible_range(bounds.size.width);
            for row in self.lane_rows() {
                if row.top + row.height < self.lane_scroll
                    || row.top > self.lane_scroll + bounds.size.height
                    || row.target().is_some()
                    || !self.spectrogram_tracks.contains(&row.track)
                {
                    continue;
                }
                let Some(track) = self.project().track(row.track) else {
                    continue;
                };
                if let TrackKind::Audio(audio) = &track.kind {
                    for clip in &audio.clips {
                        let length =
                            sounding_length(self.audio_clip_length_ticks(clip), clip.loop_end);
                        if clip.start <= end
                            && clip.start + length >= start
                            && !visible.contains(&clip.source)
                        {
                            visible.push(clip.source);
                        }
                    }
                }
            }
        }
        self.spectrograms
            .prune(&self.session, &visible.iter().copied().collect());
        if self.spectrograms.pending.is_some() {
            return;
        }
        let job = visible.into_iter().find_map(|source| {
            if self
                .spectrograms
                .entries
                .iter()
                .any(|entry| entry.job.source() == source)
            {
                None
            } else {
                self.session.spectrogram_job(source)
            }
        });
        let Some(job) = job else { return };
        let generation = self.spectrograms.generation;
        let source = job.source();
        self.spectrograms.pending = Some((source, generation));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let image = make_image(&job.run());
                    (job, image)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.spectrograms.pending != Some((source, generation)) {
                    return;
                }
                this.spectrograms.pending = None;
                let (job, image) = result;
                if this.spectrograms.generation == generation
                    && this.session.spectrogram_job_is_current(&job)
                {
                    this.spectrograms.entries.push_back(Entry { job, image });
                }
                this.poll_spectrograms(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// A fixed, ordered colour scale keeps equal source levels comparable across tracks.
fn colour(level: f32) -> [u8; 4] {
    const STOPS: [[f32; 3]; 5] = [
        [9.0, 8.0, 24.0],
        [61.0, 24.0, 110.0],
        [163.0, 42.0, 98.0],
        [239.0, 111.0, 36.0],
        [252.0, 245.0, 164.0],
    ];
    let normalized = if level.is_finite() {
        ((level - DISPLAY_FLOOR_DB) / -DISPLAY_FLOOR_DB).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let position = normalized * (STOPS.len() - 1) as f32;
    let lower = (position as usize).min(STOPS.len() - 2);
    let fraction = position - lower as f32;
    let channel = |index: usize| {
        (STOPS[lower][index] * (1.0 - fraction) + STOPS[lower + 1][index] * fraction).round() as u8
    };
    // RenderImage uses image::RgbaImage storage but GPUI's upload expects BGRA bytes.
    [channel(2), channel(1), channel(0), 255]
}

fn make_image(spectrum: &Spectrogram) -> Option<Arc<SpectrogramImage>> {
    if spectrum.columns() == 0 || spectrum.bands() == 0 {
        return None;
    }
    let mut pixels = RgbaImage::new(spectrum.columns() as u32, spectrum.bands() as u32);
    for column in 0..spectrum.columns() {
        for (band, level) in spectrum.column(column)?.iter().enumerate() {
            pixels.put_pixel(
                column as u32,
                (spectrum.bands() - 1 - band) as u32,
                Rgba(colour(*level)),
            );
        }
    }
    Some(Arc::new(SpectrogramImage {
        image: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        frames: spectrum.frame_count() as u64,
        low_hz: spectrum.low_hz(),
        high_hz: spectrum.high_hz(),
    }))
}

/// Place the whole source image so a clip-sized mask reveals precisely its source range.
fn source_bounds(
    bounds: Bounds<Pixels>,
    source_frames: u64,
    offset: u64,
    length: u64,
) -> Option<Bounds<Pixels>> {
    if source_frames == 0 || length == 0 || offset >= source_frames {
        return None;
    }
    let per_frame = f64::from(f32::from(bounds.size.width)) / length as f64;
    let left = f64::from(f32::from(bounds.origin.x)) - offset as f64 * per_frame;
    let width = source_frames as f64 * per_frame;
    (left.is_finite() && width.is_finite() && width > 0.0).then(|| Bounds {
        origin: point(px(left as f32), bounds.origin.y),
        size: size(px(width as f32), bounds.size.height),
    })
}

/// Paint source audio; clip gain, fades and effects keep their existing independent overlays.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_spectrogram(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    spectrum: &SpectrogramImage,
    offset: u64,
    length: u64,
    muted: bool,
    theme: &Theme,
) {
    let Some(image_bounds) = source_bounds(bounds, spectrum.frames, offset, length) else {
        return;
    };
    if let Err(error) = window.paint_image(
        image_bounds,
        Corners::default(),
        Arc::clone(&spectrum.image),
        0,
        muted,
    ) {
        log::debug!("could not draw spectrogram: {error}");
    }
    if bounds.size.height < px(48.0) || bounds.size.width < px(120.0) {
        return;
    }
    let visible = bounds.intersect(&window.content_mask().bounds);
    if visible.size.width < px(120.0) {
        return;
    }
    let label = |window: &mut Window, cx: &mut gpui::App, y: Pixels, text: String| {
        let at = point(visible.origin.x + px(3.0), y);
        let width = paint::measure_label(window, text.clone(), px(9.0));
        paint::rect(
            window,
            Bounds {
                origin: at,
                size: size(width + px(4.0), px(13.0)),
            },
            theme.surface_sunken,
        );
        paint::label(
            window,
            cx,
            point(at.x + px(2.0), at.y),
            text,
            px(9.0),
            theme.text,
        );
    };
    label(
        window,
        cx,
        bounds.origin.y + px(2.0),
        format!("{} Hz", spectrum.high_hz.round() as u32),
    );
    let height = f32::from(bounds.size.height);
    let mut last_y = bounds.origin.y + px(2.0);
    for hz in [10_000.0_f64, 1_000.0, 100.0] {
        if hz <= spectrum.low_hz || hz >= spectrum.high_hz {
            continue;
        }
        let fraction =
            ((hz / spectrum.low_hz).ln() / (spectrum.high_hz / spectrum.low_hz).ln()) as f32;
        let y = bounds.origin.y + px((1.0 - fraction) * height);
        if y - last_y < px(16.0) || y > bounds.origin.y + bounds.size.height - px(31.0) {
            continue;
        }
        paint::hline(window, visible, y, Theme::translucent(theme.text, 0.15));
        label(window, cx, y, format!("{} Hz", hz as u32));
        last_y = y;
    }
    label(
        window,
        cx,
        bounds.origin.y + bounds.size.height - px(15.0),
        format!("{} Hz · −90…0 dBFS", spectrum.low_hz.round() as u32),
    );
}

#[cfg(test)]
#[path = "spectrogram_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_crop_keeps_offsets_and_stretched_lengths_on_the_timeline() {
        let bounds = Bounds {
            origin: point(px(100.0), px(20.0)),
            size: size(px(200.0), px(80.0)),
        };
        let drawn = source_bounds(bounds, 48_000, 12_000, 24_000).unwrap();
        assert_eq!(drawn.origin.x, px(0.0));
        assert_eq!(drawn.size.width, px(400.0));
        // The quarter-source trim starts at the clip's left edge, including after stretching.
        assert_eq!(drawn.origin.x + drawn.size.width * 0.25, bounds.origin.x);
        let stretched = Bounds {
            size: size(px(400.0), bounds.size.height),
            ..bounds
        };
        let drawn = source_bounds(stretched, 48_000, 12_000, 24_000).unwrap();
        assert_eq!(drawn.origin.x + drawn.size.width * 0.25, stretched.origin.x);
        assert_eq!(drawn.size.width, px(800.0));
        assert!(source_bounds(bounds, 0, 0, 10).is_none());
        assert!(source_bounds(bounds, 10, 10, 10).is_none());
        assert!(source_bounds(bounds, 10, 0, 0).is_none());
    }

    #[test]
    fn colour_scale_is_finite_clamped_and_brightens_with_level() {
        assert_eq!(colour(f32::NAN), colour(DISPLAY_FLOOR_DB));
        assert_eq!(colour(-120.0), colour(DISPLAY_FLOOR_DB));
        assert_eq!(colour(12.0), colour(0.0));
        let brightness = |db| colour(db)[..3].iter().map(|v| u32::from(*v)).sum::<u32>();
        assert!(brightness(-90.0) < brightness(-60.0));
        assert!(brightness(-60.0) < brightness(-30.0));
        assert!(brightness(-30.0) < brightness(0.0));
    }
}
