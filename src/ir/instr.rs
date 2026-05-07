use crate::ir::types::{BlockId, IrType, ValueId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IrInst {
    ConstInt {
        dst: ValueId,
        ty: IrType,
        value: i128,
    },
    ConstFloat {
        dst: ValueId,
        value: f64,
    },
    SsaBind {
        dst: ValueId,
        ty: IrType,
        src: ValueId,
    },
    Binary {
        dst: ValueId,
        op: BinaryOp,
        ty: IrType,
        lhs: ValueId,
        rhs: ValueId,
    },
    Compare {
        dst: ValueId,
        op: CompareOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Call {
        dst: Option<ValueId>,
        callee: String,
        args: Vec<ValueId>,
        return_ty: Option<IrType>,
    },
    Cast {
        dst: ValueId,
        from: IrType,
        to: IrType,
        value: ValueId,
    },
    Alloca {
        dst: ValueId,
        size: usize,
        align: usize,
    },
    /// Advance a pointer by a compile-time byte offset.
    ///
    /// `dst = ptr_offset base + offset`
    ///
    /// Used to address struct fields at non-zero offsets.  When `offset`
    /// is 0 the instruction is still valid but the caller should prefer
    /// reusing `base` directly to avoid a no-op instruction.
    PtrOffset {
        dst: ValueId,
        base: ValueId,
        offset: usize,
    },
    /// Add a runtime byte offset to a pointer.
    ///
    /// `dst = ptr_add base + offset`
    ///
    /// Used to address array elements at runtime-computed byte offsets.
    /// `offset` must be an I64 SSA value holding the precomputed byte count.
    PtrAdd {
        dst: ValueId,
        base: ValueId,
        offset: ValueId,
    },
    Load {
        dst: ValueId,
        ty: IrType,
        ptr: ValueId,
    },
    Store {
        ptr: ValueId,
        value: ValueId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum IrTerminator {
    Jump {
        target: BlockId,
        args: Vec<ValueId>,
    },
    Branch {
        cond: ValueId,
        then_block: BlockId,
        then_args: Vec<ValueId>,
        else_block: BlockId,
        else_args: Vec<ValueId>,
    },
    Return {
        value: Option<ValueId>,
    },
    /// Unconditional abort — emitted for assertion failures.
    ///
    /// In the JIT backend this lowers to a Cranelift `trap` instruction,
    /// which raises a hardware fault and terminates the process.  No values
    /// are consumed and no successor block exists.
    Trap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inst_variants_hold_expected_typed_fields() {
        let inst = IrInst::Binary {
            dst: ValueId(7),
            op: BinaryOp::Mul,
            ty: IrType::I32,
            lhs: ValueId(1),
            rhs: ValueId(2),
        };

        match inst {
            IrInst::Binary {
                dst,
                op,
                ty,
                lhs,
                rhs,
            } => {
                assert_eq!(dst, ValueId(7));
                assert_eq!(op, BinaryOp::Mul);
                assert_eq!(ty, IrType::I32);
                assert_eq!(lhs, ValueId(1));
                assert_eq!(rhs, ValueId(2));
            }
            other => panic!("unexpected instruction variant: {other:?}"),
        }
    }

    #[test]
    fn trap_terminator_is_distinct_from_other_variants() {
        let term = IrTerminator::Trap;
        assert!(matches!(term, IrTerminator::Trap));
        assert!(!matches!(term, IrTerminator::Return { .. }));
        assert!(!matches!(term, IrTerminator::Jump { .. }));
        assert!(!matches!(term, IrTerminator::Branch { .. }));
    }

    #[test]
    fn terminator_variants_hold_expected_target_value_data() {
        let term = IrTerminator::Branch {
            cond: ValueId(0),
            then_block: BlockId(1),
            then_args: vec![ValueId(2)],
            else_block: BlockId(3),
            else_args: vec![ValueId(4), ValueId(5)],
        };

        match term {
            IrTerminator::Branch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                assert_eq!(cond, ValueId(0));
                assert_eq!(then_block, BlockId(1));
                assert_eq!(then_args, vec![ValueId(2)]);
                assert_eq!(else_block, BlockId(3));
                assert_eq!(else_args, vec![ValueId(4), ValueId(5)]);
            }
            other => panic!("unexpected terminator variant: {other:?}"),
        }
    }
}
