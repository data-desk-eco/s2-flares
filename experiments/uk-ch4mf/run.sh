#!/bin/sh
# uk ch4mf benchmark driver (UK_REVIEW.md experiment): one matched-filter pass
# per labelled (site, date) pair from pairs.csv. resumable — per-scene csv
# presence == done, backgrounds cached per site+tile under out/bg/.
cd "$(dirname "$0")" || exit 1
export AWS_ACCESS_KEY_ID="$CDSE_S3_ACCESS_KEY" AWS_SECRET_ACCESS_KEY="$CDSE_S3_SECRET_KEY"
tail -n +2 pairs.csv | while IFS=, read -r site lon lat date label _; do
  echo "$site,$lon,$lat" >"/tmp/ch4-site-$site.csv"
  ~/Tools/s2-flares-ch4mf/target/release/s2-flares ch4 --sites "/tmp/ch4-site-$site.csv" \
    --start "$date" --end "$date" --source cdse --out out --dxch4 --concurrency 4
done
