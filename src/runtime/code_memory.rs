use crate::compile::FunctionLoc;
use crate::placeholder::mmap::Mmap;
use crate::runtime::MmapVec;
use crate::trap::Trap;
use alloc::vec::Vec;

#[derive(Debug)]
pub struct CodeMemory {
    mmap: Mmap,
    len: usize,
    published: bool,

    trap_offsets: Vec<u32>,
    traps: Vec<Trap>,
}

impl CodeMemory {
    pub fn new(mmap_vec: MmapVec<u8>, trap_offsets: Vec<u32>, traps: Vec<Trap>) -> Self {
        let (mmap, size) = mmap_vec.into_parts();
        Self {
            mmap,
            len: size,
            published: false,
            trap_offsets,
            traps,
        }
    }

    pub fn publish(&mut self) -> crate::Result<()> {
        debug_assert!(!self.published);
        self.published = true;

        if self.mmap.is_empty() {
            tracing::warn!("Compiled module has no code to publish");
            return Ok(());
        }

        self.mmap.make_readonly(0..self.len)?;

        // Switch the executable portion from readonly to read/execute.
        self.mmap.make_executable(0..self.len, true)?;

        Ok(())
    }

    #[inline]
    pub fn text(&self) -> &[u8] {
        // Safety: The constructor has to ensure that `self.len` is valid.
        unsafe { self.mmap.slice(0..self.len) }
    }

    pub fn resolve_function_loc(&self, func_loc: FunctionLoc) -> usize {
        let text_range = {
            let r = self.text().as_ptr_range();
            r.start as usize..r.end as usize
        };

        let addr = text_range.start + func_loc.start as usize;

        tracing::trace!(
            "resolve_function_loc {func_loc:?}, text {:?} => {:?}",
            self.mmap.as_ptr(),
            addr,
        );

        // Assert the function location actually lies in our text section
        debug_assert!(
            text_range.start <= addr
                && text_range.end >= addr.saturating_add(usize::try_from(func_loc.length).unwrap())
        );

        addr
    }

    pub fn lookup_trap_code(&self, text_offset: usize) -> Option<Trap> {
        let text_offset = u32::try_from(text_offset).unwrap();

        let index = self
            .trap_offsets
            .binary_search_by_key(&text_offset, |val| *val)
            .ok()?;

        Some(self.traps[index])
    }
}
