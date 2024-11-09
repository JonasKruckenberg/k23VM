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
}
