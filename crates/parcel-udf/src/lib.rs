//! Write a parcel user-defined function in ordinary Rust; compile it to WebAssembly.
//!
//! ```ignore
//! parcel_udf::export! {
//!     fn is_meter_serial(s: &str) -> bool {
//!         s.len() == 12 && s.starts_with("MK") && s[2..].bytes().all(|b| b.is_ascii_digit())
//!     }
//! }
//! ```
//!
//! Build with `cargo build --release --target wasm32-unknown-unknown` (crate type `cdylib`),
//! then register the module with a manifest (`parcel function register`).
//!
//! Argument and return types: `bool`, `i64` (CEL `int`), `u64` (`uint`), `f64` (`double`),
//! `&str` or `String` (`string`), `&[u8]` or `Vec<u8>` (`bytes`). A null in any argument gives a
//! null result without calling the function.
//!
//! # ABI (parcel-udf v1)
//!
//! The module exports `memory`, `parcel_abi_version() -> u32` (1), `parcel_alloc(len) -> ptr`,
//! `parcel_free(ptr, len)`, and one `parcel_fn_<name>(ptr, len) -> u64` per function, which
//! returns `(out_ptr << 32) | out_len`. It imports nothing. Batches use Arrow's memory layout,
//! little endian:
//!
//! ```text
//! batch  := rows:u32 cols:u32 col*
//! col    := type:u8 validity:[u8; ceil(rows/8)] values
//! values := bool: [u8; ceil(rows/8)]   int/uint/double: [8 bytes; rows]
//!         | string/bytes: offsets:[u32; rows+1] data:[u8; offsets[rows]]
//! output := status:u8 (0 = ok, then one col; 1 = error, then a UTF-8 message)
//! ```

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub const ABI_VERSION: u32 = 1;

/// Column type tags.
pub mod tag {
    pub const BOOL: u8 = 1;
    pub const INT: u8 = 2;
    pub const UINT: u8 = 3;
    pub const DOUBLE: u8 = 4;
    pub const STRING: u8 = 5;
    pub const BYTES: u8 = 6;
}

/// A decoded input column, borrowing the input buffer.
pub struct Col<'a> {
    pub tag: u8,
    pub rows: usize,
    validity: &'a [u8],
    values: &'a [u8],
    offsets: &'a [u8],
}

fn bit(bits: &[u8], i: usize) -> bool {
    bits[i / 8] & (1 << (i % 8)) != 0
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl<'a> Col<'a> {
    pub fn is_valid(&self, row: usize) -> bool {
        bit(self.validity, row)
    }

    fn fixed(&self, row: usize) -> [u8; 8] {
        let mut out = [0u8; 8];
        out.copy_from_slice(&self.values[row * 8..row * 8 + 8]);
        out
    }

    fn var(&self, row: usize) -> &'a [u8] {
        let start = u32_at(self.offsets, row * 4) as usize;
        let end = u32_at(self.offsets, row * 4 + 4) as usize;
        &self.values[start..end]
    }
}

/// Decode a batch; returns the columns and the row count.
pub fn decode(buf: &[u8]) -> Result<(Vec<Col<'_>>, usize), &'static str> {
    if buf.len() < 8 {
        return Err("batch too short");
    }
    let rows = u32_at(buf, 0) as usize;
    let ncols = u32_at(buf, 4) as usize;
    let bitmap = rows.div_ceil(8);
    let mut at = 8;
    let mut cols = Vec::with_capacity(ncols);
    for _ in 0..ncols {
        let tag = *buf.get(at).ok_or("truncated column")?;
        at += 1;
        let validity = buf.get(at..at + bitmap).ok_or("truncated validity")?;
        at += bitmap;
        let (values, offsets) = match tag {
            tag::BOOL => {
                let v = buf.get(at..at + bitmap).ok_or("truncated values")?;
                at += bitmap;
                (v, &buf[0..0])
            }
            tag::INT | tag::UINT | tag::DOUBLE => {
                let v = buf.get(at..at + rows * 8).ok_or("truncated values")?;
                at += rows * 8;
                (v, &buf[0..0])
            }
            tag::STRING | tag::BYTES => {
                let offsets = buf
                    .get(at..at + (rows + 1) * 4)
                    .ok_or("truncated offsets")?;
                at += (rows + 1) * 4;
                let len = u32_at(offsets, rows * 4) as usize;
                let v = buf.get(at..at + len).ok_or("truncated data")?;
                at += len;
                (v, offsets)
            }
            _ => return Err("unknown column type"),
        };
        cols.push(Col {
            tag,
            rows,
            validity,
            values,
            offsets,
        });
    }
    Ok((cols, rows))
}

/// An output column under construction.
pub struct Builder {
    tag: u8,
    rows: usize,
    validity: Vec<u8>,
    values: Vec<u8>,
    offsets: Vec<u8>,
}

impl Builder {
    pub fn new(tag: u8, rows: usize) -> Builder {
        let bitmap = rows.div_ceil(8);
        let mut b = Builder {
            tag,
            rows,
            validity: alloc::vec![0; bitmap],
            values: Vec::new(),
            offsets: Vec::new(),
        };
        match tag {
            tag::BOOL => b.values = alloc::vec![0; bitmap],
            tag::STRING | tag::BYTES => b.offsets.extend_from_slice(&0u32.to_le_bytes()),
            _ => {}
        }
        b
    }

    pub fn push(&mut self, row: usize, v: Option<Value>) {
        if v.is_some() {
            self.validity[row / 8] |= 1 << (row % 8);
        }
        match (self.tag, v) {
            (tag::BOOL, Some(Value::Bool(true))) => self.values[row / 8] |= 1 << (row % 8),
            (tag::BOOL, _) => {}
            (tag::INT, v) => self.values.extend_from_slice(&match v {
                Some(Value::Int(x)) => x.to_le_bytes(),
                _ => [0; 8],
            }),
            (tag::UINT, v) => self.values.extend_from_slice(&match v {
                Some(Value::Uint(x)) => x.to_le_bytes(),
                _ => [0; 8],
            }),
            (tag::DOUBLE, v) => self.values.extend_from_slice(&match v {
                Some(Value::Double(x)) => x.to_le_bytes(),
                _ => [0; 8],
            }),
            (_, v) => {
                if let Some(Value::Bytes(b)) = v {
                    self.values.extend_from_slice(&b);
                }
                let end = self.values.len() as u32;
                self.offsets.extend_from_slice(&end.to_le_bytes());
            }
        }
    }

    pub fn finish(self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(2 + self.validity.len() + self.offsets.len() + self.values.len());
        out.push(0); // status: ok
        out.push(self.tag);
        out.extend_from_slice(&self.validity);
        out.extend_from_slice(&self.offsets);
        out.extend_from_slice(&self.values);
        let _ = self.rows;
        out
    }
}

/// A result value; strings travel as their UTF-8 bytes.
pub enum Value {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Double(f64),
    Bytes(Vec<u8>),
}

/// A type a function can take as an argument.
pub trait Arg<'a>: Sized {
    const TAG: u8;
    fn get(col: &Col<'a>, row: usize) -> Self;
}

impl<'a> Arg<'a> for bool {
    const TAG: u8 = tag::BOOL;
    fn get(col: &Col<'a>, row: usize) -> Self {
        bit(col.values, row)
    }
}
impl<'a> Arg<'a> for i64 {
    const TAG: u8 = tag::INT;
    fn get(col: &Col<'a>, row: usize) -> Self {
        i64::from_le_bytes(col.fixed(row))
    }
}
impl<'a> Arg<'a> for u64 {
    const TAG: u8 = tag::UINT;
    fn get(col: &Col<'a>, row: usize) -> Self {
        u64::from_le_bytes(col.fixed(row))
    }
}
impl<'a> Arg<'a> for f64 {
    const TAG: u8 = tag::DOUBLE;
    fn get(col: &Col<'a>, row: usize) -> Self {
        f64::from_le_bytes(col.fixed(row))
    }
}
impl<'a> Arg<'a> for &'a str {
    const TAG: u8 = tag::STRING;
    fn get(col: &Col<'a>, row: usize) -> Self {
        core::str::from_utf8(col.var(row)).unwrap_or("")
    }
}
impl<'a> Arg<'a> for String {
    const TAG: u8 = tag::STRING;
    fn get(col: &Col<'a>, row: usize) -> Self {
        String::from(core::str::from_utf8(col.var(row)).unwrap_or(""))
    }
}
impl<'a> Arg<'a> for &'a [u8] {
    const TAG: u8 = tag::BYTES;
    fn get(col: &Col<'a>, row: usize) -> Self {
        col.var(row)
    }
}
impl<'a> Arg<'a> for Vec<u8> {
    const TAG: u8 = tag::BYTES;
    fn get(col: &Col<'a>, row: usize) -> Self {
        col.var(row).to_vec()
    }
}

/// A type a function can return.
pub trait Ret {
    const TAG: u8;
    fn into_value(self) -> Value;
}
impl Ret for bool {
    const TAG: u8 = tag::BOOL;
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}
impl Ret for i64 {
    const TAG: u8 = tag::INT;
    fn into_value(self) -> Value {
        Value::Int(self)
    }
}
impl Ret for u64 {
    const TAG: u8 = tag::UINT;
    fn into_value(self) -> Value {
        Value::Uint(self)
    }
}
impl Ret for f64 {
    const TAG: u8 = tag::DOUBLE;
    fn into_value(self) -> Value {
        Value::Double(self)
    }
}
impl Ret for String {
    const TAG: u8 = tag::STRING;
    fn into_value(self) -> Value {
        Value::Bytes(self.into_bytes())
    }
}
impl Ret for Vec<u8> {
    const TAG: u8 = tag::BYTES;
    fn into_value(self) -> Value {
        Value::Bytes(self)
    }
}

/// An error output: status 1 and a message.
pub fn error(msg: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + msg.len());
    out.push(1);
    out.extend_from_slice(msg.as_bytes());
    out
}

/// Check that the input columns have the types the function takes.
pub fn check_tags(cols: &[Col<'_>], want: &[u8]) -> Result<(), &'static str> {
    if cols.len() != want.len() {
        return Err("wrong number of arguments");
    }
    for (c, t) in cols.iter().zip(want) {
        if c.tag != *t {
            return Err("argument type does not match the function");
        }
    }
    Ok(())
}

/// Hand a buffer to the host: leak it and pack `(ptr << 32) | len`. The host frees it with `parcel_free`.
pub fn hand_over(out: Vec<u8>) -> u64 {
    let out = out.into_boxed_slice();
    let len = out.len() as u64;
    let ptr = alloc::boxed::Box::into_raw(out) as *mut u8 as usize as u64;
    (ptr << 32) | len
}

/// Borrow the input the host wrote at `ptr`.
///
/// # Safety
/// `ptr..ptr+len` must be a buffer the host wrote after `parcel_alloc(len)`.
pub unsafe fn input<'a>(ptr: u32, len: u32) -> &'a [u8] {
    unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len as usize) }
}

/// The exports every module needs: memory management and the ABI version. `export!` emits them.
#[macro_export]
#[doc(hidden)]
macro_rules! __abi {
    () => {
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            extern crate alloc as __parcel_alloc;
            #[unsafe(no_mangle)]
            pub extern "C" fn parcel_abi_version() -> u32 {
                $crate::ABI_VERSION
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn parcel_alloc(len: u32) -> u32 {
        let buf = __parcel_alloc::vec![0u8; len as usize].into_boxed_slice();
                __parcel_alloc::boxed::Box::into_raw(buf) as *mut u8 as usize as u32
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn parcel_free(ptr: u32, len: u32) {
                unsafe {
                    let slice =
                        core::ptr::slice_from_raw_parts_mut(ptr as usize as *mut u8, len as usize);
                    drop(__parcel_alloc::boxed::Box::from_raw(slice));
                }
            }
        };
    };
}

/// Export functions from a module. See the crate documentation.
#[macro_export]
macro_rules! export {
    ($($(#[$meta:meta])* fn $name:ident($($arg:ident : $ty:ty),* $(,)?) -> $ret:ty $body:block)*) => {
        $crate::__abi!();
        $(
            $(#[$meta])*
            pub fn $name($($arg: $ty),*) -> $ret $body

            #[cfg(target_arch = "wasm32")]
            const _: () = {
                #[unsafe(export_name = concat!("parcel_fn_", stringify!($name)))]
                pub extern "C" fn __parcel_call(ptr: u32, len: u32) -> u64 {
                    let buf = unsafe { $crate::input(ptr, len) };
                    let out = match $crate::decode(buf) {
                        Err(e) => $crate::error(e),
                        Ok((cols, rows)) => {
                            let tags: &[u8] = &[$(<$ty as $crate::Arg>::TAG),*];
                            match $crate::check_tags(&cols, tags) {
                                Err(e) => $crate::error(e),
                                Ok(()) => {
                                    let mut b = $crate::Builder::new(<$ret as $crate::Ret>::TAG, rows);
                                    for row in 0..rows {
                                        if cols.iter().all(|c| c.is_valid(row)) {
                                            let mut __i = 0usize;
                                            $(
                                                let $arg: $ty = $crate::Arg::get(&cols[__i], row);
                                                __i += 1;
                                            )*
                                            let _ = __i;
                                            b.push(row, Some($crate::Ret::into_value($name($($arg),*))));
                                        } else {
                                            b.push(row, None);
                                        }
                                    }
                                    b.finish()
                                }
                            }
                        }
                    };
                    $crate::hand_over(out)
                }
            };
        )*
    };
}
