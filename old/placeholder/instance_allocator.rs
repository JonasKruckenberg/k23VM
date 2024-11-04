//! A placeholder implementation of the `InstanceAllocator` trait that just delegates to the
//! `Mmap` based implementations of the memory and table types.
//! 
//! # The "production" version
//! 
//! This non-placeholder version of this allocator will be based on the pooling allocator strategy
//! implemented by Wasmtime see [here](https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html)
//! for details. But in essence, it takes the *entire* available virtual address range and splits it into 5 different 
//! pools (for code, vmctxs, tables, memories, and stacks) Each pool is then further divided into fixed-size
//! slots.
//!
//! The idea behind this design is to reduce the impact of virtual memory mapping on instantiation time,
//! by frontloading the mapping so that during normal operation *zero* mapping is required and
//! by splitting the into fixed, large regions to reduce the cost of that initial mapping.
//! 
//! ### Difference to Wasmtime
//! 
//! While the pooling strategy is disabled in Wasmtime by default, the reasons for it don't really apply
//! to k23.
//! - **Very large reservation of virtual memory** - k23 owns all the virtual address space, and
//!     we spin up a new address space for a new program anyway so the limit of ~64k linear memories
//!     per (48-bit) address space should be sufficient. If this ever becomes an issue we can use 
//!     larger addressing modes, reduce the max slot size, or consider using the kernel-half of the
//!     address space as well (why not)
//! - **Keeping unused memory alive** - Wasmtime's allocator keeps a number of "warm" (ie recently deallocated)
//!     slots paged-in which *will* retain their physical memory but that really isn't a concern for k23.
//!     
//!     For an OS like k23 "unused memory" really means "wasted memory", since the OS allocator 
//!     (in our case the pooling allocator) is **the only** allocator on the system. All memory not
//!     used by it are wasted since they will not be used by anyone else.
//! 
//!     This goes a step further too: An OS *should* ideally use all available physical memory 
//!     at all times (you paid for that hardware after all!). In traditional OSes "unused memory"
//!     will be used by the kernel to cache things like file system nodes etc. to help speed 
//!     up operations.
//!     Likewise, k23 should use all available memory to cache instance allocations, memories and such
//!     since that helps to speed up operations.

use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::instance_allocator::InstanceAllocator;
use crate::translate::{MemoryPlan, TranslatedModule, TablePlan};
use crate::runtime::{Memory, Table};
use crate::runtime::{OwnedVMContext, VMOffsets};

pub struct PlaceholderAllocatorDontUse;

impl InstanceAllocator for PlaceholderAllocatorDontUse {
    unsafe fn allocate_vmctx(
        &self,
        _module: &TranslatedModule,
        plan: &VMOffsets,
    ) -> crate::Result<OwnedVMContext> {
        OwnedVMContext::try_new(plan)
    }

    unsafe fn allocate_memory(
        &self,
        _module: &TranslatedModule,
        memory_plan: &MemoryPlan,
        _memory_index: DefinedMemoryIndex,
    ) -> crate::Result<Memory> {
        // TODO we could call out to some resource management instance here to obtain
        // dynamic "minimum" and "maximum" values that reflect the state of the real systems
        // memory consumption

        // If the minimum memory size overflows the size of our own address
        // space, then we can't satisfy this request, but defer the error to
        // later so the `store` can be informed that an effective oom is
        // happening.
        let minimum = memory_plan
            .minimum_byte_size()
            .ok()
            .and_then(|m| usize::try_from(m).ok())
            .expect("memory minimum size exceeds memory limits");

        // The plan stores the maximum size in units of wasm pages, but we
        // use units of bytes. Unlike for the `minimum` size we silently clamp
        // the effective maximum size to the limits of what we can track. If the
        // maximum size exceeds `usize` or `u64` then there's no need to further
        // keep track of it as some sort of runtime limit will kick in long
        // before we reach the statically declared maximum size.
        let maximum = memory_plan
            .maximum_byte_size()
            .ok()
            .and_then(|m| usize::try_from(m).ok());

        Memory::new(memory_plan, minimum, maximum)
    }

    unsafe fn deallocate_memory(&self, _memory_index: DefinedMemoryIndex, _memory: Memory) {}

    unsafe fn allocate_table(
        &self,
        _module: &TranslatedModule,
        table_plan: &TablePlan,
        _table_index: DefinedTableIndex,
    ) -> crate::Result<Table> {
        // TODO we could call out to some resource management instance here to obtain
        // dynamic "minimum" and "maximum" values that reflect the state of the real systems
        // memory consumption
        let maximum = table_plan.maximum.and_then(|m| usize::try_from(m).ok());

        Table::new(table_plan, maximum)
    }

    unsafe fn deallocate_table(&self, _table_index: DefinedTableIndex, _table: Table) {}
}
