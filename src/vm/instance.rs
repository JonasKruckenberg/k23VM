use crate::const_eval::ConstExprEvaluator;
use crate::indices::{
    DataIndex, DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, ElemIndex, EntityIndex,
    FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TypeIndex,
};
use crate::instance_allocator::InstanceAllocator;
use crate::module::Module;
use crate::parse::{TableInitialValue, TableSegmentElements};
use crate::vm::builtins::VMBuiltinFunctionsArray;
use crate::vm::memory::Memory;
use crate::vm::table::Table;
use crate::vm::vmcontext::{
    VMArrayCallFunction, VMFuncRef, VMGlobalDefinition, VMMemoryDefinition, VMOpaqueContext,
    VMTableDefinition, VMWasmCallFunction,
};
use crate::vm::{
    Export, ExportedFunction, ExportedGlobal, ExportedMemory, ExportedTable, Imports,
    OwnedVMContext, VMFunctionImport, VMGlobalImport, VMMemoryImport, VMTableImport,
    VMCONTEXT_MAGIC,
};
use crate::Extern;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::{fmt, mem, ptr};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{EntitySet, PrimaryMap};
use std::slice;

#[derive(Debug)]
pub struct Instance {
    pub vmctx: OwnedVMContext,
    module: Module,
    tables: PrimaryMap<DefinedTableIndex, Table>,
    memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    dropped_elems: EntitySet<ElemIndex>,
    dropped_data: EntitySet<DataIndex>,

    pub exports: Vec<Option<Extern>>,
}

impl Instance {
    pub(crate) unsafe fn new_unchecked(
        alloc: &dyn InstanceAllocator,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::Result<Self> {
        let (mut vmctx, mut tables, mut memories) = alloc.allocate_module(&module)?;

        initialize_vmctx(
            const_eval,
            &mut vmctx,
            &mut tables,
            &mut memories,
            &module,
            imports,
        )?;
        initialize_tables(const_eval, &mut tables, &module)?;
        initialize_memories(const_eval, &mut memories, &module)?;

        let exports = vec![None; module.exports().len()];

        Ok(Self {
            vmctx,
            tables,
            memories,
            dropped_elems: module.parsed().active_table_initializers.clone(),
            dropped_data: module.parsed().active_memory_initializers.clone(),
            exports,

            module,
        })
    }

    pub(crate) fn module(&self) -> &Module {
        &self.module
    }

    pub fn get_exported_func(&mut self, index: FuncIndex) -> ExportedFunction {
        let func_ref = self.get_func_ref(index).unwrap();
        let func_ref = NonNull::new(func_ref as *const VMFuncRef as *mut _).unwrap();
        ExportedFunction { func_ref }
    }

    fn get_func_ref(&mut self, index: FuncIndex) -> Option<*mut VMFuncRef> {
        if index == FuncIndex::reserved_value() {
            return None;
        }

        let func = &self.module().parsed().functions[index];
        let sig = func.signature;

        unsafe {
            let func_ref: *mut VMFuncRef = self.vmctx.plus_offset_mut::<VMFuncRef>(
                self.module().vmctx_plan().vmctx_func_ref(func.func_ref),
            );
            self.construct_func_ref(index, sig, func_ref);
            Some(func_ref)
        }
    }
    unsafe fn construct_func_ref(
        &mut self,
        index: FuncIndex,
        type_index: TypeIndex,
        into: *mut VMFuncRef,
    ) {
        let func_ref = if let Some(def_index) = self.module().parsed().defined_func_index(index) {
            let array_call = self.module().compiled().funcs[def_index]
                .host_to_wasm_trampoline
                .expect("escaping function requires trampoline");
            let wasm_call = self.module().compiled().funcs[def_index].wasm_func_loc;

            VMFuncRef {
                host_call: mem::transmute::<usize, VMArrayCallFunction>(
                    self.module().code().resolve_function_loc(array_call),
                ),
                wasm_call: NonNull::new(
                    self.module().code().resolve_function_loc(wasm_call) as *mut VMWasmCallFunction
                )
                .unwrap(),
                vmctx: VMOpaqueContext::from_vmcontext(self.vmctx.as_mut_ptr()),
                type_index,
            }
        } else {
            let import = self.imported_function(index);

            VMFuncRef {
                host_call: import.host_call,
                wasm_call: import.wasm_call,
                vmctx: import.vmctx,
                type_index,
            }
        };

        // Safety: we have a `&mut self`, so we have exclusive access
        // to this Instance.
        unsafe {
            ptr::write(into, func_ref);
        }
    }
    pub fn imported_function(&self, index: FuncIndex) -> &VMFunctionImport {
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().vmctx_plan().vmctx_function_import(index))
        }
    }

    pub fn get_exported_table(&mut self, index: TableIndex) -> ExportedTable {
        let (definition, vmctx) =
            if let Some(def_index) = self.module().parsed().defined_table_index(index) {
                (self.table_ptr(def_index), self.vmctx.as_mut_ptr())
            } else {
                let import = self.imported_table(index);
                (import.from, import.vmctx)
            };

        ExportedTable {
            definition,
            vmctx,
            table: self.module().parsed().table_plans[index].clone(),
        }
    }
    pub fn table_ptr(&mut self, index: DefinedTableIndex) -> *mut VMTableDefinition {
        unsafe {
            self.vmctx
                .plus_offset_mut(self.module().vmctx_plan().vmctx_table_definition(index))
        }
    }
    pub fn imported_table(&self, index: TableIndex) -> &VMTableImport {
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().vmctx_plan().vmctx_table_import(index))
        }
    }

    pub fn get_exported_memory(&mut self, index: MemoryIndex) -> ExportedMemory {
        let (definition, vmctx) =
            if let Some(def_index) = self.module().parsed().defined_memory_index(index) {
                (self.memory_ptr(def_index), self.vmctx.as_mut_ptr())
            } else {
                let import = self.imported_memory(index);
                (import.from, import.vmctx)
            };

        ExportedMemory {
            definition,
            vmctx,
            memory: self.module().parsed().memory_plans[index].clone(),
        }
    }
    pub fn memory_ptr(&mut self, index: DefinedMemoryIndex) -> *mut VMMemoryDefinition {
        unsafe {
            self.vmctx
                .plus_offset_mut(self.module().vmctx_plan().vmctx_memory_definition(index))
        }
    }
    pub fn imported_memory(&self, index: MemoryIndex) -> &VMMemoryImport {
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().vmctx_plan().vmctx_memory_import(index))
        }
    }

    pub fn get_exported_global(&mut self, index: GlobalIndex) -> ExportedGlobal {
        let (definition, vmctx) =
            if let Some(def_index) = self.module().parsed().defined_global_index(index) {
                (self.global_ptr(def_index), self.vmctx.as_mut_ptr())
            } else {
                let import = self.imported_global(index);
                (import.from, import.vmctx)
            };

        ExportedGlobal {
            definition,
            vmctx,
            ty: self.module().parsed().globals[index].clone(),
        }
    }
    pub fn global_ptr(&mut self, index: DefinedGlobalIndex) -> *mut VMGlobalDefinition {
        unsafe {
            self.vmctx
                .plus_offset_mut(self.module().vmctx_plan().vmctx_global_definition(index))
        }
    }
    pub fn imported_global(&self, index: GlobalIndex) -> &VMGlobalImport {
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().vmctx_plan().vmctx_global_import(index))
        }
    }

    pub fn get_export_by_index(&mut self, index: EntityIndex) -> Export {
        match index {
            EntityIndex::Function(i) => Export::Function(self.get_exported_func(i)),
            EntityIndex::Global(i) => Export::Global(self.get_exported_global(i)),
            EntityIndex::Table(i) => Export::Table(self.get_exported_table(i)),
            EntityIndex::Memory(i) => Export::Memory(self.get_exported_memory(i)),
            EntityIndex::Tag(_) => todo!(),
        }
    }

    pub fn debug_vmctx(&self) {
        struct Dbg<'a> {
            data: &'a Instance,
        }
        impl fmt::Debug for Dbg<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("VMContext")
                        .field("magic", &self.data.vmctx_magic())
                        .field("stack_limit", &(self.data.vmctx_stack_limit() as *const u8))
                        .field("builtin_functions", &self.data.vmctx_builtin_functions())
                        .field(
                            "last_wasm_exit_fp",
                            &(self.data.vmctx_last_wasm_exit_fp() as *const u8),
                        )
                        .field(
                            "last_wasm_exit_pc",
                            &(self.data.vmctx_last_wasm_exit_pc() as *const u8),
                        )
                        .field(
                            "last_wasm_entry_fp",
                            &(self.data.vmctx_last_wasm_entry_fp() as *const u8),
                        )
                        .field("imported_functions", &self.data.vmctx_function_imports())
                        .field("imported_tables", &self.data.vmctx_table_imports())
                        .field("imported_memories", &self.data.vmctx_memory_imports())
                        .field("imported_globals", &self.data.vmctx_global_imports())
                        .field("func_refs", &self.data.vmctx_func_refs())
                        .field("tables", &self.data.vmctx_table_definitions())
                        .field("memories", &self.data.vmctx_memory_definitions())
                        .field("globals", &self.data.vmctx_global_definitions())
                        .finish()
                }
            }
        }

        tracing::debug!("{:#?}", Dbg { data: self });
    }

    pub(crate) unsafe fn vmctx_magic(&self) -> u32 {
        *self
            .vmctx
            .plus_offset::<u32>(self.module.vmctx_plan().fixed.vmctx_magic())
    }
    pub(crate) unsafe fn vmctx_stack_limit(&self) -> usize {
        *self
            .vmctx
            .plus_offset::<usize>(self.module.vmctx_plan().fixed.vmctx_stack_limit())
    }
    pub(crate) unsafe fn vmctx_builtin_functions(&self) -> *const VMBuiltinFunctionsArray {
        self.vmctx.plus_offset::<VMBuiltinFunctionsArray>(
            self.module.vmctx_plan().fixed.vmctx_builtin_functions(),
        )
    }
    pub(crate) unsafe fn vmctx_last_wasm_exit_fp(&self) -> usize {
        *self
            .vmctx
            .plus_offset::<usize>(self.module.vmctx_plan().fixed.vmctx_last_wasm_exit_fp())
    }
    pub(crate) unsafe fn vmctx_last_wasm_exit_pc(&self) -> usize {
        *self
            .vmctx
            .plus_offset::<usize>(self.module.vmctx_plan().fixed.vmctx_last_wasm_exit_pc())
    }
    pub(crate) unsafe fn vmctx_last_wasm_entry_fp(&self) -> usize {
        *self
            .vmctx
            .plus_offset::<usize>(self.module.vmctx_plan().fixed.vmctx_last_wasm_entry_fp())
    }
    pub(crate) unsafe fn vmctx_table_definitions(&self) -> &[VMTableDefinition] {
        slice::from_raw_parts(
            self.vmctx.plus_offset::<VMTableDefinition>(
                self.module.vmctx_plan().vmctx_table_definitions_start(),
            ),
            self.module.vmctx_plan().num_defined_tables() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_memory_definitions(&self) -> &[VMMemoryDefinition] {
        slice::from_raw_parts(
            self.vmctx.plus_offset::<VMMemoryDefinition>(
                self.module.vmctx_plan().vmctx_memory_definitions_start(),
            ),
            self.module.vmctx_plan().num_defined_memories() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_global_definitions(&self) -> &[VMGlobalDefinition] {
        slice::from_raw_parts(
            self.vmctx.plus_offset::<VMGlobalDefinition>(
                self.module.vmctx_plan().vmctx_global_definitions_start(),
            ),
            self.module.vmctx_plan().num_defined_globals() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_func_refs(&self) -> &[VMFuncRef] {
        slice::from_raw_parts(
            self.vmctx
                .plus_offset::<VMFuncRef>(self.module.vmctx_plan().vmctx_func_refs_start()),
            self.module.vmctx_plan().num_escaped_funcs() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_function_imports(&self) -> &[VMFunctionImport] {
        slice::from_raw_parts(
            self.vmctx.plus_offset::<VMFunctionImport>(
                self.module.vmctx_plan().vmctx_function_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_funcs() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_table_imports(&self) -> &[VMTableImport] {
        slice::from_raw_parts(
            self.vmctx
                .plus_offset::<VMTableImport>(self.module.vmctx_plan().vmctx_table_imports_start()),
            self.module.vmctx_plan().num_imported_tables() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_memory_imports(&self) -> &[VMMemoryImport] {
        slice::from_raw_parts(
            self.vmctx.plus_offset::<VMMemoryImport>(
                self.module.vmctx_plan().vmctx_memory_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_memories() as usize,
        )
    }
    pub(crate) unsafe fn vmctx_global_imports(&self) -> &[VMGlobalImport] {
        slice::from_raw_parts(
            self.vmctx.plus_offset::<VMGlobalImport>(
                self.module.vmctx_plan().vmctx_global_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_globals() as usize,
        )
    }
}

unsafe fn initialize_vmctx(
    const_eval: &mut ConstExprEvaluator,
    vmctx: &mut OwnedVMContext,
    tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    module: &Module,
    imports: Imports,
) -> crate::Result<()> {
    let vmctx_plan = module.vmctx_plan();
    *vmctx.plus_offset_mut(vmctx_plan.fixed.vmctx_magic()) = VMCONTEXT_MAGIC;

    // Initialize the built-in functions
    *vmctx.plus_offset_mut(vmctx_plan.fixed.vmctx_builtin_functions()) =
        &VMBuiltinFunctionsArray::INIT;

    ptr::copy_nonoverlapping(
        imports.functions.as_ptr(),
        vmctx.plus_offset_mut::<VMFunctionImport>(vmctx_plan.vmctx_function_imports_start()),
        imports.functions.len(),
    );
    ptr::copy_nonoverlapping(
        imports.tables.as_ptr(),
        vmctx.plus_offset_mut::<VMTableImport>(vmctx_plan.vmctx_table_imports_start()),
        imports.tables.len(),
    );
    ptr::copy_nonoverlapping(
        imports.memories.as_ptr(),
        vmctx.plus_offset_mut::<VMMemoryImport>(vmctx_plan.vmctx_memory_imports_start()),
        imports.memories.len(),
    );
    ptr::copy_nonoverlapping(
        imports.globals.as_ptr(),
        vmctx.plus_offset_mut::<VMGlobalImport>(vmctx_plan.vmctx_global_imports_start()),
        imports.globals.len(),
    );

    // Initialize the defined tables
    for def_index in module
        .parsed()
        .table_plans
        .keys()
        .filter_map(|index| module.parsed().defined_table_index(index))
    {
        let ptr = vmctx
            .plus_offset_mut::<VMTableDefinition>(vmctx_plan.vmctx_table_definition(def_index));
        ptr.write(tables[def_index].as_vmtable_definition());
    }

    // Initialize the `defined_memories` table.
    for (def_index, plan) in module
        .parsed()
        .memory_plans
        .iter()
        .filter_map(|(index, plan)| Some((module.parsed().defined_memory_index(index)?, plan)))
    {
        assert!(!plan.shared, "shared memories are not currently supported");

        let ptr = vmctx
            .plus_offset_mut::<VMMemoryDefinition>(vmctx_plan.vmctx_memory_definition(def_index));

        ptr.write(memories[def_index].as_vmmemory_definition());
    }

    initialize_vmctx_globals(const_eval, vmctx, module)
}

unsafe fn initialize_vmctx_globals(
    const_eval: &mut ConstExprEvaluator,
    vmctx: &mut OwnedVMContext,
    module: &Module,
) -> crate::Result<()> {
    for (def_index, init_expr) in module.parsed().global_initializers.iter() {
        let val = const_eval.eval(init_expr)?;
        let ptr = vmctx.plus_offset_mut::<VMGlobalDefinition>(
            module.vmctx_plan().vmctx_global_definition(def_index),
        );
        ptr.write(VMGlobalDefinition::from_vmval(val))
    }

    Ok(())
}

unsafe fn initialize_tables(
    const_eval: &mut ConstExprEvaluator,
    tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    module: &Module,
) -> crate::Result<()> {
    // update initial values
    for (def_index, init) in module.parsed().table_initializers.initial_values.iter() {
        let val = match init {
            TableInitialValue::RefNull => None,
            TableInitialValue::ConstExpr(expr) => {
                let funcref = const_eval.eval(expr)?.get_funcref();
                // TODO assert funcref ptr is valid
                Some(NonNull::new(funcref.cast()).unwrap())
            }
        };

        tables[def_index].elements_mut().fill(val);
    }

    // run active elements
    for segment in module.parsed().table_initializers.segments.iter() {
        let elements: Vec<_> = match &segment.elements {
            TableSegmentElements::Functions(funcs) => {
                funcs.iter().map(|_| todo!("obtain func ref")).collect()
            }
            TableSegmentElements::Expressions(exprs) => exprs
                .iter()
                .map(|expr| -> crate::Result<Option<NonNull<VMFuncRef>>> {
                    let funcref = const_eval.eval(expr)?.get_funcref();
                    // TODO assert funcref ptr is valid
                    Ok(Some(NonNull::new(funcref.cast()).unwrap()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        let offset = const_eval.eval(&segment.offset)?;
        let offset = usize::try_from(offset.get_u64()).unwrap();

        if let Some(def_index) = module.parsed().defined_table_index(segment.table_index) {
            tables[def_index].elements_mut()[offset..offset + elements.len()]
                .copy_from_slice(&elements);
        } else {
            todo!("initializing imported table")
        }
    }

    Ok(())
}

unsafe fn initialize_memories(
    const_eval: &mut ConstExprEvaluator,
    memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    module: &Module,
) -> crate::Result<()> {
    for init in module.parsed().memory_initializers.iter() {
        let offset = const_eval.eval(&init.offset)?;
        let offset = usize::try_from(offset.get_u64()).unwrap();

        if let Some(def_index) = module.parsed().defined_memory_index(init.memory_index) {
            let src = &module.code().wasm_data()[init.data.start as usize..init.data.end as usize];

            memories[def_index].as_slice_mut()[offset..offset + init.data.len()]
                .copy_from_slice(src);
        } else {
            todo!("initializing imported table")
        }
    }

    Ok(())
}
