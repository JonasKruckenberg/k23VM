use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::instance_allocator::InstanceAllocator;
use crate::memory::Memory;
use crate::table::Table;
use crate::translate::{MemoryPlan, TablePlan, TranslatedModule};
use crate::vmcontext::{OwnedVMContext, VMContextPlan};

pub struct PlaceholderAllocatorDontUse;

impl InstanceAllocator for PlaceholderAllocatorDontUse {
    unsafe fn allocate_vmctx(
        &self,
        module: &TranslatedModule,
        plan: &VMContextPlan,
    ) -> crate::TranslationResult<OwnedVMContext> {
        OwnedVMContext::try_new(plan)
    }

    unsafe fn allocate_memory(
        &self,
        module: &TranslatedModule,
        memory_plan: &MemoryPlan,
        memory_index: DefinedMemoryIndex,
    ) -> crate::TranslationResult<Memory> {
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

        Ok(Memory::new(memory_plan, minimum, maximum))
    }

    unsafe fn deallocate_memory(&self, memory_index: DefinedMemoryIndex, memory: Memory) {}

    unsafe fn allocate_table(
        &self,
        module: &TranslatedModule,
        table_plan: &TablePlan,
        table_index: DefinedTableIndex,
    ) -> crate::TranslationResult<Table> {
        // TODO we could call out to some resource management instance here to obtain
        // dynamic "minimum" and "maximum" values that reflect the state of the real systems
        // memory consumption
        let maximum = table_plan.ty.maximum.and_then(|m| usize::try_from(m).ok());

        Ok(Table::new(table_plan, maximum))
    }

    unsafe fn deallocate_table(&self, table_index: DefinedTableIndex, table: Table) {}
}