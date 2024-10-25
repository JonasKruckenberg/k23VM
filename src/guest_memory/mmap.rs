use crate::guest_memory::usize_is_multiple_of_host_page_size;
use crate::HOST_PAGE_SIZE;
use core::ops::Range;
use core::ptr::NonNull;
use core::{ptr, slice};
use rustix::mm::MprotectFlags;

#[derive(Debug)]
pub struct Mmap {
    memory: NonNull<[u8]>,
}

impl Mmap {
    pub fn new_empty() -> Self {
        Self {
            memory: NonNull::from(&mut []),
        }
    }

    pub fn new(size: usize) -> crate::TranslationResult<Self> {
        assert!(usize_is_multiple_of_host_page_size(size));
        let ptr = unsafe {
            rustix::mm::mmap_anonymous(
                ptr::null_mut(),
                size,
                rustix::mm::ProtFlags::READ | rustix::mm::ProtFlags::WRITE,
                rustix::mm::MapFlags::PRIVATE,
            )
            .unwrap()
        };
        let memory = unsafe { slice::from_raw_parts_mut(ptr.cast(), size) };
        let memory = NonNull::new(memory).unwrap();
        Ok(Mmap { memory })
    }

    pub fn with_reserve(size: usize) -> crate::TranslationResult<Self> {
        assert!(usize_is_multiple_of_host_page_size(size));
        let ptr = unsafe {
            rustix::mm::mmap_anonymous(
                ptr::null_mut(),
                size,
                rustix::mm::ProtFlags::empty(),
                rustix::mm::MapFlags::PRIVATE,
            )
            .unwrap()
        };

        let memory = unsafe { slice::from_raw_parts_mut(ptr.cast(), size) };
        let memory = NonNull::new(memory).unwrap();
        Ok(Mmap { memory })
    }

    #[inline]
    pub unsafe fn slice(&self, range: Range<usize>) -> &[u8] {
        assert!(range.start <= range.end);
        assert!(range.end <= self.len());
        slice::from_raw_parts(self.as_ptr().add(range.start), range.end - range.start)
    }
    pub unsafe fn slice_mut(&mut self, range: Range<usize>) -> &mut [u8] {
        assert!(range.start <= range.end);
        assert!(range.end <= self.len());
        slice::from_raw_parts_mut(self.as_mut_ptr().add(range.start), range.end - range.start)
    }
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.memory.as_ptr() as *const u8
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.memory.as_ptr().cast()
    }

    #[inline]
    pub fn len(&self) -> usize {
        unsafe { (*self.memory.as_ptr()).len() }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn make_accessible(&mut self, start: usize, len: usize) -> crate::TranslationResult<()> {
        let ptr = self.memory.as_ptr();
        unsafe {
            rustix::mm::mprotect(
                ptr.byte_add(start).cast(),
                len,
                MprotectFlags::READ | MprotectFlags::WRITE,
            )
            .unwrap()
        }

        Ok(())
    }

    pub unsafe fn make_executable(
        &self,
        range: Range<usize>,
        enable_branch_protection: bool,
    ) -> crate::TranslationResult<()> {
        assert!(range.start <= self.len());
        assert!(range.end <= self.len());
        assert!(range.start <= range.end);
        assert_eq!(
            range.start % HOST_PAGE_SIZE,
            0,
            "changing of protections isn't page-aligned",
        );

        let base = self.memory.as_ptr().byte_add(range.start).cast();
        let len = range.end - range.start;

        let flags = MprotectFlags::READ | MprotectFlags::EXEC;
        let flags = if enable_branch_protection {
            #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
            if std::arch::is_aarch64_feature_detected!("bti") {
                MprotectFlags::from_bits_retain(flags.bits() | /* PROT_BTI */ 0x10)
            } else {
                flags
            }

            #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
            flags
        } else {
            flags
        };

        rustix::mm::mprotect(base, len, flags).unwrap();

        Ok(())
    }

    pub unsafe fn make_readonly(
        &self,
        range: Range<usize>,
        enable_branch_protection: bool,
    ) -> crate::TranslationResult<()> {
        assert!(range.start <= self.len());
        assert!(range.end <= self.len());
        assert!(range.start <= range.end);
        assert_eq!(
            range.start % HOST_PAGE_SIZE,
            0,
            "changing of protections isn't page-aligned",
        );

        let base = self.memory.as_ptr().byte_add(range.start).cast();
        let len = range.end - range.start;

        rustix::mm::mprotect(base, len, MprotectFlags::READ).unwrap();

        Ok(())
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        unsafe {
            let ptr = self.memory.as_ptr().cast();
            let len = (*self.memory.as_ptr()).len();
            if len == 0 {
                return;
            }
            rustix::mm::munmap(ptr, len).expect("munmap failed");
        }
    }
}
