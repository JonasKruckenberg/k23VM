use crate::runtime::{ConstExprEvaluator, Imports, InstanceAllocator};
use crate::translate::EntityType;
use crate::{Engine, Extern, Instance, Module, Store};
use alloc::sync::Arc;
use alloc::vec::Vec;
use hashbrown::hash_map::Entry;
use hashbrown::HashMap;

#[derive(Debug)]
pub struct Linker {
    engine: Engine,
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
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
            string2idx: HashMap::new(),
            strings: Vec::new(),
            map: HashMap::new(),
        }
    }

    pub fn get(&self, module: &str, name: &str) -> Option<&Extern> {
        let key = ImportKey {
            module: *self.string2idx.get(module)?,
            name: *self.string2idx.get(name)?,
        };
        self.map.get(&key)
    }

    pub fn alias_module(&mut self, module: &str, as_module: &str) -> crate::Result<&mut Self> {
        let module = self.intern_str(module);
        let as_module = self.intern_str(as_module);
        let items = self
            .map
            .iter()
            .filter(|(key, _def)| key.module == module)
            .map(|(key, def)| (key.name, def.clone()))
            .collect::<Vec<_>>();
        for (name, item) in items {
            self.insert(
                ImportKey {
                    module: as_module,
                    name,
                },
                item,
            )?;
        }
        Ok(self)
    }

    pub fn define_instance(
        &mut self,
        store: &mut Store,
        module_name: &str,
        instance: Instance,
    ) -> crate::Result<&mut Self> {
        let exports = instance
            .exports(store)
            .map(|e| (self.import_key(module_name, Some(e.name)), e.value))
            .collect::<Vec<_>>(); // TODO can we somehow get rid of this?

        for (key, ext) in exports {
            self.insert(key, ext)?;
        }

        Ok(self)
    }

    pub fn instantiate(
        &self,
        store: &mut Store,
        alloc: &dyn InstanceAllocator,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<Instance> {
        let mut imports = Imports::with_capacity_for(module.translated());
        for import in module.imports() {
            let def = self.get(&import.module, &import.name).unwrap_or_else(|| {
                panic!(
                    "missing {:?} import {}::{}",
                    import.ty, import.module, import.name
                )
            });

            match (def, &import.ty) {
                (Extern::Func(func), EntityType::Function(_ty)) => {
                    imports.functions.push(func.as_vmfunction_import(store))
                }
                (Extern::Table(table), EntityType::Table(_ty)) => {
                    imports.tables.push(table.as_vmtable_import(store))
                }
                (Extern::Memory(memory), EntityType::Memory(_ty)) => {
                    imports.memories.push(memory.as_vmmemory_import(store))
                }
                (Extern::Global(global), EntityType::Global(_ty)) => {
                    imports.globals.push(global.as_vmglobal_import(store))
                }
                _ => panic!("mismatched import type"),
            }
        }

        unsafe { Instance::new_unchecked(store, alloc, const_eval, module.clone(), imports) }
    }

    fn insert(&mut self, key: ImportKey, item: Extern) -> crate::Result<()> {
        match self.map.entry(key) {
            Entry::Occupied(_) => {
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