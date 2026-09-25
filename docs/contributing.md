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
