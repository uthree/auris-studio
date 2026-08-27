//! Registration of every instrument in this crate.

use auris_core::{PluginPack, PluginRegistry};

use crate::chiptune::Chiptune;
use crate::fm2::Fm2;
use crate::noisedrum::NoiseDrum;
use crate::vocal::Vocal;

/// Installs the built-in instruments into a registry.
///
/// ```
/// use auris_core::PluginRegistry;
/// use auris_synth::SynthPack;
///
/// let mut registry = PluginRegistry::new();
/// registry.install::<SynthPack>();
/// assert!(registry.has_instrument("auris.synth.chiptune"));
/// ```
pub struct SynthPack;

impl PluginPack for SynthPack {
    fn register(registry: &mut PluginRegistry) {
        registry.register_instrument(|| Box::new(Chiptune::new()));
        registry.register_instrument(|| Box::new(Fm2::new()));
        registry.register_instrument(|| Box::new(NoiseDrum::new()));
        registry.register_instrument(|| Box::new(Vocal::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::{
        AudioBuffer, Instrument, NoteEvent, PluginCategory, PrepareContext, ProcessContext,
    };

    /// One fresh instance of every instrument in the pack, so a fourth instrument added later
    /// is covered by the shape and determinism tests below without touching them.
    fn every_instrument() -> Vec<(String, Box<dyn Instrument>)> {
        let mut registry = PluginRegistry::new();
        registry.install::<SynthPack>();
        let ids: Vec<String> = registry.instruments().map(|d| d.id.to_string()).collect();
        ids.into_iter()
            .map(|id| {
                let instrument = registry.create_instrument(&id).unwrap();
                (id, instrument)
            })
            .collect()
    }

    fn note_on(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch,
            velocity: 1.0,
        }
    }

    /// Renders `frames` frames in `block`-frame chunks into a `channels`-channel buffer,
    /// re-basing each event onto the block it falls in. Returns channel 0.
    fn render(
        instrument: &mut dyn Instrument,
        frames: usize,
        block: usize,
        channels: usize,
        events: &[NoteEvent],
    ) -> Vec<f32> {
        let mut buffer = AudioBuffer::new(channels, block, 48_000.0);
        let mut out = Vec::with_capacity(frames);
        let mut done = 0usize;
        while done < frames {
            let this = block.min(frames - done);
            buffer.set_frame_count(this);
            let (start, end) = (done as u32, (done + this) as u32);
            let block_events: Vec<NoteEvent> = events
                .iter()
                .filter(|event| (start..end).contains(&event.frame()))
                .map(|event| event.with_frame(event.frame() - start))
                .collect();
            let ctx = ProcessContext::realtime(48_000.0, this, done as u64, 120.0, true);
            instrument.process(&block_events, &mut buffer, &ctx);
            out.extend_from_slice(buffer.channel(0));
            done += this;
        }
        out
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
    }

    #[test]
    fn the_pack_registers_every_instrument() {
        let mut registry = PluginRegistry::new();
        registry.install::<SynthPack>();

        let ids: Vec<&str> = registry.instruments().map(|d| d.id.as_ref()).collect();
        assert_eq!(
            ids,
            vec![
                "auris.synth.chiptune",
                "auris.synth.fm2",
                "auris.synth.noisedrum",
                "auris.synth.vocal"
            ]
        );
        assert_eq!(registry.len(), 4);
        assert_eq!(registry.effects().count(), 0);
    }

    #[test]
    fn categories_group_the_instruments_for_the_browser() {
        let mut registry = PluginRegistry::new();
        registry.install::<SynthPack>();
        let drums: Vec<&str> = registry
            .instruments()
            .filter(|d| d.category == PluginCategory::Drum)
            .map(|d| d.id.as_ref())
            .collect();
        assert_eq!(drums, vec!["auris.synth.noisedrum"]);
    }

    #[test]
    fn every_registered_instrument_renders_a_block() {
        let mut registry = PluginRegistry::new();
        registry.install::<SynthPack>();
        let ids: Vec<String> = registry.instruments().map(|d| d.id.to_string()).collect();

        for id in ids {
            let mut instrument = registry.create_instrument(&id).unwrap();
            instrument.prepare(&PrepareContext::new(44_100.0, 256, 2));
            let mut buffer = AudioBuffer::stereo(256, 44_100.0);
            let events = [auris_core::NoteEvent::NoteOn {
                frame: 8,
                pitch: 60,
                velocity: 0.8,
            }];
            let ctx = ProcessContext::realtime(44_100.0, 256, 0, 120.0, true);
            instrument.process(&events, &mut buffer, &ctx);

            assert_eq!(instrument.active_voices(), 1, "{id} did not start a voice");
            assert!(
                buffer.channel(0)[..8].iter().all(|s| *s == 0.0),
                "{id} ignored the event's frame offset"
            );
            assert!(buffer.peak() > 0.0, "{id} produced silence");
            assert!(
                buffer.channel(0).iter().all(|s| s.is_finite()),
                "{id} produced a non-finite sample"
            );
            assert_eq!(buffer.channel(0), buffer.channel(1), "{id} left a channel");
        }
    }

    #[test]
    fn every_instrument_renders_into_a_mono_buffer() {
        // `spread_to_all_channels` has to cope with there being nothing to spread to. A
        // single-channel bus is a real configuration, and indexing channel 1 would panic.
        for (id, mut instrument) in every_instrument() {
            instrument.prepare(&PrepareContext::new(48_000.0, 512, 1));
            let rendered = render(instrument.as_mut(), 512, 512, 1, &[note_on(0, 60)]);
            assert_eq!(rendered.len(), 512);
            assert!(peak(&rendered) > 0.0, "{id} was silent on a mono buffer");
            assert!(
                rendered.iter().all(|s| s.is_finite()),
                "{id} went non-finite"
            );
        }
    }

    #[test]
    fn every_instrument_honours_an_event_frame_in_one_frame_blocks() {
        // The smallest block the engine can hand out is one frame. Segment splitting must not
        // produce an empty range, and the event still has to land on its own frame.
        for (id, mut instrument) in every_instrument() {
            instrument.prepare(&PrepareContext::new(48_000.0, 512, 2));
            let rendered = render(instrument.as_mut(), 64, 1, 2, &[note_on(3, 60)]);
            assert!(
                rendered[..3].iter().all(|s| *s == 0.0),
                "{id} sounded before its event in one-frame blocks"
            );
            assert!(peak(&rendered[3..]) > 0.0, "{id} never started");
        }
    }

    #[test]
    fn every_instrument_is_deterministic() {
        // The realtime contract requires identical output for identical input and state, so
        // that an offline export matches what was heard. Any uninitialised or clock-derived
        // state — an unseeded noise register above all — would break this.
        let events = [note_on(10, 60), note_on(300, 67), note_on(1_000, 72)];
        for ((id, mut first), (_, mut second)) in
            every_instrument().into_iter().zip(every_instrument())
        {
            first.prepare(&PrepareContext::new(48_000.0, 512, 2));
            second.prepare(&PrepareContext::new(48_000.0, 512, 2));
            let a = render(first.as_mut(), 4_096, 512, 2, &events);
            let b = render(second.as_mut(), 4_096, 512, 2, &events);
            assert_eq!(a, b, "{id} does not render the same twice");
        }
    }

    #[test]
    fn every_instrument_survives_process_before_prepare() {
        // The engine always prepares first, but a plugin that indexes an empty voice pool
        // would take the whole audio thread down if that order were ever broken.
        for (id, mut instrument) in every_instrument() {
            let rendered = render(instrument.as_mut(), 256, 128, 2, &[note_on(0, 60)]);
            assert!(
                rendered.iter().all(|s| *s == 0.0),
                "{id} produced audio without being prepared"
            );
        }
    }
}
