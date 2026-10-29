//! Bounded draft auditions; the composer never waits for a render.

use super::{JobEvent, Preview, audio_gate};
use auris_session::{
    RenderJob, Session,
    prelude::{OfflineOptions, RenderProgress},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub(super) struct Renderer {
    sender: Option<mpsc::SyncSender<(u64, RenderJob)>>,
    stopped: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    captured: Option<(u64, Instant)>,
}

impl Renderer {
    pub(super) fn start(id: u64, events: mpsc::Sender<JobEvent>) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<(u64, RenderJob)>(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&stopped);
        let busy = Arc::new(AtomicBool::new(false));
        let rendering = Arc::clone(&busy);
        let handle = std::thread::Builder::new()
            .name(format!("auris-draft-audio-{id}"))
            .spawn(move || {
                while let Ok((revision, mut job)) = receiver.recv() {
                    let Ok(_gate) = audio_gate(&cancel) else {
                        break;
                    };
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let result = job
                        .render(
                            &OfflineOptions {
                                sample_rate: Some(22_050.0),
                                include_tail: false,
                                ..Default::default()
                            },
                            &mut RenderProgress::default().cancelled_by(&cancel),
                        )
                        .map_err(|e| e.to_string());
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    match result {
                        // No playable content yet is normal while a draft is being assembled.
                        Ok(audio) if audio.peak() <= 1e-8 => {}
                        Ok(audio) => {
                            let _ = events.send(JobEvent::Preview(
                                id,
                                Ok(Preview {
                                    revision,
                                    audio: Arc::new(audio),
                                }),
                            ));
                        }
                        Err(error) => {
                            let _ = events.send(JobEvent::Preview(id, Err(error)));
                        }
                    }
                    rendering.store(false, Ordering::Release);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender: Some(sender),
            stopped,
            busy,
            handle: Some(handle),
            captured: None,
        })
    }

    pub(super) fn capture(&mut self, session: &mut Session) {
        let revision = session.revision();
        if self.busy.load(Ordering::Acquire)
            || self.captured.is_some_and(|(previous, time)| {
                previous == revision || time.elapsed() < Duration::from_secs(10)
            })
            || session.project().duration_seconds() > 330.0
            || !session.project().tracks.iter().any(|t| {
                t.kind
                    .note_clips()
                    .is_some_and(|clips| clips.iter().any(|c| !c.notes.is_empty()))
            })
        {
            return;
        }
        self.busy.store(true, Ordering::Release);
        if self.sender.as_ref().is_some_and(|sender| {
            sender
                .try_send((revision, session.render_job_at(Some(22_050.0))))
                .is_ok()
        }) {
            self.captured = Some((revision, Instant::now()));
        } else {
            self.busy.store(false, Ordering::Release);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::{SessionOptions, prelude::*};

    fn score() -> Session {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Motif", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(72, Ticks::ZERO, Ticks::QUARTER)])
            .unwrap();
        session
    }

    #[test]
    fn audition_captures_the_unfinished_score_without_editing_or_waiting_for_completion() {
        let mut session = score();
        let project = session.project().clone();
        let revision = session.revision();
        let (sender, events) = mpsc::channel();
        let mut renderer = Renderer::start(7, sender).unwrap();
        renderer.capture(&mut session);
        // Subsequent edits must not mutate the captured audio or its revision.
        session.set_tempo_at(Ticks::ZERO, 80.0);
        let JobEvent::Preview(id, result) = events.recv_timeout(Duration::from_secs(20)).unwrap()
        else {
            panic!("preview expected")
        };
        let preview = result.unwrap();
        assert_eq!(id, 7);
        assert_eq!(preview.revision, revision);
        assert!(preview.audio.peak() > 0.001);
        assert!((preview.audio.duration_seconds() - project.duration_seconds()).abs() < 0.01);
        assert_eq!(session.project().tracks, project.tracks);
        renderer.capture(&mut session);
        assert!(events.try_recv().is_err(), "refreshes are throttled");
    }

    #[test]
    fn dropping_a_waiting_renderer_cancels_without_late_audio_or_a_gate_deadlock() {
        let _gate = super::super::AUDIO_WORK.lock().unwrap();
        let mut session = score();
        let (sender, events) = mpsc::channel();
        let mut renderer = Renderer::start(8, sender).unwrap();
        renderer.capture(&mut session);
        let start = Instant::now();
        drop(renderer);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(events.try_recv().is_err());
    }
}
