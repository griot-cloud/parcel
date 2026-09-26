//! The sandbox holds: runaway loops run out of fuel, memory bombs hit the cap, lies are caught.
#![cfg(feature = "wasm")]

use parcel_core::registry::FunctionManifest;
use parcel_runtime::wasm::install;

fn manifest(name: &str, sig: &str) -> FunctionManifest {
    FunctionManifest {
        name: name.into(),
        version: 1,
        signatures: vec![sig.into()],
        deterministic: true,
        cost: parcel_core::registry::Cost::Cheap,
    }
}

/// A module with the parcel ABI exports around a function body given in WAT.
fn module(body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 1024))
            (func (export "parcel_abi_version") (result i32) i32.const 1)
            (func (export "parcel_alloc") (param $len i32) (result i32)
              (local $p i32)
              global.get $next
              local.set $p
              global.get $next
              local.get $len
              i32.add
              global.set $next
              local.get $p)
            (func (export "parcel_free") (param i32 i32))
            {body})"#
    ))
    .unwrap()
}

#[test]
fn an_infinite_loop_runs_out_of_fuel() {
    let m = module(
        r#"(func (export "parcel_fn_spin") (param i32 i32) (result i64) (loop $l (br $l)) i64.const 0)"#,
    );
    let err = install(&m, &manifest("spin", "(int) -> int"), "evil")
        .err()
        .unwrap();
    assert!(
        err.contains("trapped") && err.to_lowercase().contains("fuel"),
        "{err}"
    );
}

#[test]
fn memory_growth_is_capped() {
    let m = module(
        r#"(func (export "parcel_fn_bomb") (param i32 i32) (result i64)
             (drop (memory.grow (i32.const 60000)))
             unreachable)"#,
    );
    // Growing to ~4 GB is refused (memory.grow returns -1); the function then traps.
    let err = install(&m, &manifest("bomb", "(int) -> int"), "evil")
        .err()
        .unwrap();
    assert!(err.contains("trapped"), "{err}");
}

#[test]
fn a_wrong_output_type_is_caught() {
    // Claims (int) -> int but returns an ok status with a bool column.
    let m = module(
        r#"(data (i32.const 16) "\00\01\ff\00")
           (func (export "parcel_fn_liar") (param i32 i32) (result i64)
             (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const 4)))"#,
    );
    let err = install(&m, &manifest("liar", "(int) -> int"), "evil")
        .err()
        .unwrap();
    assert!(err.contains("different type"), "{err}");
}
