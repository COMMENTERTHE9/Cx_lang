#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;

use crate::frontend::ast::Op;
use crate::frontend::semantic_types::{
    BindingId, SemanticExpr, SemanticExprKind, SemanticLValue, SemanticProgram, SemanticStmt,
    SemanticType, SemanticValue,
};
use crate::ir::builder::IrBuilder;
use crate::ir::instr::{BinaryOp, CompareOp, IrInst, IrTerminator};
use crate::ir::types::{IrFunction, IrModule, IrType, ValueId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoweringError {
    UnsupportedSemanticConstruct { construct: String },
    UnsupportedSemanticType { ty: String },
    UnresolvedSemanticArtifact { artifact: String },
    InternalInvariantViolation { detail: String },
}

impl fmt::Display for LoweringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSemanticConstruct { construct } => {
                write!(
                    f,
                    "unsupported semantic construct during lowering: {construct}"
                )
            }
            Self::UnsupportedSemanticType { ty } => {
                write!(f, "unsupported semantic type during lowering: {ty}")
            }
            Self::UnresolvedSemanticArtifact { artifact } => {
                write!(
                    f,
                    "unresolved semantic artifact reached lowering: {artifact}"
                )
            }
            Self::InternalInvariantViolation { detail } => {
                write!(f, "lowering invariant violation: {detail}")
            }
        }
    }
}

impl std::error::Error for LoweringError {}

#[derive(Clone, Debug, PartialEq)]
struct LoweredValue {
    value: ValueId,
    ty: IrType,
}

struct LoweringCtx {
    builder: IrBuilder,
    block: crate::ir::builder::IrBlockBuilder,
    bindings: HashMap<BindingId, ValueId>,
}

impl LoweringCtx {
    fn new() -> Self {
        let mut builder = IrBuilder::new();
        let block = builder.block(vec![]);
        Self {
            builder,
            block,
            bindings: HashMap::new(),
        }
    }

    fn emit(&mut self, inst: IrInst) {
        self.block.append_inst(inst);
    }

    fn fresh_value(&mut self) -> ValueId {
        self.builder.fresh_value()
    }
}

pub fn lower_program(program: &SemanticProgram) -> Result<IrModule, LoweringError> {
    if program.stmts.is_empty() {
        return Ok(IrModule {
            debug_name: "cxir_v0".into(),
            functions: vec![],
        });
    }

    let mut ctx = LoweringCtx::new();

    for stmt in &program.stmts {
        lower_stmt(stmt, &mut ctx)?;
    }

    ctx.block
        .set_terminator(IrTerminator::Return { value: None })
        .map_err(|err| LoweringError::InternalInvariantViolation {
            detail: format!("failed to finalize synthetic main terminator: {err:?}"),
        })?;

    let block = ctx
        .block
        .finish()
        .map_err(|err| LoweringError::InternalInvariantViolation {
            detail: format!("failed to finalize synthetic main block: {err:?}"),
        })?;

    let mut function = IrFunction {
        name: "main".to_string(),
        params: vec![],
        return_ty: None,
        blocks: vec![],
    };
    ctx.builder.append_block(&mut function, block);

    let mut module = IrModule {
        debug_name: "cxir_v0".into(),
        functions: vec![],
    };
    ctx.builder.append_function(&mut module, function);
    Ok(module)
}

fn lower_stmt(stmt: &SemanticStmt, ctx: &mut LoweringCtx) -> Result<(), LoweringError> {
    match stmt {
        SemanticStmt::Decl { ty, .. } => {
            if let Some(ty) = ty {
                let _ = lower_type(ty)?;
            }
            Ok(())
        }
        SemanticStmt::Assign { target, expr, .. } => {
            let lowered = lower_expr(expr, ctx)?;
            match target {
                SemanticLValue::Binding { binding, ty, .. } => {
                    let target_ty = lower_type(ty)?;
                    ensure_type_match("assign", target_ty.clone(), lowered.ty)?;
                    let dst = ctx.fresh_value();
                    ctx.emit(IrInst::SsaBind {
                        dst,
                        ty: target_ty,
                        src: lowered.value,
                    });
                    ctx.bindings.insert(*binding, dst);
                    Ok(())
                }
                SemanticLValue::DotAccess { .. } => {
                    Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: "Assign::DotAccess".to_string(),
                    })
                }
            }
        }
        SemanticStmt::TypedAssign {
            binding, ty, expr, ..
        } => {
            let lowered = lower_expr(expr, ctx)?;
            let target_ty = lower_type(ty)?;
            ensure_type_match("typed assignment", target_ty.clone(), lowered.ty)?;
            let dst = ctx.fresh_value();
            ctx.emit(IrInst::SsaBind {
                dst,
                ty: target_ty,
                src: lowered.value,
            });
            ctx.bindings.insert(*binding, dst);
            Ok(())
        }
        SemanticStmt::ExprStmt { expr, .. } => {
            let _ = lower_expr(expr, ctx)?;
            Ok(())
        }
        SemanticStmt::CompoundAssign { .. } => Err(LoweringError::UnresolvedSemanticArtifact {
            artifact: "CompoundAssign".to_string(),
        }),
        SemanticStmt::FuncDef(_) => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "FuncDef".to_string(),
        }),
        SemanticStmt::Return { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "top-level Return".to_string(),
        }),
        SemanticStmt::EnumDef { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "EnumDef".to_string(),
        }),
        SemanticStmt::Print { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Print".to_string(),
        }),
        SemanticStmt::PrintInline { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "PrintInline".to_string(),
        }),
        SemanticStmt::Block { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Block".to_string(),
        }),
        SemanticStmt::While { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "While".to_string(),
        }),
        SemanticStmt::For { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "For".to_string(),
        }),
        SemanticStmt::Loop { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Loop".to_string(),
        }),
        SemanticStmt::Break { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Break".to_string(),
        }),
        SemanticStmt::Continue { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Continue".to_string(),
        }),
        SemanticStmt::When { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "When".to_string(),
        }),
    }
}

fn lower_expr(expr: &SemanticExpr, ctx: &mut LoweringCtx) -> Result<LoweredValue, LoweringError> {
    match &expr.kind {
        SemanticExprKind::Value(value) => lower_value(value, &expr.ty, ctx),
        SemanticExprKind::VarRef { binding, name } => {
            let ty = lower_type(&expr.ty)?;
            let value = ctx.bindings.get(binding).copied().ok_or_else(|| {
                LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "binding '{name}' ({}) referenced before any SSA value was assigned",
                        binding.0
                    ),
                }
            })?;
            Ok(LoweredValue { value, ty })
        }
        SemanticExprKind::Binary { lhs, op, rhs, .. } => lower_binary(lhs, *op, rhs, &expr.ty, ctx),
        SemanticExprKind::Cast { expr, from, to } => {
            let lowered = lower_expr(expr, ctx)?;
            let from_ty = lower_type(from)?;
            let to_ty = lower_type(to)?;
            ensure_type_match("cast source", from_ty.clone(), lowered.ty)?;
            let dst = ctx.fresh_value();
            ctx.emit(IrInst::Cast {
                dst,
                from: from_ty,
                to: to_ty.clone(),
                value: lowered.value,
            });
            Ok(LoweredValue {
                value: dst,
                ty: to_ty,
            })
        }
        SemanticExprKind::Call { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Call".to_string(),
        }),
        SemanticExprKind::DotAccess { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "DotAccess".to_string(),
        }),
        SemanticExprKind::HandleNew { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "HandleNew".to_string(),
        }),
        SemanticExprKind::HandleVal { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "HandleVal".to_string(),
        }),
        SemanticExprKind::HandleDrop { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "HandleDrop".to_string(),
        }),
        SemanticExprKind::Range { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Range".to_string(),
        }),
        SemanticExprKind::Unary { .. } => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Unary".to_string(),
        }),
    }
}

fn lower_value(
    value: &SemanticValue,
    semantic_ty: &SemanticType,
    ctx: &mut LoweringCtx,
) -> Result<LoweredValue, LoweringError> {
    let ty = lower_type(semantic_ty)?;
    let dst = ctx.fresh_value();

    match value {
        SemanticValue::Num(n) => {
            ctx.emit(IrInst::ConstInt {
                dst,
                ty: ty.clone(),
                value: *n,
            });
            Ok(LoweredValue { value: dst, ty })
        }
        SemanticValue::Float(value) => {
            if ty != IrType::F64 {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!("float literal lowered with non-f64 type: {ty:?}"),
                });
            }
            ctx.emit(IrInst::ConstFloat { dst, value: *value });
            Ok(LoweredValue { value: dst, ty })
        }
        SemanticValue::Bool(value) => {
            if ty != IrType::Bool {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!("bool literal lowered with non-bool type: {ty:?}"),
                });
            }
            ctx.emit(IrInst::ConstInt {
                dst,
                ty: IrType::Bool,
                value: i128::from(*value),
            });
            Ok(LoweredValue { value: dst, ty })
        }
        SemanticValue::Unknown => Err(LoweringError::UnsupportedSemanticType {
            ty: "Unknown".to_string(),
        }),
        SemanticValue::Str(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Str".to_string(),
        }),
        SemanticValue::Char(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Char".to_string(),
        }),
        SemanticValue::EnumVariant { .. } => Err(LoweringError::UnsupportedSemanticType {
            ty: "Enum".to_string(),
        }),
    }
}

fn lower_binary(
    lhs: &SemanticExpr,
    op: Op,
    rhs: &SemanticExpr,
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
) -> Result<LoweredValue, LoweringError> {
    let lhs = lower_expr(lhs, ctx)?;
    let rhs = lower_expr(rhs, ctx)?;
    let dst = ctx.fresh_value();

    match op {
        Op::Plus | Op::Minus | Op::Mul | Op::Div | Op::Mod => {
            let ty = lower_type(result_ty)?;
            ensure_type_match("binary lhs", ty.clone(), lhs.ty)?;
            ensure_type_match("binary rhs", ty.clone(), rhs.ty)?;
            let op = match op {
                Op::Plus => BinaryOp::Add,
                Op::Minus => BinaryOp::Sub,
                Op::Mul => BinaryOp::Mul,
                Op::Div => BinaryOp::Div,
                Op::Mod => BinaryOp::Rem,
                _ => unreachable!(),
            };
            ctx.emit(IrInst::Binary {
                dst,
                op,
                ty: ty.clone(),
                lhs: lhs.value,
                rhs: rhs.value,
            });
            Ok(LoweredValue { value: dst, ty })
        }
        Op::EqEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => {
            ensure_type_match("compare lhs/rhs", lhs.ty, rhs.ty)?;
            let result_ty = lower_type(result_ty)?;
            if result_ty != IrType::Bool {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!("comparison produced non-bool semantic type: {result_ty:?}"),
                });
            }
            let op = match op {
                Op::EqEq => CompareOp::Eq,
                Op::Lt => CompareOp::Lt,
                Op::LtEq => CompareOp::Le,
                Op::Gt => CompareOp::Gt,
                Op::GtEq => CompareOp::Ge,
                _ => unreachable!(),
            };
            ctx.emit(IrInst::Compare {
                dst,
                op,
                lhs: lhs.value,
                rhs: rhs.value,
            });
            Ok(LoweredValue {
                value: dst,
                ty: IrType::Bool,
            })
        }
        Op::And => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Binary::And".to_string(),
        }),
        Op::Or => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Binary::Or".to_string(),
        }),
    }
}

fn lower_type(ty: &SemanticType) -> Result<IrType, LoweringError> {
    match ty {
        SemanticType::I8 => Ok(IrType::I8),
        SemanticType::I16 => Ok(IrType::I16),
        SemanticType::I32 => Ok(IrType::I32),
        SemanticType::I64 => Ok(IrType::I64),
        SemanticType::I128 => Ok(IrType::I128),
        SemanticType::F64 => Ok(IrType::F64),
        SemanticType::Bool => Ok(IrType::Bool),
        SemanticType::Numeric => Err(LoweringError::UnsupportedSemanticType {
            ty: "Numeric".to_string(),
        }),
        SemanticType::Unknown => Err(LoweringError::UnsupportedSemanticType {
            ty: "Unknown".to_string(),
        }),
        SemanticType::Handle(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Handle".to_string(),
        }),
        SemanticType::StrRef => Err(LoweringError::UnsupportedSemanticType {
            ty: "StrRef".to_string(),
        }),
        SemanticType::Container => Err(LoweringError::UnsupportedSemanticType {
            ty: "Container".to_string(),
        }),
        SemanticType::Str => Err(LoweringError::UnsupportedSemanticType {
            ty: "Str".to_string(),
        }),
        SemanticType::Char => Err(LoweringError::UnsupportedSemanticType {
            ty: "Char".to_string(),
        }),
        SemanticType::Enum(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Enum".to_string(),
        }),
    }
}

fn ensure_type_match(context: &str, expected: IrType, got: IrType) -> Result<(), LoweringError> {
    if expected == got {
        Ok(())
    } else {
        Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "{context} type mismatch after semantic analysis: expected {expected:?}, got {got:?}"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::{Op, Type};
    use crate::frontend::semantic_types::{FunctionId, SemanticFunction};
    use crate::ir::instr::{BinaryOp, CompareOp, IrInst, IrTerminator};

    fn int_expr(value: i128, ty: SemanticType) -> SemanticExpr {
        SemanticExpr {
            ty,
            kind: SemanticExprKind::Value(SemanticValue::Num(value)),
        }
    }

    fn float_expr(value: f64) -> SemanticExpr {
        SemanticExpr {
            ty: SemanticType::F64,
            kind: SemanticExprKind::Value(SemanticValue::Float(value)),
        }
    }

    fn bool_expr(value: bool) -> SemanticExpr {
        SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Value(SemanticValue::Bool(value)),
        }
    }

    fn binding_ref(binding: BindingId, name: &str, ty: SemanticType) -> SemanticExpr {
        SemanticExpr {
            ty,
            kind: SemanticExprKind::VarRef {
                binding,
                name: name.to_string(),
            },
        }
    }

    fn typed_assign(
        binding: BindingId,
        name: &str,
        ty: SemanticType,
        expr: SemanticExpr,
    ) -> SemanticStmt {
        SemanticStmt::TypedAssign {
            binding,
            name: name.to_string(),
            ty,
            expr,
            pos_type: 0,
        }
    }

    #[test]
    fn lowers_top_level_typed_assign_into_synthetic_main() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I64,
                int_expr(7, SemanticType::I64),
            )],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].name, "main");
        assert_eq!(module.functions[0].blocks.len(), 1);
    }

    #[test]
    fn declaration_only_decl_does_not_invent_ssa_value() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::Decl {
                binding: BindingId(1),
                name: "x".to_string(),
                ty: Some(SemanticType::I64),
                pos: 0,
            }],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        assert!(module.functions[0].blocks[0].insts.is_empty());
    }

    #[test]
    fn lowers_arithmetic_to_binary_with_correct_op_and_type() {
        let expr = SemanticExpr {
            ty: SemanticType::I64,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(int_expr(1, SemanticType::I64)),
                op: Op::Plus,
                pos: 0,
                rhs: Box::new(int_expr(2, SemanticType::I64)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![typed_assign(BindingId(1), "x", SemanticType::I64, expr)],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        assert!(module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(
                inst,
                IrInst::Binary {
                    op: BinaryOp::Add,
                    ty: IrType::I64,
                    ..
                }
            )
        }));
    }

    #[test]
    fn lowers_comparisons_to_compare_for_equality_and_ordering() {
        let eq_expr = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(int_expr(1, SemanticType::I64)),
                op: Op::EqEq,
                pos: 0,
                rhs: Box::new(int_expr(1, SemanticType::I64)),
            },
        };
        let lt_expr = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(float_expr(1.0)),
                op: Op::Lt,
                pos: 0,
                rhs: Box::new(float_expr(2.0)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(BindingId(1), "eq", SemanticType::Bool, eq_expr),
                typed_assign(BindingId(2), "lt", SemanticType::Bool, lt_expr),
            ],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        let insts = &module.functions[0].blocks[0].insts;
        assert!(insts.iter().any(|inst| matches!(
            inst,
            IrInst::Compare {
                op: CompareOp::Eq,
                ..
            }
        )));
        assert!(insts.iter().any(|inst| matches!(
            inst,
            IrInst::Compare {
                op: CompareOp::Lt,
                ..
            }
        )));
        assert!(insts.iter().any(|inst| matches!(
            inst,
            IrInst::SsaBind {
                ty: IrType::Bool,
                ..
            }
        )));
    }

    #[test]
    fn lowers_explicit_cast_to_cast_instruction() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I64,
                SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Cast {
                        expr: Box::new(int_expr(1, SemanticType::I128)),
                        from: SemanticType::I128,
                        to: SemanticType::I64,
                    },
                },
            )],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        assert!(module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(
                inst,
                IrInst::Cast {
                    from: IrType::I128,
                    to: IrType::I64,
                    ..
                }
            )
        }));
    }

    #[test]
    fn variable_references_use_current_ssa_mapping() {
        let binding = BindingId(1);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(
                    binding,
                    "x",
                    SemanticType::I64,
                    int_expr(2, SemanticType::I64),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(binding, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        let insts = &module.functions[0].blocks[0].insts;
        // The typed assign should produce exactly one SsaBind.
        // The ExprStmt with a VarRef just looks up the existing value — no new bind emitted.
        let binds: Vec<_> = insts.iter().filter_map(|inst| match inst {
            IrInst::SsaBind {
                dst,
                src,
                ty: IrType::I64,
            } => Some((*dst, *src)),
            _ => None,
        }).collect();
        assert_eq!(binds.len(), 1, "only the typed-assign should emit a bind");
        let (dst, _src) = binds[0];
        // Verify the binding produced a valid SSA value
        assert!(dst.0 > 0, "SSA value id should be positive");
    }

    #[test]
    fn straight_line_block_ends_in_return_none() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::ExprStmt {
                expr: bool_expr(true),
                pos: 0,
            }],
            enums: vec![],
        };

        let module = lower_program(&program).expect("lowering should succeed");
        assert_eq!(
            module.functions[0].blocks[0].term,
            IrTerminator::Return { value: None }
        );
    }

    #[test]
    fn rejects_numeric_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Numeric,
                SemanticExpr {
                    ty: SemanticType::Numeric,
                    kind: SemanticExprKind::Value(SemanticValue::Num(1)),
                },
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Numeric".to_string()
            }
        );
    }

    #[test]
    fn rejects_unknown_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Unknown,
                SemanticExpr {
                    ty: SemanticType::Unknown,
                    kind: SemanticExprKind::Value(SemanticValue::Unknown),
                },
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Unknown".to_string()
            }
        );
    }

    #[test]
    fn rejects_handle_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Handle(Box::new(SemanticType::I64)),
                bool_expr(true),
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Handle".to_string()
            }
        );
    }

    #[test]
    fn rejects_str_ref_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::StrRef,
                bool_expr(true),
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "StrRef".to_string()
            }
        );
    }

    #[test]
    fn rejects_container_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Container,
                bool_expr(true),
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Container".to_string()
            }
        );
    }

    #[test]
    fn rejects_compound_assign() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::CompoundAssign {
                binding: Some(BindingId(1)),
                name: "x".to_string(),
                op: Op::Plus,
                operand: int_expr(1, SemanticType::I64),
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnresolvedSemanticArtifact {
                artifact: "CompoundAssign".to_string()
            }
        );
    }

    #[test]
    fn rejects_func_def() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::FuncDef(SemanticFunction {
                id: FunctionId(0),
                name: "foo".to_string(),
                params: vec![],
                return_ty: Some(SemanticType::I64),
                body: vec![],
                ret_expr: Some(int_expr(1, SemanticType::I64)),
                pos: 0,
            })],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "FuncDef".to_string()
            }
        );
    }

    #[test]
    fn rejects_call_expression() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::ExprStmt {
                expr: SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Call {
                        callee: "foo".to_string(),
                        function: FunctionId(0),
                        args: vec![],
                    },
                },
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "Call".to_string()
            }
        );
    }

    #[test]
    fn rejects_declared_but_never_assigned_binding_use() {
        let binding = BindingId(1);
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::Decl {
                    binding,
                    name: "x".to_string(),
                    ty: Some(SemanticType::I64),
                    pos: 0,
                },
                SemanticStmt::ExprStmt {
                    expr: binding_ref(binding, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        match lower_program(&program).expect_err("lowering should fail") {
            LoweringError::InternalInvariantViolation { detail } => {
                assert!(detail.contains("referenced before any SSA value was assigned"));
            }
            other => panic!("expected invariant error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_statement() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::While {
                cond: bool_expr(true),
                body: vec![],
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "While".to_string()
            }
        );
    }

    #[test]
    fn empty_program_lowers_to_empty_module() {
        let module = lower_program(&SemanticProgram {
            stmts: vec![],
            enums: vec![],
        })
        .expect("empty program should lower");

        assert!(module.functions.is_empty());
    }

    #[test]
    fn lowers_plain_assign_using_resolved_binding_target() {
        let binding = BindingId(3);
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::Decl {
                    binding,
                    name: "x".to_string(),
                    ty: Some(SemanticType::I64),
                    pos: 0,
                },
                SemanticStmt::Assign {
                    target: SemanticLValue::Binding {
                        binding,
                        name: "x".to_string(),
                        ty: SemanticType::I64,
                    },
                    expr: int_expr(9, SemanticType::I64),
                    pos_eq: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_program(&program).expect("assign should lower");
        assert!(module.functions[0].blocks[0]
            .insts
            .iter()
            .any(|inst| matches!(
                inst,
                IrInst::SsaBind {
                    ty: IrType::I64,
                    ..
                }
            )));
    }

    #[test]
    fn bool_constants_lower_cleanly() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "flag",
                SemanticType::Bool,
                bool_expr(true),
            )],
            enums: vec![],
        };

        let module = lower_program(&program).expect("bool constant should lower");
        assert!(module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(
                inst,
                IrInst::ConstInt {
                    ty: IrType::Bool,
                    value: 1,
                    ..
                }
            )
        }));
    }
}
