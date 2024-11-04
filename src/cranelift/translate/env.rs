#![allow(unused)]

use crate::cranelift::translate::global::Global;
use crate::cranelift::translate::heap::Heap;
use crate::indices::{
    DataIndex, ElemIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TypeIndex,
};
use crate::runtime::VMOffsets;
use crate::translate::{ModuleTypes, WasmparserTypeConverter};
use crate::translate::{TranslatedModule, WasmHeapTopTypeInner, WasmHeapType, WasmRefType};
use crate::utils::{value_type, wasm_call_signature};
use crate::NS_WASM_FUNC;
use alloc::vec;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64};
use cranelift_codegen::ir::{
    ExtFuncData, ExternalName, Fact, FuncRef, GlobalValue, GlobalValueData, Inst, MemFlags,
    MemoryType, SigRef, Signature, Type, UserExternalName, Value,
};
use cranelift_codegen::ir::{Function, InstBuilder};
use cranelift_codegen::isa::TargetIsa;
use cranelift_frontend::FunctionBuilder;
use smallvec::SmallVec;
use crate::cranelift::translate::builtins::BuiltinFunctions;

/// A smallvec that holds the IR values for a struct's fields.
pub type StructFieldsVec = SmallVec<[Value; 4]>;

pub struct TranslationEnvironment<'module_env> {
    isa: &'module_env dyn TargetIsa,
    module: &'module_env TranslatedModule,
    types: &'module_env ModuleTypes,
    vmoffsets: VMOffsets,
    
    /// Caches of signatures for builtin functions.
    builtin_functions: BuiltinFunctions,

    /// The Cranelift global holding the vmctx address.
    vmctx: Option<GlobalValue>,
    /// The PCC memory type describing the vmctx layout, if we're
    /// using PCC.
    pcc_vmctx_memtype: Option<MemoryType>,

    /// Whether to force relaxed simd instructions to be deterministic.
    relaxed_simd_deterministic: bool,
    /// Whether to use the heap access spectre mitigation.
    heap_access_spectre_mitigation: bool,
    /// Whether to use proof-carrying code to verify lowerings.
    proof_carrying_code: bool,
}

impl<'module_env> TranslationEnvironment<'module_env> {
    pub(crate) fn new(
        isa: &'module_env dyn TargetIsa,
        module: &'module_env TranslatedModule,
        types: &'module_env ModuleTypes,
    ) -> Self {
        let vmoffsets = VMOffsets::for_module(module, isa.pointer_bytes() as u32);
        let builtin_functions = BuiltinFunctions::new(isa);
        Self {
            isa,
            module,
            types,
            vmoffsets,
            builtin_functions,
            
            vmctx: None,
            pcc_vmctx_memtype: None,

            relaxed_simd_deterministic: false,
            heap_access_spectre_mitigation: true,
            proof_carrying_code: true,
        }
    }

    fn vmctx(&mut self, func: &mut Function) -> GlobalValue {
        self.vmctx.unwrap_or_else(|| {
            let vmctx = func.create_global_value(GlobalValueData::VMContext);

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
    ) -> (GlobalValue, i32) {
        let vmctx = self.vmctx(func);

        if let Some(def_index) = self.module.defined_global_index(index) {
            let offset =
                i32::try_from(self.vmoffsets.vmctx_global_definition(def_index)).unwrap();
            (vmctx, offset)
        } else {
            let from_offset = self.vmoffsets.vmctx_global_import_from(index);
            let global = func.create_global_value(ir::GlobalValueData::Load {
                base: vmctx,
                offset: Offset32::new(i32::try_from(from_offset).unwrap()),
                global_type: self.pointer_type(),
                flags: MemFlags::trusted().with_readonly(),
            });
            (global, 0)
        }
    }
}

impl<'module_env> TranslationEnvironment<'module_env> {
    pub fn make_direct_func(
        &self,
        func: &mut Function,
        index: FuncIndex,
    ) -> crate::Result<FuncRef> {
        let sig_index = self.module.functions[index].signature;
        let sig = self
            .types
            .get_wasm_type(self.module.types[sig_index])
            .unwrap()
            .unwrap_func();

        let signature = func.import_signature(wasm_call_signature(self.isa, sig));
        let name =
            ExternalName::User(func.declare_imported_user_function(UserExternalName::new(
                NS_WASM_FUNC,
                index.as_u32(),
            )));

        Ok(func.import_function(ExtFuncData {
            name,
            signature,
            colocated: self.module.defined_func_index(index).is_some(),
        }))
    }

    pub fn make_indirect_sig(
        &self,
        func: &mut Function,
        sig_index: TypeIndex,
    ) -> crate::Result<SigRef> {
        todo!()
    }

    pub fn make_heap(&mut self, func: &mut Function, index: MemoryIndex) -> crate::Result<Heap> {
        let plan = &self.module.memory_plans[index];

        let (vmctx, base_offset, ptr_memtype) = match self.module.defined_memory_index(index) {
            None => todo!("imported memory"),
            Some(_) if plan.shared => todo!("shared memory"),
            Some(def_index) => {
                let vmctx = self.vmctx(func);
                let base_offset = self.vmoffsets.vmctx_memory_definition_base(def_index);
                let base_offset = i32::try_from(base_offset).unwrap();

                (vmctx, base_offset, self.pcc_vmctx_memtype)
            }
        };

        let (base_fact, memory_type) = if let Some(ptr_memtype) = ptr_memtype {
            // Create a memtype representing the untyped memory region.
            let data_mt = func.create_memory_type(ir::MemoryTypeData::Memory {
                // Since we have one memory per address space, the maximum value this can be is u64::MAX
                // TODO this isn't correct I think
                size: plan.max_size_based_on_index_type(),
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
            base: vmctx,
            offset: Offset32::new(base_offset),
            global_type: self.pointer_type(),
            flags,
        });
        func.global_value_facts[heap_base] = base_fact;

        let min_size = plan.minimum_byte_size().unwrap_or_else(|_| {
            // The only valid Wasm memory size that won't fit in a 64-bit
            // integer is the maximum memory64 size (2^64) which is one
            // larger than `u64::MAX` (2^64 - 1). In this case, just say the
            // minimum heap size is `u64::MAX`.
            debug_assert_eq!(plan.minimum, 1 << 48);
            debug_assert_eq!(plan.page_size(), 1 << 16);
            u64::MAX
        });
        let max_size = plan.maximum_byte_size().ok();

        Ok(Heap {
            base_gv: heap_base,
            memory_type,
            min_size,
            max_size,
            bound: plan.max_size_based_on_index_type(),
            index_type: if plan.memory64 { I64 } else { I32 },
            offset_guard_size: plan.offset_guard_size,
            page_size_log2: plan.page_size_log2,
        })
    }

    pub(crate) fn make_global(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
    ) -> crate::Result<Global> {
        let global = &self.module.globals[index];
        debug_assert!(!global.shared);

        let (gv, offset) = self.get_global_location(func, index);

        Ok(Global::Memory {
            gv,
            offset: offset.into(),
            ty: value_type(
                &self.module.globals[index].content_type,
                self.pointer_type(),
            ),
        })
    }
    pub fn target_isa(&self) -> &dyn TargetIsa {
        self.isa
    }
    /// Whether or not to force relaxed simd instructions to have deterministic
    /// lowerings meaning they will produce the same results across all hosts,
    /// regardless of the cost to performance.
    pub fn relaxed_simd_deterministic(&self) -> bool {
        self.relaxed_simd_deterministic
    }
    pub fn heap_access_spectre_mitigation(&self) -> bool {
        self.heap_access_spectre_mitigation
    }
    pub fn proof_carrying_code(&self) -> bool {
        self.proof_carrying_code
    }

    /// Get the Cranelift integer type to use for native pointers.
    ///
    /// This returns `I64` for 64-bit architectures and `I32` for 32-bit architectures.
    pub fn pointer_type(&self) -> Type {
        self.target_isa().pointer_type()
    }

    /// Get the Cranelift reference type to use for the given Wasm reference
    /// type.
    ///
    /// Returns a pair of the CLIF reference type to use and a boolean that
    /// describes whether the value should be included in GC stack maps or not.
    pub fn reference_type(&self, hty: &WasmHeapType) -> (Type, bool) {
        let ty = crate::utils::reference_type(&hty, self.pointer_type());
        let needs_stack_map = match hty.top().inner {
            WasmHeapTopTypeInner::Extern | WasmHeapTopTypeInner::Any => {
                todo!("references other than functions are not yet supported")
            }
            WasmHeapTopTypeInner::Func => false,
            _ => todo!(),
        };
        (ty, needs_stack_map)
    }

    pub(crate) fn convert_heap_type(&self, ty: &wasmparser::HeapType) -> WasmHeapType {
        WasmparserTypeConverter::new(self.types, self.module).convert_heap_type(ty)
    }

    pub fn has_native_fma(&self) -> bool {
        self.target_isa().has_native_fma()
    }
    pub fn is_x86(&self) -> bool {
        self.target_isa().triple().architecture == target_lexicon::Architecture::X86_64
    }
    pub fn use_x86_blendv_for_relaxed_laneselect(&self, ty: Type) -> bool {
        self.target_isa().has_x86_blendv_lowering(ty)
    }
    pub fn use_x86_pshufb_for_relaxed_swizzle(&self) -> bool {
        self.target_isa().has_x86_pshufb_lowering()
    }
    pub fn use_x86_pmulhrsw_for_relaxed_q15mul(&self) -> bool {
        self.target_isa().has_x86_pmulhrsw_lowering()
    }
    pub fn use_x86_pmaddubsw_for_dot(&self) -> bool {
        self.target_isa().has_x86_pmaddubsw_lowering()
    }

    /// Is the given parameter of the given function a wasm parameter or
    /// an internal implementation-detail parameter?
    pub fn is_wasm_parameter(&self, index: usize) -> bool {
        // The first two parameters are the function vmctx and caller vmctx. The rest are
        // the wasm parameters.
        index >= 2
    }

    /// Is the given parameter of the given function a wasm parameter or
    /// an internal implementation-detail parameter?
    pub fn is_wasm_return(&self, signature: &Signature, index: usize) -> bool {
        signature.returns[index].purpose == ir::ArgumentPurpose::Normal
    }

    /// Optional hook for customizing how `trap` is lowered.
    pub fn trap(&mut self, builder: &mut FunctionBuilder, code: ir::TrapCode) {
        builder.ins().trap(code);
    }

    /// Optional hook for customizing how `trapz` is lowered.
    pub fn trapz(&mut self, builder: &mut FunctionBuilder, value: Value, code: ir::TrapCode) {
        builder.ins().trapz(value, code);
    }

    /// Optional hook for customizing how `trapnz` is lowered.
    pub fn trapnz(&mut self, builder: &mut FunctionBuilder, value: Value, code: ir::TrapCode) {
        builder.ins().trapnz(value, code);
    }

    /// Optional hook for customizing how `uadd_overflow_trap` is lowered.
    pub fn uadd_overflow_trap(
        &mut self,
        builder: &mut FunctionBuilder,
        lhs: Value,
        rhs: Value,
        code: ir::TrapCode,
    ) -> Value {
        builder.ins().uadd_overflow_trap(lhs, rhs, code)
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
    pub fn translate_ref_null(
        &mut self,
        pos: FuncCursor,
        hty: &WasmHeapType,
    ) -> crate::Result<Value> {
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
    // Translate a `struct.new` instruction.
    pub fn translate_struct_new(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        fields: StructFieldsVec,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.new_default` instruction.
    pub fn translate_struct_new_default(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.set` instruction.
    pub fn translate_struct_set(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
        value: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a `struct.get` instruction.
    pub fn translate_struct_get(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.get_s` instruction.
    pub fn translate_struct_get_s(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.get_u` instruction.
    pub fn translate_struct_get_u(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new` instruction.
    pub fn translate_array_new(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        elem: Value,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_default` instruction.
    pub fn translate_array_new_default(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_fixed` instruction.
    pub fn translate_array_new_fixed(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        elems: &[Value],
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_data` instruction.
    pub fn translate_array_new_data(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        data_index: DataIndex,
        data_offset: Value,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_elem` instruction.
    pub fn translate_array_new_elem(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        elem_index: ElemIndex,
        elem_offset: Value,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.copy` instruction.
    pub fn translate_array_copy(
        &mut self,
        builder: &mut FunctionBuilder,
        dst_array_type_index: TypeIndex,
        dst_array: Value,
        dst_index: Value,
        src_array_type_index: TypeIndex,
        src_array: Value,
        src_index: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.fill` instruction.
    pub fn translate_array_fill(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
        value: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.init_data` instruction.
    pub fn translate_array_init_data(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        dst_index: Value,
        data_index: DataIndex,
        data_offset: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.init_elem` instruction.
    pub fn translate_array_init_elem(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        dst_index: Value,
        elem_index: ElemIndex,
        elem_offset: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.len` instruction.
    pub fn translate_array_len(
        &mut self,
        builder: &mut FunctionBuilder,
        array: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.get` instruction.
    pub fn translate_array_get(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.get_s` instruction.
    pub fn translate_array_get_s(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.get_u` instruction.
    pub fn translate_array_get_u(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.set` instruction.
    pub fn translate_array_set(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
        value: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a `ref.test` instruction.
    pub fn translate_ref_test(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        ref_ty: WasmRefType,
        gc_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }
}
