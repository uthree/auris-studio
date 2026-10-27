use auris_core::param::{MixerControl, ParamTarget, ParamUnit, ParamValueCurve, mixer_descriptor};
use auris_core::{EffectSlotId, SendId, TrackId};

#[test]
fn mixer_controls_have_one_canonical_descriptor_each() {
    let gain = MixerControl::Gain.descriptor();
    assert_eq!(
        (
            gain.key.as_ref(),
            gain.name.as_ref(),
            gain.min,
            gain.max,
            gain.default
        ),
        ("gain", "Volume", -60.0, 12.0, 0.0)
    );
    assert_eq!(
        (gain.unit, gain.curve, gain.steps),
        (ParamUnit::Decibels, ParamValueCurve::Linear, None)
    );

    let pan = MixerControl::Pan.descriptor();
    assert_eq!(
        (
            pan.key.as_ref(),
            pan.name.as_ref(),
            pan.min,
            pan.max,
            pan.default
        ),
        ("pan", "Pan", -1.0, 1.0, 0.0)
    );
    assert_eq!(pan.unit, ParamUnit::Pan);

    let send = MixerControl::Send.descriptor();
    assert_eq!(
        (
            send.key.as_ref(),
            send.name.as_ref(),
            send.min,
            send.max,
            send.default
        ),
        ("send", "Send", -60.0, 0.0, 0.0)
    );
    assert_eq!(send.unit, ParamUnit::Decibels);
}

#[test]
fn mixer_descriptor_resolves_all_builtin_targets_and_rejects_plugins() {
    let targets = [
        ParamTarget::TrackGain(TrackId(1)),
        ParamTarget::MasterGain,
        ParamTarget::TrackPan(TrackId(2)),
        ParamTarget::MasterPan,
        ParamTarget::Send {
            track: TrackId(3),
            send: SendId(4),
        },
    ];
    for target in targets {
        assert!(mixer_descriptor(target).is_some());
    }
    assert!(
        mixer_descriptor(ParamTarget::Instrument {
            track: TrackId(1),
            param: 0u32.into()
        })
        .is_none()
    );
    assert!(
        mixer_descriptor(ParamTarget::Effect {
            track: None,
            slot: EffectSlotId(5),
            param: 0u32.into()
        })
        .is_none()
    );
}
