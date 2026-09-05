//! The track picker discovers Engine singers even for a legacy one-style connection.

use super::*;
use crate::app::SingerFailure;
use crate::harness::{choose, click, paint, resize, with_a_singer_clip};
use crate::ui::context_menu::MenuEntry;
use auris_session::VoicevoxStyle;
use gpui::{Modifiers, TestAppContext, point, px, size};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct Scratch {
    root: PathBuf,
    parent: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = parent.join(format!(
            "auris-voicevox-picker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        assert!(root.starts_with(&parent) && root != parent);
        Self { root, parent }
    }

    fn voice(&self, file: &str, url: &str) -> PathBuf {
        let path = self.root.join(file);
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "format_version": 1,
                "name": "VOICEVOX singer",
                "url": url,
                "sample_rate": 24000,
                "frame_rate": 93.75,
                "styles": [{
                    "name": "Singer / normal",
                    "query_style_id": 6000,
                    "decode_style_id": 3001,
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Ok(resolved) = std::fs::canonicalize(&self.root)
            && resolved == self.root
            && resolved.starts_with(&self.parent)
            && resolved != self.parent
        {
            let _ = std::fs::remove_dir_all(resolved);
        }
    }
}

struct Engine {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<Vec<String>>>,
}

impl Engine {
    fn singers() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut paths = Vec::new();
            while paths.len() < 2 && Instant::now() < deadline && !stopped.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("the mock Engine could not accept a request: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut block = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let count = stream.read(&mut block).unwrap();
                    assert!(
                        count != 0 && request.len() < 16_384,
                        "a complete HTTP header is required"
                    );
                    request.extend_from_slice(&block[..count]);
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap().to_string();
                let response = match path.as_str() {
                    "/version" => "\"0.24.1\"",
                    "/singers" => {
                        r#"[
                        {"name":"波音リツ","speaker_uuid":"teacher","version":"0.24.1",
                         "styles":[{"name":"ノーマル","id":6000,"type":"singing_teacher"}],
                         "supported_features":{"permitted_synthesis_morphing":"ALL"}},
                        {"name":"ずんだもん","speaker_uuid":"decoder","version":"0.24.1",
                         "styles":[{"name":"あまあま","id":3001,"type":"frame_decode"},
                                   {"name":"ノーマル","id":3003,"type":"frame_decode"}],
                         "supported_features":{"permitted_synthesis_morphing":"ALL"}}
                    ]"#
                    }
                    _ => panic!("unexpected Engine request: {path}"),
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                paths.push(path);
            }
            paths
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }

    fn finish(&mut self) {
        let paths = self.worker.take().unwrap().join().unwrap();
        assert_eq!(paths, ["/version", "/singers"]);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn catalog() -> VoicevoxCatalog {
    VoicevoxCatalog {
        version: "0.24.1".into(),
        query: vec![VoicevoxStyle {
            id: 6000,
            singer: "波音リツ".into(),
            name: "ノーマル".into(),
        }],
        decode: vec![
            VoicevoxStyle {
                id: 3001,
                singer: "ずんだもん".into(),
                name: "あまあま".into(),
            },
            VoicevoxStyle {
                id: 3003,
                singer: "ずんだもん".into(),
                name: "ノーマル".into(),
            },
        ],
    }
}

fn arm_request(app: &mut AurisApp, track: TrackId) -> (VoicevoxConnection, u64, u64) {
    let connection = app.session.singer_voicevox_connection(track).unwrap();
    app.voicevox_menu_generation += 1;
    let request = app.voicevox_menu_generation;
    let mut menu = app.singer_speaker_menu(track, point(px(200.0), px(100.0)));
    menu.async_request = Some(request);
    app.open_menu(menu);
    (connection, request, app.sung_preview_generation)
}

#[gpui::test]
fn the_track_picker_fetches_real_engine_names_and_persists_the_clicked_style(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut engine = Engine::singers();
    let path = scratch.voice("legacy.voicevox.json", &engine.url);
    let (app, cx, track, _) = with_a_singer_clip(cx);
    app.update(cx, |this, _| {
        this.panels = crate::dock::PanelLayout::default();
        this.select_track(track);
        this.session.set_singer_voice(track, Some(&path)).unwrap();
        assert_eq!(
            this.singer_voice_label(track).as_deref(),
            Some("VOICEVOX singer · Singer / normal")
        );
        let saved = this.singer_speaker_menu(track, point(px(10.0), px(10.0)));
        assert_eq!(
            saved.entries.len(),
            1,
            "the old connection really contains just the placeholder"
        );
    });
    paint(&app, cx);
    click("singer-speaker", cx);
    cx.run_until_parked();
    engine.finish();
    let command = app.update(cx, |this, _| {
        let menu = this
            .menu
            .as_ref()
            .expect("the requesting picker stays open");
        let styles: Vec<_> = menu
            .entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item)
                    if matches!(item.command, MenuCommand::VoicevoxSpeaker { .. }) =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(styles.len(), 2);
        assert_eq!(styles[0].label.as_ref(), "ずんだもん / あまあま");
        assert!(
            styles[0].checked,
            "the actual saved decoder 3001 is selected by its ID"
        );
        assert_eq!(styles[1].label.as_ref(), "ずんだもん / ノーマル");
        assert!(!styles[1].checked);
        let command = styles[1].command.clone();
        assert_eq!(
            this.singer_voice_label(track).as_deref(),
            Some("VOICEVOX singer · ずんだもん / あまあま")
        );
        this.sung_failures.insert(
            track,
            SingerFailure {
                revision: this.session.revision(),
                fingerprint: 0,
                folder: None,
                message: "The previous speaker failed".into(),
            },
        );
        command
    });
    paint(&app, cx);
    choose(&app, cx, &command);
    app.update(cx, |this, _| {
        assert!(this.menu.is_none());
        assert!(
            !this.sung_failures.contains_key(&track),
            "a new speaker permits synthesis again"
        );
        assert!(!this.status_failed);
        assert_eq!(
            this.singer_voice_label(track).as_deref(),
            Some("VOICEVOX singer · ずんだもん / ノーマル")
        );
        let connection = this.session.singer_voicevox_connection(track).unwrap();
        assert_eq!(connection.query_style_id, 6000);
        assert_eq!(connection.decode_style_id, 3003);
        this.session
            .save_as(&scratch.root.join("Song.auris"))
            .unwrap();
        let document = this.session.path().unwrap().to_path_buf();
        this.session.open(&document).unwrap();
        this.voicevox_catalogs.clear();
        assert_eq!(
            this.singer_voice_label(track).as_deref(),
            Some("VOICEVOX singer · ずんだもん / ノーマル")
        );
        let saved = this.singer_speaker_menu(track, point(px(10.0), px(10.0)));
        assert!(
            saved
                .entries
                .iter()
                .any(|entry| matches!(entry, MenuEntry::Item(item)
            if item.checked && item.label.as_ref() == "ずんだもん / ノーマル"))
        );
        assert_eq!(
            this.session
                .singer_voicevox_connection(track)
                .unwrap()
                .decode_style_id,
            3003
        );
    });
}

#[gpui::test]
fn late_catalogues_do_not_reopen_closed_or_replaced_menus(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let path = scratch.voice("legacy.voicevox.json", "http://127.0.0.1:0");
    let (app, cx, track, _) = with_a_singer_clip(cx);
    app.update(cx, |this, cx| {
        this.session.set_singer_voice(track, Some(&path)).unwrap();
        let (connection, request, generation) = arm_request(this, track);
        this.close_menu();
        this.finish_voicevox_speakers(track, connection, request, generation, Ok(catalog()), cx);
        assert!(
            this.menu.is_none(),
            "closing a loading menu cancels its display result"
        );
        assert!(this.voicevox_catalogs.is_empty());

        let (connection, request, generation) = arm_request(this, track);
        this.open_menu(
            ContextMenu::new(point(px(10.0), px(10.0)), "Another menu")
                .item("Add track", MenuCommand::NewAudioTrack),
        );
        this.finish_voicevox_speakers(track, connection, request, generation, Ok(catalog()), cx);
        assert_eq!(this.menu.as_ref().unwrap().title.as_ref(), "Another menu");
        assert!(this.voicevox_catalogs.is_empty());

        let (connection, request, generation) = arm_request(this, track);
        let (_, latest, _) = arm_request(this, track);
        this.finish_voicevox_speakers(track, connection, request, generation, Ok(catalog()), cx);
        assert_eq!(this.menu.as_ref().unwrap().async_request, Some(latest));
        assert!(
            this.voicevox_catalogs.is_empty(),
            "an older response cannot replace the new request"
        );
    });
}

#[gpui::test]
fn changing_the_voice_rejects_both_a_late_catalogue_and_an_old_choice(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let original = scratch.voice("original.voicevox.json", "http://127.0.0.1:0");
    let replacement = scratch.voice("replacement.voicevox.json", "http://127.0.0.1:0");
    let (app, cx, track, _) = with_a_singer_clip(cx);
    app.update(cx, |this, cx| {
        this.session
            .set_singer_voice(track, Some(&original))
            .unwrap();
        let (connection, request, generation) = arm_request(this, track);
        this.invalidate_sung_previews();
        this.finish_voicevox_speakers(track, connection, request, generation, Ok(catalog()), cx);
        assert!(
            this.menu.is_none(),
            "a changed voice generation rejects an old request"
        );

        let (connection, request, generation) = arm_request(this, track);
        this.session
            .set_singer_voice(track, Some(&replacement))
            .unwrap();
        this.finish_voicevox_speakers(track, connection, request, generation, Ok(catalog()), cx);
        assert!(
            this.menu.is_none(),
            "a direct session edit also invalidates the connection"
        );
        assert!(this.voicevox_catalogs.is_empty());

        this.session
            .set_singer_voice(track, Some(&original))
            .unwrap();
        let (connection, request, generation) = arm_request(this, track);
        let choice = catalog().speaker_choices(&connection)[1].clone();
        this.finish_voicevox_speakers(
            track,
            connection.clone(),
            request,
            generation,
            Ok(catalog()),
            cx,
        );
        this.session
            .set_singer_voice(track, Some(&replacement))
            .unwrap();
        let revision = this.session.revision();
        let old_bytes = std::fs::read(&original).unwrap();
        this.choose_voicevox_speaker(track, Arc::new(connection), choice, cx);
        assert_eq!(
            this.session.revision(),
            revision,
            "an old row cannot edit the new voice"
        );
        assert_eq!(
            this.session.singer_voice_info(track).unwrap().unwrap().path,
            replacement
        );
        assert_eq!(
            this.session
                .singer_voicevox_connection(track)
                .unwrap()
                .decode_style_id,
            3001
        );
        assert_eq!(
            std::fs::read(&original).unwrap(),
            old_bytes,
            "the stale choice writes neither connection"
        );
    });
}

#[gpui::test]
fn a_failed_catalogue_preserves_the_saved_speaker_and_offers_refresh(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let path = scratch.voice("legacy.voicevox.json", "http://127.0.0.1:0");
    let (app, cx, track, _) = with_a_singer_clip(cx);
    app.update(cx, |this, cx| {
        this.session.set_singer_voice(track, Some(&path)).unwrap();
        let revision = this.session.revision();
        let (connection, request, generation) = arm_request(this, track);
        this.finish_voicevox_speakers(track, connection, request, generation, Err("Engine offline".into()), cx);
        let entries = &this.menu.as_ref().unwrap().entries;
        assert!(entries.iter().any(|entry| matches!(entry, MenuEntry::Item(item)
            if item.checked && item.label.as_ref() == "Singer / normal")));
        assert!(entries.iter().any(|entry| matches!(entry, MenuEntry::Item(item)
            if !item.enabled && item.label.as_ref() == "Engine offline")));
        assert!(entries.iter().any(|entry| matches!(entry, MenuEntry::Item(item)
            if item.enabled && matches!(item.command, MenuCommand::RefreshVoicevoxSpeakers { .. }))));
        assert_eq!(this.session.revision(), revision);
        assert!(this.voicevox_catalogs.is_empty());
    });
}

#[gpui::test]
fn a_long_engine_singer_list_scrolls_and_its_last_voice_can_be_clicked(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let path = scratch.voice("legacy.voicevox.json", "http://127.0.0.1:0");
    let (app, cx, track, _) = with_a_singer_clip(cx);
    let viewport = size(px(640.0), px(320.0));
    resize(&app, cx, viewport);
    app.update(cx, |this, cx| {
        this.select_track(track);
        this.session.set_singer_voice(track, Some(&path)).unwrap();
        let mut many = catalog();
        many.decode = (0..81)
            .map(|index| VoicevoxStyle {
                id: 3001 + index,
                singer: "Singer".into(),
                name: format!("Style {index:02}"),
            })
            .collect();
        let (connection, request, generation) = arm_request(this, track);
        this.finish_voicevox_speakers(track, connection, request, generation, Ok(many), cx);
    });
    paint(&app, cx);
    let panel = cx.debug_bounds("context-menu-panel").unwrap();
    assert!(panel.top() >= px(0.0) && panel.bottom() <= viewport.height);
    assert!(panel.left() >= px(0.0) && panel.right() <= viewport.width);
    let rows = cx.debug_bounds("context-menu-rows").unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: rows.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-10000.0))),
        ..Default::default()
    });
    paint(&app, cx);
    let last = cx.debug_bounds("menu-item-80").unwrap();
    let rows = cx.debug_bounds("context-menu-rows").unwrap();
    assert!(
        last.top() >= rows.top() && last.bottom() <= rows.bottom(),
        "the final singer is inside the scrolled hit area"
    );
    cx.simulate_click(last.center(), Modifiers::none());
    app.read_with(cx, |this, _| {
        assert!(this.menu.is_none());
        assert_eq!(
            this.session
                .singer_voicevox_connection(track)
                .unwrap()
                .decode_style_id,
            3081
        );
        assert_eq!(
            this.singer_voice_label(track).as_deref(),
            Some("VOICEVOX singer · Singer / Style 80")
        );
    });
}
