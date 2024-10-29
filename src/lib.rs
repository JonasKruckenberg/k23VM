#![feature(allocator_api)]
// #![cfg_attr(not(test), no_std)]

extern crate alloc;
extern crate core;

mod builtins;
mod compile_cranelift;
mod const_eval;
mod errors;
mod func;
mod global;
mod indices;
mod instance;
mod instance_allocator;
mod linker;
mod memory;
mod module;
mod parse;
#[cfg(unix)]
mod placeholder;
mod store;
mod table;
mod translate_cranelift;
mod traps;
mod utils;
mod vm;

use crate::vm::{Export, VMVal};
use wasmparser::ValType;

pub type Result<T> = core::result::Result<T, Error>;
pub use compile_cranelift::Compiler;
pub use const_eval::ConstExprEvaluator;
pub use errors::Error;
pub use func::Func;
pub use global::Global;
pub use instance::Instance;
pub use instance_allocator::InstanceAllocator;
pub use linker::Linker;
pub use memory::Memory;
pub use module::Module;
pub use placeholder::PlaceholderAllocatorDontUse;
pub use store::Store;
pub use table::Table;
pub use traps::{Trap, WasmBacktrace, FrameSymbol};

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

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
    V128([u8; 16]),
}
impl Val {
    fn as_vmval(&self) -> VMVal {
        match self {
            Val::I32(i) => VMVal::i32(*i),
            Val::I64(i) => VMVal::i64(*i),
            Val::F32(u) => VMVal::f32(*u),
            Val::F64(u) => VMVal::f64(*u),
            Val::V128(b) => VMVal::v128(u128::from_le_bytes(*b)),
        }
    }

    unsafe fn from_vmval(vmval: VMVal, ty: ValType) -> Val {
        match ty {
            ValType::I32 => Self::I32(vmval.get_i32()),
            ValType::I64 => Self::I64(vmval.get_i64()),
            ValType::F32 => Self::F32(vmval.get_f32()),
            ValType::F64 => Self::F64(vmval.get_f64()),
            ValType::V128 => Self::V128(vmval.get_v128().to_le_bytes()),
            ValType::Ref(_) => todo!(),
        }
    }

    enum_accessors! {
        e
        (I32(i32) i32 into_i32 *e)
        (I64(i64) i64 into_i64 *e)
        (F32(f32) f32 into_f32 f32::from_bits(*e))
        (F64(f64) f64 into_f64 f64::from_bits(*e))
        (V128([u8; 16]) v128 into_v128 *e)
    }
}

pub enum Ref {}

#[derive(Debug, Clone)]
pub enum Extern {
    Func(Func),
    Table(Table),
    Memory(Memory),
    Global(Global),
}

impl Extern {
    pub(crate) fn from_export(export: Export, store: &mut Store) -> Self {
        match export {
            Export::Function(e) => Self::Func(Func::from_vm_export(store, e)),
            Export::Table(e) => Self::Table(Table::from_vm_export(store, e)),
            Export::Memory(e) => Self::Memory(Memory::from_vm_export(store, e)),
            Export::Global(e) => Self::Global(Global::from_vm_export(store, e)),
        }
    }

    owned_enum_accessors! {
        e
        (Func(Func) into_func e)
        (Table(Table) into_table e)
        (Memory(Memory) into_memory e)
        (Global(Global) into_global e)
    }
}

/// A position within an original source file,
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePos(u32);

impl Default for FilePos {
    fn default() -> Self {
        Self(u32::MAX)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use cranelift_codegen::settings::Configurable;
    use wasmparser::Validator;

    #[test_log::test]
    fn test_trap() -> Result<()> {
        let compiler = {
            let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
            let mut b = cranelift_codegen::settings::builder();
            b.set("opt_level", "speed_and_size").unwrap();
            b.set("libcall_call_conv", "isa_default").unwrap();
            b.set("preserve_frame_pointers", "true").unwrap();
            b.set("enable_probestack", "true").unwrap();
            b.set("probestack_strategy", "inline").unwrap();

            let target_isa = isa_builder
                .finish(cranelift_codegen::settings::Flags::new(b))
                .unwrap();

            Compiler::new(target_isa)
        };
        let mut validator = Validator::default();
        let linker = Linker::default();
        let alloc = PlaceholderAllocatorDontUse;
        let mut const_eval = ConstExprEvaluator::default();
        let mut store = Store::default();

        let module = Module::from_bytes(
            &mut validator,
            &compiler,
            include_bytes!("../tests/trap.wasm"),
        )?;

        let instance = linker
            .instantiate(&mut store, &alloc, &mut const_eval, &module)?;
        instance.debug_vmctx(&store);

        let func = instance.get_func(&mut store, "test_trap").unwrap();
        if let Err(err) = func.call(&mut store, &[], &mut []) {
            println!("{err}");
        }
        
        Ok(())
    }
}
