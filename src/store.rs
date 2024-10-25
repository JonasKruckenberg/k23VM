use crate::const_eval::ConstExprEvaluator;
use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::instance::InstanceData;
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::module::Module;
use crate::table::Table;
use crate::translate::{TableInitialValue, TableSegmentElements, TranslatedModule};
use crate::vmcontext::{
    OwnedVMContext, VMFuncRef, VMGlobalDefinition, VMMemoryDefinition, VMTableDefinition,
    VMCONTEXT_MAGIC,
};
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::{mem, ptr};
use cranelift_entity::PrimaryMap;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct InstanceHandle(usize);

#[derive(Default)]
pub struct Store<'wasm> {
    instances: Vec<InstanceData<'wasm>>,
}

impl<'wasm> Store<'wasm> {
    pub(crate) fn allocate_module(
        &mut self,
        alloc: &dyn InstanceAllocator,
        module: &Module<'wasm>,
        const_eval: &mut ConstExprEvaluator,
    ) -> crate::TranslationResult<InstanceHandle> {
        let num_defined_memories =
            module.module().memory_plans.len() - module.module().num_imported_memories as usize;
        let mut memories = PrimaryMap::with_capacity(num_defined_memories);

        let num_defined_tables =
            module.module().table_plans.len() - module.module().num_imported_tables as usize;
        let mut tables = PrimaryMap::with_capacity(num_defined_tables);

        match (|| unsafe {
            alloc.allocate_memories(module.module(), &mut memories)?;
            alloc.allocate_tables(module.module(), &mut tables)?;
            alloc.allocate_vmctx(module.module(), &module.vmctx_plan())
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

    pub(crate) fn instance_data(&self, handle: InstanceHandle) -> &InstanceData<'wasm> {
        &self.instances[handle.0]
    }
    pub(crate) fn instance_data_mut(&mut self, handle: InstanceHandle) -> &mut InstanceData<'wasm> {
        &mut self.instances[handle.0]
    }
}
