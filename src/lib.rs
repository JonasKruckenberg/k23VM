#![feature(allocator_api)]
#![cfg_attr(not(test), no_std)]
#![allow(unused)]

extern crate alloc;

mod compile;
mod errors;
mod indices;
mod translate;
mod trap;
mod utils;
mod vmcontext;

use alloc::format;
use core::ops::Range;
use core::{ptr, slice};
use core::ptr::NonNull;
use cranelift_codegen::settings::Configurable;
use rustix::mm::{mprotect, MprotectFlags};
pub use errors::TranslationError;
use crate::translate::MemoryPlan;
use crate::vmcontext::VMMemoryDefinition;

pub(crate) type TranslationResult<T> = core::result::Result<T, TranslationError>;

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASM_BUILTIN: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;
/// Maximum size, in bytes, of 32-bit memories (4G)
pub const WASM32_MAX_SIZE: u64 = 1 << 32;

/***************** Settings *******************************************/
/// Whether lowerings for relaxed simd instructions are forced to
/// be deterministic.
pub const RELAXED_SIMD_DETERMINISTIC: bool = false;
/// 2 GiB of guard pages
/// TODO why does this help to eliminate bounds checks?
pub const DEFAULT_OFFSET_GUARD_SIZE: u64 = 0x8000_0000;
pub const DEFAULT_STATIC_MEMORY_RESERVATION: usize = 1 << 32;

pub const HOST_PAGE_SIZE: usize = 4096;

/// Is `bytes` a multiple of the host page size?
pub fn usize_is_multiple_of_host_page_size(bytes: usize) -> bool {
    bytes % HOST_PAGE_SIZE == 0
}

pub fn round_u64_up_to_host_pages(bytes: u64) -> u64 {
    let page_size = u64::try_from(HOST_PAGE_SIZE).unwrap();
    debug_assert!(page_size.is_power_of_two());
    bytes
        .checked_add(page_size - 1)
        .map(|val| val & !(page_size - 1))
        .expect(&format!(
            "{bytes} is too large to be rounded up to a multiple of the host page size of {page_size}"
        ))
}

/// Same as `round_u64_up_to_host_pages` but for `usize`s.
pub fn round_usize_up_to_host_pages(bytes: usize) -> usize {
    let bytes = u64::try_from(bytes).unwrap();
    let rounded = round_u64_up_to_host_pages(bytes);
    usize::try_from(rounded).unwrap()
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::engine::Engine;
//     use crate::module::Module;
//     use tracing::log;
//     use crate::linker::Linker;
// 
//     #[test_log::test]
//     fn smoke() {
//         let engine = Engine::default();
//         let mut store = Store::default();
//         let mut linker = Linker::default();
//         
//         let wasm = include_bytes!("../tests/fib_cpp.wasm");
//         let module = Module::from_binary(&engine, &mut store, wasm).unwrap();
// 
//         let instance = linker.instantiate(&engine, &mut store, &module).unwrap();
//         
//         log::debug!("{module:?}");
//     }
// }
