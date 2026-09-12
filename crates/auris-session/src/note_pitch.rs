//! Pitch notation shared by model-facing note commands.

/// Accepted JSON pitch representations. C4 is middle C (60).
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Input {
    /// Integer MIDI key, 0 through 127.
    Midi(#[schemars(range(min = 0, max = 127))] u8),
    /// Scientific pitch name or decimal MIDI key, for example "C4" or "60".
    Name(String),
}

/// Deserialize either supported wire representation to a validated MIDI key.
pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    use serde::Deserialize;
    let input = Input::deserialize(deserializer).map_err(|e| {
        serde::de::Error::custom(format!(
            "pitch must be an integer MIDI key or a string such as 60, \"60\", or \"C4\": {e}"
        ))
    })?;
    let result = match input {
        Input::Midi(value) => parse(&value.to_string()),
        Input::Name(value) => parse(&value),
    };
    result.map_err(|e| serde::de::Error::custom(format!("pitch: {e}; use 60, \"60\", or \"C4\"")))
}

/// Deserialize either supported wire representation while retaining a string API.
pub fn deserialize_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    deserialize(deserializer).map(|value| value.to_string())
}

/// Parse a scientific pitch name or a decimal MIDI number (0 through 127).
pub fn parse(text: &str) -> Result<u8, String> {
    let text = text.trim();
    if let Ok(number) = text.parse::<i32>() {
        if (0..=127).contains(&number) {
            return Ok(number as u8);
        }
        return Err(format!("MIDI numbers run 0-127; {number} is outside that"));
    }
    let split = text
        .find(|mark: char| mark.is_ascii_digit() || mark == '-')
        .ok_or_else(|| format!("'{text}' is not a pitch — a name like \"F#4\", or 0-127"))?;
    let class = auris_core::theory::pitch::PitchClass::parse(&text[..split])
        .ok_or_else(|| format!("'{text}' is not a pitch — a name like \"F#4\", or 0-127"))?;
    let octave: i32 = text[split..]
        .parse()
        .map_err(|_| format!("'{text}' is not a pitch — a name like \"F#4\", or 0-127"))?;
    // `midi` is plain i32 arithmetic, and an octave in the hundreds of millions would overflow
    // it before the 0-127 check below could answer. MIDI lives in octaves -1 to 9; a couple
    // either side still falls through to the friendlier answer that names the number.
    if !(-4..=12).contains(&octave) {
        return Err(format!("{text} is far outside the MIDI range 0-127"));
    }
    let midi = class.midi(octave);
    u8::try_from(midi)
        .ok()
        .filter(|midi| *midi <= 127)
        .ok_or_else(|| format!("{text} is MIDI {midi}, outside 0-127"))
}
