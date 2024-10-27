use crate::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::memory::Memory;
use crate::table::Table;
use crate::translate::{MemoryPlan, TablePlan, TranslatedModule};
use crate::vmcontext::{OwnedVMContext, VMContextPlan};
use core::mem;
use cranelift_entity::PrimaryMap;

pub trait InstanceAllocator {
    unsafe fn allocate_vmctx(
        &self,
        module: &TranslatedModule,
        plan: &VMContextPlan,
    ) -> crate::TranslationResult<OwnedVMContext>;

    /// Allocate a memory for an instance.
    ///
    /// # Unsafety
    ///
    /// The memory and its associated module must have already been validated by
    /// `Self::validate_module` and passed that validation.
    unsafe fn allocate_memory(
        &self,
        module: &TranslatedModule,
        memory_plan: &MemoryPlan,
        memory_index: DefinedMemoryIndex,
    ) -> crate::TranslationResult<Memory>;
    /// Deallocate an instance's previously allocated memory.
    ///
    /// # Unsafety
    ///
    /// The memory must have previously been allocated by
    /// `Self::allocate_memory`, be at the given index, and must currently be
    /// allocated. It must never be used again.
    unsafe fn deallocate_memory(&self, memory_index: DefinedMemoryIndex, memory: Memory);
    /// Allocate a table for an instance.
    ///
    /// # Unsafety
    ///
    /// The table and its associated module must have already been validated by
    /// `Self::validate_module` and passed that validation.
    unsafe fn allocate_table(
        &self,
        module: &TranslatedModule,
        table_plan: &TablePlan,
        table_index: DefinedTableIndex,
    ) -> crate::TranslationResult<Table>;
    /// Deallocate an instance's previously allocated table.
    ///
    /// # Unsafety
    ///
    /// The table must have previously been allocated by `Self::allocate_table`,
    /// be at the given index, and must currently be allocated. It must never be
    /// used again.
    unsafe fn deallocate_table(&self, table_index: DefinedTableIndex, table: Table);

    unsafe fn allocate_memories(
        &self,
        module: &TranslatedModule,
        memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    ) -> crate::TranslationResult<()> {
        for (index, plan) in module.memory_plans.iter() {
            if let Some(def_index) = module.defined_memory_index(index) {
                let new_def_index = memories.push(self.allocate_memory(module, plan, def_index)?);
                debug_assert_eq!(def_index, new_def_index);
            }
        }
        Ok(())
    }

    unsafe fn allocate_tables(
        &self,
        module: &TranslatedModule,
        tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    ) -> crate::TranslationResult<()> {
        for (index, plan) in module.table_plans.iter() {
            if let Some(def_index) = module.defined_table_index(index) {
                let new_def_index = tables.push(self.allocate_table(module, plan, def_index)?);
                debug_assert_eq!(def_index, new_def_index);
            }
        }
        Ok(())
    }

    unsafe fn deallocate_memories(&self, memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>) {
        for (memory_index, memory) in mem::take(memories) {
            // Because deallocating memory is infallible, we don't need to worry
            // about leaking subsequent memories if the first memory failed to
            // deallocate. If deallocating memory ever becomes fallible, we will
            // need to be careful here!
            self.deallocate_memory(memory_index, memory);
        }
    }

    unsafe fn deallocate_tables(&self, tables: &mut PrimaryMap<DefinedTableIndex, Table>) {
        for (table_index, table) in mem::take(tables) {
            self.deallocate_table(table_index, table);
        }
    }
}
