use crate::store::Stored;
use crate::runtime::{self, VMTableImport};
use crate::{Store};
use wasmparser::TableType;

#[derive(Debug, Clone)]
pub struct Table(Stored<runtime::ExportedTable>);

impl Table {
    pub fn new(_store: &mut Store, _ty: TableType, _init: ()) -> crate::Result<Self> {
        todo!()
    }
    pub fn ty(&self, _store: &Store) -> &TableType {
        todo!()
    }
    pub(crate) fn as_vmtable_import(&self, store: &Store) -> VMTableImport {
        VMTableImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedTable) -> Self {
        Self(store.push_exported_table(export))
    }
}
