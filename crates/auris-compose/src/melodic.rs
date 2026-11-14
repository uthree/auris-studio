//! Why the tune is shaped the way it is, and how that was measured.
//!
//! Nothing here is code. It is the account of one question — *why do the composer's melodies sound
//! unnatural when its accompaniment does not* — and of the answer, because the answer is a set of
//! numbers that the next person to touch `parts::melody` will otherwise
//! have to find again. The constants in that module are what this page argues for; read them
//! together or neither will make sense.
//!
//! # Rhythm and the hook
//!
//! Listening feedback on the September 2026 phrase changes preferred the accompaniment's
//! contrast but still found the main melody hard to follow. Saved-score inspection exposed
//! a separate problem from the pitch intervals discussed below: independently sampled attacks
//! could cluster at a bar's edges, and reusing that bar repeated the scattered rhythm. A last
//! note on the final subdivision also left no room for the intended phrase breath.
//!
//! Automatic instrumental melodies draw two-beat gestures, then place a busy approach's
//! target against the next gesture in the whole bar. An early anticipation followed by a
//! pickup could otherwise spend two beats holding and resting immediately after a pair of
//! short notes, repeating that interruption in every bar. When a busy approach lands more
//! than a felt beat before the next gesture, its existing attacks are spread towards that
//! answer on the meter's hierarchy. Its note count, contour degrees and following pickup
//! rest stay intact; no extra random draw changes the piece's pitch germ. Sparse holds and
//! sustained palettes can still remain still, and late held arrivals need no redistribution.
//!
//! Chiptune gestures reach their faster approaches earlier on the density dial, with at most
//! four subdivisions per beat. The held arrival and phrase breath remain intact. Other
//! palettes retain their density mapping.
//!
//! Rhythmic variations preserve the opening cells and develop the tail. A closing bar leaves a final
//! felt beat of silence when the meter has room. If its answer would fall back to an early
//! preparation, it instead writes the final degree at a later arrival; an anticipated arrival
//! already near the final two beats is retained. Explicit rhythms keep their authored onsets.
//! Timing can still affect realized pitches through the existing harmonic and metric rules.
//!
//! These rules make the intended rhythm observable; they do not prove catchiness. The
//! `tools/eval/melody_ab.py` probe transplants the new melody into the same saved backing and
//! mixer so listening comparisons isolate it. `docs/evaluation.md` describes the reproducible
//! comparison workflow. The tables below describe earlier pitch-focused experiments, not the
//! results of these rhythm changes.
//!
//! # A call and its response
//!
//! A four-bar phrase first states its figure — a bar that calls and the bar that answers it,
//! see the fifth pass below — then develops the figure's final interval gesture, then recalls
//! its head while approaching a reachable answer. The response carries the degree reached by
//! the preceding bar. A descending call can therefore return upward rather than restarting its
//! descent in every bar. The excursion is bounded by the figure's own span, and the approach
//! reserves enough movement for its final arrival. Repeated degrees remain valid. A phrase
//! shorter than four bars states the call alone; longer phrases keep their existing shared
//! breath boundary. A given motif is one bar long and each of the two bars says it whole, so
//! there the second bar is an echo and the development reads the call by itself.
//!
//! This plan changes contour degrees after rhythmic slots and the closing breath are allotted.
//! The chord-scale, register join and prepared-tension passes then realize those degrees as
//! pitches. An answer's landing belongs to the chord sounding there; it is not forced onto the
//! key tonic. The shared skeleton and the other writers retain their existing responsibilities.
//!
//! A separate timing pass gives the second, nonclosing bar a delayed entry or an anticipated
//! late arrival. It keeps note count, ordered pitches, velocities and gesture endpoints, and
//! stays within the original harmonic event while weakening the moved onset's metric position.
//! A named random stream selects the recurring response without changing pitch draws. Written
//! rhythms and sustained palettes keep their timing; shorter meters keep their late holds.
//!
//! The four-condition probe in `tools/eval/melody_phrase_ab.py` compares the previous writer,
//! pitch development, timing development and both together over the same saved accompaniment.
//! Component controls are checked before audio rendering. A pitch change can also alter the
//! finalizer's mandatory shortening at a same-pitch restrike after swing; the probe verifies and
//! reports this articulation coupling. These are structural decisions, not a score of catchiness.
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
//! # The fifth pass: the figure is two bars
//!
//! Every pass above was about pitch, and the line still sounded like a machine. What was left
//! was not in the intervals at all. A section drew **one bar** of rhythm and one bar of
//! contour and said them in every bar: one sixteen-bar take over `I V vi IV` wrote
//! `.x--x-x-..xxx---` in eight of its bars and a cut-down copy of it in the other eight, the
//! same six-note shape climbing the scale under it. An ear does not hear that as a hook. It
//! hears a loop.
//!
//! Measured over 2,160 sixteen-bar takes — six charts, five styles, three densities and
//! twenty-four seeds — against the sung parts of the PDMX pop and rock scores that carry
//! lyrics: 181 songs in 4/4, cut into 607 sixteen-bar windows, everything quantised to a
//! sixteenth. A *line* is what lies between two rests of a beat or more.
//!
//! | | before | after | corpus |
//! | --- | --- | --- | --- |
//! | the commonest bar rhythm, as a share of the bars | 54% | 31% | 26% |
//! | a bar with the onsets of the bar before it | 26% | 1% | 9% |
//! | … of the bar two before | 33% | 22% | 19% |
//! | … of the bar four before | **80%** | 22% | 23% |
//! | … of the bar eight before | 81% | 37% | 20% |
//! | contour correlation with the bar four before | 0.76 | 0.27 | 0.31 |
//! | notes held across a bar line, in sixteen bars | **0** | 1.4 | 3.2 |
//! | lines entering in the second half of a bar | 3% | 30% | 37% |
//! | lines entering at the commonest point | 97% | 68% | 63% |
//! | change in note count from a bar to the next | 1.1 | 2.7 | 2.1 |
//! | the longest note in sixteen bars, in beats | 1.4 | 2.3 | 3.3 |
//! | notes of two beats or more | 2% | 7% | 9% |
//!
//! The corpus has no period at all: a bar resembles the bars one, two, four and eight before it
//! about equally, a fifth of the time exactly. What it does not do is what the composer did —
//! return to one bar four times in five. Five changes in `parts::melody` and its two helpers:
//!
//! 1. **The figure is a call and the bar that answers it.** Each has a rhythm of its own, and
//!    the germ is one line drawn across both, so the second bar carries on from where the
//!    first got to instead of starting again. The answer is drawn a subdivision calmer and
//!    more often than not comes to rest on a note held through its last two beats, which is
//!    also where the tune's long notes come from. A phrase plays call, answer, call varied,
//!    answer closed.
//! 2. **A variation may turn the rhythm three times in four.** The rhythmic operation and the
//!    inflection of the arrival were one draw, and half its outcomes were an inflection and
//!    nothing else — so half of every phrase's third bars came back as its first exactly.
//! 3. **A continuation leads with the answer.** The same two bars the other way round: no new
//!    material, and the phrase still does not open the way the one before it did.
//! 4. **A phrase is let go of in one of three ways.** A beat on the arrival where the line
//!    means to go on, two where it has got where it was going, and a beat and a half let go
//!    early where a pickup follows. It was a beat of note and a beat of rest every time.
//! 5. **The line may cross a bar line.** An answering bar can enter an offbeat ahead of its
//!    downbeat and hold across it — a choice made once for a section, likelier the more
//!    syncopated the part — and a phrase after the first can be led into by one note in the
//!    breath before it, a scale step from where it is going.
//!
//! The first of these alone was not enough, and measuring it on its own said so: a bar stopped
//! resembling its neighbour and matched the bar *two* before it nearly half the time, with the
//! four-bar figure untouched at 81%. The repetition had only moved up a level. A written
//! rhythm keeps one bar and every attack where it was typed; none of the five reaches it
//! beyond the two-bar contour.
//!
//! Two tests in `parts::melody` hold the table: `a_section_is_not_one_bar_said_sixteen_times`
//! and `the_line_crosses_bar_lines_and_some_phrases_are_led_into`.
//!
//! # The sixth pass: which corpus
//!
//! The first four passes held the line to 68 steps and 11 repeated notes in a hundred. Those
//! are Palestrina's figures, and they were met: the composer wrote a tune that walked up and
//! down a scale and seldom said a note twice, which is well formed and does not sound sung. The
//! fifth pass measured the pitches against sung pop while it was there, and the same takes and
//! the same 607 windows say this:
//!
//! | | before | after | sung pop |
//! | --- | --- | --- | --- |
//! | repeated note | 11% | 26% | 29% |
//! | step (1–2 semitones) | 59% | 45% | 42% |
//! | third (3–4) | 21% | 19% | 17% |
//! | fourth or wider | 9% | 10% | 12% |
//! | a repeat after a repeat | 14% | 34% | 48% |
//! | a repeat after a move | 11% | 24% | 21% |
//! | a step up carried on up | 36% | 26% | 27% |
//! | a step down carried on down | 39% | 30% | 33% |
//! | a leap carried on the same way | **41%** | 23% | 17% |
//! | a leap followed by its own landed note | 8% | 27% | 28% |
//! | notes inside a stepwise run of four or more | **24%** | 10% | 12% |
//! | entering after a rest by a third or wider | 22% | 47% | 57% |
//! | span of sixteen bars, in semitones | 19.2 | 15.6 | 13.7 |
//! | span of four bars | 11.6 | 10.1 | 8.8 |
//!
//! POP909's melody tracks agree with the first two rows at 22% and 47%, so this is not one
//! corpus's habit. Three things in it are not about how often an interval is drawn at all, and
//! no table of weights could have reached them:
//!
//! * **A sung line recites.** Its repeated notes come in runs — a repeat is more than twice as
//!   likely after a repeat as after a move — where the composer sprinkled its few evenly.
//! * **Step inertia belongs to falling lines only.** A step down is followed by another more
//!   often than not; a step *up* is followed by a turn slightly more often than by another.
//!   The walk applied it both ways at 85 in a hundred, and that is where the scale runs came
//!   from.
//! * **A leap is not carried on.** The note after one turns back, or says the landed note
//!   again. The walk has always turned its own leaps round, but most of the leaps on the page
//!   are made after the walk, by the chord a strong beat is moved onto and by the join between
//!   two bars.
//!
//! What changed, in `parts::melody` unless it says otherwise:
//!
//! 1. **The table is a singer's.** A repeated note is 23 draws in a hundred rather than 11,
//!    and after one a move is turned into another a third of the time.
//! 2. **Inertia only falls**, and at the strength it is found at.
//! 3. **A repeat reaches the page.** A figure no busier than the germ takes the germ's own
//!    notes rather than a line interpolated between them, which had been blurring every
//!    repeated note into a slope; and a note the figure says twice is said again at the pitch
//!    that was *played*, which a strong beat may have moved. With the table at 23 in a hundred
//!    and recitation on top of it, 17 notes in a hundred had been coming out repeated.
//! 4. **A leap is answered where it is made.** A weak note that would carry a leap of a fourth
//!    or more on the way it went says the landed note again instead.
//! 5. **A bar line is joined only when it needs to be.** A bar that would enter within a
//!    fourth of where the last one left off is left where its anchor puts it, and a repeat
//!    ranks with a third rather than below a fourth. Joining every bar line left a fourth or
//!    wider at six crossings in a hundred, where a sung line has twenty: it moves *more* across
//!    a bar line than inside one.
//! 6. **A phrase falls, and the next one starts again.** The answer aims a third under the
//!    degree the call began on, and a phrase after the first starts from its own anchor rather
//!    than from the note the last was let go of on.
//! 7. **The arch is a singer's.** The structural pitches in `frame` arch over three tenths of
//!    the range rather than half of it, and a take sits a little above or below the middle so
//!    that two takes of a chart are not hung on the same pitches.
//!
//! One thing was tried and taken out. A continuation was made to follow the call's own closing
//! gesture, sign and all, instead of pointing every move at where it has to arrive; it moved
//! the continuation's share of scale runs from 18.5% to 17.5% and was not worth its code.
//!
//! `the_tune_still_measures_like_a_tune` and `the_tune_moves_the_way_a_sung_line_does` hold
//! the table, and the first of them now guards both sides: the arpeggio the line was before the
//! first pass, and the scale it was before this one.
//!
//! # The seventh pass: note values
//!
//! The fifth pass and the sixth chose their own measurements, so they were checked against
//! one they had not: the corpus's own long-term model (`tools/eval/melody_corpus.py`), a PPM
//! model over each note's interval and its distance from the note before, trained on the 620
//! PDMX lead sheets. Its own songs read at 5.57 bits a note, give or take 1.65. The composer
//! read at 6.89 before the fifth pass and at **7.16** after the sixth. Split in two, the
//! intervals had come nearer and the distances had gone further away: the two passes had
//! mended what repeats and how the line moves, and nothing had ever asked what a note is
//! *worth*.
//!
//! | | before | after | sung pop |
//! | --- | --- | --- | --- |
//! | a note a sixteenth after the one before | 22% | 13% | 12% |
//! | … an eighth after | 34% | 52% | 48% |
//! | … a quarter after | 22% | 20% | 20% |
//! | … a dotted quarter after | 7.6% | 5.4% | 4.8% |
//! | bars with a note off the eighth | 47% | 23% | 22% |
//! | half-bars holding three notes a sixteenth apart | **19%** | 11% | 4% |
//! | half-bars of four straight eighths | 1.5% | 9.7% | **9.7%** |
//! | half-bars with no note starting in them | 5% | 11% | 19% |
//! | bits a note under the corpus's model | 7.16 | 6.45 | 5.57 ± 1.65 |
//!
//! Every gesture was a run of short notes into a held one inside two beats, and the short
//! notes were as short as arithmetic made them: an approach half a beat long, split in two as
//! any ordinary density asked, is a pair of sixteenths. So one half-bar in five held three
//! notes a sixteenth apart, which a singer sings in one in twenty-five, and the commonest thing
//! a singer does sing after a held note — four eighths straight through half a bar — was
//! something the gestures could not write. In `parts::melody`'s rhythm helper:
//!
//! 1. **A gesture is written on the offbeat** unless it is drawn fine, and how often it is
//!    drawn fine belongs to the density. A fine gesture is on the grid's own step and fits one
//!    more note into a run. The syncopation that pushed a note late pushes it by the gesture's
//!    own unit, where it used to push a step and write a sixteenth inside a run of eighths.
//! 2. **A gesture may run**: eighths straight through, arriving on whatever the next gesture
//!    opens with. Never the last gesture of a bar, which is where the bar's held note is.
//! 3. **Inside the bar a pickup carries on** into the gesture that follows, and **an
//!    anticipation is late** — an offbeat ahead of the next gesture rather than of the second
//!    beat. Both keep their old shape in a bar's last gesture, where they come to rest.
//! 4. **An answer may rest on the note it is already holding.** Half the answers that come to
//!    rest strike nothing in their last two beats, which is where a note of three beats comes
//!    from and why a half-bar may now be empty.
//!
//! The density dial had a dead stretch that this closes: 0.3 and 0.5 used to write the same
//! rhythm, take for take, because a note count rounded to the same number of subdivisions over
//! a third of the dial. They now differ in how often a gesture is fine — 2 notes in a hundred a
//! sixteenth after the last at 0.3, 9 at 0.5 and 24 at 0.7.
//!
//! `a_gesture_is_written_in_offbeats_until_the_density_asks_for_finer` and
//! `note_values_are_a_singers_rather_than_an_arpeggiators`, beside the gestures, hold this.
//!
//! # What is still wrong
//!
//! One thing is known and not fixed from the early passes: resolving a dissonance onto a chord
//! tone sometimes lands on the note beside it, which is refused where it would stutter but not
//! otherwise.
//!
//! The line is still fuller than a singer's. A half-bar with nothing starting in it is 11 in
//! a hundred against 19, a note twelve sixteenths or more after the last is 1.4 against 5.4,
//! and one bar in twelve of a sung line is empty altogether where no bar of the composer's is.
//! It has almost no dotted eighths — 1 note in a hundred three sixteenths after the last,
//! against 4 — its sixteenths still come three in a row where a singer's come in pairs, and
//! two eighths into a held note on the second beat is still six times as common as it should
//! be. Under the corpus's model it reads half a standard deviation above
//! the corpus's own songs, which is inside the spread and not at its middle.
//!
//! Against sung pop, a bar line is still crossed too smoothly — by a fourth or wider 11 times
//! in a hundred where a singer does it 20 — and a continuation, the third bar of a phrase, is
//! still the most scale-like bar there is, with 17% of its notes in a stepwise run and one
//! move in twenty a leap. Sixteen bars span two semitones more than a singer's. And a bar
//! resembles the bar two before it in rhythm but not in contour: the correlation is −0.11
//! where the corpus has 0.23.
//!
//! The corpus is sung. A lead that is not a voice may want fewer repeated notes than a line
//! with a syllable on every one of them, and nothing here measures an instrumental line against
//! instrumental lines.
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
//! The figures above were taken over the whole-song presets the crate used to ship. What holds
//! them now is the melody writer's own test `the_tune_still_measures_like_a_tune`: eight-bar takes
//! over six charts in the presets' keys, moods and styles, at four seeds each, and the signed
//! difference between consecutive pitches. Its bands sit between the figures reported here and the
//! ones reported for the writer before these rules existed.
