//! Which plugin binaries a document may make this process run.
//!
//! A hosted plugin is code, and a project names it by path. Loading whatever path the document
//! holds would make every project file an executable: a project received from somebody else could
//! point at a library shipped beside it, on the disk image it came in, or at a relative path that
//! resolves against wherever the process happened to start. So a document's plugin is loaded only
//! from a **plugin folder** — the standard CLAP and VST3 locations for this platform, plus the
//! folders and files the user listed in their settings — after every link in its path has been
//! resolved. Anything else is *blocked*: its slot stays silent, exactly as a plugin that is not
//! installed does, and the frontend is told once so it can say which folder to add.
//!
//! A library the session already has open counts as allowed. It was opened either from a plugin
//! folder or because somebody chose it through an explicit command such as
//! [`Session::hosted_plugins_in`](crate::Session::hosted_plugins_in), and refusing to reuse it
//! would not unload its code.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::Session;

impl Session {
    /// Replaces the user's own plugin folders and files, beside the standard ones.
    ///
    /// A frontend calls this whenever its plugin-folder setting changes. A plugin the document
    /// names that was refused before is tried again on the rebuild this triggers.
    pub fn set_plugin_paths(&mut self, paths: &[PathBuf]) {
        if self.plugin_trust.set_extra(paths) {
            self.invalidate_graph();
        }
    }

    /// A plugin file the document names that was not loaded because it lies outside every plugin
    /// folder, reported once per file.
    ///
    /// Its slot is silent, as a missing plugin's is. A frontend should say which file it was and
    /// that adding its folder to the plugin folders loads it.
    pub fn take_blocked_plugin(&mut self) -> Option<PathBuf> {
        self.plugin_trust.take_blocked()
    }

    /// Whether a plugin file may run on a document's or a specification's say-so: it lies in a
    /// plugin folder, or this session already has it open.
    pub(super) fn plugin_file_allowed(&mut self, file: &Path) -> bool {
        if self.hosted.has_library(file) || self.plugin_trust.allows(file) {
            return true;
        }
        self.plugin_trust.block(file);
        false
    }
}

/// The plugin folders, resolved once, and the files refused since a frontend last asked.
#[derive(Debug, Default)]
pub(crate) struct PluginTrust {
    /// The user's own plugin folders and files, as configured.
    extra: Vec<PathBuf>,
    /// Every plugin folder with its links resolved. Folders that do not exist are left out.
    roots: Vec<PathBuf>,
    /// Files refused and not yet reported.
    blocked: BTreeSet<PathBuf>,
    /// Files already reported, so a rebuild does not announce the same refusal every time.
    reported: BTreeSet<PathBuf>,
}

impl PluginTrust {
    /// Configured paths used when preparing isolated hosts against this session's policy.
    pub(super) fn extra_paths(&self) -> &[PathBuf] {
        &self.extra
    }

    /// The standard plugin folders plus `extra`.
    pub(crate) fn new(extra: &[PathBuf]) -> Self {
        let mut trust = Self::default();
        trust.set_extra(extra);
        trust
    }

    /// Replaces the user's own plugin folders. Answers whether anything changed.
    pub(super) fn set_extra(&mut self, extra: &[PathBuf]) -> bool {
        let roots = canonical_roots(
            super::hosted::clap_search_paths()
                .into_iter()
                .chain(auris_vst3::vst3_search_paths(extra)),
        );
        if self.extra == extra && self.roots == roots {
            return false;
        }
        self.extra = extra.to_vec();
        self.roots = roots;
        // A folder that has just been added may hold a plugin refused a moment ago; it gets
        // reported afresh if it is still refused.
        self.reported.clear();
        true
    }

    /// Whether `file` lies in a plugin folder.
    pub(crate) fn allows(&self, file: &Path) -> bool {
        plugin_file_is_trusted(file, &self.roots)
    }

    /// Records that `file` was refused.
    pub(super) fn block(&mut self, file: &Path) {
        if !self.reported.contains(file) {
            self.blocked.insert(file.to_path_buf());
        }
    }

    /// One refused file that has not been reported yet.
    pub(super) fn take_blocked(&mut self) -> Option<PathBuf> {
        let file = self.blocked.pop_first()?;
        self.reported.insert(file.clone());
        Some(file)
    }
}

/// The given folders with their links resolved, skipping any that cannot be.
fn canonical_roots(roots: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut resolved: Vec<PathBuf> = roots
        .into_iter()
        .filter(|root| root.is_absolute())
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    resolved.sort();
    resolved.dedup();
    resolved
}

/// Whether `file` may be loaded on a document's say-so.
///
/// It must be absolute — a relative path would mean whatever directory the process started in —
/// and, once its links are resolved, lie inside one of `roots`, which must already be resolved.
/// A root may also be a single plugin file or bundle, which then allows exactly itself.
pub(crate) fn plugin_file_is_trusted(file: &Path, roots: &[PathBuf]) -> bool {
    if !file.is_absolute() {
        return false;
    }
    let Ok(file) = std::fs::canonicalize(file) else {
        return false;
    };
    roots.iter().any(|root| file.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_files_inside_a_plugin_folder_are_trusted() {
        let folder = tempfile::tempdir().unwrap();
        let plugins = folder.path().join("Plugins");
        let elsewhere = folder.path().join("Downloads");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let inside = plugins.join("Gain.clap");
        let outside = elsewhere.join("Gain.clap");
        std::fs::write(&inside, b"plugin").unwrap();
        std::fs::write(&outside, b"plugin").unwrap();
        let roots = canonical_roots([plugins.clone()]);

        assert!(plugin_file_is_trusted(&inside, &roots));
        assert!(!plugin_file_is_trusted(&outside, &roots));
        assert!(!plugin_file_is_trusted(Path::new("Gain.clap"), &roots));
        // A path that walks out of the folder is judged by where it ends up.
        let escape = plugins.join("..").join("Downloads").join("Gain.clap");
        assert!(!plugin_file_is_trusted(&escape, &roots));
        // A file that does not exist is not trusted, whatever folder it names.
        assert!(!plugin_file_is_trusted(
            &plugins.join("Missing.clap"),
            &roots
        ));
    }

    #[test]
    fn a_configured_single_file_trusts_itself_and_nothing_beside_it() {
        let folder = tempfile::tempdir().unwrap();
        let chosen = folder.path().join("Chosen.clap");
        let neighbour = folder.path().join("Neighbour.clap");
        std::fs::write(&chosen, b"plugin").unwrap();
        std::fs::write(&neighbour, b"plugin").unwrap();
        let roots = canonical_roots([chosen.clone()]);

        assert!(plugin_file_is_trusted(&chosen, &roots));
        assert!(!plugin_file_is_trusted(&neighbour, &roots));
    }

    #[test]
    fn a_refusal_is_reported_once_until_the_folders_change() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("Gain.clap");
        std::fs::write(&file, b"plugin").unwrap();
        let mut trust = PluginTrust::new(&[]);

        assert!(!trust.allows(&file));
        trust.block(&file);
        trust.block(&file);
        assert_eq!(trust.take_blocked(), Some(file.clone()));
        assert_eq!(trust.take_blocked(), None);
        trust.block(&file);
        assert_eq!(trust.take_blocked(), None, "already reported");

        assert!(trust.set_extra(&[folder.path().to_path_buf()]));
        assert!(trust.allows(&file));
        assert!(!trust.set_extra(&[folder.path().to_path_buf()]));
    }
}
