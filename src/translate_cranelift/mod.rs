mod builtins;
mod code_translator;
mod env;
mod func_translator;
mod heap;
mod state;
mod table;
mod utils;

use crate::indices::{FuncIndex, GlobalIndex, TypeIndex};
use cranelift_codegen::ir;

pub use builtins::BuiltinFunctionSignatures;
pub use env::TranslationEnvironment;
pub use func_translator::FuncTranslator;

/// The value of a WebAssembly global variable.
#[derive(Clone, Copy)]
pub enum IRGlobal {
    /// This is a constant global with a value known at compile_cranelift time.
    Const(ir::Value),

    /// This is a variable in memory that should be referenced through a `GlobalValue`.
    Memory {
        /// The address of the global variable storage.
        gv: ir::GlobalValue,
        /// An offset to add to the address.
        offset: ir::immediates::Offset32,
        /// The global variable's type.
        ty: ir::Type,
    },

    /// This is a global variable that needs to be handled by the environment.
    Custom,
}
