//! The flattened, render-ready form of a [`Project`].
//!
//! A [`RenderGraph`] is built on the UI thread — where allocating, instantiating plugins and
//! walking the tempo map are all free — and then *moved* onto the audio thread. Nothing in it is
//! shared: the audio thread owns the graph outright while it renders, and the previous graph is
//! handed back so its destructor runs on the UI thread.
//!
//! Building resolves the three indirections the document model deliberately keeps:
//!
//! * plugin ids become live [`Instrument`] and [`Effect`] objects from the registry — or, for the
//!   few the registry cannot build, from the [`PlacedEffects`] and [`PlacedInstruments`] the
//!   caller brought,
//! * musical tick positions become absolute timeline sample positions through the [`TempoMap`],
//! * [`SourceId`](auris_core::project::SourceId)s become `Arc<AudioBuffer>` handles from the
//!   [`AudioSourceBank`].
//!
//! Everything a block needs is pre-sized here, including the per-block event scratch, so
//! rendering never touches the allocator.
//!
//! Building itself is here, with [`RenderGraph`]. The five topics it calls out to have a file
//! each, and none of them calls back: `automation` resolves the document's lanes, `latency` lays
//! out the delay lines, `schedule` flattens musical time into frames, `strip` is the mixer strip
//! and `track` is the node the routing joins up. They are private and re-exported, so every path
//! into the module is the one it always was.

mod automation;
mod latency;
mod schedule;
mod strip;
mod track;

pub use schedule::{RenderAudioClip, ScheduledEvent};
pub use strip::{MuteFade, RenderStrip, SmoothedGain};
pub use track::{RenderSource, RenderTrack};

pub(crate) use latency::LatencyDelay;
pub(crate) use track::RenderSend;

use std::collections::BTreeMap;
use std::sync::Arc;

use auris_core::param::db_to_gain;
use auris_core::plugin::{Effect, Instrument, PrepareContext};
use auris_core::project::{AudioSourceBank, EffectSlotId, Project, TrackId, TrackKind};
use auris_core::registry::PluginRegistry;
use auris_core::time::{Samples, TempoMap};
use auris_core::{AudioBuffer, ParamId};

use automation::{RenderAutomation, drive_automation, resolve_automation};
use latency::{longest_paths, plan_latency};
use schedule::{
    max_events_in_window, max_sounding_notes, resolve_audio_clip, schedule_clip, sort_events,
};

/// Channel count of the internal mix bus.
///
/// The graph always renders stereo because the pan law is a stereo law; the renderer maps the
/// bus onto whatever channel count the output buffer has.
pub const RENDER_CHANNELS: usize = 2;

/// Rate used when neither the caller nor the project offers a usable one.
const DEFAULT_SAMPLE_RATE: f64 = 48_000.0;

/// Effects the caller built itself, waiting to be placed in their slots.
///
/// Almost every effect comes from the [`PluginRegistry`], whose factory is
/// `Fn() -> Box<dyn Effect>` and so can build one from nothing but an id. Some cannot be built
/// that way: a hosted CLAP plugin is two objects, and the half that answers questions has to stay
/// on the main thread while only the half that renders belongs in a graph. The caller that owns
/// the other half builds those and leaves them here.
///
/// Building **takes** each effect it uses, so a map that still holds entries afterwards names
/// slots that are no longer in the project — which is how the caller finds out that a hosted
/// plugin can be let go.
pub type PlacedEffects = BTreeMap<EffectSlotId, Box<dyn Effect>>;

/// Instruments the caller built itself, waiting to be placed on their tracks.
///
/// [`PlacedEffects`] one lane over, and for exactly the same reason. A track holds at most one
/// instrument, so this is keyed by the track rather than by a slot; a track not named here gets
/// its instrument from the registry as it always did.
pub type PlacedInstruments = BTreeMap<TrackId, Box<dyn Instrument>>;

/// Room left in each track's per-block event buffer for notes played from the UI.
const AUDITION_HEADROOM: usize = 16;

/// Room the note-chase needs on top of whatever it is re-issuing: the all-notes-off that clears
/// the old position before the notes belonging to the new one go in.
pub(crate) const CHASE_HEADROOM: usize = 1;

/// Number of MIDI pitches, and therefore the size of the chase table.
pub(crate) const PITCH_COUNT: usize = 128;

/// Number of graphs the audio thread can hold before it stops accepting new ones.
pub(crate) const RETIRED_GRAPH_SLOTS: usize = 8;

/// How long a mute takes to close or open, in milliseconds.
///
/// Short enough that a mute still feels instant under the finger, long enough that the step it
/// replaces is inaudible. At 48 kHz it is 240 frames — under half a typical block, so in practice
/// a mute is silent by the end of the block it was pressed in.
pub(crate) const MUTE_FADE_MS: f64 = 5.0;

/// Every track plus the master bus, sized and prepared for one particular block size.
pub struct RenderGraph {
    pub(crate) tracks: Vec<RenderTrack>,
    pub(crate) master: RenderStrip,
    sample_rate: f64,
    max_block: usize,
    /// Where every track's signal is summed on the way to the master, one buffer per bus.
    ///
    /// Cleared at the start of every segment and filled as the routing order is walked, so a bus
    /// finds its whole input waiting by the time its own turn comes.
    pub(crate) bus_inputs: Vec<AudioBuffer>,
    /// Which track each bus input belongs to, so the latency plan can be written in track indices
    /// while the render loop addresses buffers directly.
    bus_tracks: Vec<usize>,
    /// Track indices ordered so that everything feeding a bus comes before it.
    pub(crate) order: Vec<usize>,
    /// Total latency the delay lines were laid out for: playhead to output, in frames.
    latency: usize,
    /// Every strip's own latency as it was when the delays were laid out — each track's, then the
    /// master's last.
    ///
    /// Kept rather than recomputed so it can be compared with what the strips report now: a
    /// parameter that moves a plugin's latency after the graph was built leaves the two
    /// disagreeing, and that disagreement is the signal to rebuild. Compared entry by entry rather
    /// than as a total, because two plugins can trade latency between them and leave the total
    /// alone while the tracks fall out of step with each other.
    built_latencies: Vec<usize>,
    pub(crate) tempo_map: TempoMap,
    /// The document's automation, resolved to positions in this graph.
    ///
    /// Empty for a project nobody has automated, which is what makes the whole feature free when
    /// it is not in use: the renderer's per-segment call returns on the first line.
    automation: Vec<RenderAutomation>,
    /// Frame the next segment has to begin on for the automation to be *continuing* rather than
    /// arriving.
    ///
    /// The same idea as [`RenderTrack::continued_from`] and for the same reason: a playhead that
    /// jumped is not a parameter that moved. A seek into the middle of a fade should sound like
    /// that part of the fade at once, and ramping there from wherever the fader was left is a
    /// swell nobody wrote. `None` on a fresh graph, so the first segment always arrives.
    automation_from: Option<u64>,
    /// The click, which is not a track and does not go through one.
    ///
    /// In the graph rather than beside it so that it survives the same way everything else does:
    /// a rebuild happens on every structural edit, and a metronome the UI held would have to be
    /// re-sent after each of them or fall silent halfway through a session.
    pub(crate) metronome: crate::metronome::Metronome,
    pub(crate) master_scratch: AudioBuffer,
    pub(crate) master_peak: [f32; 2],
    /// Where a spectrum display, if one is open, gets its samples.
    ///
    /// Shared with the UI rather than owned by it, because only the render path ever sees a
    /// strip's signal — the document holds parameter values, not audio.
    pub(crate) scope: Arc<crate::scope::Scope>,
    /// The live input, and the track it plays through, while somebody is monitoring.
    pub(crate) monitor: Option<MonitorTap>,
}

/// A live input joined to the track it is heard through.
///
/// The input enters the mix as though the track were playing it: before the effects, before the
/// fader, before the sends. That is what makes monitoring *useful* rather than merely audible — a
/// singer hears themselves through the reverb they will be recorded into, at the level the fader
/// is set to, and a muted track is silent because a muted track is silent.
pub(crate) struct MonitorTap {
    pub(crate) ring: Arc<crate::monitor::MonitorRing>,
    /// Index into [`RenderGraph::tracks`], resolved when the tap was attached.
    pub(crate) track: usize,
}

impl RenderGraph {
    /// Builds a render graph for `project` at the project's own sample rate.
    ///
    /// Never fails: a plugin id the registry does not know is logged and replaced by a silent
    /// stand-in, so a project still opens when a plugin has been removed — and every track and
    /// effect slot keeps its position, which is what command indices are addressed by.
    pub fn build(
        project: &Project,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        max_block: usize,
    ) -> RenderGraph {
        Self::build_at(project, bank, registry, max_block, project.sample_rate)
    }

    /// Builds a render graph at an explicit sample rate.
    ///
    /// The audio device decides the rate the engine actually runs at, which is not necessarily
    /// the rate stored in the project, so the caller passes it in.
    pub fn build_at(
        project: &Project,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        max_block: usize,
        sample_rate: f64,
    ) -> RenderGraph {
        Self::build_with(
            project,
            bank,
            registry,
            &mut PlacedEffects::new(),
            &mut PlacedInstruments::new(),
            max_block,
            sample_rate,
        )
    }

    /// Builds a render graph, taking some of its plugins from the caller.
    ///
    /// See [`PlacedEffects`] and [`PlacedInstruments`] for why a caller would have any. Anything
    /// not named there is built from the registry exactly as before, so a project with no hosted
    /// plugins takes the same path it always did.
    pub fn build_with(
        project: &Project,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        placed: &mut PlacedEffects,
        instruments: &mut PlacedInstruments,
        max_block: usize,
        sample_rate: f64,
    ) -> RenderGraph {
        let max_block = max_block.max(1);
        // A zero, negative or NaN rate would make every derived position meaningless and would
        // poison the meters, so fall back to the project's rate and then to a sane default: a
        // project deserialised from a corrupt file can carry a nonsense rate too.
        let sample_rate = [sample_rate, project.sample_rate, DEFAULT_SAMPLE_RATE]
            .into_iter()
            .find(|rate| rate.is_finite() && *rate > 0.0)
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        let prepare = PrepareContext::new(sample_rate, max_block, RENDER_CHANNELS);
        // The solo resolution travels along the routing, so it is worked out for the whole project
        // at once rather than per track: soloing a drum track has to leave the drum bus open.
        let solo = project.solo_resolution();
        // One input buffer per bus, and the slot each bus track owns. Resolved before the tracks
        // are built so that an output or a send can name a bus that appears later in the list.
        let bus_tracks: Vec<usize> = (0..project.tracks.len())
            .filter(|index| project.tracks[*index].kind.is_bus())
            .collect();
        let bus_slot = |id: TrackId| {
            let index = project.track_index(id)?;
            bus_tracks.iter().position(|track| *track == index)
        };

        let mut tracks = Vec::with_capacity(project.tracks.len());
        for (index, track) in project.tracks.iter().enumerate() {
            // `audible` carries only the solo resolution; the strip keeps its own mute so a
            // mute toggle is a command rather than a rebuild.
            let audible = solo.get(index).copied().unwrap_or(true);
            let strip = RenderStrip::from_mixer(&track.mixer, audible, registry, placed, &prepare);
            let source = match &track.kind {
                TrackKind::Instrument(instrument_track) => {
                    // The caller's own instrument first, and it is *taken*: what is left in the
                    // map afterwards names tracks the project no longer plays that way.
                    let built = match instruments.remove(&track.id) {
                        Some(instrument) => Ok(instrument),
                        None => registry.create_instrument(&instrument_track.instrument_id),
                    };
                    match built {
                        Ok(mut instrument) => {
                            instrument.load_state(&instrument_track.instrument_state);
                            instrument.prepare(&prepare);
                            let mut events = Vec::new();
                            for clip in &instrument_track.clips {
                                schedule_clip(clip, &project.tempo_map, sample_rate, &mut events);
                            }
                            sort_events(&mut events);
                            RenderSource::Instrument { instrument, events }
                        }
                        Err(error) => {
                            log::warn!(
                                "track `{}` keeps its slot but stays silent: {error}",
                                track.name
                            );
                            RenderSource::Silence
                        }
                    }
                }
                TrackKind::Audio(audio_track) => {
                    let mut clips = Vec::with_capacity(audio_track.clips.len());
                    for clip in &audio_track.clips {
                        // A clip's trim is counted in the frames of the file it came from, which
                        // is not necessarily the rate this graph renders at.
                        let source_rate = project
                            .audio_sources
                            .get(&clip.source)
                            .map_or(project.sample_rate, |source| source.sample_rate);
                        resolve_audio_clip(
                            clip,
                            bank,
                            &project.tempo_map,
                            sample_rate,
                            source_rate,
                            &mut clips,
                        );
                    }
                    clips.sort_by_key(|clip| clip.start_frame);
                    RenderSource::Audio { clips }
                }
                // A bus that somehow has no slot cannot happen — the slots are made from exactly
                // the tracks that are buses — but silence is the right answer if it ever did.
                TrackKind::Bus => match bus_slot(track.id) {
                    Some(input) => RenderSource::Bus { input },
                    None => RenderSource::Silence,
                },
            };

            // Two independent bounds on the per-block event buffer: how many scheduled events
            // can fall inside one block, and how many notes the chase can re-issue after a jump.
            let (event_headroom, chase_headroom) = match &source {
                RenderSource::Instrument { events, .. } => (
                    max_events_in_window(events, max_block as u64),
                    max_sounding_notes(events) + CHASE_HEADROOM,
                ),
                _ => (0, 0),
            };
            let mut scratch = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
            scratch.reserve_frames(max_block);

            // A route to a bus that is not there was already repaired when the document was
            // loaded; dropping it to the master here is the second lock on that door, and it is
            // what keeps the render loop free of a check on every block.
            let output = match track.output.bus() {
                Some(id) => match bus_slot(id) {
                    Some(slot) => Some(slot),
                    None => {
                        log::warn!("track `{}` names a bus that is not there", track.name);
                        None
                    }
                },
                None => None,
            };
            let sends = track
                .sends
                .iter()
                .filter_map(|send| {
                    Some(RenderSend {
                        target: bus_slot(send.target)?,
                        gain: SmoothedGain::new(db_to_gain(send.level_db)),
                        pre_fader: send.pre_fader,
                        // Both sized below, once every chain's latency is known.
                        delay: LatencyDelay::new(0, RENDER_CHANNELS),
                        scratch: AudioBuffer::new(RENDER_CHANNELS, 0, sample_rate),
                    })
                })
                .collect();

            tracks.push(RenderTrack {
                id: track.id,
                name: track.name.clone(),
                source,
                strip,
                output,
                sends,
                // Sized below, once every chain's latency is known.
                delay: LatencyDelay::new(0, RENDER_CHANNELS),
                output_delay: LatencyDelay::new(0, RENDER_CHANNELS),
                scratch,
                block_events: Vec::with_capacity(
                    event_headroom + AUDITION_HEADROOM + chase_headroom,
                ),
                audition: Vec::with_capacity(AUDITION_HEADROOM),
                continued_from: None,
                chase_counts: [0; PITCH_COUNT],
                chase_velocity: [0.0; PITCH_COUNT],
                peak: 0.0,
            });
        }

        let master = RenderStrip::from_mixer(&project.master, true, registry, placed, &prepare);
        let mut master_scratch = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
        master_scratch.reserve_frames(max_block);

        // Plugin delay compensation, laid out over the whole routing rather than over one row of
        // tracks: the sources run in parallel into a graph of buses, and they can only stay in
        // step if each is held back to the longest path through it. A graph where nothing looks
        // ahead allocates nothing here, because every delay comes out zero.
        let order = project.routing_order();
        let plan = plan_latency(&tracks, &bus_tracks, &order, master.latency_frames());
        for (index, track) in tracks.iter_mut().enumerate() {
            track.delay = LatencyDelay::new(plan.node[index], RENDER_CHANNELS);
            let mut edges = plan.edges[index].iter().copied();
            track.output_delay = LatencyDelay::new(edges.next().unwrap_or(0), RENDER_CHANNELS);
            for send in &mut track.sends {
                let frames = edges.next().unwrap_or(0);
                send.delay = LatencyDelay::new(frames, RENDER_CHANNELS);
                if frames > 0 {
                    // Only a send that is actually held back needs somewhere to be held, which is
                    // why an ordinary send costs one extra buffer of nothing at all.
                    send.scratch = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
                    send.scratch.reserve_frames(max_block);
                }
            }
        }

        let mut bus_inputs = Vec::with_capacity(bus_tracks.len());
        for _ in &bus_tracks {
            let mut buffer = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
            buffer.reserve_frames(max_block);
            bus_inputs.push(buffer);
        }

        let built_latencies = tracks
            .iter()
            .map(|track| track.strip.latency_frames())
            .chain(std::iter::once(master.latency_frames()))
            .collect();

        RenderGraph {
            tracks,
            master,
            sample_rate,
            max_block,
            bus_inputs,
            bus_tracks,
            order,
            latency: plan.total,
            built_latencies,
            tempo_map: project.tempo_map.clone(),
            automation: resolve_automation(project),
            automation_from: None,
            metronome: {
                let mut metronome = crate::metronome::Metronome::new(project.signatures.clone());
                metronome.set_enabled(project.metronome);
                metronome
            },
            master_scratch,
            master_peak: [0.0, 0.0],
            scope: Arc::new(crate::scope::Scope::new()),
            monitor: None,
        }
    }

    /// Points this graph at the scope the UI is reading.
    ///
    /// Handed in after building rather than created here, so a rebuild — which happens on every
    /// structural edit — does not leave an open spectrum display reading a scope nothing writes
    /// to any more.
    pub fn set_scope(&mut self, scope: Arc<crate::scope::Scope>) {
        self.scope = scope;
    }

    /// Plays a live input through `track`, or stops doing so with `None`.
    ///
    /// Handed in after building for the same reason the scope is, and re-applied on every rebuild:
    /// a graph is replaced whenever the document changes structurally, and a monitor that did not
    /// survive that would go quiet the moment somebody added a track.
    ///
    /// A track id the graph does not hold silently monitors nothing, the way an unknown plugin id
    /// silently plays nothing — a document and a device disagreeing is not a reason to stop the
    /// audio thread.
    pub fn set_monitor(&mut self, monitor: Option<(Arc<crate::monitor::MonitorRing>, TrackId)>) {
        self.monitor = monitor.and_then(|(ring, id)| {
            let track = self.tracks.iter().position(|track| track.id == id)?;
            Some(MonitorTap { ring, track })
        });
    }

    /// Rate this graph was prepared for.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Largest block the graph's plugins and scratch buffers are sized for.
    ///
    /// The renderer splits anything longer into chunks of at most this many frames.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Channel count of the internal mix bus.
    pub fn channel_count(&self) -> usize {
        RENDER_CHANNELS
    }

    /// Number of tracks, which always matches the project's track count.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// The tracks, in project order.
    pub fn tracks(&self) -> &[RenderTrack] {
        &self.tracks
    }

    /// The tracks, mutably.
    pub fn tracks_mut(&mut self) -> &mut [RenderTrack] {
        &mut self.tracks
    }

    /// One track by index.
    pub fn track(&self, index: usize) -> Option<&RenderTrack> {
        self.tracks.get(index)
    }

    /// One track by index, mutably.
    pub fn track_mut(&mut self, index: usize) -> Option<&mut RenderTrack> {
        self.tracks.get_mut(index)
    }

    /// The master bus strip.
    pub fn master(&self) -> &RenderStrip {
        &self.master
    }

    /// The master bus strip, mutably.
    pub fn master_mut(&mut self) -> &mut RenderStrip {
        &mut self.master
    }

    /// The tempo map positions were flattened against.
    pub fn tempo_map(&self) -> &TempoMap {
        &self.tempo_map
    }

    /// Tempo in effect at an absolute frame position.
    pub fn bpm_at_frame(&self, frame: u64) -> f64 {
        let tick = self
            .tempo_map
            .samples_to_ticks(Samples(frame), self.sample_rate);
        self.tempo_map.bpm_at(tick)
    }

    /// How many parameters this graph is driving from a lane.
    pub fn automated_count(&self) -> usize {
        self.automation.len()
    }

    /// Drives every automated parameter to the value its lane holds at `frame`.
    ///
    /// Called once per rendered segment rather than once per sample. That is not a compromise for
    /// the faders — a gain and a pan are both *targets* that the strip ramps across the block it
    /// is given, so a segment-rate write comes out as a continuous slope. It is the honest rate
    /// for a plugin parameter, which has nowhere finer to put one.
    ///
    /// `frames` is how long the segment about to be rendered is, which is how the next call knows
    /// whether the playhead carried on or jumped. Arriving somewhere puts the values there
    /// outright; carrying on ramps to them.
    ///
    /// Allocation-free and lock-free: the lanes were resolved to positions when the graph was
    /// built, and what happens here is a binary search per lane and a store.
    pub(crate) fn apply_automation(&mut self, frame: u64, frames: usize) {
        if self.automation.is_empty() {
            return;
        }
        let continuing = self.automation_from == Some(frame);
        self.automation_from = Some(frame + frames as u64);
        let tick = self
            .tempo_map
            .samples_to_ticks(Samples(frame), self.sample_rate);
        // Destructured rather than walked through `self`, so the lanes can be read while the
        // tracks they point at are written.
        let Self {
            tracks,
            master,
            automation,
            ..
        } = self;
        drive_automation(automation, tracks, master, tick, continuing);
    }

    /// Post-fader peak of a track's last rendered block.
    pub fn track_peak(&self, index: usize) -> f32 {
        self.tracks.get(index).map_or(0.0, |track| track.peak)
    }

    /// Peak of one master channel over the last rendered block.
    pub fn master_channel_peak(&self, channel: usize) -> f32 {
        self.master_peak.get(channel).copied().unwrap_or(0.0)
    }

    /// Loudest master channel over the last rendered block.
    pub fn master_peak(&self) -> f32 {
        self.master_peak[0].max(self.master_peak[1])
    }

    /// How long the graph keeps producing sound after its last event, in frames.
    ///
    /// The longest path through the routing decides it. Tracks run in parallel, so the longest of
    /// them wins; a chain runs in series with whatever it feeds, so a track's tail, its bus's tail
    /// and the master's tail add up rather than overlapping — the bus is still being fed for the
    /// whole of the track's decay, and the master for the whole of the bus's.
    ///
    /// The offline renderer keeps going for this long past the end of the arrangement so that
    /// reverbs and delays are not chopped off in the exported file.
    pub fn tail_frames(&self) -> usize {
        let master = self.master.tail_frames();
        let through = longest_paths(
            &self.tracks,
            &self.bus_tracks,
            &self.order,
            RenderStrip::tail_frames,
            master,
        );
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| !matches!(track.source, RenderSource::Bus { .. }))
            .map(|(index, _)| through[index])
            .max()
            .unwrap_or(master)
    }

    /// How far behind the playhead this graph's output runs, in frames.
    ///
    /// Every source is held back to the longest path through the routing, and that path's own
    /// length is this. Playback simply arrives this late; an export renders the extra frames and
    /// drops them, so the file still lines up with the timeline.
    pub fn latency_frames(&self) -> usize {
        self.latency
    }

    /// `true` when the delay lines no longer match what the chains need.
    ///
    /// Compared strip by strip rather than as one total, because two plugins can trade latency
    /// between them and leave the total where it was while the tracks fall out of step with each
    /// other. Nothing but a parameter moving a plugin's latency — the limiter's lookahead is the
    /// only one that ships — can cause this, since every other way of changing a chain rebuilds
    /// the graph outright.
    ///
    /// Allocation-free, because the audio thread asks it once per callback.
    pub fn latency_is_stale(&self) -> bool {
        let now = self
            .tracks
            .iter()
            .map(|track| track.strip.latency_frames())
            .chain(std::iter::once(self.master.latency_frames()));
        self.built_latencies.iter().copied().ne(now)
    }

    /// Last frame any scheduled event or audio clip touches.
    pub fn end_frame(&self) -> u64 {
        self.tracks
            .iter()
            .map(|track| match &track.source {
                RenderSource::Instrument { events, .. } => {
                    events.last().map_or(0, |event| event.frame)
                }
                RenderSource::Audio { clips } => clips
                    .iter()
                    .map(|clip| clip.start_frame + clip.length)
                    .max()
                    .unwrap_or(0),
                // A bus has nothing of its own, so it can never be what makes a project longer.
                RenderSource::Bus { .. } | RenderSource::Silence => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// Moves a track's fader.
    pub fn set_track_gain_db(&mut self, index: usize, gain_db: f32) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.strip.set_gain_db(gain_db);
        }
    }

    /// Moves a track's pan control.
    pub fn set_track_pan(&mut self, index: usize, pan: f32) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.strip.set_pan(pan);
        }
    }

    /// Toggles a track's mute.
    pub fn set_track_mute(&mut self, index: usize, mute: bool) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.strip.set_mute(mute);
        }
    }

    /// Moves one of a track's send levels. Out-of-range indices are ignored.
    ///
    /// Addressed by position in the track's send list, the same way an effect is addressed by
    /// position in its chain: a send the graph could not resolve keeps no slot, so this is the one
    /// place where a document position and a graph position can disagree — which is why adding or
    /// removing a send rebuilds the graph rather than sending a command.
    pub fn set_send_level_db(&mut self, track: usize, send: usize, level_db: f32) {
        if let Some(send) = self
            .tracks
            .get_mut(track)
            .and_then(|track| track.sends.get_mut(send))
        {
            send.gain.set_target(db_to_gain(level_db));
        }
    }

    /// Moves the master fader. `gain_db` is in decibels.
    pub fn set_master_gain_db(&mut self, gain_db: f32) {
        self.master.set_gain_db(gain_db);
    }

    /// Sets the master bus pan.
    pub fn set_master_pan(&mut self, pan: f32) {
        self.master.set_pan(pan);
    }

    /// Writes an effect parameter on a track, or on the master bus when `track` is `None`.
    pub fn set_effect_param(
        &mut self,
        track: Option<usize>,
        slot: usize,
        param: ParamId,
        value: f32,
    ) {
        match track {
            Some(index) => {
                if let Some(track) = self.tracks.get_mut(index) {
                    track.strip.set_effect_param(slot, param, value);
                }
            }
            None => self.master.set_effect_param(slot, param, value),
        }
    }

    /// Writes a parameter on a track's instrument.
    pub fn set_instrument_param(&mut self, track: usize, param: ParamId, value: f32) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.set_instrument_param(param, value);
        }
    }

    /// Queues an audition note on a track.
    pub fn note_on(&mut self, track: usize, pitch: u8, velocity: f32) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.note_on(pitch, velocity);
        }
    }

    /// Queues an audition note release on a track.
    pub fn note_off(&mut self, track: usize, pitch: u8) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.note_off(pitch);
        }
    }

    /// Queues a bend of a track's instrument.
    pub fn pitch_bend(&mut self, track: usize, semitones: f32) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.pitch_bend(semitones);
        }
    }

    /// Queues a move of a track's modulation wheel.
    pub fn modulation(&mut self, track: usize, amount: f32) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.modulation(amount);
        }
    }

    /// Drops every sounding voice without touching effect tails.
    ///
    /// This is what a stop or a seek does: notes must not hang, but a reverb should keep ringing.
    ///
    /// The click is left alone here, alongside the tails and for the same reason: forty
    /// milliseconds of it may be in the air, and cutting that short is a step to silence — the one
    /// noise a click exists not to make.
    pub fn reset_voices(&mut self) {
        for track in &mut self.tracks {
            track.reset_voices();
        }
    }

    /// Whether the click is heard.
    pub fn metronome_enabled(&self) -> bool {
        self.metronome.is_enabled()
    }

    /// Turns the click on or off without rebuilding.
    ///
    /// A rebuild would carry it too — the document holds the switch — but a rebuild instantiates
    /// every plugin in the project, and this is a button somebody presses mid-take.
    pub fn set_metronome(&mut self, enabled: bool) {
        self.metronome.set_enabled(enabled);
    }

    /// Silences everything: voices, delay lines and filter memory.
    ///
    /// Notes stay off until the next note-on rather than being chased back, because a panic that
    /// immediately restored what it had just killed would be useless.
    pub fn panic(&mut self) {
        self.metronome.reset();
        for track in &mut self.tracks {
            track.silence_voices();
            track.strip.reset();
            track.delay.reset();
            track.output_delay.reset();
            for send in &mut track.sends {
                send.delay.reset();
            }
            track.peak = 0.0;
        }
        for input in &mut self.bus_inputs {
            input.clear();
        }
        self.master.reset();
        self.master_peak = [0.0, 0.0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use auris_core::project::Note;
    use auris_core::time::Ticks;

    /// The smallest project worth building: one instrument track carrying one quarter note.
    ///
    /// Shared with the sibling modules' tests, which is why it lives up here rather than beside
    /// any one of them.
    pub(super) fn quarter_note_project() -> Project {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes
            .push(Note::new(60, Ticks::QUARTER, Ticks::QUARTER));
        project
    }

    #[test]
    fn the_master_tail_follows_the_longest_track_tail_rather_than_overlapping_it() {
        // Tracks are parallel, so the two of them together still only reach one tail's worth —
        // but the master runs after both, so its tail starts where theirs ends.
        let mut project = Project::new("Tails", 48_000.0);
        let a = project.add_instrument_track("A", testkit::TONE_ID);
        let b = project.add_instrument_track("B", testkit::TONE_ID);
        project.add_effect(Some(a), testkit::TAIL_ID);
        project.add_effect(Some(b), testkit::TAIL_ID);
        project.add_effect(None, testkit::TAIL_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.tail_frames(), 2 * testkit::TAIL_FRAMES);
    }

    #[test]
    fn a_nonsense_tail_saturates_instead_of_overflowing() {
        // `tail_frames` is a figure a plugin chooses, so two greedy ones must not wrap the sum
        // round to a short export.
        let mut project = Project::new("Tails", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::HUGE_TAIL_ID);
        project.add_effect(Some(track), testkit::HUGE_TAIL_ID);
        project.add_effect(None, testkit::HUGE_TAIL_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.tracks()[0].strip().tail_frames(), usize::MAX);
        assert_eq!(graph.tail_frames(), usize::MAX);
    }

    #[test]
    fn saved_plugin_state_reaches_the_instance() {
        let mut project = quarter_note_project();
        let track_id = project.tracks[0].id;
        project.tracks[0]
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state
            .params
            .insert("amplitude".into(), 0.25);
        let slot = project
            .add_effect(Some(track_id), testkit::GAIN_ID)
            .unwrap();
        let effect = project.tracks[0]
            .mixer
            .effects
            .iter_mut()
            .find(|e| e.id == slot)
            .unwrap();
        effect.state.params.insert("gain".into(), 3.0);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { instrument, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        assert_eq!(instrument.param(ParamId(0)), 0.25);
        assert_eq!(
            graph.tracks()[0].strip().effects[0].param(ParamId(0)),
            3.0,
            "a saved effect parameter must survive the rebuild"
        );
    }

    #[test]
    fn an_instrument_the_caller_placed_beats_the_registry_and_is_taken() {
        // The registry could build this track's instrument perfectly well. The caller's own has
        // to win anyway: for a hosted plugin the registry's answer would be a different instance
        // with none of the state, and there would be no way to tell from the sound.
        let project = quarter_note_project();
        let track = project.tracks[0].id;
        let mut instruments = PlacedInstruments::new();
        instruments.insert(
            track,
            testkit::registry()
                .create_instrument(testkit::TONE_ID)
                .unwrap(),
        );
        // A value the registry's own default would not have.
        instruments
            .get_mut(&track)
            .unwrap()
            .set_param(ParamId(0), 0.125);

        let graph = RenderGraph::build_with(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut PlacedEffects::new(),
            &mut instruments,
            512,
            48_000.0,
        );

        let RenderSource::Instrument { instrument, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        assert_eq!(instrument.param(ParamId(0)), 0.125);
        assert!(
            instruments.is_empty(),
            "building takes what it uses, so what is left names tracks the project has dropped"
        );
    }

    #[test]
    fn an_instrument_placed_for_a_track_that_is_gone_is_left_behind() {
        let project = Project::new("Graph", 48_000.0);
        let mut instruments = PlacedInstruments::new();
        instruments.insert(
            TrackId(999),
            testkit::registry()
                .create_instrument(testkit::TONE_ID)
                .unwrap(),
        );

        RenderGraph::build_with(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut PlacedEffects::new(),
            &mut instruments,
            512,
            48_000.0,
        );

        assert_eq!(
            instruments.len(),
            1,
            "the caller has to be told, not guessed at"
        );
    }

    #[test]
    fn a_placed_instrument_is_still_given_the_documents_parameters() {
        // The plugin arrives carrying whatever it had; the document is what the user edited. The
        // document wins, exactly as it does for one the registry built.
        let mut project = quarter_note_project();
        let track = project.tracks[0].id;
        project.tracks[0]
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state
            .params
            .insert("amplitude".into(), 0.5);

        let mut instruments = PlacedInstruments::new();
        instruments.insert(
            track,
            testkit::registry()
                .create_instrument(testkit::TONE_ID)
                .unwrap(),
        );

        let graph = RenderGraph::build_with(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut PlacedEffects::new(),
            &mut instruments,
            512,
            48_000.0,
        );

        let RenderSource::Instrument { instrument, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        assert_eq!(instrument.param(ParamId(0)), 0.5);
    }

    #[test]
    fn a_nonsense_sample_rate_falls_back_instead_of_poisoning_the_graph() {
        let mut project = quarter_note_project();
        project.sample_rate = 0.0;
        let graph = RenderGraph::build_at(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            512,
            f64::NAN,
        );
        assert_eq!(graph.sample_rate(), DEFAULT_SAMPLE_RATE);
        assert!(graph.sample_rate().is_finite());
    }

    /// A track whose notes all start at tick 0 and last `length`.
    fn stacked_note_project(pitches: std::ops::Range<u8>, length: Ticks) -> Project {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Chords", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Stack", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        for pitch in pitches {
            midi.notes.push(Note::new(pitch, Ticks::ZERO, length));
        }
        project
    }

    #[test]
    fn the_event_scratch_is_sized_for_the_densest_block() {
        // Six note-ons on frame 0; their releases are a quarter note away, so no 512-frame
        // window ever holds more than six events — and six is also the most that can be sounding
        // at once, so the chase needs room for six and the all-notes-off before them.
        let sparse = stacked_note_project(60..66, Ticks::QUARTER);
        let graph = RenderGraph::build(&sparse, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(
            graph.tracks()[0].block_events.capacity(),
            6 + AUDITION_HEADROOM + 6 + CHASE_HEADROOM
        );

        // Every pitch struck and released inside one block: 256 events in a single window, twice
        // the 128 that are ever sounding together, so the density term is what sizes the buffer.
        let dense = stacked_note_project(0..128, Ticks(1));
        let graph = RenderGraph::build(&dense, &AudioSourceBank::new(), &testkit::registry(), 512);
        let expected = 256 + AUDITION_HEADROOM + 128 + CHASE_HEADROOM;
        assert_eq!(graph.tracks()[0].block_events.capacity(), expected);
    }

    #[test]
    fn the_chase_headroom_counts_overlapping_notes_on_one_pitch() {
        // Four notes on the same pitch, each starting before the last has ended: all four are
        // sounding together at the third strike, so a seek there has to re-issue four voices.
        let mut project = Project::new("Overlap", 48_000.0);
        let track = project.add_instrument_track("Pedal", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(16.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        for index in 0..4 {
            midi.notes.push(Note::new(
                60,
                Ticks::from_beats(index as f64),
                Ticks::from_beats(8.0),
            ));
        }

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        // One note-on per block at most, so density contributes 1.
        assert_eq!(
            graph.tracks()[0].block_events.capacity(),
            1 + AUDITION_HEADROOM + 4 + CHASE_HEADROOM
        );
    }

    #[test]
    fn the_graph_can_be_moved_to_the_audio_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<RenderGraph>();
        assert_send::<crate::EngineCommand>();
    }
}
