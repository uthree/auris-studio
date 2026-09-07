//! Track routing, audition state and static instrument controls.

use super::*;

/// Replaces a software instrument, drum or singer track with its rendered audio.
pub mod convert_track_to_audio {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "convert_track_to_audio";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Renders an instrument, drum or singer track and replaces it with an audio track at the same position, preserving its ID, mixer, effects and routing. Instrument automation is baked; mixer automation stays editable. A singer uses its current take or generates a fresh one through its chosen voice. Saves with a checkpoint so the original score can be restored. Refuses audio tracks, buses, empty tracks and unavailable sounds.";

    /// One track to convert in an existing project.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Exact track name or stable id:N selector from describe.
        pub track: String,
    }

    /// Converts the track and checkpoints the previous saved score.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        let clip = session
            .convert_track_to_audio(track)
            .map_err(|error| error.to_string())?;
        session
            .save_with_checkpoint()
            .map_err(|error| error.to_string())?;
        Ok(serde_json::json!({"track": format!("id:{}", track.0), "clip": clip.0, "kind": "audio"}).to_string())
    }
}

/// Reads and changes track outputs and auxiliary sends.
pub mod routing {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "routing";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Reads or changes a track's output and sends. Start with operation list to discover available buses and send IDs. Output requires destination (a bus name, id:N, or master). Add_send requires a bus destination and optionally level_db (-60 to 0) and pre_fader. Remove_send, send_mode and send_level select an existing send by destination or send_id; send_mode requires pre_fader and send_level requires level_db (-60 to 0). Send levels report whether automation overrides the static value. A bus can be created with add_track kind bus. Returns the actual routing; changes are checkpointed and saved.";

    /// Which routing command to perform.
    #[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum Operation {
        /// Read the output, sends and available buses without saving.
        List,
        /// Point the main output at a bus or the master.
        Output,
        /// Add a send to a bus that this track does not already send to.
        AddSend,
        /// Delete a send, including its automation.
        RemoveSend,
        /// Change whether an existing send is taken before the fader.
        SendMode,
        /// Change the static level of an existing send.
        SendLevel,
    }

    /// A routing request with its operation's fields at the top level.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Source track name or stable id:N selector from describe.
        pub track: String,
        /// list, output, add_send, remove_send, send_mode, or send_level.
        pub operation: Operation,
        /// Bus name or id:N; output also accepts master. Required for output/add_send.
        /// For existing sends, supply this or send_id, never both.
        pub destination: Option<String>,
        /// Send level, -60 to 0 dB; required for send_level, optional for add_send (defaults to 0).
        pub level_db: Option<f32>,
        /// True takes a send before the fader. Required for send_mode, optional for
        /// add_send (defaults to false); omit for other operations.
        pub pre_fader: Option<bool>,
        /// Stable send ID returned by list; existing-send operations only. Use this
        /// instead of destination when multiple sends feed the same bus.
        pub send_id: Option<u64>,
    }

    fn validate(args: &Args) -> Result<(), String> {
        let valid = match args.operation {
            Operation::List => {
                args.destination.is_none()
                    && args.level_db.is_none()
                    && args.pre_fader.is_none()
                    && args.send_id.is_none()
            }
            Operation::Output => {
                args.destination.is_some()
                    && args.level_db.is_none()
                    && args.pre_fader.is_none()
                    && args.send_id.is_none()
            }
            Operation::AddSend => args.destination.is_some() && args.send_id.is_none(),
            Operation::SendLevel => {
                (args.destination.is_some() != args.send_id.is_some())
                    && args.level_db.is_some()
                    && args.pre_fader.is_none()
            }
            Operation::RemoveSend | Operation::SendMode => {
                (args.destination.is_some() != args.send_id.is_some())
                    && args.level_db.is_none()
                    && (args.pre_fader.is_some() == matches!(args.operation, Operation::SendMode))
            }
        };
        if !valid {
            return Err(match args.operation {
                Operation::List => "list accepts only project, track and operation",
                Operation::Output => {
                    "output requires destination; omit level_db, pre_fader and send_id"
                }
                Operation::AddSend => {
                    "add_send requires destination; optional level_db and pre_fader; omit send_id"
                }
                Operation::RemoveSend => {
                    "remove_send requires destination or send_id, never both; omit level_db and pre_fader"
                }
                Operation::SendMode => {
                    "send_mode requires destination or send_id, never both, and pre_fader; omit level_db"
                }
                Operation::SendLevel => {
                    "send_level requires destination or send_id, never both, and level_db; omit pre_fader"
                }
            }
            .into());
        }
        if let Some(level) = args.level_db
            && !(-60.0..=0.0).contains(&level)
        {
            return Err("level_db must be between -60 and 0 dB".into());
        }
        Ok(())
    }

    fn existing_send(session: &Session, track: TrackId, args: &Args) -> Result<SendId, String> {
        let sends = &session
            .project()
            .track(track)
            .expect("resolved track")
            .sends;
        if let Some(id) = args.send_id {
            return sends
                .iter()
                .find(|send| send.id.0 == id)
                .map(|send| send.id)
                .ok_or_else(|| format!("no send_id {id} on this track; use routing list"));
        }
        let destination = track_by_name(
            session.project(),
            args.destination.as_deref().expect("validated destination"),
        )?
        .id;
        let mut matches = sends.iter().filter(|send| send.target == destination);
        match (matches.next(), matches.next()) {
            (Some(send), None) => Ok(send.id),
            (None, _) => Err("this track has no send to that bus; use routing list".into()),
            _ => Err("multiple sends feed that bus; use send_id from routing list".into()),
        }
    }

    fn describe(session: &Session, id: TrackId) -> String {
        let project = session.project();
        let track = project.track(id).expect("resolved track");
        let destination = |id: TrackId| {
            serde_json::json!({
                "track": format!("id:{}", id.0),
                "name": project.track(id).map(|track| &track.name)
            })
        };
        let sends: Vec<_> = track
            .sends
            .iter()
            .map(|send| {
                serde_json::json!({
                    "send_id": send.id.0,
                    "destination": destination(send.target),
                    "level_db": send.level_db,
                    "pre_fader": send.pre_fader,
                    "automated": session.is_automated(ParamTarget::Send { track: id, send: send.id })
                })
            })
            .collect();
        let buses: Vec<_> = session
            .available_buses(id)
            .into_iter()
            .map(destination)
            .collect();
        serde_json::json!({
            "track": format!("id:{}", id.0),
            "name": track.name,
            "output": track.output.bus().map(destination).unwrap_or_else(|| serde_json::json!({"track":"master"})),
            "sends": sends,
            "available_buses": buses
        })
        .to_string()
    }

    /// Performs one validated routing operation and returns the current routing.
    pub fn run(args: &Args) -> Result<String, String> {
        validate(args)?;
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?.id;
        match args.operation {
            Operation::List => {}
            Operation::Output => {
                let destination = strip_by_name(
                    session.project(),
                    args.destination.as_deref().expect("validated destination"),
                )?;
                let output = destination.map_or(Output::Master, Output::Bus);
                session
                    .set_track_output(track, output)
                    .map_err(|e| e.to_string())?;
            }
            Operation::AddSend => {
                let destination = track_by_name(
                    session.project(),
                    args.destination.as_deref().expect("validated destination"),
                )?
                .id;
                if session
                    .project()
                    .track(track)
                    .expect("resolved track")
                    .sends
                    .iter()
                    .any(|send| send.target == destination)
                {
                    return Err("this track already sends to that bus; use routing send_level for its level or send_mode for its tap".into());
                }
                let send = session
                    .add_send(track, destination)
                    .map_err(|e| e.to_string())?;
                session
                    .set_send_level(track, send, args.level_db.unwrap_or(0.0))
                    .map_err(|e| e.to_string())?;
                session
                    .set_send_pre_fader(track, send, args.pre_fader.unwrap_or(false))
                    .map_err(|e| e.to_string())?;
            }
            Operation::RemoveSend => {
                let send = existing_send(&session, track, args)?;
                session
                    .remove_send(track, send)
                    .map_err(|e| e.to_string())?;
            }
            Operation::SendMode => {
                let send = existing_send(&session, track, args)?;
                session
                    .set_send_pre_fader(track, send, args.pre_fader.expect("validated tap"))
                    .map_err(|e| e.to_string())?;
            }
            Operation::SendLevel => {
                let send = existing_send(&session, track, args)?;
                session
                    .set_send_level(track, send, args.level_db.expect("validated level"))
                    .map_err(|e| e.to_string())?;
            }
        }
        if !matches!(args.operation, Operation::List) {
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        Ok(describe(&session, track))
    }
}

/// Sets a track's mute and solo switches.
pub mod set_track_state {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "set_track_state";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Sets a track's mute and/or solo state and saves with a checkpoint. Supply mute, solo, or both; omitted switches stay unchanged. Solo is additive: other soloed tracks remain soloed. Returns the actual switches and all currently soloed tracks. Use mixer to inspect the whole mix.";

    /// Explicit switch values for one track.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Track name or stable id:N selector from describe; the master has no mute/solo switches.
        pub track: String,
        /// True silences the track; false unmutes it. Omit to keep the current value.
        pub mute: Option<bool>,
        /// True adds this track to the solo selection; false removes it. Omit to keep it.
        pub solo: Option<bool>,
    }

    /// Changes the requested switches, preserving every other track's state.
    pub fn run(args: &Args) -> Result<String, String> {
        if args.mute.is_none() && args.solo.is_none() {
            return Err("supply mute, solo, or both; use mixer to read the current state".into());
        }
        let mut session = opened(&args.project)?;
        let id = track_by_name(session.project(), &args.track)?.id;
        if let Some(mute) = args.mute {
            session
                .set_track_mute(id, mute)
                .map_err(|e| e.to_string())?;
        }
        if let Some(solo) = args.solo {
            session
                .set_track_solo(id, solo)
                .map_err(|e| e.to_string())?;
        }
        session.save_with_checkpoint().map_err(|e| e.to_string())?;
        let track = session.project().track(id).expect("resolved track");
        let soloed: Vec<_> = session
            .project()
            .tracks
            .iter()
            .filter(|track| track.mixer.solo)
            .map(|track| serde_json::json!({"track":format!("id:{}",track.id.0),"name":track.name}))
            .collect();
        Ok(serde_json::json!({"track":format!("id:{}",id.0),"name":track.name,"mute":track.mixer.mute,"solo":track.mixer.solo,"soloed_tracks":soloed}).to_string())
    }
}

/// Changes one instrument parameter without creating an automation lane.
pub mod set_instrument_param {
    use super::*;

    /// The tool's wire name.
    pub const NAME: &str = "set_instrument_param";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Sets one instrument parameter's static value and saves with a checkpoint. Discover exact parameter keys, units and ranges using automation with target {kind:instrument} and operation {action:read}. Values use those units; invalid ranges and fractional discrete choices are refused. Existing automation is preserved and reported because it overrides the static value during playback.";

    /// One parameter value in its native units.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Instrument track name or stable id:N selector from describe.
        pub track: String,
        /// Exact parameter key discovered with automation's instrument target and read operation.
        pub param: String,
        /// Value in parameter units, within the discovered range. Discrete choices require a valid step.
        pub value: f32,
    }

    /// Validates the exact parameter and value before changing the saved document.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = track_by_name(session.project(), &args.track)?;
        if track.kind.as_instrument().is_none() {
            return Err("set_instrument_param requires an instrument track".into());
        }
        let id = track.id;
        let descriptors = session.instrument_descriptors(id);
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.key.as_ref() == args.param)
            .ok_or_else(|| {
                format!(
                    "unknown instrument parameter '{}'; available keys: {}",
                    args.param,
                    descriptors
                        .iter()
                        .map(|descriptor| descriptor.key.as_ref())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        if !(descriptor.min..=descriptor.max).contains(&args.value) {
            return Err(format!(
                "{} must be between {} and {} ({:?})",
                descriptor.key, descriptor.min, descriptor.max, descriptor.unit
            ));
        }
        let clamped = descriptor.clamp(args.value);
        let tolerance = f32::EPSILON * args.value.abs().max(clamped.abs()).max(1.0) * 4.0;
        if (clamped - args.value).abs() > tolerance {
            return Err(format!(
                "{} requires a discrete step; the nearest valid value is {}",
                descriptor.key, clamped
            ));
        }
        let target = ParamTarget::Instrument {
            track: id,
            param: descriptor.id,
        };
        let previous = session.param_value(target, descriptor);
        session.set_param(target, args.value);
        session.save_with_checkpoint().map_err(|e| e.to_string())?;
        let automated = session.is_automated(target);
        Ok(serde_json::json!({
            "track":format!("id:{}",id.0),"param":descriptor.key,"previous":previous,
            "value":session.param_value(target, descriptor),"unit":format!("{:?}",descriptor.unit),
            "automated":automated,
            "warning":automated.then_some("The existing automation lane overrides this static value during playback; use automation to edit or clear it.")
        }).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    struct Fixture {
        root: PathBuf,
        path: String,
        lead: TrackId,
        bus: TrackId,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("auris-track-tools-{}-{label}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let lead = session.add_default_instrument_track("Lead").unwrap();
            let bus = session.add_bus_track("Reverb");
            let path = root.join("Song.auris");
            session.save(&path).unwrap();
            Self {
                root,
                path: path.to_string_lossy().into_owned(),
                lead,
                bus,
            }
        }

        fn route(&self, request: Value) -> Result<Value, String> {
            let mut request = request;
            request["project"] = self.path.clone().into();
            if request.get("track").is_none() {
                request["track"] = format!("id:{}", self.lead.0).into();
            }
            let args = serde_json::from_value(request).unwrap();
            routing::run(&args).map(|answer| serde_json::from_str(&answer).unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn conversion_tool_saves_audio_with_a_checkpoint_of_the_score() {
        let fixture = Fixture::new("conversion");
        let mut session = opened(&fixture.path).unwrap();
        let clip = session
            .add_midi_clip(fixture.lead, "Phrase", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.save(std::path::Path::new(&fixture.path)).unwrap();
        let args = convert_track_to_audio::Args {
            project: fixture.path.clone(),
            track: format!("id:{}", fixture.lead.0),
        };
        let response: Value =
            serde_json::from_str(&convert_track_to_audio::run(&args).unwrap()).unwrap();
        assert_eq!(response["kind"], "audio");
        let reopened = opened(&fixture.path).unwrap();
        assert!(
            reopened
                .project()
                .track(fixture.lead)
                .unwrap()
                .kind
                .as_audio()
                .is_some()
        );
        assert!(!reopened.checkpoints().unwrap().is_empty());
        let saved = std::fs::read(&fixture.path).unwrap();
        assert!(convert_track_to_audio::run(&args).is_err());
        assert_eq!(std::fs::read(&fixture.path).unwrap(), saved);
    }

    #[test]
    fn routing_round_trips_outputs_send_levels_modes_and_removal() {
        let fixture = Fixture::new("routing");
        let initial = std::fs::read(&fixture.path).unwrap();
        let listed = fixture.route(json!({"operation":"list"})).unwrap();
        assert_eq!(
            listed["available_buses"][0]["track"],
            format!("id:{}", fixture.bus.0)
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), initial);
        assert!(
            opened(&fixture.path)
                .unwrap()
                .checkpoints()
                .unwrap()
                .is_empty()
        );
        fixture
            .route(json!({"operation":"output","destination":"Reverb"}))
            .unwrap();
        let added = fixture.route(json!({"operation":"add_send","destination":"Reverb","level_db":-12.0,"pre_fader":true})).unwrap();
        let send_id = added["sends"][0]["send_id"].as_u64().unwrap();
        let reopened = opened(&fixture.path).unwrap();
        let track = reopened.project().track(fixture.lead).unwrap();
        assert_eq!(track.output, Output::Bus(fixture.bus));
        assert_eq!(track.sends.len(), 1);
        assert_eq!(track.sends[0].level_db, -12.0);
        assert!(track.sends[0].pre_fader);
        assert!(!reopened.checkpoints().unwrap().is_empty());
        let level = fixture
            .route(json!({"operation":"send_level","destination":"Reverb","level_db":-18.0}))
            .unwrap();
        assert_eq!(level["sends"][0]["level_db"], -18.0);
        assert_eq!(
            opened(&fixture.path)
                .unwrap()
                .project()
                .track(fixture.lead)
                .unwrap()
                .sends[0]
                .level_db,
            -18.0
        );
        fixture
            .route(json!({"operation":"send_mode","send_id":send_id,"pre_fader":false}))
            .unwrap();
        assert!(
            !opened(&fixture.path)
                .unwrap()
                .project()
                .track(fixture.lead)
                .unwrap()
                .sends[0]
                .pre_fader
        );
        fixture
            .route(json!({"operation":"remove_send","destination":format!("id:{}",fixture.bus.0)}))
            .unwrap();
        fixture
            .route(json!({"operation":"output","destination":"master"}))
            .unwrap();
        let reopened = opened(&fixture.path).unwrap();
        let track = reopened.project().track(fixture.lead).unwrap();
        assert_eq!(track.output, Output::Master);
        assert!(track.sends.is_empty());
    }

    #[test]
    fn routing_rejects_cycles_duplicate_sends_and_wrong_fields_without_saving() {
        let fixture = Fixture::new("routing-rejections");
        fixture
            .route(json!({"operation":"add_send","destination":"Reverb"}))
            .unwrap();
        let before = std::fs::read(&fixture.path).unwrap();
        for request in [
            json!({"operation":"add_send","destination":"Reverb"}),
            json!({"operation":"output","destination":"Lead"}),
            json!({"operation":"list","destination":"Reverb"}),
            json!({"operation":"send_mode","destination":"Reverb"}),
            json!({"operation":"add_send","destination":"Reverb","level_db":1.0}),
            json!({"operation":"send_level","destination":"Reverb"}),
            json!({"operation":"send_level","level_db":-12.0}),
            json!({"operation":"send_level","destination":"Reverb","send_id":1,"level_db":-12.0}),
            json!({"operation":"send_level","destination":"Reverb","level_db":1.0}),
            json!({"operation":"send_level","destination":"Reverb","level_db":-61.0}),
            json!({"operation":"send_level","destination":"Reverb","level_db":-12.0,"pre_fader":true}),
            json!({"operation":"send_level","send_id":u64::MAX,"level_db":-12.0}),
        ] {
            assert!(fixture.route(request).is_err());
            assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
        }
        let mut session = opened(&fixture.path).unwrap();
        let second_bus = session.add_bus_track("Delay");
        session
            .set_track_output(fixture.bus, Output::Bus(second_bus))
            .unwrap();
        session.save_in_place().unwrap();
        let before = std::fs::read(&fixture.path).unwrap();
        assert!(
            fixture
                .route(json!({"track":"Delay","operation":"output","destination":"Reverb"}))
                .is_err()
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    }

    #[test]
    fn duplicate_existing_sends_require_an_id_and_removal_clears_only_its_lane() {
        let fixture = Fixture::new("duplicate-sends");
        let mut session = opened(&fixture.path).unwrap();
        let first = session.add_send(fixture.lead, fixture.bus).unwrap();
        let second = session.add_send(fixture.lead, fixture.bus).unwrap();
        let first_target = ParamTarget::Send {
            track: fixture.lead,
            send: first,
        };
        let second_target = ParamTarget::Send {
            track: fixture.lead,
            send: second,
        };
        session
            .write_automation_points(
                first_target,
                &[AutomationPoint::new(Ticks::ZERO, -9.0)],
                true,
                None,
            )
            .unwrap();
        session
            .write_automation_points(
                second_target,
                &[AutomationPoint::new(Ticks::ZERO, -6.0)],
                true,
                None,
            )
            .unwrap();
        session.save_in_place().unwrap();
        let before = std::fs::read(&fixture.path).unwrap();
        assert!(
            fixture
                .route(json!({"operation":"remove_send","destination":"Reverb"}))
                .unwrap_err()
                .contains("send_id")
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
        assert!(
            fixture
                .route(json!({"operation":"send_level","destination":"Reverb","level_db":-24.0}))
                .unwrap_err()
                .contains("send_id")
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
        let changed = fixture
            .route(json!({"operation":"send_level","send_id":second.0,"level_db":-24.0}))
            .unwrap();
        assert_eq!(changed["sends"][0]["level_db"], 0.0);
        assert_eq!(changed["sends"][1]["level_db"], -24.0);
        assert_eq!(changed["sends"][1]["automated"], true);
        let reopened = opened(&fixture.path).unwrap();
        assert!(reopened.is_automated(first_target));
        assert!(reopened.is_automated(second_target));
        fixture
            .route(json!({"operation":"remove_send","send_id":first.0}))
            .unwrap();
        let reopened = opened(&fixture.path).unwrap();
        assert_eq!(
            reopened.project().track(fixture.lead).unwrap().sends[0].id,
            second
        );
        assert!(!reopened.is_automated(first_target));
        assert!(reopened.is_automated(second_target));
    }

    #[test]
    fn track_switches_preserve_omitted_values_and_other_soloed_tracks() {
        let fixture = Fixture::new("switches");
        let set =
            |request| set_track_state::run(&serde_json::from_value(request).unwrap()).unwrap();
        set(json!({"project":fixture.path,"track":"Reverb","solo":true}));
        set(json!({"project":fixture.path,"track":"Lead","mute":true,"solo":true}));
        let reply: Value = serde_json::from_str(&set(
            json!({"project":fixture.path,"track":"Lead","mute":false}),
        ))
        .unwrap();
        assert_eq!(reply["soloed_tracks"].as_array().unwrap().len(), 2);
        let reopened = opened(&fixture.path).unwrap();
        let lead = reopened.project().track(fixture.lead).unwrap();
        assert!(!lead.mixer.mute);
        assert!(lead.mixer.solo);
        assert!(reopened.project().track(fixture.bus).unwrap().mixer.solo);
        let before = std::fs::read(&fixture.path).unwrap();
        assert!(
            set_track_state::run(
                &serde_json::from_value(json!({"project":fixture.path,"track":"Lead"})).unwrap()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    }

    #[test]
    fn instrument_static_values_round_trip_without_replacing_automation() {
        let fixture = Fixture::new("parameters");
        let mut session = opened(&fixture.path).unwrap();
        let descriptors = session.instrument_descriptors(fixture.lead);
        let catalog = automation::run(
            &serde_json::from_value(json!({
                "project":fixture.path,"track":"Lead","target":{"kind":"instrument"},
                "operation":{"action":"read"}
            }))
            .unwrap(),
        )
        .unwrap();
        let catalog: Value = serde_json::from_str(&catalog).unwrap();
        let choice = descriptors
            .iter()
            .find(|descriptor| !descriptor.choices.is_empty())
            .unwrap();
        let listed = catalog["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|parameter| parameter["key"] == choice.key.as_ref())
            .unwrap();
        assert_eq!(listed["steps"], choice.steps.unwrap());
        assert_eq!(
            listed["choices"].as_array().unwrap().len(),
            choice.choices.len()
        );
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.steps.is_none() && descriptor.min < descriptor.max)
            .unwrap();
        let target = ParamTarget::Instrument {
            track: fixture.lead,
            param: descriptor.id,
        };
        let value = descriptor.min + (descriptor.max - descriptor.min) * 0.5;
        session
            .write_automation_points(
                target,
                &[AutomationPoint::new(Ticks::ZERO, descriptor.min)],
                true,
                None,
            )
            .unwrap();
        session.save_in_place().unwrap();
        let reply = set_instrument_param::run(
            &serde_json::from_value(
                json!({"project":fixture.path,"track":"Lead","param":descriptor.key,"value":value}),
            )
            .unwrap(),
        )
        .unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["automated"], true);
        let reopened = opened(&fixture.path).unwrap();
        assert_eq!(reopened.param_value(target, descriptor), value);
        assert_eq!(
            reopened.automation().lane(target).unwrap().points()[0].value,
            descriptor.min
        );
        let before = std::fs::read(&fixture.path).unwrap();
        for (track, param, value) in [
            ("Lead", descriptor.key.as_ref(), descriptor.max + 1.0),
            ("Lead", "missing_parameter", 0.0),
            ("Reverb", descriptor.key.as_ref(), value),
        ] {
            assert!(
                set_instrument_param::run(
                    &serde_json::from_value(
                        json!({"project":fixture.path,"track":track,"param":param,"value":value})
                    )
                    .unwrap()
                )
                .is_err()
            );
            assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
        }
        let discrete = descriptors
            .iter()
            .find(|descriptor| descriptor.steps.is_some_and(|steps| steps > 1))
            .unwrap();
        let fractional = discrete.min
            + (discrete.max - discrete.min) / (discrete.steps.unwrap() - 1) as f32 * 0.25;
        assert!(set_instrument_param::run(&serde_json::from_value(json!({"project":fixture.path,"track":"Lead","param":discrete.key,"value":fractional})).unwrap()).unwrap_err().contains("discrete"));
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    }
}
