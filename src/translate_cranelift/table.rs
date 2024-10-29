use crate::translate_cranelift::TranslationEnvironment;
use crate::traps::TRAP_TABLE_OUT_OF_BOUNDS;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::FunctionBuilder;

#[derive(Debug)]
pub struct TranslatedTable {
    /// Global value giving the address of the start of the table.
    pub base_gv: ir::GlobalValue,

    /// The table kind, including kind-specific data.
    pub kind: TranslatedTableKind,

    /// The size of a table element, in bytes.
    pub element_size: u32,
}

impl TranslatedTable {
    /// Return a CLIF value containing a native pointer to the beginning of the
    /// given index within this table.
    pub fn prepare_addr(
        &self,
        builder: &mut FunctionBuilder,
        index: ir::Value,
        _addr_ty: ir::Type,
        enable_table_access_spectre_mitigation: bool,
        env: &mut TranslationEnvironment,
    ) -> (ir::Value, ir::MemFlags) {
        let index_ty = builder.func.dfg.value_type(index);

        // Start with the bounds check. Trap if `index + 1 > bound`.
        let bound = self.kind.bound(builder.cursor(), index_ty);

        // `index > bound - 1` is the same as `index >= bound`.
        let oob = builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, bound);

        if !enable_table_access_spectre_mitigation {
            env.trapnz(builder, oob, TRAP_TABLE_OUT_OF_BOUNDS);
        }

        todo!()
    }
}

/// Size of a WebAssembly table, in elements.
#[derive(Debug, Clone)]
pub enum TranslatedTableKind {
    /// Non-resizable table.
    Static {
        /// Non-resizable tables have a constant size known at compile_cranelift time.
        bound: u32,
    },
    /// Resizable table.
    Dynamic {
        /// Resizable tables declare a Cranelift global value to load the
        /// current size from.
        bound_gv: ir::GlobalValue,
    },
}

impl TranslatedTableKind {
    /// Get a CLIF value representing the current bounds of this table.
    pub fn bound(&self, mut pos: FuncCursor, index_ty: ir::Type) -> ir::Value {
        match *self {
            TranslatedTableKind::Static { bound } => {
                pos.ins().iconst(index_ty, Imm64::new(i64::from(bound)))
            }
            TranslatedTableKind::Dynamic { bound_gv } => pos.ins().global_value(index_ty, bound_gv),
        }
    }
}
