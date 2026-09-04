//! Listening to a mix and reporting what a listener would measure, changing nothing.
//!
//! The read-only sibling of [`levels`](super::levels): that module renders and measures in
//! order to *move* faders, this one in order to *answer*. It is a session command rather than
//! frontend arithmetic for the usual boundary reason — a frontend may not name the DSP that
//! does the measuring — and for one more that is new: the reader who needs these numbers most
//! cannot hear at all. A language model driving the MCP frontend iterates on a piece by
//! rendering it, asking what it measured, and editing the specification against the answer;
//! this report is that loop's ears.
//!
//! Everything is measured off one render of the whole mix, sliced where the song's sections
//! sit, so the report describes a single performance rather than several. The one exception is
//! the per-track table, which needs a render per track and is therefore asked for explicitly.

use auris_core::AudioBuffer;
use auris_core::time::Ticks;
use auris_dsp::integrated_lufs;
use auris_engine::{OfflineOptions, RenderProgress};
use auris_gpu::analysis::analyze_loudness;

use crate::error::SessionError;

use super::Session;

/// What one named stretch of the song measured, inside the whole mix.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionLoudness {
    /// The section's name.
    pub label: String,
    /// Which occurrence of that name it is, counting from one — a chorus played twice is two
    /// entries, and their loudness differing is usually the point of asking.
    pub instance: usize,
    /// The 1-based bar the section starts in.
    pub start_bar: u32,
    /// The 1-based bar it ends in, inclusive.
    pub end_bar: u32,
    /// Its integrated loudness, or `None` where it made no sound.
    pub lufs: Option<f32>,
    /// Its loudest sample, in dBFS.
    pub peak_db: f32,
}

/// What one track measured playing alone.
///
/// Soloed rather than rendered dry, exactly as the balance pass measures — the buses it feeds
/// stay open, so a part's number includes the room it is sent to.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackLoudness {
    /// The track's name.
    pub name: String,
    /// Its integrated loudness alone, or `None` where it made no sound.
    pub lufs: Option<f32>,
}

/// What the whole mix measured.
#[derive(Clone, Debug, PartialEq)]
pub struct MixAnalysis {
    /// How long the render is, effect tails included.
    pub seconds: f64,
    /// The integrated loudness of the whole piece, or `None` for silence.
    pub lufs: Option<f32>,
    /// The loudest sample, in dBFS.
    pub peak_db: f32,
    /// The estimated true peak, in dBFS — what the converter downstream will actually meet.
    pub true_peak_db: f32,
    /// One entry per named section, in timeline order. Empty where the song has no structure.
    pub sections: Vec<SectionLoudness>,
    /// One entry per track that plays, when asked for. Empty otherwise.
    pub tracks: Vec<TrackLoudness>,
}

impl Session {
    /// Renders the mix and reports what it measured, moving nothing.
    ///
    /// The render is the piece as a listener meets it — every fader where it stands, the
    /// master and its effects included — unlike the balance pass, which deliberately measures
    /// upstream of the master. Asking twice therefore answers the same twice: nothing here
    /// writes to the document, the engine, or the history.
    ///
    /// `per_track` adds one render per playing track and prices the table accordingly; the
    /// section table is free, because it is the one mix render sliced where the sections sit.
    pub fn analyze(&mut self, per_track: bool) -> Result<MixAnalysis, SessionError> {
        let mix = self.render_job().render(
            &OfflineOptions::whole_project(),
            &mut RenderProgress::default(),
        )?;
        let loudness = analyze_loudness(self.gpu.as_deref(), &mix);

        let end = self.project.end_tick();
        let mut sections = Vec::new();
        for span in self.project.sections.spans_in(Ticks::ZERO, end) {
            let from = self.project.tempo_map.ticks_to_seconds(span.start).0;
            let to = self.project.tempo_map.ticks_to_seconds(span.end).0;
            let slice = slice_seconds(&mix, from, to);
            sections.push(SectionLoudness {
                label: span.label.clone(),
                instance: span.instance,
                start_bar: self.project.signatures.bar_of(span.start),
                // The last tick inside the span, so a section ending exactly on a bar line is
                // not reported as reaching into the bar it stops at.
                end_bar: self.project.signatures.bar_of(span.end - Ticks(1)),
                lufs: integrated_lufs(&slice),
                peak_db: analyze_loudness(self.gpu.as_deref(), &slice).peak_db(),
            });
        }

        let mut tracks = Vec::new();
        if per_track {
            let played: Vec<_> = self
                .project
                .tracks
                .iter()
                .filter(|track| !track.kind.is_bus())
                .map(|track| (track.id, track.name.clone()))
                .collect();
            for (id, name) in played {
                tracks.push(TrackLoudness {
                    name,
                    lufs: self.measure_alone(id)?,
                });
            }
        }

        Ok(MixAnalysis {
            seconds: mix.duration_seconds(),
            lufs: integrated_lufs(&mix),
            peak_db: loudness.peak_db(),
            true_peak_db: loudness.true_peak_db(),
            sections,
            tracks,
        })
    }
}

/// The stretch of `buffer` between two moments, as a buffer of its own.
///
/// Copied rather than borrowed because the measurements downstream want an [`AudioBuffer`],
/// and a minute of audio is small next to the render that just produced it. A window that
/// falls outside the buffer comes back empty rather than out of range.
fn slice_seconds(buffer: &AudioBuffer, from: f64, to: f64) -> AudioBuffer {
    let rate = buffer.sample_rate();
    let clamp =
        |seconds: f64| ((seconds * rate).round().max(0.0) as usize).min(buffer.frame_count());
    let (start, end) = (clamp(from), clamp(to));
    let channels = buffer
        .iter_channels()
        .map(|samples| samples[start..end.max(start)].to_vec())
        .collect();
    AudioBuffer::from_planar(channels, rate).expect("equal-length slices of one buffer")
}

#[cfg(test)]
mod tests {
    use super::super::SessionOptions;
    use super::*;

    /// Two sections, quiet then loud, on the built-in instruments — no SoundFont, so it
    /// measures the same on every machine.
    fn analyzed() -> MixAnalysis {
        let spec = auris_compose::SongSpec::parse(
            r#"
            form = ["hush", "roar"]
            ending = "none"
            seed = 4
            [section.hush]
            bars = 4
            intensity = 0.15
            [section.roar]
            bars = 4
            intensity = 0.95
            [[part]]
            name = "tune"
            role = "melody"
            [[part]]
            name = "low"
            role = "bass"
            "#,
        )
        .expect("a specification this file wrote");
        let mut session = Session::new(SessionOptions::headless().with_balance(false))
            .expect("a headless session opens");
        session
            .compose(&auris_compose::compose(&spec))
            .expect("two parts compose");
        session.analyze(true).expect("the piece renders")
    }

    #[test]
    fn the_report_hears_the_arc_the_intensity_wrote() {
        let report = analyzed();
        assert!(report.lufs.is_some(), "the piece made sound");
        assert!(
            report.peak_db <= 0.0,
            "{:.1} dBFS is over full scale",
            report.peak_db
        );
        assert!(report.seconds > 0.0);

        assert_eq!(report.sections.len(), 2, "both sections reported");
        let (hush, roar) = (&report.sections[0], &report.sections[1]);
        assert_eq!((hush.start_bar, hush.end_bar), (1, 4));
        assert_eq!((roar.start_bar, roar.end_bar), (5, 8));
        let quiet = hush.lufs.expect("the quiet section still plays");
        let loud = roar.lufs.expect("the loud section plays");
        assert!(
            loud > quiet + 1.0,
            "intensity 0.95 measured {loud:.1} LUFS against {quiet:.1} at 0.15 — the report \
             cannot hear the arc"
        );
    }

    #[test]
    fn the_track_table_names_the_parts_and_costs_nothing_unasked() {
        let report = analyzed();
        let names: Vec<&str> = report
            .tracks
            .iter()
            .map(|track| track.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["tune", "low"],
            "the parts, in track order, and no buses"
        );
        for track in &report.tracks {
            assert!(track.lufs.is_some(), "`{}` plays and measures", track.name);
        }
    }

    #[test]
    fn asking_changes_nothing() {
        let spec = auris_compose::SongSpec::parse("form = [\"verse\"]\n[section.verse]\nbars = 2")
            .expect("a two-line song");
        let mut session = Session::new(SessionOptions::headless().with_balance(false))
            .expect("a headless session opens");
        session
            .compose(&auris_compose::compose(&spec))
            .expect("the piece composes");
        let before = session.project().clone();
        session.analyze(true).expect("the piece renders");
        assert_eq!(*session.project(), before, "a report is not an edit");
    }
}
