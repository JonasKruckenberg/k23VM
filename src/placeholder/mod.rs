mod instance_allocator;
mod mmap;

use crate::vm::CodeMemory;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

pub use instance_allocator::PlaceholderAllocatorDontUse;
pub use mmap::Mmap;

fn global_code() -> &'static RwLock<GlobalRegistry> {
    static GLOBAL_CODE: OnceLock<RwLock<GlobalRegistry>> = OnceLock::new();
    GLOBAL_CODE.get_or_init(Default::default)
}

type GlobalRegistry = BTreeMap<usize, (usize, Arc<CodeMemory>)>;

/// Find which registered region of code contains the given program counter, and
/// what offset that PC is within that module's code.
pub fn lookup_code(pc: usize) -> Option<(Arc<CodeMemory>, usize)> {
    let all_modules = global_code().read().unwrap();

    let (_end, (start, module)) = all_modules.range(pc..).next()?;
    let text_offset = pc.checked_sub(*start)?;
    Some((module.clone(), text_offset))
}

/// Registers a new region of code.
///
/// Must not have been previously registered and must be `unregister`'d to
/// prevent leaking memory.
///
/// This is required to enable trap_handling to work correctly since the signal handler
/// will lookup in the `GLOBAL_CODE` list to determine which a particular pc
/// is a trap or not.
pub fn register_code(code: &Arc<CodeMemory>) {
    let text = code.text();
    if text.is_empty() {
        return;
    }
    let start = text.as_ptr() as usize;
    let end = start + text.len() - 1;
    let prev = global_code()
        .write()
        .unwrap()
        .insert(end, (start, code.clone()));
    assert!(prev.is_none());
}
