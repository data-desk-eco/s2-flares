//! The scene list a campaign runs from.
//!
//! Discovery is the fragile half of a bulk run: one catalogue search per AOI feature,
//! all of it ahead of any reading, against an endpoint that sheds bursts. A fleet made
//! it worse by having every member search independently, and a resume worse again by
//! repeating the whole pass. The manifest moves that work to one machine, one time:
//! `s2e discover` writes it, `s2e/fleet` pushes it beside the binary and the AOI, and
//! `detect --scenes` reads it instead of the network.
//!
//! It holds every scene, deduplicated, with the cloud threshold NOT applied, so one
//! file serves any `--cloud` and any shard. Gzipped NDJSON: a header line then a scene
//! per line, which streams on both ends and which DuckDB reads directly when a
//! campaign has to be audited.
//!
//! A manifest is checked rather than trusted. It is valid for a run when it names the
//! same catalogue and the same AOI file and covers at least the window asked for;
//! anything else is an error that says which of those failed. There is no hidden key,
//! no silent fall back to the network, and no way for a stale file to be used quietly.

use crate::stac::Item;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub manifest: u32,
    /// The catalogue profile the scenes came from; a run must ask for the same one.
    pub source: String,
    /// The window actually covered, which is wider than the run's when a plume run
    /// needs background scenes either side of it.
    pub start: String,
    pub end: String,
    pub aoi: String,
    /// The AOI file's own hash. An edited AOI is a different run, and this is what
    /// notices.
    pub aoi_sha256: String,
    pub s2e: String,
    pub created: String,
    pub scenes: usize,
    /// Catalogue requests it took to build, which is the number the whole exercise
    /// exists to keep small.
    pub requests: usize,
}

/// The AOI file's hash, which ties a manifest to the exact features it covers.
pub fn aoi_sha256(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read aoi {path}: {e}"))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

/// Write header and scenes, then rename into place. A half-written manifest never
/// exists under its real name, so there is nothing partial for a run to pick up.
pub fn write(path: &Path, header: &Header, items: &[Item]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("manifest dir: {e}"))?;
        }
    }
    let tmp = tmp_path(path);
    let file = File::create(&tmp).map_err(|e| format!("manifest create: {e}"))?;
    let mut out =
        flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::default());
    writeln!(
        out,
        "{}",
        serde_json::to_string(header).map_err(|e| format!("manifest header: {e}"))?
    )
    .map_err(|e| format!("manifest write: {e}"))?;
    for it in items {
        writeln!(
            out,
            "{}",
            serde_json::to_string(it).map_err(|e| format!("manifest scene: {e}"))?
        )
        .map_err(|e| format!("manifest write: {e}"))?;
    }
    out.finish()
        .map_err(|e| format!("manifest finish: {e}"))?
        .flush()
        .map_err(|e| format!("manifest flush: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("manifest rename: {e}"))
}

/// Read a manifest back. A scene line that will not parse is fatal: a run that
/// silently skipped scenes would look complete and be short.
pub fn read(path: &Path) -> Result<(Header, Vec<Item>), String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let reader = BufReader::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    let mut lines = reader.lines();
    let first = lines
        .next()
        .transpose()
        .map_err(|e| format!("manifest read: {e}"))?
        .ok_or("manifest is empty")?;
    let header: Header =
        serde_json::from_str(&first).map_err(|e| format!("manifest header: {e}"))?;
    if header.manifest != VERSION {
        return Err(format!(
            "manifest is version {} and this s2e reads version {VERSION}",
            header.manifest
        ));
    }
    let mut items = Vec::with_capacity(header.scenes);
    for (n, line) in lines.enumerate() {
        let line = line.map_err(|e| format!("manifest read: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        items.push(
            serde_json::from_str(&line)
                .map_err(|e| format!("manifest scene {}: {e}", n + 1))?,
        );
    }
    Ok((header, items))
}

/// Is this manifest the right one for this run? Same catalogue, same AOI, and a
/// window that covers what the run needs. Covering rather than matching is what lets
/// one manifest serve a flare run, a plume run's padded background window, and every
/// shard of both.
pub fn check(
    h: &Header,
    source: &str,
    start: &str,
    end: &str,
    aoi_sha: &str,
) -> Result<(), String> {
    if h.source != source {
        return Err(format!(
            "manifest holds {} scenes and this run asks for {source} — discover again",
            h.source
        ));
    }
    if h.aoi_sha256 != aoi_sha {
        return Err(format!(
            "manifest was built for a different {} — the AOI file has changed since, so discover again",
            h.aoi
        ));
    }
    if h.start.as_str() > start || h.end.as_str() < end {
        return Err(format!(
            "manifest covers {} to {} and this run needs {start} to {end} — discover again over the wider window",
            h.start, h.end
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stac::{Bands, Item};

    fn bands() -> Bands {
        Bands {
            b01: None, b02: None, b03: None, b04: Some("s3://eodata/x_B04.jp2".into()),
            b05: None, b06: None, b07: None, b08: None, b12: None, b11: None,
            b8a: None, b09: None, b10: None, scl: Some("s3://eodata/x_SCL.jp2".into()),
            product_metadata: None, granule_metadata: None,
        }
    }

    fn item(id: &str, date: &str, cloud: f64, bbox: [f64; 4]) -> Item {
        Item {
            id: id.into(),
            date: date.into(),
            datetime: format!("{date}T10:00:00Z"),
            cloud_cover: Some(cloud),
            mgrs: "30UXB".into(),
            epsg: 32630,
            bbox,
            sun_elevation: Some(40.0),
            sun_azimuth: Some(160.0),
            bands: bands(),
            level: "l1c".into(),
        }
    }

    fn header(start: &str, end: &str, scenes: usize) -> Header {
        Header {
            manifest: VERSION,
            source: "cdse-l1c".into(),
            start: start.into(),
            end: end.into(),
            aoi: "lng.geojson".into(),
            aoi_sha256: "abc123".into(),
            s2e: "0.3.1".into(),
            created: "2026-08-21T00:00:00Z".into(),
            scenes,
            requests: 7,
        }
    }

    #[test]
    fn round_trips_header_and_every_scene() {
        // the manifest is the only record of what a campaign was asked to look at, so
        // what comes back has to be what went in — band hrefs and all.
        let dir = std::env::temp_dir().join(format!("s2e-manifest-{}", std::process::id()));
        let path = dir.join("scenes.ndjson.gz");
        let items = vec![
            item("A", "2026-06-01", 10.0, [-1.0, 51.0, -0.5, 51.5]),
            item("B", "2026-06-06", 80.0, [-1.0, 51.0, -0.5, 51.5]),
        ];
        write(&path, &header("2026-01-01", "2026-12-31", items.len()), &items).unwrap();
        let (h, back) = read(&path).unwrap();
        assert_eq!(h.scenes, 2);
        assert_eq!(h.source, "cdse-l1c");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].id, "A");
        assert_eq!(back[1].bands.scl.as_deref(), Some("s3://eodata/x_SCL.jp2"));
        assert_eq!(back[0].level, "l1c");
        // and nothing partial is left behind under a name a run could pick up
        assert!(!tmp_path(&path).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_manifest_that_does_not_fit_the_run_is_refused() {
        // the failure this replaces was a cache that answered whatever it was asked.
        // every one of these has to be an error rather than a quiet wrong answer.
        let h = header("2026-01-01", "2026-12-31", 0);
        assert!(check(&h, "cdse-l1c", "2026-03-01", "2026-09-01", "abc123").is_ok());
        // exactly the covered window still fits
        assert!(check(&h, "cdse-l1c", "2026-01-01", "2026-12-31", "abc123").is_ok());
        // a run reaching past either end does not
        assert!(check(&h, "cdse-l1c", "2025-12-31", "2026-09-01", "abc123").is_err());
        assert!(check(&h, "cdse-l1c", "2026-03-01", "2027-01-01", "abc123").is_err());
        // a different catalogue is a different scene list
        assert!(check(&h, "aws-l1c", "2026-03-01", "2026-09-01", "abc123").is_err());
        // an edited AOI is a different run
        assert!(check(&h, "cdse-l1c", "2026-03-01", "2026-09-01", "def456").is_err());
    }
}
