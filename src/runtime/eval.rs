use super::runtime::*;
use crate::frontend::{ast::*, types::*};
use crate::frontend::semantic_types::*;
use std::collections::HashMap;

impl RunTime {
    pub fn eval_semantic_expr(&mut self, expr: &SemanticExpr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            SemanticExprKind::Value(sv) => Ok(self.semantic_value_to_runtime(sv)),
            SemanticExprKind::VarRef { binding, name } => {
                self.get_var_by_id(*binding, name, 0)
            }
            SemanticExprKind::Unary { op, expr: inner, pos } => {
                let val = self.eval_semantic_expr(inner)?;
                let result = self.apply_unary(op, val, *pos)?;
                Ok(apply_numeric_cast(result, &expr.ty))
            }
            SemanticExprKind::Binary { lhs, op, pos, rhs } => {
                // Short-circuit semantics for && and ||: evaluate lhs first; if
                // the result is already determined, skip rhs entirely.  This
                // matches the CFG-based JIT lowering added in CX-105.
                if matches!(op, Op::And | Op::Or) {
                    let l = self.eval_semantic_expr(lhs)?;
                    let skip = match (&l, op) {
                        (Value::Bool(false), Op::And) | (Value::TBool(0), Op::And) => true,
                        (Value::Bool(true), Op::Or)  | (Value::TBool(1), Op::Or)  => true,
                        _ => false,
                    };
                    if skip {
                        return Ok(apply_numeric_cast(l, &expr.ty));
                    }
                    let r = self.eval_semantic_expr(rhs)?;
                    let result = self.apply_op(l, op.clone(), *pos, r)?;
                    return Ok(apply_numeric_cast(result, &expr.ty));
                }
                // Evaluation order guarantee: lhs is fully evaluated before rhs.
                // Any side effects in lhs (e.g. function calls with print) occur
                // before any side effects in rhs.  The IR lowering (lower_binary
                // in ir/lower.rs) mirrors this order exactly.  This is a Cx 0.1
                // language guarantee.  See docs/backend/cx_eval_order.md.
                let l = self.eval_semantic_expr(lhs)?;
                let r = self.eval_semantic_expr(rhs)?;
                let result = self.apply_op(l, op.clone(), *pos, r)?;
                Ok(apply_numeric_cast(result, &expr.ty))
            }
            SemanticExprKind::Call { callee, args, .. } => {
                self.call_semantic_func(callee, args, 0)
            }
            SemanticExprKind::DotAccess { container, field, .. } => {
                self.get_field(container, field, 0)
            }
            SemanticExprKind::StructInstance { type_name, fields } => {
                let mut map = HashMap::new();
                for (fname, fexpr) in fields {
                    let val = self.eval_semantic_expr(fexpr)?;
                    map.insert(fname.clone(), val);
                }
                Ok(Value::Struct(type_name.clone(), map))
            }
            SemanticExprKind::ArrayLit { elements } => {
                let mut vals = Vec::new();
                for e in elements {
                    vals.push(self.eval_semantic_expr(e)?);
                }
                Ok(Value::Array(vals))
            }
            SemanticExprKind::Index { target, index, pos } => {
                let arr = self.eval_semantic_expr(target)?;
                let idx = self.eval_semantic_expr(index)?;
                match (arr, idx) {
                    (Value::Array(elems), Value::Num(i)) => {
                        let length = elems.len();
                        elems.get(i as usize).cloned().ok_or(RuntimeError::IndexOutOfBounds { pos: *pos, index: i as i64, length })
                    }
                    _ => Err(RuntimeError::BadAssignTarget { pos: *pos })
                }
            }
            SemanticExprKind::When { expr, arms, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                let result = self.run_semantic_when(val, arms)?;
                Ok(result)
            }
            SemanticExprKind::If { condition, then_body, else_body, pos } => {
                let cond_val = self.eval_semantic_expr(condition)?;
                let is_true = match &cond_val {
                    Value::Bool(b) => *b,
                    Value::TBool(0) => false,
                    Value::TBool(1) => true,
                    // #046 reuses #026: an unknown condition can't choose a value.
                    Value::TBool(2) | Value::Unknown(_) => {
                        return Err(RuntimeError::UnknownCondition { pos: *pos });
                    }
                    Value::Num(n) => *n != 0,
                    _ => false,
                };
                let body = if is_true { then_body } else { else_body };
                // Run the chosen block; its trailing expression is the value
                // (mirroring `run_semantic_when` arm-body execution).
                self.push_scope();
                let mut last_val = Value::Num(0);
                for s in body {
                    let step = match s {
                        SemanticStmt::ExprStmt { expr, .. } => {
                            self.eval_semantic_expr(expr).map(|v| last_val = v)
                        }
                        _ => self.run_semantic_stmt(s).map(|_| ()),
                    };
                    if let Err(e) = step {
                        self.pop_scope();
                        return Err(e);
                    }
                }
                self.pop_scope();
                Ok(last_val)
            }
            SemanticExprKind::MethodCall { instance, method, args, pos, .. } => {
                self.call_semantic_method(instance, method, args, *pos)
            }
            SemanticExprKind::Range { .. } => Ok(Value::Num(0)), // stub
            SemanticExprKind::HandleNew { value, .. } => {
                let val = self.eval_semantic_expr(value)?;
                let h = self.handles.insert(val);
                Ok(Value::Handle(h))
            }
            SemanticExprKind::HandleVal { name, pos, .. } => {
                let val = self.get_var(name, *pos)?;
                if let Value::Handle(h) = val {
                    match self.handles.get(h) {
                        Some(v) => Ok(v.clone()),
                        None => Err(RuntimeError::StaleHandle { pos: *pos }),
                    }
                } else {
                    Err(RuntimeError::StaleHandle { pos: *pos })
                }
            }
            SemanticExprKind::HandleDrop { name, pos, .. } => {
                let val = self.get_var(name, *pos)?;
                if let Value::Handle(h) = val {
                    self.handles.remove(h);
                    Ok(Value::Num(0))
                } else {
                    Err(RuntimeError::StaleHandle { pos: *pos })
                }
            }
            SemanticExprKind::ResultOk { expr } => {
                let val = self.eval_semantic_expr(expr)?;
                Ok(Value::ResultOk(Box::new(val)))
            }
            SemanticExprKind::ResultErr { expr } => {
                let val = self.eval_semantic_expr(expr)?;
                Ok(Value::ResultErr(Box::new(val)))
            }
            SemanticExprKind::Try { expr, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                match val {
                    Value::ResultOk(v) => Ok(*v),
                    Value::ResultErr(e) => Err(RuntimeError::EarlyReturn(Value::ResultErr(e))),
                    _ => Ok(val),
                }
            }
            SemanticExprKind::Cast { expr, to, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                Ok(apply_numeric_cast(val, to))
            }
        }
    }

    pub(crate) fn semantic_value_to_runtime(&mut self, sv: &SemanticValue) -> Value {
        match sv {
            SemanticValue::Num(n) => Value::Num(*n),
            SemanticValue::Float(f) => Value::Float(*f),
            SemanticValue::Str(s) => {
                let (off, len) = self.alloc_str(s);
                Value::Str(off, len)
            }
            SemanticValue::Bool(b) => Value::Bool(*b),
            SemanticValue::Char(c) => Value::Char(*c),
            SemanticValue::EnumVariant { enum_name, variant_name, .. } => {
                Value::EnumVariant(enum_name.clone(), variant_name.clone())
            }
            SemanticValue::Unknown => Value::Unknown(Type::T32),
        }
    }
}
