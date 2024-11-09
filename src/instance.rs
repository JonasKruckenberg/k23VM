use crate::func::Func;
use crate::global::Global;
use crate::indices::EntityIndex;
use crate::memory::Memory;
use crate::runtime::{ConstExprEvaluator, Imports, InstanceAllocator};
use crate::store::Stored;
use crate::table::Table;
use crate::{runtime, Extern, Module, Store};

pub struct Instance(Stored<runtime::Instance>);

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

    // pub fn exports<'a>(
    //     &'a self,
    //     store: &'a mut Store,
    // ) -> impl ExactSizeIterator<Item = Export<'a>> + 'a {
    //     todo!()
    // }

    pub fn get_export(&self, store: &mut Store, name: &str) -> Option<Extern> {
        let (export_name_index, _, index) =
            self.module(store).translated().exports.get_full(name)?;
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

    pub(crate) fn comes_from_same_store(&self, store: &Store) -> bool {
        store.has_instance(self.0)
    }
}
