use crate::compile::{
    FunctionLoc, ELF_K23_INFO, ELF_K23_TRAPS, ELF_TEXT, ELF_WASM_DATA, ELF_WASM_DWARF,
    ELF_WASM_NAMES,
};
use crate::mmap_vec::MmapVec;
use crate::placeholder::Mmap;
use crate::trap;
use crate::trap::{trap_for_offset, Trap};
use core::ops::Range;
use core::{fmt, slice};
use object::{File, Object, ObjectSection};

pub struct CodeMemory {
    inner: Mmap,
    len: usize,
    published: bool,

    text: Range<usize>,
    wasm_data: Range<usize>,
    func_name_data: Range<usize>,
    trap_data: Range<usize>,
    dwarf: Range<usize>,
    info: Range<usize>,
}

impl CodeMemory {
    pub fn new(vec: MmapVec<u8>) -> Self {
        let obj = File::parse(vec.slice()).expect("failed to parse compilation artifact");

        let mut text = None;
        let mut wasm_data = Range::default();
        let mut func_name_data = Range::default();
        let mut trap_data = Range::default();
        let mut dwarf = Range::default();
        let mut info = Range::default();

        for section in obj.sections() {
            let name = section.name().unwrap();
            let range = unsafe {
                let range = section.data().unwrap().as_ptr_range();

                range.start as usize..range.end as usize
            };

            // Double-check that sections are all aligned properly.
            if section.align() != 0 && !range.is_empty() {
                // debug_assert!(
                //     range.is_aligned(usize::try_from(section.align()).unwrap()),
                //     "section `{}` isn't aligned to {:#x} ({range:?})",
                //     section.name().unwrap_or("ERROR"),
                //     section.align(),
                // );
            }

            match name {
                ELF_TEXT => {
                    // debug_assert!(
                    //     range.is_aligned(kconfig::PAGE_SIZE),
                    //     "text section isn't aligned to PAGE_SIZE"
                    // );

                    text = Some(range);
                }
                ELF_WASM_DATA => wasm_data = range,
                ELF_WASM_NAMES => func_name_data = range,
                ELF_WASM_DWARF => dwarf = range,

                ELF_K23_TRAPS => trap_data = range,
                ELF_K23_INFO => info = range,
                _ => {}
            }
        }

        let (mmap, len) = vec.into_parts();

        Self {
            inner: mmap,
            published: false,
            len,

            text: text.expect("object file had no text section"),
            wasm_data,
            func_name_data,
            trap_data,
            dwarf,
            info,
        }
    }

    pub fn symbol_map(&self) -> object::SymbolMap<object::SymbolMapName> {
        // TODO this is horrible wow
        File::parse(self.as_slice()).unwrap().symbol_map()
    }

    pub fn publish(&mut self) {
        debug_assert!(!self.published);
        self.published = true;

        if self.inner.is_empty() {
            return;
        }

        unsafe {
            self.inner.make_readonly(0..self.inner.len()).unwrap();

            let text = self.text();
            // Switch the executable portion from readonly to read/execute.
            let offset = self.inner.as_ptr() as usize;
            self.inner
                .make_executable(self.text.start - offset..self.text.end - offset, true)
                .expect("unable to make memory executable");
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.inner.as_ptr(), self.len) }
    }
    pub fn text(&self) -> &[u8] {
        let offset = self.inner.as_ptr() as usize;
        &self.as_slice()[self.text.start - offset..self.text.end - offset]
    }
    pub fn wasm_data(&self) -> &[u8] {
        let offset = self.inner.as_ptr() as usize;
        &self.as_slice()[self.wasm_data.start - offset..self.wasm_data.end - offset]
    }
    pub fn func_name_data(&self) -> &[u8] {
        let offset = self.inner.as_ptr() as usize;
        &self.as_slice()[self.func_name_data.start - offset..self.func_name_data.end - offset]
    }
    pub fn trap_data(&self) -> &[u8] {
        let offset = self.inner.as_ptr() as usize;
        &self.as_slice()[self.trap_data.start - offset..self.trap_data.end - offset]
    }
    pub fn dwarf(&self) -> &[u8] {
        let offset = self.inner.as_ptr() as usize;
        &self.as_slice()[self.dwarf.start - offset..self.dwarf.end - offset]
    }
    pub fn info(&self) -> &[u8] {
        let offset = self.inner.as_ptr() as usize;
        &self.as_slice()[self.info.start - offset..self.info.end - offset]
    }

    pub fn resolve_function_loc(&self, func_loc: FunctionLoc) -> usize {
        let addr = self.text.start + func_loc.start as usize;

        tracing::trace!(
            "resolve_function_loc {func_loc:?}, text {:?} => {:?}",
            self.text,
            addr,
        );

        // Assert the function location actually lies in our text section
        debug_assert!(self.text.start <= addr && self.text.end >= addr + func_loc.length as usize);

        addr
    }

    pub fn lookup_trap_code(&self, text_offset: usize) -> Option<Trap> {
        let text_offset = u32::try_from(text_offset).ok()?;
        trap_for_offset(self.trap_data(), text_offset)
    }
}

impl fmt::Debug for CodeMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeMemory")
            .field("published", &self.published)
            .field("text", &self.text)
            .field("wasm_data", &self.wasm_data)
            .field("func_name_data", &self.func_name_data)
            .field("trap_data", &self.trap_data)
            .field("dwarf", &self.dwarf)
            .field("info", &self.info)
            .finish()
    }
}
