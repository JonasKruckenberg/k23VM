//! Support for building and parsing intermediate compilation artifacts in object format

use crate::compile::{CompileOutput, CompiledFunction, CompiledModule, Compiler, FunctionLoc, FunctionName, RelocationTarget, UnlinkedCompileOutputs};
use crate::translate::{DebugInfo, TranslatedModule, Translation};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::Range;
use cranelift_codegen::control::ControlPlane;
use object::write::{
    Object, SectionId, StandardSegment, Symbol, SymbolId, SymbolSection, WritableBuffer,
};
use object::{
    SectionKind, SymbolFlags, SymbolKind, SymbolScope,
};
use crate::compile::trap::TrapSectionBuilder;
use crate::compile::address_map::AddressMapSectionBuilder;

pub const ELFOSABI_K23: u8 = 223;

pub const ELF_TEXT: &str = ".text";
pub const ELF_WASM_DATA: &str = ".rodata.wasm";
pub const ELF_WASM_NAMES: &str = ".name.wasm";
pub const ELF_WASM_DWARF: &str = ".k23.dwarf";
pub const ELF_K23_TRAPS: &str = ".k23.trap_handling";
pub const ELF_K23_ADDRESS_MAP: &str = ".k23.address_map";
pub const ELF_K23_INFO: &str = ".k23.info";

/// Builder for intermediate compilation artifacts in ELF format
pub struct ObjectBuilder<'obj> {
    result: Object<'obj>,
    data_section: SectionId,
    dwarf_section: Option<SectionId>,
    names_section: Option<SectionId>,
}

impl<'obj> ObjectBuilder<'obj> {
    pub fn new(mut obj: Object<'obj>) -> Self {
        let data_section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            ELF_WASM_DATA.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );

        ObjectBuilder {
            result: obj,
            data_section,
            dwarf_section: None,
            names_section: None,
        }
    }

    /// Constructs a new helper [`TextSectionBuilder`] which can be used to
    /// build and append the objects text section.
    pub fn text_builder(
        &mut self,
        text_builder: Box<dyn cranelift_codegen::TextSectionBuilder>,
    ) -> TextSectionBuilder<'_, 'obj> {
        TextSectionBuilder::new(&mut self.result, text_builder)
    }

    pub(crate) fn append(
        &mut self,
        compiler: &dyn Compiler,
        unlinked_outputs: UnlinkedCompileOutputs,
        translation: Translation,
    ) -> crate::Result<CompiledModule> {
        let Translation {
            mut module,
            debug_info,
            data,
            passive_data,
            ..
        } = translation;

        tracing::trace!("Appending compiled functions to intermediate code object...");
        let funcs = unlinked_outputs.link_and_append(self, compiler, &module)?;

        tracing::trace!("Appending read-only data to intermediate code object...");
        self.append_rodata(&mut module, data, passive_data);

        // If any names are present in the module then the `ELF_NAME_DATA` section
        // is create and appended.
        tracing::trace!("Appending name data to intermediate code object...");
        let func_names = self.append_names(&debug_info);

        tracing::trace!("Appending DWARF debug info to intermediate code object...");
        let mut dwarf_section_info = Vec::with_capacity(15); // The number of sections we append
        self.append_dwarf_debug_info(&mut dwarf_section_info, &debug_info);

        // Sort this for binary-search-lookup later in `symbolize_context`.
        dwarf_section_info.sort_by_key(|(id, _)| *id);

        Ok(CompiledModule {
            module,
            funcs,
            func_names,
            dwarf: dwarf_section_info,
            code_section_offset: debug_info.code_section_offset,
        })
    }

    fn append_rodata(
        &mut self,
        module: &mut TranslatedModule,
        data: Vec<&[u8]>,
        passive_data: Vec<&[u8]>,
    ) {
        // Place all data from the wasm module into a section which will the
        // source of the data later at runtime. This additionally keeps track of
        // the offset of
        let mut total_data_len = 0;
        let data_offset = self.result.append_section_data(self.data_section, &[], 1);
        for data in data.iter() {
            self.result.append_section_data(self.data_section, data, 1);
            total_data_len += data.len();
        }
        for data in passive_data.iter() {
            self.result.append_section_data(self.data_section, data, 1);
        }

        // The offsets in `ParsedModule::memory_initializers` and `ParsedModule::passive_memory_initializers`
        // are all relative to the `translation.data` vec which is now being appended to the end of this object
        // files data section. Since one object file can potentially hold multiple modules (e.g. wasm component model)
        // we make sure to account for the possibility of data already present.
        // This just means we need to adjust offsets.

        let data_offset = u32::try_from(data_offset).unwrap();
        for init in module.memory_initializers.iter_mut() {
            init.data.start = init.data.start.checked_add(data_offset).unwrap();
            init.data.end = init.data.end.checked_add(data_offset).unwrap()
        }

        let data_offset = data_offset + u32::try_from(total_data_len).unwrap();
        for (_, range) in module.passive_memory_initializers.iter_mut() {
            range.start = range.start.checked_add(data_offset).unwrap();
            range.end = range.end.checked_add(data_offset).unwrap();
        }
    }

    fn append_names(&mut self, debug_info: &DebugInfo) -> Vec<FunctionName> {
        let mut func_names = Vec::new();
        if debug_info.names.func_names.len() > 0 {
            let name_id = *self.names_section.get_or_insert_with(|| {
                self.result.add_section(
                    self.result.segment_name(StandardSegment::Data).to_vec(),
                    ELF_WASM_NAMES.as_bytes().to_vec(),
                    SectionKind::ReadOnlyData,
                )
            });
            let mut sorted_names = debug_info.names.func_names.iter().collect::<Vec<_>>();
            sorted_names.sort_by_key(|(idx, _name)| *idx);
            for (idx, name) in sorted_names {
                let offset = self.result.append_section_data(name_id, name.as_bytes(), 1);
                let offset = match u32::try_from(offset) {
                    Ok(offset) => offset,
                    Err(_) => panic!("name section too large (> 4gb)"),
                };
                let len = u32::try_from(name.len()).unwrap();
                func_names.push(FunctionName {
                    idx: *idx,
                    offset,
                    len,
                });
            }
        }
        func_names
    }

    fn append_dwarf_debug_info(
        &mut self,
        section_info: &mut Vec<(u8, Range<u64>)>,
        info: &DebugInfo,
    ) {
        self.append_dwarf_section(section_info, &info.dwarf.debug_abbrev);
        self.append_dwarf_section(section_info, &info.dwarf.debug_addr);
        self.append_dwarf_section(section_info, &info.dwarf.debug_info);
        self.append_dwarf_section(section_info, &info.dwarf.debug_line);
        self.append_dwarf_section(section_info, &info.dwarf.debug_line_str);
        self.append_dwarf_section(section_info, &info.dwarf.debug_str);
        self.append_dwarf_section(section_info, &info.dwarf.debug_str_offsets);
        if let Some(inner) = &info.dwarf.sup {
            self.append_dwarf_section(section_info, &inner.debug_str);
        }
        self.append_dwarf_section(section_info, &info.dwarf.debug_types);
        self.append_dwarf_section(section_info, &info.debug_loc);
        self.append_dwarf_section(section_info, &info.debug_loclists);
        self.append_dwarf_section(section_info, &info.debug_ranges);
        self.append_dwarf_section(section_info, &info.debug_rnglists);
        self.append_dwarf_section(section_info, &info.debug_cu_index);
        self.append_dwarf_section(section_info, &info.debug_tu_index);
    }

    fn append_dwarf_section<'b, T>(
        &mut self,
        dwarf_section_info: &mut Vec<(u8, Range<u64>)>,
        section: &T,
    ) where
        T: gimli::Section<gimli::EndianSlice<'b, gimli::LittleEndian>>,
    {
        let data = section.reader().slice();
        if data.is_empty() {
            return;
        }

        let section_id = *self.dwarf_section.get_or_insert_with(|| {
            self.result.add_section(
                self.result.segment_name(StandardSegment::Debug).to_vec(),
                ELF_WASM_DWARF.as_bytes().to_vec(),
                SectionKind::Debug,
            )
        });
        let offset = self.result.append_section_data(section_id, data, 1);
        dwarf_section_info.push((T::id() as u8, offset..offset + data.len() as u64))
    }

    /// Finished the object and flushes it into the given buffer
    pub fn finish<T: WritableBuffer>(self, buf: &mut T) -> object::write::Result<()> {
        self.result.emit(buf)
    }
}

pub struct TextSectionBuilder<'a, 'obj> {
    /// The object file that generated code will be placed into
    obj: &'a mut Object<'obj>,
    /// The text section ID in the object
    text_section: SectionId,
    /// The cranelift `TextSectionBuilder` that keeps the in-progress text section
    /// that we're building
    inner: Box<dyn cranelift_codegen::TextSectionBuilder>,
    /// Last offset within the text section
    len: u64,

    ctrl_plane: ControlPlane,
}

impl<'a, 'obj> TextSectionBuilder<'a, 'obj> {
    pub fn new(
        obj: &'a mut Object<'obj>,
        text_builder: Box<dyn cranelift_codegen::TextSectionBuilder>,
    ) -> Self {
        let text_section = obj.add_section(
            obj.segment_name(StandardSegment::Text).to_vec(),
            ELF_TEXT.as_bytes().to_vec(),
            SectionKind::Text,
        );

        Self {
            obj,
            text_section,
            inner: text_builder,
            ctrl_plane: ControlPlane::default(),
            len: 0,
        }
    }

    pub fn push_funcs<'b>(
        &mut self,
        funcs: impl ExactSizeIterator<Item = &'b CompileOutput> + 'b,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> Vec<(SymbolId, FunctionLoc)> {
        let mut ret = Vec::with_capacity(funcs.len());
        let mut traps = TrapSectionBuilder::default();
        let mut address_map = AddressMapSectionBuilder::default();

        for output in funcs {
            let (sym, range) =
                self.push_func(&output.symbol, &output.function, &resolve_reloc_target);

            traps.push_traps(&range, output.function.traps());
            address_map.push(&range, output.function.metadata().address_map.iter());

            let info = FunctionLoc {
                start: u32::try_from(range.start).unwrap(),
                length: u32::try_from(range.end - range.start).unwrap(),
            };

            ret.push((sym, info));
        }

        traps.append(self.obj);
        address_map.append(self.obj);

        ret
    }

    /// Append the `func` with name `name` to this object.
    pub fn push_func(
        &mut self,
        name: &str,
        compiled_func: &CompiledFunction,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> (SymbolId, Range<u64>) {
        let body = compiled_func.buffer.data();
        let alignment = compiled_func.alignment;
        let body_len = body.len() as u64;
        let off = self
            .inner
            .append(true, body, alignment, &mut self.ctrl_plane);

        let symbol_id = self.obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: off,
            size: body_len,
            kind: SymbolKind::Text,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(self.text_section),
            flags: SymbolFlags::None,
        });

        for r in compiled_func.relocations() {
            let target = resolve_reloc_target(r.target);

            // Ensure that we actually resolved the relocation
            debug_assert!(self.inner.resolve_reloc(
                off + u64::from(r.offset),
                r.kind,
                r.addend,
                target
            ));
        }

        self.len = off + body_len;

        (symbol_id, off..off + body_len)
    }

    /// Finish building the text section and flush it into the object file
    pub fn finish(mut self, text_alignment: u64) {
        let text = self.inner.finish(&mut self.ctrl_plane);

        self.obj
            .section_mut(self.text_section)
            .set_data(text, text_alignment);
    }
}

