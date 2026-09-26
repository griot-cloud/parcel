# Contributing

The workspace contains the compiler, runtime, function SDK and CLI. Use the Rust version required by `Cargo.toml`. Substrait support also needs `protoc`.

## Check code changes

From the repository root:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build -p parcel-cli
PARCEL=$PWD/target/debug/parcel examples/run-all.sh
```

The example suite needs Python with `duckdb` installed. Changes to expression semantics should include a case in `crates/parcel-runtime/tests/differential.rs` to check agreement between the CEL interpreter and DataFusion.

## Build the documentation

```bash
python -m pip install -r docs/requirements.txt
sphinx-build -W --keep-going -b html docs docs/_build/html
```

The build treats warnings as errors. Keep examples consistent with the implementation and use small datasets whose expected results are easy to inspect.

## Releases

Bump the workspace version in `Cargo.toml` and add a `## [x.y.z]: title` entry to `CHANGELOG.md`. Once the change reaches `main` and tests pass, CI builds the release binaries, then creates the version tag and publishes the archives, checksums and installers. A manually pushed tag does not trigger this release workflow.
