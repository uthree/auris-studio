//! Live route details in the same card as ordinary control tooltips.

use super::*;
use gpui::{AnyView, App, Context, Render, Subscription, WeakEntity, Window};

struct WireTooltip {
    app: WeakEntity<AurisApp>,
    id: SharedString,
    _updates: Option<Subscription>,
}

/// Build a card that follows the application's normal meter refresh cadence.
pub(super) fn build(app: WeakEntity<AurisApp>, id: SharedString, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        // The normal UI pulse already reads the lock-free meters. Reuse it while this card is
        // open instead of adding another timer or capturing the level at hover time.
        let updates = app
            .upgrade()
            .map(|app| cx.observe(&app, |_, _, cx| cx.notify()));
        WireTooltip {
            app,
            id,
            _updates: updates,
        }
    })
    .into()
}

fn dbfs(peak: f32) -> String {
    if !peak.is_finite() || peak <= 0.0 {
        return "−∞ dBFS".to_string();
    }
    let db = 20.0 * peak.log10();
    if db.abs() < 0.05 {
        "0.0 dBFS".to_string()
    } else if db > 0.0 {
        format!("+{db:.1} dBFS")
    } else {
        format!("{db:.1} dBFS")
    }
}

impl AurisApp {
    fn wire_node_name(&self, node: MixerNode) -> Option<String> {
        let owner = match node {
            MixerNode::Strip(owner) | MixerNode::Effect(owner, _) => owner,
        };
        let mixer = match owner {
            Some(id) => Some(self.project().track(id)?),
            None => None,
        };
        let name = mixer.map_or_else(
            || self.t(Key::Master).to_string(),
            |track| format!("{} #{}", track.name, track.id.0),
        );
        match node {
            MixerNode::Strip(_) => Some(name),
            MixerNode::Effect(_, slot) => {
                let effect = mixer
                    .map_or(&self.project().master, |track| &track.mixer)
                    .effects
                    .iter()
                    .find(|effect| effect.id == slot)?;
                Some(format!(
                    "{name} / {} #{}",
                    self.effect_label(slot, &effect.effect_id),
                    slot.0
                ))
            }
        }
    }
}

impl Render for WireTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let app = app.read(cx);
        let Some(edge) = app
            .mixer_canvas
            .as_ref()
            .and_then(|canvas| {
                canvas
                    .document
                    .edges
                    .iter()
                    .find(|edge| edge_id(edge) == self.id)
            })
            .copied()
        else {
            return div().into_any_element();
        };
        let (Some(source), Some(target)) = (
            app.wire_node_name(edge.source),
            app.wire_node_name(edge.target),
        ) else {
            return div().into_any_element();
        };
        let kind = match edge.kind {
            MixerEdgeKind::Output => app.t(Key::OutputShort).to_string(),
            MixerEdgeKind::Send {
                send, pre_fader, ..
            } => format!(
                "{} #{}{}",
                app.t(Key::Sends),
                send.0,
                if pre_fader {
                    format!(" · {}", app.t(Key::SendPreFaderMark))
                } else {
                    String::new()
                }
            ),
            MixerEdgeKind::Sidechain => app.t(Key::MenuSidechain).to_string(),
            MixerEdgeKind::Notes => app.t(Key::MixerNotesWire).to_string(),
            MixerEdgeKind::Chain => app.t(Key::Effects).to_string(),
        };
        let theme = cx.global::<Theme>();
        let level = dbfs(app.session.mixer_edge_peak(&edge));
        let mut card = div()
            .id("mixer-wire-tooltip")
            .debug_selector(|| "mixer-wire-tooltip".to_string())
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .max_w_96()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .font(theme.font.clone())
            .text_xs()
            .text_color(theme.text)
            .child(
                div()
                    .debug_selector({
                        let kind = kind.clone();
                        move || format!("mixer-wire-tooltip-kind:{kind}")
                    })
                    .text_color(theme.text_muted)
                    .child(kind),
            );
        for (key, value, field) in [
            (Key::MixerConnectionFrom, source, "from"),
            (Key::MixerConnectionTo, target, "to"),
            (Key::MixerSignalLevel, level, "level"),
        ] {
            let selector = format!("mixer-wire-tooltip-{field}:{value}");
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_color(theme.text_muted).child(app.t(key)))
                    .child(div().debug_selector(move || selector.clone()).child(value)),
            );
        }
        card.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::dbfs;

    #[test]
    fn dbfs_reports_silence_full_scale_clipping_and_very_quiet_signals() {
        for peak in [0., -1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(dbfs(peak), "−∞ dBFS");
        }
        for (peak, expected) in [
            (0.5, "-6.0 dBFS"),
            (0.1, "-20.0 dBFS"),
            (1., "0.0 dBFS"),
            (0.9999, "0.0 dBFS"),
            (2., "+6.0 dBFS"),
            (1e-8, "-160.0 dBFS"),
        ] {
            assert_eq!(dbfs(peak), expected);
        }
    }
}
