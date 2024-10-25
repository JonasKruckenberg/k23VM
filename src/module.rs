use crate::compile::{CompileJobs, CompiledModuleInfo, Compiler, ObjectBuilder};
use crate::guest_memory::{CodeMemory, MmapVec};
use crate::indices::EntityIndex;
use crate::store::Store;
use crate::translate::{Import, ModuleTranslator, TranslatedModule, Translation};
use crate::vmcontext::VMContextPlan;
use crate::HOST_PAGE_SIZE;
use alloc::sync::Arc;
use spin::Mutex;
use tracing::log;
use wasmparser::Validator;

#[derive(Debug, Clone)]
pub struct Module<'wasm>(pub(crate) Arc<ModuleInner<'wasm>>);

#[derive(Debug)]
pub(crate) struct ModuleInner<'wasm> {
    pub info: Arc<CompiledModuleInfo<'wasm>>,
    pub code: Arc<Mutex<CodeMemory>>,
    pub vmctx_plan: VMContextPlan,
}

impl<'wasm> Module<'wasm> {
    pub fn from_binary(
        validator: &mut Validator,
        compiler: &Compiler,
        store: &mut Store,
        bytes: &'wasm [u8],
    ) -> crate::TranslationResult<Self> {
        tracing::trace!("Translating module to Cranelift IR...");
        let translation = ModuleTranslator::new(validator).translate(bytes)?;

        tracing::debug!("{translation:?}");

        tracing::trace!("Compiling functions to machine code...");
        let Translation {
            module,
            debug_info,
            required_features,
            func_compile_inputs,
        } = translation;
        let unlinked_compile_outputs = compiler
            .compile_inputs(CompileJobs::from_module(&module, func_compile_inputs))
            .unwrap();
        tracing::debug!("{unlinked_compile_outputs:?}");

        tracing::trace!("Setting up intermediate code object...");
        let mut obj_builder = ObjectBuilder::new(compiler.create_intermediate_code_object());

        tracing::trace!("Appending info to intermediate code object...");
        obj_builder.append_debug_info(&debug_info);

        tracing::trace!("Allocating new output buffer for compiled module...");
        // TODO ca we get a size hint for this somehow??
        let mut code_buffer = MmapVec::new();

        tracing::trace!("Appending compiled functions to intermediate code object...");
        let info = unlinked_compile_outputs.link_append_and_finish(
            compiler,
            module,
            obj_builder,
            &mut code_buffer,
        );

        let mut code = CodeMemory::new(code_buffer);
        code.publish();

        Ok(Self(Arc::new(ModuleInner {
            vmctx_plan: VMContextPlan::for_module(compiler.target_isa(), &info.module),
            info: Arc::new(info),
            code: Arc::new(Mutex::new(code)),
        })))
    }

    pub(crate) fn module(&self) -> &TranslatedModule<'wasm> {
        &self.0.info.module
    }
    pub(crate) fn vmctx_plan(&self) -> &VMContextPlan {
        &self.0.vmctx_plan
    }

    pub fn imports(&self) -> impl ExactSizeIterator<Item = &Import<'wasm>> {
        self.module().imports.iter()
    }
    pub fn exports(&self) -> impl ExactSizeIterator<Item = (&'wasm str, EntityIndex)> + '_ {
        self.module()
            .exports
            .iter()
            .map(|(name, index)| (*name, *index))
    }

    pub fn get_export(&self, name: &str) -> Option<EntityIndex> {
        self.module().exports.get(name).copied()
    }
}
