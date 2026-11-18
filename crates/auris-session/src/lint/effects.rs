//! Effect settings held to the ranges their style's page names.
//!
//! The genre pages tell the composing agent which effect a part takes and where its settings
//! start: a clean city pop guitar's amplifier at a drive of 0 to 6 dB, an ambient hall's room
//! size near the top. A model asked to follow them inserts the effect and, often enough, never
//! sets it, so the city pop guitar plays through the amplifier's crunch default of 12 dB. A
//! setting left at a default outside the range is that oversight and is a warning; a setting
//! moved outside it was chosen and is only noted, because the page's ranges are starting points
//! and a part may want another.

use super::{Diagnostic, Rule, Severity};
use auris_core::Project;
use auris_core::project::{EffectSlot, MixerStrip, TrackId};
use auris_core::registry::PluginRegistry;
use auris_core::time::Ticks;

/// How far outside a range a value may sit and still count as in it, for the rounding of a
/// value typed as text.
const TOLERANCE: f32 = 1e-3;

/// Every enabled effect on a track or the master whose setting falls outside its style's range.
pub(crate) fn effect_settings(project: &Project, registry: &PluginRegistry) -> Vec<Diagnostic> {
    let ranges = crate::session::style_checks::style_checks(project.style).effect_ranges;
    if ranges.is_empty() {
        return Vec::new();
    }
    let end = project.end_tick().max(Ticks(1));
    let strips = project
        .tracks
        .iter()
        .map(|track| (Some(track.id), &track.mixer))
        .chain([(None, &project.master)]);
    let mut diagnostics = Vec::new();
    for (track, strip) in strips {
        diagnostics.extend(strip_settings(track, strip, ranges, registry, end));
    }
    diagnostics
}

fn strip_settings(
    track: Option<TrackId>,
    strip: &MixerStrip,
    ranges: &[crate::session::style_checks::EffectRange],
    registry: &PluginRegistry,
    end: Ticks,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for slot in strip.effects.iter().filter(|slot| slot.enabled) {
        let wanted: Vec<_> = ranges
            .iter()
            .filter(|range| range.effect == slot.effect_id)
            .collect();
        if wanted.is_empty() {
            continue;
        }
        for range in wanted {
            let Some((value, set)) = setting(slot, range.param, registry) else {
                continue;
            };
            let limit = if value < range.low - TOLERANCE {
                range.low
            } else if value > range.high + TOLERANCE {
                range.high
            } else {
                continue;
            };
            let severity = if set {
                Severity::Info
            } else {
                Severity::Warning
            };
            let mut diagnostic = Diagnostic::new(Rule::EffectSetting, severity, Ticks::ZERO, end);
            diagnostic.track = track;
            diagnostic.effect = Some(slot.id);
            diagnostic.param = Some(range.param.to_string());
            diagnostic.value = Some(value);
            diagnostic.limit = Some(limit);
            diagnostic.range = Some((range.low, range.high));
            diagnostics.push(diagnostic);
        }
    }
    diagnostics
}

/// The value `slot` plays `param` at, and whether it was set rather than left at the default.
///
/// A slot stores only what was set; the default comes from the effect itself, which is built
/// only when a value is missing, so a project whose settings are all stored builds nothing.
fn setting(slot: &EffectSlot, param: &str, registry: &PluginRegistry) -> Option<(f32, bool)> {
    if let Some(value) = slot.state.params.get(param) {
        return Some((*value, true));
    }
    let effect = registry.create_effect(&slot.effect_id).ok()?;
    let default = effect
        .parameters()
        .iter()
        .find(|descriptor| descriptor.key == param)?
        .default;
    Some((default, false))
}

#[cfg(test)]
mod tests {
    use crate::lint::{Rule, Severity};
    use crate::{Session, SessionOptions};
    use auris_core::style::{Genre, Style};

    fn session(genre: Genre) -> (Session, auris_core::project::TrackId) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Guitar").unwrap();
        session.set_style(Style {
            genre: Some(genre),
            usage: None,
        });
        (session, track)
    }

    fn findings(session: &Session) -> Vec<(Severity, Option<f32>, Option<f32>, String)> {
        session
            .lint_score()
            .diagnostics
            .into_iter()
            .filter(|d| d.rule == Rule::EffectSetting)
            .map(|d| (d.severity, d.value, d.limit, d.param.unwrap_or_default()))
            .collect()
    }

    fn set(session: &mut Session, track: auris_core::project::TrackId, key: &str, value: f32) {
        let slot = session.project().track(track).unwrap().mixer.effects[0].id;
        let id = session
            .param_descriptors("auris.fx.guitar_amp")
            .iter()
            .find(|param| param.key == key)
            .unwrap()
            .id;
        session.set_param(
            auris_core::param::ParamTarget::Effect {
                track: Some(track),
                slot,
                param: id,
            },
            value,
        );
    }

    #[test]
    fn an_amplifier_left_at_its_crunch_default_warns_on_a_clean_city_pop_guitar() {
        let (mut session, track) = session(Genre::CityPop);
        assert!(findings(&session).is_empty());
        session
            .add_effect(Some(track), "auris.fx.guitar_amp")
            .unwrap();
        // The default drive is 12 dB, above the page's 0 to 6 for a clean part.
        assert_eq!(
            findings(&session),
            vec![(Severity::Warning, Some(12.0), Some(6.0), "drive_db".into())]
        );
        // The model is told the whole range, not only the bound it crossed, so it can choose a
        // value inside rather than on the edge.
        let message = session
            .lint_score()
            .diagnostics
            .iter()
            .find(|d| d.rule == Rule::EffectSetting)
            .unwrap()
            .describe();
        assert!(
            message.contains("above the 0 to 6 this style's page names"),
            "{message}"
        );
        set(&mut session, track, "drive_db", 3.0);
        assert!(findings(&session).is_empty());
        // Driven past the range on purpose: a note, not a warning.
        set(&mut session, track, "drive_db", 20.0);
        assert_eq!(
            findings(&session),
            vec![(Severity::Info, Some(20.0), Some(6.0), "drive_db".into())]
        );
    }

    #[test]
    fn a_bypassed_effect_or_an_unset_style_is_not_held_to_a_range() {
        let (mut session, track) = session(Genre::CityPop);
        let slot = session
            .add_effect(Some(track), "auris.fx.guitar_amp")
            .unwrap();
        session.set_effect_enabled(Some(track), slot, false);
        assert!(findings(&session).is_empty());
        session.set_effect_enabled(Some(track), slot, true);
        assert_eq!(findings(&session).len(), 1);
        session.set_style(Style::default());
        assert!(findings(&session).is_empty());
    }

    #[test]
    fn rock_allows_every_guitar_from_clean_to_lead() {
        let (mut session, track) = session(Genre::Rock);
        session
            .add_effect(Some(track), "auris.fx.guitar_amp")
            .unwrap();
        assert!(findings(&session).is_empty());
        set(&mut session, track, "drive_db", 40.0);
        assert_eq!(
            findings(&session),
            vec![(Severity::Info, Some(40.0), Some(34.0), "drive_db".into())]
        );
    }
}
