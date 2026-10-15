//! Package the built library as a loadable DuckDB extension: append the metadata footer
//! DuckDB reads before loading it (platform, C API version, extension version, ABI type, and
//! room for a signature).
//!
//! ```text
//! cargo build --release -p parcel-duckdb-extension
//! cargo run -p parcel-duckdb-extension --bin package -- target/release/libparcel_duckdb.dylib parcel.duckdb_extension
//! ```

use std::io::Write;

/// The C API version the extension is built against.
const C_API_VERSION: &str = "v1.2.0";

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "osx_arm64",
        ("macos", "x86_64") => "osx_amd64",
        ("linux", "x86_64") => "linux_amd64",
        ("linux", "aarch64") => "linux_arm64",
        ("windows", "x86_64") => "windows_amd64",
        (os, arch) => panic!("no DuckDB platform name for {os}/{arch}"),
    }
}

fn field(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..s.len()].copy_from_slice(s.as_bytes());
    out
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [library, out] = args.as_slice() else {
        eprintln!("usage: package <library> <out.duckdb_extension>");
        std::process::exit(2);
    };
    let mut bytes = std::fs::read(library)?;
    // A WebAssembly custom section header, so the footer is valid in Wasm builds too:
    // section 0, length 531 (LEB128 0x93 0x04), name "duckdb_signature", payload 512 (0x80 0x04).
    bytes.extend_from_slice(&[0, 0x93, 0x04, 16]);
    bytes.extend_from_slice(b"duckdb_signature");
    bytes.extend_from_slice(&[0x80, 0x04]);
    for f in [
        "",
        "",
        "",
        "C_STRUCT",
        env!("CARGO_PKG_VERSION"),
        C_API_VERSION,
        platform(),
        "4",
    ] {
        bytes.extend_from_slice(&field(f));
    }
    bytes.extend_from_slice(&[0u8; 256]);
    std::fs::File::create(out)?.write_all(&bytes)?;
    println!(
        "{out}  {}  C API {C_API_VERSION}  {}",
        platform(),
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
