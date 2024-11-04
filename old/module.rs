use crate::compile::{CompileInputs, CompiledModule, Compiler, ObjectBuilder};
use crate::indices::{EntityIndex, FuncIndex};
use crate::runtime::{CodeMemory, MmapVec, VMOffsets};
use crate::translate::{Import, ModuleInternedStr, ModuleStrings, ModuleTranslator};
use alloc::sync::Arc;
use core::mem;
use gimli::EndianSlice;
use wasmparser::Validator;

#[derive(Debug, Clone)]
pub struct Module(pub(crate) Arc<ModuleInner>);

impl Module {
    pub fn from_wat(
        validator: &mut Validator,
        compiler: &dyn Compiler,
        str: &str,
    ) -> crate::Result<Self> {
        Self::from_bytes(validator, compiler, &wat::parse_str(str)?)
    }

    pub fn from_bytes(
        validator: &mut Validator,
        compiler: &dyn Compiler,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        tracing::trace!("Parsing WASM module...");

        let (mut translation, types, strings) =
            ModuleTranslator::new(validator).translate(bytes)?;
        // TODO assert compatability with target features

        let function_body_data = mem::take(&mut translation.function_bodies);
        // TODO figure out correct ptr size source
        let vmoffsets = VMOffsets::for_module(&translation.module, size_of::<usize>() as u32);


        let inputs = CompileInputs::from_translation(&translation, &types, function_body_data);
        let unlinked_outputs = inputs.compile(compiler)?;

        let mut obj_builder = ObjectBuilder::new(compiler.create_intermediate_code_object());
        let module = obj_builder.append(compiler, unlinked_outputs, translation)?;

        tracing::trace!("Allocating new output buffer for compiled module...");
        let mut code_buffer = MmapVec::new();

        obj_builder.finish(&mut code_buffer)?;

        let mut code = CodeMemory::new(code_buffer);
        code.publish()?;
        let code = Arc::new(code);

        crate::placeholder::code_registry::register_code(&code);

        Ok(Self(Arc::new(ModuleInner {
            module: Arc::new(module),
            vmoffsets,
            code,
            strings
        })))
    }

    pub fn imports(&self) -> impl ExactSizeIterator<Item = &Import> {
        self.0.module.module.imports.iter()
    }

    pub fn exports(&self) -> impl ExactSizeIterator<Item = (&str, EntityIndex)> + '_ {
        self.0
            .module
            .module
            .exports
            .iter()
            .map(|(name, index)| {
                let name = &self.0.strings[*name];

                (name, *index)

            })
    }

    pub fn name(&self) -> Option<&str> {
        Some(&self.0.strings[self.0.module.module.name?])
    }

    pub(crate) fn symbolize_context(&self) -> crate::Result<Option<SymbolizeContext<'_>>> {
        let dwarf = gimli::Dwarf::load(|id| -> crate::Result<_> {
            let data = self
                .compiled()
                .dwarf
                .binary_search_by_key(&(id as u8), |(id, _)| *id)
                .ok()
                .and_then(|i| {
                    let (_, range) = &self.compiled().dwarf[i];
                    let start = range.start.try_into().ok()?;
                    let end = range.end.try_into().ok()?;
                    self.code().dwarf().get(start..end)
                })
                .unwrap_or(&[]);

            Ok(EndianSlice::new(data, gimli::LittleEndian))
        })?;

        let cx = addr2line::Context::from_dwarf(dwarf)?;

        Ok(Some(SymbolizeContext {
            inner: cx,
            code_section_offset: self.compiled().code_section_offset,
        }))
    }
    pub(crate) fn get_export(&self, name: ModuleInternedStr) -> Option<EntityIndex> {
        self.0.module.module.exports.get(&name).copied()
    }
    pub(crate) fn compiled(&self) -> &CompiledModule {
        &self.0.module
    }
    pub(crate) fn code(&self) -> &CodeMemory {
        &self.0.code
    }
    pub(crate) fn vmoffsets(&self) -> &VMOffsets {
        &self.0.vmoffsets
    }

    pub(crate) fn func_name(&self, idx: FuncIndex) -> Option<&str> {
        // Find entry for `idx`, if present.
        let i = self
            .compiled()
            .func_names
            .binary_search_by_key(&idx, |n| n.idx)
            .ok()?;
        let name = &self.compiled().func_names[i];

        // Here we `unwrap` the `from_utf8` but this can theoretically be a
        // `from_utf8_unchecked` if we really wanted since this section is
        // guaranteed to only have valid utf-8 data. Until it's a problem it's
        // probably best to double-check this though.
        let data = self.code().func_name_data();
        Some(core::str::from_utf8(&data[name.offset as usize..][..name.len as usize]).unwrap())
    }
}

#[derive(Debug)]
pub struct ModuleInner {
    module: Arc<CompiledModule>,
    code: Arc<CodeMemory>,
    vmoffsets: VMOffsets,
    pub strings: ModuleStrings
}

type Addr2LineContext<'a> = addr2line::Context<EndianSlice<'a, gimli::LittleEndian>>;

/// A context which contains dwarf debug information to translate program
/// counters back to filenames and line numbers.
pub struct SymbolizeContext<'a> {
    inner: Addr2LineContext<'a>,
    code_section_offset: u64,
}

impl<'a> SymbolizeContext<'a> {
    /// Returns access to the [`addr2line::Context`] which can be used to query
    /// frame information with.
    pub fn addr2line(&self) -> &Addr2LineContext<'a> {
        &self.inner
    }

    /// Returns the offset of the code section in the original wasm file, used
    /// to calculate lookup values into the DWARF.
    pub fn code_section_offset(&self) -> u64 {
        self.code_section_offset
    }
}
