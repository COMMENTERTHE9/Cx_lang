use std::collections::HashMap;

use crate::frontend::ast::*;

#[derive(Debug, Clone)]
pub struct SemanticError {
    pub msg: String,
    pub pos: usize,
}

#[derive(Debug, Clone)]
struct VarInfo {
    declared: Option<Type>,
    inferred: Option<Ty>,
    initialized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ty {
    Num,
    Bool,
    TBool,
    Str,
    Char,
    Enum(String),
    Unknown,
    Handle,
}
fn ty_from_decl(t: Type) -> Ty {
    match t {
        Type::T8 | Type::T16 | Type::T32 | Type::T64 | Type::T128 => Ty::Num,
        Type::Bool => Ty::Bool,
        Type::Str => Ty::Str,
        Type::Char => Ty::Char,
        Type::Enum(name) => Ty::Enum(name.clone()),
        Type::Unknown => Ty::Unknown,
        Type::Handle(_) => Ty::Handle,
    }
}
fn ty_from_value(v: &AstValue) -> Ty {
    match v {
        AstValue::Num(_) => Ty::Num,
        AstValue::Float(_) => Ty::Num,
        AstValue::Bool(_) => Ty::Bool,
        AstValue::Str(_) => Ty::Str,
        AstValue::Char(_) => Ty::Char,
        AstValue::EnumVariant(enum_name, _) => Ty::Enum(enum_name.clone()),
        AstValue::Unknown => Ty::Unknown,
    }
}

fn check_num_range(ty: Type, n: u128, pos: usize) -> Result<(), SemanticError> {
    let max: Option<u128> = match ty {
        Type::T8 => Some(u8::MAX as u128),
        Type::T16 => Some(u16::MAX as u128),
        Type::T32 => Some(u32::MAX as u128),
        Type::T64 => Some(u64::MAX as u128),
        Type::T128 => None,
        _ => None,
    };

    if let Some(max) = max {
        if n > max {
            return Err(SemanticError {
                msg: format!("value {} overflows type {:?} (max {})", n, ty, max),
                pos,
            });
        }
    }
    Ok(())
}

type Scope = HashMap<String, VarInfo>;

pub struct Analyzer {
    scopes: Vec<Scope>,
    current_ret_ty: Option<Ty>,
    in_function: bool,
    funcs: HashMap<String, (Vec<Option<Ty>>, Option<Ty>)>,
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            current_ret_ty: None,
            in_function: false,
            funcs: HashMap::new(),
        } // global scope
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, info: VarInfo, pos: usize) -> Result<(), SemanticError> {
        let scope = self.scopes.last_mut().unwrap();
        if scope.contains_key(name) {
            return Err(SemanticError {
                msg: format!("variable already declared in this scope: {}", name),
                pos,
            });
        }
        scope.insert(name.to_string(), info);
        Ok(())
    }

    fn lookup_var(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    fn lookup_var_mut(&mut self, name: &str) -> Option<&mut VarInfo> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                return Some(info);
            }
        }
        None
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> Result<(), SemanticError> {
        match stmt {
            Stmt::EnumDef { .. } => Ok(()),
            // Declare introduces a symbol in the current scope
            Stmt::Decl { name, ty, pos } => self.declare(
                name,
                VarInfo {
                    declared: ty.clone(),
                    inferred: None,
                    initialized: false,
                },
                *pos,
            ),

            // Assignment: type-check RHS and update declared/inferred type state.
            Stmt::Assign { target, expr, pos_eq } => {
                let expr_ty = self.type_expr(expr)?;
                match target {
                    Expr::Ident(name, _) => {
                        let info = self.lookup_var_mut(name).ok_or_else(|| SemanticError {
                            msg: format!("use of undeclared variable '{}'", name),
                            pos: *pos_eq,
                        })?;

                        if let Some(declared) = &info.declared {
                            let expected = ty_from_decl(declared.clone());
                            if expected != expr_ty {
                                return Err(SemanticError {
                                    msg: format!(
                                        "type mismatch: expected {:?}, got {:?}",
                                        expected, expr_ty
                                    ),
                                    pos: *pos_eq,
                                });
                            }
                        } else if let Some(expected) = &info.inferred {
                            if *expected != expr_ty {
                                return Err(SemanticError {
                                    msg: format!(
                                        "type mismatch: expected {:?}, got {:?}",
                                        expected, expr_ty
                                    ),
                                    pos: *pos_eq,
                                });
                            }
                        } else {
                            info.inferred = Some(expr_ty);
                        }

                        info.initialized = true;
                        Ok(())
                    }
                    Expr::DotAccess(container, _) => {
                        let info = self.lookup_var(container).ok_or_else(|| SemanticError {
                            msg: format!("use of undeclared variable '{}'", container),
                            pos: *pos_eq,
                        })?;
                        if !info.initialized {
                            return Err(SemanticError {
                                msg: format!("use of uninitialized variable '{}'", container),
                                pos: *pos_eq,
                            });
                        }
                        Ok(())
                    }
                    _ => Err(SemanticError {
                        msg: "bad assignment target".to_string(),
                        pos: *pos_eq,
                    }),
                }
            }

            // TypedAssign: type-check RHS and introduce a typed, initialized variable.
            Stmt::TypedAssign {
                name,
                pos_type,
                expr,
                ty,
            } => {
                let expr_ty = self.type_expr(expr)?;

                if let Expr::Val(AstValue::Num(n)) = expr {
                    check_num_range(ty.clone(), *n, *pos_type)?;
                }

                let declared_ty = ty_from_decl(ty.clone());
                if expr_ty != declared_ty {
                    return Err(SemanticError {
                        msg: format!(
                            "type mismatch: expected {:?}, got {:?}",
                            declared_ty, expr_ty
                        ),
                        pos: *pos_type,
                    });
                }

                self.declare(
                    name,
                    VarInfo {
                        declared: Some(ty.clone()),
                        inferred: None,
                        initialized: true,
                    },
                    *pos_type,
                )
            }

            Stmt::FuncDef {
                name,
                params,
                ret_ty,
                body,
                ret_expr,
                pos,
            } => {
                self.declare(
                    name,
                    VarInfo {
                        declared: ret_ty.clone(),
                        inferred: None,
                        initialized: true,
                    },
                    *pos,
                )?;

                let param_tys: Vec<Option<Ty>> = params
                    .iter()
                    .map(|param| match param {
                        ParamKind::Typed(_, ty) => Some(ty_from_decl(ty.clone())),
                        ParamKind::Copy(_) | ParamKind::CopyFree(_) | ParamKind::CopyInto(_, _) => None,
                    })
                    .collect();
                let ret = ret_ty.clone().map(ty_from_decl);
                self.funcs.insert(name.clone(), (param_tys, ret));

                self.push_scope();

                let prev_ret_ty = self.current_ret_ty.clone();
                let prev_in_function = self.in_function;
                self.in_function = true;
                self.current_ret_ty = ret_ty.clone().map(ty_from_decl);

                for param in params {
                    match param {
                        ParamKind::Typed(pname, pty) => {
                            self.declare(
                                pname,
                                VarInfo {
                                    declared: Some(pty.clone()),
                                    inferred: None,
                                    initialized: true,
                                },
                                *pos,
                            )?;
                        }
                        ParamKind::Copy(pname) | ParamKind::CopyFree(pname) => {
                            self.declare(
                                pname,
                                VarInfo {
                                    declared: None,
                                    inferred: None,
                                    initialized: true,
                                },
                                *pos,
                            )?;
                        }
                        ParamKind::CopyInto(pname, _) => {
                            self.declare(
                                pname,
                                VarInfo {
                                    declared: None,
                                    inferred: None,
                                    initialized: true,
                                },
                                *pos,
                            )?;
                        }
                    }
                }
                for s in body {
                    self.analyze_stmt(s)?;
                }

                if let Some(expr) = ret_expr {
                    let got = self.type_expr(expr)?;
                    if let Some(expected) = &self.current_ret_ty {
                        if got != *expected {
                            return Err(SemanticError {
                                msg: format!(
                                    "return type mismatch: expected {:?}, got {:?}",
                                    expected, got
                                ),
                                pos: *pos,
                            });
                        }
                    }
                } else if ret_ty.is_some() && !contains_return_stmt(body) {
                    let expected = ty_from_decl(ret_ty.clone().unwrap());
                    return Err(SemanticError {
                        msg: format!("missing return value, expected {:?}", expected),
                        pos: *pos,
                    });
                }

                self.current_ret_ty = prev_ret_ty;
                self.in_function = prev_in_function;
                self.pop_scope();
                Ok(())
            }

            // Print: analyze expr
            Stmt::Print { expr, .. } => self.type_expr(expr).map(|_| ()),
            Stmt::PrintInline { expr, .. } => self.type_expr(expr).map(|_| ()),
            Stmt::ExprStmt { expr, .. } => self.type_expr(expr).map(|_| ()),
            Stmt::Return { expr, pos } => {
                if !self.in_function {
                    return Err(SemanticError {
                        msg: "return used outside function body".to_string(),
                        pos: *pos,
                    });
                }

                match (expr, self.current_ret_ty.clone()) {
                    (Some(e), Some(expected)) => {
                        let got = self.type_expr(e)?;
                        if got != expected {
                            return Err(SemanticError {
                                msg: format!(
                                    "return type mismatch: expected {:?}, got {:?}",
                                    expected, got
                                ),
                                pos: *pos,
                            });
                        }
                    }
                    (None, None) => {}
                    (None, Some(expected)) => {
                        return Err(SemanticError {
                            msg: format!("missing return value, expected {:?}", expected),
                            pos: *pos,
                        });
                    }
                    (Some(e), None) => {
                        self.type_expr(e)?;
                        return Err(SemanticError {
                            msg: "unexpected return value in void function".to_string(),
                            pos: *pos,
                        });
                    }
                }
                Ok(())
            }

            // Block: push scope, analyze children, pop scope
            Stmt::Block { stmts, .. } => {
                self.push_scope();
                for s in stmts {
                    self.analyze_stmt(s)?;
                }
                self.pop_scope();
                Ok(())
            }
            Stmt::When { expr, arms, pos: _ } => {
                let _ = self.type_expr(expr)?;
                let _has_catchall = arms.iter().any(|a| matches!(a.pattern, WhenPattern::Catchall));
                for arm in arms {
                    for s in &arm.body {
                        self.analyze_stmt(s)?;
                    }
                }
                Ok(())
            }
            Stmt::While { cond, body, pos } => {
                let cond_ty = self.type_expr(cond)?;
                if matches!(cond_ty, Ty::Unknown) {
                    return Err(SemanticError {
                        msg: "Unknown value cannot be used as a loop condition — control-critical context".to_string(),
                        pos: *pos,
                    });
                }
                for s in body {
                    self.analyze_stmt(s)?;
                }
                Ok(())
            }
            Stmt::For { var, body, pos, .. } => {
                self.push_scope();
                self.declare(
                    var,
                    VarInfo {
                        declared: Some(Type::T64),
                        inferred: Some(Ty::Num),
                        initialized: true,
                    },
                    *pos,
                )?;
                for s in body {
                    match s {
                        Stmt::Assign { target: Expr::Ident(name, _), .. } if name == var => {
                            self.pop_scope();
                            return Err(SemanticError {
                                msg: format!("loop variable '{}' is read-only", var),
                                pos: *pos,
                            });
                        }
                        Stmt::CompoundAssign { name, .. } if name == var => {
                            self.pop_scope();
                            return Err(SemanticError {
                                msg: format!("loop variable '{}' is read-only", var),
                                pos: *pos,
                            });
                        }
                        _ => {}
                    }
                    self.analyze_stmt(s)?;
                }
                self.pop_scope();
                Ok(())
            }
            Stmt::Loop { body, .. } => {
                for s in body {
                    self.analyze_stmt(s)?;
                }
                Ok(())
            }
            Stmt::Break { .. } => Ok(()),
            Stmt::Continue { .. } => Ok(()),
            Stmt::CompoundAssign { .. } => Ok(()),
        }
    }

    fn type_expr(&mut self, expr: &Expr) -> Result<Ty, SemanticError> {
        match expr {
            Expr::Val(v) => Ok(ty_from_value(v)),
            Expr::Ident(name, pos) => {
                let info = self.lookup_var(name).ok_or_else(|| SemanticError {
                    msg: format!("use of undeclared variable '{}'", name),
                    pos: *pos,
                })?;

                if !info.initialized {
                    return Err(SemanticError {
                        msg: format!("use of uninitialized variable '{}'", name),
                        pos: *pos,
                    });
                }

                if let Some(t) = &info.declared {
                    Ok(ty_from_decl(t.clone()))
                } else if let Some(t) = &info.inferred {
                    Ok(t.clone())
                } else {
                    Ok(Ty::Num)
                }
            }
            Expr::Call(name, args, pos) => {
                let (param_tys, ret_ty) =
                    self.funcs
                        .get(name)
                        .cloned()
                        .ok_or_else(|| SemanticError {
                            msg: format!("call to undefined function '{}'", name),
                            pos: *pos,
                        })?;

                if args.len() != param_tys.len() {
                    return Err(SemanticError {
                        msg: format!(
                            "function '{}' expects {} argument(s), got {}",
                            name,
                            param_tys.len(),
                            args.len()
                        ),
                        pos: *pos,
                    });
                }

                for (i, arg) in args.iter().enumerate() {
                    match arg {
                        CallArg::Expr(expr) => {
                            if let Some(Some(expected)) = param_tys.get(i) {
                                let got = self.type_expr(expr)?;
                                if got != *expected {
                                    return Err(SemanticError {
                                        msg: format!(
                                            "argument {} to '{}': expected {:?}, got {:?}",
                                            i + 1,
                                            name,
                                            expected,
                                            got
                                        ),
                                        pos: *pos,
                                    });
                                }
                            }
                        }
                        CallArg::Copy(outer_name) | CallArg::CopyFree(outer_name) => {
                            let info = self.lookup_var(outer_name).ok_or_else(|| SemanticError {
                                msg: format!("'.copy' argument '{}' has not been declared", outer_name),
                                pos: *pos,
                            })?;
                            if !info.initialized {
                                return Err(SemanticError {
                                    msg: format!("'.copy' argument '{}' is not initialized", outer_name),
                                    pos: *pos,
                                });
                            }
                        }
                        CallArg::CopyInto(outer_names) => {
                            for oname in outer_names {
                                let info = self.lookup_var(oname).ok_or_else(|| SemanticError {
                                    msg: format!("copy_into variable '{}' has not been declared", oname),
                                    pos: *pos,
                                })?;
                                if !info.initialized {
                                    return Err(SemanticError {
                                        msg: format!("copy_into variable '{}' is not initialized", oname),
                                        pos: *pos,
                                    });
                                }
                            }
                        }
                    }
                }

                Ok(ret_ty.unwrap_or(Ty::Num))
            }
            Expr::DotAccess(container, _field) => {
                let info = self.lookup_var(container).ok_or_else(|| SemanticError {
                    msg: format!("use of undeclared variable '{}'", container),
                    pos: 0,
                })?;
                if !info.initialized {
                    return Err(SemanticError {
                        msg: format!("use of uninitialized variable '{}'", container),
                        pos: 0,
                    });
                }
                Ok(Ty::Num)
            }
            Expr::Range(_, _, _) => Ok(Ty::Num),
            Expr::HandleNew(_, _) => Ok(Ty::Handle),
            Expr::HandleVal(_, _) => Ok(Ty::Num),
            Expr::HandleDrop(_, _) => Ok(Ty::Handle),
            Expr::Unary(_, inner, _) => self.type_expr(inner),

            Expr::Bin(lhs, op, op_pos, rhs) => {
                let lt = self.type_expr(lhs)?;
                let rt = self.type_expr(rhs)?;
                match op {
                    Op::Plus | Op::Minus | Op::Mul | Op::Div | Op::Mod => {
                        if lt == Ty::Unknown || rt == Ty::Unknown {
                            Ok(Ty::Unknown)
                        } else if lt == Ty::Num && rt == Ty::Num {
                            Ok(Ty::Num)
                        } else {
                            Err(SemanticError {
                                msg: format!(
                                    "arithmetic requires numeric operands, got {:?} and {:?}",
                                    lt, rt
                                ),
                                pos: *op_pos,
                            })
                        }
                    }
                    Op::EqEq => {
                        if lt == Ty::Unknown || rt == Ty::Unknown {
                            Ok(Ty::Unknown)
                        } else if matches!(
                            (lt.clone(), rt.clone()),
                            (Ty::Num, Ty::Num)
                                | (Ty::Bool, Ty::Bool)
                                | (Ty::Char, Ty::Char)
                                | (Ty::Str, Ty::Str)
                        ) {
                            Ok(Ty::Bool)
                        } else {
                            Err(SemanticError {
                                msg: format!("cannot compare {:?} == {:?}", lt, rt),
                                pos: *op_pos,
                            })
                        }
                    }
                    Op::Lt | Op::Gt | Op::LtEq | Op::GtEq => {
                        if lt == Ty::Unknown || rt == Ty::Unknown {
                            Ok(Ty::Unknown)
                        } else {
                            Ok(Ty::Bool)
                        }
                    }
                    Op::And | Op::Or => {
                        if matches!(lt, Ty::Bool | Ty::Unknown) && matches!(rt, Ty::Bool | Ty::Unknown) {
                            if lt == Ty::Unknown || rt == Ty::Unknown {
                                Ok(Ty::Unknown)
                            } else {
                                Ok(Ty::Bool)
                            }
                        } else {
                            Err(SemanticError {
                                msg: format!(
                                    "logical operation requires bool operands, got {:?} and {:?}",
                                    lt, rt
                                ),
                                pos: *op_pos,
                            })
                        }
                    }
                }
            }
        }
    }
}

fn contains_return_stmt(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_return)
}

fn stmt_contains_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::Block { stmts, .. } => contains_return_stmt(stmts),
        // Nested function returns do not count for the outer function.
        Stmt::FuncDef { .. } => false,
        _ => false,
    }
}

pub fn analyze_program(program: &Program) -> Vec<SemanticError> {
    let mut a = Analyzer::new();
    let mut errors = Vec::new();
    for stmt in &program.stmts {
        if let Err(e) = a.analyze_stmt(stmt) {
            errors.push(e);
        }
    }
    errors
}

