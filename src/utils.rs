use crate::translate::TranslationEnvironment;
use core::alloc::Allocator;
use core::hash::{BuildHasher, Hash};
use cranelift_codegen::ir;
use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Signature, Type};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use wasmparser::{FuncType, ValType};

#[macro_export]
macro_rules! enum_accessors {
    ($bind:ident $(($variant:ident($ty:ty) $get:ident $unwrap:ident $cvt:expr))*) => ($(
        /// Attempt to access the underlying value of this `Val`, returning
        /// `None` if it is not the correct type.
        #[inline]
        pub fn $get(&self) -> Option<$ty> {
            if let Self::$variant($bind) = self {
                Some($cvt)
            } else {
                None
            }
        }

        /// Returns the underlying value of this `Val`, panicking if it's the
        /// wrong type.
        ///
        /// # Panics
        ///
        /// Panics if `self` is not of the right type.
        #[inline]
        pub fn $unwrap(&self) -> $ty {
            self.$get().expect(concat!("expected ", stringify!($ty)))
        }
    )*)
}

fn blank_sig(isa: &dyn TargetIsa, call_conv: CallConv) -> Signature {
    let pointer_type = isa.pointer_type();
    let mut sig = Signature::new(call_conv);

    // Add the caller/callee `vmctx` parameters.
    // Add the caller/callee `vmctx` parameters.
    sig.params
        .push(AbiParam::special(pointer_type, ArgumentPurpose::VMContext));
    sig.params.push(AbiParam::new(pointer_type));

    sig
}

pub fn value_type(ty: ValType) -> Type {
    match ty {
        ValType::I32 => ir::types::I32,
        ValType::I64 => ir::types::I64,
        ValType::F32 => ir::types::F32,
        ValType::F64 => ir::types::F64,
        ValType::V128 => ir::types::I8X16,
        // TODO maybe stack map?
        ValType::Ref(_) => todo!(),
    }
}

pub fn wasm_call_signature(isa: &dyn TargetIsa, func_ty: &FuncType) -> Signature {
    let mut sig = blank_sig(isa, CallConv::Fast);

    let cvt = |ty: &ValType| AbiParam::new(value_type(*ty));
    sig.params.extend(func_ty.params().iter().map(&cvt));
    sig.returns.extend(func_ty.results().iter().map(&cvt));

    sig
}

#[allow(unused)]
pub fn native_call_signature(isa: &dyn TargetIsa, wasm_func_ty: &FuncType) -> Signature {
    let mut sig = blank_sig(isa, CallConv::triple_default(isa.triple()));

    let cvt = |ty: &ValType| AbiParam::new(value_type(*ty));
    sig.params.extend(wasm_func_ty.params().iter().map(&cvt));
    sig.returns.extend(wasm_func_ty.results().iter().map(&cvt));

    sig
}

/// Get the Cranelift signature for all array-call functions, that is:
///
/// ```ignore
/// unsafe extern "C" fn(
///     callee_vmctx: *mut VMOpaqueContext,
///     caller_vmctx: *mut VMOpaqueContext,
///     values_ptr: *mut ValRaw,
///     values_len: usize,
/// )
/// ```
///
/// This signature uses the target's default calling convention.
///
/// Note that regardless of the Wasm function type, the array-call calling
/// convention always uses that same signature.
pub fn array_call_signature(isa: &dyn TargetIsa) -> ir::Signature {
    let mut sig = blank_sig(isa, CallConv::triple_default(isa.triple()));
    // The array-call signature has an added parameter for the `values_vec`
    // input/output buffer in addition to the size of the buffer, in units
    // of `ValRaw`.
    sig.params.push(AbiParam::new(isa.pointer_type()));
    sig.params.push(AbiParam::new(isa.pointer_type()));
    sig
}

pub(crate) trait HashMapEntryTryExt<'a, K, V, S>: Sized {
    fn or_try_insert_with<E, F: FnOnce() -> Result<V, E>>(self, default: F) -> Result<&'a mut V, E>
    where
        K: Hash,
        S: BuildHasher;
}

impl<'a, K, V, S, A: Allocator> HashMapEntryTryExt<'a, K, V, S>
    for hashbrown::hash_map::Entry<'a, K, V, S, A>
{
    fn or_try_insert_with<E, F: FnOnce() -> Result<V, E>>(self, default: F) -> Result<&'a mut V, E>
    where
        K: Hash,
        S: BuildHasher,
    {
        match self {
            hashbrown::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            hashbrown::hash_map::Entry::Vacant(entry) => Ok(entry.insert(default()?)),
        }
    }
}
