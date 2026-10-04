//! Resolve a TOML song specification into the numeric dials used by the evaluator.
//!
//! Reads stdin, including sparse `auris compose --print` output. Defaults come
//! from the Rust model so Python never duplicates the composition format's policy.

use std::io::{self, Read};

use auris_compose::spec::SongSpec;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    let spec = SongSpec::parse(&text).map_err(|errors| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    println!(
        "{}",
        serde_json::json!({
            "humanize": spec.humanize,
            "dynamics": spec.dynamics,
            "fill": spec.fill,
            "variation": spec.variation,
            "brightness": spec.mood.brightness,
            "energy": spec.mood.energy,
            "tension": spec.mood.tension,
            "syncopation": spec.mood.syncopation,
            "tempo": spec.tempo,
            "swing": spec.swing,
        })
    );
    Ok(())
}
