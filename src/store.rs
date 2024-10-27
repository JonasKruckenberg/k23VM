use crate::const_eval::ConstExprEvaluator;
use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::instance::InstanceData;
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::module::Module;
use crate::table::Table;
use crate::translate::{TableInitialValue, TableSegmentElements, TranslatedModule};
use crate::vmcontext::{
    OwnedVMContext, VMContext, VMFuncRef, VMGlobalDefinition, VMMemoryDefinition,
    VMTableDefinition, VMVal, VMCONTEXT_MAGIC,
};
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::{mem, ptr};
use cranelift_entity::PrimaryMap;
use hashbrown::HashMap;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct InstanceHandle(usize);

#[derive(Default)]
pub struct Store<'wasm> {
    instances: Vec<InstanceData<'wasm>>,
    wasm_vmval_storage: Vec<VMVal>,
    pub(crate) vmctx2instance: HashMap<NonNull<VMContext>, InstanceHandle>,
}

unsafe impl Send for Store<'_> {}
unsafe impl Sync for Store<'_> {}

impl<'wasm> Store<'wasm> {
    #[allow(clippy::type_complexity)]
    pub(crate) fn allocate_module(
        &mut self,
        alloc: &dyn InstanceAllocator,
        const_eval: &mut ConstExprEvaluator,
        module: &Module<'wasm>,
    ) -> crate::TranslationResult<(
        OwnedVMContext,
        PrimaryMap<DefinedTableIndex, Table>,
        PrimaryMap<DefinedMemoryIndex, Memory>,
    )> {
        let num_defined_memories =
            module.module().memory_plans.len() - module.module().num_imported_memories as usize;
        let mut memories = PrimaryMap::with_capacity(num_defined_memories);

        let num_defined_tables =
            module.module().table_plans.len() - module.module().num_imported_tables as usize;
        let mut tables = PrimaryMap::with_capacity(num_defined_tables);

        match (|| unsafe {
            alloc.allocate_memories(module.module(), &mut memories)?;
            alloc.allocate_tables(module.module(), &mut tables)?;
            alloc.allocate_vmctx(module.module(), module.vmctx_plan())
        })() {
            Ok(vmctx) => Ok((vmctx, tables, memories)),
            Err(e) => Err(e),
        }
    }
    pub(crate) fn push_instance(
        &mut self,
        mut instance_data: InstanceData<'wasm>,
    ) -> InstanceHandle {
        let handle = InstanceHandle(self.instances.len());
        self.vmctx2instance.insert(
            NonNull::new(instance_data.vmctx.as_mut_ptr()).unwrap(),
            handle,
        );
        self.instances.push(instance_data);
        handle
    }
    pub(crate) fn instance_data(&self, handle: InstanceHandle) -> &InstanceData<'wasm> {
        &self.instances[handle.0]
    }
    pub(crate) fn instance_data_mut(&mut self, handle: InstanceHandle) -> &mut InstanceData<'wasm> {
        &mut self.instances[handle.0]
    }
    pub(crate) fn take_wasm_vmval_storage(&mut self) -> Vec<VMVal> {
        mem::take(&mut self.wasm_vmval_storage)
    }
    pub(crate) fn return_wasm_vmval_storage(&mut self, vec: Vec<VMVal>) {
        self.wasm_vmval_storage = vec;
    }
}
