use crate::indices::{
    DataIndex, ElemIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TypeIndex,
};
use crate::parse::ParsedModule;
use crate::translate_cranelift::builtins::BuiltinFunctions;
use crate::translate_cranelift::heap::IRHeap;
use crate::translate_cranelift::IRGlobal;
use crate::traps::{Trap, TRAP_INTERNAL_ASSERT};
use crate::utils::{value_type, wasm_call_signature};
use crate::vm::VMContextPlan;
use alloc::vec;
use alloc::vec::Vec;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64, I8};
use cranelift_codegen::ir::{
    ArgumentPurpose, Fact, FuncRef, Function, Inst, InstBuilder, MemFlags, SigRef, Signature,
    TrapCode, Type, Value,
};
use cranelift_codegen::isa::TargetIsa;
use cranelift_frontend::FunctionBuilder;
use wasmparser::HeapType;

/// Environment state required for function translation.
///
/// This type holds all information about the wider WASM module and runtime to
/// facilitate the translation of a single function.
pub struct TranslationEnvironment<'module_env> {
    isa: &'module_env dyn TargetIsa,
    module: &'module_env ParsedModule,

    pub(crate) vmctx_plan: VMContextPlan,

    /// The Cranelift global holding the vmctx address.
    vmctx: Option<ir::GlobalValue>,
    /// The PCC memory type describing the vmctx layout, if we're
    /// using PCC.
    pcc_vmctx_memtype: Option<ir::MemoryType>,

    /// Caches of signatures for builtin functions.
    builtin_functions: BuiltinFunctions,

    /// Whether to use software trap_handling for WASM trap_handling instead of native trap_handling.
    ///
    /// This is useful when signal/interrupt handling is not possible or desired.
    software_traps: bool,
}

impl<'module_env> TranslationEnvironment<'module_env> {
    pub(crate) fn new(isa: &'module_env dyn TargetIsa, module: &'module_env ParsedModule) -> Self {
        let builtin_functions = BuiltinFunctions::new(isa);
        Self {
            isa,
            module,
            vmctx_plan: VMContextPlan::for_module(isa, module),
            vmctx: None,
            pcc_vmctx_memtype: None,
            software_traps: true,
            builtin_functions,
        }
    }

    pub(crate) fn target_isa(&self) -> &dyn TargetIsa {
        self.isa
    }

    /// Get the Cranelift integer type to use for native pointers.
    ///
    /// This returns `I64` for 64-bit architectures and `I32` for 32-bit architectures.
    pub(crate) fn pointer_type(&self) -> Type {
        self.isa.pointer_type()
    }

    /// Get the Cranelift reference type to use for the given Wasm reference
    /// type.
    ///
    /// Returns a pair of the CLIF reference type to use and a boolean that
    /// describes whether the value should be included in GC stack maps or not.
    pub fn reference_type(&self, hty: HeapType) -> (Type, bool) {
        match hty {
            HeapType::FUNC => (self.pointer_type(), false),
            HeapType::EXTERN => (ir::types::I32, true),
            _ => unreachable!(),
        }
    }

    fn memory_index_type(&self, index: MemoryIndex) -> Type {
        if self.module.memory_plans[index].memory64 {
            I64
        } else {
            I32
        }
    }

    /// Whether or not to force relaxed simd instructions to have deterministic
    /// lowerings meaning they will produce the same results across all hosts,
    /// regardless of the cost to performance.
    pub(crate) fn relaxed_simd_deterministic(&self) -> bool {
        false
    }
    pub(crate) fn heap_access_spectre_mitigation(&self) -> bool {
        true
    }
    pub(crate) fn proof_carrying_code(&self) -> bool {
        true
    }
    pub(crate) fn has_native_fma(&self) -> bool {
        self.isa.has_native_fma()
    }
    pub(crate) fn is_x86(&self) -> bool {
        self.isa.triple().architecture == target_lexicon::Architecture::X86_64
    }
    pub(crate) fn use_x86_blendv_for_relaxed_laneselect(&self, ty: Type) -> bool {
        self.isa.has_x86_blendv_lowering(ty)
    }
    pub(crate) fn use_x86_pshufb_for_relaxed_swizzle(&self) -> bool {
        self.isa.has_x86_pshufb_lowering()
    }
    pub(crate) fn use_x86_pmulhrsw_for_relaxed_q15mul(&self) -> bool {
        self.isa.has_x86_pmulhrsw_lowering()
    }
    pub(crate) fn use_x86_pmaddubsw_for_dot(&self) -> bool {
        self.isa.has_x86_pmaddubsw_lowering()
    }

    /// Is the given parameter of the given function a wasm parameter or
    /// an internal implementation-detail parameter?
    pub fn is_wasm_parameter(&self, index: usize) -> bool {
        // The first two parameters are the vmctx and caller vmctx. The rest are
        // the wasm parameters.
        index >= 2
    }

    /// Is the given parameter of the given function a wasm parameter or
    /// an internal implementation-detail parameter?
    pub fn is_wasm_return(&self, signature: &Signature, index: usize) -> bool {
        signature.returns[index].purpose == ir::ArgumentPurpose::Normal
    }
}

impl TranslationEnvironment<'_> {
    fn vmctx(&mut self, func: &mut Function) -> ir::GlobalValue {
        self.vmctx.unwrap_or_else(|| {
            let vmctx = func.create_global_value(ir::GlobalValueData::VMContext);
            if self.isa.flags().enable_pcc() {
                // Create a placeholder memtype for the vmctx; we'll
                // add fields to it as we lazily create HeapData
                // structs and global values.
                let vmctx_memtype = func.create_memory_type(ir::MemoryTypeData::Struct {
                    size: 0,
                    fields: vec![],
                });

                self.pcc_vmctx_memtype = Some(vmctx_memtype);
                func.global_value_facts[vmctx] = Some(Fact::Mem {
                    ty: vmctx_memtype,
                    min_offset: 0,
                    max_offset: 0,
                    nullable: false,
                });
            }

            self.vmctx = Some(vmctx);
            vmctx
        })
    }

    pub(crate) fn vmctx_val(&mut self, pos: &mut FuncCursor<'_>) -> Value {
        let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(&mut pos.func);
        pos.ins().global_value(pointer_type, vmctx)
    }

    fn get_global_location(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
    ) -> (ir::GlobalValue, i32) {
        let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(func);
        if let Some(def_index) = self.module.defined_global_index(index) {
            let offset = i32::try_from(self.vmctx_plan.vmctx_global_definition(def_index)).unwrap();
            (vmctx, offset)
        } else {
            let from_offset = self.vmctx_plan.vmctx_global_import_from(index);
            let global = func.create_global_value(ir::GlobalValueData::Load {
                base: vmctx,
                offset: Offset32::new(i32::try_from(from_offset).unwrap()),
                global_type: pointer_type,
                flags: MemFlags::trusted().with_readonly(),
            });
            (global, 0)
        }
    }

    pub(crate) fn make_global(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
    ) -> crate::Result<IRGlobal> {
        let (gv, offset) = self.get_global_location(func, index);

        Ok(IRGlobal::Memory {
            gv,
            offset: offset.into(),
            ty: value_type(self.module.globals[index].content_type),
        })
    }

    pub(crate) fn make_heap(
        &mut self,
        func: &mut Function,
        index: MemoryIndex,
    ) -> crate::Result<IRHeap> {
        let mp = &self.module.memory_plans[index];

        let min_size = mp.minimum_byte_size().unwrap_or_else(|_| {
            // The only valid Wasm memory size that won't fit in a 64-bit
            // integer is the maximum memory64 size (2^64) which is one
            // larger than `u64::MAX` (2^64 - 1). In this case, just say the
            // minimum heap size is `u64::MAX`.
            debug_assert_eq!(mp.minimum, 1 << 48);
            debug_assert_eq!(mp.page_size(), 1 << 16);
            u64::MAX
        });
        let max_size = mp.maximum_byte_size().ok();

        let (ptr, base_offset, ptr_memtype) = {
            let vmctx = self.vmctx(func);

            match self.module.defined_memory_index(index) {
                // The memory is imported. The actual `VMMemoryDefinition` is stored elsewhere,
                // and we just store a `*mut VMMemoryDefinition` in our `VMContext`.
                None => {
                    todo!("imported memories")
                    // let from_offset = self.vmctx_plan.vmctx_memory_import_from(index);
                    // let (memory, def_mt) = self.load_pointer_with_memtypes(
                    //     func,
                    //     vmctx,
                    //     from_offset,
                    //     true,
                    //     self.pcc_vmctx_memtype,
                    // );
                    // let base_offset = i32::from(self.vmctx_plan.vmctx_memory_definition_base_offset());
                    // (memory, base_offset, def_mt)
                }
                // The memory is defined here, but potentially shared.
                // As with imported memories, only store a `*mut VMMemoryDefinition`.
                Some(def_index) if mp.shared => {
                    todo!("shared memories")
                    // let from_offset = self.vmctx_plan.vmctx_memory_pointer(def_index);
                    // let (memory, def_mt) = self.load_pointer_with_memtypes(
                    //     func,
                    //     vmctx,
                    //     from_offset,
                    //     true,
                    //     self.pcc_vmctx_memtype,
                    // );
                    // let base_offset = i32::from(self.vmctx_plan.vmctx_memory_definition_base_offset());
                    //
                    // (memory, base_offset, def_mt)
                }
                // The memory is defined here (good ol' classic memories)
                Some(def_index) => {
                    let base_offset = self.vmctx_plan.vmctx_memory_definition_base(def_index);
                    let current_base_offset = i32::try_from(base_offset).unwrap();

                    (vmctx, current_base_offset, self.pcc_vmctx_memtype)
                }
            }
        };

        let (base_fact, data_mt) = if let Some(ptr_memtype) = ptr_memtype {
            // Create a memtype representing the untyped memory region.
            let data_mt = func.create_memory_type(ir::MemoryTypeData::Memory {
                // Since we have one memory per address space, the maximum value this can be is u64::MAX
                // TODO verify this through testing
                size: mp.max_size_based_on_index_type(),
            });
            // This fact applies to any pointer to the start of the memory.
            let base_fact = Fact::Mem {
                ty: data_mt,
                min_offset: 0,
                max_offset: 0,
                nullable: false,
            };
            // Create a field in the vmctx for the base pointer.
            match &mut func.memory_types[ptr_memtype] {
                ir::MemoryTypeData::Struct { size, fields } => {
                    let offset = u64::try_from(base_offset).unwrap();
                    fields.push(ir::MemoryTypeField {
                        offset,
                        ty: self.isa.pointer_type(),
                        // Read-only field from the PoV of PCC checks:
                        // don't allow stores to this field. (Even if
                        // it is a dynamic memory whose base can
                        // change, that update happens inside the
                        // runtime, not in generated code.)
                        readonly: true,
                        fact: Some(base_fact.clone()),
                    });
                    *size =
                        core::cmp::max(*size, offset + u64::from(self.isa.pointer_type().bytes()));
                }
                _ => {
                    panic!("Bad memtype");
                }
            }
            // Apply a fact to the base pointer.
            (Some(base_fact), Some(data_mt))
        } else {
            (None, None)
        };

        let mut flags = MemFlags::trusted().with_checked();
        flags.set_readonly();
        let heap_base = func.create_global_value(ir::GlobalValueData::Load {
            base: ptr,
            offset: Offset32::new(base_offset),
            global_type: self.pointer_type(),
            flags,
        });
        func.global_value_facts[heap_base] = base_fact;

        let heap = IRHeap {
            base_gv: heap_base,
            min_size,
            max_size,
            bound: mp.max_size_based_on_index_type(),
            index_type: self.memory_index_type(index),
            memory_type: ptr_memtype,
            // bound: u64::MAX,
            offset_guard_size: mp.offset_guard_size,
            page_size_log2: mp.page_size_log2,
        };

        tracing::debug!("Created heap for memory {index:?}: {heap:?}");

        Ok(heap)
    }

    pub fn make_direct_func(
        &self,
        func: &mut Function,
        index: FuncIndex,
    ) -> crate::Result<FuncRef> {
        let sig_index = self.module.functions[index].signature;
        let sig = &self.module.types[sig_index];

        let sig = wasm_call_signature(self.isa, sig);
        let signature = func.import_signature(sig);

        let name =
            ir::ExternalName::User(func.declare_imported_user_function(ir::UserExternalName {
                namespace: crate::NS_WASM_FUNC,
                index: index.as_u32(),
            }));

        Ok(func.import_function(ir::ExtFuncData {
            name,
            signature,

            // the value of this flag determines the codegen for calls to this
            // function. if this flag is `false` then absolute relocations will
            // be generated for references to the function, which requires
            // load-time relocation resolution. if this flag is set to `true`
            // then relative relocations are emitted which can be resolved at
            // object-link-time, just after all functions are compiled.
            //
            // this flag is set to `true` for functions defined in the object
            // we'll be defining in this compilation unit, or everything local
            // to the wasm module. this means that between functions in a wasm
            // module there's relative calls encoded. all calls external to a
            // wasm module (e.g. imports or libcalls) are either encoded through
            // the `vmcontext` as relative jumps (hence no relocations) or
            // they're libcalls with absolute relocations.
            colocated: self.module.defined_func_index(index).is_some(),
        }))
    }

    pub(crate) fn make_indirect_sig(
        &self,
        func: &mut Function,
        index: TypeIndex,
    ) -> crate::Result<SigRef> {
        todo!()
    }

    pub fn trap(&mut self, builder: &mut FunctionBuilder, code: TrapCode) {
        match (self.software_traps, Trap::from_trap_code(code)) {
            // If software trap_handling are enabled and there is a trap for this code,
            // insert a call to the trap libcall. We also insert a native trap instruction afterward
            // for safety.
            (true, Some(trap)) => {
                let libcall = self.builtin_functions.trap(&mut builder.func);
                let vmctx = self.vmctx_val(&mut builder.cursor());
                let trap_code = builder.ins().iconst(I8, i64::from(u8::from(trap)));

                builder.ins().call(libcall, &[vmctx, trap_code]);

                builder.ins().trap(TRAP_INTERNAL_ASSERT);
            }

            // Otherwise, if software trap_handling are disabled or there is no trap for this code, just emit a native trap instruction.
            (false, _) | (_, None) => {
                builder.ins().trap(code);
            }
        }
    }
    pub fn trapz(&mut self, builder: &mut FunctionBuilder, value: Value, code: TrapCode) {
        if self.software_traps {
            let ty = builder.func.dfg.value_type(value);
            let zero = builder.ins().iconst(ty, 0);
            let cmp = builder.ins().icmp(IntCC::Equal, value, zero);
            self.conditionally_trap(builder, cmp, code);
        } else {
            builder.ins().trapz(value, code);
        }
    }
    pub fn trapnz(&mut self, builder: &mut FunctionBuilder, value: Value, code: TrapCode) {
        if self.software_traps {
            let ty = builder.func.dfg.value_type(value);
            let zero = builder.ins().iconst(ty, 0);
            let cmp = builder.ins().icmp(IntCC::NotEqual, value, zero);
            self.conditionally_trap(builder, cmp, code);
        } else {
            builder.ins().trapnz(value, code);
        }
    }

    /// Helper to conditionally trap on `cond`.
    fn conditionally_trap(&mut self, builder: &mut FunctionBuilder, cond: Value, code: TrapCode) {
        debug_assert!(self.software_traps);

        let trap_block = builder.create_block();
        builder.set_cold_block(trap_block);
        let continuation_block = builder.create_block();

        builder
            .ins()
            .brif(cond, trap_block, &[], continuation_block, &[]);

        builder.seal_block(trap_block);
        builder.seal_block(continuation_block);

        builder.switch_to_block(trap_block);
        self.trap(builder, code);
        builder.switch_to_block(continuation_block);
    }

    /// Translate a WASM `global.get` instruction at the builder's current position
    /// for a global that is custom.
    pub fn translate_custom_global_get(
        &mut self,
        builder: &mut FunctionBuilder,
        index: GlobalIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `global.set` instruction at the builder's current position
    /// for a global that is custom.
    pub fn translate_custom_global_set(
        &mut self,
        builder: &mut FunctionBuilder,
        index: GlobalIndex,
        value: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `call` instruction at the builder's current
    /// position.
    ///
    /// Insert instructions for a *direct call* to the function `callee_index`.
    /// The function reference `callee` was previously created by `make_direct_func()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    pub fn translate_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: FuncIndex,
        callee: FuncRef,
        call_args: &[Value],
    ) -> crate::Result<Inst> {
        let mut real_call_args = Vec::with_capacity(call_args.len() + 2);
        let caller_vmctx = builder
            .func
            .special_param(ArgumentPurpose::VMContext)
            .unwrap();

        // If the function is locally defined to a direct call
        if !self.module.is_imported_func(callee_index) {
            // First append the callee vmctx address, which is the same as the caller vmctx in
            // this case.
            real_call_args.push(caller_vmctx);

            // Then append the caller vmctx address.
            real_call_args.push(caller_vmctx);

            // Then append the regular call arguments.
            real_call_args.extend_from_slice(call_args);

            Ok(builder.ins().call(callee, &real_call_args))
        } else {
            let pointer_type = self.pointer_type();
            let sig_ref = builder.func.dfg.ext_funcs[callee].signature;
            let vmctx = self.vmctx(builder.func);
            let base = builder.ins().global_value(pointer_type, vmctx);
            let mem_flags = MemFlags::trusted().with_readonly();

            // Load the callee address.
            let body_offset = i32::try_from(
                self.vmctx_plan
                    .vmctx_function_import_wasm_call(callee_index),
            )
            .unwrap();
            let func_addr = builder
                .ins()
                .load(pointer_type, mem_flags, base, body_offset);

            // First append the callee vmctx address.
            let vmctx_offset =
                i32::try_from(self.vmctx_plan.vmctx_function_import_vmctx(callee_index)).unwrap();
            let vmctx = builder
                .ins()
                .load(pointer_type, mem_flags, base, vmctx_offset);
            real_call_args.push(vmctx);
            real_call_args.push(caller_vmctx);

            // Then append the regular call arguments.
            real_call_args.extend_from_slice(call_args);

            Ok(builder
                .ins()
                .call_indirect(sig_ref, func_addr, &real_call_args))
        }
    }

    /// Translate a WASM `call_indirect` instruction at the builder's current
    /// position.
    ///
    /// Insert instructions for an *indirect call* to the function `callee` in the table
    /// `table_index` with WASM signature `sig_index`. The `callee` value will have type
    /// `i32`.
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    /// Returns `None` if this statically trap_handling instead of creating a call
    /// instruction.
    pub fn translate_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        sig_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::Result<Option<Inst>> {
        todo!()
    }

    /// Translate a WASM `call_ref` instruction at the builder's current
    /// position.
    ///
    /// Insert instructions at the builder's current position for an *indirect call*
    /// to the function `callee`. The `callee` value will be a Wasm funcref
    /// that may need to be translated to a native function address depending on
    /// your implementation of this trait.
    ///
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    pub fn translate_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::Result<Inst> {
        todo!()
    }

    /// Translate a WASM `return_call` instruction at the builder's
    /// current position.
    ///
    /// Insert instructions at the builder's current position for a *direct tail call*
    /// to the function `callee_index`.
    ///
    /// The function reference `callee` was previously created by `make_direct_func()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    pub fn translate_return_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: FuncIndex,
        callee: FuncRef,
        args: &[Value],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `return_call_indirect` instruction at the
    /// builder's current position.
    ///
    /// Insert instructions at the builder's current position for an *indirect tail call*
    /// to the function `callee` in the table `table_index` with WebAssembly signature
    /// `sig_index`. The `callee` value will have type `i32`.
    ///
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    pub fn translate_return_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        type_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `return_call_ref` instruction at the builder's
    /// current position.
    ///
    /// Insert instructions at the builder's current position for an *indirect tail call*
    /// to the function `callee`. The `callee` value will be a Wasm funcref that may need
    /// to be translated to a native function address depending on your implementation of
    /// this trait.
    ///
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    pub fn translate_return_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `memory.grow` instruction at `pos`.
    ///
    /// The `memory_index` identifies the linear memory to grow and `delta` is the
    /// requested memory size in WASM pages.
    ///
    /// Returns the old size (in WASM pages) of the memory or `-1` to indicate failure.
    pub fn translate_memory_grow(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        delta: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `memory.size` instruction at `pos`.
    ///
    /// The `memory_index` identifies the linear memory.
    ///
    /// Returns the current size (in WASM pages) of the memory.
    pub fn translate_memory_size(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `memory.copy` instruction.
    ///
    /// The `src_index` and `dst_index` identify the source and destination linear memories respectively,
    /// `src_pos` and `dst_pos` are the source and destination offsets in bytes, and `len` is the number of bytes to copy.
    pub fn translate_memory_copy(
        &mut self,
        pos: FuncCursor,
        src_index: MemoryIndex,
        dst_index: MemoryIndex,
        src_pos: Value,
        dst_pos: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `memory.fill` instruction.
    ///
    /// The `memory_index` identifies the linear memory, `dst` is the offset in bytes, `val` is the
    /// value to fill the memory with and `len` is the number of bytes to fill.
    pub fn translate_memory_fill(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        dst: Value,
        value: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `memory.init` instruction.
    ///
    /// The `memory_index` identifies the linear memory amd `data_index` identifies the passive data segment.
    /// The `dst` value is the destination offset into the linear memory, `_src` is the offset into the
    /// data segment and `len` is the number of bytes to copy.
    pub fn translate_memory_init(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        data_index: DataIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `data.drop` instruction.
    pub fn translate_data_drop(
        &mut self,
        pos: FuncCursor,
        data_index: DataIndex,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.size` instruction.
    ///
    /// The `table_index` identifies the table.
    ///
    /// Returns the table size in elements.
    pub fn translate_table_size(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `table.grow` instruction.
    ///
    /// The `table_index` identifies the table, `delta` is the number of elements to grow by
    /// and `initial_value` the value to fill the newly created elements with.
    ///
    /// Returns the old size of the table or `-1` to indicate failure.
    pub fn translate_table_grow(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        delta: Value,
        initial_value: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `table.get` instruction.
    ///
    /// The `table_index` identifies the table and `index` is the index of the element to retrieve.
    ///
    /// Returns the element at the given index.
    pub fn translate_table_get(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `table.set` instruction.
    ///
    /// The `table_index` identifies the table, `value` is the value to set and `index` is the index of the element to set.
    pub fn translate_table_set(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        value: Value,
        index: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.copy` instruction.
    ///
    /// The `src_index` and `dst_index` identify the source and destination tables respectively,
    /// `dst` and `_src` are the destination and source offsets and `len` is the number of elements to copy.
    pub fn translate_table_copy(
        &mut self,
        pos: FuncCursor,
        src_index: TableIndex,
        dst_index: TableIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.fill` instruction.
    ///
    /// The `table_index` identifies the table, `dst` is the offset, `value` is the value to fill the range.
    pub fn translate_table_fill(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        dst: Value,
        value: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.init` instruction.
    ///
    /// The `table_index` identifies the table, `elem_index` identifies the passive element segment,
    /// `dst` is the destination offset, `_src` is the source offset and `len` is the number of elements to copy.
    pub fn translate_table_init(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        elem_index: ElemIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `elem.drop` instruction.
    pub fn translate_elem_drop(
        &mut self,
        pos: FuncCursor,
        elem_index: ElemIndex,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM i32.atomic.wait` or `i64.atomic.wait` instruction.
    ///
    /// The `memory_index` identifies the linear memory and `address` is the address to wait on.
    /// Whether the waited-on value is 32- or 64-bit can be determined by examining the type of
    /// `expected`, which must be only I32 or I64.
    ///
    /// TODO address?
    /// TODO timeout?
    /// TODO expected_value?
    ///
    /// Returns an i32, which is negative if the helper call failed.
    pub fn translate_atomic_wait(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        address: Value,
        expected_value: Value,
        timeout: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `atomic.notify` instruction.
    ///
    /// The `memory_index` identifies the linear memory.
    ///
    /// TODO address?
    /// TODO count?
    ///
    /// Returns an i64, which is negative if the helper call failed.
    pub fn translate_atomic_notify(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        address: Value,
        count: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `ref.null T` WebAssembly instruction.
    pub fn translate_ref_null(&mut self, pos: FuncCursor, hty: HeapType) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `ref.is_null` WebAssembly instruction.
    pub fn translate_ref_is_null(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `ref.func` WebAssembly instruction.
    pub fn translate_ref_func(
        &mut self,
        pos: FuncCursor,
        index: FuncIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `i32` value into an `i31ref`.
    pub fn translate_ref_i31(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }

    /// Sign-extend an `i31ref` into an `i32`.
    pub fn translate_i31_get_s(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }

    /// Zero-extend an `i31ref` into an `i32`.
    pub fn translate_i31_get_u(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }
}