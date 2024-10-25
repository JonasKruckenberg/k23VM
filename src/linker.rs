use crate::const_eval::ConstExprEvaluator;
use crate::instance::Instance;
use crate::instance_allocator::InstanceAllocator;
use crate::module::Module;
use crate::store::Store;

#[derive(Default)]
pub struct Linker {}

impl Linker {
    pub fn instantiate<'wasm>(
        &self,
        store: &mut Store,
        alloc: &dyn InstanceAllocator,
        module: &Module<'wasm>,
        const_eval: &mut ConstExprEvaluator,
    ) -> crate::TranslationResult<Instance> {
        Instance::new_internal(store, alloc, module, const_eval, ())
    }
}
