//! The instruments of the open document and the controls each one is set to.
use super::Session;
use crate::param::ParamTarget;
use serde_json::{Value, json};

impl Session {
    /// Reads every track's instrument source and current static controls.
    ///
    /// Nothing is rendered or changed. Mixer and effect settings, automation lanes, audio takes
    /// and opaque plugin state are in the project and not repeated here.
    pub fn inspect_composition_state(&mut self) -> Value {
        let ids = self
            .project
            .tracks
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();
        let mut instruments = Vec::new();
        for id in ids {
            let singer_id = self
                .project
                .track(id)
                .and_then(|track| track.kind.as_singer())
                .map(|singer| singer.instrument_id.clone());
            let descriptors = match singer_id {
                Some(instrument) => self.param_descriptors(&instrument),
                None => self.instrument_descriptors(id),
            };
            let Some(track) = self.project.track(id) else {
                continue;
            };
            let (instrument_id, instrument_file, mut parameters) =
                if let Some(inner) = track.kind.as_instrument() {
                    (
                        inner.instrument_id.clone(),
                        inner.file.clone(),
                        inner.instrument_state.params.clone(),
                    )
                } else if let Some(inner) = track.kind.as_singer() {
                    (
                        inner.instrument_id.clone(),
                        None,
                        inner.instrument_state.params.clone(),
                    )
                } else {
                    (String::new(), None, Default::default())
                };
            let mut parameters_available = !descriptors.is_empty();
            for descriptor in descriptors.iter() {
                let value = if track.kind.as_instrument().is_some() {
                    self.param_value(
                        ParamTarget::Instrument {
                            track: id,
                            param: descriptor.id,
                        },
                        descriptor,
                    )
                } else {
                    parameters
                        .get(descriptor.key.as_ref())
                        .copied()
                        .unwrap_or(descriptor.default)
                };
                if value.is_finite() {
                    parameters.insert(descriptor.key.to_string(), value);
                } else {
                    parameters_available = false;
                    parameters.remove(descriptor.key.as_ref());
                }
            }
            let controls = parameters.iter().map(|(key,value)| {
                let descriptor = descriptors.iter().find(|descriptor|descriptor.key == key.as_str());
                json!({"param":key,"value":value,"name":descriptor.map(|descriptor|descriptor.name.as_ref()),
                    "min":descriptor.map(|descriptor|descriptor.min),"max":descriptor.map(|descriptor|descriptor.max),
                    "unit":descriptor.map(|descriptor|format!("{:?}",descriptor.unit)),
                    "automated":descriptor.is_some_and(|descriptor|self.is_automated(ParamTarget::Instrument{track:id,param:descriptor.id}))})
            }).collect::<Vec<_>>();
            instruments.push(json!({"track":format!("id:{}",id.0),"id":id.0,"name":track.name,
                "kind":track.kind.label(),"instrument_id":instrument_id,"file":instrument_file,"parameters":controls,
                "parameter_metadata_available":parameters_available,"soundfont_preset":self.track_preset(id),
                "voice":track.kind.as_singer().and_then(|singer|singer.voice.as_ref())}));
        }
        json!({"instruments":instruments,
            "final_state":"The .auris project holds the edited state; this reads it as it is now."})
    }
}

#[cfg(test)]
mod tests {
    use crate::SessionOptions;
    use crate::session::Session;

    #[test]
    fn inspection_reports_every_instrument_and_its_controls() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let report = session.inspect_composition_state();
        let instruments = report["instruments"].as_array().unwrap();
        let entry = instruments
            .iter()
            .find(|entry| entry["id"] == lead.0)
            .expect("the lead is reported");
        assert_eq!(entry["name"], "Lead");
        assert!(!entry["instrument_id"].as_str().unwrap().is_empty());
        assert!(
            entry["parameters"]
                .as_array()
                .is_some_and(|p| !p.is_empty())
        );
    }
}
