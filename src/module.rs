use crate::compile_cranelift::{CompiledModule, Compiler, ObjectBuilder};
use crate::indices::{EntityIndex, FuncIndex};
use crate::parse::{Import, ModuleParser, ParsedModule};
use crate::vm::{CodeMemory, MmapVec, VMContextPlan};
use alloc::sync::Arc;
use gimli::EndianSlice;
use wasmparser::Validator;

#[derive(Debug, Clone)]
pub struct Module(Arc<ModuleInner>);

impl Module {
    pub fn from_wat(validator: &mut Validator, compiler: &Compiler, str: &str) -> crate::Result<Self> {
        Self::from_bytes(validator, compiler, &wat::parse_str(str)?)
    }

    pub fn from_bytes(
        validator: &mut Validator,
        compiler: &Compiler,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        tracing::trace!("Parsing WASM module...");

        let res = ModuleParser::new(validator).parse(bytes)?;
        // TODO assert compatability with target features

        let mut obj_builder = ObjectBuilder::new(compiler.create_intermediate_code_object());
        let module = obj_builder.append(compiler, res)?;

        tracing::trace!("Allocating new output buffer for compiled module...");
        let mut code_buffer = MmapVec::new();

        obj_builder.finish(&mut code_buffer).unwrap();

        let mut code = CodeMemory::new(code_buffer);
        code.publish()?;
        let code = Arc::new(code);

        crate::placeholder::register_code(&code);

        Ok(Self(Arc::new(ModuleInner {
            name: None,
            vmctx_plan: VMContextPlan::for_module(compiler.target_isa(), &module.module),
            module: Arc::new(module),
            code,
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
            .map(|(name, index)| (name.as_str(), *index))
    }

    pub fn name(&self) -> Option<&str> {
        self.0.name.as_ref().map(|s| s.as_str())
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
    pub(crate) fn get_export(&self, name: &str) -> Option<EntityIndex> {
        self.0.module.module.exports.get(name).copied()
    }
    pub(crate) fn parsed(&self) -> &ParsedModule {
        &self.0.module.module
    }
    pub(crate) fn compiled(&self) -> &CompiledModule {
        &self.0.module
    }
    pub(crate) fn code(&self) -> &CodeMemory {
        &self.0.code
    }
    pub(crate) fn vmctx_plan(&self) -> &VMContextPlan {
        &self.0.vmctx_plan
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
    name: Option<String>,
    module: Arc<CompiledModule>,
    code: Arc<CodeMemory>,
    vmctx_plan: VMContextPlan,
}

type Addr2LineContext<'a> = addr2line::Context<gimli::EndianSlice<'a, gimli::LittleEndian>>;

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
