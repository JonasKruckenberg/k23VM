use alloc::string::{String, ToString};
use core::fmt;

/// A WebAssembly translation error.
#[derive(Debug, onlyerror::Error)]
pub enum TranslationError {
    /// The input WebAssembly code is invalid.
    ///
    /// This error code is used by a WebAssembly translator when it encounters invalid WebAssembly
    /// code. This should never happen for validated WebAssembly code.
    #[error("invalid WASM input at {offset}: {message}")]
    InvalidWebAssembly {
        /// A string describing the validation error.
        message: String,
        /// The bytecode offset where the error occurred.
        offset: usize,
    },
    /// An implementation limit was exceeded.
    #[error("implementation limit was exceeded")]
    ImplLimitExceeded,
    #[error("Feature used by the WebAssembly code is not supported: {0}")]
    Unsupported(String),
}

impl From<wasmparser::BinaryReaderError> for TranslationError {
    fn from(e: wasmparser::BinaryReaderError) -> Self {
        Self::InvalidWebAssembly {
            message: e.message().into(),
            offset: e.offset(),
        }
    }
}

#[macro_export]
macro_rules! wasm_unsupported {
    ($($arg:tt)*) => { $crate::TranslationError::Unsupported(alloc::format!($($arg)*)) }
}

#[derive(Debug, onlyerror::Error)]
pub enum CompileError {
    #[error("todo")]
    Translate(#[from] TranslationError),
    #[error("todo")]
    Cranelift {
        func_name: String,
        error: cranelift_codegen::CodegenError,
    },
}

impl From<cranelift_codegen::CompileError<'_>> for CompileError {
    fn from(e: cranelift_codegen::CompileError<'_>) -> Self {
        Self::Cranelift {
            func_name: e.func.name.to_string(),
            error: e.inner,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SizeOverflow;

impl fmt::Display for SizeOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("size overflow calculating memory size")
    }
}

impl core::error::Error for SizeOverflow {}
