use crate::compile::{CompileInputs, CompiledFunctionInfo};
use crate::indices::{DefinedFuncIndex, EntityIndex, VMSharedTypeIndex};
use crate::runtime::CodeMemory;
use crate::runtime::{MmapVec, VMOffsets};
use crate::translate::{Import, TranslatedModule};
use crate::type_registry::RuntimeTypeCollection;
use crate::{Engine, ModuleTranslator};
use alloc::sync::Arc;
use core::mem;
use cranelift_entity::PrimaryMap;
use wasmparser::Validator;

#[derive(Debug, Clone)]
pub struct Module(Arc<ModuleInner>);

#[derive(Debug)]
struct ModuleInner {
    translated: TranslatedModule,
    offsets: VMOffsets,
    code: Arc<CodeMemory>,
    type_collection: RuntimeTypeCollection,
    function_info: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl Module {
    pub fn from_str(
        engine: &Engine,
        validator: &mut Validator,
        str: &str,
    ) -> crate::Result<Self> {
        let bytes = wat::parse_str(str)?;
        Self::from_bytes(engine, validator, &bytes)
    }

    pub fn from_bytes(
        engine: &Engine,
        validator: &mut Validator,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        let (mut translation, types) = ModuleTranslator::new(validator).translate(bytes)?;

        let function_body_data = mem::take(&mut translation.function_bodies);
        let inputs = CompileInputs::from_module(&translation, &types, function_body_data);

        let unlinked_outputs = inputs.compile(engine.compiler())?;
        let (code, function_info, (trap_offsets, traps)) =
            unlinked_outputs.link_and_finish(engine, &translation.module)?;

        let type_collection = engine.type_registry().register_module_types(engine, types);

        tracing::trace!("Allocating new memory map for compiled module...");
        let vec = MmapVec::from_slice(&code)?;
        let mut code = CodeMemory::new(vec, trap_offsets, traps);
        code.publish()?;
        let code = Arc::new(code);
        
        crate::placeholder::code_registry::register_code(&code);

        Ok(Self(Arc::new(ModuleInner {
            offsets: VMOffsets::for_module(
                engine.compiler().triple().pointer_width().unwrap().bytes(),
                &translation.module,
            ),
            translated: translation.module,
            function_info,
            code,
            type_collection,
        })))
    }

    pub fn imports(&self) -> impl ExactSizeIterator<Item = &Import> {
        self.0.translated.imports.iter()
    }

    pub fn exports(&self) -> impl ExactSizeIterator<Item = (&str, EntityIndex)> + '_ {
        self.0
            .translated
            .exports
            .iter()
            .map(|(name, index)| (name.as_str(), *index))
    }

    pub fn name(&self) -> Option<&str> {
        self.0.translated.name.as_deref()
    }

    pub(crate) fn get_export(&self, name: &str) -> Option<EntityIndex> {
        self.0.translated.exports.get(name).copied()
    }

    pub(crate) fn translated(&self) -> &TranslatedModule {
        &self.0.translated
    }
    pub(crate) fn offsets(&self) -> &VMOffsets {
        &self.0.offsets
    }
    pub(crate) fn code(&self) -> &CodeMemory {
        &self.0.code
    }
    pub(crate) fn type_collection(&self) -> &RuntimeTypeCollection {
        &self.0.type_collection
    }
    pub(crate) fn type_ids(&self) -> &[VMSharedTypeIndex] {
        self.0.type_collection.type_map().values().as_slice()
    }
    pub(crate) fn function_info(&self) -> &PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> {
        &self.0.function_info
    }
}
