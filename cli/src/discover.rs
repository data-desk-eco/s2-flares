//! Build a campaign's scene manifest in as few catalogue requests as the endpoint
//! will take.
//!
//! The endpoint answers a MultiPolygon `intersects`, so a batch of AOI features is one
//! request rather than one each. That is the whole reduction: a 1198-feature AOI over
//! an eight-year window is about 24,000 requests one feature at a time — counting the
//! L2A twin search each one triggers for its cloud mask — and about 190 in batches of
//! a hundred. Fewer requests is the only thing that makes bulk discovery survive an
//! endpoint that sheds bursts.

use super::{die, load_aois, manifest, stac, Common};
use std::path::Path;

/// Scenes shared by two batches arrive twice. Dedup by scene id, which is safe because
/// the tile/date winner is a property of the tile and date, not of which envelope
/// matched it.
fn merge(into: &mut Vec<stac::Item>, seen: &mut std::collections::HashSet<String>, got: Vec<stac::Item>) {
    for it in got {
        if seen.insert(it.id.clone()) {
            into.push(it);
        }
    }
}

pub fn run(c: &Common, out: &str, batch: usize, pad: i64) {
    let Some(aoi_path) = c.aoi.as_deref() else {
        die("discover: --aoi is what there is to discover; --bbox and --region search as they run");
    };
    if c.shard.is_some() {
        die("discover: a manifest covers the whole AOI and every member reads the same one — drop --shard");
    }
    if batch == 0 {
        die("discover: --batch must be at least 1");
    }
    let aois = load_aois(c);
    if aois.is_empty() {
        die("discover: the AOI has no features");
    }
    // The window is padded because a plume run selects its background from scenes
    // either side of the requested dates. One manifest then serves flares, plumes and
    // both; `detect` narrows to what each pass actually wants.
    let (start, end) = c.dates();
    let (start, end) = (
        super::shift_date(&start, -pad),
        super::shift_date(&end, pad),
    );
    let envelopes: Vec<[f64; 4]> = aois.iter().map(|a| a.bbox).collect();
    let batches = envelopes.len().div_ceil(batch);
    eprintln!(
        "discover: {} feature(s) · {} batch(es) of up to {batch} · {start} -> {end} · {}",
        envelopes.len(),
        batches,
        c.source
    );

    let mut scenes: Vec<stac::Item> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (n, chunk) in envelopes.chunks(batch).enumerate() {
        match stac::discover(chunk, &start, &end, &c.source) {
            Ok(got) => {
                let found = got.len();
                merge(&mut scenes, &mut seen, got);
                eprintln!(
                    "  batch {}/{batches}: {found} scenes ({} total, {} requests)",
                    n + 1,
                    scenes.len(),
                    stac::requests()
                );
            }
            // A batch that will not resolve leaves a hole in every feature it covered,
            // and a hole in a manifest is invisible to the run that reads it. Stop.
            Err(e) => die(&format!(
                "discover: batch {}/{batches} failed: {e}\n\
                 nothing written — the manifest would have been short by every scene \
                 for {} of the AOI's features",
                n + 1,
                chunk.len()
            )),
        }
    }

    let header = manifest::Header {
        manifest: manifest::VERSION,
        source: c.source.clone(),
        start,
        end,
        aoi: Path::new(aoi_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        aoi_sha256: manifest::aoi_sha256(aoi_path).unwrap_or_else(|e| die(&e)),
        s2e: env!("CARGO_PKG_VERSION").to_string(),
        created: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        scenes: scenes.len(),
        requests: stac::requests(),
    };
    manifest::write(Path::new(out), &header, &scenes).unwrap_or_else(|e| die(&e));
    eprintln!(
        "discover: wrote {out} — {} scenes in {} requests",
        header.scenes, header.requests
    );
}
