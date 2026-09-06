//! Effect chains and parameter automation through the shared session commands.
use super::*;

fn chain(session: &Session, track: Option<TrackId>) -> &[EffectSlot] {
    match track {
        Some(id) => {
            &session
                .project()
                .track(id)
                .expect("resolved track")
                .mixer
                .effects
        }
        None => &session.project().master.effects,
    }
}

fn slot(session: &Session, track: Option<TrackId>, number: usize) -> Result<EffectSlotId, String> {
    chain(session, track)
        .get(number.wrapping_sub(1))
        .map(|s| s.id)
        .ok_or_else(|| format!("no effect slot {number}; read mixer for 1-based positions"))
}

/// Inserts, removes, reorders and connects effects.
pub mod effects {
    use super::*;
    /// The wire name.
    pub const NAME: &str = "effects";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Lists available effects and a strip's chain, or adds, removes, reorders, bypasses or sidechains an effect. Use track master for the master bus. Slots and positions are 1-based; re-read after changing order. Sidechains connect a source track to an effect that accepts one; source null disconnects. Use set_effect for static parameters and automation for curves. Changes are checkpointed and saved.";
    /// An effect operation, with the required arguments for that operation.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
    pub enum Action {
        /// Read the catalog and current chain.
        List,
        /// Append an effect by its catalog id.
        Add {
            /// Exact effect id from this tool's catalog.
            effect: String,
        },
        /// Remove one slot and its automation.
        Remove {
            /// 1-based slot number.
            slot: usize,
        },
        /// Move one slot to a new position in the chain.
        Move {
            /// 1-based slot number.
            slot: usize,
            /// New 1-based position.
            position: usize,
        },
        /// Enable or bypass a slot.
        Enable {
            /// 1-based slot number.
            slot: usize,
            /// Whether the effect processes audio.
            enabled: bool,
        },
        /// Connect or disconnect the detector input.
        Sidechain {
            /// 1-based slot number.
            slot: usize,
            /// Source track name/id; null disconnects.
            source: Option<String>,
        },
    }
    /// A strip and the requested chain operation.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Track name, id:N or master.
        pub track: String,
        /// Operation and its arguments.
        pub operation: Action,
    }
    /// Performs one operation, then returns the actual chain and catalog.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = strip_by_name(session.project(), &args.track)?;
        match &args.operation {
            Action::List => {}
            Action::Add { effect } => {
                session
                    .add_effect(track, effect)
                    .map_err(|e| e.to_string())?;
            }
            Action::Remove { slot: number } => {
                let id = slot(&session, track, *number)?;
                session.remove_effect(id);
            }
            Action::Move {
                slot: number,
                position,
            } => {
                let id = slot(&session, track, *number)?;
                if !(1..=chain(&session, track).len()).contains(position) {
                    return Err("position is outside the effect chain".into());
                }
                session.move_effect(track, id, *position as isize - *number as isize);
            }
            Action::Enable {
                slot: number,
                enabled,
            } => {
                let id = slot(&session, track, *number)?;
                session.set_effect_enabled(track, id, *enabled);
            }
            Action::Sidechain {
                slot: number,
                source,
            } => {
                let id = slot(&session, track, *number)?;
                let source = source
                    .as_ref()
                    .map(|name| track_by_name(session.project(), name).map(|t| t.id))
                    .transpose()?;
                if source.is_some() && !session.effect_wants_sidechain(track, id) {
                    return Err("this effect does not accept a sidechain input".into());
                }
                session
                    .set_effect_sidechain(track, id, source)
                    .map_err(|e| e.to_string())?;
            }
        }
        if !matches!(args.operation, Action::List) {
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        let slots = chain(&session, track).to_vec();
        let slots: Vec<_> = slots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                serde_json::json!({
                    "slot": i + 1, "effect": s.effect_id, "enabled": s.enabled,
                    "accepts_sidechain": session.effect_wants_sidechain(track, s.id),
                    "source": session.effect_sidechain(track, s.id).map(|id| format!("id:{}", id.0))
                })
            })
            .collect();
        let catalog: Vec<_> = session
            .registry()
            .effects()
            .map(|d| serde_json::json!({"id":d.id,"name":d.name}))
            .collect();
        Ok(serde_json::json!({"chain":slots,"available_effects":catalog}).to_string())
    }
}

/// Reads and writes arbitrary mixer or instrument automation.
pub mod automation {
    use super::*;
    /// The wire name.
    pub const NAME: &str = "automation";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Reads parameter keys, units, ranges, static values and automation, or writes/clears one lane. Targets include mixer gain/pan, sends, instruments and effects. Read first without param to discover keys. Points use absolute quarter-note beats from zero and parameter units. Set upserts points; replace true replaces the entire lane. Curve is linear or hold. Changes are validated before saving with a checkpoint.";
    /// Which group of parameters to inspect.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    pub enum Target {
        /// Track/master gain and pan.
        Mixer,
        /// Instrument parameters on a note track.
        Instrument,
        /// Parameters of one effect.
        Effect {
            /// 1-based slot number from mixer/effects.
            slot: usize,
        },
        /// Send gain.
        Send {
            /// Destination track name or id:N.
            destination: String,
        },
    }
    /// One automation point.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Point {
        /// Absolute quarter-note beats from the song start, starting at zero.
        pub beat: f64,
        /// Value in parameter units.
        pub value: f32,
    }
    /// Interpolation between points.
    #[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum Curve {
        /// Ramp between values.
        Linear,
        /// Hold each value until the next point.
        Hold,
    }
    /// Requested lane operation.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
    pub enum Action {
        /// Read all parameters, or just the selected key.
        Read {
            /// Exact parameter key; omit to list all keys and ranges.
            param: Option<String>,
        },
        /// Write points on one parameter.
        Set {
            /// Exact parameter key returned by read, e.g. threshold_db.
            param: String,
            /// Nonempty points, in any order. Duplicate positions are refused.
            #[schemars(length(min = 1))]
            points: Vec<Point>,
            /// Replace the entire lane instead of merging points.
            #[serde(default)]
            replace: bool,
            /// Interpolation; absent preserves the current/default curve.
            curve: Option<Curve>,
        },
        /// Remove the entire selected lane.
        Clear {
            /// Exact parameter key whose lane should be removed.
            param: String,
        },
    }
    /// A parameter group and lane operation.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// Track name, id:N or master.
        pub track: String,
        /// Parameter group.
        pub target: Target,
        /// Operation and arguments.
        pub operation: Action,
    }
    /// Resolves parameter keys, validates and applies a lane operation, and returns readback.
    pub fn run(args: &Args) -> Result<String, String> {
        let mut session = opened(&args.project)?;
        let track = strip_by_name(session.project(), &args.track)?;
        let mut targets = Vec::new();
        match &args.target {
            Target::Mixer => targets.extend([
                track.map_or(ParamTarget::MasterGain, ParamTarget::TrackGain),
                track.map_or(ParamTarget::MasterPan, ParamTarget::TrackPan),
            ]),
            Target::Send { destination } => {
                let id = track.ok_or("the master has no sends")?;
                let to = track_by_name(session.project(), destination)?.id;
                let sends: Vec<_> = session
                    .project()
                    .track(id)
                    .unwrap()
                    .sends
                    .iter()
                    .filter(|s| s.target == to)
                    .collect();
                if sends.len() != 1 {
                    return Err("expected exactly one send to this destination; read mixer".into());
                }
                targets.push(ParamTarget::Send {
                    track: id,
                    send: sends[0].id,
                });
            }
            _ => {
                let effect = match args.target {
                    Target::Effect { slot: number } => Some(slot(&session, track, number)?),
                    _ => None,
                };
                for i in 0..65_536 {
                    let target = if let Some(slot) = effect {
                        ParamTarget::Effect {
                            track,
                            slot,
                            param: ParamId(i),
                        }
                    } else {
                        ParamTarget::Instrument {
                            track: track.ok_or("the master has no instrument")?,
                            param: ParamId(i),
                        }
                    };
                    if session.descriptor_for(target).is_none() {
                        break;
                    }
                    targets.push(target);
                }
            }
        }
        let param = match &args.operation {
            Action::Read { param } => param.as_deref(),
            Action::Set { param, .. } | Action::Clear { param } => Some(param.as_str()),
        };
        let mut parameters: Vec<_> = targets
            .into_iter()
            .filter_map(|target| session.descriptor_for(target).map(|d| (target, d)))
            .filter(|(_, d)| param.is_none_or(|key| d.key.as_ref() == key))
            .collect();
        if parameters.is_empty() {
            return Err(
                "no matching parameter; read this target without param to discover keys".into(),
            );
        }
        if !matches!(args.operation, Action::Read { .. }) {
            if parameters.len() != 1 {
                return Err("set/clear requires one exact param key; read first".into());
            }
            let (target, _) = &parameters[0];
            match &args.operation {
                Action::Set {
                    points,
                    replace,
                    curve,
                    ..
                } => {
                    let points = points
                        .iter()
                        .map(|p| {
                            if !p.beat.is_finite() || !(0.0..=1_000_000.0).contains(&p.beat) {
                                return Err(
                                    "beat must be finite and between 0 and 1000000".to_string()
                                );
                            }
                            Ok(AutomationPoint::new(Ticks::from_beats(p.beat), p.value))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let curve = curve.map(|c| match c {
                        Curve::Linear => AutomationCurve::Linear,
                        Curve::Hold => AutomationCurve::Hold,
                    });
                    session
                        .write_automation_points(*target, &points, *replace, curve)
                        .map_err(|e| e.to_string())?;
                }
                Action::Clear { .. } => {
                    session.clear_automation(*target);
                }
                Action::Read { .. } => unreachable!(),
            }
            session.save_with_checkpoint().map_err(|e| e.to_string())?;
        }
        let result: Vec<_> = parameters.drain(..).map(|(target, d)| serde_json::json!({
            "key":d.key,"name":d.name,"unit":format!("{:?}",d.unit),"min":d.min,"max":d.max,
            "value":session.param_value(target, &d),"automatable":session.automatable(target).is_some(),
            "lane":session.automation().lane(target).map(|lane| serde_json::json!({
                "curve":match lane.curve { AutomationCurve::Linear => "linear", AutomationCurve::Hold => "hold" },
                "points":lane.points().iter().map(|p| serde_json::json!({"beat":p.tick.as_beats(),"value":p.value})).collect::<Vec<_>>()
            }))
        })).collect();
        Ok(serde_json::json!({"parameters":result}).to_string())
    }
}
