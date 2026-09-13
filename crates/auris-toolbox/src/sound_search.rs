//! Saved-file bindings for the shared preset discovery and acoustic index.
use super::*;

/// Bounded name, library, vendor, and preset-tag search.
pub mod search_instruments {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "search_instruments";
    /// Model-facing contract.
    pub const DESCRIPTION: &str = "Search sounds by name, library, vendor or preset tags. All query words must match (case-insensitive). Includes loaded SoundFonts, built-ins, CLAP provider presets and VST3 advertised programs/standard preset files. Returns at most 50 exact sound IDs, never the full library. Pass a returned id as sound_id to add_track/set_instrument for the same project. Unsupported plugin preset discovery is reported. Prefer this over list_instruments.";
    /// Search arguments.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute project path; search the same project you will edit.
        pub project: String,
        /// Nonempty words, e.g. `saw`, `Surge bass`, or `piano`.
        pub query: String,
        /// Maximum results, 1..50; defaults to 10.
        #[serde(default = "limit")]
        #[schemars(range(min = 1, max = 50))]
        pub limit: usize,
        /// First matching result; follow next_offset with the same query.
        #[serde(default)]
        pub offset: usize,
        /// Rescan after installing/changing presets; only on offset 0. Invalidates the acoustic cache.
        #[serde(default)]
        pub refresh: bool,
    }
    /// Queries without editing or saving the project.
    pub fn run(args: &Args) -> Result<String, String> {
        opened(&args.project)?.sound_library_job(&[]).run(
            auris_session::SoundSearch::Text {
                query: args.query.clone(),
                limit: args.limit,
                offset: args.offset,
            },
            args.refresh,
        )
    }
}

/// Acoustic neighbors using the same full feature space as the timbre map.
pub mod similar_instruments {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "similar_instruments";
    /// Model-facing contract.
    pub const DESCRIPTION: &str = "Find up to n acoustic alternatives to a sound ID from search_instruments. Uses full standardized timbre vectors, not 2D map positions or names. Includes measurable built-ins, melodic SoundFonts and discovered CLAP/VST3 presets; excludes the reference. First call starts background indexing and returns status indexing with progress; continue other work then retry with the same id. Lower distance is closer, not a quality rating. Silent/failed sources are reported, drum kits are excluded.";
    /// Neighbor arguments.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Absolute path of the same project used for search_instruments.
        pub project: String,
        /// Exact id returned by search_instruments.
        pub id: String,
        /// Maximum neighbors, 1..50; defaults to 10.
        #[serde(default = "limit")]
        #[schemars(range(min = 1, max = 50))]
        pub limit: usize,
    }
    /// Queries or starts the read-only acoustic index.
    pub fn run(args: &Args) -> Result<String, String> {
        opened(&args.project)?.sound_library_job(&[]).run(
            auris_session::SoundSearch::Similar {
                id: args.id.clone(),
                limit: args.limit,
            },
            false,
        )
    }
}
fn limit() -> usize {
    10
}
