mod address_map;
mod compiled_function;
mod key;
mod obj_builder;
mod trap;

use crate::builtins::BuiltinFunctionIndex;
use crate::indices::{DefinedFuncIndex, FuncIndex};
use crate::translate::{
    FunctionBodyData, ModuleTypes, TranslatedModule, Translation, WasmFuncType,
};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use cranelift_entity::{EntitySet, PrimaryMap};
use key::CompileKey;
use object::write::Object;
use wasmparser::{FuncToValidate, FunctionBody, ValidatorResources};

pub use address_map::InstructionAddressMapping;
pub use compiled_function::{CompiledFunction, RelocationTarget};
pub use obj_builder::{ObjectBuilder, ELFOSABI_K23, ELF_K23_ADDRESS_MAP, ELF_K23_TRAPS};
pub use trap::parse_trap_section;

pub trait Compiler: Send + Sync {
    /// Compile the translated WASM function `index` within `translation`.
    fn compile_function(
        &self,
        translation: &Translation,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
        body: FunctionBody<'_>,
        validator: FuncToValidate<ValidatorResources>,
    ) -> crate::Result<CompiledFunction>;

    /// Compile a trampoline for calling the `index` WASM function through the
    /// array-calling convention used by host code to call into WASM.
    fn compile_array_to_wasm_trampoline(
        &self,
        translation: &Translation,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
    ) -> crate::Result<CompiledFunction>;

    /// Compile a trampoline for calling the  a(host-defined) function through the array
    /// calling convention used to by WASM to call into host code.
    fn compile_wasm_to_array_trampoline(
        &self,
        wasm_func_ty: &WasmFuncType,
    ) -> crate::Result<CompiledFunction>;

    /// Compile a trampoline for calling the `index` builtin function from WASM.
    fn compile_wasm_to_builtin(
        &self,
        index: BuiltinFunctionIndex,
    ) -> crate::Result<CompiledFunction>;

    fn text_section_builder(
        &self,
        capacity: usize,
    ) -> Box<dyn cranelift_codegen::TextSectionBuilder>;

    fn create_intermediate_code_object(&self) -> Object;
}

#[derive(Debug)]
pub struct CompiledModule {
    pub module: TranslatedModule,
    pub funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
    pub func_names: Vec<FunctionName>,
    pub dwarf: Vec<(u8, core::ops::Range<u64>)>,
    pub code_section_offset: u64,
}

impl CompiledModule {
    pub fn text_offset_to_func(&self, text_offset: usize) -> Option<(DefinedFuncIndex, u32)> {
        let text_offset = u32::try_from(text_offset).unwrap();

        let index = match self.funcs.binary_search_values_by_key(&text_offset, |e| {
            debug_assert!(e.wasm_func_loc.length > 0);
            // Return the inclusive "end" of the function
            e.wasm_func_loc.start + e.wasm_func_loc.length - 1
        }) {
            Ok(k) => {
                // Exact match, pc is at the end of this function
                k
            }
            Err(k) => {
                // Not an exact match, k is where `pc` would be "inserted"
                // Since we key based on the end, function `k` might contain `pc`,
                // so we'll validate on the range check below
                k
            }
        };

        let CompiledFunctionInfo { wasm_func_loc, .. } = self.funcs.get(index)?;
        let start = wasm_func_loc.start;
        let end = wasm_func_loc.start + wasm_func_loc.length;

        if text_offset < start || end < text_offset {
            return None;
        }

        Some((index, text_offset - wasm_func_loc.start))
    }

    pub(crate) fn wasm_func_info(&self, index: DefinedFuncIndex) -> &CompiledFunctionInfo {
        &self
            .funcs
            .get(index)
            .expect("defined function should be present")
    }
}

#[derive(Debug)]
pub struct CompiledFunctionInfo {
    /// The [`FunctionLoc`] indicating the location of this function in the text
    /// section of the compilation artifact.
    pub wasm_func_loc: FunctionLoc,
    /// A trampoline for host callers (e.g. `Func::wrap`) calling into this function (if needed).
    pub host_to_wasm_trampoline: Option<FunctionLoc>,
    pub start_srcloc: FilePos,
}

/// Description of where a function is located in the text section of a
/// compiled image.
#[derive(Debug, Copy, Clone)]
pub struct FunctionLoc {
    /// The byte offset from the start of the text section where this
    /// function starts.
    pub start: u32,
    /// The byte length of this function's function body.
    pub length: u32,
}

#[derive(Debug)]
pub struct FunctionName {
    pub idx: FuncIndex,
    pub offset: u32,
    pub len: u32,
}

pub type CompileInput<'a> =
    Box<dyn FnOnce(&dyn Compiler) -> crate::Result<CompileOutput> + Send + 'a>;

pub struct CompileInputs<'a>(Vec<CompileInput<'a>>);

impl<'a> CompileInputs<'a> {
    pub fn from_translation(
        translation: &'a Translation,
        types: &'a ModuleTypes,
        function_body_data: PrimaryMap<DefinedFuncIndex, FunctionBodyData<'a>>,
    ) -> Self {
        let mut inputs: Vec<CompileInput> = Vec::new();

        for (def_func_index, function_body_data) in function_body_data {
            // push the "main" function compilation job
            inputs.push(Box::new(move |compiler| {
                let symbol = format!("wasm[0]::function[{}]", def_func_index.as_u32());
                tracing::debug!("compiling {symbol}...");

                let function = compiler.compile_function(
                    translation,
                    types,
                    def_func_index,
                    function_body_data.body,
                    function_body_data.validator,
                )?;

                Ok(CompileOutput {
                    key: CompileKey::wasm_function(def_func_index),
                    function,
                    symbol,
                })
            }));

            // Compile a host->wasm trampoline for every function that are flags as "escaping"
            // and could therefore theoretically be called by native code.
            let func_index = translation.module.func_index(def_func_index);
            if translation.module.functions[func_index].is_escaping() {
                inputs.push(Box::new(move |compiler| {
                    let symbol =
                        format!("wasm[0]::array_to_wasm_trampoline[{}]", func_index.as_u32());
                    tracing::debug!("compiling {symbol}...");

                    let function = compiler.compile_array_to_wasm_trampoline(
                        translation,
                        types,
                        def_func_index,
                    )?;

                    Ok(CompileOutput {
                        key: CompileKey::array_to_wasm_trampoline(def_func_index),
                        function,
                        symbol,
                    })
                }));
            }
        }

        // TODO collect wasm->native trampolines

        Self(inputs)
    }

    pub fn compile(self, compiler: &dyn Compiler) -> crate::Result<UnlinkedCompileOutputs> {
        let mut outputs = self
            .0
            .into_iter()
            .map(|f| f(compiler))
            .collect::<Result<Vec<_>, _>>()?;

        compile_required_builtin_trampolines(compiler, &mut outputs)?;

        let mut indices: BTreeMap<u32, BTreeMap<CompileKey, usize>> = BTreeMap::new();
        for (index, output) in outputs.iter().enumerate() {
            indices
                .entry(output.key.kind())
                .or_default()
                .insert(output.key, index);
        }

        Ok(UnlinkedCompileOutputs { indices, outputs })
    }
}

fn compile_required_builtin_trampolines(
    compiler: &dyn Compiler,
    outputs: &mut Vec<CompileOutput>,
) -> crate::Result<()> {
    let mut builtins = EntitySet::new();
    let mut new_jobs: Vec<CompileInput<'_>> = Vec::new();

    let builtin_indicies = outputs
        .iter()
        .flat_map(|output| output.function.relocations())
        .filter_map(|reloc| match reloc.target {
            RelocationTarget::Wasm(_) => None,
            RelocationTarget::Builtin(index) => Some(index),
        });

    let compile_builtin = |builtin: BuiltinFunctionIndex| -> CompileInput {
        Box::new(move |compiler: &dyn Compiler| {
            let symbol = format!("wasm_builtin_{}", builtin.name());
            tracing::debug!("compiling {symbol}...");
            Ok(CompileOutput {
                key: CompileKey::wasm_to_builtin_trampoline(builtin),
                symbol,
                function: compiler.compile_wasm_to_builtin(builtin)?,
            })
        })
    };

    for index in builtin_indicies {
        if builtins.insert(index) {
            new_jobs.push(compile_builtin(index));
        }
    }

    outputs.extend(
        new_jobs
            .into_iter()
            .map(|f| f(compiler))
            .collect::<Result<Vec<_>, _>>()?,
    );

    Ok(())
}

#[derive(Debug)]
pub struct CompileOutput {
    pub key: CompileKey,
    pub function: CompiledFunction,
    pub symbol: String,
}

#[derive(Debug)]
pub struct UnlinkedCompileOutputs {
    indices: BTreeMap<u32, BTreeMap<CompileKey, usize>>,
    outputs: Vec<CompileOutput>,
}

impl UnlinkedCompileOutputs {
    /// Append the compiled functions to the given object resolving any relocations in the process.
    ///
    /// This is the final step if compilation.
    pub fn link_and_append(
        mut self,
        obj_builder: &mut ObjectBuilder,
        compiler: &dyn Compiler,
        module: &TranslatedModule,
    ) -> crate::Result<PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>> {
        let text_builder = compiler.text_section_builder(self.outputs.len());
        let mut text_builder = obj_builder.text_builder(text_builder);

        let symbol_ids_and_locs =
            text_builder.push_funcs(self.outputs.iter(), |callee| match callee {
                RelocationTarget::Wasm(callee_index) => {
                    let def_func_index = module.defined_func_index(callee_index).unwrap();

                    self.indices[&CompileKey::WASM_FUNCTION_KIND]
                        [&CompileKey::wasm_function(def_func_index)]
                }
                RelocationTarget::Builtin(index) => {
                    self.indices[&CompileKey::WASM_TO_BUILTIN_TRAMPOLINE_KIND]
                        [&CompileKey::wasm_to_builtin_trampoline(index)]
                }
            });

        text_builder.finish(crate::host_page_size() as u64);

        let wasm_functions = self
            .indices
            .remove(&CompileKey::WASM_FUNCTION_KIND)
            .unwrap_or_default()
            .into_iter();

        let mut host_to_wasm_trampolines = self
            .indices
            .remove(&CompileKey::ARRAY_TO_WASM_TRAMPOLINE_KIND)
            .unwrap_or_default();

        let funcs = wasm_functions
            .map(|(key, index)| {
                let host_to_wasm_trampoline_key =
                    CompileKey::array_to_wasm_trampoline(DefinedFuncIndex::from_u32(key.index));
                let host_to_wasm_trampoline = host_to_wasm_trampolines
                    .remove(&host_to_wasm_trampoline_key)
                    .map(|index| symbol_ids_and_locs[index].1);

                CompiledFunctionInfo {
                    start_srcloc: self.outputs[index].function.metadata.start_srcloc,
                    wasm_func_loc: symbol_ids_and_locs[index].1,
                    host_to_wasm_trampoline,
                }
            })
            .collect();

        // TODO If configured attempt to use static memory initialization which
        // can either at runtime be implemented as a single memcpy to
        // initialize memory or otherwise enabling virtual-memory-tricks
        // such as guest_memory'ing from a file to get copy-on-write.

        Ok(funcs)
    }
}

/// A position within an original source file,
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePos(u32);

impl Default for FilePos {
    fn default() -> Self {
        Self(u32::MAX)
    }
}

impl FilePos {
    pub(crate) fn new(pos: u32) -> Self {
        Self(pos)
    }

    pub fn file_offset(self) -> Option<u32> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(self.0)
        }
    }
}
