//! Piano-roll grid policy shared by note creation, movement, resizing and painting.

use auris_i18n::{Key, Language};
use auris_session::PianoRollGrid;
use auris_session::prelude::*;

use gpui::px;

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::timeline::TimelineView;

const NOTE_GRIDS: [(PianoRollGrid, &str); 14] = [
    (PianoRollGrid::Whole, "1/1"),
    (PianoRollGrid::Half, "1/2"),
    (PianoRollGrid::Quarter, "1/4"),
    (PianoRollGrid::Eighth, "1/8"),
    (PianoRollGrid::Sixteenth, "1/16"),
    (PianoRollGrid::ThirtySecond, "1/32"),
    (PianoRollGrid::SixtyFourth, "1/64"),
    (PianoRollGrid::WholeTriplet, "1/1 T"),
    (PianoRollGrid::HalfTriplet, "1/2 T"),
    (PianoRollGrid::QuarterTriplet, "1/4 T"),
    (PianoRollGrid::EighthTriplet, "1/8 T"),
    (PianoRollGrid::SixteenthTriplet, "1/16 T"),
    (PianoRollGrid::ThirtySecondTriplet, "1/32 T"),
    (PianoRollGrid::SixtyFourthTriplet, "1/64 T"),
];

fn grid_label(choice: PianoRollGrid, language: Language) -> &'static str {
    let key = match choice {
        PianoRollGrid::Smart => Key::SnapSmart,
        PianoRollGrid::Bar => Key::SnapBar,
        PianoRollGrid::Beat => Key::SnapBeat,
        PianoRollGrid::Division => Key::SnapDivision,
        PianoRollGrid::Off => Key::GridFree,
        _ => {
            return NOTE_GRIDS
                .iter()
                .find(|(grid, _)| *grid == choice)
                .map_or("", |(_, label)| label);
        }
    };
    key.get(language)
}

/// A song-relative snap grid. Its origin follows the current time-signature segment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoteSnap {
    /// Distance between adjacent grid positions; one tick means free editing.
    pub(crate) grid: Ticks,
    /// Song position from which grid positions are counted.
    pub(crate) origin: Ticks,
    /// Whether edits preserve the original edge's offset from the grid.
    pub(crate) relative: bool,
}

impl NoteSnap {
    /// An absolute song position on the grid, used when creating a note.
    pub(crate) fn position(self, tick: Ticks) -> Ticks {
        self.origin + (tick - self.origin).snap_nearest(self.grid)
    }

    /// Movement of the grabbed edge; the same delta is applied to the whole selection.
    pub(crate) fn delta(self, edge: Ticks, travel: Ticks) -> Ticks {
        if travel == Ticks::ZERO {
            return Ticks::ZERO;
        }
        if self.relative {
            travel.snap_nearest(self.grid)
        } else {
            self.position(edge + travel) - edge
        }
    }
}

/// Grid spacing for both editing and drawing, including signature-aware bars and beats.
pub(crate) fn grid_step(
    view: &TimelineView,
    signature: TimeSignature,
    choice: PianoRollGrid,
    division: Ticks,
) -> Ticks {
    use PianoRollGrid::*;
    let whole = TICKS_PER_QUARTER * 4;
    match choice {
        Smart => [256, 128, 64, 32, 16, 8, 4, 2, 1]
            .into_iter()
            .map(|denominator| Ticks(whole / denominator))
            .find(|step| step.raw() as f32 * view.pixels_per_tick() >= 7.0)
            .unwrap_or_else(|| view.grid_step(signature, px(7.0))),
        Bar => signature.ticks_per_bar(),
        Beat => signature.ticks_per_beat(),
        Division => division.max(Ticks(1)),
        Whole => Ticks(whole),
        Half => Ticks(whole / 2),
        Quarter => Ticks(whole / 4),
        Eighth => Ticks(whole / 8),
        Sixteenth => Ticks(whole / 16),
        ThirtySecond => Ticks(whole / 32),
        SixtyFourth => Ticks(whole / 64),
        WholeTriplet => Ticks(whole * 2 / 3),
        HalfTriplet => Ticks(whole / 3),
        QuarterTriplet => Ticks(whole / 6),
        EighthTriplet => Ticks(whole / 12),
        SixteenthTriplet => Ticks(whole / 24),
        ThirtySecondTriplet => Ticks(whole / 48),
        SixtyFourthTriplet => Ticks(whole / 96),
        Off => Ticks(1),
    }
}

impl AurisApp {
    /// Current grid, including Smart's effective interval as the editor is zoomed.
    pub(crate) fn note_snap_label(&self) -> String {
        let mut grid = grid_label(self.settings.piano_roll_grid, self.language()).to_owned();
        if self.settings.piano_roll_grid == PianoRollGrid::Smart {
            let at = self
                .selected_midi_clip()
                .map_or(Ticks::ZERO, |clip| clip.start);
            let step = self
                .note_snap(at, gpui::Modifiers::none(), false)
                .grid
                .raw();
            grid = format!("{grid} (1/{})", TICKS_PER_QUARTER * 4 / step.max(1));
        }
        grid
    }

    /// Full snap state for the compact header control's tooltip.
    pub(crate) fn note_snap_tip(&self) -> String {
        let mode = self.t(if self.settings.relative_note_snap {
            Key::SnapRelative
        } else {
            Key::SnapAbsolute
        });
        format!(
            "{}: {} · {mode}",
            self.t(Key::SnapShort),
            self.note_snap_label()
        )
    }

    /// The existing command menu supplies keyboard navigation and Escape dismissal.
    pub(crate) fn note_snap_menu(&self, at: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(at, self.t(Key::PianoRollSnap));
        for grid in [
            PianoRollGrid::Smart,
            PianoRollGrid::Bar,
            PianoRollGrid::Beat,
            PianoRollGrid::Division,
            PianoRollGrid::Off,
        ]
        .into_iter()
        .chain(NOTE_GRIDS.iter().map(|(grid, _)| *grid))
        {
            menu = menu.toggle(
                grid_label(grid, self.language()),
                MenuCommand::PianoRollGrid(grid),
                self.settings.piano_roll_grid == grid,
            );
        }
        menu.separator()
            .toggle(
                self.t(Key::SnapAbsolute),
                MenuCommand::RelativeNoteSnap(false),
                !self.settings.relative_note_snap,
            )
            .toggle(
                self.t(Key::SnapRelative),
                MenuCommand::RelativeNoteSnap(true),
                self.settings.relative_note_snap,
            )
            .separator()
            .toggle(
                self.t(Key::SnapNoteLengthsShort),
                MenuCommand::SnapNoteEnds(!self.settings.snap_note_lengths),
                self.settings.snap_note_lengths,
            )
    }

    /// Persists editor preferences without recording a document edit.
    pub(crate) fn remember_note_snap(&self) {
        if let Err(error) = self.save_settings() {
            log::warn!("could not save note snap settings: {error}");
        }
    }

    /// Opens the snap menu from the command palette or a user-assigned key binding.
    pub(crate) fn on_note_snap(
        &mut self,
        _: &crate::actions::PianoRollSnap,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let at = self.roll_origin();
        self.open_menu(self.note_snap_menu(at));
        window.focus(&self.focus);
        cx.notify();
    }

    /// Snap policy at a song position. Control refines the grid; Control-Shift edits in ticks.
    pub(crate) fn note_snap(
        &self,
        tick: Ticks,
        modifiers: gpui::Modifiers,
        drawing: bool,
    ) -> NoteSnap {
        let signature = self.project().signatures.signature_at(tick);
        let mut grid = grid_step(
            &self.roll_timeline,
            signature,
            self.settings.piano_roll_grid,
            self.project().grid,
        );
        if (modifiers.control && modifiers.shift) || (!drawing && modifiers.secondary()) {
            grid = Ticks(1);
        } else if modifiers.control && (!drawing || cfg!(target_os = "macos")) {
            grid = self.project().grid.min(grid).max(Ticks(1));
        }
        NoteSnap {
            grid,
            origin: self.project().signatures.change_at(tick),
            relative: self.settings.relative_note_snap,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute() -> NoteSnap {
        NoteSnap {
            grid: Ticks(240),
            origin: Ticks::ZERO,
            relative: false,
        }
    }

    #[test]
    fn moving_an_off_grid_note_aligns_its_start_in_absolute_mode() {
        assert_eq!(
            Ticks(70) + absolute().delta(Ticks(70), Ticks(240)),
            Ticks(240)
        );
    }

    #[test]
    fn moving_in_relative_mode_preserves_the_off_grid_start() {
        let snap = NoteSnap {
            relative: true,
            ..absolute()
        };
        assert_eq!(Ticks(70) + snap.delta(Ticks(70), Ticks(240)), Ticks(310));
    }

    #[test]
    fn resizing_aligns_the_end_instead_of_quantizing_the_duration() {
        assert_eq!(
            Ticks(400) + absolute().delta(Ticks(400), Ticks(80)),
            Ticks(480)
        );
    }

    #[test]
    fn relative_resize_preserves_the_original_end_offset() {
        let snap = NoteSnap {
            relative: true,
            ..absolute()
        };
        assert_eq!(Ticks(400) + snap.delta(Ticks(400), Ticks(240)), Ticks(640));
    }

    #[test]
    fn free_movement_does_not_round_the_initial_pointer_position() {
        let snap = NoteSnap {
            grid: Ticks(1),
            ..absolute()
        };
        assert_eq!(snap.delta(Ticks(70), Ticks(100)), Ticks(100));
    }

    #[test]
    fn transposing_without_horizontal_travel_preserves_timing() {
        assert_eq!(absolute().delta(Ticks(70), Ticks::ZERO), Ticks::ZERO);
    }

    #[test]
    fn grids_follow_the_signature_change_instead_of_the_clip_start() {
        let snap = NoteSnap {
            origin: Ticks(100),
            ..absolute()
        };
        assert_eq!(snap.position(Ticks(700)), Ticks(820));
    }

    #[test]
    fn smart_changes_with_zoom_while_fixed_and_triplet_grids_do_not() {
        let mut view = TimelineView::default();
        let signature = TimeSignature::new(4, 4);
        let at = |view: &TimelineView, choice| grid_step(view, signature, choice, Ticks(240));
        let wide = at(&view, PianoRollGrid::Smart);
        view.pixels_per_beat *= 4.0;
        assert!(at(&view, PianoRollGrid::Smart) < wide);
        assert_eq!(at(&view, PianoRollGrid::Sixteenth), Ticks(240));
        assert_eq!(at(&view, PianoRollGrid::SixteenthTriplet), Ticks(160));
        assert_eq!(at(&view, PianoRollGrid::SixtyFourthTriplet), Ticks(40));
        assert_eq!(at(&view, PianoRollGrid::Off), Ticks(1));
        view.pixels_per_beat = TimelineView::MAX_PIXELS_PER_BEAT;
        assert!(at(&view, PianoRollGrid::Smart) < Ticks(60));
    }
}
