//! Shared vocal-song guidance and the phase-specific composition instructions.

use auris_session::production_record::{Phase, WritingOrder};

/// Default scope and completion checks for new vocal songs.
pub const VOCAL_SONG_FORM: &str = "\nNew vocal songs include verse 2 and later choruses/ending unless the user limits the scope. Honor duration/bar limits, one-chorus demos, excerpts, lyric-only work and local edits. Plan the whole form before notes; write section ranges with edit_harmony and track progress in production_record when available. A first-verse sketch is intermediate. Give verse 2 new lyrics developing the story, recognizable melody and deliberate backing/phrasing changes. Use separate clips for different words; looping verse 1 is not verse 2. Chorus lyrics may repeat; a bridge or third verse is optional. In .asong use distinct sections/lyrics and melody_from for melodic reuse; diagnose_song checks fit. Complete notes, lyrics, the song-wide lyrics annotation (edit_lyrics) and backing in every planned sung section. Before finishing, compare inspect_project with the plan and read later sections' notes/lyrics. Where available check inspect_singer section_coverage, following next_section_offset. Labels, counts or a current take alone do not prove completeness; instrumental sections may be empty. Synthesize the whole score when requested. If blocked, preserve it and name the unfinished sections/synthesis.\n";

// Each phase is a paragraph of what to do, its guidance, then a technique index: names only,
// each a heading of docs/composition-techniques.md, which search_documentation returns. The
// index is there so a model knows the move exists; the page is there so the prompt need not
// carry how to make it.

const SCORE_BEFORE_ORDER: &str = "Score phase: decide what the song is. ";

const SCORE: &str = "Record purpose, length, mood and constraints; distinguish stated requirements from assumptions. When inspect_project reports the title \"Untitled\", give a new song a short title in the user's language with rename_project. Set tempo/meter with edit_timeline, then named sections covering the whole song, including an intro and an outro of at least eight bars each, and the key/chords of each with edit_harmony; audible chord notes alone do not populate those maps. Before writing notes, plan each section's lines and record the plan with production_record: what each line does (state, answer, turn, land), where it enters (a pickup, the downbeat or late), what it ends on against the chord, and where the hook repeats exactly. For a vocal song write lyrics in natural word groups and check each line against the notes it will sing with analyze_lyrics fit: the same count where a hook repeats exactly, its own count elsewhere, with the melody adapting by a pickup, a split or a merge. Develop one short foreground motif before expanding the form, and complete every planned section. Author the melody note by note with add_clip, read_notes and edit_notes, one section at a time: read each section back, with melody_shape, before writing the next, rather than sending the whole melody in one batch. Make every chorus lift: higher on average than the section before it, the song's highest notes, a hook that leaps. Read melody_shape for the lead and resolve its warnings before singing. Then run lint_project and resolve its warnings and errors: held notes outside the key and chord, a singer asked for two notes at once, sung notes without a lyric, duplicate notes, bars that sound chords with none written. apply_lint_fixes takes the obvious fixes by id; a finding without one needs your own edit. A warning you keep on purpose is recorded with its reason in production_record; info findings are optional. Add one plain guide part (piano or pad chords, generate_phrase is fine) on a track of its own so the score can be heard; it is a sketch the arrangement deletes, not a first draft of a part. Register roles with production_record set_roles: the foreground tracks, the guide tracks, or no_foreground with a reason for a piece without a melody. Choose the voice, write_lyrics and sing every sung section; confirm take_state current and the song-wide lyrics annotation. For a new song from scratch, end the turn at this demo and ask with offer_choices whether to arrange it or revise the melody, lyrics or chords, unless the user asked for the whole song in one go. Sound choice, backing and mixing belong to later phases.
Guidance
- Build the foreground from a short motif: two to six notes, a beat to two bars long, with a recognizable rhythm or contour, and a phrase destination against the actual harmony. A motif is not a whole lyric line. Repeat it exactly where the hook must be recognised; elsewhere keep its head and change its tail (rhythm, last interval, register or the chord under it). Repetition and repeated pitches can carry a hook and complexity is not a quality target, but a section whose lines all sing the same notes is a loop, not a melody.
- Give a section's lines different jobs. Four lines can state, answer with a change, turn (new rhythm, register or harmony) and land, as in 起承転結; a pre-chorus can change the phrase length; a chorus can state the hook, answer it, reach its peak and land.
- Vary entries and note values. Choose per section between a pickup from the bar before (弱起), the downbeat and a late entry rather than starting every line on beat 1. Use an eighth-note pulse as a pop melody's backbone, sixteenths chosen for pickups, turns and answers, and longer values for arrivals; at a fast tempo let one section speak in eighths while another holds. Anticipate a strong beat (食い) with a key word or the chorus's first word where the drive helps. Do not give every syllable of a line the same value.
- Plan line endings against the actual chord. Pair open endings (degree 2, 5 or 7, or a non-tonic chord) with closed ones (degree 1 or 3 over the tonic), and land the last chorus on the tonic unless the brief wants it open. Write falling and arched lines as well as rising ones.
- Write each chorus as the song's arrival. Keep the verse and pre-chorus (Bメロ) lower and narrower so the chorus has somewhere to go: lift it a minor third to a fifth above the pre-chorus on average, give it the song's highest notes, and let its hook leap (a fifth, a sixth, an octave, even an octave jumped back and forth) where the pre-chorus moved by step, at the moment that matters rather than at the head of every line. Prepare the entrance with a held note, a rest or a rising line, then land the chorus on a strong beat or just before it. Stay inside the voice's useful range; when the chorus cannot go higher, set the verse lower instead. A subdued ballad chorus may lift less, but by choice, not by default.
- Give sections their own harmony. A verse may loop a simple progression, but give the pre-chorus and the chorus their own progression or harmonic rhythm rather than the verse's loop started on another chord.
- Japanese is counted in moras. Where a hook repeats exactly, every line sung to it must fit its notes exactly, because 字余り and 字足らず are heard at once: drop or add a particle (君を待つ/君待つ), use a word of another length, or hold a long vowel (ー, さあ) on one note. Elsewhere let line lengths differ and let the melody adapt, adding or dropping a pickup, splitting a long note or merging two, rather than writing every line to one count; lines all in 7-5 meter on one even note per mora sound like a school song (唱歌). っ is the rest after its note; ん and every other mora take a note, and a held long vowel (ー) may carry a melisma across two pitches. Never squeeze two sung moras onto one note. Preserve fixed words, authored notes and a liked attack pattern.
- New vocal songs include a second verse with new words that develop the story, and later choruses and an ending, unless the user limits the scope; a loop of verse 1 is not verse 2. Use separate clips for different words.
- For edit_harmony, group chords by bar, e.g. chords:[\"I\",\"IV V\",\"vi\",\"ii V\"]. Each entry is a bar, not a chord; two chords in a 4/4 entry start on beats 1 and 3. Bare multi-chord strings are ambiguous.
- edit_notes takes top-level expected_revision and input {\"kind\":\"inline\",\"edits\":[...]} with typed patch/replace edits; a new phrase needs add_clip, then read_notes, then edit_notes. Use time-order views, retaining storage indices for patches and write_lyrics from.
- Keep original kanji/kana in write_lyrics text and give the kana pronunciation separately in reading. Before writing a line onto its notes, call analyze_lyrics with fit set to that phrase's note count; when it fits, pass its note_reading as the write_lyrics reading (| separates notes; a group ending in っ needs a rest after its note). When analyze_lyrics reports dictionary_available false, spell long vowels as sung with ー (りょーて, せんせー). With edit_notes set original lyrics and explicit phonemes together. Singer clip names follow written lyrics; do not rename them.
- list_voices discovers installed voices; set_singer_voice selects one, sing makes playback, and inspect_singer verifies current takes and every sung section (follow coverage pages). Unavailable voices, stale takes and failed synthesis remain unresolved; do not truncate or resubmit unresolved inference.
Techniques (search_documentation the name for detail): Section roles; Line roles (起承転結); Entrances and anticipation (弱起・食い); Cadence plan; Section harmony; Chorus lift; Motif repetition and development; The chorus hook in the intro; Drop chorus (落ちサビ); Last-chorus key change (ラスサビ転調); Singable range.";

const ARRANGEMENT: &str = "Arrangement phase: dress the fixed score. Keep the foreground tracks' notes, lyrics and timing, the harmony, sections, meter and tempo map: the host withdraws an edit that changes them and says what it changed. To change them, enter_phase score with the user's words as the reason. Coming back from the score, the record's phase.review lists the bars that changed and the tracks that play over them: fix those tracks there before anything else. Read the foreground and harmony first. Before writing, plan the energy of the song section by section and record it with production_record: which parts play in each section and how busy they are, verses thinner than choruses, a bridge or drop chorus stripped back, the last chorus fullest. Choose each track's sound for its role with search_instruments, then write the parts around the foreground: bass, harmony, pulse and colour in complementary registers. Use accompaniment or generate_phrase for plain chords, pads, simple arpeggios and basic drum pulses; author counterlines, distinctive bass, fills and backing vocals note by note. Copy a finished pattern with edit_clip and vary the repeat rather than rewriting it. Notes you type play exactly on the grid until a performance stage moves them, so give every part you write, the foreground melody included, a starting feel: list performance_presets with its clip, apply its suggested_preset or an entry that suits the genre better, then tune it with set_performance without rewriting notes. Leave a part machine-tight only when the style wants it, such as chiptune or a drum machine. Shape entrances, exits, transitions into each section and the ending. Delete the guide track before you write the harmony part, and write that part afresh on a new track, in this song's groove and for its sections; do not cut the guide into section clips and keep it. A guide stays registered in the roles until its track is deleted or silent, so setting roles again without it does not replace it. A rough fader balance is fine here; inserts, sends, sidechains and normalize_mix belong to the mix phase. Read back the parts, read arrangement_shape and resolve its warnings, then run lint_project and resolve the warnings in the parts you wrote: crowded onsets, bars too dense to hear, held or muddy low seconds, notes outside the key and chord, notes too soft to hear. A finding on a foreground track or in the harmony is a score change, so its fix is withdrawn here: enter_phase score to make it, or record why it stays. Then inspect_audio representative sections, transitions and the ending.
Guidance
- Make sections differ for a reason: register, texture, rhythm, harmony or instrument handoff. Support the foreground with complementary registers and timing, and leave space for the actual instrument and effect decay at the ending.
- Write the backing section by section: one clip per section on each track, the first written by hand and later ones copied with edit_clip and varied, never one clip running the whole song. Each chorus must be fuller than the section before it, with another part or busier playing.
- Mark the way into each chorus and each new section with a drum fill in the bar before it and a crash on its first beat, and change the hats, ride or kick pattern between verse and chorus. A drum part that plays one bar for the whole song does not follow the form. Give each section its own main groove rather than reusing the verse's bar for the intro, interludes and outro with only the cymbals changed: vary the kick and hats, use half time or a two-beat feel where it fits, and scale fills with the phrase (small every four bars, larger at eight, the largest into a chorus).
- Strum guitar chords. Apply a guitar performance preset before tuning it, and keep the strum spread at the preset's value or wider: under about 15 ms across a chord the strings are heard as one hit, which no hand plays. string_ms sets the time per string, so a six-string chord takes longer than a power chord; keep variation above zero so no two strokes are identical. A sampled note sounds as long as it is written: damp strummed chords at their changes, write palm mutes short and soft, and choke cymbals.
- Give the song an intro of at least eight bars that sets up the groove and the hook before the voice, and an outro of at least eight bars that resolves rather than stops.
- Search sounds with search_instruments request {\"mode\":\"text\",\"query\":\"piano\"} and copy sound_id exactly. For timbre impressions use mode character with a query such as \"ふんわり\" or 0..1 targets; translate prose and negation into targets. Preserve chosen sounds unless asked to change them.
- Read inspect_performance before set_performance and verify the performed result after. Swing is the song's feel, not seasoning: a straight song stays at swing 50, and a swung song swings every part alike, vocal included.
- Discover instrument parameter keys and units before setting them; hosted VST3 values are normalized 0..1, CLAP values use native ranges.
Techniques (search_documentation the name for detail): Part roles and registers; Voicings out of the voice; Kick and bass together; Fills and crashes at transitions; Drum groove by section; Build-up and break; More in verse 2; Copy, then vary; Performance controls; Strummed chords; Generators for scaffolding, authored notes for what is remembered.";

const MIX: &str = "Mix phase: balance the finished arrangement. Keep every note, sound and performance, and the score: the host withdraws an edit that changes them and says what it changed. To change them, enter_phase arrangement (or score) with a reason. Read mixer_graph and set levels and panning for foreground intelligibility before adding processing. Use EQ, dynamics, reverb/delay sends, sidechains and automation each for a stated purpose; discover supported parameters first. Set explicit loudness and true-peak goals (-14 LUFS and -1 dBTP unless the user names others), finish with normalize_mix after the last sound or fader change and read what it measured. Then run lint_project with audio true and resolve its errors and warnings: clipping, a mix far too loud or quiet, a track that plays but makes no sound, an instrument note off its pitch, stretches that cancel in mono. Changing a sound or a note to fix one is an arrangement change. On a sung track, pitch_mismatch and unpitched describe the synthesized take, which note edits do not fix: report them rather than chase them. Run it again after a fix, until each remaining finding is fixed or recorded with its reason. Measure representative sections, transitions and the ending of the full mix after the last edit; audition the loop wrap when relevant. For requested exports use render/export_midi with new destinations, in the project workspace (renders/mix.wav) unless the user names an absolute location, and verify_render; a failing verdict is unfinished delivery. Report delivered paths, measurements and what still needs human listening.
Guidance
- Balance foreground, bass, pulse and harmony with the actual sounds before detailed effects. More notes, layers, processing or loudness do not establish improvement.
- Read mixer_graph before editing and copy stable node and parameter IDs. Keep routing acyclic. Hosted VST3 values are normalized 0..1, CLAP values use native ranges; master normalization does not replace track balancing.
- inspect_audio renders local measurements, not listening. Use bounded ranges during edits and the full current mix after the last change. Verification mutes, solos and bypasses belong in an isolated snapshot or an advertised non-persistent override.
- For exports read render, export_midi and verify_render help, choose an explicit range and tail and new destinations (workspace-relative unless the user names a location), and inspect PASS/FAIL and the actual files.
Techniques (search_documentation the name for detail): Volume and pan first; Low cut on everything but the bass; Kick-triggered sidechain; Reverb and delay sends for the voice; Bus processing; Section automation; normalize_mix last.";

/// What one phase asks of the agent, given the score phase's starting point.
///
/// Returned with the production record after `enter_phase`, so a model only carries the
/// phase it is in. The rules that hold in every phase stay in the system prompt.
pub fn instructions(phase: Phase, order: Option<WritingOrder>) -> String {
    let order = match order {
        Some(WritingOrder::HarmonyFirst) => {
            "Establish harmonic direction, then shape a memorable melody and supporting rhythm. "
        }
        Some(WritingOrder::MelodyFirst) => {
            "Establish the foreground motif and phrase arc first; derive compatible harmony and supporting rhythm from its actual notes. "
        }
        Some(WritingOrder::RhythmFirst) => {
            "Establish the groove and phrase accents first; fit motif and harmonic pacing to that rhythmic identity. "
        }
        None => "",
    };
    match phase {
        Phase::Score => format!("{SCORE_BEFORE_ORDER}{order}{SCORE}"),
        Phase::Arrangement => ARRANGEMENT.into(),
        Phase::Mix => MIX.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TECHNIQUES: &str = include_str!("../../../docs/composition-techniques.md");
    const INDEX: &str = "Techniques (search_documentation the name for detail): ";

    #[test]
    fn every_indexed_technique_is_a_heading_search_can_find() {
        let headings: Vec<&str> = TECHNIQUES
            .lines()
            .filter_map(|line| line.strip_prefix("### "))
            .collect();
        for phase in [Phase::Score, Phase::Arrangement, Phase::Mix] {
            let text = instructions(phase, None);
            let index = text
                .lines()
                .find_map(|line| line.strip_prefix(INDEX))
                .unwrap_or_else(|| panic!("{phase:?} has no technique index"));
            let names: Vec<&str> = index.trim_end_matches('.').split("; ").collect();
            assert!(names.len() >= 5, "{phase:?}: {names:?}");
            for name in names {
                assert!(
                    headings.contains(&name),
                    "{phase:?} names {name:?}, not a heading"
                );
            }
        }
    }

    #[test]
    fn each_phase_carries_the_guidance_that_left_the_system_prompt() {
        let score = instructions(Phase::Score, Some(WritingOrder::MelodyFirst));
        for needle in [
            "melody_shape",
            "字余り",
            "note_reading",
            "edit_harmony",
            "inspect_singer",
            "lint_project",
            // The rhythm and form guidance that a single copied phrase template ignored.
            "起承転結",
            "弱起",
            "食い",
            "one section at a time",
            "own count elsewhere",
        ] {
            assert!(score.contains(needle), "score lacks {needle}");
        }
        let arrangement = instructions(Phase::Arrangement, None);
        for needle in [
            "search_instruments",
            "swing 50",
            "complementary registers",
            "arrangement_shape",
            "eight bars",
            "fill",
            "Delete the guide track",
            "lint_project",
            "own main groove",
            "string_ms",
            "15 ms",
        ] {
            assert!(arrangement.contains(needle), "arrangement lacks {needle}");
        }
        let mix = instructions(Phase::Mix, None);
        for needle in [
            "mixer_graph",
            "normalize_mix",
            "verify_render",
            "VST3",
            "lint_project with audio true",
        ] {
            assert!(mix.contains(needle), "mix lacks {needle}");
        }
    }
}
