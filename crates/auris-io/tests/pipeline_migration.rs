use auris_core::pipeline::content::ContentLimits;
use auris_io::{PipelineMigrationError, load_native_candidate, load_project};
use std::fs;

const FIXTURE: &str = include_str!("fixtures/project_formats/v27.auris");

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 64,
        clips: 1024,
        stages: 4096,
        processors: 4096,
        groups: 256,
        tracks: 256,
        edges: 8192,
        external_ids: 1024,
        automations: 256,
        automation_points: 8192,
        bindings: 1024,
    }
}

fn file(name: &str, bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().prefix(name).tempfile().unwrap();
    std::io::Write::write_all(&mut file, bytes).unwrap();
    file
}

#[test]
fn candidate_keeps_exact_source_bytes_and_original_version() {
    let source = file("migration-source", FIXTURE.as_bytes());
    let candidate = load_native_candidate(source.path(), limits()).unwrap();
    assert_eq!(candidate.source_bytes(), FIXTURE.as_bytes());
    assert_eq!(candidate.report().source_format_version, 27);
    assert_eq!(candidate.document().context.source_format_version, 27);
    assert!(candidate.source_matches(source.path()).unwrap());
    fs::write(source.path(), b"changed after capture").unwrap();
    assert!(!candidate.source_matches(source.path()).unwrap());
    assert_eq!(candidate.source_bytes(), FIXTURE.as_bytes());
}

#[test]
fn repairs_are_reported_from_the_captured_source() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    value["next_id"] = serde_json::json!(0);
    value["tracks"][0]["output"] = serde_json::json!({ "bus": 999999 });
    let bytes = serde_json::to_vec(&value).unwrap();
    let source = file("migration-repairs", &bytes);
    let candidate = load_native_candidate(source.path(), limits()).unwrap();
    let report = candidate.report();
    assert!(report.id_counter_repaired);
    assert!(report.routing_repaired);
    assert_eq!(candidate.source_bytes(), bytes.as_slice());
}

#[test]
fn future_versions_are_rejected_before_conversion() {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    value["format_version"] = serde_json::json!(u32::MAX);
    let bytes = serde_json::to_vec(&value).unwrap();
    let source = file("migration-future", &bytes);
    let error = load_native_candidate(source.path(), limits()).unwrap_err();
    assert!(matches!(error, PipelineMigrationError::Io(_)));
    assert!(error.to_string().contains("version"));
}

#[test]
fn failed_conversion_does_not_overwrite_destination() {
    let source = file("migration-fail", FIXTURE.as_bytes());
    let mut constrained = limits();
    constrained.tracks = 1;
    assert!(matches!(
        load_native_candidate(source.path(), constrained),
        Err(PipelineMigrationError::Conversion(_))
    ));
    assert_eq!(fs::read(source.path()).unwrap(), FIXTURE.as_bytes());
}

#[test]
fn backup_is_the_captured_raw_source_not_reserialized_json() {
    let source = file("migration-backup", FIXTURE.as_bytes());
    let candidate = load_native_candidate(source.path(), limits()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("legacy.auris");
    candidate.preserve_backup(&destination).unwrap();
    assert_eq!(fs::read(destination).unwrap(), FIXTURE.as_bytes());
}

#[test]
fn normal_loader_keeps_its_existing_normalized_version_behavior() {
    let source = file("migration-loader", FIXTURE.as_bytes());
    assert_eq!(load_project(source.path()).unwrap().format_version, 32);
}
