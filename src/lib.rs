#![feature(allocator_api)]
#![cfg_attr(target_os = "none", no_std)]
#![allow(unused)]

extern crate alloc;

mod code_memory;
mod compile;
mod const_eval;
mod errors;
mod func;
mod indices;
mod instance;
mod instance_allocator;
mod linker;
mod memory;
mod mmap_vec;
mod module;
mod placeholder;
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

pub fn host_page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE).try_into().unwrap() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::Compiler;
    use crate::const_eval::ConstExprEvaluator;
    use crate::func::Func;
    use crate::linker::Linker;
    use crate::module::Module;
    use crate::placeholder::PlaceholderAllocatorDontUse;
    use crate::store::Store;
    use crate::vmcontext::VMVal;
    use alloc::borrow::ToOwned;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::ptr;
    use cranelift_codegen::settings::Configurable;
    use tracing::log;
    use wasmparser::Validator;

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

        let instance = linker
            .instantiate(&mut store, &alloc, &module, &mut const_eval)
            .unwrap();
        log::debug!("{instance:?}");
        instance.debug_print_vmctx(&store);

        let export = instance.get_export(&mut store, "fib").unwrap();
        let func = export.unwrap_func();
        let func = unsafe {
            let sig = &module.module().types[func.func_ref.as_ref().type_index];
            Func::from_raw(func.to_owned(), sig.to_owned())
        };

        let mut results = vec![VMVal::v128(0); 1];
        func.call(&mut store, &[VMVal::i32(9)], &mut results);

        // the 10th fibonacci number should be 55
        assert_eq!(results[0], VMVal::i32(55))
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

        let instance = linker
            .instantiate(&mut store, &alloc, &module, &mut const_eval)
            .unwrap();
        log::debug!("{instance:?}");

        log::debug!("{:?}", store.instance_data(instance.0));
    }

    #[test_log::test]
    fn linking() {
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

        tracing::info!("instantiate the fib module");
        {
            let wasm = include_bytes!("../tests/fib_cpp.wasm");
            let module = Module::from_binary(&mut validator, &compiler, &mut store, wasm).unwrap();
            log::debug!("{module:?}");

            let instance = linker
                .instantiate(&mut store, &alloc, &module, &mut const_eval)
                .unwrap();
            log::debug!("{instance:?}");
            instance.debug_print_vmctx(&store);

            linker
                .define_instance(&mut store, "fib_cpp", instance)
                .unwrap();
        }

        tracing::info!("instantiate the test module");
        {
            let wasm = include_bytes!("../tests/fib_test.wasm");
            let module = Module::from_binary(&mut validator, &compiler, &mut store, wasm).unwrap();
            log::debug!("{module:#?}");

            let instance = linker
                .instantiate(&mut store, &alloc, &module, &mut const_eval)
                .unwrap();
            log::debug!("{instance:?}");
            instance.debug_print_vmctx(&store);

            let export = instance.get_export(&mut store, "fib_test").unwrap();
            let func = export.unwrap_func();
            let func = unsafe {
                let sig = &module.module().types[func.func_ref.as_ref().type_index];
                Func::from_raw(func.to_owned(), sig.to_owned())
            };

            tracing::trace!("before call");
            func.call(&mut store, &[], &mut []);
            tracing::trace!("after call");
        }
    }

    // SymbolMap { symbols: [
    //     SymbolMapName { address: 0, name: "wasm[0]::function[0]" },
    //     SymbolMapName { address: 64, name: "wasm[0]::function[1]" },
    //     SymbolMapName { address: 160, name: "wasm[0]::host_to_wasm_trampoline[1]" }]
    // }
    // [
    //     CompiledFunctionInfo {
    //         wasm_func_loc: FunctionLoc { start: 0, length: 64 },
    //         host_to_wasm_trampoline: Some(FunctionLoc { start: 64, length: 88 })
    //     },
    //     CompiledFunctionInfo {
    //         wasm_func_loc: FunctionLoc { start: 160, length: 84 },
    //         host_to_wasm_trampoline: None
    //     }
    // ]
    // SymbolMapName { address: 0, name: "wasm[0]::function[0]" },
    // SymbolMapName { address: 64, name: "wasm[0]::function[1]" },
    // SymbolMapName { address: 160, name: "wasm[0]::host_to_wasm_trampoline[1]" }

    // [TRACE k23_vm::compile::obj_builder] wasm[0]::function[0] -> FunctionLoc { start: 0, length: 64 }
    // [TRACE k23_vm::compile::obj_builder] wasm[0]::function[1] -> FunctionLoc { start: 64, length: 88 }
    // [TRACE k23_vm::compile::obj_builder] wasm[0]::host_to_wasm_trampoline[1] -> FunctionLoc { start: 160, length: 84 }
}
