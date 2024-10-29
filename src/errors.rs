use crate::traps::{Trap, WasmBacktrace};
use alloc::string::{String, ToString};
use core::fmt;

#[derive(Debug)]
pub enum Error {
    /// The input WebAssembly code is invalid.
    ///
    /// This error code is used by a WebAssembly translator when it encounters invalid WebAssembly
    /// code. This should never happen for validated WebAssembly code.
    InvalidWebAssembly {
        /// A string describing the validation error.
        message: String,
        /// The bytecode offset where the error occurred.
        offset: usize,
    },
    /// Failed to compile a function.
    Cranelift {
        func_name: String,
        error: cranelift_codegen::CodegenError,
    },
    /// Failed to parse DWARF debug information.
    Gimli(gimli::Error),
    /// The WebAssembly code used an unsupported feature.
    Unsupported(String),
    /// A WebAssembly trap ocurred.
    Trap {
        backtrace: Option<WasmBacktrace>,
        trap: Trap,
        message: String,
    },
}

impl From<wasmparser::BinaryReaderError> for Error {
    fn from(e: wasmparser::BinaryReaderError) -> Self {
        Self::InvalidWebAssembly {
            message: e.message().into(),
            offset: e.offset(),
        }
    }
}

impl From<cranelift_codegen::CompileError<'_>> for Error {
    fn from(e: cranelift_codegen::CompileError<'_>) -> Self {
        Self::Cranelift {
            func_name: e.func.name.to_string(),
            error: e.inner,
        }
    }
}

impl From<gimli::Error> for Error {
    fn from(value: gimli::Error) -> Self {
        Self::Gimli(value)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidWebAssembly { message, offset } => {
                f.write_fmt(format_args!("invalid WASM input at {offset}: {message}"))
            }
            Error::Cranelift { func_name, error } => f.write_fmt(format_args!(
                "failed to compile function {func_name}: {error}"
            )),
            Error::Unsupported(feature) => f.write_fmt(format_args!(
                "Feature used by the WebAssembly code is not supported: {feature}"
            )),
            Error::Gimli(e) => {
                f.write_fmt(format_args!("Failed to parse DWARF debug information: {e}"))
            }
            Error::Trap {
                backtrace,
                trap,
                message,
            } => {
                f.write_fmt(format_args!("{message}. Reason {trap}"))?;
                if let Some(backtrace) = backtrace {
                    f.write_fmt(format_args!("\n{backtrace}"))?;
                }
                Ok(())
            }
        }
    }
}

impl core::error::Error for Error {}

#[macro_export]
macro_rules! wasm_unsupported {
    ($($arg:tt)*) => { $crate::Error::Unsupported(alloc::format!($($arg)*)) }
}

#[derive(Copy, Clone, Debug)]
pub struct SizeOverflow;

impl fmt::Display for SizeOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("size overflow calculating memory size")
    }
}

impl core::error::Error for SizeOverflow {}
