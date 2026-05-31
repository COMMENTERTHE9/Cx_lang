use super::runtime::*;
use crate::frontend::{ast::*, types::*};

impl RunTime {
    pub(crate) fn apply_op(
        &self,
        left: Value,
        op: Op,
        pos: usize,
        right: Value,
    ) -> Result<Value, RuntimeError> {
        match (&left, &op, &right) {
            (Value::Bool(false), Op::And, Value::Unknown(_)) => return Ok(Value::Bool(false)),
            (Value::Unknown(_), Op::And, Value::Bool(false)) => return Ok(Value::Bool(false)),
            (Value::Bool(true), Op::Or, Value::Unknown(_)) => return Ok(Value::Bool(true)),
            (Value::Unknown(_), Op::Or, Value::Bool(true)) => return Ok(Value::Bool(true)),
            (Value::TBool(0), Op::And, _) => return Ok(Value::TBool(0)),
            (_, Op::And, Value::TBool(0)) => return Ok(Value::TBool(0)),
            (Value::TBool(1), Op::Or, _) => return Ok(Value::TBool(1)),
            (_, Op::Or, Value::TBool(1)) => return Ok(Value::TBool(1)),
            (Value::Unknown(_), Op::Mul, Value::Num(0)) => return Ok(Value::Num(0)),
            (Value::Num(0), Op::Mul, Value::Unknown(_)) => return Ok(Value::Num(0)),
            (Value::Unknown(ty), _, _) => return Ok(Value::Unknown(ty.clone())),
            (_, _, Value::Unknown(ty)) => return Ok(Value::Unknown(ty.clone())),
            _ => {}
        }

        match op {
            Op::Plus => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a.wrapping_add(*b))),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        Ok(Value::Float(a + b))
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Minus => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a.wrapping_sub(*b))),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        Ok(Value::Float(a - b))
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Mul => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => {
                    Ok(Value::Num(a.wrapping_mul(*b)))
                }
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        Ok(Value::Float(a * b))
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Div => match (&left, &right) {
                (Value::Num(_), Value::Num(0)) => Err(RuntimeError::DivByZero { pos }),
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(if *b == -1 { a.wrapping_neg() } else { a / b })),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        if b == 0.0 {
                            Err(RuntimeError::DivByZero { pos })
                        } else {
                            Ok(Value::Float(a / b))
                        }
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Mod => match (&left, &right) {
                (Value::Num(_), Value::Num(0)) => Err(RuntimeError::DivByZero { pos }),
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(if *b == -1 { 0 } else { a % b })),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        if b == 0.0 {
                            Err(RuntimeError::DivByZero { pos })
                        } else {
                            Ok(Value::Float(a % b))
                        }
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::EqEq => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a == b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a == b)),
                (Value::Num(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) == *b)),
                (Value::Float(a), Value::Num(b)) => Ok(Value::Bool(*a == (*b as f64))),
                (Value::Str(a_off, a_len), Value::Str(b_off, b_len)) => Ok(Value::Bool(
                    self.resolve_str(*a_off, *a_len) == self.resolve_str(*b_off, *b_len),
                )),
                (Value::Char(a), Value::Char(b)) => Ok(Value::Bool(a == b)),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
                (Value::TBool(2), _) | (_, Value::TBool(2)) => Ok(Value::TBool(2)),
                (Value::TBool(a), Value::TBool(b)) => Ok(Value::Bool(a == b)),
                (Value::TBool(a), Value::Bool(b)) => Ok(Value::Bool((*a == 1) == *b)),
                (Value::Bool(a), Value::TBool(b)) => Ok(Value::Bool(*a == (*b == 1))),
                (l, r) => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left: l.clone(),
                    right: r.clone(),
                }),
            },
            Op::NotEq => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a != b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a != b)),
                (Value::Num(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) != *b)),
                (Value::Float(a), Value::Num(b)) => Ok(Value::Bool(*a != (*b as f64))),
                (Value::Str(a_off, a_len), Value::Str(b_off, b_len)) => Ok(Value::Bool(
                    self.resolve_str(*a_off, *a_len) != self.resolve_str(*b_off, *b_len),
                )),
                (Value::Char(a), Value::Char(b)) => Ok(Value::Bool(a != b)),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a != b)),
                (Value::TBool(2), _) | (_, Value::TBool(2)) => Ok(Value::TBool(2)),
                (Value::TBool(a), Value::TBool(b)) => Ok(Value::Bool(a != b)),
                (Value::TBool(a), Value::Bool(b)) => Ok(Value::Bool((*a == 1) != *b)),
                (Value::Bool(a), Value::TBool(b)) => Ok(Value::Bool(*a != (*b == 1))),
                (l, r) => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left: l.clone(),
                    right: r.clone(),
                }),
            },
            Op::Lt => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a < b)),
                _ => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left,
                    right,
                }),
            },
            Op::Gt => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a > b)),
                _ => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left,
                    right,
                }),
            },
            Op::LtEq => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a <= b)),
                _ => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left,
                    right,
                }),
            },
            Op::GtEq => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a >= b)),
                _ => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left,
                    right,
                }),
            },
            Op::Not => unreachable!("Op::Not is unary only"),
            Op::And => match (&left, &right) {
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a && *b)),
                (Value::TBool(a), Value::TBool(b)) => Ok(Value::TBool(match (a, b) {
                    (0, _) | (_, 0) => 0,
                    (1, 1) => 1,
                    _ => 2,
                })),
                (Value::Bool(true), Value::TBool(b)) => Ok(Value::TBool(*b)),
                (Value::TBool(a), Value::Bool(true)) => Ok(Value::TBool(*a)),
                (Value::Bool(false), Value::TBool(_)) => Ok(Value::TBool(0)),
                (Value::TBool(_), Value::Bool(false)) => Ok(Value::TBool(0)),
                (l, r) => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left: l.clone(),
                    right: r.clone(),
                }),
            },
            Op::Or => match (&left, &right) {
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(*a || *b)),
                (Value::TBool(a), Value::TBool(b)) => Ok(Value::TBool(match (a, b) {
                    (1, _) | (_, 1) => 1,
                    (0, 0) => 0,
                    _ => 2,
                })),
                (Value::Bool(false), Value::TBool(b)) => Ok(Value::TBool(*b)),
                (Value::TBool(a), Value::Bool(false)) => Ok(Value::TBool(*a)),
                (Value::Bool(true), Value::TBool(_)) => Ok(Value::TBool(1)),
                (Value::TBool(_), Value::Bool(true)) => Ok(Value::TBool(1)),
                (l, r) => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left: l.clone(),
                    right: r.clone(),
                }),
            },
        }
    }

    // ── Semantic IR interpreter ──────────────────────────────────────

    pub(crate) fn apply_unary(&self, op: &Op, val: Value, pos: usize) -> Result<Value, RuntimeError> {
        match (op, val) {
            (Op::Minus, Value::Num(n)) => Ok(Value::Num(n.wrapping_neg())),
            (Op::Minus, Value::Float(f)) => Ok(Value::Float(-f)),
            (Op::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            (Op::Not, Value::TBool(n)) => Ok(Value::TBool(if n == 0 { 1 } else if n == 1 { 0 } else { 2 })),
            (Op::Mul, Value::Array(elems)) => {
                elems.get(0).cloned().ok_or_else(|| RuntimeError::BadAssignTarget { pos })
            }
            (Op::Mul, v) => Ok(v),
            _ => Err(RuntimeError::TypeMismatch { pos, expected: Type::Unknown, got: Type::Unknown }),
        }
    }
}
