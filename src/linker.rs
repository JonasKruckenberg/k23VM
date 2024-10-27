use crate::const_eval::ConstExprEvaluator;
use crate::instance::{
    ExportFunction, ExportGlobal, ExportMemory, ExportTable, Extern, Imports, Instance,
};
use crate::instance_allocator::InstanceAllocator;
use crate::module::Module;
use crate::store::{InstanceHandle, Store};
use crate::translate::EntityType;
use crate::vmcontext::{VMFunctionImport, VMGlobalImport, VMMemoryImport, VMTableImport};
use alloc::sync::Arc;
use alloc::vec::Vec;
use hashbrown::hash_map::Entry;
use hashbrown::HashMap;
use wasmparser::{FuncType, GlobalType, MemoryType, TableType};

#[derive(Debug, Default)]
pub struct Linker {
    string2idx: HashMap<Arc<str>, usize>,
    strings: Vec<Arc<str>>,
    map: HashMap<ImportKey, Extern>,
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
struct ImportKey {
    name: usize,
    module: usize,
}

impl Linker {
    pub fn get(&self, module: &str, name: &str) -> Option<&Extern> {
        let key = ImportKey {
            module: *self.string2idx.get(module)?,
            name: *self.string2idx.get(name)?,
        };
        self.map.get(&key)
    }

    pub fn define_instance(
        &mut self,
        store: &mut Store,
        module_name: &str,
        instance: Instance,
    ) -> crate::TranslationResult<&mut Self> {
        let exports = instance
            .exports(store)
            .map(|e| (self.import_key(module_name, Some(e.name)), e.ext))
            .collect::<Vec<_>>(); // TODO can we somehow get rid of this?

        for (key, ext) in exports {
            self.insert(key, ext)?;
        }

        Ok(self)
    }

    pub fn instantiate<'wasm>(
        &self,
        store: &mut Store<'wasm>,
        alloc: &dyn InstanceAllocator,
        module: &Module<'wasm>,
        const_eval: &mut ConstExprEvaluator,
    ) -> crate::TranslationResult<Instance> {
        let mut imports = Imports::default();
        for import in module.imports() {
            let def = self
                .get(import.module, import.name)
                .expect("missing import");

            match import.ty {
                EntityType::Function(_) => unsafe {
                    // TODO typecheck
                    let f = def.unwrap_func().func_ref;

                    imports.functions.push(VMFunctionImport {
                        wasm_call: f.as_ref().wasm_call,
                        array_call: f.as_ref().array_call,
                        vmctx: f.as_ref().vmctx,
                    });
                },
                EntityType::Table(_) => {
                    // TODO typecheck
                    let t = def.unwrap_table();

                    imports.tables.push(VMTableImport {
                        from: t.definition,
                        vmctx: t.vmctx,
                    })
                }
                EntityType::Memory(_) => {
                    // TODO typecheck
                    let t = def.unwrap_memory();

                    imports.memories.push(VMMemoryImport {
                        from: t.definition,
                        vmctx: t.vmctx,
                    })
                }
                EntityType::Global(_) => {
                    // TODO typecheck
                    let t = def.unwrap_global();

                    imports.globals.push(VMGlobalImport {
                        from: t.definition,
                        vmctx: t.vmctx,
                    })
                }
                EntityType::Tag(_) => {
                    todo!()
                }
            }
        }

        Instance::new(store, alloc, const_eval, module.clone(), imports)
    }

    fn insert(&mut self, key: ImportKey, item: Extern) -> crate::TranslationResult<()> {
        match self.map.entry(key) {
            Entry::Occupied(_) => {
                let module = &self.strings[key.module];
                panic!("import defined twice");
            }
            Entry::Vacant(v) => {
                v.insert(item);
            }
        }

        Ok(())
    }

    fn import_key(&mut self, module: &str, name: Option<&str>) -> ImportKey {
        ImportKey {
            module: self.intern_str(module),
            name: name.map(|name| self.intern_str(name)).unwrap_or(usize::MAX),
        }
    }

    fn intern_str(&mut self, string: &str) -> usize {
        if let Some(idx) = self.string2idx.get(string) {
            return *idx;
        }
        let string: Arc<str> = string.into();
        let idx = self.strings.len();
        self.strings.push(string.clone());
        self.string2idx.insert(string, idx);
        idx
    }
}
