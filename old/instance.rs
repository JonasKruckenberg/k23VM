use crate::const_eval::ConstExprEvaluator;
use crate::func::Func;
use crate::global::Global;
use crate::indices::EntityIndex;
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::module::Module;
use crate::store::Stored;
use crate::table::Table;
use crate::{runtime, Extern, Store};
use crate::runtime::Imports;

#[derive(Debug, Clone, Copy)]
pub struct Instance(pub(crate) Stored<runtime::Instance>);

impl Instance {
    pub(crate) unsafe fn new_unchecked(
        store: &mut Store,
        alloc: &dyn InstanceAllocator,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::Result<Self> {
        let instance = runtime::Instance::new_unchecked(alloc, const_eval, module, imports)?;
        let handle = store.push_instance(instance);
        Ok(Self(handle))
    }

    pub fn module<'s>(&self, store: &'s Store) -> &'s Module {
        store[self.0].module()
    }

    pub fn exports<'a>(
        &'a self,
        store: &'a mut Store,
    ) -> impl ExactSizeIterator<Item = Export<'a>> + 'a {
        let exports = &store[self.0].exports;
        if exports.iter().any(|e| e.is_none()) {
            let data = &store[self.0];
            let module = data.module().clone();
            let module = &module.compiled().module;

            for name in module.exports.keys() {
                if let Some((export_name_index, _, &entity)) = module.exports.get_full(name) {
                    self._get_export(store, entity, export_name_index);
                }
            }
        }

        let data = &store[self.0];
        let module = data.module();
        module
            .compiled().module
            .exports
            .iter()
            .zip(&data.exports)
            .map(|((name, _), export)| Export {
                name: &module.0.strings[*name],
                value: export.clone().unwrap(),
            })
    }

    pub fn get_export(&self, store: &mut Store, name: &str) -> Option<Extern> {
        let idx = self.module(store).0.strings.lookup(name)?;
        let (export_name_index, _, index) = self.module(store).compiled().module.exports.get_full(&idx)?;
        self._get_export(store, *index, export_name_index)
    }

    pub fn get_func(&self, store: &mut Store, name: &str) -> Option<Func> {
        self.get_export(store, name)?.into_func()
    }

    pub fn get_table(&self, store: &mut Store, name: &str) -> Option<Table> {
        self.get_export(store, name)?.into_table()
    }

    pub fn get_memory(&self, store: &mut Store, name: &str) -> Option<Memory> {
        self.get_export(store, name)?.into_memory()
    }

    pub fn get_global(&self, store: &mut Store, name: &str) -> Option<Global> {
        self.get_export(store, name)?.into_global()
    }

    pub fn debug_vmctx(&self, store: &Store) {
        store[self.0].debug_vmctx()
    }

    fn _get_export(
        &self,
        store: &mut Store,
        entity: EntityIndex,
        export_name_index: usize,
    ) -> Option<Extern> {
        // Instantiated instances will lazily fill in exports, so we process
        // all that lazy logic here.
        let data = &store[self.0];

        if let Some(export) = &data.exports[export_name_index] {
            return Some(export.clone());
        }

        let instance = &mut store[self.0]; // Reborrow the &mut InstanceHandle
        let item = Extern::from_export(instance.get_export_by_index(entity), store);
        let data = &mut store[self.0];
        data.exports[export_name_index] = Some(item.clone());
        Some(item)
    }
}

pub struct Export<'instance> {
    pub name: &'instance str,
    pub value: Extern,
}
