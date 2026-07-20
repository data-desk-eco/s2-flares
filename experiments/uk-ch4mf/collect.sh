#!/bin/sh
# join the ch4 per-scene rows with the labelled pairs → results.csv, one row per
# (pair, tile). the headline number is the separation between the barrow t_stat
# and the artefact t distribution.
cd "$(dirname "$0")" || exit 1
duckdb -c "
COPY (
  SELECT p.site, p.date, p.label, round(p.mars_p, 2) AS mars_p,
         round(p.mars_flux_kg_h) AS mars_flux, r.mgrs, r.status,
         round(r.wind_speed, 1) AS wind_ms, round(r.t_stat, 2) AS t,
         round(r.t_upwind, 2) AS t_up, r.n_plume_px AS px, r.detected,
         round(r.flux_kg_h) AS flux, round(r.flux_std_kg_h) AS flux_std,
         r.lit_hot_px AS lit, round(r.clear_frac, 2) AS clear, r.bg_passes
  FROM read_csv('pairs.csv', header=true) p
  LEFT JOIN read_csv('out/ch4/*/*.csv', header=true, union_by_name=true, nullstr='') r
    ON r.site = p.site AND r.date = p.date
  ORDER BY p.label DESC, t DESC NULLS LAST
) TO 'results.csv' (FORMAT CSV, HEADER);
SELECT label, count(*) AS rows, sum(CASE WHEN detected=1 THEN 1 ELSE 0 END) AS detections,
       round(max(t_stat), 2) AS max_t
FROM read_csv('out/ch4/*/*.csv', header=true, union_by_name=true, nullstr='') r
JOIN read_csv('pairs.csv', header=true) p ON r.site=p.site AND r.date=p.date
GROUP BY label;
"
