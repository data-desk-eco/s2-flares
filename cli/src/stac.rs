//! stac search — 1:1 port of lib/stac.js. two source profiles (aws element84 cog
//! hrefs; cdse copernicus eopf s3://eodata jp2). blocking http (ureq) + serde_json
//! so the fan-out can be plain rayon threads, no async runtime.

use std::time::Duration;

use s2e_core::epsg_from_mgrs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bands {
    pub b01: Option<String>,
    pub b02: Option<String>,
    pub b03: Option<String>,
    pub b04: Option<String>,
    pub b05: Option<String>,
    pub b06: Option<String>,
    pub b07: Option<String>,
    pub b08: Option<String>,
    pub b12: Option<String>,
    pub b11: Option<String>,
    pub b8a: Option<String>,
    pub b09: Option<String>,
    pub b10: Option<String>,
    pub scl: Option<String>,
    pub product_metadata: Option<String>,
    pub granule_metadata: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(dead_code)] // cloud_cover/bbox carried for parity; the whole-tile bbox feeds the scene-store cache
pub struct Item {
    pub id: String,
    pub date: String,
    pub datetime: String,
    pub cloud_cover: Option<f64>,
    pub mgrs: String,
    pub epsg: i32,
    pub bbox: [f64; 4],
    pub sun_elevation: Option<f64>,
    pub sun_azimuth: Option<f64>,
    pub bands: Bands,
    /// Sentinel product radiometry: "l1c" (TOA) or "l2a" (surface reflectance).
    pub level: String,
}

fn api(source: &str) -> &'static str {
    if source.starts_with("cdse") {
        "https://stac.dataspace.copernicus.eu/v1"
    } else {
        "https://earth-search.aws.element84.com/v1"
    }
}

fn level(source: &str) -> &'static str {
    if source.ends_with("l1c") {
        "l1c"
    } else {
        "l2a"
    }
}

/// the L2A profile on the same catalogue, for borrowing a cloud mask.
fn l2a_twin(source: &str) -> &'static str {
    if source.starts_with("cdse") {
        "cdse"
    } else {
        "aws"
    }
}

fn href(it: &Value, key: &str) -> Option<String> {
    it["assets"][key]["href"].as_str().map(String::from)
}

fn aws_l1c_href(it: &Value, key: &str) -> Option<String> {
    href(it, key).map(|url| {
        url.strip_prefix("s3://sentinel-s2-l1c/")
            .map(|path| format!("https://sentinel-s2-l1c.s3.eu-central-1.amazonaws.com/{path}"))
            .unwrap_or(url)
    })
}

// cdse's stac records for pre-2026 l1c items omit the B10 asset even though the
// jp2 is present in the safe on eodata; derive its href from B11's.
fn cdse_b10(it: &Value) -> Option<String> {
    href(it, "B10").or_else(|| href(it, "B11").map(|u| u.replace("_B11.jp2", "_B10.jp2")))
}

fn bands_of(it: &Value, source: &str) -> Bands {
    if source.starts_with("cdse") && level(source) == "l1c" {
        Bands {
            b01: href(it, "B01"),
            b02: href(it, "B02"),
            b03: href(it, "B03"),
            b04: href(it, "B04"),
            b05: href(it, "B05"),
            b06: href(it, "B06"),
            b07: href(it, "B07"),
            b08: href(it, "B08"),
            b8a: href(it, "B8A"),
            b09: href(it, "B09"),
            b10: cdse_b10(it),
            b11: href(it, "B11"),
            b12: href(it, "B12"),
            scl: None,
            product_metadata: href(it, "product_metadata"),
            granule_metadata: href(it, "granule_metadata"),
        }
    } else if source.starts_with("cdse") {
        Bands {
            b01: None,
            b02: None,
            b03: None,
            b04: None,
            b05: None,
            b06: None,
            b07: None,
            b08: None,
            b09: None,
            b10: None,
            b12: href(it, "B12_20m"),
            b11: href(it, "B11_20m"),
            b8a: href(it, "B8A_20m"),
            scl: href(it, "SCL_20m"),
            product_metadata: href(it, "product_metadata"),
            granule_metadata: href(it, "granule_metadata"),
        }
    } else if level(source) == "l1c" {
        Bands {
            b01: aws_l1c_href(it, "coastal"),
            b02: aws_l1c_href(it, "blue"),
            b03: aws_l1c_href(it, "green"),
            b04: aws_l1c_href(it, "red"),
            b05: aws_l1c_href(it, "rededge1"),
            b06: aws_l1c_href(it, "rededge2"),
            b07: aws_l1c_href(it, "rededge3"),
            b08: aws_l1c_href(it, "nir"),
            b8a: aws_l1c_href(it, "nir08"),
            b09: aws_l1c_href(it, "nir09"),
            b10: aws_l1c_href(it, "cirrus"),
            b11: aws_l1c_href(it, "swir16"),
            // Earth Search currently advertises a non-existent
            // `product_metadata.xml` object for this collection.  The tile's
            // `metadata.xml` contains the authoritative L1C quantification and
            // RADIO_ADD_OFFSET values (and is the asset AWS actually serves).
            b12: aws_l1c_href(it, "swir22"),
            scl: None,
            product_metadata: aws_l1c_href(it, "granule_metadata"),
            granule_metadata: aws_l1c_href(it, "granule_metadata"),
        }
    } else {
        Bands {
            b01: None,
            b02: None,
            b03: None,
            b04: None,
            b05: None,
            b06: None,
            b07: None,
            b08: None,
            b09: None,
            b10: None,
            b12: href(it, "swir22"),
            b11: href(it, "swir16"),
            b8a: href(it, "nir08"),
            scl: href(it, "scl"),
            product_metadata: None,
            granule_metadata: None,
        }
    }
}

fn epsg_of(it: &Value, source: &str) -> i32 {
    if source.starts_with("cdse") {
        epsg_from_mgrs(it["properties"]["grid:code"].as_str().unwrap_or(""))
    } else {
        it["properties"]["proj:epsg"].as_i64().unwrap_or(0) as i32
    }
}

const STAC_ATTEMPTS: usize = 7;

/// The catalogue caps a page per collection: L1C serves 1000, and L2A refuses more
/// than 200 because its items are much larger. A bigger page is a shorter burst,
/// and the burst is what the edge sheds.
fn page_limit(source: &str) -> usize {
    if level(source) == "l1c" {
        1000
    } else {
        200
    }
}

/// A WAF sits in front of the catalogue and sheds a burst past about seven requests,
/// answering 429 with `Retry-After: 2`. Pagination sends its pages back to back, so
/// without a floor between requests a single deep search becomes that burst. This is
/// the whole reason bulk discovery used to stall.
const PACE: Duration = Duration::from_millis(250);
static LAST_REQUEST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
static REQUESTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many catalogue requests this process has made, for the manifest header.
pub fn requests() -> usize {
    REQUESTS.load(std::sync::atomic::Ordering::Relaxed)
}

fn pace() {
    let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        let since = t.elapsed();
        if since < PACE {
            std::thread::sleep(PACE - since);
        }
    }
    *last = Some(std::time::Instant::now());
    REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn retryable(code: u16) -> bool {
    code == 408 || code == 429 || code >= 500
}

/// POST one STAC page, retrying transient transport/HTTP failures. CDSE sheds load
/// with 429/500/504; failing an AOI search silently omits that site, so retry each
/// pagination request in place rather than forcing a costly second run over the
/// entire AOI catalogue.
///
/// Two responses are not transient and must not be retried. A body that is not JSON
/// is the WAF's own rejection page, which it serves with status 200 — retrying it
/// seven times just spends the budget that made it reject us. And when the endpoint
/// states a `Retry-After`, that is the wait to honour; the local backoff is only a
/// guess at the same number.
fn post_page(url: &str, body: &Value) -> Result<Value, String> {
    for attempt in 1..=STAC_ATTEMPTS {
        pace();
        let result = ureq::post(url)
            .set("Content-Type", "application/json")
            .send_json(body.clone());
        let (err, stated) = match result {
            Ok(resp) => match resp.into_json() {
                Ok(data) => return Ok(data),
                Err(e) => {
                    return Err(format!(
                        "stac: response was not json ({e}) — the endpoint rejected the request \
                         rather than answering it, so the query itself has to change"
                    ))
                }
            },
            Err(ureq::Error::Status(code, resp)) if retryable(code) => {
                let wait = resp
                    .header("Retry-After")
                    .and_then(|v| v.trim().parse::<u64>().ok());
                (format!("stac http: status {code}"), wait)
            }
            Err(e @ ureq::Error::Status(..)) => return Err(format!("stac http: {e}")),
            Err(e) => (format!("stac http: {e}"), None),
        };
        if attempt == STAC_ATTEMPTS {
            return Err(format!("{err} after {STAC_ATTEMPTS} attempts"));
        }
        let delay = stated.unwrap_or(1u64 << (attempt - 1).min(4)); // 1, 2, 4, 8, then 16 s
        eprintln!(
            "  stac transient failure; retry {}/{} in {delay}s: {err}",
            attempt + 1,
            STAC_ATTEMPTS
        );
        std::thread::sleep(Duration::from_secs(delay));
    }
    unreachable!()
}

/// A GeoJSON Point has a zero-area envelope, which STAC APIs reject. The plume
/// reader still uses its fixed 2 km chip; this epsilon only makes the catalogue
/// intersection well-defined.
fn widen(b: [f64; 4]) -> [f64; 4] {
    let mut q = b;
    if q[0] >= q[2] {
        q[0] -= 1e-6;
        q[2] += 1e-6;
    }
    if q[1] >= q[3] {
        q[1] -= 1e-6;
        q[3] += 1e-6;
    }
    q
}

/// A MultiPolygon's parts may not overlap. GEOS answers an intersects against
/// overlapping parts with a topology exception, which the endpoint returns as a 500 —
/// and neighbouring AOI features overlap all the time, so a batch that merged nothing
/// would fail on any pair of sites within a couple of kilometres of each other.
///
/// Merge every envelope that meets another into the box covering both, transitively.
/// The query then asks for a superset of what the batch wanted, which costs a few
/// extra scenes in the manifest and nothing in correctness: each feature still selects
/// only the scenes its own envelope meets.
fn disjoint(area: &[[f64; 4]]) -> Vec<[f64; 4]> {
    let mut out: Vec<[f64; 4]> = Vec::new();
    for &b in area {
        let mut cur = b;
        loop {
            let mut merged = false;
            let mut keep: Vec<[f64; 4]> = Vec::with_capacity(out.len());
            for &o in &out {
                if cur[0] <= o[2] && cur[2] >= o[0] && cur[1] <= o[3] && cur[3] >= o[1] {
                    cur = [
                        cur[0].min(o[0]),
                        cur[1].min(o[1]),
                        cur[2].max(o[2]),
                        cur[3].max(o[3]),
                    ];
                    merged = true;
                } else {
                    keep.push(o);
                }
            }
            out = keep;
            if !merged {
                break;
            }
        }
        out.push(cur);
    }
    out
}

/// One envelope is a `bbox` query. Several become a MultiPolygon `intersects`, which
/// the catalogue answers in a single request — so a batch of AOI features costs one
/// round trip instead of one each, which is what takes a bulk run from tens of
/// thousands of requests to a few hundred.
fn payload(area: &[[f64; 4]], start: &str, end: &str, source: &str) -> Value {
    let mut p = serde_json::json!({
        "collections": [format!("sentinel-2-{}", level(source))],
        "datetime": format!("{start}T00:00:00Z/{end}T23:59:59Z"),
        "limit": page_limit(source),
    });
    match disjoint(area).as_slice() {
        [one] => p["bbox"] = serde_json::json!(one),
        many => {
            let rings: Vec<_> = many
                .iter()
                .map(|b| {
                    [[
                        [b[0], b[1]],
                        [b[2], b[1]],
                        [b[2], b[3]],
                        [b[0], b[3]],
                        [b[0], b[1]],
                    ]]
                })
                .collect();
            p["intersects"] = serde_json::json!({"type": "MultiPolygon", "coordinates": rings});
        }
    }
    p
}

/// Page through one search and return the raw features.
fn query(area: &[[f64; 4]], start: &str, end: &str, source: &str) -> Result<Vec<Value>, String> {
    let mut features: Vec<Value> = Vec::new();
    let mut url = format!("{}/search", api(source));
    let mut body = payload(area, start, end, source);
    loop {
        let data = post_page(&url, &body)?;
        if let Some(arr) = data["features"].as_array() {
            features.extend(arr.iter().cloned());
        }
        // follow the rel:next link (post body) if present.
        let next = data["links"]
            .as_array()
            .and_then(|ls| ls.iter().find(|l| l["rel"] == "next").cloned());
        match next.and_then(|l| Some((l["href"].as_str()?.to_string(), l.get("body")?.clone()))) {
            Some((h, b)) => {
                url = h;
                body = b;
            }
            None => break,
        }
    }
    Ok(features)
}

/// Does a scene envelope meet a query envelope? Discovery drops what fails this, and
/// selecting one feature's scenes out of a manifest applies the identical test, which
/// is why a manifest run and a per-feature search agree.
pub fn meets(item: &Item, env: [f64; 4]) -> bool {
    let q = widen(env);
    let b = item.bbox;
    b[2] - b[0] < 5.0 && b[0] <= q[2] && b[2] >= q[0] && b[1] <= q[3] && b[3] >= q[1]
}

/// Drop the scenes a run's cloud threshold excludes. This is a pure function of what
/// `discover` returns, and the tile/date dedup ahead of it keeps the clearest scene
/// regardless of the threshold — so one discovery serves any `--cloud`.
pub fn filter_cloud(items: Vec<Item>, max_cloud_cover: f64) -> Vec<Item> {
    items
        .into_iter()
        .filter(|it| it.cloud_cover.unwrap_or(100.0) <= max_cloud_cover)
        .collect()
}

/// Every scene the catalogue holds for these envelopes over this window, deduplicated
/// by tile and date keeping the clearest, with the L2A cloud mask resolved — and
/// deliberately NOT filtered by cloud. The threshold is applied by `filter_cloud`
/// afterwards, so one discovery serves any `--cloud` and a manifest need not be
/// rebuilt to change it.
pub fn discover(
    area: &[[f64; 4]],
    start: &str,
    end: &str,
    source: &str,
) -> Result<Vec<Item>, String> {
    if area.is_empty() {
        return Ok(Vec::new());
    }
    let q: Vec<[f64; 4]> = area.iter().map(|b| widen(*b)).collect();
    let mut features = query(&q, start, end, source)?;

    // cdse occasionally returns antimeridian tiles for queries anywhere on the
    // globe (seen: pacific t01/t60 items for a uk point search, with degenerate
    // [-179.57..180] bboxes that intersect everything). a stray scene computes a
    // chip window half a world from its raster and ooms the box, so drop items
    // whose bbox misses every query envelope or is wider than any real s2 tile.
    features.retain(|it| {
        it["bbox"].as_array().is_none_or(|b| {
            let v: Vec<f64> = b.iter().filter_map(|x| x.as_f64()).collect();
            v.len() != 4
                || (v[2] - v[0] < 5.0
                    && q.iter()
                        .any(|e| v[0] <= e[2] && v[2] >= e[0] && v[1] <= e[3] && v[3] >= e[1]))
        })
    });

    // dedup by tile+date, keep lowest cloud.
    let mut best: std::collections::HashMap<String, (Value, f64)> =
        std::collections::HashMap::new();
    for it in features {
        let p = &it["properties"];
        let dt = p["datetime"]
            .as_str()
            .unwrap_or("")
            .get(..10)
            .unwrap_or("")
            .to_string();
        let cloud = p["eo:cloud_cover"].as_f64().unwrap_or(100.0);
        let tile = p["grid:code"]
            .as_str()
            .or_else(|| p["s2:mgrs_tile"].as_str())
            .or_else(|| it["id"].as_str())
            .unwrap_or("")
            .to_string();
        // temporary experiment override: S2_KEEP_ORBITS=1 keys the dedup by
        // orbit too, so same-day dual-look acquisitions both survive.
        let orbit = if std::env::var("S2_KEEP_ORBITS").is_ok() {
            it["id"]
                .as_str()
                .unwrap_or("")
                .split('_')
                .find(|t| t.len() == 4 && t.starts_with('R'))
                .unwrap_or("")
        } else {
            ""
        };
        let key = format!("{tile}_{dt}_{orbit}");
        match best.get(&key) {
            Some((_, c)) if *c <= cloud => {}
            _ => {
                best.insert(key, (it, cloud));
            }
        }
    }

    let mut out = Vec::new();
    for (it, _) in best.into_values() {
        let p = &it["properties"];
        out.push(Item {
            id: it["id"].as_str().unwrap_or("").to_string(),
            date: p["datetime"]
                .as_str()
                .unwrap_or("")
                .get(..10)
                .unwrap_or("")
                .to_string(),
            datetime: p["datetime"].as_str().unwrap_or("").to_string(),
            cloud_cover: p["eo:cloud_cover"].as_f64(),
            mgrs: p["grid:code"]
                .as_str()
                .or_else(|| p["s2:mgrs_tile"].as_str())
                .unwrap_or("")
                .replace("MGRS-", ""),
            epsg: epsg_of(&it, source),
            bbox: {
                let b = it["bbox"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>())
                    .unwrap_or_default();
                [
                    b.first().copied().unwrap_or(0.0),
                    b.get(1).copied().unwrap_or(0.0),
                    b.get(2).copied().unwrap_or(0.0),
                    b.get(3).copied().unwrap_or(0.0),
                ]
            },
            sun_elevation: p["view:sun_elevation"].as_f64(),
            sun_azimuth: p["view:sun_azimuth"].as_f64(),
            bands: bands_of(&it, source),
            level: level(source).to_string(),
        });
    }

    // cloud is cloud. the scene classification (SCL) ships only with L2A, so a
    // detector reading L1C radiometry had bands.scl = None and wrote an empty
    // cloud mask for every scene — which is why ops/clouds carries almost nothing
    // from the flare method and 9,595 of 9,603 clusters have no clear-sky
    // denominator. the mask does not depend on the radiometry we detect in, so
    // resolve it from the L2A twin of the same acquisition (same tile, same day)
    // whatever --source asks for. a failed twin search degrades to no mask rather
    // than failing the run, and scenes with no L2A counterpart (some pre-2018)
    // keep scl: None — unrated, which is honest, not counted as clear.
    if level(source) == "l1c" {
        // propagate, don't degrade. post_page already retries transient failures
        // STAC_ATTEMPTS times with backoff, so an Err here is a real outage — and
        // swallowing it would hand back scenes with no mask, which write an EMPTY
        // cloud record indistinguishable from "we looked and nothing was clear".
        // a partial mask is the worst case of all: it undercounts n_clear_obs and
        // silently OVERSTATES persistence. fail the run instead.
        let twins = discover(area, start, end, l2a_twin(source))?;
        let scl: std::collections::HashMap<String, String> = twins
            .into_iter()
            .filter_map(|t| {
                t.bands
                    .scl
                    .map(|href| (format!("{}_{}", t.mgrs, t.date), href))
            })
            .collect();
        for it in out.iter_mut() {
            if it.bands.scl.is_none() {
                it.bands.scl = scl.get(&format!("{}_{}", it.mgrs, it.date)).cloned();
            }
        }
        // a genuine gap, not a failure: L2A is not global before ~2018. those looks
        // stay unrated (write_clouds records them status "unavailable"), but they
        // are missing denominator, so say how many rather than letting it pass.
        let missing = out.iter().filter(|it| it.bands.scl.is_none()).count();
        if missing > 0 {
            eprintln!(
                "  cloud: {missing}/{} scenes have no l2a counterpart — unrated, not clear",
                out.len()
            );
        }
    }
    Ok(out)
}

/// One feature's scenes straight from the catalogue: discovery over its envelope,
/// then the run's cloud threshold. A run with a manifest never reaches this.
pub fn search(
    bbox: [f64; 4],
    start: &str,
    end: &str,
    max_cloud_cover: f64,
    source: &str,
) -> Result<Vec<Item>, String> {
    Ok(filter_cloud(
        discover(&[bbox], start, end, source)?,
        max_cloud_cover,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(bbox: [f64; 4], cloud: f64) -> Item {
        Item {
            id: "S2A_MSIL1C_x".into(),
            date: "2026-06-01".into(),
            datetime: "2026-06-01T10:00:00Z".into(),
            cloud_cover: Some(cloud),
            mgrs: "30UXB".into(),
            epsg: 32630,
            bbox,
            sun_elevation: None,
            sun_azimuth: None,
            bands: Bands {
                b01: None, b02: None, b03: None, b04: None, b05: None, b06: None,
                b07: None, b08: None, b12: None, b11: None, b8a: None, b09: None,
                b10: None, scl: None, product_metadata: None, granule_metadata: None,
            },
            level: "l1c".into(),
        }
    }

    /// Selecting a feature's scenes out of a manifest has to answer what a search for
    /// that feature alone would have: the same envelope test, including the guard that
    /// drops a degenerate antimeridian tile which would otherwise intersect everything.
    #[test]
    fn selection_matches_what_discovery_keeps() {
        let feature = [-1.0, 51.0, -0.9, 51.1];
        assert!(meets(&at([-1.5, 50.5, -0.5, 51.5], 0.0), feature)); // covers it
        assert!(meets(&at([-1.0, 51.0, -0.9, 51.1], 0.0), feature)); // exactly it
        assert!(!meets(&at([2.0, 51.0, 3.0, 51.5], 0.0), feature)); // elsewhere
        assert!(!meets(&at([-179.6, 50.0, 180.0, 52.0], 0.0), feature)); // world-wide junk
    }

    /// A point AOI has a zero-area envelope, which the catalogue rejects and which a
    /// naive intersection test would also fail.
    #[test]
    fn a_point_feature_still_meets_its_scene() {
        let point = [-1.0, 51.0, -1.0, 51.0];
        assert!(meets(&at([-1.5, 50.5, -0.5, 51.5], 0.0), point));
    }

    /// The threshold is applied after discovery, never during it, so one manifest
    /// serves any --cloud.
    #[test]
    fn cloud_filter_is_a_pure_function_of_the_scene_list() {
        let scenes = vec![at([-1.0, 51.0, -0.9, 51.1], 10.0), at([-1.0, 51.0, -0.9, 51.1], 80.0)];
        assert_eq!(filter_cloud(scenes.clone(), 100.0).len(), 2);
        assert_eq!(filter_cloud(scenes.clone(), 50.0).len(), 1);
        assert_eq!(filter_cloud(scenes, 5.0).len(), 0);
    }

    /// Overlapping parts of a MultiPolygon make GEOS throw, and the endpoint turns that
    /// into a 500 that no amount of retrying fixes — this is what made a batch of
    /// neighbouring sites fail outright.
    #[test]
    fn overlapping_envelopes_are_merged_before_they_are_queried() {
        // two sites a few km apart, envelopes overlapping
        let merged = disjoint(&[[51.5, 25.8, 51.6, 25.9], [51.55, 25.85, 51.65, 25.95]]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0], [51.5, 25.8, 51.65, 25.95]);
        // a chain merges transitively, however the pairs are ordered
        let chain = disjoint(&[
            [0.0, 0.0, 1.0, 1.0],
            [4.0, 4.0, 5.0, 5.0],
            [0.5, 0.5, 4.5, 4.5],
        ]);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], [0.0, 0.0, 5.0, 5.0]);
        // genuinely separate sites stay separate, so the batch keeps its precision
        let apart = disjoint(&[[0.0, 0.0, 1.0, 1.0], [10.0, 10.0, 11.0, 11.0]]);
        assert_eq!(apart.len(), 2);
        // and a merged batch that collapses to one envelope goes back to a bbox query
        let one = payload(
            &[[51.5, 25.8, 51.6, 25.9], [51.55, 25.85, 51.65, 25.95]],
            "2026-01-01", "2026-01-31", "cdse-l1c",
        );
        assert!(one["bbox"].is_array());
        assert!(one["intersects"].is_null());
    }

    /// L2A items are much larger and the catalogue refuses a page over 200; asking for
    /// 1000 there fails the whole search.
    #[test]
    fn page_size_respects_each_collection() {
        assert_eq!(page_limit("cdse-l1c"), 1000);
        assert_eq!(page_limit("aws-l1c"), 1000);
        assert_eq!(page_limit("cdse"), 200);
    }

    /// Many envelopes go as one MultiPolygon, which is the batching the whole manifest
    /// exists to make possible; one stays a plain bbox query.
    #[test]
    fn one_envelope_is_a_bbox_and_many_are_a_multipolygon() {
        let one = payload(&[[-1.0, 51.0, -0.9, 51.1]], "2026-01-01", "2026-01-31", "cdse-l1c");
        assert!(one["bbox"].is_array());
        assert!(one["intersects"].is_null());
        assert_eq!(one["limit"], 1000);

        let many = payload(
            &[[-1.0, 51.0, -0.9, 51.1], [2.0, 48.0, 2.1, 48.1]],
            "2026-01-01", "2026-01-31", "cdse-l1c",
        );
        assert!(many["bbox"].is_null());
        assert_eq!(many["intersects"]["type"], "MultiPolygon");
        assert_eq!(many["intersects"]["coordinates"].as_array().unwrap().len(), 2);
        // a closed ring per envelope
        assert_eq!(many["intersects"]["coordinates"][0][0].as_array().unwrap().len(), 5);
    }
}
