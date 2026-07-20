# uk ch4mf benchmark + barrow physics review — results

date: 2026-07-19/20. protocol: UK_REVIEW.md "first experiment". runner:
`run.sh` over `pairs.csv` (27 labelled site/date pairs from the uk-gas-onshore
runs), collector `collect.sh` → `results.csv`. detector: `~/Tools/s2-flares-ch4mf`
`ch4` (mbmp → ΔXCH4 → wind-aligned matched filter → glrt), source cdse eodata,
era5 wind, t_min 4.

## headline results

- **fp gate: PASSED.** 0 detections over all 26 human-rejected candidate pairs
  (30 rows incl. dual-tile; max artefact t = 3.61 < 4.0). statuses: 20 ok,
  10 insufficient_valid, 2 cloudy, 2 background fails.
- **tp gate: not detected.** barrow 2026-03-14: t = 2.16 (r037) / 2.10 (r080).
  ch4mf as built is precision-only at uk noise: its composite-background ΔX
  noise is ~10,000 ppb/px against a measured single-pair floor of ~1,700 ppb
  (4.1% ratio mad) — a ~5 t/h event cannot reach t ≥ 4.
- **no correlation between ch4mf t and mars max p** (churchover p .99 → t 2.7;
  cambridge p .97 → t −0.8; point-of-ayr p .88 → t −3.7). the two detectors
  fail differently, which is what makes the physics side useful as a check.
- **wind-direction sensitivity confirmed** (predicted weakness #4): barrow t =
  2.10 (era5 bearing) / 1.65 (−30°) / 0.61 (geos-fp, +12°) / 0.40 (+30°).
  a single-bearing template is fragile; fit over a ± sector.

## barrow 2026-03-14 — physics review verdict: ~90% real, flux ~2–10 t/h

the benchmark's sole tp was challenged and survived. the evidence arc
(figures/):

1. in-mask per-pixel mbmp double-ratio: **−1.5 to −2.7% against three
   independent references** (11:21 same-day, 03-07, 03-04) — the −1.3% a
   4.7 t/h release predicts. all four clean same-orbit null day-pairs sit
   POSITIVE (+0.5..+1.5%) → the event is the only clean-pair negative
   (figures/barrow-evidence.png).
2. **the mars pairing itself is contaminated**: its background was the same-day
   other-orbit look, and the no-event 04-13 r037→r080 replica reproduces the
   in-mask signature at −2.17% (vs event −1.94%), pattern-correlated r≈0.5
   across days. cross-orbit brdf ≈ ±2% mbmp — plume-sized.
3. **the clean-background event signal is NOT that artefact** (r = 0.02 against
   the replica pattern) — it is specific to 14 march.
4. **model-level null test**: `detect` on 04-13 with the dual-look pairing
   forced (`S2_KEEP_ORBITS=1`) → mars returned zero features on the
   artefact-replica input. the cnn is not thresholding its mbmp channel.
5. **operational corroboration** (independent, timestamped): remit umm —
   barrow north terminal gas plant unavailable from 11:04 gmt 13 mar, extended
   through 14 mar, dismissed 03:00 15 mar; national gas entry flows 31 →
   2 GWh/d; further trips 16–17 and 20 mar. pulsed venting also explains the
   11:21-silent / 11:33-detected pair.
6. the legacy archive's other barrow "detections" (87–95 t/h on 03-27 with the
   event scene AS background; 160–180 t/h on 04-06) are the background stage
   failing, and were correctly rejected in review.

flux caveat: the recorded 4.7 ± 1.8 t/h is quantified against the contaminated
pairing; treat as ~2–10 t/h.

a cautionary note for future review tooling: an initial "refuted at 3.6σ" call
here came from computing difference-of-medians per band instead of the median
of the per-pixel double ratio. any band-decomposition veto must be per-pixel
and calibrated against null day-pairs, or it will execute real events.

## what this changes in s2e (landed with this commit)

- `review` gains a **cross-orbit-bg** flag: background scene from a different
  relative orbit than the scene → flagged (weight 0.25). mechanistically
  explains the scene-day regime clusters (03-18/19 etc. are adjacent-day
  r037↔r137 pairs over the midlands overlap).
- `S2_KEEP_ORBITS=1` env override on the stac dedup keeps same-day dual-look
  acquisitions — the dual-geometry confirmation tool UK_REVIEW proposed.

## defects found and fixed in s2-flares-ch4mf (uncommittable orphan worktree;
changes live in its working tree)

1. `cli/src/stac.rs` — cdse l1c asset keys are now plain `B12` (was `B12_20m`);
   every background silently failed.
2. `cli/src/methane.rs` — background window included the scene's own date.
3. `cli/src/methane.rs` — background selection was clearest-first across 12
   months (summer bg vs winter scene); now nearest-date-first.
4. `cli/src/methane.rs` — **per-band independent medians corrupted the
   background ratio** (mbmp uses the background only through per-pixel
   b12/b11); now picks the median-ratio pass per pixel. measured 5× noise
   inflation from this bug.
5. `cli/src/methane.rs` — scl joined by date; on two-orbit days the wrong
   acquisition's mask blanked the chip. now joined by acquisition key.
6. `cli/src/methane.rs` + `core/src/methane.rs` — pass acceptance gated on
   scl-clear (water counts as clear) → coastal winter backgrounds carried no
   land; now gated on retrievable fraction (≥20%), and the glrt null-support
   gate relaxed for coastal chips (≥4 nulls at 0.2·support).

## next steps (ranked)

1. same-orbit-only background selection (or a hard review veto) for both
   detectors — the single highest-value change from this work.
2. temporal z-scoring / seasonal background per UK_REVIEW's reference-image
   list — ch4mf needs a ~5× noise reduction to reach uk-relevant recall.
3. ± sector template fit for wind-direction error.
4. re-point the ch4mf tp gate at the published fixture (t_emit_227 2024-10-25)
   — the uk set now has one probable-but-marginal tp, not a clean one.
5. dual-geometry confirmation via `S2_KEEP_ORBITS` for persistent-source claims.
