use cranelift_codegen::ir;

/// The value of a WebAssembly global variable.
#[derive(Clone, Copy)]
pub enum Global {
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
