mod compiled_function;
mod compiler;
mod obj_builder;

use crate::errors::CompileError;
use crate::indices::DefinedFuncIndex;
use crate::translate::{FuncCompileInput, TranslatedModule};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
pub use compiled_function::{
    CompiledFunction, CompiledFunctionMetadata, Relocation, RelocationTarget, TrapInfo,
};
pub use compiler::Compiler;
use cranelift_entity::PrimaryMap;
pub use obj_builder::{
    FunctionLoc, ObjectBuilder, ELFOSABI_K23, ELF_K23_ENGINE, ELF_K23_INFO, ELF_K23_TRAPS,
    ELF_TEXT, ELF_WASM_DATA, ELF_WASM_DWARF, ELF_WASM_NAMES,
};
use object::write::WritableBuffer;

#[derive(Debug)]
pub struct CompiledModuleInfo<'wasm> {
    pub module: TranslatedModule<'wasm>,
    pub funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl CompiledModuleInfo<'_> {
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
}

#[derive(Debug)]
pub struct CompiledFunctionInfo {
    /// The [`FunctionLoc`] indicating the location of this function in the text
    /// section of the compilation artifact.
    pub wasm_func_loc: FunctionLoc,
    /// A trampoline for host callers (e.g. `Func::wrap`) calling into this function (if needed).
    pub host_to_wasm_trampoline: Option<FunctionLoc>,
}

type CompileJob<'a> = Box<dyn FnOnce(&Compiler) -> Result<CompileOutput, CompileError> + Send + 'a>;

pub struct CompileJobs<'a>(Vec<CompileJob<'a>>);

impl<'a> CompileJobs<'a> {
    /// Gather all functions that need compilation - including trampolines.
    pub fn from_module(
        module: &'a TranslatedModule,
        function_body_inputs: PrimaryMap<DefinedFuncIndex, FuncCompileInput<'a>>,
    ) -> Self {
        let mut inputs: Vec<CompileJob> = Vec::new();
        let mut num_trampolines = 0;

        for (def_func_index, body_input) in function_body_inputs {
            // push the "main" function compilation job
            inputs.push(Box::new(move |compiler| {
                let function = compiler.compile_function(module, def_func_index, body_input)?;

                Ok(CompileOutput {
                    key: CompileKey::wasm_function(def_func_index),
                    function,
                    symbol: format!("wasm[0]::function[{}]", def_func_index.as_u32()),
                })
            }));

            // Compile a host->wasm trampoline for every function that are flags as "escaping"
            // and could therefore theoretically be called by native code.
            let func_index = module.func_index(def_func_index);
            if module.functions[func_index].is_escaping() {
                num_trampolines += 1;

                inputs.push(Box::new(move |compiler| {
                    let function =
                        compiler.compile_host_to_wasm_trampoline(module, def_func_index)?;

                    Ok(CompileOutput {
                        key: CompileKey::host_to_wasm_trampoline(def_func_index),
                        function,
                        symbol: format!(
                            "wasm[0]::host_to_wasm_trampoline[{}]",
                            func_index.as_u32()
                        ),
                    })
                }));
            }
        }

        // TODO collect wasm->native trampolines

        Self(inputs)
    }
}

#[derive(Debug)]
pub struct UnlinkedCompileOutputs {
    indices: BTreeMap<u32, BTreeMap<CompileKey, usize>>,
    outputs: Vec<CompileOutput>,
}

#[derive(Debug)]
pub struct CompileOutput {
    pub key: CompileKey,
    pub function: CompiledFunction,
    pub symbol: String,
}

impl UnlinkedCompileOutputs {
    /// Append the compiled functions to the given object resolving any relocations in the process.
    ///
    /// This is the final step if compilation.
    pub fn link_append_and_finish<'wasm, T: WritableBuffer>(
        mut self,
        compiler: &Compiler,
        module: TranslatedModule<'wasm>,
        mut obj_builder: ObjectBuilder,
        output_buffer: &mut T,
    ) -> CompiledModuleInfo<'wasm> {
        let text_builder = compiler
            .target_isa()
            .text_section_builder(self.outputs.len());
        let mut text_builder = obj_builder.text_builder(text_builder);

        let symbol_ids_and_locs =
            text_builder.push_funcs(self.outputs.iter(), |callee| match callee {
                RelocationTarget::Wasm(callee_index) => {
                    let def_func_index = module.defined_func_index(callee_index).unwrap();

                    self.indices[&CompileKey::WASM_FUNCTION_KIND]
                        [&CompileKey::wasm_function(def_func_index)]
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
            .remove(&CompileKey::HOST_TO_WASM_TRAMPOLINE_KIND)
            .unwrap_or_default();

        let funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> = wasm_functions
            .map(|(key, index)| {
                let host_to_wasm_trampoline_key =
                    CompileKey::host_to_wasm_trampoline(DefinedFuncIndex::from_u32(key.index));
                let host_to_wasm_trampoline = host_to_wasm_trampolines
                    .remove(&host_to_wasm_trampoline_key)
                    .map(|index| symbol_ids_and_locs[index].1);

                CompiledFunctionInfo {
                    wasm_func_loc: symbol_ids_and_locs[index].1,
                    host_to_wasm_trampoline,
                }
            })
            .collect();

        // TODO If configured attempt to use static memory initialization which
        // can either at runtime be implemented as a single memcpy to
        // initialize memory or otherwise enabling virtual-memory-tricks
        // such as guest_memory'ing from a file to get copy-on-write.

        obj_builder.finish(output_buffer).unwrap();

        CompiledModuleInfo { module, funcs }
    }
}

/// A sortable, comparable key for a compilation output.
/// This is used to sort by compilation output kind and bucket results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompileKey {
    // The namespace field is bitpacked like:
    //
    //     [ kind:i3 module:i29 ]
    namespace: u32,
    pub index: u32,
}

impl CompileKey {
    const KIND_BITS: u32 = 3;
    const KIND_OFFSET: u32 = 32 - Self::KIND_BITS;
    const KIND_MASK: u32 = ((1 << Self::KIND_BITS) - 1) << Self::KIND_OFFSET;

    pub fn kind(self) -> u32 {
        self.namespace & Self::KIND_MASK
    }

    pub const WASM_FUNCTION_KIND: u32 = Self::new_kind(0);
    const HOST_TO_WASM_TRAMPOLINE_KIND: u32 = Self::new_kind(2);
    const WASM_TO_NATIVE_TRAMPOLINE_KIND: u32 = Self::new_kind(3);
    pub const WASM_TO_BUILTIN_TRAMPOLINE_KIND: u32 = Self::new_kind(4);

    const fn new_kind(kind: u32) -> u32 {
        assert!(kind < (1 << Self::KIND_BITS));
        kind << Self::KIND_OFFSET
    }

    pub fn wasm_function(index: DefinedFuncIndex) -> Self {
        let module = 0; // TODO change this when we support multiple modules per compilation (components?)
        Self {
            namespace: Self::WASM_FUNCTION_KIND | module,
            index: index.as_u32(),
        }
    }

    fn host_to_wasm_trampoline(index: DefinedFuncIndex) -> Self {
        let module = 0; // TODO change this when we support multiple modules per compilation (components?)
        Self {
            namespace: Self::HOST_TO_WASM_TRAMPOLINE_KIND | module,
            index: index.as_u32(),
        }
    }
}
