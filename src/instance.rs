use crate::const_eval::ConstExprEvaluator;
use crate::enum_accessors;
use crate::indices::{DefinedMemoryIndex, DefinedTableIndex, EntityIndex};
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::module::Module;
use crate::store::{InstanceHandle, Store};
use crate::table::Table;
use crate::translate::{
    FunctionType, MemoryPlan, TableInitialValue, TablePlan, TableSegmentElements,
};
use crate::vmcontext::{
    OwnedVMContext, VMArrayCallFunction, VMContext, VMContextPlan, VMFuncRef, VMFunctionBody,
    VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition, VMMemoryImport,
    VMTableDefinition, VMTableImport, VMWasmCallFunction, VMCONTEXT_MAGIC,
};
use alloc::borrow::ToOwned;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::NonNull;
use core::{fmt, mem, ptr, slice};
use cranelift_entity::PrimaryMap;
use serde_derive::{Deserialize, Serialize};
use tracing::log;
use wasmparser::GlobalType;

#[derive(Debug, Clone, Copy)]
pub struct Instance(pub(crate) InstanceHandle);

impl Instance {
    pub fn new<'wasm>(
        store: &mut Store<'wasm>,
        alloc: &dyn InstanceAllocator,
        const_eval: &mut ConstExprEvaluator,
        module: Module<'wasm>,
        imports: Imports,
    ) -> crate::TranslationResult<Self> {
        let (mut vmctx, mut tables, mut memories) =
            store.allocate_module(alloc, const_eval, &module)?;

        unsafe {
            initialize_vmctx(
                const_eval,
                &mut vmctx,
                &mut tables,
                &mut memories,
                &module,
                imports,
            );
            initialize_tables(const_eval, &mut tables, &module)?;
            initialize_memories(const_eval, &mut memories, &module)?;
        }

        // TODO optionally call start func
        assert!(
            module.module().start.is_none(),
            "start function not supported yet"
        );

        let handle = store.push_instance(InstanceData {
            module,
            memories,
            tables,
            vmctx,
        });

        Ok(Self(handle))
    }

    pub fn exports<'s, 'wasm>(
        &self,
        store: &'s mut Store<'wasm>,
    ) -> impl Iterator<Item = Export<'wasm>> + use<'s, 'wasm> {
        store.instance_data_mut(self.0).exports()
    }

    pub fn get_export(&self, store: &mut Store, name: &str) -> Option<Extern> {
        store.instance_data_mut(self.0).get_export(name)
    }

    pub fn debug_print_vmctx(&self, store: &Store) {
        struct Dbg<'a, 'wasm> {
            data: &'a InstanceData<'wasm>,
        }

        impl fmt::Debug for Dbg<'_, '_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("VMContext")
                        .field("<vmctx address>", &self.data.vmctx.as_ptr())
                        .field("magic", &self.data.vmctx_magic())
                        .field("tables", &self.data.vmctx_table_definitions())
                        .field("memories", &self.data.vmctx_memory_definitions())
                        .field("globals", &self.data.vmctx_global_definitions())
                        .field("func_refs", &self.data.vmctx_func_refs())
                        .field("imported_functions", &self.data.vmctx_function_imports())
                        .field("imported_tables", &self.data.vmctx_table_imports())
                        .field("imported_memories", &self.data.vmctx_memory_imports())
                        .field("imported_globals", &self.data.vmctx_global_imports())
                        .field("stack_limit", &self.data.vmctx_stack_limit())
                        .field("last_wasm_exit_fp", &self.data.vmctx_last_wasm_exit_fp())
                        .field("last_wasm_exit_pc", &self.data.vmctx_last_wasm_exit_pc())
                        .field("last_wasm_entry_sp", &self.data.vmctx_last_wasm_entry_sp())
                        .finish()
                }
            }
        }

        tracing::debug!(
            "{:#?}",
            Dbg {
                data: store.instance_data(self.0)
            }
        );
    }
}

#[derive(Debug)]
pub(crate) struct InstanceData<'wasm> {
    pub module: Module<'wasm>,
    memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    tables: PrimaryMap<DefinedTableIndex, Table>,
    pub vmctx: OwnedVMContext,
}

impl<'wasm> InstanceData<'wasm> {
    pub(crate) fn exports(&mut self) -> impl Iterator<Item = Export<'wasm>> + use<'_, 'wasm> {
        let exports = self.module.exports().collect::<Vec<_>>();

        exports.into_iter().map(|(name, index)| Export {
            name,
            ext: self._get_export(index),
        })
    }

    pub fn get_export(&mut self, name: &str) -> Option<Extern> {
        let index = self.module.get_export(name)?;
        Some(self._get_export(index))
    }

    fn _get_export(&mut self, index: EntityIndex) -> Extern {
        match index {
            EntityIndex::Function(index) => {
                let func_ref = self.module.module().functions[index].func_ref;
                let ptr: *mut VMFuncRef = unsafe {
                    self.vmctx_plus_offset_mut(self.module.vmctx_plan().vmctx_func_ref(func_ref))
                };

                Extern::Func(ExportFunction {
                    func_ref: NonNull::new(ptr).unwrap(),
                })
            }
            EntityIndex::Table(index) => {
                let (definition, vmctx) = if let Some(def_index) =
                    self.module.module().defined_table_index(index)
                {
                    let definition = unsafe {
                        self.vmctx_plus_offset_mut(
                            self.module.vmctx_plan().vmctx_table_definition(def_index),
                        )
                    };
                    let vmctx = self.vmctx.as_mut_ptr();

                    (definition, vmctx)
                } else {
                    let import: VMTableImport = unsafe {
                        *self.vmctx_plus_offset(self.module.vmctx_plan().vmctx_table_import(index))
                    };

                    (import.from, import.vmctx)
                };

                Extern::Table(ExportTable {
                    definition,
                    vmctx,
                    table: self.module.module().table_plans[index].to_owned(),
                })
            }
            EntityIndex::Memory(index) => {
                let (definition, vmctx) = if let Some(def_index) =
                    self.module.module().defined_memory_index(index)
                {
                    let definition: *mut VMMemoryDefinition = unsafe {
                        self.vmctx_plus_offset_mut(
                            self.module.vmctx_plan().vmctx_memory_definition(def_index),
                        )
                    };
                    let vmctx = self.vmctx.as_mut_ptr();

                    (definition, vmctx)
                } else {
                    let import: VMMemoryImport = unsafe {
                        *self.vmctx_plus_offset(self.module.vmctx_plan().vmctx_memory_import(index))
                    };

                    (import.from, import.vmctx)
                };

                Extern::Memory(ExportMemory {
                    definition,
                    vmctx,
                    memory: self.module.module().memory_plans[index].to_owned(),
                })
            }
            EntityIndex::Global(index) => {
                let (definition, vmctx) = if let Some(def_index) =
                    self.module.module().defined_global_index(index)
                {
                    let definition: *mut VMGlobalDefinition = unsafe {
                        self.vmctx_plus_offset_mut(
                            self.module.vmctx_plan().vmctx_global_definition(def_index),
                        )
                    };
                    let vmctx = self.vmctx.as_mut_ptr();

                    (definition, vmctx)
                } else {
                    let import: VMGlobalImport = unsafe {
                        *self.vmctx_plus_offset(self.module.vmctx_plan().vmctx_global_import(index))
                    };

                    (import.from, import.vmctx)
                };

                Extern::Global(ExportGlobal {
                    definition,
                    vmctx,
                    ty: self.module.module().globals[index].to_owned(),
                })
            }
            EntityIndex::Tag(_) => todo!(),
        }
    }

    unsafe fn vmctx_plus_offset<T>(&self, offset: u32) -> *const T {
        self.vmctx.plus_offset(offset)
    }

    unsafe fn vmctx_plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        self.vmctx.plus_offset_mut(offset)
    }

    unsafe fn vmctx_magic(&self) -> u32 {
        *self.vmctx_plus_offset::<u32>(self.module.vmctx_plan().vmctx_magic())
    }

    unsafe fn vmctx_stack_limit(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.module.vmctx_plan().vmctx_stack_limit())
    }

    unsafe fn vmctx_last_wasm_exit_fp(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.module.vmctx_plan().vmctx_last_wasm_exit_fp())
    }

    unsafe fn vmctx_last_wasm_exit_pc(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.module.vmctx_plan().vmctx_last_wasm_exit_pc())
    }

    unsafe fn vmctx_last_wasm_entry_sp(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.module.vmctx_plan().vmctx_last_wasm_entry_sp())
    }

    unsafe fn vmctx_table_definitions(&self) -> &[VMTableDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMTableDefinition>(
                self.module.vmctx_plan().vmctx_table_definitions_start(),
            ),
            self.module.vmctx_plan().num_defined_tables() as usize,
        )
    }

    unsafe fn vmctx_memory_definitions(&self) -> &[VMMemoryDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMMemoryDefinition>(
                self.module.vmctx_plan().vmctx_memory_definitions_start(),
            ),
            self.module.vmctx_plan().num_defined_memories() as usize,
        )
    }

    unsafe fn vmctx_global_definitions(&self) -> &[VMGlobalDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMGlobalDefinition>(
                self.module.vmctx_plan().vmctx_global_definitions_start(),
            ),
            self.module.vmctx_plan().num_defined_globals() as usize,
        )
    }

    unsafe fn vmctx_func_refs(&self) -> &[VMFuncRef] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMFuncRef>(self.module.vmctx_plan().vmctx_func_refs_start()),
            self.module.vmctx_plan().num_escaped_funcs() as usize,
        )
    }

    unsafe fn vmctx_function_imports(&self) -> &[VMFunctionImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMFunctionImport>(
                self.module.vmctx_plan().vmctx_function_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_funcs() as usize,
        )
    }

    unsafe fn vmctx_table_imports(&self) -> &[VMTableImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMTableImport>(
                self.module.vmctx_plan().vmctx_table_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_tables() as usize,
        )
    }

    unsafe fn vmctx_memory_imports(&self) -> &[VMMemoryImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMMemoryImport>(
                self.module.vmctx_plan().vmctx_memory_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_memories() as usize,
        )
    }

    unsafe fn vmctx_global_imports(&self) -> &[VMGlobalImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMGlobalImport>(
                self.module.vmctx_plan().vmctx_global_imports_start(),
            ),
            self.module.vmctx_plan().num_imported_globals() as usize,
        )
    }
}

#[derive(Default, Debug)]
pub struct Imports {
    pub functions: Vec<VMFunctionImport>,
    pub tables: Vec<VMTableImport>,
    pub memories: Vec<VMMemoryImport>,
    pub globals: Vec<VMGlobalImport>,
}

#[derive(Clone)]
pub struct Export<'wasm> {
    pub name: &'wasm str,
    pub ext: Extern,
}

#[derive(Debug, Clone)]
pub enum Extern {
    Func(ExportFunction),
    Table(ExportTable),
    Memory(ExportMemory),
    Global(ExportGlobal),
}

impl Extern {
    enum_accessors! {
        e
        (Func(&ExportFunction) func unwrap_func e)
        (Table(&ExportTable) table unwrap_table e)
        (Memory(&ExportMemory) memory unwrap_memory e)
        (Global(&ExportGlobal) global unwrap_global e)
    }
}

/// A function export value.
#[derive(Debug, Clone, Copy)]
pub struct ExportFunction {
    /// The `VMFuncRef` for this exported function.
    ///
    /// Note that exported functions cannot be a null funcref, so this is a
    /// non-null pointer.
    pub func_ref: NonNull<VMFuncRef>,
}

/// A table export value.
#[derive(Debug, Clone)]
pub struct ExportTable {
    /// The address of the table descriptor.
    pub definition: *mut VMTableDefinition,
    /// Pointer to the containing `VMContext`.
    pub vmctx: *mut VMContext,
    /// The table declaration, used for compatibility checking.
    pub table: TablePlan,
}

/// A memory export value.
#[derive(Debug, Clone)]
pub struct ExportMemory {
    /// The address of the memory descriptor.
    pub definition: *mut VMMemoryDefinition,
    /// Pointer to the containing `VMContext`.
    pub vmctx: *mut VMContext,
    /// The memory declaration, used for compatibility checking.
    pub memory: MemoryPlan,
}

/// A global export value.
#[derive(Debug, Clone)]
pub struct ExportGlobal {
    /// The address of the global storage.
    pub definition: *mut VMGlobalDefinition,
    /// Pointer to the containing `VMContext`. May be null for host-created
    /// globals.
    pub vmctx: *mut VMContext,
    /// The global declaration, used for compatibility checking.
    pub ty: GlobalType,
}

unsafe fn initialize_vmctx(
    const_eval: &mut ConstExprEvaluator,
    vmctx: &mut OwnedVMContext,
    tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    module: &Module,
    imports: Imports,
) {
    let vmctx_plan = module.vmctx_plan();
    *vmctx.plus_offset_mut(vmctx_plan.vmctx_magic()) = VMCONTEXT_MAGIC;

    // TODO init builtins field
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

    for (index, signature_index, func_ref_index) in
        module
            .module()
            .functions
            .iter()
            .filter_map(|(index, func)| {
                func.is_escaping()
                    .then_some((index, func.signature, func.func_ref))
            })
    {
        let def_index = module
            .module()
            .defined_func_index(index)
            .expect("is this even possible?");

        let (array_call, wasm_call) = {
            let info = &module.0.info.funcs[def_index];

            let array_call = module.0.code.resolve_function_loc(
                info.host_to_wasm_trampoline
                    .expect("escaping function requires trampoline"),
            );
            let wasm_call = module.0.code.resolve_function_loc(info.wasm_func_loc);

            (array_call, wasm_call)
        };

        let ptr: *mut VMFuncRef = vmctx.plus_offset_mut(vmctx_plan.vmctx_func_ref(func_ref_index));
        ptr.write(VMFuncRef {
            array_call: mem::transmute::<usize, VMArrayCallFunction>(array_call),
            wasm_call: NonNull::new(wasm_call as *mut VMWasmCallFunction).unwrap(),
            vmctx: vmctx.as_mut_ptr().cast(),
            type_index: signature_index,
        })
    }

    // Initialize the defined tables
    for def_index in module
        .module()
        .table_plans
        .keys()
        .filter_map(|index| module.module().defined_table_index(index))
    {
        let ptr = vmctx
            .plus_offset_mut::<VMTableDefinition>(vmctx_plan.vmctx_table_definition(def_index));
        ptr.write(tables[def_index].as_vmtable_definition());
    }

    // Initialize the `defined_memories` table.
    for (def_index, plan) in module
        .module()
        .memory_plans
        .iter()
        .filter_map(|(index, plan)| Some((module.module().defined_memory_index(index)?, plan)))
    {
        assert!(!plan.shared, "shared memories are not currently supported");

        let ptr = vmctx
            .plus_offset_mut::<VMMemoryDefinition>(vmctx_plan.vmctx_memory_definition(def_index));

        ptr.write(memories[def_index].as_vmmemory_definition());
    }

    initialize_vmctx_globals(const_eval, vmctx, module);
}

unsafe fn initialize_vmctx_globals(
    const_eval: &mut ConstExprEvaluator,
    vmctx: &mut OwnedVMContext,
    module: &Module,
) -> crate::TranslationResult<()> {
    for (def_index, init_expr) in module.module().global_initializers.iter() {
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
) -> crate::TranslationResult<()> {
    // update initial values
    for (def_index, init) in module.module().table_initializers.initial_values.iter() {
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
    for segment in module.module().table_initializers.segments.iter() {
        let elements: Vec<_> = match &segment.elements {
            TableSegmentElements::Functions(funcs) => funcs
                .iter()
                .map(|func_index| todo!("obtain func ref"))
                .collect(),
            TableSegmentElements::Expressions(exprs) => exprs
                .iter()
                .map(
                    |expr| -> crate::TranslationResult<Option<NonNull<VMFuncRef>>> {
                        let funcref = const_eval.eval(expr)?.get_funcref();
                        // TODO assert funcref ptr is valid
                        Ok(Some(NonNull::new(funcref.cast()).unwrap()))
                    },
                )
                .collect::<Result<Vec<_>, _>>()?,
        };

        let offset = usize::try_from(const_eval.eval(&segment.offset)?.get_u64()).unwrap();

        if let Some(def_index) = module.module().defined_table_index(segment.table_index) {
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
) -> crate::TranslationResult<()> {
    for init in module.module().memory_initializers.iter() {
        let memory64 = module.module().memory_plans[init.memory_index].memory64;

        let offset = usize::try_from(const_eval.eval(&init.offset)?.get_u64()).unwrap();

        if let Some(def_index) = module.module().defined_memory_index(init.memory_index) {
            memories[def_index].as_slice_mut()[offset..offset + init.bytes.len()]
                .copy_from_slice(init.bytes);
        } else {
            todo!("initializing imported table")
        }
    }

    Ok(())
}
