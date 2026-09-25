# Contributing

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
PARCEL=target/debug/parcel examples/run-all.sh
```

`--all-features` includes Substrait, which needs `protoc`. A change to the language needs a
case in the differential test (`crates/parcel-runtime/tests/differential.rs`), so the
interpreter and DataFusion are shown to agree on it. For the documentation:

```bash
python -m pip install -r docs/requirements.txt
sphinx-build -W --keep-going -b html docs docs/_build/html
```

## Releasing

Nobody creates a tag. To release, bump `[workspace.package] version` in `Cargo.toml` and add a
`## [x.y.z]: title` section to `CHANGELOG.md`. When that commit is on `main` and the tests
pass, CI builds the `parcel` binary for Linux (x86_64, arm64), macOS (arm64, x86_64) and
Windows (x64), runs it on every platform that can, and only then tags `vx.y.z` and attaches the
archives, their sha256 files, `install.sh` and `install.ps1` to the release. A tag pushed by hand triggers nothing.
