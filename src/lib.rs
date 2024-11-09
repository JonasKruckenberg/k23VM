//! #k23VM - k23 WebAssembly Virtual Machine

#![feature(allocator_api)]
#![cfg_attr(feature = "no_std", no_std)]
#![warn(missing_docs)]

extern crate alloc;
extern crate core;

mod builtins;
mod compile;
mod cranelift;
mod engine;
mod errors;
mod indices;
mod module;
mod placeholder;
mod runtime;
mod translate;
mod trap;
mod type_registry;
mod utils;

pub use errors::Error;
pub type Result<T> = core::result::Result<T, Error>;
pub use engine::Engine;
pub use module::Module;
pub use translate::ModuleTranslator;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;
/// Maximum size, in bytes, of 32-bit memories (4G).
pub const WASM32_MAX_SIZE: u64 = 1 << 32;
/// Maximum size, in bytes of WebAssembly stacks.
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

/// Returns the host page size in bytes.
pub fn host_page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE).try_into().unwrap() }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    use crate::module::Module;
    use capstone::arch::arm64::ArchMode;
    use capstone::arch::BuildsCapstone;
    use capstone::Capstone;
    
    use wasmparser::Validator;

    #[test_log::test]
    fn basic() {
        let engine = Engine::default();
        let mut validator = Validator::new();

        let module = Module::from_bytes(
            &engine,
            &mut validator,
            include_bytes!("../tests/embenchen_fannkuch.wasm"),
        )
        .unwrap();

        let cs = Capstone::new()
            .arm64()
            .detail(true)
            .mode(ArchMode::Arm)
            .build()
            .expect("Failed to create Capstone object");

        for (index, info) in module.function_info() {
            let range = info.wasm_func_loc.start as usize
                ..info.wasm_func_loc.start as usize + info.wasm_func_loc.length as usize;
            let insns = cs
                .disasm_all(&module.code().text()[range.clone()], range.start as u64)
                .expect("Failed to disassemble");
            tracing::debug!("{index:?}\n{insns}");
        }
    }

    #[test_log::test]
    fn call_ref() {
        let str = r#"
        (module
  (type $ii (func (param i32) (result i32)))

  (func $apply (param $f (ref null $ii)) (param $x i32) (result i32)
    (call_ref $ii (local.get $x) (local.get $f))
  )
  )"#;

        let engine = Engine::default();
        let mut validator = Validator::new();

        let module = Module::from_str(&engine, &mut validator, str).unwrap();

        let cs = Capstone::new()
            .arm64()
            .detail(true)
            .mode(ArchMode::Arm)
            .build()
            .expect("Failed to create Capstone object");

        let insns = cs
            .disasm_all(module.code().text(), 0)
            .expect("Failed to disassemble");
        tracing::debug!("\n{insns}");
    }

    #[test_log::test]
    fn kiwi_editor() {
        let engine = Engine::default();
        let mut validator = Validator::new();

        let module = Module::from_bytes(
            &engine,
            &mut validator,
            include_bytes!("../tests/kiwi-editor.wasm"),
        )
        .unwrap();

        let cs = Capstone::new()
            .arm64()
            .detail(true)
            .mode(ArchMode::Arm)
            .build()
            .expect("Failed to create Capstone object");

        for (index, info) in module.function_info() {
            let range = info.wasm_func_loc.start as usize
                ..info.wasm_func_loc.start as usize + info.wasm_func_loc.length as usize;
            let insns = cs
                .disasm_all(&module.code().text()[range.clone()], range.start as u64)
                .expect("Failed to disassemble");
            tracing::debug!("{index:?}\n{insns}");
        }
    }
}
