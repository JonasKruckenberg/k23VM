use crate::translate_cranelift::TranslationEnvironment;
use crate::traps::TRAP_HEAP_MISALIGNED;
use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    Expr, Fact, InstBuilder, MemFlags, RelSourceLoc, TrapCode, Type, Value,
};
use cranelift_frontend::FunctionBuilder;
use wasmparser::MemArg;

/// Like `Option<T>` but specifically for passing information about transitions
/// from reachable to unreachable state and the like from callees to callers.
///
/// Marked `must_use` to force callers to update
/// `FuncTranslationState::reachable` as necessary.
#[derive(PartialEq, Eq)]
#[must_use]
pub enum Reachability<T> {
    /// The Wasm execution state is reachable, here is a `T`.
    Reachable(T),
    /// The Wasm execution state has been determined to be statically
    /// unreachable. It is the receiver of this value's responsibility to update
    /// `FuncTranslationState::reachable` as necessary.
    Unreachable,
}

#[derive(Debug)]
pub struct IRHeap {
    /// The address of the start of the heap's storage.
    pub base_gv: ir::GlobalValue,
    /// The index type for the heap.
    pub index_type: Type,
    /// The memory type for the pointed-to memory, if using proof-carrying code.
    pub memory_type: Option<ir::MemoryType>,
    /// Heap bound in bytes. The offset-guard pages are allocated after the
    /// bound.
    pub bound: u64,
    /// Guaranteed minimum heap size in bytes. Heap accesses before `min_size`
    /// don't need bounds checking.
    pub min_size: u64,
    /// The maximum heap size in bytes.
    ///
    /// Heap accesses larger than this will always trap.
    pub max_size: Option<u64>,
    /// Size in bytes of the offset-guard pages following the heap.
    pub offset_guard_size: u64,
    /// The log2 of this memory's page size.
    pub page_size_log2: u8,
}

impl IRHeap {
    /// Returns `None` when the Wasm access will unconditionally trap.
    ///
    /// Returns `(flags, wasm_addr, native_addr)`.
    pub fn prepare_addr(
        &self,
        builder: &mut FunctionBuilder,
        index: Value,
        access_size: u8,
        memarg: &MemArg,
        env: &mut TranslationEnvironment,
    ) -> crate::Result<Reachability<(MemFlags, Value, Value)>> {
        let addr = match u32::try_from(memarg.offset) {
            // If our offset fits within a u32, then we can place it into the
            // offset immediate of the `heap_addr` instruction.
            Ok(offset) => {
                self.bounds_check_and_compute_addr(builder, index, offset, access_size, env)?
            }
            // If the offset doesn't fit within a u32, then we can't pass it
            // directly into `heap_addr`.
            Err(_) => {
                let offset = builder.ins().iconst(self.index_type, memarg.offset as i64);
                let adjusted_index =
                    builder
                        .ins()
                        .uadd_overflow_trap(index, offset, TrapCode::HEAP_OUT_OF_BOUNDS);
                self.bounds_check_and_compute_addr(builder, adjusted_index, 0, access_size, env)?
            }
        };

        let addr = match addr {
            Reachability::Unreachable => return Ok(Reachability::Unreachable),
            Reachability::Reachable(a) => a,
        };

        // Note that we don't set `is_aligned` here, even if the load instruction's
        // alignment immediate may say it's aligned, because WebAssembly's
        // immediate field is just a hint, while Cranelift's aligned flag needs a
        // guarantee. WebAssembly memory accesses are always little-endian.
        let mut flags = MemFlags::new();
        flags.set_endianness(ir::Endianness::Little);

        if self.memory_type.is_some() {
            // Proof-carrying code is enabled; check this memory access.
            flags.set_checked();
        }

        // The access occurs to the `heap` disjoint category of abstract
        // state. This may allow alias analysis to merge redundant loads,
        // etc. when heap accesses occur interleaved with other (table,
        // vmctx, stack) accesses.
        flags.set_alias_region(Some(ir::AliasRegion::Heap));

        Ok(Reachability::Reachable((flags, index, addr)))
    }

    /// Like `prepare_addr` but for atomic accesses.
    ///
    /// Returns `None` when the Wasm access will unconditionally trap.
    pub fn prepare_atomic_addr(
        &self,
        builder: &mut FunctionBuilder,
        index: Value,
        loaded_bytes: u8,
        memarg: &MemArg,
        env: &mut TranslationEnvironment,
    ) -> crate::Result<Reachability<(MemFlags, Value, Value)>> {
        // Atomic addresses must all be aligned correctly, and for now we check
        // alignment before we check out-of-bounds-ness. The order of this check may
        // need to be updated depending on the outcome of the official threads
        // proposal itself.
        //
        // Note that with an offset>0 we generate an `iadd_imm` where the result is
        // thrown away after the offset check. This may truncate the offset and the
        // result may overflow as well, but those conditions won't affect the
        // alignment check itself. This can probably be optimized better and we
        // should do so in the future as well.
        if loaded_bytes > 1 {
            let effective_addr = if memarg.offset == 0 {
                index
            } else {
                builder
                    .ins()
                    .iadd_imm(index, i64::from(memarg.offset as i32))
            };
            debug_assert!(loaded_bytes.is_power_of_two());
            let misalignment = builder
                .ins()
                .band_imm(effective_addr, i64::from(loaded_bytes - 1));
            let f = builder.ins().icmp_imm(IntCC::NotEqual, misalignment, 0);
            builder.ins().trapnz(f, TRAP_HEAP_MISALIGNED);
        }

        self.prepare_addr(builder, index, loaded_bytes, memarg, env)
    }

    fn bounds_check_and_compute_addr(
        &self,
        builder: &mut FunctionBuilder,
        // Dynamic operand indexing into the memory.
        index: Value,
        // Static immediate added to the index.
        offset: u32,
        // Static size of the heap access.
        access_size: u8,
        env: &mut TranslationEnvironment,
    ) -> crate::Result<Reachability<Value>> {
        let pointer_bit_width = u16::try_from(env.pointer_type().bits()).unwrap();
        let orig_index = index;
        let index = cast_index_to_pointer_ty(
            index,
            self.index_type,
            env.pointer_type(),
            self.memory_type.is_some(),
            &mut builder.cursor(),
        );

        let spectre_mitigations_enabled = env.heap_access_spectre_mitigation();
        let pcc = env.proof_carrying_code();
        // Cannot overflow because we are widening to `u64`.
        let offset_and_size = offset as u64 + access_size as u64;

        // TODO what s this and why do we need it??
        let host_page_size_log2 = env.target_isa().page_size_align_log2();
        let can_use_virtual_memory = self.page_size_log2 >= host_page_size_log2;
        assert!(
            can_use_virtual_memory,
            "k23's memories require the ability to use virtual memory"
        );

        let make_compare = |builder: &mut FunctionBuilder,
                            compare_kind: IntCC,
                            lhs: Value,
                            lhs_off: Option<i64>,
                            rhs: Value,
                            rhs_off: Option<i64>| {
            let result = builder.ins().icmp(compare_kind, lhs, rhs);
            if pcc {
                // Name the original value as a def of the SSA value;
                // if the value was extended, name that as well with a
                // dynamic range, overwriting the basic full-range
                // fact that we previously put on the uextend.
                builder.func.dfg.facts[orig_index] = Some(Fact::Def { value: orig_index });
                if index != orig_index {
                    builder.func.dfg.facts[index] =
                        Some(Fact::value(pointer_bit_width, orig_index));
                }

                // Create a fact on the LHS that is a "trivial symbolic
                // fact": v1 has range v1+LHS_off..=v1+LHS_off
                builder.func.dfg.facts[lhs] = Some(Fact::value_offset(
                    pointer_bit_width,
                    orig_index,
                    lhs_off.unwrap(),
                ));
                // If the RHS is a symbolic value (v1 or gv1), we can
                // emit a Compare fact.
                if let Some(rhs) = builder.func.dfg.facts[rhs]
                    .as_ref()
                    .and_then(|f| f.as_symbol())
                {
                    builder.func.dfg.facts[result] = Some(Fact::Compare {
                        kind: compare_kind,
                        lhs: Expr::offset(&Expr::value(orig_index), lhs_off.unwrap()).unwrap(),
                        rhs: Expr::offset(rhs, rhs_off.unwrap()).unwrap(),
                    });
                }
                // Likewise, if the RHS is a constant, we can emit a
                // Compare fact.
                if let Some(k) = builder.func.dfg.facts[rhs]
                    .as_ref()
                    .and_then(|f| f.as_const(pointer_bit_width))
                {
                    builder.func.dfg.facts[result] = Some(Fact::Compare {
                        kind: compare_kind,
                        lhs: Expr::offset(&Expr::value(orig_index), lhs_off.unwrap()).unwrap(),
                        rhs: Expr::constant((k as i64).checked_add(rhs_off.unwrap()).unwrap()),
                    });
                }
            }
            result
        };

        if offset_and_size > self.bound {
            // 1. First special case: trap immediately if `offset + access_size >
            //    bound`, since we will end up being out-of-bounds regardless of the
            //    given `index`.
            builder.ins().trap(TrapCode::HEAP_OUT_OF_BOUNDS);
            Ok(Reachability::Unreachable)
        } else if self.index_type == ir::types::I32
            && u64::from(u32::MAX) <= self.bound + self.offset_guard_size - offset_and_size
        {
            // 2. Second special case for when we can completely omit explicit
            //    bounds checks for 32-bit static memories.
            //
            //    First, let's rewrite our comparison to move all the constants
            //    to one side:
            //
            //            index + offset + access_size > bound
            //        ==> index > bound - (offset + access_size)
            //
            //    We know the subtraction on the right-hand side won't wrap because
            //    we didn't hit the first special case.
            //
            //    Additionally, we add our guard pages (if any) to the right-hand
            //    side, since we can rely on the virtual memory subsystem at runtime
            //    to catch out-of-bound accesses within the range `bound .. bound +
            //    guard_size`. So now we are dealing with
            //
            //        index > bound + guard_size - (offset + access_size)
            //
            //    Note that `bound + guard_size` cannot overflow for
            //    correctly-configured heaps, as otherwise the heap wouldn't fit in
            //    a 64-bit memory space.
            //
            //    The complement of our should-this-trap comparison expression is
            //    the should-this-not-trap comparison expression:
            //
            //        index <= bound + guard_size - (offset + access_size)
            //
            //    If we know the right-hand side is greater than or equal to
            //    `u32::MAX`, then
            //
            //        index <= u32::MAX <= bound + guard_size - (offset + access_size)
            //
            //    This expression is always true when the heap is indexed with
            //    32-bit integers because `index` cannot be larger than
            //    `u32::MAX`. This means that `index` is always either in bounds or
            //    within the guard page region, neither of which require emitting an
            //    explicit bounds check.

            Ok(Reachability::Reachable(
                self.compute_addr(
                    &mut builder.cursor(),
                    env.pointer_type(),
                    index,
                    offset,
                    self.memory_type
                        .map(|ty| (ty, self.bound + self.offset_guard_size)),
                ),
            ))
        } else {
            // 3. General case for static memories.
            //
            //    We have to explicitly test whether
            //
            //        index > bound - (offset + access_size)
            //
            //    and trap if so.
            //
            //    Since we have to emit explicit bounds checks, we might as well be
            //    precise, not rely on the virtual memory subsystem at all, and not
            //    factor in the guard pages here.

            // NB: this subtraction cannot wrap because we didn't hit the first
            // special case.
            let adjusted_bound = self.bound - offset_and_size;
            let adjusted_bound_value = builder
                .ins()
                .iconst(env.pointer_type(), adjusted_bound as i64);
            if pcc {
                builder.func.dfg.facts[adjusted_bound_value] =
                    Some(Fact::constant(pointer_bit_width, adjusted_bound));
            }
            let oob = make_compare(
                builder,
                IntCC::UnsignedGreaterThan,
                index,
                Some(0),
                adjusted_bound_value,
                Some(0),
            );
            Ok(Reachability::Reachable(
                self.explicit_check_oob_condition_and_compute_addr(
                    builder,
                    env.pointer_type(),
                    index,
                    offset,
                    access_size,
                    spectre_mitigations_enabled,
                    self.memory_type.map(|ty| (ty, self.bound)),
                    oob,
                    env,
                ),
            ))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn explicit_check_oob_condition_and_compute_addr(
        &self,
        builder: &mut FunctionBuilder,
        addr_ty: Type,
        index: Value,
        offset: u32,
        access_size: u8,
        // Whether Spectre mitigations are enabled for heap accesses.
        spectre_mitigations_enabled: bool,
        // Whether we're emitting PCC facts.
        pcc: Option<(ir::MemoryType, u64)>,
        // The `i8` boolean value that is non-zero when the heap access is out of
        // bounds (and therefore we should trap) and is zero when the heap access is
        // in bounds (and therefore we can proceed).
        oob_condition: Value,
        env: &mut TranslationEnvironment,
    ) -> Value {
        if !spectre_mitigations_enabled {
            env.trapnz(builder, oob_condition, TrapCode::HEAP_OUT_OF_BOUNDS);
        }
        let mut addr = self.compute_addr(&mut builder.cursor(), addr_ty, index, offset, pcc);

        if spectre_mitigations_enabled {
            let null = builder.ins().iconst(addr_ty, 0);
            addr = builder
                .ins()
                .select_spectre_guard(oob_condition, null, addr);

            if let Some((ty, size)) = pcc {
                builder.func.dfg.facts[null] =
                    Some(Fact::constant(u16::try_from(addr_ty.bits()).unwrap(), 0));
                builder.func.dfg.facts[addr] = Some(Fact::Mem {
                    ty,
                    min_offset: 0,
                    max_offset: size.checked_sub(u64::from(access_size)).unwrap(),
                    nullable: true,
                });
            }
        }

        addr
    }

    /// Emit code for the native address computation of a Wasm address,
    /// without any bounds checks or overflow checks.
    ///
    /// It is the caller's responsibility to ensure that any necessary bounds and
    /// overflow checks are emitted, and that the resulting address is never used
    /// unless they succeed.
    fn compute_addr(
        &self,
        pos: &mut FuncCursor,
        addr_ty: Type,
        index: Value,
        offset: u32,
        pcc: Option<(ir::MemoryType, u64)>,
    ) -> Value {
        debug_assert_eq!(pos.func.dfg.value_type(index), addr_ty);

        let heap_base = pos.ins().global_value(addr_ty, self.base_gv);

        match pcc {
            None => {}
            Some((ty, _size)) => {
                pos.func.dfg.facts[heap_base] = Some(Fact::Mem {
                    ty,
                    min_offset: 0,
                    max_offset: 0,
                    nullable: false,
                });
            }
        }

        let base_and_index = pos.ins().iadd(heap_base, index);

        if let Some((ty, _)) = pcc {
            if let Some(idx) = pos.func.dfg.facts[index]
                .as_ref()
                .and_then(|f| f.as_symbol())
                .cloned()
            {
                pos.func.dfg.facts[base_and_index] = Some(Fact::DynamicMem {
                    ty,
                    min: idx.clone(),
                    max: idx,
                    nullable: false,
                });
            } else {
                pos.func.dfg.facts[base_and_index] = Some(Fact::Mem {
                    ty,
                    min_offset: 0,
                    max_offset: u64::from(u32::MAX),
                    nullable: false,
                });
            }
        }

        if offset == 0 {
            base_and_index
        } else {
            // NB: The addition of the offset immediate must happen *before* the
            // `select_spectre_guard`, if any. If it happens after, then we
            // potentially are letting speculative execution read the whole first
            // 4GiB of memory.
            let offset_val = pos.ins().iconst(addr_ty, i64::from(offset));

            if pcc.is_some() {
                pos.func.dfg.facts[offset_val] = Some(Fact::constant(
                    u16::try_from(addr_ty.bits()).unwrap(),
                    u64::from(offset),
                ));
            }

            let result = pos.ins().iadd(base_and_index, offset_val);

            if let Some((ty, _)) = pcc {
                if let Some(idx) = pos.func.dfg.facts[index]
                    .as_ref()
                    .and_then(|f| f.as_symbol())
                {
                    pos.func.dfg.facts[result] = Some(Fact::DynamicMem {
                        ty,
                        min: idx.clone(),
                        // Safety: adding an offset to an expression with
                        // zero offset -- add cannot wrap, so `unwrap()`
                        // cannot fail.
                        max: Expr::offset(idx, i64::from(offset)).unwrap(),
                        nullable: false,
                    });
                } else {
                    pos.func.dfg.facts[result] = Some(Fact::Mem {
                        ty,
                        min_offset: u64::from(offset),
                        // Safety: can't overflow -- two u32s summed in a
                        // 64-bit add. TODO: when memory64 is supported here,
                        // `u32::MAX` is no longer true, and we'll need to
                        // handle overflow here.
                        max_offset: u64::from(u32::MAX) + u64::from(offset),
                        nullable: false,
                    });
                }
            }

            result
        }
    }
}

fn cast_index_to_pointer_ty(
    index: Value,
    index_ty: Type,
    pointer_ty: Type,
    pcc: bool,
    pos: &mut FuncCursor,
) -> Value {
    if index_ty == pointer_ty {
        return index;
    }
    // Note that using 64-bit heaps on a 32-bit host is not currently supported,
    // would require at least a bounds check here to ensure that the truncation
    // from 64-to-32 bits doesn't lose any upper bits. For now though we're
    // mostly interested in the 32-bit-heaps-on-64-bit-hosts cast.
    assert!(index_ty.bits() < pointer_ty.bits());

    // Convert `index` to `addr_ty`.
    let extended_index = pos.ins().uextend(pointer_ty, index);

    // Add a range fact on the extended value.
    if pcc {
        pos.func.dfg.facts[extended_index] = Some(Fact::max_range_for_width_extended(
            u16::try_from(index_ty.bits()).unwrap(),
            u16::try_from(pointer_ty.bits()).unwrap(),
        ));
    }

    // Add debug value-label alias so that debuginfo can name the extended
    // value as the address
    let loc = pos.srcloc();
    let loc = RelSourceLoc::from_base_offset(pos.func.params.base_srcloc(), loc);
    pos.func
        .stencil
        .dfg
        .add_value_label_alias(extended_index, loc, index);

    extended_index
}
