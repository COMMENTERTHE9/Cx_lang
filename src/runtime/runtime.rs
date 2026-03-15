// incremental rebuild test 3
use crate::frontend::{ast::*, diagnostics, types::*};
use crate::runtime::arena::Arena;
use crate::runtime::handle::HandleRegistry;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct EnumRuntimeInfo {
    pub variants: Vec<String>,
    pub groups: HashMap<String, Vec<String>>,
    pub super_group_order: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
pub enum ScopeEvent {
    Open(String),
    Close(String),
    Add(String, Value),
    Mutate(String, Value),
    Free(String),
    BleedBack(String, Value),
    HandleAlloc { slot: u32, gen: u32 },
    HandleDrop { slot: u32, gen: u32 },
    HandleAccess { slot: u32, gen: u32, stale: bool },
    ArenaReset { bytes: usize, chunks: usize },
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    type_params: Vec<String>,
    params: Vec<ParamKind>,
    body: Vec<Stmt>,
    ret_expr: Option<Expr>,
}

pub struct ScopeFrame {
    pub vars: HashMap<String, VarEntry>,
    pub freed: HashSet<String>,
    pub bleed_back: HashMap<String, (usize, String)>,
    pub local_funcs: HashMap<String, FuncDef>,
    pub arena: Option<Arena>,
    pub seen: HashSet<String>,
    // inner param name -> (outer scope index, outer var name)
}

pub struct RunTime {
    pub string_arena: Vec<u8>,
    pub handles: HandleRegistry<Value>,
    pub enums: HashMap<String, EnumRuntimeInfo>,
    pub structs: HashMap<String, Vec<(String, Type)>>,
    pub impls: HashMap<(String, String), (Vec<(String, Type)>, (String, Vec<ParamKind>, Option<Type>, Vec<Stmt>, Option<Expr>))>,
    scopes: Vec<ScopeFrame>,
    funcs: HashMap<String, FuncDef>,
    pub debug_scope: bool,
}

impl RunTime {
    pub fn register_func(
        &mut self,
        name: String,
        params: Vec<ParamKind>,
        body: Vec<Stmt>,
        ret_expr: Option<Expr>,
    ) {
        self.funcs.insert(
            name,
            FuncDef {
                type_params: vec![],
                params,
                body,
                ret_expr,
            },
        );
    }

    pub fn alloc_str(&mut self, s: &str) -> (u32, u32) {
        let offset = self.string_arena.len() as u32;
        self.string_arena.extend_from_slice(s.as_bytes());
        (offset, s.len() as u32)
    }

    pub fn resolve_str(&self, offset: u32, len: u32) -> &str {
        let bytes = &self.string_arena[offset as usize..(offset + len) as usize];
        std::str::from_utf8(bytes).expect("arena string was not valid utf8")
    }

    fn super_group_handler_index(
        &self,
        enum_name: &str,
        super_name: &str,
        variant_name: &str,
    ) -> Option<usize> {
        let info = self.enums.get(enum_name)?;
        let sub_names = info.super_group_order.get(super_name)?;
        sub_names.iter().enumerate().find_map(|(i, sub)| {
            let members = info.groups.get(sub)?;
            if members.iter().any(|m| m == variant_name) {
                Some(i)
            } else {
                None
            }
        })
    }

    fn resolve_assigned_value(&mut self, value: Value, pos: usize) -> Result<Value, RuntimeError> {
        match value {
            Value::Str(off, len) => {
                let expanded = expand_template(self, self.resolve_str(off, len), pos)?;
                let (off, len) = self.alloc_str(&expanded);
                Ok(Value::Str(off, len))
            }
            other => Ok(other),
        }
    }

    fn track_in_arena(&mut self, value: &Value) {
        let size = match value {
            Value::Str(_, len) => *len as usize + 1,
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
            string_arena: Vec::new(),
            handles: HandleRegistry::new(),
            enums: HashMap::new(),
            structs: HashMap::new(),
            impls: HashMap::new(),
            scopes: vec![ScopeFrame {
                vars: HashMap::new(),
                freed: HashSet::new(),
                bleed_back: HashMap::new(),
                local_funcs: HashMap::new(),
                arena: None, // top level is not a function scope
                seen: HashSet::new(),
            }],
            funcs: HashMap::new(),
            debug_scope: false,
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(ScopeFrame {
            vars: HashMap::new(),
            freed: HashSet::new(),
            bleed_back: HashMap::new(),
            local_funcs: HashMap::new(),
            arena: None, // block scope - no arena
            seen: HashSet::new(),
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
            local_funcs: HashMap::new(),
            arena: Some(Arena::new()), // function scope - gets its own arena
            seen: HashSet::new(),
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

        let (bleed_values, debug_info) = {
            let frame = self.scopes.last().unwrap();
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
                    frame.vars.get(param_name)
                        .and_then(|entry| entry.val.clone())
                        .map(|val| (*outer_idx, outer_name.clone(), val))
                })
                .collect();

            let debug_info = if self.debug_scope {
                let bleed_events: Vec<(String, Value)> = bleeds
                    .iter()
                    .filter_map(|(param_name, _, outer_name)| {
                        frame.vars.get(param_name)
                            .and_then(|entry| entry.val.clone())
                            .map(|val| (outer_name.clone(), val))
                    })
                    .collect();
                let free_names: Vec<String> = frame.vars.keys()
                    .filter(|name| !frame.freed.contains(*name))
                    .cloned()
                    .collect();
                let close_label = format!("scope#{}", self.scopes.len() - 1);
                let had_arena = frame.arena.as_ref()
                    .map(|a| (a.bytes_used(), a.chunk_count()));
                Some((free_names, bleed_events, close_label, had_arena))
            } else {
                None
            };

            (bleed_values, debug_info)
        };

        self.scopes.pop();

        for (outer_idx, outer_name, val) in bleed_values {
            if let Some(outer_frame) = self.scopes.get_mut(outer_idx) {
                if let Some(entry) = outer_frame.vars.get_mut(&outer_name) {
                    entry.val = Some(val);
                }
            }
        }

        if let Some((free_names, bleed_events, close_label, had_arena)) = debug_info {
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

    pub fn declare(
        &mut self,
        name: String,
        ty: Option<Type>,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let frame = self.scopes.last_mut().unwrap();

        if frame.vars.contains_key(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        frame.seen.insert(name.clone());
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
            let tracked_value;
            {
                let frame = &mut self.scopes[i];
                let entry = frame.vars.get_mut(&name).unwrap();

                let was_initialized = entry.val.is_some();
                if entry.ty.is_none() {
                    entry.ty = Some(type_of_value(&value));
                }

                let expected = entry.ty.clone().unwrap();
                let got = type_of_value(&value);
                if !value_matches_type(&value, &expected) {
                    return Err(RuntimeError::TypeMismatch { pos, expected, got });
                }

                entry.val = Some(value);
                tracked_value = entry.val.clone();

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
        let was_seen = self.scopes.last().unwrap().seen.contains(&name);
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
        if !value_matches_type(&value, &ty) {
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

            frame.seen.insert(name.clone());
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
                match &mut entry.val {
                    Some(Value::Container(map)) => {
                        map.insert(field.to_string(), value);
                        if self.debug_scope {
                            diagnostics::print_scope_event(&ScopeEvent::Mutate(
                                format!("{}.{}", container, field),
                                logged,
                            ));
                        }
                        return Ok(());
                    }
                    Some(Value::Struct(_, map)) => {
                        map.insert(field.to_string(), value);
                        if self.debug_scope {
                            diagnostics::print_scope_event(&ScopeEvent::Mutate(
                                format!("{}.{}", container, field),
                                logged,
                            ));
                        }
                        return Ok(());
                    }
                    _ => {
                        return Err(RuntimeError::NotAContainer {
                            pos,
                            name: container.to_string(),
                        });
                    }
                }
            }
        }
        Err(RuntimeError::UndefinedVar {
            pos,
            name: container.to_string(),
        })
    }

    pub fn get_field(&self, container: &str, field: &str, pos: usize) -> Result<Value, RuntimeError> {
        for frame in self.scopes.iter().rev() {
            if let Some(entry) = frame.vars.get(container) {
                match &entry.val {
                    Some(Value::Struct(_, map)) | Some(Value::Container(map)) => {
                        return map.get(field).cloned().ok_or_else(|| {
                            RuntimeError::UndefinedVar { pos, name: format!("{}.{}", container, field) }
                        });
                    }
                    _ => return Err(RuntimeError::NotAContainer { pos, name: container.to_string() }),
                }
            }
        }
        Err(RuntimeError::UndefinedVar { pos, name: container.to_string() })
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
        let was_seen = self.scopes.last().unwrap().seen.contains(&owned);
        Err(diagnostics::unresolved_var_error(pos, owned, was_seen))
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Val(ast_val) => Ok(match ast_val {
                AstValue::Num(n) => Value::Num(*n),
                AstValue::Float(f) => Value::Float(*f),
                AstValue::Bool(b) => Value::Bool(*b),
                AstValue::Char(c) => Value::Char(*c),
                AstValue::EnumVariant(e, v) => Value::EnumVariant(e.clone(), v.clone()),
                AstValue::Unknown => Value::Unknown(Type::Unknown),
                AstValue::Str(s) => {
                    let (off, len) = self.alloc_str(s);
                    Value::Str(off, len)
                }
                AstValue::StructInstance(name, fields) => {
                    let mut map = HashMap::new();
                    for (fname, fexpr) in fields {
                        let val = self.eval_expr(fexpr)?;
                        map.insert(fname.clone(), val);
                    }
                    return Ok(Value::Struct(name.clone(), map));
                }
            }),
            Expr::Ident(name, pos) => self.get_var(name, *pos),
            Expr::DotAccess(container, field) => {
                for frame in self.scopes.iter().rev() {
                    if let Some(entry) = frame.vars.get(container) {
                        match &entry.val {
                            Some(Value::Container(map)) => {
                                return map.get(field).cloned().ok_or_else(|| {
                                    RuntimeError::UndefinedVar {
                                        pos: 0,
                                        name: format!("{}.{}", container, field),
                                    }
                                });
                            }
                            Some(Value::Struct(_, map)) => {
                                return map.get(field).cloned().ok_or_else(|| {
                                    RuntimeError::UndefinedVar {
                                        pos: 0,
                                        name: format!("{}.{}", container, field),
                                    }
                                });
                            }
                            _ => {
                                return Err(RuntimeError::NotAContainer {
                                    pos: 0,
                                    name: container.to_string(),
                                });
                            }
                        }
                    }
                }
                Err(RuntimeError::UndefinedVar {
                    pos: 0,
                    name: container.to_string(),
                })
            }
            Expr::HandleNew(inner_expr, _pos) => {
                let val = self.eval_expr(inner_expr)?;
                let h = self.handles.insert(val);
                if self.debug_scope {
                    diagnostics::print_scope_event(&ScopeEvent::HandleAlloc {
                        slot: h.slot,
                        gen: h.gen,
                    });
                }
                Ok(Value::Handle(h))
            }
            Expr::HandleVal(name, pos) => {
                let val = self.get_var(name, *pos)?;
                if let Value::Handle(h) = val {
                    match self.handles.get(h) {
                        Some(v) => {
                            if self.debug_scope {
                                diagnostics::print_scope_event(&ScopeEvent::HandleAccess {
                                    slot: h.slot,
                                    gen: h.gen,
                                    stale: false,
                                });
                            }
                            Ok(v.clone())
                        }
                        None => {
                            if self.debug_scope {
                                diagnostics::print_scope_event(&ScopeEvent::HandleAccess {
                                    slot: h.slot,
                                    gen: h.gen,
                                    stale: true,
                                });
                            }
                            Err(RuntimeError::StaleHandle { pos: *pos })
                        }
                    }
                } else {
                    Err(RuntimeError::StaleHandle { pos: *pos })
                }
            }
            Expr::HandleDrop(name, pos) => {
                let val = self.get_var(name, *pos)?;
                if let Value::Handle(h) = val {
                    let removed = self.handles.remove(h);
                    if self.debug_scope {
                        match removed {
                            Some(_) => diagnostics::print_scope_event(&ScopeEvent::HandleDrop {
                                slot: h.slot,
                                gen: h.gen,
                            }),
                            None => diagnostics::print_scope_event(&ScopeEvent::HandleAccess {
                                slot: h.slot,
                                gen: h.gen,
                                stale: true,
                            }),
                        }
                    }
                    Ok(Value::Num(0))
                } else {
                    Err(RuntimeError::StaleHandle { pos: *pos })
                }
            }
            Expr::Unary(op, inner, pos) => {
                let val = self.eval_expr(inner)?;
                match (op, val) {
                    (Op::Minus, Value::Num(n)) => Ok(Value::Num(n.wrapping_neg())),
                    (Op::Minus, Value::Float(f)) => Ok(Value::Float(-f)),
                    (Op::Mul, v) => Ok(v),
                    _ => Err(RuntimeError::BadAssignTarget { pos: *pos }),
                }
            }
            Expr::ArrayLit(elems) => {
                let mut vals = Vec::with_capacity(elems.len());
                for e in elems {
                    vals.push(self.eval_expr(e)?);
                }
                Ok(Value::Array(vals))
            }
            Expr::Index(base_expr, idx_expr, pos) => {
                let base = self.eval_expr(base_expr)?;
                let idx = self.eval_expr(idx_expr)?;
                let i = match idx {
                    Value::Num(n) => n as usize,
                    _ => return Err(RuntimeError::BadAssignTarget { pos: *pos }),
                };
                match base {
                    Value::Array(elems) => Ok(elems
                        .into_iter()
                        .nth(i)
                        .unwrap_or(Value::Unknown(Type::Unknown))),
                    _ => Err(RuntimeError::BadAssignTarget { pos: *pos }),
                }
            }
            Expr::MethodCall(instance, method, args, pos) => {
                let instance_val = self.get_var(instance, *pos)?;
                let type_name = match &instance_val {
                    Value::Struct(name, _) => name.clone(),
                    _ => return Err(RuntimeError::NotAContainer { pos: *pos, name: instance.clone() }),
                };

                let (aliases, (_, params, _, body, ret_expr)) = self.impls
                    .get(&(type_name.clone(), method.clone()))
                    .cloned()
                    .ok_or_else(|| RuntimeError::UndefinedVar { pos: *pos, name: format!("{}.{}", instance, method) })?;

                self.push_function_scope();

                let call_result = (|| -> Result<Value, RuntimeError> {
                    let alias_name = aliases[0].0.clone();
                    let alias_ty = Type::Struct(type_name.clone());
                    self.set_var_typed(alias_name.clone(), alias_ty, instance_val, *pos)?;

                    for (param, arg) in params.iter().zip(args.iter()) {
                        if let (ParamKind::Typed(pname, _), CallArg::Expr(expr)) = (param, arg) {
                            let val = self.eval_expr(expr)?;
                            self.set_var_typed(pname.clone(), type_of_value(&val), val, *pos)?;
                        }
                    }

                    for stmt in &body {
                        match self.run_stmt(stmt) {
                            Ok(_) => {}
                            Err(RuntimeError::EarlyReturn(val)) => return Ok(val),
                            Err(e) => return Err(e),
                        }
                    }

                    if let Some(expr) = &ret_expr {
                        self.eval_expr(expr)
                    } else {
                        Ok(Value::Num(0))
                    }
                })();

                let mutated_alias = if call_result.is_ok() {
                    let alias_name = aliases[0].0.clone();
                    self.get_var(&alias_name, *pos).ok()
                } else {
                    None
                };

                self.pop_scope();

                if let Some(mutated) = mutated_alias {
                    let _ = self.set_var(instance.clone(), mutated, *pos);
                }

                call_result
            }
            Expr::Call(name, args, pos) => {
                if name == "is_known" {
                    let val = match args.first() {
                        Some(CallArg::Expr(expr)) => self.eval_expr(expr)?,
                        _ => {
                            return Err(RuntimeError::UndefinedVar {
                                pos: *pos,
                                name: name.clone(),
                            })
                        }
                    };
                    return Ok(match val {
                        Value::Unknown(_) | Value::TBool(2) => Value::Bool(false),
                        _ => Value::Bool(true),
                    });
                }

                let func = self
                    .scopes
                    .iter()
                    .rev()
                    .find_map(|frame| frame.local_funcs.get(name))
                    .cloned()
                    .or_else(|| self.funcs.get(name).cloned())
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
                                frame
                                    .bleed_back
                                    .insert(pname, (outer_scope_idx, outer_name));
                            }
                        }
                    }

                    for stmt in &func.body {
                        match self.run_stmt(&stmt) {
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
            Expr::Range(_, _, _) => Err(RuntimeError::BadOperands {
                pos: 0,
                op: Op::Plus,
                left: Value::Num(0),
                right: Value::Num(0),
            }),
        }
    }

    fn apply_op(
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
                    Ok(Value::Num(a.checked_mul(*b).unwrap_or(u128::MAX as i128)))
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

    pub fn run_stmt(&mut self, stmt: &Stmt) -> Result<(), RuntimeError> {
    match stmt {
        Stmt::StructDef { name, fields, .. } => {
            self.structs.insert(name.clone(), fields.clone());
            Ok(())
        }
        Stmt::ImplBlock { aliases, methods, .. } => {
            for (method_name, params, ret_ty, body, ret_expr) in methods {
                for (_, alias_type) in aliases {
                    let type_key = match alias_type {
                        Type::Struct(name) => name.clone(),
                        _ => continue,
                    };
                    self.impls.insert(
                        (type_key, method_name.clone()),
                        (aliases.clone(), (method_name.clone(), params.clone(), ret_ty.clone(), body.clone(), ret_expr.clone())),
                    );
                }
            }
            Ok(())
        }
        Stmt::EnumDef {
            name,
            variants,
            groups,
            super_groups,
            ..
        } => {
            let mut group_map: HashMap<String, Vec<String>> = HashMap::new();

            for (group_name, group_variants) in groups {
                group_map.insert(group_name.clone(), group_variants.clone());
            }

            for (_super_name, sub_groups) in super_groups {
                for (sub_name, sub_variants) in sub_groups {
                    group_map.insert(sub_name.clone(), sub_variants.clone());
                }
            }

            for (super_name, sub_groups) in super_groups {
                let all_variants: Vec<String> = sub_groups
                    .iter()
                    .flat_map(|(_, sv)| sv.iter().cloned())
                    .collect();
                group_map.insert(super_name.clone(), all_variants);
            }

            let mut super_order: HashMap<String, Vec<String>> = HashMap::new();
            for (super_name, sub_groups) in super_groups {
                let ordered: Vec<String> = sub_groups
                    .iter()
                    .map(|(sub_name, _)| sub_name.clone())
                    .collect();
                super_order.insert(super_name.clone(), ordered);
            }

            self.enums.insert(
                name.clone(),
                EnumRuntimeInfo {
                    variants: variants.clone(),
                    groups: group_map,
                    super_group_order: super_order,
                },
            );
            Ok(())
        }
        Stmt::Decl { name, ty, pos } => {
            if let Some(Type::Array(size, elem_ty)) = &ty {
                let slots = vec![Value::Unknown(*elem_ty.clone()); *size];
                self.set_var_typed(name.clone(), ty.clone().unwrap(), Value::Array(slots), *pos)
            } else {
                self.declare(name.clone(), ty.clone(), *pos)
            }
        }
        Stmt::Assign {
            target,
            expr,
            pos_eq,
        } => {
            let value = self.eval_expr(&expr)?;
            match target {
                Expr::Ident(name, _) => self.set_var(name.clone(), value, *pos_eq),
                Expr::DotAccess(container, field) => {
                    self.set_container_field(&container, &field, value, *pos_eq)
                }
                _ => Err(RuntimeError::BadAssignTarget { pos: *pos_eq }),
            }
        }
        Stmt::TypedAssign {
            name,
            ty,
            expr,
            pos_type,
        } => {
            let value = self.eval_expr(&expr)?;
            let value = match (&ty, value) {
                (Type::Bool, Value::Unknown(_)) => Value::TBool(2),
                (_, v) => v,
            };
            self.set_var_typed(name.clone(), ty.clone(), value, *pos_type)
        }
        Stmt::Print { expr, pos: _pos } => {
            let value = self.eval_expr(&expr)?;
            match value {
                Value::Num(n) => println!("{}", n),
                Value::Float(x) => println!("{}", x),
                Value::Str(off, len) => println!("{}", self.resolve_str(off, len)),
                Value::Bool(b) => println!("{}", b),
                Value::TBool(b) => println!(
                    "{}",
                    match b {
                        0 => "false",
                        1 => "true",
                        _ => "?",
                    }
                ),
                Value::Char(c) => println!("{}", c),
                Value::EnumVariant(e, v) => println!("{}::{}", e, v),
                Value::Unknown(_) => println!("?"),
                Value::Array(elems) => {
                    let parts: Vec<String> = elems
                        .iter()
                        .map(|v| value_to_string(self, v.clone()))
                        .collect();
                    println!("[{}]", parts.join(", "));
                }
                Value::Handle(h) => println!("handle({},{})", h.slot, h.gen),
                Value::Container(map) => println!("{:?}", map),
                Value::Struct(name, map) => {
                    let parts: Vec<String> = map.iter().map(|(k, v)| format!("{}: {}", k, value_to_string(self, v.clone()))).collect();
                    println!("{} {{ {} }}", name, parts.join(", "));
                }
            }
            Ok(())
        }
        Stmt::PrintInline { expr, _pos: _ } => {
            let value = self.eval_expr(&expr)?;
            match value {
                Value::Num(n) => print!("{}", n),
                Value::Float(x) => print!("{}", x),
                Value::Str(off, len) => print!("{}", self.resolve_str(off, len)),
                Value::Bool(b) => print!("{}", b),
                Value::TBool(b) => print!(
                    "{}",
                    match b {
                        0 => "false",
                        1 => "true",
                        _ => "?",
                    }
                ),
                Value::Char(c) => print!("{}", c),
                Value::EnumVariant(e, v) => print!("{}::{}", e, v),
                Value::Unknown(_) => print!("?"),
                Value::Handle(h) => print!("handle({},{})", h.slot, h.gen),
                Value::Container(map) => print!("{:?}", map),
                Value::Struct(name, map) => {
                    let parts: Vec<String> = map.iter().map(|(k, v)| format!("{}: {}", k, value_to_string(self, v.clone()))).collect();
                    print!("{} {{ {} }}", name, parts.join(", "));
                }
                Value::Array(elems) => {
                    let parts: Vec<String> = elems
                        .iter()
                        .map(|v| value_to_string(self, v.clone()))
                        .collect();
                    print!("[{}]", parts.join(", "));
                }
            }
            use std::io::Write;
            std::io::stdout().flush().ok();
            Ok(())
        }
        Stmt::ExprStmt { expr, .. } => {
            self.eval_expr(&expr)?;
            Ok(())
        }
        Stmt::Return { expr, .. } => {
            let val = match expr {
                Some(e) => self.eval_expr(&e)?,
                None => Value::Num(0),
            };
            Err(RuntimeError::EarlyReturn(val))
        }
        Stmt::FuncDef {
            name,
            type_params,
            params,
            ret_ty: _,
            body,
            ret_expr,
            ..
        } => {
            let def = FuncDef {
                type_params: type_params.clone(),
                params: params.clone(),
                body: body.clone(),
                ret_expr: ret_expr.clone(),
            };
            if let Some(frame) = self.scopes.last_mut() {
                frame.local_funcs.insert(name.clone(), def);
            } else {
                self.funcs.insert(name.clone(), def);
            }
            Ok(())
        }
        Stmt::Block { stmts, .. } => {
            self.push_scope();
            for stmt in stmts {
                if let Err(err) = self.run_stmt(stmt) {
                    self.pop_scope();
                    return Err(err);
                }
            }
            self.pop_scope();
            Ok(())
        }
        Stmt::When {
            expr,
            arms,
            pos: _pos,
        } => {
            let val = self.eval_expr(&expr)?;
            for arm in arms {
                let matched = match &arm.pattern {
                    WhenPattern::Catchall => true,
                    WhenPattern::Literal(AstValue::Unknown) => matches!(val, Value::Unknown(_)),
                    WhenPattern::Literal(AstValue::Bool(b)) => {
                        matches!(&val, Value::Bool(v) if v == b)
                    }
                    WhenPattern::Literal(AstValue::Num(n)) => {
                        matches!(&val, Value::Num(v) if v == n)
                    }
                    WhenPattern::EnumVariant(en, vn) => {
                        matches!(&val, Value::EnumVariant(e, v) if e == en && v == vn)
                    }
                    WhenPattern::Range(AstValue::Num(start), AstValue::Num(end), inclusive) => {
                        if let Value::Num(n) = &val {
                            if *inclusive {
                                n >= start && n <= end
                            } else {
                                n >= start && n < end
                            }
                        } else {
                            false
                        }
                    }
                    WhenPattern::Range(_, _, _) => false,
                    WhenPattern::Literal(_) => false,
                    WhenPattern::Group(_, group_name) => {
                        if let Value::EnumVariant(enum_name, variant_name) = &val {
                            if let Some(info) = self.enums.get(enum_name) {
                                if let Some(members) = info.groups.get(group_name) {
                                    members.contains(&variant_name.to_string())
                                } else {
                                    self.super_group_handler_index(
                                        enum_name,
                                        group_name,
                                        variant_name,
                                    )
                                    .is_some()
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    WhenPattern::Placeholder => false,
                };
                if matched {
                    self.push_scope();
                    match &arm.body {
                        WhenBody::Stmts(stmts) => {
                            for s in stmts {
                                if let Err(e) = self.run_stmt(s) {
                                    self.pop_scope();
                                    return Err(e);
                                }
                            }
                        }
                        WhenBody::SuperGroup(handlers) => {
                            let idx = match (&arm.pattern, &val) {
                                (
                                    WhenPattern::Group(_, super_name),
                                    Value::EnumVariant(enum_name, variant_name),
                                ) => self.super_group_handler_index(
                                    enum_name,
                                    super_name,
                                    variant_name,
                                ),
                                _ => None,
                            };
                            if let Some(idx) = idx {
                                if let Some(SuperGroupHandler::Stmts(stmts)) =
                                    handlers.iter().nth(idx)
                                {
                                    for s in stmts {
                                        if let Err(e) = self.run_stmt(s) {
                                            self.pop_scope();
                                            return Err(e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    self.pop_scope();
                    return Ok(());
                }
            }
            Ok(())
        }
        Stmt::While { cond, body, .. } => {
            loop {
                let val = self.eval_expr(&cond)?;
                match val {
                    Value::Bool(true) => {}
                    _ => break,
                }
                self.push_scope();
                let mut should_break = false;
                for s in body {
                    match self.run_stmt(s) {
                        Ok(_) => {}
                        Err(RuntimeError::BreakSignal) => {
                            should_break = true;
                            break;
                        }
                        Err(RuntimeError::ContinueSignal) => {
                            break;
                        }
                        Err(e) => {
                            self.pop_scope();
                            return Err(e);
                        }
                    }
                }
                self.pop_scope();
                if should_break {
                    break;
                }
            }
            Ok(())
        }
        Stmt::For {
            var,
            start,
            end,
            inclusive,
            body,
            ..
        } => {
            let start_val = match self.eval_expr(&start)? {
                Value::Num(n) => n,
                _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
            };
            let end_val = match self.eval_expr(&end)? {
                Value::Num(n) => n,
                _ => return Err(RuntimeError::BadAssignTarget { pos: 0 }),
            };
            'for_loop: {
                if *inclusive {
                    for i in start_val..=end_val {
                        self.push_scope();
                        self.declare(var.clone(), None, 0)?;
                        self.set_var(var.clone(), Value::Num(i), 0)?;
                        for s in body {
                            match self.run_stmt(s) {
                                Ok(_) => {}
                                Err(RuntimeError::BreakSignal) => {
                                    self.pop_scope();
                                    break 'for_loop;
                                }
                                Err(RuntimeError::ContinueSignal) => break,
                                Err(e) => {
                                    self.pop_scope();
                                    return Err(e);
                                }
                            }
                        }
                        self.pop_scope();
                    }
                } else {
                    for i in start_val..end_val {
                        self.push_scope();
                        self.declare(var.clone(), None, 0)?;
                        self.set_var(var.clone(), Value::Num(i), 0)?;
                        for s in body {
                            match self.run_stmt(s) {
                                Ok(_) => {}
                                Err(RuntimeError::BreakSignal) => {
                                    self.pop_scope();
                                    break 'for_loop;
                                }
                                Err(RuntimeError::ContinueSignal) => break,
                                Err(e) => {
                                    self.pop_scope();
                                    return Err(e);
                                }
                            }
                        }
                        self.pop_scope();
                    }
                }
            }
            Ok(())
        }
        Stmt::Loop { body, .. } => {
            loop {
                self.push_scope();
                let mut should_break = false;
                for s in body {
                    match self.run_stmt(s) {
                        Ok(_) => {}
                        Err(RuntimeError::BreakSignal) => {
                            should_break = true;
                            break;
                        }
                        Err(RuntimeError::ContinueSignal) => {
                            break;
                        }
                        Err(e) => {
                            self.pop_scope();
                            return Err(e);
                        }
                    }
                }
                self.pop_scope();
                if should_break {
                    break;
                }
            }
            Ok(())
        }
        Stmt::Break { .. } => Err(RuntimeError::BreakSignal),
        Stmt::Continue { .. } => Err(RuntimeError::ContinueSignal),
        Stmt::CompoundAssign {
            target,
            op,
            operand,
            pos,
        } => {
            match target {
                AssignTarget::Var(name) => {
                    let current = self.get_var(name, *pos)?;
                    let rhs = self.eval_expr(operand)?;
                    let result = self.apply_op(current, op.clone(), *pos, rhs)?;
                    self.set_var(name.clone(), result, *pos)
                }
                AssignTarget::Field(container, field) => {
                    let current = self.get_field(container, field, *pos)?;
                    let rhs = self.eval_expr(operand)?;
                    let result = self.apply_op(current, op.clone(), *pos, rhs)?;
                    self.set_container_field(container, field, result, *pos)
                }
            }
        }
    }
}
}

fn value_to_string(rt: &RunTime, v: Value) -> String {
    match v {
        Value::Num(n) => n.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Str(off, len) => rt.resolve_str(off, len).to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::TBool(b) => match b {
            0 => "false".to_string(),
            1 => "true".to_string(),
            _ => "?".to_string(),
        },
        Value::Char(c) => c.to_string(),
        Value::EnumVariant(e, v) => format!("{}::{}", e, v),
        Value::Unknown(_) => "?".to_string(),
        Value::Array(elems) => {
            let parts: Vec<String> = elems
                .iter()
                .map(|v| value_to_string(rt, v.clone()))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Handle(h) => format!("handle({},{})", h.slot, h.gen),
        Value::Container(map) => format!("{:?}", map),
        Value::Struct(name, map) => {
            let parts: Vec<String> = map.iter().map(|(k, v)| format!("{}: {}", k, value_to_string(rt, v.clone()))).collect();
            format!("{} {{ {} }}", name, parts.join(", "))
        }
    }
}

fn type_of_value(v: &Value) -> Type {
    match v {
        Value::Num(_) => Type::T128,
        Value::Float(_) => Type::T64,
        Value::Str(_, _) => Type::Str,
        Value::Bool(_) => Type::Bool,
        Value::TBool(_) => Type::Bool,
        Value::Char(_) => Type::Char,
        Value::EnumVariant(e, _) => Type::Enum(e.clone()),
        Value::Unknown(_) => Type::Unknown,
        Value::Handle(_) => Type::Handle(Box::new(Type::T128)),
        Value::Container(_) => Type::Container,
        Value::Array(_) => Type::Array(0, Box::new(Type::Unknown)),
        Value::Struct(name, _) => Type::Struct(name.clone()),
    }
}

fn value_matches_type(v: &Value, t: &Type) -> bool {
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
        (Value::Str(_, _), Type::Str) => true,
        (Value::Str(_, _), Type::StrRef) => true,
        (Value::Container(_), Type::Container) => true,
        (Value::Bool(_), Type::Bool) => true,
        (Value::Char(_), Type::Char) => true,
        (Value::EnumVariant(e, _), Type::Enum(t)) if e == t => true,
        (Value::Handle(_), Type::Handle(_)) => true,
        (Value::TBool(_), Type::Bool) => true,
        (Value::Array(_), Type::Array(_, _)) => true,
        (Value::Struct(name, _), Type::Struct(t)) if name == t => true,
        (Value::Unknown(_), _) => true,
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
                out.push_str(&value_to_string(rt, v));
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}
