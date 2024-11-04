#![allow(unused)]

use alloc::boxed::Box;
use cranelift_codegen::TextSectionBuilder;
use object::write::Object;
use crate::builtins::BuiltinFunctionIndex;
use crate::compile::{CompiledFunction, Compiler};
use crate::indices::DefinedFuncIndex;
use crate::translate::{ModuleTypes, Translation, WasmFuncType};
use wasmparser::{FuncToValidate, FunctionBody, ValidatorResources};

#[derive(Default)]
pub struct BaselineCompiler {}

impl Compiler for BaselineCompiler {
    fn compile_function(
        &self,
        translation: &Translation,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
        body: FunctionBody<'_>,
        validator: FuncToValidate<ValidatorResources>,
    ) -> crate::Result<CompiledFunction> {
        // let mut validator = validator.into_validator(Default::default()); // TODO reuse allocation

        todo!()
    }

    fn compile_array_to_wasm_trampoline(
        &self,
        translation: &Translation,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
    ) -> crate::Result<CompiledFunction> {
        todo!()
    }

    fn compile_wasm_to_array_trampoline(
        &self,
        wasm_func_ty: &WasmFuncType,
    ) -> crate::Result<CompiledFunction> {
        todo!()
    }

    fn compile_wasm_to_builtin(
        &self,
        index: BuiltinFunctionIndex,
    ) -> crate::Result<CompiledFunction> {
        todo!()
    }

    fn text_section_builder(&self, capacity: usize) -> Box<dyn TextSectionBuilder> {
        todo!()
    }

    fn create_intermediate_code_object(&self) -> Object {
        todo!()
    }
}
