#![feature(allocator_api)]
#![cfg_attr(not(test), no_std)]
#![allow(unused)]

extern crate alloc;

mod compile;
mod const_eval;
mod errors;
mod guest_memory;
mod indices;
mod instance;
mod instance_allocator;
mod linker;
mod memory;
mod module;
mod store;
mod table;
mod translate;
mod trap;
mod utils;
mod vmcontext;

pub use errors::TranslationError;

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
/// The absolute maximum size of a memory in bytes
pub const MEMORY_MAX: usize = 1 << 32;
/// The absolute maximum size of a table in elements
pub const TABLE_MAX: usize = 1 << 10;

pub const HOST_PAGE_SIZE: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::Compiler;
    use crate::module::Module;
    use crate::store::Store;
    use cranelift_codegen::settings::Configurable;
    use tracing::log;
    use wasmparser::Validator;
    use crate::const_eval::ConstExprEvaluator;
    use crate::instance_allocator::PlaceholderAllocatorDontUse;
    use crate::linker::Linker;

    #[test_log::test]
    fn fib_cpp() {
        // Global state
        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
        let mut b = cranelift_codegen::settings::builder();
        b.set("opt_level", "speed_and_size").unwrap();
        let target_isa = isa_builder
            .finish(cranelift_codegen::settings::Flags::new(b))
            .unwrap();
        let compiler = Compiler::new(target_isa);
        let mut validator = Validator::new();
        let mut store = Store::default();
        let mut linker = Linker::default();
        let mut const_eval = ConstExprEvaluator::default();
        let alloc = PlaceholderAllocatorDontUse;

        // actual module compilation & instantiation
        let wasm = include_bytes!("../tests/fib_cpp.wasm");
        let module = Module::from_binary(&mut validator, &compiler, &mut store, wasm).unwrap();
        log::debug!("{module:?}");

        let instance = linker.instantiate(&mut store, &alloc, &module, &mut const_eval).unwrap();
        log::debug!("{instance:?}");
        
        log::debug!("{:?}", store.instance_data(instance.0));
    }

    #[test_log::test]
    fn large() {
        // Global state
        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
        let mut b = cranelift_codegen::settings::builder();
        b.set("opt_level", "speed_and_size").unwrap();
        let target_isa = isa_builder
            .finish(cranelift_codegen::settings::Flags::new(b))
            .unwrap();
        let compiler = Compiler::new(target_isa);
        let mut validator = Validator::new();
        let mut store = Store::default();
        let mut linker = Linker::default();
        let mut const_eval = ConstExprEvaluator::default();
        let alloc = PlaceholderAllocatorDontUse;

        // actual module compilation & instantiation
        let wasm = include_bytes!("../tests/kiwi-editor.wasm");
        let module = Module::from_binary(&mut validator, &compiler, &mut store, wasm).unwrap();
        log::debug!("{module:?}");

        let instance = linker.instantiate(&mut store, &alloc, &module, &mut const_eval).unwrap();
        log::debug!("{instance:?}");

        log::debug!("{:?}", store.instance_data(instance.0));
    }
}
