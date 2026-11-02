//! Shared vocal-song guidance and opt-in phase-specific composition instructions.

use auris_session::production_record::{Phase, WritingOrder};

/// Default scope and completion checks for new vocal songs, shared by every prompt profile.
pub const VOCAL_SONG_FORM: &str = "\nNew vocal songs include verse 2 and later choruses/ending unless the user limits the scope. Honor duration/bar limits, one-chorus demos, excerpts, lyric-only work and local edits. Plan the whole form before notes; write section ranges with edit_harmony and track progress in production_record when available. A first-verse sketch is intermediate. Give verse 2 new lyrics developing the story, recognizable melody and deliberate backing/phrasing changes. Use separate clips for different words; looping verse 1 is not verse 2. Chorus lyrics may repeat; a bridge or third verse is optional. In .asong use distinct sections/lyrics and melody_from for melodic reuse; diagnose_song checks fit. Complete notes, lyrics, the song-wide lyrics annotation (edit_lyrics) and backing in every planned sung section. Before finishing, compare inspect_project with the plan and read later sections' notes/lyrics. Where available check inspect_singer section_coverage, following next_section_offset. Labels, counts or a current take alone do not prove completeness; instrumental sections may be empty. Synthesize the whole score when requested. If blocked, preserve it and name the unfinished sections/synthesis.\n";

/// Opt-in only until controlled listening demonstrates acceptable composition quality.
pub fn enabled() -> bool {
    std::env::var("AURIS_COMPOSITION_WORKFLOWS").is_ok_and(|v| v == "1")
}

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
        Phase::Score => format!(
            "Score phase: decide what the song is. {order}Record purpose, length, mood and constraints; distinguish stated requirements from assumptions. Set tempo/meter with edit_timeline, then named sections covering the whole song and the key/chords of each with edit_harmony; audible chord notes alone do not populate those maps. For a vocal song write lyrics in natural word groups first, and check every line against its melody's note count with analyze_lyrics fit, the same count wherever a motif repeats; prefer changing words to changing the motif, and split or merge a note only sparingly. Develop one short foreground motif before expanding the form, and complete every planned section. Author the melody note by note with add_clip, read_notes and edit_notes; code may serialize or copy authored notes but must not calculate creative ones. Make every chorus lift: higher on average than the section before it, the song's highest notes, a hook that leaps. Read melody_shape for the lead and resolve its warnings before singing. Add one plain guide part (piano or pad chords, generate_phrase is fine) so the score can be heard, and register roles with production_record set_roles: the foreground tracks, the guide tracks, or no_foreground with a reason for a piece without a melody. Choose the voice, write_lyrics and sing every sung section; confirm take_state current and the song-wide lyrics annotation. For a new song from scratch, end the turn at this demo and ask with offer_choices whether to arrange it or revise the melody, lyrics or chords, unless the user asked for the whole song in one go. Sound choice, backing and mixing belong to later phases."
        ),
        Phase::Arrangement => "Arrangement phase: dress the fixed score. Keep the foreground tracks' notes, lyrics and timing, the harmony, sections and tempo map; to change them, enter_phase score with the user's words as the reason. Read the foreground and harmony first. Choose each track's sound for its role with search_instruments, then write the parts around the foreground: bass, harmony, pulse and colour in complementary registers. Use accompaniment or generate_phrase for plain chords, pads, simple arpeggios and basic drum pulses; author counterlines, distinctive bass, fills and backing vocals note by note. Copy a finished pattern with edit_clip and vary the repeat rather than rewriting it. Use set_performance for feel without rewriting notes. Shape entrances, exits, transitions into each section and the ending. Replace or remove the guide part. A rough fader balance is fine here; inserts, sends, sidechains and normalize_mix belong to the mix phase. Read back the parts and inspect_audio representative sections, transitions and the ending.".into(),
        Phase::Mix => "Mix phase: balance the finished arrangement. Keep every note, sound and performance; to change them, enter_phase arrangement (or score) with a reason. Read mixer_graph and set levels and panning for foreground intelligibility before adding processing. Use EQ, dynamics, reverb/delay sends, sidechains and automation each for a stated purpose; discover supported parameters first. Set explicit loudness and true-peak goals (-14 LUFS and -1 dBTP unless the user names others), finish with normalize_mix after the last sound or fader change and read what it measured. Measure representative sections, transitions and the ending of the full mix after the last edit; audition the loop wrap when relevant. For requested exports use render/export_midi with absolute new destinations and verify_render; a failing verdict is unfinished delivery. Report delivered paths, measurements and what still needs human listening.".into(),
    }
}
