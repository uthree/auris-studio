use super::*;

#[test]
fn replacement_routes_and_configured_outputs_are_atomic_and_undoable() {
    let mut session = session();
    let source = session.add_default_instrument_track("Source").unwrap();
    let bus = session.add_bus_track("Bus");
    let from = format!("track:{}", source.0);
    let destination = format!("track:{}", bus.0);
    let before = session.project().clone();
    apply(
        &mut session,
        json!({"action":"set_outputs","source":from,"targets":[destination]}),
    )
    .unwrap();
    assert_eq!(
        session.project().track(source).unwrap().outputs,
        vec![Output::to_bus(bus)]
    );
    session.undo();
    assert_eq!(session.project(), &before);
    apply(
        &mut session,
        json!({"action":"add_output","source":from,"target":destination,"gain_db":-17.5,"pre_fader":true}),
    )
    .unwrap();
    let added = session.project().track(source).unwrap().outputs[1];
    assert_eq!(
        added,
        Output {
            to: Destination::Bus(bus),
            gain_db: -17.5,
            tap: Tap::PreFader,
        }
    );
    session.undo();
    assert_eq!(session.project(), &before);
    assert!(apply(&mut session,json!({"action":"add_output","source":from,"target":destination,"gain_db":-70,"pre_fader":true})).is_err());
    assert_eq!(session.project(), &before);
}

fn session() -> Session {
    Session::new(crate::SessionOptions::headless()).unwrap()
}

#[test]
fn merged_track_controls_include_master_and_validate_before_one_undoable_edit() {
    let mut session = session();
    let track = session.add_bus_track("Bus");
    let before = serde_json::to_value(session.project()).unwrap();
    let node = MixerNode(Some(track)).key();
    apply(
        &mut session,
        json!({"action":"set_strip","node":node,"gain_db":-12,"pan":0.25,"mute":true}),
    )
    .unwrap();
    let strip = &session.project().track(track).unwrap().mixer;
    assert_eq!((strip.gain_db, strip.pan, strip.mute), (-12.0, 0.25, true));
    session.undo();
    assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
    assert!(
        apply(
            &mut session,
            json!({"action":"set_strip","node":node,"gain_db":-12,"pan":9,"mute":true})
        )
        .is_err()
    );
    assert!(
        apply(
            &mut session,
            json!({"action":"set_strip","node":"track:master","gain_db":-3,"mute":true})
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
    apply(
        &mut session,
        json!({"action":"set_strip","node":"track:master","gain_db":-3}),
    )
    .unwrap();
    assert_eq!(session.project().master.gain_db, -3.0);
}
fn apply(session: &mut Session, value: Value) -> Result<Value, String> {
    session.apply_mixer_graph(&serde_json::from_value(value).unwrap())
}

#[test]
fn compact_graph_pages_are_topological_and_track_nodes_contain_faders() {
    let mut session = session();
    let source = session.add_bus_track("Source");
    let bus = session.add_bus_track("Bus");
    let slot = session
        .add_effect(Some(source), "auris.fx.compressor")
        .unwrap();
    session
        .set_track_output(source, Destination::Bus(bus))
        .unwrap();
    let graph = session.mixer_graph_report(0, Some(64)).unwrap();
    assert_eq!(
        graph["total_nodes"], 3,
        "two strips and the master; no effect nodes"
    );
    let nodes = graph["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["id"], MixerNode(Some(source)).key());
    assert!(nodes[0]["controls"]["gain_db"].is_number());
    assert_eq!(nodes[0]["effects"][0]["id"], effect_key(slot));
    assert_eq!(nodes[0]["effects"][0]["position"], 1);
    assert!(nodes[0].get("output_node").is_none());
    let order: std::collections::HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node["id"].as_str().unwrap(), i))
        .collect();
    for edge in graph["edges"].as_array().unwrap() {
        assert!(order[edge["source"].as_str().unwrap()] < order[edge["target"].as_str().unwrap()]);
    }
    let mut paged_nodes = Vec::new();
    let mut paged_edges = Vec::new();
    for offset in 0..3 {
        let page = session.mixer_graph_report(offset, Some(1)).unwrap();
        paged_nodes.extend(page["nodes"].as_array().unwrap().iter().cloned());
        paged_edges.extend(page["edges"].as_array().unwrap().iter().cloned());
    }
    assert_eq!(paged_nodes, *nodes);
    assert_eq!(paged_edges, *graph["edges"].as_array().unwrap());
}

#[test]
fn graph_actions_reject_mixed_cycles_and_effect_endpoints_without_history_changes() {
    let mut session = session();
    let a = session.add_bus_track("A");
    let b = session.add_bus_track("B");
    let slot = session.add_effect(Some(b), "auris.fx.compressor").unwrap();
    let from = MixerNode(Some(a)).key();
    let effect = effect_key(slot);
    apply(
        &mut session,
        json!({"action":"set_sidechain","effect":effect,"source":from}),
    )
    .unwrap();
    let keyed = session.mixer_graph_report(0, None).unwrap();
    assert!(
        keyed["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["kind"] == "sidechain"
                && edge["source"] == from
                && edge["target"] == MixerNode(Some(b)).key()
                && edge["effect"] == effect)
    );
    let before = serde_json::to_value(session.project()).unwrap();
    assert!(
        apply(
            &mut session,
            json!({"action":"connect","source":MixerNode(Some(b)).key(),"target":from})
        )
        .unwrap_err()
        .contains("cycle")
    );
    for (source, target) in [(&from, &effect), (&effect, &from)] {
        assert!(
            apply(
                &mut session,
                json!({"action":"connect","source":source,"target":target})
            )
            .unwrap_err()
            .contains("not a graph node")
        );
    }
    assert!(
        apply(
            &mut session,
            json!({"action":"set_sidechain","effect":effect,"source":"track:master"})
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
    session.undo();
    assert_eq!(session.effect_sidechain(Some(b), slot), None);
}

#[test]
fn effect_ids_survive_ordering_and_parameter_edits_target_the_same_slot() {
    let mut session = session();
    let track = session.add_bus_track("Bus");
    let input = MixerNode(Some(track)).key();
    let added = apply(
        &mut session,
        json!({"action":"add_effect","input":input,"effect":"auris.fx.compressor"}),
    )
    .unwrap();
    let node = added["result"]["effect"].as_str().unwrap();
    let slot = session.graph_effect(node).unwrap().1;
    session.add_effect(Some(track), "auris.fx.reverb").unwrap();
    apply(
        &mut session,
        json!({"action":"move_effect","node":node,"position":2}),
    )
    .unwrap();
    assert_eq!(
        session.project().track(track).unwrap().mixer.effects[1].id,
        slot
    );
    let params = apply(&mut session, json!({"action":"parameters","node":node})).unwrap();
    assert!(
        params["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["key"] == "threshold_db")
    );
    let set = apply(
        &mut session,
        json!({"action":"set_parameter","node":node,"param":"threshold_db","value":-18}),
    )
    .unwrap();
    assert_eq!(set["result"]["value"], -18.0);
    let before = serde_json::to_value(session.project()).unwrap();
    assert!(
        apply(
            &mut session,
            json!({"action":"set_parameter","node":node,"param":"threshold_db","value":9000})
        )
        .is_err()
    );
    assert!(
        apply(
            &mut session,
            json!({"action":"move_effect","node":node,"position":0})
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
    apply(&mut session, json!({"action":"remove_effect","node":node})).unwrap();
    assert!(
        apply(
            &mut session,
            json!({"action":"enable_effect","node":node,"enabled":true})
        )
        .is_err()
    );
}

#[test]
fn output_controls_undo_as_one_and_duplicate_connects_are_unchanged() {
    let mut session = session();
    let a = session.add_bus_track("A");
    let b = session.add_bus_track("B");
    let source = MixerNode(Some(a)).key();
    let target = MixerNode(Some(b)).key();
    let connect = json!({"action":"connect","source":source,"target":target});
    apply(&mut session, connect.clone()).unwrap();
    assert_eq!(apply(&mut session, connect).unwrap()["changed"], false);
    let before = serde_json::to_value(session.project()).unwrap();
    assert!(
        apply(
            &mut session,
            json!({"action":"set_output","source":source,"target":target,"gain_db":5,"move_pre_fader":true})
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
    apply(
        &mut session,
        json!({"action":"set_output","source":source,"target":target,"gain_db":-12,"move_pre_fader":true}),
    )
    .unwrap();
    assert_eq!(
        session.project().track(a).unwrap().outputs[1],
        Output {
            to: Destination::Bus(b),
            gain_db: -12.0,
            tap: Tap::PreFader,
        }
    );
    session.undo();
    assert_eq!(serde_json::to_value(session.project()).unwrap(), before);

    // A pre- and a post-fader output to the same bus are told apart by pre_fader.
    apply(
        &mut session,
        json!({"action":"add_output","source":source,"target":target,"gain_db":-6,"pre_fader":true}),
    )
    .unwrap();
    let ambiguous = json!({"action":"disconnect","source":source,"target":target});
    assert!(apply(&mut session, ambiguous).is_err());
    apply(
        &mut session,
        json!({"action":"disconnect","source":source,"target":target,"pre_fader":true}),
    )
    .unwrap();
    assert_eq!(
        session.project().track(a).unwrap().outputs,
        vec![Output::master(), Output::to_bus(b)]
    );
}

#[test]
fn disconnect_removes_only_the_named_output_and_allows_no_outputs() {
    let mut session = session();
    let a = session.add_bus_track("A");
    let b = session.add_bus_track("B");
    let source = MixerNode(Some(a)).key();
    let target = MixerNode(Some(b)).key();
    apply(
        &mut session,
        json!({"action":"connect","source":source,"target":target}),
    )
    .unwrap();
    let c = session.add_bus_track("C");
    assert!(
        apply(
            &mut session,
            json!({"action":"disconnect","source":source,"target":MixerNode(Some(c)).key()})
        )
        .is_err()
    );
    assert_eq!(
        session.project().track(a).unwrap().outputs,
        vec![Output::master(), Output::to_bus(b)]
    );
    apply(
        &mut session,
        json!({"action":"disconnect","source":source,"target":target}),
    )
    .unwrap();
    assert_eq!(
        session.project().track(a).unwrap().outputs,
        vec![Output::master()]
    );
    apply(
        &mut session,
        json!({"action":"disconnect","source":source,"target":"track:master"}),
    )
    .unwrap();
    assert!(session.project().track(a).unwrap().outputs.is_empty());
    session.undo();
    assert_eq!(
        session.project().track(a).unwrap().outputs,
        vec![Output::master()]
    );
}
