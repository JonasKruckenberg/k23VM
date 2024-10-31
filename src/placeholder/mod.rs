//! Placeholder implementations for OS-dependent functionality.
//! 
//! This module only exists for initial hosted development and testing, it will be replaced
//! with k23-specific implementations when the VM gets migrated. Most of the code here is
//! pretty haphazard, copied-together and quite probably incredibly unreliable.

mod instance_allocator;
mod mmap;
mod setjmp;
mod signals;
mod code_registry;

pub use instance_allocator::PlaceholderAllocatorDontUse;
pub use mmap::Mmap;
pub use setjmp::{jmp_buf, longjmp, setjmp};
pub use signals::register_signal_handler;
pub use code_registry::{register_code, lookup_code};
