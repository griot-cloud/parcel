# Quickstart

```bash
cargo install --git https://github.com/griot-cloud/parcel parcel-cli
git clone https://github.com/griot-cloud/parcel && cd parcel/examples/quickstart
```

The workspace has a contract (`contracts/orders.yaml`), a sample of the data
(`incoming/orders.csv`) and three callers (`callers/*.yaml`).

## Check the contract against the sample

```bash
parcel check contracts/orders.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/globex-analyst.yaml --caller callers/acme-admin.yaml --caller callers/marketing.yaml
```

`check` compiles the contract against the sample's schema and prints four things:

1. **How each rule will execute.** `analytics_only` is decided at plan time from the caller
   alone; `own_or_admin` prunes files and row groups; `amount_consistent` is evaluated when data
   is written and stored as a flag; `mask_email` is a projection, skipped when the column is not
   selected.
2. **The verdict over the sample**: row count, each assertion's pass rate, whether each
   guarantee holds. A deny-level failure makes the verdict invalid.
3. **What each caller would see**: `globex/wanjiru sees 195 of 600 rows`,
   `globex/kamau refused by 'analytics_only'`.
4. **The differential test**: every rule evaluated by a CEL interpreter and by DataFusion, row
   by row and caller by caller, and required to agree.

Add `--json` for a machine-readable result, for CI. The exit code is non-zero when the verdict
is invalid or the two engines disagree.

## Compile it

```bash
parcel compile contracts/orders.yaml --schema incoming/orders.csv --type msisdn=utf8 -o orders.parcel.json
```

The bundle is what an engine loads. [peQL](https://griot-cloud.github.io/peQL/) registers it,
writes data under it, and answers queries through it:

```bash
peql register orders.parcel.json
```

The validation plan can also leave for other engines as SQL (`--sql duckdb`) or Substrait
(`--substrait plan.bin`); see {doc}`exports`.
