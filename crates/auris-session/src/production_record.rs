//! Bounded conversation notes and revision-bound evidence, independent of the score.

use auris_core::{Project, time::Ticks};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A claim's author; agents cannot manufacture tool-observed evidence through note updates.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A condition attributed to a quoted user request, still subject to transcript verification.
    UserRequest,
    /// An explicit model assumption or musical decision.
    AgentAssumption,
}

/// The musical purpose of a retained note.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Purpose, duration, instrumentation, mood or constraints.
    Intent,
    /// Foreground, backing, counterline or section purpose.
    Role,
    /// Motif to retain or develop.
    Motif,
    /// Accepted/rejected candidate and its rationale.
    Decision,
    /// An unresolved musical or technical question.
    Unresolved,
}

/// One bounded, attributable note in a conversation's production record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Stable key for replacement, up to 64 ASCII letters/digits/dots/underscores/hyphens.
    #[schemars(length(min = 1, max = 64), regex(pattern = r"^[A-Za-z0-9_.-]+$"))]
    pub key: String,
    /// Musical category.
    pub category: Category,
    /// Attribution, not independent proof of a user's intent.
    pub source: Source,
    /// Exact short user quote for user_request, otherwise the assumption's rationale.
    pub basis: String,
    /// Plain text, at most 2048 UTF-8 bytes.
    pub text: String,
    /// Explicit clip IDs; missing IDs are reported on later reads.
    #[serde(default)]
    pub clips: Vec<u64>,
}

/// Read or edit conversation metadata without changing the song.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Enter a production phase, forward or back. Arrangement and mix require roles first.
    EnterPhase {
        /// The phase to work in from now on.
        phase: Phase,
        /// Why: the user's words when they asked to go back, otherwise the agent's rationale.
        reason: String,
        /// Starting point for the score phase only.
        #[serde(default)]
        writing_order: Option<WritingOrder>,
    },
    /// Register which tracks carry the foreground and which are a disposable guide.
    SetRoles {
        /// 1..16 foreground track IDs: the melody or lead vocal the later phases keep intact.
        #[serde(default)]
        foreground: Vec<u64>,
        /// Up to 16 guide track IDs the arrangement replaces or removes. A guide stays
        /// registered once named: leaving it out later does not replace it.
        #[serde(default)]
        guide: Vec<u64>,
        /// Instead of foreground: why this piece has no foreground melody.
        #[serde(default)]
        no_foreground: Option<String>,
    },
    /// Read notes, current/stale evidence and outstanding verification regions.
    Read,
    /// Replace one stable key; every other entry and tool observation is retained.
    Put {
        /// Attributed note; cannot set the tool evidence ledger.
        entry: Entry,
    },
    /// Remove one explicitly named note.
    Remove {
        /// Existing stable key.
        key: String,
    },
}

/// A stage of production, entered explicitly by the agent and never inferred from a keyword.
///
/// The order is the production order: a later phase keeps what an earlier one decided, and
/// going back to an earlier one leaves the later ones to be revisited.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Form, tempo and meter, key and chords, lyrics, the foreground melody and its voice.
    Score,
    /// Sounds, backing parts, drums, performance, transitions and SFX around a fixed score.
    Arrangement,
    /// Balance, effects, sends, automation, loudness and the verified deliverable.
    Mix,
}

impl Phase {
    const ALL: [Self; 3] = [Self::Score, Self::Arrangement, Self::Mix];
}

/// A musical starting point, independent of the order in which metadata is stored.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WritingOrder {
    /// Start from harmonic direction.
    HarmonyFirst,
    /// Start from a foreground motif.
    MelodyFirst,
    /// Start from groove and accents.
    RhythmFirst,
}

/// Tool-owned observations retained independently of free-form model summaries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    /// Actual tool name.
    pub tool: String,
    /// Content revision observed before the measurement.
    pub revision: String,
    /// Session lifetime; reopening conservatively invalidates observations.
    pub instance: String,
    /// Monotonic owner revision; even edit-then-Undo invalidates earlier verification.
    pub session_revision: u64,
    /// Exact measured range in one-based bars, if applicable.
    pub range: Option<(u64, u64)>,
    /// A track solo does not verify the full mix.
    pub track: Option<u64>,
    /// Actual tool result, bounded to 16 KiB, with no audio/image payload.
    pub result: Value,
}

/// Where production stands: the phase in force and the furthest one reached.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhaseState {
    /// The phase the agent works in now.
    pub phase: Phase,
    /// The latest phase ever entered; phases after `phase` up to it are to be revisited.
    pub reached: Phase,
    /// Starting point chosen for the score phase.
    pub writing_order: Option<WritingOrder>,
}

/// One explicit phase change, kept so a later turn knows why production went back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transition {
    /// The phase left, if any.
    pub from: Option<Phase>,
    /// The phase entered.
    pub to: Phase,
    /// The stated reason.
    pub reason: String,
}

/// Which tracks the later phases must keep and which they may discard.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Roles {
    /// Tracks whose notes, lyrics and timing are the song's foreground.
    pub foreground: Vec<u64>,
    /// Placeholder accompaniment written so the score can be heard.
    pub guide: Vec<u64>,
    /// Why there is no foreground, when there is none.
    pub no_foreground: Option<String>,
}

/// Stored independently of model context compaction and document Undo.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The explicitly entered phase, restored on resume.
    pub phase: Option<PhaseState>,
    /// The most recent 16 phase changes.
    pub transitions: Vec<Transition>,
    /// Registered foreground and guide tracks.
    pub roles: Option<Roles>,
    /// At most 64 short attributed notes.
    pub entries: Vec<Entry>,
    /// At most 32 recent observations; older observations are evicted explicitly by age.
    pub evidence: Vec<Evidence>,
    /// [`crate::phase::score_bars`] as production last went back to the score from a later
    /// phase, kept until it comes forward again.
    pub left_score: Option<Value>,
    /// What coming forward from that return asked to look at again, until the next phase change.
    pub review: Option<Value>,
}

impl Record {
    /// What the phase in force keeps fixed, if anything: nothing in the score phase, the score
    /// in the arrangement, and every note, sound and performance besides in the mix.
    pub fn lock(&self) -> Option<crate::phase::Lock> {
        match self.phase.as_ref()?.phase {
            Phase::Score => None,
            Phase::Arrangement => Some(crate::phase::Lock::Arrangement {
                foreground: self
                    .roles
                    .as_ref()
                    .map(|roles| roles.foreground.clone())
                    .unwrap_or_default(),
            }),
            Phase::Mix => Some(crate::phase::Lock::Mix),
        }
    }

    /// [`Self::update`], remembering the score when production goes back to it and saying what
    /// changed when production comes forward again.
    ///
    /// `document` is the inspection the update is made against; its `phase_facts.score_bars`
    /// is what is remembered and compared. The snapshot is taken on the first step back and
    /// kept across further steps, so a detour score, arrangement, score still compares with
    /// the arrangement as first left.
    pub fn update_with_document(
        &mut self,
        operation: Operation,
        document: &Value,
    ) -> Result<(), String> {
        let entering = match &operation {
            Operation::EnterPhase { phase, .. } => Some(*phase),
            _ => None,
        };
        let from = self.phase.as_ref().map(|state| state.phase);
        self.update(operation)?;
        let Some(to) = entering else {
            return Ok(());
        };
        let bars = &document["phase_facts"]["score_bars"];
        self.review = None;
        if to == Phase::Score {
            if from.is_some_and(|from| from > Phase::Score)
                && self.left_score.is_none()
                && bars.is_object()
            {
                self.left_score = Some(bars.clone());
            }
        } else if let Some(before) = self.left_score.take() {
            let foreground: &[u64] = self.roles.as_ref().map_or(&[], |r| &r.foreground);
            self.review = Some(crate::phase::review(&before, bars, foreground));
        }
        Ok(())
    }

    /// Update a note after checking all limits; a failure leaves the record unchanged.
    pub fn update(&mut self, operation: Operation) -> Result<(), String> {
        match operation {
            Operation::EnterPhase {
                phase,
                reason,
                writing_order,
            } => {
                if reason.trim().is_empty() || reason.len() > 2048 {
                    return Err("Give a nonempty reason up to 2048 bytes".into());
                }
                if writing_order.is_some() && phase != Phase::Score {
                    return Err("writing_order applies to the score phase only".into());
                }
                if phase > Phase::Score && self.roles.is_none() {
                    return Err("Register the foreground first: production_record set_roles with foreground track IDs, or no_foreground with a reason".into());
                }
                let previous = self.phase.as_ref();
                let state = PhaseState {
                    phase,
                    reached: previous.map_or(phase, |p| p.reached.max(phase)),
                    writing_order: writing_order.or_else(|| previous.and_then(|p| p.writing_order)),
                };
                self.transitions.push(Transition {
                    from: previous.map(|p| p.phase),
                    to: phase,
                    reason,
                });
                if self.transitions.len() > 16 {
                    self.transitions.remove(0);
                }
                self.phase = Some(state);
            }
            Operation::SetRoles {
                foreground,
                mut guide,
                no_foreground,
            } => {
                // An agent in the arrangement once re-registered roles with no guide and kept
                // the generated sketch as the finished part, which silenced guide_replaced
                // without replacing anything. Only deleting or silencing the track retires it.
                let earlier = self.roles.iter().flat_map(|roles| &roles.guide);
                for id in earlier {
                    if !guide.contains(id) && !foreground.contains(id) {
                        guide.push(*id);
                    }
                }
                let reason = no_foreground.filter(|r| !r.trim().is_empty());
                if foreground.is_empty() == reason.is_none()
                    || foreground.len() > 16
                    || guide.len() > 16
                    || reason.as_ref().is_some_and(|r| r.len() > 2048)
                    || guide.iter().any(|id| foreground.contains(id))
                {
                    return Err("Give 1..16 foreground track IDs or a no_foreground reason (not both), at most 16 guide IDs, none in both lists".into());
                }
                self.roles = Some(Roles {
                    foreground,
                    guide,
                    no_foreground: reason,
                });
            }
            Operation::Read => {}
            Operation::Put { entry } => {
                if entry.key.is_empty()
                    || entry.key.len() > 64
                    || !entry
                        .key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                    || entry.text.trim().is_empty()
                    || entry.text.len() > 2048
                    || entry.basis.trim().is_empty()
                    || entry.basis.len() > 2048
                    || entry.clips.len() > 16
                {
                    return Err("Use a valid key, nonempty text/basis up to 2048 bytes and at most 16 clip IDs".into());
                }
                if let Some(existing) = self.entries.iter_mut().find(|e| e.key == entry.key) {
                    *existing = entry;
                } else if self.entries.len() < 64 {
                    self.entries.push(entry);
                } else {
                    return Err(
                        "Production record has 64 entries; remove an obsolete entry first".into(),
                    );
                }
            }
            Operation::Remove { key } => self.entries.retain(|e| e.key != key),
        }
        Ok(())
    }

    /// Retain a genuine tool response only when its numeric revision matches inspection.
    pub fn observe(
        &mut self,
        state: &Value,
        tool: &str,
        revision: u64,
        range: Option<(u64, u64)>,
        track: Option<u64>,
        result: Value,
    ) {
        if state["session_revision"].as_u64() != Some(revision)
            || !state["revision"].is_string()
            || !state["document_instance"].is_string()
            || serde_json::to_vec(&result).map_or(true, |b| b.len() > 16 * 1024)
        {
            return;
        }
        self.evidence.push(Evidence {
            tool: tool.into(),
            revision: state["revision"].as_str().unwrap_or_default().into(),
            instance: state["document_instance"]
                .as_str()
                .unwrap_or_default()
                .into(),
            session_revision: revision,
            range,
            track,
            result,
        });
        if self.evidence.len() > 32 {
            self.evidence.remove(0);
        }
    }

    /// Reconcile persisted claims with today's IDs and conservatively invalidate old evidence.
    pub fn view(&self, state: &Value) -> Value {
        let clips: Vec<u64> = state["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|t| t["clips"].as_array().into_iter().flatten())
            .filter_map(|c| c["clip"].as_u64())
            .collect();
        let current = |e: &Evidence| {
            state["revision"] == e.revision
                && state["document_instance"] == e.instance
                && state["session_revision"] == e.session_revision
        };
        let entries: Vec<_> = self.entries.iter().map(|entry| json!({"entry":entry,
            "missing_clips":entry.clips.iter().filter(|id| !clips.contains(id)).collect::<Vec<_>>() })).collect();
        let evidence: Vec<_> = self
            .evidence
            .iter()
            .map(|e| json!({"observation":e,"status":if current(e) {"current"} else {"stale"}}))
            .collect();
        let coverage: Vec<_> = state["verification_regions"].as_array().into_iter().flatten().map(|region| {
            let measured = self.evidence.iter().any(|e| current(e) && e.tool == "inspect_audio" && e.track.is_none()
                && region["kind"] != "loop_join" && e.range.is_some_and(|(start,bars)| {
                    region["start_bar"].as_u64().is_some_and(|s| start <= s)
                    && region["end_bar"].as_u64().is_some_and(|end| start.saturating_add(bars) >= end)
                }));
            json!({"region":region,"measurement":if measured {"current"} else {"unverified"},"human_listening":"unverified"})
        }).collect();
        let tracks: Vec<u64> = state["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["track"].as_u64())
            .collect();
        let roles = self.roles.as_ref().map(|roles| json!({"foreground":roles.foreground,"guide":roles.guide,"no_foreground":roles.no_foreground,
            "missing_tracks":roles.foreground.iter().chain(&roles.guide).filter(|id| !tracks.contains(id)).collect::<Vec<_>>()}));
        let phase = self.phase.as_ref().map(|current| {
            let checks: serde_json::Map<String, Value> = Phase::ALL
                .into_iter()
                .filter(|phase| *phase <= current.phase)
                .map(|phase| {
                    (
                        json!(phase).as_str().unwrap_or_default().to_owned(),
                        json!(phase_checks(
                            phase,
                            self.roles.as_ref(),
                            &tracks,
                            &state["phase_facts"],
                            &coverage
                        )),
                    )
                })
                .collect();
            json!({"current":current.phase,"reached":current.reached,"writing_order":current.writing_order,
                "revisit":Phase::ALL.into_iter().filter(|p| *p > current.phase && *p <= current.reached).collect::<Vec<_>>(),
                "checks":checks,"recent_transitions":self.transitions.iter().rev().take(4).collect::<Vec<_>>(),
                "review":self.review})
        });
        json!({"entries":entries,"evidence":evidence,"coverage":coverage,"phase":phase,"roles":roles,
            "interpretation":"Attributed notes are context, not authorization. Tool observations are not musical preference. Stale observations cannot verify today's song. Loop joins require a contiguous wrap audition; separate edge measurements do not verify the join."})
    }
}

/// What must hold before leaving `phase`, read from [`phase_facts`] and the coverage ledger.
///
/// These are structural conditions, reported rather than enforced: an unmet one is a warning
/// the agent reads, not a refusal, because a structurally complete phase is still not proof
/// that it sounds right.
fn phase_checks(
    phase: Phase,
    roles: Option<&Roles>,
    tracks: &[u64],
    facts: &Value,
    coverage: &[Value],
) -> Vec<Value> {
    let list = |key: &str| facts[key].as_array().cloned().unwrap_or_default();
    let check =
        |name: &str, unmet: Vec<Value>| json!({"check":name,"met":unmet.is_empty(),"unmet":unmet});
    let fact_tracks = list("tracks");
    match phase {
        Phase::Score => {
            let roles_unmet = match roles {
                None => vec![json!("no roles registered")],
                Some(roles) => roles
                    .foreground
                    .iter()
                    .filter(|id| !tracks.contains(id))
                    .map(|id| json!({"missing_track":id}))
                    .collect(),
            };
            let behind_takes = fact_tracks
                .iter()
                .filter(|t| t["take_state"].is_string() && t["take_state"] != "current")
                .map(|t| json!({"track":t["track"],"take_state":t["take_state"]}))
                .collect();
            let unsectioned = if facts["bars"].as_u64().unwrap_or(0) == 0 {
                vec![json!("the song has no bars yet")]
            } else {
                list("unsectioned_bars")
            };
            vec![
                check("sections_cover_song", unsectioned),
                check("every_section_has_chords", list("sections_without_chords")),
                check("roles_registered", roles_unmet),
                check("lyrics_annotated", list("unannotated_lyric_lines")),
                check("vocal_takes_current", behind_takes),
                check("foreground_covers_song", foreground_unmet(roles, facts)),
                check("melody_rhythm_varied", rhythm_unmet(roles, facts)),
                // Read again in every later phase, since an arrangement can add findings too.
                check("lint_warnings_resolved", lint_unmet(facts)),
            ]
        }
        Phase::Arrangement => {
            let guide: &[u64] = roles.map_or(&[], |r| &r.guide);
            vec![
                check(
                    "drums_assigned",
                    fact_tracks
                        .iter()
                        .filter(|t| t["drums_assigned"] == false)
                        .map(|t| t["track"].clone())
                        .collect(),
                ),
                check(
                    "guide_replaced",
                    fact_tracks
                        .iter()
                        .filter(|t| {
                            t["audible"] == true
                                && t["track"].as_u64().is_some_and(|id| guide.contains(&id))
                        })
                        .map(|t| t["track"].clone())
                        .collect(),
                ),
            ]
        }
        Phase::Mix => vec![check(
            "full_mix_measured",
            coverage
                .iter()
                .filter(|c| c["region"]["kind"] != "loop_join" && c["measurement"] != "current")
                .map(|c| c["region"].clone())
                .collect(),
        )],
    }
}

/// The bars a track's unmuted clips sound notes in, repeats included, as inclusive 1-based
/// `[first, last]` runs: a few pairs for a whole song, where a list of bars would be hundreds.
fn bars_with_notes(project: &Project, clips: &[auris_core::project::MidiClip]) -> Vec<[u32; 2]> {
    let bar = |tick: Ticks| project.signatures.bar_beat_at(tick).0;
    let mut bars = std::collections::BTreeSet::new();
    for clip in clips.iter().filter(|clip| !clip.muted) {
        for note in clip.sounding_notes(120.0) {
            let start = clip.start + note.start;
            let end = (clip.start + note.end()).max(start + Ticks(1));
            bars.extend(bar(start)..=bar(end - Ticks(1)));
        }
    }
    let mut runs: Vec<[u32; 2]> = Vec::new();
    for b in bars {
        match runs.last_mut() {
            Some(run) if run[1] + 1 == b => run[1] = b,
            _ => runs.push([b, b]),
        }
    }
    runs
}

/// The rhythm warnings `melody_shape` gives a sung foreground track, which the score phase is
/// not finished with: every line entering at one point of the bar, a melody with no
/// syncopation, every line one length, or most notes one spacing apart.
fn rhythm_unmet(roles: Option<&Roles>, facts: &Value) -> Vec<Value> {
    let Some(roles) = roles else {
        return vec![];
    };
    facts["tracks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|track| {
            track["track"]
                .as_u64()
                .is_some_and(|id| roles.foreground.contains(&id))
        })
        .filter(|track| {
            track["melody_rhythm"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty())
        })
        .map(|track| json!({"track":track["track"],"melody_shape":track["melody_rhythm"]}))
        .collect()
}

/// How many of the song's bars at least one foreground track sounds in, against the song's
/// length: a melody that stops after one verse is a sketch reported finished.
fn foreground_unmet(roles: Option<&Roles>, facts: &Value) -> Vec<Value> {
    let Some(roles) = roles.filter(|roles| !roles.foreground.is_empty()) else {
        return vec![];
    };
    let song = facts["bars"].as_u64().unwrap_or(0);
    let mut bars = std::collections::BTreeSet::new();
    for track in facts["tracks"].as_array().into_iter().flatten() {
        if track["track"]
            .as_u64()
            .is_some_and(|id| roles.foreground.contains(&id))
        {
            for run in track["bars_with_notes"].as_array().into_iter().flatten() {
                if let (Some(first), Some(last)) = (run[0].as_u64(), run[1].as_u64()) {
                    bars.extend(first..=last);
                }
            }
        }
    }
    let played = bars.len() as u64;
    if song == 0 || played * 2 >= song {
        return vec![];
    }
    vec![json!({"foreground_bars":played,"song_bars":song,
        "needed":"the foreground sounds in at least half of the song's bars"})]
}

/// The structural checks of every phase the record has reached that the song does not meet,
/// each with its phase: what a host sends a composer back to when it reports a song finished.
///
/// Read from the live session, not from an inspection the model may have made several edits
/// ago. Lint findings and listening coverage are left out: the first has its own send-back,
/// and the second goes stale with every edit, so it would send back every song.
pub fn unmet_structure(record: &Record, session: &crate::Session) -> Vec<Value> {
    // A record without phases belongs to a song made before them, or to a small revision that
    // never needed one: there is nothing it promised.
    let Some(state) = &record.phase else {
        return vec![];
    };
    let facts = phase_facts(session);
    let tracks: Vec<u64> = session.project().tracks.iter().map(|t| t.id.0).collect();
    Phase::ALL
        .into_iter()
        .filter(|phase| *phase <= state.reached)
        .flat_map(|phase| {
            phase_checks(phase, record.roles.as_ref(), &tracks, &facts, &[])
                .into_iter()
                .filter(|check| {
                    check["met"] == false
                        && !matches!(
                            check["check"].as_str(),
                            Some("full_mix_measured" | "lint_warnings_resolved")
                        )
                })
                .map(move |mut check| {
                    check["phase"] = json!(phase);
                    check
                })
        })
        .collect()
}

/// The score lint's unsuppressed warnings and errors, counted by rule, from [`phase_facts`].
fn lint_unmet(facts: &Value) -> Vec<Value> {
    facts["lint"]["by_rule"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(rule, count)| json!({"rule":rule,"count":count,"see":"lint_project"}))
        .collect()
}

/// Structural facts the phase checks read, computed from the live document.
///
/// Bars are 1-based and inclusive. Lists are capped at 16 entries, which is enough to say
/// where to look without growing the inspection with the song.
pub fn phase_facts(session: &crate::Session) -> Value {
    const CAP: usize = 16;
    let project = session.project();
    let end = project.end_tick();
    let bar = |tick: Ticks| u64::from(project.signatures.bar_beat_at(tick).0);
    let bars = if end > Ticks::ZERO {
        bar(end - Ticks(1))
    } else {
        0
    };
    let spans = project.sections.spans_in(Ticks::ZERO, end);
    let mut unsectioned = vec![];
    let mut cursor = Ticks::ZERO;
    for start in spans.iter().map(|span| span.start).chain([end]) {
        if start > cursor {
            unsectioned.push(json!({"first_bar":bar(cursor),"last_bar":bar(start - Ticks(1))}));
        }
        cursor = spans
            .iter()
            .find(|span| span.start == start)
            .map_or(cursor, |span| cursor.max(span.end));
    }
    let without_chords: Vec<_> = spans
        .iter()
        .filter(|span| {
            project.harmony.chords.numeral_at(span.start).is_none()
                && !project
                    .harmony
                    .chords
                    .points()
                    .iter()
                    .any(|p| p.tick > span.start && p.tick < span.end && p.chord.is_some())
        })
        .map(|span| json!({"section":span.label,"first_bar":bar(span.start)}))
        .take(CAP)
        .collect();
    let tracks: Vec<_> = project
        .tracks
        .iter()
        .filter_map(|track| {
            let clips = track.kind.note_clips()?;
            if !clips.iter().any(|c| !c.notes.is_empty()) {
                return None;
            }
            let mut fact = json!({"track":track.id.0,"audible":!track.mixer.mute && clips.iter().any(|c| !c.muted && !c.notes.is_empty()),
                "bars_with_notes":bars_with_notes(project, clips)});
            if track.kind.is_drum() {
                fact["drums_assigned"] = session
                    .drum_assignments(track.id)
                    .is_some_and(|map| !map.voices.is_empty() || !map.lanes.is_empty())
                    .into();
            }
            if track.kind.is_singer() {
                fact["take_state"] = match session.singer_take_state(track.id) {
                    Ok(crate::SingerTakeState::Current) => "current",
                    Ok(crate::SingerTakeState::Behind) => "behind",
                    _ => "missing",
                }
                .into();
                fact["melody_rhythm"] =
                    crate::session::melody_form::rhythm_warnings(project, track.id).into();
            }
            Some(fact)
        })
        .collect();
    let unannotated: Vec<_> = project
        .tracks
        .iter()
        .filter(|track| track.kind.is_singer())
        .flat_map(|track| session.sung_lyric_lines(Some(track.id)))
        .filter(|line| !line.text.trim().is_empty() && project.lyrics.text_at(line.start).is_none())
        .take(CAP)
        .map(|line| json!({"first_bar":bar(line.start),"text":line.text}))
        .collect();
    json!({"bars":bars,"unsectioned_bars":unsectioned.into_iter().take(CAP).collect::<Vec<_>>(),
        "sections_without_chords":without_chords,"tracks":tracks,"unannotated_lyric_lines":unannotated,
        "lint":session.unresolved_lint_counts(),
        "score_bars":crate::phase::score_bars(project)})
}

/// Representative section excerpts, boundaries, ending and optional wrap seam to verify.
pub fn verification_regions(project: &Project) -> Value {
    let end = project.end_tick();
    if end <= Ticks::ZERO {
        return json!([]);
    }
    let bar = |tick: Ticks| u64::from(project.signatures.bar_beat_at(tick).0);
    let end_bar = bar(end - Ticks(1)) + 1;
    let mut regions = vec![];
    for section in project
        .sections
        .spans_in(Ticks::ZERO, end)
        .into_iter()
        .take(64)
    {
        let start = bar(section.start);
        regions.push(json!({"kind":"section","name":section.label,"start_bar":start,"end_bar":(start+2).min(bar(section.end-Ticks(1))+1)}));
        if section.start > Ticks::ZERO {
            regions.push(json!({"kind":"boundary","start_bar":start.saturating_sub(1).max(1),"end_bar":(start+1).min(end_bar)}));
        }
    }
    regions.push(
        json!({"kind":"ending","start_bar":end_bar.saturating_sub(2).max(1),"end_bar":end_bar}),
    );
    if project.loop_enabled
        && let Some((start, end)) = project.loop_region
    {
        regions.push(json!({"kind":"loop_join","tail_tick":end.raw(),"head_tick":start.raw()}));
    }
    json!(regions)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dotted_production_keys_survive_replacement_and_resume() {
        let mut record = Record::default();
        for text in ["first", "updated"] {
            record
                .update(Operation::Put {
                    entry: Entry {
                        key: "local_edit.validation.intent".into(),
                        category: Category::Intent,
                        source: Source::AgentAssumption,
                        basis: "runtime trial".into(),
                        text: text.into(),
                        clips: vec![],
                    },
                })
                .unwrap();
        }
        let restored: Record =
            serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(restored.entries.len(), 1);
        assert_eq!(restored.entries[0].text, "updated");
        let mut invalid = restored.entries[0].clone();
        invalid.key = "not/a/path".into();
        assert!(record.update(Operation::Put { entry: invalid }).is_err());
        assert_eq!(record.entries.len(), 1);
    }
    #[test]
    fn evidence_invalidates_on_revision_or_document_and_never_verifies_human_listening() {
        let mut record = Record::default();
        let state = json!({"revision":"a","session_revision":4,"document_instance":"one",
            "verification_regions":[{"kind":"section","start_bar":1,"end_bar":3},{"kind":"loop_join"}]});
        record.observe(
            &state,
            "inspect_audio",
            4,
            Some((1, 2)),
            None,
            json!({"peak_dbfs":-6}),
        );
        assert_eq!(record.view(&state)["coverage"][0]["measurement"], "current");
        assert_eq!(
            record.view(&state)["coverage"][0]["human_listening"],
            "unverified"
        );
        assert_eq!(
            record.view(&state)["coverage"][1]["measurement"],
            "unverified"
        );
        for field in ["revision", "document_instance"] {
            let mut changed = state.clone();
            changed[field] = "different".into();
            assert_eq!(record.view(&changed)["evidence"][0]["status"], "stale");
            assert_eq!(
                record.view(&changed)["coverage"][0]["measurement"],
                "unverified"
            );
        }
        record.observe(&state, "inspect_audio", 5, Some((1, 2)), None, json!({}));
        assert_eq!(record.evidence.len(), 1);
        let mut undone = state.clone();
        undone["session_revision"] = 6.into();
        assert_eq!(record.view(&undone)["evidence"][0]["status"], "stale");
    }
    fn enter(record: &mut Record, phase: &str) -> Result<(), String> {
        record.update(
            serde_json::from_value(json!({"kind":"enter_phase","phase":phase,"reason":"test"}))
                .unwrap(),
        )
    }

    #[test]
    fn later_phases_need_roles_and_going_back_leaves_them_to_revisit() {
        let mut record = Record::default();
        enter(&mut record, "score").unwrap();
        assert!(enter(&mut record, "arrangement").is_err());
        assert!(
            record
                .update(
                    serde_json::from_value(
                        json!({"kind":"set_roles","foreground":[3],"no_foreground":"ambient"})
                    )
                    .unwrap()
                )
                .is_err()
        );
        record
            .update(
                serde_json::from_value(json!({"kind":"set_roles","foreground":[3],"guide":[4]}))
                    .unwrap(),
            )
            .unwrap();
        enter(&mut record, "arrangement").unwrap();
        enter(&mut record, "mix").unwrap();
        enter(&mut record, "score").unwrap();
        let state = json!({"tracks":[{"track":3}]});
        let view = record.view(&state);
        assert_eq!(view["phase"]["current"], "score");
        assert_eq!(view["phase"]["revisit"], json!(["arrangement", "mix"]));
        assert_eq!(view["roles"]["missing_tracks"], json!([4]));
        assert!(view["phase"]["checks"]["arrangement"].is_null());
        enter(&mut record, "arrangement").unwrap();
        assert_eq!(record.view(&state)["phase"]["revisit"], json!(["mix"]));
        assert!(
            record
                .update(
                    serde_json::from_value(
                        json!({"kind":"enter_phase","phase":"mix","reason":" "})
                    )
                    .unwrap()
                )
                .is_err()
        );
        assert!(
            record
                .update(
                    serde_json::from_value(json!({"kind":"enter_phase","phase":"mix","reason":"x","writing_order":"melody_first"}))
                        .unwrap()
                )
                .is_err()
        );
        let restored: Record =
            serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(restored.view(&state)["phase"]["reached"], "mix");
    }

    #[test]
    fn arrangement_checks_read_drums_and_an_audible_guide() {
        let roles = Roles {
            foreground: vec![1],
            guide: vec![2],
            no_foreground: None,
        };
        let facts = json!({"tracks":[{"track":1,"audible":true},{"track":2,"audible":true},
            {"track":5,"audible":true,"drums_assigned":false}]});
        let checks = phase_checks(Phase::Arrangement, Some(&roles), &[1, 2, 5], &facts, &[]);
        assert_eq!(checks[0]["unmet"], json!([5]));
        assert_eq!(checks[1]["unmet"], json!([2]));
        let muted = json!({"tracks":[{"track":2,"audible":false}]});
        let checks = phase_checks(Phase::Arrangement, Some(&roles), &[1, 2], &muted, &[]);
        assert!(checks.iter().all(|c| c["met"] == true));
    }

    #[test]
    fn a_guide_stays_registered_when_roles_are_set_again_without_it() {
        let mut record = Record::default();
        let mut roles = |value: Value| record.update(serde_json::from_value(value).unwrap());
        roles(json!({"kind":"set_roles","foreground":[1],"guide":[2]})).unwrap();
        roles(json!({"kind":"set_roles","foreground":[1],"guide":[]})).unwrap();
        roles(json!({"kind":"set_roles","foreground":[1],"guide":[4]})).unwrap();
        assert_eq!(record.roles.as_ref().unwrap().guide, vec![4, 2]);
        let facts = json!({"tracks":[{"track":1,"audible":true},{"track":2,"audible":true}]});
        let checks = phase_checks(
            Phase::Arrangement,
            record.roles.as_ref(),
            &[1, 2],
            &facts,
            &[],
        );
        assert_eq!(checks[1]["unmet"], json!([2]));
        // Promoting the sketch to the foreground is a deliberate change, and allowed.
        record
            .update(
                serde_json::from_value(json!({"kind":"set_roles","foreground":[1,2],"guide":[]}))
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(record.roles.as_ref().unwrap().guide, vec![4]);
    }

    #[test]
    fn coming_back_from_the_score_lists_the_backing_over_the_changed_bars() {
        let facts = |global: &str, lead: &str, pad: &str| json!({"phase_facts":{"score_bars":{"global":global,"tracks":{"1":lead,"2":pad}}}});
        let enter = |record: &mut Record, phase: &str, document: &Value| {
            record
                .update_with_document(
                    serde_json::from_value(
                        json!({"kind":"enter_phase","phase":phase,"reason":"test"}),
                    )
                    .unwrap(),
                    document,
                )
                .unwrap();
        };
        let left = facts("aaaaaaaabbbbbbbb", "1111111122222222", "33333333--------");
        let mut record = Record::default();
        enter(&mut record, "score", &left);
        record
            .update(serde_json::from_value(json!({"kind":"set_roles","foreground":[1]})).unwrap())
            .unwrap();
        enter(&mut record, "arrangement", &left);
        assert!(record.review.is_none(), "nothing went back yet");
        enter(&mut record, "score", &left);
        // The chords of bar 1 change; bar 2's melody changes, but no backing plays there.
        let edited = facts("ccccccccbbbbbbbb", "1111111144444444", "33333333--------");
        enter(&mut record, "score", &edited);
        enter(&mut record, "arrangement", &edited);
        let review = record.review.as_ref().unwrap();
        assert_eq!(review["changed_bars"], json!([[1, 2]]));
        assert_eq!(
            review["tracks_to_review"],
            json!([{"track":2,"bars":[[1,1]]}])
        );
        assert!(record.left_score.is_none());
        assert_eq!(
            record.view(&json!({}))["phase"]["review"]["changed_bars"],
            json!([[1, 2]])
        );
        enter(&mut record, "mix", &edited);
        assert!(record.review.is_none());
    }

    #[test]
    fn each_phase_locks_what_the_earlier_ones_decided() {
        fn run(record: &mut Record, value: Value) {
            record
                .update(serde_json::from_value(value).unwrap())
                .unwrap();
        }
        let mut record = Record::default();
        run(
            &mut record,
            json!({"kind":"enter_phase","phase":"score","reason":"new song"}),
        );
        run(
            &mut record,
            json!({"kind":"set_roles","foreground":[3],"guide":[5]}),
        );
        assert_eq!(record.lock(), None);
        run(
            &mut record,
            json!({"kind":"enter_phase","phase":"arrangement","reason":"demo approved"}),
        );
        assert_eq!(
            record.lock(),
            Some(crate::phase::Lock::Arrangement {
                foreground: vec![3]
            })
        );
        run(
            &mut record,
            json!({"kind":"enter_phase","phase":"mix","reason":"parts done"}),
        );
        assert_eq!(record.lock(), Some(crate::phase::Lock::Mix));
        // Going back unlocks: the user asked for the change.
        run(
            &mut record,
            json!({"kind":"enter_phase","phase":"score","reason":"「サビのメロを変えたい」"}),
        );
        assert_eq!(record.lock(), None);
    }

    #[test]
    fn a_sung_lead_on_one_rhythm_throughout_is_unmet_structure() {
        use auris_core::project::Note;
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let lead = session.add_singer_track("Lead");
        let bar = Ticks(Ticks::QUARTER.raw() * 4);
        // Eight two-bar lines, each four quarter notes from the downbeat and a bar's rest.
        let clip = session
            .add_midi_clip(lead, "Verse", Ticks::ZERO, bar * 16)
            .unwrap();
        let notes = (0..8)
            .flat_map(|line| {
                (0..4).map(move |i| {
                    Note::new(
                        60 + i as u8,
                        bar * (2 * line) + Ticks::QUARTER * i,
                        Ticks(Ticks::QUARTER.raw() * 3 / 4),
                    )
                })
            })
            .collect();
        session.replace_notes(clip, notes).unwrap();
        let mut record = Record::default();
        record
            .update(Operation::EnterPhase {
                phase: Phase::Score,
                reason: "new song".into(),
                writing_order: None,
            })
            .unwrap();
        record
            .update(Operation::SetRoles {
                foreground: vec![lead.0],
                guide: vec![],
                no_foreground: None,
            })
            .unwrap();
        let rhythm: Vec<Value> = unmet_structure(&record, &session)
            .into_iter()
            .filter(|c| c["check"] == "melody_rhythm_varied")
            .collect();
        assert_eq!(rhythm.len(), 1, "{rhythm:?}");
        let text = rhythm[0]["unmet"].to_string();
        assert!(text.contains("same point of the bar"), "{text}");
        // A track that is not the foreground is not held to it.
        record
            .update(Operation::SetRoles {
                foreground: vec![],
                guide: vec![],
                no_foreground: Some("an instrumental".into()),
            })
            .unwrap();
        assert!(
            unmet_structure(&record, &session)
                .iter()
                .all(|c| c["check"] != "melody_rhythm_varied")
        );
    }

    #[test]
    fn a_foreground_that_stops_after_a_verse_is_unmet_structure_until_it_covers_the_song() {
        use auris_core::project::Note;
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let bar = Ticks(Ticks::QUARTER.raw() * 4);
        let verse = session
            .add_midi_clip(lead, "Verse", Ticks::ZERO, bar * 2)
            .unwrap();
        session
            .replace_notes(verse, vec![Note::new(72, Ticks::ZERO, bar * 2)])
            .unwrap();
        let pad = session.add_default_instrument_track("Pad").unwrap();
        let chords = session
            .add_midi_clip(pad, "Chords", Ticks::ZERO, bar * 8)
            .unwrap();
        session
            .replace_notes(chords, vec![Note::new(48, Ticks::ZERO, bar * 8)])
            .unwrap();
        assert_eq!(
            phase_facts(&session)["tracks"][0]["bars_with_notes"],
            json!([[1, 2]])
        );
        let mut record = Record::default();
        record
            .update(Operation::EnterPhase {
                phase: Phase::Score,
                reason: "new song".into(),
                writing_order: None,
            })
            .unwrap();
        record
            .update(Operation::SetRoles {
                foreground: vec![lead.0],
                guide: vec![],
                no_foreground: None,
            })
            .unwrap();
        let unmet = |record: &Record, session: &crate::Session| -> Vec<String> {
            unmet_structure(record, session)
                .iter()
                .map(|c| c["check"].as_str().unwrap().to_owned())
                .collect()
        };
        let checks = unmet(&record, &session);
        assert!(
            checks.contains(&"foreground_covers_song".to_owned()),
            "{checks:?}"
        );
        // Two bars of eight is a sketch; six of eight is a song with an intro and an outro.
        let chorus = session
            .add_midi_clip(lead, "Chorus", bar * 2, bar * 4)
            .unwrap();
        session
            .replace_notes(chorus, vec![Note::new(74, Ticks::ZERO, bar * 4)])
            .unwrap();
        assert!(!unmet(&record, &session).contains(&"foreground_covers_song".to_owned()));
        // Neither lint findings nor listening coverage are structure.
        assert!(
            unmet(&record, &session)
                .iter()
                .all(|c| c != "lint_warnings_resolved")
        );
        // A record that never entered a phase promised nothing.
        assert!(unmet_structure(&Record::default(), &session).is_empty());
    }

    #[test]
    fn score_facts_follow_sections_and_chords_in_the_live_document() {
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let mut run = |value: Value| {
            session
                .agent_command(serde_json::from_value(value).unwrap())
                .unwrap()
        };
        let added: Value = serde_json::from_str(&run(
            json!({"action":"add_track","name":"Lead","kind":"instrument"}),
        ))
        .unwrap();
        let track = added["track"].as_u64().unwrap();
        run(json!({"action":"add_clip","track":track,"name":"Verse","start_bar":1,"bars":4}));
        run(json!({"action":"edit_harmony","start_bar":1,"bars":2,"section":"Verse"}));
        let facts = phase_facts(&session);
        assert_eq!(facts["bars"], 4);
        assert_eq!(
            facts["unsectioned_bars"],
            json!([{"first_bar":3,"last_bar":4}])
        );
        assert_eq!(facts["sections_without_chords"][0]["section"], "Verse");
        let mut run = |value: Value| {
            session
                .agent_command(serde_json::from_value(value).unwrap())
                .unwrap()
        };
        run(
            json!({"action":"edit_harmony","start_bar":1,"bars":4,"section":"Verse","chords":"| I | V | I | V |"}),
        );
        let facts = phase_facts(&session);
        assert_eq!(facts["unsectioned_bars"], json!([]));
        assert_eq!(facts["sections_without_chords"], json!([]));
    }

    #[test]
    fn attributed_motifs_survive_round_trip_and_report_missing_ids() {
        let mut record = Record::default();
        record
            .update(Operation::Put {
                entry: Entry {
                    key: "motif".into(),
                    category: Category::Motif,
                    source: Source::UserRequest,
                    basis: "keep the pickup".into(),
                    text: "retain rising pickup".into(),
                    clips: vec![7],
                },
            })
            .unwrap();
        let restored: Record =
            serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(
            restored.view(&json!({}))["entries"][0]["missing_clips"],
            json!([7])
        );
        assert!(serde_json::from_value::<Operation>(json!({"kind":"put","evidence":[]})).is_err());
    }
}
