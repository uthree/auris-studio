//! Singer interaction regressions using metadata-only VOICEVOX connections.

use super::*;
use crate::harness::open;
use gpui::TestAppContext;
use std::path::Path;

struct Scratch {
    root: PathBuf,
    parent: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = parent.join(format!(
            "auris-gpui-singer-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        assert!(root.starts_with(&parent) && root != parent);
        Self { root, parent }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn voice(&self, entry: &str) -> PathBuf {
        let path = self.join(entry);
        write_voice(&path, "Shared voice", 93.75, &["First", "Second"]);
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Resolve again before recursive cleanup: never follow a replaced directory outside
        // this test's unique, verified temporary root.
        if let Ok(resolved) = std::fs::canonicalize(&self.root)
            && resolved == self.root
            && resolved.starts_with(&self.parent)
            && resolved != self.parent
        {
            let _ = std::fs::remove_dir_all(resolved);
        }
    }
}

fn write_voice(path: &Path, name: &str, frame_rate: f64, speakers: &[&str]) {
    let styles: Vec<_> = speakers
        .iter()
        .enumerate()
        .map(|(index, name)| {
            serde_json::json!({
                "name": name,
                "query_style_id": 6000,
                "decode_style_id": 3001 + index,
            })
        })
        .collect();
    let config = serde_json::json!({
        "format_version": 1,
        "name": name,
        "url": "http://127.0.0.1:0",
        "sample_rate": 24000,
        "frame_rate": frame_rate,
        "styles": styles,
    });
    std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
}

fn add_singer(
    app: &mut AurisApp,
    name: &str,
    voice: &Path,
    notes: bool,
) -> (TrackId, Option<ClipId>) {
    let track = app.session.add_singer_track(name);
    app.session.set_singer_voice(track, Some(voice)).unwrap();
    let clip = notes.then(|| {
        let clip = app
            .session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        app.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        app.session.set_note_lyric(clip, 0, "あ").unwrap();
        clip
    });
    (track, clip)
}

fn settle_debounce(app: &mut AurisApp) {
    app.auto_sing_seen = (
        app.session.revision(),
        std::time::Instant::now() - crate::ui::commands::AUTO_SING_DEBOUNCE,
    );
}

#[gpui::test]
fn previews_distinguish_speakers_and_same_named_voice_files(cx: &mut TestAppContext) {
    let scratch = Scratch::new("preview-identity");
    let one = scratch.voice("one.voicevox.json");
    let two = scratch.voice("two.voicevox.json");
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        let (first, _) = add_singer(this, "First", &one, false);
        let (second, _) = add_singer(this, "Second", &one, false);
        let (other_file, _) = add_singer(this, "Other file", &two, false);
        this.session
            .set_singer_speaker(second, Some("Second"))
            .unwrap();

        assert!(this.wish_sung_preview(first, 60, None));
        let first_key = this.sung_preview_wish.as_ref().unwrap().key.clone();
        this.finish_sung_preview(
            first,
            first_key.clone(),
            this.sung_preview_generation,
            Ok((vec![0.1; 128], 24000.0)),
        );
        assert!(this.sung_previews.contains_key(&first_key));
        assert!(this.sung_preview_wish.is_none());

        for track in [second, other_file] {
            assert!(this.wish_sung_preview(track, 60, None));
            let wish = this
                .sung_preview_wish
                .as_ref()
                .expect("a different singer needs its own audio");
            assert_eq!(wish.track, track);
            assert_ne!(wish.key, first_key);
            assert!(!this.sung_previews.contains_key(&wish.key));
        }
        this.stop_audition();
    });
}

#[gpui::test]
fn choosing_the_current_voice_preserves_its_speaker_and_document(cx: &mut TestAppContext) {
    let scratch = Scratch::new("reselect-voice");
    let voice = scratch.voice("current.voicevox.json");
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        let (track, _) = add_singer(this, "Singer", &voice, true);
        this.session
            .set_singer_speaker(track, Some("Second"))
            .unwrap();
        let other = this.session.add_default_instrument_track("Other").unwrap();
        this.select_track(other);
        let revision = this.session.revision();

        this.apply_singer_voice(track, &voice);

        assert_eq!(this.selected_track, Some(track));
        assert_eq!(
            this.session
                .singer_voice(track)
                .unwrap()
                .unwrap()
                .speaker
                .as_deref(),
            Some("Second"),
            "choosing the current voice must not reset its selected speaker"
        );
        assert_eq!(
            this.session.revision(),
            revision,
            "reselecting a voice is not a document edit"
        );
    });
}

#[gpui::test]
fn stale_preview_completions_preserve_the_latest_request(cx: &mut TestAppContext) {
    let scratch = Scratch::new("preview-completion");
    let one = scratch.voice("one.voicevox.json");
    let two = scratch.voice("two.voicevox.json");
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        let (track, _) = add_singer(this, "Singer", &one, false);
        assert!(this.wish_sung_preview(track, 60, None));
        let old = this.sung_preview_wish.as_ref().unwrap().key.clone();
        let generation = this.sung_preview_generation;
        this.sung_preview_rendering = true;
        this.apply_singer_voice(track, &two);
        assert!(this.wish_sung_preview(track, 62, None));
        let current = this.sung_preview_wish.as_ref().unwrap().key.clone();
        this.finish_sung_preview(
            track,
            old.clone(),
            generation,
            Ok((vec![0.1; 128], 24000.0)),
        );
        assert!(
            this.sung_previews.is_empty(),
            "old-generation audio must not repopulate the cache"
        );
        assert_eq!(this.sung_preview_wish.as_ref().unwrap().key, current);
        assert!(!this.sung_preview_rendering);

        let generation = this.sung_preview_generation;
        assert!(this.wish_sung_preview(track, 64, None));
        let latest = this.sung_preview_wish.as_ref().unwrap().key.clone();
        this.sung_preview_rendering = true;
        this.finish_sung_preview(track, current, generation, Err(()));
        assert_eq!(
            this.sung_preview_wish.as_ref().unwrap().key,
            latest,
            "failure of an older pitch must not cancel the new pitch"
        );
        this.finish_sung_preview(
            track,
            latest.clone(),
            generation,
            Ok((vec![0.2; 128], 24000.0)),
        );
        assert!(this.sung_previews.contains_key(&latest));
        assert!(!this.sung_previews.contains_key(&old));
        assert!(
            this.sung_preview_wish.is_none(),
            "the current result fulfils its request"
        );
    });
}

#[gpui::test]
fn empty_and_missing_voices_do_not_stop_the_next_singer(cx: &mut TestAppContext) {
    let scratch = Scratch::new("scheduler-neighbours");
    let valid = scratch.voice("valid.voicevox.json");
    let missing = scratch.voice("missing.voicevox.json");
    let (app, cx) = open(cx);
    let (empty, unavailable, ready) = app.update(cx, |this, cx| {
        let (empty, _) = add_singer(this, "Empty", &valid, false);
        let (unavailable, _) = add_singer(this, "Unavailable", &missing, true);
        let (ready, _) = add_singer(this, "Ready", &valid, true);
        this.session.save_as(&scratch.join("Song.auris")).unwrap();
        std::fs::remove_file(&missing).unwrap();
        this.retry_singer_take(empty, cx);
        assert!(
            !this.sung_retry.contains(&empty),
            "an empty track cannot leave a permanent retry request"
        );
        assert!(this.auto_sing.is_none());
        settle_debounce(this);
        this.poll_auto_sing(cx);
        assert_eq!(
            this.auto_sing.as_ref().map(|render| render.track),
            Some(ready)
        );
        assert!(!this.sung_failures.contains_key(&empty));
        assert!(this.sung_failures.contains_key(&unavailable));
        (empty, unavailable, ready)
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.auto_sing.is_none());
        assert!(!this.sung_failures.contains_key(&empty));
        assert!(this.sung_failures.contains_key(&unavailable));
        assert!(
            this.sung_failures.contains_key(&ready),
            "the ready metadata reached inference and its offline server refused"
        );
    });
}

#[gpui::test]
fn failed_inference_waits_for_retry_or_changed_singing_input(cx: &mut TestAppContext) {
    let scratch = Scratch::new("scheduler-failures");
    let voice = scratch.voice("offline.voicevox.json");
    let (app, cx) = open(cx);
    let (track, clip, other) = app.update(cx, |this, cx| {
        let (track, clip) = add_singer(this, "Singer", &voice, true);
        let other = this
            .session
            .add_default_instrument_track("Unrelated")
            .unwrap();
        this.session.save_as(&scratch.join("Song.auris")).unwrap();
        settle_debounce(this);
        this.poll_auto_sing(cx);
        assert_eq!(
            this.auto_sing.as_ref().map(|render| render.track),
            Some(track)
        );
        (track, clip.unwrap(), other)
    });
    cx.run_until_parked();
    let failed_fingerprint = app.read_with(cx, |this, _| {
        assert!(this.auto_sing.is_none());
        this.sung_failures
            .get(&track)
            .expect("the offline engine failure is retained")
            .fingerprint
    });
    app.update(cx, |this, cx| {
        for _ in 0..3 {
            settle_debounce(this);
            this.poll_auto_sing(cx);
            assert!(
                this.auto_sing.is_none(),
                "an unchanged failure must not loop on every repaint"
            );
        }
        this.session
            .rename_track(other, "Renamed unrelated track")
            .unwrap();
        settle_debounce(this);
        this.poll_auto_sing(cx);
        assert!(
            this.auto_sing.is_none(),
            "unrelated edits must not retry a failed voice"
        );
        assert_eq!(this.sung_failures[&track].fingerprint, failed_fingerprint);
        this.retry_singer_take(track, cx);
        assert_eq!(
            this.auto_sing.as_ref().map(|render| render.track),
            Some(track)
        );
        assert!(!this.sung_failures.contains_key(&track));
    });
    cx.run_until_parked();
    app.update(cx, |this, cx| {
        assert!(this.sung_failures.contains_key(&track));
        this.session.set_note_lyric(clip, 0, "さ").unwrap();
        settle_debounce(this);
        this.poll_auto_sing(cx);
        assert_eq!(
            this.auto_sing.as_ref().map(|render| render.track),
            Some(track),
            "changing this singer's score starts a fresh attempt"
        );
        assert!(!this.sung_failures.contains_key(&track));
    });
    cx.run_until_parked();
    let original_acceleration = app.update(cx, |this, cx| {
        assert_ne!(this.sung_failures[&track].fingerprint, failed_fingerprint);
        let original = this.session.singer_acceleration();
        let changed = if original == Acceleration::Cpu {
            Acceleration::Auto
        } else {
            Acceleration::Cpu
        };
        this.apply_singer_acceleration(changed);
        assert!(
            this.sung_failures.is_empty(),
            "changing the inference processor permits a new attempt"
        );
        assert_eq!(this.session.singer_acceleration(), changed);
        settle_debounce(this);
        this.poll_auto_sing(cx);
        assert_eq!(
            this.auto_sing.as_ref().map(|render| render.track),
            Some(track)
        );
        original
    });
    cx.run_until_parked();
    app.update(cx, |this, _| {
        assert!(
            this.sung_failures.contains_key(&track),
            "the changed processor attempted the offline engine again"
        );
        this.apply_singer_acceleration(original_acceleration);
    });
}

#[gpui::test]
fn changing_speaker_discards_a_pending_manual_sing(cx: &mut TestAppContext) {
    let scratch = Scratch::new("manual-speaker-change");
    let voice = scratch.voice("manual.voicevox.json");
    let (app, cx) = open(cx);
    let track = app.update(cx, |this, cx| {
        let (track, _) = add_singer(this, "Singer", &voice, true);
        this.session.save_as(&scratch.join("Song.auris")).unwrap();
        this.select_track(track);
        this.sing_track(cx);
        assert!(this.export.as_ref().unwrap().result.is_none());
        this.set_singer_speaker_for(track, "Second".to_string());
        track
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        let export = this.export.as_ref().unwrap();
        assert_eq!(
            export.result,
            Some(Ok(this.t(Key::SingCancelled).to_string()))
        );
        assert_eq!(export.outcome(), ExportOutcome::Stopped);
        assert!(
            !this.status_failed,
            "the discarded old failure is not a failure of the new speaker"
        );
        assert!(this.sung_failures.is_empty());
        let singer = this
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_singer()
            .unwrap();
        assert!(singer.take.is_none());
        assert_eq!(
            singer.voice.as_ref().unwrap().speaker.as_deref(),
            Some("Second")
        );
    });
}

#[gpui::test]
fn replacing_a_connection_discards_manual_singing_even_when_the_score_matches(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new("manual-connection-change");
    let voice = scratch.voice("manual.voicevox.json");
    let (app, cx) = open(cx);
    let track = app.update(cx, |this, cx| {
        let (track, _) = add_singer(this, "Singer", &voice, true);
        this.session.save_as(&scratch.join("Song.auris")).unwrap();
        this.select_track(track);
        let fingerprint = this.session.singer_input_fingerprint(track).unwrap();
        this.sing_track(cx);
        assert!(this.export.as_ref().unwrap().result.is_none());
        write_voice(
            &voice,
            "Updated connection name",
            93.75,
            &["First", "Second"],
        );
        this.singer_configuration_changed(&voice, cx);
        assert_eq!(
            this.session.singer_input_fingerprint(track).unwrap(),
            fingerprint
        );
        track
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        let export = this.export.as_ref().unwrap();
        assert_eq!(
            export.result,
            Some(Ok(this.t(Key::SingCancelled).to_string()))
        );
        assert_eq!(export.outcome(), ExportOutcome::Stopped);
        assert!(this.sung_failures.is_empty());
        assert!(
            this.project()
                .track(track)
                .unwrap()
                .kind
                .as_singer()
                .unwrap()
                .take
                .is_none()
        );
        assert!(
            this.sung_retry.contains(&track),
            "the new connection still needs its own attempt"
        );
    });
}

#[gpui::test]
fn saving_a_changed_connection_refreshes_metadata_without_changing_selection(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new("configuration-refresh");
    let voice = scratch.voice("edited.voicevox.json");
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        let (track, _) = add_singer(this, "Singer", &voice, true);
        this.session
            .set_singer_speaker(track, Some("Second"))
            .unwrap();
        let other = this
            .session
            .add_default_instrument_track("Selected instrument")
            .unwrap();
        this.select_track(other);
        assert!(this.wish_sung_preview(track, 60, None));
        let old = this.sung_preview_wish.as_ref().unwrap().key.clone();
        let generation = this.sung_preview_generation;
        this.finish_sung_preview(track, old, generation, Ok((vec![0.1; 128], 24000.0)));
        assert!(!this.sung_previews.is_empty());
        write_voice(&voice, "Updated voice name", 100.0, &["Replacement"]);
        this.singer_configuration_changed(&voice, cx);
        assert_eq!(
            this.selected_track,
            Some(other),
            "saving setup must not navigate away from the current track"
        );
        let singer = this
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_singer()
            .unwrap();
        assert!((singer.frame_hop - 0.01).abs() < 1e-12);
        let selected = singer.voice.as_ref().unwrap();
        assert_eq!(selected.name, "Updated voice name");
        assert!(
            selected.speaker.is_none(),
            "a removed speaker falls back to the replacement default"
        );
        let info = this.session.singer_voice_info(track).unwrap().unwrap();
        assert_eq!(info.speaker.as_deref(), Some("Replacement"));
        assert_eq!(info.speakers, ["Replacement"]);
        assert!(this.sung_retry.contains(&track));
        assert!(this.sung_previews.is_empty());
        assert_ne!(this.sung_preview_generation, generation);
    });
}
