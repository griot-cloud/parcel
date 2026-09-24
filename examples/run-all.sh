#!/usr/bin/env bash
# Run every example workspace from a clean state. Exits non-zero on the first failure.
#   PARCEL=path/to/parcel examples/run-all.sh
set -euo pipefail
P=${PARCEL:-parcel}
here=$(cd "$(dirname "$0")" && pwd)

echo "== quickstart"
cd "$here/quickstart"
rm -rf data
"$P" check contracts/orders.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/globex-analyst.yaml --caller callers/acme-admin.yaml | tail -4
"$P" write contracts/orders.yaml --input incoming/orders.csv --type msisdn=utf8
"$P" list
"$P" query 'SELECT region, COUNT(*) AS orders, SUM(amount_cents) AS revenue FROM "sales/orders" GROUP BY region ORDER BY region' \
  --caller callers/globex-analyst.yaml
"$P" query 'SELECT region, COUNT(*) AS n FROM "sales/orders_ea" GROUP BY region' --caller callers/globex-analyst.yaml
if "$P" query 'SELECT COUNT(*) FROM "sales/orders"' --caller callers/marketing.yaml; then
  echo "marketing should have been refused" >&2; exit 1
fi
if python3 -c "import duckdb" 2>/dev/null; then
  PARCEL="$P" python3 "$here/verify-duckdb.py" . contracts/orders.yaml incoming/orders.csv msisdn=utf8
fi

echo "== utility (tenant WebAssembly functions)"
cd "$here/utility"
rm -rf data _functions
for f in is_meter_serial units county; do
  "$P" function register functions/meter_serial.wasm --manifest "functions/$f.yaml" --owner kplc
done
"$P" check contracts/tokens.yaml --data incoming/tokens.csv --caller callers/kisumu-analyst.yaml --caller callers/admin.yaml | tail -2
"$P" write contracts/tokens.yaml --input incoming/tokens.csv
"$P" query 'SELECT meter AS county, COUNT(*) AS tokens FROM "kplc/tokens" GROUP BY meter' --caller callers/kisumu-analyst.yaml

echo "== all examples passed"
