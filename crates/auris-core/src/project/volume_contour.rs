//! Editable, normalized volume envelopes shared by playback and the editor.

use super::{CurvePoint, curve_at};
use crate::Ticks;
use serde::{Deserialize, Serialize};

/// A volume shape stretched over each eligible note, before the strength blend.
/// Point positions use 0..=10_000 for the note's start through release, independent of tempo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "Vec<CurvePoint>", into = "Vec<CurvePoint>")]
pub struct VolumeContour {
    points: Vec<CurvePoint>,
}

impl VolumeContour {
    /// Normalized position of the note's release.
    pub const END: Ticks = Ticks(10_000);

    /// Constructs an ordered shape with distinct positions, bounded levels and both endpoints.
    /// Invalid levels are discarded; an empty shape becomes full volume.
    pub fn new(mut points: Vec<CurvePoint>) -> Self {
        points.retain(|point| point.value.is_finite());
        for point in &mut points {
            point.at = point.at.clamp(Ticks::ZERO, Self::END);
            point.value = point.value.clamp(0.0, 1.0);
        }
        points.sort_by_key(|point| point.at);
        let mut unique: Vec<CurvePoint> = Vec::new();
        for point in points {
            if let Some(last) = unique.last_mut().filter(|last| last.at == point.at) {
                *last = point;
            } else {
                unique.push(point);
            }
        }
        if unique.is_empty() {
            unique.push(CurvePoint {
                at: Ticks::ZERO,
                value: 1.0,
            });
        }
        if unique[0].at != Ticks::ZERO {
            unique.insert(
                0,
                CurvePoint {
                    at: Ticks::ZERO,
                    value: unique[0].value,
                },
            );
        }
        let last = unique[unique.len() - 1];
        if last.at != Self::END {
            unique.push(CurvePoint {
                at: Self::END,
                value: last.value,
            });
        }
        Self { points: unique }
    }

    /// Ordered editable points, including the fixed start and release positions.
    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    /// Interpolated volume at a fraction of the note's duration, in 0..=1.
    pub fn level_at(&self, position: f64) -> f32 {
        let position = position.clamp(0.0, 1.0) * Self::END.raw() as f64;
        let at = Ticks(position.floor() as i64);
        let before = curve_at(&self.points, at);
        let after = curve_at(&self.points, (at + Ticks(1)).min(Self::END));
        before + (after - before) * position.fract() as f32
    }

    /// The matching factory preset, or `None` for a customized shape.
    pub fn preset(&self) -> Option<VolumeContourPreset> {
        VolumeContourPreset::ALL
            .into_iter()
            .find(|preset| preset.contour() == *self)
    }
}

impl Default for VolumeContour {
    fn default() -> Self {
        VolumeContourPreset::Bowed.contour()
    }
}

impl From<Vec<CurvePoint>> for VolumeContour {
    fn from(points: Vec<CurvePoint>) -> Self {
        Self::new(points)
    }
}

impl From<VolumeContour> for Vec<CurvePoint> {
    fn from(contour: VolumeContour) -> Self {
        contour.points
    }
}

/// Starting shapes for a long-note volume envelope; choosing one copies its editable points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeContourPreset {
    /// Brief attack, early dip, late peak and softened release.
    Bowed,
    /// Quiet edges and a broad central peak.
    Swell,
    /// Gradually increasing volume.
    Crescendo,
    /// Gradually decreasing volume.
    Decrescendo,
    /// Slow attack, sustained body and gentle release.
    SoftAttack,
    /// Full volume throughout.
    Flat,
}

impl VolumeContourPreset {
    /// Factory presets in the editor's display order.
    pub const ALL: [Self; 6] = [
        Self::Bowed,
        Self::Swell,
        Self::Crescendo,
        Self::Decrescendo,
        Self::SoftAttack,
        Self::Flat,
    ];

    /// A fresh editable copy of this preset.
    pub fn contour(self) -> VolumeContour {
        let anchors: &[(i64, f32)] = match self {
            Self::Bowed => &[
                (0, 0.8),
                (600, 0.8),
                (1200, 0.35),
                (2400, 0.3),
                (4800, 0.3),
                (7600, 0.85),
                (9000, 1.0),
                (9500, 0.98),
                (10_000, 0.6),
            ],
            Self::Swell => &[
                (0, 0.2),
                (1500, 0.25),
                (5000, 1.0),
                (6500, 1.0),
                (10_000, 0.2),
            ],
            Self::Crescendo => &[(0, 0.2), (10_000, 1.0)],
            Self::Decrescendo => &[(0, 1.0), (10_000, 0.2)],
            Self::SoftAttack => &[(0, 0.1), (3000, 1.0), (8500, 1.0), (10_000, 0.4)],
            Self::Flat => &[(0, 1.0), (10_000, 1.0)],
        };
        VolumeContour::new(
            anchors
                .iter()
                .map(|&(at, value)| CurvePoint {
                    at: Ticks(at),
                    value,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_distinct_editable_shapes_and_expected_directions() {
        for preset in VolumeContourPreset::ALL {
            let contour = preset.contour();
            assert_eq!(contour.preset(), Some(preset));
            assert_eq!(contour.points()[0].at, Ticks::ZERO);
            assert_eq!(contour.points().last().unwrap().at, VolumeContour::END);
        }
        let rising = VolumeContourPreset::Crescendo.contour();
        assert!(rising.level_at(0.2) < rising.level_at(0.8));
        let falling = VolumeContourPreset::Decrescendo.contour();
        assert!(falling.level_at(0.2) > falling.level_at(0.8));
        let swell = VolumeContourPreset::Swell.contour();
        assert!(swell.level_at(0.5) > swell.level_at(0.1));
        assert!(swell.level_at(0.5) > swell.level_at(0.9));
    }

    #[test]
    fn custom_points_are_normalized_and_round_trip_without_losing_the_shape() {
        let contour = VolumeContour::new(vec![
            CurvePoint {
                at: Ticks(7000),
                value: 0.2,
            },
            CurvePoint {
                at: Ticks(3000),
                value: 1.5,
            },
            CurvePoint {
                at: Ticks(7000),
                value: 0.6,
            },
            CurvePoint {
                at: Ticks(6000),
                value: f32::NAN,
            },
        ]);
        assert_eq!(contour.points().len(), 4);
        assert_eq!(contour.preset(), None);
        assert!((contour.level_at(0.5) - 0.8).abs() < 0.0001);
        let restored: VolumeContour =
            serde_json::from_str(&serde_json::to_string(&contour).unwrap()).unwrap();
        assert_eq!(restored, contour);
        assert_eq!(
            VolumeContour::new(Vec::new()),
            VolumeContourPreset::Flat.contour()
        );
    }

    #[test]
    fn legacy_settings_keep_the_original_bowed_envelope() {
        let settings: super::super::PitchPerformance =
            serde_json::from_str(r#"{"volume_swell":0.7}"#).unwrap();
        assert_eq!(
            settings.volume_contour,
            VolumeContourPreset::Bowed.contour()
        );
        assert!((settings.volume_contour.level_at(0.48) - 0.3).abs() < 0.0001);
        assert_eq!(settings.volume_swell, 0.7);
    }
}
