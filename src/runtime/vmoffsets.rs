use crate::indices::{DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, FuncRefIndex, GlobalIndex, MemoryIndex, TableIndex};
use crate::runtime::vmcontext::{
    VMFuncRef, VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition,
    VMMemoryImport, VMTableDefinition, VMTableImport,
};
use crate::translate::TranslatedModule;
use core::mem::offset_of;
use cranelift_entity::packed_option::ReservedValue;

#[derive(Debug, Clone)]
pub struct StaticVMOffsets {
    magic: u32,
    builtin_functions: u32,
    /// The current stack limit.
    /// TODO clarify what this means
    pub stack_limit: u32,

    /// The value of the frame pointer register when we last called from Wasm to
    /// the host.
    ///
    /// Maintained by our Wasm-to-host trampoline, and cleared just before
    /// calling into Wasm in `catch_traps`.
    ///
    /// This member is `0` when Wasm is actively running and has not called out
    /// to the host.
    ///
    /// Used to find the start of a contiguous sequence of Wasm frames when
    /// walking the stack.
    pub last_wasm_exit_fp: u32,

    /// The last Wasm program counter before we called from Wasm to the host.
    ///
    /// Maintained by our Wasm-to-host trampoline, and cleared just before
    /// calling into Wasm in `catch_traps`.
    ///
    /// This member is `0` when Wasm is actively running and has not called out
    /// to the host.
    ///
    /// Used when walking a contiguous sequence of Wasm frames.
    pub last_wasm_exit_pc: u32,

    /// The last host stack pointer before we called into Wasm from the host.
    ///
    /// Maintained by our host-to-Wasm trampoline, and cleared just before
    /// calling into Wasm in `catch_traps`.
    ///
    /// This member is `0` when Wasm is actively running and has not called out
    /// to the host.
    ///
    /// When a host function is wrapped into a `wasmtime::Func`, and is then
    /// called from the host, then this member has the sentinel value of `-1 as
    /// usize`, meaning that this contiguous sequence of Wasm frames is the
    /// empty sequence, and it is not safe to dereference the
    /// `last_wasm_exit_fp`.
    ///
    /// Used to find the end of a contiguous sequence of Wasm frames when
    /// walking the stack.
    pub last_wasm_entry_fp: u32,

    size: u32,
}

impl StaticVMOffsets {
    pub fn new(ptr_size: u32) -> Self {
        let mut offset = 0;
        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        Self {
            magic: member_offset(ptr_size),
            builtin_functions: member_offset(ptr_size),
            stack_limit: member_offset(ptr_size),
            last_wasm_exit_fp: member_offset(ptr_size),
            last_wasm_exit_pc: member_offset(ptr_size),
            last_wasm_entry_fp: member_offset(ptr_size),
            size: offset,
        }
    }

    /// Returns the offset of the `VMContext`s `magic` field.
    #[inline]
    pub fn vmctx_magic(&self) -> u32 {
        self.magic
    }
    /// Returns the offset of the `VMContext`s `builtin_functions` field.
    #[inline]
    pub fn vmctx_builtin_functions(&self) -> u32 {
        self.builtin_functions
    }
    /// Returns the offset of the `VMContext`s `last_wasm_exit_fp` field.
    #[inline]
    pub fn vmctx_stack_limit(&self) -> u32 {
        self.stack_limit
    }
    /// Returns the offset of the `VMContext`s `last_wasm_exit_fp` field.
    #[inline]
    pub fn vmctx_last_wasm_exit_fp(&self) -> u32 {
        self.last_wasm_exit_fp
    }
    /// Returns the offset of the `VMContext`s `last_wasm_exit_pc` field.
    #[inline]
    pub fn vmctx_last_wasm_exit_pc(&self) -> u32 {
        self.last_wasm_exit_pc
    }
    /// Returns the offset of the `VMContext`s `last_wasm_entry_fp` field.
    #[inline]
    pub fn vmctx_last_wasm_entry_fp(&self) -> u32 {
        self.last_wasm_entry_fp
    }
}

#[derive(Debug)]
pub struct VMOffsets {
    num_imported_funcs: u32,
    num_imported_tables: u32,
    num_imported_memories: u32,
    num_imported_globals: u32,
    num_defined_tables: u32,
    num_defined_memories: u32,
    num_defined_globals: u32,
    num_escaped_funcs: u32,

    // offsets
    pub static_: StaticVMOffsets,
    func_refs: u32,
    imported_functions: u32,
    imported_tables: u32,
    imported_memories: u32,
    imported_globals: u32,
    tables: u32,
    memories: u32,
    globals: u32,

    size: u32,
}

impl VMOffsets {
    pub fn for_module(module: &TranslatedModule, ptr_size: u32) -> Self {
        let static_ = StaticVMOffsets::new(ptr_size);

        let mut offset = static_.size;
        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        Self {
            num_imported_funcs: module.num_imported_functions(),
            num_imported_tables: module.num_imported_tables(),
            num_imported_memories: module.num_imported_memories(),
            num_imported_globals: module.num_imported_globals(),
            num_defined_tables: module.num_defined_tables(),
            num_defined_memories: module.num_defined_memories(),
            num_defined_globals: module.num_defined_globals(),
            num_escaped_funcs: module.num_escaped_funcs(),

            // offsets
            static_,
            func_refs: member_offset(u32_size_of::<VMFuncRef>() * module.num_escaped_funcs()),
            imported_functions: member_offset(
                u32_size_of::<VMFunctionImport>() * module.num_imported_functions(),
            ),
            imported_tables: member_offset(
                u32_size_of::<VMTableImport>() * module.num_imported_tables(),
            ),
            imported_memories: member_offset(
                u32_size_of::<VMMemoryImport>() * module.num_imported_memories(),
            ),
            imported_globals: member_offset(
                u32_size_of::<VMGlobalImport>() * module.num_imported_globals(),
            ),
            tables: member_offset(u32_size_of::<VMTableDefinition>() * module.num_defined_tables()),
            memories: member_offset(ptr_size * module.num_defined_memories()),
            globals: member_offset(
                u32_size_of::<VMGlobalDefinition>() * module.num_defined_globals(),
            ),

            size: offset,
        }
    }

    #[inline]
    pub fn size(&self) -> u32 {
        self.size
    }
    #[inline]
    pub fn num_defined_tables(&self) -> u32 {
        self.num_defined_tables
    }
    #[inline]
    pub fn num_defined_memories(&self) -> u32 {
        self.num_defined_memories
    }
    #[inline]
    pub fn num_defined_globals(&self) -> u32 {
        self.num_defined_globals
    }
    #[inline]
    pub fn num_escaped_funcs(&self) -> u32 {
        self.num_escaped_funcs
    }
    #[inline]
    pub fn num_imported_funcs(&self) -> u32 {
        self.num_imported_funcs
    }
    #[inline]
    pub fn num_imported_tables(&self) -> u32 {
        self.num_imported_tables
    }
    #[inline]
    pub fn num_imported_memories(&self) -> u32 {
        self.num_imported_memories
    }
    #[inline]
    pub fn num_imported_globals(&self) -> u32 {
        self.num_imported_globals
    }

    /// Returns the offset of the *start* of the `VMContext` `function_imports` array.
    #[inline]
    pub fn vmctx_function_imports_start(&self) -> u32 {
        self.imported_functions
    }
    /// Returns the offset of the `VMFunctionImport` given by `index` within `VMContext`s
    /// `function_imports` array.
    #[inline]
    pub fn vmctx_function_import(&self, index: FuncIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_funcs);
        self.imported_functions + index.as_u32() * u32_size_of::<VMFunctionImport>()
    }

    /// Returns the offset of the *start* of the `VMContext` `table_imports` array.
    #[inline]
    pub fn vmctx_table_imports_start(&self) -> u32 {
        self.imported_tables
    }
    /// Returns the offset of the `VMTableImport` given by `index` within `VMContext`s
    /// `table_imports` array.
    #[inline]
    pub fn vmctx_table_import(&self, index: TableIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_tables);
        self.imported_tables + index.as_u32() * u32_size_of::<VMTableImport>()
    }

    /// Returns the offset of the *start* of the `VMContext` `memory_imports` array.
    #[inline]
    pub fn vmctx_memory_imports_start(&self) -> u32 {
        self.imported_memories
    }
    /// Returns the offset of the `VMMemoryImport` given by `index` within `VMContext`s
    /// `memory_imports` array.
    #[inline]
    pub fn vmctx_memory_import(&self, index: MemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_memories);
        self.imported_memories + index.as_u32() * u32_size_of::<VMMemoryImport>()
    }

    /// Returns the offset of the *start* of the `VMContext` `global_imports` array.
    #[inline]
    pub fn vmctx_global_imports_start(&self) -> u32 {
        self.imported_globals
    }
    /// Returns the offset of the `VMGlobalImport` given by `index` within `VMContext`s
    /// `global_imports` array.
    #[inline]
    pub fn vmctx_global_import(&self, index: GlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_globals);
        self.imported_globals + index.as_u32() * u32_size_of::<VMGlobalImport>()
    }
    /// Returns the offset of the `from` field of the `VMGlobalImport` given by `index`
    /// within `VMContext`s `global_imports` array.
    #[inline]
    pub fn vmctx_global_import_from(&self, index: GlobalIndex) -> u32 {
        self.vmctx_global_import(index) + offset_of!(VMGlobalImport, from) as u32
    }

    /// Returns the offset of the *start* of the `VMContext` `func_refs` array.
    #[inline]
    pub fn vmctx_func_refs_start(&self) -> u32 {
        self.func_refs
    }
    /// Returns the offset of the `VMFuncRef` given by `index` within `VMContext`s
    /// `func_refs` array.
    #[inline]
    pub fn vmctx_func_ref(&self, index: FuncRefIndex) -> u32 {
        assert!(!index.is_reserved_value());
        assert!(index.as_u32() < self.num_escaped_funcs);
        self.func_refs + index.as_u32() * u32_size_of::<VMFuncRef>()
    }

    /// Returns the offset of the *start* of the `VMContext` `table_definitions` array.
    #[inline]
    pub fn vmctx_table_definitions_start(&self) -> u32 {
        self.tables
    }
    /// Returns the offset of the `VMTableDefinition` given by `index` within `VMContext`s
    /// `table_definitions` array.
    #[inline]
    pub fn vmctx_table_definition(&self, index: DefinedTableIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_tables);
        self.tables + index.as_u32() * u32_size_of::<VMTableDefinition>()
    }

    /// Returns the offset of the *start* of the `VMContext` `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definitions_start(&self) -> u32 {
        self.memories
    }
    /// Returns the offset of the `VMMemoryDefinition` given by `index` within `VMContext`s
    /// `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definition(&self, index: DefinedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_memories);
        self.memories + index.as_u32() * u32_size_of::<VMMemoryDefinition>()
    }
    /// Returns the offset of the `base` field of the `VMMemoryDefinition` given by `index` within `VMContext`s
    /// `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definition_base(&self, index: DefinedMemoryIndex) -> u32 {
        self.vmctx_memory_definition(index) + offset_of!(VMMemoryDefinition, base) as u32
    }

    /// Returns the offset of the *start* of the `VMContext` `global_definitions` array.
    #[inline]
    pub fn vmctx_global_definitions_start(&self) -> u32 {
        self.globals
    }
    /// Returns the offset of the `VMGlobalDefinition` given by `index` within `VMContext`s
    /// `global_definitions` array.
    #[inline]
    pub fn vmctx_global_definition(&self, index: DefinedGlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_globals);
        self.globals + index.as_u32() * u32_size_of::<VMGlobalDefinition>()
    }
}

/// Like `mem::size_of` but returns `u32` instead of `usize`
/// # Panics
///
/// Panics if the size of `T` is greater than `u32::MAX`.
fn u32_size_of<T: Sized>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap()
}
