//! Bounded presentation of immutable, temporary report snapshots.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Seek, Write},
    sync::{Arc, Mutex, OnceLock},
};

const PAGE_BYTES: usize = 8192;
const CACHE_BYTES: u64 = 128 * 1024 * 1024;
type Entry = (String, Arc<Mutex<std::fs::File>>, u64);
static REPORTS: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();

fn cache() -> &'static Mutex<VecDeque<Entry>> {
    REPORTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn reference(value: &Value, path: &str) -> Value {
    let (kind, total) = match value {
        Value::Array(items) => ("array", items.len()),
        Value::Object(items) => ("object", items.len()),
        Value::String(text) => ("string", text.chars().count()),
        _ => return value.clone(),
    };
    json!({"report_path":path,"type":kind,"total":total})
}

fn brief(value: &Value, path: &str) -> Value {
    match value {
        Value::Array(_) | Value::Object(_) => reference(value, path),
        Value::String(text) if text.len() > 256 => reference(value, path),
        _ => value.clone(),
    }
}

/// Preserve small JSON results; large results become immutable snapshots with bounded views.
pub(crate) fn publish(value: impl serde::Serialize) -> Result<String, String> {
    let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    if text.len() <= PAGE_BYTES {
        return Ok(text);
    }
    snapshot(value, text.as_bytes())
}

/// Preserve small text reports and page larger ones by Unicode characters.
pub(crate) fn publish_text(text: String) -> Result<String, String> {
    if text.len() <= PAGE_BYTES {
        return Ok(text);
    }
    publish(Value::String(text))
}

fn snapshot(value: Value, bytes: &[u8]) -> Result<String, String> {
    // Anonymous/delete-on-close files are cleaned by the OS even when static caches do not drop.
    let mut file = tempfile::tempfile().map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let id = format!(
        "report-{}-{epoch}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let response = page(
        &value,
        &read_report::Args {
            report_id: id.clone(),
            path: String::new(),
            offset: 0,
            limit: None,
        },
    )?;
    let mut reports = cache().lock().map_err(|e| e.to_string())?;
    // Keep a single oversized report usable, but never retain other reports beside it.
    while !reports.is_empty()
        && (reports.len() >= 8
            || reports
                .iter()
                .map(|e| e.2)
                .sum::<u64>()
                .saturating_add(bytes.len() as u64)
                > CACHE_BYTES)
    {
        reports.pop_front();
    }
    reports.push_back((id, Arc::new(Mutex::new(file)), bytes.len() as u64));
    Ok(response)
}

fn page(root: &Value, args: &read_report::Args) -> Result<String, String> {
    let value = root
        .pointer(&args.path)
        .ok_or("Unknown JSON pointer; copy report_path from a previous page")?;
    let limit = args.limit.unwrap_or(16).clamp(1, 32);
    let total = match value {
        Value::Array(items) => items.len(),
        Value::Object(items) => items.len(),
        Value::String(text) => text.chars().count(),
        _ => 1,
    };
    if args.offset > total {
        return Err("offset exceeds total".into());
    }
    let mut count = (total - args.offset).min(if value.is_string() { 1024 } else { limit });
    loop {
        let data = match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .skip(args.offset)
                    .take(count)
                    .map(|(i, v)| {
                        let path = child(&args.path, &i.to_string());
                        if let Value::Object(fields) = v
                            && fields.len() <= 16
                        {
                            return Value::Object(
                                fields
                                    .iter()
                                    .map(|(k, v)| (k.clone(), brief(v, &child(&path, k))))
                                    .collect(),
                            );
                        }
                        brief(v, &path)
                    })
                    .collect(),
            ),
            Value::Object(fields) => Value::Array(
                fields
                    .iter()
                    .skip(args.offset)
                    .take(count)
                    .map(|(key, v)| {
                        let path = child(&args.path, key);
                        json!({"key":key,"report_path":path,"value":brief(v,&path)})
                    })
                    .collect(),
            ),
            Value::String(text) => {
                Value::String(text.chars().skip(args.offset).take(count).collect())
            }
            _ => value.clone(),
        };
        let next = args.offset + count;
        let response = json!({"report_id":args.report_id,"path":args.path,"offset":args.offset,"total":total,
            "next_offset":(next < total).then_some(next),"data":data,
            "usage":"Immutable snapshot, possibly predating edits. Use read_report with report_id, report_path as path, and next_offset as offset. Arrays/objects count entries; strings count Unicode characters. Expires on server restart or cache eviction; regenerate the original read-only report if expired."}).to_string();
        if response.len() <= PAGE_BYTES {
            return Ok(response);
        }
        if count <= 1 {
            return Err("Selected key or path is too large for a report page".into());
        }
        count /= 2;
    }
}

/// Read immutable report data without repeating analysis or applying edits.
pub mod read_report {
    use super::*;
    /// Wire name.
    pub const NAME: &str = "read_report";
    /// Model-facing description.
    pub const DESCRIPTION: &str = "Reads a cached immutable report snapshot without repeating analysis or edits. Copy report_id and report_path from a large report response. path is a JSON pointer (empty for root); offset follows next_offset. Arrays/objects page entries; strings page Unicode characters. Snapshots expire on server restart or eviction and may predate project edits.";
    /// Snapshot selection and bounded page.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Args {
        /// Opaque ID returned by the original report tool.
        pub report_id: String,
        /// JSON pointer copied from report_path; empty selects the root.
        #[serde(default)]
        pub path: String,
        /// Zero-based offset; follow next_offset without changing path.
        #[serde(default)]
        pub offset: usize,
        /// Entry page size, default 16, clamped to 1-32. Strings use up to 1024 characters.
        pub limit: Option<usize>,
    }
    /// Retrieve one snapshot page; never opens a caller-provided filesystem path.
    pub fn run(args: &Args) -> Result<String, String> {
        let file = cache().lock().map_err(|e| e.to_string())?.iter().find(|e| e.0 == args.report_id).map(|e| Arc::clone(&e.1))
            .ok_or("Report expired or unknown; rerun the original read-only report (apply:false, without MIDI output) to obtain a new snapshot")?;
        let mut file = file.lock().map_err(|e| e.to_string())?;
        file.rewind().map_err(|e| e.to_string())?;
        let root: Value = serde_json::from_reader(std::io::BufReader::new(&mut *file))
            .map_err(|e| e.to_string())?;
        page(&root, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_reports_are_paged_without_losing_nested_values() {
        let root = json!({"a/b~c":(0..1000).map(|i|json!({"index":i,"notes":vec![i;100]})).collect::<Vec<_>>()});
        let first: Value = serde_json::from_str(&publish(&root).unwrap()).unwrap();
        let id = first["report_id"].as_str().unwrap().to_string();
        let args = read_report::Args {
            report_id: id,
            path: "/a~1b~0c".into(),
            offset: 32,
            limit: Some(32),
        };
        let result = read_report::run(&args).unwrap();
        assert!(result.len() <= PAGE_BYTES);
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["data"][0]["index"], 32);
        assert_eq!(result["next_offset"], 64);
        assert_eq!(
            result["data"][0]["notes"]["report_path"],
            "/a~1b~0c/32/notes"
        );
        let last = page(
            &root,
            &read_report::Args {
                offset: 999,
                ..args
            },
        )
        .unwrap();
        assert!(serde_json::from_str::<Value>(&last).unwrap()["next_offset"].is_null());
    }
    #[test]
    fn text_pages_preserve_unicode_and_small_results_keep_their_shape() {
        assert_eq!(publish(json!({"notes":[1]})).unwrap(), r#"{"notes":[1]}"#);
        let root = Value::String("朝🎵".repeat(6000));
        let args = read_report::Args {
            report_id: "test".into(),
            path: String::new(),
            offset: 1,
            limit: None,
        };
        let result: Value = serde_json::from_str(&page(&root, &args).unwrap()).unwrap();
        assert!(result["data"].as_str().unwrap().starts_with('🎵'));
        assert_eq!(result["next_offset"], 1025);
        assert!(read_report::run(&args).is_err());
    }
}
