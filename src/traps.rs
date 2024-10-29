use crate::indices::FuncIndex;
use crate::{FilePos, Module};
use core::fmt;
use cranelift_codegen::ir::TrapCode;
use object::{Bytes, LittleEndian, U32};

const TRAP_OFFSET: u8 = 1;
pub const TRAP_INTERNAL_ASSERT: TrapCode =
    TrapCode::unwrap_user(Trap::InternalAssertionFailed as u8 + TRAP_OFFSET);
pub const TRAP_HEAP_MISALIGNED: TrapCode =
    TrapCode::unwrap_user(Trap::HeapMisaligned as u8 + TRAP_OFFSET);
pub const TRAP_TABLE_OUT_OF_BOUNDS: TrapCode =
    TrapCode::unwrap_user(Trap::TableOutOfBounds as u8 + TRAP_OFFSET);
pub const TRAP_INDIRECT_CALL_TO_NULL: TrapCode =
    TrapCode::unwrap_user(Trap::IndirectCallToNull as u8 + TRAP_OFFSET);
pub const TRAP_BAD_SIGNATURE: TrapCode =
    TrapCode::unwrap_user(Trap::BadSignature as u8 + TRAP_OFFSET);
pub const TRAP_UNREACHABLE: TrapCode =
    TrapCode::unwrap_user(Trap::UnreachableCodeReached as u8 + TRAP_OFFSET);
pub const TRAP_NULL_REFERENCE: TrapCode =
    TrapCode::unwrap_user(Trap::NullReference as u8 + TRAP_OFFSET);
pub const TRAP_I31_NULL_REFERENCE: TrapCode =
    TrapCode::unwrap_user(Trap::NullI31Ref as u8 + TRAP_OFFSET);

#[derive(onlyerror::Error, Debug)]
pub enum Trap {
    /// Internal assertion failed
    #[error("internal assertion failed")]
    InternalAssertionFailed,
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
    /// Code that was supposed to have been unreachable was reached.
    #[error("unreachable code executed")]
    UnreachableCodeReached,
    /// Call to a null reference.
    #[error("null reference called")]
    NullReference,
    /// Attempt to get the bits of a null `i31ref`.
    #[error("null i32 reference called")]
    NullI31Ref,

    /// The current stack space was exhausted.
    #[error("call stack exhausted")]
    StackOverflow,
    /// An out-of-bounds memory access.
    #[error("out of bounds memory access")]
    MemoryOutOfBounds,
    /// An integer arithmetic operation caused an overflow.
    #[error("integer overflow")]
    IntegerOverflow,
    /// An integer division by zero.
    #[error("integer division by zero")]
    IntegerDivisionByZero,
    /// Failed float-to-int conversion.
    #[error("invalid conversion to integer")]
    BadConversionToInteger,
}

impl Trap {
    pub(crate) fn from_trap_code(code: TrapCode) -> Option<Self> {
        match code {
            TrapCode::STACK_OVERFLOW => Some(Trap::StackOverflow),
            TrapCode::HEAP_OUT_OF_BOUNDS => Some(Trap::MemoryOutOfBounds),
            TrapCode::INTEGER_OVERFLOW => Some(Trap::IntegerOverflow),
            TrapCode::INTEGER_DIVISION_BY_ZERO => Some(Trap::IntegerDivisionByZero),
            TrapCode::BAD_CONVERSION_TO_INTEGER => Some(Trap::BadConversionToInteger),

            TRAP_INTERNAL_ASSERT => Some(Trap::InternalAssertionFailed),
            TRAP_HEAP_MISALIGNED => Some(Trap::HeapMisaligned),
            TRAP_TABLE_OUT_OF_BOUNDS => Some(Trap::TableOutOfBounds),
            TRAP_INDIRECT_CALL_TO_NULL => Some(Trap::IndirectCallToNull),
            TRAP_BAD_SIGNATURE => Some(Trap::BadSignature),
            TRAP_UNREACHABLE => Some(Trap::UnreachableCodeReached),
            TRAP_NULL_REFERENCE => Some(Trap::NullReference),
            TRAP_I31_NULL_REFERENCE => Some(Trap::NullI31Ref),
            c => {
                tracing::warn!("unknown trap code {c}");
                None
            }
        }
    }
}

impl From<Trap> for u8 {
    fn from(value: Trap) -> Self {
        match value {
            Trap::InternalAssertionFailed => 0,
            Trap::HeapMisaligned => 1,
            Trap::TableOutOfBounds => 2,
            Trap::IndirectCallToNull => 3,
            Trap::BadSignature => 4,
            Trap::UnreachableCodeReached => 5,
            Trap::NullReference => 6,
            Trap::NullI31Ref => 7,

            Trap::StackOverflow => 8,
            Trap::MemoryOutOfBounds => 9,
            Trap::IntegerOverflow => 10,
            Trap::IntegerDivisionByZero => 11,
            Trap::BadConversionToInteger => 12,
        }
    }
}

impl TryFrom<u8> for Trap {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::InternalAssertionFailed),
            1 => Ok(Self::HeapMisaligned),
            2 => Ok(Self::TableOutOfBounds),
            3 => Ok(Self::IndirectCallToNull),
            4 => Ok(Self::BadSignature),
            5 => Ok(Self::UnreachableCodeReached),
            6 => Ok(Self::NullReference),
            7 => Ok(Self::NullI31Ref),

            8 => Ok(Self::StackOverflow),
            9 => Ok(Self::MemoryOutOfBounds),
            10 => Ok(Self::IntegerOverflow),
            11 => Ok(Self::IntegerDivisionByZero),
            12 => Ok(Self::BadConversionToInteger),
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

#[derive(Debug)]
pub struct WasmBacktrace {
    wasm_trace: Vec<FrameInfo>,
}

impl WasmBacktrace {
    pub(crate) fn from_captured(
        module: &Module,
        runtime_trace: crate::vm::Backtrace,
        trap_pc: Option<usize>,
    ) -> Self {
        let mut wasm_trace = Vec::<FrameInfo>::with_capacity(runtime_trace.frames().len());

        for frame in runtime_trace.frames() {
            debug_assert!(frame.pc != 0);

            let pc_to_lookup = if Some(frame.pc) == trap_pc {
                frame.pc
            } else {
                frame.pc - 1
            };

            let text_offset = pc_to_lookup.checked_sub(module.code().text().as_ptr() as usize).unwrap();
            let text_offset = usize::try_from(text_offset).unwrap();

            if let Some(info) = FrameInfo::new(module.clone(), text_offset) {
                wasm_trace.push(info);
            }
        }

        Self { wasm_trace }
    }
}

impl fmt::Display for WasmBacktrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, frame) in self.wasm_trace.iter().enumerate() {
            let name = frame.module.name().unwrap_or("<unknown>");
            write!(f, "  {i:>3}: ")?;

            if frame.symbols().is_empty() {
                write!(f, "{name}::")?;

                demangle_function_name_or_index(f, frame.func_name(), frame.func_index())?;
            } else {
                for (i, symbol) in frame.symbols().iter().enumerate() {
                    if i > 0 {
                        write!(f, "              - ")?;
                    } else {
                        // ...
                    }
                    match symbol.name() {
                        Some(name) => demangle_function_name(f, name)?,
                        None if i == 0 => demangle_function_name_or_index(
                            f,
                            frame.func_name(),
                            frame.func_index(),
                        )?,
                        None => write!(f, "<inlined function>")?,
                    }
                    if let Some(file) = symbol.file() {
                        writeln!(f, "")?;
                        write!(f, "                    at {file}")?;
                        if let Some(line) = symbol.line() {
                            write!(f, ":{line}")?;
                            if let Some(col) = symbol.column() {
                                write!(f, ":{col}")?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn demangle_function_name_or_index(
    f: &mut fmt::Formatter,
    maybe_name: Option<&str>,
    index: FuncIndex,
) -> fmt::Result {
    match maybe_name {
        Some(name) => demangle_function_name(f, name),
        None => write!(f, "<wasm function {}>", index.as_u32()),
    }
}

// TODO if we can pipe the information from the WASM producers section into
// this we can have a better informed demangling
fn demangle_function_name(f: &mut fmt::Formatter, raw: &str) -> fmt::Result {
    if let Ok(demangled) = rustc_demangle::try_demangle(raw) {
        write!(f, "{demangled}")
    } else {
        write!(f, "{raw}")
    }
}

#[derive(Debug)]
pub struct FrameInfo {
    module: Module,
    func_index: FuncIndex,
    func_name: Option<String>,
    func_start: FilePos,
    instr: Option<FilePos>,
    symbols: Vec<FrameSymbol>,
}

impl FrameInfo {
    fn new(module: Module, text_offset: usize) -> Option<Self> {
        let (index, _) = module.compiled().text_offset_to_func(text_offset)?;

        let info = module.compiled().wasm_func_info(index);
        let func_index = module.parsed().func_index(index);
        let func_name = module.func_name(func_index).map(|s| s.to_string());
        let func_start = info.start_srcloc;
        let instr = None;

        let mut symbols = Vec::new();

        let _ = &mut symbols;
        if let Some(s) = module.symbolize_context().ok().flatten() {
            // if let Some(offset) = instr.and_then(|i| i.file_offset()) {
            //     let to_lookup = u64::from(offset) - s.code_section_offset();
            //     if let Ok(mut frames) = s.addr2line().find_frames(to_lookup).skip_all_loads() {
            //         while let Ok(Some(frame)) = frames.next() {
            //             symbols.push(FrameSymbol {
            //                 name: frame
            //                     .function
            //                     .as_ref()
            //                     .and_then(|l| l.raw_name().ok())
            //                     .map(|s| s.to_string()),
            //                 file: frame
            //                     .location
            //                     .as_ref()
            //                     .and_then(|l| l.file)
            //                     .map(|s| s.to_string()),
            //                 line: frame.location.as_ref().and_then(|l| l.line),
            //                 column: frame.location.as_ref().and_then(|l| l.column),
            //             });
            //         }
            //     }
            // }
        }

        Some(Self {
            module,
            func_index,
            func_name,
            func_start,
            instr,
            symbols,
        })
    }

    pub fn func_index(&self) -> FuncIndex {
        self.func_index
    }
    pub fn module(&self) -> &Module {
        &self.module
    }
    pub fn func_name(&self) -> Option<&str> {
        self.func_name.as_deref()
    }
    pub fn symbols(&self) -> &[FrameSymbol] {
        &self.symbols
    }
}

#[derive(Debug)]
pub struct FrameSymbol {
    name: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

impl FrameSymbol {
    /// Returns the function name associated with this symbol.
    ///
    /// Note that this may not be present with malformed debug information, or
    /// the debug information may not include it. Also note that the symbol is
    /// frequently mangled, so you might need to run some form of demangling
    /// over it.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the source code filename this symbol was defined in.
    ///
    /// Note that this may not be present with malformed debug information, or
    /// the debug information may not include it.
    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    /// Returns the 1-indexed source code line number this symbol was defined
    /// on.
    ///
    /// Note that this may not be present with malformed debug information, or
    /// the debug information may not include it.
    pub fn line(&self) -> Option<u32> {
        self.line
    }

    /// Returns the 1-indexed source code column number this symbol was defined
    /// on.
    ///
    /// Note that this may not be present with malformed debug information, or
    /// the debug information may not include it.
    pub fn column(&self) -> Option<u32> {
        self.column
    }
}
