mod vmcontext;
mod vmoffsets;

use crate::runtime::vmcontext::{
    VMFuncRef, VMGlobalDefinition, VMMemoryDefinition, VMTableDefinition,
};
use crate::translate::GlobalType;
use crate::translate::{MemoryPlan, TablePlan, TranslatedModule};
use alloc::vec::Vec;
use core::ptr::NonNull;

pub use vmcontext::{
    VMContext, VMFunctionImport, VMGlobalImport, VMMemoryImport, VMTableImport, VMVal,
    VMCONTEXT_MAGIC,
};
pub use vmoffsets::{StaticVMOffsets, VMOffsets};

pub enum Export {
    Function(ExportedFunction),
    Table(ExportedTable),
    Memory(ExportedMemory),
    Global(ExportedGlobal),
}

/// A function export value.
#[derive(Debug, Clone, Copy)]
pub struct ExportedFunction {
    /// The `VMFuncRef` for this exported function.
    ///
    /// Note that exported functions cannot be a null funcref, so this is a
    /// non-null pointer.
    pub func_ref: NonNull<VMFuncRef>,
}

/// A table export value.
#[derive(Debug, Clone)]
pub struct ExportedTable {
    /// The address of the table descriptor.
    pub definition: *mut VMTableDefinition,
    /// Pointer to the containing `VMContext`.
    pub vmctx: *mut VMContext,
    /// The table declaration, used for compatibility checking.
    pub table: TablePlan,
}

/// A memory export value.
#[derive(Debug, Clone)]
pub struct ExportedMemory {
    /// The address of the memory descriptor.
    pub definition: *mut VMMemoryDefinition,
    /// Pointer to the containing `VMContext`.
    pub vmctx: *mut VMContext,
    /// The memory declaration, used for compatibility checking.
    pub memory: MemoryPlan,
}

/// A global export value.
#[derive(Debug, Clone)]
pub struct ExportedGlobal {
    /// The address of the global storage.
    pub definition: *mut VMGlobalDefinition,
    /// Pointer to the containing `VMContext`. May be null for host-created
    /// globals.
    pub vmctx: *mut VMContext,
    /// The global declaration, used for compatibility checking.
    pub ty: GlobalType,
}

#[derive(Debug, Default)]
pub struct Imports {
    pub functions: Vec<VMFunctionImport>,
    pub tables: Vec<VMTableImport>,
    pub memories: Vec<VMMemoryImport>,
    pub globals: Vec<VMGlobalImport>,
}

impl Imports {
    pub(crate) fn with_capacity_for(raw: &TranslatedModule) -> Self {
        let mut this = Self::default();

        this.functions.reserve(raw.num_imported_functions as usize);
        this.tables.reserve(raw.num_imported_tables as usize);
        this.memories.reserve(raw.num_imported_memories as usize);
        this.globals.reserve(raw.num_imported_globals as usize);

        this
    }
}
