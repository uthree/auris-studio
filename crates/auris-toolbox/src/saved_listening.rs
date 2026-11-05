//! Saved-project `listen` keeps opaque audio baselines between calls that each open the file anew.

use crate::*;
use auris_session::audio_audition::{Audition, ListeningContext};
use std::sync::{Mutex, OnceLock};

// Two projects, each with the session's four bounded 30-second excerpts, stay below 128 MiB.
static CONTEXTS: OnceLock<Mutex<Vec<(PathBuf, ListeningContext)>>> = OnceLock::new();

/// Renders the saved project's bars and answers the whole [`Audition`] as JSON, clips included;
/// the MCP door turns it into a text block and audio blocks.
pub(crate) fn run(
    session: &mut Session,
    start_bar: u32,
    bars: u32,
    compare_to: Option<&str>,
) -> Result<String, String> {
    let audition = listen(session, start_bar, bars, compare_to)?;
    serde_json::to_string(&audition).map_err(|e| e.to_string())
}

fn listen(
    session: &mut Session,
    start_bar: u32,
    bars: u32,
    compare_to: Option<&str>,
) -> Result<Audition, String> {
    let key = session
        .path()
        .ok_or("Missing saved project path")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let cache = CONTEXTS.get_or_init(Default::default);
    let mut context = {
        let mut contexts = cache.lock().map_err(|_| "Listening context lock failed")?;
        contexts
            .iter()
            .position(|(path, _)| path == &key)
            .map(|index| contexts.remove(index).1)
            .unwrap_or_default()
    };
    session.swap_listening_context(&mut context);
    let result = (|| {
        let job = session.begin_listening(start_bar, bars, compare_to)?;
        let result = job.run(crate::cancellation::current().flag())?;
        session.continue_listening(result)
    })();
    session.swap_listening_context(&mut context);
    let mut contexts = cache.lock().map_err(|_| "Listening context lock failed")?;
    contexts.retain(|(path, _)| path != &key);
    if contexts.len() >= 2 {
        contexts.remove(0);
    }
    contexts.push((key, context));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_saved_calls_compare_opaque_baselines_without_saving() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Song.auris");
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(3840))
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks(997))])
            .unwrap();
        session.save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let first = listen(&mut session, 1, 1, None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(session);
        // A second call opens the file anew, as every MCP call does.
        let mut session = crate::opened(path.to_str().unwrap()).unwrap();
        session.set_param(auris_session::ParamTarget::TrackGain(track), -12.0);
        session.save_with_checkpoint().unwrap();
        let edited = std::fs::read(&path).unwrap();
        let second = listen(&mut session, 1, 1, first.report["audition_id"].as_str()).unwrap();
        assert_eq!(second.report["compare_to"], first.report["audition_id"]);
        assert_eq!(std::fs::read(&path).unwrap(), edited);
        assert_eq!(second.clips.len(), 2);
        assert_eq!(second.clips[0].wav_base64, first.clips[0].wav_base64);
        assert_ne!(second.clips[0].wav_base64, second.clips[1].wav_base64);
    }
}
