//! Token state machine matching MuScriptor 0.3.0 OpenNoteTracker and tie forcing.

use super::{AnalysisError, MixtureNote, error};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
pub(super) struct VocabularyEntry {
    r#type: String,
    value: i64,
}

fn event(token: i64) -> Option<(&'static str, i64)> {
    Some(match token {
        0 => ("PAD", 0),
        1 => ("EOS", 0),
        2 => ("UNK", 0),
        3..=1003 => ("shift", token - 3),
        1004..=1131 => ("pitch", token - 1004),
        1132..=1133 => ("velocity", token - 1132),
        1134 => ("tie", 0),
        1135..=1264 => ("program", token - 1135),
        1265..=1392 => ("drum", token - 1265),
        _ => return None,
    })
}
pub(super) fn valid_vocabulary(v: &[VocabularyEntry]) -> bool {
    v.len() == 1393
        && v.iter()
            .enumerate()
            .all(|(i, e)| event(i as i64) == Some((e.r#type.as_str(), e.value)))
}

pub(super) struct Tracker<'a> {
    programs: &'a [String],
    open: BTreeMap<(i64, u8), f64>,
    notes: Vec<MixtureNote>,
    seek: f64,
    next: Option<f64>,
    tick: i64,
    program: Option<i64>,
    velocity: Option<i64>,
    prologue: bool,
    skip: bool,
    ties: BTreeSet<(i64, u8)>,
    started: bool,
}
impl<'a> Tracker<'a> {
    pub(super) fn new(programs: &'a [String]) -> Self {
        Self {
            programs,
            open: BTreeMap::new(),
            notes: Vec::new(),
            seek: 0.0,
            next: None,
            tick: 0,
            program: None,
            velocity: None,
            prologue: true,
            skip: false,
            ties: BTreeSet::new(),
            started: false,
        }
    }
    fn close(&mut self, key: (i64, u8), end: f64) {
        if let Some(start) = self.open.remove(&key) {
            self.notes.push(MixtureNote {
                pitch: key.1,
                start,
                end,
                instrument: self.programs[key.0 as usize].clone(),
            });
        }
    }
    fn close_all(&mut self, end: f64) {
        for key in self.open.keys().copied().collect::<Vec<_>>() {
            self.close(key, end);
        }
    }
    pub(super) fn boundary(&mut self, seek: f64, next: Option<f64>) {
        if self.started && self.prologue {
            self.close_all(self.seek);
        }
        self.seek = seek;
        self.next = next;
        self.tick = (seek * 100.0).round() as i64;
        self.program = None;
        self.velocity = None;
        self.prologue = true;
        self.skip = false;
        self.ties.clear();
        self.started = true;
    }
    pub(super) fn prompt(&self) -> Vec<i64> {
        let mut result = Vec::new();
        let mut program = None;
        for &(p, pitch) in self.open.keys() {
            if program != Some(p) {
                result.push(1135 + p);
                program = Some(p);
            }
            result.push(1004 + i64::from(pitch));
        }
        result.push(1134);
        result
    }
    pub(super) fn feed(&mut self, token: i64) -> Result<(), AnalysisError> {
        if self.notes.len() + self.open.len() > 200_000 {
            return Err(error("too many decoded notes"));
        }
        let (kind, value) = event(token).ok_or_else(|| error("invalid MuScriptor token"))?;
        if self.prologue {
            match kind {
                "tie" => {
                    self.prologue = false;
                    self.velocity = None;
                    let ended: Vec<_> = self
                        .open
                        .keys()
                        .filter(|k| !self.ties.contains(k))
                        .copied()
                        .collect();
                    for key in ended {
                        self.close(key, self.seek);
                    }
                }
                "shift" => {
                    self.prologue = false;
                    self.skip = true;
                    self.close_all(self.seek);
                }
                "program" => self.program = Some(value),
                "pitch" => {
                    if let Some(p) = self.program {
                        self.ties.insert((p, value as u8));
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        if self.skip {
            return Ok(());
        }
        let time = self.tick as f64 / 100.0;
        match kind {
            "shift" if value > 0 => self.tick = (self.seek * 100.0).round() as i64 + value,
            "program" => self.program = Some(value),
            "velocity" => self.velocity = Some(value),
            "drum" if self.next.is_none_or(|n| time < n) => self.notes.push(MixtureNote {
                pitch: value as u8,
                start: time,
                end: time + 0.01,
                instrument: "drums".into(),
            }),
            "pitch" if self.next.is_none_or(|n| time < n) => {
                if let (Some(p), Some(v)) = (self.program, self.velocity) {
                    let key = (p, value as u8);
                    self.close(key, time);
                    if v > 0 {
                        self.open.insert(key, time);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn finish(mut self) -> Vec<MixtureNote> {
        if self.started && self.prologue {
            self.close_all(self.seek);
        } else {
            for (key, start) in self.open.clone() {
                self.close(key, start + 0.01);
            }
        }
        self.notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ties_retriggers_drums_and_absolute_chunk_shifts() {
        let names = vec!["piano".into(); 130];
        let mut t = Tracker::new(&names);
        t.boundary(0.0, Some(5.0));
        for x in [1134, 1135, 1133, 103, 1064] {
            t.feed(x).unwrap();
        }
        t.boundary(5.0, None);
        assert_eq!(t.prompt(), vec![1135, 1064, 1134]);
        for x in t.prompt() {
            t.feed(x).unwrap();
        }
        for x in [1132, 103, 1064, 1265 + 36] {
            t.feed(x).unwrap();
        }
        let notes = t.finish();
        assert_eq!(notes.len(), 2);
        assert_eq!(
            (notes[0].pitch, notes[0].start, notes[0].end),
            (60, 1.0, 6.0)
        );
        assert_eq!(notes[1].instrument, "drums");
        assert_eq!(notes[1].end, 6.01);
    }
    #[test]
    fn missing_tie_drops_chunk_and_missing_offset_uses_minimum_duration() {
        let names = vec!["piano".into(); 130];
        let mut t = Tracker::new(&names);
        t.boundary(0.0, Some(5.0));
        for x in [1134, 1135, 1133, 103, 1064] {
            t.feed(x).unwrap();
        }
        t.boundary(5.0, Some(10.0));
        for x in [103, 1135, 1133, 1065] {
            t.feed(x).unwrap();
        }
        t.boundary(10.0, None);
        for x in [1134, 1135, 1133, 53, 1067] {
            t.feed(x).unwrap();
        }
        let notes = t.finish();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].end, 5.0);
        assert_eq!((notes[1].start, notes[1].end), (10.5, 10.51));
    }
}
