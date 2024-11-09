use crate::compile::FunctionLoc;
use crate::placeholder::mmap::Mmap;
use crate::runtime::MmapVec;

#[derive(Debug)]
pub struct CodeMemory {
    mmap: Mmap,
    len: usize,
    published: bool,
}

impl CodeMemory {
    pub fn new(mmap_vec: MmapVec<u8>) -> Self {
        let (mmap, size) = mmap_vec.into_parts();
        Self {
            mmap,
            len: size,
            published: false,
        }
    }

    pub fn publish(&mut self) -> crate::Result<()> {
        debug_assert!(!self.published);
        self.published = true;

        if self.mmap.is_empty() {
            tracing::warn!("Compiled module has no code to publish");
            return Ok(());
        }

        unsafe {
            self.mmap.make_readonly(0..self.len)?;

            // Switch the executable portion from readonly to read/execute.
            self.mmap.make_executable(0..self.len, true)?;
        }

        Ok(())
    }

    #[inline]
    pub fn text(&self) -> &[u8] {
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
            text_range.start <= addr && text_range.end >= addr + func_loc.length as usize
        );

        addr
    }
}
