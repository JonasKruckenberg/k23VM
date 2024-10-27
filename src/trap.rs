use object::{Bytes, LittleEndian, U32};

/// Trap code used for debug assertions we emit in our JIT code.
pub const DEBUG_ASSERT_TRAP_CODE: u16 = u16::MAX;

#[derive(onlyerror::Error, Debug)]
pub enum Trap {
    /// The current stack space was exhausted.
    #[error("call stack exhausted")]
    StackOverflow,
    /// An out-of-bounds memory access.
    #[error("out of bounds memory access")]
    MemoryOutOfBounds,
    /// A wasm atomic operation was presented with a not-naturally-aligned linear-memory address.
    #[error("unaligned atomic operation")]
    HeapMisaligned,
    /// Out-of-bounds access to a table.
    #[error("out of bounds table access")]
    TableOutOfBounds,
    /// Indirect call to a null table entry.
    #[error("accessed uninitialized table element")]
    IndirectCallToNull,
    /// Signature mismatch on indirect call.
    #[error("indirect call signature mismatch")]
    BadSignature,
    /// An integer arithmetic operation caused an overflow.
    #[error("integer overflow")]
    IntegerOverflow,
    /// An integer division by zero.
    #[error("integer division by zero")]
    IntegerDivisionByZero,
    /// Failed float-to-int conversion.
    #[error("invalid conversion to integer")]
    BadConversionToInteger,
    /// Code that was supposed to have been unreachable was reached.
    #[error("unreachable code executed")]
    UnreachableCodeReached,
    /// Used to indicate that a trap was raised by atomic wait operations on non shared memory.
    #[error("atomic wait on non-shared memory")]
    AtomicWaitNonSharedMemory,
    /// Call to a null reference.
    #[error("null reference called")]
    NullReference,
    /// Attempt to get the bits of a null `i31ref`.
    #[error("null i32 reference called")]
    NullI31Ref,
    /// Debug assertion failed
    #[error("debug assertion failed")]
    DebugAssertionFailed,
}

impl From<Trap> for u8 {
    fn from(value: Trap) -> Self {
        match value {
            Trap::StackOverflow => 0,
            Trap::MemoryOutOfBounds => 1,
            Trap::HeapMisaligned => 2,
            Trap::TableOutOfBounds => 3,
            Trap::IndirectCallToNull => 4,
            Trap::BadSignature => 5,
            Trap::IntegerOverflow => 6,
            Trap::IntegerDivisionByZero => 7,
            Trap::BadConversionToInteger => 8,
            Trap::UnreachableCodeReached => 9,
            Trap::AtomicWaitNonSharedMemory => 10,
            Trap::NullReference => 11,
            Trap::NullI31Ref => 12,
            Trap::DebugAssertionFailed => 13,
        }
    }
}

impl TryFrom<u8> for Trap {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::StackOverflow),
            1 => Ok(Self::MemoryOutOfBounds),
            2 => Ok(Self::HeapMisaligned),
            3 => Ok(Self::TableOutOfBounds),
            4 => Ok(Self::IndirectCallToNull),
            5 => Ok(Self::BadSignature),
            6 => Ok(Self::IntegerOverflow),
            7 => Ok(Self::IntegerDivisionByZero),
            8 => Ok(Self::BadConversionToInteger),
            9 => Ok(Self::UnreachableCodeReached),
            10 => Ok(Self::AtomicWaitNonSharedMemory),
            11 => Ok(Self::NullReference),
            12 => Ok(Self::NullI31Ref),
            13 => Ok(Self::DebugAssertionFailed),
            _ => Err(()),
        }
    }
}

#[allow(unused)]
pub fn trap_for_offset(trap_section: &[u8], offset: u32) -> Option<Trap> {
    let mut section = Bytes(trap_section);

    let count = section.read::<U32<LittleEndian>>().unwrap();
    let offsets = section
        .read_slice::<U32<LittleEndian>>(count.get(LittleEndian) as usize)
        .unwrap();
    let traps = section.read_slice::<u8>(offsets.len()).unwrap();

    let index = offsets
        .binary_search_by_key(&offset, |val| val.get(LittleEndian))
        .ok()?;

    Trap::try_from(traps[index]).ok()
}

pub mod signals {
    use crate::code_memory::CodeMemory;
    use std::collections::BTreeMap;
    use std::sync::{Arc, OnceLock, RwLock};

    fn global_code() -> &'static RwLock<GlobalRegistry> {
        static GLOBAL_CODE: OnceLock<RwLock<GlobalRegistry>> = OnceLock::new();
        GLOBAL_CODE.get_or_init(Default::default)
    }

    type GlobalRegistry = BTreeMap<usize, (usize, Arc<CodeMemory>)>;

    /// Find which registered region of code contains the given program counter, and
    /// what offset that PC is within that module's code.
    pub fn lookup_code(pc: usize) -> Option<(Arc<CodeMemory>, usize)> {
        let all_modules = global_code().read().unwrap();

        let (_end, (start, module)) = all_modules.range(pc..).next()?;
        let text_offset = pc.checked_sub(*start)?;
        Some((module.clone(), text_offset))
    }

    /// Registers a new region of code.
    ///
    /// Must not have been previously registered and must be `unregister`'d to
    /// prevent leaking memory.
    ///
    /// This is required to enable traps to work correctly since the signal handler
    /// will lookup in the `GLOBAL_CODE` list to determine which a particular pc
    /// is a trap or not.
    pub fn register_code(code: &Arc<CodeMemory>) {
        let text = code.text();
        if text.is_empty() {
            return;
        }
        let start = text.as_ptr() as usize;
        let end = start + text.len() - 1;
        let prev = global_code()
            .write()
            .unwrap()
            .insert(end, (start, code.clone()));
        assert!(prev.is_none());
    }
}
