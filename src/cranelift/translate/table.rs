use crate::cranelift::translate::env::TranslationEnvironment;
use cranelift_codegen::ir;
use cranelift_codegen::ir::{MemFlags, Value};
use cranelift_frontend::FunctionBuilder;

pub struct Table {}

impl Table {
    /// Return a CLIF value containing a native pointer to the beginning of the
    /// given index within this table.
    pub fn prepare_addr(
        &self,
        _builder: &mut FunctionBuilder,
        _index: ir::Value,
        _addr_ty: ir::Type,
        _enable_table_access_spectre_mitigation: bool,
        _env: &mut TranslationEnvironment,
    ) -> crate::Result<(Value, MemFlags)> {
        todo!()
    }
}
