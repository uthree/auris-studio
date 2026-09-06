//! Renders individual kit notes and reports measurements without passing note identities to DSP.

use auris_core::{AudioBuffer, Instrument, NoteEvent, PrepareContext, ProcessContext};
use auris_dsp::drum_analysis::analyze_drum_audio;
use auris_synth::DrumKit;

fn main() {
    println!("key  centroid_Hz  low     body    high    duration_s  role_fitness");
    for key in [36, 38, 42, 46, 49, 51, 47] {
        let mut instrument = DrumKit::new();
        instrument.prepare(&PrepareContext::new(48_000.0, 512, 2));
        let mut audio = AudioBuffer::stereo(144_000, 48_000.0);
        let mut block = AudioBuffer::stereo(512, 48_000.0);
        for start in (0..144_000).step_by(512) {
            let count = (144_000 - start).min(512);
            block.set_frame_count(count);
            let note = [NoteEvent::NoteOn {
                frame: 0,
                pitch: key,
                velocity: 1.0,
            }];
            let events = if start == 0 { &note[..] } else { &[] };
            instrument.process(
                events,
                &mut block,
                &ProcessContext::realtime(48_000.0, count, start as u64, 120.0, true),
            );
            for channel in 0..2 {
                audio.channel_mut(channel)[start..start + count]
                    .copy_from_slice(block.channel(channel));
            }
        }
        let measured = analyze_drum_audio(&audio).expect("a finite rendered hit");
        let spectrum = &measured.spectrum;
        println!(
            "{key:3}  {:11.1}  {:.3}   {:.3}   {:.3}   {:10.3}  {:?}",
            spectrum.centroid_hz,
            spectrum.low,
            spectrum.body,
            spectrum.high,
            measured.energy_duration_seconds,
            measured.fitness
        );
    }
}
