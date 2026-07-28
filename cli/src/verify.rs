//! Run completeness over a detect output tree: every requested AOI feature has a
//! durable record, and no retryable `.err` is left behind. The fleet orchestrator
//! only reads the exit code, so the detail printed here is for a human.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn target_id(path: &PathBuf) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    record["analysis"]["target"]["id"]
        .as_str()
        .map(String::from)
}

/// Print a summary; true when the output is complete and clean. `expected` is the
/// AOI feature ids this run was asked for — `None` for a bbox/region run, which has
/// no per-feature ledger and is judged on errors alone.
pub fn run(input: &Path, expected: Option<&[String]>) -> bool {
    let mut files = Vec::new();
    walk(&input.join("observations"), &mut files);
    let has_ext = |path: &PathBuf, ext: &str| path.extension().is_some_and(|e| e == ext);
    let errors: Vec<&PathBuf> = files.iter().filter(|p| has_ext(p, "err")).collect();
    let scanned: HashSet<String> = files
        .iter()
        .filter(|p| has_ext(p, "geojson"))
        .filter_map(target_id)
        .collect();
    let gaps: Vec<&String> = expected
        .unwrap_or_default()
        .iter()
        .filter(|id| !scanned.contains(*id))
        .collect();

    match expected {
        Some(ids) => println!(
            "  {}/{} features scanned, {} unscanned, {} errored scenes",
            ids.len() - gaps.len(),
            ids.len(),
            gaps.len(),
            errors.len()
        ),
        None => println!(
            "  no --aoi (bbox/region run) — {} errored scenes",
            errors.len()
        ),
    }
    for gap in &gaps {
        println!("    unscanned: {gap}");
    }
    for error in &errors {
        println!("    errored: {}", error.display());
    }
    gaps.is_empty() && errors.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(root: &Path, name: &str, target: &str) {
        let dir = root.join("observations").join(target).join("S2_X");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(name),
            format!(r#"{{"analysis":{{"target":{{"id":"{target}"}}}},"features":[]}}"#),
        )
        .unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("s2e-verify-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_requested_feature_must_have_a_record() {
        let root = scratch("gap");
        scene(&root, "flares-abc.geojson", "alpha");
        let want = ["alpha".to_string(), "beta".to_string()];
        assert!(!run(&root, Some(&want)), "beta was never scanned");
        assert!(run(&root, Some(&want[..1])));
    }

    #[test]
    fn a_retryable_error_fails_even_when_nothing_was_asked_for() {
        let root = scratch("err");
        scene(&root, "flares-abc.geojson", "alpha");
        assert!(run(&root, None));
        std::fs::write(root.join("observations/alpha/S2_X/flares-abc.err"), "boom").unwrap();
        assert!(!run(&root, None));
    }

    #[test]
    fn an_empty_or_missing_tree_is_only_clean_when_nothing_was_asked_for() {
        let root = scratch("empty");
        assert!(run(&root, None));
        assert!(!run(&root, Some(&["alpha".to_string()])));
    }
}
