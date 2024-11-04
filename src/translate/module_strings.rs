//! Interning of strings within WASM.
//!
//! WASM contains very few strings, but notably imports and exports are identified by strings.
//! Since especially the module being imported from is repeated many times, interning these strings
//! helps to reduce allocations and improve lookup performance.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::Index;
use hashbrown::HashMap;

/// An index for a string interned within a single WebAssembly module.
///
/// Note that this only deduplicates strings within a single module, not across modules.
#[derive(Debug, Clone, Copy, Eq, PartialOrd, Ord, PartialEq, Hash)]
pub struct ModuleInternedStr(usize);

/// A collection of strings interned within a single WebAssembly module.
#[derive(Debug, Default)]
pub struct ModuleStrings {
    string2idx: HashMap<Arc<str>, ModuleInternedStr>,
    strings: Vec<Arc<str>>,
}

impl Index<ModuleInternedStr> for ModuleStrings {
    type Output = str;

    fn index(&self, id: ModuleInternedStr) -> &str {
        self.get(id).unwrap()
    }
}

impl ModuleStrings {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            string2idx: HashMap::with_capacity(cap),
            strings: Vec::with_capacity(cap),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<ModuleInternedStr> {
        self.string2idx.get(name).copied()
    }

    pub fn get(&self, id: ModuleInternedStr) -> Option<&str> {
        self.strings.get(id.0).map(|str| str.as_ref())
    }

    pub fn intern(&mut self, string: &str) -> ModuleInternedStr {
        if let Some(idx) = self.string2idx.get(string) {
            return *idx;
        }
        let string: Arc<str> = string.into();
        let idx = ModuleInternedStr(self.strings.len());
        self.strings.push(string.clone());
        self.string2idx.insert(string, idx);
        idx
    }
}
