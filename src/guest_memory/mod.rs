use crate::HOST_PAGE_SIZE;
use alloc::format;
use core::ops::{Deref, DerefMut};
mod code_memory;
mod mmap;
mod vec;

pub use code_memory::CodeMemory;
pub use mmap::Mmap;
pub use vec::MmapVec;

/// Is `bytes` a multiple of the host page size?
pub fn usize_is_multiple_of_host_page_size(bytes: usize) -> bool {
    bytes % HOST_PAGE_SIZE == 0
}

pub fn round_u64_up_to_host_pages(bytes: u64) -> u64 {
    let page_size = u64::try_from(HOST_PAGE_SIZE).unwrap();
    debug_assert!(page_size.is_power_of_two());
    bytes
        .checked_add(page_size - 1)
        .map(|val| val & !(page_size - 1))
        .expect(&format!(
            "{bytes} is too large to be rounded up to a multiple of the host page size of {page_size}"
        ))
}

/// Same as `round_u64_up_to_host_pages` but for `usize`s.
pub fn round_usize_up_to_host_pages(bytes: usize) -> usize {
    let bytes = u64::try_from(bytes).unwrap();
    let rounded = round_u64_up_to_host_pages(bytes);
    usize::try_from(rounded).unwrap()
}
