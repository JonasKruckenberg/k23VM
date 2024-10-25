use crate::guest_memory::Mmap;
use crate::translate::MemoryPlan;
use crate::vmcontext::VMMemoryDefinition;
use crate::MEMORY_MAX;

#[derive(Debug)]
pub struct Memory {
    /// The underlying allocation backing this memory
    mmap: Mmap,
    /// The current length of this Wasm memory, in bytes.
    len: usize,
    /// The optional maximum accessible size, in bytes, for this linear memory.
    ///
    /// This **does not** include guard pages and might be smaller than `self.accessible`
    /// since the underlying allocation is always a multiple of the host page size.
    maximum: Option<usize>,
    /// The log2 of this Wasm memory's page size, in bytes.
    page_size_log2: u8,
    /// Size in bytes of extra guard pages after the end to
    /// optimize loads and stores with constant offsets.
    offset_guard_size: usize,
}

impl Memory {
    pub fn new(
        plan: &MemoryPlan,
        actual_minimum_bytes: usize,
        actual_maximum_bytes: Option<usize>,
    ) -> Self {
        let offset_guard_bytes = usize::try_from(plan.offset_guard_size).unwrap();
        // Ensure that our guard regions are multiples of the host page size.
        let offset_guard_bytes =
            crate::guest_memory::round_usize_up_to_host_pages(offset_guard_bytes);

        let bound_bytes = crate::guest_memory::round_usize_up_to_host_pages(MEMORY_MAX);
        let allocation_bytes = bound_bytes.min(actual_maximum_bytes.unwrap_or(usize::MAX));

        let request_bytes = allocation_bytes.checked_add(offset_guard_bytes).unwrap();
        let mut mmap = Mmap::with_reserve(request_bytes).unwrap();

        if actual_minimum_bytes > 0 {
            let accessible =
                crate::guest_memory::round_usize_up_to_host_pages(actual_minimum_bytes);
            mmap.make_accessible(0, accessible).unwrap();
        }

        Self {
            mmap,
            len: actual_minimum_bytes,
            maximum: actual_maximum_bytes,
            page_size_log2: plan.page_size_log2,
            offset_guard_size: offset_guard_bytes,
        }
    }

    pub(crate) fn as_vmmemory_definition(&mut self) -> VMMemoryDefinition {
        VMMemoryDefinition {
            base: unsafe { self.mmap.as_mut_ptr() },
            current_length: self.len.into(),
        }
    }

    pub(crate) fn page_size_log2(&self) -> u8 {
        self.page_size_log2
    }

    pub(crate) fn byte_size(&self) -> usize {
        self.len
    }

    pub(crate) fn maximum_byte_size(&self) -> Option<usize> {
        self.maximum
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        unsafe { self.mmap.slice(0..self.len) }
    }
    pub(crate) fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { self.mmap.slice_mut(0..self.len) }
    }
}
