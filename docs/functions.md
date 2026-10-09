# Custom functions

Custom functions add calculations or checks to rule expressions. Parcel loads them as WebAssembly modules. This example checks for a meter serial beginning with `MK` followed by ten digits, such as `MK1234567890`.

## Write and build the function

The repository includes a complete Rust crate in `examples/udf-meter-serial`. Its function is exported using `parcel-udf`:

```rust
parcel_udf::export! {
    fn is_meter_serial(s: &str) -> bool {
        s.len() == 12 && s.starts_with("MK") && s.as_bytes()[2..].iter().all(|b| b.is_ascii_digit())
    }
}
```

From the repository root:

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/udf-meter-serial/Cargo.toml
```

The compiled module is written under the example's `target/wasm32-unknown-unknown/release/` directory. A YAML manifest describes its name, inputs, output, and execution properties:

```yaml
name: is_meter_serial
version: 1
signatures: ["(string) -> bool"]
deterministic: true
cost: cheap
```

## Verify and use it

The utility example includes a module, its manifest and a contract that uses it. From the repository root:

```bash
cd examples/utility
parcel function verify functions/meter_serial.wasm --manifest functions/is_meter_serial.yaml --owner kplc
parcel check contracts/tokens.yaml --data incoming/tokens.csv \
  --function functions/meter_serial.wasm=functions/is_meter_serial.yaml \
  --function functions/meter_serial.wasm=functions/units.yaml \
  --function functions/meter_serial.wasm=functions/county.yaml \
  --caller callers/kisumu-analyst.yaml --caller callers/admin.yaml
```

This example module also exports `units` and `county`, which the contract uses for derived fields; each function has its own manifest.

A contract with `owner: kplc` can call `is_meter_serial(row.meter)` after the function is loaded for that owner. Pass the same `--function` options when compiling the contract. The application loading it needs the matching function implementations.

## Execution limits

Parcel rejects modules that import external functions and limits their instruction count and memory use. Loading checks the function's inputs and output and runs a test batch. Compiled contracts record a hash of the function implementation; a changed implementation requires recompilation.

`parcel check` also checks that custom functions produce consistent results when used in rules.
