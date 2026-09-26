# Custom functions

Use a WebAssembly function when a rule needs a domain-specific check that the built-ins do not provide. For example, a utility might require a meter serial to be `MK` followed by ten digits.

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

The module is written under that example's `target/wasm32-unknown-unknown/release/` directory. A manifest declares the function signature and execution properties:

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

A contract with `owner: kplc` can call `is_meter_serial(row.meter)` after the function is loaded for that owner. Pass the same `--function` option when compiling a bundle; the bundle carries the module needed by the receiver.

## Execution limits

The runtime rejects modules with imports and runs accepted functions under fuel and memory limits. Loading checks the module's interface and evaluates a smoke batch. Function pins identify the implementation by hash, so changing a module requires recompilation of contracts that use the new implementation.

The CEL interpreter and DataFusion use the same module. `parcel check` compares their rule results on the supplied sample.
