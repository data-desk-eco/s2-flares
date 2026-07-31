//! stac search — 1:1 port of lib/stac.js. two source profiles (aws element84 cog
//! hrefs; cdse copernicus eopf s3://eodata jp2). blocking http (ureq) + serde_json
//! so the fan-out can be plain rayon threads, no async runtime.

use std::time::Duration;

use s2e_core::epsg_from_mgrs;
use serde_json::Value;

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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
    pub level: &'static str,
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

fn retryable(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::Transport(_) => true,
        ureq::Error::Status(code, _) => *code == 408 || *code == 429 || *code >= 500,
    }
}

/// POST one STAC page, retrying transient transport/HTTP failures and malformed
/// responses. CDSE intermittently returns 500/504 under bulk load; failing an AOI
/// search silently omits that site, so retry each pagination request in place rather
/// than forcing a costly second run over the entire AOI catalogue.
fn post_page(url: &str, body: &Value) -> Result<Value, String> {
    for attempt in 1..=STAC_ATTEMPTS {
        let result = ureq::post(url)
            .set("Content-Type", "application/json")
            .send_json(body.clone());
        let err = match result {
            Ok(resp) => match resp.into_json() {
                Ok(data) => return Ok(data),
                Err(e) => format!("stac json: {e}"),
            },
            Err(e) if retryable(&e) => format!("stac http: {e}"),
            Err(e) => return Err(format!("stac http: {e}")),
        };
        if attempt == STAC_ATTEMPTS {
            return Err(format!("{err} after {STAC_ATTEMPTS} attempts"));
        }
        let delay = 1u64 << (attempt - 1).min(4); // 1, 2, 4, 8, then 16 s
        eprintln!(
            "  stac transient failure; retry {}/{} in {delay}s: {err}",
            attempt + 1,
            STAC_ATTEMPTS
        );
        std::thread::sleep(Duration::from_secs(delay));
    }
    unreachable!()
}

/// search a date window over a bbox, dedup by mgrs tile + date keeping lowest
/// cloud cover, return normalised items (cloud cover ≤ max_cloud_cover).
pub fn search(
    bbox: [f64; 4],
    start: &str,
    end: &str,
    max_cloud_cover: f64,
    source: &str,
) -> Result<Vec<Item>, String> {
    let base = api(source);
    // A GeoJSON Point has a zero-area envelope, which STAC APIs reject.  The
    // plume reader still uses its fixed 2 km chip; this epsilon only makes the
    // catalogue intersection well-defined.
    let mut query_bbox = bbox;
    if query_bbox[0] >= query_bbox[2] {
        query_bbox[0] -= 1e-6;
        query_bbox[2] += 1e-6;
    }
    if query_bbox[1] >= query_bbox[3] {
        query_bbox[1] -= 1e-6;
        query_bbox[3] += 1e-6;
    }
    let payload = serde_json::json!({
        "collections": [format!("sentinel-2-{}", level(source))],
        "bbox": query_bbox,
        "datetime": format!("{start}T00:00:00Z/{end}T23:59:59Z"),
        "limit": 100,
    });

    let mut features: Vec<Value> = Vec::new();
    let mut url = format!("{base}/search");
    let mut body = payload;
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

    // cdse occasionally returns antimeridian tiles for queries anywhere on the
    // globe (seen: pacific t01/t60 items for a uk point search, with degenerate
    // [-179.57..180] bboxes that intersect everything). a stray scene computes a
    // chip window half a world from its raster and ooms the box, so drop items
    // whose bbox misses the query envelope or is wider than any real s2 tile.
    features.retain(|it| {
        it["bbox"].as_array().is_none_or(|b| {
            let v: Vec<f64> = b.iter().filter_map(|x| x.as_f64()).collect();
            v.len() != 4
                || (v[2] - v[0] < 5.0
                    && v[0] <= query_bbox[2]
                    && v[2] >= query_bbox[0]
                    && v[1] <= query_bbox[3]
                    && v[3] >= query_bbox[1])
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
        let cloud = p["eo:cloud_cover"].as_f64();
        if cloud.unwrap_or(100.0) > max_cloud_cover {
            continue;
        }
        out.push(Item {
            id: it["id"].as_str().unwrap_or("").to_string(),
            date: p["datetime"]
                .as_str()
                .unwrap_or("")
                .get(..10)
                .unwrap_or("")
                .to_string(),
            datetime: p["datetime"].as_str().unwrap_or("").to_string(),
            cloud_cover: cloud,
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
            level: level(source),
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
        let twins = search(bbox, start, end, 100.0, l2a_twin(source))?;
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
