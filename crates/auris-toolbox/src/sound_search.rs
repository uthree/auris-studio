//! Saved-file bindings for the shared preset discovery and acoustic index.
use super::*;

/// Bounded name, library, vendor, and preset-tag search.
pub mod search_instruments {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "search_instruments";
    /// Model-facing contract.
    pub const DESCRIPTION: &str = "Search sounds by name, library, vendor or tags; all query words must match. Filter by source and library before paging. Returns at most 50 sound IDs for sound_id in add_track/set_instrument/setup_tracks. IDs expire on explicit refresh, a library identity change, bounded handle eviction, or server restart. Each sound.library indexes the response libraries array. Read instrument_diagnostics for scan failures.";
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
        /// Optional source and library filters.
        #[serde(default, flatten)]
        pub filter: auris_session::SoundFilter,
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
                filter: args.filter.clone(),
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
    pub const DESCRIPTION: &str = "Find acoustic alternatives to a searched sound ID using full standardized timbre vectors. Filter source/library before selecting at most 50 neighbors; excludes the reference and drum kits. First use starts indexing: continue other work and retry the same id later. Lower distance is closer, not better. Each sound.library indexes libraries. Read instrument_diagnostics for failed measurements.";
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
        /// Optional source and library filters.
        #[serde(default, flatten)]
        pub filter: auris_session::SoundFilter,
    }
    /// Queries or starts the read-only acoustic index.
    pub fn run(args: &Args) -> Result<String, String> {
        opened(&args.project)?.sound_library_job(&[]).run(
            auris_session::SoundSearch::Similar {
                id: args.id.clone(),
                limit: args.limit,
                filter: args.filter.clone(),
            },
            false,
        )
    }
}
fn limit() -> usize {
    10
}

/// On-demand bounded library diagnostics.
pub mod instrument_diagnostics {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "instrument_diagnostics";
    /// Model-facing contract.
    pub const DESCRIPTION: &str = "Read scan and acoustic measurement failures after search_instruments or similar_instruments. Does not rescan. Messages are bounded; follow next_offset.";
    /// Diagnostic page selector.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Project path or project_id.
        pub project: String,
        /// Zero-based first diagnostic.
        #[serde(default)]
        pub offset: usize,
        /// Page size, 1..16; defaults to 10.
        #[serde(default = "limit")]
        #[schemars(range(min = 1, max = 16))]
        pub limit: usize,
    }
    /// Reads an existing snapshot.
    pub fn run(args: &Args) -> Result<String, String> {
        opened(&args.project)?
            .sound_library_job(&[])
            .diagnostics(args.offset, args.limit)
    }
}
