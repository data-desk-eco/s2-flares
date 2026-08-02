# uk retrieval review — findings, deterministic fixes, ch4mf test plan

context: the 2026 + 2025 `aoi/uk-gas-onshore.geojson` runs (33 targets: 23 nts
compressor stations, 7 storage sites, 3 processing terminals; ~16k scene
records) produced ~30 positive plume records from the mars-s2l detector. human
review rejected every one. the sole confirmed uk true positive remains barrow
2026-03-14 (`pipeline-barrow:2026-03-14:plume-1`, 4.7 t/h, max p 0.97, 458 px,
from the earlier import-terminals run). this doc records what failed, the
deterministic review layer we want, and the first experiment to run.

working materials:
- `out/uk-onshore-hits/NOTES.md` — full per-candidate triage log
- `out/uk-onshore-hits/raw/<target>/<scene>/` — positive records + prob rasters
- `out/uk-onshore-hits/chips/` — truecolor/b12 chips + overlay composites
  (box-side extraction recipe in the shell history of NOTES; chips are 3.2 km,
  480 px, EPSG:4326, gdalwarp from /vsis3/eodata jp2s)
- review artifact (visual): https://claude.ai/code/artifact/bdc473b6-5ec0-4433-a38c-731f9d3ab849
- method fingerprints: 2026 records `plumes-c0a72d80c3560a56`, 2025 records
  `plumes-a3ae536e28fcb74a` (same parameters; the version string changed 2.0.0
  → 0.1.0 mid-day and it participates in the fingerprint hash)

## failure taxonomy (all human-confirmed)

1. boundary-following masks — mask edges snap to roads / field boundaries /
   hedgerows over hundreds of metres. mechanism: sub-pixel registration
   residual + boundary-organised surface change in the b12/b11 differencing;
   the cnn's elongated-anomaly prior then scores it as a plume. examples:
   alrewas 2026-04-23 (quarry road/boundary, 900 m from station), aylesbury
   (several).
2. fixed dark features — swir-dark surfaces re-flagged on many dates under
   different winds, mask pinned at a constant offset. examples: churchover ×4
   (patchy bare-soil field by the nts junction), warrington 2026-04-30 (risley
   moss peat), point-of-ayr 2026-03-14 (dee estuary foreshore), aberdeen ×2
   (forest east of the site, "repeated identically"), holford 2026-01-10
   (brinefield).
3. scene-day regimes — one acquisition fires several sites at once; winter low
   sun and swath edges. observed: 2026-01-10 (wooler+holford), 2026-03-18/19
   (churchover+huntingdon, abson+wormington), 2026-04-30
   (warrington+wormington+atwick), 2026-06-24 (aylesbury+atwick).
4. wind-inconsistent placement — mask upwind of the site, or undisplaced under
   a strong (8 m/s) wind.
5. implausible magnitudes — 20–54 t/h retrievals with no third-party
   corroboration (ghgsat's real recurring uk sources are 0.2–1.4 t/h), and
   zero-flux records where quantification collapsed.

barrow passes every test: compact (458 px), origin on the pipeline, elongated
downwind, non-repeating, plausible magnitude. the discriminators are real.

## Deterministic review layer

A scored verdict pass over `data-desk/retrievals` that never mutates
canonical records; output is a ranked candidate list for `data/valid-plumes.txt`
curation. checks, strongest first:

- linear-feature collinearity: buffer osm roads/boundaries/waterways/hedges by
  ~1 px; score fraction of mask pixels inside + alignment of the mask's
  principal axis with the feature. high overlap → reject.
- wind consistency, two separate terms (a naive centroid-vs-wind test
  misclassifies continuous sources):
  - origin anchoring: mask onset within n pixels of the target facility;
  - tail direction: mask decay direction vs geos-fp wind bearing.
- recurrence at fixed offset: same mask location across dates under different
  winds (query the archive) → fixed-feature artefact.
- same-day multi-site veto: ≥2 targets positive on one acquisition day →
  scene-regime flag on all of them.
- magnitude prior: flux > ~20 t/h uncorroborated → flag; flux ≈ 0 → reject.
- scene hygiene: sun-elevation floor (winter), swath-edge flag, clear-percent
  floor, cloud residue.
- max probability is a floor only (reject < ~0.6), never a ranking weight:
  artefacts scored 0.91–0.99 while the credible-looking vents scored ~0.7.
- optional upgrade: dual-geometry confirmation for persistent-source claims —
  require the signal under both relative orbits (r037/r137 for the midlands).

limitation to keep in mind: this layer restores precision, not recall. it is
triage for mars-s2l, not a fix.

## first experiment: validate ch4mf against the labelled benchmark

`~/Tools/s2-flares-ch4mf` (untested) contains `core/src/methane.rs`: a
physics matched-filter detector — mbmp double-ratio → ΔXCH4 via a calibrated
beer–lambert inversion → wind-aligned plume template anchored at the known
source → glrt amplitude test → ime quantification. structurally it inverts the
problem: instead of segment-then-check, it asks whether the ΔXCH4 field
projects onto a source-anchored wind-aligned template. modes 1, 2 and 4 above
decorrelate from the template by construction; the glrt gives a calibrated
false-alarm rate.

test protocol (the run gave us an adversarial labelled set):
1. true-positive gate: barrow 2026-03-14, s2a r080 t30uve, site
   (-3.235, 54.096) approx — must detect, flux within ~2× of 4.7 t/h.
   (also the published fixture t_emit_227 2024-10-25 if a second tp is wanted.)
2. false-positive gate: run the same site/date pairs as every rejected
   candidate in `out/uk-onshore-hits/NOTES.md` (churchover ×4, aylesbury ×6,
   aberdeen ×3, alrewas, chelmsford, warrington, wooler, holford, atwick ×2,
   wormington ×2, hole-house, point-of-ayr, felindre, cambridge, huntingdon,
   abson) — target zero detections at the same glrt threshold.
3. report per-candidate glrt t-statistics alongside mars-s2l's max p — the
   separation between the barrow t and the artefact t distribution is the
   headline result.
4. known weaknesses to probe: geos-fp wind-direction error (fit the template
   over a ± sector, not a single bearing); surface change leaking through the
   mbmp background (see reference-image work below).

## reference-image improvements (for whichever detector wins)

ranked by expected value over the uk:
1. temporal z-scoring over the clear-scene stack instead of single-background
   differencing: per-pixel mean/σ of the band ratio; detect in σ units. noisy
   boundary/field pixels self-downweight via their variance. biggest single fix.
2. robust multi-scene composite background (per-pixel median of n clear
   scenes) — cheap, kills transient surface state.
3. seasonal matching for background selection: minimise Δsun-elevation +
   Δndvi (same month across years) rather than nearest-date-clear. fixes the
   winter regime.
4. sub-pixel co-registration refinement (phase correlation per chip) before
   differencing — attacks the boundary-residual mechanism directly.
5. embeddings (modest expectations): background *selection* (nearest embedding
   = most similar surface state) and change *masking* (embedding distance
   flags surface-change pixels to exclude). not for the retrieval itself.

## addendum (2026-07-20): experiment ran — see experiments/uk-ch4mf/RESULTS.md

- ch4mf fp gate passed (0/26 rejected pairs at t≥4, max artefact t 3.61); tp
  gate not passed (barrow t≈2.2 — the detector's composite-background noise is
  ~6× the single-pair floor; precision-only at uk noise).
- barrow 2026-03-14 was challenged and survived physics review (~90% real,
  flux ~2–10 t/h): clean same-orbit mbmp is the only negative among null
  day-pairs, the model rejects the 04-13 artefact-replica input, and a remit
  umm outage (13–15 mar, flows 31→2 GWh/d) corroborates pulsed venting.
- new mechanism found: **cross-orbit backgrounds** (published nearest-date
  selection is orbit-blind) inject ±2% brdf mbmp fields — this is the
  scene-day-regime engine and inflated barrow's quantification. `review` now
  flags cross-orbit-bg records; `S2_KEEP_ORBITS=1` enables dual-look runs.

## cdse quirks fixed during the run (already in cli/src/stac.rs)

- pre-2026 l1c stac items omit the B10 asset; derived from B11's href.
- the 2025-01-31→02-07 index window returns antimeridian tiles (t01/t60) with
  degenerate world-spanning bboxes for any query; filtered by envelope
  intersection + a 5° bbox-width cap. unfiltered they oom an 8 gb box (chip
  window computed half a world from its raster).
