//! Menus for the lanes that run along the song rather than across it: the ruler, the structure
//! lane and the harmony lane.
//!
//! What they edit is the timeline's own state — the cycle, the tempo, the meter, the sections and
//! the chords — none of which belongs to any track, which is why the transport bar's list of
//! meters is here as well. The sheets they open are here too, because a prompt aimed at a
//! position is the same decision as the row that opened it, and the aiming itself is free
//! functions at the foot of the file so it can be checked without a window.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{Pixels, Point};

use crate::app::AurisApp;
use crate::ui::prompt::{Prompt, PromptTarget};

use super::{ContextMenu, MenuCommand};

impl AurisApp {
    /// The menu for the bar ruler: the cycle above, the tempo below.
    pub(crate) fn ruler_menu(&self, anchor: Point<Pixels>, tick: Ticks) -> ContextMenu {
        ContextMenu::new(anchor, self.t(Key::MenuCycleTitle))
            .item(
                self.t(Key::MenuCycleStartHere),
                MenuCommand::SetLoopStart(tick),
            )
            .item(self.t(Key::MenuCycleEndHere), MenuCommand::SetLoopEnd(tick))
            .separator()
            .toggle(
                self.t(Key::MenuCycleTitle),
                MenuCommand::ToggleLoop,
                self.project().loop_enabled,
            )
            .item_if(
                self.project().loop_region.is_some(),
                self.t(Key::MenuClearCycle),
                MenuCommand::ClearLoop,
            )
            .separator()
            // The punch section reads as the cycle's twin on purpose: the same two edges, the same
            // switch, and one extra entry for the case that is nearly always what somebody wants —
            // the bars they have just been looping are the bars they are about to replace.
            .item(
                self.t(Key::MenuPunchStartHere),
                MenuCommand::SetPunchStart(tick),
            )
            .item(
                self.t(Key::MenuPunchEndHere),
                MenuCommand::SetPunchEnd(tick),
            )
            .item_if(
                self.project().loop_region.is_some(),
                self.t(Key::MenuPunchFromCycle),
                MenuCommand::PunchFromCycle,
            )
            .toggle(
                self.t(Key::MenuPunchTitle),
                MenuCommand::TogglePunch,
                self.project().punch_enabled,
            )
            .item_if(
                self.project().punch_region.is_some(),
                self.t(Key::MenuClearPunch),
                MenuCommand::ClearPunch,
            )
            .separator()
            .item(self.t(Key::MenuSetTempoHere), MenuCommand::SetTempoAt(tick))
            // Offered only where a change governs: the anchor at tick zero is the song's own
            // tempo, not a change, and cannot be removed.
            .item_if(
                self.project().tempo_map.change_at(tick) != Ticks::ZERO,
                self.t(Key::MenuRemoveTempoHere),
                MenuCommand::RemoveTempoAt(tick),
            )
            .separator()
            .item(
                self.t(Key::MenuSetSignatureHere),
                MenuCommand::SetSignatureAt(tick),
            )
            .item_if(
                self.project().signatures.change_at(tick) != Ticks::ZERO,
                self.t(Key::MenuRemoveSignatureHere),
                MenuCommand::RemoveSignatureAt(tick),
            )
    }

    /// The list of meters the transport's signature field drops.
    ///
    /// The common ones with a tick beside whichever is in force, then a way to type one the list
    /// does not hold, then — where a change governs rather than the song's own meter — a way to
    /// take it away. Turning one of these *replaces* the meter of the stretch the playhead is in;
    /// writing a new change part way through a song is the ruler's job, which is where a person
    /// can see the bar they are aiming at.
    pub(crate) fn signature_menu(&self, anchor: Point<Pixels>, at: Ticks) -> ContextMenu {
        let current = self.session.signature_at(at);
        let mut menu = ContextMenu::new(anchor, self.t(Key::Signature));
        for signature in TimeSignature::COMMON {
            menu = menu.toggle(
                signature.to_string(),
                MenuCommand::SetSignature(at, signature),
                signature == current,
            );
        }
        menu.separator()
            .item(self.t(Key::MenuOtherSignature), MenuCommand::TypeSignature)
            .item_if(
                self.project().signatures.change_at(at) != Ticks::ZERO,
                self.t(Key::MenuRemoveSignatureHere),
                MenuCommand::RemoveSignatureAt(at),
            )
    }

    /// Opens the signature sheet aimed at the bar `at` rounds to.
    ///
    /// The ruler's counterpart to [`Self::prompt_for_signature`], and the same shape as
    /// [`Self::prompt_for_tempo_from`]: the field comes up holding the meter already in force
    /// there, so the sheet reads as "the meter from here is —".
    pub(crate) fn prompt_for_signature_from(&mut self, at: Ticks) {
        let title = self.t(Key::SetSignatureTitle);
        let current = self.session.signature_at(at).to_string();
        self.open_prompt(Prompt::new(title, PromptTarget::SignatureFrom(at), current));
    }

    /// Opens the tempo sheet aimed at the beat `at` rounds to.
    ///
    /// The field comes up holding the tempo already in force there, so the sheet reads as "the
    /// tempo from here is —" whether the answer confirms it or changes it.
    pub(crate) fn prompt_for_tempo_from(&mut self, at: Ticks) {
        let title = self.t(Key::SetTempoTitle);
        let current = format!("{:.2}", self.project().tempo_map.bpm_at(at));
        self.open_prompt(Prompt::new(title, PromptTarget::TempoFrom(at), current));
    }

    /// Opens the naming sheet for the section in force at `tick`.
    ///
    /// The field comes up holding the current name, so double-clicking a section renames it
    /// and double-clicking an empty stretch gives it its first one.
    pub(crate) fn prompt_for_section(&mut self, tick: Ticks) {
        let current = self
            .project()
            .sections
            .label_at(tick)
            .map(str::to_string)
            .unwrap_or_default();
        let title = self.t(Key::SetSectionTitle);
        self.open_prompt(Prompt::new(title, PromptTarget::Section(tick), current));
    }

    /// The menu for the structure lane.
    ///
    /// Headed by the section under the pointer when there is one — numbered the way the lane
    /// draws it — because that is what the items act on, wherever inside it the press landed.
    pub(crate) fn structure_menu(&self, anchor: Point<Pixels>, tick: Ticks) -> ContextMenu {
        let sections = &self.project().sections;
        let named = sections.label_at(tick).is_some();
        let title = match sections.section_at(tick) {
            Some((label, instance)) => {
                if sections.repeats(label) > 1 {
                    format!("{label} {instance}")
                } else {
                    label.to_string()
                }
            }
            None => self.t(Key::SetSectionTitle).to_string(),
        };

        ContextMenu::new(anchor, title)
            .item(
                self.t(Key::MenuSetSectionHere),
                MenuCommand::SetSectionAt(tick),
            )
            .item_if(
                named,
                self.t(Key::MenuRemoveSectionHere),
                MenuCommand::RemoveSectionAt(tick),
            )
            .item_if(
                named,
                self.t(Key::MenuEndSectionsHere),
                MenuCommand::EndSectionsAt(tick),
            )
    }

    /// The menu for the harmony lane, aimed at whatever the pointer is over.
    ///
    /// `tick` is where the pointer is, unrounded. What the chord items act on is the chord *in
    /// force* there rather than that position: a chord occupies everything up to the next change,
    /// so retyping or removing "the chord here" has to mean the one you can see, not one that
    /// happens to begin under the pixel. Only writing a chord where there is none falls back to
    /// the rounded position, which is what [`Session::snap_harmony`] decides.
    ///
    /// The range a progression is written across is the cycle region when there is one, and the
    /// chart's own length otherwise. That is the rule everywhere else in the application — set the
    /// cycle over the chorus, then act on it — and it saves inventing a "how many bars" field
    /// nothing else would use.
    pub(crate) fn harmony_menu(&self, anchor: Point<Pixels>, tick: Ticks) -> ContextMenu {
        let signatures = &self.project().signatures;
        let placed = self.session.snap_harmony(tick);
        let (from, bars) = progression_target(self.project().loop_region, placed, signatures);
        let harmony = &self.project().harmony;
        let target = harmony_target(harmony, tick, placed);

        // Naming the chord rather than the bar when there is one: this menu can retype or remove
        // it, and a heading reading "Harmony · bar 5" over an item that acts on the chord that
        // started in bar 3 would be pointing somewhere else entirely.
        let title = match harmony.chord_at(tick) {
            Some(chord) => messages::harmony_chord(
                self.language(),
                &harmony
                    .numeral_at(tick)
                    .map(|numeral| numeral.to_string())
                    .unwrap_or_default(),
                &chord.to_string(),
            ),
            None => messages::harmony_at_bar(self.language(), signatures.bar_of(placed)),
        };

        ContextMenu::new(anchor, title)
            .item(
                self.t(Key::MenuSetChordHere),
                MenuCommand::SetChordAt(target.chord),
            )
            .item_if(
                target.sounding,
                self.t(Key::MenuRemoveChordHere),
                MenuCommand::RemoveChordAt(tick),
            )
            .item(self.t(Key::MenuSetKeyHere), MenuCommand::SetKeyAt(placed))
            .item_if(
                target.removable_key,
                self.t(Key::MenuRemoveKeyHere),
                MenuCommand::RemoveKeyAt(tick),
            )
            .separator()
            .item(
                self.t(Key::MenuWriteProgression),
                MenuCommand::ShowProgressionPicker { at: from, anchor },
            )
            .item_if(
                !harmony.is_empty(),
                self.t(Key::MenuClearHarmony),
                MenuCommand::ClearHarmony {
                    from,
                    // Counted off the ruler, so clearing "four bars from here" clears the four
                    // the ruler shows even where one of them is in a different meter.
                    to: signatures.bar_start(signatures.bar_of(from) + bars.max(1) as u32),
                },
            )
    }

    /// Every progression the composer knows by name, aimed at one position.
    pub(crate) fn progression_picker_menu(&self, anchor: Point<Pixels>, at: Ticks) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuWriteProgression));
        for entry in progression_catalog() {
            menu = menu.item(
                // What it is called, not what the parser calls it and not what it is *for*. The
                // slug — `axis-minor`, `doo-wop` — is the vocabulary of a specification file, and
                // the description is a whole sentence; a menu of sixteen sentences is one nobody
                // can scan.
                auris_i18n::audio::theory_name(entry.name, self.language()),
                MenuCommand::StampProgression {
                    name: entry.name,
                    at,
                },
            );
        }
        menu
    }
}

/// Where a progression goes, and across how many bars.
///
/// The cycle region when there is one, because "set the cycle over the chorus, then act on it" is
/// how the rest of the application already works, and it saves inventing a how-many-bars field
/// that nothing else would use. Otherwise it starts where the pointer was, and zero bars means
/// the chart's own length — which is what [`Session::stamp_named_progression`] reads it as.
pub(super) fn progression_target(
    loop_region: Option<(Ticks, Ticks)>,
    tick: Ticks,
    signatures: &SignatureMap,
) -> (Ticks, usize) {
    match loop_region {
        Some((start, end)) if end > start => {
            // Counted off the ruler rather than divided out of a length, so a cycle spanning a
            // meter change reports the bars a person would count across it.
            let bars = signatures.bar_of(end) - signatures.bar_of(start.max_zero());
            // A cycle shorter than a bar still means one bar: the user asked for *there*, and
            // writing nothing would look like the command had failed.
            (start.max_zero(), bars.max(1) as usize)
        }
        _ => (tick, 0),
    }
}

/// What the harmony lane's menu acts on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct HarmonyTarget {
    /// Where a chord typed from this menu goes.
    chord: Ticks,
    /// Whether a chord sounds here, which is what makes removing one worth offering.
    sounding: bool,
    /// Whether the key here came from a change that can be removed — the anchor at tick zero
    /// cannot, since a song is always in some key.
    removable_key: bool,
}

/// What a right-click at `tick` aims the harmony menu at, `placed` being that position rounded.
///
/// Editing a chord means editing the one you can see, and a chord occupies everything from where
/// it starts to the next change — so a menu that acted on the rounded pointer position would write
/// a *second* chord a beat later instead of retyping the one that is there. Only where nothing
/// sounds does the rounded position win, because then there is nothing to edit and the menu is
/// placing something new.
///
/// Free rather than a method so the aiming can be tested: everything else it would take is a whole
/// session and a window.
fn harmony_target(harmony: &Harmony, tick: Ticks, placed: Ticks) -> HarmonyTarget {
    let sounding = harmony.chord_at(tick).is_some();
    HarmonyTarget {
        chord: harmony
            .chords
            .change_at(tick)
            .filter(|_| sounding)
            .unwrap_or(placed),
        sounding,
        removable_key: harmony.keys.change_at(tick) != Ticks::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::context_menu::meters;

    #[test]
    fn a_progression_goes_where_the_cycle_is_when_there_is_one() {
        let bar = TimeSignature::new(4, 4).ticks_per_bar();

        // No cycle: it starts where the pointer was, for the chart's own length.
        assert_eq!(
            progression_target(None, bar * 3, &meters()),
            (bar * 3, 0),
            "zero bars means the chart decides"
        );

        // A cycle over bars 5..9 wins over wherever the pointer happened to be.
        assert_eq!(
            progression_target(Some((bar * 4, bar * 8)), bar * 99, &meters()),
            (bar * 4, 4)
        );

        // A cycle shorter than a bar still writes something.
        assert_eq!(
            progression_target(Some((Ticks::ZERO, Ticks(480))), bar, &meters()),
            (Ticks::ZERO, 1)
        );

        // An empty or inverted cycle is not a range, so the pointer decides again.
        assert_eq!(
            progression_target(Some((bar * 4, bar * 4)), bar, &meters()),
            (bar, 0)
        );
    }

    /// One bar of 4/4.
    const BAR: Ticks = Ticks(3840);

    fn numeral(text: &str) -> Option<Numeral> {
        Some(Numeral::parse(text).expect("a numeral the test wrote itself"))
    }

    #[test]
    fn the_harmony_menu_edits_the_chord_you_can_see() {
        // A chord runs until the next change, so pointing part-way into one must retype *it*
        // rather than write a second chord where the pointer rounded to. The positions matter: a
        // progression written three chords to a bar sits on thirds of one, which no editing grid
        // rounds to, so an aim that went through the grid would miss every chord in it.
        let mut harmony = Harmony::default();
        harmony.chords.set_point(BAR, numeral("I"));
        harmony.chords.set_point(BAR + Ticks(1280), numeral("IV"));

        let target = harmony_target(&harmony, BAR + Ticks(700), BAR + Ticks(960));
        assert_eq!(target.chord, BAR, "aimed at the chord, not at the grid");
        assert!(target.sounding);

        let later = harmony_target(&harmony, BAR * 4, BAR * 4);
        assert_eq!(later.chord, BAR + Ticks(1280), "the last one runs on");

        // Before anything is written there is nothing to edit, so a new chord goes where the
        // pointer rounded to.
        let empty = harmony_target(&harmony, Ticks::ZERO, Ticks::ZERO);
        assert_eq!(empty.chord, Ticks::ZERO);
        assert!(
            !empty.sounding,
            "removing a chord that is not there was offered"
        );
    }

    #[test]
    fn a_cleared_stretch_offers_a_chord_rather_than_removing_a_silence() {
        // Clearing writes a `None` marker, which is a change like any other. It is not a chord,
        // though, so the menu must offer to write one there rather than to remove one.
        let mut harmony = Harmony::default();
        harmony.chords.set_point(Ticks::ZERO, numeral("I"));
        harmony.clear(BAR, BAR * 2);

        let target = harmony_target(&harmony, BAR + Ticks(100), BAR);
        assert!(!target.sounding);
        assert_eq!(target.chord, BAR, "the rounded position: nothing to edit");
    }

    #[test]
    fn only_a_key_change_can_be_removed_and_never_the_song_s_own_key() {
        let mut harmony = Harmony::default();
        assert!(
            !harmony_target(&harmony, BAR * 9, BAR * 9).removable_key,
            "a song in one key throughout has no key change to remove"
        );

        harmony
            .keys
            .set_point(BAR * 4, MusicalKey::parse("Eb major").unwrap());
        assert!(!harmony_target(&harmony, BAR * 2, BAR * 2).removable_key);
        assert!(
            harmony_target(&harmony, BAR * 9, BAR * 9).removable_key,
            "pointing anywhere inside the E flat finds the change that started it"
        );
    }
}
