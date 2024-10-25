use crate::const_eval::ConstExprEvaluator;
use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::module::Module;
use crate::table::Table;
use crate::translate::{TableInitialValue, TableSegmentElements, TranslatedModule};
use crate::vmcontext::{OwnedVMContext, VMFuncRef, VMGlobalDefinition, VMMemoryDefinition, VMTableDefinition, VMCONTEXT_MAGIC};
use alloc::vec::Vec;
use core::{mem, ptr};
use core::ptr::NonNull;
use cranelift_entity::PrimaryMap;
use serde_derive::{Deserialize, Serialize};

#[derive(Default)]
pub struct Store {
    instances: Vec<InstanceData>,
}

impl Store {
    pub(crate) fn allocate_module(
        &mut self,
        alloc: &dyn InstanceAllocator,
        module: &Module,
        const_eval: &mut ConstExprEvaluator,
    ) -> crate::TranslationResult<InstanceHandle> {
        let num_defined_memories =
            module.module().memory_plans.len() - module.module().num_imported_memories as usize;
        let mut memories = PrimaryMap::with_capacity(num_defined_memories);

        let num_defined_tables =
            module.module().table_plans.len() - module.info.module.num_imported_tables as usize;
        let mut tables = PrimaryMap::with_capacity(num_defined_tables);

        match (|| unsafe {
            alloc.allocate_memories(module.module(), &mut memories)?;
            alloc.allocate_tables(module.module(), &mut tables)?;
            alloc.allocate_vmctx(module.module(), &module.vmctx_plan)
        })() {
            Ok(vmctx) => {
                let handle = InstanceHandle(self.instances.len());
                self.instances.push(InstanceData::new(
                    vmctx, tables, memories, module, const_eval,
                )?);
                Ok(handle)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn instance_data(&self, handle: InstanceHandle) -> &InstanceData {
        &self.instances[handle.0]
    }
    pub(crate) fn instance_data_mut(&mut self, handle: InstanceHandle) -> &mut InstanceData {
        &mut self.instances[handle.0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct InstanceHandle(usize);

#[derive(Debug)]
pub(crate) struct InstanceData {
    memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    tables: PrimaryMap<DefinedTableIndex, Table>,
    vmctx: OwnedVMContext,
}

impl InstanceData {
    fn new(
        vmctx: OwnedVMContext,
        tables: PrimaryMap<DefinedTableIndex, Table>,
        memories: PrimaryMap<DefinedMemoryIndex, Memory>,
        module: &Module,
        const_eval: &mut ConstExprEvaluator,
    ) -> crate::TranslationResult<InstanceData> {
        let mut this = Self {
            vmctx,
            tables,
            memories,
        };

        unsafe {
            this.initialize_vmctx(const_eval, &module);
            this.initialize_tables(const_eval, &module)?;
            this.initialize_memories(const_eval, &module)?;
        }

        // TODO optionally call start func
        assert!(module.module().start.is_none(), "start function not supported yet");

        Ok(this)
    }

    unsafe fn initialize_vmctx(&mut self, const_eval: &mut ConstExprEvaluator, module: &Module) {
        *self.vmctx_plus_offset_mut(module.vmctx_plan.vmctx_magic()) = VMCONTEXT_MAGIC;

        // TODO init builtins field
        // TODO Initialize the imports fields
        // `func_refs` array values are initialized on-demand?

        // Initialize the defined tables
        for def_index in module
            .module()
            .table_plans
            .keys()
            .filter_map(|index| module.module().defined_table_index(index))
        {
            let ptr = self.vmctx_plus_offset_mut::<VMTableDefinition>(
                module.vmctx_plan.vmctx_table_definition(def_index),
            );
            ptr.write(self.tables[def_index].as_vmtable_definition());
        }

        // Initialize the defined memories. This fills in both the `defined_memories` table
        // and the `owned_memories` table at the same time.
        for (def_index, plan) in module
            .module()
            .memory_plans
            .iter()
            .filter_map(|(index, plan)| Some((module.module().defined_memory_index(index)?, plan)))
        {
            assert!(!plan.shared, "shared memories are not currently supported");
            let owned_index = module.module().owned_memory_index(def_index);

            let ptr = self.vmctx_plus_offset_mut::<*mut VMMemoryDefinition>(
                module.vmctx_plan.vmctx_memory_pointer(def_index),
            );
            let owned_ptr = self.vmctx_plus_offset_mut::<VMMemoryDefinition>(
                module.vmctx_plan.vmctx_memory_definition(owned_index),
            );

            owned_ptr.write(self.memories[def_index].as_vmmemory_definition());
            ptr.write(owned_ptr);
        }

        self.initialize_vmctx_globals(const_eval, module);
    }

    unsafe fn initialize_tables(&mut self, const_eval: &mut ConstExprEvaluator, module: &Module) -> crate::TranslationResult<()> {
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

            self.tables[def_index].elements_mut().fill(val);
        }

        // run active elements
        for segment in module.module().table_initializers.segments.iter() {
            let elements: Vec<_> = match &segment.elements {
                TableSegmentElements::Functions(funcs) => funcs
                    .iter()
                    .map(|func_index| {
                        todo!("obtain func ref")
                    })
                    .collect(),
                TableSegmentElements::Expressions(exprs) => exprs
                    .iter()
                    .map(|expr| -> crate::TranslationResult<Option<NonNull<VMFuncRef>>> {
                        let funcref = const_eval.eval(expr)?.get_funcref();
                        // TODO assert funcref ptr is valid
                        Ok(Some(NonNull::new(funcref.cast()).unwrap()))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            };

            let offset = usize::try_from(const_eval.eval(&segment.offset)?.get_u64()).unwrap();

            if let Some(def_index) = module.module().defined_table_index(segment.table_index) {
                self.tables[def_index].elements_mut()[offset..offset + elements.len()].copy_from_slice(&elements);
            } else {
                todo!("initializing imported table")
            }
        }

        Ok(())
    }

    unsafe fn initialize_memories(&mut self, const_eval: &mut ConstExprEvaluator, module: &Module) -> crate::TranslationResult<()> {
        for init in module.module().memory_initializers.iter() {
            let memory64 = module.module().memory_plans[init.memory_index].memory64;

            let offset = usize::try_from(const_eval
                .eval(&init.offset)?
                .get_u64()).unwrap();

            if let Some(def_index) = module.module().defined_memory_index(init.memory_index) {
                self.memories[def_index].as_slice_mut()[offset..offset + init.bytes.len()].copy_from_slice(init.bytes);
            } else {
                todo!("initializing imported table")
            }
        }

        Ok(())
    }

    unsafe fn initialize_vmctx_globals(
        &mut self,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::TranslationResult<()> {
        for (def_index, init_expr) in module.module().global_initializers.iter() {
            let val = const_eval.eval(init_expr)?;
            let ptr = self.vmctx_plus_offset_mut::<VMGlobalDefinition>(module.vmctx_plan.vmctx_global_definition(def_index));
            ptr.write(VMGlobalDefinition::from_vmval(val))
        }

        Ok(())
    }

    unsafe fn vmctx_plus_offset<T>(&self, offset: u32) -> *const T {
        self.vmctx
            .as_vmctx()
            .byte_add(usize::try_from(offset).unwrap())
            .cast()
    }

    unsafe fn vmctx_plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        self.vmctx
            .as_vmctx_mut()
            .byte_add(usize::try_from(offset).unwrap())
            .cast()
    }
}
