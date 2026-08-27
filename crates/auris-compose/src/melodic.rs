//! Why the tune is shaped the way it is, and how that was measured.
//!
//! Nothing here is code. It is the account of one question — *why do the composer's melodies sound
//! unnatural when its accompaniment does not* — and of the answer, because the answer is a set of
//! numbers that the next person to touch `parts::melody` will otherwise
//! have to find again. The constants in that module are what this page argues for; read them
//! together or neither will make sense.
//!
//! # The asymmetry the question starts from
//!
//! An accompaniment is right or wrong *locally*. Given the chord, a bass note is a member of it or
//! it is not, a voicing is close or it is not, a kick pattern is in the meter or it is not — and
//! every one of those rules fits in a function that can see one chord at a time. That is why
//! `parts::bass` and `parts::comp` sound like players
//! and why they were the easy half of the composer.
//!
//! A melody has no such external specifier. What makes a tune well formed is a property of its own
//! trajectory over time — where it has been, where it is going, what it has already said. The
//! literature puts it the same way: a melody generator has no "explicit harmonic background, which
//! is a crucial guide for note selection", and harmonising an existing melody is "a smaller
//! challenge than generating an entire well-harmonized piece from scratch".
//!
//! So the composer's melody writer was solving the wrong shape of problem. It chose each note from
//! a table that did not know what the note before it had done.
//!
//! # What was measured
//!
//! Eight presets at four seeds each, every melodic interval the composer wrote: 5,147 of them.
//! Against the corpus figures — 68% steps, 21% leaps and 11% repetitions in Palestrina; a mean
//! interval of 2.8 semitones with repetitions removed in folk song; post-leap reversal at about
//! seven times in ten.
//!
//! | | before | after | corpus |
//! | --- | --- | --- | --- |
//! | step (1–2 semitones) | 31.4% | 41.8% | 68% |
//! | leap of a fourth or wider | **34.3%** | **15.1%** | 21% for *all* leaps |
//! | repeated note | 14.0% | 22.8% | 11% |
//! | mean interval, repeats removed | 3.89 | 3.00 | 2.8 |
//! | post-leap reversal | 65.7% | 73.7% | ~70% |
//! | step followed by a step the same way | 16.7% | 30.2% | the modal continuation |
//!
//! A third of every melodic move being a fourth or wider is not a tune. It is an arpeggio's
//! interval distribution, and it came from one line: the figure drew its moves from a table of
//! scale steps that was half seconds, a quarter thirds and a quarter **fourths**.
//!
//! The 65.7% post-leap reversal in the *before* column is worth a warning. It looks healthy and
//! meant nothing: von Hippel and Huron's finding is that most of post-leap reversal in real music
//! is regression to the mean seen from one side, and a bounded random walk regresses to its mean
//! without being told to. A statistic that a null model reproduces is not evidence of health.
//!
//! # Where the width came from
//!
//! Splitting the same intervals three ways said which of three suspects was guilty, and the answer
//! was not the one that looked most likely:
//!
//! | | mean interval | leaps | n |
//! | --- | --- | --- | --- |
//! | crossing a bar line | **4.24** | 44.5% | 1,100 |
//! | inside one bar | 3.10 | 31.5% | 4,047 |
//! | arriving on a step snapped to a chord tone | 3.91 | 46.6% | **58** |
//!
//! The chord-tone snapping is the widest per interval and reaches 58 notes in five thousand. It
//! was the first hypothesis and it was wrong. What mattered was the bar line: the figure restarted
//! from its anchor every bar, so the join between one bar and the next was whatever fell out of the
//! difference between two structural pitches — and nothing had chosen it.
//!
//! # The five rules that came out of it
//!
//! In the order they were applied, which is the order of what they were worth.
//!
//! 1. **The join is chosen.** A restated figure is shifted bodily by up to a third so that its
//!    first note continues from where the last bar finished — see
//!    `join_offset` in `parts::melody`. Bar crossings went from a mean interval of 4.24 to
//!    2.4, in line with the rest of the line rather than twice as wide.
//! 2. **The interval table is the corpus distribution.** Including repeated notes, which the old
//!    table had no entry for at all: every note had to go somewhere, and the repeats that did occur
//!    were accidents of the range clamp.
//! 3. **The walk has a memory.** After a leap the line turns and fills the gap in; after a step it
//!    tends to carry on the same way. Neither is absolute — a melody that filled in every leap
//!    would be as mechanical as one that never did.
//! 4. **A dissonance resolves by step.** A non-chord tone left by a leap is not heard as leaning on
//!    its neighbour, it is heard as a wrong note. This is the most reliable single marker of a tune
//!    a machine wrote.
//! 5. **A phrase ends on a chord tone.** The last note of a closing bar is treated as strong
//!    however weak its step: a phrase that ends on a passing note has not ended, it has stopped.
//!
//! Rule 3 does not work without a sixth that is not on anybody's list. Inertia gives the walk a
//! direction, and a walk with a direction meets its range clamp and stands there — adding inertia
//! alone took repeated notes from 14% to 31%, because a figure at its ceiling repeats the note it
//! is standing on until something turns it round. Melodic regression to the mean is what turns it
//! round, and it is in the literature for its own sake.
//!
//! # The second pass: the repeats nobody drew
//!
//! The table above left steps at 41.8% against the corpus 68, and most of the difference had gone
//! into repeated notes: 22.8% of the line stood still where corpora say 11. The interval table
//! draws a repeat one time in nine, so half of those were accidents — and classifying every
//! repeated pair by where it sat said which accidents:
//!
//! | where the second note of a repeated pair sat | of 1,176 repeats |
//! | --- | --- |
//! | crossing a bar line | 221 |
//! | within two semitones of a range edge | 155 |
//! | in a closing bar, or beside a snapped strong step | 267 |
//! | elsewhere — mostly the table's own draws | 533 |
//!
//! Two fixes in `parts::melody` came out of it:
//!
//! 1. **The join is chosen against the note that will actually play.** A bar's first note is
//!    almost always on a strong step, where it is snapped onto a chord tone *after* the join is
//!    chosen — so `join_offset` was optimising a pitch that never sounded, and the snap pulled a
//!    fifth of all bar crossings back onto the very note the last bar had ended on. The landings
//!    are snapped first now, and a repeated landing is ranked below anything within a fourth.
//! 2. **A repeat the range made is undone.** `shift_within` shrinks an interval at the edge of
//!    the range, and two cells that asked for *different* degrees could arrive on one pitch.
//!    `unstick` moves the second of them one scale step onward — or back, at the very edge.
//!
//! | | before | after | corpus |
//! | --- | --- | --- | --- |
//! | step (1–2 semitones) | 41.8% | 53.3% | 68% |
//! | repeated note | 22.8% | 10.5% | 11% |
//! | leap of a fourth or wider | 15.1% | 15.0% | 21% for *all* leaps |
//! | mean interval, repeats removed | 3.00 | 2.84 | 2.8 |
//! | post-leap reversal | 73.7% | 71.2% | ~70% |
//! | bar crossing against inside a bar | 3.09 / 2.98 | 2.97 / 2.81 | in line |
//!
//! The repeats were not redistributed at random: the leap share did not move, so every repeat the
//! two fixes removed became a step or a third. `vary_motif` can still write a degree-space repeat
//! into a closing bar, and that is left alone on purpose — the repeat share now sits *at* the
//! corpus figure, and removing those too would push it under it.
//!
//! # The third pass: the anchor under a busy bar
//!
//! The bar line was not the only place the figure's footing moved without a join being chosen: a
//! chord change *inside* a bar moved the anchor under a figure that was mid-flight, the same
//! fault in a place the presets could not show — their charts are all one chord to the bar, so
//! not one of the 5,147 intervals above crosses a mid-bar change. A two-chords-a-bar chart
//! measured it at 3.32 semitones per crossing against 2.28 inside one chord, the bar line's old
//! ratio almost exactly. The repair is the bar line's too: the figure is re-joined to the note
//! it just played whenever it walks onto a new event, within `JOIN_REACH` of where it sat, and
//! the crossings came in at 2.34 against 2.27. `a_chord_change_inside_a_bar_is_joined_like_a_bar_line`
//! in `parts::melody` holds the chart that showed it.
//!
//! # The fourth pass: the germ
//!
//! The passes above straightened the line inside a section; this one is about the piece. Every
//! section drew its own contour from its own stream, so one song had as many tunes as it had
//! section names — the architecture said "state, restate, answer" and then the chorus answered a
//! question the verse never asked. Now one contour per part — the **germ** — is drawn at a
//! piece-level stream, and every section's figure wears it, re-sampled onto the section's own
//! rhythm: a busy chorus fills the line in with passing steps, a sparse verse says it in fewer,
//! wider words. Plain rounding of the resampled line put a stammer where every gentle slope
//! crossed a half — repeats hit 19.3% — so a landing that rounds onto its predecessor while the
//! line is moving steps with the line instead; where the germ wrote a genuine repeat the line is
//! flat and the repeat is kept.
//!
//! Measured over the presets, three seeds each, contour correlation resampled to 32 points:
//!
//! | pair                             | before | after |
//! |----------------------------------|--------|-------|
//! | different sections of one song   | 0.40   | 0.46  |
//! | sections of different songs      | 0.15   | 0.04  |
//!
//! And the interval grammar improved as a side effect, because the fill-in notes are steps by
//! construction where a freshly drawn contour was free to leap: steps 55.4% → 60.5%, mean
//! interval 2.53 → 2.29 semitones, repeats 9.9% → 12.1% against the corpus' 11.
//! `a_verse_and_a_chorus_are_two_statements_of_one_tune` in `parts::melody` holds the shape, and
//! `dressed` is the resampling with the anti-stammer rule.
//!
//! # What is still wrong
//!
//! Steps sit at 60.5% where a corpus says 68. One thing is known and not fixed: resolving a
//! dissonance onto a chord tone sometimes lands on the note beside it, which is refused where it
//! would stutter but not otherwise.
//!
//! # What is not a code problem
//!
//! The question this began with was whether an unnatural melody is intrinsic to composing without
//! a learned model. It is not, and the reason is not an argument:
//!
//! * Systems that learn nothing have passed listening tests. In 1997 an audience at the University
//!   of Oregon heard three pieces in the style of Bach — by Bach, by a human composer, and by
//!   Cope's *Experiments in Musical Intelligence* — and picked the program's as the real Bach.
//!   Ebcioğlu's CHORAL harmonises chorales with some 350 rules in first-order predicate calculus.
//!   MorpheuS generates structured pieces by combinatorial optimisation with no learning at all,
//!   and they have been performed in concert.
//! * Every deficit in the table above is a *published, closed-form* regularity. Pitch proximity,
//!   step inertia, step declination, the melodic arch and regression to the mean were found by
//!   counting intervals in corpora, not by fitting a network, and each is a few lines to encode.
//!
//! What is genuinely hard without a corpus is two other things, and they should not be confused
//! with this one. **Long-term structure** — MorpheuS calls it "an important challenge, which is key
//! to conveying a sense of musical coherence" — although this composer is better placed there than
//! most, because a section states a figure, restates it and answers it, which is the right
//! architecture and the reason its pieces have anything to remember. And **style**: what makes a
//! line sound like city pop rather than merely well formed is exactly the thing a distribution over
//! a corpus knows and a rule does not.
//!
//! # Sources
//!
//! * Huron, *Sweet Anticipation: Music and the Psychology of Expectation*, MIT Press, 2006 —
//!   chapter 5 for pitch proximity, step declination, step inertia, melodic regression and the
//!   melodic arch, and for post-leap reversal as regression to the mean, after von Hippel.
//! * Chiu and Temperley, "Melodic Differences Between Styles: Modeling Music With Step Inertia",
//!   *Music & Science*, 2024.
//! * Temperley, "Probabilistic Models of Melodic Interval", *Music Perception*, 2014.
//! * Narmour, the implication-realization model: a small interval implies continuation, a large one
//!   implies reversal.
//! * Herremans and Chew, "MorpheuS: Generating Structured Music with Constrained Patterns and
//!   Tension", *IEEE Transactions on Affective Computing*.
//! * Herremans, Chuan and Chew, "A Functional Taxonomy of Music Generation Systems",
//!   *ACM Computing Surveys* 50(5), 2017.
//! * Ebcioğlu, "An Expert System for Harmonizing Chorales in the Style of J. S. Bach",
//!   *Journal of Logic Programming*, 1990.
//!
//! # Reproducing the numbers
//!
//! The measurement is [`compose`](crate::compose) over [`PRESETS`](crate::PRESETS) at several
//! seeds, the melody track picked out by [`Role::Melody`](crate::Role)'s colour, and the signed
//! difference between consecutive pitches within each clip. Nothing else is needed and nothing here
//! depends on a fixture: the interval distribution of a piece is a property of the piece.
