//! Independent song drafts whose audition and adoption share one exact document.

use super::{SaveReport, Session, SessionOptions};
use crate::{Edit, SessionError};
use auris_compose::{SongSpec, compose, preset};
use auris_core::{AssetPath, AudioBuffer, Project};
use auris_engine::{OfflineOptions, RenderProgress};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A bounded musical direction proposed by the language model.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SongIdea {
    /// Short display title.
    pub title: String,
    /// Audible differences, in the user's language.
    pub description: String,
    /// Built-in whole-song preset name.
    pub preset: String,
    /// Quarter-note tempo, 50..180 BPM.
    pub tempo: f64,
    /// Tonal centre, such as C major or A minor.
    pub key: String,
    /// Pitched accompaniment density, 0.2..0.85.
    pub density: f32,
    /// Lead instrument: piano, guitar, flute, strings or synth.
    pub lead: String,
}

impl SongIdea {
    /// Validate all model-controlled fields before any composition or allocation.
    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty()
            || self.title.chars().count() > 80
            || self.description.trim().is_empty()
            || self.description.chars().count() > 400
            || preset(&self.preset).is_none()
            || !self.tempo.is_finite()
            || !(50.0..=180.0).contains(&self.tempo)
            || !self.density.is_finite()
            || !(0.2..=0.85).contains(&self.density)
            || auris_core::theory::key::Key::parse(&self.key).is_none()
            || !matches!(
                self.lead.as_str(),
                "piano" | "guitar" | "flute" | "strings" | "synth"
            )
        {
            return Err("Invalid song direction returned by the model".into());
        }
        Ok(())
    }

    fn specification(&self, seconds: u32, seed: u64) -> Result<SongSpec, String> {
        self.validate()?;
        if !(30..=300).contains(&seconds) {
            return Err("Choose a duration between 30 and 300 seconds".into());
        }
        let mut spec = preset(&self.preset).ok_or("Unknown preset")?.spec();
        spec.title = self.title.clone();
        spec.tempo = self.tempo;
        spec.key = auris_core::theory::key::Key::parse(&self.key).ok_or("Invalid key")?;
        spec.seed = seed;
        spec.singer = None;
        spec.singer_speaker = None;
        let bars = ((f64::from(seconds) * self.tempo
            / 60.0
            / spec.meter.ticks_per_bar().as_beats()
            / spec.form.len().max(1) as f64)
            .round() as usize)
            .clamp(2, 64);
        for section in spec.sections.values_mut() {
            section.bars = bars;
            section.lyrics.clear();
        }
        for part in &mut spec.parts {
            part.source = None;
            part.program = None;
            if part.name == "melody" || part.name == "lead" {
                part.instrument = match self.lead.as_str() {
                    "piano" => "auris.physical.piano",
                    "guitar" => "auris.physical.guitar",
                    "flute" => "auris.physical.tin_whistle",
                    "strings" => "auris.physical.violin",
                    _ => "auris.synth.fm2",
                }
                .into();
                part.params.clear();
            }
            part.density = Some(self.density);
        }
        Ok(spec)
    }
}

/// A selected voice and speaker, independent of any live project.
#[derive(Clone, Debug, PartialEq)]
pub struct SongSinger {
    /// Display name for the singer's editable track.
    pub name: String,
    /// Installed voice configuration file.
    pub path: std::path::PathBuf,
    /// Exact speaker name, or the voice's default.
    pub speaker: Option<String>,
}

impl SongSinger {
    fn track_name(&self) -> String {
        self.speaker
            .as_ref()
            .filter(|speaker| *speaker != &self.name)
            .map_or_else(
                || self.name.clone(),
                |speaker| format!("{} — {speaker}", self.name),
            )
    }
}

/// How multiple voices share the score.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SongSinging {
    /// Alternate lyrical sections, with all voices joining the last section.
    #[default]
    Auto,
    /// Alternate lyrical sections.
    Alternate,
    /// Sing together, with additional voices on diatonic harmony parts.
    Harmony,
}

/// An exact instrument choice for an arrangement role.
#[derive(Clone, Debug)]
pub struct SongSourceChoice {
    /// Role name, or `drums` for the entire kit.
    pub role: String,
    /// Built-in instrument identifier when no external source is selected.
    pub instrument: String,
    /// Exact library source, if selected.
    pub source: Option<auris_compose::spec::PartSource>,
}

/// Immutable controls captured before planning or generating a candidate batch.
#[derive(Clone, Debug, Default)]
pub struct SongRequest {
    /// Approximate seconds; absence lets the preset and lyric capacity decide.
    pub seconds: Option<u32>,
    /// Authored lyrics; line breaks separate phrases and blank lines separate sections.
    pub lyrics: String,
    /// Selected singers. Empty requests an instrumental song.
    pub singers: Vec<SongSinger>,
    /// Vocal arrangement policy.
    pub singing: SongSinging,
    /// Explicit part choices; unspecified roles retain their arrangement's instruments.
    pub sources: Vec<SongSourceChoice>,
    /// User-configured trusted plugin search paths.
    pub plugin_paths: Vec<std::path::PathBuf>,
    /// Japanese pronunciation dictionary used by the originating session, if available.
    pub japanese_dictionary: Option<std::path::PathBuf>,
}

impl SongRequest {
    /// Prepare an isolated, note-free document with the user's exact sound and voice choices.
    /// The agent supplies the form, chords, notes and mix; no composition writer runs here.
    pub fn authoring_session(&self, idea: &SongIdea) -> Result<Session, String> {
        self.validate()?;
        idea.validate()?;
        let mut session = Session::new(
            SessionOptions::headless()
                .with_balance(false)
                .with_plugin_paths(self.plugin_paths.clone()),
        )
        .map_err(|e| e.to_string())?;
        let mut project = Project::new(&idea.title, session.project.sample_rate);
        // The agent writes the notes and the mix here, but a master it can lean on is the
        // document's, not the agent's to remember: without one, normalising stops at the first
        // drum hit that reaches the ceiling.
        super::levels::install_master_limiter(
            &session.registry,
            &mut project,
            super::levels::NEW_DOCUMENT_CEILING_DB,
        );
        let mut hosted = Vec::new();
        for choice in &self.sources {
            let source = session
                .prepare_composed_source(choice.source.as_ref())
                .map_err(|e| e.to_string())?;
            let instrument = source.as_ref().map_or_else(
                || choice.instrument.clone(),
                |source| source.instrument_id(),
            );
            if source.is_none() && !session.registry.has_instrument(&instrument) {
                return Err(format!("Unavailable instrument: {instrument}"));
            }
            let track = if choice.role == "drums" {
                project.add_drum_track(&choice.role, &instrument)
            } else {
                project.add_instrument_track(&choice.role, &instrument)
            };
            if let Some(source) = source
                && let Some(source) = session.install_composed_source(&mut project, track, source)
            {
                hosted.push((track, source));
            }
        }
        for (track, source) in hosted {
            session.install_composed_hosted_source(track, source);
        }
        session.replace_project(project);
        session.install_library_fonts();
        let drums: Vec<_> = session
            .project
            .tracks
            .iter()
            .filter(|track| track.kind.is_drum())
            .map(|track| track.id)
            .collect();
        for track in drums {
            if let Some(map) = session.suggested_drum_map(track) {
                session
                    .set_drum_map(track, map)
                    .map_err(|e| e.to_string())?;
            }
        }
        session.set_tempo_at(auris_core::Ticks::ZERO, idea.tempo);
        session.set_key(
            auris_core::Ticks::ZERO,
            auris_core::theory::key::Key::parse(&idea.key).ok_or("Invalid key")?,
        );
        if !self.singers.is_empty()
            && let Some(dictionary) = &self.japanese_dictionary
        {
            session
                .set_japanese_dictionary(Some(dictionary))
                .map_err(|e| e.to_string())?;
        }
        for singer in &self.singers {
            let track = session.add_singer_track(singer.track_name());
            session
                .set_singer_voice(track, Some(&singer.path))
                .map_err(|e| e.to_string())?;
            session
                .set_singer_speaker(track, singer.speaker.as_deref())
                .map_err(|e| e.to_string())?;
        }
        Ok(session)
    }

    /// Reject inconsistent controls before starting any model or render work.
    pub fn validate(&self) -> Result<(), String> {
        if self.seconds.is_some_and(|s| !(30..=300).contains(&s)) {
            return Err("Choose a duration between 30 and 300 seconds".into());
        }
        if !self.singers.is_empty() && self.lyrics.trim().is_empty() {
            return Err("Enter or generate lyrics before creating a vocal song".into());
        }
        if self.lyrics.chars().count() > 8000 {
            return Err("Lyrics must contain at most 8000 characters".into());
        }
        for singer in &self.singers {
            if !singer.path.is_file() {
                return Err(format!("Voice is unavailable: {}", singer.name));
            }
        }
        Ok(())
    }

    fn specification(
        &self,
        idea: &SongIdea,
        seed: u64,
        session: &Session,
    ) -> Result<SongSpec, String> {
        self.validate()?;
        let mut spec = idea.specification(self.seconds.unwrap_or(120), seed)?;
        if self.seconds.is_none() {
            let original = preset(&idea.preset).ok_or("Unknown preset")?.spec();
            for (name, section) in &mut spec.sections {
                section.bars = original.sections[name].bars;
            }
        }
        for choice in &self.sources {
            let mut matched = false;
            for part in &mut spec.parts {
                let role = part.role.name();
                if role == choice.role
                    || (choice.role == "drums"
                        && matches!(role, "kick" | "snare" | "hat" | "crash"))
                {
                    part.source = choice.source.clone();
                    part.instrument.clone_from(&choice.instrument);
                    part.params.clear();
                    part.program = None;
                    matched = true;
                }
            }
            if !matched {
                let role = auris_compose::spec::Role::parse(&choice.role)
                    .ok_or_else(|| format!("The arrangement has no {} part", choice.role))?;
                let mut part = auris_compose::spec::PartSpec::of_role(choice.role.clone(), role);
                part.instrument.clone_from(&choice.instrument);
                part.source = choice.source.clone();
                spec.parts.push(part);
                for section in spec.sections.values_mut() {
                    if !section.parts.is_empty() {
                        section.parts.push(choice.role.clone());
                    }
                }
            }
        }
        if self.singers.is_empty() {
            return Ok(spec);
        }
        // Give every occurrence its own lyric identity; repeated choruses must not
        // accidentally repeat or discard an authored verse.
        let old = spec.sections.clone();
        spec.sections.clear();
        for (index, name) in spec.form.iter_mut().enumerate() {
            let mut section = old.get(name).ok_or("Unknown song section")?.clone();
            *name = format!("section{}", index + 1);
            section.name.clone_from(name);
            section.melody_from = None;
            spec.sections.insert(name.clone(), section);
        }
        let normalized = self.lyrics.replace("\r\n", "\n");
        let stanzas: Vec<_> = normalized
            .split("\n\n")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let phrases: Vec<_> = normalized
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let sections: Vec<String> = if stanzas.len() > 1 {
            let count = stanzas.len().min(spec.form.len());
            (0..count)
                .map(|index| {
                    stanzas[index * stanzas.len() / count..(index + 1) * stanzas.len() / count]
                        .join("\n\n")
                })
                .collect()
        } else {
            let count = spec.form.len().min(phrases.len());
            (0..count)
                .map(|index| {
                    phrases[index * phrases.len() / count..(index + 1) * phrases.len() / count]
                        .join("\n")
                })
                .collect()
        };
        let slots = sections.len();
        if slots < self.singers.len() && self.singing != SongSinging::Harmony {
            return Err("Add more lyric phrases for alternating singers, or choose harmony".into());
        }
        for (index, lyrics) in sections.into_iter().enumerate() {
            let section = spec
                .sections
                .get_mut(&spec.form[index])
                .ok_or("Unknown section")?;
            section.lyrics = lyrics;
            if self.seconds.is_none() {
                let measured = session.measure_lyrics(&section.lyrics, spec.meter);
                while measured.fits_in_bars(spec.meter, section.bars) != Some(true)
                    && section.bars < 64
                {
                    section.bars += 1;
                }
            }
        }
        if spec.total_bars() as f64 * spec.meter.ticks_per_bar().as_beats() * 60.0 / spec.tempo
            > 330.0
        {
            return Err("The lyrics need more than five minutes; shorten them or choose a shorter arrangement".into());
        }
        session
            .validate_song_lyrics(&spec)
            .map_err(|e| e.to_string())?;
        Ok(spec)
    }
}

/// Finished editable document and its full rendered audition, including retained vocal assets.
pub struct SongCandidate {
    /// Original musical direction.
    pub idea: SongIdea,
    project: Project,
    /// Original render; prepare it for the current output rate before auditioning.
    pub audio: Arc<AudioBuffer>,
    /// Waveform and score sections captured for this exact audition.
    pub timeline: Arc<crate::AuditionTimeline>,
    _assets: Option<tempfile::TempDir>,
}

impl SongCandidate {
    /// Authored lyrics in song order, preserving section breaks and repeated sections.
    /// Manually authored drafts without section lyrics use their singer clips' note text.
    pub fn lyrics(&self) -> String {
        if let Some(spec) = self
            .project
            .song_spec
            .as_deref()
            .and_then(|text| SongSpec::parse(text).ok())
        {
            let lyrics = spec
                .form
                .iter()
                .filter_map(|name| spec.sections.get(name))
                .map(|section| section.lyrics.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if !lyrics.is_empty() {
                return lyrics;
            }
        }
        let mut phrases = std::collections::BTreeSet::new();
        for singer in self
            .project
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_singer())
        {
            for clip in singer.clips.iter().filter(|clip| !clip.muted) {
                let mut notes: Vec<_> = clip.notes.iter().enumerate().collect();
                notes.sort_by_key(|(index, note)| (note.start, *index));
                let text: String = notes
                    .iter()
                    .map(|(_, note)| note.lyric.as_str())
                    .filter(|text| *text != super::LYRIC_CONTINUATION)
                    .collect();
                if !text.trim().is_empty() {
                    phrases.insert((clip.start, text.trim().to_owned()));
                }
            }
        }
        phrases
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Stable musical addresses for planning a scoped continuation.
    pub fn workshop_outline(&self) -> serde_json::Value {
        serde_json::json!({"tracks": self.project.tracks.iter().map(|t| serde_json::json!({
            "track":t.id.0, "name":t.name, "clips": t.kind.note_clips().map(|clips| clips.iter().map(|c| serde_json::json!({"clip":c.id.0,"notes":c.notes.len()})).collect::<Vec<_>>())
        })).collect::<Vec<_>>()})
    }
    /// Restore an exact persisted workshop candidate and its audition.
    pub fn load_workshop(folder: &std::path::Path, idea: SongIdea) -> Result<Self, String> {
        let song = folder.join("song");
        let mut project =
            auris_io::load_project(&song.join("song.auris")).map_err(|e| e.to_string())?;
        let resolve = |asset: &mut AssetPath| -> Result<(), String> {
            *asset = AssetPath::external(
                asset
                    .resolve(Some(&song))
                    .ok_or("Missing candidate asset")?,
            );
            Ok(())
        };
        for source in project.audio_sources.values_mut() {
            resolve(&mut source.path)?;
        }
        for font in project.soundfonts.values_mut() {
            resolve(&mut font.path)?;
        }
        for track in &mut project.tracks {
            if let Some(instrument) = track.kind.as_instrument_mut()
                && let Some(file) = &mut instrument.file
            {
                resolve(file)?;
            }
            if let Some(singer) = track.kind.as_singer_mut()
                && let Some(voice) = &mut singer.voice
            {
                resolve(&mut voice.path)?;
            }
        }
        let audio = auris_io::import_audio_file(&folder.join("audition.wav"), 22_050.0)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            idea,
            timeline: Arc::new(crate::AuditionTimeline::from_project(&project, &audio)),
            project,
            audio: Arc::new(audio),
            _assets: None,
        })
    }
    /// Track names in the retained arrangement, including each selected singer.
    pub fn part_names(&self) -> Vec<String> {
        self.project
            .tracks
            .iter()
            .filter(|track| track.kind.note_clips().is_some())
            .map(|track| track.name.clone())
            .collect()
    }

    /// Writes this draft as a project of its own, leaving any open document alone.
    ///
    /// Adopting one draft discards the others, so this is how a draft that was not chosen is
    /// kept. `chosen` lands in a folder of its own exactly as [`Session::save_as`] places it, and
    /// the sung audio, which until now lives only in a temporary directory, is copied into that
    /// folder's `Audio/`. An existing document is refused rather than replaced: keeping a draft
    /// must never cost another project.
    pub fn save_as(&self, chosen: &std::path::Path) -> Result<SaveReport, SessionError> {
        let document = auris_io::document_in_folder(chosen);
        let folder = auris_io::project_folder(&document)
            .ok_or(SessionError::NoPath)?
            .to_path_buf();
        std::fs::create_dir_all(&folder)
            .map_err(|error| auris_io::IoError::from_fs(&folder, error))?;
        // Share the lock with ordinary saves, including the destination check and publication.
        let _lock = super::files::project_write_lock(&document)?;
        if document.exists() {
            return Err(SessionError::WouldReplace(document));
        }
        let audio = folder.join(auris_io::AUDIO_DIR);
        let mut project = self.project.clone();
        for source in project.audio_sources.values_mut() {
            let from = source.path.resolve(None).ok_or(SessionError::NoPath)?;
            let name = auris_io::copy_into(&from, &audio)?;
            source.path = AssetPath::inside(std::path::Path::new(auris_io::AUDIO_DIR).join(name));
            source.byte_size = auris_io::byte_size(&from);
        }
        // Also refuse a destination created by a writer that does not use Auris's lock.
        auris_io::stage_project(&document, &mut project)?
            .publish_noclobber()
            .map_err(|error| match error {
                auris_io::IoError::ExportDestinationExists(path) => {
                    SessionError::WouldReplace(path)
                }
                error => error.into(),
            })?;
        Ok(SaveReport {
            document,
            uncollected: Vec::new(),
        })
    }

    /// Compose and render on a worker without opening any realtime device.
    pub fn generate(
        idea: SongIdea,
        brief: &str,
        seconds: u32,
        seed: u64,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        Self::generate_with(
            idea,
            brief,
            &SongRequest {
                seconds: Some(seconds),
                ..Default::default()
            },
            seed,
            cancel,
            &mut |_| {},
        )
    }

    /// Generate a complete candidate on its own session thread, including selected voices.
    pub fn generate_with(
        idea: SongIdea,
        brief: &str,
        request: &SongRequest,
        seed: u64,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Self, String> {
        let mut session = Session::new(
            SessionOptions::headless()
                .with_balance(false)
                .with_plugin_paths(request.plugin_paths.clone()),
        )
        .map_err(|e| e.to_string())?;
        if !request.singers.is_empty()
            && let Some(dictionary) = &request.japanese_dictionary
        {
            session
                .set_japanese_dictionary(Some(dictionary))
                .map_err(|e| e.to_string())?;
        }
        let spec = request.specification(&idea, seed, &session)?;
        progress("compose");
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        let composition = compose(&spec);
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        let report = session
            .compose_without_balance(&composition)
            .map_err(|e| e.to_string())?;
        if !report.substituted.is_empty() {
            return Err(format!(
                "Unavailable built-in instruments: {:?}",
                report.substituted
            ));
        }
        if !report.unsung.is_empty() {
            return Err(format!("Could not fit lyrics: {:?}", report.unsung));
        }
        if !request.singers.is_empty() {
            session.arrange_candidate_singers(request, &spec)?;
            let tracks: Vec<_> = session
                .project
                .tracks
                .iter()
                .filter(|t| t.kind.as_singer().is_some())
                .map(|t| t.id)
                .collect();
            for track in tracks {
                if cancel.load(Ordering::Acquire) {
                    return Err("Cancelled".into());
                }
                progress("sing");
                let plan = session
                    .sing_plan(track, Some(seed))
                    .map_err(|e| e.to_string())?;
                let voice = session
                    .voice_model_at(&plan.voice)
                    .map_err(|e| e.to_string())?;
                let render = voice
                    .lock()
                    .map_err(|_| "Voice worker stopped")?
                    .sing_render_with(
                        &plan.frames,
                        &plan.score,
                        plan.speaker,
                        plan.seed,
                        |_, _| !cancel.load(Ordering::Acquire),
                    )
                    .map_err(|e| e.to_string())?;
                let landed = session
                    .begin_singer_landing(&plan)
                    .map_err(|e| e.to_string())?
                    .run(render, cancel)
                    .map_err(|e| e.to_string())?
                    .ok_or("Cancelled")?;
                session
                    .continue_singer_landing(landed)
                    .map_err(|e| e.to_string())?
                    .ok_or("Stale vocal render")?;
            }
        }
        Self::from_authored_session(idea, brief, &mut session, cancel, progress)
    }

    /// Retain and render the exact authored document for audition and later adoption.
    /// Does not generate notes, substitute instruments or rebalance the mix.
    pub fn from_authored_session(
        idea: SongIdea,
        brief: &str,
        session: &mut Session,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Self, String> {
        session.collect_hosted_state().map_err(|e| e.to_string())?;
        let mut project = session.project().clone();
        project.creation_brief = Some(format!(
            "{}\nChosen direction: {} — {}",
            brief.chars().take(2400).collect::<String>(),
            idea.title,
            idea.description
        ));
        progress("render");
        let audio = session
            .render_job_at(Some(22_050.0))
            .render(
                &OfflineOptions {
                    sample_rate: Some(22_050.0),
                    ..Default::default()
                },
                &mut RenderProgress::default().cancelled_by(cancel),
            )
            .map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        if audio.peak() <= 1e-8 {
            return Err("The candidate is silent".into());
        }
        let assets = if project.audio_sources.is_empty() {
            None
        } else {
            let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
            for source in project.audio_sources.values_mut() {
                let path = source
                    .path
                    .resolve(session.project_folder())
                    .ok_or("Missing vocal asset")?;
                let name = auris_io::copy_into(&path, dir.path()).map_err(|e| e.to_string())?;
                source.path = AssetPath::external(dir.path().join(name));
            }
            Some(dir)
        };
        Ok(Self {
            idea,
            timeline: Arc::new(crate::AuditionTimeline::from_project(&project, &audio)),
            project,
            audio: Arc::new(audio),
            _assets: assets,
        })
    }
}

impl Session {
    /// Save an isolated workshop document and its exact audition before publication.
    pub fn finish_workshop_candidate(
        &mut self,
        folder: &std::path::Path,
        idea: SongIdea,
        cancel: &AtomicBool,
    ) -> Result<SongCandidate, String> {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        if self.project.duration_seconds() > 330.0 {
            return Err("A workshop candidate may be at most five minutes plus its ending".into());
        }
        for track in &self.project.tracks {
            if track.kind.is_singer()
                && self
                    .singer_take_state(track.id)
                    .map_err(|e| e.to_string())?
                    != super::SingerTakeState::Current
            {
                return Err(format!(
                    "Singer {} has no current synthesized take",
                    track.name
                ));
            }
        }
        self.project.creation_brief = Some(format!("{}\n{}", idea.title, idea.description));
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        // This is the host-owned candidate directory. A failed audition may have left
        // an earlier score here; recovery replaces that partial result from its checkpoint.
        let report = self
            .save_as_replacing(&folder.join("song.auris"))
            .map_err(|e| e.to_string())?;
        if !report.uncollected.is_empty() {
            return Err("Candidate assets could not be collected".into());
        }
        let audio = self
            .render_job_at(Some(22_050.0))
            .render(
                &OfflineOptions {
                    sample_rate: Some(22_050.0),
                    ..Default::default()
                },
                &mut RenderProgress::default().cancelled_by(cancel),
            )
            .map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        if audio.peak() <= 1e-8 {
            return Err("The candidate is silent".into());
        }
        auris_io::write_wav(
            &folder.join("audition.wav"),
            &audio,
            &auris_io::WavExportSettings {
                sample_rate: 22_050,
                bit_depth: auris_io::WavBitDepth::Float32,
                dither: false,
            },
        )
        .map_err(|e| e.to_string())?;
        SongCandidate::load_workshop(folder, idea)
    }

    fn arrange_candidate_singers(
        &mut self,
        request: &SongRequest,
        spec: &SongSpec,
    ) -> Result<(), String> {
        let original = self
            .project
            .tracks
            .iter()
            .find(|t| t.kind.as_singer().is_some())
            .cloned()
            .ok_or("No vocal notes were generated")?;
        let clips = original.kind.note_clips().ok_or("No vocal clips")?.to_vec();
        self.project.remove_track(original.id);
        for (index, voice) in request.singers.iter().enumerate() {
            let name = voice.track_name();
            let track = self.project.add_singer_track(&name, auris_synth::Vocal::ID);
            for (position, clip) in clips.iter().enumerate() {
                let together = request.singing == SongSinging::Harmony
                    || (request.singing == SongSinging::Auto && position + 1 == clips.len());
                if !together && position % request.singers.len() != index {
                    continue;
                }
                let id = self
                    .project
                    .add_midi_clip(track, &clip.name, clip.start, clip.length)
                    .ok_or("Could not create vocal clip")?;
                let target = self.project.midi_clip_mut(id).ok_or("Missing vocal clip")?;
                target.notes = clip.notes.clone();
                target.length_is_explicit = true;
                if together && index > 0 {
                    for note in &mut target.notes {
                        let degree = (-35..70)
                            .min_by_key(|&degree| {
                                (spec.key.midi(degree, 0) - i32::from(note.pitch)).abs()
                            })
                            .unwrap_or(0);
                        note.pitch =
                            spec.key.midi(degree + (index * 2) as i32, 0).clamp(0, 127) as u8;
                    }
                }
            }
            self.set_singer_voice(track, Some(&voice.path))
                .map_err(|e| e.to_string())?;
            if let Some(speaker) = &voice.speaker {
                self.set_singer_speaker(track, Some(speaker))
                    .map_err(|e| e.to_string())?;
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Adopt the exact document that produced an audition, as one undoable edit.
    pub fn adopt_song_candidate(&mut self, candidate: &SongCandidate) -> Result<(), SessionError> {
        if self.is_recording() {
            return Err(SessionError::RecordingInProgress);
        }
        candidate.project.validate_loop_expansion()?;
        // Prepare every asset before the undoable document swap. The candidate's
        // temporary directory may disappear immediately after this method returns.
        let mut project = candidate.project.clone();
        // Source banks also serve Undo. Independent candidates allocate IDs from one,
        // so collision-free IDs are essential before installing their decoded audio.
        let colliding: Vec<_> = project
            .audio_sources
            .keys()
            .copied()
            .filter(|id| self.waveforms.contains_key(id))
            .collect();
        for old_id in colliding {
            let source = project.audio_sources[&old_id].clone();
            let new_id = loop {
                let id = project.add_audio_source(
                    &source.name,
                    source.path.clone(),
                    source.frame_count,
                    source.sample_rate,
                    source.channel_count,
                );
                if !self.waveforms.contains_key(&id) {
                    break id;
                }
                project.audio_sources.remove(&id);
            };
            let mut replacement = source;
            replacement.id = new_id;
            project.audio_sources.insert(new_id, replacement);
            project.audio_sources.remove(&old_id);
            for track in &mut project.tracks {
                if let Some(singer) = track.kind.as_singer_mut()
                    && let Some(take) = &mut singer.take
                    && take.source == old_id
                {
                    take.source = new_id;
                }
            }
        }
        let folder = self
            .project_folder()
            .ok_or(SessionError::NoPath)?
            .join(auris_io::AUDIO_DIR);
        let mut fonts = Vec::new();
        for font in project.soundfonts.values() {
            let path = font.path.resolve(None).ok_or(SessionError::NoPath)?;
            fonts.push((font.id, path.clone(), auris_io::load_soundfont(&path)?));
        }
        for track in &project.tracks {
            if let Some(instrument) = track.kind.as_instrument()
                && let Some(file) = &instrument.file
            {
                let path = file.resolve(None).ok_or(SessionError::NoPath)?;
                let source = if let Some(id) =
                    instrument.instrument_id.strip_prefix(auris_clap::ID_PREFIX)
                {
                    auris_compose::spec::PartSource::Clap {
                        path,
                        plugin_id: id.into(),
                    }
                } else if let Some(id) =
                    instrument.instrument_id.strip_prefix(auris_vst3::ID_PREFIX)
                {
                    auris_compose::spec::PartSource::Vst3 {
                        path,
                        class_id: id.into(),
                    }
                } else {
                    return Err(SessionError::SongSource(format!(
                        "Unknown hosted instrument: {}",
                        instrument.instrument_id
                    )));
                };
                self.validate_candidate_source(&source)?;
            }
        }
        // Copy the audio last, once nothing else can refuse the candidate, and take the copies
        // back out if one of them fails: a refused adoption must not leave files in `Audio/`.
        let mut audio = Vec::new();
        let mut copied = Vec::new();
        // `copy_into` reuses an identical file already in the folder instead of writing another,
        // and such a file may belong to the open document or its undo history. Only names that
        // did not exist beforehand are this adoption's to take back.
        let existing: std::collections::BTreeSet<_> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect();
        let result = (|| -> Result<(), SessionError> {
            for source in project.audio_sources.values_mut() {
                let from = source.path.resolve(None).ok_or(SessionError::NoPath)?;
                let name = auris_io::copy_into(&from, &folder)?;
                let path = folder.join(&name);
                if !existing.contains(&name) {
                    copied.push(path.clone());
                }
                let buffer = auris_io::import_audio_file(&path, project.sample_rate)?;
                source.path =
                    AssetPath::inside(std::path::Path::new(auris_io::AUDIO_DIR).join(name));
                audio.push((source.id, Arc::new(buffer)));
            }
            Ok(())
        })();
        if let Err(error) = result {
            remove_copies(&copied);
            return Err(error);
        }
        self.stop_output_preview();
        if let Err(error) = self.collect_hosted_state() {
            remove_copies(&copied);
            return Err(error);
        }
        self.record(Edit::Compose);
        for (id, path, font) in fonts {
            self.cache_font(&path, Arc::clone(&font), false);
            self.fonts.insert(id, font);
        }
        for (id, buffer) in audio {
            self.install_source(id, buffer);
        }
        self.hosted.clear();
        self.vst3.clear();
        self.replace_project(project);
        self.dirty = true;
        Ok(())
    }
}

/// Removes the files an adoption copied in before it was refused.
fn remove_copies(copies: &[std::path::PathBuf]) {
    for copy in copies {
        if let Err(error) = std::fs::remove_file(copy) {
            log::warn!("could not remove {}: {error}", copy.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workshop_descendants_keep_parent_notes_and_do_not_modify_parent_files() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        session
            .agent_command(crate::live_agent::Command::Compose {
                spec: None,
                preset: Some("chiptune".into()),
                replace: false,
            })
            .unwrap();
        let idea = SongIdea {
            title: "Original".into(),
            description: "Chiptune".into(),
            preset: "chiptune".into(),
            tempo: 120.0,
            key: "C major".into(),
            density: 0.5,
            lead: "synth".into(),
        };
        let cancel = AtomicBool::new(false);
        let original = session
            .finish_workshop_candidate(&root.path().join("1"), idea.clone(), &cancel)
            .unwrap();
        let parent = root.path().join("1/song/song.auris");
        let bytes = std::fs::read(&parent).unwrap();
        let mut child = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        assert!(child.open(&parent).unwrap().is_empty());
        child.set_tempo_at(auris_core::time::Ticks::ZERO, 90.0);
        let derived = child
            .finish_workshop_candidate(&root.path().join("2"), idea, &cancel)
            .unwrap();
        assert_eq!(original.project.tracks, derived.project.tracks);
        assert_eq!(std::fs::read(parent).unwrap(), bytes);
        assert!(derived.audio.duration_seconds() > 0.0);
    }

    #[test]
    fn authoring_starts_with_exact_sounds_and_voices_but_no_generated_score() {
        let folder = tempfile::tempdir().unwrap();
        let mut request = vocal_request(folder.path(), SongSinging::Alternate);
        for singer in &mut request.singers {
            singer.name = "Shared voice".into();
        }
        request.sources.push(SongSourceChoice {
            role: "chords".into(),
            instrument: "auris.physical.piano".into(),
            source: None,
        });
        let session = request.authoring_session(&idea()).unwrap();
        assert_eq!(session.project().tracks.len(), 3);
        assert_eq!(session.project().tracks[1].name, "Shared voice — First");
        assert_eq!(session.project().tracks[2].name, "Shared voice — Second");
        assert_eq!(
            session.project().tracks[0]
                .kind
                .as_instrument()
                .unwrap()
                .instrument_id,
            "auris.physical.piano"
        );
        assert!(
            session.project().tracks.iter().all(|track| track
                .kind
                .note_clips()
                .unwrap()
                .is_empty())
        );
        assert!(session.project().song_spec.is_none());
        assert!(session.project().audio_sources.is_empty());
        for track in &session.project().tracks[1..] {
            assert!(track.kind.as_singer().unwrap().voice.is_some());
        }
    }

    fn vocal_request(folder: &std::path::Path, singing: SongSinging) -> SongRequest {
        let path = folder.join("test.voicevox.json");
        std::fs::write(&path, r#"{"format_version":1,"name":"Test voice","url":"http://127.0.0.1:1","styles":[{"name":"First","query_style_id":6000,"decode_style_id":3001},{"name":"Second","query_style_id":6000,"decode_style_id":3002}]}"#).unwrap();
        SongRequest {
            seconds: Some(90),
            lyrics: "さくらさいた\nはるがきた\n\nひかりとどく\nそらをみあげた".into(),
            singing,
            singers: ["First", "Second"]
                .into_iter()
                .map(|speaker| SongSinger {
                    name: speaker.into(),
                    path: path.clone(),
                    speaker: Some(speaker.into()),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn vocal_arrangements_preserve_lyrics_and_independent_singers() {
        let folder = tempfile::tempdir().unwrap();
        for mode in [
            SongSinging::Alternate,
            SongSinging::Harmony,
            SongSinging::Auto,
        ] {
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let request = vocal_request(folder.path(), mode);
            let spec = request.specification(&idea(), 42, &session).unwrap();
            let lyrics = spec
                .form
                .iter()
                .filter_map(|name| {
                    let text = &spec.sections[name].lyrics;
                    (!text.is_empty()).then_some(text.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            assert_eq!(lyrics, request.lyrics);
            session.compose_without_balance(&compose(&spec)).unwrap();
            session.arrange_candidate_singers(&request, &spec).unwrap();
            let voices: Vec<_> = session
                .project
                .tracks
                .iter()
                .filter(|t| t.kind.as_singer().is_some())
                .collect();
            assert_eq!(voices.len(), 2);
            assert_eq!(voices[0].name, "First");
            assert_eq!(voices[1].name, "Second");
            let first = voices[0].kind.note_clips().unwrap();
            let second = voices[1].kind.note_clips().unwrap();
            assert!(!first.is_empty() && !second.is_empty());
            if mode == SongSinging::Alternate {
                assert!(
                    first
                        .iter()
                        .all(|a| second.iter().all(|b| a.start != b.start))
                );
            }
            if mode == SongSinging::Harmony {
                assert_eq!(first.len(), second.len());
                assert_eq!(first[0].notes[0].lyric, second[0].notes[0].lyric);
                assert_ne!(first[0].notes[0].pitch, second[0].notes[0].pitch);
            }
        }
    }

    #[test]
    fn duration_and_explicit_sources_are_preserved_without_erasing_other_parts() {
        let session = Session::new(SessionOptions::headless()).unwrap();
        for seconds in [30, 40, 110, 240, 300] {
            assert!(idea().specification(seconds, 1).is_ok());
        }
        let request = SongRequest {
            sources: vec![SongSourceChoice {
                role: "melody".into(),
                instrument: "auris.physical.violin".into(),
                source: None,
            }],
            ..Default::default()
        };
        let spec = request.specification(&idea(), 42, &session).unwrap();
        assert_eq!(
            spec.parts
                .iter()
                .find(|p| p.role == auris_compose::spec::Role::Melody)
                .unwrap()
                .instrument,
            "auris.physical.violin"
        );
        assert_eq!(
            spec.total_bars(),
            preset("chiptune").unwrap().spec().total_bars()
        );
    }

    #[test]
    fn concurrent_candidate_saves_refuse_replacement() {
        let root = tempfile::tempdir().unwrap();
        let chosen = root.path().join("Concurrent.auris");
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let audio = Arc::new(AudioBuffer::from_planar(vec![vec![0.25; 480]], 48_000.0).unwrap());
        let writers: Vec<_> = (0..16)
            .map(|index| {
                let barrier = Arc::clone(&barrier);
                let chosen = chosen.clone();
                let candidate = SongCandidate {
                    idea: idea(),
                    project: Project::new(format!("Draft {index}"), 48_000.0),
                    audio: Arc::clone(&audio),
                    timeline: Default::default(),
                    _assets: None,
                };
                std::thread::spawn(move || {
                    barrier.wait();
                    candidate.save_as(&chosen)
                })
            })
            .collect();
        let results: Vec<_> = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect();
        let successes = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(
            successes, 1,
            "multiple saves replaced the same project: {results:?}"
        );
        for result in &results {
            assert!(matches!(result, Ok(_) | Err(SessionError::WouldReplace(_))));
        }
        let winner = results.iter().position(Result::is_ok).unwrap();
        let document = auris_io::document_in_folder(&chosen);
        assert_eq!(
            auris_io::load_project(&document).unwrap().name,
            format!("Draft {winner}")
        );
    }

    #[test]
    fn candidate_save_waits_for_an_ordinary_save_to_publish() {
        let root = tempfile::tempdir().unwrap();
        let chosen = root.path().join("Shared.auris");
        let mut ordinary = Session::new(SessionOptions::headless()).unwrap();
        ordinary.project.name = "Ordinary".into();
        let staged = ordinary
            .begin_save_as(&chosen)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap()
            .unwrap();
        let candidate = SongCandidate {
            idea: idea(),
            project: Project::new("Candidate", 48_000.0),
            audio: Arc::new(AudioBuffer::from_planar(vec![vec![0.25; 480]], 48_000.0).unwrap()),
            timeline: Default::default(),
            _assets: None,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || sender.send(candidate.save_as(&chosen)).unwrap());
        // The ordinary save holds the project lock until its staged document is published.
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        let report = ordinary.continue_save(staged).unwrap().unwrap();
        let saved = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        worker.join().unwrap();
        assert!(matches!(saved, Err(SessionError::WouldReplace(path)) if path == report.document));
        assert_eq!(
            auris_io::load_project(&report.document).unwrap().name,
            "Ordinary"
        );
    }

    #[test]
    fn an_unchosen_candidate_saves_as_its_own_project_and_never_replaces_one() {
        let assets = tempfile::tempdir().unwrap();
        let file = assets.path().join("voice.wav");
        let buffer = Arc::new(AudioBuffer::from_planar(vec![vec![0.25; 4800]], 48_000.0).unwrap());
        auris_io::write_wav(&file, &buffer, &Default::default()).unwrap();
        let mut project = Project::new("Draft", 48_000.0);
        let source =
            project.add_audio_source("Voice", AssetPath::external(&file), 4800, 48_000.0, 1);
        let candidate = SongCandidate {
            idea: idea(),
            project,
            audio: buffer,
            timeline: Default::default(),
            _assets: Some(assets),
        };
        let root = tempfile::tempdir().unwrap();
        let chosen = root.path().join("Second.auris");
        let report = candidate.save_as(&chosen).unwrap();
        assert_eq!(report.document, root.path().join("Second/Second.auris"));
        // The draft's temporary directory goes with it; the saved folder must not need it.
        drop(candidate);
        let mut reopened = Session::new(SessionOptions::headless()).unwrap();
        assert!(reopened.open(&report.document).unwrap().is_empty());
        assert!(reopened.project.audio_sources[&source].path.is_inside());
        assert!((reopened.bank.get(source).unwrap().peak() - 0.25).abs() < 0.001);

        let other = SongCandidate {
            idea: idea(),
            project: Project::new("Other", 48_000.0),
            audio: Arc::new(AudioBuffer::from_planar(vec![vec![0.5; 480]], 48_000.0).unwrap()),
            timeline: Default::default(),
            _assets: None,
        };
        assert!(matches!(
            other.save_as(&chosen),
            Err(SessionError::WouldReplace(path)) if path == report.document
        ));
        assert_eq!(
            auris_io::load_project(&report.document).unwrap().name,
            "Draft"
        );
    }

    #[test]
    fn vocal_assets_survive_candidate_drop_save_reload_and_undo_source_collisions() {
        let assets = tempfile::tempdir().unwrap();
        let file = assets.path().join("voice.wav");
        let buffer = Arc::new(AudioBuffer::from_planar(vec![vec![0.25; 4800]], 48_000.0).unwrap());
        auris_io::write_wav(&file, &buffer, &Default::default()).unwrap();
        let mut project = Project::new("Vocal", 48_000.0);
        let track = project.add_singer_track("Singer", auris_synth::Vocal::ID);
        let source =
            project.add_audio_source("Voice", AssetPath::external(&file), 4800, 48_000.0, 1);
        project
            .track_mut(track)
            .unwrap()
            .kind
            .as_singer_mut()
            .unwrap()
            .take = Some(auris_core::SingerTake {
            provenance: None,
            source,
            fingerprint: 0,
            seed: 0,
            backend_pitch: None,
        });
        let candidate = SongCandidate {
            idea: idea(),
            project,
            audio: buffer,
            timeline: Default::default(),
            _assets: Some(assets),
        };
        let mut target = Session::new(SessionOptions::headless()).unwrap();
        let old = Arc::new(AudioBuffer::from_planar(vec![vec![0.75; 4800]], 48_000.0).unwrap());
        target.install_source(source, old.clone());
        let before = target.project.clone();
        target.adopt_song_candidate(&candidate).unwrap();
        let new_source = target
            .project
            .track(track)
            .unwrap()
            .kind
            .as_singer()
            .unwrap()
            .take
            .as_ref()
            .unwrap()
            .source;
        assert_ne!(new_source, source);
        assert!(Arc::ptr_eq(target.bank.get(source).unwrap(), &old));
        drop(candidate);
        let stored = target.project.audio_sources[&new_source]
            .path
            .resolve(target.project_folder())
            .unwrap();
        assert!(stored.is_file());
        let adopted = target.project.clone();
        target.undo();
        assert_eq!(target.project, before);
        target.redo();
        assert_eq!(target.project, adopted);
        let saved = tempfile::tempdir().unwrap();
        target.save_as(&saved.path().join("Song.auris")).unwrap();
        let mut reopened = Session::new(SessionOptions::headless()).unwrap();
        assert!(
            reopened
                .open(&saved.path().join("Song/Song.auris"))
                .unwrap()
                .is_empty()
        );
        assert!((reopened.bank.get(new_source).unwrap().peak() - 0.25).abs() < 0.001);
    }

    #[test]
    fn candidate_displays_written_lyrics_after_saving_explicit_pronunciation() {
        use auris_core::{Note, time::Ticks};
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Lead");
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(1920))
            .unwrap();
        for beat in 0..4 {
            session
                .add_note(clip, Note::new(60, Ticks(480 * beat), Ticks(480)))
                .unwrap();
        }
        session
            .agent_command(
                serde_json::from_value(serde_json::json!({
                    "action":"write_lyrics", "clip":clip.0, "text":"月の夜", "reading":"つきのよ"
                }))
                .unwrap(),
            )
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let saved = session
            .save_as(&folder.path().join("Lyrics.auris"))
            .unwrap();
        let mut reopened = Session::new(SessionOptions::headless()).unwrap();
        reopened.open(&saved.document).unwrap();
        let candidate = SongCandidate {
            idea: idea(),
            project: reopened.project().clone(),
            audio: Arc::new(AudioBuffer::stereo(1, 48_000.0)),
            timeline: Default::default(),
            _assets: None,
        };
        assert_eq!(candidate.lyrics(), "月の夜");
        assert_eq!(
            candidate.project.midi_clip(clip).unwrap().1.notes[0].phonemes,
            ["ts", "ɯ"]
        );
    }

    #[test]
    fn candidate_lyrics_preserve_sections_repeats_and_manual_vocal_text() {
        use auris_core::{Note, time::Ticks};
        let mut candidate = SongCandidate {
            idea: idea(),
            project: Project::new("Lyrics", 48_000.0),
            audio: Arc::new(AudioBuffer::stereo(1, 48_000.0)),
            timeline: Default::default(),
            _assets: None,
        };
        assert!(candidate.lyrics().is_empty());
        let spec = SongSpec::parse(
            r#"
            form = ['verse', 'chorus', 'verse']
            [section.verse]
            lyrics = "夜の雨\n光る街"
            [section.chorus]
            lyrics = 'また会おう'
        "#,
        )
        .unwrap();
        candidate.project.song_spec = Some(spec.to_toml());
        assert_eq!(
            candidate.lyrics(),
            "夜の雨\n光る街\n\nまた会おう\n\n夜の雨\n光る街"
        );

        let mut session = Session::new(SessionOptions::headless()).unwrap();
        for name in ["Lead", "Harmony"] {
            let track = session.add_singer_track(name);
            let clip = session
                .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(960))
                .unwrap();
            let clip = session.project.midi_clip_mut(clip).unwrap();
            let mut first = Note::new(60, Ticks::ZERO, Ticks(240));
            first.lyric = "夜の雨".into();
            let mut continuation = Note::new(62, Ticks(240), Ticks(240));
            continuation.lyric = super::super::LYRIC_CONTINUATION.into();
            let mut last = Note::new(64, Ticks(480), Ticks(240));
            last.lyric = "、また会おう".into();
            clip.notes = vec![last, continuation, first];
        }
        candidate.project = session.project().clone();
        assert_eq!(candidate.lyrics(), "夜の雨、また会おう");
    }

    fn idea() -> SongIdea {
        SongIdea {
            title: "Draft".into(),
            description: "Bright keys".into(),
            preset: "chiptune".into(),
            tempo: 120.0,
            key: "C major".into(),
            density: 0.5,
            lead: "piano".into(),
        }
    }
    #[test]
    fn invalid_model_fields_are_rejected_before_generation() {
        let mut bad = idea();
        bad.tempo = f64::NAN;
        assert!(bad.validate().is_err());
        let mut bad = idea();
        bad.lead = "external.vst".into();
        assert!(bad.validate().is_err());
        assert!(idea().specification(600, 0).is_err());
    }
    #[test]
    fn generated_audio_and_adopted_score_share_one_candidate() {
        let candidate = SongCandidate::generate(
            idea(),
            "Bright instrumental",
            30,
            42,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(candidate.audio.peak().is_finite() && candidate.audio.peak() > 0.001);
        assert!(
            candidate
                .project
                .creation_brief
                .as_ref()
                .unwrap()
                .contains("Bright instrumental")
        );
        let mut target = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        target.adopt_song_candidate(&candidate).unwrap();
        assert_eq!(target.project(), &candidate.project);
        assert!(target.is_dirty());
    }

    #[test]
    fn every_catalog_direction_has_only_builtin_instrumental_parts() {
        for preset in auris_compose::PRESETS {
            for lead in ["piano", "guitar", "flute", "strings", "synth"] {
                let mut direction = idea();
                direction.preset = preset.name.into();
                direction.lead = lead.into();
                let spec = direction.specification(30, 42).unwrap();
                assert!(spec.singer.is_none());
                assert!(spec.sections.values().all(|s| s.lyrics.is_empty()));
                assert!(
                    spec.parts
                        .iter()
                        .all(|p| p.source.is_none() && p.program.is_none())
                );
                let mut session =
                    Session::new(SessionOptions::headless().with_balance(false)).unwrap();
                let report = session.compose_without_balance(&compose(&spec)).unwrap();
                assert!(
                    report.substituted.is_empty(),
                    "{} / {}: {:?}",
                    preset.name,
                    lead,
                    report.substituted
                );
            }
        }
        assert!(SongCandidate::generate(idea(), "", 30, 42, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn adoption_preserves_the_exact_document_and_is_undoable() {
        let mut source = Session::new(SessionOptions::headless()).unwrap();
        source.add_audio_track("Exact retained track");
        let candidate = SongCandidate {
            idea: idea(),
            project: source.project().clone(),
            audio: Arc::new(AudioBuffer::from_planar(vec![vec![0.1; 100]], 48_000.0).unwrap()),
            timeline: Default::default(),
            _assets: None,
        };
        let mut target = Session::new(SessionOptions::headless()).unwrap();
        let before = target.project().clone();
        target.adopt_song_candidate(&candidate).unwrap();
        assert_eq!(target.project(), &candidate.project);
        target.undo();
        assert_eq!(target.project(), &before);
    }

    #[test]
    fn a_refused_adoption_leaves_no_audio_copies_behind() {
        let assets = tempfile::tempdir().unwrap();
        let file = assets.path().join("voice.wav");
        let buffer = Arc::new(AudioBuffer::from_planar(vec![vec![0.25; 4800]], 48_000.0).unwrap());
        auris_io::write_wav(&file, &buffer, &Default::default()).unwrap();
        let mut project = Project::new("Refused", 48_000.0);
        project.add_audio_source("Voice", AssetPath::external(&file), 4800, 48_000.0, 1);
        // A font that cannot be read refuses the candidate after its audio has been found.
        project.add_soundfont(
            "Missing",
            AssetPath::external(assets.path().join("missing.sf2")),
            0,
        );
        let candidate = SongCandidate {
            idea: idea(),
            project,
            audio: buffer,
            timeline: Default::default(),
            _assets: Some(assets),
        };
        let mut target = Session::new(SessionOptions::headless()).unwrap();
        let audio = target.project_folder().unwrap().join(auris_io::AUDIO_DIR);
        let listing = || {
            std::fs::read_dir(&audio)
                .map(|entries| entries.filter_map(Result::ok).count())
                .unwrap_or(0)
        };
        let before = (target.project.clone(), listing());
        assert!(target.adopt_song_candidate(&candidate).is_err());
        assert_eq!((target.project.clone(), listing()), before);
    }
}
