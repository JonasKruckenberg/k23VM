use crate::indices::{FuncIndex, GlobalIndex};
use crate::wasm_unsupported;
use smallvec::SmallVec;

/// A WebAssembly constant expression.
///
/// This is a subset of the WebAssembly instruction used to initialize things like globals, tables
/// etc.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConstExpr {
    ops: SmallVec<[ConstOp; 2]>,
}

impl ConstExpr {
    /// Converts a `wasmparser::ConstExpr` into a `ConstExpr`.
    pub fn from_wasmparser(
        expr: wasmparser::ConstExpr<'_>,
    ) -> crate::Result<(Self, SmallVec<[FuncIndex; 1]>)> {
        let mut iter = expr
            .get_operators_reader()
            .into_iter_with_offsets()
            .peekable();

        let mut ops = SmallVec::<[ConstOp; 2]>::new();
        let mut escaped = SmallVec::<[FuncIndex; 1]>::new();
        while let Some(res) = iter.next() {
            let (op, offset) = res?;

            if matches!(op, wasmparser::Operator::End) && iter.peek().is_none() {
                break;
            }

            if let wasmparser::Operator::RefFunc { function_index } = &op {
                escaped.push(FuncIndex::from_u32(*function_index));
            }

            ops.push(ConstOp::from_wasmparser(op, offset)?);
        }

        Ok((Self { ops }, escaped))
    }

    pub(crate) fn ops(&self) -> impl ExactSizeIterator<Item = ConstOp> + '_ {
        self.ops.iter().copied()
    }
}

/// A constant operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConstOp {
    I32Const(i32),
    I64Const(i64),
    F32Const(u32),
    F64Const(u64),
    V128Const([u8; 16]),
    RefI31,
    RefNull,
    RefFunc(FuncIndex),
    GlobalGet(GlobalIndex),
    // Defined by the extended const proposal.
    I32Add,
    I32Sub,
    I32Mul,
    I64Add,
    I64Sub,
    I64Mul,
}

impl ConstOp {
    /// Converts a `wasmparser::Operator` into a `ConstOp`.
    fn from_wasmparser(op: wasmparser::Operator, offset: usize) -> crate::Result<Self> {
        use wasmparser::Operator;
        match op {
            Operator::I32Const { value } => Ok(Self::I32Const(value)),
            Operator::I64Const { value } => Ok(Self::I64Const(value)),
            Operator::F32Const { value } => Ok(Self::F32Const(value.bits())),
            Operator::F64Const { value } => Ok(Self::F64Const(value.bits())),
            Operator::V128Const { value } => Ok(Self::V128Const(*value.bytes())),
            Operator::RefI31 => Ok(Self::RefI31),
            Operator::RefNull { .. } => Ok(Self::RefNull),
            Operator::RefFunc { function_index } => {
                Ok(Self::RefFunc(FuncIndex::from_u32(function_index)))
            }
            Operator::GlobalGet { global_index } => {
                Ok(Self::GlobalGet(GlobalIndex::from_u32(global_index)))
            }
            Operator::I32Add => Ok(Self::I32Add),
            Operator::I32Sub => Ok(Self::I32Sub),
            Operator::I32Mul => Ok(Self::I32Mul),
            Operator::I64Add => Ok(Self::I64Add),
            Operator::I64Sub => Ok(Self::I64Sub),
            Operator::I64Mul => Ok(Self::I64Mul),
            _ => Err(wasm_unsupported!(
                "unsupported opcode in const expression at offset {offset:#x}: {op:?}",
            )),
        }
    }
}
