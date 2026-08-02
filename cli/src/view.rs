//! the clustering step's i/o. duckdb owns the parquet+s3 reads and writes (the
//! stated analytics layer); rust core owns the clustering. the seam is a flat
//! csv handoff — no native parquet deps.
//!
//! `cluster` is not a publisher: it reads the staging detections and the cloud
//! mask, and writes one row per site and one row per detection that joined one.
//! nothing here counts days, because only the caller holds both the detections
//! and the observations and can count them over the same window.

use s2e_core::{Cluster, Detection};
use std::process::{Command, Stdio};

pub(crate) fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("s2flares-{}-{name}", std::process::id()))
}

pub(crate) fn duckdb(sql: &str) -> Result<(), String> {
    let st = Command::new("duckdb")
        .arg("-c")
        .arg(sql)
        .status()
        .map_err(|e| format!("duckdb spawn: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err("duckdb exited non-zero".into())
    }
}

// duckdb s3 prelude. with `S2_S3_ENDPOINT` set (the box exports it for CloudFerro)
// we configure a path-style endpoint + creds; otherwise bare httpfs leans on the
// aws default credential chain (local/AWS reads). prepended to every s3-touching sql.
pub(crate) fn s3_prelude() -> String {
    let mut p = String::from("INSTALL httpfs; LOAD httpfs; ");
    if let Ok(ep) = std::env::var("S2_S3_ENDPOINT") {
        let g = |k| std::env::var(k).unwrap_or_default();
        // duckdb's archive creds are kept SEPARATE from AWS_* so the same process can
        // also drive gdal /vsis3 against eodata (AWS_*) during a detect run: the
        // duckdb (project-bucket) creds come from S2_S3_* first, falling back to AWS_*.
        let key = |s2, aws| {
            let v = g(s2);
            if v.is_empty() {
                g(aws)
            } else {
                v
            }
        };
        p += &format!(
            "SET s3_endpoint='{ep}'; SET s3_region='{}'; SET s3_url_style='path'; \
            SET s3_use_ssl=true; SET s3_access_key_id='{}'; SET s3_secret_access_key='{}'; ",
            g("S2_S3_REGION"),
            key("S2_S3_ACCESS_KEY", "AWS_ACCESS_KEY_ID"),
            key("S2_S3_SECRET_KEY", "AWS_SECRET_ACCESS_KEY")
        );
    }
    p
}

fn opt_f64(s: &str) -> Option<f64> {
    match s.trim() {
        "" => None,
        "Infinity" | "inf" => Some(f64::INFINITY),
        "-Infinity" | "-inf" => Some(f64::NEG_INFINITY),
        v => v.parse().ok(),
    }
}

/// header name → field index. duckdb writes the header, so a row is addressed by
/// name: a renamed or reordered SELECT then loses one field instead of shifting
/// every field after it silently into the wrong place.
pub(crate) fn columns(header: &str) -> std::collections::HashMap<&str, usize> {
    header
        .trim_end()
        .split(',')
        .enumerate()
        .map(|(i, n)| (n, i))
        .collect()
}

/// the flare detections to cluster, over a date window. `id` comes back out on
/// the membership, so the caller can label the row each detection came from.
pub fn read_detections(path: &str, start: &str, end: &str) -> Result<Vec<Detection>, String> {
    let out = tmp("dets.csv");
    let out_s = out.to_string_lossy();
    duckdb(&format!(
        "{prelude}\
         COPY (SELECT id, lon, lat, date, max_b12, max_b11, b12_b11_ratio, radiance, \
           sun_elevation, glint_score FROM read_parquet('{path}') \
           WHERE date >= '{start}' AND date <= '{end}') TO '{out_s}' (FORMAT CSV, HEADER)",
        prelude = s3_prelude()
    ))?;
    let text = std::fs::read_to_string(&out).map_err(|e| format!("read dets: {e}"))?;
    let _ = std::fs::remove_file(&out);
    let mut lines = text.lines();
    let cols = columns(lines.next().unwrap_or_default());
    Ok(lines
        .filter(|l| !l.is_empty())
        .map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            let g = |n: &str| cols.get(n).and_then(|&i| f.get(i)).copied().unwrap_or("");
            Detection {
                id: g("id").to_string(),
                lon: g("lon").parse().unwrap_or(0.0),
                lat: g("lat").parse().unwrap_or(0.0),
                date: g("date").to_string(),
                max_b12: g("max_b12").parse().unwrap_or(0.0),
                peak_b11: opt_f64(g("max_b11")),
                b12_b11_ratio: opt_f64(g("b12_b11_ratio")),
                radiance: g("radiance").parse().unwrap_or(0.0),
                sun_elevation: opt_f64(g("sun_elevation")),
                glint_score: opt_f64(g("glint_score")),
                ..Default::default()
            }
        })
        .collect())
}

/// read the cloud mask (lon,lat,date,cloud) over a date window, restricted to
/// `cells` (grid indices round(lon/GRID_STEP), round(lat/GRID_STEP)): the
/// semi-join runs INSIDE duckdb (tiny csv build side, mask side streams), so the
/// result — which the duckdb cli materialises in full before printing — is
/// O(anchor cells × dates), not O(mask). the unfiltered mask (~25 GB materialised)
/// OOM-killed a 7 GB box even with 16 GB of swap.
pub fn read_clouds(
    path: &str,
    start: &str,
    end: &str,
    cells: &std::collections::HashSet<(i64, i64)>,
    mut sink: impl FnMut(f64, f64, &str, f64),
) -> Result<(), String> {
    let cf = tmp("cells.csv");
    let cf_s = cf.to_string_lossy();
    let body: String = std::iter::once("i,j".to_string())
        .chain(cells.iter().map(|(i, j)| format!("{i},{j}")))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&cf, body).map_err(|e| format!("write cells: {e}"))?;
    let inv = (1.0 / s2e_core::GRID_STEP).round();
    let sql = format!(
        "{prelude}SELECT DISTINCT lon, lat, date, cloud \
         FROM read_parquet('{path}') \
         JOIN read_csv('{cf_s}', header=true, columns={{'i':'BIGINT','j':'BIGINT'}}) \
           ON CAST(round(lon*{inv}) AS BIGINT)=i AND CAST(round(lat*{inv}) AS BIGINT)=j \
         WHERE date >= '{start}' AND date <= '{end}'",
        prelude = s3_prelude()
    );
    let mut child = Command::new("duckdb")
        .args(["-csv", "-noheader", "-c", &sql])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("duckdb spawn: {e}"))?;
    use std::io::BufRead;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "duckdb stdout unavailable".to_string())?;
    for line in std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
    {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 4 {
            continue;
        }
        sink(
            f[0].parse().unwrap_or(0.0),
            f[1].parse().unwrap_or(0.0),
            f[2],
            // a look we cannot read the cloud of is not a clear look: default it
            // cloudy, so an unreadable mask shrinks the denominator rather than
            // inflating every site's persistence.
            f[3].parse().unwrap_or(1.0),
        );
    }
    let status = child.wait().map_err(|e| format!("duckdb wait: {e}"))?;
    let _ = std::fs::remove_file(&cf);
    if status.success() {
        Ok(())
    } else {
        Err("duckdb exited non-zero".into())
    }
}

fn fmt(x: f64) -> String {
    if x.is_infinite() {
        if x < 0.0 {
            "-Infinity".into()
        } else {
            "Infinity".into()
        }
    } else {
        format!("{x}")
    }
}
fn fo(x: Option<f64>) -> String {
    x.map(fmt).unwrap_or_default()
}

// the cluster row, named and typed once: the csv header and read_csv's column
// map are both built from this, so the two cannot drift. the explicit types are
// what keep the parquet schema invariant when a run's values happen to be
// integral or a column comes out entirely null.
const CLUSTER: [(&str, &str); 15] = [
    ("id", "VARCHAR"),
    ("lon", "DOUBLE"),
    ("lat", "DOUBLE"),
    ("score", "DOUBLE"),
    ("flags", "VARCHAR"),
    ("cluster_max_b12", "DOUBLE"),
    ("cluster_avg_b12", "DOUBLE"),
    ("cluster_radiance", "DOUBLE"),
    ("median_b12_b11_ratio", "DOUBLE"),
    ("min_sun_elevation", "DOUBLE"),
    ("ratio_score", "DOUBLE"),
    ("persistence_score", "DOUBLE"),
    ("glint_penalty", "DOUBLE"),
    ("max_ratio", "DOUBLE"),
    ("min_glint", "DOUBLE"),
];
const MEMBER: [(&str, &str); 2] = [("id", "VARCHAR"), ("site_id", "VARCHAR")];

// fixed width: a column added to CLUSTER without a value here fails to compile.
fn cluster_row(c: &Cluster) -> [String; CLUSTER.len()] {
    [
        c.id.clone(),
        fmt(c.lon),
        fmt(c.lat),
        fmt(c.total_score),
        c.flags().join(";"),
        fmt(c.max_b12),
        fmt(c.avg_b12),
        fmt(c.radiance),
        fo(c.median_b12_b11_ratio),
        fo(c.min_sun_elevation),
        fmt(c.ratio_score),
        fmt(c.persistence_score),
        fmt(c.glint_penalty),
        fo(c.max_ratio),
        fo(c.min_glint),
    ]
}

fn csv(cols: &[(&str, &str)], rows: impl Iterator<Item = String>) -> String {
    std::iter::once(cols.iter().map(|c| c.0).collect::<Vec<_>>().join(","))
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn types(cols: &[(&str, &str)]) -> String {
    let body: Vec<String> = cols.iter().map(|(n, t)| format!("'{n}':'{t}'")).collect();
    format!("{{{}}}", body.join(","))
}

/// write the two flat parquet outputs: one row per cluster, and one row per
/// detection naming the cluster it joined.
pub fn write(clusters: &[Cluster], clusters_out: &str, members_out: &str) -> Result<(), String> {
    let cf = tmp("clusters.csv");
    let mf = tmp("members.csv");
    std::fs::write(
        &cf,
        csv(&CLUSTER, clusters.iter().map(|c| cluster_row(c).join(","))),
    )
    .map_err(|e| format!("write clusters: {e}"))?;
    std::fs::write(
        &mf,
        csv(
            &MEMBER,
            clusters
                .iter()
                .flat_map(|c| c.members.iter().map(|m| format!("{},{}", m.id, c.id))),
        ),
    )
    .map_err(|e| format!("write members: {e}"))?;
    // flags travels as a ;-joined scalar and lands as the archive's VARCHAR[] —
    // an empty list, never null, for a site carrying no qualifier. the coalesce
    // is what makes that true: nullstr reads the empty cell back as null, and a
    // null there would read as "unknown" where we mean "none".
    let sql = format!(
        "{prelude}\
         COPY (SELECT * REPLACE ([f FOR f IN str_split(coalesce(flags, ''), ';') IF f <> ''] AS flags) \
           FROM read_csv('{c}', header=true, nullstr='', columns={ct})) \
           TO '{clusters_out}' (FORMAT PARQUET, COMPRESSION ZSTD);\n\
         COPY (SELECT * FROM read_csv('{m}', header=true, columns={mt})) \
           TO '{members_out}' (FORMAT PARQUET, COMPRESSION ZSTD);",
        prelude = s3_prelude(),
        c = cf.to_string_lossy(),
        ct = types(&CLUSTER),
        m = mf.to_string_lossy(),
        mt = types(&MEMBER),
    );
    let r = duckdb(&sql);
    let _ = std::fs::remove_file(&cf);
    let _ = std::fs::remove_file(&mf);
    r
}
