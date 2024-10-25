use crate::compile::compiled_function::CompiledFunction;
use crate::compile::obj_builder::ELFOSABI_K23;
use crate::compile::{CompileJobs, CompileKey, CompileOutput, UnlinkedCompileOutputs};
use crate::errors::CompileError;
use crate::indices::DefinedFuncIndex;
use crate::translate::{
    FuncCompileInput, FuncTranslator, TranslatedModule, TranslationEnvironment,
};
use crate::trap::DEBUG_ASSERT_TRAP_CODE;
use crate::utils::{array_call_signature, value_type, wasm_call_signature};
use crate::vmcontext::{VMContextPlan, VMCONTEXT_MAGIC};
use crate::NS_WASM_FUNC;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::mem;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::{
    Endianness, GlobalValueData, InstBuilder, MemFlags, UserExternalName, UserFuncName, Value,
};
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::{ir, Context};
use cranelift_frontend::FunctionBuilder;
use object::write::Object;
use object::{BinaryFormat, FileFlags};
use spin::Mutex;
use target_lexicon::Architecture;
use wasmparser::{FuncValidatorAllocations, ValType};

pub struct Compiler {
    isa: OwnedTargetIsa,
    contexts: Mutex<Vec<CompilationContext>>,
}

impl Compiler {
    pub fn new(isa: OwnedTargetIsa) -> Self {
        Self {
            isa,
            contexts: Mutex::new(Vec::new()),
        }
    }

    pub fn target_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
    }

    /// Creates an elf file for the target architecture to hold the final
    /// compiled code.
    pub fn create_intermediate_code_object(&self) -> Object {
        let architecture = match self.isa.triple().architecture {
            Architecture::X86_32(_) => object::Architecture::I386,
            Architecture::X86_64 => object::Architecture::X86_64,
            Architecture::Arm(_) => object::Architecture::Arm,
            Architecture::Aarch64(_) => object::Architecture::Aarch64,
            Architecture::S390x => object::Architecture::S390x,
            Architecture::Riscv64(_) => object::Architecture::Riscv64,
            _ => panic!("unsupported"),
        };

        let endianness = match self.isa.endianness() {
            Endianness::Little => object::Endianness::Little,
            Endianness::Big => object::Endianness::Big,
        };

        let mut obj = Object::new(BinaryFormat::Elf, architecture, endianness);
        obj.flags = FileFlags::Elf {
            os_abi: ELFOSABI_K23,
            e_flags: 0,
            abi_version: 0,
        };

        obj
    }

    /// Feed the collected inputs through the compiler, producing [`UnlinkedCompileOutputs`] which holds
    /// the resulting artifacts.
    pub fn compile_inputs(
        &self,
        inputs: CompileJobs,
    ) -> Result<UnlinkedCompileOutputs, CompileError> {
        let mut indices = BTreeMap::new();
        let mut outputs: BTreeMap<u32, BTreeMap<CompileKey, CompileOutput>> = BTreeMap::new();

        for (idx, f) in inputs.0.into_iter().enumerate() {
            let output = f(self)?;
            indices.insert(output.key, idx);

            outputs
                .entry(output.key.kind())
                .or_default()
                .insert(output.key, output);
        }

        let mut unlinked_compile_outputs = UnlinkedCompileOutputs { indices, outputs };
        let flattened: Vec<_> = unlinked_compile_outputs
            .outputs
            .values()
            .flat_map(|inner| inner.values())
            .collect();

        let mut builtins = BTreeMap::new();

        // compile_required_builtins(engine, module, flattened.into_iter(), &mut builtins)?;

        unlinked_compile_outputs
            .outputs
            .insert(CompileKey::WASM_TO_BUILTIN_TRAMPOLINE_KIND, builtins);

        Ok(unlinked_compile_outputs)
    }

    /// Compiles the function `index` within `translation`.
    pub fn compile_function(
        &self,
        module: &TranslatedModule,
        def_func_index: DefinedFuncIndex,
        input: FuncCompileInput,
    ) -> Result<CompiledFunction, CompileError> {
        let isa = self.target_isa();

        let mut compiler = self.function_compiler();
        let context = &mut compiler.ctx.codegen_context;

        let mut env = TranslationEnvironment::new(isa, module);

        // Setup function signature
        let func_index = module.func_index(def_func_index);
        let sig_index = module.functions[func_index].signature;
        let func_ty = &module.types[sig_index];

        context.func.signature = wasm_call_signature(self.target_isa(), func_ty);
        context.func.name = UserFuncName::User(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: func_index.as_u32(),
        });

        // setup stack limit
        let vmctx = context.func.create_global_value(GlobalValueData::VMContext);

        let stack_limit = context.func.create_global_value(GlobalValueData::Load {
            base: vmctx,
            offset: i32::try_from(env.vmctx_plan.vmctx_stack_limit())
                .unwrap()
                .into(),
            global_type: isa.pointer_type(),
            flags: MemFlags::trusted(),
        });
        context.func.stack_limit = Some(stack_limit);

        // collect debug info
        context.func.collect_debug_info();

        let FuncCompileInput { validator, body } = input;
        let mut validator =
            validator.into_validator(mem::take(&mut compiler.ctx.validator_allocations));
        compiler.ctx.func_translator.translate_body(
            &mut validator,
            body.clone(),
            &mut context.func,
            &mut env,
        )?;

        compiler.finish()
    }

    /// Compiles a trampoline for calling a WASM function from the host
    pub fn compile_host_to_wasm_trampoline(
        &self,
        module: &TranslatedModule,
        def_func_index: DefinedFuncIndex,
    ) -> Result<CompiledFunction, CompileError> {
        // This function has a special calling convention where all arguments and return values
        // are passed through an array in memory (so we can have dynamic function signatures in rust)

        let func_index = module.func_index(def_func_index);
        let sig_index = module.functions[func_index].signature;
        let func_ty = &module.types[sig_index];

        let wasm_call_sig = wasm_call_signature(self.target_isa(), func_ty);
        let array_call_sig = array_call_signature(self.target_isa());

        let mut compiler = self.function_compiler();
        let func = ir::Function::with_name_signature(Default::default(), array_call_sig);
        let (mut builder, block0) = compiler.builder(func);

        let (vmctx, caller_vmctx, values_vec_ptr, values_vec_len) = {
            let params = builder.func.dfg.block_params(block0);
            (params[0], params[1], params[2], params[3])
        };

        // First load the actual arguments out of the array.
        let mut args = load_values_from_array(
            self.target_isa(),
            func_ty.params(),
            &mut builder,
            values_vec_ptr,
            values_vec_len,
        );
        args.insert(0, caller_vmctx);
        args.insert(0, vmctx);

        // Assert that we were really given a core Wasm vmctx, since that's
        // what we are assuming with our offsets below.
        debug_assert_vmctx_kind(self.target_isa(), &mut builder, vmctx, VMCONTEXT_MAGIC);

        let offsets = VMContextPlan::for_module(self.target_isa(), module);
        // Then store our current stack pointer into the appropriate slot.
        let sp = builder.ins().get_stack_pointer(self.isa.pointer_type());
        builder.ins().store(
            MemFlags::trusted(),
            sp,
            vmctx,
            Offset32::new(offsets.vmctx_last_wasm_entry_sp() as i32),
        );

        // Then call the Wasm function with those arguments.
        let call = declare_and_call(&mut builder, wasm_call_sig, func_index.as_u32(), &args);
        let results = builder.func.dfg.inst_results(call).to_vec();

        store_values_to_array(
            self.target_isa(),
            &mut builder,
            func_ty.results(),
            &results,
            values_vec_ptr,
            values_vec_len,
        );

        builder.ins().return_(&[]);
        builder.finalize();

        Ok(compiler.finish()?)
    }

    fn function_compiler(&self) -> FunctionCompiler<'_> {
        let saved_context = self.contexts.lock().pop();
        FunctionCompiler {
            compiler: self,
            ctx: saved_context
                .map(|mut ctx| {
                    ctx.codegen_context.clear();
                    ctx
                })
                .unwrap_or_default(),
        }
    }
}

struct FunctionCompiler<'a> {
    compiler: &'a Compiler,
    ctx: CompilationContext,
}

impl<'a> FunctionCompiler<'a> {
    fn builder(&mut self, func: ir::Function) -> (FunctionBuilder<'_>, ir::Block) {
        self.ctx.codegen_context.func = func;
        let mut builder = FunctionBuilder::new(
            &mut self.ctx.codegen_context.func,
            self.ctx.func_translator.context(),
        );

        let block0 = builder.create_block();
        builder.append_block_params_for_function_params(block0);
        builder.switch_to_block(block0);
        builder.seal_block(block0);
        (builder, block0)
    }

    fn finish(mut self) -> Result<CompiledFunction, CompileError> {
        let context = &mut self.ctx.codegen_context;
        let isa = &*self.compiler.isa;

        let compiled_code =
            context.compile(self.compiler.target_isa(), &mut ControlPlane::default())?;

        let preferred_alignment = self.compiler.isa.function_alignment().preferred;
        let alignment = compiled_code.buffer.alignment.max(preferred_alignment);
        let mut compiled_function = CompiledFunction::new(
            compiled_code.buffer.clone(),
            context.func.params.user_named_funcs().clone(),
            alignment,
        );

        compiled_function.metadata.sized_stack_slots =
            mem::take(&mut context.func.sized_stack_slots);

        self.compiler.contexts.lock().push(self.ctx);

        Ok(compiled_function)
    }
}

struct CompilationContext {
    func_translator: FuncTranslator,
    codegen_context: Context,
    validator_allocations: FuncValidatorAllocations,
}

impl Default for CompilationContext {
    fn default() -> Self {
        let mut codegen_context = Context::new();
        codegen_context.set_disasm(true);
        Self {
            func_translator: FuncTranslator::new(),
            codegen_context,
            validator_allocations: Default::default(),
        }
    }
}

/// Helper function for declaring a cranelift function
/// and immediately inserting a call instruction.
fn declare_and_call(
    builder: &mut FunctionBuilder,
    signature: ir::Signature,
    func_index: u32,
    args: &[Value],
) -> ir::Inst {
    let name = ir::ExternalName::User(builder.func.declare_imported_user_function(
        UserExternalName {
            namespace: NS_WASM_FUNC,
            index: func_index,
        },
    ));
    let signature = builder.func.import_signature(signature);
    let callee = builder.func.dfg.ext_funcs.push(ir::ExtFuncData {
        name,
        signature,
        colocated: true,
    });
    builder.ins().call(callee, &args)
}

/// Used for loading the values of an array-call host function's value
/// array.
///
/// This can be used to load arguments out of the array if the trampoline we
/// are building exposes the array calling convention, or it can be used to
/// load results out of the array if the trampoline we are building calls a
/// function that uses the array calling convention.
fn load_values_from_array(
    isa: &dyn TargetIsa,
    types: &[ValType],
    builder: &mut FunctionBuilder,
    values_vec_ptr: Value,
    values_vec_capacity: Value,
) -> Vec<Value> {
    let value_size = size_of::<u128>();

    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity);

    // Note that this is little-endian like `store_values_to_array` above,
    // see notes there for more information.
    let flags = MemFlags::new()
        .with_notrap()
        .with_endianness(ir::Endianness::Little);

    let mut results = Vec::new();
    for (i, ty) in types.iter().enumerate() {
        let ir_ty = value_type(*ty);
        let val = builder.ins().load(
            ir_ty,
            flags,
            values_vec_ptr,
            i32::try_from(i * value_size).unwrap(),
        );
        results.push(val);
    }
    results
}

/// Store values to an array in the array calling convention.
///
/// Used either to store arguments to the array when calling a function
/// using the array calling convention, or used to store results to the
/// array when implementing a function that exposes the array calling
/// convention.
fn store_values_to_array(
    isa: &dyn TargetIsa,
    builder: &mut FunctionBuilder,
    types: &[ValType],
    values: &[Value],
    values_vec_ptr: Value,
    values_vec_capacity: Value,
) {
    debug_assert_eq!(types.len(), values.len());
    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity);

    // Note that loads and stores are unconditionally done in the
    // little-endian format rather than the host's native-endianness,
    // despite this load/store being unrelated to execution in wasm itself.
    // For more details on this see the `ValRaw` type in
    // `wasmtime::runtime::vm`.
    let flags = MemFlags::new()
        .with_notrap()
        .with_endianness(Endianness::Little);

    let value_size = size_of::<u128>();
    for (i, val) in values.iter().copied().enumerate() {
        builder.ins().store(
            flags,
            val,
            values_vec_ptr,
            i32::try_from(i * value_size).unwrap(),
        );
    }
}

fn debug_assert_enough_capacity_for_length(
    builder: &mut FunctionBuilder,
    length: usize,
    capacity: Value,
) {
    if cfg!(debug_assertions) {
        let enough_capacity = builder.ins().icmp_imm(
            ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
            capacity,
            ir::immediates::Imm64::new(length.try_into().unwrap()),
        );
        builder
            .ins()
            .trapz(enough_capacity, ir::TrapCode::User(DEBUG_ASSERT_TRAP_CODE));
    }
}

fn debug_assert_vmctx_kind(
    isa: &dyn TargetIsa,
    builder: &mut FunctionBuilder,
    vmctx: Value,
    expected_vmctx_magic: u32,
) {
    if cfg!(debug_assertions) {
        let magic = builder.ins().load(
            ir::types::I32,
            MemFlags::trusted().with_endianness(isa.endianness()),
            vmctx,
            0,
        );
        let is_expected_vmctx = builder.ins().icmp_imm(
            ir::condcodes::IntCC::Equal,
            magic,
            i64::from(expected_vmctx_magic),
        );
        builder.ins().trapz(
            is_expected_vmctx,
            ir::TrapCode::User(DEBUG_ASSERT_TRAP_CODE),
        );
    }
}
