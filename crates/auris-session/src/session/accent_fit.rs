//! Where a sung melody moves against the spoken pitch accent of its words.
//!
//! Tokyo-dialect Japanese rises onto a word's second mora and falls once, after its nucleus
//! (`auris_vocal::accent_contour`). A melody stepping the other way can make 箸 heard as 橋.
//! Pop songs override the accent somewhere on purpose — a chorus hook outranks the word — so
//! this is a report, never a warning: it says where the melody and the words disagree, and the
//! composer decides which wins.
//!
//! Each clip's words are read again through the dictionary, from the text its notes carry in
//! order, and the moras are matched to the notes by their phonemes. That reads both ways a
//! lyric is written: one mora's text on each note, or a whole phrase on its first note with `+`
//! continuations. A clip whose phonemes do not match the dictionary's reading, because a person
//! corrected them by hand, is counted as unread rather than guessed at.

use auris_core::theory::contour::Contour;
use auris_core::{Ticks, TrackId};
use serde_json::{Value, json};

use super::Session;
use super::singer::LYRIC_CONTINUATION;

/// Clips listed in the report at most; the counts cover the rest.
const LISTED_CLIPS: usize = 12;
/// Steps listed for one clip at most.
const LISTED_STEPS: usize = 16;
/// Where a lyric is cut into separately read phrases. The step across a cut is free, as the
/// step into an accent phrase is.
const PHRASE_BREAKS: [char; 10] = ['\n', '\r', '、', '。', '！', '？', '!', '?', ' ', '　'];

/// One mora as the dictionary reads it, with what it asks of the step arriving at it.
struct Mora {
    phonemes: Vec<String>,
    contour: Contour,
    text: String,
}

impl Session {
    /// The spoken pitch accent of `track`'s lyrics against the steps of its melody, or `None`
    /// when the track sings nothing or no dictionary is loaded to read the accent.
    pub fn accent_fit(&self, track: TrackId) -> Option<Value> {
        let dictionary = self.japanese.as_ref()?;
        let project = self.project();
        let singer = project.track(track)?.kind.as_singer()?;
        let (mut checked, mut against, mut unread, mut read) = (0, 0, 0, 0);
        let mut listed = Vec::new();
        for clip in singer.clips.iter().filter(|clip| !clip.muted) {
            let bpm = project.tempo_map.bpm_at(clip.start);
            let mut notes: Vec<_> = clip
                .sounding_notes_with_meter(bpm, project.signatures.clone())
                .filter(|note| !note.phonemes.is_empty())
                .collect();
            notes.sort_by_key(|note| note.start);
            let text: String = notes
                .iter()
                .map(|note| note.lyric.trim())
                .filter(|lyric| *lyric != LYRIC_CONTINUATION)
                .collect();
            if text.trim().is_empty() {
                continue;
            }
            let Some(moras) = read_moras(dictionary, &text) else {
                unread += 1;
                continue;
            };
            let phonemes: Vec<&[String]> = notes.iter().map(|n| n.phonemes.as_slice()).collect();
            let Some(arrivals) = align(&phonemes, &moras) else {
                unread += 1;
                continue;
            };
            read += 1;
            let mut wrong = Vec::new();
            for index in 1..notes.len() {
                let (contour, text) = &arrivals[index];
                let step = i32::from(notes[index].pitch) - i32::from(notes[index - 1].pitch);
                let kept = match contour {
                    Contour::Free => continue,
                    Contour::Rise => step > 0,
                    Contour::Fall => step < 0,
                    Contour::NoFall => step >= 0,
                };
                checked += 1;
                if !kept {
                    against += 1;
                    if wrong.len() < LISTED_STEPS {
                        let at: Ticks = clip.start + notes[index].start;
                        wrong.push(json!({"bar": project.signatures.bar_of(at), "mora": text,
                            "wants": wants(*contour), "step": step}));
                    }
                }
            }
            if !wrong.is_empty() && listed.len() < LISTED_CLIPS {
                listed.push(json!({"clip": clip.id.0, "name": clip.name, "against": wrong}));
            }
        }
        if read + unread == 0 {
            return None;
        }
        Some(json!({
            "checked_steps": checked,
            "against_accent": against,
            "clips_read": read,
            "clips_unread": unread,
            "clips": listed,
            "note": "Spoken Tokyo pitch accent against the melody's steps: wants is the direction the word moves when spoken, step the semitones the melody moves. Information only, never a warning; a hook may override the accent on purpose, but a word sung against its accent can be heard as another word.",
        }))
    }
}

/// The dictionary's moras for `text`, phrase by phrase, or `None` when it cannot read it.
fn read_moras(dictionary: &auris_vocal::JapaneseDictionary, text: &str) -> Option<Vec<Mora>> {
    let mut moras = Vec::new();
    for segment in text.split(PHRASE_BREAKS).filter(|s| !s.trim().is_empty()) {
        for phrase in dictionary.accent_phrases(segment).ok()? {
            for (mora, contour) in phrase.moras.iter().zip(phrase.contour()) {
                moras.push(Mora {
                    phonemes: mora.phonemes.clone(),
                    contour,
                    text: mora.text.clone(),
                });
            }
        }
    }
    Some(moras)
}

/// The contour and text arriving at each note, or `None` when the notes' phonemes are not the
/// dictionary's moras in order. A note may carry two moras: ん or a bare vowel riding on the
/// one before.
fn align(notes: &[&[String]], moras: &[Mora]) -> Option<Vec<(Contour, String)>> {
    let mut arrivals = Vec::new();
    let mut next = 0;
    for phonemes in notes {
        let first = moras.get(next)?;
        if same_mora(phonemes, &first.phonemes) {
            arrivals.push((first.contour, first.text.clone()));
            next += 1;
            continue;
        }
        let second = moras.get(next + 1)?;
        let joined: Vec<String> = first
            .phonemes
            .iter()
            .chain(&second.phonemes)
            .cloned()
            .collect();
        if !same_mora(phonemes, &joined) {
            return None;
        }
        arrivals.push((first.contour, format!("{}{}", first.text, second.text)));
        next += 2;
    }
    (next == moras.len()).then_some(arrivals)
}

/// Whether a note's phonemes are the dictionary's mora. A long vowel is sung as it is written
/// and read as it is spoken — 向こう is ko-ɯ on the notes and ko-o in the dictionary — so the
/// last vowel may differ.
fn same_mora(sung: &[String], read: &[String]) -> bool {
    const VOWELS: [&str; 5] = ["a", "i", "ɯ", "e", "o"];
    sung == read
        || (sung.len() == read.len()
            && sung[..sung.len() - 1] == read[..read.len() - 1]
            && sung
                .last()
                .zip(read.last())
                .is_some_and(|(a, b)| VOWELS.contains(&a.as_str()) && VOWELS.contains(&b.as_str())))
}

fn wants(contour: Contour) -> &'static str {
    match contour {
        Contour::Rise => "rise",
        Contour::Fall => "fall",
        Contour::NoFall => "no fall",
        Contour::Free => "free",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mora(phonemes: &[&str], contour: Contour, text: &str) -> Mora {
        Mora {
            phonemes: phonemes.iter().map(|p| p.to_string()).collect(),
            contour,
            text: text.into(),
        }
    }

    #[test]
    fn a_note_may_carry_a_second_mora_and_a_mismatch_reads_nothing() {
        let moras = [
            mora(&["s", "a"], Contour::Free, "さ"),
            mora(&["N"], Contour::Rise, "ん"),
            mora(&["p", "o"], Contour::NoFall, "ぽ"),
        ];
        let sa_n = vec!["s".to_string(), "a".into(), "N".into()];
        let po = vec!["p".to_string(), "o".into()];
        let arrivals = align(&[&sa_n, &po], &moras).unwrap();
        assert_eq!(arrivals[0].1, "さん");
        assert_eq!(arrivals[1].0, Contour::NoFall);
        // A hand-corrected phoneme, or a mora left over, is not guessed at.
        let ka = vec!["k".to_string(), "a".into()];
        assert!(align(&[&sa_n, &ka], &moras).is_none());
        assert!(align(&[&sa_n], &moras).is_none());
        // こう is sung ko-ɯ and read ko-o.
        let ko = [
            mora(&["k", "o"], Contour::Free, "こ"),
            mora(&["o"], Contour::Fall, "う"),
        ];
        let u = vec!["ɯ".to_string()];
        let ko_sung = vec!["k".to_string(), "o".into()];
        assert!(align(&[&ko_sung, &u], &ko).is_some());
    }
}
