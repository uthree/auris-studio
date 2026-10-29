//! Exact candidate audition with stale-output protection.
use super::*;
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
                .spawn(async move { auris_session::prepare_output_preview(&audio, rate) })
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
                    let options = state.players[index].options(audio.duration_seconds());
                    this.session.play_output_preview_with(audio, options)
                }) {
                    Ok(status) => {
                        state.played_audio[index] = Some(source);
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
