// incremental rebuild test 3
use crate::frontend::{ast::*, diagnostics, types::*};
use crate::frontend::semantic_types::*;
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
    pub semantic_funcs: HashMap<String, SemanticFunction>,
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

    pub fn register_semantic_func(&mut self, func: SemanticFunction) {
        self.semantic_funcs.insert(func.name.clone(), func);
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
            semantic_funcs: HashMap::new(),
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
                    (Op::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
                    (Op::Not, Value::TBool(n)) => Ok(Value::TBool(if n == 0 { 1 } else if n == 1 { 0 } else { 2 })),
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
        Stmt::IfElse { .. } => Ok(()),
        Stmt::WhileIn { .. } => Ok(()),
    }
}

    // ── Semantic IR interpreter ──────────────────────────────────────

    pub fn eval_semantic_expr(&mut self, expr: &SemanticExpr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            SemanticExprKind::Value(sv) => Ok(self.semantic_value_to_runtime(sv)),
            SemanticExprKind::VarRef { name, .. } => {
                self.get_var(name, 0)
            }
            SemanticExprKind::Unary { op, expr, pos } => {
                let val = self.eval_semantic_expr(expr)?;
                self.apply_unary(op, val, *pos)
            }
            SemanticExprKind::Binary { lhs, op, pos, rhs } => {
                let l = self.eval_semantic_expr(lhs)?;
                let r = self.eval_semantic_expr(rhs)?;
                self.apply_op(l, op.clone(), *pos, r)
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
                        elems.get(i as usize).cloned().ok_or_else(|| RuntimeError::UndefinedVar { pos: *pos, name: format!("index {}", i) })
                    }
                    _ => Err(RuntimeError::BadAssignTarget { pos: *pos })
                }
            }
            SemanticExprKind::MethodCall { instance, method, args, pos } => {
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
            SemanticExprKind::Cast { expr, .. } => self.eval_semantic_expr(expr),
        }
    }

    pub fn run_semantic_stmt(&mut self, stmt: &SemanticStmt) -> Result<(), RuntimeError> {
        match stmt {
            SemanticStmt::Print { expr, .. } => {
                let value = self.eval_semantic_expr(expr)?;
                self.print_value(&value);
                Ok(())
            }
            SemanticStmt::PrintInline { expr, .. } => {
                let value = self.eval_semantic_expr(expr)?;
                self.print_value_inline(&value);
                Ok(())
            }
            SemanticStmt::Decl { name, ty, .. } => {
                let rt_ty: Option<Type> = ty.as_ref().map(|t| semantic_type_to_ast(t));
                self.declare(name.clone(), rt_ty, 0)
            }
            SemanticStmt::TypedAssign { name, ty, expr, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                let rt_ty = semantic_type_to_ast(ty);
                let val = match (&rt_ty, val) {
                    (Type::Bool, Value::Unknown(_)) => Value::TBool(2),
                    (_, v) => v,
                };
                self.set_var_typed(name.clone(), rt_ty, val, 0)
            }
            SemanticStmt::Assign { target, expr, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                match target {
                    SemanticLValue::Binding { name, .. } => self.set_var(name.clone(), val, 0),
                    SemanticLValue::DotAccess { container, field, .. } => {
                        self.set_container_field(container, field, val, 0)
                    }
                }
            }
            SemanticStmt::CompoundAssign { target, op, operand, .. } => {
                match target {
                    SemanticLValue::Binding { name, .. } => {
                        let current = self.get_var(name, 0)?;
                        let rhs = self.eval_semantic_expr(operand)?;
                        let result = self.apply_op(current, op.clone(), 0, rhs)?;
                        self.set_var(name.clone(), result, 0)
                    }
                    SemanticLValue::DotAccess { container, field, .. } => {
                        let current = self.get_field(container, field, 0)?;
                        let rhs = self.eval_semantic_expr(operand)?;
                        let result = self.apply_op(current, op.clone(), 0, rhs)?;
                        self.set_container_field(container, field, result, 0)
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
            SemanticStmt::For { var, start, end, inclusive, body, .. } => {
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
                            self.declare(var.clone(), None, 0)?;
                            self.set_var(var.clone(), Value::Num(i), 0)?;
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
                            self.declare(var.clone(), None, 0)?;
                            self.set_var(var.clone(), Value::Num(i), 0)?;
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
                // Register in semantic registry for semantic dispatch
                self.semantic_funcs.insert(sem_func.name.clone(), sem_func.clone());
                // Also register in AST registry for copy-param fallback path
                self.funcs.insert(sem_func.name.clone(), FuncDef {
                    type_params: sem_func.type_params.clone(),
                    params: sem_func.params.iter().map(|p| semantic_param_to_ast(&p.kind)).collect(),
                    body: vec![],
                    ret_expr: None,
                });
                Ok(())
            }
            SemanticStmt::StructDef { name, fields, .. } => {
                self.structs.insert(name.clone(), fields.iter().map(|(n, t)| (n.clone(), semantic_type_to_ast(t))).collect());
                Ok(())
            }
            SemanticStmt::ImplBlock { aliases, methods, .. } => {
                for sem_func in methods {
                    for (_, alias_type) in aliases {
                        let type_key = match alias_type {
                            SemanticType::Struct(n) => n.clone(),
                            _ => continue,
                        };
                        self.impls.insert(
                            (type_key, sem_func.name.clone()),
                            (aliases.iter().map(|(n, t)| (n.clone(), semantic_type_to_ast(t))).collect(),
                             (sem_func.name.clone(), sem_func.params.iter().map(|p| semantic_param_to_ast(&p.kind)).collect(),
                              sem_func.return_ty.as_ref().map(|t| semantic_type_to_ast(t)),
                              vec![], None))
                        );
                    }
                }
                Ok(())
            }
            SemanticStmt::EnumDef { name, variants, .. } => {
                let variant_names: Vec<String> = variants.iter().map(|v| v.clone()).collect();
                self.enums.insert(name.clone(), EnumRuntimeInfo {
                    variants: variant_names,
                    groups: HashMap::new(),
                    super_group_order: HashMap::new(),
                });
                Ok(())
            }
            SemanticStmt::When { expr, arms, .. } => {
                let val = self.eval_semantic_expr(expr)?;
                self.run_semantic_when(val, arms)
            }
            SemanticStmt::IfElse { .. } => Ok(()), // stub
            SemanticStmt::WhileIn { .. } => Ok(()), // stub
        }
    }

    fn semantic_value_to_runtime(&self, sv: &SemanticValue) -> Value {
        match sv {
            SemanticValue::Num(n) => Value::Num(*n),
            SemanticValue::Float(f) => Value::Float(*f),
            SemanticValue::Str(s) => {
                // Allocate in arena — but for now just store as inline
                Value::Str(0, 0) // stub — needs arena integration
            }
            SemanticValue::Bool(b) => Value::Bool(*b),
            SemanticValue::Char(c) => Value::Char(*c),
            SemanticValue::EnumVariant { enum_name, variant_name, .. } => {
                Value::EnumVariant(enum_name.clone(), variant_name.clone())
            }
            SemanticValue::Unknown => Value::Unknown(Type::T32),
        }
    }

    fn apply_unary(&self, op: &Op, val: Value, pos: usize) -> Result<Value, RuntimeError> {
        match (op, val) {
            (Op::Minus, Value::Num(n)) => Ok(Value::Num(n.wrapping_neg())),
            (Op::Minus, Value::Float(f)) => Ok(Value::Float(-f)),
            (Op::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            (Op::Not, Value::TBool(n)) => Ok(Value::TBool(if n == 0 { 1 } else if n == 1 { 0 } else { 2 })),
            (Op::Mul, v) => Ok(v), // deref — passthrough for now
            _ => Err(RuntimeError::TypeMismatch { pos, expected: Type::Unknown, got: Type::Unknown }),
        }
    }

    fn call_semantic_func(&mut self, callee: &str, args: &[SemanticCallArg], pos: usize) -> Result<Value, RuntimeError> {
        // Built-in: is_known
        if callee == "is_known" {
            if let Some(SemanticCallArg::Expr(e)) = args.first() {
                let val = self.eval_semantic_expr(e)?;
                return Ok(match val {
                    Value::Unknown(_) | Value::TBool(2) => Value::Bool(false),
                    _ => Value::Bool(true),
                });
            }
        }

        // Built-in: print
        if callee == "print" || callee == "println" {
            for arg in args {
                if let SemanticCallArg::Expr(e) = arg {
                    let v = self.eval_semantic_expr(e)?;
                    self.print_value(&v);
                }
            }
            return Ok(Value::Num(0));
        }

        // Check if any arg uses copy semantics — if so, fall back to AST path
        let has_copy_args = args.iter().any(|a| matches!(a,
            SemanticCallArg::Copy { .. } | SemanticCallArg::CopyFree { .. } | SemanticCallArg::CopyInto(_)
        ));

        if has_copy_args {
            // Convert semantic args to AST CallArgs and delegate to old eval_expr path
            let ast_args: Vec<CallArg> = args.iter().map(|a| match a {
                SemanticCallArg::Expr(e) => {
                    // Evaluate the semantic expr and wrap as a literal for the AST path
                    CallArg::Expr(Expr::Val(AstValue::Num(0))) // placeholder — will be resolved below
                }
                SemanticCallArg::Copy { name, .. } => CallArg::Copy(name.clone()),
                SemanticCallArg::CopyFree { name, .. } => CallArg::CopyFree(name.clone()),
                SemanticCallArg::CopyInto(bindings) => CallArg::CopyInto(bindings.iter().map(|b| b.name.clone()).collect()),
            }).collect();

            // For copy calls, we need to pre-evaluate semantic Expr args and inject as identifiers
            // Build a mixed arg list: evaluate semantic exprs, keep copy args as-is
            let mut final_args: Vec<CallArg> = Vec::new();
            for a in args {
                match a {
                    SemanticCallArg::Expr(e) => {
                        let val = self.eval_semantic_expr(e)?;
                        // Store in a temp var and pass as ident
                        let tmp = format!("__tmp_arg_{}", self.string_arena.len());
                        let ty = type_of_value(&val);
                        self.set_var_typed(tmp.clone(), ty, val, pos)?;
                        final_args.push(CallArg::Expr(Expr::Ident(tmp, pos)));
                    }
                    SemanticCallArg::Copy { name, .. } => final_args.push(CallArg::Copy(name.clone())),
                    SemanticCallArg::CopyFree { name, .. } => final_args.push(CallArg::CopyFree(name.clone())),
                    SemanticCallArg::CopyInto(bindings) => final_args.push(CallArg::CopyInto(bindings.iter().map(|b| b.name.clone()).collect())),
                }
            }

            return self.eval_expr(&Expr::Call(callee.to_string(), final_args, pos));
        }

        // Pure expression args — use semantic path
        let func = self.semantic_funcs.get(callee).cloned()
            .ok_or_else(|| RuntimeError::UndefinedVar { pos, name: callee.to_string() })?;

        // Evaluate and bind args using semantic param names
        let mut resolved: Vec<(String, Value)> = Vec::new();
        for (param, arg) in func.params.iter().zip(args.iter()) {
            let val = match arg {
                SemanticCallArg::Expr(e) => self.eval_semantic_expr(e)?,
                _ => unreachable!("copy args handled above"),
            };
            resolved.push((param.name.clone(), val));
        }

        // Push scope, bind params, run semantic body
        self.push_function_scope();
        let result = (|| -> Result<Value, RuntimeError> {
            for (pname, val) in resolved {
                let ty = type_of_value(&val);
                self.set_var_typed(pname, ty, val, pos)?;
            }
            for stmt in &func.body {
                match self.run_semantic_stmt(stmt) {
                    Ok(_) => {}
                    Err(RuntimeError::EarlyReturn(v)) => return Ok(v),
                    Err(e) => return Err(e),
                }
            }
            if let Some(expr) = &func.ret_expr {
                self.eval_semantic_expr(expr)
            } else {
                Ok(Value::Num(0))
            }
        })();
        self.pop_scope();
        result
    }

    fn call_semantic_method(&mut self, instance: &str, method: &str, args: &[SemanticCallArg], pos: usize) -> Result<Value, RuntimeError> {
        // Get instance value and type
        let inst_val = self.get_var(instance, pos)?;
        let type_name = match &inst_val {
            Value::Struct(name, _) => name.clone(),
            _ => return Err(RuntimeError::NotAContainer { pos, name: instance.to_string() }),
        };

        // Look up method in impls
        let (aliases, (_, params, ret_ty, body, ret_expr)) = self.impls
            .get(&(type_name.clone(), method.to_string()))
            .cloned()
            .ok_or_else(|| RuntimeError::UndefinedVar { pos, name: format!("{}.{}", type_name, method) })?;

        // Push scope, bind alias
        self.push_scope();
        for (alias_name, _) in &aliases {
            self.declare(alias_name.clone(), None, pos)?;
            self.set_var(alias_name.clone(), inst_val.clone(), pos)?;
        }

        // Bind params — evaluate semantic args
        for (i, arg) in args.iter().enumerate() {
            if let Some(param) = params.get(i) {
                let param_name = match param {
                    ParamKind::Typed(name, _) => name.clone(),
                    ParamKind::Copy(name) => name.clone(),
                    ParamKind::CopyFree(name) => name.clone(),
                    ParamKind::CopyInto(name, _) => name.clone(),
                };
                let val = match arg {
                    SemanticCallArg::Expr(e) => self.eval_semantic_expr(e)?,
                    SemanticCallArg::Copy { name, .. } => self.get_var(name, pos)?,
                    SemanticCallArg::CopyFree { name, .. } => self.get_var(name, pos)?,
                    _ => Value::Num(0),
                };
                self.declare(param_name.clone(), None, pos)?;
                self.set_var(param_name, val, pos)?;
            }
        }

        // Execute body (AST body from impls registry)
        let mut result = Value::Num(0);
        for s in &body {
            match self.run_stmt(s) {
                Ok(_) => {}
                Err(RuntimeError::EarlyReturn(v)) => { result = v; break; }
                Err(e) => { self.pop_scope(); return Err(e); }
            }
        }
        if let Some(re) = &ret_expr {
            result = self.eval_expr(re)?;
        }

        // Write alias mutations back
        for (alias_name, _) in &aliases {
            if let Ok(updated) = self.get_var(alias_name, pos) {
                self.pop_scope();
                self.set_var(instance.to_string(), updated, pos)?;
                return Ok(result);
            }
        }
        self.pop_scope();
        Ok(result)
    }

    fn print_value(&self, v: &Value) {
        match v {
            Value::Num(n) => println!("{}", n),
            Value::Float(x) => println!("{}", x),
            Value::Str(off, len) => println!("{}", self.resolve_str(*off, *len)),
            Value::Bool(b) => println!("{}", b),
            Value::TBool(b) => println!("{}", match b { 0 => "false", 1 => "true", _ => "?" }),
            Value::Char(c) => println!("{}", c),
            Value::EnumVariant(e, v) => println!("{}::{}", e, v),
            Value::Unknown(_) => println!("?"),
            Value::Array(elems) => {
                let parts: Vec<String> = elems.iter().map(|v| format!("{:?}", v)).collect();
                println!("[{}]", parts.join(", "));
            }
            Value::Handle(h) => println!("handle({},{})", h.slot, h.gen),
            Value::Container(map) => println!("{:?}", map),
            Value::Struct(name, map) => {
                let parts: Vec<String> = map.iter().map(|(k, v)| format!("{}: {:?}", k, v)).collect();
                println!("{} {{ {} }}", name, parts.join(", "));
            }
        }
    }

    fn print_value_inline(&self, v: &Value) {
        match v {
            Value::Num(n) => print!("{}", n),
            Value::Float(x) => print!("{}", x),
            Value::Str(off, len) => print!("{}", self.resolve_str(*off, *len)),
            Value::Bool(b) => print!("{}", b),
            Value::TBool(b) => print!("{}", match b { 0 => "false", 1 => "true", _ => "?" }),
            Value::Char(c) => print!("{}", c),
            Value::EnumVariant(e, v) => print!("{}::{}", e, v),
            Value::Unknown(_) => print!("?"),
            Value::Array(elems) => {
                let parts: Vec<String> = elems.iter().map(|v| format!("{:?}", v)).collect();
                print!("[{}]", parts.join(", "));
            }
            Value::Handle(h) => print!("handle({},{})", h.slot, h.gen),
            Value::Container(map) => print!("{:?}", map),
            Value::Struct(name, map) => {
                let parts: Vec<String> = map.iter().map(|(k, v)| format!("{}: {:?}", k, v)).collect();
                print!("{} {{ {} }}", name, parts.join(", "));
            }
        }
    }

    fn run_semantic_when(&mut self, val: Value, arms: &[SemanticWhenArm]) -> Result<(), RuntimeError> {
        for arm in arms {
            let matches = match &arm.pattern {
                SemanticWhenPattern::Literal(sv) => {
                    let pat_val = self.semantic_value_to_runtime(sv);
                    val == pat_val
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
                for s in &arm.body {
                    match self.run_semantic_stmt(s) {
                        Ok(_) => {}
                        Err(e) => { self.pop_scope(); return Err(e); }
                    }
                }
                self.pop_scope();
                return Ok(());
            }
        }
        Ok(())
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

fn semantic_type_to_ast(st: &SemanticType) -> Type {
    match st {
        SemanticType::I8 => Type::T8,
        SemanticType::I16 => Type::T16,
        SemanticType::I32 => Type::T32,
        SemanticType::I64 => Type::T64,
        SemanticType::I128 => Type::T128,
        SemanticType::F64 => Type::T64,
        SemanticType::Bool => Type::Bool,
        SemanticType::Str => Type::Str,
        SemanticType::StrRef => Type::StrRef,
        SemanticType::Char => Type::Char,
        SemanticType::Enum(name) => Type::Enum(name.clone()),
        SemanticType::Struct(name) => Type::Struct(name.clone()),
        SemanticType::Container => Type::Container,
        SemanticType::Handle(inner) => Type::Handle(Box::new(semantic_type_to_ast(inner))),
        SemanticType::TypeParam(name) => Type::TypeParam(name.clone()),
        SemanticType::Array(size, elem_ty) => Type::Array(*size, Box::new(semantic_type_to_ast(elem_ty))),
        SemanticType::Unknown | SemanticType::Numeric => Type::T64, // fallback
    }
}

fn semantic_param_to_ast(sk: &SemanticParamKind) -> ParamKind {
    match sk {
        SemanticParamKind::Typed => ParamKind::Typed("_".into(), Type::T64), // placeholder — name comes from SemanticParam
        SemanticParamKind::Copy => ParamKind::Copy("_".into()),
        SemanticParamKind::CopyFree => ParamKind::CopyFree("_".into()),
        SemanticParamKind::CopyInto => ParamKind::CopyInto(String::new(), vec![]),
    }
}

impl From<SemanticType> for Type {
    fn from(st: SemanticType) -> Type {
        match st {
            SemanticType::I8 => Type::T8,
            SemanticType::I16 => Type::T16,
            SemanticType::I32 => Type::T32,
            SemanticType::I64 => Type::T64,
            SemanticType::I128 => Type::T128,
            SemanticType::F64 => Type::T64,
            SemanticType::Bool => Type::Bool,
            SemanticType::Str => Type::Str,
            SemanticType::StrRef => Type::StrRef,
            SemanticType::Container => Type::Container,
            SemanticType::Char => Type::Char,
            SemanticType::Enum(name) => Type::Enum(name),
            SemanticType::Unknown => Type::Unknown,
            SemanticType::Handle(inner) => Type::Handle(Box::new((*inner).into())),
            SemanticType::Numeric => Type::T128,
            SemanticType::Struct(name) => Type::Struct(name),
            SemanticType::TypeParam(name) => Type::TypeParam(name),
            SemanticType::Array(size, elem_ty) => Type::Array(size, Box::new((*elem_ty).into())),
        }
    }
}

impl From<SemanticParamKind> for ParamKind {
    fn from(spk: SemanticParamKind) -> ParamKind {
        match spk {
            SemanticParamKind::Typed => ParamKind::Typed(String::new(), Type::Unknown),
            SemanticParamKind::Copy => ParamKind::Copy(String::new()),
            SemanticParamKind::CopyFree => ParamKind::CopyFree(String::new()),
            SemanticParamKind::CopyInto => ParamKind::CopyInto(String::new(), vec![]),
        }
    }
}
