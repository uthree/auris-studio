//! Exact candidate audition with stale-output protection.
use super::*;

/// New source data may replace an idle audition, never a playing or preparing one.
pub(super) fn should_refresh_audition(
    played: Option<&Arc<auris_session::prelude::AudioBuffer>>,
    latest: Option<&Arc<auris_session::prelude::AudioBuffer>>,
    playing: bool,
    preparing: bool,
) -> bool {
    !playing
        && !preparing
        && played
            .zip(latest)
            .is_some_and(|(played, latest)| !Arc::ptr_eq(played, latest))
}

impl AurisApp {
    pub(crate) fn preview_song_candidate(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.acquire_audio(cx) {
            return;
        }
        let Some(state) = &mut self.song_workshop else {
            return;
        };
        let Some(audio) = state.audition(index) else {
            return;
        };
        let timeline = state.latest_timeline(index);
        // A loop song's audition is one seamless cycle, and plays round it until stopped.
        let loop_start = timeline
            .as_ref()
            .and_then(|timeline| timeline.loop_start_seconds);
        for player in &mut state.players {
            player.stop();
        }
        state.preview_serial = state.preview_serial.wrapping_add(1);
        let serial = state.preview_serial;
        let identity = state.identity;
        state.preparing = Some(index);
        let rate = self.session.sample_rate();
        cx.spawn(async move |this, cx| {
            let source = Arc::clone(&audio);
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    auris_session::prepare_looping_output_preview(&audio, rate, loop_start)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(state) = &mut this.song_workshop else {
                    return;
                };
                if state.identity != identity || state.preview_serial != serial {
                    return;
                }
                state.preparing = None;
                if this.session.audio_suspended() {
                    return;
                }
                match prepared.and_then(|audio| {
                    let options = auris_session::OutputPreviewOptions {
                        loop_start_seconds: loop_start,
                        ..state.players[index].options(audio.duration_seconds())
                    };
                    this.session.play_output_preview_with(audio, options)
                }) {
                    Ok(status) => {
                        state.played_audio[index] = Some(source);
                        state.played_timeline[index] = timeline;
                        state.players[index].start(status);
                    }
                    Err(error) => state.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_audio_refreshes_only_after_playback_and_preparation_are_idle() {
        let played = Arc::new(auris_session::prelude::AudioBuffer::stereo(100, 100.0));
        let same = Arc::clone(&played);
        let newer = Arc::new(auris_session::prelude::AudioBuffer::stereo(200, 100.0));
        assert!(!should_refresh_audition(
            Some(&played),
            Some(&newer),
            true,
            false
        ));
        assert!(!should_refresh_audition(
            Some(&played),
            Some(&newer),
            false,
            true
        ));
        assert!(should_refresh_audition(
            Some(&played),
            Some(&newer),
            false,
            false
        ));
        assert!(!should_refresh_audition(
            Some(&played),
            Some(&same),
            false,
            false
        ));
        assert!(!should_refresh_audition(None, Some(&newer), false, false));
        assert!(!should_refresh_audition(Some(&played), None, false, false));
    }
}
