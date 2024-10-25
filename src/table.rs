use core::ops::Deref;
use crate::guest_memory::{round_usize_up_to_host_pages, MmapVec};
use crate::translate::TablePlan;
use crate::vmcontext::{VMFuncRef, VMTableDefinition};
use crate::TABLE_MAX;
use core::ptr::NonNull;

#[derive(Debug)]
pub struct Table {
    elements: MmapVec<Option<NonNull<VMFuncRef>>>,
    /// The optional maximum accessible size, in elements, for this table.
    maximum: Option<usize>,
}

impl Table {
    pub fn new(plan: &TablePlan, actual_maximum: Option<usize>) -> Self {
        let reserve_size = TABLE_MAX.min(actual_maximum.unwrap_or(usize::MAX));

        // TODO allow more ref types
        assert!(plan.ty.element_type.is_func_ref());

        let mut elements = MmapVec::with_reserve(round_usize_up_to_host_pages(reserve_size)).unwrap();
        elements.try_extend_with(usize::try_from(plan.ty.initial).unwrap(), None);

        Self {
            elements,
            maximum: actual_maximum,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn as_vmtable_definition(&mut self) -> VMTableDefinition {
        VMTableDefinition {
            base: self.elements.as_mut_ptr() as *mut u8,
            current_length: u64::try_from(self.len()).unwrap(),
        }
    }
    
    pub fn elements(&self) -> &[Option<NonNull<VMFuncRef>>] {
        self.elements.slice()
    }
    pub fn elements_mut(&mut self) -> &mut [Option<NonNull<VMFuncRef>>] {
        self.elements.slice_mut()
    }
}
