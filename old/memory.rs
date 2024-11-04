use wasmparser::MemoryType;
use crate::store::Stored;
use crate::runtime::VMMemoryImport;
use crate::{runtime, Store};

#[derive(Debug, Clone)]
pub struct Memory(Stored<runtime::ExportedMemory>);

impl Memory {
    // pub fn new(store: &mut Store, ty: MemoryType) -> crate::Result<Self> {
    //     todo!()
    // }
    pub fn ty(&self, _store: &Store) -> &MemoryType {
        todo!()
    }
    pub(crate) fn as_vmmemory_import(&self, store: &Store) -> VMMemoryImport {
        VMMemoryImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedMemory) -> Self {
        Self(store.push_exported_memory(export))
    }
}
