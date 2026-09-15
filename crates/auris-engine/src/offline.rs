//! Faster-than-realtime rendering of a whole project into memory or a bounded block sink.
//!
//! Export runs the *same* [`render_block`] the audio callback does, block by block, with the
//! transport rolling. Cycle exports warm up the graph before capturing one repetition.
//! That is what makes an exported file match what
//! was heard: there is no second code path that could drift from the first.

use auris_core::AudioBuffer;
use auris_core::project::{AudioSourceBank, Project};
use auris_core::registry::PluginRegistry;

use crate::error::EngineError;
use crate::graph::{RENDER_CHANNELS, RenderGraph};
use crate::renderer::render_block;
use crate::transport::Transport;

/// Failure from either the render graph or the consumer of a streamed render block.
///
/// Keeping the two errors distinct lets a file exporter report a codec or filesystem failure
/// without teaching the engine about file formats.
#[derive(Debug, thiserror::Error)]
pub enum OfflineStreamError<E> {
    /// Rendering the next block failed or was cancelled.
    #[error(transparent)]
    Render(#[from] EngineError),
    /// The caller could not consume a completed output block.
    #[error("offline render sink failed: {0}")]
    Sink(E),
}

/// Longest span an offline render will attempt, in frames: twenty-four hours at 192 kHz.
///
/// This is a sanity bound, not a quota. Nothing a user can arrange comes anywhere near it, so
/// anything that does is a corrupt figure — and turning that into an error is what keeps the
/// export path from panicking in the allocator on a number it was handed rather than chose.
const MAX_RENDER_FRAMES: u64 = 24 * 60 * 60 * 192_000;

/// Raw `f32` sample storage a complete in-memory render may retain: 512 MiB.
///
/// Long file exports use [`OfflineRender::render_streamed`] and do not consume this budget.
const MAX_BUFFERED_RENDER_BYTES: usize = 512 * 1024 * 1024;

/// Largest block whose event offsets and scratch allocation remain practical and representable.
const MAX_BLOCK_FRAMES: usize = 1_048_576;

fn buffered_render_bytes(frames: usize) -> Result<usize, EngineError> {
    let too_large = || EngineError::RenderBufferTooLarge {
        frames,
        channels: RENDER_CHANNELS,
        limit_bytes: MAX_BUFFERED_RENDER_BYTES,
    };
    let bytes = frames
        .checked_mul(RENDER_CHANNELS)
        .and_then(|samples| samples.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(too_large)?;
    if bytes > MAX_BUFFERED_RENDER_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

fn allocate_render_buffer(frames: usize, sample_rate: f64) -> Result<AudioBuffer, EngineError> {
    buffered_render_bytes(frames)?;
    let allocation_error = || EngineError::RenderBufferAllocation {
        frames,
        channels: RENDER_CHANNELS,
    };
    let mut channels = Vec::new();
    channels
        .try_reserve_exact(RENDER_CHANNELS)
        .map_err(|_| allocation_error())?;
    for _ in 0..RENDER_CHANNELS {
        let mut channel = Vec::new();
        channel
            .try_reserve_exact(frames)
            .map_err(|_| allocation_error())?;
        channel.resize(frames, 0.0);
        channels.push(channel);
    }
    AudioBuffer::from_planar(channels, sample_rate).map_err(EngineError::from)
}

/// How much of a project to render, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflineOptions {
    /// First frame to render.
    pub start_frames: u64,
    /// One past the last frame to render. `None` means the end of the arrangement.
    pub end_frames: Option<u64>,
    /// Frames per processing block. Larger blocks export faster; the output is identical.
    pub block_frames: usize,
    /// Whether to keep rendering past the end for the longest effect tail in the graph.
    pub include_tail: bool,
    /// Capture one cycle after warming up instruments and effects for at least one cycle
    /// and the graph's reported tail duration. No tail is appended to the output.
    pub looping: bool,
    /// Rate to render at. `None` uses the project's own rate.
    pub sample_rate: Option<f64>,
}

impl Default for OfflineOptions {
    fn default() -> Self {
        Self {
            start_frames: 0,
            end_frames: None,
            block_frames: 1_024,
            include_tail: true,
            looping: false,
            sample_rate: None,
        }
    }
}

impl OfflineOptions {
    /// Options rendering the whole arrangement plus its tails.
    pub fn whole_project() -> Self {
        Self::default()
    }

    /// Restricts the render to a frame range.
    pub fn with_range(mut self, start_frames: u64, end_frames: u64) -> Self {
        self.start_frames = start_frames;
        self.end_frames = Some(end_frames);
        self
    }

    /// Sets the processing block size.
    pub fn with_block_frames(mut self, block_frames: usize) -> Self {
        self.block_frames = block_frames.clamp(1, MAX_BLOCK_FRAMES);
        self
    }
}

/// A render's two-way channel with whoever asked for it: how far it has got, and whether to stop.
///
/// The two travel together because they are read in the same place — once per block, at the
/// bottom of the loop — and because a caller that wants either almost always wants both. An
/// export with a progress bar and no way out is a window somebody has to wait in front of, and
/// there is no other moment at which a render can be interrupted: a block is short, and stopping
/// between two of them costs nothing.
///
/// Both halves are optional. [`Default`] is a render nobody is watching and nobody can stop,
/// which is what [`render_project`] and every test wants.
#[derive(Default)]
pub struct RenderProgress<'a> {
    report: Option<&'a mut dyn FnMut(f32)>,
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
    commit: Option<&'a dyn Fn() -> bool>,
    /// Where this render sits inside the job the caller is watching: a start and a width, both
    /// fractions of the whole. `(0.0, 1.0)` for a render that is the whole of what is happening.
    window: (f32, f32),
}

impl<'a> RenderProgress<'a> {
    /// A render whose progress is reported to `report`.
    pub fn reporting(report: &'a mut dyn FnMut(f32)) -> Self {
        Self {
            report: Some(report),
            cancel: None,
            commit: None,
            window: (0.0, 1.0),
        }
    }

    /// Runs `job`, whose progress covers `base..base + span` of what this is watching.
    ///
    /// What an export made of several renders needs: each of them believes it is running from
    /// nothing to everything, and a bar that jumped back to zero four times over would be a bar
    /// saying the export had restarted. Nesting works, because the window a job is given is
    /// measured inside whatever window it was already in.
    ///
    /// A scope rather than a second `RenderProgress`, because there is only one reporter and it
    /// belongs to the caller: handing out a copy would mean handing out the borrow.
    pub fn within<T>(&mut self, base: f32, span: f32, job: impl FnOnce(&mut Self) -> T) -> T {
        let outer = self.window;
        self.window = (outer.0 + base * outer.1, span * outer.1);
        let result = job(self);
        self.window = outer;
        result
    }

    /// The same render, stopped when `flag` becomes true.
    ///
    /// A flag rather than a channel or a handle: the render checks it between blocks and the
    /// caller sets it from wherever the button was pressed, and neither has to know anything
    /// about the other's thread.
    pub fn cancelled_by(mut self, flag: &'a std::sync::atomic::AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }

    /// Uses `commit` to arbitrate cancellation immediately before durable output is published.
    ///
    /// Blockwise cancellation alone leaves a narrow race after the last block: a request can be
    /// cancelled while an encoder is finalising but before its staged file is installed. A
    /// frontend with request cancellation supplies one atomic gate here. Ordinary callers omit
    /// it and retain the existing behavior.
    pub fn committing_with(mut self, commit: &'a dyn Fn() -> bool) -> Self {
        self.commit = Some(commit);
        self
    }

    /// Attempts to cross the durable-output boundary.
    ///
    /// Returns `false` if cancellation was already visible or the frontend's atomic commit gate
    /// gave cancellation priority. Once this succeeds, the frontend must ignore later
    /// cancellation and let publication finish.
    pub fn begin_commit(&self) -> bool {
        if self.is_cancelled() {
            return false;
        }
        self.commit.is_none_or(|commit| commit())
    }

    /// Reports how far along the render is, from 0.0 to 1.0 — of its own window, not of the job.
    pub fn report(&mut self, fraction: f32) {
        let (base, span) = self.window;
        if let Some(report) = self.report.as_mut() {
            report(base + fraction * span);
        }
    }

    /// Whether the render has been asked to stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// Renders a project into a single buffer.
///
/// See [`render_project_with_progress`] when the caller wants to drive a progress bar.
pub fn render_project(
    project: &Project,
    bank: &AudioSourceBank,
    registry: &PluginRegistry,
    options: &OfflineOptions,
) -> Result<AudioBuffer, EngineError> {
    render_project_with_progress(
        project,
        bank,
        registry,
        options,
        &mut RenderProgress::default(),
    )
}

/// Renders a project into a single buffer, reporting progress from 0.0 to 1.0.
///
/// `progress` is reported once before any work starts and once per block afterwards, finishing
/// at exactly 1.0. It runs on the calling thread, so a UI must marshal it as usual, and it is
/// also where a cancellation is noticed -- see [`RenderProgress`].
pub fn render_project_with_progress(
    project: &Project,
    bank: &AudioSourceBank,
    registry: &PluginRegistry,
    options: &OfflineOptions,
    progress: &mut RenderProgress<'_>,
) -> Result<AudioBuffer, EngineError> {
    render_project_using(
        project,
        bank,
        registry,
        &mut crate::graph::PlacedEffects::new(),
        &mut crate::graph::PlacedInstruments::new(),
        options,
        progress,
    )
}

/// Renders a project, taking some of its plugins from the caller.
///
/// See [`PlacedEffects`](crate::graph::PlacedEffects) and
/// [`PlacedInstruments`](crate::graph::PlacedInstruments) for why a caller would have any. An
/// export that went through [`render_project`] instead would silently leave every hosted plugin
/// out — the bounce would be the mix minus whatever somebody loaded, which is the worst way for a
/// feature to be missing.
pub fn render_project_using(
    project: &Project,
    bank: &AudioSourceBank,
    registry: &PluginRegistry,
    placed: &mut crate::graph::PlacedEffects,
    instruments: &mut crate::graph::PlacedInstruments,
    options: &OfflineOptions,
    progress: &mut RenderProgress<'_>,
) -> Result<AudioBuffer, EngineError> {
    let mut render = OfflineRender::new(project, bank, registry, placed, instruments, options)?;
    let mut out = render.buffer()?;
    render.render(&mut out, progress)?;
    Ok(out)
}

/// A built graph and the geometry of the render it was built for, playable more than once.
///
/// [`render_project_using`] is one call of this, and is what almost everything wants. What the
/// object is for is the export that needs the *same* graph played several times over — stems, one
/// per track — because building it twice is not the same thing twice. A hosted plugin is
/// instantiated when the graph is built and cannot be built again for the same slot without two
/// of it existing at once, and everything else that is expensive here (sources resolved, notes
/// flattened, chains prepared) would be paid for again on every pass.
///
/// Between passes, [`Self::set_audible`] chooses what is heard and [`Self::render`] silences
/// whatever the last pass left ringing. Nothing else carries over.
pub struct OfflineRender {
    graph: RenderGraph,
    sample_rate: f64,
    block_frames: usize,
    start_frames: u64,
    /// Frames the output holds: the range, plus the tail when one was asked for.
    total: usize,
    /// Frames to render, which is [`Self::total`] plus the compensation lead-in.
    end: usize,
    /// Frames of cycle warm-up and plugin delay compensation to discard.
    latency: usize,
    /// Whether the render has an explicit end to stop the transport at.
    ranged: bool,
    /// Frames of the output the transport is rolling for; the rest is tail.
    performed: usize,
    /// Render this range repeatedly while warming up and capturing the cycle.
    looping: bool,
}

impl OfflineRender {
    /// Builds the graph and works out the geometry of the render.
    pub fn new(
        project: &Project,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        placed: &mut crate::graph::PlacedEffects,
        instruments: &mut crate::graph::PlacedInstruments,
        options: &OfflineOptions,
    ) -> Result<Self, EngineError> {
        let sample_rate = options.sample_rate.unwrap_or(project.sample_rate);
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(EngineError::InvalidSampleRate(sample_rate));
        }
        let block_frames = options.block_frames.clamp(1, MAX_BLOCK_FRAMES);

        let mut graph = RenderGraph::build_with(
            project,
            bank,
            registry,
            placed,
            instruments,
            block_frames,
            sample_rate,
        );
        if let Some(error) = graph.take_resource_error() {
            return Err(error);
        }

        let end_frames = options.end_frames.unwrap_or_else(|| {
            // Both figures measure the same thing, but `Project::end_tick` rounds an audio clip's
            // length to whole ticks on the way through while the graph converts it to frames
            // directly, so they can land a frame or two apart. Taking whichever reaches further
            // can only ever add silence; using the tick figure alone would chop the end off a
            // clip.
            let from_ticks = project
                .tempo_map
                .ticks_to_samples(project.end_tick(), sample_rate)
                .raw();
            from_ticks.max(graph.end_frame())
        });
        if end_frames < options.start_frames {
            return Err(EngineError::InvalidRange {
                start: options.start_frames,
                end: end_frames,
            });
        }

        let tail = if options.include_tail && !options.looping {
            graph.tail_frames()
        } else {
            0
        };
        // A corrupt tempo map or a bad `end_frames` can make this span astronomically large, and
        // the buffer that follows would then panic inside the allocator rather than returning an
        // error.
        let span = end_frames - options.start_frames;
        let too_long = || EngineError::RenderTooLong {
            frames: span,
            limit: MAX_RENDER_FRAMES,
        };
        if span > MAX_RENDER_FRAMES {
            return Err(too_long());
        }
        let total = usize::try_from(span)
            .ok()
            .and_then(|span| span.checked_add(tail))
            .ok_or_else(too_long)?;

        // Plugin delay compensation holds the whole mix back, so the first `latency` frames out
        // of the graph are the lead-in of empty delay lines rather than anything on the timeline.
        // The render runs that much longer and the file starts where the lead-in ends, which is
        // what keeps an export lined up with what the arrangement shows.
        let warmup = if options.looping && span > 0 {
            let span = usize::try_from(span).map_err(|_| too_long())?;
            graph
                .tail_frames()
                .max(span)
                .div_ceil(span)
                .checked_mul(span)
                .ok_or_else(too_long)?
        } else {
            0
        };
        let latency = graph
            .latency_frames()
            .checked_add(warmup)
            .ok_or_else(too_long)?;
        let end = total.checked_add(latency).ok_or_else(too_long)?;
        if end as u64 > MAX_RENDER_FRAMES {
            return Err(too_long());
        }

        Ok(Self {
            graph,
            sample_rate,
            block_frames,
            start_frames: options.start_frames,
            total,
            end,
            latency,
            // Only an explicit range can have material lying beyond its end; a whole-project
            // render ends where the material does, and stopping it there would cut the natural
            // releases out of its own tail.
            ranged: options.end_frames.is_some() && !options.looping,
            performed: total - tail,
            looping: options.looping,
        })
    }

    /// Frames one pass produces.
    pub fn frames(&self) -> usize {
        self.total
    }

    /// The rate the render runs at, which is not always the project's.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Allocates an output buffer of the right size and rate for [`Self::render`].
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::RenderBufferTooLarge`] when retaining the complete render would
    /// exceed the in-memory budget, or [`EngineError::RenderBufferAllocation`] when the allocator
    /// refuses an otherwise bounded buffer. Use [`Self::render_streamed`] for long file exports.
    pub fn buffer(&self) -> Result<AudioBuffer, EngineError> {
        allocate_render_buffer(self.total, self.sample_rate)
    }

    /// Chooses which tracks are heard, by project position, settled rather than faded.
    ///
    /// The solo resolution rather than a mute: what a caller hands over is the answer to "which
    /// tracks lie on a path through the one I am exporting", which
    /// [`Project::solo_resolution`](auris_core::project::Project::solo_resolution) works out. A
    /// drum track's stem needs the drum bus left open, or it has nowhere to come out.
    pub fn set_audible(&mut self, audible: &[bool]) {
        self.graph.set_audible(audible);
    }

    /// Renders one pass into `out`, which must be [`Self::frames`] long.
    ///
    /// Overwrites `out` and silences the graph first, so a second pass carries nothing over from
    /// the first: no ringing reverb, no delay line still emptying, no note still sounding.
    pub fn render(
        &mut self,
        out: &mut AudioBuffer,
        progress: &mut RenderProgress<'_>,
    ) -> Result<(), EngineError> {
        buffered_render_bytes(self.total)?;
        if out.channel_count() != RENDER_CHANNELS
            || out.frame_count() != self.total
            || out.sample_rate() != self.sample_rate
        {
            return Err(auris_core::CoreError::LayoutMismatch(format!(
                "offline render needs {RENDER_CHANNELS} channels, {} frames at {} Hz; the output has {} channels, {} frames at {} Hz",
                self.total,
                self.sample_rate,
                out.channel_count(),
                out.frame_count(),
                out.sample_rate()
            ))
            .into());
        }
        out.clear();
        let mut written = 0;
        let result = self.render_streamed(progress, |block| {
            let next = written + block.frame_count();
            for channel in 0..RENDER_CHANNELS {
                out.channel_mut(channel)[written..next].copy_from_slice(block.channel(channel));
            }
            written = next;
            Ok::<(), std::convert::Infallible>(())
        });
        match result {
            Ok(()) => Ok(()),
            Err(OfflineStreamError::Render(error)) => Err(error),
            Err(OfflineStreamError::Sink(never)) => match never {},
        }
    }

    /// Renders one pass as bounded output blocks and hands each block to `sink` in order.
    ///
    /// No allocation in this method grows with [`Self::frames`]: the render scratch and the
    /// output block are each at most the configured processing block size. This is the path for
    /// long file exports. [`Self::render`] remains available when a caller genuinely needs the
    /// complete audio in memory.
    ///
    /// The graph is silenced before the pass, exactly as it is for [`Self::render`]. Blocks cover
    /// the requested output with latency lead-in removed; their frame counts sum to
    /// [`Self::frames`]. An empty render calls no sink and still completes progress.
    pub fn render_streamed<E>(
        &mut self,
        progress: &mut RenderProgress<'_>,
        mut sink: impl FnMut(&AudioBuffer) -> Result<(), E>,
    ) -> Result<(), OfflineStreamError<E>> {
        self.graph.panic();
        progress.report(0.0);
        if self.total == 0 {
            progress.report(1.0);
            return Ok(());
        }

        let mut transport = Transport::playing_from(self.start_frames);
        if self.looping {
            transport.set_loop(
                true,
                self.start_frames,
                self.start_frames + self.performed as u64,
            );
        }
        let mut scratch = AudioBuffer::new(RENDER_CHANNELS, self.block_frames, self.sample_rate);
        let mut output = AudioBuffer::new(RENDER_CHANNELS, self.block_frames, self.sample_rate);
        let mut rendered = 0;
        let mut emitted = 0;
        while rendered < self.end {
            // The range's end is a Stop, exactly as realtime playback stops there: the voices are
            // released and what runs on into the tail is the effects' ring-out — not the material
            // that lies beyond the range, which used to keep performing straight through it.
            if self.ranged && transport.playing && rendered >= self.performed {
                transport.playing = false;
                self.graph.reset_voices();
            }
            let mut frames = self.block_frames.min(self.end - rendered);
            if self.ranged && transport.playing {
                // Never render across the boundary; the block after this one starts stopped.
                frames = frames.min(self.performed - rendered);
            }
            scratch.set_frame_count(frames);
            render_block(&mut self.graph, &mut transport, &mut scratch, true);
            // How much of this block is still lead-in, and where the rest lands in the file.
            let skip = self.latency.saturating_sub(rendered).min(frames);
            if skip < frames {
                let count = (frames - skip).min(self.total - emitted);
                output.set_frame_count(count);
                for channel in 0..RENDER_CHANNELS {
                    output
                        .channel_mut(channel)
                        .copy_from_slice(&scratch.channel(channel)[skip..skip + count]);
                }
                sink(&output).map_err(OfflineStreamError::Sink)?;
                emitted += count;
            }
            rendered += frames;
            progress.report(rendered as f32 / self.end as f32);
            // Between blocks, which is the only place a render can be interrupted — and cheap
            // enough that the check costs nothing next to the block that just ran.
            if progress.is_cancelled() {
                return Err(OfflineStreamError::Render(EngineError::RenderCancelled));
            }
        }
        debug_assert_eq!(emitted, self.total);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_exports_keep_the_warm_tail_and_exact_length_at_any_block_size() {
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        for delayed in [false, true] {
            if delayed {
                project.add_effect(None, testkit::LOOKAHEAD_ID);
            }
            for block_frames in [1, 64, 3_000] {
                // A cycle shorter than the tail also exercises multiple warm-up passes.
                let rendered = render_project(
                    &project,
                    &AudioSourceBank::new(),
                    &testkit::registry(),
                    &OfflineOptions {
                        start_frames: 24_000,
                        end_frames: Some(24_100),
                        looping: true,
                        block_frames,
                        ..OfflineOptions::default()
                    },
                )
                .unwrap();
                assert_eq!(rendered.frame_count(), 100);
                // y[n] = x[n] + 0.5*y[n-1] reaches twice the input amplitude.
                // The first sample must already contain the preceding cycle's tail.
                for sample in rendered.channel(0) {
                    assert!(
                        (sample - 2.0 * TONE_AMPLITUDE).abs() < 1e-5,
                        "cold or interrupted cycle: {sample}, block {block_frames}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_empty_cycle_exports_no_tail() {
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions {
                end_frames: Some(0),
                looping: true,
                ..OfflineOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered.frame_count(), 0);
    }

    #[test]
    fn a_cycle_keeps_sample_positions_with_latency_and_a_tail() {
        let mut project = Project::new("Pulse", SAMPLE_RATE);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "pulse",
            auris_core::AssetPath::inside("Audio/pulse.wav"),
            100,
            SAMPLE_RATE,
            2,
        );
        project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let mut pulse = vec![0.0; 100];
        pulse[99] = 0.5;
        let mut bank = AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(AudioBuffer::from_planar(vec![pulse.clone(), pulse], SAMPLE_RATE).unwrap()),
        );
        project.add_effect(None, testkit::TAIL_ID);
        project.add_effect(None, testkit::LOOKAHEAD_ID);
        for block_frames in [7, 1024] {
            let rendered = render_project(
                &project,
                &bank,
                &testkit::registry(),
                &OfflineOptions {
                    end_frames: Some(100),
                    looping: true,
                    block_frames,
                    ..OfflineOptions::default()
                },
            )
            .unwrap();
            assert_eq!(rendered.frame_count(), 100);
            assert!((rendered.channel(0)[0] - 0.25).abs() < 1e-5);
            assert!((rendered.channel(0)[1] - 0.125).abs() < 1e-5);
            assert!((rendered.channel(0)[99] - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn oversized_blocks_are_clamped_before_event_offsets_are_narrowed() {
        assert_eq!(
            OfflineOptions::default()
                .with_block_frames(usize::MAX)
                .block_frames,
            MAX_BLOCK_FRAMES
        );
    }
    use crate::testkit::{self, TAIL_FRAMES, TONE_AMPLITUDE};
    use auris_core::AudioBuffer;
    use auris_core::project::Note;
    use auris_core::time::Ticks;
    use std::sync::Arc;

    const SAMPLE_RATE: f64 = 48_000.0;

    #[test]
    fn aggregate_audio_windows_make_an_offline_render_fail() {
        let mut project = Project::new("Audio aggregate", SAMPLE_RATE);
        let track = project.add_audio_track("Loops");
        let source = project.add_audio_source(
            "one frame",
            auris_core::AssetPath::inside("Audio/one.wav"),
            1,
            SAMPLE_RATE,
            2,
        );
        for _ in 0..7 {
            let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
            project.audio_clip_mut(clip).unwrap().loop_end = Ticks(16_384);
        }
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1, SAMPLE_RATE)));

        let error = render_project(
            &project,
            &bank,
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, 1),
        )
        .expect_err("an incomplete export must not be reported as successful");

        assert!(matches!(
            error,
            EngineError::ProjectAudioScheduleTooLarge {
                windows: 114_688,
                ..
            }
        ));
    }

    #[test]
    fn aggregate_midi_events_make_an_offline_render_fail() {
        let mut project = Project::new("MIDI aggregate", SAMPLE_RATE);
        for index in 0..5 {
            let track = project.add_instrument_track(format!("Part {index}"), testkit::TONE_ID);
            let clip = project
                .add_midi_clip(track, "Dense", Ticks::ZERO, Ticks(1))
                .unwrap();
            let clip = project.midi_clip_mut(clip).unwrap();
            clip.notes = (0..1_000)
                .map(|_| Note::new(60, Ticks::ZERO, Ticks(1)))
                .collect();
            clip.loop_end = Ticks(401);
        }

        let error = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, 1),
        )
        .expect_err("an incomplete export must not be reported as successful");

        assert!(matches!(
            error,
            EngineError::ProjectScheduleTooLarge {
                events: 4_010_000,
                ..
            }
        ));
    }

    /// Four beats of held note at 120 BPM: 96 000 frames.
    fn four_beat_project() -> Project {
        let mut project = Project::new("Export", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(4.0),
        ));
        project
    }

    #[test]
    fn the_output_is_the_arrangement_length_when_nothing_has_a_tail() {
        let project = four_beat_project();
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000);
        assert_eq!(rendered.sample_rate(), SAMPLE_RATE);
        assert!((rendered.channel(0)[95_999] - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn the_same_render_played_twice_produces_the_same_samples() {
        // What a stem export rests on. A second pass over one graph has to begin where the first
        // one did and with nothing left ringing, or every stem after the first would carry the
        // tail of the one before it.
        let project = four_beat_project();
        let mut render = OfflineRender::new(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut crate::graph::PlacedEffects::new(),
            &mut crate::graph::PlacedInstruments::new(),
            &OfflineOptions::default(),
        )
        .expect("a render");

        let mut first = render.buffer().expect("the first output buffer");
        render
            .render(&mut first, &mut RenderProgress::default())
            .expect("the first pass");
        let mut again = render.buffer().expect("the second output buffer");
        render
            .render(&mut again, &mut RenderProgress::default())
            .expect("the second pass");
        assert_eq!(first.channel(0), again.channel(0));
        assert!(first.peak() > 0.0, "the passes agreed on silence");
    }

    #[test]
    fn a_track_left_out_of_the_audible_list_is_not_in_the_render() {
        let project = four_beat_project();
        let mut render = OfflineRender::new(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut crate::graph::PlacedEffects::new(),
            &mut crate::graph::PlacedInstruments::new(),
            &OfflineOptions::default(),
        )
        .expect("a render");

        let mut out = render.buffer().expect("an output buffer");
        render.set_audible(&[false]);
        render
            .render(&mut out, &mut RenderProgress::default())
            .expect("render");
        assert_eq!(out.peak(), 0.0, "a silenced track was still heard");

        // And back again, from the first frame rather than fading in over it.
        render.set_audible(&[true]);
        render
            .render(&mut out, &mut RenderProgress::default())
            .expect("render");
        assert!(
            (out.channel(0)[0].abs() - TONE_AMPLITUDE).abs() < 1e-5,
            "the pass began with a fade on it: {}",
            out.channel(0)[0]
        );
    }

    #[test]
    fn progress_inside_a_window_is_reported_against_the_whole_job() {
        let mut seen = Vec::new();
        let mut report = |fraction: f32| seen.push(fraction);
        let mut progress = RenderProgress::reporting(&mut report);

        progress.within(0.0, 0.5, |progress| progress.report(1.0));
        progress.within(0.5, 0.5, |progress| {
            progress.report(0.0);
            // Nested, because a job made of jobs is still one bar.
            progress.within(0.5, 0.5, |progress| progress.report(1.0));
        });
        // And the window is put back afterwards.
        progress.report(1.0);
        assert_eq!(seen, vec![0.5, 0.5, 1.0, 1.0]);
    }

    #[test]
    fn the_longest_effect_tail_is_appended() {
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000 + TAIL_FRAMES);
        // The tail must actually contain the ring-out, not silence.
        assert!(rendered.slice(96_000, TAIL_FRAMES).peak() > 0.0);

        let without = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions {
                include_tail: false,
                ..OfflineOptions::default()
            },
        )
        .expect("render");
        assert_eq!(without.frame_count(), 96_000);
    }

    #[test]
    fn a_range_export_does_not_perform_what_lies_beyond_the_range() {
        // A note that starts after the exported range, in a graph with a tail. The tail must
        // hold the ring-out of what was *in* the range — realtime playback of the same span
        // followed by Stop releases the voices and lets only the effects ring — and it used to
        // hold a full-level performance of material past the range instead, because nothing
        // ever told the transport the range had ended.
        let mut project = Project::new("Range", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(
                track,
                "Late",
                Ticks::from_beats(4.0),
                Ticks::from_beats(4.0),
            )
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(4.0),
        ));
        project.add_effect(None, testkit::TAIL_ID);

        // Bars one to four hold nothing; the note begins at bar five, past the range.
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, 96_000),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000 + TAIL_FRAMES);
        assert!(
            rendered.slice(96_000, TAIL_FRAMES).peak() < TONE_AMPLITUDE * 0.01,
            "the tail performed material that lies beyond the range"
        );
    }

    #[test]
    fn a_tail_behind_another_tail_lengthens_the_export_by_both() {
        // The master chain is fed for the whole of the track's ring-out and only then starts its
        // own, so the export needs room for the two end to end.
        let mut project = four_beat_project();
        let track = project.tracks[0].id;
        project.add_effect(Some(track), testkit::TAIL_ID);
        project.add_effect(None, testkit::TAIL_ID);

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000 + 2 * TAIL_FRAMES);
    }

    #[test]
    fn a_bypassed_effect_does_not_lengthen_the_export() {
        // A bypassed slot is never handed a block, so it cannot ring out; counting its declared
        // tail would pad every export with silence. This is also what makes the placeholder for
        // a missing plugin harmless, since that one is bypassed by construction.
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        project.master.effects[0].enabled = false;

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 96_000);
    }

    #[test]
    fn the_export_matches_block_by_block_realtime_rendering() {
        let mut project = four_beat_project();
        project.add_effect(None, testkit::TAIL_ID);
        project.master.gain_db = -3.0;
        project.tracks[0].mixer.pan = -0.4;
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();

        let options = OfflineOptions::default().with_block_frames(512);
        let exported =
            render_project(&project, &bank, &registry, &options).expect("offline render");

        // The same project driven the way the audio callback drives it.
        let mut graph = RenderGraph::build_at(&project, &bank, &registry, 512, SAMPLE_RATE);
        let mut transport = Transport::playing_from(0);
        let mut block = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        let mut realtime = AudioBuffer::new(RENDER_CHANNELS, exported.frame_count(), SAMPLE_RATE);
        let mut written = 0;
        while written < realtime.frame_count() {
            let frames = 512.min(realtime.frame_count() - written);
            block.set_frame_count(frames);
            render_block(&mut graph, &mut transport, &mut block, false);
            for channel in 0..RENDER_CHANNELS {
                realtime.channel_mut(channel)[written..written + frames]
                    .copy_from_slice(&block.channel(channel)[..frames]);
            }
            written += frames;
        }

        assert_eq!(exported.channel(0), realtime.channel(0));
        assert_eq!(exported.channel(1), realtime.channel(1));
    }

    #[test]
    fn an_export_still_starts_on_the_timeline_when_something_looks_ahead() {
        // Compensation holds the whole mix back, so the graph's first frames are the empty delay
        // lines. Those are a lead-in, not part of the piece, and the file must not begin with
        // them — an export that did would be shifted against every other export of the project.
        let mut project = four_beat_project();
        project.add_effect(None, testkit::LOOKAHEAD_ID);

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");

        assert_eq!(rendered.frame_count(), 96_000);
        assert!(
            (rendered.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5,
            "the file starts with the compensation lead-in: {}",
            rendered.channel(0)[0]
        );
        assert!((rendered.channel(0)[95_999] - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn a_look_ahead_effect_does_not_change_what_an_export_contains() {
        // The same project with and without an effect that only delays: once compensated, the
        // exported samples have to match.
        let project = four_beat_project();
        let plain = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");

        let mut delayed_project = project.clone();
        delayed_project.add_effect(None, testkit::LOOKAHEAD_ID);
        let delayed = render_project(
            &delayed_project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");

        assert_eq!(plain.channel(0), delayed.channel(0));
    }

    #[test]
    fn the_export_block_size_does_not_change_the_result() {
        let project = four_beat_project();
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();
        let a = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions::default().with_block_frames(64),
        )
        .expect("render");
        let b = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions::default().with_block_frames(3_000),
        )
        .expect("render");
        assert_eq!(a.channel(0), b.channel(0));
    }

    #[test]
    fn an_explicit_range_renders_only_that_span() {
        let project = four_beat_project();
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(24_000, 36_000),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 12_000);
        assert!((rendered.peak() - TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn an_explicit_range_matches_realtime_seeking_to_the_same_place() {
        // Two overlapping notes, so the range starts inside one of them and the second joins
        // partway through: the render is only right if the note chase is.
        let mut project = Project::new("Range", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes
            .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(4.0)));
        midi.notes.push(Note::new(
            64,
            Ticks::from_beats(2.0),
            Ticks::from_beats(4.0),
        ));
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();

        let exported = render_project(
            &project,
            &bank,
            &registry,
            &OfflineOptions::default()
                .with_range(30_000, 70_000)
                .with_block_frames(512),
        )
        .expect("render");

        let mut graph = RenderGraph::build(&project, &bank, &registry, 512);
        let mut transport = Transport::playing_from(30_000);
        let mut block = AudioBuffer::new(RENDER_CHANNELS, 512, SAMPLE_RATE);
        let mut realtime = AudioBuffer::new(RENDER_CHANNELS, 40_000, SAMPLE_RATE);
        let mut written = 0;
        while written < 40_000 {
            let frames = 512.min(40_000 - written);
            block.set_frame_count(frames);
            render_block(&mut graph, &mut transport, &mut block, false);
            for channel in 0..RENDER_CHANNELS {
                realtime.channel_mut(channel)[written..written + frames]
                    .copy_from_slice(&block.channel(channel)[..frames]);
            }
            written += frames;
        }
        assert_eq!(exported.channel(0), realtime.channel(0));
        // Pitch 60 alone at the range start; pitch 64 joins at frame 48 000.
        assert!((exported.channel(0)[0] - TONE_AMPLITUDE).abs() < 1e-5);
        assert!((exported.channel(0)[20_000] - 2.0 * TONE_AMPLITUDE).abs() < 1e-5);
    }

    #[test]
    fn an_overridden_sample_rate_scales_every_position() {
        let mut project = Project::new("Rate", SAMPLE_RATE);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes.push(Note::new(
            60,
            Ticks::ZERO,
            Ticks::from_beats(0.5),
        ));

        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions {
                sample_rate: Some(96_000.0),
                ..OfflineOptions::default()
            },
        )
        .expect("render");

        // Eight beats at 120 BPM is 4 s, which is 384 000 frames at 96 kHz.
        assert_eq!(rendered.frame_count(), 384_000);
        assert_eq!(rendered.sample_rate(), 96_000.0);
        // The note lasts 0.25 s, so it ends at frame 24 000 rather than 12 000.
        assert!((rendered.channel(0)[23_999] - TONE_AMPLITUDE).abs() < 1e-5);
        assert_eq!(rendered.channel(0)[24_000], 0.0);
    }

    #[test]
    fn an_overridden_sample_rate_converts_a_clips_frame_counts() {
        // The clip's trim is counted in the frames of the file as it was imported — 48 000 of
        // them, one second — while this export runs at half that rate, where one second is
        // 24 000 frames. The caller hands the bank over already converted, so the clip covers
        // the whole export rather than half of it or twice it.
        let mut project = Project::new("Clip", SAMPLE_RATE);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            48_000,
            SAMPLE_RATE,
            1,
        );
        project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let mut bank = AudioSourceBank::new();
        bank.insert(
            source,
            Arc::new(AudioBuffer::from_planar(vec![vec![0.5; 24_000]], 24_000.0).expect("planar")),
        );

        let rendered = render_project(
            &project,
            &bank,
            &testkit::registry(),
            &OfflineOptions {
                sample_rate: Some(24_000.0),
                ..OfflineOptions::default()
            },
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 24_000);
        assert_eq!(rendered.sample_rate(), 24_000.0);
        assert!((rendered.channel(0)[0] - 0.5).abs() < 1e-5);
        assert!(
            (rendered.channel(0)[23_999] - 0.5).abs() < 1e-5,
            "the clip stopped short of the end of the export"
        );
    }

    #[test]
    fn a_render_asked_to_stop_stops_and_says_which_it_was() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // Raised before the first block, so the very first check fires. The render still
        // reports its progress up to that point — the bar shows where it got to, which is what
        // a stopped export has to be able to draw.
        let flag = AtomicBool::new(true);
        let mut reports = Vec::new();
        let ended = render_project_with_progress(
            &four_beat_project(),
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_block_frames(8_192),
            &mut RenderProgress::reporting(&mut |fraction| reports.push(fraction))
                .cancelled_by(&flag),
        );
        assert!(matches!(ended, Err(EngineError::RenderCancelled)));
        // 0.0 before the loop and one block's worth inside it, then out.
        assert_eq!(reports.len(), 2);
        assert!(reports[1] < 1.0, "it stopped part way, not at the end");

        // And a flag that is never raised changes nothing.
        flag.store(false, Ordering::Relaxed);
        assert!(
            render_project_with_progress(
                &four_beat_project(),
                &AudioSourceBank::new(),
                &testkit::registry(),
                &OfflineOptions::default().with_block_frames(8_192),
                &mut RenderProgress::default().cancelled_by(&flag),
            )
            .is_ok()
        );
    }

    #[test]
    fn progress_runs_from_zero_to_one_and_is_monotonic() {
        let project = four_beat_project();
        let mut reports = Vec::new();
        render_project_with_progress(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_block_frames(8_192),
            &mut RenderProgress::reporting(&mut |fraction| reports.push(fraction)),
        )
        .expect("render");

        assert_eq!(reports.first().copied(), Some(0.0));
        assert_eq!(reports.last().copied(), Some(1.0));
        assert!(reports.windows(2).all(|pair| pair[0] <= pair[1]));
        // 96 000 frames in 8 192-frame blocks is 12 blocks, plus the initial 0.0.
        assert_eq!(reports.len(), 13);
    }

    #[test]
    fn a_backwards_range_is_rejected() {
        let project = four_beat_project();
        let error = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(9_000, 1_000),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EngineError::InvalidRange {
                start: 9_000,
                end: 1_000
            }
        ));
    }

    #[test]
    fn an_absurd_frame_count_is_an_error_rather_than_an_allocator_panic() {
        let project = four_beat_project();
        let error = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, u64::MAX),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                EngineError::RenderTooLong {
                    frames: u64::MAX,
                    limit: MAX_RENDER_FRAMES
                }
            ),
            "got {error}"
        );
    }

    #[test]
    fn the_buffered_render_budget_counts_checked_stereo_f32_bytes() {
        let frames = MAX_BUFFERED_RENDER_BYTES / RENDER_CHANNELS / std::mem::size_of::<f32>();

        assert_eq!(
            buffered_render_bytes(frames).expect("the exact boundary must fit"),
            MAX_BUFFERED_RENDER_BYTES
        );
    }

    #[test]
    fn a_buffered_render_one_frame_over_budget_is_rejected_before_allocation() {
        let frames = MAX_BUFFERED_RENDER_BYTES / RENDER_CHANNELS / std::mem::size_of::<f32>() + 1;
        let project = Project::new("Too large for memory", SAMPLE_RATE);
        let error = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default().with_range(0, frames as u64),
        )
        .expect_err("the complete buffer must be rejected");

        assert!(matches!(
            error,
            EngineError::RenderBufferTooLarge {
                frames: requested,
                channels: RENDER_CHANNELS,
                limit_bytes: MAX_BUFFERED_RENDER_BYTES,
            } if requested == frames
        ));
    }

    #[test]
    fn buffered_render_size_arithmetic_overflow_is_rejected() {
        let error = buffered_render_bytes(usize::MAX).expect_err("the byte count must not wrap");

        assert!(matches!(
            error,
            EngineError::RenderBufferTooLarge {
                frames: usize::MAX,
                ..
            }
        ));
    }

    #[test]
    fn direct_full_buffer_rendering_obeys_the_memory_budget() {
        let frames = MAX_BUFFERED_RENDER_BYTES / RENDER_CHANNELS / std::mem::size_of::<f32>() + 1;
        let project = Project::new("Too large for direct render", SAMPLE_RATE);
        let options = OfflineOptions::default().with_range(0, frames as u64);
        let mut render = OfflineRender::new(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut crate::graph::PlacedEffects::new(),
            &mut crate::graph::PlacedInstruments::new(),
            &options,
        )
        .expect("streamable render geometry");
        let mut small_buffer = AudioBuffer::stereo(1, SAMPLE_RATE);
        let error = render
            .render(&mut small_buffer, &mut RenderProgress::default())
            .expect_err("direct rendering must enforce the complete-buffer budget");

        assert!(matches!(error, EngineError::RenderBufferTooLarge { .. }));
    }

    #[test]
    fn direct_render_refuses_a_wrong_output_layout_instead_of_panicking() {
        let project = Project::new("Output layout", SAMPLE_RATE);
        let options = OfflineOptions::default().with_range(0, 8);
        let mut render = OfflineRender::new(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut crate::graph::PlacedEffects::new(),
            &mut crate::graph::PlacedInstruments::new(),
            &options,
        )
        .expect("render geometry");

        for mut output in [
            AudioBuffer::new(1, 8, SAMPLE_RATE),
            AudioBuffer::stereo(7, SAMPLE_RATE),
            AudioBuffer::stereo(8, 44_100.0),
        ] {
            let error = render
                .render(&mut output, &mut RenderProgress::default())
                .expect_err("an incompatible caller buffer must be refused");
            assert!(matches!(
                error,
                EngineError::Core(auris_core::CoreError::LayoutMismatch(_))
            ));
        }
    }

    #[test]
    fn the_twenty_four_hour_frame_limit_remains_streamable() {
        let project = Project::new("Maximum stream", SAMPLE_RATE);
        let options = OfflineOptions::default()
            .with_range(0, MAX_RENDER_FRAMES)
            .with_block_frames(1);
        let mut render = OfflineRender::new(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut crate::graph::PlacedEffects::new(),
            &mut crate::graph::PlacedInstruments::new(),
            &options,
        )
        .expect("streamable render geometry");

        let result = render.render_streamed(&mut RenderProgress::default(), |_block| {
            Err("stop after the first bounded block")
        });

        assert!(matches!(
            result,
            Err(OfflineStreamError::Sink(
                "stop after the first bounded block"
            ))
        ));
    }

    #[test]
    fn a_long_but_plausible_range_is_still_accepted() {
        // A minute at 48 kHz: well inside the bound, and cheap enough to actually render.
        let project = four_beat_project();
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default()
                .with_range(0, 2_880_000)
                .with_block_frames(65_536),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 2_880_000);
    }

    #[test]
    fn a_long_streamed_render_never_builds_a_whole_output_buffer() {
        let project = Project::new("Long stream", SAMPLE_RATE);
        let options = OfflineOptions::default()
            .with_range(0, 2_880_000)
            .with_block_frames(65_536);
        let mut render = OfflineRender::new(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut crate::graph::PlacedEffects::new(),
            &mut crate::graph::PlacedInstruments::new(),
            &options,
        )
        .expect("render geometry");
        let mut received = 0;
        let mut largest = 0;
        render
            .render_streamed(&mut RenderProgress::default(), |block| {
                received += block.frame_count();
                largest = largest.max(block.frame_count());
                assert!(
                    block
                        .channels()
                        .iter()
                        .all(|channel| channel.capacity() <= 65_536),
                    "a channel retained an allocation larger than one render block"
                );
                Ok::<(), std::convert::Infallible>(())
            })
            .expect("streamed render");

        assert_eq!(received, 2_880_000);
        assert_eq!(largest, 65_536);
    }

    #[test]
    fn an_empty_project_renders_an_empty_buffer() {
        let project = Project::new("Empty", SAMPLE_RATE);
        let rendered = render_project(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &OfflineOptions::default(),
        )
        .expect("render");
        assert_eq!(rendered.frame_count(), 0);
    }

    #[test]
    fn rendering_twice_produces_identical_samples() {
        let project = four_beat_project();
        let bank = AudioSourceBank::new();
        let registry = testkit::registry();
        let first =
            render_project(&project, &bank, &registry, &OfflineOptions::default()).expect("render");
        let second =
            render_project(&project, &bank, &registry, &OfflineOptions::default()).expect("render");
        assert_eq!(first, second);
    }
}
