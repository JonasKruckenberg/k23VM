pub mod arch;
pub mod instance_allocator;
pub mod mmap;
mod setjmp;
pub(crate) mod signals;
pub mod code_registry;
pub mod trap_handling;

use std::convert::TryInto;

/// Returns the host page size in bytes.
pub fn host_page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE).try_into().unwrap() }
}