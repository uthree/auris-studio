//! Shared phrase boundaries and functions, decided before any part is written.

use crate::PerformanceStyle;

/// What a phrase contributes to the surrounding passage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhraseRole {
    /// Introduces or recalls the recognizable material.
    Statement,
    /// Develops that material without announcing an arrival yet.
    Continuation,
    /// Answers the preceding phrase.
    Answer,
    /// Leaves space at the end of a longer passage.
    Release,
}

/// A half-open span of bars shared by harmony, melody and accompaniment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhrasePlan {
    /// First bar, relative to its section.
    pub start_bar: usize,
    /// Number of bars, always positive.
    pub bars: usize,
    /// Its function within the section.
    pub role: PhraseRole,
}

impl PhrasePlan {
    /// First bar after this phrase.
    pub fn end_bar(&self) -> usize {
        self.start_bar + self.bars
    }
}

/// Plans balanced phrases without a stray one-bar tail.
///
/// Sustained and orchestral palettes have room for an eight-bar breath; rhythmic palettes
/// start with four. Short or odd sections are divided evenly, so five bars are 3 + 2 and
/// ten bars in a rhythmic palette are 4 + 3 + 3. Explicit harmony is never rewritten here.
pub fn plan_phrases(bars: usize, style: Option<PerformanceStyle>) -> Vec<PhrasePlan> {
    let span = match style {
        Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral) => 8,
        _ => 4,
    };
    let count = bars.div_ceil(span);
    let mut start_bar = 0;
    (0..count)
        .map(|index| {
            let length = bars / count + usize::from(index < bars % count);
            let role = if index == 0 || index % 4 == 2 {
                PhraseRole::Statement
            } else if index + 1 == count && count > 2 {
                PhraseRole::Release
            } else if index % 4 == 1 && index + 1 < count {
                PhraseRole::Continuation
            } else {
                PhraseRole::Answer
            };
            let phrase = PhrasePlan {
                start_bar,
                bars: length,
                role,
            };
            start_bar += length;
            phrase
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_cover_odd_and_short_sections_without_gaps() {
        for style in [None, Some(PerformanceStyle::Ambient)] {
            for bars in 0..65 {
                let phrases = plan_phrases(bars, style);
                assert_eq!(phrases.iter().map(|p| p.bars).sum::<usize>(), bars);
                assert!(phrases.iter().all(|p| p.bars > 0));
                assert!(phrases.windows(2).all(|p| p[0].end_bar() == p[1].start_bar));
                if bars > 1 {
                    assert!(phrases.iter().all(|p| p.bars > 1));
                }
            }
        }
        assert_eq!(
            plan_phrases(5, None)
                .iter()
                .map(|p| p.bars)
                .collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(
            plan_phrases(10, None)
                .iter()
                .map(|p| p.bars)
                .collect::<Vec<_>>(),
            [4, 3, 3]
        );
    }
}
