use auris_core::Ticks;
use auris_core::automation::{AutomationCurve, AutomationLane, AutomationPoint};
use auris_core::param::ParamUnit;
use auris_core::pipeline::content::{
    AutomationBinding, AutomationDocument, AutomationError, AutomationId, ParameterAddress,
};
use auris_core::pipeline::{ControlDomain, EdgeId, NodeId};
use auris_core::project::TrackId;

fn domain() -> ControlDomain {
    ControlDomain::Native {
        unit: ParamUnit::Decibels,
        min: -60.0,
        max: 6.0,
    }
}

fn points() -> Vec<AutomationPoint> {
    vec![
        AutomationPoint::new(Ticks::ZERO, -12.0),
        AutomationPoint::new(Ticks(480), -6.0),
        AutomationPoint::new(Ticks(960), 0.0),
    ]
}

#[test]
fn document_round_trip_preserves_curve_domain_and_points() {
    let document = AutomationDocument::new(
        AutomationId(17),
        domain(),
        AutomationCurve::Linear,
        points(),
    )
    .unwrap();
    assert_eq!(document.value_at(Ticks(240)), -9.0);
    assert_eq!(document.value_at(Ticks(-1)), -12.0);
    assert_eq!(document.value_at(Ticks(2_000)), 0.0);

    let json = serde_json::to_vec(&document).unwrap();
    let decoded: AutomationDocument = serde_json::from_slice(&json).unwrap();
    assert_eq!(decoded, document);
}

#[test]
fn hold_curve_keeps_exact_legacy_endpoint_behavior() {
    let document =
        AutomationDocument::new(AutomationId(18), domain(), AutomationCurve::Hold, points())
            .unwrap();
    assert_eq!(document.value_at(Ticks::ZERO), -12.0);
    assert_eq!(document.value_at(Ticks(479)), -12.0);
    assert_eq!(document.value_at(Ticks(480)), -6.0);
    assert_eq!(document.value_at(Ticks(1_000)), 0.0);
}

#[test]
fn malformed_curves_are_rejected_without_reordering_or_deduplicating() {
    let duplicate = vec![
        AutomationPoint::new(Ticks::ZERO, 0.0),
        AutomationPoint::new(Ticks::ZERO, 1.0),
    ];
    assert!(matches!(
        AutomationDocument::new(
            AutomationId(1),
            ControlDomain::Normalized { min: 0.0, max: 1.0 },
            AutomationCurve::Linear,
            duplicate
        ),
        Err(AutomationError::Unordered { index: 1 })
    ));

    let unsorted = vec![
        AutomationPoint::new(Ticks(10), 0.0),
        AutomationPoint::new(Ticks::ZERO, 1.0),
    ];
    assert!(matches!(
        AutomationDocument::new(
            AutomationId(1),
            ControlDomain::Normalized { min: 0.0, max: 1.0 },
            AutomationCurve::Linear,
            unsorted
        ),
        Err(AutomationError::Unordered { index: 1 })
    ));
    assert_eq!(
        AutomationDocument::new(
            AutomationId(1),
            domain(),
            AutomationCurve::Linear,
            Vec::new()
        ),
        Err(AutomationError::Empty)
    );
}

#[test]
fn domains_reject_invalid_bounds_and_out_of_range_values() {
    assert!(matches!(
        AutomationDocument::new(
            AutomationId(1),
            ControlDomain::Normalized {
                min: -0.1,
                max: 1.0
            },
            AutomationCurve::Linear,
            vec![AutomationPoint::new(Ticks::ZERO, 0.0)]
        ),
        Err(AutomationError::InvalidDomain)
    ));
    assert!(matches!(
        AutomationDocument::new(
            AutomationId(1),
            domain(),
            AutomationCurve::Linear,
            vec![AutomationPoint::new(Ticks::ZERO, 7.0)]
        ),
        Err(AutomationError::OutOfRange { index: 0 })
    ));
    assert!(matches!(
        AutomationDocument::new(
            AutomationId(1),
            domain(),
            AutomationCurve::Linear,
            vec![AutomationPoint::new(Ticks(-1), 0.0)]
        ),
        Err(AutomationError::NegativeTick { index: 0 })
    ));
    assert!(matches!(
        AutomationDocument::new(
            AutomationId(1),
            domain(),
            AutomationCurve::Linear,
            vec![AutomationPoint::new(Ticks::ZERO, f32::NAN)]
        ),
        Err(AutomationError::NonFinite { index: 0 })
    ));
}

#[test]
fn validate_rechecks_public_domain_after_mutation() {
    let mut document = AutomationDocument::new(
        AutomationId(19),
        ControlDomain::Normalized { min: 0.0, max: 1.0 },
        AutomationCurve::Linear,
        vec![AutomationPoint::new(Ticks::ZERO, 0.5)],
    )
    .unwrap();
    document.domain = Some(ControlDomain::Normalized { min: 0.8, max: 0.2 });
    assert_eq!(document.validate(), Err(AutomationError::InvalidDomain));
}

#[test]
fn from_lane_preserves_points_and_interpolation() {
    let lane = AutomationLane::new(
        auris_core::param::ParamTarget::TrackGain(TrackId(3)),
        None,
        AutomationCurve::Hold,
        points(),
    )
    .unwrap();
    let document = AutomationDocument::from_lane(AutomationId(9), domain(), &lane).unwrap();
    assert_eq!(document.points(), lane.points());
    assert_eq!(document.curve, lane.curve);
}

#[test]
fn binding_serialization_keeps_curve_reference_and_parameter_key() {
    let binding = AutomationBinding {
        id: EdgeId(23),
        source: NodeId(7),
        target: ParameterAddress {
            node: NodeId(8),
            key: "cutoff_hz".into(),
        },
        enabled: false,
    };
    let decoded: AutomationBinding =
        serde_json::from_value(serde_json::to_value(binding.clone()).unwrap()).unwrap();
    assert_eq!(decoded, binding);
}

#[test]
fn unresolved_legacy_curve_round_trips_and_domain_resolution_is_atomic() {
    let lane = AutomationLane::new(
        auris_core::param::ParamTarget::TrackGain(TrackId(3)),
        None,
        AutomationCurve::Linear,
        points(),
    )
    .unwrap();
    let mut document = AutomationDocument::from_lane_unresolved(AutomationId(24), &lane).unwrap();
    assert_eq!(document.domain, None);
    let encoded = serde_json::to_vec(&document).unwrap();
    assert_eq!(
        serde_json::from_slice::<AutomationDocument>(&encoded).unwrap(),
        document
    );
    let mut missing_domain = serde_json::to_value(&document).unwrap();
    missing_domain.as_object_mut().unwrap().remove("domain");
    assert_eq!(
        serde_json::from_value::<AutomationDocument>(missing_domain).unwrap(),
        document
    );
    assert_eq!(document.value_at(Ticks(240)), lane.value_at(Ticks(240)));
    assert!(
        document
            .resolve_domain(ControlDomain::Normalized { min: 0.0, max: 1.0 })
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&document).unwrap(), encoded);
    document.resolve_domain(domain()).unwrap();
    assert_eq!(document.domain, Some(domain()));
    assert_eq!(document.points(), lane.points());
}

#[test]
fn resolved_domain_rejects_interpolation_overflow() {
    assert_eq!(
        AutomationDocument::new(
            AutomationId(25),
            ControlDomain::Native {
                unit: ParamUnit::Plain,
                min: -f32::MAX,
                max: f32::MAX
            },
            AutomationCurve::Linear,
            vec![AutomationPoint::new(Ticks::ZERO, 0.0)],
        ),
        Err(AutomationError::InvalidDomain)
    );
}
