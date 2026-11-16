//! What a resonating effect is doing, and the song's harmony as such an effect hears it.
//!
//! Two things a resonator needs that a plain effect does not. One is to be *seen*: a bank of
//! sympathetic strings or a guitar's box is a body like an instrument's, and the window shows it
//! moving. [`ResonanceCapture`] publishes a fixed-size [`ResonanceFrame`] from the audio thread
//! the way [`MotionCapture`](crate::motion::MotionCapture) publishes an instrument's, through a
//! bounded seqlock that never makes the writer wait. The other is to know what key the song is
//! in, because a string tuned to the song is a string that answers it. [`HarmonyTimeline`] is the
//! document's harmony laid out in frames, built once with the graph and read with a binary search
//! on the audio thread.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use crate::harmony::Harmony;
use crate::time::{TempoMap, Ticks};

/// Most strings a frame describes.
pub const RESONANCE_STRINGS: usize = 16;
/// Spatial samples along each described string.
pub const RESONANCE_POINTS: usize = 32;
/// Lowest modes of a body described, lowest first.
pub const RESONANCE_MODES: usize = 24;
/// Points of the body's frequency response, from [`RESONANCE_LOW_HZ`] to
/// [`RESONANCE_HIGH_HZ`] spaced evenly in log frequency.
pub const RESONANCE_RESPONSE: usize = 96;
/// Lowest frequency of a frame's response curve.
pub const RESONANCE_LOW_HZ: f32 = 40.0;
/// Highest frequency of a frame's response curve.
pub const RESONANCE_HIGH_HZ: f32 = 12_000.0;

/// Points of a frame's two free curves, [`ResonanceFrame::curve`] and [`ResonanceFrame::trace`].
pub const RESONANCE_CURVE: usize = 128;
/// Kind-specific readings a frame carries in [`ResonanceFrame::values`].
pub const RESONANCE_VALUES: usize = 8;

/// Frequency of point `index` of a frame's response curve.
pub fn response_hz(index: usize) -> f32 {
    let t = index as f32 / (RESONANCE_RESPONSE - 1) as f32;
    RESONANCE_LOW_HZ * (RESONANCE_HIGH_HZ / RESONANCE_LOW_HZ).powf(t)
}

/// What kind of resonator a frame describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResonanceKind {
    /// A bank of strings tuned to pitches, each answering the input at its own harmonics.
    #[default]
    Strings,
    /// A box or a plate: a bank of modes with a frequency response.
    Body,
    /// A steel plate hung in a frame: its lowest modes, and its decay time against frequency
    /// as the curve.
    Plate,
    /// Coiled springs, each a lane of the wave travelling along it, and the springs' delay
    /// against frequency as the curve.
    Spring,
    /// A tape loop past its heads: the tape's magnetisation against the head's field as the
    /// trace, and the tape's speed over the last moments as the curve.
    Tape,
    /// A valve stage: its transfer curve, and the recent operating points as the trace.
    Tube,
    /// A rotating horn over a rotating drum: their angles and speeds in the values, and their
    /// speeds over the last moments as the curve and the trace.
    Rotary,
}

impl ResonanceKind {
    /// Every kind, in the order they are numbered when published.
    pub const ALL: [ResonanceKind; 7] = [
        ResonanceKind::Strings,
        ResonanceKind::Body,
        ResonanceKind::Plate,
        ResonanceKind::Spring,
        ResonanceKind::Tape,
        ResonanceKind::Tube,
        ResonanceKind::Rotary,
    ];
}

/// One sympathetic string, as it is moving now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResonanceString {
    /// MIDI pitch the string is tuned to.
    pub pitch: f32,
    /// How strongly it rings, as an amplitude relative to unity input.
    pub level: f32,
    /// Displacement along the string from the nut to the bridge, before display gain.
    pub points: [f32; RESONANCE_POINTS],
}

impl Default for ResonanceString {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            level: 0.0,
            points: [0.0; RESONANCE_POINTS],
        }
    }
}

/// A coherent, fixed-size observation of one resonating effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResonanceFrame {
    /// Which picture the frame is for.
    pub kind: ResonanceKind,
    /// Level of what is coming in, as a smoothed peak.
    pub input: f32,
    /// Level of what the resonator adds, as a smoothed peak.
    pub output: f32,
    /// Pitch class (0 = C) the strings are tuned around, or -1 when nothing is.
    pub root: i32,
    /// Which pitch classes the strings are tuned to, bit 0 being C.
    pub classes: u16,
    /// Strings described, up to [`RESONANCE_STRINGS`].
    pub string_count: usize,
    /// The strings, in the order they hang: a retuned string keeps its place, so after a chord
    /// change they need not run from low to high.
    pub strings: [ResonanceString; RESONANCE_STRINGS],
    /// Where each of the body's lowest modes is in its swing, signed, lowest first.
    pub modes: [f32; RESONANCE_MODES],
    /// Centre frequency of each of those modes, in hertz.
    pub mode_hz: [f32; RESONANCE_MODES],
    /// The body's magnitude response, in decibels relative to its average, at [`response_hz`].
    pub response: [f32; RESONANCE_RESPONSE],
    /// A curve in the kind's own units, as `[x, y]`; what it is depends on [`Self::kind`].
    pub curve: [[f32; 2]; RESONANCE_CURVE],
    /// Points of [`Self::curve`] in use.
    pub curve_len: usize,
    /// A second curve, usually the path the effect has just travelled.
    pub trace: [[f32; 2]; RESONANCE_CURVE],
    /// Points of [`Self::trace`] in use.
    pub trace_len: usize,
    /// Kind-specific readings, each documented by the effect that writes it.
    pub values: [f32; RESONANCE_VALUES],
}

impl Default for ResonanceFrame {
    fn default() -> Self {
        Self {
            kind: ResonanceKind::default(),
            input: 0.0,
            output: 0.0,
            root: -1,
            classes: 0,
            string_count: 0,
            strings: [ResonanceString::default(); RESONANCE_STRINGS],
            modes: [0.0; RESONANCE_MODES],
            mode_hz: [0.0; RESONANCE_MODES],
            response: [0.0; RESONANCE_RESPONSE],
            curve: [[0.0; 2]; RESONANCE_CURVE],
            curve_len: 0,
            trace: [[0.0; 2]; RESONANCE_CURVE],
            trace_len: 0,
            values: [0.0; RESONANCE_VALUES],
        }
    }
}

const STRING_WORDS: usize = 2 + RESONANCE_POINTS;
const HEADER: usize = 6;
const STRINGS_AT: usize = HEADER;
const MODES_AT: usize = STRINGS_AT + RESONANCE_STRINGS * STRING_WORDS;
const MODE_HZ_AT: usize = MODES_AT + RESONANCE_MODES;
const RESPONSE_AT: usize = MODE_HZ_AT + RESONANCE_MODES;
const CURVE_AT: usize = RESPONSE_AT + RESONANCE_RESPONSE;
const TRACE_AT: usize = CURVE_AT + 1 + 2 * RESONANCE_CURVE;
const VALUES_AT: usize = TRACE_AT + 1 + 2 * RESONANCE_CURVE;
const WORDS: usize = VALUES_AT + RESONANCE_VALUES;

fn write_curve(
    words: &mut [u32; WORDS],
    at: usize,
    curve: &[[f32; 2]; RESONANCE_CURVE],
    len: usize,
) {
    words[at] = len.min(RESONANCE_CURVE) as u32;
    for (index, [x, y]) in curve.iter().enumerate() {
        words[at + 1 + 2 * index] = x.to_bits();
        words[at + 2 + 2 * index] = y.to_bits();
    }
}

fn read_curve(words: &[u32; WORDS], at: usize, curve: &mut [[f32; 2]; RESONANCE_CURVE]) -> usize {
    for (index, point) in curve.iter_mut().enumerate() {
        *point = [
            f32::from_bits(words[at + 1 + 2 * index]),
            f32::from_bits(words[at + 2 + 2 * index]),
        ];
    }
    (words[at] as usize).min(RESONANCE_CURVE)
}

impl ResonanceFrame {
    fn to_words(self) -> [u32; WORDS] {
        let mut words = [0_u32; WORDS];
        words[0] = ResonanceKind::ALL
            .iter()
            .position(|kind| *kind == self.kind)
            .unwrap_or(0) as u32;
        words[1] = self.input.to_bits();
        words[2] = self.output.to_bits();
        words[3] = self.root as u32;
        words[4] = u32::from(self.classes);
        words[5] = self.string_count.min(RESONANCE_STRINGS) as u32;
        for (index, string) in self.strings.iter().enumerate() {
            let start = STRINGS_AT + index * STRING_WORDS;
            words[start] = string.pitch.to_bits();
            words[start + 1] = string.level.to_bits();
            for (offset, point) in string.points.iter().enumerate() {
                words[start + 2 + offset] = point.to_bits();
            }
        }
        for (index, mode) in self.modes.iter().enumerate() {
            words[MODES_AT + index] = mode.to_bits();
        }
        for (index, hz) in self.mode_hz.iter().enumerate() {
            words[MODE_HZ_AT + index] = hz.to_bits();
        }
        for (index, db) in self.response.iter().enumerate() {
            words[RESPONSE_AT + index] = db.to_bits();
        }
        write_curve(&mut words, CURVE_AT, &self.curve, self.curve_len);
        write_curve(&mut words, TRACE_AT, &self.trace, self.trace_len);
        for (index, value) in self.values.iter().enumerate() {
            words[VALUES_AT + index] = value.to_bits();
        }
        words
    }

    fn from_words(words: &[u32; WORDS]) -> Self {
        let value = |index: usize| f32::from_bits(words[index]);
        let mut frame = Self {
            kind: ResonanceKind::ALL
                .get(words[0] as usize)
                .copied()
                .unwrap_or_default(),
            input: value(1),
            output: value(2),
            root: words[3] as i32,
            classes: words[4] as u16,
            string_count: (words[5] as usize).min(RESONANCE_STRINGS),
            ..Self::default()
        };
        for (index, string) in frame.strings.iter_mut().enumerate() {
            let start = STRINGS_AT + index * STRING_WORDS;
            string.pitch = value(start);
            string.level = value(start + 1);
            for (offset, point) in string.points.iter_mut().enumerate() {
                *point = value(start + 2 + offset);
            }
        }
        for (index, mode) in frame.modes.iter_mut().enumerate() {
            *mode = value(MODES_AT + index);
        }
        for (index, hz) in frame.mode_hz.iter_mut().enumerate() {
            *hz = value(MODE_HZ_AT + index);
        }
        for (index, db) in frame.response.iter_mut().enumerate() {
            *db = value(RESPONSE_AT + index);
        }
        frame.curve_len = read_curve(words, CURVE_AT, &mut frame.curve);
        frame.trace_len = read_curve(words, TRACE_AT, &mut frame.trace);
        for (index, slot) in frame.values.iter_mut().enumerate() {
            *slot = value(VALUES_AT + index);
        }
        frame
    }
}

/// Reader handle for a resonating effect's live observation.
#[derive(Debug)]
pub struct ResonanceMonitor {
    enabled: AtomicBool,
    version: AtomicU32,
    words: [AtomicU32; WORDS],
}

impl ResonanceMonitor {
    /// Asks the writer to publish frames, or to stop.
    pub fn watch(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether anyone has asked for frames.
    pub fn is_watched(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Reads a coherent frame, or `None` before the first publication and when three attempts
    /// all overlapped one. The reader never spins waiting for the audio thread.
    pub fn read(&self) -> Option<ResonanceFrame> {
        for _ in 0..3 {
            let version = self.version.load(Ordering::SeqCst);
            // Nothing published yet: the words are zeros, not a frame.
            if version == 0 {
                return None;
            }
            if version & 1 != 0 {
                continue;
            }
            let words =
                std::array::from_fn::<_, WORDS, _>(|i| self.words[i].load(Ordering::SeqCst));
            if self.version.load(Ordering::SeqCst) != version {
                continue;
            }
            return Some(ResonanceFrame::from_words(&words));
        }
        None
    }
}

/// The single writer, owned by the audio-side effect. A clone has its own, unwatched monitor.
#[derive(Debug)]
pub struct ResonanceCapture(Arc<ResonanceMonitor>);

impl Default for ResonanceCapture {
    fn default() -> Self {
        Self(Arc::new(ResonanceMonitor {
            enabled: AtomicBool::new(false),
            version: AtomicU32::new(0),
            words: std::array::from_fn(|_| AtomicU32::new(0)),
        }))
    }
}

impl Clone for ResonanceCapture {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl ResonanceCapture {
    /// A reader handle, taken off the realtime thread.
    pub fn monitor(&self) -> Arc<ResonanceMonitor> {
        Arc::clone(&self.0)
    }

    /// Whether a frame would be read by anyone.
    pub fn is_watched(&self) -> bool {
        self.0.is_watched()
    }

    /// Publishes a frame without allocating, locking or waiting for a reader.
    pub fn publish(&mut self, frame: &ResonanceFrame) {
        let words = frame.to_words();
        self.0.version.fetch_add(1, Ordering::SeqCst);
        for (dest, value) in self.0.words.iter().zip(words) {
            dest.store(value, Ordering::SeqCst);
        }
        self.0.version.fetch_add(1, Ordering::SeqCst);
    }
}

/// The harmony in force over one stretch of the timeline, as a resonator reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HarmonySpan {
    /// First frame of the stretch.
    pub start: u64,
    /// Tonic of the key, as a pitch class from C.
    pub key_root: u8,
    /// Pitch classes of the key's scale, bit 0 being C.
    pub key_classes: u16,
    /// Root of the chord sounding — or, in a stretch with no chord written, of the last chord
    /// before it, which is what strings left ringing would still be sounding. `None` only
    /// before the first chord.
    pub chord_root: Option<u8>,
    /// Pitch classes of that chord, bit 0 being C; zero before the first chord.
    pub chord_classes: u16,
    /// Whether a chord is written over this stretch rather than held from before it.
    pub chord_written: bool,
}

/// The document's key and chords laid out in frames at one sample rate.
///
/// Built on the thread that builds the render graph and handed to every effect that asks for
/// it; the audio thread only ever looks a frame up in it, which is a binary search over a
/// slice and allocates nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HarmonyTimeline {
    spans: Vec<HarmonySpan>,
}

fn classes_mask(classes: impl IntoIterator<Item = crate::theory::pitch::PitchClass>) -> u16 {
    classes
        .into_iter()
        .fold(0_u16, |mask, class| mask | (1 << class.semitones()))
}

impl HarmonyTimeline {
    /// Lays `harmony` out in frames at `sample_rate`, through `tempo`.
    pub fn of(harmony: &Harmony, tempo: &TempoMap, sample_rate: f64) -> Self {
        let mut edges: Vec<Ticks> = vec![Ticks::ZERO];
        edges.extend(harmony.keys.points().iter().map(|point| point.tick));
        edges.extend(harmony.chords.points().iter().map(|point| point.tick));
        edges.retain(|tick| *tick >= Ticks::ZERO);
        edges.sort();
        edges.dedup();
        let mut spans: Vec<HarmonySpan> = Vec::with_capacity(edges.len());
        let mut held: (Option<u8>, u16) = (None, 0);
        for tick in edges {
            let key = harmony.key_at(tick);
            let chord = harmony.chord_at(tick);
            if let Some(chord) = chord {
                held = (
                    Some(chord.root.semitones() as u8),
                    classes_mask(chord.classes()),
                );
            }
            let span = HarmonySpan {
                start: tempo.ticks_to_samples(tick, sample_rate).raw(),
                key_root: key.tonic.semitones() as u8,
                key_classes: classes_mask(key.scale.classes(key.tonic)),
                chord_root: held.0,
                chord_classes: held.1,
                chord_written: chord.is_some(),
            };
            let same = |last: &HarmonySpan| {
                (
                    last.key_root,
                    last.key_classes,
                    last.chord_root,
                    last.chord_classes,
                ) == (
                    span.key_root,
                    span.key_classes,
                    span.chord_root,
                    span.chord_classes,
                ) && last.chord_written == span.chord_written
            };
            match spans.last_mut() {
                // Two edges that land on one frame: the later one is what is in force.
                Some(last) if last.start == span.start => *last = span,
                Some(last) if same(last) => {}
                _ => spans.push(span),
            }
        }
        if let Some(first) = spans.first_mut() {
            first.start = 0;
        }
        Self { spans }
    }

    /// A timeline holding one key and no chords, for a resonator with no document behind it.
    pub fn constant(key_root: u8, key_classes: u16) -> Self {
        Self {
            spans: vec![HarmonySpan {
                start: 0,
                key_root: key_root % 12,
                key_classes,
                chord_root: None,
                chord_classes: 0,
                chord_written: false,
            }],
        }
    }

    /// The stretches, in order.
    pub fn spans(&self) -> &[HarmonySpan] {
        &self.spans
    }

    /// The harmony in force at `frame`, if the timeline has any.
    pub fn at(&self, frame: u64) -> Option<&HarmonySpan> {
        let after = self.spans.partition_point(|span| span.start <= frame);
        self.spans.get(after.checked_sub(1)?)
    }

    /// The first frame after `frame` at which the harmony changes.
    pub fn next_change(&self, frame: u64) -> Option<u64> {
        let after = self.spans.partition_point(|span| span.start <= frame);
        self.spans.get(after).map(|span| span.start)
    }
}

/// Shared handle to a harmony timeline.
pub type SharedHarmony = Arc<HarmonyTimeline>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{key::Key, numeral::Numeral};

    #[test]
    fn a_frame_round_trips_through_the_monitor() {
        let mut writer = ResonanceCapture::default();
        let monitor = writer.monitor();
        assert_eq!(monitor.read(), None, "nothing has been published");
        assert!(!writer.is_watched());
        monitor.watch(true);
        assert!(writer.is_watched());
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Body,
            input: 0.5,
            output: 0.25,
            root: 7,
            classes: 0b1000_1001_0001,
            string_count: 3,
            ..Default::default()
        };
        frame.strings[2].pitch = 55.0;
        frame.strings[2].points[RESONANCE_POINTS - 1] = -0.2;
        frame.modes[RESONANCE_MODES - 1] = 0.01;
        frame.mode_hz[0] = 102.0;
        frame.response[RESONANCE_RESPONSE - 1] = -12.0;
        writer.publish(&frame);
        assert_eq!(monitor.read(), Some(frame));
        for kind in ResonanceKind::ALL {
            frame.kind = kind;
            frame.curve[RESONANCE_CURVE - 1] = [3.0, -4.0];
            frame.curve_len = 9;
            frame.trace[0] = [0.5, 0.25];
            frame.trace_len = RESONANCE_CURVE;
            frame.values[RESONANCE_VALUES - 1] = 1.5;
            writer.publish(&frame);
            assert_eq!(monitor.read(), Some(frame), "{kind:?}");
        }
        assert!(!Arc::ptr_eq(&monitor, &writer.clone().monitor()));
    }

    #[test]
    fn the_response_axis_runs_between_its_ends() {
        assert!((response_hz(0) - RESONANCE_LOW_HZ).abs() < 1.0e-3);
        assert!((response_hz(RESONANCE_RESPONSE - 1) - RESONANCE_HIGH_HZ).abs() < 0.5);
    }

    #[test]
    fn a_timeline_follows_the_chords_and_holds_the_key_between_them() {
        let mut harmony = Harmony::in_key(Key::parse("C major").unwrap());
        let bar = Ticks::from_beats(4.0);
        harmony
            .chords
            .set_point(Ticks::ZERO, Some(Numeral::parse("I").unwrap()));
        harmony
            .chords
            .set_point(bar, Some(Numeral::parse("V").unwrap()));
        harmony.chords.set_point(bar + bar, None);
        let tempo = TempoMap::constant(120.0);
        let timeline = HarmonyTimeline::of(&harmony, &tempo, 48_000.0);
        // One bar of 4/4 at 120 BPM is two seconds.
        let first = timeline.at(0).unwrap();
        assert_eq!(first.chord_root, Some(0));
        assert_eq!(first.chord_classes, 0b0000_1001_0001);
        assert_eq!(first.key_classes, 0b1010_1011_0101);
        let second = timeline.at(96_000).unwrap();
        assert_eq!(second.chord_root, Some(7));
        assert_eq!(timeline.at(95_999).unwrap().chord_root, Some(0));
        assert_eq!(timeline.next_change(0), Some(96_000));
        // Past the last chord, the last chord is what is still ringing.
        let after = timeline.at(500_000).unwrap();
        assert_eq!(after.chord_root, Some(7));
        assert!(!after.chord_written);
        assert!(second.chord_written);
        assert_eq!(after.key_root, 0);
        // Before the first chord there is only the key.
        let mut late = Harmony::in_key(Key::parse("A minor").unwrap());
        late.chords
            .set_point(bar, Some(Numeral::parse("i").unwrap()));
        let timeline = HarmonyTimeline::of(&late, &tempo, 48_000.0);
        assert_eq!(timeline.at(0).unwrap().chord_root, None);
        assert_eq!(timeline.at(0).unwrap().key_root, 9);
    }

    #[test]
    fn a_constant_timeline_answers_everywhere() {
        let timeline = HarmonyTimeline::constant(14, 0b101);
        assert_eq!(timeline.at(u64::MAX).unwrap().key_root, 2);
        assert_eq!(timeline.next_change(0), None);
        assert!(HarmonyTimeline::default().at(0).is_none());
    }
}
