//! Source, performed-track and project spectrograms prepared away from the UI and audio threads.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{
    Bounds, Context, Corners, IntoElement, Pixels, RenderImage, Window, canvas, div, point,
    prelude::*, px, size,
};
use image::{Frame, Rgba, RgbaImage};

use crate::app::AurisApp;
use crate::theme::Theme;
use crate::ui::paint;

/// Retain recently viewed sources, with room for every source currently visible.
const CACHE_LIMIT: usize = 32;
const DISPLAY_FLOOR_DB: f32 = -90.0;
pub(crate) const PROJECT_SPECTROGRAM_HEIGHT: Pixels = px(132.0);

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

/// One worker at a time; source identity and revision checks reject obsolete results.
#[derive(Default)]
pub(crate) struct SpectrogramCache {
    entries: VecDeque<Entry>,
    pending: Option<(SourceId, u64)>,
    generation: u64,
    retired: Vec<Arc<RenderImage>>,
    pub(crate) project_enabled: bool,
    rendered: Vec<RenderedEntry>,
    rendering: Option<(Option<TrackId>, u64, Arc<AtomicBool>)>,
}

struct RenderedEntry {
    track: Option<TrackId>,
    revision: u64,
    image: Option<Arc<SpectrogramImage>>,
    failed: bool,
}

/// A rendered spectrum and its mapping from elapsed samples to musical time.
pub(crate) struct RenderedSpectrumPaint {
    image: Option<Arc<SpectrogramImage>>,
    message: String,
    tempo: TempoMap,
    rate: f64,
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
        self.project_enabled = false;
        if let Some((_, _, cancel)) = &self.rendering {
            cancel.store(true, Ordering::Relaxed);
        }
        for entry in self.rendered.drain(..) {
            if let Some(image) = entry.image {
                self.retired.push(Arc::clone(&image.image));
            }
        }
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
        if self.project().track(track).is_none() {
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
        self.spectrogram_tracks
            .retain(|id| tracks.iter().any(|track| track.id == *id));
        self.poll_rendered_spectrograms(cx);
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
        if self.spectrograms.pending.is_some() || self.spectrograms.rendering.is_some() {
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

    pub(crate) fn set_project_spectrogram(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.spectrograms.project_enabled = enabled;
        self.poll_spectrograms(cx);
        cx.notify();
    }

    fn poll_rendered_spectrograms(&mut self, cx: &mut Context<Self>) {
        let revision = self.session.revision();
        let mut wanted = Vec::new();
        if self.spectrograms.project_enabled {
            wanted.push(None);
        }
        if let Some(bounds) = self.canvas.lanes.get() {
            for row in self.lane_rows() {
                if row.target().is_none()
                    && row.top + row.height >= self.lane_scroll
                    && row.top <= self.lane_scroll + bounds.size.height
                    && self.spectrogram_tracks.contains(&row.track)
                    && self
                        .project()
                        .track(row.track)
                        .is_some_and(|track| track.kind.as_audio().is_none())
                {
                    wanted.push(Some(row.track));
                }
            }
        }
        let mut index = 0;
        while index < self.spectrograms.rendered.len() {
            let entry = &self.spectrograms.rendered[index];
            if entry.revision != revision || !wanted.contains(&entry.track) {
                let entry = self.spectrograms.rendered.remove(index);
                if let Some(image) = entry.image {
                    self.spectrograms.retired.push(Arc::clone(&image.image));
                }
            } else {
                index += 1;
            }
        }
        if let Some((track, captured, cancel)) = &self.spectrograms.rendering {
            if *captured != revision || !wanted.contains(track) {
                cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        if self.spectrograms.pending.is_some() {
            return;
        }
        let Some(track) = wanted.into_iter().find(|track| {
            !self
                .spectrograms
                .rendered
                .iter()
                .any(|entry| entry.track == *track)
        }) else {
            return;
        };
        let Some(job) = self.session.rendered_spectrogram_job(track) else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.spectrograms.rendering = Some((track, revision, Arc::clone(&cancel)));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let cancel = Arc::clone(&cancel);
                    async move { job.run(&cancel).map(|spectrum| make_image(&spectrum)) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.spectrograms.rendering = None;
                if !cancel.load(Ordering::Relaxed) && this.session.revision() == revision {
                    let failed = result.is_err();
                    if let Err(error) = &result {
                        log::warn!("could not render spectrogram: {error}");
                    }
                    this.spectrograms.rendered.push(RenderedEntry {
                        track,
                        revision,
                        image: result.ok().flatten(),
                        failed,
                    });
                }
                this.poll_spectrograms(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn rendered_spectrum_paint(&self, track: Option<TrackId>) -> RenderedSpectrumPaint {
        let entry = self
            .spectrograms
            .rendered
            .iter()
            .find(|entry| entry.track == track && entry.revision == self.session.revision());
        let message = match entry {
            None => Key::SpectrogramLoading,
            Some(entry) if entry.failed => Key::SpectrogramFailed,
            Some(_) => Key::SpectrogramEmpty,
        };
        RenderedSpectrumPaint {
            image: entry.and_then(|entry| entry.image.clone()),
            message: self.t(message).to_string(),
            tempo: self.project().tempo_map.clone(),
            rate: self.project().sample_rate,
        }
    }

    pub(crate) fn render_project_spectrogram(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let spectrum = self.rendered_spectrum_paint(None);
        let view = self.timeline.clone();
        let theme = self.theme.clone();
        div()
            .id("project-spectrogram")
            .flex()
            .flex_col()
            .h(PROJECT_SPECTROGRAM_HEIGHT)
            .flex_shrink_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                this.scroll_timeline(event, cx);
            }))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px(px(6.0))
                    .text_size(px(11.0))
                    .child(self.t(Key::MenuProjectSpectrogram))
                    .child(
                        div()
                            .id("close-project-spectrogram")
                            .debug_selector(|| "close-project-spectrogram".to_string())
                            .cursor_pointer()
                            .child("×")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_project_spectrogram(false, cx)
                            })),
                    ),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        paint::clipped(window, bounds, |window| {
                            paint_rendered_spectrum(window, cx, bounds, &spectrum, &view, &theme);
                        });
                    },
                )
                .w_full()
                .flex_1(),
            )
    }
}

/// Split at tempo changes so an elapsed-time texture follows the musical ruler.
pub(crate) fn paint_rendered_spectrum(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    spectrum: &RenderedSpectrumPaint,
    view: &crate::ui::timeline::TimelineView,
    theme: &Theme,
) {
    paint::rect(window, bounds, theme.surface_sunken);
    let Some(image) = &spectrum.image else {
        paint::label(
            window,
            cx,
            bounds.origin + point(px(4.0), px(22.0)),
            spectrum.message.clone(),
            px(11.0),
            theme.text_muted,
        );
        return;
    };
    for (start, end, offset, length) in
        rendered_segments(&spectrum.tempo, spectrum.rate, image.frames)
    {
        let segment = Bounds {
            origin: point(bounds.origin.x + view.tick_to_x(start), bounds.origin.y),
            size: size(view.duration_to_width(end - start), bounds.size.height),
        };
        let visible = segment.intersect(&bounds);
        if visible.size.width > px(0.0) {
            paint::clipped(window, visible, |window| {
                paint_spectrogram(window, cx, segment, image, offset, length, false, theme);
            });
        }
    }
}

fn rendered_segments(tempo: &TempoMap, rate: f64, frames: u64) -> Vec<(Ticks, Ticks, u64, u64)> {
    let end = tempo.seconds_to_ticks(Seconds(frames as f64 / rate));
    let mut ticks = vec![Ticks::ZERO];
    ticks.extend(
        tempo
            .points()
            .iter()
            .map(|point| point.tick)
            .filter(|tick| *tick > Ticks::ZERO && *tick < end),
    );
    ticks.push(end);
    ticks
        .windows(2)
        .filter_map(|pair| {
            let offset = tempo.ticks_to_samples(pair[0], rate).raw().min(frames);
            let limit = if pair[1] == end {
                frames
            } else {
                tempo.ticks_to_samples(pair[1], rate).raw().min(frames)
            };
            (limit > offset && pair[1] > pair[0]).then_some((
                pair[0],
                pair[1],
                offset,
                limit - offset,
            ))
        })
        .collect()
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
    fn rendered_time_mapping_splits_at_tempo_changes_and_keeps_the_tail() {
        let mut tempo = TempoMap::default();
        tempo.set_initial_bpm(120.0);
        tempo.set_point(Ticks::QUARTER, 60.0);
        let segments = rendered_segments(&tempo, 48_000.0, 96_000);
        assert_eq!(
            segments,
            vec![
                (Ticks::ZERO, Ticks::QUARTER, 0, 24_000),
                (Ticks::QUARTER, Ticks::from_beats(2.5), 24_000, 72_000),
            ]
        );
        assert!(rendered_segments(&tempo, 48_000.0, 0).is_empty());
    }

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
