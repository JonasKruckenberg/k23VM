use crate::runtime::VMGlobalImport;
use crate::store::Stored;
use crate::{runtime, Store, Val};

#[derive(Debug, Clone, Copy)]
pub struct Global(Stored<runtime::ExportedGlobal>);

impl Global {
    // pub fn new(store: &mut Store, ty: GlobalType) -> crate::Result<Self> {
    //     todo!()
    // }
    // pub fn ty(&self, _store: &Store) -> &GlobalType {
    //     todo!()
    // }
    pub fn get(&self, _store: &Store) -> Val {
        todo!()
    }
    // pub fn set(&self, store: &mut Store, val: Val) {
    //     todo!()
    // }
    pub(crate) fn as_vmglobal_import(&self, store: &Store) -> VMGlobalImport {
        VMGlobalImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedGlobal) -> Self {
        Self(store.push_global(export))
    }

    pub(crate) fn comes_from_same_store(&self, store: &Store) -> bool {
        store.has_global(self.0)
    }
}
