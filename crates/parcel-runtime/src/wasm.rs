//! Tenants' WebAssembly functions (design 7.4): verified at registration, sandboxed at run time.
//!
//! A module imports nothing, so it has no clock, randomness or I/O and can only be a pure
//! function of its inputs. Each call runs under a fuel budget and a memory cap. The same
//! loaded module serves DataFusion (as a UDF, through the core registry's implementation
//! hook) and the reference CEL interpreter, so a user function has one body.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use cel::Value;
use datafusion_common::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array,
};
use datafusion_common::arrow::buffer::{Buffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use datafusion_common::arrow::datatypes::DataType;
use parcel_core::registry::{FunctionEntry, FunctionManifest, provide_implementation};
use parcel_core::types::Type;
use wasmtime::{
    Config, Engine, Instance, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

/// Memory a module instance may grow to.
pub const MEMORY_LIMIT: usize = 64 << 20;
/// Fuel per call, plus this much per row.
pub const FUEL_BASE: u64 = 10_000_000;
pub const FUEL_PER_ROW: u64 = 1_000_000;

struct Instance_ {
    store: Store<StoreLimits>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<(u32, u32), ()>,
    call: TypedFunc<(u32, u32), u64>,
}

/// One user function, loaded from its module.
pub struct WasmFunction {
    pub entry: FunctionEntry,
    arg_types: Vec<Type>,
    ret: Type,
    inner: Mutex<Instance_>,
}

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| {
        let mut c = Config::new();
        c.consume_fuel(true);
        Engine::new(&c).expect("wasmtime engine")
    })
}

impl WasmFunction {
    /// Load and verify: imports nothing, speaks ABI v1, exports the function, and passes a smoke batch.
    pub fn load(module_bytes: &[u8], entry: FunctionEntry) -> Result<WasmFunction, String> {
        let module = Module::new(engine(), module_bytes)
            .map_err(|e| format!("not a WebAssembly module: {e}"))?;
        if let Some(i) = module.imports().next() {
            return Err(format!(
                "the module imports `{}::{}`; parcel functions may import nothing (no clock, randomness or I/O)",
                i.module(),
                i.name()
            ));
        }
        let export = format!("parcel_fn_{}", entry.name);
        for required in [
            "memory",
            "parcel_abi_version",
            "parcel_alloc",
            "parcel_free",
            export.as_str(),
        ] {
            if module.get_export(required).is_none() {
                return Err(format!(
                    "the module does not export `{required}`; build it with parcel_udf::export!"
                ));
            }
        }
        let limits = StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build();
        let mut store = Store::new(engine(), limits);
        store.limiter(|l| l);
        store.set_fuel(FUEL_BASE).map_err(|e| e.to_string())?;
        let instance = Instance::new(&mut store, &module, &[]).map_err(|e| e.to_string())?;
        let version: TypedFunc<(), u32> = instance
            .get_typed_func(&mut store, "parcel_abi_version")
            .map_err(|e| e.to_string())?;
        let v = version.call(&mut store, ()).map_err(|e| e.to_string())?;
        if v != parcel_udf::ABI_VERSION {
            return Err(format!(
                "the module speaks parcel-udf ABI v{v}; this runtime speaks v{}",
                parcel_udf::ABI_VERSION
            ));
        }
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("no memory export")?;
        let alloc = instance
            .get_typed_func(&mut store, "parcel_alloc")
            .map_err(|e| format!("parcel_alloc: {e}"))?;
        let free = instance
            .get_typed_func(&mut store, "parcel_free")
            .map_err(|e| format!("parcel_free: {e}"))?;
        let call = instance
            .get_typed_func(&mut store, &export)
            .map_err(|e| format!("{export}: {e}"))?;
        let sig = entry.signatures.first().ok_or("no signature")?.clone();
        let f = WasmFunction {
            entry,
            arg_types: sig.args,
            ret: sig.ret,
            inner: Mutex::new(Instance_ {
                store,
                memory,
                alloc,
                free,
                call,
            }),
        };
        f.smoke()?;
        Ok(f)
    }

    /// Run a batch with typical values and a null row; the output must have the declared type,
    /// the right length, and a null where an argument was null.
    fn smoke(&self) -> Result<(), String> {
        let rows = 4;
        let args: Vec<ArrayRef> = self.arg_types.iter().map(|t| sample(t, rows)).collect();
        let out = self
            .call(&args, rows)
            .map_err(|e| format!("smoke batch failed: {e}"))?;
        if out.len() != rows {
            return Err(format!("smoke batch: {} rows in, {} out", rows, out.len()));
        }
        if out.data_type() != &self.ret.to_arrow() {
            return Err(format!(
                "smoke batch: declared {}, returned {}",
                self.ret,
                out.data_type()
            ));
        }
        if !self.arg_types.is_empty() && !out.is_null(rows - 1) {
            return Err("smoke batch: a null argument must give a null result".into());
        }
        Ok(())
    }

    /// Call the function on a batch of argument columns.
    pub fn call(&self, args: &[ArrayRef], rows: usize) -> Result<ArrayRef, String> {
        if args.len() != self.arg_types.len() {
            return Err(format!(
                "`{}` takes {} arguments",
                self.entry.name,
                self.arg_types.len()
            ));
        }
        let input = encode(args, rows)?;
        let mut g = self
            .inner
            .lock()
            .map_err(|_| "function instance poisoned")?;
        let i = &mut *g;
        i.store
            .set_fuel(FUEL_BASE + FUEL_PER_ROW * rows as u64)
            .map_err(|e| e.to_string())?;
        let ptr = i
            .alloc
            .call(&mut i.store, input.len() as u32)
            .map_err(|e| trap(&self.entry.name, e))?;
        i.memory
            .write(&mut i.store, ptr as usize, &input)
            .map_err(|e| e.to_string())?;
        let packed = i
            .call
            .call(&mut i.store, (ptr, input.len() as u32))
            .map_err(|e| trap(&self.entry.name, e))?;
        i.free
            .call(&mut i.store, (ptr, input.len() as u32))
            .map_err(|e| trap(&self.entry.name, e))?;
        let (out_ptr, out_len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
        let mut out = vec![0u8; out_len];
        i.memory
            .read(&i.store, out_ptr, &mut out)
            .map_err(|e| e.to_string())?;
        i.free
            .call(&mut i.store, (out_ptr as u32, out_len as u32))
            .map_err(|e| trap(&self.entry.name, e))?;
        drop(g);
        decode_output(&out, rows, &self.ret)
    }

    /// Call on one row of CEL values (the reference interpreter's side).
    pub fn call_values(&self, args: &[Value]) -> Result<Value, String> {
        let arrays = args
            .iter()
            .zip(&self.arg_types)
            .map(|(v, t)| {
                crate::reference::to_scalar(v, t)
                    .and_then(|s| s.to_array().map_err(|e| e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let out = self.call(&arrays, 1)?;
        let s =
            datafusion_common::ScalarValue::try_from_array(&out, 0).map_err(|e| e.to_string())?;
        crate::reference::from_scalar(&s).ok_or_else(|| "null result".into())
    }
}

fn trap(name: &str, e: wasmtime::Error) -> String {
    format!("`{name}` trapped: {e:#}")
}

fn tag(t: &Type) -> Result<u8, String> {
    use parcel_udf::tag::*;
    Ok(match t {
        Type::Bool => BOOL,
        Type::Int => INT,
        Type::Uint => UINT,
        Type::Double => DOUBLE,
        Type::String => STRING,
        Type::Bytes => BYTES,
        other => return Err(format!("{other} cannot cross the function boundary")),
    })
}

fn validity(a: &dyn Array, rows: usize) -> Vec<u8> {
    let mut bits = vec![0u8; rows.div_ceil(8)];
    for i in 0..rows {
        if a.is_valid(i) {
            bits[i / 8] |= 1 << (i % 8);
        }
    }
    bits
}

/// Arrow columns to the ABI's batch encoding.
fn encode(args: &[ArrayRef], rows: usize) -> Result<Vec<u8>, String> {
    use datafusion_common::arrow::compute::cast;
    let mut out = Vec::new();
    out.extend_from_slice(&(rows as u32).to_le_bytes());
    out.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for a in args {
        let t = Type::from_arrow(a.data_type())?;
        let a = cast(a, &t.to_arrow()).map_err(|e| e.to_string())?;
        out.push(tag(&t)?);
        out.extend_from_slice(&validity(a.as_ref(), rows));
        match t {
            Type::Bool => {
                let b = a.as_any().downcast_ref::<BooleanArray>().ok_or("bool")?;
                let mut bits = vec![0u8; rows.div_ceil(8)];
                for i in 0..rows {
                    if b.is_valid(i) && b.value(i) {
                        bits[i / 8] |= 1 << (i % 8);
                    }
                }
                out.extend_from_slice(&bits);
            }
            Type::Int => a
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or("int")?
                .values()
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            Type::Uint => a
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or("uint")?
                .values()
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            Type::Double => a
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or("double")?
                .values()
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            Type::String | Type::Bytes => {
                let (offsets, data): (Vec<i32>, &[u8]) = if t == Type::String {
                    let s = a.as_any().downcast_ref::<StringArray>().ok_or("string")?;
                    (s.value_offsets().to_vec(), s.value_data())
                } else {
                    let b = a.as_any().downcast_ref::<BinaryArray>().ok_or("bytes")?;
                    (b.value_offsets().to_vec(), b.value_data())
                };
                let base = offsets[0];
                for o in &offsets {
                    out.extend_from_slice(&((o - base) as u32).to_le_bytes());
                }
                out.extend_from_slice(&data[base as usize..offsets[rows] as usize]);
            }
            _ => unreachable!("tag() rejected it"),
        }
    }
    Ok(out)
}

/// The ABI's output encoding to an Arrow column.
fn decode_output(buf: &[u8], rows: usize, ret: &Type) -> Result<ArrayRef, String> {
    match buf.first() {
        Some(0) => {}
        Some(1) => return Err(String::from_utf8_lossy(&buf[1..]).into_owned()),
        _ => return Err("malformed output".into()),
    }
    if buf.get(1) != Some(&tag(ret)?) {
        return Err(format!(
            "the function returned a different type from its declared {ret}"
        ));
    }
    let bitmap = rows.div_ceil(8);
    let mut at = 2;
    let validity = buf.get(at..at + bitmap).ok_or("truncated output")?;
    at += bitmap;
    let nulls = NullBuffer::new(datafusion_common::arrow::buffer::BooleanBuffer::new(
        Buffer::from(validity.to_vec()),
        0,
        rows,
    ));
    let nulls = (nulls.null_count() > 0).then_some(nulls);
    let fixed = |at: usize| -> Result<Buffer, String> {
        Ok(Buffer::from(
            buf.get(at..at + rows * 8)
                .ok_or("truncated output")?
                .to_vec(),
        ))
    };
    Ok(match ret {
        Type::Bool => {
            let values = datafusion_common::arrow::buffer::BooleanBuffer::new(
                Buffer::from(buf.get(at..at + bitmap).ok_or("truncated")?.to_vec()),
                0,
                rows,
            );
            Arc::new(BooleanArray::new(values, nulls))
        }
        Type::Int => Arc::new(Int64Array::new(
            ScalarBuffer::new(fixed(at)?, 0, rows),
            nulls,
        )),
        Type::Uint => Arc::new(UInt64Array::new(
            ScalarBuffer::new(fixed(at)?, 0, rows),
            nulls,
        )),
        Type::Double => Arc::new(Float64Array::new(
            ScalarBuffer::new(fixed(at)?, 0, rows),
            nulls,
        )),
        Type::String | Type::Bytes => {
            let raw = buf
                .get(at..at + (rows + 1) * 4)
                .ok_or("truncated offsets")?;
            let offs: Vec<i32> = raw
                .chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i32)
                .collect();
            at += (rows + 1) * 4;
            let data = Buffer::from(
                buf.get(at..at + offs[rows] as usize)
                    .ok_or("truncated data")?
                    .to_vec(),
            );
            let offsets = OffsetBuffer::new(ScalarBuffer::from(offs));
            if *ret == Type::String {
                Arc::new(
                    StringArray::try_new(offsets, data, nulls)
                        .map_err(|e| format!("the function returned invalid UTF-8: {e}"))?,
                )
            } else {
                Arc::new(BinaryArray::try_new(offsets, data, nulls).map_err(|e| e.to_string())?)
            }
        }
        other => return Err(format!("{other} cannot cross the function boundary")),
    })
}

/// A column of typical values with a null in the last row.
fn sample(t: &Type, rows: usize) -> ArrayRef {
    let null = |i: usize| i + 1 == rows;
    match t {
        Type::Bool => Arc::new(
            (0..rows)
                .map(|i| (!null(i)).then_some(i % 2 == 0))
                .collect::<BooleanArray>(),
        ),
        Type::Int => Arc::new(
            (0..rows)
                .map(|i| (!null(i)).then_some(i as i64 * 7 - 3))
                .collect::<Int64Array>(),
        ),
        Type::Uint => Arc::new(
            (0..rows)
                .map(|i| (!null(i)).then_some(i as u64 * 7))
                .collect::<UInt64Array>(),
        ),
        Type::Double => Arc::new(
            (0..rows)
                .map(|i| (!null(i)).then_some(i as f64 * 1.5))
                .collect::<Float64Array>(),
        ),
        Type::String => Arc::new(
            (0..rows)
                .map(|i| (!null(i)).then(|| ["", "MK4700000001", "héllo"][i % 3]))
                .collect::<StringArray>(),
        ),
        _ => Arc::new(
            (0..rows)
                .map(|i| (!null(i)).then(|| &b"\x00\x01"[..i % 3]))
                .collect::<BinaryArray>(),
        ),
    }
}

/// Loaded functions by pinned hash, for the reference interpreter.
pub(crate) fn loaded() -> &'static RwLock<HashMap<String, Arc<WasmFunction>>> {
    static L: OnceLock<RwLock<HashMap<String, Arc<WasmFunction>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// Load a tenant's function and make it callable by both engines under its pinned hash.
pub fn install(
    module_bytes: &[u8],
    manifest: &FunctionManifest,
    owner: &str,
) -> Result<FunctionEntry, String> {
    let module_hash = parcel_core::hash::sha256_hex(module_bytes);
    let entry = FunctionEntry::user(manifest, owner, &module_hash)?;
    if let Some(existing) = loaded().read().expect("lock").get(&entry.hash) {
        return Ok(existing.entry.clone());
    }
    let f = Arc::new(WasmFunction::load(module_bytes, entry.clone())?);
    let for_df = f.clone();
    provide_implementation(
        &entry.hash,
        Arc::new(move |args: &[ArrayRef], rows: usize| for_df.call(args, rows)),
    );
    loaded()
        .write()
        .expect("lock")
        .insert(entry.hash.clone(), f);
    Ok(entry)
}

pub(crate) fn _unused(_: DataType) {}
