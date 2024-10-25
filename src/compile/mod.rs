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
use cranelift_entity::PrimaryMap;
use object::write::WritableBuffer;

pub use compiled_function::{
    CompiledFunction, CompiledFunctionMetadata, Relocation, RelocationTarget, TrapInfo,
};
pub use compiler::Compiler;
pub use obj_builder::{
    FunctionLoc, ObjectBuilder, ELFOSABI_K23, ELF_K23_ENGINE, ELF_K23_INFO, ELF_K23_TRAPS,
    ELF_TEXT, ELF_WASM_DATA, ELF_WASM_DWARF, ELF_WASM_NAMES,
};

#[derive(Debug)]
pub struct CompiledModuleInfo<'wasm> {
    pub module: TranslatedModule<'wasm>,
    pub funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
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
                        compiler.compile_host_to_wasm_trampoline(&module, def_func_index)?;

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
    indices: BTreeMap<CompileKey, usize>,
    outputs: BTreeMap<u32, BTreeMap<CompileKey, CompileOutput>>,
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
        let flattened: Vec<_> = self
            .outputs
            .values()
            .flat_map(|inner| inner.values())
            .collect();

        let text_builder = compiler.target_isa().text_section_builder(flattened.len());

        let mut text_builder = obj_builder.text_builder(text_builder);

        let symbol_ids_and_locs =
            text_builder.push_funcs(flattened.into_iter(), |callee| match callee {
                RelocationTarget::Wasm(callee_index) => {
                    let def_func_index = module.defined_func_index(callee_index).unwrap();
                    self.indices[&CompileKey::wasm_function(def_func_index)]
                }
            });

        text_builder.finish(compiler.target_isa().function_alignment().preferred as u64);

        let wasm_functions = self
            .outputs
            .remove(&CompileKey::WASM_FUNCTION_KIND)
            .unwrap_or_default()
            .into_iter();

        let funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> = wasm_functions
            .map(|(key, _)| {
                let wasm_func_index = self.indices[&key];
                let (_, wasm_func_loc) = symbol_ids_and_locs[wasm_func_index];

                let host_to_wasm_trampoline = self
                    .indices
                    .get(&CompileKey {
                        namespace: CompileKey::HOST_TO_WASM_TRAMPOLINE_KIND,
                        index: key.index,
                    })
                    .map(|wasm_func_index| {
                        let (_, trampoline_func_loc) = symbol_ids_and_locs[*wasm_func_index];
                        trampoline_func_loc
                    });

                CompiledFunctionInfo {
                    wasm_func_loc,
                    host_to_wasm_trampoline,
                }
            })
            .collect();

        // TODO If configured attempt to use static memory initialization which
        // can either at runtime be implemented as a single memcpy to
        // initialize memory or otherwise enabling virtual-memory-tricks
        // such as mmap'ing from a file to get copy-on-write.

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

    // pub fn wasm_to_builtin_trampoline(index: BuiltinFunctionIndex) -> Self {
    //     Self {
    //         namespace: Self::WASM_TO_BUILTIN_TRAMPOLINE_KIND,
    //         index: index.as_u32(),
    //     }
    // }

    fn host_to_wasm_trampoline(index: DefinedFuncIndex) -> Self {
        let module = 0; // TODO change this when we support multiple modules per compilation (components?)
        Self {
            namespace: Self::HOST_TO_WASM_TRAMPOLINE_KIND | module,
            index: index.as_u32(),
        }
    }

    // fn wasm_to_native_trampoline(index: TypeIndex) -> Self {
    //     Self {
    //         namespace: Self::WASM_TO_NATIVE_TRAMPOLINE_KIND,
    //         index: index.as_u32(),
    //     }
    // }
}
