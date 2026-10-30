//! Compile this fixture author against the historical core revisions listed in provenance.md.
//! It does not regenerate scores or load any plugin, model or audio file.

use std::error::Error;

use auris_core::project::CurvePoint;
use auris_core::theory::numeral::Numeral;
use auris_core::{
    AssetPath, AutomationCurve, AuxSend, ClipPreset, ClipRecipe, DrumMap, DrumRole, Note,
    NoteTransform, Output, ParamId, ParamTarget, PitchPerformance, Project, Ticks,
};
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let destination = std::env::args().nth(1).ok_or("missing output path")?;
    let mut project = Project::new("Pipeline migration fixture", 48_000.0);
    project.saved_by = "historical-core-fixture-author".into();
    project.set_bpm(120.0);
    project.tempo_map.set_point(Ticks(3840), 90.0);
    project.loop_region = Some((Ticks::ZERO, Ticks(7680)));
    project.loop_enabled = true;
    let chord = match Project::FORMAT_VERSION {
        31.. => "V13",
        30 => "V11",
        _ => "V7",
    };
    project.harmony.chords.set_point(
        Ticks::ZERO,
        Some(Numeral::parse(chord).ok_or("unsupported historical chord")?),
    );

    let lead = project.add_instrument_track("Lead", "fixture.instrument");
    let clip_id = project
        .add_midi_clip(lead, "Authored phrase", Ticks(960), Ticks(3840))
        .unwrap();
    let clip = project.midi_clip_mut(clip_id).unwrap();
    clip.notes = vec![
        Note::new(60, Ticks::ZERO, Ticks(1440)),
        Note::new(67, Ticks(1920), Ticks(960)),
    ];
    clip.notes[1].velocity = 0.5;
    clip.recipe = Some(ClipRecipe::new(ClipPreset::Lead, 1234));
    clip.loop_end = Ticks(7680);
    clip.length_is_explicit = true;
    clip.transforms = vec![
        NoteTransform::Humanize {
            amount: 0.25,
            seed: 42,
        },
        NoteTransform::Transpose { semitones: 12 },
        NoteTransform::Pitch {
            settings: PitchPerformance {
                volume_swell: 0.7,
                ..Default::default()
            },
        },
    ];
    clip.bend = vec![
        CurvePoint {
            at: Ticks::ZERO,
            value: 0.0,
        },
        CurvePoint {
            at: Ticks(960),
            value: 2.0,
        },
    ];
    clip.controllers.insert(
        64,
        vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 1.0,
            },
            CurvePoint {
                at: Ticks(3360),
                value: 0.0,
            },
        ],
    );
    let instrument = project
        .track_mut(lead)
        .unwrap()
        .kind
        .as_instrument_mut()
        .unwrap();
    instrument.file = Some(AssetPath::external("/fixture/plugins/instrument.clap"));
    instrument
        .instrument_state
        .params
        .insert("cutoff".into(), 0.375);
    instrument
        .instrument_state
        .set_hosted_bytes(&[0, 255, 17, 128, 42]);

    let drums = project.add_drum_track("Kit", "auris.synth.drumkit");
    let drum_clip = project
        .add_midi_clip(drums, "Kick", Ticks::ZERO, Ticks(3840))
        .unwrap();
    project
        .midi_clip_mut(drum_clip)
        .unwrap()
        .notes
        .push(Note::new(73, Ticks::ZERO, Ticks(120)));
    let mut drum_map = DrumMap::default();
    drum_map.voices.insert(DrumRole::Kick, 73);
    drum_map.store(
        &mut project
            .track_mut(drums)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state,
    );

    let audio = project.add_audio_track("Audio");
    let asset = project.add_audio_source(
        "Loop",
        AssetPath::inside("Audio/loop.wav"),
        96_000,
        48_000.0,
        2,
    );
    project.add_audio_clip(audio, asset, Ticks(1920)).unwrap();
    project.add_soundfont(
        "Fixture font",
        AssetPath::external("/fixture/fonts/library.sf2"),
        123_456,
    );

    let singer = project.add_singer_track("Singer", "auris.synth.vocal");
    let lyric_clip = project
        .add_midi_clip(singer, "Lyrics", Ticks::ZERO, Ticks(3840))
        .unwrap();
    let mut note = Note::new(64, Ticks(480), Ticks(1920));
    note.lyric = "あ".into();
    note.phonemes = vec!["a".into()];
    note.phoneme_seconds = vec![0.2];
    project.midi_clip_mut(lyric_clip).unwrap().notes.push(note);
    let take = project.add_audio_source(
        "Saved take",
        AssetPath::inside("Audio/singer.wav"),
        192_000,
        48_000.0,
        2,
    );
    let singer_data = project
        .track_mut(singer)
        .unwrap()
        .kind
        .as_singer_mut()
        .unwrap();
    singer_data.voice = Some(serde_json::from_value(json!({
        "path": AssetPath::inside("voice.voicevox.json"),
        "name": "Fixture voice", "speaker": "fixture-speaker"
    }))?);
    singer_data.take = Some(serde_json::from_value(json!({
        "source": take, "fingerprint": 987654321, "seed": 123
    }))?);

    let bus = project.add_bus_track("Return");
    let first = project.add_effect(Some(lead), "auris.fx.gain").unwrap();
    let second = project.add_effect(Some(lead), "fixture.effect").unwrap();
    let slots = &mut project.track_mut(lead).unwrap().mixer.effects;
    slots[0].state.params.insert("gain_db".into(), -3.0);
    slots[1].enabled = false;
    slots[1].file = Some(AssetPath::external("/fixture/plugins/effect.clap"));
    slots[1].sidechain = Some(drums);
    slots[1].state.set_hosted_bytes(&[255, 0, 34, 128]);
    project.add_effect(Some(bus), "auris.fx.reverb").unwrap();
    project.add_effect(None, "auris.fx.limiter").unwrap();
    let strip = &mut project.track_mut(lead).unwrap().mixer;
    strip.gain_db = -6.0;
    strip.pan = -0.25;
    let pre_id = project.next_send_id();
    let post_id = project.next_send_id();
    let mut pre = AuxSend::new(pre_id, bus);
    pre.level_db = -12.0;
    pre.pre_fader = true;
    let mut post = AuxSend::new(post_id, bus);
    post.level_db = -18.0;
    project.track_mut(lead).unwrap().sends = vec![pre, post];
    for (target, key, from, to) in [
        (ParamTarget::TrackGain(lead), None, -6.0, -3.0),
        (
            ParamTarget::Instrument {
                track: lead,
                param: ParamId(0),
            },
            Some("cutoff"),
            0.375,
            0.75,
        ),
        (
            ParamTarget::Effect {
                track: Some(lead),
                slot: first,
                param: ParamId(0),
            },
            Some("gain_db"),
            -3.0,
            0.0,
        ),
        (
            ParamTarget::Send {
                track: lead,
                send: pre_id,
            },
            None,
            -12.0,
            -9.0,
        ),
    ] {
        project.automation.set_point(
            target,
            key.map(str::to_owned),
            AutomationCurve::Linear,
            Ticks(960),
            from,
        );
        project.automation.set_point(
            target,
            key.map(str::to_owned),
            AutomationCurve::Linear,
            Ticks(4800),
            to,
        );
    }

    // The historical Rust field changed in format 32, so compile against its actual API.
    #[cfg(not(feature = "multi-output"))]
    {
        assert!(Project::FORMAT_VERSION < 32);
        project.track_mut(lead).unwrap().output = Output::Bus(bus);
    }
    #[cfg(feature = "multi-output")]
    {
        assert!(Project::FORMAT_VERSION >= 32);
        project.track_mut(lead).unwrap().outputs = vec![Output::Master, Output::Bus(bus)];
        project.track_mut(audio).unwrap().outputs.clear();
    }
    assert_eq!(project.track(lead).unwrap().mixer.effects[1].id, second);
    std::fs::write(
        destination,
        format!("{}\n", serde_json::to_string_pretty(&project)?),
    )?;
    Ok(())
}
