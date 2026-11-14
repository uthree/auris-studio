//! The MIDI page: which keyboards are listened to, how they strike, and how a take is written.

use super::*;
use auris_session::{MidiPreferences, NoteRecordMode, VelocityCurve};

/// The MIDI inputs as the main window last saw them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct MidiPorts {
    /// Every input the system had.
    available: Vec<String>,
    /// The ones being listened to.
    connected: Vec<String>,
}

impl MidiPorts {
    /// Reads them off a session.
    pub(super) fn of(session: &Session) -> Self {
        Self {
            available: session.available_midi_inputs().to_vec(),
            connected: session.midi_inputs(),
        }
    }
}

/// The rows the inputs list shows: every input there is, then every one switched off that is
/// not plugged in now, so a choice made about a keyboard can be taken back while it is away.
pub(super) fn input_rows(ports: &MidiPorts, ignored: &[String]) -> Vec<String> {
    let mut rows = ports.available.clone();
    for name in ignored {
        if !rows.contains(name) {
            rows.push(name.clone());
        }
    }
    rows
}

/// A picker's label for a quantise grid, as notes to a whole note.
pub(super) fn division_label(division: u32) -> Option<&'static str> {
    Some(match division {
        4 => "1/4",
        8 => "1/8",
        12 => "1/8T",
        16 => "1/16",
        24 => "1/16T",
        32 => "1/32",
        _ => return None,
    })
}

impl SettingsWindow {
    /// Hands edited MIDI preferences to the application, which applies and saves them.
    fn apply_midi(&mut self, midi: MidiPreferences, cx: &mut Context<Self>) {
        self.midi = midi.clone();
        let _ = self
            .app
            .update(cx, |app, _| app.apply_midi_preferences(midi));
        cx.notify();
    }

    pub(super) fn render_midi(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let midi = self.midi.clone();

        let mut inputs = FormGroup::new(self.t(Key::MidiInputsGroup));
        let any_inputs = self.matches_section(Section::MidiInputs);
        if any_inputs {
            inputs = inputs.child(
                FormField::new(self.t(Key::MidiEnabled))
                    .description(self.t(Key::MidiEnabledNote))
                    .beside()
                    .child(
                        Switch::new("midi-enabled", midi.enabled).on_change(cx.listener(
                            |this, on: &bool, _, cx| {
                                let midi = MidiPreferences {
                                    enabled: *on,
                                    ..this.midi.clone()
                                };
                                this.apply_midi(midi, cx);
                            },
                        )),
                    ),
            );
            let rows = input_rows(&self.midi_ports, &midi.ignored_inputs);
            if rows.is_empty() {
                inputs = inputs.child(note(self.t(Key::MidiNoInputs), &theme));
            }
            for name in rows {
                let ignored = midi.ignored_inputs.contains(&name);
                let state = if ignored {
                    Key::MidiInputIgnored
                } else if self.midi_ports.connected.contains(&name) {
                    Key::MidiInputConnected
                } else {
                    Key::MidiInputAbsent
                };
                let id = gpui::SharedString::from(format!("midi-input-{name}"));
                let row = name.clone();
                inputs = inputs.child(
                    FormField::new(name.clone())
                        .description(self.t(state))
                        .beside()
                        .child(Switch::new(id, !ignored).on_change(cx.listener(
                            move |this, on: &bool, _, cx| {
                                let mut midi = this.midi.clone();
                                midi.ignored_inputs.retain(|name| *name != row);
                                if !*on {
                                    midi.ignored_inputs.push(row.clone());
                                }
                                this.apply_midi(midi, cx);
                            },
                        ))),
                );
            }
            let curves = VelocityCurve::ALL
                .into_iter()
                .map(|curve| {
                    let label = match curve {
                        VelocityCurve::Linear => Key::VelocityLinear,
                        VelocityCurve::Soft => Key::VelocitySoft,
                        VelocityCurve::Hard => Key::VelocityHard,
                        VelocityCurve::Fixed => Key::VelocityFixed,
                    };
                    (curve, self.t(label).to_owned(), String::new())
                })
                .collect();
            let velocity = self.dropdown(
                "midi-velocity",
                curves,
                &midi.velocity,
                |this, velocity, cx| {
                    let midi = MidiPreferences {
                        velocity,
                        ..this.midi.clone()
                    };
                    this.apply_midi(midi, cx);
                },
                cx,
            );
            let octaves = (-auris_session::MIDI_OCTAVE_LIMIT..=auris_session::MIDI_OCTAVE_LIMIT)
                .map(|octave| (octave, crate::ui::root::octave_label(octave), String::new()))
                .collect();
            let octave = self.dropdown(
                "midi-octave",
                octaves,
                &midi.octave,
                |this, octave, cx| {
                    let midi = MidiPreferences {
                        octave,
                        ..this.midi.clone()
                    };
                    this.apply_midi(midi, cx);
                },
                cx,
            );
            inputs = inputs.child(
                FormField::new(self.t(Key::MidiOctave))
                    .description(self.t(Key::MidiOctaveNote))
                    .child(octave),
            );
            inputs = inputs.child(
                FormField::new(self.t(Key::MidiVelocity))
                    .description(self.t(Key::MidiVelocityNote))
                    .child(velocity),
            );
        }

        let mut recording = FormGroup::new(self.t(Key::MidiRecordingGroup));
        let any_recording = self.matches_section(Section::MidiRecording);
        if any_recording {
            let modes = [
                (NoteRecordMode::Takes, Key::RecordModeTakes),
                (NoteRecordMode::Merge, Key::RecordModeMerge),
                (NoteRecordMode::Replace, Key::RecordModeReplace),
            ]
            .into_iter()
            .map(|(mode, label)| (mode, self.t(label).to_owned(), String::new()))
            .collect();
            let mode = self.dropdown(
                "note-record-mode",
                modes,
                &midi.record_mode,
                |this, record_mode, cx| {
                    let midi = MidiPreferences {
                        record_mode,
                        ..this.midi.clone()
                    };
                    this.apply_midi(midi, cx);
                },
                cx,
            );
            recording = recording.child(
                FormField::new(self.t(Key::NoteRecordModeLabel))
                    .description(self.t(Key::NoteRecordModeNote))
                    .child(mode),
            );
            let divisions = MidiPreferences::DIVISIONS
                .into_iter()
                .map(|division| {
                    let label = division_label(division)
                        .map_or_else(|| self.t(Key::RecordQuantizeOff).to_owned(), str::to_owned);
                    (division, label, String::new())
                })
                .collect();
            let grid = self.dropdown(
                "record-quantize",
                divisions,
                &midi.quantize_division,
                |this, quantize_division, cx| {
                    let midi = MidiPreferences {
                        quantize_division,
                        ..this.midi.clone()
                    };
                    this.apply_midi(midi, cx);
                },
                cx,
            );
            recording = recording.child(
                FormField::new(self.t(Key::RecordQuantizeLabel))
                    .description(self.t(Key::RecordQuantizeNote))
                    .child(grid),
            );
            if midi.quantize_division != 0 {
                // In quarters, because a strength is felt in coarse steps and a slider's
                // thousand positions would be nine hundred and ninety-six nobody could tell apart.
                let percents = [100u32, 75, 50, 25];
                let current = percents
                    .into_iter()
                    .min_by_key(|percent| {
                        (midi.quantize_strength * 100.0 - *percent as f32).abs() as u32
                    })
                    .unwrap_or(100);
                let strengths = percents
                    .into_iter()
                    .map(|percent| (percent, format!("{percent}%"), String::new()))
                    .collect();
                let strength = self.dropdown(
                    "record-quantize-strength",
                    strengths,
                    &current,
                    |this, percent, cx| {
                        let midi = MidiPreferences {
                            quantize_strength: percent as f32 / 100.0,
                            ..this.midi.clone()
                        };
                        this.apply_midi(midi, cx);
                    },
                    cx,
                );
                recording = recording
                    .child(FormField::new(self.t(Key::RecordQuantizeStrength)).child(strength));
            }
        }

        div()
            .flex()
            .flex_col()
            .gap(theme.atelier.spacing.xl)
            .when(any_inputs, |page| page.child(inputs))
            .when(any_recording, |page| page.child(recording))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyboard_switched_off_stays_listed_while_it_is_unplugged() {
        let ports = MidiPorts {
            available: vec!["microKEY".into()],
            connected: vec!["microKEY".into()],
        };
        assert_eq!(input_rows(&ports, &[]), vec!["microKEY".to_string()]);
        assert_eq!(
            input_rows(&ports, &["Loopback".into(), "microKEY".into()]),
            vec!["microKEY".to_string(), "Loopback".to_string()]
        );
    }

    #[test]
    fn every_division_offered_but_off_has_a_label() {
        for division in MidiPreferences::DIVISIONS {
            assert_eq!(division_label(division).is_none(), division == 0);
        }
    }
}
