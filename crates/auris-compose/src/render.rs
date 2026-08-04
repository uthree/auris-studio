//! Turning the parts into clips a document can hold.

use auris_core::Note;
use auris_core::time::{Ticks, TimeSignature};

use crate::frame::{Frame, plan};
use crate::parts::{ScoreSettings, write_parts};
use crate::spec::SongSpec;

/// One clip: a run of notes with a place on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipDraft {
    /// What the clip is called.
    pub name: String,
    /// Where it starts in the song.
    pub start: Ticks,
    /// How long it lasts.
    pub length: Ticks,
    /// Its notes, positioned from the clip's own start.
    pub notes: Vec<Note>,
}

/// One track: an instrument and the clips it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackDraft {
    /// The track's name.
    pub name: String,
    /// The plugin that plays it.
    pub instrument: String,
    /// Level trim in decibels.
    pub gain_db: f32,
    /// Stereo position.
    pub pan: f32,
    /// The clips, in time order.
    pub clips: Vec<ClipDraft>,
}

/// A finished piece, ready to become a project.
#[derive(Clone, Debug, PartialEq)]
pub struct Composition {
    /// What the piece is called.
    pub title: String,
    /// Beats per minute.
    pub tempo: f64,
    /// The time signature.
    pub meter: TimeSignature,
    /// How long the piece is.
    pub length: Ticks,
    /// The seed it was written from, so it can be written again.
    pub seed: u64,
    /// The tracks, in the order the parts were declared.
    pub tracks: Vec<TrackDraft>,
}

impl Composition {
    /// How many notes the piece contains.
    pub fn note_count(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .map(|clip| clip.notes.len())
            .sum()
    }

    /// A one-line-per-track summary, for a command line to print.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} · {:.0} BPM · {}/{} · {} bars · seed {}\n",
            self.title,
            self.tempo,
            self.meter.numerator,
            self.meter.denominator,
            self.length.raw() / self.meter.ticks_per_bar().raw().max(1),
            self.seed
        );
        for track in &self.tracks {
            out.push_str(&format!(
                "  {:<12} {:<24} {} clips, {} notes\n",
                track.name,
                track.instrument,
                track.clips.len(),
                track
                    .clips
                    .iter()
                    .map(|clip| clip.notes.len())
                    .sum::<usize>()
            ));
        }
        out
    }
}

/// Writes a piece from its specification.
///
/// The whole crate is this one function: a spec in, notes out, with nothing in between that
/// depends on the time of day. The same spec and the same seed always give the same piece.
pub fn compose(spec: &SongSpec) -> Composition {
    let frame = plan(spec);
    render(spec, &frame)
}

/// Turns a planned frame and its parts into tracks of clips.
fn render(spec: &SongSpec, frame: &Frame) -> Composition {
    let drafts = write_parts(&ScoreSettings::from(spec), &spec.parts, frame);
    let mut tracks = Vec::new();

    for draft in drafts {
        let mut clips = Vec::new();
        for (index, section) in frame.sections.iter().enumerate() {
            let mut notes: Vec<Note> = draft
                .notes
                .iter()
                .filter(|note| note.section == index)
                .filter_map(|note| {
                    // Rebase onto the clip. A note humanisation nudged a few ticks over a
                    // section boundary is clamped back rather than deleted — dropping it took
                    // the downbeat out of every section at the default humanisation.
                    let start = (note.start - section.start)
                        .max_zero()
                        .min(section.length - Ticks(1));
                    if note.start - section.start >= section.length {
                        return None;
                    }
                    // Truncate rather than let a note overhang: the scheduler would drop it
                    // silently, and `fit_length_to_notes` would grow the clip if it did not.
                    let length = note.length.min(section.length - start).max(Ticks(1));
                    Some(Note {
                        pitch: note.pitch.min(127),
                        velocity: note.velocity.clamp(0.0, 1.0),
                        start,
                        length,
                    })
                })
                .collect();

            // A canonical order, so two runs of the same spec compare equal byte for byte.
            notes.sort_by_key(|note| (note.start.raw(), note.pitch));

            // An empty clip is a hole in the arrangement rather than a block of silence.
            if notes.is_empty() {
                continue;
            }
            clips.push(ClipDraft {
                name: format!("{} {} · {}", section.name, section.instance, draft.name),
                start: section.start,
                length: section.length,
                notes,
            });
        }

        // A part that never plays leaves no track at all.
        if clips.is_empty() {
            continue;
        }
        tracks.push(TrackDraft {
            name: draft.name,
            instrument: draft.instrument,
            gain_db: draft.gain_db,
            pan: draft.pan,
            clips,
        });
    }

    Composition {
        title: spec.title.clone(),
        tempo: spec.tempo,
        meter: spec.meter,
        length: frame.length,
        seed: spec.seed,
        tracks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose_text(text: &str) -> Composition {
        compose(&SongSpec::parse(text).expect("the fixture parses"))
    }

    const BASE: &str = "
        title: Test
        form: intro verse chorus
        chords: @axis
        humanize: 0
        [section intro]
        bars: 4
        [section verse]
        bars: 8
        [section chorus]
        bars: 8
    ";

    /// Everything a piece is, as one line per section and one number for the notes.
    ///
    /// Written so that a change shows up as a diff a person can read: the chords are the part a
    /// musician would notice, and the digest catches a note that moved by one tick.
    fn fingerprint(text: &str) -> String {
        let spec = SongSpec::parse(text).expect("the fixture parses");
        let frame = plan(&spec);
        let piece = render(&spec, &frame);

        let mut out = String::new();
        for section in &frame.sections {
            out.push_str(&format!("{}·{} ", section.name, section.instance));
            out.push_str(&section.key.to_text());
            out.push_str(" |");
            for event in &section.events {
                // Both, and not only one: `name()` reads the numeral, which is what the timeline
                // will store, while `chord` is what the colouring pass actually produced and what
                // is heard. The whole difficulty of this split is that today they can disagree,
                // so a fingerprint that showed one of them would hide it.
                let numeral = event.name();
                let sounding = event.chord.name_in(event.key);
                if numeral == sounding {
                    out.push_str(&format!(" {numeral}"));
                } else {
                    out.push_str(&format!(" {numeral}→{sounding}"));
                }
            }
            out.push_str(" |\n");
        }
        // A cheap order-sensitive digest: a note that moves, changes pitch or changes length
        // changes it, and two pieces that differ anywhere differ here.
        let mut digest: u64 = 1469598103934665603;
        for track in &piece.tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    for value in [
                        note.pitch as i64,
                        note.start.raw(),
                        note.length.raw(),
                        (note.velocity * 1000.0) as i64,
                        clip.start.raw(),
                    ] {
                        digest ^= value as u64;
                        digest = digest.wrapping_mul(1099511628211);
                    }
                }
            }
        }
        out.push_str(&format!(
            "{} notes, digest {digest:016x}\n",
            piece.note_count()
        ));
        out
    }

    /// The pieces the composer writes today, pinned exactly.
    ///
    /// Not because this output is sacred — it is a composer, and what it writes is a matter of
    /// taste — but because it is about to be taken apart and reassembled, and a change that
    /// nobody chose is the one thing that must not happen quietly. A fixture that moves is either
    /// a bug or a decision, and this is what makes anyone look.
    ///
    /// It last moved when comping gained the `Cross` and `Driving` figures and a chord's length
    /// became the gap to the next chord rather than a fixed beat: 179 notes to 222, every one of
    /// them in the `chords` part, with the melody, bass and kit note for note as they were.
    #[test]
    fn the_composer_writes_what_it_wrote_before() {
        // A chart nobody asked for is the composer's own, and so the only kind it colours. In a
        // major key every colour it can reach is writable as a numeral.
        assert_eq!(
            fingerprint(
                "form: verse\nkey: C major\nseed: 7\ntension: 0.95\n[section verse]\nbars: 8"
            ),
            "verse·1 C major | C→Cmaj7 G Am F→Fmaj7 C→Cmaj7 G→Gmaj7 Am→Am9 F |\n\
             222 notes, digest f7a7aa0ff3d7898e\n"
        );

        // The same in a minor key. `Fm→Gbm` is the borrow that has no spelling: `vi` read in the
        // parallel major is an F sharp minor, and in A minor no combination of degree and
        // accidental names an F sharp at all — `degree_class` measures from the key's own scale
        // at zero and from the major scale otherwise, and F sharp falls between the two.
        assert_eq!(
            fingerprint(
                "form: verse\nkey: A minor\nseed: 1\nmood: tense\n[section verse]\nbars: 8"
            ),
            "verse·1 A minor | A→Amaj7 E Fm→Fm7 D A→Amaj7 E→Emaj7 Fm→Gbm D→Dmaj7 |\n\
             242 notes, digest 37f1cdfd7defa868\n"
        );

        // A quoted chart, which is never coloured, over a form that repeats.
        assert_eq!(
            fingerprint(BASE),
            "intro·1 C major | C G Am F |\n\
             verse·1 C major | C G Am F C G Am F |\n\
             chorus·1 C major | C G Am F C G Am F |\n\
             463 notes, digest 152189cfed55124d\n"
        );

        // A transposed section, which is about to become a key change on the timeline.
        assert_eq!(
            fingerprint(
                "form: verse chorus\nchords: @marusa\nkey: C major\nseed: 3\n\
                 [section verse]\nbars: 4\n[section chorus]\nbars: 4\ntranspose: 3"
            ),
            "verse·1 C major | Fmaj7 E7 Am7 C7 |\n\
             chorus·1 Eb major | Abmaj7 G7 Cm7 Eb7 |\n\
             267 notes, digest 953bf73e67184e07\n"
        );
    }

    #[test]
    fn a_default_spec_writes_a_playable_piece() {
        let piece = compose_text("");
        assert!(!piece.tracks.is_empty(), "no tracks");
        assert!(
            piece.note_count() > 100,
            "only {} notes",
            piece.note_count()
        );
        assert_eq!(piece.tempo, 120.0);
    }

    #[test]
    fn every_section_becomes_a_clip_on_every_playing_part() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            assert_eq!(
                track.clips.len(),
                3,
                "`{}` has {} clips",
                track.name,
                track.clips.len()
            );
        }
        let bar = TimeSignature::default().ticks_per_bar();
        let lead = &piece.tracks[0];
        assert_eq!(lead.clips[0].start, Ticks::ZERO);
        assert_eq!(lead.clips[0].length, bar * 4);
        assert_eq!(lead.clips[1].start, bar * 4);
        assert_eq!(lead.clips[2].start, bar * 12);
        assert_eq!(piece.length, bar * 20);
    }

    #[test]
    fn clip_notes_are_rebased_and_fit_inside_their_clip() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    assert!(
                        note.start >= Ticks::ZERO,
                        "`{}` has a note before its start",
                        clip.name
                    );
                    assert!(
                        note.start < clip.length,
                        "`{}` has a note starting past its end",
                        clip.name
                    );
                    assert!(
                        note.end() <= clip.length,
                        "`{}` has a note overhanging by {}",
                        clip.name,
                        (note.end() - clip.length).raw()
                    );
                    assert!(note.length > Ticks::ZERO);
                    assert!((0.0..=1.0).contains(&note.velocity));
                }
            }
        }
    }

    #[test]
    fn notes_come_out_in_a_canonical_order() {
        let piece = compose_text(BASE);
        for track in &piece.tracks {
            for clip in &track.clips {
                let keys: Vec<(i64, u8)> = clip
                    .notes
                    .iter()
                    .map(|note| (note.start.raw(), note.pitch))
                    .collect();
                let mut sorted = keys.clone();
                sorted.sort_unstable();
                assert_eq!(keys, sorted, "`{}` is out of order", clip.name);
            }
        }
    }

    #[test]
    fn humanising_never_loses_a_sections_downbeat() {
        // Deleting a note nudged a few ticks before its section took the downbeat out of every
        // section but the first; it is clamped back into the clip instead.
        let straight = compose_text(BASE);
        let loose = compose_text(&BASE.replace("humanize: 0", "humanize: 1.0"));
        for (a, b) in straight.tracks.iter().zip(&loose.tracks) {
            for (before, after) in a.clips.iter().zip(&b.clips) {
                assert_eq!(
                    before.notes.len(),
                    after.notes.len(),
                    "`{}` lost {} notes to humanisation",
                    after.name,
                    before.notes.len() as i64 - after.notes.len() as i64
                );
            }
        }
    }

    #[test]
    fn an_extended_chord_is_voiced_upward_rather_than_folded_flat() {
        // A ninth folded into the triad sounds as a second against the root.
        let piece = compose_text(
            "
            key: C major
            form: verse
            chords: | Imaj9 | Imaj9 | Imaj9 | Imaj9 |
            humanize: 0
            [section verse]
            bars: 4
            [part chords]
            ",
        );
        let clip = &piece.tracks[0].clips[0];
        let first: Vec<u8> = clip
            .notes
            .iter()
            .filter(|note| note.start == Ticks::ZERO)
            .map(|note| note.pitch)
            .collect();
        assert!(first.len() >= 4, "only {} notes in the chord", first.len());
        let span = first.iter().max().unwrap() - first.iter().min().unwrap();
        assert!(
            span > 12,
            "a ninth chord spanning {span} semitones has been folded into one octave"
        );
    }

    #[test]
    fn the_same_spec_writes_the_same_piece() {
        assert_eq!(compose_text(BASE), compose_text(BASE));
    }

    #[test]
    fn a_part_that_never_plays_leaves_no_track() {
        let piece = compose_text(
            "
            form: intro
            humanize: 0

            [section intro]
            bars: 4
            parts: bass

            [part bass]
            [part hat]
            ",
        );
        let names: Vec<&str> = piece.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["bass"],
            "a silent part should not leave an empty track"
        );
    }

    #[test]
    fn a_clip_is_named_after_its_section_and_part() {
        let piece = compose_text(BASE);
        let lead = &piece.tracks[0];
        assert_eq!(lead.clips[0].name, format!("intro 1 · {}", lead.name));
        assert_eq!(lead.clips[1].name, format!("verse 1 · {}", lead.name));
    }

    #[test]
    fn the_summary_names_every_track() {
        let piece = compose_text(BASE);
        let summary = piece.summary();
        assert!(summary.contains("Test"));
        assert!(summary.contains("120 BPM"));
        for track in &piece.tracks {
            assert!(summary.contains(&track.name), "`{}` is missing", track.name);
        }
    }

    #[test]
    fn humanising_never_pushes_a_note_out_of_its_clip() {
        // The one place humanisation could corrupt the document rather than just move a note.
        let piece = compose_text(&BASE.replace("humanize: 0", "humanize: 1.0"));
        for track in &piece.tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    assert!(note.start >= Ticks::ZERO);
                    assert!(note.end() <= clip.length);
                }
            }
        }
    }

    #[test]
    fn a_named_progression_reaches_the_notes() {
        // The bass plays roots, so its first note of each bar spells the progression out.
        let piece = compose_text(
            "
            key: C major
            form: verse
            chords: @marusa
            humanize: 0
            [section verse]
            bars: 4
            [part bass]
            ",
        );
        let bass = &piece.tracks[0];
        let bar = TimeSignature::default().ticks_per_bar();
        let roots: Vec<u8> = (0..4)
            .filter_map(|index| {
                bass.clips[0]
                    .notes
                    .iter()
                    .find(|note| note.start == bar * index)
                    .map(|note| note.pitch % 12)
            })
            .collect();
        // F, E, A, C — the roots of 丸サ進行 in C.
        assert_eq!(roots, vec![5, 4, 9, 0]);
    }
}
