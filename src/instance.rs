use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::module::Module;
use crate::store::{InstanceHandle, Store};
use crate::table::Table;
use crate::vmcontext::{OwnedVMContext, VMContextPlan, VMCONTEXT_MAGIC};
use cranelift_entity::PrimaryMap;
use crate::const_eval::ConstExprEvaluator;

#[derive(Debug, Clone)]
pub struct Instance(pub(crate) InstanceHandle);

impl Instance {
    pub(crate) fn new_internal(
        store: &mut Store,
        alloc: &dyn InstanceAllocator,
        module: &Module,
        const_eval: &mut ConstExprEvaluator,
        imports: (),
    ) -> crate::TranslationResult<Instance> {
        let handle = store.allocate_module(alloc, module, const_eval)?;
        Ok(Self(handle))
    }
}

