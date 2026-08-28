//! The performance a written part starts with.
//!
//! The composer used to bake its feel into the notes: a random wander over timing and velocity,
//! and a constant per-role lean, both applied as the parts were written. Both are *performance*
//! rather than score — they are how the text is played, not what it says — so they now arrive as
//! [`NoteTransform`]s installed on the finished clip, where the piano roll shows the text on the
//! grid, a person turns the feel without a rewrite, and a looped clip is loose differently on
//! every pass. This module is what is left for the composer to decide: *which* transforms a part
//! begins with, which is a table of who pushes, who drags, and who may not wander at all.
//!
//! Two rules carried over from when the feel was baked, because they are about the band and not
//! about the mechanism:
//!
//! * **A drum does not wander.** Everything else is loose *against* somebody keeping the time,
//!   and if the kit moves too there is nothing to be loose against. A drum part therefore gets a
//!   lean and never a humanise.
//! * **A lean is not a wobble.** The hat a little early, the snare a little late, by the same
//!   amount in every bar — a thing a drummer does on purpose that reads as a feel. It is
//!   deterministic, so it is its own transform and survives at `looseness` settings where the
//!   wander would be inaudible.
//!
//! The swing is *not* here: which pairs it delays is the groove's own answer, so it stays
//! written into the text by the one pass `crate::parts` still runs over the timing.

use auris_core::{ClipPreset, NoteTransform};

use crate::phrase::roles_of;
use crate::spec::Role;

/// How loose a part written without a whole song around it is played.
///
/// The value [`ClipRecipe`](auris_core::ClipRecipe) used to carry as its `humanize` dial's
/// default, kept so that *Write a Part Here* sounds as it always did. A whole song reads its
/// specification's own `humanize` instead.
const DEFAULT_LOOSENESS: f32 = 0.25;

/// A stab starts tighter than the default, as its recipe always had it.
///
/// Its whole identity is a rhythm hammered by a chord, and a rhythm that wanders is a rhythm.
const STAB_LOOSENESS: f32 = 0.1;

/// How far a role sits against the beat at full looseness, in ticks.
///
/// The lean the humanisation pass used to add: a hat pushes, a bass drags, the snare lays back.
/// Ticks rather than milliseconds because a lean is part of how the part sits in the bar — the
/// same fraction of a beat at any tempo.
fn lean_ticks(role: Role) -> f32 {
    match role {
        Role::Hat => -8.0,
        Role::Melody | Role::Arp => -4.0,
        Role::Bass => 6.0,
        Role::Snare => 10.0,
        _ => 0.0,
    }
}

/// The transforms one part of a composed song starts with.
///
/// `looseness` is the specification's `humanize` dial and `seed` is the clip's own — the same
/// number its recipe carries, so a take and its feel are named by one number. The stack comes
/// back in the canonical order the panel keeps: lean before wander, both after the swing the
/// text already carries.
///
/// A part at `looseness` 0 starts with nothing at all, which is the machine the dial has always
/// promised; a drum gets its lean and no wander, for the module's first rule.
pub fn part_performance(role: Role, looseness: f32, seed: u64) -> Vec<NoteTransform> {
    let looseness = looseness.clamp(0.0, 1.0);
    let mut stack = Vec::new();
    if looseness <= 0.0 {
        return stack;
    }
    let lean = (lean_ticks(role) * looseness).round() as i64;
    if lean != 0 {
        stack.push(NoteTransform::Lean { ticks: lean });
    }
    if !role.is_drum() {
        stack.push(NoteTransform::Humanize {
            amount: looseness,
            seed,
        });
    }
    stack
}

/// The transforms a clip generated from a recipe starts with.
///
/// The preset's own looseness, because a recipe no longer carries a `humanize` dial: the feel is
/// the clip's performance stack from the moment it is written, and the panel that edits the
/// stack is the panel that edits it thereafter. Writing the clip again touches the text alone.
///
/// [`ClipPreset::Drums`] is the one preset holding several roles in one clip, and their leans
/// disagree — the hat pushes while the snare lays back — so a merged kit starts with nothing.
pub fn clip_performance(preset: ClipPreset, seed: u64) -> Vec<NoteTransform> {
    let looseness = match preset {
        ClipPreset::Stab => STAB_LOOSENESS,
        _ => DEFAULT_LOOSENESS,
    };
    match roles_of(preset) {
        [role] => part_performance(*role, looseness, seed),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pitched_part_leans_and_wanders_in_the_panels_order() {
        let stack = part_performance(Role::Bass, 1.0, 42);
        assert_eq!(
            stack,
            vec![
                NoteTransform::Lean { ticks: 6 },
                NoteTransform::Humanize {
                    amount: 1.0,
                    seed: 42
                },
            ]
        );
    }

    #[test]
    fn a_drum_leans_but_never_wanders() {
        let hat = part_performance(Role::Hat, 1.0, 7);
        assert_eq!(hat, vec![NoteTransform::Lean { ticks: -8 }]);
        assert!(
            part_performance(Role::Kick, 1.0, 7).is_empty(),
            "the kick neither leans nor wanders"
        );
    }

    #[test]
    fn the_lean_scales_with_the_looseness_and_zero_is_the_machine() {
        let half = part_performance(Role::Snare, 0.5, 7);
        assert_eq!(half, vec![NoteTransform::Lean { ticks: 5 }]);
        assert!(part_performance(Role::Melody, 0.0, 7).is_empty());
        // A lean that rounds to nothing is not stored as a transform that does nothing.
        let faint = part_performance(Role::Melody, 0.1, 7);
        assert_eq!(
            faint,
            vec![NoteTransform::Humanize {
                amount: 0.1,
                seed: 7
            }]
        );
    }

    #[test]
    fn a_generated_clip_starts_at_the_looseness_its_recipe_used_to_carry() {
        let lead = clip_performance(ClipPreset::Lead, 3);
        assert!(matches!(
            lead.as_slice(),
            [
                NoteTransform::Lean { ticks: -1 },
                NoteTransform::Humanize { amount, seed: 3 }
            ] if (*amount - DEFAULT_LOOSENESS).abs() < 1e-6
        ));
        let stab = clip_performance(ClipPreset::Stab, 3);
        assert!(matches!(
            stab.as_slice(),
            [NoteTransform::Humanize { amount, .. }] if (*amount - STAB_LOOSENESS).abs() < 1e-6
        ));
        // The merged kit's roles disagree about which way to lean, so it starts square.
        assert!(clip_performance(ClipPreset::Drums, 3).is_empty());
        // A single drum keeps its own lean.
        assert_eq!(
            clip_performance(ClipPreset::Hat, 3),
            vec![NoteTransform::Lean { ticks: -2 }]
        );
    }
}
