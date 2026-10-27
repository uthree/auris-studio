//! Persistent automation curves and their native pipeline bindings.
//!
//! The document stores a curve once and lets several compatible bindings refer to it. This
//! module currently supports project-tick automation only; clip-local stage envelopes remain a
//! later contract because their event sampling and loop rules are not yet defined.

use serde::{Deserialize, Serialize};

use crate::Ticks;
use crate::automation::{AutomationCurve, AutomationLane, AutomationPoint, evaluate_points};
use crate::pipeline::content::ParameterAddress;
use crate::pipeline::{ControlDomain, EdgeId, NodeId};

/// Persistent identity of one automation curve in the shared document ID namespace.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AutomationId(pub u64);

/// Failure while constructing or decoding a native automation curve.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AutomationError {
    /// A curve must contain at least one point.
    #[error("automation curve must contain at least one point")]
    Empty,
    /// A point is before the project timeline origin.
    #[error("automation point {index} has a negative tick")]
    NegativeTick {
        /// Index of the invalid point.
        index: usize,
    },
    /// A point contains NaN or infinity.
    #[error("automation point {index} has a non-finite value")]
    NonFinite {
        /// Index of the invalid point.
        index: usize,
    },
    /// Points must be strictly increasing on the wire.
    #[error("automation points are not strictly ordered at index {index}")]
    Unordered {
        /// Index of the point that does not follow its predecessor.
        index: usize,
    },
    /// A point falls outside its declared control domain.
    #[error("automation point {index} is outside the control domain")]
    OutOfRange {
        /// Index of the point outside the domain.
        index: usize,
    },
    /// The domain has invalid bounds.
    #[error("automation control domain has invalid bounds")]
    InvalidDomain,
}

/// A validated project-tick automation curve owned by the native document.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AutomationDocument {
    /// Stable curve identity.
    pub id: AutomationId,
    /// Unit and bounds, or unresolved legacy metadata retained without guessing units.
    pub domain: Option<ControlDomain>,
    /// Interpolation policy between points.
    pub curve: AutomationCurve,
    points: Vec<AutomationPoint>,
}

#[derive(Deserialize)]
struct AutomationDocumentRepr {
    id: AutomationId,
    domain: Option<ControlDomain>,
    curve: AutomationCurve,
    points: Vec<AutomationPoint>,
}

impl<'de> Deserialize<'de> for AutomationDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = AutomationDocumentRepr::deserialize(deserializer)?;
        let document = Self {
            id: repr.id,
            domain: repr.domain,
            curve: repr.curve,
            points: repr.points,
        };
        document.validate().map_err(serde::de::Error::custom)?;
        Ok(document)
    }
}

impl AutomationDocument {
    /// Creates a validated project-tick curve without sorting or silently deduplicating points.
    pub fn new(
        id: AutomationId,
        domain: ControlDomain,
        curve: AutomationCurve,
        points: Vec<AutomationPoint>,
    ) -> Result<Self, AutomationError> {
        validate_domain(domain)?;
        validate_points(Some(domain), &points)?;
        Ok(Self {
            id,
            domain: Some(domain),
            curve,
            points,
        })
    }

    /// Converts one legacy lane while preserving its points, interpolation and values.
    pub fn from_lane(
        id: AutomationId,
        domain: ControlDomain,
        lane: &AutomationLane,
    ) -> Result<Self, AutomationError> {
        validate_domain(domain)?;
        validate_points(Some(domain), lane.points())?;
        Self::new(id, domain, lane.curve, lane.points().to_vec())
    }

    /// Revalidates a document after callers inspect or mutate its public domain fields.
    pub fn validate(&self) -> Result<(), AutomationError> {
        if let Some(domain) = self.domain {
            validate_domain(domain)?;
        }
        validate_points(self.domain, &self.points)
    }

    /// Preserves a legacy lane whose plugin value domain is not available during decoding.
    ///
    /// The curve remains authored data. Capability resolution refuses to publish a control
    /// output until an owner supplies a compatible domain; no parameter range is inferred.
    pub fn from_lane_unresolved(
        id: AutomationId,
        lane: &AutomationLane,
    ) -> Result<Self, AutomationError> {
        validate_points(None, lane.points())?;
        Ok(Self {
            id,
            domain: None,
            curve: lane.curve,
            points: lane.points().to_vec(),
        })
    }

    /// Assigns an owner-provided value domain after validating every preserved point.
    /// Failure leaves the prior domain untouched.
    pub fn resolve_domain(&mut self, domain: ControlDomain) -> Result<(), AutomationError> {
        validate_domain(domain)?;
        validate_points(Some(domain), &self.points)?;
        self.domain = Some(domain);
        Ok(())
    }

    /// Returns points in their persisted timeline order.
    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }

    /// Evaluates the curve using the legacy endpoint and interpolation behavior.
    pub fn value_at(&self, tick: Ticks) -> f32 {
        evaluate_points(self.curve, &self.points, tick)
    }
}

/// One enabled or disabled edge from a curve source to a native parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationBinding {
    /// Stable connection identity. It shares the native edge namespace.
    pub id: EdgeId,
    /// Native automation processor node that emits the curve.
    pub source: NodeId,
    /// Native processor receiving the control value.
    pub target: ParameterAddress,
    /// Disabled bindings preserve their curve and static target value takes effect.
    pub enabled: bool,
}

/// Validates a control domain before point validation.
fn validate_domain(domain: ControlDomain) -> Result<(), AutomationError> {
    let (min, max) = match domain {
        ControlDomain::Normalized { min, max } => {
            if !(min.is_finite() && max.is_finite() && 0.0 <= min && min < max && max <= 1.0) {
                return Err(AutomationError::InvalidDomain);
            }
            (min, max)
        }
        ControlDomain::Native { min, max, .. } => {
            if !(min.is_finite() && max.is_finite() && min < max) {
                return Err(AutomationError::InvalidDomain);
            }
            (min, max)
        }
    };
    if min < max && (max - min).is_finite() {
        Ok(())
    } else {
        Err(AutomationError::InvalidDomain)
    }
}

/// Validates strict wire ordering and inclusive domain bounds.
fn validate_points(
    domain: Option<ControlDomain>,
    points: &[AutomationPoint],
) -> Result<(), AutomationError> {
    if points.is_empty() {
        return Err(AutomationError::Empty);
    }
    let bounds = domain.map(domain_bounds);
    let mut previous = None;
    for (index, point) in points.iter().enumerate() {
        if point.tick < Ticks::ZERO {
            return Err(AutomationError::NegativeTick { index });
        }
        if !point.value.is_finite() {
            return Err(AutomationError::NonFinite { index });
        }
        if previous.is_some_and(|tick| point.tick <= tick) {
            return Err(AutomationError::Unordered { index });
        }
        if bounds.is_some_and(|(min, max)| point.value < min || point.value > max) {
            return Err(AutomationError::OutOfRange { index });
        }
        previous = Some(point.tick);
    }
    Ok(())
}

fn domain_bounds(domain: ControlDomain) -> (f32, f32) {
    match domain {
        ControlDomain::Normalized { min, max } | ControlDomain::Native { min, max, .. } => {
            (min, max)
        }
    }
}
