use std::collections::HashMap;

use crate::{CallArg, Expr, Op, ParamKind, Program, SemanticError, Stmt, Type, Value};

#[derive(Debug, Clone)]
struct VarInfo {
    declared: Option<Type>,
    inferred: Option<Ty>,
    initialized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Num,
    Bool,
    Str,
    Char,
}

fn ty_from_decl(t: Type) -> Ty {
    match t {
        Type::T8 | Type::T16 | Type::T32 | Type::T64 | Type::T128 => Ty::Num,
        Type::Bool => Ty::Bool,
        Type::Str => Ty::Str,
        Type::Char => Ty::Char,
    }
}

fn ty_from_value(v: &Value) -> Ty {
    match v {
        Value::Num(_) => Ty::Num,
        Value::Float(_) => Ty::Num,
        Value::Bool(_) => Ty::Bool,
        Value::Str(_) => Ty::Str,
        Value::Char(_) => Ty::Char,
        Value::Container(_) => Ty::Str,
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

    pub fn analyze_program(&mut self, program: &Program) -> Result<(), SemanticError> {
        for stmt in &program.stmts {
            self.analyze_stmt(stmt)?;
        }
        Ok(())
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> Result<(), SemanticError> {
        match stmt {
            // Declare introduces a symbol in the current scope
            Stmt::Decl { name, ty, pos } => self.declare(
                name,
                VarInfo {
                    declared: *ty,
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

                        if let Some(declared) = info.declared {
                            let expected = ty_from_decl(declared);
                            if expected != expr_ty {
                                return Err(SemanticError {
                                    msg: format!(
                                        "type mismatch: expected {:?}, got {:?}",
                                        expected, expr_ty
                                    ),
                                    pos: *pos_eq,
                                });
                            }
                        } else if let Some(expected) = info.inferred {
                            if expected != expr_ty {
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

                if let Expr::Val(Value::Num(n)) = expr {
                    check_num_range(*ty, *n, *pos_type)?;
                }

                let declared_ty = ty_from_decl(*ty);
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
                        declared: Some(*ty),
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
                        declared: *ret_ty,
                        inferred: None,
                        initialized: true,
                    },
                    *pos,
                )?;

                let param_tys: Vec<Option<Ty>> = params
                    .iter()
                    .map(|param| match param {
                        ParamKind::Typed(_, ty) => Some(ty_from_decl(*ty)),
                        ParamKind::Copy(_) | ParamKind::CopyFree(_) => None,
                    })
                    .collect();
                let ret = ret_ty.map(ty_from_decl);
                self.funcs.insert(name.clone(), (param_tys, ret));

                self.push_scope();

                let prev_ret_ty = self.current_ret_ty;
                let prev_in_function = self.in_function;
                self.in_function = true;
                self.current_ret_ty = ret_ty.map(ty_from_decl);

                for param in params {
                    match param {
                        ParamKind::Typed(pname, pty) => {
                            self.declare(
                                pname,
                                VarInfo {
                                    declared: Some(*pty),
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
                    }
                }
                for s in body {
                    self.analyze_stmt(s)?;
                }

                if let Some(expr) = ret_expr {
                    let got = self.type_expr(expr)?;
                    if let Some(expected) = self.current_ret_ty {
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
                } else if ret_ty.is_some() && !contains_return_stmt(body) {
                    let expected = ty_from_decl(ret_ty.unwrap());
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

                match (expr, self.current_ret_ty) {
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

                if let Some(t) = info.declared {
                    Ok(ty_from_decl(t))
                } else if let Some(t) = info.inferred {
                    Ok(t)
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
                        CallArg::CopyInto(_) => {}
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

            Expr::Bin(lhs, op, op_pos, rhs) => {
                let lt = self.type_expr(lhs)?;
                let rt = self.type_expr(rhs)?;

                match op {
                    Op::Plus | Op::Minus | Op::Mul | Op::Div | Op::Mod => {
                        if lt == Ty::Num && rt == Ty::Num {
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
                        let ok = matches!(
                            (lt, rt),
                            (Ty::Num, Ty::Num)
                                | (Ty::Bool, Ty::Bool)
                                | (Ty::Char, Ty::Char)
                                | (Ty::Str, Ty::Str)
                        );

                        if ok {
                            Ok(Ty::Bool)
                        } else {
                            Err(SemanticError {
                                msg: format!("cannot compare {:?} == {:?}", lt, rt),
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
