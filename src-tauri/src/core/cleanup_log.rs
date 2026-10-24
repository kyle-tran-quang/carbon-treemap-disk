//! A local record of what the cleanup moved to the Trash.
//!
//! "Put Back" may be missing for items moved with `NSFileManager`, so the log
//! keeps each original path. One JSON object per line, appended. The log only
//! ever grows by appending; nothing here reads, rewrites or deletes files
//! other than creating and appending to the log itself.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::cleanup::{ItemResult, Outcome, serialize_path};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogLine<'a> {
    moved_at_secs: u64,
    #[serde(serialize_with = "serialize_path")]
    path: &'a std::path::PathBuf,
    name: &'a str,
    size: u64,
    files: u64,
}

/// Appends one line for each item in `results` that was moved. Items that
/// failed or were already gone are not logged. Returns how many were written.
pub fn log_moved(log: &Path, results: &[ItemResult], now: SystemTime) -> io::Result<usize> {
    let moved_at_secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut text = String::new();
    let mut count = 0;
    for result in results.iter().filter(|r| r.outcome == Outcome::Moved) {
        let line = LogLine {
            moved_at_secs,
            path: &result.path,
            name: &result.name,
            size: result.size,
            files: result.files,
        };
        text.push_str(&serde_json::to_string(&line).map_err(io::Error::other)?);
        text.push('\n');
        count += 1;
    }
    if count == 0 {
        return Ok(0);
    }
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    // One write for the whole batch keeps lines from interleaving.
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?
        .write_all(text.as_bytes())?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn result(name: &str, outcome: Outcome) -> ItemResult {
        ItemResult {
            node: 1,
            path: PathBuf::from("/Users/me/Library/Caches").join(name),
            name: name.to_owned(),
            size: 1234,
            files: 3,
            outcome,
            message: None,
        }
    }

    fn temp_log(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("nested/cleanup-log.jsonl")
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn lines(log: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn only_moved_items_are_logged_with_their_original_path() {
        let log = temp_log("moved");
        let results = [
            result("Homebrew", Outcome::Moved),
            result("Broken", Outcome::Failed),
            result("Gone", Outcome::AlreadyGone),
        ];
        assert_eq!(log_moved(&log, &results, at(1_700_000_000)).unwrap(), 1);
        let written = lines(&log);
        assert_eq!(written.len(), 1);
        assert_eq!(written[0]["path"], "/Users/me/Library/Caches/Homebrew");
        assert_eq!(written[0]["size"], 1234);
        assert_eq!(written[0]["files"], 3);
        assert_eq!(written[0]["movedAtSecs"], 1_700_000_000u64);
    }

    #[test]
    fn the_parent_folder_is_created() {
        let log = temp_log("parent");
        log_moved(&log, &[result("A", Outcome::Moved)], at(1)).unwrap();
        assert!(log.exists());
    }

    #[test]
    fn later_runs_append_and_never_overwrite() {
        let log = temp_log("append");
        log_moved(&log, &[result("A", Outcome::Moved)], at(1)).unwrap();
        log_moved(
            &log,
            &[result("B", Outcome::Moved), result("C", Outcome::Moved)],
            at(2),
        )
        .unwrap();
        let names: Vec<_> = lines(&log).iter().map(|l| l["name"].clone()).collect();
        assert_eq!(names, ["A", "B", "C"]);
    }

    #[test]
    fn nothing_moved_means_no_file_is_created() {
        let log = temp_log("empty");
        assert_eq!(
            log_moved(&log, &[result("A", Outcome::Failed)], at(1)).unwrap(),
            0
        );
        assert!(!log.exists());
    }

    #[test]
    fn an_unwritable_location_is_an_error_not_a_panic() {
        let blocker = temp_log("blocked");
        fs::create_dir_all(blocker.parent().unwrap()).unwrap();
        fs::write(&blocker, b"x").unwrap();
        let under_a_file = blocker.join("log.jsonl");
        assert!(log_moved(&under_a_file, &[result("A", Outcome::Moved)], at(1)).is_err());
    }
}
