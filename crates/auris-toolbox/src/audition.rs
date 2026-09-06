//! Range selection shared by export and short audio previews.

use super::*;

/// A contiguous part of the arrangement to audition or export.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct RenderRange {
    /// First bar, 1-based. Supply bars as well; cannot combine with section.
    pub start_bar: Option<u32>,
    /// Number of bars to render, 1-4096.
    pub bars: Option<u32>,
    /// Section label. Repeated labels require an explicit instance.
    pub section: Option<String>,
    /// Section occurrence, 1-based.
    pub instance: Option<usize>,
    /// Include effect tails; defaults to false for ranges and true for whole-song exports.
    pub include_tail: Option<bool>,
}

impl RenderRange {
    pub(crate) fn options(&self, session: &Session) -> Result<OfflineOptions, String> {
        let project = session.project();
        let range = match (&self.section, self.start_bar, self.bars) {
            (Some(label), None, None) => {
                let spans: Vec<_> = project
                    .sections
                    .spans_in(Ticks::ZERO, project.end_tick())
                    .into_iter()
                    .filter(|span| span.label.eq_ignore_ascii_case(label))
                    .filter(|span| {
                        self.instance
                            .is_none_or(|instance| span.instance == instance)
                    })
                    .collect();
                if spans.len() != 1 {
                    return Err("section must identify one occurrence; call inspect_composition and supply instance for repeated labels".into());
                }
                Some((spans[0].start, spans[0].end))
            }
            (None, Some(start), Some(bars)) if self.instance.is_none() => {
                if start == 0 {
                    return Err("start_bar is 1-based".into());
                }
                bounded_bars(bars, "render")?;
                let end = bar_after(start, bars)?;
                Some((
                    project.signatures.bar_start(start),
                    project.signatures.bar_start(end),
                ))
            }
            (None, None, None) if self.instance.is_none() => None,
            _ => return Err("choose start_bar + bars, or section + optional instance".into()),
        };
        match range {
            Some((from, to)) => session
                .render_range_options(from, to, self.include_tail.unwrap_or(false))
                .ok_or_else(|| "the requested range is outside the arrangement".into()),
            None => Ok(OfflineOptions {
                include_tail: self.include_tail.unwrap_or(true),
                ..Default::default()
            }),
        }
    }
}

/// A short WAV that MCP can return as an audio resource and the panel can attach.
pub mod preview {
    use super::*;
    /// The tool's wire name.
    pub const NAME: &str = "preview";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Renders a short WAV audition of one section or bar range (at most 120 seconds, without effect tails). Returns a local audio file; MCP also returns an audio/wav resource link readable through resources/read. Does not change the project. Use render for unrestricted exports.";
    /// Preview request.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path.
        pub project: String,
        /// The range to audition.
        #[serde(flatten)]
        pub range: RenderRange,
    }
    /// Rendered audio and its human-readable measurements.
    pub struct Preview {
        /// The immutable-by-convention output file.
        pub path: PathBuf,
        /// Length and peak measurements.
        pub text: String,
    }
    /// Renders a preview for a transport that can attach the audio itself.
    pub fn create(args: &Args) -> Result<Preview, String> {
        let mut session = opened(&args.project)?;
        let mut options = args.range.options(&session)?;
        if args.range.include_tail == Some(true) {
            return Err("preview omits effect tails; use render to include them".into());
        }
        options.include_tail = false;
        let project = session.project();
        let end = options.end_frames.unwrap_or_else(|| {
            project
                .tempo_map
                .ticks_to_samples(project.end_tick(), project.sample_rate)
                .raw()
        });
        if end.saturating_sub(options.start_frames) as f64 > project.sample_rate * 120.0 {
            return Err("preview is limited to 120 seconds; choose a shorter range".into());
        }
        // A fixed preview rate bounds resource size even for high-rate source projects.
        let source_rate = project.sample_rate;
        options.start_frames =
            (options.start_frames as f64 * 24_000.0 / source_rate).round() as u64;
        options.end_frames = Some((end as f64 * 24_000.0 / source_rate).round() as u64);
        options.sample_rate = Some(24_000.0);
        let folder = session.project_folder().ok_or("project has no folder")?;
        let directory = folder.join(".auris-previews");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        if !directory
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(folder.canonicalize().map_err(|e| e.to_string())?)
        {
            return Err("preview directory must remain inside the project folder".into());
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path = directory.join(format!(
            "preview-{now}-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let settings = WavExportSettings {
            bit_depth: WavBitDepth::Int16,
            sample_rate: 24_000,
            ..Default::default()
        };
        let summary = session
            .render_job()
            .render_to_wav(&path, &settings, &options, &mut RenderProgress::default())
            .map_err(|e| e.to_string())?;
        let text = format!(
            "{}{}",
            playback_warnings(&session),
            wrote_line(&path, &summary, &settings)
        );
        Ok(Preview { path, text })
    }
    /// Renders and names an audio file for the agent frontend.
    pub fn run(args: &Args) -> Result<String, String> {
        create(args).map(|preview| preview.text)
    }
}
