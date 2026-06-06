use super::runtime::*;
use crate::frontend::{ast::*, types::*};
use crate::frontend::semantic_types::*;
use std::sync::Arc;

impl RunTime {
    pub fn run_semantic_stmt(&mut self, stmt: &SemanticStmt) -> Result<(), RuntimeError> {
        match stmt {
            SemanticStmt::Noop => Ok(()),
SemanticStmt::Decl { binding, name, ty, .. } => {
                let rt_ty: Option<Type> = ty.as_ref().map(|t| t.clone().into());
                self.declare(*binding, name.clone(), rt_ty, 0)
            }
            SemanticStmt::TypedAssign { binding, name, ty, expr, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                let rt_ty: Type = ty.clone().into();
                let val = match (&rt_ty, val) {
                    (Type::Bool, Value::Unknown(_)) => Value::TBool(2),
                    (_, v) => v,
                };
                self.set_var_typed(*binding, name.clone(), rt_ty, val, 0)
            }
            SemanticStmt::Assign { target, expr, pos_eq } => {
                let val = self.eval_semantic_expr(expr)?;
                match target {
                    SemanticLValue::Binding { binding, name, ty } => {
                        let truncated = apply_numeric_cast(val, ty);
                        self.set_var_by_id(*binding, name, truncated, 0)
                    }
                    SemanticLValue::DotAccess { container, field, ty, .. } => {
                        let truncated = apply_numeric_cast(val, ty);
                        self.set_container_field(container, field, truncated, 0)
                    }
                    SemanticLValue::Index { target, index, elem_ty } => {
                        let arr_name = match &target.kind {
                            SemanticExprKind::VarRef { name, .. } => name.clone(),
                            _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                        };
                        let idx = match self.eval_semantic_expr(index)? {
                            Value::Num(n) => n as usize,
                            _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                        };
                        let truncated = apply_numeric_cast(val, elem_ty);
                        self.set_array_element(&arr_name, idx, truncated, *pos_eq)
                    }
                }
            }
            SemanticStmt::CompoundAssign { target, op, operand, pos } => {
                match target {
                    SemanticLValue::Binding { binding, name, ty } => {
                        let current = self.get_var_by_id(*binding, name, 0)?;
                        let rhs = self.eval_semantic_expr(operand)?;
                        let result = self.apply_op(current, op.clone(), 0, rhs)?;
                        let truncated = apply_numeric_cast(result, ty);
                        self.set_var_by_id(*binding, name, truncated, 0)
                    }
                    SemanticLValue::DotAccess { container, field, ty, .. } => {
                        let current = self.get_field(container, field, 0)?;
                        let rhs = self.eval_semantic_expr(operand)?;
                        let result = self.apply_op(current, op.clone(), 0, rhs)?;
                        let truncated = apply_numeric_cast(result, ty);
                        self.set_container_field(container, field, truncated, 0)
                    }
                    SemanticLValue::Index { target, index, elem_ty } => {
                        let arr_name = match &target.kind {
                            SemanticExprKind::VarRef { name, .. } => name.clone(),
                            _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                        };
                        let idx = match self.eval_semantic_expr(index)? {
                            Value::Num(n) => n as usize,
                            _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                        };
                        let current_val = {
                            let arr = self.get_var(&arr_name, 0)?;
                            match arr {
                                Value::Array(elems) => {
                                    let length = elems.len();
                                    elems
                                        .get(idx)
                                        .cloned()
                                        .ok_or(RuntimeError::IndexOutOfBounds {
                                            pos: *pos,
                                            index: idx as i64,
                                            length,
                                        })?
                                }
                                _ => return Err(RuntimeError::NotAContainer {
                                    pos: 0,
                                    name: arr_name.clone(),
                                }),
                            }
                        };
                        let rhs = self.eval_semantic_expr(operand)?;
                        let result = self.apply_op(current_val, op.clone(), 0, rhs)?;
                        let truncated = apply_numeric_cast(result, elem_ty);
                        self.set_array_element(&arr_name, idx, truncated, *pos)
                    }
                }
            }
            SemanticStmt::Return { expr, .. } => {
                if let Some(e) = expr {
                    let val = self.eval_semantic_expr(e)?;
                    Err(RuntimeError::EarlyReturn(val))
                } else {
                    Err(RuntimeError::EarlyReturn(Value::Num(0)))
                }
            }
            SemanticStmt::ExprStmt { expr, .. } => {
                self.eval_semantic_expr(expr)?;
                Ok(())
            }
            SemanticStmt::Block { stmts, .. } => {
                self.push_scope();
                for s in stmts {
                    match self.run_semantic_stmt(s) {
                        Ok(_) => {}
                        Err(e) => { self.pop_scope(); return Err(e); }
                    }
                }
                self.pop_scope();
                Ok(())
            }
            SemanticStmt::While { cond, body, .. } => {
                loop {
                    let cv = self.eval_semantic_expr(cond)?;
                    match cv {
                        Value::Bool(false) | Value::TBool(0) => break,
                        Value::Bool(true) | Value::TBool(1) => {}
                        _ => break,
                    }
                    self.push_scope();
                    let mut should_break = false;
                    for s in body {
                        match self.run_semantic_stmt(s) {
                            Ok(_) => {}
                            Err(RuntimeError::BreakSignal) => { should_break = true; break; }
                            Err(RuntimeError::ContinueSignal) => break,
                            Err(e) => { self.pop_scope(); return Err(e); }
                        }
                    }
                    self.pop_scope();
                    if should_break { break; }
                }
                Ok(())
            }
            SemanticStmt::For { binding, var, start, end, inclusive, body, .. } => {
                let start_val = match self.eval_semantic_expr(start)? {
                    Value::Num(n) => n,
                    _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                };
                let end_val = match self.eval_semantic_expr(end)? {
                    Value::Num(n) => n,
                    _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                };
                'sem_for: {
                    if *inclusive {
                        for i in start_val..=end_val {
                            self.push_scope();
                            self.declare(*binding, var.clone(), None, 0)?;
                            self.set_var_by_id(*binding, var, Value::Num(i), 0)?;
                            for s in body {
                                match self.run_semantic_stmt(s) {
                                    Ok(_) => {}
                                    Err(RuntimeError::BreakSignal) => { self.pop_scope(); break 'sem_for; }
                                    Err(RuntimeError::ContinueSignal) => break,
                                    Err(e) => { self.pop_scope(); return Err(e); }
                                }
                            }
                            self.pop_scope();
                        }
                    } else {
                        for i in start_val..end_val {
                            self.push_scope();
                            self.declare(*binding, var.clone(), None, 0)?;
                            self.set_var_by_id(*binding, var, Value::Num(i), 0)?;
                            for s in body {
                                match self.run_semantic_stmt(s) {
                                    Ok(_) => {}
                                    Err(RuntimeError::BreakSignal) => { self.pop_scope(); break 'sem_for; }
                                    Err(RuntimeError::ContinueSignal) => break,
                                    Err(e) => { self.pop_scope(); return Err(e); }
                                }
                            }
                            self.pop_scope();
                        }
                    }
                }
                Ok(())
            }
            SemanticStmt::Loop { body, .. } => {
                loop {
                    self.push_scope();
                    let mut should_break = false;
                    for s in body {
                        match self.run_semantic_stmt(s) {
                            Ok(_) => {}
                            Err(RuntimeError::BreakSignal) => { should_break = true; break; }
                            Err(RuntimeError::ContinueSignal) => break,
                            Err(e) => { self.pop_scope(); return Err(e); }
                        }
                    }
                    self.pop_scope();
                    if should_break { break; }
                }
                Ok(())
            }
            SemanticStmt::Break { .. } => Err(RuntimeError::BreakSignal),
            SemanticStmt::Continue { .. } => Err(RuntimeError::ContinueSignal),
            SemanticStmt::FuncDef(sem_func) => {
                self.semantic_funcs.insert(sem_func.name.clone(), Arc::new(sem_func.clone()));
                Ok(())
            }
            SemanticStmt::StructDef { name, fields, .. } => {
                self.structs.insert(name.clone(), fields.iter().map(|(n, t)| (n.clone(), t.clone().into())).collect());
                Ok(())
            }
            SemanticStmt::ImplBlock { aliases, methods, method_alias_params, .. } => {
                for (mi, sem_func) in methods.iter().enumerate() {
                    // Carry the per-method alias BindingIds (the ids the method
                    // body's alias VarRefs were resolved against, #009) alongside
                    // each alias name+type. method_alias_params[mi] is parallel to
                    // `aliases` (both in declaration order).
                    let method_aliases: Vec<(BindingId, String, SemanticType)> = method_alias_params
                        .get(mi)
                        .map(|params| {
                            params
                                .iter()
                                .enumerate()
                                .map(|(ai, p)| {
                                    let ty = aliases
                                        .get(ai)
                                        .map(|(_, t)| t.clone())
                                        .or_else(|| p.ty.clone())
                                        .unwrap_or(SemanticType::Unknown);
                                    (p.binding, p.name.clone(), ty)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    for (_, alias_type) in aliases {
                        let type_key = match alias_type {
                            SemanticType::Struct(n) => n.clone(),
                            _ => continue,
                        };
                        self.semantic_impls.insert(
                            (type_key, sem_func.name.clone()),
                            (method_aliases.clone(), Arc::new(sem_func.clone())),
                        );
                    }
                }
                Ok(())
            }
            SemanticStmt::ConstDecl { binding, name, ty, value, .. } => {
                let val = self.eval_semantic_expr(value)?;
                self.consts.insert(name.clone(), val.clone());
                let ast_ty: Type = ty.clone().into();
                self.set_var_typed(*binding, name.clone(), ast_ty, val, 0)?;
                Ok(())
            }
            SemanticStmt::EnumDef { .. } => {
                // Enum variants are resolved at semantic analysis time
                // Runtime registration not needed — when matching uses SemanticWhenPattern
                Ok(())
            }
            SemanticStmt::When { expr, arms, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                self.run_semantic_when(val, arms)?;
                Ok(())
            }
            SemanticStmt::IfElse { condition, then_body, else_ifs, else_body, pos } => {
                let cond_val = self.eval_semantic_expr(condition)?;
                let is_true = match &cond_val {
                    Value::Bool(b) => *b,
                    Value::TBool(0) => false,
                    Value::TBool(1) => true,
                    // An unknown TBool condition can't choose a branch — error
                    // instead of silently taking `else` (tracker #026).
                    Value::TBool(2) | Value::Unknown(_) => {
                        return Err(RuntimeError::UnknownCondition { pos: *pos });
                    }
                    Value::Num(n) => *n != 0,
                    _ => false,
                };

                if is_true {
                    self.push_scope();
                    for stmt in then_body {
                        match self.run_semantic_stmt(stmt) {
                            Ok(_) => {}
                            Err(e) => { self.pop_scope(); return Err(e); }
                        }
                    }
                    self.pop_scope();
                    return Ok(());
                }

                for (else_if_cond, else_if_body) in else_ifs {
                    let cond_val = self.eval_semantic_expr(else_if_cond)?;
                    let is_true = match &cond_val {
                        Value::Bool(b) => *b,
                        Value::TBool(0) => false,
                        Value::TBool(1) => true,
                        // Unknown `else if` condition — same rule as `if` (#026).
                        Value::TBool(2) | Value::Unknown(_) => {
                            return Err(RuntimeError::UnknownCondition { pos: *pos });
                        }
                        Value::Num(n) => *n != 0,
                        _ => false,
                    };
                    if is_true {
                        self.push_scope();
                        for stmt in else_if_body {
                            match self.run_semantic_stmt(stmt) {
                                Ok(_) => {}
                                Err(e) => { self.pop_scope(); return Err(e); }
                            }
                        }
                        self.pop_scope();
                        return Ok(());
                    }
                }

                if let Some(else_body) = else_body {
                    self.push_scope();
                    for stmt in else_body {
                        match self.run_semantic_stmt(stmt) {
                            Ok(_) => {}
                            Err(e) => { self.pop_scope(); return Err(e); }
                        }
                    }
                    self.pop_scope();
                }

                Ok(())
            }
            SemanticStmt::WhileIn { arr, start_slot, range_start, range_end, inclusive, body, then_chains, result, .. } => {
                let start_val = match self.eval_semantic_expr(range_start)? {
                    Value::Num(n) => n,
                    _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                };
                let end_val = match self.eval_semantic_expr(range_end)? {
                    Value::Num(n) => n,
                    _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                };

                // Primary chain
                'primary: {
                    if *inclusive {
                        for i in start_val..=end_val {
                            let elem = {
                                let arr_val = self.get_var(arr, 0)?;
                                match arr_val {
                                    Value::Array(elems) => elems.get(i as usize).cloned().unwrap_or(Value::Unknown(Type::Unknown)),
                                    _ => return Err(RuntimeError::NotAContainer { pos: 0, name: arr.clone() }),
                                }
                            };
                            {
                                let arr_val = self.get_var(arr, 0)?;
                                if let Value::Array(mut elems) = arr_val {
                                    if *start_slot < elems.len() { elems[*start_slot] = elem; }
                                    self.set_var(arr.clone(), Value::Array(elems), 0)?;
                                }
                            }
                            self.push_scope();
                            let mut should_break = false;
                            for stmt in body {
                                match self.run_semantic_stmt(stmt) {
                                    Ok(_) => {}
                                    Err(RuntimeError::BreakSignal) => { should_break = true; break; }
                                    Err(RuntimeError::ContinueSignal) => break,
                                    Err(e) => { self.pop_scope(); return Err(e); }
                                }
                            }
                            self.pop_scope();
                            if should_break { break 'primary; }
                        }
                    } else {
                        for i in start_val..end_val {
                            let elem = {
                                let arr_val = self.get_var(arr, 0)?;
                                match arr_val {
                                    Value::Array(elems) => elems.get(i as usize).cloned().unwrap_or(Value::Unknown(Type::Unknown)),
                                    _ => return Err(RuntimeError::NotAContainer { pos: 0, name: arr.clone() }),
                                }
                            };
                            {
                                let arr_val = self.get_var(arr, 0)?;
                                if let Value::Array(mut elems) = arr_val {
                                    if *start_slot < elems.len() { elems[*start_slot] = elem; }
                                    self.set_var(arr.clone(), Value::Array(elems), 0)?;
                                }
                            }
                            self.push_scope();
                            let mut should_break = false;
                            for stmt in body {
                                match self.run_semantic_stmt(stmt) {
                                    Ok(_) => {}
                                    Err(RuntimeError::BreakSignal) => { should_break = true; break; }
                                    Err(RuntimeError::ContinueSignal) => break,
                                    Err(e) => { self.pop_scope(); return Err(e); }
                                }
                            }
                            self.pop_scope();
                            if should_break { break 'primary; }
                        }
                    }
                }

                // Then chains
                for chain in then_chains {
                    let chain_start = match self.eval_semantic_expr(&chain.range_start)? {
                        Value::Num(n) => n,
                        _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                    };
                    let chain_end = match self.eval_semantic_expr(&chain.range_end)? {
                        Value::Num(n) => n,
                        _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
                    };
                    if chain.inclusive {
                        for i in chain_start..=chain_end {
                            let elem = {
                                let arr_val = self.get_var(&chain.arr, 0)?;
                                match arr_val {
                                    Value::Array(elems) => elems.get(i as usize).cloned().unwrap_or(Value::Unknown(Type::Unknown)),
                                    _ => return Err(RuntimeError::NotAContainer { pos: 0, name: chain.arr.clone() }),
                                }
                            };
                            {
                                let arr_val = self.get_var(&chain.arr, 0)?;
                                if let Value::Array(mut elems) = arr_val {
                                    if chain.start_slot < elems.len() { elems[chain.start_slot] = elem; }
                                    self.set_var(chain.arr.clone(), Value::Array(elems), 0)?;
                                }
                            }
                            self.push_scope();
                            let mut should_break = false;
                            for stmt in &chain.body {
                                match self.run_semantic_stmt(stmt) {
                                    Ok(_) => {}
                                    Err(RuntimeError::BreakSignal) => { should_break = true; break; }
                                    Err(RuntimeError::ContinueSignal) => break,
                                    Err(e) => { self.pop_scope(); return Err(e); }
                                }
                            }
                            self.pop_scope();
                            if should_break { break; }
                        }
                    } else {
                        for i in chain_start..chain_end {
                            let elem = {
                                let arr_val = self.get_var(&chain.arr, 0)?;
                                match arr_val {
                                    Value::Array(elems) => elems.get(i as usize).cloned().unwrap_or(Value::Unknown(Type::Unknown)),
                                    _ => return Err(RuntimeError::NotAContainer { pos: 0, name: chain.arr.clone() }),
                                }
                            };
                            {
                                let arr_val = self.get_var(&chain.arr, 0)?;
                                if let Value::Array(mut elems) = arr_val {
                                    if chain.start_slot < elems.len() { elems[chain.start_slot] = elem; }
                                    self.set_var(chain.arr.clone(), Value::Array(elems), 0)?;
                                }
                            }
                            self.push_scope();
                            let mut should_break = false;
                            for stmt in &chain.body {
                                match self.run_semantic_stmt(stmt) {
                                    Ok(_) => {}
                                    Err(RuntimeError::BreakSignal) => { should_break = true; break; }
                                    Err(RuntimeError::ContinueSignal) => break,
                                    Err(e) => { self.pop_scope(); return Err(e); }
                                }
                            }
                            self.pop_scope();
                            if should_break { break; }
                        }
                    }
                }

                if let Some(result_expr) = result {
                    self.eval_semantic_expr(result_expr)?;
                }

                Ok(())
            }
        }
    }

    pub(crate) fn run_semantic_when(&mut self, val: Value, arms: &[SemanticWhenArm]) -> Result<Value, RuntimeError> {
        for arm in arms {
            let matches = match &arm.pattern {
                SemanticWhenPattern::Literal(sv) => {
                    // #044: a bool `when` scrutinee can arrive in two equivalent
                    // representations — `Value::Bool` / `Value::Unknown` (definite
                    // or untyped-`?` values) or the canonical TBool wire value
                    // `Value::TBool(0|1|2)` (typed `bool` declarations via the
                    // coercion above, and TBool arithmetic in ops.rs). Match a
                    // bool pattern against BOTH forms, mirroring #026's
                    // `if`-condition handling, so e.g. a typed `b: bool = ?`
                    // (stored as `TBool(2)`) fires the `unknown` arm instead of
                    // silently falling through. TBool 0/1/2 == false/true/unknown
                    // by the fixed ABI wire definition, so these are exact, not
                    // lenient, matches. The true/false TBool arms are defensive:
                    // definite values reach `when` as `Value::Bool` today, but a
                    // future `TBool(0|1)`-producing path would otherwise mismatch
                    // the same way `unknown` did. Once the `unknown` arm fires for
                    // `TBool(2)`, #027's syntactic exhaustiveness is sound with no
                    // checker change.
                    match sv {
                        SemanticValue::Bool(true) => matches!(&val, Value::Bool(true) | Value::TBool(1)),
                        SemanticValue::Bool(false) => matches!(&val, Value::Bool(false) | Value::TBool(0)),
                        SemanticValue::Unknown => matches!(&val, Value::Unknown(_) | Value::TBool(2)),
                        _ => {
                            let pat_val = self.semantic_value_to_runtime(sv);
                            match (&val, &pat_val) {
                                (Value::Str(vo, vl), Value::Str(po, pl)) => {
                                    self.resolve_str(*vo, *vl) == self.resolve_str(*po, *pl)
                                }
                                _ => val == pat_val,
                            }
                        }
                    }
                }
                SemanticWhenPattern::Range(lo, hi, inclusive) => {
                    let lo_val = self.semantic_value_to_runtime(lo);
                    let hi_val = self.semantic_value_to_runtime(hi);
                    match (&val, &lo_val, &hi_val) {
                        (Value::Num(v), Value::Num(l), Value::Num(h)) => {
                            if *inclusive { v >= l && v <= h } else { v >= l && v < h }
                        }
                        _ => false,
                    }
                }
                SemanticWhenPattern::EnumVariant { enum_name, variant_name, .. } => {
                    match &val {
                        Value::EnumVariant(e, v) => e == enum_name && v == variant_name,
                        _ => false,
                    }
                }
                SemanticWhenPattern::Catchall => true,
            };
            if matches {
                self.push_scope();
                let mut last_val = Value::Num(0);
                for s in &arm.body {
                    match s {
                        SemanticStmt::ExprStmt { expr, .. } => {
                            last_val = self.eval_semantic_expr(expr)?;
                        }
                        _ => {
                            match self.run_semantic_stmt(s) {
                                Ok(_) => {}
                                Err(e) => { self.pop_scope(); return Err(e); }
                            }
                        }
                    }
                }
                self.pop_scope();
                return Ok(last_val);
            }
        }
        Ok(Value::Num(0))
    }
}
