use crate::parse::TablePlan;
use crate::utils::round_usize_up_to_host_pages;
use crate::vm::vmcontext::{VMFuncRef, VMTableDefinition};
use crate::vm::MmapVec;
use crate::TABLE_MAX;
use core::ptr::NonNull;

#[derive(Debug)]
pub struct Table {
    elements: MmapVec<Option<NonNull<VMFuncRef>>>,
    /// The optional maximum accessible size, in elements, for this table.
    maximum: Option<usize>,
}

impl Table {
    pub fn new(plan: &TablePlan, actual_maximum: Option<usize>) -> crate::Result<Self> {
        let reserve_size = TABLE_MAX.min(actual_maximum.unwrap_or(usize::MAX));

        // TODO allow more ref types
        assert!(plan.ty.element_type.is_func_ref());

        let elements = if reserve_size == 0 {
            MmapVec::new()
        } else {
            let mut elements = MmapVec::with_reserved(round_usize_up_to_host_pages(reserve_size))?;
            elements.try_extend_with(usize::try_from(plan.ty.initial).unwrap(), None)?;
            elements
        };

        Ok(Self {
            elements,
            maximum: actual_maximum,
        })
    }
    pub fn elements_mut(&mut self) -> &mut [Option<NonNull<VMFuncRef>>] {
        self.elements.slice_mut()
    }
    pub(crate) fn as_vmtable_definition(&mut self) -> VMTableDefinition {
        VMTableDefinition {
            base: self.elements.as_mut_ptr().cast(),
            current_length: self.elements.len() as u64,
        }
    }
}
