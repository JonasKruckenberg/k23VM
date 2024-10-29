use crate::store::Stored;
use crate::vm::{ExportedTable, VMTableImport};
use crate::{vm, Store};
use wasmparser::TableType;

#[derive(Debug, Clone)]
pub struct Table(Stored<vm::ExportedTable>);

impl Table {
    pub fn new(store: &mut Store, ty: TableType, init: ()) -> crate::Result<Self> {
        todo!()
    }
    pub fn ty(&self, store: &Store) -> &TableType {
        todo!()
    }
    pub(crate) fn as_vmtable_import(&self, store: &Store) -> VMTableImport {
        VMTableImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: ExportedTable) -> Self {
        Self(store.push_exported_table(export))
    }
}
