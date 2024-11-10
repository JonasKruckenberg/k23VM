use crate::func::Func;
use crate::runtime::VMVal;
use crate::translate::{WasmHeapTopTypeInner, WasmHeapType, WasmValType};
use crate::{enum_accessors, Store};
use core::ptr;

/// A reference value that a WebAssembly module can consume or produce.
#[derive(Debug, Clone, Copy)]
pub enum Val {
    /// A 32-bit integer.
    I32(i32),
    /// A 64-bit integer.
    I64(i64),
    /// A 32-bit float.
    ///
    /// Note that the raw bits of the float are stored here, and you can use
    /// `f32::from_bits` to create an `f32` value.
    F32(u32),
    /// A 64-bit float.
    ///
    /// Note that the raw bits of the float are stored here, and you can use
    /// `f64::from_bits` to create an `f64` value.
    F64(u64),
    /// A 128-bit number.
    V128(u128),
    /// A function reference.
    FuncRef(Option<Func>),
    // /// An external reference.
    // ExternRef(Option<Rooted<ExternRef>>),
    // /// An internal reference.
    // AnyRef(Option<Rooted<AnyRef>>),
}

impl Val {
    /// Returns the null reference for the given heap type.
    #[inline]
    pub fn null_ref(heap_type: &WasmHeapType) -> Self {
        Ref::null(heap_type).into()
    }

    /// Returns the null function reference value.
    ///
    /// The return value has type `(ref null nofunc)` aka `nullfuncref` and is a
    /// subtype of all function references.
    #[inline]
    pub const fn null_func_ref() -> Self {
        Self::FuncRef(None)
    }

    /// Convenience method to convert this [`Val`] into a [`ValRaw`].
    ///
    /// # Unsafety
    ///
    /// This method is unsafe for the reasons that [`ExternRef::to_raw`] and
    /// [`Func::to_raw`] are unsafe.
    pub unsafe fn as_vmval(&self, store: &mut Store) -> crate::Result<VMVal> {
        match self {
            Val::I32(i) => Ok(VMVal::i32(*i)),
            Val::I64(i) => Ok(VMVal::i64(*i)),
            Val::F32(u) => Ok(VMVal::f32(*u)),
            Val::F64(u) => Ok(VMVal::f64(*u)),
            Val::V128(b) => Ok(VMVal::v128(*b)),
            Val::FuncRef(f) => Ok(VMVal::funcref(match f {
                Some(f) => f.to_raw(store),
                None => ptr::null_mut(),
            })),
        }
    }

    /// Convenience method to convert a [`ValRaw`] into a [`Val`].
    pub unsafe fn from_vmval(_store: &mut Store, raw: VMVal, ty: WasmValType) -> Self {
        match ty {
            WasmValType::I32 => Self::I32(raw.get_i32()),
            WasmValType::I64 => Self::I64(raw.get_i64()),
            WasmValType::F32 => Self::F32(raw.get_f32()),
            WasmValType::F64 => Self::F64(raw.get_f64()),
            WasmValType::V128 => Self::V128(raw.get_v128()),
            WasmValType::Ref(_) => todo!(),
        }
    }

    enum_accessors! {
        e
        (I32(i32) is_i32 i32 unwrap_i32 *e)
        (I64(i64) is_i64 i64 unwrap_i64 *e)
        (F32(f32) is_f32 f32 unwrap_f32 f32::from_bits(*e))
        (F64(f64) is_f64 f64 unwrap_f64 f64::from_bits(*e))
        (V128(u128) is_v128 v128 unwrap_v128 *e)
    }
}

impl From<i32> for Val {
    #[inline]
    fn from(val: i32) -> Val {
        Val::I32(val)
    }
}

impl From<i64> for Val {
    #[inline]
    fn from(val: i64) -> Val {
        Val::I64(val)
    }
}

impl From<f32> for Val {
    #[inline]
    fn from(val: f32) -> Val {
        Val::F32(val.to_bits())
    }
}

impl From<f64> for Val {
    #[inline]
    fn from(val: f64) -> Val {
        Val::F64(val.to_bits())
    }
}

impl From<Ref> for Val {
    #[inline]
    fn from(val: Ref) -> Val {
        match val {
            Ref::Func(f) => Val::FuncRef(f),
        }
    }
}

/// A reference value that a WebAssembly module can consume or produce.
pub enum Ref {
    /// A function reference.
    Func(Option<Func>),
}

impl Ref {
    /// Returns the null reference for the given heap type.
    #[inline]
    pub fn null(heap_type: &WasmHeapType) -> Self {
        match heap_type.top().inner {
            WasmHeapTopTypeInner::Func => Ref::Func(None),
            ty => todo!("heap type: {ty:?}"),
        }
    }

    /// Is this a null reference?
    #[inline]
    pub fn is_null(&self) -> bool {
        match self {
            Self::Func(None) => true,
            Self::Func(Some(_)) => false,
        }
    }

    /// Is this a non-null reference?
    #[inline]
    pub fn is_non_null(&self) -> bool {
        !self.is_null()
    }
}

#[allow(irrefutable_let_patterns)] // bc we only have one enum variant rn
impl Ref {
    enum_accessors! {
        e
        (Func(&Option<Func>) is_func get_func unwrap_func e)
    }
}
