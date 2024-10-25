use crate::indices::{
    DataIndex, DefinedMemoryIndex, ElemIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex,
    TypeIndex,
};
use crate::translate::heap::IRHeap;
use crate::translate::{IRGlobal, TranslatedModule};
use crate::utils::value_type;
use crate::vmcontext::VMContextPlan;
use alloc::vec;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64};
use cranelift_codegen::ir::{
    Fact, FuncRef, Function, GlobalValue, Inst, MemFlags, SigRef, Signature, Type, Value,
};
use cranelift_codegen::isa::TargetIsa;
use cranelift_frontend::FunctionBuilder;
use tracing::log;
use wasmparser::{HeapType, ValType};

/// Environment state required for function translation.
///
/// This type holds all information about the wider WASM module and runtime to
/// facilitate the translation of a single function.
pub struct TranslationEnvironment<'module_env, 'wasm> {
    isa: &'module_env dyn TargetIsa,
    module: &'module_env TranslatedModule<'wasm>,

    pub(crate) vmctx_plan: VMContextPlan,

    /// The Cranelift global holding the vmctx address.
    vmctx: Option<ir::GlobalValue>,
    /// The PCC memory type describing the vmctx layout, if we're
    /// using PCC.
    pcc_vmctx_memtype: Option<ir::MemoryType>,
}

impl<'module_env, 'wasm> TranslationEnvironment<'module_env, 'wasm> {
    pub(crate) fn new(
        isa: &'module_env dyn TargetIsa,
        module: &'module_env TranslatedModule<'wasm>,
    ) -> Self {
        Self {
            isa,
            module,
            vmctx_plan: VMContextPlan::for_module(isa, module),
            vmctx: None,
            pcc_vmctx_memtype: None,
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

impl TranslationEnvironment<'_, '_> {
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
    ) -> crate::TranslationResult<IRGlobal> {
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
    ) -> crate::TranslationResult<IRHeap> {
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
                    let owned_index = self.module.owned_memory_index(def_index);
                    let owned_base_offset =
                        self.vmctx_plan.vmctx_memory_definition_base(owned_index);
                    let current_base_offset = i32::try_from(owned_base_offset).unwrap();

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

        log::debug!("Created heap for memory {index:?}: {heap:?}");

        Ok(heap)
    }

    pub fn make_direct_func(
        &self,
        func: &mut Function,
        index: FuncIndex,
    ) -> crate::TranslationResult<FuncRef> {
        todo!()
    }

    pub(crate) fn make_indirect_sig(
        &self,
        func: &mut Function,
        index: TypeIndex,
    ) -> crate::TranslationResult<SigRef> {
        todo!()
    }

    /// Translate a WASM `global.get` instruction at the builder's current position
    /// for a global that is custom.
    pub fn translate_custom_global_get(
        &mut self,
        builder: &mut FunctionBuilder,
        index: GlobalIndex,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Translate a WASM `global.set` instruction at the builder's current position
    /// for a global that is custom.
    pub fn translate_custom_global_set(
        &mut self,
        builder: &mut FunctionBuilder,
        index: GlobalIndex,
        value: Value,
    ) -> crate::TranslationResult<()> {
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
        _callee_index: FuncIndex,
        callee: FuncRef,
        call_args: &[Value],
    ) -> crate::TranslationResult<Inst> {
        todo!()
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
    /// Returns `None` if this statically traps instead of creating a call
    /// instruction.
    pub fn translate_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        sig_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::TranslationResult<Option<Inst>> {
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
    ) -> crate::TranslationResult<Inst> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<Value> {
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
    ) -> crate::TranslationResult<Value> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
        todo!()
    }

    /// Translate a WASM `data.drop` instruction.
    pub fn translate_data_drop(
        &mut self,
        pos: FuncCursor,
        data_index: DataIndex,
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<Value> {
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
    ) -> crate::TranslationResult<Value> {
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
    ) -> crate::TranslationResult<Value> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<()> {
        todo!()
    }

    /// Translate a WASM `elem.drop` instruction.
    pub fn translate_elem_drop(
        &mut self,
        pos: FuncCursor,
        elem_index: ElemIndex,
    ) -> crate::TranslationResult<()> {
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
    ) -> crate::TranslationResult<Value> {
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
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Translate a `ref.null T` WebAssembly instruction.
    pub fn translate_ref_null(
        &mut self,
        pos: FuncCursor,
        hty: HeapType,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Translate a `ref.is_null` WebAssembly instruction.
    pub fn translate_ref_is_null(
        &mut self,
        pos: FuncCursor,
        value: Value,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Translate a `ref.func` WebAssembly instruction.
    pub fn translate_ref_func(
        &mut self,
        pos: FuncCursor,
        index: FuncIndex,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Translate an `i32` value into an `i31ref`.
    pub fn translate_ref_i31(
        &mut self,
        pos: FuncCursor,
        value: Value,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Sign-extend an `i31ref` into an `i32`.
    pub fn translate_i31_get_s(
        &mut self,
        pos: FuncCursor,
        value: Value,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }

    /// Zero-extend an `i31ref` into an `i32`.
    pub fn translate_i31_get_u(
        &mut self,
        pos: FuncCursor,
        value: Value,
    ) -> crate::TranslationResult<Value> {
        todo!()
    }
}
