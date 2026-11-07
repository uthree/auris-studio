//! Turns `locales/*.toml` into the `Language`, `Key` and `messages` the crate re-exports.
//!
//! A key one language lacks, or a placeholder one translation names and another does not, stops
//! the build here with the file and the key named.

fn main() {
    atelier_i18n::build::Catalog::new("locales")
        .fallback("en")
        .serde(true)
        .generate()
        .unwrap_or_else(|error| panic!("{error}"));
}
