//! The piano roll: note editing for the selected MIDI clip.

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;

use gpui::{
    App, Bounds, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, Window, canvas, div,
    point, prelude::*, px, size,
};

use crate::app::{AurisApp, Drag, OrnamentHandle, PhonemeSpan, PitchContour, SungGeometry};
use crate::dock::Dock;
use crate::gestures::{EmptyPress, empty_press};
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::paint;
use crate::ui::timeline::{PitchView, TimelineView};
use crate::ui::widgets::{ButtonStyle, button};

/// What the pointer does in the note grid.
///
/// Logic Pro's tool menu, reduced to the two tools this editor has. A tool rather than a modifier
/// because there is no modifier left to give it: ⌘ creates a note and suspends the grid, ⌥
/// deletes, ⇧ extends the selection, and Logic's own ⌃⌥-drag cannot arrive at all — gpui rewrites
/// a ⌃-left-click into a right-click on macOS, and strips the ⌃ off it on the way, so the gesture
/// reaches the window as a request for the context menu.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RollTool {
    /// Select, move, resize and create — everything the roll did before there were tools.
    #[default]
    Pointer,
    /// Drag a note up or down to say how hard it is struck.
    Velocity,
}

impl RollTool {
    /// Every tool, in the order the strip shows them.
    pub const ALL: [RollTool; 2] = [RollTool::Pointer, RollTool::Velocity];

    /// What the tool is called.
    pub fn label(self) -> Key {
        match self {
            RollTool::Pointer => Key::ToolPointer,
            RollTool::Velocity => Key::ToolVelocity,
        }
    }

    /// The next tool along, wrapping round at the end.
    ///
    /// One bindable command rather than one per tool. Logic's tool key swaps back to the tool
    /// before it when pressed twice, which with two tools is the same gesture as cycling — and
    /// this way the keymap grows by one entry rather than by one for every tool there will ever
    /// be.
    pub fn next(self) -> Self {
        let at = RollTool::ALL
            .iter()
            .position(|tool| *tool == self)
            .unwrap_or(0);
        RollTool::ALL[(at + 1) % RollTool::ALL.len()]
    }
}

/// Width of the grab zone on a note's right edge, in pixels.
const RESIZE_HANDLE: f32 = 5.0;

/// How far the pointer travels for one step of MIDI velocity.
///
/// The whole range is then about 190 pixels — a comfortable drag, short enough to reach either
/// end without letting go, and long enough that a single step is still deliberate.
const PIXELS_PER_VELOCITY_STEP: f32 = 1.5;

/// The softest a note may be struck.
///
/// MIDI spends velocity 0 on "this note has stopped", so nothing is written at it. A note dragged
/// down to nothing would otherwise still be drawn, still be selected and still be movable, and
/// never once be heard.
const MIN_VELOCITY: u8 = 1;

/// A stored velocity as the number a musician and the MIDI cable both use.
fn midi_velocity(velocity: f32) -> u8 {
    (velocity.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// Where a note struck at `origin` ends up after the pointer has travelled `dy` from where the
/// drag began.
///
/// Up is louder. Screen y grows downward, which is the negation — but every fader in the
/// application, every fader in every mixing desk, and Logic's own velocity tool all move that
/// way, and a velocity drag that went the other way would be wrong however consistent the
/// arithmetic was.
///
/// Measured from `origin` rather than from wherever the note is now, so the whole gesture is one
/// idempotent rewrite: a drag past either end and back leaves the selection exactly as it was
/// rather than collapsed against the limit it was pushed into.
fn dragged_velocity(origin: u8, dy: Pixels) -> u8 {
    let steps = -f32::from(dy) / PIXELS_PER_VELOCITY_STEP;
    (i32::from(origin) + steps.round() as i32).clamp(i32::from(MIN_VELOCITY), 127) as u8
}

/// How wide the grab zone on a note's right edge is, for a note drawn `width` across.
///
/// Never more than a third of the note. At a 1/32 grid a note is about eight pixels wide, and a
/// fixed five-pixel handle either side of its end swallowed the whole thing: the note could be
/// stretched and never moved, which reads as the roll refusing to let go of it.
fn resize_grab(width: Pixels) -> f32 {
    RESIZE_HANDLE.min(f32::from(width) / 3.0)
}

/// The horizontal span of a note's resize grab, as an origin and a width.
///
/// The inner half of the zone the press measures, which is the half a press can reach: the
/// resize check is only arrived at once [`AurisApp::note_at`] has found a note under the
/// pointer's tick, and the outer half is past the note's end. `None` only when the note has zero
/// or negative width; a positive sub-pixel note keeps a proportionally smaller grab zone.
fn note_end_span(start_x: Pixels, end_x: Pixels) -> Option<(Pixels, Pixels)> {
    let grab = px(resize_grab(end_x - start_x));
    (grab > px(0.0)).then_some((end_x - grab, grab))
}

/// Length given to a note drawn with a single click.
fn default_note_length(grid: Ticks) -> Ticks {
    Ticks(grid.raw().max(1))
}

/// The grabbed note's written values, captured before the canvas borrows the window.
#[derive(Debug)]
struct NoteEditTag {
    start: Ticks,
    length: Ticks,
    pitch: u8,
    lines: [String; 3],
}

/// Clip-relative note start snapped to the song grid drawn behind it.
fn snapped_note_start(tick: Ticks, clip_start: Ticks, grid: Ticks) -> Ticks {
    (tick.snap_nearest(grid) - clip_start).max_zero()
}

/// Which of a track's clips are drawn faintly behind the one being edited.
///
/// Every *other* clip on the track with any part of itself on screen. Not only the two touching
/// it: the roll scrolls and zooms anywhere, so "the next clip along" stops being the next one the
/// moment the view moves, and a roll showing an empty stretch where a clip plainly is would be
/// lying about the track.
///
/// A clip is taken as half-open, the way a clip's end is the tick the next one may start on, so
/// one ending exactly where the view begins has nothing inside it to draw and is left out.
fn ghosted(clips: &[(ClipId, Ticks, Ticks)], editing: ClipId, view: (Ticks, Ticks)) -> Vec<ClipId> {
    let (from, to) = view;
    clips
        .iter()
        .filter(|(id, start, end)| *id != editing && *end > from && *start < to)
        .map(|(id, _, _)| *id)
        .collect()
}

/// How near a press must land to a phoneme boundary to take hold of it, in pixels.
const PHONEME_GRAB: f32 = 5.0;
const PHONEME_GRAB_HALF: f32 = PHONEME_GRAB / 2.0;

/// Which phoneme boundary of `note` a press `at` seconds along the timeline takes hold of.
///
/// The answer is the phoneme whose *end* sits within `slack` — the one whose width the drag
/// will pin — and where that phoneme begins, in timeline seconds. Only boundaries strictly
/// inside the note answer: its edges belong to the resize and move gestures, and a note
/// singing a single phoneme has no cut to move.
fn grabbed_phoneme_boundary(
    note: &Note,
    start_seconds: f64,
    end_seconds: f64,
    at_seconds: f64,
    slack_seconds: f64,
    widths: Option<&ConsonantWidths>,
) -> Option<(usize, f64)> {
    if note.phonemes.len() < 2 {
        return None;
    }
    let length = (end_seconds - start_seconds).max(0.0);
    let layout = phoneme_layout(&note.phonemes, &note.phoneme_seconds, length, widths);
    layout
        .iter()
        .take(layout.len().saturating_sub(1))
        .enumerate()
        .filter(|(_, (_, to))| *to > 0.0 && *to < length)
        .filter(|(_, (_, to))| (start_seconds + to - at_seconds).abs() <= slack_seconds)
        .min_by(|(_, (_, a)), (_, (_, b))| {
            (start_seconds + a - at_seconds)
                .abs()
                .total_cmp(&(start_seconds + b - at_seconds).abs())
        })
        .map(|(index, (from, _))| (index, start_seconds + from))
}

/// The pitch contour a singer track's frames describe, as drawable runs.
///
/// One run per voiced span — silence is a gap in the line, not a dive to nowhere — each
/// point a timeline tick and a *fractional* MIDI pitch, so a drawn bend reads as the slide
/// it is and a consonant rides its vowel's pitch the way the model will sing it. Pure
/// arithmetic on the frames, which is what lets a test hear it without a window.
fn f0_contour(frames: &SingerFrames, tempo: &TempoMap) -> PitchContour {
    pitch_contour(
        frames.f0_hz.iter().map(|hz| f64::from(*hz)),
        frames.hop_seconds,
        tempo,
    )
}

fn pitch_contour(
    hz: impl Iterator<Item = f64>,
    hop_seconds: f64,
    tempo: &TempoMap,
) -> PitchContour {
    let mut runs: PitchContour = Vec::new();
    let mut run: Vec<(Ticks, f32)> = Vec::new();
    for (index, hz) in hz.enumerate() {
        if hz.is_finite() && hz > 0.0 {
            let seconds = index as f64 * hop_seconds;
            let tick = tempo.seconds_to_ticks(Seconds(seconds));
            let pitch = 69.0 + 12.0 * (hz / 440.0).log2();
            run.push((tick, pitch as f32));
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

/// The phoneme segmentation a singer track's frames describe: each sung span's half-open
/// tick range and the symbol the model is given for it.
///
/// Run-length over the frames, silence dropped — a rest is the absence of a phoneme, not a
/// segment called silence. Two notes singing the same vowel back to back come out as one
/// span, and the painter clips against the notes to give each its own symbol.
fn phoneme_spans(frames: &SingerFrames, tempo: &TempoMap) -> Vec<PhonemeSpan> {
    let tick = |frame: usize| tempo.seconds_to_ticks(Seconds(frame as f64 * frames.hop_seconds));
    let mut spans = Vec::new();
    let mut start = 0usize;
    for index in 1..=frames.phonemes.len() {
        if index < frames.phonemes.len() && frames.phonemes[index] == frames.phonemes[start] {
            continue;
        }
        let symbol = frames
            .inventory
            .get(frames.phonemes[start] as usize)
            .cloned()
            .unwrap_or_default();
        if symbol != SILENCE {
            spans.push(PhonemeSpan {
                from: tick(start),
                to: tick(index),
                symbol,
            });
        }
        start = index;
    }
    spans
}

/// Shortest a phoneme segment may be drawn before its symbol is left off, in pixels.
///
/// The divider still marks the cut: a sixty-millisecond consonant far zoomed out is a
/// boundary worth seeing even where its symbol would smear into its neighbour's.
const PHONEME_LABEL_MIN: f32 = 12.0;

/// Draws the model's phoneme segmentation on the notes: a divider inside the note at each
/// cut, the symbol above it where there is room.
///
/// This is the *timed* truth — the same frames the model is fed, so a sixty-millisecond
/// consonant is sixty milliseconds wide however long its note holds — where the static list
/// [`paint_lyric`] used to write only said what the phonemes were. Spans are clipped to
/// each note, so two same-vowel notes in a row each carry their own symbol even though the
/// frames run them together.
#[allow(clippy::too_many_arguments)]
fn paint_phoneme_spans(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    spans: &[PhonemeSpan],
    notes: &[Note],
    clip_start: Ticks,
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    // The symbols borrow the row above the note, the same bargain as the untimed list they
    // replace: below this zoom they would read as notes, so only the dividers stay.
    let labelled = pitch_view.row_height + 2.0 >= LYRIC_PHONEME_MIN_ROW;
    for note in notes {
        let from = clip_start + note.start;
        let to = clip_start + note.end();
        let y = bounds.origin.y + pitch_view.pitch_to_y(note.pitch);
        if y + px(pitch_view.row_height) < bounds.origin.y
            || y > bounds.origin.y + bounds.size.height
        {
            continue;
        }
        // The dividers stop short of the note's edges, so a cut reads as a mark on the note
        // rather than a note ending there.
        let row = Bounds {
            origin: point(bounds.origin.x, y + px(3.0)),
            size: size(
                bounds.size.width,
                px((pitch_view.row_height - 6.0).max(1.0)),
            ),
        };
        let ink = theme.text_on(theme.velocity_color(note.velocity));
        let faint = gpui::Hsla {
            a: ink.a * 0.5,
            ..ink
        };
        for span in spans {
            if span.to <= from || span.from >= to {
                continue;
            }
            let begin = span.from.max(from);
            let end = span.to.min(to);
            let x = bounds.origin.x + view.tick_to_x(begin);
            if x < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
                continue;
            }
            if begin > from {
                paint::vline(window, row, x, px(1.0), faint);
            }
            if labelled && f32::from(view.duration_to_width(end - begin)) >= PHONEME_LABEL_MIN {
                paint::label(
                    window,
                    cx,
                    point(x + px(2.0), y - px(11.0)),
                    span.symbol.clone(),
                    px(8.5),
                    theme.text_muted,
                );
            }
        }
    }
}

/// How near a press must land to an ornament handle to take hold of it, in pixels.
const ORNAMENT_GRAB: f32 = 6.0;

fn within_ornament_handle(dx: f32, dy: f32) -> bool {
    dx.abs() <= ORNAMENT_GRAB / 2.0 && dy.abs() <= ORNAMENT_GRAB / 2.0
}

/// Where a note's ornament handles sit: `(handle, seconds into the note, semitones off it)`.
///
/// The scoop's and the fall's handles sit at the *corner* of each gesture — as far into the
/// note as it reaches, the full depth under it — because the corner is the one point whose x
/// is the span and whose y is the depth, so a single drag shapes both. The vibrato's sits at
/// the crest of its first sway: where the sway begins, the depth above. Spans wear the same
/// half-note cap the sung shape wears, so a handle always sits on the audible gesture.
fn ornament_handles(note: &Note, length: f64) -> Vec<(OrnamentHandle, f64, f32)> {
    let mut handles = Vec::new();
    if let Some(scoop) = &note.scoop {
        handles.push((
            OrnamentHandle::Scoop,
            ornament_reach(scoop.seconds, length),
            -scoop.depth,
        ));
    }
    if let Some(fall) = &note.fall {
        handles.push((
            OrnamentHandle::Fall,
            length - ornament_reach(fall.seconds, length),
            -fall.depth,
        ));
    }
    if let Some(vibrato) = &note.vibrato {
        handles.push((
            OrnamentHandle::Vibrato,
            vibrato.delay.clamp(0.0, length),
            vibrato.depth,
        ));
    }
    handles
}

/// Draws the grab handles of every ornamented note: a small square at each shaping point.
///
/// Painted with the contour they shape and in its accent, and only while the sung geometry
/// is on screen — a handle on a track that cannot sing would offer a gesture nothing hears.
#[allow(clippy::too_many_arguments)]
fn paint_ornament_handles(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    notes: &[Note],
    clip_start: Ticks,
    tempo: &TempoMap,
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    let half = px(ORNAMENT_GRAB / 2.0);
    for note in notes {
        if note.scoop.is_none() && note.fall.is_none() && note.vibrato.is_none() {
            continue;
        }
        let start = tempo.ticks_to_seconds(clip_start + note.start).0;
        let end = tempo.ticks_to_seconds(clip_start + note.end()).0;
        let centre = (pitch_view.top_pitch as f32 - f32::from(note.pitch)) * pitch_view.row_height
            + pitch_view.row_height / 2.0;
        for (_, t, semis) in ornament_handles(note, end - start) {
            let x = bounds.origin.x + view.tick_to_x(tempo.seconds_to_ticks(Seconds(start + t)));
            let y = bounds.origin.y + px(centre - semis * pitch_view.row_height);
            paint::rect(
                window,
                Bounds {
                    origin: point(x - half, y - half),
                    size: size(px(ORNAMENT_GRAB), px(ORNAMENT_GRAB)),
                },
                theme.accent,
            );
        }
    }
}

/// The selected clip's occupied time ranges, merging overlapping and adjacent notes.
fn note_pitch_spans(notes: &[Note], clip_start: Ticks, clip_end: Ticks) -> Vec<(Ticks, Ticks)> {
    let mut spans: Vec<_> = notes
        .iter()
        .filter_map(|note| {
            let start = (clip_start + note.start).max(clip_start);
            let end = (clip_start + note.end()).min(clip_end);
            (start < end).then_some((start, end))
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(Ticks, Ticks)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

/// Clips track-wide pitch runs to the edited clip's occupied note spans.
///
/// Interpolating at note boundaries keeps short notes visible and breaks even a rest that falls
/// between two sampled points. The same clipping applies to host and backend pitch.
fn note_pitch_contour(contour: &PitchContour, spans: &[(Ticks, Ticks)]) -> PitchContour {
    let mut clipped = Vec::new();
    for run in contour.iter().filter(|run| run.len() > 1) {
        for &(from, to) in spans {
            let from = from.max(run[0].0);
            let to = to.min(run[run.len() - 1].0);
            if from >= to {
                continue;
            }
            let boundary = |tick: Ticks| {
                let next = run.partition_point(|(at, _)| *at <= tick);
                let (before, pitch) = run[next - 1];
                if before == tick {
                    return (tick, pitch);
                }
                let (after, next_pitch) = run[next];
                let fraction = (tick - before).raw() as f32 / (after - before).raw() as f32;
                (tick, pitch + (next_pitch - pitch) * fraction)
            };
            let first = run.partition_point(|(tick, _)| *tick <= from);
            let end = run.partition_point(|(tick, _)| *tick < to);
            let mut segment = Vec::with_capacity(end - first + 2);
            segment.push(boundary(from));
            segment.extend_from_slice(&run[first..end]);
            segment.push(boundary(to));
            clipped.push(segment);
        }
    }
    clipped
}

/// Draws the sung pitch contour over the notes.
///
/// Trimmed to the edited clip's occupied note spans — the frames cover the whole track, and the
/// neighbouring clips are already ghosts — with y at the centre of the row a note at that
/// pitch would occupy, which is where the eye lines a curve up against a note.
fn paint_f0_curve(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    contour: &PitchContour,
    spans: &[(Ticks, Ticks)],
    view: &TimelineView,
    pitch_view: &PitchView,
    color: gpui::Hsla,
) {
    let centre = |pitch: f32| {
        (pitch_view.top_pitch as f32 - pitch) * pitch_view.row_height + pitch_view.row_height / 2.0
    };
    for run in note_pitch_contour(contour, spans) {
        let drawn: Vec<gpui::Point<Pixels>> = run
            .iter()
            .map(|(tick, pitch)| {
                point(
                    bounds.origin.x + view.tick_to_x(*tick),
                    bounds.origin.y + px(centre(*pitch)),
                )
            })
            .collect();
        if drawn.len() > 1 {
            paint::polyline(window, &drawn, px(1.5), color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_performed_pitch(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    notes: &[Note],
    bend: &[CurvePoint],
    clip_start: Ticks,
    view: &TimelineView,
    pitch_view: &PitchView,
    color: gpui::Hsla,
) {
    if bend.is_empty() {
        return;
    }
    for note in notes {
        let start = note.start;
        let end = note.end() - Ticks(1);
        if clip_start + end < view.scroll_ticks
            || clip_start + start > view.x_to_tick(bounds.size.width)
        {
            continue;
        }
        let mut points = vec![(start, auris_session::prelude::curve_at(bend, start))];
        points.extend(
            bend.iter()
                .filter(|p| p.at > start && p.at < end)
                .map(|p| (p.at, p.value)),
        );
        points.push((end, auris_session::prelude::curve_at(bend, end)));
        let drawn: Vec<_> = points
            .into_iter()
            .map(|(at, value)| {
                point(
                    bounds.origin.x + view.tick_to_x(clip_start + at),
                    bounds.origin.y
                        + px(
                            (pitch_view.top_pitch as f32 - (f32::from(note.pitch) + value) + 0.5)
                                * pitch_view.row_height,
                        ),
                )
            })
            .collect();
        paint::polyline(window, &drawn, px(1.5), color);
    }
}

impl AurisApp {
    /// The selected clip's track's sung geometry — pitch contour and phoneme cuts — cached
    /// against the revision.
    ///
    /// The frames render walks the whole track and the roll paints thirty times a second;
    /// the cache is the same arithmetic, and the same cure, as the take badge's.
    fn singer_sung_geometry(&mut self) -> Option<std::sync::Arc<SungGeometry>> {
        let clip = self.selected_clip?;
        let (track, _) = self.project().midi_clip(clip)?;
        let revision = self.session.revision();
        if self.sung_geometry_revision != revision {
            self.sung_geometry.clear();
            self.sung_geometry_revision = revision;
        }
        if let Some(geometry) = self.sung_geometry.get(&track) {
            return Some(std::sync::Arc::clone(geometry));
        }
        let frames = self.session.singer_frames(track).ok()?;
        let tempo = &self.project().tempo_map;
        let geometry = std::sync::Arc::new(SungGeometry {
            contour: f0_contour(&frames, tempo),
            backend_contour: self
                .session
                .singer_backend_pitch(track)
                .map(|pitch| pitch_contour(pitch.hz.iter().copied(), pitch.hop_seconds, tempo))
                .unwrap_or_default(),
            phonemes: phoneme_spans(&frames, tempo),
        });
        self.sung_geometry
            .insert(track, std::sync::Arc::clone(&geometry));
        Some(geometry)
    }

    /// Renders the piano roll panel.
    pub(crate) fn render_piano_roll(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let pitch_view = self.pitch.clone();
        let view = self.timeline.clone();
        let signatures = self.project().signatures.spans();
        let playhead = self.playhead_ticks();

        let Some(clip) = self.selected_midi_clip() else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(theme.surface_sunken)
                .text_color(theme.text_muted)
                .text_xs()
                .child(messages::piano_roll_empty(
                    self.language(),
                    self.t(self.pointer.create.label()),
                ))
                .into_any_element();
        };

        let clip_start = clip.start;
        let source = self.source_score();
        let clip_length = if source {
            clip.length
        } else {
            clip.sounding_length()
        };
        let clip_name = clip.name.clone();
        let notes = self.score_notes();
        let performed_bend = self.score_preview_bend();
        let singing = self.editing_a_singer_clip();
        let manual_phonemes = self
            .selected_clip
            .is_some_and(|clip| self.clip_accepts_phonemes(clip));
        let ghosts = if source {
            self.neighbouring_notes()
        } else {
            Vec::new()
        };
        let mut note_ends = if source {
            self.note_end_zones(clip_start, &notes)
        } else {
            Vec::new()
        };
        // The phoneme boundaries wear the same arrow: both zones drag a vertical edge.
        if source {
            note_ends.extend(self.phoneme_divider_zones(clip_start, &notes));
        }
        let selected: Vec<usize> = self
            .selected_notes
            .iter()
            .copied()
            .filter(|_| source)
            .collect();
        // After the last read of `clip`, whose borrow the cache lookup cannot share. What
        // the voice will sing, drawn over the notes: the pitch contour so a drawn slide
        // reads as the slide it is, and the phoneme cuts so the sixty milliseconds a
        // consonant takes is sixty milliseconds on screen.
        let geometry = match singing && source {
            true => self.singer_sung_geometry(),
            false => None,
        };
        let band = source
            .then(|| self.rubber_band(crate::app::BandSurface::Roll))
            .flatten();
        let velocity_tag = source.then(|| self.velocity_tag()).flatten();
        let edit_tag = source.then(|| self.note_edit_tag()).flatten();
        let tempo = self.project().tempo_map.clone();
        // Built before the chain rather than inside it: each one needs `&mut self`, and the
        // builder below is already holding a borrow of it.
        let curve_lanes = if source {
            self.panels.curve_lanes()
        } else if !performed_bend.is_empty() {
            vec![ClipCurve::Bend]
        } else {
            Vec::new()
        };
        let lanes: Vec<gpui::AnyElement> = curve_lanes
            .into_iter()
            .map(|which| self.render_curve_lane(which, cx))
            .collect();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            // Sized by the dock it is in and not by the widest thing inside it. Without this the
            // panel's own smallest width is its header strip's — over a thousand pixels of title,
            // tools, hint, button and slider — so a narrow window gave it that much anyway and it
            // ran out through the side of the window, taking the slider and the curve-lane button
            // somewhere no pointer could reach. The dock clips, so the damage was invisible and
            // the controls were simply gone.
            .min_w_0()
            .bg(theme.surface_sunken)
            .child(self.score_layer_tabs(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(Metrics::PANEL_HEADER_HEIGHT)
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    // The strip is words first and controls last, and a window narrow enough that
                    // they do not all fit has to give the words up rather than the controls: the
                    // dock clips, so whatever is pushed past its edge cannot be reached at all,
                    // and the two things at the end are the only way to open a curve lane or
                    // change the zoom. `min_w_0` on the row and on each run of text is what lets
                    // the text be the part that goes; without it the row's smallest width is the
                    // sum of every word in it, and the slider was the first thing over the side.
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink()
                            .min_w_0()
                            .truncate()
                            .child(messages::piano_roll_title(self.language(), &clip_name)),
                    )
                    .when(source, |row| row.child(self.tool_strip(cx)))
                    .when(
                        geometry
                            .as_ref()
                            .is_some_and(|g| !g.backend_contour.is_empty()),
                        |row| {
                            row.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.accent)
                                    .child(self.t(Key::SingerHostPitch)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.warning)
                                    .child(self.t(Key::SingerBackendPitch)),
                            )
                        },
                    )
                    .child(div().flex_1().min_w_0())
                    // The hint describes the tool in hand. It named the create and delete
                    // gestures unconditionally, and holding the velocity tool while being told
                    // how to add a note is being told about a different editor.
                    //
                    // The first thing to be given up, and the right one: it is a reminder of a
                    // gesture rather than a way of making one, so a hand that cannot see all of
                    // it has lost nothing it needs.
                    .child(div().flex_shrink().min_w_0().truncate().child(if !source {
                        self.t(Key::ScorePerformedHint).to_string()
                    } else {
                        match self.tool {
                            RollTool::Pointer => messages::piano_roll_hint(
                                self.language(),
                                self.t(self.pointer.create.label()),
                                self.t(self.pointer.delete.label()),
                            ),
                            RollTool::Velocity => {
                                messages::piano_roll_velocity_hint(self.language())
                            }
                        }
                    }))
                    .when(source, |row| {
                        row.child(button(
                            "roll-snap-length",
                            self.t(Key::SnapNoteLengthsShort),
                            ButtonStyle::Ghost,
                            self.settings.snap_note_lengths,
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                                this.apply_snap_note_lengths(!this.settings.snap_note_lengths);
                                cx.notify();
                            }),
                        ))
                    })
                    .when(source, |row| {
                        row.child(button(
                            "roll-lanes",
                            self.t(Key::CurveLanes),
                            ButtonStyle::Ghost,
                            !self.panels.curve_lanes().is_empty(),
                            theme.accent_soft,
                            &theme,
                            Self::opens_menu(cx, |this, at| this.curve_lane_menu(at)),
                        ))
                    })
                    .child(self.zoom_slider("roll-zoom", cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("keyboard")
                            .w(Metrics::KEYBOARD_WIDTH)
                            .flex_shrink_0()
                            .h_full()
                            .child({
                                let theme = theme.clone();
                                let pitch_view = pitch_view.clone();
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, cx| {
                                        paint::clipped(window, bounds, |window| {
                                            paint::keyboard(
                                                window,
                                                cx,
                                                bounds,
                                                &pitch_view,
                                                &theme,
                                            );
                                        });
                                    },
                                )
                                .size_full()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.audition_from_keyboard(event, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.stop_audition()),
                            )
                            // The keyboard scrolls with the rows beside it. It is the strip a
                            // user reaches for when they want to see a different octave, and the
                            // wheel did nothing there while working on the grid an inch away.
                            .on_scroll_wheel(cx.listener(
                                |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                    this.scroll_roll(event, cx);
                                },
                            )),
                    )
                    .child(
                        div()
                            .id("roll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            // The grid says which tool is in hand under the pointer as well as in
                            // the header. A mode is only dangerous while it is invisible, and the
                            // header is the one place the eye is not while editing notes.
                            .when(source && self.tool == RollTool::Velocity, |this| {
                                this.cursor(gpui::CursorStyle::ResizeUpDown)
                            })
                            .child({
                                let theme = theme.clone();
                                let view = view.clone();
                                let pitch_view = pitch_view.clone();
                                let recorded = self.canvas.roll.clone();
                                canvas(
                                    move |bounds, window, _| {
                                        recorded.set(Some(bounds));
                                        note_ends
                                            .iter()
                                            .map(|zone| {
                                                window.insert_hitbox(
                                                    Bounds {
                                                        origin: bounds.origin + zone.origin,
                                                        size: zone.size,
                                                    },
                                                    gpui::HitboxBehavior::Normal,
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                    move |bounds, ends: Vec<gpui::Hitbox>, window, cx| {
                                        // What the lanes do for a clip's edges: the grab is a few
                                        // pixels of a canvas with nothing behind it, so the arrow
                                        // is the only thing that can say it is there.
                                        for end in &ends {
                                            window.set_cursor_style(
                                                gpui::CursorStyle::ResizeLeftRight,
                                                end,
                                            );
                                        }
                                        paint::clipped(window, bounds, |window| {
                                            paint::rect(window, bounds, theme.surface_sunken);
                                            paint::pitch_rows(window, bounds, &pitch_view, &theme);
                                            paint::time_grid(
                                                window,
                                                bounds,
                                                &view,
                                                &signatures,
                                                &theme,
                                            );
                                            paint_clip_extent(
                                                window,
                                                bounds,
                                                &view,
                                                clip_start,
                                                clip_length,
                                                &theme,
                                            );
                                            // Under the clip in hand and over the shade, so the
                                            // neighbours read as further away without being
                                            // dimmed twice into invisibility.
                                            paint_ghost_notes(
                                                window,
                                                bounds,
                                                &ghosts,
                                                &view,
                                                &pitch_view,
                                                &theme,
                                            );
                                            paint_notes(
                                                window,
                                                cx,
                                                bounds,
                                                &notes,
                                                &selected,
                                                velocity_tag,
                                                clip_start,
                                                &view,
                                                &pitch_view,
                                                &theme,
                                                singing,
                                                geometry.is_some() || !manual_phonemes,
                                            );
                                            if !source && !singing {
                                                paint_performed_pitch(
                                                    window,
                                                    bounds,
                                                    &notes,
                                                    &performed_bend,
                                                    clip_start,
                                                    &view,
                                                    &pitch_view,
                                                    theme.accent,
                                                );
                                            }
                                            if let Some(geometry) = &geometry {
                                                let pitch_spans = note_pitch_spans(
                                                    &notes,
                                                    clip_start,
                                                    clip_start + clip_length,
                                                );
                                                if manual_phonemes {
                                                    paint_phoneme_spans(
                                                        window,
                                                        cx,
                                                        bounds,
                                                        &geometry.phonemes,
                                                        &notes,
                                                        clip_start,
                                                        &view,
                                                        &pitch_view,
                                                        &theme,
                                                    );
                                                }
                                                paint_f0_curve(
                                                    window,
                                                    bounds,
                                                    &geometry.contour,
                                                    &pitch_spans,
                                                    &view,
                                                    &pitch_view,
                                                    theme.accent,
                                                );
                                                paint_f0_curve(
                                                    window,
                                                    bounds,
                                                    &geometry.backend_contour,
                                                    &pitch_spans,
                                                    &view,
                                                    &pitch_view,
                                                    theme.warning,
                                                );
                                                paint_ornament_handles(
                                                    window,
                                                    bounds,
                                                    &notes,
                                                    clip_start,
                                                    &tempo,
                                                    &view,
                                                    &pitch_view,
                                                    &theme,
                                                );
                                            }
                                            paint::playhead(
                                                window,
                                                bounds,
                                                bounds.origin.x + view.tick_to_x(playhead),
                                                &theme,
                                            );
                                            if let Some(band) = band {
                                                paint::selection_band(window, band, &theme);
                                            }
                                            if let Some(tag) = &edit_tag {
                                                paint_note_edit_tag(
                                                    window,
                                                    cx,
                                                    bounds,
                                                    tag,
                                                    &view,
                                                    &pitch_view,
                                                    &theme,
                                                );
                                            }
                                        });
                                    },
                                )
                                .size_full()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.begin_note_drag(event, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.open_roll_menu(event, cx);
                                }),
                            )
                            .on_scroll_wheel(cx.listener(
                                |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                    this.scroll_roll(event, cx);
                                },
                            )),
                    ),
            )
            // The strips scroll among themselves once they would take more than half the panel.
            // A lane is seventy pixels and there are a hundred and twenty-nine of them to open:
            // without this, opening a fifth one squeezes the notes it is supposed to be shaping
            // down to nothing, and the way back is a lane you can no longer see to close.
            .child(
                div()
                    .id("curve-lanes")
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .max_h(gpui::relative(0.5))
                    .overflow_y_scroll()
                    .children(lanes),
            )
            .into_any_element()
    }

    /// One of the strips under the notes: the pitch bend, or the modulation.
    ///
    /// Under rather than beside, and spanning the same timeline: both are things that happen *at a
    /// moment in the phrase*, so the only useful way to look at one is with the notes it is
    /// shaping directly above it. The gutter on the left is the keyboard's width, for the same
    /// reason the track headers reserve what the ruler spends — a strip that started at the panel
    /// edge would put every point a keyboard's width away from the note it belongs to.
    ///
    /// One function for both, because they differ in exactly two ways — what the vertical axis
    /// means and what the gutter says — and a second copy would be a second set of gestures to
    /// keep in step with the first.
    fn render_curve_lane(
        &mut self,
        which: ClipCurve,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let view = self.timeline.clone();
        let playhead = self.playhead_ticks();
        let source = self.source_score();
        let Some(clip) = self.selected_midi_clip() else {
            return div().into_any_element();
        };
        let (start, length) = (
            clip.start,
            if source {
                clip.length
            } else {
                clip.sounding_length()
            },
        );
        let points = if source {
            clip.curve(which).to_vec()
        } else {
            self.score_preview_bend().as_ref().clone()
        };
        let recorded = self.canvas.curve(which).clone();

        div()
            .flex()
            .h(px(CURVE_LANE_HEIGHT))
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface_sunken)
            .child(
                div()
                    .w(Metrics::KEYBOARD_WIDTH)
                    .flex_shrink_0()
                    .h_full()
                    .px_1()
                    .pt_1()
                    .bg(theme.surface)
                    .border_r_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(curve_tag(which, self.language()))
                    // The way back out of a lane, beside the lane. The menu closes one too, but a
                    // strip somebody opened by accident should not have to be found in a menu to
                    // be put away again.
                    .when(source, |lane| {
                        lane.child(button(
                            ("curve-lane-close", lane_id(which)),
                            "×",
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                this.panels.set_curve_lane(which, false);
                                this.remember_layout();
                                cx.notify();
                            }),
                        ))
                    }),
            )
            .child(
                div()
                    .id(("curve-lane", lane_id(which)))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child({
                        let theme = theme.clone();
                        canvas(
                            move |bounds, _, _| recorded.set(Some(bounds)),
                            move |bounds, _, window, cx| {
                                paint::clipped(window, bounds, |window| {
                                    paint_curve(
                                        window, cx, bounds, &view, which, start, length, &points,
                                        playhead, &theme,
                                    );
                                });
                            },
                        )
                        .size_full()
                    })
                    .when(source, |lane| {
                        lane.cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.press_curve_lane(which, event, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                Self::opens_menu(cx, move |this, at| {
                                    // Taking points off one at a time is the ⌥-click; this is the
                                    // way back from a curve that got away from somebody. With
                                    // nothing selected there is nothing to straighten, and an
                                    // empty menu is a menu `open_menu` declines to show.
                                    let menu = crate::ui::context_menu::ContextMenu::new(
                                        at,
                                        curve_label(which, this.language()),
                                    );
                                    match this.selected_clip {
                                        Some(clip) => menu.item(
                                            this.t(Key::StraightenCurve),
                                            crate::ui::context_menu::MenuCommand::ClearCurve {
                                                clip,
                                                which,
                                            },
                                        ),
                                        None => menu,
                                    }
                                }),
                            )
                    })
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_roll(event, cx);
                    })),
            )
            .into_any_element()
    }

    /// The menu that opens and closes the strips under the notes.
    ///
    /// Ticks rather than two menus, because "which strips are showing" is one question with a list
    /// of answers, and a row that is already ticked is the way back to hiding it. The clip's own
    /// curves are marked in the label, so an imported part says where its material is.
    fn curve_lane_menu(&mut self, at: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let language = self.language();
        let open = self.panels.curve_lanes();
        let carried: Vec<ClipCurve> = self
            .selected_midi_clip()
            .map(|clip| clip.curves().collect())
            .unwrap_or_default();
        curve_lane_choices(&open, &carried).into_iter().fold(
            ContextMenu::new(at, self.t(Key::CurveLanes)),
            |menu, which| {
                let shown = self.panels.curve_lane(which);
                let label = match carried.contains(&which) {
                    true => format!("{} •", curve_label(which, language)),
                    false => curve_label(which, language),
                };
                menu.toggle(
                    label,
                    MenuCommand::ShowCurveLane {
                        which,
                        shown: !shown,
                    },
                    shown,
                )
            },
        )
    }

    /// The strip in the header saying which tool the roll has in hand.
    ///
    /// Latched buttons rather than a menu, because a mode has to be visible from across the room:
    /// the whole hazard of a tool is reaching for it, being interrupted, and coming back to a
    /// pointer that no longer does what the hand expects.
    pub(crate) fn tool_strip(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let current = self.tool;
        div()
            .flex()
            .items_center()
            .gap_1()
            // The tool in hand is the one thing in the header strip a hand reaches for while
            // editing, so it keeps its width whatever the window does.
            .flex_shrink_0()
            .children(RollTool::ALL.map(|tool| {
                button(
                    ("roll-tool", tool as usize),
                    self.t(tool.label()),
                    ButtonStyle::Ghost,
                    tool == current,
                    theme.accent_soft,
                    &theme,
                    cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        this.tool = tool;
                        cx.notify();
                    }),
                )
            }))
            .into_any_element()
    }

    /// Window origin of the note grid, taken from where it was last painted.
    ///
    /// It used to be derived from the window height and the bottom panel's fixed height, which was
    /// correct until that panel became resizable — after that, every note the user clicked was off
    /// by however far they had dragged the divider. The fallback below is only reached before the
    /// first paint, and reads the dock's *current* height for the same reason. It assumes the roll
    /// is in the bottom dock, which is where it starts and where it usually is; a roll parked down
    /// one side is one frame out of place and right from the next paint onwards.
    pub(crate) fn roll_origin(&self) -> Point<Pixels> {
        self.canvas.roll.get().map_or_else(
            || {
                point(
                    Metrics::KEYBOARD_WIDTH,
                    self.viewport_height - Metrics::STATUS_HEIGHT - self.panels.size(Dock::Bottom)
                        + Metrics::PANEL_HEADER_HEIGHT,
                )
            },
            |bounds| bounds.origin,
        )
    }

    /// Starts a note gesture, using the configured create gesture on empty space.
    fn begin_note_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        if !self.source_score() {
            return;
        }
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let origin = self.roll_origin();
        let tick = self.timeline.x_to_tick(event.position.x - origin.x);
        // Below MIDI 0 the grid is unpainted, so a click there must not act on pitch 0.
        let Some(pitch) = self.pitch.pitch_at(event.position.y - origin.y) else {
            return;
        };
        let Some(clip_start) = self.session.midi_clip(clip_id).map(|c| c.start) else {
            return;
        };
        let local_tick = tick - clip_start;

        let under_pointer = self.note_at(clip_id, local_tick, pitch);

        // The word on a note is edited where the note is: on a singer track, a double click
        // opens the lyric sheet. Ahead of every drag, because the second press of a double
        // click would otherwise begin a move that goes nowhere and swallows the gesture.
        if let Some(index) = under_pointer
            && crate::gestures::PointerGesture::DoubleClick.matches(event)
            && !self.pointer.delete.matches(event)
            && self.editing_a_singer_clip()
        {
            self.open_lyric_prompt(clip_id, index);
            cx.notify();
            return;
        }

        // The velocity tool claims a press on a note outright, ahead of the create and delete
        // gestures: a tool that sometimes removed the note instead is not a tool anyone can trust
        // to be in hand. Empty grid still sweeps a selection, as it does under every tool, so the
        // notes to work on can be gathered without putting the tool down.
        if self.tool == RollTool::Velocity {
            match under_pointer {
                Some(index) => {
                    self.begin_velocity_drag(clip_id, index, event.position.y, event.modifiers)
                }
                None => self.begin_rubber_band(
                    crate::app::BandSurface::Roll,
                    event.position,
                    event.modifiers.shift,
                ),
            }
            cx.notify();
            return;
        }

        // Alt-click must wait for release: moving instead copies the selection. Alt-Shift
        // remains available for matching ends and lengths.
        if let Some(index) = under_pointer
            && event.modifiers.alt
            && !event.modifiers.shift
        {
            let selection_before = self.selected_notes.clone();
            if !self.selected_notes.contains(&index) {
                self.selected_notes.clear();
                self.selected_notes.insert(index);
            }
            self.begin_drag(Drag::NoteCopy {
                clip: clip_id,
                grabbed: index,
                origin_tick: local_tick,
                origin_pitch: pitch,
                origins: self.selected_note_origins(clip_id),
                pressed_at: Some(event.position),
                selection_before,
                delete_on_click: self.pointer.delete.matches(event),
            });
            cx.notify();
            return;
        }

        // Delete precedes ordinary pointer edits. Alt-Shift is reserved for matching note
        // lengths, even when plain Alt-click is configured to delete.
        if let Some(index) = under_pointer
            && self.pointer.delete.matches(event)
            && !(event.modifiers.alt && event.modifiers.shift)
        {
            let _ = self.session.remove_notes(clip_id, &[index]);
            self.selected_notes.clear();
            cx.notify();
            return;
        }

        // An ornament handle is asked before the notes are: it usually floats outside every
        // note's rectangle, where a press would otherwise draw a brand-new note right under
        // the hand.
        if self.editing_a_singer_clip()
            && let Some((index, handle)) = self.grabbed_ornament_at(
                clip_id,
                clip_start,
                point(event.position.x - origin.x, event.position.y - origin.y),
            )
        {
            self.selected_notes.clear();
            self.selected_notes.insert(index);
            self.begin_drag(Drag::Ornament {
                clip: clip_id,
                index,
                handle,
            });
            cx.notify();
            return;
        }

        match under_pointer {
            Some(index) => {
                let Some(note) = self
                    .session
                    .midi_clip(clip_id)
                    .and_then(|clip| clip.notes.get(index))
                    .cloned()
                else {
                    return;
                };
                let start_x = self.timeline.tick_to_x(clip_start + note.start);
                let end_x = self.timeline.tick_to_x(clip_start + note.end());
                let resizing = f32::from(end_x - (event.position.x - origin.x)).abs()
                    <= resize_grab(end_x - start_x);
                if !event.modifiers.shift {
                    if !self.selected_notes.contains(&index) {
                        self.selected_notes.clear();
                    }
                } else if !resizing && self.selected_notes.contains(&index) {
                    self.selected_notes.remove(&index);
                    cx.notify();
                    return;
                }
                self.selected_notes.insert(index);

                self.last_note_length = Some(note.length);
                if resizing {
                    let origins = self
                        .session
                        .midi_clip(clip_id)
                        .map(|clip| {
                            self.selected_notes
                                .iter()
                                .filter_map(|index| {
                                    clip.notes
                                        .get(*index)
                                        .map(|note| (*index, note.start, note.length))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.begin_drag(Drag::NoteResize {
                        clip: clip_id,
                        index,
                        origins,
                        drawing: false,
                        pressed_at: Some(event.position),
                    });
                } else if let Some((phoneme, from_seconds, end_seconds)) =
                    self.grabbed_boundary_at(&note, clip_start, event.position.x - origin.x)
                {
                    // A press near a phoneme cut takes the cut, not the note: dragging the
                    // divider is how a syllable is re-timed, and moving the whole note is
                    // still there a few pixels either side.
                    self.begin_drag(Drag::PhonemeDuration {
                        clip: clip_id,
                        index,
                        phoneme,
                        from_seconds,
                        end_seconds,
                    });
                } else {
                    let origins = self.selected_note_origins(clip_id);
                    self.begin_drag(Drag::NoteMove {
                        clip: clip_id,
                        grabbed: index,
                        origin_tick: local_tick,
                        origin_pitch: pitch,
                        origins,
                        pressed_at: Some(event.position),
                    });
                    self.audition_note(index, pitch);
                }
            }
            None => match empty_press(self.pointer, event) {
                EmptyPress::Create => {
                    let start = snapped_note_start(tick, clip_start, self.project().grid);
                    let length = self
                        .last_note_length
                        .unwrap_or_else(|| default_note_length(self.project().grid));
                    // The new note and the resize that follows it are one gesture, so the
                    // transaction opens first and the note lands inside it.
                    self.begin_drag(Drag::NoteResize {
                        clip: clip_id,
                        index: 0,
                        origins: Vec::new(),
                        drawing: true,
                        pressed_at: None,
                    });
                    let Ok(index) = self
                        .session
                        .add_note(clip_id, Note::new(pitch, start, length))
                    else {
                        self.abandon_drag();
                        return;
                    };
                    self.drag = Some(Drag::NoteResize {
                        clip: clip_id,
                        index,
                        origins: vec![(index, start, length)],
                        drawing: true,
                        pressed_at: Some(event.position),
                    });
                    self.selected_notes.clear();
                    self.selected_notes.insert(index);
                    self.audition_note(index, pitch);
                }
                // A drag on empty grid sweeps a selection; a press that never moves ends up
                // selecting nothing, which is the deselect it looks like.
                EmptyPress::Band { extend } => {
                    self.begin_rubber_band(crate::app::BandSurface::Roll, event.position, extend);
                }
            },
        }
        cx.notify();
    }

    /// Starts the copy transaction only after an Alt/Option press becomes a drag.
    pub(crate) fn start_note_copy(&mut self) -> bool {
        let Some(Drag::NoteCopy {
            clip,
            grabbed,
            origins,
            ..
        }) = self.drag.as_ref()
        else {
            return false;
        };
        let (clip, grabbed, origins) = (*clip, *grabbed, origins.clone());
        let indices: Vec<_> = origins.iter().map(|(index, _, _)| *index).collect();
        self.session
            .begin_transaction(auris_session::Edit::DuplicateNotes);
        let copies = match self
            .session
            .duplicate_notes_at_offset(clip, &indices, Ticks::ZERO)
        {
            Ok(copies) if copies.len() == origins.len() && !copies.is_empty() => copies,
            _ => {
                self.session.revert_transaction();
                self.abort_drag();
                return false;
            }
        };
        let copied_grabbed = origins
            .iter()
            .position(|(index, _, _)| *index == grabbed)
            .and_then(|position| copies.get(position))
            .copied();
        self.selected_notes = copies.iter().copied().collect();
        let copied_origins = copies
            .into_iter()
            .zip(origins)
            .map(|(copy, (_, start, pitch))| (copy, start, pitch))
            .collect();
        if let Some(Drag::NoteCopy {
            grabbed,
            origins,
            pressed_at,
            ..
        }) = &mut self.drag
        {
            *grabbed = copied_grabbed.unwrap_or(*grabbed);
            *origins = copied_origins;
            *pressed_at = None;
        }
        true
    }

    /// Reads the document after snapping, never the unsnapped pointer coordinates.
    fn note_edit_tag(&self) -> Option<NoteEditTag> {
        let (clip, index, copying) = match self.drag.as_ref()? {
            Drag::NoteMove {
                clip,
                grabbed,
                pressed_at: None,
                ..
            } => (*clip, *grabbed, false),
            Drag::NoteCopy {
                clip,
                grabbed,
                pressed_at: None,
                ..
            } => (*clip, *grabbed, true),
            Drag::NoteResize {
                clip,
                index,
                drawing,
                pressed_at,
                ..
            } if *drawing || pressed_at.is_none() => (*clip, *index, false),
            _ => return None,
        };
        if Some(clip) != self.selected_clip {
            return None;
        }
        let clip = self.session.midi_clip(clip)?;
        let note = clip.notes.get(index)?;
        let start = clip.start + note.start;
        let (bar, beat, tick) = self.project().signatures.bar_beat_at(start);
        let signature = self.project().signatures.signature_at(start);
        let per_bar = signature.ticks_per_bar().raw().max(1);
        let per_beat = signature.ticks_per_beat().raw().max(1);
        let length = note.length.raw();
        let pitch = crate::ui::timeline::pitch_name(note.pitch);
        let title = if copying {
            messages::note_copy_title(self.language(), &pitch, self.selected_notes.len())
        } else {
            messages::note_drag_title(self.language(), &pitch, self.selected_notes.len())
        };
        Some(NoteEditTag {
            start,
            length: note.length,
            pitch: note.pitch,
            lines: [
                title,
                messages::note_drag_position(self.language(), bar, beat, tick),
                messages::note_drag_length(
                    self.language(),
                    length / per_bar,
                    length % per_bar / per_beat,
                    length % per_beat,
                ),
            ],
        })
    }

    /// Takes hold of a note's dynamics, and of every note selected along with it.
    ///
    /// Pressing a note that is not in the selection makes it the selection, which is what the
    /// pointer tool does and what Logic does: the gesture then acts on what was aimed at rather
    /// than on a chord left selected somewhere off-screen.
    pub(crate) fn begin_velocity_drag(
        &mut self,
        clip: ClipId,
        index: usize,
        y: Pixels,
        modifiers: gpui::Modifiers,
    ) {
        if modifiers.shift && !modifiers.alt {
            if self.selected_notes.remove(&index) {
                return;
            }
            self.selected_notes.insert(index);
        } else if !self.selected_notes.contains(&index) {
            self.selected_notes.clear();
            self.selected_notes.insert(index);
        }

        let Some(target) = self.session.midi_clip(clip) else {
            return;
        };
        if self.selected_notes.is_empty() {
            return;
        }
        let origins: Vec<(usize, u8)> = self
            .selected_notes
            .iter()
            .filter_map(|index| {
                target
                    .notes
                    .get(*index)
                    .map(|note| (*index, midi_velocity(note.velocity)))
            })
            .collect();
        let struck = target
            .notes
            .get(index)
            .map(|note| (note.pitch, note.velocity));
        self.last_note_length = target.notes.get(index).map(|note| note.length);

        self.begin_drag(Drag::NoteVelocity {
            clip,
            start_y: y,
            origins,
            grabbed: index,
        });
        // Heard at the level it is written at, so the drag starts from something the ear has a
        // reference for rather than from a number.
        if let Some((pitch, velocity)) = struck {
            self.audition_note_at(index, pitch, velocity);
        }
    }

    /// Moves a velocity drag to wherever the pointer has reached.
    pub(crate) fn drag_velocity(
        &mut self,
        clip: ClipId,
        start_y: Pixels,
        origins: &[(usize, u8)],
        grabbed: usize,
        y: Pixels,
        modifiers: gpui::Modifiers,
    ) {
        let dy = y - start_y;
        let uniform = (modifiers.alt && modifiers.shift)
            .then(|| {
                origins
                    .iter()
                    .find(|(index, _)| *index == grabbed)
                    .map(|(_, origin)| dragged_velocity(*origin, dy))
            })
            .flatten();
        let changes: Vec<(usize, f32)> = origins
            .iter()
            .map(|(index, origin)| {
                (
                    *index,
                    f32::from(uniform.unwrap_or_else(|| dragged_velocity(*origin, dy))) / 127.0,
                )
            })
            .collect();
        let _ = self.session.set_note_velocities(clip, &changes);
    }

    /// The note a velocity drag has hold of and what it now says, for the tag drawn beside it.
    ///
    /// Read back out of the document rather than recomputed from the drag, so the tag reports
    /// what was actually written — including the clamp at either end, which is the moment a
    /// number is worth having.
    fn velocity_tag(&self) -> Option<(usize, u8)> {
        let Some(Drag::NoteVelocity { clip, grabbed, .. }) = &self.drag else {
            return None;
        };
        let note = self.session.midi_clip(*clip)?.notes.get(*grabbed)?;
        Some((*grabbed, midi_velocity(note.velocity)))
    }

    /// Positions of every selected note, captured at the start of a move.
    pub(crate) fn selected_note_origins(&self, clip: ClipId) -> Vec<(usize, Ticks, u8)> {
        let Some(clip) = self.session.midi_clip(clip) else {
            return Vec::new();
        };
        self.selected_notes
            .iter()
            .filter_map(|index| {
                clip.notes
                    .get(*index)
                    .map(|note| (*index, note.start, note.pitch))
            })
            .collect()
    }

    /// Where the pointer would take hold of a note's end, relative to the note grid.
    ///
    /// The arrangement's [`clip_edge_zones`](super::arrangement) for notes, and the same rule
    /// keeps it honest: the cursor lights up exactly what the press acts on. Only the *inner*
    /// half of the grab, because [`Self::note_at`] has to find a note under the pointer's tick
    /// before the resize check is reached — the half hanging past the end is a zone no press can
    /// land in. Only the end, too, because a note has no front trim.
    ///
    /// Empty while the velocity tool is in hand. That tool drags a note's velocity rather than
    /// its length, and the grid already says so with a cursor of its own.
    /// The grab zones over each phoneme boundary of a singer clip's notes.
    ///
    /// The same coordinate frame as [`Self::note_end_zones`], and appended to its answer:
    /// both wear the left-right resize arrow, because both drag a vertical edge.
    fn phoneme_divider_zones(&self, clip_start: Ticks, notes: &[Note]) -> Vec<Bounds<Pixels>> {
        if self.tool != RollTool::Pointer
            || !self
                .selected_clip
                .is_some_and(|clip| self.clip_accepts_phonemes(clip))
        {
            return Vec::new();
        }
        let widths = self.editing_voice_widths();
        let tempo = &self.project().tempo_map;
        let row = px(self.pitch.row_height);
        let mut zones = Vec::new();
        for note in notes {
            if note.phonemes.len() < 2 {
                continue;
            }
            let start = tempo.ticks_to_seconds(clip_start + note.start).0;
            let end = tempo.ticks_to_seconds(clip_start + note.end()).0;
            let layout = phoneme_layout(
                &note.phonemes,
                &note.phoneme_seconds,
                (end - start).max(0.0),
                widths.as_ref(),
            );
            for (_, to) in layout.iter().take(layout.len().saturating_sub(1)) {
                let tick = tempo.seconds_to_ticks(Seconds(start + to));
                let x = self.timeline.tick_to_x(tick);
                zones.push(Bounds {
                    origin: point(
                        x - px(PHONEME_GRAB_HALF),
                        self.pitch.pitch_to_y(note.pitch) + px(1.0),
                    ),
                    size: size(px(PHONEME_GRAB), (row - px(2.0)).max(px(2.0))),
                });
            }
        }
        zones
    }

    /// The phoneme boundary a press at roll-relative `x` takes hold of, with the anchors
    /// the drag measures against.
    ///
    /// Answers `(phoneme, from_seconds, end_seconds)`. The rule is
    /// [`grabbed_phoneme_boundary`]; this only turns pixels into seconds at the current
    /// zoom, giving the grab the same few pixels of slack at every magnification.
    /// The ornament handle under a roll-relative position, as `(note index, handle)`.
    ///
    /// Asked of every note in the clip rather than the one under the pointer, because the
    /// handles float off their notes — a scoop's corner hangs below the row, in space that
    /// belongs to no note at all.
    fn grabbed_ornament_at(
        &self,
        clip: ClipId,
        clip_start: Ticks,
        position: Point<Pixels>,
    ) -> Option<(usize, OrnamentHandle)> {
        let (_, clip) = self.project().midi_clip(clip)?;
        let tempo = &self.project().tempo_map;
        for (index, note) in clip.notes.iter().enumerate() {
            if note.scoop.is_none() && note.fall.is_none() && note.vibrato.is_none() {
                continue;
            }
            let start = tempo.ticks_to_seconds(clip_start + note.start).0;
            let end = tempo.ticks_to_seconds(clip_start + note.end()).0;
            let centre = (self.pitch.top_pitch as f32 - f32::from(note.pitch))
                * self.pitch.row_height
                + self.pitch.row_height / 2.0;
            for (handle, t, semis) in ornament_handles(note, end - start) {
                let x = self
                    .timeline
                    .tick_to_x(tempo.seconds_to_ticks(Seconds(start + t)));
                let y = centre - semis * self.pitch.row_height;
                if within_ornament_handle(f32::from(position.x - x), f32::from(position.y) - y) {
                    return Some((index, handle));
                }
            }
        }
        None
    }

    fn grabbed_boundary_at(
        &self,
        note: &Note,
        clip_start: Ticks,
        x: Pixels,
    ) -> Option<(usize, f64, f64)> {
        if !self
            .selected_clip
            .is_some_and(|clip| self.clip_accepts_phonemes(clip))
        {
            return None;
        }
        let tempo = &self.project().tempo_map;
        let start = tempo.ticks_to_seconds(clip_start + note.start).0;
        let end = tempo.ticks_to_seconds(clip_start + note.end()).0;
        let at = tempo.ticks_to_seconds(self.timeline.x_to_tick(x)).0;
        let slack = (tempo
            .ticks_to_seconds(self.timeline.x_to_tick(x + px(PHONEME_GRAB_HALF)))
            .0
            - at)
            .abs();
        let widths = self.editing_voice_widths();
        grabbed_phoneme_boundary(note, start, end, at, slack, widths.as_ref())
            .map(|(phoneme, from)| (phoneme, from, end))
    }

    fn note_end_zones(&self, clip_start: Ticks, notes: &[Note]) -> Vec<Bounds<Pixels>> {
        if self.tool != RollTool::Pointer {
            return Vec::new();
        }
        let row = px(self.pitch.row_height);
        notes
            .iter()
            .filter_map(|note| {
                let start_x = self.timeline.tick_to_x(clip_start + note.start);
                let end_x = self.timeline.tick_to_x(clip_start + note.end());
                let (x, width) = note_end_span(start_x, end_x)?;
                // The rows the note is drawn in, not the row it is binned into: a pixel inside at
                // the top and bottom, which is where the note the eye sees actually is.
                Some(Bounds {
                    origin: point(x, self.pitch.pitch_to_y(note.pitch) + px(1.0)),
                    size: size(width, (row - px(2.0)).max(px(2.0))),
                })
            })
            .collect()
    }

    /// The notes of the clips either side of the one being edited, each with its clip's start.
    ///
    /// Gathered from the document rather than remembered, because a neighbour can be moved,
    /// trimmed or written to from the arrangement while the roll is open, and a cached copy would
    /// show where a phrase used to be.
    fn neighbouring_notes(&self) -> Vec<(Ticks, Vec<Note>)> {
        let Some(editing) = self.selected_clip else {
            return Vec::new();
        };
        let Some(track) = self.project().track_of_clip(editing) else {
            return Vec::new();
        };
        let Some(clips) = self
            .project()
            .track(track)
            .and_then(|track| track.kind.note_clips())
        else {
            return Vec::new();
        };
        // The stretch of song on screen, from the grid as it was last painted. Before the first
        // paint there is no width to ask about, and the whole song is the honest answer — the
        // painter culls per note anyway, so the worst of being wrong here is arithmetic.
        let width = self
            .canvas
            .roll
            .get()
            .map(|bounds| bounds.size.width)
            .unwrap_or(px(4096.0));
        let view = (
            self.timeline.x_to_tick(px(0.0)),
            self.timeline.x_to_tick(width),
        );
        let spans: Vec<(ClipId, Ticks, Ticks)> = clips
            .iter()
            .map(|clip| (clip.id, clip.start, clip.start + clip.length))
            .collect();
        let drawn = ghosted(&spans, editing, view);
        clips
            .iter()
            .filter(|clip| drawn.contains(&clip.id))
            .map(|clip| (clip.start, clip.notes.clone()))
            .collect()
    }

    /// Whether the clip in the roll sits on a singer track.
    ///
    /// The one question that turns the lyric affordances on: the fields exist on every note,
    /// but words drawn over an instrument part would be noise about a feature it does not have.
    /// The consonant widths of the edited clip's voice, where a voice with a table is chosen.
    ///
    /// What keeps the drawn segmentation, the boundary grab and the sung frames one story: all
    /// three lay phonemes out through [`phoneme_layout`] with this same answer.
    pub(crate) fn editing_voice_widths(&self) -> Option<ConsonantWidths> {
        self.selected_clip
            .and_then(|clip| self.project().track_of_clip(clip))
            .and_then(|track| self.project().track(track))
            .and_then(|track| track.kind.as_singer())
            .and_then(|singer| singer.voice.as_ref())
            .and_then(|voice| voice.consonants.clone())
    }

    pub(crate) fn editing_a_singer_clip(&self) -> bool {
        self.selected_clip
            .and_then(|clip| self.project().track_of_clip(clip))
            .and_then(|track| self.project().track(track))
            .is_some_and(|track| track.kind.is_singer())
    }

    /// Opens the sheet that edits one note's lyric.
    pub(crate) fn open_lyric_prompt(&mut self, clip: ClipId, index: usize) {
        let Some(note) = self
            .session
            .midi_clip(clip)
            .and_then(|target| target.notes.get(index))
        else {
            return;
        };
        self.selected_notes.clear();
        self.selected_notes.insert(index);
        let title = self.t(Key::PromptLyric);
        self.open_prompt(crate::ui::prompt::Prompt::new(
            title,
            crate::ui::prompt::PromptTarget::Lyric { clip, index },
            note.lyric.clone(),
        ));
    }

    /// Opens the sheet that corrects one note's phonemes.
    pub(crate) fn open_phonemes_prompt(&mut self, clip: ClipId, index: usize) {
        if !self.clip_accepts_phonemes(clip) {
            self.set_failed_status(self.t(Key::VoicevoxPhonemesHint).to_string());
            return;
        }
        let Some(note) = self
            .session
            .midi_clip(clip)
            .and_then(|target| target.notes.get(index))
        else {
            return;
        };
        self.selected_notes.clear();
        self.selected_notes.insert(index);
        let title = self.t(Key::PromptPhonemes);
        self.open_prompt(crate::ui::prompt::Prompt::new(
            title,
            crate::ui::prompt::PromptTarget::Phonemes { clip, index },
            note.phonemes.join(" "),
        ));
    }

    /// Opens the sheet that lays a phrase across the selected notes.
    pub(crate) fn open_write_lyrics_prompt(&mut self, clip: ClipId) {
        let title = self.t(Key::PromptLyrics);
        self.open_prompt(crate::ui::prompt::Prompt::new(
            title,
            crate::ui::prompt::PromptTarget::Lyrics { clip },
            String::new(),
        ));
    }

    /// The note sung after `index`, in the order the words go by: start, then pitch.
    ///
    /// What Return advances along, so a verse can be typed word after word without touching
    /// the mouse. Indices break ties so two notes struck together are each visited once.
    pub(crate) fn next_sung_note(&self, clip: ClipId, index: usize) -> Option<usize> {
        let notes = &self.session.midi_clip(clip)?.notes;
        let current = notes.get(index)?;
        let key = (current.start, current.pitch, index);
        notes
            .iter()
            .enumerate()
            .filter(|(at, note)| (note.start, note.pitch, *at) > key)
            .min_by_key(|(at, note)| (note.start, note.pitch, *at))
            .map(|(at, _)| at)
    }

    /// Index of the note at a clip-relative position.
    fn note_at(&self, clip: ClipId, tick: Ticks, pitch: u8) -> Option<usize> {
        let clip = self.session.midi_clip(clip)?;
        // Search backwards so the most recently added note wins when notes overlap, which is
        // what the user just drew and therefore what they expect to grab.
        clip.notes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, note)| note.pitch == pitch && note.contains(tick))
            .map(|(index, _)| index)
    }

    /// Opens the menu for whatever is under the pointer in the note grid.
    fn open_roll_menu(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        if !self.source_score() {
            return;
        }
        let origin = self.roll_origin();
        let tick = self.timeline.x_to_tick(event.position.x - origin.x);
        let Some(pitch) = self.pitch.pitch_at(event.position.y - origin.y) else {
            return;
        };
        let clip_start = self
            .selected_clip
            .and_then(|clip| self.session.midi_clip(clip))
            .map(|clip| clip.start)
            .unwrap_or(Ticks::ZERO);
        let local_tick = tick - clip_start;

        let under_pointer = self
            .selected_clip
            .and_then(|clip| self.note_at(clip, local_tick, pitch));
        // Right-clicking a note that is not part of the selection makes it the selection, so
        // Delete and Transpose act on what was pointed at rather than on something off-screen.
        if let Some(index) = under_pointer
            && !self.selected_notes.contains(&index)
        {
            self.selected_notes.clear();
            self.selected_notes.insert(index);
        }

        let menu = self.roll_menu(
            event.position,
            under_pointer,
            pitch,
            snapped_note_start(tick, clip_start, self.project().grid),
        );
        self.open_menu(menu);
        cx.notify();
    }

    /// Wheel handling: plain scrolls pitch, shift scrolls time, alt zooms time,
    /// control zooms the pitch axis.
    fn scroll_roll(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut gpui::Context<Self>) {
        let delta = event.delta.pixel_delta(px(24.0));
        if event.modifiers.control {
            let anchor = event.position.y - self.roll_origin().y;
            let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
            self.pitch.zoom_by(factor, anchor);
        } else if event.modifiers.alt {
            // The same origin the vertical branch above uses, and the same one the painter does.
            // `KEYBOARD_WIDTH` is only half of it — the roll starts after the panel's own padding
            // as well — so the anchor was a constant off, and the notes slid sideways on every
            // zoom notch instead of staying put under the pointer.
            let anchor = event.position.x - self.roll_origin().x;
            let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
            self.timeline.zoom_by(factor, anchor);
        } else if event.modifiers.shift {
            self.timeline.scroll_by(-delta.y - delta.x);
        } else {
            self.pitch.scroll_by(-delta.y);
            self.timeline.scroll_by(-delta.x);
        }
        cx.notify();
    }

    /// Plays the key the pointer landed on, so the keyboard strip is playable.
    fn audition_from_keyboard(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.roll_origin();
        let Some(pitch) = self.pitch.pitch_at(event.position.y - origin.y) else {
            return;
        };
        self.audition(pitch);
        cx.notify();
    }
}

pub(super) fn paint_clip_extent(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    clip_start: Ticks,
    clip_length: Ticks,
    theme: &Theme,
) {
    // Dim everything outside the clip: notes there exist but are never played.
    let start_x = bounds.origin.x + view.tick_to_x(clip_start);
    let end_x = bounds.origin.x + view.tick_to_x(clip_start + clip_length);
    let shade = Theme::translucent(theme.background, 0.45);
    if start_x > bounds.origin.x {
        paint::rect(
            window,
            Bounds {
                origin: bounds.origin,
                size: size(start_x - bounds.origin.x, bounds.size.height),
            },
            shade,
        );
    }
    let right_edge = bounds.origin.x + bounds.size.width;
    if end_x < right_edge {
        paint::rect(
            window,
            Bounds {
                origin: point(end_x.max(bounds.origin.x), bounds.origin.y),
                size: size(right_edge - end_x.max(bounds.origin.x), bounds.size.height),
            },
            shade,
        );
    }
    paint::vline(window, bounds, start_x, px(1.0), theme.accent);
    paint::vline(window, bounds, end_x, px(1.0), theme.accent);
}

/// How solid a neighbouring clip's notes are drawn.
///
/// Faint enough to read as another clip at a glance and not as something to reach for, solid
/// enough to make out a phrase against the rows behind it.
const GHOST_ALPHA: f32 = 0.34;

/// The notes of the clips either side, drawn flat and faint.
///
/// No velocity in the fill and no selection outline, both of which the clip in hand has: these
/// cannot be edited from here, and a ghost that read like a note would be an invitation to try.
/// What they are for is the shape on either side — where the phrase before this one ended, and
/// what the next one starts on.
fn paint_ghost_notes(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    ghosts: &[(Ticks, Vec<Note>)],
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    let colour = Theme::translucent(theme.text_muted, GHOST_ALPHA);
    for (clip_start, notes) in ghosts {
        for note in notes {
            let x = bounds.origin.x + view.tick_to_x(*clip_start + note.start);
            let width = view.duration_to_width(note.length).max(px(2.0));
            if x + width < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
                continue;
            }
            let y = bounds.origin.y + pitch_view.pitch_to_y(note.pitch);
            if y + px(pitch_view.row_height) < bounds.origin.y
                || y > bounds.origin.y + bounds.size.height
            {
                continue;
            }
            paint::rounded_rect(
                window,
                Bounds {
                    origin: point(x, y + px(1.0)),
                    size: size(width, px((pitch_view.row_height - 2.0).max(2.0))),
                },
                Metrics::RADIUS_XS,
                colour,
            );
        }
    }
}

/// Distance from a note's ends to the velocity bar inside it.
const VELOCITY_BAR_INSET: f32 = 2.0;

/// Narrowest a note may be and still carry a velocity bar.
///
/// Below this the bar is a smudge that says nothing about the value and everything about the
/// zoom, and the note's own colour is left to do the talking.
const VELOCITY_BAR_MIN_WIDTH: f32 = 12.0;

/// Shortest a row may be and still carry a velocity bar.
const VELOCITY_BAR_MIN_ROW: f32 = 7.0;

/// Width of the tag that reports the value during a drag.
///
/// Fixed at three digits rather than measured, so the tag does not change width — and so the
/// number does not shuffle sideways — as the value crosses 9 and 99.
const VELOCITY_TAG_WIDTH: f32 = 30.0;

/// Whether a note drawn this size has room for a legible velocity bar.
///
/// Zoomed out far enough, every note is a few pixels of colour and a bar inside one is a smudge
/// that says more about the zoom than about the value. The colour is left to do the talking
/// there, which it can, because it does not depend on how much room there is.
fn bar_fits(note_bounds: Bounds<Pixels>) -> bool {
    f32::from(note_bounds.size.width) - VELOCITY_BAR_INSET * 2.0 >= VELOCITY_BAR_MIN_WIDTH
        && f32::from(note_bounds.size.height) >= VELOCITY_BAR_MIN_ROW
}

/// Draws the bar inside a note that says how hard it is struck.
///
/// Logic's marking, and the reason it is worth having on top of the colour: a hue ramp says
/// roughly where in the range a note sits, and cannot show the difference between 96 and 100,
/// which is exactly the difference a velocity drag is being made to find.
fn paint_velocity_bar(
    window: &mut Window,
    note_bounds: Bounds<Pixels>,
    velocity: f32,
    theme: &Theme,
    fill: gpui::Hsla,
) {
    if !bar_fits(note_bounds) {
        return;
    }
    let span = f32::from(note_bounds.size.width) - VELOCITY_BAR_INSET * 2.0;
    let thickness = (note_bounds.size.height / 5.0).clamp(px(1.0), px(2.0));
    let filled = (span * velocity.clamp(0.0, 1.0)).max(1.0);
    paint::rect(
        window,
        Bounds {
            origin: point(
                note_bounds.origin.x + px(VELOCITY_BAR_INSET),
                note_bounds.origin.y + (note_bounds.size.height - thickness) / 2.0,
            ),
            size: size(px(filled), thickness),
        },
        // Against the note's own fill, which runs from blue to red: a fixed colour would vanish
        // into one end of the ramp or the other.
        Theme::translucent(theme.text_on(fill), 0.8),
    );
}

/// Fits the readout beside a note, including notes dragged beyond the visible canvas.
fn note_tag_bounds(
    canvas: Bounds<Pixels>,
    note: Bounds<Pixels>,
    wanted: gpui::Size<Pixels>,
    gap: Pixels,
) -> Bounds<Pixels> {
    let width = wanted.width.min(canvas.size.width).max(px(0.0));
    let height = wanted.height.min(canvas.size.height).max(px(0.0));
    let x = if note.right() + gap + width <= canvas.right() {
        note.right() + gap
    } else {
        note.left() - gap - width
    };
    let y = if note.top() - gap - height >= canvas.top() {
        note.top() - gap - height
    } else {
        note.bottom() + gap
    };
    Bounds {
        origin: point(
            x.clamp(canvas.left(), canvas.right() - width),
            y.clamp(canvas.top(), canvas.bottom() - height),
        ),
        size: size(width, height),
    }
}

/// Paints a non-interactive readout, measured in the current font and kept inside the roll.
fn paint_note_edit_tag(
    window: &mut Window,
    cx: &mut App,
    canvas: Bounds<Pixels>,
    tag: &NoteEditTag,
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    let font_size = window.rem_size() * 0.75;
    let padding = font_size / 2.0;
    let line_height = font_size * paint::LINE_HEIGHT;
    let width = tag
        .lines
        .iter()
        .map(|line| paint::measure_label(window, line.clone(), font_size))
        .fold(px(0.0), |a, b| a.max(b))
        + padding * 2.0;
    let note = Bounds {
        origin: point(
            canvas.left() + view.tick_to_x(tag.start),
            canvas.top() + pitch_view.pitch_to_y(tag.pitch),
        ),
        size: size(
            view.duration_to_width(tag.length),
            px(pitch_view.row_height),
        ),
    };
    let bounds = note_tag_bounds(
        canvas,
        note,
        size(width, line_height * 3.0 + padding * 2.0),
        padding,
    );
    paint::rounded_rect(window, bounds, Metrics::RADIUS_XS, theme.surface_raised);
    paint::rounded_outline(
        window,
        bounds,
        Metrics::RADIUS_XS,
        px(1.0),
        theme.text_muted,
    );
    paint::clipped(window, bounds, |window| {
        for (row, line) in tag.lines.iter().enumerate() {
            paint::label(
                window,
                cx,
                point(
                    bounds.left() + padding,
                    bounds.top() + padding + line_height * row as f32,
                ),
                line.clone(),
                font_size,
                theme.text,
            );
        }
    });
}

/// Draws the number a velocity drag has reached, beside the note it has hold of.
///
/// Logic's help tag. The value is the one thing a continuous drag cannot say by itself, and it is
/// wanted most at the ends, where the note stops changing and only the number can explain why.
fn paint_velocity_tag(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    note_bounds: Bounds<Pixels>,
    midi: u8,
    theme: &Theme,
) {
    let height = px(16.0);
    let width = px(VELOCITY_TAG_WIDTH);
    // Beside the note by preference, and on its other side when that would leave the canvas:
    // the tag is worth nothing clipped in half against the right-hand edge.
    let right = note_bounds.origin.x + note_bounds.size.width + px(6.0);
    let x = if right + width <= bounds.origin.x + bounds.size.width {
        right
    } else {
        (note_bounds.origin.x - width - px(6.0)).max(bounds.origin.x)
    };
    let y = (note_bounds.origin.y + (note_bounds.size.height - height) / 2.0).clamp(
        bounds.origin.y,
        bounds.origin.y + bounds.size.height - height,
    );
    let plate = Bounds {
        origin: point(x, y),
        size: size(width, height),
    };
    paint::rounded_rect(window, plate, Metrics::RADIUS_XS, theme.surface_raised);
    paint::rounded_outline(window, plate, Metrics::RADIUS_XS, px(1.0), theme.border);
    paint::label(
        window,
        cx,
        point(x + px(6.0), y + px(2.0)),
        midi.to_string(),
        px(10.0),
        theme.text,
    );
}

/// Shortest a row may be for a note to carry its lyric, in pixels.
///
/// Below this the word is a smear over the grid; the roll still shows the melody, and zooming
/// in is how the words come back.
const LYRIC_MIN_ROW: f32 = 11.0;

/// Shortest a row may be for the phonemes to be written above the note.
///
/// Taller than [`LYRIC_MIN_ROW`], because the phonemes borrow the row above: at a zoom where
/// rows are thin, a symbol between two notes reads as a note.
const LYRIC_PHONEME_MIN_ROW: f32 = 15.0;

/// Draws what a note sings: the lyric on the note, the phonemes in the row above it.
///
/// The lyric gets the contrast the velocity bar would have had — the two want the same spot,
/// and on a singer track the word is the thing being edited. The phonemes are the model's
/// truth, dimmer and smaller, so a hand-corrected reading is visible without being shouted.
fn paint_lyric(
    window: &mut Window,
    cx: &mut App,
    note_bounds: Bounds<Pixels>,
    note: &Note,
    theme: &Theme,
    timed: bool,
    fill: gpui::Hsla,
) {
    let row = f32::from(note_bounds.size.height);
    if row >= LYRIC_MIN_ROW && !note.lyric.is_empty() {
        let size = px((row - 4.0).clamp(8.0, 11.0));
        paint::label(
            window,
            cx,
            point(
                note_bounds.origin.x + px(3.0),
                note_bounds.origin.y + (note_bounds.size.height - size * paint::LINE_HEIGHT) / 2.0,
            ),
            note.lyric.clone(),
            size,
            theme.text_on(fill),
        );
    }
    // The untimed list yields to the timed segmentation — the same symbols drawn where
    // their frames actually fall — so it is only written when no frames are around to say.
    if !timed && row + 2.0 >= LYRIC_PHONEME_MIN_ROW && !note.phonemes.is_empty() {
        paint::label(
            window,
            cx,
            point(
                note_bounds.origin.x + px(3.0),
                note_bounds.origin.y - px(11.0),
            ),
            note.phonemes.join(" "),
            px(8.5),
            theme.text_muted,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_notes(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    notes: &[Note],
    selected: &[usize],
    tag: Option<(usize, u8)>,
    clip_start: Ticks,
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
    lyrics: bool,
    timed_phonemes: bool,
) {
    // Where the tag goes, decided in the loop and drawn after it, so a note painted later cannot
    // land on top of the one thing on the canvas that is only there to be read.
    let mut tag_at = None;
    for (index, note) in notes.iter().enumerate() {
        let x = bounds.origin.x + view.tick_to_x(clip_start + note.start);
        let width = view.duration_to_width(note.length).max(px(2.0));
        if x + width < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
            continue;
        }
        let y = bounds.origin.y + pitch_view.pitch_to_y(note.pitch);
        if y + px(pitch_view.row_height) < bounds.origin.y
            || y > bounds.origin.y + bounds.size.height
        {
            continue;
        }
        let selected = selected.contains(&index);
        let fill = theme.note_fill(note.velocity, selected);
        let note_bounds = Bounds {
            origin: point(x, y + px(1.0)),
            size: size(width, px((pitch_view.row_height - 2.0).max(2.0))),
        };
        paint::rounded_rect(window, note_bounds, Metrics::RADIUS_XS, fill);
        // The word and the bar want the same pixels; on a singer track the word wins.
        if lyrics {
            paint_lyric(window, cx, note_bounds, note, theme, timed_phonemes, fill);
        }
        if !(lyrics
            && !note.lyric.is_empty()
            && f32::from(note_bounds.size.height) >= LYRIC_MIN_ROW)
        {
            paint_velocity_bar(window, note_bounds, note.velocity, theme, fill);
        }
        if tag.is_some_and(|(grabbed, _)| grabbed == index) {
            tag_at = Some(note_bounds);
        }
        // The outline keeps selected notes visible against the lane even when the reversed
        // fill resembles its background. Bar length still conveys velocity.
        if selected {
            paint::rounded_outline(window, note_bounds, Metrics::RADIUS_XS, px(2.0), theme.text);
        }
    }

    if let (Some(note_bounds), Some((_, midi))) = (tag_at, tag) {
        paint_velocity_tag(window, cx, bounds, note_bounds, midi, theme);
    }
}

// ------------------------------------------------------------- the curve lanes

/// How tall a curve strip is drawn.
const CURVE_LANE_HEIGHT: f32 = 76.0;

/// How near a point a press has to land to take hold of it, in pixels.
const CURVE_GRAB: f32 = 7.0;

/// How large a point is drawn.
const CURVE_POINT_RADIUS: f32 = 3.0;

/// How large the numbers down the side of a curve lane are drawn.
const CURVE_SCALE_TEXT: f32 = 9.0;

/// What a strip is called, in the gutter and in the menu that opens it.
///
/// A controller is named where MIDI has a name for it and numbered where it does not — see
/// `auris_i18n::controller`. The bend is not a controller and never was: it is fourteen bits of
/// its own message, and calling it CC anything would be wrong in a way somebody would act on.
pub fn curve_label(which: ClipCurve, language: Language) -> String {
    match which {
        ClipCurve::Bend => Key::BendLane.get(language).to_string(),
        ClipCurve::Controller(number) => auris_i18n::controller::controller_label(number, language),
    }
}

/// What a strip is called in its own gutter, which is a keyboard wide and no wider.
///
/// The number rather than the name, for everything but the bend. Fifty-six pixels does not hold
/// "エクスプレッション", and a name cut off after three characters says less than `CC11` does —
/// the menu that opened the lane is where the names are, and it is one click away.
pub fn curve_tag(which: ClipCurve, language: Language) -> String {
    match which {
        ClipCurve::Bend => Key::BendLane.get(language).to_string(),
        ClipCurve::Controller(number) => format!("CC{number}"),
    }
}

/// A number that tells one strip's elements from another's.
///
/// gpui needs an id per interactive element, and two strips sharing one would share their scroll
/// state and their hover. The bend is 128 because that is the one number no controller can be.
fn lane_id(which: ClipCurve) -> usize {
    match which {
        ClipCurve::Bend => usize::from(CONTROLLER_MAX) + 1,
        ClipCurve::Controller(number) => usize::from(number),
    }
}

/// The rows the lane menu offers, in the order it offers them.
///
/// The bend, then the controllers a keyboard actually has, then anything else this clip is
/// already carrying or already showing. That last group is what makes an imported file usable: a
/// part shaped by controller 85 has a lane to open, and it is the one lane somebody is looking
/// for — a menu of eight fixed rows would leave that curve audible and undrawable.
pub fn curve_lane_choices(open: &[ClipCurve], carried: &[ClipCurve]) -> Vec<ClipCurve> {
    let mut rows = vec![ClipCurve::Bend];
    rows.extend(
        auris_i18n::controller::NOTABLE
            .into_iter()
            .map(ClipCurve::Controller),
    );
    let mut extra: Vec<ClipCurve> = open
        .iter()
        .chain(carried)
        .copied()
        .filter(|which| which.controller().is_some() && !rows.contains(which))
        .collect();
    extra.sort_unstable();
    extra.dedup();
    rows.extend(extra);
    rows
}

/// Where `value` sits in the strip, from 0 at the top to 1 at the bottom.
///
/// A bend gets the whole of its limit either way with nothing at the middle — not the two
/// semitones MIDI assumes, because the document works in semitones and can hold an octave, and a
/// strip that only reached a tone would make a dive of a fifth undrawable and, worse, unreadable
/// once written. The wheel gets the bottom of the strip for nothing and the top for all the way
/// up, because that is the whole of its travel: half a strip drawn under a control that cannot go
/// there would be half a strip of nothing.
pub fn curve_row(which: ClipCurve, value: f32) -> f32 {
    let (low, high) = which.range();
    let value = value.clamp(low, high);
    match which.is_bipolar() {
        true => 0.5 - (value / high) * 0.5,
        false => 1.0 - value / high,
    }
}

/// The value a row in the strip stands for. The inverse of [`curve_row`].
pub fn curve_of_row(which: ClipCurve, row: f32) -> f32 {
    let (_, high) = which.range();
    let row = row.clamp(0.0, 1.0);
    match which.is_bipolar() {
        true => (0.5 - row) * 2.0 * high,
        false => (1.0 - row) * high,
    }
}

/// The two numbers written down the side of a curve lane: the top of the scale, then the bottom.
///
/// Both ends, and not just the top. A single number in a corner reads as a *value*: the wheel's
/// lane carried "127" alone, over a zero line that sits on the floor where there is nothing to
/// mark it out as a line, and it was taken for a wheel that had come up full. Two numbers, one at
/// each end, can only be a scale.
///
/// The bend is written signed for the same reason — a lane labelled `12` and `-12` says at a
/// glance that zero is between them.
pub fn curve_scale(which: ClipCurve) -> (String, String) {
    let (low, high) = which.range();
    match which {
        ClipCurve::Bend => (format!("{high:+.0}"), format!("{low:+.0}")),
        // Stored as a fraction, but read as a MIDI controller: 127 is what somebody who has seen
        // a mod wheel before expects the top of its travel to be called.
        ClipCurve::Controller(_) => ("127".to_string(), format!("{low:.0}")),
    }
}

/// `CURVE_GRAB` as a duration, so the zone is the same handful of pixels at any zoom.
///
/// A radius is a *length*, which is why this is `width_to_duration` and can never be `x_to_tick`:
/// the latter answers where a pixel column is, and so adds the scroll. Five bars along, seven
/// pixels came back as some nineteen thousand ticks — a press on empty strip took hold of a point
/// a bar or more away and the first move of the pointer flung it across the clip, an alt-click
/// deleted a point nowhere near it, and a curve that had one point could never be given a second
/// anywhere on screen. Never less than a tick, so that zoomed far enough out the zone is at least
/// the point itself.
fn curve_grab_radius(view: &TimelineView) -> Ticks {
    Ticks(view.width_to_duration(px(CURVE_GRAB)).raw().abs().max(1))
}

/// The point within `radius` ticks of `at`, nearest first.
///
/// In ticks rather than pixels so the answer does not change with the zoom in a way the caller has
/// to compensate for; the caller turns its pixels into ticks, which it has to do anyway.
pub fn curve_point_at(
    points: &[CurvePoint],
    clip_length: Ticks,
    at: Ticks,
    radius: Ticks,
) -> Option<Ticks> {
    points
        .iter()
        .filter(|point| point.at <= clip_length)
        .map(|point| (point.at, (point.at - at).raw().abs()))
        .filter(|(_, distance)| *distance <= radius.raw().abs())
        .min_by_key(|(_, distance)| *distance)
        .map(|(at, _)| at)
}

/// Draws a curve: its zero line, the curve itself, and a handle on each point.
#[allow(clippy::too_many_arguments)]
fn paint_curve(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    which: ClipCurve,
    clip_start: Ticks,
    clip_length: Ticks,
    points: &[CurvePoint],
    playhead: Ticks,
    theme: &Theme,
) {
    let top = f32::from(bounds.origin.y);
    let height = f32::from(bounds.size.height);
    let at = |tick: Ticks, value: f32| {
        point(
            bounds.origin.x + view.tick_to_x(clip_start + tick),
            px(top + height * curve_row(which, value)),
        )
    };

    // The stretch the clip covers, so a curve is read against the notes rather than against the
    // whole song — the same tint the grid above uses for the same purpose.
    paint::rect(
        window,
        Bounds {
            origin: point(
                bounds.origin.x + view.tick_to_x(clip_start),
                bounds.origin.y,
            ),
            size: size(
                view.tick_to_x(clip_start + clip_length) - view.tick_to_x(clip_start),
                bounds.size.height,
            ),
        },
        Theme::translucent(theme.surface, 0.5),
    );
    // Nothing, drawn: without it a flat curve at rest and an empty strip look the same, and the
    // one number a person needs to find again is where they started from. On a bend that line is
    // across the middle; on the wheel it is the floor, which is where nothing is.
    paint::rect(
        window,
        Bounds {
            origin: point(bounds.origin.x, px(top + height * curve_row(which, 0.0))),
            size: size(bounds.size.width, px(1.0)),
        },
        theme.border,
    );
    let (high, low) = curve_scale(which);
    paint::label(
        window,
        cx,
        point(bounds.origin.x + px(3.0), px(top + 1.0)),
        high,
        px(CURVE_SCALE_TEXT),
        theme.text_faint,
    );
    paint::label(
        window,
        cx,
        point(
            bounds.origin.x + px(3.0),
            px(top + height - CURVE_SCALE_TEXT * paint::LINE_HEIGHT - 1.0),
        ),
        low,
        px(CURVE_SCALE_TEXT),
        theme.text_faint,
    );

    let visible: Vec<&CurvePoint> = points
        .iter()
        .filter(|point| point.at <= clip_length)
        .collect();
    if !visible.is_empty() {
        // Held flat outside the points, which is what `curve_at` says and therefore what is heard.
        // A curve drawn only between its ends would show a slide starting somewhere it does not.
        let mut drawn: Vec<Point<Pixels>> = Vec::with_capacity(visible.len() + 2);
        let first = visible[0];
        let last = visible[visible.len() - 1];
        drawn.push(at(Ticks::ZERO, first.value));
        drawn.extend(visible.iter().map(|point| at(point.at, point.value)));
        if last.at < clip_length {
            drawn.push(at(clip_length, last.value));
        }
        paint::polyline(window, &drawn, px(1.5), theme.accent);
        for held in visible {
            let centre = at(held.at, held.value);
            paint::rounded_rect(
                window,
                Bounds {
                    origin: point(
                        centre.x - px(CURVE_POINT_RADIUS),
                        centre.y - px(CURVE_POINT_RADIUS),
                    ),
                    size: size(px(CURVE_POINT_RADIUS * 2.0), px(CURVE_POINT_RADIUS * 2.0)),
                },
                px(CURVE_POINT_RADIUS),
                theme.accent,
            );
        }
    }

    paint::playhead(
        window,
        bounds,
        bounds.origin.x + view.tick_to_x(playhead),
        theme,
    );
}

impl AurisApp {
    /// A press in a curve strip: take a point off, take hold of one, or write one and drag it.
    ///
    /// The three cases in the order a hand expects them, which is the order the automation lane
    /// put them in — delete first so the gesture bound to deleting takes a point off rather than
    /// adding one on top of it, and a press on empty strip writes the point it is about to drag,
    /// so placing a bend and shaping it is one gesture rather than click, look, click again.
    fn press_curve_lane(
        &mut self,
        which: ClipCurve,
        event: &MouseDownEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        if !self.source_score() {
            return;
        }
        let (Some(bounds), Some(clip)) = (self.canvas.curve(which).get(), self.selected_clip)
        else {
            return;
        };
        let Some(held) = self.session.midi_clip(clip) else {
            return;
        };
        let (clip_start, clip_length, points) =
            (held.start, held.length, held.curve(which).to_vec());
        let raw = self.timeline.x_to_tick(event.position.x - bounds.origin.x);
        let at = (self.snap_unless_held(raw, event.modifiers) - clip_start).max_zero();
        let grabbed = curve_point_at(
            &points,
            clip_length,
            (raw - clip_start).max_zero(),
            curve_grab_radius(&self.timeline),
        );

        if let Some(existing) = grabbed
            && self.pointer.delete.matches(event)
        {
            self.session.remove_curve_point(clip, which, existing);
            cx.notify();
            return;
        }

        let row =
            f32::from(event.position.y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0);
        let from = grabbed.unwrap_or(at);
        // Transaction first, point inside it — the same order `press_automation` and note
        // creation keep, so writing the point and the wobble before release undo as one step.
        self.begin_drag(Drag::CurvePoint {
            clip,
            which,
            at: from,
        });
        if grabbed.is_none()
            && !self
                .session
                .set_curve_point(clip, which, at, curve_of_row(which, row))
        {
            self.abandon_drag();
            return;
        }
        cx.notify();
    }

    /// Moves the point in hand to where the pointer is, and says where it landed.
    ///
    /// The point is looked up by where it currently sits rather than by where the drag began, the
    /// way the automation lane's is: a point dropped onto another replaces it, and the drag has to
    /// go on holding whatever survived.
    pub(crate) fn drag_curve_point(
        &mut self,
        clip: ClipId,
        which: ClipCurve,
        at: Ticks,
        event: &gpui::MouseMoveEvent,
    ) -> Option<Ticks> {
        let bounds = self.canvas.curve(which).get()?;
        let clip_start = self.session.midi_clip(clip)?.start;
        let to = (self.snap_unless_held(
            self.timeline.x_to_tick(event.position.x - bounds.origin.x),
            event.modifiers,
        ) - clip_start)
            .max_zero();
        let row =
            f32::from(event.position.y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0);
        self.session
            .move_curve_point(clip, which, at, to, curve_of_row(which, row))
    }
}
#[cfg(test)]
mod backend_window_tests {
    use super::*;

    #[gpui::test]
    fn voicevox_boundaries_do_not_offer_an_inaudible_drag(cx: &mut gpui::TestAppContext) {
        let path = crate::ui::singer::voicevox_test_file();
        let (app, cx, track, clip) = crate::harness::with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.select_track(track);
            let mut note = Note::new(60, Ticks::ZERO, Ticks::QUARTER);
            note.phonemes = vec!["k".into(), "a".into()];
            note.phoneme_seconds = vec![0.1, 0.0];
            this.session.add_note(clip, note.clone()).unwrap();
            let cut = this.project().tempo_map.seconds_to_ticks(Seconds(0.1));
            let x = this.timeline.tick_to_x(cut);
            assert!(
                !this
                    .phoneme_divider_zones(Ticks::ZERO, &[note.clone()])
                    .is_empty()
            );
            assert!(this.grabbed_boundary_at(&note, Ticks::ZERO, x).is_some());
            this.session.set_singer_voice(track, Some(&path)).unwrap();
            assert!(
                this.phoneme_divider_zones(Ticks::ZERO, &[note.clone()])
                    .is_empty()
            );
            assert!(this.grabbed_boundary_at(&note, Ticks::ZERO, x).is_none());
            assert!(
                !this.note_end_zones(Ticks::ZERO, &[note]).is_empty(),
                "ordinary note length stays editable"
            );
        });
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_readouts_fit_at_canvas_edges_and_when_the_canvas_is_small() {
        for canvas in [
            Bounds {
                origin: point(px(50.0), px(100.0)),
                size: size(px(400.0), px(180.0)),
            },
            Bounds {
                origin: point(px(50.0), px(100.0)),
                size: size(px(30.0), px(20.0)),
            },
        ] {
            for origin in [
                canvas.origin,
                canvas.bottom_right(),
                point(px(-500.0), px(900.0)),
            ] {
                let note = Bounds {
                    origin,
                    size: size(px(80.0), px(12.0)),
                };
                let tag = note_tag_bounds(canvas, note, size(px(170.0), px(65.0)), px(6.0));
                assert!(tag.left() >= canvas.left() && tag.right() <= canvas.right());
                assert!(tag.top() >= canvas.top() && tag.bottom() <= canvas.bottom());
            }
        }
    }

    #[test]
    fn new_notes_snap_to_the_song_grid_before_becoming_clip_relative() {
        let grid = Ticks(480);
        let clip_start = Ticks(100);
        assert_eq!(snapped_note_start(Ticks(700), clip_start, grid), Ticks(380));
        assert_eq!(clip_start + Ticks(380), Ticks(480));
    }

    #[test]
    fn ornament_hit_testing_matches_the_drawn_square() {
        assert!(within_ornament_handle(3.0, -3.0));
        assert!(!within_ornament_handle(3.1, 0.0));
        assert!(!within_ornament_handle(0.0, -3.1));
    }

    #[test]
    fn phoneme_hit_slack_is_the_drawn_half_width() {
        assert_eq!(PHONEME_GRAB_HALF * 2.0, PHONEME_GRAB);
    }

    #[test]
    fn a_boundary_grab_answers_the_phoneme_and_ignores_the_edges() {
        let mut note = Note::new(60, Ticks::ZERO, Ticks::from_beats(1.0));
        note.phonemes = vec!["k".to_string(), "a".to_string()];
        // Near the 60 ms cut of a note spanning 1.0..1.5 s: the k is in hand, measured
        // from the note's start.
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.062, 0.01, None),
            Some((0, 1.0))
        );
        // Too far from the cut, nothing answers; the note's own edges never do.
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.2, 0.01, None),
            None
        );
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.0, 0.005, None),
            None
        );
        // A voice whose export measured its consonants moves the cut, and the grab sits on
        // the moved cut — the segmentation drawn and the boundary grabbed stay one layout.
        let widths = ConsonantWidths {
            default: 0.060,
            seconds: [("k".to_string(), 0.100)].into_iter().collect(),
        };
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.1, 0.01, Some(&widths)),
            Some((0, 1.0))
        );
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.062, 0.01, Some(&widths)),
            None,
            "the old fixed cut no longer answers"
        );
        // A pin moves the cut, and the grab follows it.
        note.phoneme_seconds = vec![0.2, 0.0];
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.2, 0.01, None),
            Some((0, 1.0))
        );
        // When generous pointer slack reaches two cuts, the cut nearest the pointer wins rather
        // than whichever phoneme happened to come first in the note.
        note.phonemes = vec!["k".to_string(), "a".to_string(), "i".to_string()];
        note.phoneme_seconds = vec![0.1, 0.1, 0.0];
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.19, 0.1, None),
            Some((1, 1.1))
        );
        // One phoneme has no cut to move.
        note.phonemes = vec!["a".to_string()];
        note.phoneme_seconds.clear();
        assert_eq!(
            grabbed_phoneme_boundary(&note, 1.0, 1.5, 1.06, 0.5, None),
            None
        );
    }

    #[test]
    fn the_handles_sit_on_the_corners_and_the_crest() {
        let mut note = Note::new(60, Ticks::ZERO, Ticks::QUARTER);
        note.scoop = Some(Scoop {
            depth: 1.0,
            seconds: 0.2,
        });
        note.fall = Some(Fall {
            depth: 2.0,
            seconds: 0.4,
        });
        note.vibrato = Some(Vibrato {
            depth: 0.5,
            rate: 6.0,
            delay: 0.3,
            fade_in: 0.2,
        });
        let handles = ornament_handles(&note, 1.0);
        assert_eq!(handles.len(), 3);
        assert_eq!(handles[0], (OrnamentHandle::Scoop, 0.2, -1.0));
        assert_eq!(handles[1], (OrnamentHandle::Fall, 0.6, -2.0));
        assert_eq!(handles[2], (OrnamentHandle::Vibrato, 0.3, 0.5));

        // A span past half the note is capped where the audible gesture is capped, so the
        // handle stays on the contour.
        note.scoop = Some(Scoop {
            depth: 1.0,
            seconds: 3.0,
        });
        assert_eq!(ornament_handles(&note, 1.0)[0].1, 0.5);

        // An unadorned note offers nothing to grab.
        let plain = Note::new(60, Ticks::ZERO, Ticks::QUARTER);
        assert!(ornament_handles(&plain, 1.0).is_empty());
    }

    #[test]
    fn backend_contour_uses_its_frame_clock_and_breaks_at_unvoiced_frames() {
        let tempo = TempoMap::constant(120.0);
        let hz = [0.0, 440.0, 466.1637615, 0.0, 220.0, f64::NAN, -1.0];
        let runs = pitch_contour(hz.into_iter(), 256.0 / 24000.0, &tempo);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len(), 2);
        assert_eq!(runs[1].len(), 1);
        assert_eq!(
            runs[0][0].0,
            tempo.seconds_to_ticks(Seconds(256.0 / 24000.0))
        );
        assert!((runs[0][0].1 - 69.0).abs() < 0.001);
        assert!((runs[0][1].1 - 70.0).abs() < 0.001);
        assert!((runs[1][0].1 - 57.0).abs() < 0.001);
        assert_eq!(
            runs[1][0].0,
            tempo.seconds_to_ticks(Seconds(4.0 * 256.0 / 24000.0))
        );
    }

    #[test]
    fn pitch_is_drawn_only_inside_the_selected_clips_notes() {
        let notes = [
            Note::new(60, Ticks(20), Ticks(20)),
            Note::new(62, Ticks(61), Ticks(19)),
        ];
        let spans = note_pitch_spans(&notes, Ticks(100), Ticks(200));
        // Nonzero backend pitch everywhere, including before, between and after the notes.
        let contour = vec![
            (100..=200)
                .step_by(10)
                .map(|tick| (Ticks(tick), 60.0))
                .collect(),
        ];
        let drawn = note_pitch_contour(&contour, &spans);
        assert_eq!(
            drawn,
            [
                vec![(Ticks(120), 60.0), (Ticks(130), 60.0), (Ticks(140), 60.0)],
                vec![(Ticks(161), 60.0), (Ticks(170), 60.0), (Ticks(180), 60.0)],
            ]
        );
        assert!(note_pitch_contour(&contour, &[]).is_empty());

        // A one-tick rest contains no sampled point, but must still split the polyline.
        let notes = [
            Note::new(60, Ticks(20), Ticks(20)),
            Note::new(62, Ticks(41), Ticks(39)),
        ];
        let spans = note_pitch_spans(&notes, Ticks(100), Ticks(200));
        let contour = vec![vec![
            (Ticks(125), 60.0),
            (Ticks(135), 60.0),
            (Ticks(145), 61.0),
            (Ticks(155), 61.0),
        ]];
        let drawn = note_pitch_contour(&contour, &spans);
        assert_eq!(drawn.len(), 2);
        assert_eq!(
            drawn[0],
            [(Ticks(125), 60.0), (Ticks(135), 60.0), (Ticks(140), 60.5)]
        );
        assert_eq!(
            drawn[1],
            [(Ticks(141), 60.6), (Ticks(145), 61.0), (Ticks(155), 61.0)]
        );
    }

    #[test]
    fn pitch_spans_merge_overlaps_and_clip_notes_to_the_clip_edges() {
        let notes = [
            Note::new(64, Ticks(30), Ticks(100)),
            Note::new(60, Ticks(0), Ticks(20)),
            Note::new(62, Ticks(10), Ticks(20)),
            Note::new(65, Ticks(200), Ticks(20)),
        ];
        let spans = note_pitch_spans(&notes, Ticks(100), Ticks(200));
        assert_eq!(spans, [(Ticks(100), Ticks(200))]);
        // Restricting to notes must also preserve the backend's unvoiced gaps.
        let contour = vec![
            vec![(Ticks(110), 60.0), (Ticks(120), 60.0)],
            vec![(Ticks(130), 62.0), (Ticks(140), 62.0)],
        ];
        assert_eq!(note_pitch_contour(&contour, &spans), contour);
        assert!(note_pitch_spans(&[], Ticks(100), Ticks(200)).is_empty());
    }

    #[gpui::test]
    fn the_roll_keeps_both_contours_and_refreshes_after_take_and_note_edits(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx, track, clip) = crate::harness::with_a_singer_clip(cx);
        app.update(cx, |this, cx| {
            let path = this.session.project_folder().unwrap().join("pitch.voicevox.json");
            std::fs::write(&path, r#"{"format_version":1,"url":"http://127.0.0.1:1","styles":[{"name":"Test","query_style_id":6000,"decode_style_id":3001}]}"#).unwrap();
            this.session.set_singer_voice(track, Some(&path)).unwrap();
            this.session.add_note(clip, Note::new(69, Ticks::ZERO, Ticks::QUARTER)).unwrap();
            this.session.add_note(clip, Note::new(72, Ticks::QUARTER * 2, Ticks::QUARTER)).unwrap();
            this.selected_clip = Some(clip);
            let before = this.singer_sung_geometry().unwrap();
            assert!(!before.contour.is_empty());
            assert!(before.backend_contour.is_empty());
            let plan = this.session.sing_plan(track, None).unwrap();
            let render = auris_session::SingingRender {
                samples: vec![0.0; plan.frames.len() * 256],
                backend_pitch: Some(SingerPitch {
                    hop_seconds: plan.frames.hop_seconds,
                    hz: vec![450.0; plan.frames.len()],
                }),
            };
            this.session.land_singer_render(&plan, &render).unwrap();
            let after = this.singer_sung_geometry().unwrap();
            assert_eq!(before.contour, after.contour);
            assert!(!after.backend_contour.is_empty());
            assert_ne!(after.contour, after.backend_contour);
            let selected = this.selected_midi_clip().unwrap();
            let spans = note_pitch_spans(&selected.notes, selected.start, selected.start + selected.length);
            assert!(after.backend_contour.iter().flatten().any(|(tick, _)| {
                *tick > Ticks::QUARTER && *tick < Ticks::QUARTER * 2
            }), "the backend supplies pitch even in the rest");
            for contour in [&after.contour, &after.backend_contour] {
                let drawn = note_pitch_contour(contour, &spans);
                assert!(drawn.len() >= 2, "both notes retain their pitch curves");
                assert!(drawn.iter().all(|run| spans.iter().any(|&(from, to)| {
                    run.first().unwrap().0 >= from && run.last().unwrap().0 <= to
                })), "neither curve may cross the rest or leave the written notes");
            }
            this.session.transpose_notes(clip, &[0], 1).unwrap();
            assert!(this.singer_sung_geometry().unwrap().backend_contour.is_empty());
            this.session.undo();
            assert_eq!(this.singer_sung_geometry().unwrap().backend_contour, after.backend_contour);
            cx.notify();
        });
        crate::harness::paint(&app, cx);
    }

    #[test]
    fn the_contour_splits_at_silence_and_reads_pitch_in_fractions() {
        // Two voiced spans around a rest, the second a quarter-tone above A3: the drawn
        // line must break at the rest, and the fraction must survive into the pitch so a
        // bend reads as a slide rather than a stair.
        let frames = SingerFrames {
            hop_seconds: 0.01,
            inventory: vec![SILENCE.to_string(), "a".to_string()],
            phonemes: vec![1, 1, 0, 1, 1],
            f0_hz: vec![440.0, 440.0, 0.0, 226.446, 226.446],
            energy: vec![0.5, 0.5, 0.0, 0.5, 0.5],
        };
        let tempo = TempoMap::constant(120.0);
        let runs = f0_contour(&frames, &tempo);
        assert_eq!(runs.len(), 2, "the rest is a gap, not a point");
        assert!(runs[0].iter().all(|(_, pitch)| (pitch - 69.0).abs() < 1e-3));
        assert!(
            runs[1].iter().all(|(_, pitch)| (pitch - 57.5).abs() < 0.01),
            "a quarter-tone above A3 stays a fraction, got {:?}",
            runs[1]
        );
        // Ticks march with time: at 120 BPM a 10 ms hop is 1/50 beat.
        assert_eq!(runs[0][0].0, Ticks::ZERO);
        assert!(runs[0][1].0 > Ticks::ZERO);
        assert!(runs[1][0].0 > runs[0][1].0);
    }

    #[test]
    fn the_pitch_curve_stops_at_notes_and_splits_even_sub_frame_rests() {
        let contour = vec![
            (0..=6)
                .map(|index| (Ticks(index * 10), 60.0 + index as f32))
                .collect(),
        ];
        let notes = [
            Note::new(60, Ticks(10), Ticks(15)),
            Note::new(62, Ticks(26), Ticks(12)),
            Note::new(64, Ticks(41), Ticks(1)),
        ];
        let spans = note_pitch_spans(&notes, Ticks::ZERO, Ticks(60));
        let drawn = note_pitch_contour(&contour, &spans);
        assert_eq!(
            drawn,
            vec![
                vec![(Ticks(10), 61.0), (Ticks(20), 62.0), (Ticks(25), 62.5)],
                vec![(Ticks(26), 62.6), (Ticks(30), 63.0), (Ticks(38), 63.8)],
                vec![(Ticks(41), 64.1), (Ticks(42), 64.2)],
            ],
            "no leading/trailing curve or bridge over a rest, even between sampled points"
        );
        assert!(note_pitch_contour(&contour, &[]).is_empty());
    }

    #[test]
    fn the_pitch_curve_uses_absolute_time_and_stays_inside_the_clip() {
        let contour = vec![vec![(Ticks(90), 60.0), (Ticks(150), 66.0)]];
        let notes = [
            Note::new(60, Ticks(-10), Ticks(20)),
            Note::new(64, Ticks(30), Ticks(30)),
            Note::new(67, Ticks(50), Ticks(10)),
        ];
        assert_eq!(
            note_pitch_contour(&contour, &note_pitch_spans(&notes, Ticks(100), Ticks(140))),
            vec![
                vec![(Ticks(100), 61.0), (Ticks(110), 62.0)],
                vec![(Ticks(130), 64.0), (Ticks(140), 65.0)],
            ]
        );
    }

    #[test]
    fn overlapping_notes_share_a_pitch_curve_without_joining_unvoiced_spans() {
        let contour = vec![
            vec![(Ticks(0), 60.0), (Ticks(10), 61.0), (Ticks(20), 62.0)],
            vec![(Ticks(40), 64.0), (Ticks(50), 65.0), (Ticks(60), 66.0)],
        ];
        let notes = [
            Note::new(64, Ticks(15), Ticks(50)),
            Note::new(60, Ticks(0), Ticks(25)),
            Note::new(62, Ticks(25), Ticks(15)),
        ];
        assert_eq!(
            note_pitch_contour(&contour, &note_pitch_spans(&notes, Ticks::ZERO, Ticks(80))),
            contour,
            "overlapping and adjacent unsorted notes preserve each voiced run once"
        );
    }

    #[test]
    fn the_segmentation_reads_the_frames_and_drops_the_rests() {
        // か held over four frames between rests: the consonant and the vowel come out as
        // two spans sharing one boundary, and the silence around them is no segment at all.
        let frames = SingerFrames {
            hop_seconds: 0.01,
            inventory: vec![SILENCE.to_string(), "k".to_string(), "a".to_string()],
            phonemes: vec![0, 1, 1, 2, 2, 0],
            f0_hz: vec![0.0, 440.0, 440.0, 440.0, 440.0, 0.0],
            energy: vec![0.0, 0.5, 0.5, 0.5, 0.5, 0.0],
        };
        let tempo = TempoMap::constant(120.0);
        let spans = phoneme_spans(&frames, &tempo);
        assert_eq!(spans.len(), 2, "silence is no segment");
        assert_eq!(spans[0].symbol, "k");
        assert_eq!(spans[1].symbol, "a");
        assert_eq!(
            spans[0].to, spans[1].from,
            "the cut is one boundary, not a gap"
        );
        assert!(
            spans[0].from > Ticks::ZERO,
            "the leading rest pushed the consonant off zero"
        );
        assert!(spans[1].to > spans[1].from);
    }

    #[test]
    fn the_roll_ghosts_the_rest_of_the_track_and_never_the_clip_in_hand() {
        let bar = Ticks::QUARTER * 4;
        let id = ClipId;
        let clips = [
            (id(1), Ticks::ZERO, bar),
            (id(2), bar, bar * 2),
            (id(3), bar * 2, bar * 3),
            (id(4), bar * 8, bar * 9),
        ];

        // Editing the second: the ones either side of it, and not itself. Drawing the edited clip
        // twice would put a grey copy of every note under the note being dragged.
        let view = (Ticks::ZERO, bar * 4);
        assert_eq!(ghosted(&clips, id(2), view), vec![id(1), id(3)]);

        // Far off to the right and outside the view, so not drawn — and once the view reaches it,
        // drawn, without the roll having to know which clip is "next".
        assert!(!ghosted(&clips, id(2), view).contains(&id(4)));
        assert!(ghosted(&clips, id(2), (bar * 7, bar * 10)).contains(&id(4)));

        // A clip ending exactly where the view starts has nothing inside it to draw: a clip's end
        // is the tick the next one may begin on, so the two would otherwise both claim it.
        assert!(ghosted(&clips, id(2), (bar, bar * 2)).is_empty());
    }

    #[test]
    fn a_note_can_always_be_moved_however_short_it_is() {
        // At a 1/32 grid a note is about eight pixels across. A fixed five-pixel handle either
        // side of its end covered the whole note, so it could be stretched and never dragged.
        assert_eq!(resize_grab(px(120.0)), RESIZE_HANDLE);
        let short = resize_grab(px(8.0));
        assert!(short < RESIZE_HANDLE);
        assert!(
            short * 2.0 < 8.0,
            "there is a middle left over to take hold of",
        );
    }

    #[test]
    fn the_resize_cursor_covers_what_a_press_would_actually_grab() {
        // The arrow and the press read the same number, and this is what keeps them reading it
        // the same way: every pixel the arrow lights up has to be one where the press takes the
        // resize branch *and* one where `note_at` still finds the note. A zone that ran past the
        // end would promise a grab on empty grid.
        for width in [3.0f32, 8.0, 24.0, 96.0, 400.0] {
            let (start_x, end_x) = (px(200.0), px(200.0 + width));
            let Some((x, zone)) = note_end_span(start_x, end_x) else {
                continue;
            };
            let grab = resize_grab(end_x - start_x);
            assert!(x >= start_x, "a {width}px note lit up grid to its left");
            assert_eq!(x + zone, end_x, "the zone stops at the note's end");
            for step in 0..=10 {
                let at = x + zone * (step as f32 / 10.0);
                // A thousandth of a pixel of slack, and only at the boundary: `end_x - grab`
                // and then `end_x` minus that do not round-trip exactly in `f32`, so the zone's
                // own left edge can miss the press rule by an ulp. No pointer position lands
                // there, and widening the zone to hide it would be arithmetic dressed as design.
                assert!(
                    f32::from(end_x - at).abs() <= grab + 1e-3,
                    "{at:?} on a {width}px note is lit but would not resize",
                );
                assert!(at <= end_x, "{at:?} is past the note");
            }
        }
        // A zero-width note has no end to grab. A positive note keeps a proportional zone even
        // below three pixels, so a very short note can still be resized rather than only moved.
        assert_eq!(note_end_span(px(100.0), px(100.0)), None);
        let (start, width) = note_end_span(px(100.0), px(101.0)).unwrap();
        assert!((f32::from(start) - 100.0 - 2.0 / 3.0).abs() < 1e-3);
        assert!((f32::from(width) - 1.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn the_roll_opens_holding_the_pointer() {
        // A tool is a mode, and a mode that outlives the moment is one the user comes back to
        // having forgotten. Nothing persists it, and the strip lists every tool there is, so
        // none of them can be reached and then not left.
        assert_eq!(RollTool::default(), RollTool::Pointer);
        assert!(RollTool::ALL.contains(&RollTool::default()));
        assert_ne!(
            RollTool::Pointer.label(),
            RollTool::Velocity.label(),
            "two buttons under one name is a strip nobody can read",
        );
    }

    #[test]
    fn the_tool_key_reaches_every_tool_and_comes_back() {
        // One binding for the lot, so the cycle has to reach all of them and return. A tool the
        // key could get into and not out of would be a mode with no way back.
        let mut tool = RollTool::default();
        let mut seen = vec![tool];
        for _ in 1..RollTool::ALL.len() {
            tool = tool.next();
            assert!(!seen.contains(&tool), "{tool:?} came round twice");
            seen.push(tool);
        }
        assert_eq!(seen.len(), RollTool::ALL.len());
        assert_eq!(tool.next(), RollTool::default(), "and then it wraps");
    }

    #[test]
    fn a_velocity_drag_goes_up_for_louder_and_never_reaches_silence() {
        let mid = 64;
        assert!(dragged_velocity(mid, px(-30.0)) > mid, "up is louder");
        assert!(dragged_velocity(mid, px(30.0)) < mid, "down is softer");
        assert_eq!(dragged_velocity(mid, px(0.0)), mid);

        // Both ends clamp, and the soft end stops at 1 rather than 0: MIDI spends 0 on "this
        // note has stopped", so a note dragged to nothing would still be drawn, still be
        // selected, still be movable, and never once be heard.
        assert_eq!(dragged_velocity(mid, px(-9999.0)), 127);
        assert_eq!(dragged_velocity(mid, px(9999.0)), MIN_VELOCITY);
    }

    #[test]
    fn a_step_is_deliberate_and_the_whole_range_is_one_movement() {
        assert_eq!(dragged_velocity(64, px(-PIXELS_PER_VELOCITY_STEP)), 65);
        assert_eq!(
            dragged_velocity(64, px(-PIXELS_PER_VELOCITY_STEP / 3.0)),
            64,
            "less than half a step is not a step — a press that wobbles must not rewrite the note",
        );

        let whole_range = 127.0 * PIXELS_PER_VELOCITY_STEP;
        assert!(
            whole_range < 240.0,
            "the range takes {whole_range} pixels, which is further than the roll is tall",
        );
        assert_eq!(dragged_velocity(MIN_VELOCITY, px(-whole_range)), 127);
    }

    #[test]
    fn a_drag_past_the_end_and_back_leaves_the_selection_as_it_was() {
        // Measured from where the notes started rather than from where they now are, which is
        // the whole reason the origins are captured when the button goes down: a chord pushed
        // into the ceiling and brought back down gets its shape returned rather than flattened.
        let chord = [40u8, 80, 120];
        let against = |dy| -> Vec<u8> {
            chord
                .iter()
                .map(|origin| dragged_velocity(*origin, dy))
                .collect()
        };
        assert_eq!(against(px(-400.0)), vec![127, 127, 127]);
        assert_eq!(against(px(0.0)), chord.to_vec());
        assert_eq!(
            against(px(-15.0)),
            vec![50, 90, 127],
            "and each note moves by the same amount until it runs out of room",
        );
    }

    #[test]
    fn what_is_stored_and_what_is_shown_are_the_same_number() {
        // The document holds a fraction and the tag reports MIDI. A round trip that lost a step
        // would make the drag feel as though it were sticking.
        for midi in 0..=127u8 {
            assert_eq!(midi_velocity(f32::from(midi) / 127.0), midi);
        }
        assert_eq!(midi_velocity(-1.0), 0);
        assert_eq!(midi_velocity(2.0), 127);
    }

    #[test]
    fn a_note_too_small_to_carry_a_bar_does_not_get_one() {
        let note = |width: f32, height: f32| Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(width), px(height)),
        };
        assert!(bar_fits(note(80.0, 14.0)));
        assert!(
            !bar_fits(note(8.0, 14.0)),
            "a 1/32 note at a normal zoom is narrower than the bar would need",
        );
        assert!(
            !bar_fits(note(80.0, 5.0)),
            "and the rows go down to five pixels, which is thinner than the bar",
        );
    }
}

#[cfg(test)]
mod curve_tests {
    use super::*;

    #[test]
    fn a_value_survives_the_trip_to_the_strip_and_back() {
        // A curve is drawn from `curve_row` and dragged through `curve_of_row`, so a value that
        // did not round-trip would make the point slide out from under the pointer holding it.
        for which in [ClipCurve::Bend, ClipCurve::MODULATION] {
            let (low, high) = which.range();
            for value in [low, low / 2.0, 0.0, high / 2.0, high] {
                let back = curve_of_row(which, curve_row(which, value));
                assert!(
                    (back - value).abs() < 0.001,
                    "{which:?} took {value} to {} and gave back {back}",
                    curve_row(which, value)
                );
            }
            // Past either end clamps rather than drawing off the strip.
            assert_eq!(curve_row(which, high * 99.0), 0.0);
            assert_eq!(curve_of_row(which, -2.0), high);
            assert_eq!(curve_of_row(which, 3.0), low);
        }
    }

    #[test]
    fn the_zero_line_is_where_each_curve_rests() {
        // The rule drawn across a strip is what makes an empty one different from a flat one, so
        // it has to be *at* the value the instrument holds when nothing is written.
        assert_eq!(
            curve_row(ClipCurve::Bend, 0.0),
            0.5,
            "a bend rests in the middle, because it goes both ways"
        );
        assert_eq!(
            curve_row(ClipCurve::MODULATION, 0.0),
            1.0,
            "and a wheel rests on the floor, because there is nothing below it"
        );
        assert_eq!(curve_row(ClipCurve::Bend, BEND_LIMIT), 0.0, "the top is up");
        assert_eq!(curve_row(ClipCurve::Bend, -BEND_LIMIT), 1.0);
        assert_eq!(curve_row(ClipCurve::MODULATION, CONTROLLER_LIMIT), 0.0);
    }

    #[test]
    fn both_ends_of_a_lane_are_written_down() {
        // The bug this fixes was not in the audio: the wheel's lane showed "127" and nothing else,
        // and a lone number over an unlabelled floor was read as the wheel's *position*. Whatever
        // the numbers say, there have to be two of them.
        for which in [ClipCurve::Bend, ClipCurve::MODULATION] {
            let (high, low) = curve_scale(which);
            assert!(!high.is_empty() && !low.is_empty(), "{which:?}");
            assert_ne!(high, low, "{which:?}");
        }

        assert_eq!(
            curve_scale(ClipCurve::MODULATION),
            ("127".to_string(), "0".to_string()),
            "the wheel is read in the controller's units, and rests at nothing"
        );
        assert_eq!(
            curve_scale(ClipCurve::Bend),
            (format!("+{BEND_LIMIT:.0}"), format!("-{BEND_LIMIT:.0}")),
            "and the bend is signed, so the reader can see zero is between them"
        );
    }

    #[test]
    fn the_wheel_never_goes_below_nothing() {
        // Half a strip drawn under a control that cannot reach it would be half a strip of
        // nothing — and a drag into it would write a negative wheel position, which is not a
        // thing.
        for row in [0.0, 0.5, 1.0, 2.0] {
            assert!(curve_of_row(ClipCurve::MODULATION, row) >= 0.0, "{row}");
        }
        assert!(curve_of_row(ClipCurve::Bend, 1.0) < 0.0, "a bend does");
    }

    #[test]
    fn a_press_takes_the_point_it_landed_on_and_nothing_else() {
        let points = vec![
            CurvePoint {
                at: Ticks(0),
                value: 0.0,
            },
            CurvePoint {
                at: Ticks(480),
                value: 2.0,
            },
            CurvePoint {
                at: Ticks(960),
                value: 0.0,
            },
        ];
        let radius = Ticks(40);
        assert_eq!(
            curve_point_at(&points, Ticks(2_000), Ticks(480), radius),
            Some(Ticks(480))
        );
        assert_eq!(
            curve_point_at(&points, Ticks(2_000), Ticks(500), radius),
            Some(Ticks(480)),
            "just inside the zone"
        );
        assert_eq!(
            curve_point_at(&points, Ticks(2_000), Ticks(600), radius),
            None,
            "the line between two points belongs to neither"
        );
        // Nearest rather than first, so two points dragged close together still resolve to the
        // one under the pointer.
        assert_eq!(
            curve_point_at(&points, Ticks(2_000), Ticks(700), Ticks(400)),
            Some(Ticks(480))
        );
        assert_eq!(curve_point_at(&[], Ticks(2_000), Ticks(0), radius), None);
        assert_eq!(
            curve_point_at(&points, Ticks(700), Ticks(960), radius),
            None,
            "a point past a shortened clip is not interactive"
        );
    }

    #[test]
    fn the_grab_zone_is_a_length_and_stays_one_however_far_the_view_has_scrolled() {
        // Every other test in this file sits at the start of the song, which is the one place a
        // length and a position are the same number — and so the one place this could not be
        // seen. The radius was read out of `x_to_tick`, which adds the scroll, so five bars in
        // the seven-pixel zone had swollen to the better part of twenty thousand ticks.
        let per_beat = 48.0;
        let at_start = TimelineView {
            pixels_per_beat: per_beat,
            scroll_ticks: Ticks::ZERO,
        };
        let five_bars_in = TimelineView {
            pixels_per_beat: per_beat,
            scroll_ticks: Ticks(TICKS_PER_QUARTER * 20),
        };
        assert_eq!(
            curve_grab_radius(&five_bars_in),
            curve_grab_radius(&at_start),
            "scrolling the view must not widen the zone",
        );
        // And it is the seven pixels it claims to be: a quarter note is drawn 48 across, so the
        // zone reaches seven forty-eighths of one either way.
        assert_eq!(
            curve_grab_radius(&five_bars_in),
            Ticks((TICKS_PER_QUARTER as f32 * CURVE_GRAB / per_beat) as i64),
        );

        // Which is what keeps a second point writable. One point at the clip's start, a press a
        // beat later: near enough to reach only if the zone has grown, and if it has, the press
        // grabs that point and drags it instead of writing a new one.
        let points = vec![CurvePoint {
            at: Ticks::ZERO,
            value: 0.0,
        }];
        let radius = curve_grab_radius(&five_bars_in);
        assert_eq!(
            curve_point_at(&points, Ticks::QUARTER, Ticks::ZERO, radius),
            Some(Ticks::ZERO),
            "a press on the point itself still takes hold of it",
        );
        assert_eq!(
            curve_point_at(&points, Ticks::QUARTER, Ticks(TICKS_PER_QUARTER), radius,),
            None,
            "a beat away is empty strip, and a press there writes a point of its own",
        );

        // Zoomed all the way out a tick is far narrower than a pixel, and the zone must not round
        // down to nothing: a zero radius is a point that can be drawn and never grabbed again.
        let far_out = TimelineView {
            pixels_per_beat: TimelineView::MIN_PIXELS_PER_BEAT,
            scroll_ticks: Ticks(TICKS_PER_QUARTER * 20),
        };
        assert!(curve_grab_radius(&far_out) >= Ticks(1));
    }

    #[test]
    fn each_strip_says_which_one_it_is() {
        // Strips of the same size stacked under the notes, and the gutter is the only thing that
        // tells them apart. Fifty-six pixels of it, so the tag is the number rather than the name.
        let language = Language::English;
        let tags: Vec<String> = [
            ClipCurve::Bend,
            ClipCurve::MODULATION,
            ClipCurve::Controller(11),
        ]
        .into_iter()
        .map(|which| curve_tag(which, language))
        .collect();
        assert_eq!(tags, vec!["Bend", "CC1", "CC11"]);

        // The menu that opens them has the room to say what they are.
        assert_eq!(
            curve_label(ClipCurve::Controller(11), language),
            "Expression"
        );
        assert_eq!(
            curve_label(ClipCurve::Controller(85), language),
            "CC 85",
            "an unnamed controller keeps its number rather than borrowing a name"
        );
        assert_eq!(curve_label(ClipCurve::Bend, language), "Bend");
    }

    #[test]
    fn the_lane_menu_offers_the_usual_controllers_and_whatever_this_clip_uses() {
        // The fixed list is the controls a keyboard has. Anything else arrives with a MIDI file,
        // and the lane it wrote on has to be reachable or its curve is audible and undrawable.
        let rows = curve_lane_choices(&[], &[ClipCurve::Controller(85), ClipCurve::Bend]);
        assert_eq!(rows[0], ClipCurve::Bend, "the bend leads");
        assert!(rows.contains(&ClipCurve::MODULATION));
        assert!(rows.contains(&ClipCurve::Controller(11)));
        assert_eq!(
            rows.last(),
            Some(&ClipCurve::Controller(85)),
            "the clip's own controller is offered, after the usual ones"
        );

        // A lane that is open and also written on is one row, not two, and the bend is never
        // listed twice however it arrives.
        let rows = curve_lane_choices(
            &[ClipCurve::Bend, ClipCurve::Controller(85)],
            &[ClipCurve::Controller(85), ClipCurve::MODULATION],
        );
        let mut seen = rows.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), rows.len(), "a row was offered twice: {rows:?}");
    }
}

/// The roll's gestures, driven through the window rather than through the handlers underneath.
///
/// A press in the grid is a sequence of questions — velocity tool, delete, note under the pointer,
/// empty grid — and the order they are asked in *is* the behaviour. The pure rules each have their
/// own test above; what these check is that a pointer at a position still reaches the right one.
#[cfg(test)]
mod window_tests {
    use gpui::TestAppContext;

    use auris_session::prelude::*;

    use crate::gestures::PointerGesture;
    use crate::harness::{
        click_at, creating, deleting, double_click, drag, drag_with, paint, press, release,
        roll_point, show_pitch, with_a_clip, with_a_singer_clip,
    };
    use crate::ui::context_menu::MenuCommand;

    /// Middle C, which is far enough from either end of the keyboard that a pitch either side of
    /// it is still a pitch.
    const MIDDLE_C: u8 = 60;

    /// One beat, the unit these tests place and move notes by.
    const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);

    /// Half a beat, which is where a press lands on a one-beat note's body rather than on the
    /// resize handle at its end.
    const HALF_BEAT: Ticks = Ticks(TICKS_PER_QUARTER / 2);

    #[gpui::test]
    fn performed_bend_lane_does_not_offer_source_edit_or_visibility_controls(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _, clip) = with_a_clip(cx);
        let original = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.panels.set_curve_lane(ClipCurve::Bend, true);
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT * 2))
                .unwrap();
            this.session
                .set_curve_point(clip, ClipCurve::Bend, Ticks::ZERO, 0.2);
            this.session
                .set_clip_transforms(
                    clip,
                    vec![NoteTransform::Pitch {
                        settings: PitchPerformance {
                            scoop: 1.0,
                            ..PitchPerformance::default()
                        },
                    }],
                )
                .unwrap();
            this.open_clip_in_editor(clip);
            this.session.midi_clip(clip).unwrap().clone()
        });
        paint(&app, cx);
        let close_selector: &'static str = Box::leak(
            gpui::ElementId::from(("curve-lane-close", super::lane_id(ClipCurve::Bend)))
                .to_string()
                .into_boxed_str(),
        );
        let source_close = cx.debug_bounds(close_selector).unwrap().center();

        crate::harness::click("score-performed", cx);
        paint(&app, cx);
        // GPUI retains old debug bounds after an element disappears. Press the position where
        // Source offered its close button and check the actual handler's effect instead.
        click_at(cx, source_close, gpui::Modifiers::none());
        app.read_with(cx, |this, _| {
            assert!(!this.source_score());
            assert!(
                this.panels.curve_lane(ClipCurve::Bend),
                "the performed lane must not close the source lane"
            );
        });
        let at = app.read_with(cx, |this, _| {
            assert!(!this.score_preview_bend().is_empty());
            this.canvas.curve(ClipCurve::Bend).get().unwrap().center()
        });
        drag(cx, at, gpui::point(at.x + gpui::px(20.0), at.y));
        crate::harness::right_press(cx, at);
        cx.simulate_mouse_up(at, gpui::MouseButton::Right, gpui::Modifiers::none());
        app.read_with(cx, |this, _| {
            assert!(this.menu.is_none(), "a read-only curve has no edit menu");
            assert!(this.drag.is_none());
            assert!(this.panels.curve_lane(ClipCurve::Bend));
            assert_eq!(this.session.midi_clip(clip).unwrap(), &original);
        });

        crate::harness::click("score-source", cx);
        paint(&app, cx);
        let close = cx.debug_bounds(close_selector).unwrap().center();
        click_at(cx, close, gpui::Modifiers::none());
        app.read_with(cx, |this, _| {
            assert!(!this.panels.curve_lane(ClipCurve::Bend));
            assert_eq!(this.session.midi_clip(clip).unwrap(), &original);
        });
    }

    /// The notes of the clip under test, in the order the document holds them.
    fn notes(app: &gpui::Entity<crate::app::AurisApp>, cx: &gpui::TestAppContext) -> Vec<Note> {
        app.read_with(cx, |this, _| {
            this.session
                .midi_clip(this.selected_clip.expect("a clip is open"))
                .expect("the clip is still there")
                .notes
                .clone()
        })
    }

    /// A window with the fixture's clip open in the roll, painted and ready to be pressed.
    fn with_the_roll_open(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<crate::app::AurisApp>,
        &mut gpui::VisualTestContext,
        ClipId,
    ) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| this.open_clip_in_editor(clip));
        paint(&app, cx);
        // The roll opens showing the top of the keyboard, and an empty clip gives
        // `center_roll_on_selection` nothing to centre on — so middle C is a couple of octaves
        // below the grid until somebody scrolls to it, which is what a hand does too.
        show_pitch(&app, cx, MIDDLE_C);
        (app, cx, clip)
    }

    /// Selects two unequal notes by sweeping the grid, leaving a third outside the rectangle.
    fn with_a_swept_phrase(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<crate::app::AurisApp>,
        &mut gpui::VisualTestContext,
        ClipId,
    ) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            for (pitch, start, length, velocity) in [
                (MIDDLE_C, BEAT, BEAT * 2, 40),
                (MIDDLE_C + 2, BEAT * 2, BEAT, 80),
                (MIDDLE_C - 2, BEAT * 4, BEAT, 100),
            ] {
                let mut note = Note::new(pitch, start, length);
                note.velocity = velocity as f32 / 127.0;
                this.session.add_note(clip, note).unwrap();
            }
        });
        paint(&app, cx);
        let from = roll_point(&app, cx, HALF_BEAT, MIDDLE_C + 3);
        let to = roll_point(&app, cx, BEAT * 3 + HALF_BEAT, MIDDLE_C - 1);
        drag(cx, from, to);
        app.read_with(cx, |this, _| assert_eq!(this.selected_notes, [0, 1].into()));
        (app, cx, clip)
    }

    #[gpui::test]
    fn edit_readouts_follow_the_grabbed_note_and_use_absolute_song_positions(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _) = with_the_roll_open(cx);
        let clip = app.update(cx, |this, _| {
            let track = this.selected_track.unwrap();
            let clip = this
                .session
                .add_midi_clip(track, "Offset", BEAT * 8, BEAT * 4)
                .unwrap();
            this.session
                .add_note(clip, Note::new(MIDDLE_C, BEAT, BEAT * 2 + Ticks(7)))
                .unwrap();
            this.open_clip_in_editor(clip);
            this.timeline.scroll_ticks = BEAT * 8;
            clip
        });
        paint(&app, cx);
        show_pitch(&app, cx, MIDDLE_C);
        let from = roll_point(&app, cx, BEAT * 9 + HALF_BEAT, MIDDLE_C);
        let to = roll_point(&app, cx, BEAT * 10 + HALF_BEAT, MIDDLE_C + 2);
        press(cx, from);
        app.read_with(cx, |this, _| assert!(this.note_edit_tag().is_none()));
        crate::harness::drag_to(cx, to);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let tag = this.note_edit_tag().unwrap();
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert_eq!(tag.start, BEAT * 10);
            assert_eq!(tag.pitch, note.pitch);
            assert_eq!(tag.length, note.length);
            assert_eq!(
                tag.lines[1],
                auris_i18n::messages::note_drag_position(this.language(), 3, 3, 0)
            );
            assert_eq!(
                tag.lines[2],
                auris_i18n::messages::note_drag_length(this.language(), 0, 2, 7)
            );
        });
        release(cx, to);
        let mut edge = roll_point(&app, cx, BEAT * 12 + Ticks(7), MIDDLE_C + 2);
        edge.x -= gpui::px(1.0);
        let resized = roll_point(&app, cx, BEAT * 13, MIDDLE_C + 2);
        press(cx, edge);
        crate::harness::drag_to(cx, resized);
        app.read_with(cx, |this, _| {
            assert_eq!(this.note_edit_tag().unwrap().length, BEAT * 3)
        });
        cx.simulate_keystrokes("escape");
        release(cx, resized);
        app.read_with(cx, |this, _| {
            assert!(this.note_edit_tag().is_none());
            assert_eq!(
                this.session.midi_clip(clip).unwrap().notes[0].length,
                BEAT * 2 + Ticks(7)
            );
        });
    }

    #[gpui::test]
    fn alt_drag_copies_only_the_selection_once_and_undoes_as_one_edit(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        let from = roll_point(&app, cx, BEAT * 2 + HALF_BEAT, MIDDLE_C + 2);
        let to = roll_point(&app, cx, BEAT * 3 + HALF_BEAT, MIDDLE_C + 6);
        cx.simulate_mouse_down(from, gpui::MouseButton::Left, deleting());
        assert_eq!(
            notes(&app, cx),
            original,
            "pressing has not deleted anything"
        );
        cx.simulate_mouse_move(to, gpui::MouseButton::Left, deleting());
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.selected_notes, [3, 4].into());
            let tag = this.note_edit_tag().unwrap();
            assert_eq!(
                tag.pitch,
                MIDDLE_C + 6,
                "follow the grabbed copy, not the first index"
            );
            assert_eq!(tag.start, BEAT * 3);
            assert_eq!(tag.length, BEAT);
            assert_eq!(
                this.auditioning.as_ref().unwrap().1,
                vec![MIDDLE_C + 4, MIDDLE_C + 6]
            );
        });
        // Releasing Alt after starting the copy must not produce another copy or move originals.
        let farther = roll_point(&app, cx, BEAT * 4 + HALF_BEAT, MIDDLE_C + 6);
        crate::harness::drag_to(cx, farther);
        release(cx, farther);
        let changed = notes(&app, cx);
        assert_eq!(changed.len(), 5);
        assert_eq!(&changed[..3], original.as_slice());
        for (copy, source) in changed[3..].iter().zip(&original[..2]) {
            assert_eq!(copy.start, source.start + BEAT * 2);
            assert_eq!(copy.pitch, source.pitch + 4);
            assert_eq!(copy.length, source.length);
            assert_eq!(copy.velocity, source.velocity);
        }
        app.update(cx, |this, _| {
            assert!(this.note_edit_tag().is_none());
            assert!(this.auditioning.is_none());
            assert_eq!(
                this.session.undo(),
                Some(auris_session::Edit::DuplicateNotes)
            );
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, original);
            this.session.redo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, changed);
        });
    }

    #[gpui::test]
    fn cancelling_an_outsider_copy_restores_the_original_selection(cx: &mut TestAppContext) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        let from = roll_point(&app, cx, BEAT * 4 + HALF_BEAT, MIDDLE_C - 2);
        let to = roll_point(&app, cx, BEAT * 5 + HALF_BEAT, MIDDLE_C);
        cx.simulate_mouse_down(from, gpui::MouseButton::Left, deleting());
        cx.simulate_mouse_move(to, gpui::MouseButton::Left, deleting());
        assert_eq!(notes(&app, cx).len(), 4);
        cx.simulate_keystrokes("escape");
        release(cx, to);
        assert_eq!(notes(&app, cx), original);
        app.read_with(cx, |this, _| {
            assert_eq!(this.selected_notes, [0, 1].into());
            assert!(this.note_edit_tag().is_none());
            assert!(this.auditioning.is_none());
        });
    }

    #[gpui::test]
    fn alt_click_waits_for_release_and_escape_cancels_pending_deletion(cx: &mut TestAppContext) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        let at = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C);
        cx.simulate_mouse_down(at, gpui::MouseButton::Left, deleting());
        cx.simulate_keystrokes("escape");
        release(cx, at);
        assert_eq!(notes(&app, cx), original);
        cx.simulate_mouse_down(at, gpui::MouseButton::Left, deleting());
        cx.simulate_mouse_move(
            gpui::point(at.x + gpui::px(1.0), at.y),
            gpui::MouseButton::Left,
            deleting(),
        );
        assert_eq!(notes(&app, cx), original);
        cx.simulate_mouse_up(at, gpui::MouseButton::Left, deleting());
        assert_eq!(notes(&app, cx).len(), 2);
        app.update(cx, |this, _| {
            assert_eq!(this.session.undo(), Some(auris_session::Edit::DeleteNotes));
        });
        assert_eq!(notes(&app, cx), original);
    }

    #[gpui::test]
    fn alt_copy_remains_available_with_a_custom_delete_gesture(cx: &mut TestAppContext) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        app.update(cx, |this, _| {
            this.pointer.set_delete(PointerGesture::DoubleClick)
        });
        let from = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C);
        click_at(cx, from, deleting());
        assert_eq!(notes(&app, cx).len(), 3);
        let to = roll_point(&app, cx, BEAT * 2 + HALF_BEAT, MIDDLE_C + 1);
        drag_with(cx, from, to, deleting());
        assert_eq!(notes(&app, cx).len(), 5);
    }

    #[gpui::test]
    fn swept_notes_resize_together_and_recover_after_the_minimum(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        let mut from = roll_point(&app, cx, BEAT * 3, MIDDLE_C);
        from.x -= gpui::px(1.0);
        let shorter = roll_point(&app, cx, BEAT, MIDDLE_C);
        let longer = roll_point(&app, cx, BEAT * 4, MIDDLE_C);
        press(cx, from);
        crate::harness::drag_to(cx, shorter);
        assert_eq!(notes(&app, cx)[1].length, Ticks(1));
        crate::harness::drag_to(cx, longer);
        release(cx, longer);
        let changed = notes(&app, cx);
        assert_eq!(changed[0].length, BEAT * 3);
        assert_eq!(changed[1].length, BEAT * 2);
        assert_eq!(changed[2], original[2]);
        for (before, after) in original.iter().zip(&changed) {
            assert_eq!(
                (before.start, before.pitch, before.velocity),
                (after.start, after.pitch, after.velocity)
            );
        }
        app.update(cx, |this, _| {
            assert_eq!(this.session.undo(), Some(auris_session::Edit::ResizeNote));
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, original);
            this.session.redo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, changed);
            assert_eq!(this.selected_notes, [0, 1].into());
        });
    }

    #[gpui::test]
    fn swept_notes_support_matching_ends_and_lengths(cx: &mut TestAppContext) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        for alt in [false, true] {
            let mut from = roll_point(&app, cx, BEAT * 3, MIDDLE_C);
            from.x -= gpui::px(1.0);
            let to = roll_point(&app, cx, BEAT * 4, MIDDLE_C);
            drag_with(
                cx,
                from,
                to,
                gpui::Modifiers {
                    shift: true,
                    alt,
                    ..gpui::Modifiers::none()
                },
            );
            let changed = notes(&app, cx);
            assert_eq!(changed.len(), 3, "Alt-Shift must not invoke Alt-delete");
            assert_eq!(changed[0].length, BEAT * 3);
            assert_eq!(changed[1].length, if alt { BEAT * 3 } else { BEAT * 2 });
            assert_eq!(changed[2].length, BEAT);
            app.update(cx, |this, _| {
                this.session.undo();
            });
            paint(&app, cx);
        }
    }

    #[gpui::test]
    fn swept_notes_share_velocity_drags_and_can_be_made_equal(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        app.update(cx, |this, _| this.tool = super::RollTool::Velocity);
        paint(&app, cx);
        let from = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C);
        let to = gpui::point(from.x, from.y - gpui::px(15.0));
        drag(cx, from, to);
        let changed = notes(&app, cx);
        assert_eq!(super::midi_velocity(changed[0].velocity), 50);
        assert_eq!(super::midi_velocity(changed[1].velocity), 90);
        assert_eq!(changed[2], original[2]);
        app.update(cx, |this, _| {
            assert_eq!(
                this.session.undo(),
                Some(auris_session::Edit::SetNoteVelocity)
            );
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, original);
            this.session.redo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, changed);
            this.session.undo();
        });
        paint(&app, cx);
        drag_with(
            cx,
            from,
            to,
            gpui::Modifiers {
                alt: true,
                shift: true,
                ..gpui::Modifiers::none()
            },
        );
        let equal = notes(&app, cx);
        assert_eq!(super::midi_velocity(equal[0].velocity), 50);
        assert_eq!(super::midi_velocity(equal[1].velocity), 50);
        assert_eq!(equal[2], original[2]);
        app.read_with(cx, |this, _| assert_eq!(this.selected_notes, [0, 1].into()));
    }

    #[gpui::test]
    fn velocity_shift_deselect_does_not_drag_the_remaining_notes(cx: &mut TestAppContext) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        app.update(cx, |this, _| this.tool = super::RollTool::Velocity);
        paint(&app, cx);
        let from = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C);
        let to = gpui::point(from.x, from.y - gpui::px(15.0));
        drag_with(
            cx,
            from,
            to,
            gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::none()
            },
        );
        assert_eq!(notes(&app, cx), original);
        app.read_with(cx, |this, _| assert_eq!(this.selected_notes, [1].into()));
    }

    #[gpui::test]
    fn swept_note_edits_cancel_and_grabbing_an_outsider_selects_only_it(cx: &mut TestAppContext) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        let original = notes(&app, cx);
        for tool in [super::RollTool::Pointer, super::RollTool::Velocity] {
            app.update(cx, |this, _| this.tool = tool);
            paint(&app, cx);
            let mut from = roll_point(&app, cx, BEAT * 3, MIDDLE_C);
            from.x -= gpui::px(1.0);
            let to = gpui::point(from.x + gpui::px(20.0), from.y - gpui::px(15.0));
            press(cx, from);
            crate::harness::drag_to(cx, to);
            assert_ne!(notes(&app, cx), original);
            cx.simulate_keystrokes("escape");
            release(cx, to);
            assert_eq!(notes(&app, cx), original);
        }
        for tool in [super::RollTool::Pointer, super::RollTool::Velocity] {
            app.update(cx, |this, _| {
                this.tool = tool;
                this.selected_notes = [0, 1].into();
            });
            paint(&app, cx);
            let mut from = roll_point(&app, cx, BEAT * 5, MIDDLE_C - 2);
            from.x -= gpui::px(1.0);
            let to = gpui::point(from.x + gpui::px(20.0), from.y - gpui::px(15.0));
            drag(cx, from, to);
            let changed = notes(&app, cx);
            assert_eq!(&changed[..2], &original[..2]);
            assert_ne!(changed[2], original[2]);
            app.update(cx, |this, _| {
                assert_eq!(this.selected_notes, [2].into());
                this.session.undo();
            });
        }
    }

    /// The create gesture writes a note and hands it straight to the resize, so placing a note and
    /// giving it a length is one movement of the hand rather than click, look, click again.
    #[gpui::test]
    fn drawing_snaps_lengths_even_while_the_create_modifier_is_held(cx: &mut TestAppContext) {
        let (app, cx, _) = with_the_roll_open(cx);
        app.update(cx, |this, _| this.session.set_grid(HALF_BEAT));
        let from = roll_point(&app, cx, BEAT, MIDDLE_C);
        let to = roll_point(&app, cx, BEAT * 3 + Ticks(HALF_BEAT.raw() / 3), MIDDLE_C);
        drag_with(cx, from, to, creating());
        assert_eq!(notes(&app, cx)[0].length, BEAT * 2);
    }

    #[gpui::test]
    fn cancelled_lengths_do_not_leak_into_the_next_note(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, BEAT, BEAT * 2))
                .unwrap();
        });
        paint(&app, cx);
        let mut edge = roll_point(&app, cx, BEAT * 3, MIDDLE_C);
        edge.x -= gpui::px(1.0);
        let end = roll_point(&app, cx, BEAT * 4, MIDDLE_C);
        press(cx, edge);
        crate::harness::drag_to(cx, end);
        cx.simulate_keystrokes("escape");
        release(cx, end);
        let at = roll_point(&app, cx, BEAT, MIDDLE_C + 4);
        click_at(cx, at, creating());
        assert_eq!(notes(&app, cx)[1].length, BEAT * 2);
    }

    #[gpui::test]
    fn length_snap_toggle_and_fine_resize_use_the_same_grid(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            this.session.set_grid(HALF_BEAT);
            this.session
                .add_note(clip, Note::new(MIDDLE_C, BEAT, BEAT))
                .unwrap();
        });
        paint(&app, cx);
        let mut edge = roll_point(&app, cx, BEAT * 2, MIDDLE_C);
        edge.x -= gpui::px(1.0);
        let end = roll_point(&app, cx, BEAT * 3 + Ticks(HALF_BEAT.raw() / 3), MIDDLE_C);
        drag(cx, edge, end);
        assert_eq!(notes(&app, cx)[0].length, BEAT * 2);
        app.update(cx, |this, _| {
            this.session.undo();
        });
        paint(&app, cx);
        drag_with(cx, edge, end, creating());
        assert!(notes(&app, cx)[0].length > BEAT * 2);
        app.update(cx, |this, _| {
            this.session.undo();
        });
        paint(&app, cx);
        crate::harness::click("roll-snap-length", cx);
        app.read_with(cx, |this, _| assert!(!this.settings.snap_note_lengths));
        drag(cx, edge, end);
        assert!(notes(&app, cx)[0].length > BEAT * 2);
    }

    #[gpui::test]
    fn clicked_notes_reuse_the_last_length_without_pointer_wobble(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, BEAT, BEAT * 2))
                .unwrap();
        });
        paint(&app, cx);
        let picked = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C);
        click_at(cx, picked, gpui::Modifiers::none());
        let at = roll_point(&app, cx, BEAT, MIDDLE_C + 4);
        drag_with(cx, at, gpui::point(at.x + gpui::px(1.0), at.y), creating());
        assert_eq!(notes(&app, cx)[1].length, BEAT * 2);

        let mut edge = roll_point(&app, cx, BEAT * 3, MIDDLE_C + 4);
        edge.x -= gpui::px(1.0);
        let end = roll_point(&app, cx, BEAT * 4, MIDDLE_C + 4);
        drag(cx, edge, end);
        let next = roll_point(&app, cx, BEAT, MIDDLE_C + 7);
        click_at(cx, next, creating());
        assert_eq!(notes(&app, cx)[2].length, BEAT * 3);
        app.update(cx, |this, cx| {
            this.run_menu_command(
                crate::ui::context_menu::MenuCommand::NewNote {
                    pitch: MIDDLE_C + 9,
                    start: BEAT,
                },
                cx,
            );
        });
        assert_eq!(notes(&app, cx)[3].length, BEAT * 3);
    }

    #[gpui::test]
    fn swept_notes_audition_the_whole_transposed_selection_and_stop_on_release(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _) = with_a_swept_phrase(cx);
        let from = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C);
        let to = roll_point(&app, cx, BEAT * 2 + HALF_BEAT, MIDDLE_C + 3);
        press(cx, from);
        crate::harness::drag_to(cx, to);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.auditioning.as_ref().unwrap().1,
                vec![MIDDLE_C + 3, MIDDLE_C + 5]
            );
        });
        release(cx, to);
        app.read_with(cx, |this, _| assert!(this.auditioning.is_none()));

        paint(&app, cx);
        press(cx, to);
        crate::harness::drag_to(cx, from);
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(this.auditioning.is_none()));
        release(cx, from);
        assert_eq!(notes(&app, cx)[0].pitch, MIDDLE_C + 3);
    }

    #[gpui::test]
    fn the_create_gesture_writes_a_note_and_stretches_it_in_one_go(cx: &mut TestAppContext) {
        let (app, cx, _) = with_the_roll_open(cx);
        let from = roll_point(&app, cx, BEAT, MIDDLE_C);
        let to = roll_point(&app, cx, BEAT * 3, MIDDLE_C);

        drag_with(cx, from, to, creating());

        let notes = notes(&app, cx);
        assert_eq!(notes.len(), 1, "one note, not one per pointer move");
        assert_eq!(notes[0].pitch, MIDDLE_C);
        assert_eq!(notes[0].start, BEAT);
        assert_eq!(notes[0].end(), BEAT * 3, "stretched to where it was let go");
    }

    /// Dragging a note sideways and upwards moves it in both axes at once, which is the gesture
    /// people actually make.
    #[gpui::test]
    fn a_note_dragged_up_and_along_changes_pitch_and_position(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT))
                .expect("the clip takes a note");
        });
        paint(&app, cx);

        // Half a beat in, so the press is on the note's body rather than on its right edge.
        let from = roll_point(&app, cx, HALF_BEAT, MIDDLE_C);
        let to = roll_point(&app, cx, BEAT + HALF_BEAT, MIDDLE_C + 2);
        drag(cx, from, to);

        let notes = notes(&app, cx);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, MIDDLE_C + 2);
        assert_eq!(notes[0].start, BEAT);
    }

    /// The same wobble guard the clips have: a press that never travelled is a selection.
    #[gpui::test]
    fn a_press_that_does_not_travel_leaves_the_note_where_it_was(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        // Off the grid, so a snap would show up in the answer.
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks(70), BEAT))
                .expect("the clip takes a note");
        });
        paint(&app, cx);

        let at = roll_point(&app, cx, Ticks(70) + HALF_BEAT, MIDDLE_C);
        press(cx, at);
        crate::harness::drag_to(cx, gpui::point(at.x + gpui::px(1.0), at.y));
        release(cx, at);

        assert_eq!(notes(&app, cx)[0].start, Ticks(70));
    }

    /// The delete gesture takes a note off, and is asked before anything else could claim the
    /// press — otherwise it would be unreachable.
    #[gpui::test]
    fn the_delete_gesture_takes_a_note_off(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT))
                .expect("the clip takes a note");
        });
        paint(&app, cx);

        let at = roll_point(&app, cx, HALF_BEAT, MIDDLE_C);
        click_at(cx, at, deleting());

        assert!(notes(&app, cx).is_empty());
    }

    /// The note and the length it was given are one gesture, so they are one step back.
    ///
    /// The transaction opens before the note is written for exactly this: without it, drawing a
    /// note would leave a step for the note and another for every pixel of the drag that gave it
    /// a length, and taking one note back would be a dozen presses of ⌘Z.
    #[gpui::test]
    fn drawing_a_note_takes_one_undo_to_take_back(cx: &mut TestAppContext) {
        let (app, cx, _) = with_the_roll_open(cx);
        let from = roll_point(&app, cx, BEAT, MIDDLE_C);
        let to = roll_point(&app, cx, BEAT * 3, MIDDLE_C);

        drag_with(cx, from, to, creating());
        assert_eq!(notes(&app, cx).len(), 1);

        cx.dispatch_action(crate::actions::Undo);

        assert!(
            notes(&app, cx).is_empty(),
            "one Undo took back the note and the length together"
        );
    }

    /// A press below MIDI 0 must not act on pitch 0: the grid is unpainted down there, and a
    /// click on nothing that wrote a note at the bottom of the keyboard would be a note nobody
    /// asked for, in a place nobody was looking.
    #[gpui::test]
    fn a_press_off_the_bottom_of_the_keyboard_writes_nothing(cx: &mut TestAppContext) {
        let (app, cx, _) = with_the_roll_open(cx);
        let below = app.read_with(cx, |this, _| {
            let origin = this.roll_origin();
            // One row past pitch 0, which `pitch_at` answers `None` for.
            gpui::point(
                origin.x + this.timeline.tick_to_x(BEAT),
                origin.y + this.pitch.pitch_to_y(0) + gpui::px(this.pitch.row_height * 1.5),
            )
        });

        click_at(cx, below, creating());

        assert!(notes(&app, cx).is_empty());
    }

    /// The singer fixture with two notes in the roll, ready to be given words.
    fn with_two_sung_notes(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<crate::app::AurisApp>,
        &mut gpui::VisualTestContext,
        ClipId,
    ) {
        let (app, cx, _, clip) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT))
                .expect("the clip takes a note");
            this.session
                .add_note(clip, Note::new(MIDDLE_C + 2, BEAT, BEAT))
                .expect("and a second");
            this.open_clip_in_editor(clip);
        });
        paint(&app, cx);
        show_pitch(&app, cx, MIDDLE_C);
        (app, cx, clip)
    }

    /// The whole re-timing flow, made as a hand makes it: grab the k|a divider inside the
    /// note, drag it right, and the k's length lands in the document as a pin — then the
    /// note's menu offers the reset, and the pin comes off.
    #[gpui::test]
    fn dragging_a_phoneme_divider_pins_its_length(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT * 2))
                .unwrap();
            this.session.set_note_lyric(clip, 0, "か").unwrap();
            this.open_clip_in_editor(clip);
        });
        paint(&app, cx);
        show_pitch(&app, cx, MIDDLE_C);

        // Where the rule put the cut, and where the hand will carry it — asked of the same
        // layout the grab and the painter read.
        let (from_tick, to_tick) = app.read_with(cx, |this, _| {
            let tempo = &this.project().tempo_map;
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert_eq!(note.phonemes, ["k", "a"]);
            let length = tempo.ticks_to_seconds(note.end()).0;
            let layout = phoneme_layout(&note.phonemes, &note.phoneme_seconds, length, None);
            (
                tempo.seconds_to_ticks(Seconds(layout[0].1)),
                tempo.seconds_to_ticks(Seconds(0.200)),
            )
        });
        let from = roll_point(&app, cx, from_tick, MIDDLE_C);
        let to = roll_point(&app, cx, to_tick, MIDDLE_C);
        drag(cx, from, to);

        app.read_with(cx, |this, _| {
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            let pinned = note.phoneme_seconds.first().copied().unwrap_or_default();
            assert!(
                (pinned - 0.200).abs() < 0.02,
                "the k now holds about 200 ms, got {pinned}"
            );
            assert!(
                this.session.midi_clip(clip).unwrap().notes[0].start == Ticks::ZERO,
                "the note itself never moved"
            );
        });

        // The gesture is one undo step.
        app.update(cx, |this, _| {
            this.session.undo();
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert!(
                note.phoneme_seconds.is_empty(),
                "one drag, one step, and it lands back at the rule"
            );
        });
    }

    /// The whole ornament flow, made as a hand makes it: the scoop's corner handle is
    /// grabbed where the geometry says it sits, carried right and down, and the note's
    /// scoop lands deeper and longer in the document — one gesture, one undo step.
    #[gpui::test]
    fn dragging_the_scoop_corner_deepens_and_lengthens_it(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT * 2))
                .unwrap();
            this.session
                .set_note_scoop(clip, 0, Some(Scoop::default()))
                .unwrap();
            this.open_clip_in_editor(clip);
        });
        paint(&app, cx);
        show_pitch(&app, cx, MIDDLE_C);

        // The corner sits at the scoop's reach, its depth under the note's centre row —
        // the same arithmetic the painter and the grab test read.
        let (from, to) = app.read_with(cx, |this, _| {
            let origin = this.roll_origin();
            let tempo = &this.project().tempo_map;
            let row = this.pitch.row_height;
            let centre = origin.y
                + gpui::px((this.pitch.top_pitch as f32 - f32::from(MIDDLE_C)) * row + row / 2.0);
            let scoop = this.session.midi_clip(clip).unwrap().notes[0]
                .scoop
                .expect("just set");
            let x_at = |seconds: f64| {
                origin.x
                    + this
                        .timeline
                        .tick_to_x(tempo.seconds_to_ticks(Seconds(seconds)))
            };
            (
                gpui::point(x_at(scoop.seconds), centre + gpui::px(scoop.depth * row)),
                gpui::point(x_at(0.3), centre + gpui::px(2.0 * row)),
            )
        });
        drag(cx, from, to);

        app.read_with(cx, |this, _| {
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            let scoop = note.scoop.expect("still worn");
            assert!(
                (scoop.seconds - 0.3).abs() < 0.02,
                "the rise now takes about 300 ms, got {}",
                scoop.seconds
            );
            assert!(
                (scoop.depth - 2.0).abs() < 0.05,
                "and starts about two semitones under, got {}",
                scoop.depth
            );
            assert_eq!(note.start, Ticks::ZERO, "the note itself never moved");
            assert_eq!(note.pitch, MIDDLE_C);
        });

        // The gesture is one undo step, landing back at the default it started from.
        app.update(cx, |this, _| {
            this.session.undo();
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert_eq!(note.scoop, Some(Scoop::default()));
        });
    }

    /// The note menu dresses and undresses a note: a toggle row adds a default ornament,
    /// and the reset takes every ornament off at once.
    #[gpui::test]
    fn the_menu_toggles_ornaments_on_a_sung_note(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_singer_clip(cx);
        app.update(cx, |this, cx| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT))
                .unwrap();
            this.open_clip_in_editor(clip);
            this.run_menu_command(
                MenuCommand::SetVibrato {
                    clip,
                    index: 0,
                    on: true,
                },
                cx,
            );
        });
        app.read_with(cx, |this, _| {
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert_eq!(note.vibrato, Some(Vibrato::default()));
        });

        app.update(cx, |this, cx| {
            this.run_menu_command(
                MenuCommand::SetScoop {
                    clip,
                    index: 0,
                    on: true,
                },
                cx,
            );
            this.run_menu_command(MenuCommand::ResetOrnaments { clip, index: 0 }, cx);
        });
        app.read_with(cx, |this, _| {
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert!(note.scoop.is_none() && note.vibrato.is_none());
        });
    }

    /// The whole lyric flow, made as a hand makes it: double-click a note, type the word,
    /// press Return — and the sheet walks to the next note so the verse can be typed through.
    #[gpui::test]
    fn a_double_click_types_a_lyric_and_return_walks_to_the_next_note(cx: &mut TestAppContext) {
        let (app, cx, _clip) = with_two_sung_notes(cx);

        let at = roll_point(&app, cx, HALF_BEAT, MIDDLE_C);
        double_click(cx, at);
        paint(&app, cx);
        assert!(
            app.read_with(cx, |this, _| this.prompt.is_some()),
            "the double click opened the lyric sheet"
        );

        cx.simulate_input("さ");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);

        let sung = notes(&app, cx);
        assert_eq!(sung[0].lyric, "さ");
        assert_eq!(sung[0].phonemes, ["s", "a"], "the phonemes landed with it");
        assert!(
            app.read_with(cx, |this, _| this.prompt.is_some()),
            "Return walked on to the second note"
        );

        cx.simulate_input("く");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);

        let sung = notes(&app, cx);
        assert_eq!(sung[1].lyric, "く");
        assert_eq!(sung[1].phonemes, ["k", "ɯ"]);
        assert!(
            app.read_with(cx, |this, _| this.prompt.is_none()),
            "the walk ends where the words do"
        );

        // The two words were two edits: each Return was one commitment.
        cx.dispatch_action(crate::actions::Undo);
        assert!(notes(&app, cx)[1].lyric.is_empty());
        assert_eq!(notes(&app, cx)[0].lyric, "さ");
    }

    /// A configured destructive gesture wins over the singer roll's ordinary double-click action.
    #[gpui::test]
    fn a_double_click_deletes_a_sung_note_when_that_is_the_configured_gesture(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _, clip) = with_a_singer_clip(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT))
                .expect("the singer clip takes a note");
            this.pointer.set_delete(PointerGesture::DoubleClick);
            this.open_clip_in_editor(clip);
        });
        paint(&app, cx);
        show_pitch(&app, cx, MIDDLE_C);

        let at = roll_point(&app, cx, HALF_BEAT, MIDDLE_C);
        double_click(cx, at);

        assert!(notes(&app, cx).is_empty(), "the configured delete won");
        assert!(
            app.read_with(cx, |this, _| this.prompt.is_none()),
            "the lyric sheet did not intercept the delete"
        );
    }

    /// On an instrument track the same gesture means what it always meant — nothing extra —
    /// because words drawn over a synth part would be an affordance about a feature it lacks.
    #[gpui::test]
    fn a_double_click_on_an_instrument_note_opens_no_sheet(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_the_roll_open(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(MIDDLE_C, Ticks::ZERO, BEAT))
                .expect("the clip takes a note");
        });
        paint(&app, cx);

        let at = roll_point(&app, cx, HALF_BEAT, MIDDLE_C);
        double_click(cx, at);

        assert!(app.read_with(cx, |this, _| this.prompt.is_none()));
    }

    /// The batch sheet lays a phrase across the selection one mora to a note, in the order the
    /// notes are sung rather than the order they were selected.
    #[gpui::test]
    fn the_write_lyrics_sheet_fills_the_selection_in_sung_order(cx: &mut TestAppContext) {
        let (app, cx, clip) = with_two_sung_notes(cx);
        app.update(cx, |this, cx| {
            this.selected_notes.clear();
            this.selected_notes.insert(1);
            this.selected_notes.insert(0);
            this.open_write_lyrics_prompt(clip);
            cx.notify();
        });
        paint(&app, cx);

        cx.simulate_input("さく");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);

        let sung = notes(&app, cx);
        assert_eq!(sung[0].lyric, "さ");
        assert_eq!(sung[1].lyric, "く");
    }
}
