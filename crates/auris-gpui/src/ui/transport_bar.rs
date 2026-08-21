//! The transport bar across the top of the window.

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;

use gpui::{Axis, IntoElement, Pixels, Window, div, prelude::*, px};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{
    ButtonStyle, button, db_to_meter_position, icon_button, level_meter, readout,
};

/// Grid divisions offered in the transport bar, as a fraction of a quarter note.
///
/// The command palette offers the same seven, so that typing `1/16` reaches the grid the button
/// cycles to. One list, or the two would drift.
pub(crate) const GRID_CHOICES: [(&str, i64); 7] = [
    ("1/1", TICKS_PER_QUARTER * 4),
    ("1/2", TICKS_PER_QUARTER * 2),
    ("1/4", TICKS_PER_QUARTER),
    ("1/8", TICKS_PER_QUARTER / 2),
    ("1/16", TICKS_PER_QUARTER / 4),
    ("1/32", TICKS_PER_QUARTER / 8),
    // Off. A tick is the finest position the document can hold, so snapping to one is not
    // snapping — and `Key::GridFree` has labelled this state since the button was written, for
    // a value the cycle could never reach.
    (GRID_OFF_LABEL, 1),
];

/// Marks the entry whose label is translated rather than notation. See [`AurisApp::grid_label`].
const GRID_OFF_LABEL: &str = "";

/// What a finished take gets reported as.
///
/// A free function because the four answers are a decision — the two that produced no clip in
/// particular, neither of which is a failure and both of which read as one unless they are said
/// out loud — and a decision in a view is a decision with no test.
pub(crate) fn recording_summary(report: &RecordingReport, language: Language) -> String {
    let duration = Seconds(report.seconds).format_clock();
    match (report.clip.is_some(), report.dropped_frames) {
        // Played, but not where the punch was. Nothing is wrong with the microphone and saying
        // "the take was empty" would send somebody to check it.
        (false, _) if report.outside_punch => messages::recorded_outside_punch(language),
        (false, _) => messages::recorded_nothing(language),
        (true, 0) => messages::recorded_take(language, &duration),
        // The take is kept and usable, and everything after the gap has moved earlier by that
        // much. Something a person needs told now rather than found in a mix next week.
        (true, dropped) => messages::recorded_with_gaps(language, &duration, dropped),
    }
}

/// One captioned meter in the transport bar's right-hand column.
///
/// The master and the input are the same object with different numbers in it. Drawn from one
/// place so they stay the same width and the same height as each other: two blocks side by side
/// that disagree about either read as one of them having gone wrong.
fn meter_block(
    caption: &'static str,
    value: String,
    db: f32,
    theme: &crate::theme::Theme,
) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .justify_center()
        .w(px(124.0))
        .child(
            div()
                .flex()
                .justify_between()
                .text_xs()
                .child(div().text_color(theme.text_faint).child(caption))
                .child(div().text_color(theme.text_muted).child(value)),
        )
        .child(div().h(px(7.0)).mt_1().child(level_meter(
            db_to_meter_position(db),
            db_to_meter_position(db),
            Axis::Horizontal,
            theme.meter_color(db),
            theme,
        )))
}

/// The input peak, written for the caption row above its bar.
///
/// A dash below the meter's own floor rather than a number. `-inf dB` is what the arithmetic
/// says and it is not what somebody setting a level needs to read; a made-up floor like `-60.0`
/// is worse, because it sits there looking like a measurement while the bar beside it has
/// bottomed out. A dash says the same thing the empty bar does.
///
/// A free function because it is the rule that keeps the number and the bar telling one story,
/// and a rule inside a view is a rule with no test.
fn input_peak_text(db: f32) -> String {
    match db > crate::ui::widgets::METER_FLOOR_DB {
        true => format!("{db:.1} dB"),
        false => "—".to_string(),
    }
}

/// Ticks in one unit of the position readout's last field.
///
/// The readout has always shown hundredths of a beat rather than raw ticks, because 960 of
/// anything is a number nobody reads. Typing one back has to use the same unit, or the box would
/// be asking for something other than what the display just said.
const POSITION_UNIT: i64 = TICKS_PER_QUARTER / 96;

/// The playhead position as the readout writes it: bar, beat and hundredth, counting from one.
///
/// Nothing about the tempo comes into it. Bars are a function of the meter alone — the tempo map
/// decides how long a bar *lasts*, never where it *is* — and this used to be asked of the tempo
/// map anyway, which was the wrong object holding the right arithmetic.
pub fn format_position(at: Ticks, signatures: &SignatureMap) -> String {
    let (bar, beat, tick) = signatures.bar_beat_at(at);
    format!("{bar}.{beat}.{:03}", tick / POSITION_UNIT)
}

/// Reads a position back out of what [`format_position`] wrote.
///
/// Forgiving about how much of it is there — `17` is the top of bar seventeen and `17.3` is its
/// third beat — because the trailing fields are almost always zero and typing `.1.000` every time
/// is the sort of thing that makes people go back to the wheel. Strict about the rest: a beat past
/// the end of its bar is a typo rather than a way of writing the next bar, and answering it by
/// silently jumping somewhere else would be worse than saying no.
pub fn parse_position(text: &str, signatures: &SignatureMap) -> Option<Ticks> {
    let mut fields = text
        .split(['.', ':', '|', ' '])
        .filter(|field| !field.is_empty());
    let bar: i64 = fields.next()?.trim().parse().ok()?;
    let beat: i64 = fields
        .next()
        .map_or(Ok(1), |field| field.trim().parse())
        .ok()?;
    let unit: i64 = fields
        .next()
        .map_or(Ok(0), |field| field.trim().parse())
        .ok()?;
    if fields.next().is_some() {
        return None;
    }

    // Refused before the bar is looked up, because the bar is the one field with no upper bound:
    // sixteen typed nines is a number no timeline reaches, and `bar_start` would spend the whole
    // map walking towards it and overflow at the end. A bar past any song is a typo like any
    // other. The limit is generous — a thousand hours at 120 BPM.
    const BARS: std::ops::RangeInclusive<i64> = 1..=1_000_000;
    if !BARS.contains(&bar) {
        return None;
    }
    // Which meter the beat is checked against is the meter *of that bar*: `2.4` is a typo in 3/4
    // and a position in 4/4, and after a change the answer differs from bar to bar.
    let start = signatures.bar_start(bar as u32);
    let signature = signatures.signature_at(start);
    let beats_per_bar = i64::from(signature.numerator.max(1));
    let ticks_per_beat = signature.ticks_per_beat().raw().max(1);
    let units_per_beat = ticks_per_beat / POSITION_UNIT.max(1);
    if !(1..=beats_per_bar).contains(&beat) || !(0..units_per_beat).contains(&unit) {
        return None;
    }
    Some(start + Ticks((beat - 1) * ticks_per_beat + unit * POSITION_UNIT))
}

impl AurisApp {
    /// Renders the transport bar.
    pub(crate) fn render_transport(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let playing = self.is_playing();
        let looping = self.project().loop_enabled;
        let punching = self.project().punch_enabled;
        let clicking = self.session.metronome();
        let recording = self.session.is_recording();
        let playhead = self.playhead_ticks();
        let position = format_position(playhead, &self.project().signatures);
        let seconds = self.project().tempo_map.ticks_to_seconds(playhead);
        // The tempo and the meter where the playhead is, not the ones the song opens in: the
        // timeline holds changes of both, so the readouts follow the playhead through them and
        // editing either turns the stretch being listened to.
        let bpm = self.project().tempo_map.bpm_at(playhead);
        let signature = self.project().signatures.signature_at(playhead);
        let grid_label = self.grid_label();
        let master_db = gain_to_db(self.master_level());
        let master_gain_db = self.project().master.gain_db;
        // `None` while no input device is open, which is what decides whether the block is drawn
        // at all. The session answers it rather than the frontend inferring it from monitoring
        // and recording: those are two questions and the device is one fact.
        let input_db = self
            .session
            .input_is_open()
            .then(|| gain_to_db(self.input_level));

        // Three columns of equal weight, so the middle one lands on the window's centre line
        // however wide the sides grow. Every hardware transport and every DAW puts the
        // controls and the position readout there; anchoring them left makes the eye hunt for
        // the playhead position on a wide window.
        div()
            .flex()
            .items_center()
            .h(Metrics::TRANSPORT_HEIGHT)
            .px_3()
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    // Export lives in the File menu, with the other commands that write a file.
                    // A transport bar is for the transport; a button that opens a save dialog was
                    // the widest thing on it and the least often pressed.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::Grid)),
                            )
                            .child(button(
                                "grid",
                                grid_label,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.cycle_grid();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        // The arrangement's own zoom, next to the grid because both are about
                        // how finely the timeline reads rather than about what it contains.
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::Zoom)),
                            )
                            .child(self.zoom_slider("timeline-zoom", cx)),
                    ),
            )
            .child(
                // Logic stacks the transport over its readouts rather than running them along
                // one line, which keeps the buttons and the numbers on the window's centre line
                // instead of pushing one of them off it.
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                icon_button(
                                    "rtz",
                                    Icon::ToStart,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.seek(Ticks::ZERO);
                                        cx.notify();
                                    }),
                                )
                                .tooltip(self.tip(Key::CmdReturnToZero, "transport.return")),
                            )
                            .child(
                                icon_button(
                                    "stop",
                                    Icon::Stop,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.session.stop();
                                        this.seek(Ticks::ZERO);
                                        cx.notify();
                                    }),
                                )
                                // The one button up here that is not a bindable command, so the
                                // card carries a name and no chip. That it has no key is a thing
                                // to fix in the table rather than to hide by leaving it nameless.
                                .tooltip(self.tip(Key::CmdStop, "")),
                            )
                            .child(
                                icon_button(
                                    "play",
                                    if playing { Icon::Pause } else { Icon::Play },
                                    playing,
                                    theme.playing,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_play();
                                        cx.notify();
                                    }),
                                )
                                .tooltip(self.tip(Key::CmdPlayStop, "transport.play")),
                            )
                            // Beside Play, where a hardware transport puts it, and lit while a
                            // take is running so that a microphone nobody remembers arming is
                            // never live without something on screen saying so.
                            .child(
                                icon_button(
                                    "record",
                                    Icon::Record,
                                    recording,
                                    theme.record,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.toggle_recording(window, cx);
                                        cx.notify();
                                    }),
                                )
                                .tooltip(self.tip(Key::CmdRecord, "transport.record")),
                            )
                            // Beside Record rather than beside the cycle, though it looks like the
                            // cycle: this one decides what a take *keeps*, and everything to its
                            // left decides what the transport does.
                            .child(
                                icon_button(
                                    "punch",
                                    Icon::Punch,
                                    punching,
                                    theme.record,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_punch();
                                        cx.notify();
                                    }),
                                )
                                .tooltip(self.tip(Key::CmdTogglePunch, "transport.punch")),
                            )
                            .child(
                                icon_button(
                                    "loop",
                                    Icon::Loop,
                                    looping,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_loop();
                                        cx.notify();
                                    }),
                                )
                                .tooltip(self.tip(Key::CmdToggleCycle, "transport.loop")),
                            )
                            // Beside the cycle button because the two are the same kind of
                            // switch: neither writes anything, both change how a pass sounds
                            // while somebody is listening to it.
                            .child(
                                icon_button(
                                    "metronome",
                                    Icon::Metronome,
                                    clicking,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_metronome();
                                        cx.notify();
                                    }),
                                )
                                .tooltip(self.tip(Key::CmdToggleMetronome, "transport.metronome")),
                            ),
                    )
                    // Musical position, wall-clock position and tempo: the readouts every DAW
                    // shows, side by side under the buttons they describe.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                // The readout is a display; the double-click that turns it into
                                // an input goes on a wrapper, so that `readout` stays the plain
                                // thing every other transport uses.
                                div()
                                    .id("position")
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                            if event.click_count < 2 {
                                                return;
                                            }
                                            this.prompt_for_position();
                                            cx.notify();
                                        }),
                                    )
                                    .child(readout(
                                        self.t(Key::Position),
                                        position,
                                        Some(seconds.format_clock().into()),
                                        px(118.0),
                                        &theme,
                                    )),
                            )
                            .child(self.render_tempo_control(bpm, cx))
                            .child(self.render_signature_control(signature, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .gap_3()
                    // The panels are switched from the status bar, where every one of them has an
                    // icon whether it is showing or not. Four buttons up here said the same thing
                    // for three of the four, and a transport bar is for the transport.
                    //
                    // What is coming in, and only while something is: a bar reading silence
                    // whenever no device is open would be a bar that says the microphone is dead.
                    // Left of the master, which is the order the signal travels in.
                    .children(input_db.map(|db| {
                        meter_block(self.t(Key::InputMeter), input_peak_text(db), db, &theme)
                    }))
                    // Master level, always visible so clipping is never a surprise.
                    .child(meter_block(
                        self.t(Key::Master),
                        format!("{master_gain_db:+.1} dB"),
                        master_db,
                        &theme,
                    )),
            )
    }

    /// One field of the transport's readout: a faint caption over a value, in a sunken box.
    ///
    /// Logic's LCD is a row of these, and the point of sharing the chrome is that they read as a
    /// row rather than as a tempo box with something bolted beside it.
    fn lcd_field(
        &self,
        id: &'static str,
        caption: &'static str,
        value: String,
        width: Pixels,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = &self.theme;
        div()
            .id(id)
            .flex()
            .flex_col()
            .justify_center()
            .w(width)
            .px_2()
            .py_1()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(theme.border_subtle)
            .cursor_pointer()
            .hover(|this| this.border_color(theme.border))
            .child(div().text_xs().text_color(theme.text_faint).child(caption))
            .child(div().text_sm().text_color(theme.text).child(value))
    }

    /// A drag-and-scroll tempo readout.
    fn render_tempo_control(
        &self,
        bpm: f64,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        self.lcd_field("tempo", self.t(Key::Tempo), format!("{bpm:.2}"), px(84.0))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if event.click_count >= 2 {
                        // The first click of the pair already began a drag. It has not moved, so
                        // it has changed nothing, but leaving it live would have the sheet's own
                        // pointer dragging the tempo behind it.
                        this.end_drag(window, cx);
                        this.prompt_for_tempo();
                        cx.notify();
                        return;
                    }
                    let at = this.playhead_ticks();
                    let start_bpm = this.project().tempo_map.bpm_at(at);
                    this.begin_drag(Drag::Tempo {
                        at,
                        start_bpm,
                        start_x: event.position.x,
                    });
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                let at = this.playhead_ticks();
                let bpm = this.project().tempo_map.bpm_at(at) + f64::from(notches);
                this.session.set_tempo_at(at, bpm);
                cx.notify();
            }))
    }

    /// The time signature readout, beside the tempo it belongs next to.
    ///
    /// Logic's LCD puts the meter immediately right of the tempo, and this is that field: the
    /// signature in force where the playhead sits, written `4/4`.
    ///
    /// Clicking it drops the list of meters, which is what clicking Logic's own signature field
    /// does. No drag and no double-click, unlike the tempo: a tempo is a number to be found by
    /// feel, and a meter is one of a handful of things a person already knows the name of. The
    /// wheel still steps the beat count, for the one case where the answer is a near neighbour.
    fn render_signature_control(
        &self,
        signature: TimeSignature,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        self.lcd_field(
            "signature",
            self.t(Key::Signature),
            signature.to_string(),
            px(64.0),
        )
        // Either button, because either is a way somebody might ask a control for its list, and
        // there is nothing else here for the right one to mean.
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(Self::open_signature_menu),
        )
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(Self::open_signature_menu),
        )
        .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
            let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
            let at = this.playhead_ticks();
            let current = this.session.signature_at(at);
            // Clamped rather than wrapped: rolling past the end of the range should stop at
            // the last meter there is, not come back round at one beat to the bar.
            let beats = (current.numerator as i64 + notches.round() as i64).clamp(
                *TimeSignature::NUMERATORS.start() as i64,
                *TimeSignature::NUMERATORS.end() as i64,
            );
            this.session
                .set_signature_at(at, TimeSignature::new(beats as u32, current.denominator));
            cx.notify();
        }))
    }

    /// Drops the signature field's list of meters, aimed where the playhead is.
    fn open_signature_menu(
        &mut self,
        event: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let menu = self.signature_menu(event.position, self.playhead_ticks());
        self.open_menu(menu);
        cx.notify();
    }

    /// Opens the sheet that takes a tempo as a number.
    ///
    /// Aimed where the playhead sits when the sheet opens — and held there, so a sheet typed
    /// into during playback still edits the stretch it came up showing.
    pub(crate) fn prompt_for_tempo(&mut self) {
        let title = self.t(Key::SetTempoTitle);
        let at = self.playhead_ticks();
        let current = format!("{:.2}", self.project().tempo_map.bpm_at(at));
        self.open_prompt(Prompt::new(title, PromptTarget::Tempo(at), current));
    }

    /// Opens the sheet that takes a position as bar, beat and hundredth.
    pub(crate) fn prompt_for_position(&mut self) {
        let title = self.t(Key::SetPositionTitle);
        let current = format_position(self.playhead_ticks(), &self.project().signatures);
        self.open_prompt(Prompt::new(title, PromptTarget::Position, current));
    }

    /// Opens the sheet that takes a meter as `6/8`, aimed at the stretch the playhead is in.
    ///
    /// The list on the button covers what nearly everybody wants; this is the way to 11/8.
    pub(crate) fn prompt_for_signature(&mut self) {
        let at = self.playhead_ticks();
        let title = self.t(Key::SetSignatureTitle);
        let current = self.session.signature_at(at).to_string();
        self.open_prompt(Prompt::new(title, PromptTarget::Signature(at), current));
    }

    /// Turns looping on or off.
    pub(crate) fn toggle_loop(&mut self) {
        let enabled = self.project().loop_enabled;
        self.session.set_loop_enabled(!enabled);
    }

    /// Turns punch recording on or off, and says which it now is.
    ///
    /// The status line does the work the button cannot: the region is drawn on the lanes, but a
    /// switch that only trims a take somebody has not recorded yet gives no other sign it is on
    /// until the take comes back shorter than they played it.
    pub(crate) fn toggle_punch(&mut self) {
        self.session
            .set_punch_enabled(!self.session.punch_enabled());
        let key = match self.session.punch_enabled() {
            true => Key::PunchOn,
            false => Key::PunchOff,
        };
        self.set_status(self.t(key));
    }

    /// Ends a punched take that has rolled past its punch-out.
    ///
    /// Called from the frame loop, where the playhead crossing a position is noticed. Stops the
    /// transport as well, for the reason the Record button does: a take that ended while the song
    /// played on would leave the playhead somewhere other than the clip that just appeared.
    pub(crate) fn finish_punch(&mut self) {
        let Some(result) = self.session.finish_punch() else {
            return;
        };
        match result {
            Ok(report) => {
                self.session.stop();
                self.set_status(recording_summary(&report, self.language));
            }
            Err(error) => self.report_session_error(&error),
        }
    }

    /// Turns the click on or off, and says which it now is.
    ///
    /// The status line is the whole of the feedback when the transport is stopped: the button
    /// lights up, but a click that only makes a sound once playback starts is otherwise a
    /// press with no answer to it.
    pub(crate) fn toggle_metronome(&mut self) {
        self.session.toggle_metronome();
        let key = match self.session.metronome() {
            true => Key::MetronomeOn,
            false => Key::MetronomeOff,
        };
        self.set_status(self.t(key));
    }

    /// The track a take would land on: the armed one, or the selected track when it is audio.
    ///
    /// The selection lives here rather than in the session — a session has no window and so no
    /// idea what is selected — so every question about where a take would go comes through this.
    pub(crate) fn record_target(&self) -> Option<TrackId> {
        self.session.record_target(self.selected_track)
    }

    /// Arms an audio track for recording, or disarms it if it already was.
    ///
    /// One at a time, so clicking a second track's button moves the arm rather than adding one:
    /// there is one input stream, and it has to go somewhere in particular.
    ///
    /// Arming is no longer how a take is aimed — selecting the track does that — so this is the
    /// override: it pins a take to a track that is not the one being looked at, and clicking it
    /// off hands the aim back to the selection.
    pub(crate) fn toggle_arm(&mut self, track: TrackId) {
        let wanted = match self.session.armed_track() == Some(track) {
            true => None,
            false => Some(track),
        };
        if let Err(error) = self.session.arm_track(wanted) {
            self.report_session_error(&error);
        }
    }

    /// Plays the live input through `track`, or stops doing so if it already was.
    ///
    /// One track at a time, like the arm and for the same reason: one input stream, and it has to
    /// come out somewhere in particular.
    ///
    /// The status line names the device *and* what listening this way costs, every time rather
    /// than once. Somebody recording through an interface that monitors in hardware and who turns
    /// this on as well hears themselves twice, a few milliseconds apart, and the fix is to turn
    /// one of them off — which is only obvious to somebody who has been told there are two.
    pub(crate) fn toggle_monitoring(&mut self, track: TrackId) {
        let wanted = match self.session.monitored_track() == Some(track) {
            true => None,
            false => Some(track),
        };
        if let Err(error) = self.session.set_monitoring(wanted) {
            let line = self.failure(Key::CmdToggleMonitoring, &error);
            self.set_failed_status(line);
            return;
        }
        self.monitor_gaps = 0;
        let line = match self.session.monitor_status() {
            Some(status) => messages::monitoring_on(self.language, &status.device),
            None => self.t(Key::MonitoringOff).to_string(),
        };
        self.set_status(line);
    }

    /// Says so when the monitor has broken up since this was last checked.
    ///
    /// Called from the frame loop, because a dropout is not something a command returns: it
    /// happens between commands, and the only evidence otherwise is a noise the user is left to
    /// interpret. Reported once per new gap rather than once per frame.
    pub(crate) fn report_monitor_gaps(&mut self) {
        let Some(status) = self.session.monitor_status() else {
            self.monitor_gaps = 0;
            return;
        };
        if status.rebuffers > self.monitor_gaps {
            self.monitor_gaps = status.rebuffers;
            self.set_status(messages::monitor_gaps(self.language, status.rebuffers));
        }
    }

    /// Starts a take, or ends the one that is running.
    ///
    /// Onto whatever [`record_target`](Self::record_target) names, so the ordinary way to record
    /// is to click an audio track and press this — the arm button is for aiming somewhere other
    /// than where the eye is.
    ///
    /// Starting rolls the transport as well, which the session deliberately does not do on its
    /// own — recording against nothing and recording against the song are both real, and which
    /// one a *button* means is a frontend's decision. Stopping stops both, because a take that
    /// ended while the song played on would leave the playhead somewhere other than where the
    /// clip that just appeared is.
    ///
    /// A project with nowhere to write gets the save dialog rather than a refusal. "Recording
    /// needs a project folder" is true and is a dead end; the answer to it is a question, and a
    /// button that asks it is a button that works the first time it is pressed.
    pub(crate) fn toggle_recording(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.session.is_recording() {
            self.finish_recording();
            return;
        }
        match self.session.start_recording(self.selected_track) {
            Ok(()) => {
                if !self.session.is_playing() {
                    self.session.play();
                }
                self.set_status(self.t(Key::RecordingStarted));
            }
            Err(SessionError::RecordingNeedsFolder) => {
                self.set_status(self.t(Key::RecordingNeedsSave));
                self.save_as_then(
                    Some(crate::ui::prompt::PendingAction::StartRecording),
                    window,
                    cx,
                );
            }
            Err(error) => self.report_session_error(&error),
        }
    }

    /// Stops the take that is running and says on the status line what became of it.
    ///
    /// Its own method because stopping is not only what the Record button does a second time: it
    /// is also what has to happen when the document is about to be closed. A take in flight is
    /// unsaved work that `is_dirty` cannot see — the document does not change until the take
    /// ends and the clip lands — and stopping it first is what turns it into work the ordinary
    /// unsaved-changes question already knows how to ask about.
    pub(crate) fn finish_recording(&mut self) {
        match self.session.stop_recording() {
            Ok(report) => {
                self.session.stop();
                self.set_status(recording_summary(&report, self.language));
            }
            Err(error) => self.report_session_error(&error),
        }
    }

    /// Puts a recording failure on the status line, named after the command that failed.
    fn report_session_error(&mut self, error: &SessionError) {
        let line = self.failure(Key::CmdRecord, error);
        self.set_failed_status(line);
    }

    /// Steps the editing grid to the next finer division, wrapping at the end.
    pub(crate) fn cycle_grid(&mut self) {
        let current = self.project().grid.raw();
        let index = GRID_CHOICES
            .iter()
            .position(|(_, ticks)| *ticks == current)
            .unwrap_or(2);
        let (_, ticks) = GRID_CHOICES[(index + 1) % GRID_CHOICES.len()];
        self.session.set_grid(Ticks(ticks));
    }
}

impl AurisApp {
    /// The grid division as it appears on the button.
    ///
    /// The fractions are notation rather than words, so only the fallback needs translating.
    pub(crate) fn grid_label(&self) -> &'static str {
        GRID_CHOICES
            .iter()
            .find(|(_, ticks)| *ticks == self.project().grid.raw())
            .map(|(label, _)| *label)
            .filter(|label| *label != GRID_OFF_LABEL)
            .unwrap_or_else(|| self.t(Key::GridFree))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4/4 for the whole timeline.
    fn four_four() -> SignatureMap {
        SignatureMap::default()
    }

    #[test]
    fn the_input_readout_goes_quiet_exactly_where_its_bar_does() {
        use crate::ui::widgets::{METER_FLOOR_DB, db_to_meter_position};

        // The pair this test exists for: a number and a bar, and the frame where one says
        // something the other does not.
        for db in [METER_FLOOR_DB, -80.0, f32::NEG_INFINITY] {
            assert_eq!(db_to_meter_position(db), 0.0, "{db} moves the bar");
            assert_eq!(input_peak_text(db), "—", "{db} is written as a level");
        }

        // And above it both have something to say.
        for db in [-59.0, -12.0, 0.0] {
            assert!(db_to_meter_position(db) > 0.0, "{db} leaves the bar empty");
            assert!(
                input_peak_text(db).ends_with(" dB"),
                "{db} is written as silence"
            );
        }
    }

    #[test]
    fn a_bar_past_any_timeline_is_refused_rather_than_wrapped() {
        // Sixteen nines parse fine as an i64 and then overflow it multiplied by a bar of
        // ticks: a debug build panicked on the multiply, a release build seeked to a wrapped
        // nonsense position. A number too large to mean anything is a typo like any other.
        assert_eq!(parse_position("9999999999999999", &four_four()), None);
        assert_eq!(parse_position(&i64::MAX.to_string(), &four_four()), None);
        // While a merely enormous song is still addressable.
        assert!(parse_position("1000000", &four_four()).is_some());
    }

    #[test]
    fn a_position_reads_back_as_the_one_the_readout_showed() {
        // The box is opened by double-clicking the readout and comes up holding what it said, so
        // a value that did not survive the round trip would move the playhead on a Return that
        // was meant to change nothing.
        for ticks in [0, 1, 240, 960, 3840, 3840 * 16 + 960 + 250] {
            let at = Ticks(ticks - ticks % POSITION_UNIT);
            let text = format_position(at, &four_four());
            assert_eq!(
                parse_position(&text, &four_four()),
                Some(at),
                "`{text}` did not read back as {}",
                at.raw()
            );
        }

        // And through a meter change, which is where the round trip earns its keep: bar 5 is
        // where the 3/4 begins, and a readout that said `5.1.000` has to come back to that tick
        // rather than to five times a bar of four.
        let mut mixed = SignatureMap::default();
        mixed.set_point(Ticks::from_beats(16.0), TimeSignature::new(3, 4));
        for bar in 1..12u32 {
            let at = mixed.bar_start(bar);
            let text = format_position(at, &mixed);
            assert_eq!(
                text,
                format!("{bar}.1.000"),
                "bar {bar} was written as `{text}`"
            );
            assert_eq!(parse_position(&text, &mixed), Some(at));
        }
        // The beat is checked against the meter of *that* bar, not the song's opening one.
        assert!(parse_position("6.3", &mixed).is_some());
        assert_eq!(parse_position("6.4", &mixed), None, "bar six is in three");
        assert!(
            parse_position("4.4", &mixed).is_some(),
            "bar four is in four"
        );
    }

    #[test]
    fn a_position_may_be_typed_with_the_trailing_fields_left_off() {
        // The fields after the bar are almost always zero, and typing `.1.000` every time is what
        // sends people back to the wheel.
        let bar = TimeSignature::default().ticks_per_bar().raw();
        let beat = TimeSignature::default().ticks_per_beat().raw();
        assert_eq!(parse_position("17", &four_four()), Some(Ticks(bar * 16)));
        assert_eq!(
            parse_position("17.3", &four_four()),
            Some(Ticks(bar * 16 + beat * 2))
        );
        assert_eq!(
            parse_position("17.3.050", &four_four()),
            Some(Ticks(bar * 16 + beat * 2 + 50 * POSITION_UNIT))
        );
        // Bar one, beat one is the start, not one bar in: the readout counts from one.
        assert_eq!(parse_position("1.1.000", &four_four()), Some(Ticks::ZERO));
        // Spaces and other separators a person might reach for.
        assert_eq!(
            parse_position(" 17 : 3 ", &four_four()),
            parse_position("17.3", &four_four())
        );
    }

    #[test]
    fn a_position_outside_its_bar_is_refused_rather_than_carried_over() {
        // Answering `1.9` by jumping to bar three would be answering a typo with a surprise. The
        // readout has never written one, so nothing is being refused that it could have shown.
        assert_eq!(
            parse_position("1.5", &four_four()),
            None,
            "beat five of four"
        );
        assert_eq!(
            parse_position("1.0", &four_four()),
            None,
            "beats count from one"
        );
        assert_eq!(parse_position("0", &four_four()), None, "so do bars");
        assert_eq!(
            parse_position("1.1.096", &four_four()),
            None,
            "hundredths run 0..95"
        );
        assert_eq!(parse_position("", &four_four()), None);
        assert_eq!(parse_position("later", &four_four()), None);
        assert_eq!(
            parse_position("1.1.0.0", &four_four()),
            None,
            "one field too many"
        );
        assert_eq!(parse_position("-3", &four_four()), None);

        // Three four has three beats to a bar, and the check follows the meter rather than a four.
        let three_four = SignatureMap::constant(TimeSignature::new(3, 4));
        assert!(parse_position("2.3", &three_four).is_some());
        assert_eq!(parse_position("2.4", &three_four), None);
    }

    #[test]
    fn every_grid_choice_is_a_distinct_division() {
        let mut seen: Vec<i64> = Vec::new();
        for (label, ticks) in GRID_CHOICES {
            assert!(
                label.starts_with("1/") || label == GRID_OFF_LABEL,
                "`{label}` is not a division",
            );
            assert!(!seen.contains(&ticks), "`{label}` repeats a division");
            seen.push(ticks);
        }
        // Coarsest first, so cycling through them halves the division each time and lands on
        // off at the end, which is the finest position there is.
        assert!(seen.windows(2).all(|pair| pair[0] > pair[1]));
        assert_eq!(
            GRID_CHOICES.last().map(|(label, _)| *label),
            Some(GRID_OFF_LABEL),
            "off comes last, so cycling reaches it without passing through it",
        );
    }

    #[test]
    fn a_take_that_recorded_nothing_is_not_reported_as_a_take() {
        // The status line is the only answer a record button gives. "recorded 00:00" over a
        // timeline with no new clip on it reads as a bug in the clip drawing rather than as an
        // input that was never connected.
        let empty = RecordingReport {
            clip: None,
            path: None,
            seconds: 0.0,
            dropped_frames: 0,
            outside_punch: false,
        };
        for language in Language::ALL {
            let line = recording_summary(&empty, language);
            assert_eq!(line, messages::recorded_nothing(language));
        }
    }

    #[test]
    fn a_take_that_missed_its_punch_is_not_reported_as_an_empty_one() {
        // Two different things that both produce no clip, and they send somebody to two different
        // places: one to their cable, the other to the punch region they set.
        let missed = RecordingReport {
            clip: None,
            path: Some(std::path::PathBuf::from("Vocals 3.wav")),
            seconds: 0.0,
            dropped_frames: 0,
            outside_punch: true,
        };
        for language in Language::ALL {
            assert_eq!(
                recording_summary(&missed, language),
                messages::recorded_outside_punch(language),
            );
            assert_ne!(
                recording_summary(&missed, language),
                messages::recorded_nothing(language),
            );
        }
    }

    #[test]
    fn a_take_with_a_hole_in_it_says_so() {
        // A dropped block moves everything after it earlier. Reporting the take as a plain
        // success would leave that to be found in a mix next week.
        let good = RecordingReport {
            clip: Some(ClipId(1)),
            path: None,
            seconds: 12.5,
            dropped_frames: 0,
            outside_punch: false,
        };
        let holed = RecordingReport {
            dropped_frames: 480,
            ..good.clone()
        };
        for language in Language::ALL {
            assert_ne!(
                recording_summary(&good, language),
                recording_summary(&holed, language),
            );
            assert!(
                recording_summary(&holed, language).contains("480"),
                "the number of lost frames is the whole point of the line"
            );
        }
    }

    #[test]
    fn the_grid_can_be_cycled_all_the_way_off_and_round_again() {
        // `Key::GridFree` has labelled this state since the button was written, for a value the
        // cycle could not produce: nothing a user placed could sit off the beat.
        let ticks: Vec<i64> = GRID_CHOICES.iter().map(|(_, ticks)| *ticks).collect();
        assert!(
            ticks.contains(&1),
            "one tick is as fine as the document gets"
        );
    }
}
