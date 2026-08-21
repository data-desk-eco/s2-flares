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

/// Split a window into `n` contiguous parts that tile it with no gap and no overlap,
/// so a tile and date falls in exactly one part and merging the parts is a
/// concatenation.
fn split(start: &str, end: &str, n: usize) -> Vec<(String, String)> {
    let (Ok(s), Ok(e)) = (
        chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d"),
        chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d"),
    ) else {
        return vec![(start.into(), end.into())];
    };
    let days = (e - s).num_days();
    if n <= 1 || days < n as i64 {
        return vec![(start.into(), end.into())];
    }
    (0..n)
        .map(|i| {
            let from = s + chrono::Duration::days(days * i as i64 / n as i64 + if i > 0 { 1 } else { 0 });
            let to = s + chrono::Duration::days(days * (i as i64 + 1) / n as i64);
            (from.to_string(), to.to_string())
        })
        .collect()
}

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

pub fn run(c: &Common, out: &str, batch: usize, pad: i64, jobs: usize) {
    let Some(aoi_path) = c.aoi.as_deref() else {
        die("discover: --aoi is what there is to discover; --bbox and --region search as they run");
    };
    if c.shard.is_some() {
        die("discover: a manifest covers the whole AOI and every member reads the same one — drop --shard");
    }
    if batch == 0 {
        die("discover: --batch must be at least 1");
    }
    if jobs == 0 {
        die("discover: --jobs must be at least 1");
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

    // Sub-windows run at once. Requests share one process-wide pace, so more jobs is
    // more of the wait overlapped rather than a faster request rate — the endpoint sees
    // the same steady stream either way. Memory is the limit: each job holds its own
    // scene list, so this is where a long campaign wants a machine of its own rather
    // than the control plane.
    let windows = split(&start, &end, jobs);
    if windows.len() > 1 {
        eprintln!("  {} window(s) at once:", windows.len());
        for (s, e) in &windows {
            eprintln!("    {s} -> {e}");
        }
    }
    let results: Vec<Result<Vec<stac::Item>, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = windows
            .iter()
            .map(|(ws, we)| {
                let (envelopes, source) = (&envelopes, &c.source);
                scope.spawn(move || {
                    let mut got: Vec<stac::Item> = Vec::new();
                    for (n, chunk) in envelopes.chunks(batch).enumerate() {
                        match stac::discover(chunk, ws, we, source) {
                            Ok(part) => {
                                got.extend(part);
                                eprintln!(
                                    "  {ws}..{we} batch {}/{batches}: {} scenes so far ({} requests)",
                                    n + 1,
                                    got.len(),
                                    stac::requests()
                                );
                            }
                            Err(e) => return Err(format!("{ws}..{we} batch {}/{batches}: {e}", n + 1)),
                        }
                    }
                    Ok(got)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut scenes: Vec<stac::Item> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in results {
        match r {
            Ok(got) => merge(&mut scenes, &mut seen, got),
            // A window that will not resolve leaves a hole in every feature it covered,
            // and a hole in a manifest is invisible to the run that reads it. Stop.
            Err(e) => die(&format!(
                "discover: {e}\n\
                 nothing written — the manifest would have been short every scene in \
                 that window, for all {} of the AOI's features",
                envelopes.len()
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

#[cfg(test)]
mod tests {
    use super::split;

    /// The windows must tile the span exactly: a gap loses every scene in it, silently,
    /// and an overlap costs a second search for scenes already had.
    #[test]
    fn windows_tile_the_span_without_gap_or_overlap() {
        let w = split("2018-01-01", "2026-12-31", 5);
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].0, "2018-01-01");
        assert_eq!(w[4].1, "2026-12-31");
        for pair in w.windows(2) {
            let (_, prev_end) = &pair[0];
            let (next_start, _) = &pair[1];
            let prev = chrono::NaiveDate::parse_from_str(prev_end, "%Y-%m-%d").unwrap();
            let next = chrono::NaiveDate::parse_from_str(next_start, "%Y-%m-%d").unwrap();
            assert_eq!((next - prev).num_days(), 1, "{prev_end} -> {next_start}");
        }
    }

    /// One job, or a span too short to divide, stays the window it was given.
    #[test]
    fn a_single_window_is_left_alone() {
        assert_eq!(
            split("2026-01-01", "2026-12-31", 1),
            vec![("2026-01-01".to_string(), "2026-12-31".to_string())]
        );
        assert_eq!(split("2026-01-01", "2026-01-03", 8).len(), 1);
        // an unparseable window is passed through rather than silently dropped
        assert_eq!(split("whenever", "2026-01-01", 4).len(), 1);
    }
}
