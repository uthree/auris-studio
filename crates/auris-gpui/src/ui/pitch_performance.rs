//! Automatic pitch controls for monophonic instrument clips.
use crate::{
    app::AurisApp,
    ui::{
        performance::PerformDial,
        widgets::{ButtonStyle, button},
    },
};
use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Context, IntoElement, prelude::*};

/// One parameter of a derived pitch contour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchDial {
    /// Scoop depth.
    Scoop,
    /// Scoop duration.
    ScoopTime,
    /// Vibrato depth.
    Vibrato,
    /// Vibrato rate.
    VibratoRate,
    /// Vibrato onset delay.
    VibratoDelay,
    /// Fall depth.
    Fall,
    /// Fall duration.
    FallTime,
    /// Connection duration on each side of the note boundary.
    Glide,
}

impl PitchDial {
    pub(crate) fn label(self) -> Key {
        match self {
            Self::Scoop => Key::PerformPitchScoop,
            Self::ScoopTime => Key::PerformPitchScoopTime,
            Self::Vibrato => Key::PerformPitchVibrato,
            Self::VibratoRate => Key::PerformPitchRate,
            Self::VibratoDelay => Key::PerformPitchDelay,
            Self::Fall => Key::PerformPitchFall,
            Self::FallTime => Key::PerformPitchFallTime,
            Self::Glide => Key::PerformPitchGlide,
        }
    }
    fn range(self) -> (f32, f32) {
        match self {
            Self::Scoop => (0.0, 4.0),
            Self::ScoopTime => (10.0, 300.0),
            Self::Vibrato => (0.0, 1.0),
            Self::VibratoRate => (2.0, 9.0),
            Self::VibratoDelay => (0.0, 1000.0),
            Self::Fall => (0.0, 12.0),
            Self::FallTime => (10.0, 500.0),
            Self::Glide => (0.0, 250.0),
        }
    }
    fn value(self, settings: &PitchPerformance) -> f32 {
        match self {
            Self::Scoop => settings.scoop,
            Self::ScoopTime => settings.scoop_ms,
            Self::Vibrato => settings.vibrato,
            Self::VibratoRate => settings.vibrato_hz,
            Self::VibratoDelay => settings.vibrato_delay_ms,
            Self::Fall => settings.fall,
            Self::FallTime => settings.fall_ms,
            Self::Glide => settings.glide_ms,
        }
    }
    pub(crate) fn fraction(self, stack: &[NoteTransform]) -> f32 {
        let (min, max) = self.range();
        (self.value(&pitch_settings(stack)).clamp(min, max) - min) / (max - min)
    }
    pub(crate) fn text(self, stack: &[NoteTransform]) -> String {
        let value = self.value(&pitch_settings(stack));
        match self {
            Self::Scoop | Self::Vibrato | Self::Fall => format!("{value:.2} st"),
            Self::VibratoRate => format!("{value:.1} Hz"),
            _ => format!("{value:.0} ms"),
        }
    }
    pub(crate) fn change(self, stack: &[NoteTransform], fraction: f32) -> Vec<NoteTransform> {
        let mut settings = pitch_settings(stack);
        let (min, max) = self.range();
        let value = min + (max - min) * fraction.clamp(0.0, 1.0);
        match self {
            Self::Scoop => settings.scoop = value,
            Self::ScoopTime => settings.scoop_ms = value,
            Self::Vibrato => settings.vibrato = value,
            Self::VibratoRate => settings.vibrato_hz = value,
            Self::VibratoDelay => settings.vibrato_delay_ms = value,
            Self::Fall => settings.fall = value,
            Self::FallTime => settings.fall_ms = value,
            Self::Glide => settings.glide_ms = value,
        }
        let mut out = stack.to_vec();
        if let Some(stage) = out
            .iter_mut()
            .find(|t| matches!(t, NoteTransform::Pitch { .. }))
        {
            *stage = NoteTransform::Pitch { settings };
        } else {
            out.push(NoteTransform::Pitch { settings });
        }
        out
    }
}

pub(crate) fn pitch_settings(stack: &[NoteTransform]) -> PitchPerformance {
    stack
        .iter()
        .find_map(|t| {
            if let NoteTransform::Pitch { settings } = t {
                Some(settings.clone())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

impl AurisApp {
    pub(crate) fn pitch_detail_rows(
        &self,
        clip: ClipId,
        stack: &[NoteTransform],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if self.editing_a_singer_clip()
            || self.project().midi_clip(clip).is_some_and(|(id, _)| {
                self.project()
                    .tracks
                    .iter()
                    .any(|t| t.id == id && t.kind.is_drum())
            })
        {
            return Vec::new();
        }
        let mut rows = vec![
            button(
                "perform-pitch-details",
                self.t(Key::PerformPitchSettings),
                ButtonStyle::Ghost,
                self.performance_details[3],
                self.theme.accent_soft,
                &self.theme,
                cx.listener(|this, _, _, cx| {
                    this.performance_details[3] = !this.performance_details[3];
                    cx.notify();
                }),
            )
            .min_w_0()
            .overflow_hidden()
            .into_any_element(),
        ];
        if self.performance_details[3] {
            for dial in [
                PitchDial::Scoop,
                PitchDial::ScoopTime,
                PitchDial::Vibrato,
                PitchDial::VibratoRate,
                PitchDial::VibratoDelay,
                PitchDial::Fall,
                PitchDial::FallTime,
                PitchDial::Glide,
            ] {
                rows.push(self.performance_slider(clip, PerformDial::Pitch(dial), stack, cx));
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, drag_to, paint, press, release, resize, with_a_clip};
    use gpui::{TestAppContext, point, px, size};

    #[gpui::test]
    fn pitch_controls_refresh_the_read_only_curve_during_drag_and_undo(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(1920)))
                .unwrap();
            this.open_clip_in_editor(clip);
        });
        resize(&app, cx, size(px(1920.0), px(2200.0)));
        click("score-performed", cx);
        click("perform-pitch-details", cx);
        paint(&app, cx);
        let at = cx.debug_bounds("perform-dial-20").unwrap().center();
        let end = point(at.x + px(60.0), at.y);
        press(cx, at);
        drag_to(cx, end);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.score_preview_bend()[0].value < -0.1);
            assert!(this.session.midi_clip(clip).unwrap().bend.is_empty());
            assert_eq!(
                this.session.midi_clip(clip).unwrap().notes[0].length,
                Ticks(1920)
            );
        });
        release(cx, end);
        app.update(cx, |this, _| this.undo());
        paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.score_preview_bend().is_empty()));
        app.update(cx, |this, _| this.redo());
        paint(&app, cx);
        app.read_with(cx, |this, _| assert!(!this.score_preview_bend().is_empty()));
    }
}
