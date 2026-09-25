# Your own functions, in WebAssembly

A tenant can extend the rule language with functions written in Rust and compiled to
WebAssembly. A module imports nothing, so it has no clock, randomness or I/O; it runs under fuel
and memory limits; and it is checked with a smoke batch when it is loaded. Contracts pin a
function by hash, so a later version never changes an existing contract. DataFusion and the CEL
interpreter call the same module, so there is no second implementation to drift.

```rust
parcel_udf::export! {
    fn is_meter_serial(s: &str) -> bool {
        s.len() == 12 && s.starts_with("MK") && s.as_bytes()[2..].iter().all(|b| b.is_ascii_digit())
    }
}
```

```bash
cargo build --release --target wasm32-unknown-unknown
```

A manifest describes it:

```yaml
name: is_meter_serial
version: 1
signatures: ["(string) -> bool"]
deterministic: true
cost: cheap
```

```bash
parcel function verify meter_serial.wasm --manifest is_meter_serial.yaml --owner kplc
parcel check contracts/tokens.yaml --data incoming/tokens.csv \
  --function meter_serial.wasm=is_meter_serial.yaml
```

A contract with `owner: kplc` may call `is_meter_serial(row.meter)` in any rule or enricher; a
contract of another owner cannot. A bundle carries the modules it is pinned to, so an engine
can verify and run it. In peQL, `peql function register` stores them in a workspace.
