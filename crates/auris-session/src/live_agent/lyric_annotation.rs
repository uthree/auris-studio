//! The agent's door to the song-wide lyrics annotation.
use super::*;

/// One line of the song-wide lyrics, placed by bar and beat.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineInput {
    /// The line as written for a reader: original kanji/kana spelling, never the reading.
    #[schemars(length(min = 1, max = 1024))]
    pub text: String,
    /// 1-based song bar where the line's first word is sung.
    #[schemars(range(min = 1))]
    pub start_bar: u32,
    /// 1-based beat of start_bar, fractional allowed (2.5 is the second half of beat 2).
    #[serde(default = "first_beat")]
    pub start_beat: f64,
    /// 1-based song bar where the line stops being shown, normally where its last note ends.
    #[schemars(range(min = 1))]
    pub end_bar: u32,
    /// 1-based beat of end_bar, fractional allowed.
    #[serde(default = "first_beat")]
    pub end_beat: f64,
}

fn first_beat() -> f64 {
    1.0
}

/// What edit_lyrics does to the song-wide lyrics annotation.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Read every sung phrase back off a singer track's notes (a rest of a beat or more ends a line) and replace the annotation over the sung stretch. Omit track for the first singer with lyrics.
    FromVocals {
        /// Stable singer track ID; omit for the lead.
        track: Option<u64>,
    },
    /// Write explicit lines, each shown from its start to its end. Replaces annotation inside each line and keeps what follows it.
    Write {
        /// 1..512 lines.
        #[schemars(length(min = 1, max = 512))]
        lines: Vec<LineInput>,
    },
    /// Remove the annotation over a bar range.
    Clear {
        /// First bar, 1-based.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Duration in bars, 1..4096.
        #[schemars(range(min = 1, max = 4096))]
        bars: u32,
    },
}

impl Session {
    pub(super) fn agent_edit_lyrics(&mut self, operation: Operation) -> Result<String, String> {
        let written = match operation {
            Operation::FromVocals { track } => {
                let track = track.map(TrackId);
                if let Some(id) = track
                    && !self
                        .project()
                        .track(id)
                        .is_some_and(|track| track.kind.is_singer())
                {
                    return Err("track must be a singer track ID from inspect_project".into());
                }
                let lines = self.annotate_lyrics_from_vocals(track);
                if lines == 0 {
                    return Err("No sung lyrics found; write_lyrics on a singer clip first, or use kind write".into());
                }
                lines
            }
            Operation::Write { lines } => {
                if lines.is_empty() || lines.len() > 512 {
                    return Err("Supply 1..512 lines".into());
                }
                let signatures = self.project().signatures.clone();
                let mut placed = Vec::with_capacity(lines.len());
                for line in &lines {
                    let text = line.text.trim();
                    if text.is_empty() || text.len() > 1024 {
                        return Err("Each line needs nonempty text within 1024 UTF-8 bytes".into());
                    }
                    if line.start_bar == 0
                        || line.end_bar == 0
                        || !line.start_beat.is_finite()
                        || !line.end_beat.is_finite()
                        || line.start_beat < 1.0
                        || line.end_beat < 1.0
                    {
                        return Err("Bars and beats are 1-based".into());
                    }
                    let start = signatures.position(line.start_bar, line.start_beat);
                    let end = signatures.position(line.end_bar, line.end_beat);
                    if end <= start {
                        return Err(format!("The line \"{text}\" must end after it starts"));
                    }
                    placed.push((start, end, text.to_owned()));
                }
                self.begin_transaction(crate::history::Edit::SetLyricLine);
                for (start, end, text) in &placed {
                    self.write_lyric_line(*start, *end, text);
                }
                self.end_transaction();
                placed.len()
            }
            Operation::Clear { start_bar, bars } => {
                if start_bar == 0 || !(1..=4096).contains(&bars) {
                    return Err("start_bar must be at least 1 and bars within 1..4096".into());
                }
                let after = start_bar.checked_add(bars).ok_or("bar range overflow")?;
                let from = self.project().signatures.bar_start(start_bar);
                let to = self.project().signatures.bar_start(after);
                self.clear_lyric_lines(from, to);
                0
            }
        };
        Ok(serde_json::json!({
            "lines_written": written,
            "lyrics": lyric_lines_json(self.project()),
            "unsaved": true,
            "usage": "inspect_project returns the same lyrics. The annotation is what video captions and lyric sheets show; it never changes notes."
        })
        .to_string())
    }
}

/// The song-wide lyrics as an agent reads them: each sung line with its bar and beat.
pub(crate) fn lyric_lines_json(project: &auris_core::Project) -> serde_json::Value {
    let end = project.lyrics.end().max(project.end_tick()) + Ticks(1);
    project
        .lyrics
        .spans_in(Ticks::ZERO, end)
        .into_iter()
        .map(|span| {
            let (bar, beat, _) = project.signatures.bar_beat_at(span.start);
            let (end_bar, end_beat, _) = project.signatures.bar_beat_at(span.end);
            serde_json::json!({
                "text": span.text, "start_tick": span.start.raw(), "end_tick": span.end.raw(),
                "start_bar": bar, "start_beat": beat, "end_bar": end_bar, "end_beat": end_beat,
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod tests {
    use crate::live_agent::Command;
    use auris_core::time::Ticks;

    fn session() -> crate::Session {
        crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap()
    }

    fn run(session: &mut crate::Session, value: serde_json::Value) -> Result<String, String> {
        session.agent_command(serde_json::from_value::<Command>(value).unwrap())
    }

    #[test]
    fn explicit_lines_are_placed_by_bar_and_beat_and_undo_together() {
        let mut session = session();
        let report = run(
            &mut session,
            serde_json::json!({"action":"edit_lyrics","operation":{"kind":"write","lines":[
                {"text":"桜咲いた","start_bar":1,"end_bar":2},
                {"text":"春が来た","start_bar":2,"start_beat":3,"end_bar":3}
            ]}}),
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report["lines_written"], 2);
        assert_eq!(report["lyrics"][1]["start_beat"], 3);
        let lyrics = &session.project().lyrics;
        assert_eq!(lyrics.text_at(Ticks::ZERO), Some("桜咲いた"));
        assert_eq!(lyrics.text_at(Ticks(3840)), None);
        assert_eq!(lyrics.text_at(Ticks(3840 + 1920)), Some("春が来た"));
        assert!(session.undo().is_some());
        assert!(session.project().lyrics.is_empty());
    }

    #[test]
    fn a_backwards_line_or_nothing_sung_is_refused_without_an_edit() {
        let mut session = session();
        assert!(
            run(
                &mut session,
                serde_json::json!({"action":"edit_lyrics","operation":{"kind":"write","lines":[
                    {"text":"x","start_bar":3,"end_bar":2}
                ]}}),
            )
            .is_err()
        );
        assert!(
            run(
                &mut session,
                serde_json::json!({"action":"edit_lyrics","operation":{"kind":"from_vocals"}}),
            )
            .is_err()
        );
        assert!(session.undo().is_none());
    }
}
