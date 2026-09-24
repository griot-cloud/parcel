//! Functions a utility tenant uses in its contracts, compiled to WebAssembly.
#![no_std]

extern crate alloc;

use alloc::string::String;

parcel_udf::export! {
    /// A Kenya Power prepaid meter serial: `MK` then ten digits.
    fn is_meter_serial(s: &str) -> bool {
        s.len() == 12 && s.starts_with("MK") && s.as_bytes()[2..].iter().all(|b| b.is_ascii_digit())
    }

    /// Units of electricity bought for an amount in cents at a tariff in cents per unit.
    fn units(amount_cents: i64, tariff_cents: i64) -> i64 {
        if tariff_cents <= 0 { 0 } else { amount_cents / tariff_cents }
    }

    /// The county code in a meter serial, e.g. `MK47...` -> `47`.
    fn county(s: &str) -> String {
        String::from(s.get(2..4).unwrap_or(""))
    }
}

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: Bump = Bump;

/// A tiny bump allocator: modules are instantiated per engine and serve bounded batches.
#[cfg(target_arch = "wasm32")]
struct Bump;

#[cfg(target_arch = "wasm32")]
unsafe impl core::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        static mut NEXT: usize = 0;
        unsafe {
            if NEXT == 0 {
                NEXT = core::arch::wasm32::memory_size(0) * 65536;
            }
            let start = (NEXT + layout.align() - 1) & !(layout.align() - 1);
            let end = start + layout.size();
            let have = core::arch::wasm32::memory_size(0) * 65536;
            if end > have {
                let pages = (end - have).div_ceil(65536);
                if core::arch::wasm32::memory_grow(0, pages) == usize::MAX {
                    return core::ptr::null_mut();
                }
            }
            NEXT = end;
            start as *mut u8
        }
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
