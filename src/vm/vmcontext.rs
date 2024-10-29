//! `VMContext` and other data structures that are directly access by JIT code.
//!
//! As a naming convention all types that start with `VM` are types that are used by the JIT code.
//! All of which are marked as `#[repr(C)]` to have a stable ABI.
//!
//! # Safety
//!
//! It shouldn't have to be said, but since all structs in this module are exposed to JIT code
//! **accessing them is highly unsafe**. All methods exposed by these types are marked unsafe for
//! a reason, and you should **never** use access them as-is in kernel code. Especially pointers read
//! from these structs should be checked and validated before being dereferenced.
//!
//! # [`VMContext`]
//!
//! The major data structure that is passed to all JIT-compiled functions. It contains all the
//! guest-side state that the JIT code needs to access, including globals, table pointers,
//! memory pointers, and other bits of runtime info. For more details see [`VMContext`].
//!
//! This is essentially the guest-side counterpart to the [`crate::vm::instance::Instance`] struct.
//!
//! ```rust,ignore
//! struct VMContext {
//!     magic: u32,
//!     _padding: u32, // (On 64-bit systems)
//!     builtin_functions: *mut VMBuiltinFunctionsArray,
//!     last_wasm_exit_fp: u32,
//!     last_wasm_exit_pc: u32,
//!     last_wasm_entry_sp: u32,
//!     imported_functions: [VMFunctionImport; module.num_imported_functions],
//!     imported_tables: [VMTableImport; module.num_imported_tables],
//!     imported_memories: [VMMemoryImport; module.num_imported_memories],
//!     imported_globals: [VMGlobalImport; module.num_imported_globals],
//!     func_refs: [VMFuncRef; module.num_escaped_funcs],
//!     tables: [VMTableDefinition; module.num_defined_tables],
//!     memories: [VMMemoryDefinition; module.num_defined_memories],
//!     globals: [VMGlobalDefinition; module.num_defined_globals],
//! }
//! ```

use crate::indices::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, FuncRefIndex,
    GlobalIndex, MemoryIndex, TableIndex, TypeIndex,
};
use crate::parse::ParsedModule;
use crate::utils::round_usize_up_to_host_pages;
use crate::vm::MmapVec;
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
        VMVal::i64(i as i64)
    }
    #[inline]
    pub fn i64(i: i64) -> VMVal {
        VMVal { i64: i.to_le() }
    }
    #[inline]
    pub fn u32(i: u32) -> VMVal {
        VMVal::u64(i as u64)
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
    pub host_call: VMArrayCallFunction,
    /// Function pointer for this funcref if being called via the calling
    /// convention we use when compiling Wasm.
    pub wasm_call: NonNull<VMWasmCallFunction>,
    // /// Function signature's type id.
    // pub type_index: VMSharedTypeIndex,
    /// The VM state associated with this function.
    pub vmctx: *mut VMOpaqueContext,
    pub type_index: TypeIndex,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMFunctionImport {
    /// Function pointer to use when calling this imported function from Wasm.
    pub wasm_call: NonNull<VMWasmCallFunction>,
    /// Function pointer to use when calling this imported function with the
    /// "array" calling convention that `Func::new` et al use.
    pub host_call: VMArrayCallFunction,
    /// The VM state associated with this function.
    ///
    /// For Wasm functions defined by core wasm instances this will be `*mut
    /// VMContext`, but for lifted/lowered component model functions this will
    /// be a `VMComponentContext`, and for a host function it will be a
    /// `VMHostFuncContext`, etc.
    pub vmctx: *mut VMOpaqueContext,
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

/// The VM "context", which holds guest-side instance state such as
/// globals, table pointers, memory pointers and other runtime information.
///
/// This struct is empty since the size of fields within `VMContext` is dynamic and
/// therefore can't be described by Rust's type system. The exact shape of an instances `VMContext`
/// is described by its `VMContextPlan` which lets you convert entity indices into `VMContext`-relative
/// offset for use in JIT code. For a higher-level access to these fields see the `Instance` methods.
#[derive(Debug)]
#[repr(C, align(16))] // align 16 since globals are aligned to that and contained inside
pub struct VMContext {
    _m: PhantomPinned,
}

impl VMContext {
    /// Helper function to cast between context types using a debug assertion to
    /// protect against some mistakes.
    #[inline]
    pub unsafe fn from_opaque(opaque: *mut VMOpaqueContext) -> *mut VMContext {
        // Note that in general the offset of the "magic" field is stored in
        // `VMOffsets::vmctx_magic`. Given though that this is a sanity check
        // about converting this pointer to another type we ideally don't want
        // to read the offset from potentially corrupt memory. Instead it would
        // be better to catch errors here as soon as possible.
        //
        // To accomplish this the `VMContext` structure is laid out with the
        // magic field at a statically known offset (here it's 0 for now). This
        // static offset is asserted in `VMOffsets::from` and needs to be kept
        // in sync with this line for this debug assertion to work.
        //
        // Also note that this magic is only ever invalid in the presence of
        // bugs, meaning we don't actually read the magic and act differently
        // at runtime depending what it is, so this is a debug assertion as
        // opposed to a regular assertion.
        debug_assert_eq!((*opaque).magic, VMCONTEXT_MAGIC);
        opaque.cast()
    }
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
#[derive(Debug)]
#[repr(C, align(16))]
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

#[derive(Debug)]
pub struct OwnedVMContext(MmapVec<u8>);

impl OwnedVMContext {
    pub(crate) fn try_new(plan: &VMContextPlan) -> crate::Result<Self> {
        let vec = MmapVec::new_zeroed(round_usize_up_to_host_pages(plan.size() as usize))?;
        Ok(Self(vec))
    }
    pub(crate) fn as_ptr(&self) -> *const VMContext {
        self.0.as_ptr().cast()
    }
    pub(crate) fn as_mut_ptr(&mut self) -> *mut VMContext {
        self.0.as_mut_ptr().cast()
    }

    pub(crate) unsafe fn plus_offset<T>(&self, offset: u32) -> *const T {
        self.as_ptr()
            .byte_add(usize::try_from(offset).unwrap())
            .cast()
    }

    pub(crate) unsafe fn plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        self.as_mut_ptr()
            .byte_add(usize::try_from(offset).unwrap())
            .cast()
    }
}

#[derive(Debug, Clone)]
pub struct FixedVMContextPlan {
    magic: u32,
    builtin_functions: u32,
    /// The current stack limit.
    /// TODO clarify what this means
    pub stack_limit: u32,

    /// The value of the frame pointer register when we last called from Wasm to
    /// the host.
    ///
    /// Maintained by our Wasm-to-host trampoline, and cleared just before
    /// calling into Wasm in `catch_traps`.
    ///
    /// This member is `0` when Wasm is actively running and has not called out
    /// to the host.
    ///
    /// Used to find the start of a contiguous sequence of Wasm frames when
    /// walking the stack.
    pub last_wasm_exit_fp: u32,

    /// The last Wasm program counter before we called from Wasm to the host.
    ///
    /// Maintained by our Wasm-to-host trampoline, and cleared just before
    /// calling into Wasm in `catch_traps`.
    ///
    /// This member is `0` when Wasm is actively running and has not called out
    /// to the host.
    ///
    /// Used when walking a contiguous sequence of Wasm frames.
    pub last_wasm_exit_pc: u32,

    /// The last host stack pointer before we called into Wasm from the host.
    ///
    /// Maintained by our host-to-Wasm trampoline, and cleared just before
    /// calling into Wasm in `catch_traps`.
    ///
    /// This member is `0` when Wasm is actively running and has not called out
    /// to the host.
    ///
    /// When a host function is wrapped into a `wasmtime::Func`, and is then
    /// called from the host, then this member has the sentinel value of `-1 as
    /// usize`, meaning that this contiguous sequence of Wasm frames is the
    /// empty sequence, and it is not safe to dereference the
    /// `last_wasm_exit_fp`.
    ///
    /// Used to find the end of a contiguous sequence of Wasm frames when
    /// walking the stack.
    pub last_wasm_entry_fp: u32,

    size: u32,
}

impl FixedVMContextPlan {
    pub fn new(isa: &dyn TargetIsa) -> Self {
        let ptr_size = u32::from(isa.pointer_bytes());

        let mut offset = 0;
        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        Self {
            magic: member_offset(ptr_size),
            builtin_functions: member_offset(ptr_size),
            stack_limit: member_offset(ptr_size),
            last_wasm_exit_fp: member_offset(ptr_size),
            last_wasm_exit_pc: member_offset(ptr_size),
            last_wasm_entry_fp: member_offset(ptr_size),
            size: offset,
        }
    }

    /// Returns the offset of the `VMContext`s `magic` field.
    #[inline]
    pub fn vmctx_magic(&self) -> u32 {
        self.magic
    }
    /// Returns the offset of the `VMContext`s `builtin_functions` field.
    #[inline]
    pub fn vmctx_builtin_functions(&self) -> u32 {
        self.builtin_functions
    }
    /// Returns the offset of the `VMContext`s `last_wasm_exit_fp` field.
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
    /// Returns the offset of the `VMContext`s `last_wasm_entry_fp` field.
    #[inline]
    pub fn vmctx_last_wasm_entry_fp(&self) -> u32 {
        self.last_wasm_entry_fp
    }
}

#[derive(Debug, Clone)]
pub struct VMContextPlan {
    num_imported_funcs: u32,
    num_imported_tables: u32,
    num_imported_memories: u32,
    num_imported_globals: u32,
    num_defined_tables: u32,
    num_defined_memories: u32,
    num_defined_globals: u32,
    num_escaped_funcs: u32,
    /// target ISA pointer size in bytes
    // ptr_size: u32,
    size: u32,

    // offsets
    pub fixed: FixedVMContextPlan,
    func_refs: u32,
    imported_functions: u32,
    imported_tables: u32,
    imported_memories: u32,
    imported_globals: u32,
    tables: u32,
    memories: u32,
    globals: u32,
}

impl VMContextPlan {
    pub fn for_module(isa: &dyn TargetIsa, module: &ParsedModule) -> Self {
        let ptr_size = u32::from(isa.pointer_bytes());
        let fixed = FixedVMContextPlan::new(isa);

        let mut offset = fixed.size;
        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        Self {
            num_imported_funcs: module.num_imported_functions(),
            num_imported_tables: module.num_imported_tables(),
            num_imported_memories: module.num_imported_memories(),
            num_imported_globals: module.num_imported_globals(),
            num_defined_tables: module.num_defined_tables(),
            num_defined_memories: module.num_defined_memories(),
            num_defined_globals: module.num_defined_globals(),
            num_escaped_funcs: module.num_escaped_funcs(),

            // offsets
            fixed,
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
            tables: member_offset(size_of_u32::<VMTableDefinition>() * module.num_defined_tables()),
            memories: member_offset(ptr_size * module.num_defined_memories()),
            globals: member_offset(
                size_of_u32::<VMGlobalDefinition>() * module.num_defined_globals(),
            ),

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
    /// Returns the offset of the *start* of the `VMContext` `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definitions_start(&self) -> u32 {
        self.memories
    }
    /// Returns the offset of the `VMMemoryDefinition` given by `index` within `VMContext`s
    /// `memory_definitions` array.
    #[inline]
    pub fn vmctx_memory_definition(&self, index: DefinedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_memories);
        self.memories + index.as_u32() * size_of_u32::<VMMemoryDefinition>()
    }
    /// Return the offset to the `base` field in `VMMemoryDefinition` index `index`.
    #[inline]
    pub fn vmctx_memory_definition_base(&self, index: DefinedMemoryIndex) -> u32 {
        self.vmctx_memory_definition(index) + offset_of!(VMMemoryDefinition, base) as u32
    }

    /// Return the offset to the `current_length` field in `VMMemoryDefinition` index `index`.
    #[inline]
    pub fn vmctx_memory_definition_current_length(&self, index: DefinedMemoryIndex) -> u32 {
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
    /// Returns the offset of the `VMFunctionImport`s `vmctx` fields given by `index` within `VMContext`s
    /// `function_imports` array.
    pub(crate) fn vmctx_function_import_vmctx(&self, index: FuncIndex) -> u32 {
        self.vmctx_function_import(index) + offset_of!(VMFunctionImport, vmctx) as u32
    }
    /// Returns the offset of the `VMFunctionImport`s `vmctx` fields given by `index` within `VMContext`s
    /// `function_imports` array.
    pub(crate) fn vmctx_function_import_wasm_call(&self, index: FuncIndex) -> u32 {
        self.vmctx_function_import(index) + offset_of!(VMFunctionImport, wasm_call) as u32
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

/// # Panics
///
/// Panics if the size of `T` is greater than `u32::MAX`.
fn size_of_u32<T: Sized>() -> u32 {
    u32::try_from(mem::size_of::<T>()).unwrap()
}
