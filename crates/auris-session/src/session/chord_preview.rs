//! A fixed keyboard audition of the exact harmony that can be adopted by the composer.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use auris_compose::{SongSpec, frame};
use auris_core::theory::chart::{Chart, ChartOrigin};
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, Parameterized, PluginState, PrepareContext, ProcessContext,
};

use super::Session;

/// Immutable work for a background thread. No document or live instrument is touched.
#[derive(Clone)]
pub struct ChordPreviewJob {
    spec: SongSpec,
    section: String,
    alternative_seed: Option<u64>,
    sample_rate: f64,
    previous: Option<Vec<String>>,
}

/// Rendered block chords and the complete section chart from which they were taken.
pub struct ChordPreview {
    source: SongSpec,
    chart: Chart,
    /// The section this candidate replaces when adopted.
    pub section: String,
    /// Number of complete bars heard, up to eight and at most thirty seconds.
    pub bars: usize,
    /// Chord names grouped by bar, in playback order.
    pub chord_names: Vec<String>,
    /// Stereo audio at the requested device rate, including the keyboard's release.
    pub buffer: Arc<AudioBuffer>,
}

impl ChordPreviewJob {
    /// Snapshots a request. An alternative seed invents a fresh chart only for this section.
    pub fn new(
        spec: SongSpec,
        section: String,
        alternative_seed: Option<u64>,
        sample_rate: f64,
    ) -> Self {
        Self {
            spec,
            section,
            alternative_seed,
            sample_rate,
            previous: None,
        }
    }

    /// Plans and renders a candidate; cancellation is checked between audio blocks.
    pub fn run(self, cancelled: &AtomicBool) -> Result<ChordPreview, String> {
        for attempt in 0..16 {
            let mut trial = self.clone();
            trial.alternative_seed = self.alternative_seed.map(|seed| seed.wrapping_add(attempt));
            let preview = trial.render(cancelled)?;
            if self.previous.as_ref() != Some(&preview.chord_names) {
                return Ok(preview);
            }
        }
        Err("no different progression was found for these settings".into())
    }

    /// Avoids immediately repeating a previously heard candidate when requesting an alternative.
    pub fn avoiding(mut self, names: Option<Vec<String>>) -> Self {
        self.previous = names;
        self
    }

    fn render(self, cancelled: &AtomicBool) -> Result<ChordPreview, String> {
        if !self.sample_rate.is_finite() || !(8_000.0..=192_000.0).contains(&self.sample_rate) {
            return Err("unsupported preview sample rate".into());
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let mut candidate = self.spec.clone();
        if !candidate.form.contains(&self.section) {
            return Err("choose a section in the song's form".into());
        }
        if let Some(seed) = self.alternative_seed {
            candidate.seed = seed;
            let chart_name = private_chart_name(&candidate, &self.section);
            candidate
                .charts
                .insert(chart_name.clone(), Chart::unwritten());
            candidate
                .sections
                .get_mut(&self.section)
                .ok_or("unknown section")?
                .chords = chart_name;
        }
        let planned = frame::plan(&candidate);
        let section = planned
            .sections
            .iter()
            .find(|s| s.name == self.section)
            .ok_or("unknown section")?;
        let bar_ticks = candidate.meter.ticks_per_bar();
        let seconds_per_tick = 60.0 / section.tempo / auris_core::time::TICKS_PER_QUARTER as f64;
        let bar_seconds = bar_ticks.raw() as f64 * seconds_per_tick;
        if !bar_seconds.is_finite() || !(0.0..=30.0).contains(&bar_seconds) || bar_seconds == 0.0 {
            return Err("a preview bar must fit within thirty seconds".into());
        }
        let bars = section
            .bars
            .min(8)
            .min((30.0 / bar_seconds).floor() as usize);
        if bars == 0 || section.events.is_empty() {
            return Err("this section has no chords".into());
        }
        let mut chart_bars = vec![Vec::new(); section.bars];
        let mut chord_names = vec![Vec::new(); bars];
        let mut scheduled = Vec::new();
        for event in &section.events {
            let bar = (event.start.raw() / bar_ticks.raw()) as usize;
            if let Some(chords) = chart_bars.get_mut(bar) {
                // Pin colour too: adopting an unqualified numeral would allow a later pass
                // to change the chord that the person just heard.
                chords.push(event.numeral.with_quality(event.chord.quality));
            }
            if bar >= bars {
                continue;
            }
            chord_names[bar].push(event.chord.name_in(event.key));
            let on =
                (event.start.raw() as f64 * seconds_per_tick * self.sample_rate).round() as usize;
            let duration = event.length.raw() as f64 * seconds_per_tick;
            let off = on
                + ((duration - 0.06_f64.min(duration * 0.1)) * self.sample_rate).round() as usize;
            let root = event.chord.root.midi(3);
            let mut pitches: Vec<_> = event
                .chord
                .quality
                .intervals()
                .iter()
                .map(|i| (root + i) as u8)
                .collect();
            if let Some(bass) = event.chord.bass {
                pitches.push(bass.midi(2) as u8);
            }
            pitches.sort_unstable();
            pitches.dedup();
            for pitch in pitches {
                scheduled.push((
                    on,
                    NoteEvent::NoteOn {
                        frame: 0,
                        pitch,
                        velocity: 0.65,
                    },
                ));
                scheduled.push((off, NoteEvent::NoteOff { frame: 0, pitch }));
            }
        }
        scheduled.sort_by_key(|(at, _)| *at);
        let frames = ((bars as f64 * bar_seconds + 0.35) * self.sample_rate).ceil() as usize;
        let mut audio = AudioBuffer::stereo(frames, self.sample_rate);
        let mut block = AudioBuffer::stereo(512, self.sample_rate);
        let mut synth = auris_synth::Fm2::new();
        let mut state = PluginState::empty();
        state.params.insert("index".into(), 1.0);
        state.params.insert("vibrato".into(), 0.0);
        synth.load_state(&state);
        synth.prepare(&PrepareContext::new(self.sample_rate, 512, 2));
        let mut next = 0;
        for start in (0..frames).step_by(512) {
            if cancelled.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let count = (frames - start).min(512);
            block.set_frame_count(count);
            block.clear();
            let mut notes = Vec::new();
            while let Some((at, event)) = scheduled.get(next) {
                if *at >= start + count {
                    break;
                }
                let mut event = *event;
                match &mut event {
                    NoteEvent::NoteOn { frame, .. } | NoteEvent::NoteOff { frame, .. } => {
                        *frame = (at - start) as u32
                    }
                    _ => unreachable!("only note events are scheduled"),
                }
                notes.push(event);
                next += 1;
            }
            synth.process(
                &notes,
                &mut block,
                &ProcessContext::realtime(
                    self.sample_rate,
                    count,
                    start as u64,
                    section.tempo,
                    true,
                ),
            );
            for channel in 0..2 {
                audio.channel_mut(channel)[start..start + count]
                    .copy_from_slice(block.channel(channel));
            }
        }
        audio.sanitize();
        let peak = audio
            .channel(0)
            .iter()
            .chain(audio.channel(1))
            .fold(0.0_f32, |peak, v| peak.max(v.abs()));
        if peak <= 1e-6 {
            return Err("the preview rendered silence".into());
        }
        // Fixed, modest peak for every candidate, independent of the document's mixer.
        for channel in 0..2 {
            for sample in audio.channel_mut(channel) {
                *sample *= 0.25 / peak;
            }
        }
        Ok(ChordPreview {
            source: self.spec,
            chart: Chart::new(chart_bars, ChartOrigin::Given),
            section: self.section,
            bars,
            chord_names: chord_names.into_iter().map(|bar| bar.join(" / ")).collect(),
            buffer: Arc::new(audio),
        })
    }
}

fn private_chart_name(spec: &SongSpec, section: &str) -> String {
    let base = format!("preview-{section}");
    let mut name = base.clone();
    let mut suffix = 2;
    while spec.charts.contains_key(&name) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    name
}

impl ChordPreview {
    /// Adopts the exact complete section chart if the request has not changed since rendering.
    /// Other sections, melody seeds and arrangement settings retain their current values.
    pub fn apply_to(&self, spec: &mut SongSpec) -> bool {
        if *spec != self.source {
            return false;
        }
        let name = private_chart_name(spec, &self.section);
        spec.charts.insert(name.clone(), self.chart.clone());
        spec.chart_order.push(name.clone());
        spec.sections
            .get_mut(&self.section)
            .expect("the matching source has this section")
            .chords = name;
        true
    }
}

impl Session {
    /// Plays a rendered audition without creating tracks or using the document's mixer.
    pub fn play_chord_preview(&mut self, preview: &ChordPreview) -> Result<(), String> {
        if self.is_recording() {
            return Err("stop recording before auditioning chords".into());
        }
        if (preview.buffer.sample_rate() - self.sample_rate()).abs() > 0.1 {
            return Err("the audio rate changed; render the preview again".into());
        }
        self.engine
            .send(auris_engine::EngineCommand::PlayPreview(Arc::clone(
                &preview.buffer,
            )))
            .map_err(|e| e.to_string())
    }

    /// Stops the independent audition without editing the document or moving its playhead.
    pub fn stop_chord_preview(&self) {
        self.send(auris_engine::EngineCommand::StopPreview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(mode: &str) -> SongSpec {
        SongSpec::parse(&format!("key = 'D {mode}'\ntempo = 240\nmood = 'tense'\nform = 'verse chorus verse'\nending = 'loop'\n[section.verse]\nbars = 4\n[section.chorus]\nbars = 4")).unwrap()
    }

    #[test]
    fn adopted_harmony_matches_the_audition_and_survives_save_and_regeneration() {
        for mode in [
            "major",
            "minor",
            "dorian",
            "lydian",
            "mixolydian",
            "phrygian",
        ] {
            let original = request(mode);
            let preview =
                ChordPreviewJob::new(original.clone(), "verse".into(), Some(283), 8_000.0)
                    .run(&AtomicBool::new(false))
                    .unwrap();
            assert_eq!(preview.bars, 4);
            assert!((preview.buffer.duration_seconds() - 4.35).abs() < 0.001);
            let peak = preview
                .buffer
                .channel(0)
                .iter()
                .fold(0.0_f32, |p, v| p.max(v.abs()));
            assert!((peak - 0.25).abs() < 1e-5);
            assert!(preview.buffer.channel(0).iter().all(|v| v.is_finite()));
            let mut adopted = original.clone();
            assert!(preview.apply_to(&mut adopted));
            assert_eq!(adopted.seed, original.seed);
            assert_eq!(adopted.sections["chorus"], original.sections["chorus"]);
            let mut saved = SongSpec::parse(&adopted.to_toml()).unwrap();
            saved.seed += 1;
            let again = frame::plan(&saved);
            let written = &again.sections[0];
            let names: Vec<_> = written
                .events
                .iter()
                .map(|e| e.chord.name_in(e.key))
                .collect();
            assert_eq!(names.join(" / "), preview.chord_names.join(" / "), "{mode}");
            assert!(
                !preview.apply_to(&mut saved),
                "a stale preview must not overwrite new settings"
            );
        }
    }

    #[test]
    fn preview_limits_length_and_rejects_empty_cancelled_or_invalid_requests() {
        let mut spec = request("dorian");
        spec.sections.get_mut("verse").unwrap().bars = 16;
        spec.tempo = 120.0;
        let preview = ChordPreviewJob::new(spec.clone(), "verse".into(), None, 8_000.0)
            .run(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(preview.bars, 8);
        assert_eq!(
            preview.chart.bar_count(),
            16,
            "adoption keeps the complete section"
        );
        assert!(
            ChordPreviewJob::new(spec.clone(), "verse".into(), None, f64::NAN)
                .run(&AtomicBool::new(false))
                .is_err()
        );
        assert!(
            ChordPreviewJob::new(spec.clone(), "verse".into(), None, 8_000.0)
                .run(&AtomicBool::new(true))
                .is_err()
        );
        assert!(
            ChordPreviewJob::new(spec.clone(), "missing".into(), None, 8_000.0)
                .run(&AtomicBool::new(false))
                .is_err()
        );
        let chart = spec.sections["verse"].chords.clone();
        spec.charts
            .insert(chart, Chart::new(Vec::new(), ChartOrigin::Given));
        assert!(
            ChordPreviewJob::new(spec, "verse".into(), None, 8_000.0)
                .run(&AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn alternatives_skip_the_candidate_that_was_just_heard() {
        let spec = request("dorian");
        let first = ChordPreviewJob::new(spec.clone(), "verse".into(), Some(283), 8_000.0)
            .run(&AtomicBool::new(false))
            .unwrap();
        let other = ChordPreviewJob::new(spec, "verse".into(), Some(283), 8_000.0)
            .avoiding(Some(first.chord_names.clone()))
            .run(&AtomicBool::new(false))
            .unwrap();
        assert_ne!(first.chord_names, other.chord_names);
    }

    #[test]
    fn audition_and_adoption_leave_the_live_document_and_history_untouched() {
        let mut session = super::super::fixtures::session();
        let before = session.project().clone();
        let spec = request("dorian");
        let preview =
            ChordPreviewJob::new(spec.clone(), "verse".into(), None, session.sample_rate())
                .run(&AtomicBool::new(false))
                .unwrap();
        session.play_chord_preview(&preview).unwrap();
        session.stop_chord_preview();
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
    }
}
