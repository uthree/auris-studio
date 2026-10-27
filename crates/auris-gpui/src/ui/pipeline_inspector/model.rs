//! Transient presentation state for the focused rendering-pipeline inspector.
//!
//! The mixer canvas remains the editing surface. This model owns only one bounded,
//! revision-bound page at a time and is safe to discard whenever the selection changes.

use auris_session::pipeline_graph::{
    MAX_EDGES, MAX_NODES, PipelineError, PipelineNode, PipelinePage, PipelineQuery, PipelineScope,
};
use auris_session::prelude::{ClipId, Session, TrackId};

const PAGE_SIZE: usize = 32;
const EDGE_PAGE_SIZE: usize = 64;

/// The document object currently being explained by the inspector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum InspectorScope {
    /// No track or clip is selected.
    #[default]
    None,
    /// A clip takes precedence over its track.
    Clip(ClipId),
    /// The selected track's neighborhood.
    Track(TrackId),
}

impl InspectorScope {
    fn from_selection(clip: Option<ClipId>, track: Option<TrackId>) -> Self {
        clip.map(Self::Clip)
            .or_else(|| track.map(Self::Track))
            .unwrap_or(Self::None)
    }

    fn query_scope(self) -> PipelineScope {
        match self {
            Self::None => PipelineScope::All,
            Self::Clip(clip) => PipelineScope::Clip { clip: clip.0 },
            Self::Track(track) => PipelineScope::Track { track: track.0 },
        }
    }
}

/// Read-only state used by the inspector view.
#[derive(Default)]
pub(super) struct InspectorModel {
    revision: Option<u64>,
    scope: InspectorScope,
    offset: usize,
    page: Option<PipelinePage>,
    error: Option<PipelineError>,
    #[cfg(test)]
    fetch_count: usize,
}

impl InspectorModel {
    /// Creates an empty model without asking the session to build a projection.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// The last selection used to populate this model.
    pub(super) fn scope(&self) -> InspectorScope {
        self.scope
    }

    /// Session revision that owns the displayed ordinals and parameter bindings.
    pub(super) fn revision(&self) -> Option<u64> {
        self.revision
    }

    /// The cached node page in session document order. Stages and effects are not resorted.
    pub(super) fn nodes(&self) -> impl Iterator<Item = &PipelineNode> {
        self.page.iter().flat_map(|page| page.nodes.iter())
    }

    /// The cached projection page, including the bounded edge page.
    pub(super) fn page(&self) -> Option<&PipelinePage> {
        self.page.as_ref()
    }

    /// A read error from the most recent refresh, if any.
    pub(super) fn error(&self) -> Option<&PipelineError> {
        self.error.as_ref()
    }

    /// True when the selected object has no available projection nodes.
    pub(super) fn is_empty(&self) -> bool {
        self.page.as_ref().is_none_or(|page| page.nodes.is_empty())
    }

    /// Refreshes after selection or document changes. Repaints with the same revision and
    /// scope are a no-op and do not serialize or hash the document again.
    pub(super) fn refresh(
        &mut self,
        session: &Session,
        selected_clip: Option<ClipId>,
        selected_track: Option<TrackId>,
    ) {
        let scope = InspectorScope::from_selection(selected_clip, selected_track);
        let revision = session.revision();
        if self.revision == Some(revision) && self.scope == scope {
            return;
        }
        self.scope = scope;
        self.revision = Some(revision);
        self.offset = 0;
        self.load(session, 0);
    }

    /// Drops transient data when no object is selected.
    pub(super) fn clear(&mut self) {
        self.revision = None;
        self.scope = InspectorScope::None;
        self.offset = 0;
        self.page = None;
        self.error = None;
    }

    /// Moves to the next bounded node page, if one exists.
    pub(super) fn next_page(&mut self, session: &Session) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let Some(offset) = page.next_offset else {
            return;
        };
        if self.revision != Some(session.revision()) {
            self.refresh(session, self.selected_clip(), self.selected_track());
            return;
        }
        self.load(session, offset);
    }

    /// Returns to the preceding bounded node page.
    pub(super) fn previous_page(&mut self, session: &Session) {
        if self.offset == 0 {
            return;
        }
        if self.revision != Some(session.revision()) {
            self.refresh(session, self.selected_clip(), self.selected_track());
            return;
        }
        self.load(session, self.offset.saturating_sub(PAGE_SIZE));
    }

    /// Current page's zero-based node offset.
    pub(super) fn offset(&self) -> usize {
        self.offset
    }

    fn selected_clip(&self) -> Option<ClipId> {
        match self.scope {
            InspectorScope::Clip(clip) => Some(clip),
            _ => None,
        }
    }

    fn selected_track(&self) -> Option<TrackId> {
        match self.scope {
            InspectorScope::Track(track) => Some(track),
            _ => None,
        }
    }

    fn load(&mut self, session: &Session, offset: usize) {
        if self.scope == InspectorScope::None {
            self.clear();
            return;
        }
        let expected_revision = (offset > 0)
            .then(|| self.page.as_ref().map(|page| page.revision.clone()))
            .flatten();
        let query = PipelineQuery {
            scope: self.scope.query_scope(),
            offset,
            limit: Some(PAGE_SIZE.min(MAX_NODES)),
            edge_offset: 0,
            edge_limit: Some(EDGE_PAGE_SIZE.min(MAX_EDGES)),
            expected_revision,
        };
        match session.pipeline_graph(&query) {
            Ok(page) => {
                #[cfg(test)]
                {
                    self.fetch_count += 1;
                }
                self.offset = offset;
                self.page = Some(page);
                self.error = None;
            }
            Err(error) => {
                self.page = None;
                self.error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::prelude::{Note, SessionOptions, Ticks};

    fn session_with_clip() -> (Session, TrackId, ClipId) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Inspector").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)])
            .unwrap();
        (session, track, clip)
    }

    #[test]
    fn selection_prefers_clip_and_empty_selection_does_not_build_a_graph() {
        let (session, track, clip) = session_with_clip();
        let mut model = InspectorModel::new();
        model.refresh(&session, Some(clip), Some(track));
        assert_eq!(model.scope(), InspectorScope::Clip(clip));
        assert!(model.nodes().next().is_some());
        let before = serde_json::to_value(model.page().unwrap()).unwrap();
        let project_before = serde_json::to_value(session.project()).unwrap();
        model.clear();
        assert!(model.is_empty());
        assert_eq!(
            serde_json::to_value(session.project()).unwrap(),
            project_before
        );
        assert!(before["nodes"].is_array());
    }

    #[test]
    fn unchanged_revision_and_scope_reuses_the_same_page() {
        let (session, track, _) = session_with_clip();
        let mut model = InspectorModel::new();
        model.refresh(&session, None, Some(track));
        assert_eq!(model.fetch_count, 1);
        model.refresh(&session, None, Some(track));
        assert_eq!(model.fetch_count, 1);
    }

    #[test]
    fn revision_change_restarts_at_the_first_page() {
        let (mut session, track, _) = session_with_clip();
        let mut model = InspectorModel::new();
        model.refresh(&session, None, Some(track));
        let revision = session.revision();
        session.add_bus_track("Changed");
        assert_ne!(session.revision(), revision);
        model.refresh(&session, None, Some(track));
        assert_eq!(model.offset(), 0);
        assert!(model.error().is_none());
    }

    #[test]
    fn pagination_is_bounded_and_previous_page_reloads_the_same_scope() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Paged").unwrap();
        for index in 0..40 {
            session
                .add_effect(
                    Some(track),
                    if index % 2 == 0 {
                        "auris.fx.gain"
                    } else {
                        "auris.fx.delay"
                    },
                )
                .unwrap();
        }
        let mut model = InspectorModel::new();
        model.refresh(&session, None, Some(track));
        assert!(model.page().unwrap().total_nodes > PAGE_SIZE);
        assert!(model.page().unwrap().nodes.len() <= PAGE_SIZE);
        model.next_page(&session);
        assert_eq!(model.offset(), PAGE_SIZE);
        assert!(model.nodes().count() <= PAGE_SIZE);
        model.previous_page(&session);
        assert_eq!(model.offset(), 0);
        assert!(model.error().is_none());
    }

    #[test]
    fn deleted_or_unknown_scope_is_reported_without_a_panic() {
        let (session, _, _) = session_with_clip();
        let mut model = InspectorModel::new();
        model.refresh(&session, None, Some(TrackId(999_999)));
        assert!(matches!(model.error(), Some(PipelineError::UnknownScope)));
        assert!(model.nodes().next().is_none());
    }

    #[test]
    fn page_order_is_exposed_without_resorting_effects_or_stages() {
        use auris_session::pipeline_graph::PipelineNodeData;
        use auris_session::prelude::NoteTransform;
        let (mut session, track, clip) = session_with_clip();
        session
            .set_clip_transforms(clip, vec![NoteTransform::Gate { amount: 0.5 }; 2])
            .unwrap();
        let first = session.add_effect(Some(track), "auris.fx.gain").unwrap();
        let second = session.add_effect(Some(track), "auris.fx.gain").unwrap();
        session.move_effect(Some(track), second, -1);
        let mut model = InspectorModel::new();
        model.refresh(&session, None, Some(track));
        let slots: Vec<_> = model
            .nodes()
            .filter_map(|node| match node.data {
                PipelineNodeData::Effect { slot, .. } => Some(slot),
                _ => None,
            })
            .collect();
        assert_eq!(slots, [second.0, first.0]);
        let stages: Vec<_> = model
            .nodes()
            .filter_map(|node| match node.data {
                PipelineNodeData::ClipStage { stage_index, .. } => {
                    Some((node.id.as_str(), stage_index))
                }
                _ => None,
            })
            .collect();
        assert_eq!(stages.len(), 2);
        assert_ne!(stages[0].0, stages[1].0);
        assert_eq!((stages[0].1, stages[1].1), (0, 1));
    }
}
