use crate::guest_memory::mmap::Mmap;
use crate::guest_memory::round_usize_up_to_host_pages;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::{mem, ptr, slice};

#[derive(Debug)]
pub struct MmapVec<T> {
    mmap: Mmap,
    len: usize,
    _m: PhantomData<T>,
}

impl<T> MmapVec<T> {
    pub fn new() -> Self {
        Self {
            mmap: Mmap::new_empty(),
            len: 0,
            _m: PhantomData,
        }
    }
    pub fn new_zeroed(len: usize) -> crate::TranslationResult<Self> {
        Ok(Self {
            mmap: Mmap::new(len)?,
            len,
            _m: PhantomData,
        })
    }

    pub fn with_reserve(capacity: usize) -> crate::TranslationResult<Self> {
        Ok(Self {
            mmap: Mmap::with_reserve(capacity)?,
            len: 0,
            _m: PhantomData,
        })
    }

    pub fn reserve(&self) -> usize {
        self.mmap.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn slice(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.as_ptr(), self.len * size_of::<T>()) }
    }

    pub fn slice_mut(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), self.len * size_of::<T>()) }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.mmap.as_ptr().cast()
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.mmap.as_mut_ptr().cast()
    }

    pub fn try_extend_from_slice(&mut self, other: &[T]) -> crate::TranslationResult<()> {
        let count = (other).len();

        let mut tx = self.guard();
        let old_len = unsafe { tx.try_grow(count)? };
        unsafe {
            ptr::copy_nonoverlapping(other.as_ptr(), tx.vec.as_mut_ptr().add(old_len), count)
        };
        tx.finish();

        Ok(())
    }

    pub(crate) fn try_extend_with(&mut self, count: usize, elem: T) -> crate::TranslationResult<()>
    where
        T: Clone,
    {
        let mut tx = self.guard();
        let old_len = unsafe { tx.try_grow(count)? };
        tx.slice_mut()[old_len..].fill(elem);
        tx.finish();

        Ok(())
    }

    pub(crate) fn into_parts(self) -> (Mmap, usize) {
        (self.mmap, self.len)
    }

    unsafe fn try_grow(&mut self, additional: usize) -> crate::TranslationResult<usize> {
        let old_size = self.len;
        let old_accessible = self.accessible();

        if self.len + additional < self.mmap.len() {
            self.len += additional;
        } else {
            panic!("oom")
        }

        if self.accessible() > old_accessible {
            self.mmap
                .make_accessible(old_accessible, self.accessible() - old_accessible)?;
        }

        Ok(old_size)
    }

    fn accessible(&self) -> usize {
        let accessible = round_usize_up_to_host_pages(self.len);
        debug_assert!(accessible <= self.mmap.len());
        accessible
    }

    fn guard(&mut self) -> MmapVecGuard<'_, T> {
        MmapVecGuard {
            len: self.len,
            vec: self,
        }
    }
}

impl<T> Deref for MmapVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.slice()
    }
}

impl object::write::WritableBuffer for MmapVec<u8> {
    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn reserve(&mut self, size: usize) -> Result<(), ()> {
        debug_assert!(self.is_empty() && self.mmap.len() == 0);
        self.mmap = Mmap::with_reserve(round_usize_up_to_host_pages(size)).unwrap();
        Ok(())
    }

    #[inline]
    fn resize(&mut self, new_len: usize) {
        debug_assert!(new_len >= self.len());
        self.try_extend_with(new_len - self.len, 0).unwrap()
    }

    #[inline]
    fn write_bytes(&mut self, val: &[u8]) {
        debug_assert!(self.len() + val.len() <= self.mmap.len());
        self.try_extend_from_slice(val).unwrap();
    }
}

struct MmapVecGuard<'a, T> {
    len: usize,
    vec: &'a mut MmapVec<T>,
}

impl<T> Deref for MmapVecGuard<'_, T> {
    type Target = MmapVec<T>;

    fn deref(&self) -> &Self::Target {
        self.vec
    }
}

impl<T> DerefMut for MmapVecGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.vec
    }
}

impl<'a, T> MmapVecGuard<'a, T> {
    pub fn finish(self) {
        mem::forget(self)
    }
}

impl<'a, T> Drop for MmapVecGuard<'a, T> {
    fn drop(&mut self) {
        self.vec.len = self.len;
    }
}
