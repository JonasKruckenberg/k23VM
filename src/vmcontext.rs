use crate::guest_memory::round_usize_up_to_host_pages;
use crate::guest_memory::MmapVec;
use crate::indices::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, FuncRefIndex,
    GlobalIndex, MemoryIndex, OwnedMemoryIndex, TableIndex, TypeIndex,
};
use crate::translate::TranslatedModule;
use core::ffi::c_void;
use core::marker::PhantomPinned;
use core::mem::offset_of;
use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;
use core::{fmt, mem};
use cranelift_codegen::isa::TargetIsa;
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::Unsigned;
use wasmparser::ValType;

pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"vmcx");

#[derive(Debug)]
#[repr(C, align(16))] // align 16 since globals are aligned to that and contained inside
pub struct VMContext {
    _m: PhantomPinned,
}

/// An "opaque" version of `VMContext` which must be explicitly casted to a
/// target context.
///
/// This context is used to represent that contexts specified in
/// `VMFuncRef` can have any type and don't have an implicit
/// structure. Neither wasmtime nor cranelift-generated code can rely on the
/// structure of an opaque context in general and only the code which configured
/// the context is able to rely on a particular structure. This is because the
/// context pointer configured for `VMFuncRef` is guaranteed to be
/// the first parameter passed.
///
/// Note that Wasmtime currently has a layout where all contexts that are casted
/// to an opaque context start with a 32-bit "magic" which can be used in debug
/// mode to debug-assert that the casts here are correct and have at least a
/// little protection against incorrect casts.
pub struct VMOpaqueContext {
    pub(crate) magic: u32,
    _marker: PhantomPinned,
}

impl VMOpaqueContext {
    /// Helper function to clearly indicate that casts are desired.
    #[inline]
    pub fn from_vmcontext(ptr: *mut VMContext) -> *mut VMOpaqueContext {
        ptr.cast()
    }
}

/// A function pointer that exposes the array calling convention.
///
/// Regardless of the underlying Wasm function type, all functions using the
/// array calling convention have the same Rust signature.
///
/// Arguments:
///
/// * Callee `vmctx` for the function itself.
///
/// * Caller's `vmctx` (so that host functions can access the linear memory of
///   their Wasm callers).
///
/// * A pointer to a buffer of `ValRaw`s where both arguments are passed into
///   this function, and where results are returned from this function.
///
/// * The capacity of the `ValRaw` buffer. Must always be at least
///   `max(len(wasm_params), len(wasm_results))`.
pub type VMArrayCallFunction =
    unsafe extern "C" fn(*mut VMContext, *mut VMContext, *mut VMVal, usize);

/// A function pointer that exposes the Wasm calling convention.
///
/// In practice, different Wasm function types end up mapping to different Rust
/// function types, so this isn't simply a type alias the way that
/// `VMArrayCallFunction` is. However, the exact details of the calling
/// convention are left to the Wasm compiler (e.g. Cranelift or Winch). Runtime
/// code never does anything with these function pointers except shuffle them
/// around and pass them back to Wasm.
#[repr(transparent)]
pub struct VMWasmCallFunction(VMFunctionBody);

/// A placeholder byte-sized type which is just used to provide some amount of type
/// safety when dealing with pointers to JIT-compiled function bodies. Note that it's
/// deliberately not Copy, as we shouldn't be carelessly copying function body bytes
/// around.
#[repr(C)]
pub struct VMFunctionBody(u8);

#[derive(Clone, Copy)]
pub union VMVal {
    pub i32: i32,
    pub i64: i64,
    pub f32: u32,
    pub f64: u64,
    pub v128: [u8; 16],
    pub funcref: *mut c_void,
    pub externref: u32,
    pub anyref: u32,
}

impl fmt::Debug for VMVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unsafe { f.debug_tuple("VMVal").field(&self.v128).finish() }
    }
}

impl PartialEq for VMVal {
    fn eq(&self, other: &Self) -> bool {
        unsafe { self.v128 == other.v128 }
    }
}

impl VMVal {
    #[inline]
    pub fn i32(i: i32) -> VMVal {
        VMVal { i32: i.to_le() }
    }
    #[inline]
    pub fn i64(i: i64) -> VMVal {
        VMVal { i64: i.to_le() }
    }
    #[inline]
    pub fn u32(i: u32) -> VMVal {
        VMVal::i32(i as i32)
    }
    #[inline]
    pub fn u64(i: u64) -> VMVal {
        VMVal::i64(i as i64)
    }
    #[inline]
    pub fn f32(i: u32) -> VMVal {
        VMVal { f32: i.to_le() }
    }
    #[inline]
    pub fn f64(i: u64) -> VMVal {
        VMVal { f64: i.to_le() }
    }
    #[inline]
    pub fn v128(i: u128) -> VMVal {
        VMVal {
            v128: i.to_le_bytes(),
        }
    }
    #[inline]
    pub fn funcref(ptr: *mut c_void) -> VMVal {
        VMVal {
            funcref: ptr.map_addr(|i| i.to_le()),
        }
    }
    #[inline]
    pub fn externref(e: u32) -> VMVal {
        assert_eq!(e, 0, "gc not supported");
        VMVal {
            externref: e.to_le(),
        }
    }
    #[inline]
    pub fn anyref(r: u32) -> VMVal {
        assert_eq!(r, 0, "gc not supported");
        VMVal { anyref: r.to_le() }
    }

    #[inline]
    pub fn get_i32(&self) -> i32 {
        unsafe { i32::from_le(self.i32) }
    }
    #[inline]
    pub fn get_i64(&self) -> i64 {
        unsafe { i64::from_le(self.i64) }
    }
    #[inline]
    pub fn get_u32(&self) -> u32 {
        self.get_i32().unsigned()
    }
    #[inline]
    pub fn get_u64(&self) -> u64 {
        self.get_i64().unsigned()
    }
    #[inline]
    pub fn get_f32(&self) -> u32 {
        unsafe { u32::from_le(self.f32) }
    }
    #[inline]
    pub fn get_f64(&self) -> u64 {
        unsafe { u64::from_le(self.f64) }
    }
    #[inline]
    pub fn get_v128(&self) -> u128 {
        unsafe { u128::from_le_bytes(self.v128) }
    }
    #[inline]
    pub fn get_funcref(&self) -> *mut c_void {
        unsafe { self.funcref.map_addr(usize::from_le) }
    }
    #[inline]
    pub fn get_externref(&self) -> u32 {
        let externref = u32::from_le(unsafe { self.externref });
        assert_eq!(externref, 0, "gc not supported");
        externref
    }
    #[inline]
    pub fn get_anyref(&self) -> u32 {
        let anyref = u32::from_le(unsafe { self.anyref });
        assert_eq!(anyref, 0, "gc not supported");
        anyref
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMTableDefinition {
    pub base: *mut u8,
    pub current_length: u64,
}

#[derive(Debug)]
#[repr(C)]
pub struct VMMemoryDefinition {
    pub base: *mut u8,
    pub current_length: AtomicUsize,
}

#[derive(Debug)]
#[repr(C)]
pub struct VMGlobalDefinition {
    data: [u8; 16],
}

impl VMGlobalDefinition {
    #[allow(clippy::needless_pass_by_value)]
    pub unsafe fn from_vmval(vmval: VMVal) -> Self {
        Self { data: vmval.v128 }
    }

    pub unsafe fn to_vmval(&self, wasm_ty: &ValType) -> VMVal {
        match wasm_ty {
            ValType::I32 => VMVal {
                i32: *self.as_i32(),
            },
            ValType::I64 => VMVal {
                i64: *self.as_i64(),
            },
            ValType::F32 => VMVal {
                f32: *self.as_f32_bits(),
            },
            ValType::F64 => VMVal {
                f64: *self.as_f64_bits(),
            },
            ValType::V128 => VMVal { v128: self.data },
            ValType::Ref(_) => todo!(),
        }
    }

    /// Return a reference to the value as an i32.
    pub unsafe fn as_i32(&self) -> &i32 {
        &*(self.data.as_ref().as_ptr().cast::<i32>())
    }

    /// Return a mutable reference to the value as an i32.
    pub unsafe fn as_i32_mut(&mut self) -> &mut i32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<i32>())
    }

    /// Return a reference to the value as a u32.
    pub unsafe fn as_u32(&self) -> &u32 {
        &*(self.data.as_ref().as_ptr().cast::<u32>())
    }

    /// Return a mutable reference to the value as an u32.
    pub unsafe fn as_u32_mut(&mut self) -> &mut u32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u32>())
    }

    /// Return a reference to the value as an i64.
    pub unsafe fn as_i64(&self) -> &i64 {
        &*(self.data.as_ref().as_ptr().cast::<i64>())
    }

    /// Return a mutable reference to the value as an i64.
    pub unsafe fn as_i64_mut(&mut self) -> &mut i64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<i64>())
    }

    /// Return a reference to the value as an u64.
    pub unsafe fn as_u64(&self) -> &u64 {
        &*(self.data.as_ref().as_ptr().cast::<u64>())
    }

    /// Return a mutable reference to the value as an u64.
    pub unsafe fn as_u64_mut(&mut self) -> &mut u64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u64>())
    }

    /// Return a reference to the value as an f32.
    pub unsafe fn as_f32(&self) -> &f32 {
        &*(self.data.as_ref().as_ptr().cast::<f32>())
    }

    /// Return a mutable reference to the value as an f32.
    pub unsafe fn as_f32_mut(&mut self) -> &mut f32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<f32>())
    }

    /// Return a reference to the value as f32 bits.
    pub unsafe fn as_f32_bits(&self) -> &u32 {
        &*(self.data.as_ref().as_ptr().cast::<u32>())
    }

    /// Return a mutable reference to the value as f32 bits.
    pub unsafe fn as_f32_bits_mut(&mut self) -> &mut u32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u32>())
    }

    /// Return a reference to the value as an f64.
    pub unsafe fn as_f64(&self) -> &f64 {
        &*(self.data.as_ref().as_ptr().cast::<f64>())
    }

    /// Return a mutable reference to the value as an f64.
    pub unsafe fn as_f64_mut(&mut self) -> &mut f64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<f64>())
    }

    /// Return a reference to the value as f64 bits.
    pub unsafe fn as_f64_bits(&self) -> &u64 {
        &*(self.data.as_ref().as_ptr().cast::<u64>())
    }

    /// Return a mutable reference to the value as f64 bits.
    pub unsafe fn as_f64_bits_mut(&mut self) -> &mut u64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u64>())
    }

    /// Return a reference to the value as an u128.
    pub unsafe fn as_u128(&self) -> &u128 {
        &*(self.data.as_ref().as_ptr().cast::<u128>())
    }

    /// Return a mutable reference to the value as an u128.
    pub unsafe fn as_u128_mut(&mut self) -> &mut u128 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u128>())
    }

    /// Return a reference to the value as u128 bits.
    pub unsafe fn as_u128_bits(&self) -> &[u8; 16] {
        &*(self.data.as_ref().as_ptr().cast::<[u8; 16]>())
    }

    /// Return a mutable reference to the value as u128 bits.
    pub unsafe fn as_u128_bits_mut(&mut self) -> &mut [u8; 16] {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<[u8; 16]>())
    }
}

/// The VM caller-checked "funcref" record, for caller-side signature checking.
///
/// It consists of function pointer(s), a type id to be checked by the
/// caller, and the vmctx closure associated with this function.
#[derive(Debug)]
#[repr(C)]
pub struct VMFuncRef {
    /// Function pointer for this funcref if being called via the "array"
    /// calling convention that `Func::new` et al use.
    pub array_call: VMArrayCallFunction,
    /// Function pointer for this funcref if being called via the calling
    /// convention we use when compiling Wasm.
    pub wasm_call: NonNull<VMWasmCallFunction>,
    // /// Function signature's type id.
    // pub type_index: VMSharedTypeIndex,
    /// The VM state associated with this function.
    pub vmctx: *mut VMContext,
    pub type_index: TypeIndex,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMFunctionImport {
    pub from: *mut VMFuncRef,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMTableImport {
    pub from: *mut VMTableDefinition,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMMemoryImport {
    pub from: *mut VMMemoryDefinition,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMGlobalImport {
    pub from: *mut VMGlobalDefinition,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Clone)]
pub struct VMContextPlan {
    num_imported_funcs: u32,
    num_imported_tables: u32,
    num_imported_memories: u32,
    num_imported_globals: u32,
    num_defined_tables: u32,
    num_defined_memories: u32,
    num_owned_memories: u32,
    num_defined_globals: u32,
    num_escaped_funcs: u32,
    /// target ISA pointer size in bytes
    ptr_size: u32,
    size: u32,

    // offsets
    magic: u32,
    // builtins: u32,
    imported_functions: u32,
    imported_tables: u32,
    imported_memories: u32,
    imported_globals: u32,
    tables: u32,
    memories: u32,
    owned_memories: u32,
    globals: u32,
    func_refs: u32,
    stack_limit: u32,
    last_wasm_exit_fp: u32,
    last_wasm_exit_pc: u32,
    last_wasm_entry_sp: u32,
}

impl VMContextPlan {
    pub fn for_module(isa: &dyn TargetIsa, module: &TranslatedModule) -> Self {
        let mut offset = 0;

        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        let ptr_size = u32::from(isa.pointer_bytes());

        Self {
            num_imported_funcs: module.num_imported_functions(),
            num_imported_tables: module.num_imported_tables(),
            num_imported_memories: module.num_imported_memories(),
            num_imported_globals: module.num_imported_globals(),
            num_defined_tables: module.num_defined_tables(),
            num_defined_memories: module.num_defined_memories(),
            num_owned_memories: module.num_owned_memories(),
            num_defined_globals: module.num_defined_globals(),
            num_escaped_funcs: module.num_escaped_funcs(),
            ptr_size,

            // offsets
            magic: member_offset(ptr_size),
            // builtins: member_offset(ptr_size),
            tables: member_offset(size_of_u32::<VMTableDefinition>() * module.num_defined_tables()),
            memories: member_offset(ptr_size * module.num_defined_memories()),
            owned_memories: member_offset(
                size_of_u32::<VMMemoryDefinition>() * module.num_owned_memories(),
            ),
            globals: member_offset(
                size_of_u32::<VMGlobalDefinition>() * module.num_defined_globals(),
            ),
            func_refs: member_offset(size_of_u32::<VMFuncRef>() * module.num_escaped_funcs()),
            imported_functions: member_offset(
                size_of_u32::<VMFunctionImport>() * module.num_imported_functions(),
            ),
            imported_tables: member_offset(
                size_of_u32::<VMTableImport>() * module.num_imported_tables(),
            ),
            imported_memories: member_offset(
                size_of_u32::<VMMemoryImport>() * module.num_imported_memories(),
            ),
            imported_globals: member_offset(
                size_of_u32::<VMGlobalImport>() * module.num_imported_globals(),
            ),
            stack_limit: member_offset(ptr_size),
            last_wasm_exit_fp: member_offset(ptr_size),
            last_wasm_exit_pc: member_offset(ptr_size),
            last_wasm_entry_sp: member_offset(ptr_size),

            size: offset,
        }
    }

    #[inline]
    pub fn size(&self) -> u32 {
        self.size
    }

    #[inline]
    pub fn num_defined_tables(&self) -> u32 {
        self.num_defined_tables
    }
    #[inline]
    pub fn num_defined_memories(&self) -> u32 {
        self.num_defined_memories
    }
    #[inline]
    pub fn num_owned_memories(&self) -> u32 {
        self.num_owned_memories
    }
    #[inline]
    pub fn num_defined_globals(&self) -> u32 {
        self.num_defined_globals
    }
    #[inline]
    pub fn num_escaped_funcs(&self) -> u32 {
        self.num_escaped_funcs
    }
    #[inline]
    pub fn num_imported_funcs(&self) -> u32 {
        self.num_imported_funcs
    }
    #[inline]
    pub fn num_imported_tables(&self) -> u32 {
        self.num_imported_tables
    }
    #[inline]
    pub fn num_imported_memories(&self) -> u32 {
        self.num_imported_memories
    }
    #[inline]
    pub fn num_imported_globals(&self) -> u32 {
        self.num_imported_globals
    }

    /// Returns the offset of the `VMContext`s `magic` field.
    #[inline]
    pub fn vmctx_magic(&self) -> u32 {
        self.magic
    }
    /// Returns the offset of the `VMContext`s `stack_limit` field.
    #[inline]
    pub fn vmctx_stack_limit(&self) -> u32 {
        self.stack_limit
    }
    /// Returns the offset of the `VMContext`s `last_wasm_exit_fp` field.
    #[inline]
    pub fn vmctx_last_wasm_exit_fp(&self) -> u32 {
        self.last_wasm_exit_fp
    }
    /// Returns the offset of the `VMContext`s `last_wasm_exit_pc` field.
    #[inline]
    pub fn vmctx_last_wasm_exit_pc(&self) -> u32 {
        self.last_wasm_exit_pc
    }
    /// Returns the offset of the `VMContext`s `last_wasm_entry_sp` field.
    #[inline]
    pub fn vmctx_last_wasm_entry_sp(&self) -> u32 {
        self.last_wasm_entry_sp
    }
    /// Returns the offset of the *start* of the `VMContext` `table_definitions` array.
    #[inline]
    pub fn vmctx_table_definitions_start(&self) -> u32 {
        self.tables
    }
    /// Returns the offset of the `VMTableDefinition` given by `index` within `VMContext`s
    /// `table_definitions` array.
    #[inline]
    pub fn vmctx_table_definition(&self, index: DefinedTableIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_tables);
        self.tables + index.as_u32() * size_of_u32::<VMTableDefinition>()
    }
    /// Returns the offset of the *start* of the `VMContext` `memory_pointers` array.
    #[inline]
    pub fn vmctx_memory_pointers_start(&self) -> u32 {
        self.memories
    }
    /// Returns the offset of the `*mut VMMemoryDefinition` given by `index` within `VMContext`s
    /// `memory_pointers` array.
    #[inline]
    pub fn vmctx_memory_pointer(&self, index: DefinedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_memories);
        self.memories + index.as_u32() * self.ptr_size
    }
    /// Returns the offset of the *start* of the `VMContext` `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definitions_start(&self) -> u32 {
        self.owned_memories
    }
    /// Returns the offset of the `VMMemoryDefinition` given by `index` within `VMContext`s
    /// `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definition(&self, index: OwnedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_owned_memories);
        self.owned_memories + index.as_u32() * size_of_u32::<VMMemoryDefinition>()
    }
    /// Return the offset to the `base` field in `VMMemoryDefinition` index `index`.
    #[inline]
    pub fn vmctx_memory_definition_base(&self, index: OwnedMemoryIndex) -> u32 {
        self.vmctx_memory_definition(index) + offset_of!(VMMemoryDefinition, base) as u32
    }

    /// Return the offset to the `current_length` field in `VMMemoryDefinition` index `index`.
    #[inline]
    pub fn vmctx_memory_definition_current_length(&self, index: OwnedMemoryIndex) -> u32 {
        self.vmctx_memory_definition(index) + offset_of!(VMMemoryDefinition, current_length) as u32
    }
    /// Returns the offset of the *start* of the `VMContext` `global_definitions` array.
    #[inline]
    pub fn vmctx_global_definitions_start(&self) -> u32 {
        self.globals
    }
    /// Returns the offset of the `VMGlobalDefinition` given by `index` within `VMContext`s
    /// `global_definitions` array.
    #[inline]
    pub fn vmctx_global_definition(&self, index: DefinedGlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_globals);
        self.globals + index.as_u32() * size_of_u32::<VMGlobalDefinition>()
    }
    /// Returns the offset of the *start* of the `VMContext` `func_refs` array.
    #[inline]
    pub fn vmctx_func_refs_start(&self) -> u32 {
        self.func_refs
    }
    /// Returns the offset of the `VMFuncRef` given by `index` within `VMContext`s
    /// `func_refs` array.
    #[inline]
    pub fn vmctx_func_ref(&self, index: FuncRefIndex) -> u32 {
        assert!(!index.is_reserved_value());
        assert!(index.as_u32() < self.num_escaped_funcs);
        self.func_refs + index.as_u32() * size_of_u32::<VMFuncRef>()
    }
    /// Returns the offset of the *start* of the `VMContext` `function_imports` array.
    #[inline]
    pub fn vmctx_function_imports_start(&self) -> u32 {
        self.imported_functions
    }
    /// Returns the offset of the `VMFunctionImport` given by `index` within `VMContext`s
    /// `function_imports` array.
    #[inline]
    pub fn vmctx_function_import(&self, index: FuncIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_funcs);
        self.imported_functions + index.as_u32() * size_of_u32::<VMFunctionImport>()
    }
    /// Returns the offset of the *start* of the `VMContext` `table_imports` array.
    #[inline]
    pub fn vmctx_table_imports_start(&self) -> u32 {
        self.imported_tables
    }
    /// Returns the offset of the `VMTableImport` given by `index` within `VMContext`s
    /// `table_imports` array.
    #[inline]
    pub fn vmctx_table_import(&self, index: TableIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_tables);
        self.imported_tables + index.as_u32() * size_of_u32::<VMTableImport>()
    }
    /// Returns the offset of the *start* of the `VMContext` `memory_imports` array.
    #[inline]
    pub fn vmctx_memory_imports_start(&self) -> u32 {
        self.imported_memories
    }

    /// Returns the offset of the `VMMemoryImport` given by `index` within `VMContext`s
    /// `memory_imports` array.
    #[inline]
    pub fn vmctx_memory_import(&self, index: MemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_memories);
        self.imported_memories + index.as_u32() * size_of_u32::<VMMemoryImport>()
    }
    #[inline]
    pub(crate) fn vmctx_memory_import_from(&self, index: MemoryIndex) -> u32 {
        self.vmctx_memory_import(index) + offset_of!(VMMemoryImport, from) as u32
    }
    /// Returns the offset of the *start* of the `VMContext` `global_imports` array.
    #[inline]
    pub fn vmctx_global_imports_start(&self) -> u32 {
        self.imported_globals
    }
    /// Returns the offset of the `VMGlobalImport` given by `index` within `VMContext`s
    /// `global_imports` array.
    #[inline]
    pub fn vmctx_global_import(&self, index: GlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_globals);
        self.imported_globals + index.as_u32() * size_of_u32::<VMGlobalImport>()
    }
    #[inline]
    pub fn vmctx_global_import_from(&self, index: GlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_globals);
        self.imported_globals
            + index.as_u32() * size_of_u32::<VMGlobalImport>()
            + offset_of!(VMGlobalImport, from) as u32
    }

    #[inline]
    pub fn vmctx_memory_definition_base_offset(&self) -> u8 {
        u8::try_from(offset_of!(VMMemoryDefinition, base)).unwrap()
    }
}

#[derive(Debug)]
pub struct OwnedVMContext(MmapVec<u8>);

impl OwnedVMContext {
    pub(crate) fn try_new(plan: &VMContextPlan) -> crate::TranslationResult<Self> {
        let vec = MmapVec::new_zeroed(round_usize_up_to_host_pages(plan.size() as usize))?;
        Ok(Self(vec))
    }
    pub(crate) fn as_vmctx(&self) -> *const VMContext {
        self.0.as_ptr().cast()
    }
    pub(crate) fn as_vmctx_mut(&mut self) -> *mut VMContext {
        self.0.as_mut_ptr().cast()
    }
}

/// # Panics
///
/// Panics if the size of `T` is greater than `u32::MAX`.
fn size_of_u32<T: Sized>() -> u32 {
    u32::try_from(mem::size_of::<T>()).unwrap()
}
