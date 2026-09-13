//! General MIDI names and zero-based program numbers at the tool boundary.

use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(crate) enum Input {
    Program(#[schemars(range(min = 0, max = 127))] u8),
    Name(String),
}

pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let input = Option::<Input>::deserialize(deserializer).map_err(|error| {
        serde::de::Error::custom(format!(
            "sound must be a GM name or integer program 0-127, for example 81 or \"Lead 2 (sawtooth)\": {error}"
        ))
    })?;
    input
        .map(|input| match input {
            Input::Program(program) if program <= 127 => Ok(program.to_string()),
            Input::Program(_) => Err(serde::de::Error::custom("sound program must be 0-127")),
            Input::Name(name) => Ok(name),
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use serde_json::json;

    #[test]
    fn both_sound_tools_accept_numbers_names_null_and_omission() {
        for value in [
            json!(81),
            json!("81"),
            json!("Lead 2 (sawtooth)"),
            json!(null),
        ] {
            let add: add_track::Args = serde_json::from_value(json!({
                "project":"Song.auris", "name":"Lead", "kind":"instrument", "sound":value
            }))
            .unwrap();
            let set: set_instrument::Args = serde_json::from_value(json!({
                "project":"Song.auris", "track":"Lead", "sound":value
            }))
            .unwrap();
            assert_eq!(add.sound, set.sound);
            if value == json!(81) {
                assert_eq!(add.sound.as_deref(), Some("81"));
            }
        }
        let set: set_instrument::Args = serde_json::from_value(json!({
            "project":"Song.auris", "track":"Lead", "instrument":"auris.synth.square"
        }))
        .unwrap();
        assert!(set.sound.is_none());
        for value in [json!(-1), json!(128), json!(81.5), json!([81]), json!(true)] {
            assert!(
                serde_json::from_value::<set_instrument::Args>(json!({
                    "project":"Song.auris", "track":"Lead", "sound":value
                }))
                .is_err()
            );
        }
        let schema = parameter_schema::<set_instrument::Args>();
        assert!(
            schema["properties"]["sound"]
                .to_string()
                .contains("integer")
        );
    }
}
