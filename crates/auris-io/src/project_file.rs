//! Saving and loading `.auris` project documents.
//!
//! Projects are stored as pretty-printed JSON. The format is text on purpose: it diffs, it
//! survives a partial recovery by hand, and the schema is small enough that the size cost of
//! JSON is irrelevant next to the audio files a session references.
//!
//! # The project folder
//!
//! A document does not sit alone. It lives in a folder of its own, alongside the audio it owns:
//!
//! ```text
//! MySong/
//!   MySong.auris
//!   Audio/
//!     kick.wav
//! ```
//!
//! The folder is what the user moves, copies, renames and archives, and it works because the
//! document refers to everything in it *relatively*. That in turn only holds while **one folder
//! holds one project** — two documents sharing a folder would share its `Audio/` directory, and
//! saving one under a new name would silently leave both pointing at the same files. Which is why
//! [`document_in_folder`] creates the folder rather than trusting anyone to.

use std::ffi::{OsStr, OsString};
use std::hash::{DefaultHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use auris_core::Project;
use serde::Deserialize;
use tempfile::NamedTempFile;

use crate::error::{IoError, Result};

/// Serialises the tiny final replace step within this process.
///
/// Windows cannot replace a destination during the instant another successful persist still has
/// that same file open. Encoding and serialisation remain concurrent; only publication is gated.
static PUBLISH_LOCK: Mutex<()> = Mutex::new(());

/// A fully serialised and synchronised project document awaiting atomic publication.
///
/// Dropping this value removes only its private sibling file. [`Self::publish`] is intentionally
/// the sole operation that can replace the visible project document, allowing a caller to
/// revalidate its live state after slow serialisation and before the short commit.
pub struct StagedProject {
    file: NamedTempFile,
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    fingerprint: u64,
}

/// An already-written and synchronised sibling file awaiting atomic publication.
///
/// Dropping this value removes the private sibling and leaves the destination unchanged.
pub struct StagedFile {
    file: NamedTempFile,
    path: PathBuf,
}

impl StagedFile {
    /// Atomically replaces the destination with the staged bytes.
    pub fn publish(self) -> Result<()> {
        publish_ready_staged_file(self.file, &self.path)
    }
}

impl StagedProject {
    /// Publishes a new checkpoint without replacing a file created by another writer.
    pub fn publish_noclobber(self) -> Result<()> {
        publish_ready_staged_file_noclobber(self.file, &self.path)
    }
    /// Modification time that the staged file keeps when it is atomically published.
    pub fn modified(&self) -> Option<std::time::SystemTime> {
        self.modified
    }

    /// Hash of the exact staged bytes, for later external-change detection without rereading.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// Atomically replaces the destination with the already-synchronised staged document.
    pub fn publish(self) -> Result<()> {
        publish_ready_staged_file(self.file, &self.path)
    }
}

/// Extension used for Auris Studio project files, without the leading dot.
pub const PROJECT_EXTENSION: &str = "auris";

/// Largest `.auris` JSON document accepted by [`load_project`], in bytes.
///
/// Sixty-four MiB leaves room for hundreds of thousands of notes and automation points while
/// bounding both the source buffer and serde's decoded allocations for an untrusted file.
pub const MAX_PROJECT_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Sub-folder of a project folder holding the audio that project owns.
pub const AUDIO_DIR: &str = "Audio";

/// Sub-folder of a project folder that agents may read and write without asking.
///
/// Nothing in the document refers into it, so it is never an asset: a file the project should
/// keep is copied into [`AUDIO_DIR`] the way any import is. It travels with the folder all the
/// same, so renders and stems made for one song stay beside it.
pub const WORKSPACE_DIR: &str = "Workspace";

/// The folder a document lives in, which is what its relative asset paths resolve against.
///
/// `None` only for a bare file name with no directory part at all.
pub fn project_folder(document: &Path) -> Option<&Path> {
    document
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

/// Where a document should be written, given the path a save dialog returned.
///
/// Choosing `/songs/MySong.auris` gives `/songs/MySong/MySong.auris`: saving under a new name
/// creates the folder that name is going to need. Choosing a path whose parent is *already*
/// named after it — which is what saving over an existing project looks like — leaves it where
/// it is rather than burrowing one level deeper each time.
///
/// The extension is *appended* when it is missing, never substituted: `with_extension` would
/// replace a final dot-suffix, so `Mix.v2` — which a Windows save dialog passes through
/// verbatim, `v2` counting as an extension — would quietly become `Mix`, and the save would
/// land on a different project's document, or on the previous version of this one.
pub fn document_in_folder(chosen: &Path) -> PathBuf {
    let already_named = chosen
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PROJECT_EXTENSION));
    let document = if already_named {
        chosen.to_path_buf()
    } else {
        let mut name = chosen.as_os_str().to_os_string();
        name.push(".");
        name.push(PROJECT_EXTENSION);
        PathBuf::from(name)
    };
    let Some(stem) = document.file_stem().map(OsString::from) else {
        return document;
    };
    match project_folder(&document) {
        Some(parent)
            if parent
                .file_name()
                .is_some_and(|name| folder_is_named(name, &stem, CASE_INSENSITIVE_PATHS)) =>
        {
            document
        }
        Some(parent) => parent
            .join(&stem)
            .join(document.file_name().unwrap_or_default()),
        None => PathBuf::from(&stem).join(document.file_name().unwrap_or_default()),
    }
}

/// Whether a path that differs only in case names the same file here.
///
/// True on the two systems the desktop application runs on, and false on the one where only the
/// command line tool does. A `cfg!` rather than a `#[cfg]` so that both answers compile — and are
/// tested — wherever this is built, which is the only way the Windows reading gets checked from a
/// Mac.
const CASE_INSENSITIVE_PATHS: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// Whether `folder` is the folder a project called `stem` already lives in.
///
/// The question [`document_in_folder`] asks to decide between leaving a document where it is and
/// making a folder for it, and the reason it is not `==`: on a case-insensitive filesystem
/// `roughmix` and `RoughMix` are one directory, so comparing them byte for byte answers "no" about
/// a folder the save is already inside. The path built from that answer does not exist — nothing
/// does, under a name only differing in case — so the guard against replacing another project
/// stays quiet as well, and renaming `roughmix` to `RoughMix` writes a whole second project, audio
/// and all, one level down inside the first.
fn folder_is_named(folder: &OsStr, stem: &OsStr, case_insensitive: bool) -> bool {
    folder == stem
        || (case_insensitive
            && folder.to_string_lossy().to_lowercase() == stem.to_string_lossy().to_lowercase())
}

/// Just enough of the schema to read the version before committing to a full parse.
#[derive(Deserialize)]
struct FormatVersionProbe {
    format_version: u32,
}

/// Creates an exclusively named sibling used to stage a document or audio export.
///
/// A sibling keeps the final rename on one filesystem. The file is created here and callers write
/// through its existing handle, so another process cannot substitute a link between choosing the
/// name and opening it.
pub(crate) fn new_staged_file(path: &Path) -> Result<NamedTempFile> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    NamedTempFile::new_in(parent).map_err(|error| IoError::from_fs(path, error))
}

/// Flushes and synchronises a staged file, then atomically replaces `path` with it.
pub(crate) fn publish_staged_file(mut staged: NamedTempFile, path: &Path) -> Result<()> {
    staged
        .flush()
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| IoError::from_fs(path, error))?;
    publish_ready_staged_file(staged, path)
}

/// Flushes and synchronises a staged file, then atomically claims a still-absent destination.
pub(crate) fn publish_staged_file_noclobber(mut staged: NamedTempFile, path: &Path) -> Result<()> {
    staged
        .flush()
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| IoError::from_fs(path, error))?;
    publish_ready_staged_file_noclobber(staged, path)
}

/// Gives a file about to be published the permissions an ordinary create would have produced.
///
/// A staged file comes from [`NamedTempFile`], which creates at `0600` — right for a private
/// sibling nobody else should read while it is half-written, and wrong for what it becomes.
/// `persist` is a rename, so the mode survives: a saved project, a rendered WAV and an exported
/// MIDI file were all owner-only, while the `.auris-write.lock` beside the document, created
/// through `OpenOptions`, honoured the umask. A deliverable readable only by the account that
/// wrote it is a surprise on a shared drive and on any machine with a second account.
///
/// The mode comes from the directory the file lands in, not from the process umask, which `std`
/// cannot read without changing it — and changing it is not thread-safe. A folder made by
/// `create_dir_all`, which is how a project folder is made, carries `0777 & ~umask`, so dropping
/// its execute bits gives exactly the `0666 & ~umask` that `File::create` would have used. A
/// folder somebody deliberately tightened gives something tighter, which is the right answer
/// there: a document should not be more open than the folder holding it.
///
/// Best effort on purpose: a filesystem that carries no Unix modes, or a directory that cannot be
/// read, is not a reason to fail a save. Does nothing on Windows, where the inherited ACL is
/// already what a created file would have.
pub fn match_folder_permissions(file: &std::fs::File, destination: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let Some(folder) = destination.parent() else {
            return;
        };
        let Ok(metadata) = std::fs::metadata(folder) else {
            return;
        };
        let mode = metadata.permissions().mode() & 0o666;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = (file, destination);
    }
}

pub(crate) fn publish_ready_staged_file_noclobber(
    staged: NamedTempFile,
    path: &Path,
) -> Result<()> {
    match_folder_permissions(staged.as_file(), path);
    match staged.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(IoError::ExportDestinationExists(path.to_path_buf()))
        }
        Err(error) => Err(IoError::from_fs(path, error.error)),
    }
}

pub(crate) fn publish_ready_staged_file(staged: NamedTempFile, path: &Path) -> Result<()> {
    let _publish = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match_folder_permissions(staged.as_file(), path);
    staged
        .persist(path)
        .map(drop)
        .map_err(|error| IoError::from_fs(path, error.error))
}

/// Serialises and synchronises `project` into a private sibling of `path` without publishing it.
///
/// This is the worker half of a two-phase save. The returned value owns the private file and
/// removes it on drop; only [`StagedProject::publish`] makes the document visible.
pub fn stage_project(path: &Path, project: &mut Project) -> Result<StagedProject> {
    project.validate_loop_expansion()?;
    project.format_version = Project::FORMAT_VERSION;
    project.saved_by = env!("CARGO_PKG_VERSION").to_string();
    let json = serde_json::to_string_pretty(project)?;
    let mut hasher = DefaultHasher::new();
    hasher.write(json.as_bytes());
    let fingerprint = hasher.finish();

    let mut file = new_staged_file(path)?;
    file.write_all(json.as_bytes())
        .and_then(|()| file.flush())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| IoError::from_fs(path, error))?;
    let modified = file
        .as_file()
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok());
    Ok(StagedProject {
        file,
        path: path.to_path_buf(),
        modified,
        fingerprint,
    })
}

/// Writes `project` to `path` as pretty-printed JSON, stamped with this build's format version.
///
/// The document is written through the already-open handle of a randomly named sibling and then
/// renamed over the target, so an interrupted save — a full disk, a lost connection to a network
/// share — leaves the previous version of the project intact. Writing straight to `path` would
/// truncate it first, and a failure after that point would destroy the user's work with no backup
/// to fall back on, since undo history lives in the application rather than on disk.
pub fn save_project(path: &Path, project: &mut Project) -> Result<()> {
    stage_project(path, project)?.publish()
}

/// Writes and synchronises `bytes` into a private sibling without publishing the destination.
///
/// This is the worker half for small non-project artifacts whose caller must revalidate live
/// state immediately before the atomic replace.
pub fn stage_file_bytes(path: &Path, bytes: &[u8]) -> Result<StagedFile> {
    let mut file = new_staged_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| IoError::from_fs(path, error))?;
    Ok(StagedFile {
        file,
        path: path.to_path_buf(),
    })
}

/// The older format versions this build reads, carried forward by `serde(default)` and aliases.
///
/// Every version bump must decide whether the version it replaces joins this range. Version 32
/// forgot to, and a project saved the day before would not open at all.
const READABLE_OLDER: std::ops::RangeInclusive<u32> = 27..=37;

/// Reads a project from `path`.
///
/// The format version must match this build, or be one of the older versions it still reads:
/// 37 and earlier with a shape on each layer's connection (see `perform_layer_shapes`), 36 and
/// earlier with connections kept on the tracks and slots they touch (see
/// `hoist_connections`), 35 and earlier with sends beside the outputs (see
/// `fold_sends_into_outputs`), 31 with each
/// track's single output, 30 without the latest extended chords, 29 without eleventh chords, 28
/// without named drum lanes, or 27, whose volume contour is defaulted.
/// After parsing, the
/// id counter is repaired, which is what stops freshly created tracks and clips from colliding
/// with ids already in the document, and so is the routing — a file whose buses feed each other
/// in a circle has no order it can be rendered in, and repairing it beats refusing to open it.
pub fn load_project(path: &Path) -> Result<Project> {
    let bytes = read_project_source(path, MAX_PROJECT_FILE_BYTES)?;

    let probe: FormatVersionProbe = serde_json::from_slice(&bytes)?;
    if probe.format_version != Project::FORMAT_VERSION
        && !READABLE_OLDER.contains(&probe.format_version)
    {
        return Err(IoError::ProjectVersionMismatch {
            found: probe.format_version,
            supported: Project::FORMAT_VERSION,
        });
    }

    let mut project: Project = if probe.format_version < SHAPES_PERFORMED {
        let mut document: serde_json::Value = serde_json::from_slice(&bytes)?;
        if probe.format_version < SENDS_FOLDED {
            fold_sends_into_outputs(&mut document);
        }
        if probe.format_version < CONNECTIONS_HOISTED {
            hoist_connections(&mut document);
        }
        perform_layer_shapes(&mut document);
        serde_json::from_value(document)?
    } else {
        serde_json::from_slice(&bytes)?
    };
    project.format_version = Project::FORMAT_VERSION;
    if let Some(id) = project.conflicting_object_id() {
        return Err(IoError::ProjectIdConflict(id));
    }
    project.validate_loop_expansion()?;
    if !project.repair_id_counter() {
        return Err(IoError::ProjectIdsExhausted);
    }
    if project.repair_curve_order() {
        log::warn!(
            "{}: pitch-bend or controller points were out of order and have been sorted",
            path.display()
        );
    }
    if project.repair_routing() {
        log::warn!(
            "{}: the routing named a bus that is not there, or looped back on itself; \
             the tracks involved now go straight to the master",
            path.display()
        );
    }
    Ok(project)
}

/// The first format version in which a track's sends are outputs like any other.
const SENDS_FOLDED: u32 = 36;

/// The first format version in which every connection lives in the project's own list.
const CONNECTIONS_HOISTED: u32 = 37;

/// The first format version in which a layer's shape is a performance node on its notes path.
const SHAPES_PERFORMED: u32 = 38;

/// Furthest a version-37 layer shape moved its notes either way: one 4/4 bar. Kept so a document
/// sounds as it did; a delay written today has no such limit.
const OLD_LAYER_OFFSET_LIMIT: i64 = 4 * 960;

/// Rewrites a version-37 routing list into today's routing, in place.
///
/// Version 37 stored the routing as a bare list of edges, and a notes edge joined two track ids
/// and carried the layer's shape. Today the routing is an object of `edges` and `performances`,
/// a notes edge joins two nodes, and a shape is a performance standing between the source and the
/// layer, made of the transforms the shape stood for, in the order it applied them: the key
/// range, the voice, the transpose, the velocity, the length and the offset. Each new performance
/// takes the next id from the document's counter. A shape that changed nothing leaves the source
/// connected straight to the layer. A routing that is already an object is today's shape stamped
/// with an older number and is left alone.
fn perform_layer_shapes(document: &mut serde_json::Value) {
    use serde_json::{Value, json};

    let Some(Value::Array(edges)) = document.get("routing").cloned() else {
        return;
    };
    let mut next_id = document.get("next_id").and_then(Value::as_u64).unwrap_or(1);
    let mut rewritten = Vec::new();
    let mut performances = Vec::new();
    for edge in edges {
        if edge.get("kind").and_then(Value::as_str) != Some("notes") {
            rewritten.push(edge);
            continue;
        }
        let from = json!({ "track": edge.get("from").cloned().unwrap_or(Value::Null) });
        let to = json!({ "track": edge.get("to").cloned().unwrap_or(Value::Null) });
        let transforms = edge.get("shape").map(shape_transforms).unwrap_or_default();
        if transforms.is_empty() {
            rewritten.push(json!({ "kind": "notes", "from": from, "to": to }));
            continue;
        }
        let id = next_id;
        next_id += 1;
        performances.push(json!({ "id": id, "transforms": transforms }));
        let node = json!({ "performance": id });
        rewritten.push(json!({ "kind": "notes", "from": from, "to": node }));
        rewritten.push(json!({ "kind": "notes", "from": node, "to": to }));
    }
    if let Some(document) = document.as_object_mut() {
        document.insert(
            "routing".into(),
            json!({ "performances": performances, "edges": rewritten }),
        );
        document.insert("next_id".into(), json!(next_id));
    }
}

/// The transforms a version-37 layer shape stood for, leaving out every field at its identity.
fn shape_transforms(shape: &serde_json::Value) -> Vec<serde_json::Value> {
    use serde_json::{Value, json};

    let number = |field: &str| shape.get(field).and_then(Value::as_f64);
    let mut transforms = Vec::new();
    let lowest = number("lowest").unwrap_or(0.0).clamp(0.0, 127.0) as u8;
    let highest = number("highest").unwrap_or(127.0).clamp(0.0, 127.0) as u8;
    if (lowest.min(highest), lowest.max(highest)) != (0, 127) {
        transforms.push(json!({"kind": "key_range", "lowest": lowest, "highest": highest}));
    }
    if let Some(voice @ ("top" | "bottom")) = shape.get("voice").and_then(Value::as_str) {
        transforms.push(json!({"kind": "voice", "voice": voice}));
    }
    let transpose = number("transpose").unwrap_or(0.0) as i64;
    if transpose != 0 {
        transforms.push(json!({"kind": "transpose", "semitones": transpose}));
    }
    let velocity = number("velocity").unwrap_or(1.0);
    if velocity != 1.0 {
        transforms.push(json!({"kind": "velocity", "scale": velocity}));
    }
    let length = number("length").unwrap_or(1.0);
    if length != 1.0 {
        transforms.push(json!({"kind": "length", "factor": length}));
    }
    let offset = (number("offset").unwrap_or(0.0) as i64)
        .clamp(-OLD_LAYER_OFFSET_LIMIT, OLD_LAYER_OFFSET_LIMIT);
    if offset != 0 {
        transforms.push(json!({"kind": "delay", "ticks": offset}));
    }
    transforms
}

/// Moves every connection off the tracks and effect slots it was written on and into the
/// project's `routing` list, in place.
///
/// Before [`CONNECTIONS_HOISTED`] a track carried its `outputs` (a missing list meant the master
/// alone), an instrument track that was a layer carried `follows`, and an effect slot keyed from a
/// track carried `sidechain`. Each becomes one edge, in that order: every track's outputs in track
/// order, then the keys, then the layers, each layer's still carrying the shape version 37 kept
/// on it. A document that already has a `routing` is left alone.
fn hoist_connections(document: &mut serde_json::Value) {
    use serde_json::{Map, Value, json};

    if document.get("routing").is_some() {
        return;
    }
    let mut outputs = Vec::new();
    let mut keys = Vec::new();
    let mut layers = Vec::new();
    let take_keys = |effects: Option<&mut Value>, keys: &mut Vec<Value>| {
        for slot in effects.and_then(Value::as_array_mut).into_iter().flatten() {
            let Some(slot) = slot.as_object_mut() else {
                continue;
            };
            if let Some(source) = slot.remove("sidechain")
                && !source.is_null()
            {
                keys.push(json!({"kind": "key", "from": source, "to": slot.get("id")}));
            }
        }
    };
    if let Some(tracks) = document.get_mut("tracks").and_then(Value::as_array_mut) {
        for track in tracks {
            let Some(track) = track.as_object_mut() else {
                continue;
            };
            let id = track.get("id").cloned().unwrap_or(Value::Null);
            let stored = track
                .remove("outputs")
                .unwrap_or_else(|| json!([{"to": "master"}]));
            for output in stored.as_array().into_iter().flatten() {
                let mut edge = Map::new();
                edge.insert("kind".into(), json!("output"));
                edge.insert("from".into(), id.clone());
                if let Some(fields) = output.as_object() {
                    edge.extend(fields.clone());
                }
                outputs.push(Value::Object(edge));
            }
            take_keys(
                track
                    .get_mut("mixer")
                    .and_then(|mixer| mixer.get_mut("effects")),
                &mut keys,
            );
            let follows = track
                .get_mut("kind")
                .and_then(Value::as_object_mut)
                .and_then(|kind| kind.remove("follows"));
            if let Some(Value::Object(source)) = follows {
                let mut edge = Map::new();
                edge.insert("kind".into(), json!("notes"));
                edge.insert(
                    "from".into(),
                    source.get("track").cloned().unwrap_or(Value::Null),
                );
                edge.insert("to".into(), id.clone());
                if let Some(shape) = source.get("shape") {
                    edge.insert("shape".into(), shape.clone());
                }
                layers.push(Value::Object(edge));
            }
        }
    }
    take_keys(
        document
            .get_mut("master")
            .and_then(|master| master.get_mut("effects")),
        &mut keys,
    );
    if let Some(document) = document.as_object_mut() {
        outputs.extend(keys);
        outputs.extend(layers);
        document.insert("routing".into(), Value::Array(outputs));
    }
}

/// Rewrites a document older than [`SENDS_FOLDED`] into today's shape, in place.
///
/// Older documents kept two lists on a track: `output` or `outputs`, unity copies after the
/// fader, written as a bare destination (`"master"`, `{"bus": 12}`) or a list of them; and
/// `sends`, each with an id, a target bus, a level and a tap point. Today both are one list of
/// outputs, each naming its destination, level and tap, at most one per destination and tap.
///
/// A send that lands where an output already lands at the same tap is the same signal arriving
/// twice, so the two are summed into one output at the combined level, which is what was heard.
/// A send's automation lane follows it to its output, unless the send was merged that way: one
/// curve cannot stand for the sum of two levels, so that lane is dropped.
///
/// Done on the JSON rather than in `serde` so that today's types carry no trace of the old shape.
fn fold_sends_into_outputs(document: &mut serde_json::Value) {
    use serde_json::{Map, Value, json};

    let decibels = |value: &Value| value.as_f64().unwrap_or(0.0);
    let mut followed: Vec<(Value, Option<Value>)> = Vec::new();
    let Some(tracks) = document.get_mut("tracks").and_then(Value::as_array_mut) else {
        return;
    };
    for track in tracks {
        let Some(track) = track.as_object_mut() else {
            continue;
        };
        let track_id = track.get("id").cloned().unwrap_or(Value::Null);
        let stored = track
            .remove("outputs")
            .or_else(|| track.remove("output"))
            .unwrap_or_else(|| json!(["master"]));
        let stored = match stored {
            Value::Array(list) => list,
            single => vec![single],
        };
        let mut outputs: Vec<Map<String, Value>> = stored
            .into_iter()
            .map(|stored| match stored {
                // Already today's shape, as a document stamped with an older number can be.
                Value::Object(output) if output.contains_key("to") => output,
                to => {
                    let mut output = Map::new();
                    output.insert("to".into(), to);
                    output
                }
            })
            .collect();
        let sends = match track.remove("sends") {
            Some(Value::Array(sends)) => sends,
            _ => Vec::new(),
        };
        for send in sends {
            let Some(bus) = send.get("target").cloned() else {
                continue;
            };
            let to = json!({ "bus": bus });
            let tap = if send.get("pre_fader").and_then(Value::as_bool) == Some(true) {
                "pre_fader"
            } else {
                "post_fader"
            };
            let level = send.get("level_db").map(decibels).unwrap_or(0.0);
            let id = send.get("id").cloned().unwrap_or(Value::Null);
            let same = outputs.iter_mut().find(|output| {
                output.get("to") == Some(&to)
                    && output
                        .get("tap")
                        .and_then(Value::as_str)
                        .unwrap_or("post_fader")
                        == tap
            });
            match same {
                Some(output) => {
                    let existing = output.get("gain_db").map(decibels).unwrap_or(0.0);
                    let sum = 10f64.powf(existing / 20.0) + 10f64.powf(level / 20.0);
                    output.insert("gain_db".into(), json!(20.0 * sum.log10()));
                    followed.push((id, None));
                }
                None => {
                    let mut output = Map::new();
                    output.insert("to".into(), to.clone());
                    output.insert("gain_db".into(), json!(level));
                    output.insert("tap".into(), json!(tap));
                    outputs.push(output);
                    let target = json!({ "Output": { "track": track_id, "to": to, "tap": tap } });
                    followed.push((id, Some(target)));
                }
            }
        }
        track.insert(
            "outputs".into(),
            Value::Array(outputs.into_iter().map(Value::Object).collect()),
        );
    }

    let Some(lanes) = document.get_mut("automation").and_then(Value::as_array_mut) else {
        return;
    };
    lanes.retain_mut(|lane| {
        let Some(send) = lane.pointer("/target/Send/send").cloned() else {
            return true;
        };
        let target = followed
            .iter()
            .find(|(id, _)| *id == send)
            .and_then(|(_, target)| target.clone());
        match target {
            Some(target) => {
                lane["target"] = target;
                true
            }
            None => false,
        }
    });
}

fn read_project_source(path: &Path, limit: usize) -> Result<Vec<u8>> {
    crate::bounded::read_with_limit(path, limit, |observed| {
        project_too_large(path, limit, observed)
    })
}

#[cfg(test)]
fn read_project_source_after_metadata(
    path: &Path,
    limit: usize,
    after_metadata: impl FnOnce(),
) -> Result<Vec<u8>> {
    crate::bounded::read_with_limit_after_metadata(path, limit, after_metadata, |observed| {
        project_too_large(path, limit, observed)
    })
}

fn project_too_large(path: &Path, limit: usize, observed: u64) -> IoError {
    IoError::ProjectFileTooLarge {
        path: path.to_path_buf(),
        observed,
        limit: limit as u64,
    }
}

#[cfg(all(test, unix))]
mod permissions_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("the file exists")
            .permissions()
            .mode()
            & 0o777
    }

    /// What `File::create` produces in `folder`, which is `0666 & ~umask` whatever the umask is.
    fn ordinary_mode(folder: &Path) -> u32 {
        let probe = folder.join(".probe");
        std::fs::File::create(&probe).expect("an ordinary file can be created");
        let mode = mode_of(&probe);
        std::fs::remove_file(&probe).expect("the probe can be removed");
        mode
    }

    #[test]
    fn a_published_document_is_as_readable_as_anything_else_in_its_folder() {
        // The staging file is 0600 and `persist` is a rename, so without
        // `match_folder_permissions` a saved project came out owner-only while the
        // `.auris-write.lock` beside it honoured the umask. Compared against a file created the
        // ordinary way in the same folder rather than against a literal mode, so the assertion
        // holds under any umask the developer or the runner happens to have.
        let root = tempfile::tempdir().expect("a temp directory");
        let folder = root.path().join("Song");
        std::fs::create_dir_all(&folder).expect("the project folder can be made");
        let expected = ordinary_mode(&folder);

        let document = folder.join("Song.auris");
        let mut project = Project::new("Song", 48_000.0);
        save_project(&document, &mut project).expect("the project saves");

        assert_eq!(
            mode_of(&document),
            expected,
            "a saved project is not as readable as its neighbours"
        );
    }

    #[test]
    fn publishing_bytes_is_as_readable_too() {
        // The same path for the small artifacts: an exported MIDI file, a report.
        let root = tempfile::tempdir().expect("a temp directory");
        let folder = root.path().join("Export");
        std::fs::create_dir_all(&folder).expect("the folder can be made");
        let expected = ordinary_mode(&folder);

        let target = folder.join("notes.mid");
        stage_file_bytes(&target, b"not really a MIDI file")
            .expect("the bytes stage")
            .publish()
            .expect("the bytes publish");

        assert_eq!(mode_of(&target), expected, "a published file is owner-only");
    }

    #[test]
    fn a_tightened_folder_is_not_widened_by_a_save() {
        // A folder somebody made private keeps its documents private: the mode comes from the
        // folder, so this is tighter than `File::create` would have been, on purpose.
        let root = tempfile::tempdir().expect("a temp directory");
        let folder = root.path().join("Private");
        std::fs::create_dir_all(&folder).expect("the folder can be made");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))
            .expect("the folder can be tightened");

        let document = folder.join("Private.auris");
        let mut project = Project::new("Private", 48_000.0);
        save_project(&document, &mut project).expect("the project saves");

        assert_eq!(mode_of(&document), 0o600, "a private folder was widened");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempFile;
    use auris_core::{AssetPath, Note, Ticks};
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::{Arc, Barrier};

    fn legacy_predictable_scratch_path(path: &Path) -> PathBuf {
        let mut name = path
            .file_name()
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from("project"));
        name.push(format!(".{}.saving", std::process::id()));
        path.with_file_name(name)
    }

    fn demo_project() -> Project {
        let mut project = Project::new("Demo", 48_000.0);
        project.set_bpm(128.0);
        let lead = project.add_instrument_track("Lead", "auris.synth.pulse");
        let clip = project
            .add_midi_clip(lead, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
        midi.notes
            .push(Note::new(67, Ticks::QUARTER, Ticks::QUARTER));

        let drums = project.add_audio_track("Drums");
        let source = project.add_audio_source(
            "loop",
            AssetPath::inside("Audio/loop.wav"),
            96_000,
            48_000.0,
            2,
        );
        project.add_soundfont("GM", AssetPath::external("/libraries/GM.sf2"), 148_345_812);
        project.add_audio_clip(drums, source, Ticks::ZERO).unwrap();
        project.add_effect(Some(drums), "auris.fx.gain").unwrap();
        project.add_effect(None, "auris.fx.limiter").unwrap();
        project
    }

    #[test]
    fn the_extension_has_no_leading_dot() {
        assert_eq!(PROJECT_EXTENSION, "auris");
        assert!(!PROJECT_EXTENSION.starts_with('.'));
    }

    #[test]
    fn project_file_size_limit_accepts_its_edge_and_rejects_the_next_byte() {
        const LIMIT: usize = 32;
        for length in [LIMIT - 1, LIMIT] {
            let file = TempFile::new(&format!("project-{length}.auris"));
            std::fs::write(file.path(), vec![b' '; length]).unwrap();
            assert_eq!(
                read_project_source(file.path(), LIMIT).unwrap().len(),
                length
            );
        }

        let file = TempFile::new("project-too-large.auris");
        std::fs::write(file.path(), vec![b' '; LIMIT + 1]).unwrap();
        assert!(matches!(
            read_project_source(file.path(), LIMIT),
            Err(IoError::ProjectFileTooLarge {
                observed,
                limit,
                ..
            }) if observed == (LIMIT + 1) as u64 && limit == LIMIT as u64
        ));
    }

    #[test]
    fn a_project_that_grows_after_metadata_is_still_bounded() {
        const LIMIT: usize = 32;
        let file = TempFile::new("growing.auris");
        std::fs::write(file.path(), vec![b' '; LIMIT]).unwrap();

        let result = read_project_source_after_metadata(file.path(), LIMIT, || {
            let mut append = OpenOptions::new().append(true).open(file.path()).unwrap();
            append.write_all(b"x").unwrap();
            append.flush().unwrap();
        });

        assert!(matches!(
            result,
            Err(IoError::ProjectFileTooLarge { observed, .. })
                if observed == (LIMIT + 1) as u64
        ));
    }

    #[test]
    fn saving_under_a_new_name_creates_the_folder_that_name_needs() {
        assert_eq!(
            document_in_folder(Path::new("/songs/MySong.auris")),
            PathBuf::from("/songs/MySong/MySong.auris")
        );
    }

    #[test]
    fn saving_over_a_project_leaves_it_where_it_is() {
        // Otherwise every save would bury the document one directory deeper than the last.
        let settled = Path::new("/songs/MySong/MySong.auris");
        assert_eq!(document_in_folder(settled), PathBuf::from(settled));
        assert_eq!(
            document_in_folder(&document_in_folder(Path::new("/songs/MySong.auris"))),
            PathBuf::from("/songs/MySong/MySong.auris"),
            "applying the rule twice must reach the same place as applying it once"
        );
    }

    #[test]
    fn a_name_typed_without_an_extension_still_makes_a_project() {
        assert_eq!(
            document_in_folder(Path::new("/songs/MySong")),
            PathBuf::from("/songs/MySong/MySong.auris")
        );
    }

    #[test]
    fn a_dotted_name_keeps_its_dots() {
        // `with_extension` would replace `.v2`, collapsing `Mix.v2` onto `Mix` — a different
        // project, or the previous version of this one, silently saved over. The Windows save
        // dialog passes such a name through verbatim, since `v2` counts as an extension.
        assert_eq!(
            document_in_folder(Path::new("/songs/Mix.v2")),
            PathBuf::from("/songs/Mix.v2/Mix.v2.auris")
        );
        assert_eq!(
            document_in_folder(Path::new("/songs/Mix.v2.auris")),
            PathBuf::from("/songs/Mix.v2/Mix.v2.auris")
        );
    }

    #[test]
    fn the_extension_is_recognised_whatever_its_case() {
        // A document renamed to `.AURIS` elsewhere is still this application's file, not a
        // name to hang another `.auris` off the end of.
        assert_eq!(
            document_in_folder(Path::new("/songs/MySong.AURIS")),
            PathBuf::from("/songs/MySong/MySong.AURIS")
        );
    }

    #[test]
    fn the_folder_of_a_document_is_what_its_assets_resolve_against() {
        assert_eq!(
            project_folder(Path::new("/songs/MySong/MySong.auris")),
            Some(Path::new("/songs/MySong"))
        );
        assert_eq!(project_folder(Path::new("MySong.auris")), None);
    }

    #[test]
    fn a_project_round_trips_through_a_file() {
        let file = TempFile::new("round-trip.auris");
        let mut project = demo_project();
        save_project(file.path(), &mut project).unwrap();

        let loaded = load_project(file.path()).unwrap();
        assert_eq!(loaded, project);
        assert_eq!(loaded.bpm(), 128.0);
        assert_eq!(loaded.tracks.len(), 2);
        assert_eq!(loaded.audio_sources.len(), 1);
        assert_eq!(loaded.master.effects.len(), 1);
    }

    #[test]
    fn many_individually_safe_loops_round_trip_without_a_document_size_cap() {
        let file = TempFile::new("large-safe-loops.auris");
        let mut project = Project::new("Large safe arrangement", 48_000.0);
        let midi_track = project.add_instrument_track("Dense", "auris.synth.pulse");
        for index in 0..6 {
            let id = project
                .add_midi_clip(midi_track, format!("part {index}"), Ticks(index), Ticks(1))
                .unwrap();
            let clip = project.midi_clip_mut(id).unwrap();
            clip.notes = (0..1_000)
                .map(|_| Note::new(60, Ticks::ZERO, Ticks(1)))
                .collect();
            clip.loop_end = Ticks(400);
        }
        let audio_track = project.add_audio_track("Loops");
        let source = project.add_audio_source(
            "one frame",
            AssetPath::inside("Audio/one.wav"),
            1,
            48_000.0,
            2,
        );
        for _ in 0..7 {
            let id = project
                .add_audio_clip(audio_track, source, Ticks::ZERO)
                .unwrap();
            project.audio_clip_mut(id).unwrap().loop_end = Ticks(16_384);
        }

        save_project(file.path(), &mut project).unwrap();
        let loaded = load_project(file.path()).unwrap();

        assert_eq!(loaded, project);
        assert!(loaded.validate_loop_expansion().is_ok());
    }

    #[test]
    fn the_saved_file_is_pretty_printed_json() {
        let file = TempFile::new("pretty.auris");
        save_project(file.path(), &mut demo_project()).unwrap();
        let text = std::fs::read_to_string(file.path()).unwrap();
        assert!(text.lines().count() > 20, "file was written on one line");
        assert!(text.contains("\n  \"name\": \"Demo\""));
    }

    #[test]
    fn ids_handed_out_after_loading_do_not_collide() {
        let file = TempFile::new("ids.auris");
        let mut project = demo_project();
        save_project(file.path(), &mut project).unwrap();

        let mut loaded = load_project(file.path()).unwrap();
        let mut used: Vec<u64> = Vec::new();
        for track in &loaded.tracks {
            used.push(track.id.0);
            for slot in &track.mixer.effects {
                used.push(slot.id.0);
            }
            if let Some(clips) = track.kind.note_clips() {
                used.extend(clips.iter().map(|clip| clip.id.0));
            }
            if let Some(inner) = track.kind.as_audio() {
                used.extend(inner.clips.iter().map(|clip| clip.id.0));
            }
        }
        used.extend(loaded.master.effects.iter().map(|slot| slot.id.0));
        used.extend(loaded.audio_sources.keys().map(|id| id.0));
        used.extend(loaded.soundfonts.keys().map(|id| id.0));
        // Two tracks, one MIDI clip, one audio source, one SoundFont, one audio clip and two
        // effect slots.
        assert_eq!(used.len(), 8, "demo project should hand out 8 ids");

        let fresh: Vec<u64> = vec![
            loaded.add_audio_track("New").0,
            loaded.next_clip_id().0,
            loaded.next_effect_slot_id().0,
        ];
        for id in &fresh {
            assert!(!used.contains(id), "id {id} was reused after loading");
        }
        assert_eq!(fresh[1], fresh[0] + 1);
        assert_eq!(fresh[2], fresh[1] + 1);
    }

    #[test]
    fn a_document_that_exhausts_the_id_space_is_rejected() {
        let file = TempFile::new("exhausted-ids.auris");
        let mut project = demo_project();
        project.tracks[0].id.0 = u64::MAX;
        std::fs::write(file.path(), serde_json::to_string_pretty(&project).unwrap()).unwrap();

        assert!(matches!(
            load_project(file.path()),
            Err(IoError::ProjectIdsExhausted)
        ));
    }

    #[test]
    fn a_document_with_duplicate_object_ids_is_rejected() {
        let file = TempFile::new("duplicate-ids.auris");
        let mut project = demo_project();
        project.tracks[1].id = project.tracks[0].id;
        std::fs::write(file.path(), serde_json::to_string_pretty(&project).unwrap()).unwrap();

        assert!(matches!(
            load_project(file.path()),
            Err(IoError::ProjectIdConflict(id)) if id == project.tracks[0].id.0
        ));
    }

    #[test]
    fn a_document_whose_loop_would_exhaust_the_event_budget_is_rejected() {
        let file = TempFile::new("loop-expansion.auris");
        let mut project = demo_project();
        let clip = project.tracks[0]
            .kind
            .note_clips_mut()
            .unwrap()
            .first_mut()
            .unwrap();
        clip.length = Ticks(1);
        clip.notes = (0..1_000)
            .map(|_| Note::new(60, Ticks::ZERO, Ticks(1)))
            .collect();
        clip.loop_end = Ticks(501);
        std::fs::write(file.path(), serde_json::to_string_pretty(&project).unwrap()).unwrap();

        let error = load_project(file.path()).expect_err("the loop expansion must be rejected");
        assert!(error.to_string().contains("reduce its notes or repeats"));
    }

    #[test]
    fn the_writer_refuses_a_project_its_own_loader_would_reject() {
        let file = TempFile::new("unsafe-loop-save.auris");
        let mut project = demo_project();
        let clip = project.tracks[0]
            .kind
            .note_clips_mut()
            .unwrap()
            .first_mut()
            .unwrap();
        clip.length = Ticks(1);
        clip.loop_end = Ticks(i64::MAX);

        let error = save_project(file.path(), &mut project)
            .expect_err("saving must enforce every invariant loading enforces");

        assert!(error.to_string().contains("loop passes"));
        assert!(!file.path().exists());
    }

    #[test]
    fn saving_replaces_an_existing_file_and_leaves_no_scratch_behind() {
        let folder = TempFile::new("overwrite-project");
        std::fs::create_dir(folder.path()).unwrap();
        let path = folder.path().join("overwrite.auris");
        let mut project = demo_project();
        save_project(&path, &mut project).unwrap();

        project.name = "Renamed".into();
        project.set_bpm(90.0);
        save_project(&path, &mut project).unwrap();

        let loaded = load_project(&path).unwrap();
        assert_eq!(loaded.name, "Renamed");
        assert_eq!(loaded.bpm(), 90.0);
        assert_eq!(loaded, project);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_preplaced_predictable_scratch_link_cannot_damage_another_file() {
        let file = TempFile::new("preserved.auris");
        let mut project = demo_project();
        save_project(file.path(), &mut project).unwrap();

        let victim = TempFile::new("project-save-victim.txt");
        let sentinel = b"this file must not be opened or truncated by a project save";
        std::fs::write(victim.path(), sentinel).unwrap();
        let scratch = legacy_predictable_scratch_path(file.path());
        std::fs::hard_link(victim.path(), &scratch).unwrap();

        let mut replacement = demo_project();
        replacement.name = "Replacement".into();
        let result = save_project(file.path(), &mut replacement);
        let victim_after = std::fs::read(victim.path()).unwrap();
        // The hardened writer deliberately ignores this old, attacker-controlled name. Remove
        // the link before making assertions so a failing test does not leak it into the temp dir.
        let _ = std::fs::remove_file(&scratch);

        result.unwrap();
        assert_eq!(victim_after, sentinel);
        assert_eq!(load_project(file.path()).unwrap(), replacement);
    }

    #[test]
    fn simultaneous_saves_to_one_path_publish_complete_documents() {
        const WRITERS: usize = 12;

        let file = TempFile::new("parallel-save.auris");
        let start = Arc::new(Barrier::new(WRITERS));
        let mut writers = Vec::with_capacity(WRITERS);
        for index in 0..WRITERS {
            let path = file.path().to_path_buf();
            let start = Arc::clone(&start);
            writers.push(std::thread::spawn(move || {
                let mut project = demo_project();
                project.name = format!("writer-{index}-{}", "x".repeat(250_000));
                start.wait();
                save_project(&path, &mut project).map(|()| project.name)
            }));
        }

        let names = writers
            .into_iter()
            .map(|writer| writer.join().expect("save thread panicked"))
            .collect::<Result<Vec<_>>>()
            .expect("independent save attempts must not collide through a shared scratch file");
        let loaded = load_project(file.path()).unwrap();

        assert!(names.contains(&loaded.name));
    }

    #[test]
    fn saving_into_a_missing_directory_reports_the_target_path() {
        let path = std::env::temp_dir()
            .join("auris-io-no-such-directory")
            .join("project.auris");
        match save_project(&path, &mut demo_project()) {
            Err(IoError::FileNotFound(reported)) => assert_eq!(reported, path),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn different_format_versions_are_rejected_before_parsing_the_document() {
        let file = TempFile::new("version.auris");
        for version in [0, 1, 26, Project::FORMAT_VERSION + 1] {
            std::fs::write(
                file.path(),
                format!(r#"{{"format_version": {version}, "tracks": "unsupported shape"}}"#),
            )
            .unwrap();
            match load_project(file.path()) {
                Err(IoError::ProjectVersionMismatch { found, supported }) => {
                    assert_eq!(found, version);
                    assert_eq!(supported, Project::FORMAT_VERSION);
                }
                other => panic!("expected a version mismatch, got {other:?}"),
            }
        }
    }

    #[test]
    fn every_older_readable_version_opens_rather_than_being_refused() {
        // Version 32 was added without widening the range, which refused every project the
        // previous build had saved. A current document stamped with each older number must at
        // least get past the version gate.
        let file = TempFile::new("older.auris");
        let mut value = serde_json::to_value(demo_project()).unwrap();
        for version in READABLE_OLDER {
            value["format_version"] = serde_json::json!(version);
            std::fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();
            let loaded = load_project(file.path())
                .unwrap_or_else(|error| panic!("version {version} was refused: {error}"));
            assert_eq!(loaded.format_version, Project::FORMAT_VERSION);
        }
        assert_eq!(
            *READABLE_OLDER.end(),
            Project::FORMAT_VERSION - 1,
            "decide whether the version a bump replaces is still readable"
        );
    }

    #[test]
    fn version_36_moves_outputs_keys_and_layers_into_the_routing_and_shapes_into_performances() {
        let file = TempFile::new("version-36.auris");
        let mut expected = demo_project();
        let bus = expected.add_bus_track("Bus");
        let lead = expected.add_instrument_track("Lead", "auris.synth.pulse");
        let layer = expected
            .add_layer_track(lead, "Pad", "auris.synth.pulse")
            .unwrap();
        let performance = expected.add_performance(layer).unwrap();
        expected
            .routing
            .performance_mut(performance)
            .unwrap()
            .transforms = vec![auris_core::NoteTransform::Transpose { semitones: 12 }];
        expected.routing.set_outputs(
            lead,
            &[
                auris_core::Output::master(),
                auris_core::Output {
                    to: auris_core::Destination::Bus(bus),
                    gain_db: -9.0,
                    tap: auris_core::Tap::PreFader,
                },
            ],
        );
        let slot = expected
            .add_effect(Some(bus), "auris.fx.compressor")
            .unwrap();
        expected.routing.set_key(slot, Some(lead));

        // Written the way version 36 wrote it: every connection on the track or slot it touches.
        let mut value = serde_json::to_value(&expected).unwrap();
        value["format_version"] = serde_json::json!(36);
        value.as_object_mut().unwrap().remove("routing");
        for stored in value["tracks"].as_array_mut().unwrap() {
            let id = auris_core::TrackId(stored["id"].as_u64().unwrap());
            stored["outputs"] = serde_json::to_value(expected.outputs(id)).unwrap();
            // A layer named its source and carried its shape, which version 38 turns into a
            // performance standing between the two.
            if let Some(source) = expected.layer_source(id) {
                stored["kind"]["follows"] =
                    serde_json::json!({"track": source.0, "shape": {"transpose": 12}});
            }
            for effect in stored["mixer"]["effects"].as_array_mut().unwrap() {
                let slot = auris_core::EffectSlotId(effect["id"].as_u64().unwrap());
                if let Some(source) = expected.sidechain(slot) {
                    effect["sidechain"] = serde_json::json!(source.0);
                }
            }
        }
        std::fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = load_project(file.path()).unwrap();
        for track in &expected.tracks {
            assert_eq!(loaded.outputs(track.id), expected.outputs(track.id));
            assert_eq!(
                loaded.layer_source(track.id),
                expected.layer_source(track.id)
            );
        }
        let path = loaded.routing.path_into(layer).unwrap();
        assert_eq!(path.performances.len(), 1);
        assert_eq!(
            loaded
                .routing
                .performance(path.performances[0])
                .unwrap()
                .transforms,
            [auris_core::NoteTransform::Transpose { semitones: 12 }]
        );
        assert!(
            loaded.conflicting_object_id().is_none(),
            "the new performance took an id of its own"
        );
        assert_eq!(loaded.sidechain(slot), Some(lead));
        assert_eq!(loaded.routing.edges().len(), expected.routing.edges().len());
    }

    #[test]
    fn version_31_loads_each_tracks_single_output_as_one_route() {
        let file = TempFile::new("version-31.auris");
        let mut expected = demo_project();
        let bus = expected.add_bus_track("Bus");
        let track = expected.add_instrument_track("Lead", "auris.synth.pulse");
        expected
            .routing
            .set_outputs(track, &[auris_core::Output::to_bus(bus)]);
        let mut value = serde_json::to_value(&expected).unwrap();
        value["format_version"] = serde_json::json!(31);
        // Version 31 kept each track's one route on the track, as a bare destination, before
        // outputs had a level of their own or lived in a list of the project's.
        value.as_object_mut().unwrap().remove("routing");
        for stored in value["tracks"].as_array_mut().unwrap() {
            let id = auris_core::TrackId(stored["id"].as_u64().unwrap());
            let to = expected.outputs(id)[0].to;
            let fields = stored.as_object_mut().unwrap();
            fields.insert("output".into(), serde_json::to_value(to).unwrap());
        }
        std::fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = load_project(file.path()).unwrap();
        assert_eq!(loaded.outputs(track), vec![auris_core::Output::to_bus(bus)]);
    }

    #[test]
    fn version_27_loads_with_the_original_volume_shape() {
        let file = TempFile::new("version-27.auris");
        let mut expected = demo_project();
        let track = expected.add_instrument_track("Strings", "auris.synth.pulse");
        let clip = expected
            .add_midi_clip(track, "Long note", Ticks::ZERO, Ticks(3840))
            .unwrap();
        expected
            .midi_clip_mut(clip)
            .unwrap()
            .transforms
            .push(auris_core::NoteTransform::Pitch {
                settings: auris_core::PitchPerformance {
                    volume_swell: 0.7,
                    ..Default::default()
                },
            });
        let mut value = serde_json::to_value(&expected).unwrap();
        value["format_version"] = serde_json::json!(27);
        fn remove_contours(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(fields) => {
                    fields.remove("volume_contour");
                    for value in fields.values_mut() {
                        remove_contours(value);
                    }
                }
                serde_json::Value::Array(values) => {
                    for value in values {
                        remove_contours(value);
                    }
                }
                _ => {}
            }
        }
        remove_contours(&mut value);
        std::fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = load_project(file.path()).unwrap();
        assert_eq!(loaded.format_version, Project::FORMAT_VERSION);
        assert_eq!(
            loaded.midi_clip(clip).unwrap().1.transforms,
            expected.midi_clip(clip).unwrap().1.transforms
        );
    }

    #[test]
    fn version_28_drum_maps_load_with_lanes_derived_from_their_roles() {
        let file = TempFile::new("version-28.auris");
        let mut expected = demo_project();
        let track = expected.add_drum_track("Kit", "auris.synth.drumkit");
        let map = auris_core::DrumMap::from_voices([(auris_core::DrumRole::Kick, 73)]);
        map.store(
            &mut expected
                .track_mut(track)
                .unwrap()
                .kind
                .as_instrument_mut()
                .unwrap()
                .instrument_state,
        );
        let mut value = serde_json::to_value(&expected).unwrap();
        value["format_version"] = serde_json::json!(28);
        value["tracks"].as_array_mut().unwrap().last_mut().unwrap()["kind"]["instrument_state"]
            ["extra"][auris_core::DrumMap::STATE_KEY]
            .as_object_mut()
            .unwrap()
            .remove("lanes");
        std::fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = load_project(file.path()).unwrap();
        let state = &loaded
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        let loaded_map = auris_core::DrumMap::load(state).unwrap();
        assert_eq!(loaded_map.lanes.len(), 1);
        assert_eq!(loaded_map.lanes[0].note, 73);
    }

    #[test]
    fn a_missing_version_or_required_timeline_is_rejected() {
        let file = TempFile::new("incomplete.auris");
        for field in [
            "format_version",
            "signatures",
            "harmony",
            "automation",
            "soundfonts",
        ] {
            let mut json = serde_json::to_value(demo_project()).unwrap();
            json.as_object_mut().unwrap().remove(field);
            std::fs::write(file.path(), serde_json::to_string(&json).unwrap()).unwrap();
            assert!(
                matches!(load_project(file.path()), Err(IoError::Json(_))),
                "{field}"
            );
        }
    }

    #[test]
    fn a_saved_file_carries_the_version_of_the_build_that_wrote_it() {
        let file = TempFile::new("stamped.auris");
        // The saver stamps its schema version even if the in-memory field was changed.
        let mut loaded = demo_project();
        loaded.format_version = 1;

        save_project(file.path(), &mut loaded).unwrap();
        let written: FormatVersionProbe =
            serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
        assert_eq!(
            written.format_version,
            Project::FORMAT_VERSION,
            "the file records where the document came from rather than what wrote it, so an \
             older build would open a document full of fields it has never heard of"
        );
        // And the document agrees with the file it was just written to.
        assert_eq!(loaded.format_version, Project::FORMAT_VERSION);
    }

    #[test]
    fn malformed_json_reports_a_json_error() {
        let file = TempFile::new("broken.auris");
        std::fs::write(file.path(), "{ not json").unwrap();
        assert!(matches!(load_project(file.path()), Err(IoError::Json(_))));
    }

    #[test]
    fn a_missing_project_reports_file_not_found() {
        let path = std::env::temp_dir().join("auris-io-definitely-missing.auris");
        match load_project(&path) {
            Err(IoError::FileNotFound(reported)) => assert_eq!(reported, path),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_folder_is_recognised_through_a_difference_of_case_where_the_filesystem_would() {
        let folder = OsStr::new("roughmix");
        let stem = OsStr::new("RoughMix");
        assert!(folder_is_named(folder, stem, true));
        assert!(!folder_is_named(folder, stem, false));
        // Exact is exact on either kind.
        assert!(folder_is_named(stem, stem, true));
        assert!(folder_is_named(stem, stem, false));
        // And a different name is still a different name.
        assert!(!folder_is_named(OsStr::new("Demos"), stem, true));
    }

    #[test]
    fn renaming_a_project_by_case_alone_saves_in_place_rather_than_one_level_down() {
        // NTFS and APFS both hold `roughmix` and `RoughMix` as one directory, so a save that
        // capitalises the name is a save into the folder the project is already in. Comparing the
        // two byte for byte made it a save into a folder of its own inside that one, and because
        // nothing existed at the path that computed, the guard against writing over another
        // project never fired either: a second copy of the song, audio and all, appeared inside
        // the first with nothing on screen having asked.
        if !CASE_INSENSITIVE_PATHS {
            return;
        }
        assert_eq!(
            document_in_folder(Path::new("/songs/roughmix/RoughMix.auris")),
            PathBuf::from("/songs/roughmix/RoughMix.auris")
        );
        assert_eq!(
            document_in_folder(Path::new("/songs/RoughMix/RoughMix.auris")),
            PathBuf::from("/songs/RoughMix/RoughMix.auris")
        );
        // A folder that is genuinely another project still gets one made for it.
        assert_eq!(
            document_in_folder(Path::new("/songs/Demos/RoughMix.auris")),
            PathBuf::from("/songs/Demos/RoughMix/RoughMix.auris")
        );
    }
}
