//! Placeholder implementations for OS-dependent functionality.
//! 
//! This module only exists for initial hosted development and testing, it will be replaced
//! with k23-specific implementations when the VM gets migrated. Most of the code here is
//! pretty haphazard, copied-together and quite probably incredibly unreliable.

pub mod instance_allocator;
pub mod mmap;
pub mod setjmp;
pub mod signals;
pub mod code_registry;
