use crate::runtime::VMVal;
use crate::{runtime, Engine};
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::{fmt, mem};

#[derive(Debug)]
pub struct Store {
    engine: Engine,
    instances: Vec<runtime::Instance>,
    exported_funcs: Vec<runtime::ExportedFunction>,
    exported_tables: Vec<runtime::ExportedTable>,
    exported_memories: Vec<runtime::ExportedMemory>,
    exported_globals: Vec<runtime::ExportedGlobal>,
    wasm_vmval_storage: Vec<VMVal>,
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
        }
    }

    pub(crate) fn take_wasm_vmval_storage(&mut self) -> Vec<VMVal> {
        mem::take(&mut self.wasm_vmval_storage)
    }

    pub(crate) fn return_wasm_vmval_storage(&mut self, storage: Vec<VMVal>) {
        self.wasm_vmval_storage = storage;
    }
}

macro_rules! stored_impls {
    ($bind:ident $(($ty:path, $push:ident, $has:ident, $get:ident, $get_mut:ident, $field:expr))*) => {
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

                pub fn $push(&mut self, val: $ty) -> Stored<$ty> {
                    let $bind = self;
                    let index = $field.len();
                    $field.push(val);
                    Stored {
                        index,
                        _m: PhantomData
                    }
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
    (runtime::Instance, push_instance, has_instance, get_instance, get_instance_mut, s.instances)
    (runtime::ExportedFunction, push_function, has_function, get_function, get_function_mut, s.exported_funcs)
    (runtime::ExportedTable, push_table, has_table, get_table, get_table_mut, s.exported_tables)
    (runtime::ExportedMemory, push_memory, has_memory, get_memory, get_memory_mut, s.exported_memories)
    (runtime::ExportedGlobal, push_global, has_global, get_global, get_global_mut, s.exported_globals)
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
