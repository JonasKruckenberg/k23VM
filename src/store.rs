use crate::runtime::{VMContext, VMOpaqueContext, VMVal};
use crate::{runtime, Engine};
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::{fmt, mem};
use hashbrown::HashMap;

#[derive(Debug)]
pub struct Store {
    pub(crate) engine: Engine,
    instances: Vec<runtime::Instance>,
    exported_funcs: Vec<runtime::ExportedFunction>,
    exported_tables: Vec<runtime::ExportedTable>,
    exported_memories: Vec<runtime::ExportedMemory>,
    exported_globals: Vec<runtime::ExportedGlobal>,
    wasm_vmval_storage: Vec<VMVal>,

    vmctx2instance: HashMap<*mut VMOpaqueContext, Stored<runtime::Instance>>,
}

impl Store {
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
            instances: Vec::new(),
            exported_funcs: Vec::new(),
            exported_tables: Vec::new(),
            exported_memories: Vec::new(),
            exported_globals: Vec::new(),
            wasm_vmval_storage: Vec::new(),

            vmctx2instance: HashMap::new(),
        }
    }

    pub(crate) fn take_wasm_vmval_storage(&mut self) -> Vec<VMVal> {
        mem::take(&mut self.wasm_vmval_storage)
    }

    pub(crate) fn return_wasm_vmval_storage(&mut self, storage: Vec<VMVal>) {
        self.wasm_vmval_storage = storage;
    }

    pub(crate) fn get_instance_from_vmctx(
        &self,
        vmctx: *mut VMContext,
    ) -> Stored<runtime::Instance> {
        let vmctx = VMOpaqueContext::from_vmcontext(vmctx);
        self.vmctx2instance[&vmctx]
    }

    pub(crate) fn push_instance(
        &mut self,
        mut instance: runtime::Instance,
    ) -> Stored<runtime::Instance> {
        let handle = Stored::new(self.instances.len());
        self.vmctx2instance.insert(
            VMOpaqueContext::from_vmcontext(instance.vmctx_mut()),
            handle,
        );
        self.instances.push(instance);
        handle
    }

    pub fn push_function(
        &mut self,
        func: runtime::ExportedFunction,
    ) -> Stored<runtime::ExportedFunction> {
        let index = self.exported_funcs.len();
        self.exported_funcs.push(func);
        Stored::new(index)
    }

    pub fn push_table(&mut self, table: runtime::ExportedTable) -> Stored<runtime::ExportedTable> {
        let index = self.exported_tables.len();
        self.exported_tables.push(table);
        Stored::new(index)
    }

    pub fn push_memory(
        &mut self,
        memory: runtime::ExportedMemory,
    ) -> Stored<runtime::ExportedMemory> {
        let index = self.exported_memories.len();
        self.exported_memories.push(memory);
        Stored::new(index)
    }

    pub fn push_global(
        &mut self,
        global: runtime::ExportedGlobal,
    ) -> Stored<runtime::ExportedGlobal> {
        let index = self.exported_globals.len();
        self.exported_globals.push(global);
        Stored::new(index)
    }
}

macro_rules! stored_impls {
    ($bind:ident $(($ty:path, $has:ident, $get:ident, $get_mut:ident, $field:expr))*) => {
        $(
            impl Store {
                pub fn $has(&self, index: Stored<$ty>) -> bool {
                    let $bind = self;
                    $field.get(index.index).is_some()
                }

                pub fn $get(&self, index: Stored<$ty>) -> Option<&$ty> {
                    let $bind = self;
                    $field.get(index.index)
                }

                pub fn $get_mut(&mut self, index: Stored<$ty>) -> Option<&mut $ty> {
                    let $bind = self;
                    $field.get_mut(index.index)
                }
            }

            impl ::core::ops::Index<Stored<$ty>> for Store {
                type Output = $ty;

                fn index(&self, index: Stored<$ty>) -> &Self::Output {
                    self.$get(index).unwrap()
                }
            }

            impl ::core::ops::IndexMut<Stored<$ty>> for Store {
                fn index_mut(&mut self, index: Stored<$ty>) -> &mut Self::Output {
                    self.$get_mut(index).unwrap()
                }
            }
        )*
    };
}

stored_impls! {
    s
    (runtime::Instance, has_instance, get_instance, get_instance_mut, s.instances)
    (runtime::ExportedFunction, has_function, get_function, get_function_mut, s.exported_funcs)
    (runtime::ExportedTable, has_table, get_table, get_table_mut, s.exported_tables)
    (runtime::ExportedMemory, has_memory, get_memory, get_memory_mut, s.exported_memories)
    (runtime::ExportedGlobal, has_global, get_global, get_global_mut, s.exported_globals)
}

pub struct Stored<T> {
    index: usize,
    _m: PhantomData<T>,
}

impl<T> Stored<T> {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            _m: PhantomData,
        }
    }
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