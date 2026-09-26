#!/usr/bin/env bash
# Run every example. Exits non-zero on the first failure.
#   PARCEL=path/to/parcel examples/run-all.sh
set -euo pipefail
P=${PARCEL:-parcel}
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

echo "== suppliers (documentation quickstart)"
cd "$here/suppliers"
"$P" check orders.yaml --data orders.csv --caller acme.yaml --caller globex.yaml | tail -6
"$P" compile orders.yaml --schema orders.csv -o "$out/supplier_orders.json" >/dev/null

echo "== quickstart"
cd "$here/quickstart"
"$P" check contracts/orders.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/globex-analyst.yaml --caller callers/acme-admin.yaml --caller callers/marketing.yaml | tail -8
"$P" check contracts/orders_ea.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/globex-analyst.yaml | tail -3
j=$("$P" check contracts/orders.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/marketing.yaml --json)
grep -q '"refused_by": "analytics_only"' <<<"$j"
"$P" compile contracts/orders_ea.yaml --schema incoming/orders.csv --type msisdn=utf8 -o "$out/orders_ea.json" >/dev/null
if python3 -c "import duckdb" 2>/dev/null; then
  PARCEL="$P" python3 "$here/verify-duckdb.py" contracts/orders.yaml incoming/orders.csv msisdn=utf8
fi

echo "== utility (tenant WebAssembly functions)"
cd "$here/utility"
F=()
for f in is_meter_serial units county; do
  # Captured, then filtered: under pipefail, `| head -1` fails whenever head exits first.
  v=$("$P" function verify functions/meter_serial.wasm --manifest "functions/$f.yaml" --owner kplc)
  head -1 <<<"$v"
  F+=(--function "functions/meter_serial.wasm=functions/$f.yaml")
done
"$P" check contracts/tokens.yaml --data incoming/tokens.csv "${F[@]}" \
  --caller callers/kisumu-analyst.yaml --caller callers/admin.yaml | tail -4
"$P" compile contracts/tokens.yaml --schema incoming/tokens.csv "${F[@]}" -o "$out/tokens.json" >/dev/null
if "$P" check contracts/tokens.yaml --data incoming/tokens.csv >/dev/null 2>&1; then
  echo "tokens should not compile without kplc's functions" >&2; exit 1
fi

echo "== all examples passed"
