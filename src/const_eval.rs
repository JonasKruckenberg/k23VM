use crate::runtime::VMVal;
use crate::translate::{ConstExpr, ConstOp};
use smallvec::SmallVec;

#[derive(Debug, Default)]
pub struct ConstExprEvaluator {
    stack: SmallVec<[VMVal; 2]>,
}

impl ConstExprEvaluator {
    pub fn eval(&mut self, expr: &ConstExpr) -> crate::Result<VMVal> {
        for op in expr.ops() {
            match op {
                ConstOp::I32Const(value) => self.push(VMVal::i32(value)),
                ConstOp::I64Const(value) => self.push(VMVal::i64(value)),
                ConstOp::F32Const(value) => self.push(VMVal::f32(value)),
                ConstOp::F64Const(value) => self.push(VMVal::f64(value)),
                ConstOp::V128Const(value) => self.push(VMVal::v128(u128::from_le_bytes(value))),
                ConstOp::GlobalGet(_) => todo!(),
                ConstOp::RefI31 => todo!(),
                ConstOp::RefNull => todo!(),
                ConstOp::RefFunc(_) => todo!(),
                ConstOp::I32Add => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i32(arg1.get_i32().wrapping_add(arg2.get_i32())));
                }
                ConstOp::I32Sub => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i32(arg1.get_i32().wrapping_sub(arg2.get_i32())));
                }
                ConstOp::I32Mul => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i32(arg1.get_i32().wrapping_mul(arg2.get_i32())));
                }
                ConstOp::I64Add => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i64(arg1.get_i64().wrapping_add(arg2.get_i64())));
                }
                ConstOp::I64Sub => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i64(arg1.get_i64().wrapping_sub(arg2.get_i64())));
                }
                ConstOp::I64Mul => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i64(arg1.get_i64().wrapping_mul(arg2.get_i64())));
                }
            }
        }

        assert_eq!(self.stack.len(), 1);
        Ok(self.stack.pop().expect("empty stack"))
    }

    fn push(&mut self, val: VMVal) {
        self.stack.push(val);
    }

    fn pop2(&mut self) -> (VMVal, VMVal) {
        let v2 = self.stack.pop().unwrap();
        let v1 = self.stack.pop().unwrap();
        (v1, v2)
    }
}
