use crate::runtime::{self, VMContext, VMVal};
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use core::ops::{Index, IndexMut};
use hashbrown::HashMap;
use core::mem;

#[derive(Debug, Default)]
pub struct Store {
    instances: Vec<runtime::Instance>,
    vmctx2instance: HashMap<*mut VMContext, Stored<runtime::Instance>>,
    exported_funcs: Vec<runtime::ExportedFunction>,
    exported_tables: Vec<runtime::ExportedTable>,
    exported_memories: Vec<runtime::ExportedMemory>,
    exported_globals: Vec<runtime::ExportedGlobal>,
    wasm_vmval_storage: Vec<VMVal>,
}

impl Store {
    pub(crate) fn vmctx2instance(&self, vmctx: *mut VMContext) -> Stored<runtime::Instance> {
        self.vmctx2instance[&vmctx]
    }
    pub(crate) fn push_instance(&mut self, mut instance: runtime::Instance) -> Stored<runtime::Instance> {
        let handle = Stored {
            index: self.instances.len(),
            _m: PhantomData,
        };
        self.vmctx2instance
            .insert(instance.vmctx.as_mut_ptr(), handle);
        self.instances.push(instance);
        handle
    }
    pub(crate) fn push_exported_function(
        &mut self,
        export: runtime::ExportedFunction,
    ) -> Stored<runtime::ExportedFunction> {
        let handle = Stored {
            index: self.exported_funcs.len(),
            _m: PhantomData,
        };
        self.exported_funcs.push(export);
        handle
    }
    pub(crate) fn push_exported_table(
        &mut self,
        export: runtime::ExportedTable,
    ) -> Stored<runtime::ExportedTable> {
        let handle = Stored {
            index: self.exported_tables.len(),
            _m: PhantomData,
        };
        self.exported_tables.push(export);
        handle
    }
    pub(crate) fn push_exported_memory(
        &mut self,
        export: runtime::ExportedMemory,
    ) -> Stored<runtime::ExportedMemory> {
        let handle = Stored {
            index: self.exported_memories.len(),
            _m: PhantomData,
        };
        self.exported_memories.push(export);
        handle
    }
    pub(crate) fn push_exported_global(
        &mut self,
        export: runtime::ExportedGlobal,
    ) -> Stored<runtime::ExportedGlobal> {
        let handle = Stored {
            index: self.exported_globals.len(),
            _m: PhantomData,
        };
        self.exported_globals.push(export);
        handle
    }
    pub(crate) fn take_wasm_vmval_storage(&mut self) -> Vec<VMVal> {
        mem::take(&mut self.wasm_vmval_storage)
    }
    pub(crate) fn return_wasm_vmval_storage(&mut self, storage: Vec<VMVal>) {
        self.wasm_vmval_storage = storage;
    }
}

macro_rules! stored_impls {
    ($bind:ident, $(($ty:path, $field:expr))*) => {
        $(
            impl Index<Stored<$ty>> for Store {
                type Output = $ty;

                fn index(&self, index: Stored<$ty>) -> &Self::Output {
                    let $bind = self;
                    &$field[index.index]
                }
            }

            impl IndexMut<Stored<$ty>> for Store {
                fn index_mut(&mut self, index: Stored<$ty>) -> &mut Self::Output {
                    let $bind = self;
                    &mut $field[index.index]
                }
            }
        )*
    };
}

stored_impls! {
    s,
    (runtime::Instance, s.instances)
    (runtime::ExportedFunction, s.exported_funcs)
    (runtime::ExportedTable, s.exported_tables)
    (runtime::ExportedMemory, s.exported_memories)
    (runtime::ExportedGlobal, s.exported_globals)
}

pub struct Stored<T> {
    index: usize,
    _m: PhantomData<T>,
}

impl<T> Clone for Stored<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            _m: PhantomData,
        }
    }
}

impl<T> Copy for Stored<T> {}

impl<T> fmt::Debug for Stored<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Stored").field(&self.index).finish()
    }
}
