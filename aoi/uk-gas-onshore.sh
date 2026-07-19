#!/usr/bin/env bash
# uk onshore gas infrastructure aoi — nts compressor stations, gas storage and
# processing terminals not already in uk-gas-import-terminals.geojson.
# curated ids from the shared features catalogue; friendly ids follow the
# compressor-/storage-/terminal- convention. rerun to rebuild the geojson.
set -euo pipefail
cd "$(dirname "$0")"
. "${DATA_DESK:-$HOME/Tools/data-desk}/store.sh"

duckdb -noheader -list -c "
WITH curated(id, kind, src) AS (VALUES
  ('compressor-aberdeen',       'compressor_station', 'MPS:mps_mapping_terminal.acce828a-4128-421d-aab2-26cf58a6b151'),
  ('compressor-alrewas',        'compressor_station', 'MPS:mps_mapping_terminal.2ae360df-bd22-4bcf-97e0-68feefc25d01'),
  ('compressor-aylesbury',      'compressor_station', 'MPS:mps_mapping_terminal.c8bae150-8141-4bef-a925-d6bebfe8235b'),
  ('compressor-bathgate',       'compressor_station', 'MPS:mps_mapping_terminal.d37941bf-2a33-40ff-8a0c-6bc1d3a3a12c'),
  ('compressor-bishop-auckland','compressor_station', 'MPS:mps_mapping_terminal.24b82e0a-892a-4a65-ba43-c72eb70b397e'),
  ('compressor-cambridge',      'compressor_station', 'MPS:mps_mapping_terminal.1a12dae7-8a89-4a4f-bdb1-e2fcfb6a6dd1'),
  ('compressor-carnforth',      'compressor_station', 'MPS:mps_mapping_terminal.52a3b400-b007-4996-97de-c7c8fa64a5e4'),
  ('compressor-chelmsford',     'compressor_station', 'MPS:mps_mapping_terminal.0fcd007b-c160-485b-8458-07252984287d'),
  ('compressor-churchover',     'compressor_station', 'MPS:mps_mapping_terminal.2ac9e9a4-d374-4503-be34-f60b6cab9963'),
  ('compressor-diss',           'compressor_station', 'MPS:mps_mapping_terminal.bc13d7f2-058b-45cf-87b7-0c8a0fa3d36c'),
  ('compressor-felindre',       'compressor_station', 'MPS:mps_mapping_terminal.4a78994e-15b5-44d6-9620-1129ace79734'),
  ('compressor-hatton',         'compressor_station', 'MPS:mps_mapping_terminal.1c4f6a61-3d4f-4992-8ab4-c100e1c8f1dc'),
  ('compressor-huntingdon',     'compressor_station', 'MPS:mps_mapping_terminal.f512ef9b-9108-476e-a809-542406c07262'),
  ('compressor-kings-lynn',     'compressor_station', 'MPS:mps_mapping_terminal.11e011d1-856e-4dda-a97c-2b5988e30ecc'),
  ('compressor-kirriemuir',     'compressor_station', 'MPS:mps_mapping_terminal.7f1d487b-e996-4940-9535-f4105ffb93b4'),
  ('compressor-moffat',         'compressor_station', 'MPS:mps_mapping_terminal.a4a0ceaa-0516-4667-a351-c0f259dcb510'),
  ('compressor-peterborough',   'compressor_station', 'MPS:mps_mapping_terminal.bd3e2dbd-395d-470e-8efa-d95eaf63b7a6'),
  ('compressor-scunthorpe',     'compressor_station', 'MPS:mps_mapping_terminal.1a13eec2-f3d3-4014-bbc5-9a64df19b4a4'),
  ('compressor-warrington',     'compressor_station', 'MPS:mps_mapping_terminal.870b9f8e-5fcc-4e09-9f68-b37e2e2c2d97'),
  ('compressor-wisbech',        'compressor_station', 'MPS:mps_mapping_terminal.faf4c867-669d-4fd0-afec-1f28a5c7c785'),
  ('compressor-wooler',         'compressor_station', 'MPS:mps_mapping_terminal.bc423702-323d-4788-980a-00dc8a303ee9'),
  ('compressor-wormington',     'compressor_station', 'MPS:mps_mapping_terminal.35dbebf4-a845-43f0-8381-5dc70617e30b'),
  ('compressor-abson',          'compressor_station', 'OSM:r11513074'),
  ('storage-aldbrough',         'gas_storage',        'MPS:mps_mapping_terminal.6f717cdd-a14d-445f-928a-d5c7097c1712'),
  ('storage-atwick',            'gas_storage',        'MPS:mps_mapping_terminal.a6116663-ac4d-4622-9163-db19dbc156e5'),
  ('storage-holford',           'gas_storage',        'MPS:mps_mapping_terminal.93e0485c-6978-4c8a-960d-2240bbd54f3c'),
  ('storage-stublach',          'gas_storage',        'MPS:mps_mapping_terminal.7dac0b7f-6a71-411b-b6bc-7f0b4c948ed4'),
  ('storage-hole-house',        'gas_storage',        'MPS:mps_mapping_terminal.3c791f95-0a48-4e5d-a773-8a5114608b75'),
  ('storage-hatfield-moor',     'gas_storage',        'MPS:mps_mapping_terminal.ae757fee-8d4f-4010-8cbb-3ea970e5e7c8'),
  ('storage-humbly-grove',      'gas_storage',        'MPS:mps_mapping_terminal.acccf828-bea3-4081-823f-f45528f33b8c'),
  ('terminal-point-of-ayr',     'gas_processing',     'OSM:w169079879'),
  ('terminal-kinneil',          'gas_processing',     'MPS:mps_mapping_terminal.0e984332-394f-4663-87f0-fde95ae7f01f'),
  ('terminal-wytch-farm',       'gas_processing',     'OSM:w123098589')
)
SELECT to_json({'type':'FeatureCollection','features': list({
  'type':'Feature',
  'properties': {'id': c.id, 'name': f.name, 'kind': c.kind,
                 'status': 'operating', 'source_id': c.src},
  'geometry': {'type':'Point','coordinates':[f.lon, f.lat]}} ORDER BY c.id)})
FROM curated c
JOIN read_parquet('$STORE_URL/views/features/data.parquet') f ON f.id = c.src;
" | python3 -c "
# pretty-print + tidy display names for terse catalogue entries
import json, sys
fix = {
 'storage-aldbrough': 'Aldbrough Gas Storage', 'storage-atwick': 'Atwick Gas Storage (Hornsea)',
 'storage-hatfield-moor': 'Hatfield Moor Gas Storage', 'storage-hole-house': 'Hole House Farm Gas Storage',
 'storage-holford': 'Holford Gas Storage', 'storage-humbly-grove': 'Humbly Grove Gas Storage',
 'storage-stublach': 'Stublach Gas Storage', 'terminal-kinneil': 'Kinneil Terminal (Grangemouth)',
 'terminal-wytch-farm': 'Wytch Farm Gathering Station'}
d = json.load(sys.stdin)
for f in d['features']:
    p = f['properties']; p['name'] = fix.get(p['id'], p['name'])
json.dump(d, open('uk-gas-onshore.geojson', 'w'), indent=4)
print(len(d['features']), 'aois')
"
