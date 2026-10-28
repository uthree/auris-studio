use super::PreparedMidiImport;
use crate::native_edit::NativeEditOwner;
use crate::native_file_session::NativeFileSession;
use auris_core::Ticks;
use auris_core::pipeline::content::ContentLimits;
use auris_core::project::Project;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

fn limits() -> ContentLimits {
    crate::native_edit::tests::limits()
}

fn varlen(mut value: u32) -> Vec<u8> {
    let mut bytes = vec![(value & 0x7f) as u8];
    while {
        value >>= 7;
        value != 0
    } {
        bytes.push(((value & 0x7f) as u8) | 0x80);
    }
    bytes.reverse();
    bytes
}

fn track_chunk(events: Vec<u8>) -> Vec<u8> {
    let mut chunk = b"MTrk".to_vec();
    chunk.extend((events.len() as u32).to_be_bytes());
    chunk.extend(events);
    chunk
}

fn raw_fixture() -> Vec<u8> {
    let mut tempo = Vec::new();
    tempo.extend([0, 0xff, 0x51, 3, 0x09, 0x89, 0x68]);
    tempo.extend([0, 0xff, 0x58, 4, 7, 3, 24, 8]);
    tempo.extend([0, 0xff, 0x2f, 0]);

    let mut lead = Vec::new();
    lead.extend([0, 0xff, 3, 4, b'B', b'e', b'n', b'd']);
    // RPN 0,0 followed by Data Entry 24 semitones, then two same-tick bends.
    lead.extend([
        0, 0xb0, 101, 0, 0, 0xb0, 100, 0, 0, 0xb0, 6, 24, 0, 0xb0, 38, 0,
    ]);
    lead.extend([0, 0xe0, 0, 64, 0, 0xe0, 127, 127]);
    // Same-tick controller events must retain their order and both values.
    lead.extend([0, 0xb0, 1, 0, 0, 0xb0, 1, 127]);
    lead.extend([0, 0xb0, 64, 127, 0, 0x90, 60, 100]);
    lead.extend(varlen(480));
    lead.extend([0x80, 60, 0]);
    // Pedal tail after the final key release.
    lead.extend(varlen(120));
    lead.extend([0xb0, 64, 0]);
    lead.extend([0, 0xff, 0x2f, 0]);

    let mut file = b"MThd\0\0\0\x06\0\x01\0\x02\x01\xe0".to_vec();
    file.extend(track_chunk(tempo));
    file.extend(track_chunk(lead));
    file
}

#[test]
fn raw_smf_preserves_same_tick_curves_wide_rpn_and_native_round_trip() {
    let directory = TempDir::new().unwrap();
    let midi = directory.path().join("curves.mid");
    std::fs::write(&midi, raw_fixture()).unwrap();

    let prepared = PreparedMidiImport::read(
        &midi,
        "auris.synth.chiptune",
        "auris.synth.noisedrum",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    assert_eq!(prepared.tracks.len(), 1);
    let track = &prepared.tracks[0];
    assert_eq!(track.notes.len(), 1);
    assert_eq!(track.bend.len(), 2);
    assert_eq!(track.bend[0].at, Ticks::ZERO);
    assert_eq!(track.bend[1].at, Ticks::ZERO);
    assert_eq!(track.bend[0].value, 0.0);
    assert!(track.bend[1].value > 23.9);
    let modulation = &track.controllers[&1];
    assert!(modulation.len() >= 2);
    assert_eq!(modulation[0].at, Ticks::ZERO);
    assert_eq!(modulation[0].value, 0.0);
    assert_eq!(modulation[1].value, 1.0);
    assert!(
        track
            .controllers
            .values()
            .flatten()
            .any(|point| point.at > Ticks::QUARTER)
    );

    let mut base = Project::new("Native MIDI", 48_000.0);
    base.add_instrument_track("Existing", "auris.synth.chiptune");
    let document = auris_io::project_to_native(&base, limits()).unwrap();
    let mut native = NativeFileSession::new(document, limits()).unwrap();
    let job = native
        .begin_midi_append(
            &midi,
            Ticks::QUARTER,
            NativeEditOwner::Human,
            "auris.synth.chiptune",
            "auris.synth.noisedrum",
        )
        .unwrap();
    let result = job
        .run(&AtomicBool::new(false))
        .unwrap()
        .expect("MIDI worker result");
    let receipt = native.finish_midi_append(result).unwrap();
    assert_eq!(receipt.report, prepared.report());
    let saved = native
        .begin_save_as(&directory.path().join("NativeMidi.auris"))
        .unwrap()
        .run()
        .unwrap();
    native.finish_save(saved).unwrap();
    let path = native.path().unwrap().to_path_buf();
    let reopened = NativeFileSession::open(&path, limits()).unwrap();
    assert_eq!(reopened.editor().document(), native.editor().document());
}
