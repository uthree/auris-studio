use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const DISCOVERY_FILE_LIMIT: usize = 32_768;
pub(crate) const DISCOVERY_PATH_BYTE_LIMIT: usize = 8 * 1024 * 1024;
const DISCOVERY_ENTRY_LIMIT: usize = 65_536;
const DISCOVERY_DEPTH_LIMIT: usize = 64;
const DISCOVERY_TRAVERSAL_PATH_BYTE_LIMIT: usize = 16 * 1024 * 1024;

/// Installed plugin files discovered without loading third-party code.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct InstalledPluginFiles {
    /// CLAP files and bundles, sorted by path.
    pub clap: Vec<PathBuf>,
    /// VST3 files and bundles, sorted by path.
    pub vst3: Vec<PathBuf>,
    /// Whether a traversal safety limit omitted additional paths.
    pub truncated: bool,
}

#[derive(Clone, Copy)]
struct ScanLimits {
    files: usize,
    path_bytes: usize,
    entries: usize,
    traversal_path_bytes: usize,
    depth: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            files: DISCOVERY_FILE_LIMIT,
            path_bytes: DISCOVERY_PATH_BYTE_LIMIT,
            entries: DISCOVERY_ENTRY_LIMIT,
            traversal_path_bytes: DISCOVERY_TRAVERSAL_PATH_BYTE_LIMIT,
            depth: DISCOVERY_DEPTH_LIMIT,
        }
    }
}

#[derive(Default)]
struct DiscoveryBudget {
    files: usize,
    path_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScanFormats {
    clap: bool,
    vst3: bool,
}

impl ScanFormats {
    const CLAP: Self = Self {
        clap: true,
        vst3: false,
    };
    const VST3: Self = Self {
        clap: false,
        vst3: true,
    };
    const BOTH: Self = Self {
        clap: true,
        vst3: true,
    };

    fn merge(&mut self, other: Self) {
        self.clap |= other.clap;
        self.vst3 |= other.vst3;
    }
}

#[derive(Clone, Copy)]
struct ScanRootPolicy {
    formats: ScanFormats,
    report_errors: bool,
}

impl ScanRootPolicy {
    fn optional(formats: ScanFormats) -> Self {
        Self {
            formats,
            report_errors: false,
        }
    }

    fn configured(formats: ScanFormats) -> Self {
        Self {
            formats,
            report_errors: true,
        }
    }

    fn merge(&mut self, other: Self) {
        self.formats.merge(other.formats);
        self.report_errors |= other.report_errors;
    }
}

#[derive(Clone, Copy)]
enum DiscoveredFormat {
    Clap,
    Vst3,
}

fn discovered_format(path: &Path) -> Option<DiscoveredFormat> {
    let extension = path.extension()?;
    if extension.eq_ignore_ascii_case("clap") {
        Some(DiscoveredFormat::Clap)
    } else if extension.eq_ignore_ascii_case("vst3") {
        Some(DiscoveredFormat::Vst3)
    } else {
        None
    }
}

enum AddPath {
    Added,
    Duplicate,
    Rejected,
    Full,
}

pub(crate) fn plugin_path_wire_bytes(path: &Path) -> Option<usize> {
    serde_json::to_vec(path).ok().map(|path| path.len())
}

pub(crate) fn discovery_result_usage(files: &InstalledPluginFiles) -> Option<(usize, usize)> {
    let count = files.clap.len().checked_add(files.vst3.len())?;
    let bytes = files
        .clap
        .iter()
        .chain(&files.vst3)
        .try_fold(0usize, |total, path| {
            total.checked_add(plugin_path_wire_bytes(path)?)
        })?;
    Some((count, bytes))
}

impl DiscoveryBudget {
    fn add(&mut self, path: PathBuf, found: &mut BTreeSet<PathBuf>, limits: ScanLimits) -> AddPath {
        if found.contains(&path) {
            return AddPath::Duplicate;
        }
        let Some(bytes) = plugin_path_wire_bytes(&path) else {
            return AddPath::Rejected;
        };
        if self.files >= limits.files || self.path_bytes.saturating_add(bytes) > limits.path_bytes {
            return AddPath::Full;
        }
        self.files += 1;
        self.path_bytes += bytes;
        found.insert(path);
        AddPath::Added
    }
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn scan_roots_with_budget(
    roots: impl IntoIterator<Item = (PathBuf, ScanRootPolicy)>,
    limits: ScanLimits,
    budget: &mut DiscoveryBudget,
) -> Result<InstalledPluginFiles, String> {
    let mut pending = Vec::new();
    let mut visited = 0usize;
    let mut traversal_bytes = 0usize;
    let mut truncated = false;

    // One configured folder can also be a conventional format root. Merge exact duplicates before
    // walking so the same tree is classified once instead of consuming traversal budget twice.
    let mut roots_by_path = BTreeMap::<PathBuf, ScanRootPolicy>::new();
    for (root, policy) in roots {
        roots_by_path
            .entry(root)
            .and_modify(|known| known.merge(policy))
            .or_insert(policy);
    }
    let mut roots = roots_by_path.into_iter().collect::<Vec<_>>();
    roots.sort_by(|a, b| {
        a.1.report_errors
            .cmp(&b.1.report_errors)
            .then(a.0.cmp(&b.0))
    });
    // `pending` is a stack. Configured roots are pushed last so a missing or unreadable explicit
    // path is reported before a large optional system tree can consume the worker timeout.
    for (root, policy) in roots {
        let bytes = root.as_os_str().len();
        if visited >= limits.entries
            || traversal_bytes.saturating_add(bytes) > limits.traversal_path_bytes
        {
            truncated = true;
            break;
        }
        visited += 1;
        traversal_bytes += bytes;
        pending.push((root, 0usize, policy));
    }

    let mut clap = BTreeSet::new();
    let mut vst3 = BTreeSet::new();
    while let Some((path, depth, policy)) = pending.pop() {
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if policy.report_errors => {
                return Err(format!(
                    "could not read configured plugin path {}: {error}",
                    path.display()
                ));
            }
            Err(_) => continue,
        };
        // A native plugin bundle is opaque regardless of which format this root accepts. Walking
        // into (for example) a `.vst3` while looking for CLAP can expose bundled helper binaries as
        // separate plugins and spends the traversal budget on implementation details. A terminal
        // plugin symlink/reparse point is itself a valid library entry, but remains opaque so its
        // target is never traversed by discovery.
        if let Some(format) = discovered_format(&path) {
            let found = match format {
                DiscoveredFormat::Clap if policy.formats.clap => Some(&mut clap),
                DiscoveredFormat::Vst3 if policy.formats.vst3 => Some(&mut vst3),
                DiscoveredFormat::Clap | DiscoveredFormat::Vst3 => None,
            };
            if let Some(found) = found {
                match budget.add(path, found, limits) {
                    AddPath::Added | AddPath::Duplicate => {}
                    AddPath::Rejected => truncated = true,
                    AddPath::Full => {
                        truncated = true;
                        break;
                    }
                }
            }
            continue;
        }
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        if depth >= limits.depth {
            truncated = true;
            continue;
        }
        let entries = match path.read_dir() {
            Ok(entries) => entries,
            Err(error) if policy.report_errors => {
                return Err(format!(
                    "could not list configured plugin path {}: {error}",
                    path.display()
                ));
            }
            Err(_) => continue,
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if policy.report_errors => {
                    return Err(format!(
                        "could not list an entry under configured plugin path {}: {error}",
                        path.display()
                    ));
                }
                Err(_) => continue,
            };
            let path = entry.path();
            let bytes = path.as_os_str().len();
            if visited >= limits.entries
                || traversal_bytes.saturating_add(bytes) > limits.traversal_path_bytes
            {
                truncated = true;
                pending.clear();
                break;
            }
            visited += 1;
            traversal_bytes += bytes;
            pending.push((path, depth + 1, policy));
        }
    }
    Ok(InstalledPluginFiles {
        clap: clap.into_iter().collect(),
        vst3: vst3.into_iter().collect(),
        truncated,
    })
}

#[cfg(test)]
fn scan_roots_with_limits(
    roots: impl IntoIterator<Item = PathBuf>,
    extension: &str,
    limits: ScanLimits,
) -> (Vec<PathBuf>, bool) {
    let formats = if extension.eq_ignore_ascii_case("clap") {
        ScanFormats::CLAP
    } else {
        ScanFormats::VST3
    };
    let files = scan_roots_with_budget(
        roots
            .into_iter()
            .map(|root| (root, ScanRootPolicy::optional(formats))),
        limits,
        &mut DiscoveryBudget::default(),
    )
    .expect("optional test roots do not report filesystem errors");
    if formats.clap {
        (files.clap, files.truncated)
    } else {
        (files.vst3, files.truncated)
    }
}

pub(super) fn scan_clap_files(extra_paths: &[PathBuf]) -> (Vec<PathBuf>, bool) {
    let files = scan_roots_with_budget(
        super::hosted::clap_search_paths()
            .into_iter()
            .chain(extra_paths.iter().cloned())
            .map(|path| (path, ScanRootPolicy::optional(ScanFormats::CLAP))),
        ScanLimits::default(),
        &mut DiscoveryBudget::default(),
    )
    .unwrap_or_else(|error| {
        log::warn!("plugin discovery failed: {error}");
        InstalledPluginFiles::default()
    });
    (files.clap, files.truncated)
}

pub(super) fn scan_vst3_files(extra_paths: &[PathBuf]) -> (Vec<PathBuf>, bool) {
    let files = scan_roots_with_budget(
        auris_vst3::vst3_search_paths(extra_paths)
            .into_iter()
            .map(|path| (path, ScanRootPolicy::optional(ScanFormats::VST3))),
        ScanLimits::default(),
        &mut DiscoveryBudget::default(),
    )
    .unwrap_or_else(|error| {
        log::warn!("plugin discovery failed: {error}");
        InstalledPluginFiles::default()
    });
    (files.vst3, files.truncated)
}

pub(crate) fn scan_installed_plugin_files(
    extra_paths: &[PathBuf],
) -> Result<InstalledPluginFiles, String> {
    let roots = super::hosted::clap_search_paths()
        .into_iter()
        .map(|path| (path, ScanRootPolicy::optional(ScanFormats::CLAP)))
        // Extra roots are added separately below, so a configured conventional VST3 root is
        // merged with `BOTH` rather than traversed once here and once as an extra root.
        .chain(
            auris_vst3::vst3_search_paths(&[])
                .into_iter()
                .map(|path| (path, ScanRootPolicy::optional(ScanFormats::VST3))),
        )
        .chain(
            extra_paths
                .iter()
                .cloned()
                .map(|path| (path, ScanRootPolicy::configured(ScanFormats::BOTH))),
        );
    scan_roots_with_budget(
        roots,
        ScanLimits::default(),
        &mut DiscoveryBudget::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::Scratch;

    #[test]
    fn scan_accepts_case_insensitive_plugin_extensions() {
        let scratch = Scratch::new("plugin-discovery-case");
        let plugin = scratch.join("Synth.CLAP");
        std::fs::write(&plugin, b"metadata only").unwrap();

        let (found, truncated) = scan_roots_with_limits(
            [plugin.parent().unwrap().to_path_buf()],
            "clap",
            ScanLimits::default(),
        );

        assert_eq!(found, vec![plugin]);
        assert!(!truncated);
    }

    #[test]
    fn scan_stops_before_descending_past_the_depth_budget() {
        let scratch = Scratch::new("plugin-discovery-depth");
        let first = scratch.join("first");
        let second = first.join("second");
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("Synth.clap"), b"metadata only").unwrap();
        let limits = ScanLimits {
            depth: 1,
            ..ScanLimits::default()
        };

        let (found, truncated) =
            scan_roots_with_limits([first.parent().unwrap().to_path_buf()], "clap", limits);

        assert!(found.is_empty());
        assert!(truncated);
    }

    #[test]
    fn scan_stops_at_the_file_budget_without_loading_files() {
        let scratch = Scratch::new("plugin-discovery-file-limit");
        for name in ["One.vst3", "Two.vst3"] {
            std::fs::write(scratch.join(name), b"not a real plugin").unwrap();
        }
        let limits = ScanLimits {
            files: 1,
            ..ScanLimits::default()
        };

        let (found, truncated) = scan_roots_with_limits([scratch.join(".")], "vst3", limits);

        assert_eq!(found.len(), 1);
        assert!(truncated);
    }

    #[test]
    fn installed_formats_share_the_result_count_budget() {
        let scratch = Scratch::new("plugin-discovery-shared-count");
        let clap = scratch.join("One.clap");
        let vst3 = scratch.join("Two.vst3");
        std::fs::write(&clap, b"metadata only").unwrap();
        std::fs::write(&vst3, b"metadata only").unwrap();
        let limits = ScanLimits {
            files: 1,
            ..ScanLimits::default()
        };
        let files = scan_roots_with_budget(
            [(
                clap.parent().unwrap().to_path_buf(),
                ScanRootPolicy::optional(ScanFormats::BOTH),
            )],
            limits,
            &mut DiscoveryBudget::default(),
        )
        .unwrap();

        assert_eq!(files.clap.len() + files.vst3.len(), 1);
        assert!(files.truncated);
    }

    #[test]
    fn installed_formats_share_the_serialized_path_budget() {
        let scratch = Scratch::new("plugin-discovery-shared-bytes");
        let clap = scratch.join("One.clap");
        let vst3 = scratch.join("Two.vst3");
        std::fs::write(&clap, b"metadata only").unwrap();
        std::fs::write(&vst3, b"metadata only").unwrap();
        let limits = ScanLimits {
            files: 2,
            path_bytes: plugin_path_wire_bytes(&clap).unwrap(),
            ..ScanLimits::default()
        };
        let files = scan_roots_with_budget(
            [(
                clap.parent().unwrap().to_path_buf(),
                ScanRootPolicy::optional(ScanFormats::BOTH),
            )],
            limits,
            &mut DiscoveryBudget::default(),
        )
        .unwrap();

        assert_eq!(files.clap.len() + files.vst3.len(), 1);
        assert!(files.truncated);
    }

    #[test]
    fn duplicate_format_roots_are_walked_once_and_classified_together() {
        let scratch = Scratch::new("plugin-discovery-one-walk");
        let clap = scratch.join("One.clap");
        let vst3 = scratch.join("Two.vst3");
        std::fs::write(&clap, b"metadata only").unwrap();
        std::fs::write(&vst3, b"metadata only").unwrap();
        // One root plus its two children exactly fills this budget. Traversing the same root once
        // per format would consume a fourth entry and truncate one of the results.
        let limits = ScanLimits {
            entries: 3,
            ..ScanLimits::default()
        };

        let files = scan_roots_with_budget(
            [
                (
                    clap.parent().unwrap().to_path_buf(),
                    ScanRootPolicy::optional(ScanFormats::CLAP),
                ),
                (
                    vst3.parent().unwrap().to_path_buf(),
                    ScanRootPolicy::configured(ScanFormats::VST3),
                ),
            ],
            limits,
            &mut DiscoveryBudget::default(),
        )
        .unwrap();

        assert_eq!(files.clap, vec![clap]);
        assert_eq!(files.vst3, vec![vst3]);
        assert!(!files.truncated);
    }

    #[test]
    fn plugin_bundles_are_opaque_to_the_other_format() {
        let scratch = Scratch::new("plugin-discovery-opaque-bundles");
        let clap = scratch.join("Outer.clap");
        let vst3 = scratch.join("Outer.vst3");
        std::fs::create_dir_all(&clap).unwrap();
        std::fs::create_dir_all(&vst3).unwrap();
        std::fs::write(clap.join("Embedded.vst3"), b"bundle resource").unwrap();
        std::fs::write(vst3.join("Embedded.clap"), b"bundle resource").unwrap();

        let files = scan_roots_with_budget(
            [(
                clap.parent().unwrap().to_path_buf(),
                ScanRootPolicy::optional(ScanFormats::BOTH),
            )],
            ScanLimits::default(),
            &mut DiscoveryBudget::default(),
        )
        .unwrap();

        assert_eq!(files.clap, vec![clap]);
        assert_eq!(files.vst3, vec![vst3]);
        assert!(!files.truncated);
    }

    #[test]
    fn configured_missing_roots_are_reported_but_system_roots_remain_optional() {
        let scratch = Scratch::new("plugin-discovery-missing-root");
        let missing = scratch.join("not-installed");

        let optional = scan_roots_with_budget(
            [(missing.clone(), ScanRootPolicy::optional(ScanFormats::BOTH))],
            ScanLimits::default(),
            &mut DiscoveryBudget::default(),
        )
        .unwrap();
        let error = scan_roots_with_budget(
            [(
                missing.clone(),
                ScanRootPolicy::configured(ScanFormats::BOTH),
            )],
            ScanLimits::default(),
            &mut DiscoveryBudget::default(),
        )
        .unwrap_err();

        assert_eq!(optional, InstalledPluginFiles::default());
        assert!(error.contains("configured plugin path"));
        assert!(error.contains(&missing.display().to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_follow_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let plugin = outside.path().join("Outside.clap");
        std::fs::write(&plugin, b"metadata only").unwrap();
        symlink(outside.path(), root.path().join("alias")).unwrap();

        let (found, _) =
            scan_roots_with_limits([root.path().to_path_buf()], "clap", ScanLimits::default());

        assert!(!found.contains(&plugin));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_plugin_symlinks_are_listed_without_following_them() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("Target.clap");
        let linked = root.path().join("Linked.clap");
        std::fs::write(&target, b"metadata only").unwrap();
        symlink(&target, &linked).unwrap();

        let (found, truncated) =
            scan_roots_with_limits([root.path().to_path_buf()], "clap", ScanLimits::default());

        assert_eq!(found, vec![linked]);
        assert!(!truncated);
    }

    #[cfg(windows)]
    #[test]
    fn scan_does_not_follow_directory_reparse_points() {
        use std::os::windows::fs::symlink_dir;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let plugin = outside.path().join("Outside.clap");
        std::fs::write(&plugin, b"metadata only").unwrap();
        let alias = root.path().join("alias");
        if let Err(error) = symlink_dir(outside.path(), &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                return;
            }
            panic!("could not create directory symlink: {error}");
        }

        let (found, _) =
            scan_roots_with_limits([root.path().to_path_buf()], "clap", ScanLimits::default());

        assert!(!found.contains(&plugin));
        std::fs::remove_dir(alias).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn terminal_plugin_reparse_points_are_listed_without_following_them() {
        use std::os::windows::fs::symlink_dir;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("Embedded.vst3"), b"bundle resource").unwrap();
        let linked = root.path().join("Linked.vst3");
        if let Err(error) = symlink_dir(outside.path(), &linked) {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                return;
            }
            panic!("could not create directory symlink: {error}");
        }

        let (found, truncated) =
            scan_roots_with_limits([root.path().to_path_buf()], "vst3", ScanLimits::default());

        assert_eq!(found, vec![linked.clone()]);
        assert!(!truncated);
        std::fs::remove_dir(linked).unwrap();
    }
}
