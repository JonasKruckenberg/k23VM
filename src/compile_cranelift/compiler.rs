use crate::builtins::BuiltinFunctionIndex;
use crate::compile_cranelift::{
    CompileJob, CompileJobs, CompileKey, CompileOutput, CompiledFunction, RelocationTarget,
    UnlinkedCompileOutputs, ELFOSABI_K23,
};
use crate::indices::DefinedFuncIndex;
use crate::parse::{CompileInput, ParsedModule};
use crate::translate_cranelift::{
    BuiltinFunctionSignatures, FuncTranslator, TranslationEnvironment,
};
use crate::traps::TRAP_INTERNAL_ASSERT;
use crate::utils::{array_call_signature, value_type, wasm_call_signature};
use crate::vm::{FixedVMContextPlan, VMCONTEXT_MAGIC};
use crate::{FilePos, NS_WASM_FUNC};
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
use cranelift_entity::EntitySet;
use cranelift_frontend::FunctionBuilder;
use object::write::Object;
use object::{BinaryFormat, FileFlags};
use spin::Mutex;
use target_lexicon::Architecture;
use wasmparser::{FuncValidatorAllocations, FunctionBody, ValType};

pub struct Compiler {
    isa: OwnedTargetIsa,
    contexts: Mutex<Vec<CompilationContext>>,
    vmctx_plan: FixedVMContextPlan,
}

impl Compiler {
    pub fn new(isa: OwnedTargetIsa) -> Self {
        Self {
            vmctx_plan: FixedVMContextPlan::new(isa.as_ref()),
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
    pub fn compile_inputs(&self, inputs: CompileJobs) -> crate::Result<UnlinkedCompileOutputs> {
        let mut outputs = self.compile_inputs_raw(inputs.0)?;

        self.compile_required_builtin_trampolines(&mut outputs)?;

        let mut indices: BTreeMap<u32, BTreeMap<CompileKey, usize>> = BTreeMap::new();
        for (index, output) in outputs.iter().enumerate() {
            indices
                .entry(output.key.kind())
                .or_default()
                .insert(output.key, index);
        }

        Ok(UnlinkedCompileOutputs { indices, outputs })
    }

    pub fn compile_inputs_raw(&self, inputs: Vec<CompileJob>) -> crate::Result<Vec<CompileOutput>> {
        inputs
            .into_iter()
            .map(|f| f(self))
            .collect::<Result<Vec<_>, _>>()
    }

    fn compile_required_builtin_trampolines(
        &self,
        outputs: &mut Vec<CompileOutput>,
    ) -> crate::Result<()> {
        let mut builtins = EntitySet::new();
        let mut new_jobs: Vec<CompileJob<'_>> = Vec::new();

        let builtin_indicies = outputs
            .iter()
            .flat_map(|output| output.function.relocations())
            .filter_map(|reloc| match reloc.target {
                RelocationTarget::Wasm(_) => None,
                RelocationTarget::Builtin(index) => Some(index),
            });

        let compile_builtin = |builtin: BuiltinFunctionIndex| -> CompileJob {
            Box::new(move |compiler: &Compiler| {
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

        outputs.extend(self.compile_inputs_raw(new_jobs)?);

        Ok(())
    }

    /// Compiles the function `index` within `translation`.
    pub fn compile_function(
        &self,
        module: &ParsedModule,
        def_func_index: DefinedFuncIndex,
        input: CompileInput,
    ) -> crate::Result<CompiledFunction> {
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
            offset: i32::try_from(env.vmctx_plan.fixed.vmctx_stack_limit())
                .unwrap()
                .into(),
            global_type: isa.pointer_type(),
            flags: MemFlags::trusted(),
        });
        context.func.stack_limit = Some(stack_limit);

        // collect debug info
        context.func.collect_debug_info();

        let CompileInput { validator, body } = input;
        let mut validator =
            validator.into_validator(mem::take(&mut compiler.ctx.validator_allocations));
        compiler.ctx.func_translator.translate_body(
            &mut validator,
            &body,
            &mut context.func,
            &mut env,
        )?;

        compiler.finish(Some(&body))
    }

    /// Compiles a trampoline for calling a WASM function from the host
    pub fn compile_host_to_wasm_trampoline(
        &self,
        module: &ParsedModule,
        def_func_index: DefinedFuncIndex,
    ) -> crate::Result<CompiledFunction> {
        // This function has a special calling convention where all arguments and return values
        // are passed through an array in memory (so we can have dynamic function signatures in rust)
        let pointer_type = self.isa.pointer_type();

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

        // TODO remove this
        let temp_mod = ParsedModule::default();
        let mut env = TranslationEnvironment::new(self.target_isa(), &temp_mod);

        // First load the actual arguments out of the array.
        let mut args = load_values_from_array(
            func_ty.params(),
            &mut builder,
            values_vec_ptr,
            values_vec_len,
            &mut env,
        );
        args.insert(0, caller_vmctx);
        args.insert(0, vmctx);

        // Assert that we were really given a core Wasm vmctx, since that's
        // what we are assuming with our offsets below.
        debug_assert_vmctx_kind(self.target_isa(), &mut builder, vmctx, VMCONTEXT_MAGIC, &mut env);
        // Then store our current stack pointer into the appropriate slot.
        let fp = builder.ins().get_frame_pointer(pointer_type);
        builder.ins().store(
            MemFlags::trusted(),
            fp,
            vmctx,
            i32::try_from(self.vmctx_plan.last_wasm_entry_fp).unwrap(),
        );

        // Then call the Wasm function with those arguments.
        let call = declare_and_call(&mut builder, wasm_call_sig, func_index.as_u32(), &args);
        let results = builder.func.dfg.inst_results(call).to_vec();
        
        store_values_to_array(
            &mut builder,
            func_ty.results(),
            &results,
            values_vec_ptr,
            values_vec_len,
            &mut env,
        );

        builder.ins().return_(&[]);
        builder.finalize();

        compiler.finish(None)
    }

    fn compile_wasm_to_builtin(
        &self,
        index: BuiltinFunctionIndex,
    ) -> crate::Result<CompiledFunction> {
        let isa = self.target_isa();
        let pointer_type = self.target_isa().pointer_type();
        let builtin_call_sig = BuiltinFunctionSignatures::new(isa).signature(index);

        let mut compiler = self.function_compiler();
        let func = ir::Function::with_name_signature(Default::default(), builtin_call_sig.clone());
        let (mut builder, block0) = compiler.builder(func);
        
        // TODO remove this
        let temp_mod = ParsedModule::default();
        let mut env = TranslationEnvironment::new(isa, &temp_mod);

        // Debug-assert that this is the right kind of vmctx, and then
        // additionally perform the "routine of the exit trampoline" of saving
        // fp/pc/etc.
        let vmctx = builder.block_params(block0)[0];
        debug_assert_vmctx_kind(isa, &mut builder, vmctx, VMCONTEXT_MAGIC, &mut env);
        save_last_wasm_exit_fp_and_pc(&mut builder, pointer_type, &self.vmctx_plan, vmctx);

        // Now it's time to delegate to the actual builtin. Builtins are stored
        // in an array in all `VMContext`s. First load the base pointer of the
        // array and then load the entry of the array that corresponds to this
        // builtin.
        let mem_flags = MemFlags::trusted().with_readonly();
        let array_addr = builder.ins().load(
            pointer_type,
            mem_flags,
            vmctx,
            i32::try_from(self.vmctx_plan.vmctx_builtin_functions()).unwrap(),
        );
        let body_offset = i32::try_from(index.as_u32() * pointer_type.bytes()).unwrap();
        let func_addr = builder
            .ins()
            .load(pointer_type, mem_flags, array_addr, body_offset);

        // Forward all our own arguments to the libcall itself, and then return
        // all the same results as the libcall.
        let block_params = builder.block_params(block0).to_vec();
        let sig = builder.func.import_signature(builtin_call_sig);
        let call = builder.ins().call_indirect(sig, func_addr, &block_params);
        let results = builder.func.dfg.inst_results(call).to_vec();
        builder.ins().return_(&results);
        builder.finalize();

        compiler.finish(None)
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

impl FunctionCompiler<'_> {
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

    fn finish(mut self, body: Option<&FunctionBody<'_>>) -> crate::Result<CompiledFunction> {
        let context = &mut self.ctx.codegen_context;

        context.set_disasm(true);
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

        if let Some(body) = body {
            let reader = body.get_binary_reader();
            let offset = reader.original_position();
            let len = reader.bytes_remaining();

            compiled_function.metadata.start_srcloc = FilePos(u32::try_from(offset).unwrap());
            compiled_function.metadata.end_srcloc = FilePos(u32::try_from(offset + len).unwrap());
        }

        self.ctx.codegen_context.clear();
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
        Self {
            func_translator: FuncTranslator::new(),
            codegen_context: Context::new(),
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
    builder.ins().call(callee, args)
}

fn save_last_wasm_exit_fp_and_pc(
    builder: &mut FunctionBuilder,
    pointer_type: ir::Type,
    vmctx_plan: &FixedVMContextPlan,
    vmctx: Value,
) {
    // Save the exit Wasm FP to the limits. We dereference the current FP to get
    // the previous FP because the current FP is the trampoline's FP, and we
    // want the Wasm function's FP, which is the caller of this trampoline.
    let trampoline_fp = builder.ins().get_frame_pointer(pointer_type);
    let wasm_fp = builder.ins().load(
        pointer_type,
        MemFlags::trusted(),
        trampoline_fp,
        // The FP always points to the next older FP for all supported
        // targets. See assertion in
        // `crates/wasmtime/src/runtime/vm/traphandlers/backtrace.rs`.
        0,
    );
    builder.ins().store(
        MemFlags::trusted(),
        wasm_fp,
        vmctx,
        Offset32::new(vmctx_plan.vmctx_last_wasm_exit_fp() as i32),
    );
    // Finally save the Wasm return address to the limits.
    let wasm_pc = builder.ins().get_return_address(pointer_type);
    builder.ins().store(
        MemFlags::trusted(),
        wasm_pc,
        vmctx,
        Offset32::new(vmctx_plan.vmctx_last_wasm_exit_pc() as i32),
    );
}

/// Used for loading the values of an array-call host function's value
/// array.
///
/// This can be used to load arguments out of the array if the trampoline we
/// are building exposes the array calling convention, or it can be used to
/// load results out of the array if the trampoline we are building calls a
/// function that uses the array calling convention.
fn load_values_from_array(
    types: &[ValType],
    builder: &mut FunctionBuilder,
    values_vec_ptr: Value,
    values_vec_capacity: Value,
    env: &mut TranslationEnvironment
) -> Vec<Value> {
    let value_size = size_of::<u128>();

    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity, env);

    // Note that this is little-endian like `store_values_to_array` above,
    // see notes there for more information.
    let flags = MemFlags::new()
        .with_notrap()
        .with_endianness(Endianness::Little);

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
    builder: &mut FunctionBuilder,
    types: &[ValType],
    values: &[Value],
    values_vec_ptr: Value,
    values_vec_capacity: Value,
    env: &mut TranslationEnvironment
) {
    debug_assert_eq!(types.len(), values.len());
    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity, env);

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
    env: &mut TranslationEnvironment
) {
    if cfg!(debug_assertions) {
        let enough_capacity = builder.ins().icmp_imm(
            ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
            capacity,
            ir::immediates::Imm64::new(length.try_into().unwrap()),
        );
        env.trapz(builder, enough_capacity, TRAP_INTERNAL_ASSERT);
    }
}

fn debug_assert_vmctx_kind(
    isa: &dyn TargetIsa,
    builder: &mut FunctionBuilder,
    vmctx: Value,
    expected_vmctx_magic: u32,
    env: &mut TranslationEnvironment
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
        env.trapz(builder, is_expected_vmctx, TRAP_INTERNAL_ASSERT);
    }
}
