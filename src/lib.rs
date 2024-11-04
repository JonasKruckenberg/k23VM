#![feature(allocator_api)]
#![cfg_attr(feature = "no_std", no_std)]
// #![allow(unused)]

extern crate alloc;
extern crate core;

mod baseline;
mod compile;
mod const_eval;
mod cranelift;
mod errors;
mod indices;
mod runtime;
mod translate;
mod traps;
mod utils;
mod builtins;

pub use errors::Error;
pub type Result<T> = core::result::Result<T, Error>;
pub use const_eval::ConstExprEvaluator;
pub use cranelift::CraneliftCompiler;
pub use baseline::BaselineCompiler;
pub use traps::Trap;

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_BUILTIN: u32 = 1;

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
pub const MAX_WASM_STACK: usize = 512 * 1024;

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
    use cranelift_codegen::settings::{Configurable, Flags};
    use wasmparser::Validator;
    use super::*;

    // #[test_log::test]
    // fn baseline() {
    //     let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    //     let mut b = cranelift_codegen::settings::builder();
    //     b.set("opt_level", "speed_and_size").unwrap();
    //     b.set("libcall_call_conv", "isa_default").unwrap();
    //     b.set("preserve_frame_pointers", "true").unwrap();
    //     b.set("enable_probestack", "true").unwrap();
    //     b.set("probestack_strategy", "inline").unwrap();
    //     let target_isa = isa_builder.finish(Flags::new(b)).unwrap();
    //
    //     let compiler = CraneliftCompiler::new(target_isa);
    //     let mut validator = Validator::new();
    //
    //     let (mut translation, types, _strings) = ModuleTranslator::new(&mut validator)
    //         .translate(include_bytes!("../tests/fib_cpp.wasm"))
    //         .unwrap();
    //
    //     let function_body_data = mem::take(&mut translation.function_bodies);
    //
    //     let inputs = CompileInputs::from_translation(&translation, &types, function_body_data);
    //     let unlinked_outputs = inputs.compile(&compiler).unwrap();
    //
    //     let mut obj_builder = ObjectBuilder::new(compiler.create_intermediate_code_object());
    //     let module = obj_builder.append(&compiler, unlinked_outputs, translation).unwrap();
    //
    //     tracing::trace!("{module:?}")
    // }
}
