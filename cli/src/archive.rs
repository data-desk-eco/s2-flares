//! Publish canonical GeoJSON records unchanged, and merge scanned AOI coverage.
//! Turning those records into Parquet is etl's job (etl/s2e/sql), not the
//! detector's — this crate writes the canonical format and stops there.

use crate::view;
use std::fs;
use std::path::Path;
use std::process::Command;

fn join(root: &str, tail: &str) -> String {
    format!(
        "{}/{}",
        root.trim_end_matches('/'),
        tail.trim_start_matches('/')
    )
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source).map_err(|e| format!("read {}: {e}", source.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else if !matches!(
            from.extension().and_then(|x| x.to_str()),
            Some("err" | "part")
        ) {
            fs::create_dir_all(destination).map_err(|e| e.to_string())?;
            fs::copy(&from, &to)
                .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// `aws` carrying the archive endpoint and the S2_S3_* credentials, which are kept
/// separate from AWS_* so the same process can still read eodata through GDAL.
fn aws() -> Command {
    let mut command = Command::new("aws");
    if let Ok(endpoint) = std::env::var("S2_S3_ENDPOINT") {
        let endpoint = if endpoint.starts_with("http") {
            endpoint
        } else {
            format!("https://{endpoint}")
        };
        command.args(["--endpoint-url", &endpoint]);
    }
    for (s2, aws) in [
        ("S2_S3_ACCESS_KEY", "AWS_ACCESS_KEY_ID"),
        ("S2_S3_SECRET_KEY", "AWS_SECRET_ACCESS_KEY"),
        ("S2_S3_REGION", "AWS_DEFAULT_REGION"),
    ] {
        if let Ok(value) = std::env::var(s2) {
            command.env(aws, value);
        }
    }
    command
}

fn aws_sync(source: &Path, destination: &str) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    let mut command = aws();
    command.args([
        "s3",
        "sync",
        source
            .to_str()
            .ok_or_else(|| "non-utf8 input path".to_string())?,
        destination,
        "--exclude",
        "*.err",
        "--exclude",
        "*.part",
        "--only-show-errors",
    ]);
    let status = command.status().map_err(|e| format!("aws s3 sync: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("aws s3 sync exited non-zero".into())
    }
}

pub fn publish(input: &Path, destination: &str) -> Result<(), String> {
    let observations = input.join("observations");
    let assets = input.join("assets");
    if destination.starts_with("s3://") {
        aws_sync(&observations, &join(destination, "observations"))?;
        aws_sync(&assets, &join(destination, "assets"))?;
    } else {
        let destination_path = Path::new(destination);
        if input != destination_path {
            copy_tree(&observations, &destination_path.join("observations"))?;
            copy_tree(&assets, &destination_path.join("assets"))?;
        }
    }
    Ok(())
}

/// One object, local or in the store. Absent reads are `None`, not an error: the
/// coverage overlay is created by the first run that has something to put in it.
fn read_object(path: &str) -> Option<String> {
    if !path.starts_with("s3://") {
        return fs::read_to_string(path).ok();
    }
    let scratch = view::tmp("coverage.json");
    let ok = aws()
        .args(["s3", "cp", path])
        .arg(&scratch)
        .arg("--only-show-errors")
        .status()
        .is_ok_and(|s| s.success());
    let body = ok.then(|| fs::read_to_string(&scratch).ok()).flatten();
    let _ = fs::remove_file(&scratch);
    body
}

fn write_object(path: &str, body: &str) -> Result<(), String> {
    if !path.starts_with("s3://") {
        local_parent(path)?;
        return fs::write(path, body).map_err(|e| format!("write {path}: {e}"));
    }
    let scratch = view::tmp("coverage.json");
    fs::write(&scratch, body).map_err(|e| e.to_string())?;
    let status = aws()
        .args(["s3", "cp"])
        .arg(&scratch)
        .args([path, "--only-show-errors"])
        .status()
        .map_err(|e| format!("aws s3 cp: {e}"));
    let _ = fs::remove_file(&scratch);
    match status? {
        s if s.success() => Ok(()),
        _ => Err("aws s3 cp exited non-zero".into()),
    }
}

/// Merge the scanned AOI into `web/coverage.geojson` under ROOT — the web map's
/// coverage overlay and the archive-vs-detect check. Keyed by feature id, so a
/// re-scan replaces its own entry and a new AOI appends: coverage only grows.
/// Each entry is the AOI geometry stamped with the window that scanned it.
pub fn coverage(
    root: &str,
    features: &[(String, serde_json::Value)],
    start: &str,
    end: &str,
    scanned: &str,
) -> Result<(usize, usize), String> {
    let key = join(root, "web/coverage.geojson");
    let published: serde_json::Value = read_object(&key)
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_else(|| serde_json::json!({"type": "FeatureCollection", "features": []}));

    // insertion order is the published order, so an unchanged overlay round-trips.
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, serde_json::Value> = Default::default();
    for feature in published["features"].as_array().into_iter().flatten() {
        if let Some(id) = feature["properties"]["id"].as_str() {
            order.push(id.to_string());
            by_id.insert(id.to_string(), feature.clone());
        }
    }
    let mut merged = 0;
    for (id, feature) in features {
        if feature["geometry"].is_null() {
            continue;
        }
        let entry = serde_json::json!({
            "type": "Feature",
            "geometry": feature["geometry"],
            "properties": {
                "id": id,
                "name": feature["properties"]["name"].as_str().unwrap_or(""),
                "start": start, "end": end, "scanned": scanned,
            },
        });
        if by_id.insert(id.clone(), entry).is_none() {
            order.push(id.clone());
        }
        merged += 1;
    }
    if merged == 0 {
        return Ok((0, order.len()));
    }
    let features: Vec<&serde_json::Value> = order.iter().filter_map(|id| by_id.get(id)).collect();
    write_object(
        &key,
        &serde_json::json!({"type": "FeatureCollection", "features": features}).to_string(),
    )?;
    Ok((merged, order.len()))
}

fn local_parent(path: &str) -> Result<(), String> {
    if !path.starts_with("s3://") {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_work_for_local_and_object_storage() {
        assert_eq!(join("out/", "/observations"), "out/observations");
        assert_eq!(join("s3://bucket", "assets"), "s3://bucket/assets");
    }

    fn feature(id: &str, lon: f64, name: &str) -> (String, serde_json::Value) {
        (
            id.to_string(),
            serde_json::json!({
                "geometry": {"type": "Point", "coordinates": [lon, 0.0]},
                "properties": {"name": name},
            }),
        )
    }

    fn published(root: &str) -> Vec<serde_json::Value> {
        let body = fs::read_to_string(join(root, "web/coverage.geojson")).unwrap();
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["features"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// A re-scan replaces its own entry and a new AOI appends, so the overlay
    /// accumulates across runs instead of being replaced by the latest one.
    #[test]
    fn coverage_merges_by_id_and_only_grows() {
        let root = std::env::temp_dir().join(format!("s2e-coverage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let root = root.to_str().unwrap();

        let first = [feature("alpha", 1.0, "Alpha"), feature("beta", 2.0, "Beta")];
        assert_eq!(
            coverage(root, &first, "2025-01-01", "2025-12-31", "2026-01-02").unwrap(),
            (2, 2)
        );

        // one of the two rescanned over a later window, plus one new area
        let again = [feature("beta", 9.0, "Beta"), feature("gamma", 3.0, "Gamma")];
        assert_eq!(
            coverage(root, &again, "2026-01-01", "2026-06-30", "2026-07-01").unwrap(),
            (2, 3)
        );

        let features = published(root);
        assert_eq!(
            features.len(),
            3,
            "alpha survived a run that did not touch it"
        );
        let by_id = |id: &str| {
            features
                .iter()
                .find(|f| f["properties"]["id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(by_id("alpha")["properties"]["end"], "2025-12-31");
        assert_eq!(by_id("beta")["properties"]["end"], "2026-06-30");
        assert_eq!(by_id("beta")["geometry"]["coordinates"][0], 9.0);
        assert_eq!(by_id("gamma")["properties"]["name"], "Gamma");

        // nothing scannable leaves the published overlay untouched
        assert_eq!(
            coverage(root, &[], "2026-01-01", "2026-06-30", "2026-07-02").unwrap(),
            (0, 3)
        );
        assert_eq!(published(root).len(), 3);
        let _ = fs::remove_dir_all(root);
    }
}
