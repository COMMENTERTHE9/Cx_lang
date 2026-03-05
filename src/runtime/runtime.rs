use std::collections::{HashMap, HashSet};
use crate::frontend::{ast::*, diagnostics, types::*};
use crate::runtime::arena::Arena;

#[derive(Debug)]
pub enum ScopeEvent {
    Open(String),
    Close(String),
    Add(String, Value),
    Mutate(String, Value),
    Free(String),
    BleedBack(String, Value),
    ArenaReset { bytes: usize, chunks: usize },
}


#[derive(Debug, Clone)]
pub struct FuncDef {
    params: Vec<ParamKind>,
    ret_ty: Option<Type>,
    body: Vec<Stmt>,
    ret_expr: Option<Expr>,
}

pub struct ScopeFrame {
    pub vars: HashMap<String, VarEntry>,
    pub freed: HashSet<String>,
    pub bleed_back: HashMap<String, (usize, String)>,
    pub arena: Option<Arena>,
    // inner param name -> (outer scope index, outer var name)
}

pub struct RunTime {
    scopes: Vec<ScopeFrame>,
    order: Vec<String>,
    seen: HashSet<String>,
    funcs: HashMap<String, FuncDef>,
    pub debug_scope: bool,
}

impl RunTime {
    fn resolve_assigned_value(&self, value: Value, pos: usize) -> Result<Value, RuntimeError> {
        match value {
            Value::Str(s) => Ok(Value::Str(expand_template(self, &s, pos)?)),
            other => Ok(other),
        }
    }

    fn track_in_arena(&mut self, value: &Value) {
        let size = match value {
            Value::Str(s) => s.len() + 1,
            Value::Container(map) => map.iter().map(|(k, _)| k.len() + 16).sum::<usize>() + 32,
            _ => return, // numbers, bools, chars not arena tracked
        };

        for frame in self.scopes.iter_mut().rev() {
            if let Some(arena) = &mut frame.arena {
                arena.alloc(size, 1);
                return;
            }
        }
    }

    pub fn new() -> Self {
        Self {
            scopes: vec![ScopeFrame {
                vars: HashMap::new(),
                freed: HashSet::new(),
                bleed_back: HashMap::new(),
                arena: None, // top level is not a function scope
            }],
            order: Vec::new(),
            seen: HashSet::new(),
            funcs: HashMap::new(),
            debug_scope: false,
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(ScopeFrame {
            vars: HashMap::new(),
            freed: HashSet::new(),
            bleed_back: HashMap::new(),
            arena: None, // block scope - no arena
        });
        if self.debug_scope {
            diagnostics::print_scope_event(&ScopeEvent::Open(format!(
                "scope#{}",
                self.scopes.len() - 1
            )));
        }
    }

    pub fn push_function_scope(&mut self) {
        self.scopes.push(ScopeFrame {
            vars: HashMap::new(),
            freed: HashSet::new(),
            bleed_back: HashMap::new(),
            arena: Some(Arena::new()), // function scope - gets its own arena
        });
        if self.debug_scope {
            diagnostics::print_scope_event(&ScopeEvent::Open(format!(
                "scope#{}",
                self.scopes.len() - 1
            )));
        }
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.is_empty() {
            self.scopes.pop();
            return;
        }

        let (bleed_values, bleed_events, free_names, close_label, had_arena) = {
            let frame = self.scopes.last().unwrap();
            // bleed-back - write final values back to outer scope
            let bleeds: Vec<(String, usize, String)> = frame
                .bleed_back
                .iter()
                .filter(|(param_name, _)| !frame.freed.contains(*param_name))
                .map(|(param_name, (outer_idx, outer_name))| {
                    (param_name.clone(), *outer_idx, outer_name.clone())
                })
                .collect();

            let bleed_values: Vec<(usize, String, Value)> = bleeds
                .iter()
                .filter_map(|(param_name, outer_idx, outer_name)| {
                    frame
                        .vars
                        .get(param_name)
                        .and_then(|entry| entry.val.clone())
                        .map(|val| (*outer_idx, outer_name.clone(), val))
                })
                .collect();

            let bleed_events: Vec<(String, Value)> = bleeds
                .iter()
                .filter_map(|(param_name, _, outer_name)| {
                    frame
                        .vars
                        .get(param_name)
                        .and_then(|entry| entry.val.clone())
                        .map(|val| (outer_name.clone(), val))
                })
                .collect();

            let free_names: Vec<String> = frame
                .vars
                .keys()
                .filter(|name| !frame.freed.contains(*name))
                .cloned()
                .collect();

            // run normal cleanup
            for (name, _val) in &frame.vars {
                if !frame.freed.contains(name) {
                    // cleanup - currently just drop
                    // arena-allocated values are handled by arena reset below
                }
            }

            let close_label = format!("scope#{}", self.scopes.len() - 1);
            if self.debug_scope {
                if let Some(arena) = frame.arena.as_ref() {
                    arena.debug_dump();
                }
            }
            // check if frame had an arena — log it if debug
            let had_arena = frame
                .arena
                .as_ref()
                .map(|a| (a.bytes_used(), a.chunk_count()));

            (bleed_values, bleed_events, free_names, close_label, had_arena)
        };

        // pop the frame — arena drops and resets with it
        self.scopes.pop();

        // write bleed-back values AFTER pop so borrow checker is happy
        for (outer_idx, outer_name, val) in bleed_values {
            if let Some(outer_frame) = self.scopes.get_mut(outer_idx) {
                if let Some(entry) = outer_frame.vars.get_mut(&outer_name) {
                    entry.val = Some(val);
                }
            }
        }
        if self.debug_scope {
            for name in &free_names {
                diagnostics::print_scope_event(&ScopeEvent::Free(name.clone()));
            }
            for (name, val) in &bleed_events {
                diagnostics::print_scope_event(&ScopeEvent::BleedBack(name.clone(), val.clone()));
            }
            diagnostics::print_scope_event(&ScopeEvent::Close(close_label));
            if let Some((bytes, chunks)) = had_arena {
                diagnostics::print_scope_event(&ScopeEvent::ArenaReset { bytes, chunks });
            }
        }
    }

    pub fn free_variable(&mut self, name: &str) {
        if let Some(frame) = self.scopes.last_mut() {
            if !frame.freed.contains(name) {
                frame.vars.remove(name);
                frame.freed.insert(name.to_string());
                if self.debug_scope {
                    diagnostics::print_scope_event(&ScopeEvent::Free(name.to_string()));
                }
            }
        }
    }

    pub fn declare(&mut self, name: String, ty: Option<Type>, pos: usize) -> Result<(), RuntimeError> {
        let frame = self.scopes.last_mut().unwrap();

        if frame.vars.contains_key(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        if self.seen.insert(name.clone()) {
            self.order.push(name.clone());
        }

        frame.vars.insert(name, VarEntry { ty, val: None });
        Ok(())
    }

    pub fn set_var(&mut self, name: String, value: Value, pos: usize) -> Result<(), RuntimeError> {
        let value = self.resolve_assigned_value(value, pos)?;
        let mut target_idx = None;
        for i in (0..self.scopes.len()).rev() {
            if self.scopes[i].vars.contains_key(&name) {
                target_idx = Some(i);
                break;
            }
        }

        if let Some(i) = target_idx {
            let mut tracked_value: Option<Value> = None;
            {
                let frame = &mut self.scopes[i];
                let entry = frame.vars.get_mut(&name).unwrap();

                let was_initialized = entry.val.is_some();
                if entry.ty.is_none() {
                    entry.ty = Some(type_of_value(&value));
                }

                let expected = entry.ty.unwrap();
                let got = type_of_value(&value);
                if !value_matches_type(&value, expected) {
                    return Err(RuntimeError::TypeMismatch { pos, expected, got });
                }

                entry.val = Some(value);
                tracked_value = entry.val.as_ref().cloned();

                if self.debug_scope {
                    let logged = entry.val.clone().unwrap();
                    if was_initialized {
                        diagnostics::print_scope_event(&ScopeEvent::Mutate(name.clone(), logged));
                    } else {
                        diagnostics::print_scope_event(&ScopeEvent::Add(name.clone(), logged));
                    }
                }
            }

            if let Some(v) = tracked_value.as_ref() {
                self.track_in_arena(v);
            }
            return Ok(());
        }
        let was_seen = self.seen.contains(&name);
        Err(diagnostics::unresolved_var_error(pos, name, was_seen))
    }

    pub fn set_var_typed(
        &mut self,
        name: String,
        ty: Type,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let value = self.resolve_assigned_value(value, pos)?;
        let logged = value.clone();
        let got = type_of_value(&value);
        if !value_matches_type(&value, ty) {
            return Err(RuntimeError::TypeMismatch {
                pos,
                expected: ty,
                got,
            });
        }

        {
            let frame = self.scopes.last_mut().unwrap();
            if frame.vars.contains_key(&name) {
                return Err(RuntimeError::AlreadyDeclared { pos, name });
            }

            if self.seen.insert(name.clone()) {
                self.order.push(name.clone());
            }

            frame.vars.insert(
                name.clone(),
                VarEntry {
                    ty: Some(ty),
                    val: Some(value),
                },
            );
            if self.debug_scope {
                diagnostics::print_scope_event(&ScopeEvent::Add(name, logged.clone()));
            }
        }

        self.track_in_arena(&logged);
        Ok(())
    }

    pub fn set_container_field(
        &mut self,
        container: &str,
        field: &str,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let logged = value.clone();
        for frame in self.scopes.iter_mut().rev() {
            if let Some(entry) = frame.vars.get_mut(container) {
                if let Some(Value::Container(map)) = &mut entry.val {
                    map.insert(field.to_string(), value);
                    if self.debug_scope {
                        diagnostics::print_scope_event(&ScopeEvent::Mutate(
                            format!("{}.{}", container, field),
                            logged,
                        ));
                    }
                    return Ok(());
                } else {
                    return Err(RuntimeError::NotAContainer {
                        pos,
                        name: container.to_string(),
                    });
                }
            }
        }
        Err(RuntimeError::UndefinedVar {
            pos,
            name: container.to_string(),
        })
    }

    pub fn get_var(&self, name: &str, pos: usize) -> Result<Value, RuntimeError> {
        for frame in self.scopes.iter().rev() {
            if let Some(entry) = frame.vars.get(name) {
                if let Some(value) = &entry.val {
                    return Ok(value.clone());
                }
                return Err(RuntimeError::UninitializedVar {
                    pos,
                    name: name.to_string(),
                });
            }
        }
        let owned = name.to_string();
        let was_seen = self.seen.contains(&owned);
        Err(diagnostics::unresolved_var_error(pos, owned, was_seen))
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Val(v) => Ok(v.clone()),
            Expr::Ident(name, pos) => self.get_var(name, *pos),
            Expr::DotAccess(container, field) => {
                for frame in self.scopes.iter().rev() {
                    if let Some(entry) = frame.vars.get(container) {
                        if let Some(Value::Container(map)) = &entry.val {
                            return map.get(field).cloned().ok_or_else(|| RuntimeError::UndefinedVar {
                                pos: 0,
                                name: format!("{}.{}", container, field),
                            });
                        } else {
                            return Err(RuntimeError::NotAContainer {
                                pos: 0,
                                name: container.to_string(),
                            });
                        }
                    }
                }
                Err(RuntimeError::UndefinedVar {
                    pos: 0,
                    name: container.to_string(),
                })
            }
            Expr::Call(name, args, pos) => {
                let func = self
                    .funcs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| RuntimeError::UndefinedVar {
                        pos: *pos,
                        name: name.clone(),
                    })?;

                let outer_scope_idx = self.scopes.len() - 1;
                let mut resolved_args: Vec<(String, Value, Option<String>)> = Vec::new();
                // (inner param name, value, bleed_back outer name if .copy)

                for (param, arg) in func.params.iter().zip(args.iter()) {
                    match (param, arg) {
                        (ParamKind::Typed(pname, _pty), CallArg::Expr(expr)) => {
                            let val = self.eval_expr(expr)?;
                            resolved_args.push((pname.clone(), val, None));
                        }
                        (ParamKind::Copy(pname), CallArg::Copy(outer_name)) => {
                            let val = self.get_var(outer_name, *pos)?;
                            resolved_args.push((pname.clone(), val, Some(outer_name.clone())));
                        }
                        (ParamKind::CopyFree(pname), CallArg::CopyFree(outer_name)) => {
                            let val = self.get_var(outer_name, *pos)?;
                            resolved_args.push((pname.clone(), val, None));
                            // no bleed-back - copy.free is isolated
                        }
                        (ParamKind::CopyInto(pname, _), CallArg::CopyInto(outer_names)) => {
                            let mut map = HashMap::new();
                            for oname in outer_names {
                                let val = self.get_var(oname, *pos)?;
                                map.insert(oname.clone(), val);
                            }
                            resolved_args.push((pname.clone(), Value::Container(map), None));
                        }
                        _ => return Err(RuntimeError::BadAssignTarget { pos: *pos }),
                    }
                }

                self.push_function_scope();

                let call_result = (|| -> Result<Value, RuntimeError> {
                    for (pname, val, bleed_outer) in resolved_args {
                        let ty = type_of_value(&val);
                        self.set_var_typed(pname.clone(), ty, val, *pos)?;
                        if let Some(outer_name) = bleed_outer {
                            if let Some(frame) = self.scopes.last_mut() {
                                frame.bleed_back.insert(pname, (outer_scope_idx, outer_name));
                            }
                        }
                    }

                    for stmt in &func.body {
                        match run_stmt(self, stmt.clone()) {
                            Ok(_) => {}
                            Err(RuntimeError::EarlyReturn(val)) => return Ok(val),
                            Err(e) => return Err(e),
                        }
                    }

                    if let Some(expr) = &func.ret_expr {
                        self.eval_expr(expr)
                    } else {
                        Ok(Value::Num(0))
                    }
                })();

                self.pop_scope();
                call_result
            }
            Expr::Bin(lhs, op, pos, rhs) => {
                let left = self.eval_expr(lhs)?;
                let right = self.eval_expr(rhs)?;
                self.apply_op(left, *op, *pos, right)
            }
        }
    }

    fn apply_op(
        &self,
        left: Value,
        op: Op,
        pos: usize,
        right: Value,
    ) -> Result<Value, RuntimeError> {
        match op {
            Op::Plus => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a.saturating_add(*b))),
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
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a.saturating_sub(*b))),
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
                    Ok(Value::Num(a.checked_mul(*b).unwrap_or(u128::MAX)))
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
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a / b)),
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
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a % b)),
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
                (Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a == b)),
                (Value::Char(a), Value::Char(b)) => Ok(Value::Bool(a == b)),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
                (l, r) => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left: l.clone(),
                    right: r.clone(),
                }),
            },
        }
    }
}


pub fn run_stmt(rt: &mut RunTime, stmt: Stmt) -> Result<(), RuntimeError> {
    match stmt {
        Stmt::Decl { name, ty, pos } => rt.declare(name, ty, pos),
        Stmt::Assign {
            target,
            expr,
            pos_eq,
        } => {
            let value = rt.eval_expr(&expr)?;
            match target {
                Expr::Ident(name, _) => rt.set_var(name, value, pos_eq),
                Expr::DotAccess(container, field) => {
                    rt.set_container_field(&container, &field, value, pos_eq)
                }
                _ => Err(RuntimeError::BadAssignTarget { pos: pos_eq }),
            }
        }
        Stmt::TypedAssign {
            name,
            ty,
            expr,
            pos_type,
        } => {
            let value = rt.eval_expr(&expr)?;
            rt.set_var_typed(name, ty, value, pos_type)
        }
        Stmt::Print { expr, pos } => {
            let value = rt.eval_expr(&expr)?;
            match value {
                Value::Num(n) => println!("{}", n),
                Value::Float(x) => println!("{}", x),
                Value::Str(s) => println!("{}", s),
                Value::Bool(b) => println!("{}", b),
                Value::Char(c) => println!("{}", c),
                Value::Container(map) => println!("{:?}", map),
            }
            Ok(())
        }
        Stmt::PrintInline { expr, pos: _ } => {
            let value = rt.eval_expr(&expr)?;
            match value {
                Value::Num(n) => print!("{}", n),
                Value::Float(x) => print!("{}", x),
                Value::Str(s) => print!("{}", s),
                Value::Bool(b) => print!("{}", b),
                Value::Char(c) => print!("{}", c),
                Value::Container(map) => print!("{:?}", map),
            }
            use std::io::Write;
            std::io::stdout().flush().ok();
            Ok(())
        }
        Stmt::ExprStmt { expr, .. } => {
            rt.eval_expr(&expr)?;
            Ok(())
        }
        Stmt::Return { expr, .. } => {
            let val = match expr {
                Some(e) => rt.eval_expr(&e)?,
                None => Value::Num(0),
            };
            Err(RuntimeError::EarlyReturn(val))
        }
        Stmt::FuncDef {
            name,
            params,
            ret_ty,
            body,
            ret_expr,
            ..
        } => {
            rt.funcs.insert(
                name,
                FuncDef {
                    params,
                    ret_ty,
                    body,
                    ret_expr,
                },
            );
            Ok(())
        }
        Stmt::Block { stmts, .. } => {
            rt.push_scope();
            for stmt in stmts {
                if let Err(err) = run_stmt(rt, stmt) {
                    rt.pop_scope();
                    return Err(err);
                }
            }
            rt.pop_scope();
            Ok(())
        }
    }
}

fn value_to_string(v: Value) -> String {
    match v {
        Value::Num(n) => n.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Str(s) => s,
        Value::Bool(b) => b.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Container(map) => format!("{:?}", map),
    }
}

fn type_of_value(v: &Value) -> Type {
    match v {
        Value::Num(_) => Type::T128,
        Value::Float(_) => Type::T64,
        Value::Str(_) => Type::Str,
        Value::Bool(_) => Type::Bool,
        Value::Char(_) => Type::Char,
        Value::Container(_) => Type::Str,
    }
}

fn value_matches_type(v: &Value, t: Type) -> bool {
    match (v, t) {
        (Value::Num(_), Type::T8) => true,
        (Value::Num(_), Type::T16) => true,
        (Value::Num(_), Type::T32) => true,
        (Value::Num(_), Type::T64) => true,
        (Value::Num(_), Type::T128) => true,
        (Value::Float(_), Type::T8) => true,
        (Value::Float(_), Type::T16) => true,
        (Value::Float(_), Type::T32) => true,
        (Value::Float(_), Type::T64) => true,
        (Value::Float(_), Type::T128) => true,
        (Value::Str(_), Type::Str) => true,
        (Value::Container(_), Type::Str) => true,
        (Value::Bool(_), Type::Bool) => true,
        (Value::Char(_), Type::Char) => true,
        _ => false,
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Num(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

fn expand_template(rt: &RunTime, s: &str, pos: usize) -> Result<String, RuntimeError> {
    let mut out = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            let mut spec = String::new();
            let mut in_spec = false;
            while let Some(&ch) = chars.peek() {
                chars.next();
                if ch == '}' {
                    break;
                }
                if ch == ':' {
                    in_spec = true
                } else if in_spec {
                    spec.push(ch);
                } else {
                    name.push(ch);
                }
            }
            let key = name.trim();
            if !is_ident(key) {
                return Err(RuntimeError::TemplateInvalidPlaceholder {
                    pos,
                    placeholder: key.to_string(),
                });
            }
            if !(spec.is_empty() || spec == "?") {
                return Err(RuntimeError::TemplateInvalidFormat {
                    pos,
                    spec: spec.to_string(),
                });
            }
            let v = rt.get_var(key, pos)?;
            if spec == "?" {
                out.push_str(&format!("{:?}", v));
            } else {
                out.push_str(&value_to_string(v));
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

