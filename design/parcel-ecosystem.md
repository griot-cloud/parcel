# parcel in the open-source ecosystem

How parcel meets the tools people already use. The principle: **parcel is the executable meaning of a contract; it should plug into existing standards and tools rather than ask people to adopt new ones.** Every integration is a thin adapter over the same three artifacts.

## The one-line position

Open data contract standards describe *what data should be*. dbt, Soda and Great Expectations *test* it after the fact. Postgres RLS and Snowflake policies *enforce access* inside one database. parcel compiles one contract into all three kinds of enforcement: tests, write-time validation and query-time access. The output runs on any Arrow engine.

## Who touches parcel, and through what

| Person | What they do | Tool they already use | parcel surface |
|---|---|---|---|
| Contract author | Writes and reviews contracts | Git, VS Code, pull requests | YAML files, JSON Schema for autocomplete, `parcel check` in pre-commit and CI |
| Data engineer | Builds pipelines that write data | dbt, Dagster, Airflow, Python | Python package (`pip install parcel`), dbt and Dagster adapters |
| Platform engineer | Runs the query engine | DataFusion, DuckDB, Spark | `parcel-runtime` crate; ValidationPlan as SQL or Substrait |
| Data consumer | Queries data | SQL clients, notebooks, BI | Flight SQL through peQL; `describe` for schema discovery |
| Auditor / regulator | Verifies claims about data | — | Open-source certificate verifier that re-runs the ValidationPlan |

## Integration map

### 1. The contract document: meet ODCS

The Open Data Contract Standard (ODCS, Bitol project, Linux Foundation) is where the industry is converging. The Data Contract Specification from datacontract.com is merging into it. parcel should not compete with it.

- **Import:** `parcel import odcs contract.yaml` maps ODCS schema properties to `expose`, and ODCS `required`/`unique` and quality rules to `assert`. ODCS stays the source of truth for description and ownership.
- **Embed:** parcel's rules can live inside an ODCS document as a custom quality engine (`type: custom`, `engine: parcel`) or under `customProperties`. One file serves both catalogues and enforcement.
- **Export:** `parcel export odcs` for teams that author in parcel and publish to a catalogue.

Our native YAML stays the simplest way in. ODCS is the way into organisations that already have contracts.

### 2. Authoring: editors, Git, CI

- **JSON Schema** for the contract document, generated from the Rust types and published to SchemaStore. VS Code and JetBrains then autocomplete and validate contracts with no plugin.
- **`parcel check` as a pre-commit hook and a GitHub Action.** A contract change gets the report (how each rule will execute) as a PR comment.
- **Later:** a language server for CEL-aware completion of `row.`, `ctx.` and `dataset.` fields.

### 3. Pipelines: dbt, Dagster, Airflow

- **Python bindings** (PyO3, built with maturin, shipped as wheels to PyPI): `parcel.compile(contract, schema)` and `parcel.validate(contract, table)` over Arrow and Polars tables. This is the entry point for most data engineers.
- **dbt:**
  - generate a parcel contract skeleton from a dbt model contract (`schema.yml` column types)
  - run parcel's ValidationPlan as a dbt test, emitted as SQL in the warehouse's dialect
- **Dagster:** a `parcel_asset_check` that runs the ValidationPlan and reports each assertion's pass rate as asset-check metadata. Griot Cloud runs on Dagster, so this adapter is also our own.
- **Airflow:** a single operator over the Python package. Low effort, wide reach.

### 4. Engines: run the checks anywhere

The ValidationPlan is a DataFusion `LogicalPlan`, so it can leave DataFusion three ways:

- **Natively** in DataFusion, and in anything built on it: peQL, Comet, Ballista, InfluxDB 3, LanceDB.
- **As SQL** through DataFusion's unparser, which has DuckDB, Postgres, MySQL and SQLite dialects. This is the most important export: a contract becomes a query any warehouse can run. It is how Griot's "checks in formats any tool can run" promise becomes concrete.
- **As Substrait**, for engines that consume it: DuckDB via its extension, Velox, Acero, Spark via Gluten.

Registry built-ins must therefore also have SQL definitions, or be omitted from exports with a clear warning. That is a constraint on how `parcel-runtime` entries are written.

### 5. Access policies: compile to what databases already enforce

`admit` and `transform` rules map directly to native row and column policies:

| parcel | Postgres | Snowflake | Databricks Unity Catalog |
|---|---|---|---|
| `admit` | row-level security `POLICY … USING (…)` | row access policy | row filter |
| `transform` | view or column mask | masking policy | column mask |
| `ctx.tenant`, `ctx.roles` | `current_setting('app.tenant')`, `pg_has_role` | `CURRENT_ROLE()`, session context | `current_user()`, `is_account_group_member()` |

An exporter (`parcel export postgres-rls`) is a later feature. It is a strong adoption argument: write the rule once, and enforce it in peQL and in the organisation's existing database.

### 6. Storage and catalogues

- **Parquet** first, on local disk, S3 or MinIO, via DataFusion's `ListingTable` in peQL.
- **Table formats:** DuckLake (Griot Cloud's own catalog), Iceberg and Delta Lake are peQL binding resolvers. Each supplies the manifest statistics that `guarantee` rules read.
- **Metadata catalogues** (OpenMetadata, DataHub, Unity Catalog, Polaris): publish the contract, its report and its latest verdict as metadata. Push-only; parcel never depends on a catalogue.

### 7. Functions: follow `arrow-udf`

User-defined functions use the `arrow-udf` WebAssembly ABI (design 7.4), so a function written for parcel is portable to other Arrow engines that adopt the same ABI, and vice versa.

### 8. Trust: the verifier is part of the open surface

GDCP certificates sign the tuple (contract hash, ValidationPlan hash, data hash, verdict). The open-source verifier recomputes the verdict with `parcel-runtime` and the ValidationPlan. Anyone can check a Griot certificate without trusting Griot. Issuance, scoring and orchestration stay in the product.

## Decisions (2026-09-25)

- **Language-neutral first.** parcel ships as Rust crates and a native CLI, installed with each platform's package manager. No language bindings yet: the CLI is the interface for everyone.
- **No tool integrations yet.** The pipeline adapters above (dbt, Dagster, Airflow), a published GitHub Action, and catalogue publishing stay on this map as options. They get built when a user needs one, not before.
- **peQL is open source** (`griot-cloud/peql`), so "query a contract, not a table" is an open standard, not only a product feature.

## What is open and what is product

| Open source | Product (Griot) |
|---|---|
| parcel: core, runtime, udf, engine, cli | Trust scoring (AI, Audit, Operational readiness) |
| peQL, the query engine | Certificate issuance and signing keys |
| Exporters: SQL, Substrait, ODCS | Managed peQL, multi-tenant contract store, marketplace |
| JSON Schema | Proactive insight layer |
| GDCP verifier | |

## Packaging

- **License:** Apache-2.0 (`LICENSE`), matching Arrow and DataFusion.
- **Rust:** crates.io: `parcel-core`, `parcel-runtime`, `parcel-udf`, `parcel-engine`, `parcel-cli`. All five were free on 2026-09-24 and should be reserved by publishing 0.0.1. The bare `parcel` crate is taken.
- **CLI, per platform:** prebuilt binaries on GitHub Releases for Linux, macOS and Windows (x86_64 and arm64), installed through:
  - macOS and Linux: Homebrew, from a Griot tap (`brew install griot-cloud/tap/parcel`)
  - Windows: winget; the manifest goes to `microsoft/winget-pkgs`
  - Debian/Ubuntu: a `.deb` on each release, with an apt repository later
  - Fedora/RHEL: an `.rpm` on each release
  - anywhere: `cargo install parcel-cli`, or a one-line install script

## Order

1. Package managers for the CLI (above).
2. ODCS export, so parcel contracts can live in ODCS-aware catalogues.
3. Integrations (pipelines, CI actions, catalogues, RLS exporters): only on demand.

Built so far: the JSON Schema, a CI workflow for this repository, SQL export verified in DuckDB, Substrait (`--features substrait`), and ODCS import.
