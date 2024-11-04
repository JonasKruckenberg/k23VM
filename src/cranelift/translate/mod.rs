mod code_translator;
pub mod env;
mod func_translator;
mod global;
mod heap;
mod state;
mod table;
mod utils;
mod builtins;

pub use func_translator::FuncTranslator;