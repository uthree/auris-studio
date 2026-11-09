//! Behavioral layout checks for the Atelier shell composition.
//!
//! These tests intentionally inspect relationships between live controls rather than pixels or
//! colors. They catch the regressions most likely when the design language is applied: a fixed
//! side panel escaping its pane, a composer falling below the window, or a chooser losing its
//! primary actions at the minimum supported width.

#[cfg(test)]
mod tests {
    use gpui::{px, size};

    use crate::dock::Panel;
    use crate::harness::{open, paint, resize};

    #[gpui::test]
    fn agent_panel_keeps_transcript_and_composer_inside_the_panel_when_narrow(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| this.toggle_panel(Panel::Agent));
        paint(&app, cx);

        resize(&app, cx, size(px(720.0), px(720.0)));
        let panel = cx
            .debug_bounds("agent-panel")
            .expect("the agent panel remains reachable at the narrow width");
        let composer = cx
            .debug_bounds("agent-composer")
            .expect("the composer remains visible while the transcript scrolls");
        assert!(composer.left() >= panel.left());
        assert!(composer.right() <= panel.right());
        assert!(composer.bottom() <= panel.bottom());
        assert!(cx.debug_bounds("agent-input").is_some());
    }

    #[gpui::test]
    fn project_chooser_keeps_primary_actions_and_recent_region_at_narrow_width(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| this.show_project_chooser(window, cx));
        });
        resize(&app, cx, size(px(720.0), px(600.0)));

        let chooser = cx.debug_bounds("project-chooser").unwrap();
        assert!(cx.debug_bounds("chooser-content").is_some());
        for action in [
            "chooser-new",
            "chooser-recents",
            "chooser-confirm",
            "chooser-close",
        ] {
            let bounds = cx
                .debug_bounds(action)
                .expect("the chooser action is rendered");
            assert!(bounds.left() >= chooser.left(), "{action}");
            assert!(bounds.right() <= chooser.right(), "{action}");
            assert!(bounds.top() >= chooser.top(), "{action}");
            assert!(bounds.bottom() <= chooser.bottom(), "{action}");
        }
    }

    #[gpui::test]
    fn start_screen_titles_sit_on_the_window_centre_line(cx: &mut gpui::TestAppContext) {
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| this.show_project_chooser(window, cx));
        });
        for width in [1200.0, 900.0] {
            resize(&app, cx, size(px(width), px(700.0)));
            let title = cx.debug_bounds("chooser-title").unwrap();
            // The caption buttons on one side used to pull the title towards the other.
            assert!(
                (title.center().x - px(width / 2.0)).abs() <= px(1.0),
                "{width}"
            );
            assert!(cx.debug_bounds("chooser-motion").is_none());
        }
        cx.update(|window, cx| {
            app.update(cx, |this, cx| this.open_song_workshop(window, cx));
        });
        paint(&app, cx);
        let title = cx.debug_bounds("song-workshop-title").unwrap();
        assert!((title.center().x - px(450.0)).abs() <= px(1.0));
        // Back leads the bar, ahead of the title.
        let back = cx.debug_bounds("song-back").unwrap();
        assert!(back.right() <= title.left());
        assert!(back.bottom() <= crate::titlebar::HEIGHT);
    }

    #[gpui::test]
    fn choosing_ai_hands_the_panel_over_and_back_returns_it(cx: &mut gpui::TestAppContext) {
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| this.show_project_chooser(window, cx));
        });
        paint(&app, cx);
        crate::harness::click("chooser-ai", cx);
        crate::harness::click("chooser-confirm", cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_workshop.is_some());
            let transition = this.welcome_transition.expect("the hand-over runs");
            assert!(transition.to_workshop);
        });
        paint(&app, cx);
        crate::harness::click("song-back", cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_workshop.is_none());
            assert!(this.project_chooser.is_some());
            assert!(!this.welcome_transition.unwrap().to_workshop);
        });
    }

    #[gpui::test]
    fn undo_and_redo_live_in_the_titlebar_and_keep_their_edit_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx, _, clip) = crate::harness::with_a_clip(cx);
        let undo = cx.debug_bounds("editing-undo").unwrap();
        let redo = cx.debug_bounds("editing-redo").unwrap();
        assert!(undo.right() <= redo.left());
        assert!(undo.top() >= px(0.0) && undo.bottom() <= crate::titlebar::HEIGHT);
        crate::harness::click("editing-undo", cx);
        app.read_with(cx, |this, _| {
            assert!(this.session.midi_clip(clip).is_none())
        });
        crate::harness::click("editing-redo", cx);
        app.read_with(cx, |this, _| {
            assert!(this.session.midi_clip(clip).is_some())
        });
    }
}
