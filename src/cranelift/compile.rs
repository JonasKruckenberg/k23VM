use crate::builtins::BuiltinFunctionIndex;
use crate::compile::{
    CompiledFunction, Compiler, FilePos, InstructionAddressMapping, ELFOSABI_K23,
};
use crate::cranelift::translate::env::TranslationEnvironment;
use crate::cranelift::translate::FuncTranslator;
use crate::indices::DefinedFuncIndex;
use crate::runtime::{StaticVMOffsets, VMCONTEXT_MAGIC};
use crate::translate::{ModuleTypes, Translation, WasmFuncType, WasmValType};
use crate::traps::TRAP_INTERNAL_ASSERT;
use crate::utils::{array_call_signature, value_type, wasm_call_signature};
use crate::NS_WASM_FUNC;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::{
    Endianness, GlobalValueData, InstBuilder, MemFlags, Type, UserExternalName, UserFuncName, Value,
};
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::{ir, Context, MachSrcLoc, TextSectionBuilder};
use cranelift_frontend::FunctionBuilder;
use object::write::Object;
use object::{BinaryFormat, FileFlags};
use spin::Mutex;
use target_lexicon::Architecture;
use wasmparser::{FuncToValidate, FuncValidatorAllocations, FunctionBody, ValidatorResources};

pub struct CraneliftCompiler {
    isa: OwnedTargetIsa,
    contexts: Mutex<Vec<CompilationContext>>,
    vmoffsets: StaticVMOffsets,
}

impl CraneliftCompiler {
    pub fn new(isa: OwnedTargetIsa) -> Self {
        Self {
            vmoffsets: StaticVMOffsets::new(u32::try_from(isa.pointer_bytes()).unwrap()),
            isa,
            contexts: Mutex::new(Vec::with_capacity(1)), // TODO make this the size of the default threadpool
        }
    }

    fn target_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
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

impl Compiler for CraneliftCompiler {
    fn compile_function(
        &self,
        translation: &Translation,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
        body: FunctionBody<'_>,
        validator: FuncToValidate<ValidatorResources>,
    ) -> crate::Result<CompiledFunction> {
        let isa = self.target_isa();

        let mut compiler = self.function_compiler();
        let context = &mut compiler.ctx.codegen_context;

        let mut env = TranslationEnvironment::new(isa, &translation.module, types);

        // Setup function signature
        let index = translation.module.func_index(index);
        let sig_index = translation.module.functions[index].signature;
        let func_ty = types
            .get_wasm_type(translation.module.types[sig_index])
            .unwrap()
            .unwrap_func();

        context.func.signature = wasm_call_signature(self.target_isa(), func_ty);
        context.func.name = UserFuncName::User(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: index.as_u32(),
        });

        let vmctx = context.func.create_global_value(GlobalValueData::VMContext);
        let stack_limit = context.func.create_global_value(GlobalValueData::Load {
            base: vmctx,
            offset: i32::try_from(self.vmoffsets.vmctx_stack_limit())
                .unwrap()
                .into(),
            global_type: isa.pointer_type(),
            flags: MemFlags::trusted(),
        });
        context.func.stack_limit = Some(stack_limit);

        // collect debug info
        context.func.collect_debug_info();

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

    fn compile_array_to_wasm_trampoline(
        &self,
        translation: &Translation,
        types: &ModuleTypes,
        func_index: DefinedFuncIndex,
    ) -> crate::Result<CompiledFunction> {
        // This function has a special calling convention where all arguments and return values
        // are passed through an array in memory (so we can have dynamic function signatures in rust)
        let pointer_type = self.isa.pointer_type();

        let func_index = translation.module.func_index(func_index);
        let sig_index = translation.module.functions[func_index].signature;
        let func_ty = types
            .get_wasm_type(translation.module.types[sig_index])
            .unwrap()
            .unwrap_func();

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
            &func_ty.params,
            &mut builder,
            values_vec_ptr,
            values_vec_len,
            pointer_type,
        );
        args.insert(0, caller_vmctx);
        args.insert(0, vmctx);

        // Assert that we were really given a core Wasm vmctx, since that's
        // what we are assuming with our offsets below.
        debug_assert_vmctx_kind(self.target_isa(), &mut builder, vmctx, VMCONTEXT_MAGIC);
        // Then store our current stack pointer into the appropriate slot.
        let fp = builder.ins().get_frame_pointer(pointer_type);
        builder.ins().store(
            MemFlags::trusted(),
            fp,
            vmctx,
            i32::try_from(self.vmoffsets.last_wasm_entry_fp).unwrap(),
        );

        // Then call the Wasm function with those arguments.
        let call = declare_and_call(&mut builder, wasm_call_sig, func_index.as_u32(), &args);
        let results = builder.func.dfg.inst_results(call).to_vec();

        store_values_to_array(
            &mut builder,
            &func_ty.results,
            &results,
            values_vec_ptr,
            values_vec_len,
        );

        builder.ins().return_(&[]);
        builder.finalize();

        compiler.finish(None)
    }

    fn compile_wasm_to_array_trampoline(
        &self,
        _wasm_func_ty: &WasmFuncType,
    ) -> crate::Result<CompiledFunction> {
        todo!()
    }

    fn compile_wasm_to_builtin(
        &self,
        _index: BuiltinFunctionIndex,
    ) -> crate::Result<CompiledFunction> {
        todo!()
    }

    fn text_section_builder(&self, num_funcs: usize) -> Box<dyn TextSectionBuilder> {
        self.isa.text_section_builder(num_funcs)
    }

    fn create_intermediate_code_object(&self) -> Object {
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
}

struct FunctionCompiler<'a> {
    compiler: &'a CraneliftCompiler,
    ctx: CompilationContext,
}

impl FunctionCompiler<'_> {
    fn builder(&mut self, func: ir::Function) -> (FunctionBuilder<'_>, ir::Block) {
        self.ctx.codegen_context.func = func;
        let mut builder = FunctionBuilder::new(
            &mut self.ctx.codegen_context.func,
            self.ctx.func_translator.context_mut(),
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

            compiled_function.metadata.start_srcloc = FilePos::new(u32::try_from(offset).unwrap());
            compiled_function.metadata.end_srcloc =
                FilePos::new(u32::try_from(offset + len).unwrap());

            let srclocs = compiled_function
                .buffer
                .get_srclocs_sorted()
                .into_iter()
                .map(|&MachSrcLoc { start, end, loc }| (loc, start, end - start));

            compiled_function.metadata.address_map = collect_address_map(
                u32::try_from(compiled_function.buffer.data().len()).unwrap(),
                srclocs,
            )
            .into_boxed_slice();
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
    pointer_type: Type,
    vmctx_plan: &StaticVMOffsets,
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
        // targets.
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
    types: &[WasmValType],
    builder: &mut FunctionBuilder,
    values_vec_ptr: Value,
    values_vec_capacity: Value,
    pointer_type: Type,
) -> Vec<Value> {
    let value_size = size_of::<u128>();

    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity);

    // Note that this is little-endian like `store_values_to_array` above,
    // see notes there for more information.
    let flags = MemFlags::new()
        .with_notrap()
        .with_endianness(Endianness::Little);

    let mut results = Vec::new();
    for (i, ty) in types.iter().enumerate() {
        let ir_ty = value_type(ty, pointer_type);
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
    types: &[WasmValType],
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
        builder.ins().trapz(enough_capacity, TRAP_INTERNAL_ASSERT);
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
        builder.ins().trapz(is_expected_vmctx, TRAP_INTERNAL_ASSERT);
    }
}

fn collect_address_map(
    code_size: u32,
    iter: impl IntoIterator<Item = (ir::SourceLoc, u32, u32)>,
) -> Vec<InstructionAddressMapping> {
    let mut iter = iter.into_iter();
    let (mut cur_loc, mut cur_offset, mut cur_len) = match iter.next() {
        Some(i) => i,
        None => return Vec::new(),
    };
    let mut ret = Vec::new();
    for (loc, offset, len) in iter {
        // If this instruction is adjacent to the previous and has the same
        // source location then we can "coalesce" it with the current
        // instruction.
        if cur_offset + cur_len == offset && loc == cur_loc {
            cur_len += len;
            continue;
        }

        // Push an entry for the previous source item.
        ret.push(InstructionAddressMapping {
            srcloc: cvt(cur_loc),
            code_offset: cur_offset,
        });
        // And push a "dummy" entry if necessary to cover the span of ranges,
        // if any, between the previous source offset and this one.
        if cur_offset + cur_len != offset {
            ret.push(InstructionAddressMapping {
                srcloc: FilePos::default(),
                code_offset: cur_offset + cur_len,
            });
        }
        // Update our current location to get extended later or pushed on at
        // the end.
        cur_loc = loc;
        cur_offset = offset;
        cur_len = len;
    }
    ret.push(InstructionAddressMapping {
        srcloc: cvt(cur_loc),
        code_offset: cur_offset,
    });
    if cur_offset + cur_len != code_size {
        ret.push(InstructionAddressMapping {
            srcloc: FilePos::default(),
            code_offset: cur_offset + cur_len,
        });
    }

    return ret;

    fn cvt(loc: ir::SourceLoc) -> FilePos {
        if loc.is_default() {
            FilePos::default()
        } else {
            FilePos::new(loc.bits())
        }
    }
}
