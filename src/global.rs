use wasmparser::GlobalType;
use crate::store::Stored;
use crate::vm::{VMGlobalImport};
use crate::{vm, Store};

#[derive(Debug, Clone)]
pub struct Global(Stored<vm::ExportedGlobal>);

impl Global {
    // pub fn new(store: &mut Store, ty: GlobalType) -> crate::Result<Self> {
    //     todo!()
    // }
    pub fn ty(&self, store: &Store) -> &GlobalType {
        todo!()
    }
    // pub fn get(&self, store: &Store) -> Val {
    //     todo!()
    // }
    // pub fn set(&self, store: &mut Store, val: Val) {
    //     todo!()
    // }
    pub(crate) fn as_vmglobal_import(&self, store: &Store) -> VMGlobalImport {
        VMGlobalImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: vm::ExportedGlobal) -> Self {
        Self(store.push_exported_global(export))
    }
}
