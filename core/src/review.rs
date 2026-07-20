//! deterministic plume-candidate triage — the review layer recovered from the
//! human-reviewed uk-gas-onshore runs (UK_REVIEW.md). pure compute over scalars
//! flattened from the disposable retrievals view: canonical records are never
//! mutated; the output ranks candidates for data/valid-plumes.txt curation.
//! this restores precision, not recall — it is triage for mars-s2l, not a fix.
//! max probability is a floor only, never a ranking weight (uk artefacts scored
//! 0.91–0.99 while the credible vents sat near 0.7).

pub const P_FLOOR: f64 = 0.6;
pub const FLUX_ZERO_KG_H: f64 = 1.0; // quantification collapse
pub const FLUX_CAP_KG_H: f64 = 20_000.0; // real recurring uk sources: 0.2–1.4 t/h
pub const ANCHOR_MAX_M: f64 = 500.0; // plume onset must sit near the facility
pub const TAIL_MAX_DEG: f64 = 60.0; // mask decay direction vs geos-fp bearing
pub const MIN_DISP_M: f64 = 30.0; // below this the tail direction is undefined
pub const STRONG_WIND_MS: f64 = 5.0; // undisplaced under this wind → artefact
pub const RECUR_RADIUS_M: f64 = 250.0; // fixed-offset recurrence radius
pub const RECUR_WIND_DEG: f64 = 45.0; // …under winds at least this different
pub const SUN_FLOOR_DEG: f64 = 20.0; // winter low-sun regime
pub const CLEAR_FLOOR_PCT: f64 = 40.0; // cloud-residue floor
pub const COLLINEAR_REJECT: f64 = 0.5; // boundary-following mask
pub const COLLINEAR_FLAG: f64 = 0.25;

/// one detected plume feature, flattened from the retrievals view.
#[derive(Clone, Debug, Default)]
pub struct PlumeCandidate {
    /// `target:date:plume-id` — the valid-plumes.txt curation key.
    pub key: String,
    pub target: String,
    pub date: String,
    /// probability-weighted mask centroid (lon, lat).
    pub centre: [f64; 2],
    /// mask point nearest the target facility (lon, lat) — the plume onset.
    pub origin: [f64; 2],
    /// onset → facility distance, metres.
    pub anchor_m: f64,
    /// geos-fp wind (u east, v north), m/s.
    pub wind: [f64; 2],
    pub max_p: f64,
    pub flux_kg_h: f64,
    pub clear_percent: f64,
    pub sun_elevation: Option<f64>,
    pub swath_edge: bool,
    /// background scene from a different relative orbit than the scene: the
    /// pairing views the surface from another angle, and spectrally uneven
    /// brdf injects ±2% mbmp fields — plume-sized artefacts (barrow null test:
    /// the no-event 04-13 R037→R080 pair reproduced the event signature).
    pub cross_orbit_bg: bool,
    /// osm linear-feature score: buffered-overlap fraction × principal-axis
    /// alignment (None when no lines or pixel asset were available).
    pub collinearity: Option<f64>,
}

#[derive(Debug)]
pub struct Verdict {
    /// index into the input slice (for reporting candidate metrics).
    pub idx: usize,
    pub key: String,
    /// ranking score in [0, 1]; 0 for rejects.
    pub score: f64,
    pub rejects: Vec<&'static str>,
    pub flags: Vec<&'static str>,
}

/// equirectangular displacement from → to, metres (fine at chip scale).
fn local_m(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let lat = ((from[1] + to[1]) / 2.0).to_radians();
    [
        (to[0] - from[0]) * 111_320.0 * lat.cos(),
        (to[1] - from[1]) * 110_540.0,
    ]
}

pub fn dist_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = local_m(a, b);
    d[0].hypot(d[1])
}

/// unsigned angle between two vectors, degrees (None for a null vector).
fn angle_deg(a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let (na, nb) = (a[0].hypot(a[1]), b[0].hypot(b[1]));
    (na > 0.0 && nb > 0.0).then(|| {
        ((a[0] * b[0] + a[1] * b[1]) / (na * nb))
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    })
}

/// score every candidate. cross-candidate checks (recurrence at a fixed offset,
/// same-day multi-site regimes) see the whole set. sorted: survivors by score
/// descending, then rejects; ties break on key.
pub fn triage(c: &[PlumeCandidate]) -> Vec<Verdict> {
    use std::collections::{HashMap, HashSet};
    // scene-day regime: one acquisition day firing ≥2 targets flags them all.
    let mut targets_by_date: HashMap<&str, HashSet<&str>> = HashMap::new();
    for x in c {
        targets_by_date
            .entry(&x.date)
            .or_default()
            .insert(&x.target);
    }
    // fixed-offset recurrence: same target, same mask location on another date,
    // under a meaningfully different wind → fixed dark feature, not a plume.
    let recurrent: Vec<bool> = c
        .iter()
        .map(|a| {
            c.iter().any(|b| {
                a.target == b.target
                    && a.date != b.date
                    && dist_m(a.centre, b.centre) < RECUR_RADIUS_M
                    && angle_deg(a.wind, b.wind).is_some_and(|d| d > RECUR_WIND_DEG)
            })
        })
        .collect();

    let mut out: Vec<Verdict> = c
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let speed = x.wind[0].hypot(x.wind[1]);
            let disp = local_m(x.origin, x.centre);
            let disp_len = disp[0].hypot(disp[1]);
            let tail_deg = if disp_len >= MIN_DISP_M {
                angle_deg(disp, x.wind)
            } else {
                None
            };
            let mut rejects = Vec::new();
            let mut flagged: Vec<(&'static str, f64)> = Vec::new();
            if x.max_p < P_FLOOR {
                rejects.push("p-floor");
            }
            if x.flux_kg_h <= FLUX_ZERO_KG_H {
                rejects.push("zero-flux");
            }
            if recurrent[i] {
                rejects.push("recurrent");
            }
            match x.collinearity {
                Some(v) if v >= COLLINEAR_REJECT => rejects.push("collinear"),
                Some(v) if v >= COLLINEAR_FLAG => flagged.push(("boundary", 0.25)),
                _ => {}
            }
            if x.anchor_m > ANCHOR_MAX_M {
                flagged.push(("unanchored", 0.25));
            }
            if tail_deg.is_some_and(|d| d > TAIL_MAX_DEG) {
                flagged.push(("crosswind", 0.25));
            }
            if disp_len < MIN_DISP_M && speed >= STRONG_WIND_MS {
                flagged.push(("static", 0.2));
            }
            if targets_by_date[x.date.as_str()].len() >= 2 {
                flagged.push(("scene-regime", 0.2));
            }
            if x.flux_kg_h > FLUX_CAP_KG_H {
                flagged.push(("magnitude", 0.2));
            }
            if x.sun_elevation.is_some_and(|s| s < SUN_FLOOR_DEG) {
                flagged.push(("low-sun", 0.15));
            }
            if x.clear_percent < CLEAR_FLOOR_PCT {
                flagged.push(("cloud-residue", 0.1));
            }
            if x.swath_edge {
                flagged.push(("swath-edge", 0.1));
            }
            if x.cross_orbit_bg {
                flagged.push(("cross-orbit-bg", 0.25));
            }
            let score = if rejects.is_empty() {
                (1.0 - flagged.iter().map(|f| f.1).sum::<f64>()).max(0.0)
            } else {
                0.0
            };
            Verdict {
                idx: i,
                key: x.key.clone(),
                score,
                rejects,
                flags: flagged.into_iter().map(|f| f.0).collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.rejects
            .is_empty()
            .cmp(&a.rejects.is_empty())
            .then(b.score.total_cmp(&a.score))
            .then(a.key.cmp(&b.key))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // a barrow-like clean candidate: anchored on the facility, tail downwind,
    // compact, plausible flux, clear high-sun scene, non-repeating.
    fn clean(key: &str, date: &str) -> PlumeCandidate {
        PlumeCandidate {
            key: key.into(),
            target: key.split(':').next().unwrap().into(),
            date: date.into(),
            centre: [-3.230, 54.096], // ~330 m east of origin
            origin: [-3.235, 54.096],
            anchor_m: 40.0,
            wind: [8.0, 0.0], // toward east — aligned with the tail
            max_p: 0.97,
            flux_kg_h: 4700.0,
            clear_percent: 100.0,
            sun_elevation: Some(35.0),
            swath_edge: false,
            cross_orbit_bg: false,
            collinearity: Some(0.05),
        }
    }

    #[test]
    fn clean_candidate_scores_one() {
        let v = triage(&[clean("barrow:2026-03-14:plume-1", "2026-03-14")]);
        assert!(v[0].rejects.is_empty() && v[0].flags.is_empty());
        assert_eq!(v[0].score, 1.0);
    }

    #[test]
    fn probability_is_a_floor_not_a_weight() {
        let mut low = clean("a:d:p", "2026-01-01");
        low.max_p = 0.59;
        let mut mid = clean("b:d:p", "2026-01-02");
        mid.max_p = 0.7; // credible-vent territory
        let v = triage(&[low, mid]);
        assert_eq!(v[0].key, "b:d:p");
        assert_eq!(v[0].score, 1.0); // 0.7 ranks no lower than 0.97 would
        assert_eq!(v[1].rejects, ["p-floor"]);
    }

    #[test]
    fn magnitude_prior() {
        let mut zero = clean("z:d:p", "2026-01-01");
        zero.flux_kg_h = 0.0;
        let mut huge = clean("h:d:p", "2026-01-02");
        huge.flux_kg_h = 54_000.0;
        let v = triage(&[zero, huge]);
        assert_eq!(v[0].flags, ["magnitude"]);
        assert_eq!(v[1].rejects, ["zero-flux"]);
    }

    #[test]
    fn wind_terms() {
        let mut upwind = clean("u:d:p", "2026-01-01");
        upwind.wind = [-8.0, 0.0]; // tail points east, wind blows west
        let mut pinned = clean("s:d:p", "2026-01-02");
        pinned.centre = pinned.origin; // undisplaced under 8 m/s
        let mut far = clean("f:d:p", "2026-01-03");
        far.anchor_m = 900.0; // alrewas: 900 m from the station
        let v = triage(&[upwind, pinned, far]);
        let by = |k: &str| v.iter().find(|x| x.key.starts_with(k)).unwrap();
        assert_eq!(by("u").flags, ["crosswind"]);
        assert_eq!(by("s").flags, ["static"]);
        assert_eq!(by("f").flags, ["unanchored"]);
    }

    #[test]
    fn recurrence_needs_different_winds() {
        let mut a = clean("churchover:2026-03-18:plume-1", "2026-03-18");
        let mut b = clean("churchover:2026-04-30:plume-1", "2026-04-30");
        b.wind = [0.0, 8.0]; // 90° apart, mask pinned at the same offset
        a.target = "churchover".into();
        b.target = "churchover".into();
        let v = triage(&[a.clone(), b.clone()]);
        assert!(v.iter().all(|x| x.rejects == ["recurrent"]));
        b.wind = a.wind; // same wind → could be a continuous source
        let v = triage(&[a, b]);
        assert!(v.iter().all(|x| x.rejects.is_empty()));
    }

    #[test]
    fn scene_day_regime_flags_all_sites() {
        let mut a = clean("wooler:2026-01-10:plume-1", "2026-01-10");
        let mut b = clean("holford:2026-01-10:plume-1", "2026-01-10");
        a.target = "wooler".into();
        b.target = "holford".into();
        let v = triage(&[a, b]);
        assert!(v.iter().all(|x| x.flags == ["scene-regime"]));
    }

    #[test]
    fn cross_orbit_background_is_flagged() {
        let mut x = clean("barrow:2026-03-14:plume-1", "2026-03-14");
        x.cross_orbit_bg = true; // r080 scene differenced against an r037 look
        let v = triage(&[x]);
        assert_eq!(v[0].flags, ["cross-orbit-bg"]);
        assert!(v[0].rejects.is_empty()); // flag, not veto: barrow itself is real
    }

    #[test]
    fn collinear_and_hygiene() {
        let mut road = clean("r:d:p", "2026-01-01");
        road.collinearity = Some(0.8);
        let mut winter = clean("w:d:p", "2026-01-02");
        winter.sun_elevation = Some(12.0);
        winter.clear_percent = 30.0;
        winter.swath_edge = true;
        let v = triage(&[road, winter]);
        assert_eq!(v[0].flags, ["low-sun", "cloud-residue", "swath-edge"]);
        assert!((v[0].score - 0.65).abs() < 1e-9);
        assert_eq!(v[1].rejects, ["collinear"]);
        assert_eq!(v[1].score, 0.0);
    }
}
