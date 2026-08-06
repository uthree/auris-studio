//! The amplitude envelope, drawn as the shape it is rather than as four numbers.
//!
//! Attack, decay, sustain and release were four sliders, which is a correct and unreadable way to
//! present them: nobody hears four numbers, they hear a shape, and the shape is the thing being
//! adjusted. Logic draws it as a polyline with handles on the corners, and so does this.
//!
//! The sliders stay underneath. A graph is how a shape is *found* and a number is how it is
//! *said* — five milliseconds is a value somebody types, not a pixel they aim at — and neither
//! answers for the other.
//!
//! Everything above the view half is a free function with tests, on the rule this window already
//! follows: a gpui view cannot be unit-tested, so what it decides lives outside it.

/// How much of the width each of the three timed segments may take.
const SEGMENT: f32 = 0.26;

/// The flat stretch that shows the sustain level, as a share of the width.
///
/// Fixed, and not a time at all: a note is held for as long as it is held, and the envelope has no
/// opinion about that. It is here so the sustain level has somewhere to be *seen* — without it the
/// decay corner and the release corner would be the same point and the level would be a slope.
const HOLD: f32 = 0.22;

/// An envelope, and the ranges its three times move in.
///
/// The ranges travel with the values because every function here has to map a pixel back to a
/// number, and a plugin whose attack tops out at two seconds and one that tops out at eight are
/// the same picture with a different scale behind it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Adsr {
    /// Seconds to full level.
    pub attack: f32,
    /// Seconds from full level down to the sustain level.
    pub decay: f32,
    /// The level a held note settles at, from 0 to 1.
    pub sustain: f32,
    /// Seconds from the sustain level back to silence.
    pub release: f32,
    /// The longest attack, decay and release this plugin accepts.
    pub longest: (f32, f32, f32),
}

/// Which corner of the envelope a drag has hold of.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Handle {
    /// The top of the ramp. Moves along the time axis only.
    Attack,
    /// Where the decay lands. The one handle that moves in both directions at once — sideways is
    /// how long the fall takes and upwards is how far it falls to, which is exactly the pair a
    /// person is thinking about when they reach for it.
    Decay,
    /// The end of the release. Along the time axis only.
    Release,
}

impl Handle {
    /// Which parameter a drag on this handle should be recorded against.
    ///
    /// The decay handle moves two of them; the undo step is one either way, because the whole
    /// drag is one transaction, so this only decides which name the step is filed under.
    pub fn key(self) -> &'static str {
        match self {
            Handle::Attack => "attack",
            Handle::Decay => "decay",
            Handle::Release => "release",
        }
    }
}

/// Where a time sits along its segment, from 0 at none to 1 at the longest.
///
/// Cube-rooted rather than proportional. A default attack is five milliseconds against a
/// two-second range — a quarter of one per cent, which at the width of a plugin window is most of
/// a pixel, and a handle a pixel wide is a handle nobody can grab. The ear is not proportional in
/// time either: the step from 5 ms to 50 ms is enormous and the step from 1.5 s to 1.55 s is
/// nothing, so the axis that spends its room at the short end is the one that matches hearing.
pub fn time_share(seconds: f32, longest: f32) -> f32 {
    if longest <= 0.0 || !seconds.is_finite() {
        return 0.0;
    }
    (seconds / longest).clamp(0.0, 1.0).cbrt()
}

/// The time a share along a segment stands for. The inverse of [`time_share`].
pub fn time_at(share: f32, longest: f32) -> f32 {
    let share = share.clamp(0.0, 1.0);
    share * share * share * longest.max(0.0)
}

/// The five points of the envelope in a unit box: x rightwards from 0, y *upwards* from 0.
///
/// Upwards because that is what the numbers mean — the sustain level is a level. Turning it the
/// way a screen wants is the painter's job and belongs where the pixels are.
pub fn envelope_points(env: Adsr) -> [(f32, f32); 5] {
    let (la, ld, lr) = env.longest;
    let attack = SEGMENT * time_share(env.attack, la);
    let decay = SEGMENT * time_share(env.decay, ld);
    let release = SEGMENT * time_share(env.release, lr);
    let sustain = env.sustain.clamp(0.0, 1.0);
    [
        (0.0, 0.0),
        (attack, 1.0),
        (attack + decay, sustain),
        (attack + decay + HOLD, sustain),
        (attack + decay + HOLD + release, 0.0),
    ]
}

/// Where the three draggable handles sit in the unit box.
pub fn handles(env: Adsr) -> [(Handle, (f32, f32)); 3] {
    let points = envelope_points(env);
    [
        (Handle::Attack, points[1]),
        (Handle::Decay, points[2]),
        (Handle::Release, points[4]),
    ]
}

/// The handle nearest `at` and within `radius` of it, or nothing.
///
/// A radius per axis, because the box is wider than it is tall and a grab zone measured in pixels
/// is a different fraction of each. Nearest rather than first, so two handles that have been
/// dragged on top of each other still resolve to the one under the pointer.
pub fn handle_at(env: Adsr, at: (f32, f32), radius: (f32, f32)) -> Option<Handle> {
    let (rx, ry) = (radius.0.max(f32::EPSILON), radius.1.max(f32::EPSILON));
    handles(env)
        .into_iter()
        .filter_map(|(handle, point)| {
            // Scaled into a circle so "within the zone" means the same thing in both directions.
            let dx = (point.0 - at.0) / rx;
            let dy = (point.1 - at.1) / ry;
            let distance = dx * dx + dy * dy;
            (distance <= 1.0).then_some((handle, distance))
        })
        .min_by(|one, other| one.1.total_cmp(&other.1))
        .map(|(handle, _)| handle)
}

/// The envelope after `handle` has been dragged to `at` in the unit box.
///
/// The handles further along the envelope move with the ones before them, which is what makes the
/// picture readable: dragging the attack out pushes the whole shape right rather than eating the
/// decay. So a drag is measured from where its own segment *starts*, and that start is read from
/// the values the drag is not changing.
pub fn moved(env: Adsr, handle: Handle, at: (f32, f32)) -> Adsr {
    let (la, ld, lr) = env.longest;
    let attack = SEGMENT * time_share(env.attack, la);
    let decay = SEGMENT * time_share(env.decay, ld);
    match handle {
        Handle::Attack => Adsr {
            attack: time_at(at.0 / SEGMENT, la),
            ..env
        },
        Handle::Decay => Adsr {
            decay: time_at((at.0 - attack) / SEGMENT, ld),
            sustain: at.1.clamp(0.0, 1.0),
            ..env
        },
        Handle::Release => Adsr {
            release: time_at((at.0 - attack - decay - HOLD) / SEGMENT, lr),
            ..env
        },
    }
}

// ---------------------------------------------------------------- the view

use auris_session::prelude::*;
use gpui::{
    Bounds, IntoElement, MouseDownEvent, Pixels, Point, Window, canvas, div, point, prelude::*, px,
    size,
};

use crate::app::{AurisApp, Drag};
use crate::theme::Theme;
use crate::ui::paint;
use crate::ui::plugin_window::PluginSubject;

/// How tall the graph is drawn.
const GRAPH_HEIGHT: f32 = 84.0;

/// How near a corner a press has to land to take hold of it, in pixels.
const GRAB: f32 = 9.0;

/// How large a corner is drawn.
const HANDLE_RADIUS: f32 = 3.5;

impl AurisApp {
    /// The envelope of `subject`'s plugin, if it has all four of the numbers one is made of.
    ///
    /// All four or none. Three of them is not an envelope with a piece missing, it is a different
    /// shape — the drum synth has a decay and nothing else, and drawing a ramp with two corners
    /// invented for it would be a picture of something that is not happening.
    pub(crate) fn envelope_of(&mut self, subject: PluginSubject, plugin_id: &str) -> Option<Adsr> {
        let descriptors = self.session.param_descriptors(plugin_id);
        let find = |key: &str| descriptors.iter().find(|entry| entry.key == key);
        let (attack, decay, sustain, release) = (
            find("attack")?,
            find("decay")?,
            find("sustain")?,
            find("release")?,
        );
        let read = |descriptor: &ParamDescriptor| {
            self.session
                .param_value(subject.param_target(descriptor.id), descriptor)
        };
        Some(Adsr {
            attack: read(attack),
            decay: read(decay),
            sustain: read(sustain),
            release: read(release),
            longest: (attack.max, decay.max, release.max),
        })
    }

    /// The parameter behind one of the envelope's keys.
    fn envelope_target(
        &mut self,
        subject: PluginSubject,
        plugin_id: &str,
        key: &str,
    ) -> Option<ParamTarget> {
        self.session
            .param_descriptors(plugin_id)
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| subject.param_target(entry.id))
    }

    /// The envelope graph, drawn above the plugin's own controls.
    pub(crate) fn envelope_display(
        &mut self,
        subject: PluginSubject,
        env: Adsr,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let recorded = self.canvas.envelope.clone();
        let painted = theme.clone();
        div()
            .h(px(GRAPH_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .px_2()
            .py_1()
            .bg(theme.surface_sunken)
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    move |bounds, _, window, _| paint_envelope(window, bounds, env, &painted),
                )
                .size_full(),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.press_envelope(subject, event.position);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Takes hold of whichever corner the press landed on.
    fn press_envelope(&mut self, subject: PluginSubject, at: Point<Pixels>) {
        let Some(bounds) = self.canvas.envelope.get() else {
            return;
        };
        let Some((plugin_id, _)) = self.resolve_plugin(subject) else {
            return;
        };
        let Some(env) = self.envelope_of(subject, &plugin_id) else {
            return;
        };
        let radius = (
            GRAB / f32::from(bounds.size.width).max(1.0),
            GRAB / f32::from(bounds.size.height).max(1.0),
        );
        let Some(handle) = handle_at(env, unit_of(bounds, at), radius) else {
            return;
        };
        let Some(target) = self.envelope_target(subject, &plugin_id, handle.key()) else {
            return;
        };
        self.begin_drag(Drag::EnvelopeHandle {
            subject,
            handle,
            target,
        });
    }

    /// Moves the corner in hand to where the pointer is.
    ///
    /// The values are read afresh rather than carried in the drag, so the two numbers the decay
    /// corner moves stay consistent with each other and with anything else that changed underneath.
    pub(crate) fn drag_envelope_handle(
        &mut self,
        subject: PluginSubject,
        handle: Handle,
        at: Point<Pixels>,
    ) {
        let Some(bounds) = self.canvas.envelope.get() else {
            return;
        };
        let Some((plugin_id, _)) = self.resolve_plugin(subject) else {
            return;
        };
        let Some(env) = self.envelope_of(subject, &plugin_id) else {
            return;
        };
        let after = moved(env, handle, unit_of(bounds, at));
        for (key, value) in [
            ("attack", after.attack),
            ("decay", after.decay),
            ("sustain", after.sustain),
            ("release", after.release),
        ] {
            if let Some(target) = self.envelope_target(subject, &plugin_id, key) {
                self.session.set_param(target, value);
            }
        }
    }
}

/// A window position as a point in the graph's unit box, y running upwards.
fn unit_of(bounds: Bounds<Pixels>, at: Point<Pixels>) -> (f32, f32) {
    let width = f32::from(bounds.size.width).max(1.0);
    let height = f32::from(bounds.size.height).max(1.0);
    (
        (f32::from(at.x - bounds.origin.x) / width).clamp(0.0, 1.0),
        1.0 - (f32::from(at.y - bounds.origin.y) / height).clamp(0.0, 1.0),
    )
}

/// Draws the envelope and its corners.
fn paint_envelope(window: &mut Window, bounds: Bounds<Pixels>, env: Adsr, theme: &Theme) {
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    // Turned the way a screen wants it here and nowhere else: everything above this line is in
    // the coordinates the numbers mean, where a sustain level of 1 is loud.
    let at = |unit: (f32, f32)| point(px(left + width * unit.0), px(top + height * (1.0 - unit.1)));

    let points: Vec<Point<Pixels>> = envelope_points(env).into_iter().map(at).collect();
    paint::area_under(
        window,
        &points,
        bounds.origin.y + bounds.size.height,
        Theme::translucent(theme.accent, 0.22),
    );
    paint::polyline(window, &points, px(1.5), theme.accent);

    for (_, unit) in handles(env) {
        let centre = at(unit);
        paint::rounded_rect(
            window,
            Bounds {
                origin: point(centre.x - px(HANDLE_RADIUS), centre.y - px(HANDLE_RADIUS)),
                size: size(px(HANDLE_RADIUS * 2.0), px(HANDLE_RADIUS * 2.0)),
            },
            px(HANDLE_RADIUS),
            theme.accent,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adsr() -> Adsr {
        Adsr {
            attack: 0.005,
            decay: 0.12,
            sustain: 0.7,
            release: 0.15,
            longest: (2.0, 4.0, 4.0),
        }
    }

    #[test]
    fn a_time_survives_the_trip_to_the_axis_and_back() {
        // The handle is drawn from `time_share` and dragged through `time_at`, so a value that
        // did not round-trip would make the corner slide out from under the pointer holding it.
        for seconds in [0.0, 0.005, 0.05, 0.4, 1.0, 2.0] {
            let there = time_share(seconds, 2.0);
            let back = time_at(there, 2.0);
            assert!(
                (back - seconds).abs() < 0.001,
                "{seconds}s went to {there} and came back {back}"
            );
        }
        // Out of range clamps rather than running off the end of the display.
        assert_eq!(time_share(9.0, 2.0), 1.0);
        assert_eq!(time_share(-1.0, 2.0), 0.0);
        assert_eq!(
            time_share(1.0, 0.0),
            0.0,
            "a range of nothing is not a range"
        );
    }

    #[test]
    fn the_short_end_of_the_axis_is_where_the_room_is_spent() {
        // The whole reason the axis is not proportional. A default five-millisecond attack is a
        // quarter of one per cent of its range; proportionally that is under a pixel at the width
        // of a plugin window, and a handle nobody can grab is a control that does not exist.
        let share = time_share(0.005, 2.0);
        assert!(
            share > 0.10,
            "a five-millisecond attack got {share} of its segment"
        );
        // And it is still monotonic, or a longer attack would draw shorter than a short one.
        let mut last = -1.0;
        for seconds in [0.0, 0.001, 0.01, 0.1, 0.5, 1.0, 2.0] {
            let share = time_share(seconds, 2.0);
            assert!(share > last, "{seconds}s went backwards");
            last = share;
        }
    }

    #[test]
    fn the_shape_is_the_one_the_numbers_describe() {
        let points = envelope_points(adsr());
        assert_eq!(points[0], (0.0, 0.0), "it starts from silence");
        assert_eq!(points[1].1, 1.0, "the attack reaches full level");
        assert_eq!(points[2].1, 0.7, "and falls to the sustain level");
        assert_eq!(points[3].1, 0.7, "which is then held");
        assert_eq!(points[4].1, 0.0, "and released to silence");

        // Time only ever runs forwards, and the whole shape fits the box it is drawn in.
        for pair in points.windows(2) {
            assert!(pair[1].0 >= pair[0].0, "{points:?} goes back in time");
        }
        assert!(
            points[4].0 <= 1.0,
            "the shape ran off the display: {points:?}"
        );

        // Everything at its longest fills the width exactly, which is what the two constants are
        // chosen to do — a shape that stopped short would leave dead space nobody could reach.
        let longest = envelope_points(Adsr {
            attack: 2.0,
            decay: 4.0,
            release: 4.0,
            ..adsr()
        });
        assert!((longest[4].0 - 1.0).abs() < 0.001, "{longest:?}");
    }

    #[test]
    fn dragging_a_handle_to_where_it_already_is_changes_nothing() {
        // The property that keeps a grabbed corner from jumping on the first pointer move.
        for (handle, at) in handles(adsr()) {
            let after = moved(adsr(), handle, at);
            let (a, d, s, r) = (after.attack, after.decay, after.sustain, after.release);
            let before = adsr();
            assert!(
                (a - before.attack).abs() < 0.002
                    && (d - before.decay).abs() < 0.002
                    && (s - before.sustain).abs() < 0.002
                    && (r - before.release).abs() < 0.002,
                "{handle:?} moved to its own position and gave {after:?}"
            );
        }
    }

    #[test]
    fn a_handle_moves_its_own_numbers_and_no_others() {
        let before = adsr();

        let attacked = moved(before, Handle::Attack, (SEGMENT, 1.0));
        assert!((attacked.attack - 2.0).abs() < 0.001, "dragged to the end");
        assert_eq!(
            (attacked.decay, attacked.sustain, attacked.release),
            (before.decay, before.sustain, before.release)
        );

        // The decay handle is the one that moves two, on purpose: sideways is how long the fall
        // takes and upwards is how far it falls to.
        let broken = moved(before, Handle::Decay, (0.5, 0.25));
        assert!(broken.decay > before.decay);
        assert!((broken.sustain - 0.25).abs() < 0.001);
        assert_eq!(
            (broken.attack, broken.release),
            (before.attack, before.release)
        );

        let released = moved(before, Handle::Release, (0.2, 0.0));
        assert_eq!(
            (released.attack, released.decay, released.sustain),
            (before.attack, before.decay, before.sustain)
        );
        // Dragged back past where its segment starts is zero rather than a negative time.
        assert_eq!(released.release, 0.0);
    }

    #[test]
    fn a_press_takes_the_handle_it_landed_on_and_nothing_else() {
        let env = adsr();
        let radius = (0.03, 0.06);
        for (handle, at) in handles(env) {
            assert_eq!(handle_at(env, at, radius), Some(handle));
        }
        assert_eq!(
            handle_at(env, (0.9, 0.9), radius),
            None,
            "empty space claimed a handle"
        );
        // The line between two handles belongs to neither: grabbing it would move a corner the
        // pointer is nowhere near.
        assert_eq!(handle_at(env, (0.5, 0.5), radius), None);
    }
}
