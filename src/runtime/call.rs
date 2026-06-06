use super::runtime::*;
use crate::frontend::{ast::*, types::*};
use crate::frontend::builtins::{self, BuiltinKind};
use crate::frontend::semantic_types::*;
use std::collections::HashMap;

impl RunTime {
    pub fn call_semantic_func(&mut self, callee: &str, args: &[SemanticCallArg], pos: usize) -> Result<Value, RuntimeError> {
        // Built-in dispatch — names come from the single-source-of-truth
        // registry (crate::frontend::builtins, #008). Each kind's body is its
        // pre-#008 behavior verbatim. Only `is_known` may fall out of the match
        // (when its first arg is not an Expr) to the user-function path below;
        // every other arm returns.
        if let Some(def) = builtins::lookup(callee) {
            match def.kind {
                BuiltinKind::IsKnown => {
                    if let Some(SemanticCallArg::Expr(e)) = args.first() {
                        let val = self.eval_semantic_expr(e)?;
                        return Ok(match val {
                            Value::Unknown(_) | Value::TBool(2) => Value::Bool(false),
                            _ => Value::Bool(true),
                        });
                    }
                }

                // len(x) — tracker #021. Byte length for a string (the stored
                // arena-slice length, not char/grapheme count) and element count
                // for an array. Returns t64. The semantic layer guarantees the
                // arg is a string or array, so the final arm is defensive.
                BuiltinKind::Len => {
                    let val = match args.first() {
                        Some(SemanticCallArg::Expr(e)) => self.eval_semantic_expr(e)?,
                        _ => return Err(RuntimeError::TypeMismatch { pos, expected: Type::Str, got: Type::Void }),
                    };
                    let n: i128 = match &val {
                        Value::Str(_, len) => *len as i128,
                        Value::Array(elems) => elems.len() as i128,
                        _ => return Err(RuntimeError::TypeMismatch { pos, expected: Type::Str, got: type_of_value(&val) }),
                    };
                    return Ok(Value::Num(n));
                }

                // exit(code) / exit() — request process termination. Returns the
                // Exit control-flow signal (NOT a process::exit call here); the
                // top-level loop / --test loop translates it to a real exit so
                // stdout can be flushed first. Negative codes pass through to i32
                // unchanged; the OS layer (POSIX) may truncate to u8 — not
                // normalised.
                BuiltinKind::Exit => {
                    let code: i32 = match args.first() {
                        None => 0,
                        Some(SemanticCallArg::Expr(e)) => {
                            let v = self.eval_semantic_expr(e)?;
                            match v {
                                Value::Num(n) => {
                                    if n < i32::MIN as i128 || n > i32::MAX as i128 {
                                        // Out of i32 range: surface as a normal
                                        // runtime error, not an Exit signal.
                                        return Err(RuntimeError::TypeMismatch {
                                            pos,
                                            expected: Type::T32,
                                            got: Type::T64,
                                        });
                                    }
                                    n as i32
                                }
                                other => {
                                    return Err(RuntimeError::TypeMismatch {
                                        pos,
                                        expected: Type::T32,
                                        got: type_of_value(&other),
                                    });
                                }
                            }
                        }
                        // Non-Expr arg form (copy/copyfree/copyinto) is not valid
                        // for exit; arity/shape is enforced in the semantic phase.
                        Some(_) => 0,
                    };
                    return Err(RuntimeError::Exit(code));
                }

                // print (with newline) and println — see #034: both currently
                // behave identically (no distinct newline).
                BuiltinKind::Print | BuiltinKind::Println => {
                    for arg in args {
                        if let SemanticCallArg::Expr(e) = arg {
                            let v = self.eval_semantic_expr(e)?;
                            self.print_value(&v, pos)?;
                        }
                    }
                    return Ok(Value::Num(0));
                }

                // printn (no newline)
                BuiltinKind::Printn => {
                    for arg in args {
                        if let SemanticCallArg::Expr(e) = arg {
                            let v = self.eval_semantic_expr(e)?;
                            self.print_value_inline(&v, pos)?;
                        }
                    }
                    return Ok(Value::Num(0));
                }

                // assert(cond) — runtime error if condition is false
                BuiltinKind::Assert => {
                    if let Some(SemanticCallArg::Expr(e)) = args.first() {
                        let val = self.eval_semantic_expr(e)?;
                        let passed = match val {
                            Value::Bool(b) => b,
                            Value::Num(n) => n != 0,
                            _ => false,
                        };
                        if !passed {
                            return Err(RuntimeError::AssertionFailed {
                                msg: "assertion failed".to_string(),
                                pos,
                            });
                        }
                    }
                    return Ok(Value::Num(0));
                }

                // assert_eq(a, b) — runtime error if a != b
                BuiltinKind::AssertEq => {
                    let mut iter = args.iter();
                    let left = if let Some(SemanticCallArg::Expr(e)) = iter.next() {
                        self.eval_semantic_expr(e)?
                    } else { return Ok(Value::Num(0)); };
                    let right = if let Some(SemanticCallArg::Expr(e)) = iter.next() {
                        self.eval_semantic_expr(e)?
                    } else { return Ok(Value::Num(0)); };
                    let equal = match (&left, &right) {
                        (Value::Num(a), Value::Num(b)) => a == b,
                        (Value::Bool(a), Value::Bool(b)) => a == b,
                        (Value::Float(a), Value::Float(b)) => a == b,
                        (Value::Str(ao, al), Value::Str(bo, bl)) => {
                            self.resolve_str(*ao, *al) == self.resolve_str(*bo, *bl)
                        }
                        (Value::ResultOk(a), Value::ResultOk(b)) => a == b,
                        (Value::ResultErr(a), Value::ResultErr(b)) => a == b,
                        _ => false,
                    };
                    if !equal {
                        return Err(RuntimeError::AssertionFailed {
                            msg: format!("assert_eq failed: {} != {}",
                                value_to_string(self, left),
                                value_to_string(self, right)),
                            pos,
                        });
                    }
                    return Ok(Value::Num(0));
                }

                // read(var) — reads a line from stdin into var
                BuiltinKind::Read => {
                    if let Some(SemanticCallArg::Expr(e)) = args.first() {
                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input).unwrap_or(0);
                        let input = input.trim_end_matches('\n').trim_end_matches('\r').to_string();
                        let (off, len) = self.alloc_str(&input);
                        if let SemanticExprKind::VarRef { name, .. } = &e.kind {
                            self.set_var(name.clone(), Value::Str(off, len), 0)?;
                        }
                        return Ok(Value::Str(off, len));
                    }
                    return Ok(Value::Num(0));
                }

                // input("prompt", var) — prints prompt then reads into var
                BuiltinKind::Input => {
                    let mut iter = args.iter();
                    // First arg is the prompt string
                    if let Some(SemanticCallArg::Expr(prompt_expr)) = iter.next() {
                        let prompt_val = self.eval_semantic_expr(prompt_expr)?;
                        match &prompt_val {
                            Value::Str(off, len) => print!("{}", self.resolve_str(*off, *len)),
                            other => print!("{}", value_to_string(self, other.clone())),
                        }
                        use std::io::Write;
                        std::io::stdout().flush().unwrap_or(());
                    }
                    // Second arg is the variable to fill
                    if let Some(SemanticCallArg::Expr(var_expr)) = iter.next() {
                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input).unwrap_or(0);
                        let input = input.trim_end_matches('\n').trim_end_matches('\r').to_string();
                        let (off, len) = self.alloc_str(&input);
                        if let SemanticExprKind::VarRef { name, .. } = &var_expr.kind {
                            self.set_var(name.clone(), Value::Str(off, len), 0)?;
                        }
                        return Ok(Value::Str(off, len));
                    }
                    return Ok(Value::Num(0));
                }
            }
        }

        // Native copy semantics — no fallback to AST path needed
        let sem_func = self.semantic_funcs.get(callee).cloned()
            .ok_or_else(|| RuntimeError::UndefinedVar { pos, name: callee.to_string() })?;

        let outer_scope_idx = self.scopes.len() - 1;
        let mut resolved: Vec<(BindingId, String, Value, Option<String>)> = Vec::new();
        // (param binding, inner param name, value, bleed_back outer name if .copy)

        for (param, arg) in sem_func.params.iter().zip(args.iter()) {
            match arg {
                SemanticCallArg::Expr(e) => {
                    let val = self.eval_semantic_expr(e)?;
                    resolved.push((param.binding, param.name.clone(), val, None));
                }
                SemanticCallArg::Copy { name, .. } => {
                    let val = self.get_var(name, pos)?;
                    resolved.push((param.binding, param.name.clone(), val, Some(name.clone())));
                    // bleed_back registered below after push_function_scope
                }
                SemanticCallArg::CopyFree { name, .. } => {
                    let val = self.get_var(name, pos)?;
                    resolved.push((param.binding, param.name.clone(), val, None));
                    // no bleed_back — isolated copy
                }
                SemanticCallArg::CopyInto(bindings) => {
                    let mut map = HashMap::new();
                    for b in bindings {
                        let val = self.get_var(&b.name, pos)?;
                        map.insert(b.name.clone(), val);
                    }
                    resolved.push((param.binding, param.name.clone(), Value::Container(map), None));
                }
            }
        }

        self.push_function_scope();

        let result = (|| -> Result<Value, RuntimeError> {
            for (pbinding, pname, val, bleed_outer) in resolved {
                let ty = type_of_value(&val);
                self.set_var_typed(pbinding, pname.clone(), ty, val, pos)?;
                if let Some(outer_name) = bleed_outer {
                    if let Some(frame) = self.scopes.last_mut() {
                        frame.bleed_back.insert(pname, (outer_scope_idx, outer_name));
                    }
                }
            }
            for stmt in &sem_func.body {
                match self.run_semantic_stmt(stmt) {
                    Ok(_) => {}
                    Err(RuntimeError::EarlyReturn(v)) => return Ok(v),
                    Err(e) => return Err(e),
                }
            }
            if let Some(expr) = &sem_func.ret_expr {
                self.eval_semantic_expr(expr)
            } else {
                Ok(Value::Num(0))
            }
        })();

        self.pop_scope();
        result
    }

    pub(crate) fn call_semantic_method(&mut self, instance: &str, method: &str, args: &[SemanticCallArg], pos: usize) -> Result<Value, RuntimeError> {
        // Get primary instance value and type
        let instance_val = self.get_var(instance, pos)?;
        let type_name = match &instance_val {
            Value::Struct(name, _) => name.clone(),
            _ => return Err(RuntimeError::NotAContainer { pos, name: instance.to_string() }),
        };

        // Look up in semantic impl registry
        let (aliases, sem_func) = self.semantic_impls
            .get(&(type_name.clone(), method.to_string()))
            .cloned()
            .ok_or_else(|| RuntimeError::UndefinedVar { pos, name: format!("{}.{}", instance, method) })?;

        // Aliases beyond the first are passed as leading args at the call site
        // args[0..extra_alias_count] = additional alias values
        // args[extra_alias_count..] = regular params
        let extra_alias_count = aliases.len().saturating_sub(1);

        // Resolve additional alias values from leading args
        let mut alias_vals: Vec<(BindingId, String, Type, Value)> = Vec::new();
        // First alias is always the dot receiver
        let (first_alias_binding, first_alias_name, first_alias_type) = &aliases[0];
        alias_vals.push((*first_alias_binding, first_alias_name.clone(), first_alias_type.clone().into(), instance_val.clone()));

        // Remaining aliases come from leading call args
        for i in 0..extra_alias_count {
            let (alias_binding, alias_name, alias_type) = &aliases[i + 1];
            let val = match args.get(i) {
                Some(SemanticCallArg::Expr(e)) => self.eval_semantic_expr(e)?,
                Some(SemanticCallArg::Copy { name, .. }) => self.get_var(name, pos)?,
                Some(SemanticCallArg::CopyFree { name, .. }) => self.get_var(name, pos)?,
                _ => return Err(RuntimeError::UndefinedVar { pos, name: format!("missing alias arg {}", i) }),
            };
            alias_vals.push((*alias_binding, alias_name.clone(), alias_type.clone().into(), val));
        }

        // Resolve regular params from remaining args
        let regular_args = &args[extra_alias_count..];
        let mut resolved_params: Vec<(BindingId, String, Value)> = Vec::new();
        for (param, arg) in sem_func.params.iter().zip(regular_args.iter()) {
            let val = match arg {
                SemanticCallArg::Expr(e) => self.eval_semantic_expr(e)?,
                SemanticCallArg::Copy { name, .. } => self.get_var(name, pos)?,
                SemanticCallArg::CopyFree { name, .. } => self.get_var(name, pos)?,
                SemanticCallArg::CopyInto(bindings) => {
                    let mut map = HashMap::new();
                    for b in bindings {
                        map.insert(b.name.clone(), self.get_var(&b.name, pos)?);
                    }
                    Value::Container(map)
                }
            };
            resolved_params.push((param.binding, param.name.clone(), val));
        }

        // Push scope
        self.push_function_scope();

        let result = (|| -> Result<Value, RuntimeError> {
            // Bind all aliases into scope
            for (alias_binding, alias_name, alias_ty, val) in &alias_vals {
                self.set_var_typed(*alias_binding, alias_name.clone(), alias_ty.clone(), val.clone(), pos)?;
            }

            // Bind regular params
            for (pbinding, pname, val) in resolved_params {
                let ty = type_of_value(&val);
                self.set_var_typed(pbinding, pname, ty, val, pos)?;
            }

            // Run semantic body
            for stmt in &sem_func.body {
                match self.run_semantic_stmt(stmt) {
                    Ok(_) => {}
                    Err(RuntimeError::EarlyReturn(v)) => return Ok(v),
                    Err(e) => return Err(e),
                }
            }

            // Evaluate return expression
            if let Some(expr) = &sem_func.ret_expr {
                self.eval_semantic_expr(expr)
            } else {
                Ok(Value::Num(0))
            }
        })();

        // Capture all alias mutations before popping scope
        let mut mutated_aliases: Vec<(String, Value)> = Vec::new();
        if result.is_ok() {
            for (_, alias_name, _, _) in &alias_vals {
                if let Ok(mutated) = self.get_var(alias_name, pos) {
                    mutated_aliases.push((alias_name.clone(), mutated));
                }
            }
        }

        self.pop_scope();

        // Write all alias mutations back to caller scope
        // First alias writes back to instance
        // Additional aliases write back to the variable names passed as leading args
        if result.is_ok() {
            for (i, (_, mutated)) in mutated_aliases.iter().enumerate() {
                if i == 0 {
                    let _ = self.set_var(instance.to_string(), mutated.clone(), pos);
                } else if i <= extra_alias_count {
                    // Get the original variable name from the leading arg
                    if let Some(SemanticCallArg::Expr(e)) = args.get(i - 1) {
                        if let SemanticExprKind::VarRef { name, .. } = &e.kind {
                            let _ = self.set_var(name.clone(), mutated.clone(), pos);
                        }
                    } else if let Some(SemanticCallArg::Copy { name, .. }) = args.get(i - 1) {
                        let _ = self.set_var(name.clone(), mutated.clone(), pos);
                    }
                }
            }
        }

        result
    }
}
