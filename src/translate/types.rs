use crate::enum_accessors;
use crate::indices::SharedOrModuleTypeIndex;
use alloc::boxed::Box;
use core::fmt;
use crate::translate::{GlobalType, MemoryPlan, TablePlan};

/// Represents the types of values in a WebAssembly module.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmValType {
    /// The value type is i32.
    I32,
    /// The value type is i64.
    I64,
    /// The value type is f32.
    F32,
    /// The value type is f64.
    F64,
    /// The value type is v128.
    V128,
    /// The value type is a reference.
    Ref(WasmRefType),
}

impl fmt::Display for WasmValType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WasmValType::I32 => write!(f, "i32"),
            WasmValType::I64 => write!(f, "i64"),
            WasmValType::F32 => write!(f, "f32"),
            WasmValType::F64 => write!(f, "f64"),
            WasmValType::V128 => write!(f, "v128"),
            WasmValType::Ref(rt) => write!(f, "{rt}"),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmRefType {
    pub nullable: bool,
    pub heap_type: WasmHeapType,
}

impl WasmRefType {
    pub const EXTERNREF: WasmRefType = WasmRefType {
        nullable: true,
        heap_type: WasmHeapType::new(false, WasmHeapTypeInner::Extern),
    };
    pub const FUNCREF: WasmRefType = WasmRefType {
        nullable: true,
        heap_type: WasmHeapType::new(false, WasmHeapTypeInner::Func),
    };
}

impl fmt::Display for WasmRefType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::FUNCREF => write!(f, "funcref"),
            Self::EXTERNREF => write!(f, "externref"),
            _ => {
                if self.nullable {
                    write!(f, "(ref null {})", self.heap_type)
                } else {
                    write!(f, "(ref {})", self.heap_type)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmHeapType {
    pub shared: bool,
    pub ty: WasmHeapTypeInner,
}

impl WasmHeapType {
    pub(crate) const fn new(shared: bool, ty: WasmHeapTypeInner) -> Self {
        Self { shared, ty }
    }
    pub(crate) fn top(&self) -> WasmHeapTopType {
        let inner = match self.ty {
            WasmHeapTypeInner::Extern | WasmHeapTypeInner::NoExtern => WasmHeapTopTypeInner::Extern,
            WasmHeapTypeInner::Func
            | WasmHeapTypeInner::ConcreteFunc(_)
            | WasmHeapTypeInner::NoFunc => WasmHeapTopTypeInner::Extern,
            WasmHeapTypeInner::Any
            | WasmHeapTypeInner::Eq
            | WasmHeapTypeInner::I31
            | WasmHeapTypeInner::Array
            | WasmHeapTypeInner::ConcreteArray(_)
            | WasmHeapTypeInner::Struct
            | WasmHeapTypeInner::ConcreteStruct(_)
            | WasmHeapTypeInner::None => WasmHeapTopTypeInner::Any,
            WasmHeapTypeInner::Exn | WasmHeapTypeInner::NoExn => WasmHeapTopTypeInner::Exn,
            WasmHeapTypeInner::Cont | WasmHeapTypeInner::NoCont => WasmHeapTopTypeInner::Cont,
        };
        WasmHeapTopType {
            shared: self.shared,
            inner,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmHeapTypeInner {
    // External types.
    Extern,
    NoExtern,

    // Function types.
    Func,
    ConcreteFunc(SharedOrModuleTypeIndex),
    NoFunc,

    // Internal types.
    Any,
    Eq,
    I31,
    Array,
    ConcreteArray(SharedOrModuleTypeIndex),
    Struct,
    ConcreteStruct(SharedOrModuleTypeIndex),
    None,

    Exn,
    NoExn,

    Cont,
    NoCont,
}

impl fmt::Display for WasmHeapType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.shared {
            write!(f, "shared ")?;
        }
        match &self.ty {
            WasmHeapTypeInner::Extern => write!(f, "extern"),
            WasmHeapTypeInner::NoExtern => write!(f, "noextern"),
            WasmHeapTypeInner::Func => write!(f, "func"),
            WasmHeapTypeInner::ConcreteFunc(i) => write!(f, "func {i:?}"),
            WasmHeapTypeInner::NoFunc => write!(f, "nofunc"),
            WasmHeapTypeInner::Any => write!(f, "any"),
            WasmHeapTypeInner::Eq => write!(f, "eq"),
            WasmHeapTypeInner::I31 => write!(f, "i31"),
            WasmHeapTypeInner::Array => write!(f, "array"),
            WasmHeapTypeInner::ConcreteArray(i) => write!(f, "array {i:?}"),
            WasmHeapTypeInner::Struct => write!(f, "struct"),
            WasmHeapTypeInner::ConcreteStruct(i) => write!(f, "struct {i:?}"),
            WasmHeapTypeInner::None => write!(f, "none"),
            WasmHeapTypeInner::Exn => write!(f, "exn"),
            WasmHeapTypeInner::NoExn => write!(f, "noexn"),
            WasmHeapTypeInner::Cont => write!(f, "cont"),
            WasmHeapTypeInner::NoCont => write!(f, "nocont"),
        }
    }
}

pub struct WasmHeapTopType {
    pub shared: bool,
    pub inner: WasmHeapTopTypeInner,
}

pub enum WasmHeapTopTypeInner {
    /// The common supertype of all external references.
    Extern,
    /// The common supertype of all internal references.
    Any,
    /// The common supertype of all function references.
    Func,
    /// The common supertype of all exception references.
    Exn,
    /// The common supertype of all continuation references.
    Cont,
}

/// A concrete, user-defined (or host-defined) Wasm type.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmSubType {
    /// Whether this type is forbidden from being the supertype of any other
    /// type.
    pub is_final: bool,

    /// This type's supertype, if any.
    pub supertype: Option<SharedOrModuleTypeIndex>,

    /// The array, function, or struct that is defined.
    pub composite_type: WasmCompositeType,
}

impl WasmSubType {
    #[inline]
    pub fn is_func(&self) -> bool {
        self.composite_type.is_func()
    }

    #[inline]
    pub fn as_func(&self) -> Option<&WasmFuncType> {
        self.composite_type.as_func()
    }

    #[inline]
    pub fn unwrap_func(&self) -> &WasmFuncType {
        self.composite_type.unwrap_func()
    }

    #[inline]
    pub fn is_array(&self) -> bool {
        self.composite_type.is_array()
    }

    #[inline]
    pub fn as_array(&self) -> Option<&WasmArrayType> {
        self.composite_type.as_array()
    }

    #[inline]
    pub fn unwrap_array(&self) -> &WasmArrayType {
        self.composite_type.unwrap_array()
    }

    #[inline]
    pub fn is_struct(&self) -> bool {
        self.composite_type.is_struct()
    }

    #[inline]
    pub fn as_struct(&self) -> Option<&WasmStructType> {
        self.composite_type.as_struct()
    }

    #[inline]
    pub fn unwrap_struct(&self) -> &WasmStructType {
        self.composite_type.unwrap_struct()
    }
}

impl fmt::Display for WasmSubType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_final && self.supertype.is_none() {
            fmt::Display::fmt(&self.composite_type, f)
        } else {
            write!(f, "(sub")?;
            if self.is_final {
                write!(f, " final")?;
            }
            if let Some(sup) = self.supertype {
                write!(f, " {sup:?}")?;
            }
            write!(f, " {})", self.composite_type)
        }
    }
}

/// A function, array, or struct type.
///
/// Introduced by the GC proposal.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmCompositeType {
    pub inner: WasmCompositeTypeInner,
    /// Is the composite type shared? This is part of the
    /// shared-everything-threads proposal.
    pub shared: bool,
}

impl WasmCompositeType {
    pub(crate) fn new_func(shared: bool, ty: WasmFuncType) -> WasmCompositeType {
        Self {
            shared,
            inner: WasmCompositeTypeInner::Func(ty),
        }
    }
    pub(crate) fn new_array(shared: bool, ty: WasmArrayType) -> WasmCompositeType {
        Self {
            shared,
            inner: WasmCompositeTypeInner::Array(ty),
        }
    }
    pub(crate) fn new_struct(shared: bool, ty: WasmStructType) -> WasmCompositeType {
        Self {
            shared,
            inner: WasmCompositeTypeInner::Struct(ty),
        }
    }
    #[inline]
    pub fn is_func(&self) -> bool {
        self.inner.is_func()
    }

    #[inline]
    pub fn as_func(&self) -> Option<&WasmFuncType> {
        self.inner.as_func()
    }

    #[inline]
    pub fn unwrap_func(&self) -> &WasmFuncType {
        self.inner.unwrap_func()
    }

    #[inline]
    pub fn is_array(&self) -> bool {
        self.inner.is_array()
    }

    #[inline]
    pub fn as_array(&self) -> Option<&WasmArrayType> {
        self.inner.as_array()
    }

    #[inline]
    pub fn unwrap_array(&self) -> &WasmArrayType {
        self.inner.unwrap_array()
    }

    #[inline]
    pub fn is_struct(&self) -> bool {
        self.inner.is_struct()
    }

    #[inline]
    pub fn as_struct(&self) -> Option<&WasmStructType> {
        self.inner.as_struct()
    }

    #[inline]
    pub fn unwrap_struct(&self) -> &WasmStructType {
        self.inner.unwrap_struct()
    }
}

impl fmt::Display for WasmCompositeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.shared {
            write!(f, "shared ")?;
        }
        match &self.inner {
            WasmCompositeTypeInner::Func(ty) => fmt::Display::fmt(ty, f),
            WasmCompositeTypeInner::Array(ty) => fmt::Display::fmt(ty, f),
            WasmCompositeTypeInner::Struct(ty) => fmt::Display::fmt(ty, f),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmCompositeTypeInner {
    /// The type is a regular function.
    Func(WasmFuncType),
    /// The type is a GC-proposal array.
    Array(WasmArrayType),
    /// The type is a GC-proposal struct.
    Struct(WasmStructType),
}

impl WasmCompositeTypeInner {
    enum_accessors! {
        c
        (Func(&WasmFuncType) is_func as_func unwrap_func c)
        (Array(&WasmArrayType) is_array as_array unwrap_array c)
        (Struct(&WasmStructType) is_struct as_struct unwrap_struct c)
    }
}

/// A WebAssembly function type.
///
/// This is the equivalent of `wasmparser::FuncType`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmFuncType {
    pub params: Box<[WasmValType]>,
    pub results: Box<[WasmValType]>,
}

impl fmt::Display for WasmFuncType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(func")?;
        if !self.params.is_empty() {
            write!(f, " (param")?;
            for p in self.params.iter() {
                write!(f, " {p}")?;
            }
            write!(f, ")")?;
        }
        if !self.results.is_empty() {
            write!(f, " (result")?;
            for r in self.results.iter() {
                write!(f, " {r}")?;
            }
            write!(f, ")")?;
        }
        write!(f, ")")
    }
}

/// A WebAssembly GC-proposal Array type.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmArrayType(pub WasmFieldType);

impl fmt::Display for WasmArrayType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(array {})", self.0)
    }
}

/// A WebAssembly GC-proposal struct type.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmStructType {
    pub fields: Box<[WasmFieldType]>,
}

impl fmt::Display for WasmStructType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(struct")?;
        for ty in self.fields.iter() {
            write!(f, " {ty}")?;
        }
        write!(f, ")")
    }
}

/// The type of struct field or array element.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmFieldType {
    /// Whether this field can be mutated or not.
    pub mutable: bool,
    /// The field's element type.
    pub element_type: WasmStorageType,
}

impl fmt::Display for WasmFieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mutable {
            write!(f, "(mut {})", self.element_type)
        } else {
            fmt::Display::fmt(&self.element_type, f)
        }
    }
}

/// A WebAssembly GC-proposal storage type for Array and Struct fields.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmStorageType {
    /// The storage type is i8.
    I8,
    /// The storage type is i16.
    I16,
    /// The storage type is a value type.
    Val(WasmValType),
}

impl fmt::Display for WasmStorageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmStorageType::I8 => write!(f, "i8"),
            WasmStorageType::I16 => write!(f, "i16"),
            WasmStorageType::Val(v) => fmt::Display::fmt(v, f),
        }
    }
}

#[derive(Debug)]
pub enum EntityType {
    Function(SharedOrModuleTypeIndex),
    Table(TablePlan),
    Memory(MemoryPlan),
    Global(GlobalType),
}
