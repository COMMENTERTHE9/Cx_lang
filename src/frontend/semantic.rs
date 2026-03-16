use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::semantic_types::*;

#[derive(Debug, Clone)]
pub struct SemanticError {
    pub msg: String,
    pub pos: usize,
}

#[derive(Debug, Clone)]
struct VarInfo {
    binding: BindingId,
    declared: Option<Type>,
    inferred: Option<SemanticType>,
    initialized: bool,
}

#[derive(Debug, Clone)]
struct FunctionInfo {
    id: FunctionId,
    params: Vec<SemanticParam>,
    ret_ty: Option<SemanticType>,
    type_params: Vec<String>,
}

#[derive(Debug, Clone)]
struct EnumInfo {
    id: EnumId,
    variants: HashMap<String, EnumVariantId>,
    groups: Vec<(String, Vec<String>)>,
    super_groups: Vec<(String, Vec<(String, Vec<String>)>)>,
}

type Scope = HashMap<String, VarInfo>;

pub struct Analyzer {
    scopes: Vec<Scope>,
    current_ret_ty: Option<SemanticType>,
    in_function: bool,
    funcs: HashMap<String, FunctionInfo>,
    enums: HashMap<String, EnumInfo>,
    enum_defs: Vec<SemanticEnum>,
    next_binding_id: u32,
    next_function_id: u32,
    next_enum_id: u32,
    next_enum_variant_id: u32,
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            current_ret_ty: None,
            in_function: false,
            funcs: HashMap::new(),
            enums: HashMap::new(),
            enum_defs: Vec::new(),
            next_binding_id: 0,
            next_function_id: 0,
            next_enum_id: 0,
            next_enum_variant_id: 0,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn fresh_binding(&mut self) -> BindingId {
        let id = BindingId(self.next_binding_id);
        self.next_binding_id += 1;
        id
    }

    fn fresh_function(&mut self) -> FunctionId {
        let id = FunctionId(self.next_function_id);
        self.next_function_id += 1;
        id
    }

    fn fresh_enum(&mut self) -> EnumId {
        let id = EnumId(self.next_enum_id);
        self.next_enum_id += 1;
        id
    }

    fn fresh_enum_variant(&mut self) -> EnumVariantId {
        let id = EnumVariantId(self.next_enum_variant_id);
        self.next_enum_variant_id += 1;
        id
    }

    fn declare(
        &mut self,
        name: &str,
        declared: Option<Type>,
        inferred: Option<SemanticType>,
        initialized: bool,
        pos: usize,
    ) -> Result<BindingId, SemanticError> {
        let binding = self.fresh_binding();
        let scope = self.scopes.last_mut().unwrap();
        if scope.contains_key(name) {
            return Err(SemanticError {
                msg: format!("variable already declared in this scope: {}", name),
                pos,
            });
        }
        scope.insert(
            name.to_string(),
            VarInfo {
                binding,
                declared,
                inferred,
                initialized,
            },
        );
        Ok(binding)
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

    fn declare_enum(
        &mut self,
        name: &str,
        variants: &[String],
        groups: &[(String, Vec<String>)],
        super_groups: &[(String, Vec<(String, Vec<String>)>)],
    ) -> SemanticEnum {
        let enum_id = self.fresh_enum();
        let mut variant_ids = HashMap::new();
        let mut semantic_variants = Vec::new();
        let mut semantic_groups = Vec::new();
        let mut declared_variants = Vec::new();

        if !variants.is_empty() {
            declared_variants.extend(variants.iter().cloned().map(|variant| (variant, None)));
        }

        for (group_name, group_variants) in groups {
            for variant in group_variants {
                declared_variants.push((variant.clone(), Some(group_name.clone())));
            }
        }

        for (_super_group_name, sub_groups) in super_groups {
            for (sub_group_name, sub_group_variants) in sub_groups {
                for variant in sub_group_variants {
                    declared_variants.push((variant.clone(), Some(sub_group_name.clone())));
                }
            }
        }

        for (variant, group_name) in declared_variants {
            let variant_id = self.fresh_enum_variant();
            variant_ids.insert(variant.clone(), variant_id);
            semantic_variants.push(SemanticEnumVariant {
                id: variant_id,
                name: variant.clone(),
                enum_id,
                group: group_name,
            });
        }

        for (group_name, group_variants) in groups {
            let ids = group_variants
                .iter()
                .filter_map(|variant| variant_ids.get(variant).copied())
                .collect::<Vec<_>>();
            semantic_groups.push(SemanticEnumGroup {
                name: group_name.clone(),
                variants: ids,
            });
        }

        for (_super_group_name, sub_groups) in super_groups {
            for (sub_group_name, sub_group_variants) in sub_groups {
                let ids = sub_group_variants
                    .iter()
                    .filter_map(|variant| variant_ids.get(variant).copied())
                    .collect::<Vec<_>>();
                semantic_groups.push(SemanticEnumGroup {
                    name: sub_group_name.clone(),
                    variants: ids,
                });
            }
        }

        self.enums.insert(
            name.to_string(),
            EnumInfo {
                id: enum_id,
                variants: variant_ids,
                groups: groups.to_vec(),
                super_groups: super_groups.to_vec(),
            },
        );

        let semantic_enum = SemanticEnum {
            id: enum_id,
            name: name.to_string(),
            declared_ty: Type::Enum(name.to_string()),
            variants: semantic_variants,
            groups: semantic_groups,
        };
        self.enum_defs.push(semantic_enum.clone());
        semantic_enum
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> Result<SemanticStmt, SemanticError> {
        match stmt {
            Stmt::EnumDef {
                name,
                variants,
                groups,
                super_groups,
                pos,
            } => {
                let semantic_enum = self.declare_enum(name, variants, groups, super_groups);
                Ok(SemanticStmt::EnumDef {
                    enum_id: semantic_enum.id,
                    name: name.clone(),
                    variants: variants.clone(),
                    pos: *pos,
                })
            }
            Stmt::Decl { name, ty, pos } => {
                let binding = self.declare(name, ty.clone(), None, false, *pos)?;
                Ok(SemanticStmt::Decl {
                    binding,
                    name: name.clone(),
                    ty: ty.clone().map(semantic_type_from_decl),
                    pos: *pos,
                })
            }
            Stmt::Assign {
                target,
                expr,
                pos_eq,
            } => {
                let mut semantic_expr = self.analyze_expr(expr)?;
                if semantic_expr.ty == SemanticType::StrRef {
                    return Err(SemanticError {
                        msg: "cannot assign a StrRef to a variable — use an owned str instead"
                            .to_string(),
                        pos: *pos_eq,
                    });
                }
                let target = match target {
                    Expr::Ident(name, _) => {
                        let info = self.lookup_var_mut(name).ok_or_else(|| SemanticError {
                            msg: format!("use of undeclared variable '{}'", name),
                            pos: *pos_eq,
                        })?;

                        if let Some(declared) = &info.declared {
                            let expected = semantic_type_from_decl(declared.clone());
                            if !types_compatible(&expected, &semantic_expr.ty) {
                                return Err(type_mismatch_error(
                                    &expected,
                                    &semantic_expr.ty,
                                    *pos_eq,
                                ));
                            }
                            semantic_expr = insert_cast_if_needed(semantic_expr, &expected);
                        } else if let Some(expected) = &info.inferred {
                            if !types_compatible(expected, &semantic_expr.ty) {
                                return Err(type_mismatch_error(
                                    expected,
                                    &semantic_expr.ty,
                                    *pos_eq,
                                ));
                            }
                            if is_numeric(expected) && is_numeric(&semantic_expr.ty) {
                                info.inferred = Some(semantic_expr.ty.clone());
                            } else {
                                semantic_expr = insert_cast_if_needed(semantic_expr, expected);
                            }
                        } else {
                            info.inferred = Some(semantic_expr.ty.clone());
                        }

                        info.initialized = true;
                        SemanticLValue::Binding {
                            binding: info.binding,
                            name: name.clone(),
                            ty: binding_type(info),
                        }
                    }
                    Expr::DotAccess(container, field) => {
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
                        SemanticLValue::DotAccess {
                            binding: Some(info.binding),
                            container: container.clone(),
                            field: field.clone(),
                            ty: SemanticType::Numeric,
                        }
                    }
                    Expr::Index(base_expr, idx_expr, pos) => {
                        let arr_name = match base_expr.as_ref() {
                            Expr::Ident(name, _) => name.clone(),
                            _ => {
                                return Err(SemanticError {
                                    msg: "bad assignment target".to_string(),
                                    pos: *pos,
                                });
                            }
                        };
                        let (binding, initialized) = {
                            let info =
                                self.lookup_var(&arr_name).ok_or_else(|| SemanticError {
                                    msg: format!(
                                        "use of undeclared variable '{}'",
                                        arr_name
                                    ),
                                    pos: *pos,
                                })?;
                            (info.binding, info.initialized)
                        };
                        if !initialized {
                            return Err(SemanticError {
                                msg: format!(
                                    "use of uninitialized variable '{}'",
                                    arr_name
                                ),
                                pos: *pos,
                            });
                        }
                        let sem_idx = self.analyze_expr(idx_expr)?;
                        SemanticLValue::IndexAccess {
                            binding,
                            name: arr_name,
                            index: Box::new(sem_idx),
                            ty: SemanticType::Numeric,
                        }
                    }
                    _ => {
                        return Err(SemanticError {
                            msg: "bad assignment target".to_string(),
                            pos: *pos_eq,
                        });
                    }
                };

                Ok(SemanticStmt::Assign {
                    target,
                    expr: semantic_expr,
                    pos_eq: *pos_eq,
                })
            }
            Stmt::TypedAssign {
                name,
                ty,
                expr,
                pos_type,
            } => {
                let declared_ty = semantic_type_from_decl(ty.clone());
                let mut semantic_expr = self.analyze_expr(expr)?;
                if semantic_expr.ty == SemanticType::StrRef {
                    return Err(SemanticError {
                        msg: "cannot assign a StrRef to a variable — use an owned str instead"
                            .to_string(),
                        pos: *pos_type,
                    });
                }

                if let Expr::Val(AstValue::Num(n)) = expr {
                    check_num_range(ty.clone(), *n, *pos_type)?;
                }

                if !types_compatible(&declared_ty, &semantic_expr.ty) {
                    return Err(type_mismatch_error(
                        &declared_ty,
                        &semantic_expr.ty,
                        *pos_type,
                    ));
                }

                semantic_expr = insert_cast_if_needed(semantic_expr, &declared_ty);
                let binding = self.declare(name, Some(ty.clone()), None, true, *pos_type)?;

                Ok(SemanticStmt::TypedAssign {
                    binding,
                    name: name.clone(),
                    ty: declared_ty,
                    expr: semantic_expr,
                    pos_type: *pos_type,
                })
            }
            Stmt::FuncDef {
                name,
                type_params,
                params,
                ret_ty,
                body,
                ret_expr,
                pos,
            } => self.analyze_function(name, type_params, params, ret_ty, body, ret_expr, *pos),
            Stmt::Print { expr, pos } => Ok(SemanticStmt::Print {
                expr: self.analyze_expr(expr)?,
                pos: *pos,
            }),
            Stmt::PrintInline { expr, _pos } => Ok(SemanticStmt::PrintInline {
                expr: self.analyze_expr(expr)?,
                pos: *_pos,
            }),
            Stmt::ExprStmt { expr, _pos } => Ok(SemanticStmt::ExprStmt {
                expr: self.analyze_expr(expr)?,
                pos: *_pos,
            }),
            Stmt::Return { expr, pos } => self.analyze_return(expr, *pos),
            Stmt::Block { stmts, _pos } => {
                self.push_scope();
                let semantic_stmts = stmts
                    .iter()
                    .map(|stmt| self.analyze_stmt(stmt))
                    .collect::<Result<Vec<_>, _>>()?;
                self.pop_scope();
                Ok(SemanticStmt::Block {
                    stmts: semantic_stmts,
                    pos: *_pos,
                })
            }
            Stmt::When { expr, arms, pos } => {
                let semantic_expr = self.analyze_expr(expr)?;
                let explicit_groups: Vec<String> = arms
                    .iter()
                    .filter_map(|a| {
                        if let WhenPattern::Group(_, name) = &a.pattern {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                let semantic_arms = arms
                    .iter()
                    .map(|arm| {
                        if let WhenPattern::Group(_, group_name) = &arm.pattern {
                            let found = self.enums.values().any(|info| {
                                info.groups.iter().any(|(g, _)| g == group_name)
                                    || info.super_groups.iter().any(|(sg, _)| sg == group_name)
                                    || info
                                        .super_groups
                                        .iter()
                                        .any(|(_, subs)| subs.iter().any(|(sub, _)| sub == group_name))
                            });
                            if !found {
                                return Err(SemanticError {
                                    msg: format!(
                                        "'{}' is not a known group or super-group name",
                                        group_name
                                    ),
                                    pos: *pos,
                                });
                            }
                        }

                        let body = match &arm.body {
                            WhenBody::Stmts(stmts) => stmts
                                .iter()
                                .map(|stmt| self.analyze_stmt(stmt))
                                .collect::<Result<Vec<_>, _>>()?,
                            WhenBody::SuperGroup(handlers) => {
                                let super_name = if let WhenPattern::Group(_, name) = &arm.pattern {
                                    name.clone()
                                } else {
                                    return Err(SemanticError {
                                        msg: "super-group handler list is only valid on a group pattern arm".to_string(),
                                        pos: *pos,
                                    });
                                };

                                let sub_groups: Vec<String> = self
                                    .enums
                                    .values()
                                    .find_map(|info| {
                                        info.super_groups
                                            .iter()
                                            .find(|(sg, _)| sg == &super_name)
                                            .map(|(_, subs)| {
                                                subs.iter().map(|(s, _)| s.clone()).collect()
                                            })
                                    })
                                    .ok_or_else(|| SemanticError {
                                        msg: format!("'{}' is not a known super-group name", super_name),
                                        pos: *pos,
                                    })?;

                                if handlers.len() != sub_groups.len() {
                                    return Err(SemanticError {
                                        msg: format!(
                                            "super-group '{}' has {} sub-groups but {} handlers were provided",
                                            super_name,
                                            sub_groups.len(),
                                            handlers.len()
                                        ),
                                        pos: *pos,
                                    });
                                }

                                let mut semantic_stmts = Vec::new();
                                for (i, handler) in handlers.iter().enumerate() {
                                    match handler {
                                        SuperGroupHandler::Placeholder => {
                                            let sub_name = &sub_groups[i];
                                            if !explicit_groups.contains(sub_name) {
                                                return Err(SemanticError {
                                                    msg: format!(
                                                        "{{_}} at position {} covers sub-group '{}' but no explicit arm for '{}' exists in this when block",
                                                        i + 1,
                                                        sub_name,
                                                        sub_name
                                                    ),
                                                    pos: *pos,
                                                });
                                            }
                                        }
                                        SuperGroupHandler::Stmts(stmts) => {
                                            semantic_stmts.extend(
                                                stmts
                                                    .iter()
                                                    .map(|stmt| self.analyze_stmt(stmt))
                                                    .collect::<Result<Vec<_>, _>>()?,
                                            );
                                        }
                                    }
                                }
                                semantic_stmts
                            }
                        };
                        Ok(SemanticWhenArm {
                            pattern: self.analyze_when_pattern(&arm.pattern),
                            body,
                            pos: arm.pos,
                        })
                    })
                    .collect::<Result<Vec<_>, SemanticError>>()?;
                Ok(SemanticStmt::When {
                    expr: semantic_expr,
                    arms: semantic_arms,
                    pos: *pos,
                })
            }
            Stmt::StructDef {
                name,
                fields,
                pos,
            } => {
                let sem_fields: Vec<(String, SemanticType)> = fields
                    .iter()
                    .map(|(n, t)| (n.clone(), semantic_type_from_decl(t.clone())))
                    .collect();
                Ok(SemanticStmt::StructDef {
                    name: name.clone(),
                    fields: sem_fields,
                    pos: *pos,
                })
            }
            Stmt::IfElse {
                condition,
                then_body,
                else_ifs,
                else_body,
                pos,
            } => {
                let semantic_condition = self.analyze_expr(condition)?;
                if matches!(semantic_condition.ty, SemanticType::Unknown) {
                    return Err(SemanticError {
                        msg: "Unknown value cannot be used as an if condition — control-critical context".to_string(),
                        pos: *pos,
                    });
                }
                let semantic_then = then_body
                    .iter()
                    .map(|s| self.analyze_stmt(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut semantic_else_ifs = Vec::new();
                for (cond, body) in else_ifs {
                    let sem_cond = self.analyze_expr(cond)?;
                    let sem_body = body
                        .iter()
                        .map(|s| self.analyze_stmt(s))
                        .collect::<Result<Vec<_>, _>>()?;
                    semantic_else_ifs.push((sem_cond, sem_body));
                }
                let semantic_else = else_body
                    .as_ref()
                    .map(|body| {
                        body.iter()
                            .map(|s| self.analyze_stmt(s))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?;
                Ok(SemanticStmt::IfElse {
                    condition: semantic_condition,
                    then_body: semantic_then,
                    else_ifs: semantic_else_ifs,
                    else_body: semantic_else,
                    pos: *pos,
                })
            }
            Stmt::WhileIn {
                arr,
                start_slot,
                range_start,
                range_end,
                inclusive,
                body,
                then_chains,
                result,
                pos,
            } => {
                let sem_start = self.analyze_expr(range_start)?;
                let sem_end = self.analyze_expr(range_end)?;
                let sem_body = body
                    .iter()
                    .map(|s| self.analyze_stmt(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let sem_chains = then_chains
                    .iter()
                    .map(|chain| {
                        let cs = self.analyze_expr(&chain.range_start)?;
                        let ce = self.analyze_expr(&chain.range_end)?;
                        let cb = chain
                            .body
                            .iter()
                            .map(|s| self.analyze_stmt(s))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(SemanticWhileInChain {
                            arr: chain.arr.clone(),
                            start_slot: chain.start_slot,
                            range_start: cs,
                            range_end: ce,
                            inclusive: chain.inclusive,
                            body: cb,
                        })
                    })
                    .collect::<Result<Vec<_>, SemanticError>>()?;
                let sem_result = match result {
                    Some(e) => Some(self.analyze_expr(e)?),
                    None => None,
                };
                Ok(SemanticStmt::WhileIn {
                    arr: arr.clone(),
                    start_slot: *start_slot,
                    range_start: sem_start,
                    range_end: sem_end,
                    inclusive: *inclusive,
                    body: sem_body,
                    then_chains: sem_chains,
                    result: sem_result,
                    pos: *pos,
                })
            }
            Stmt::While { cond, body, pos } => {
                let semantic_cond = self.analyze_expr(cond)?;
                if matches!(semantic_cond.ty, SemanticType::Unknown) {
                    return Err(SemanticError {
                        msg: "Unknown value cannot be used as a loop condition â€” control-critical context".to_string(),
                        pos: *pos,
                    });
                }
                let semantic_body = body
                    .iter()
                    .map(|stmt| self.analyze_stmt(stmt))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(SemanticStmt::While {
                    cond: semantic_cond,
                    body: semantic_body,
                    pos: *pos,
                })
            }
            Stmt::For {
                var,
                start,
                end,
                inclusive,
                body,
                pos,
            } => self.analyze_for(var, start, end, *inclusive, body, *pos),
            Stmt::Loop { body, pos } => Ok(SemanticStmt::Loop {
                body: body
                    .iter()
                    .map(|stmt| self.analyze_stmt(stmt))
                    .collect::<Result<Vec<_>, _>>()?,
                pos: *pos,
            }),
            Stmt::Break { pos } => Ok(SemanticStmt::Break { pos: *pos }),
            Stmt::Continue { pos } => Ok(SemanticStmt::Continue { pos: *pos }),
            Stmt::CompoundAssign {
                name,
                op,
                operand,
                pos,
            } => Ok(SemanticStmt::CompoundAssign {
                binding: self.lookup_var(name).map(|info| info.binding),
                name: name.clone(),
                op: *op,
                operand: self.analyze_expr(operand)?,
                pos: *pos,
            }),
        }
    }

    fn analyze_function(
        &mut self,
        name: &str,
        type_params: &[String],
        params: &[ParamKind],
        ret_ty: &Option<Type>,
        body: &[Stmt],
        ret_expr: &Option<Expr>,
        pos: usize,
    ) -> Result<SemanticStmt, SemanticError> {
        let func_id = self.fresh_function();
        self.declare(name, ret_ty.clone(), None, true, pos)?;

        let placeholders = params
            .iter()
            .map(|param| semantic_param_placeholder(param))
            .collect::<Vec<_>>();

        self.funcs.insert(
            name.to_string(),
            FunctionInfo {
                id: func_id,
                params: placeholders.clone(),
                ret_ty: ret_ty.clone().map(semantic_type_from_decl),
                type_params: type_params.to_vec(),
            },
        );

        self.push_scope();
        let prev_ret_ty = self.current_ret_ty.clone();
        let prev_in_function = self.in_function;
        self.in_function = true;
        self.current_ret_ty = ret_ty.clone().map(semantic_type_from_decl);

        let mut resolved_params = Vec::with_capacity(params.len());
        for param in params {
            match param {
                ParamKind::Typed(param_name, param_ty) => {
                    let binding =
                        self.declare(param_name, Some(param_ty.clone()), None, true, pos)?;
                    resolved_params.push(SemanticParam {
                        binding,
                        name: param_name.clone(),
                        kind: SemanticParamKind::Typed,
                        ty: Some(semantic_type_from_decl(param_ty.clone())),
                    });
                }
                ParamKind::Copy(param_name) => {
                    let binding = self.declare(param_name, None, None, true, pos)?;
                    resolved_params.push(SemanticParam {
                        binding,
                        name: param_name.clone(),
                        kind: SemanticParamKind::Copy,
                        ty: None,
                    });
                }
                ParamKind::CopyFree(param_name) => {
                    let binding = self.declare(param_name, None, None, true, pos)?;
                    resolved_params.push(SemanticParam {
                        binding,
                        name: param_name.clone(),
                        kind: SemanticParamKind::CopyFree,
                        ty: None,
                    });
                }
                ParamKind::CopyInto(param_name, _) => {
                    let binding = self.declare(param_name, None, None, true, pos)?;
                    resolved_params.push(SemanticParam {
                        binding,
                        name: param_name.clone(),
                        kind: SemanticParamKind::CopyInto,
                        ty: None,
                    });
                }
            }
        }

        if let Some(info) = self.funcs.get_mut(name) {
            info.params = resolved_params.clone();
        }

        let semantic_body = body
            .iter()
            .map(|stmt| self.analyze_stmt(stmt))
            .collect::<Result<Vec<_>, _>>()?;

        let semantic_ret_expr = if let Some(expr) = ret_expr {
            let mut expr = self.analyze_expr(expr)?;
            if expr.ty == SemanticType::StrRef {
                return Err(SemanticError {
                    msg: "cannot return a StrRef — it does not outlive its origin scope"
                        .to_string(),
                    pos,
                });
            }
            if let Some(expected) = &self.current_ret_ty {
                if !types_compatible(expected, &expr.ty) {
                    return Err(SemanticError {
                        msg: format!(
                            "return type mismatch: expected {:?}, got {:?}",
                            classify_type(expected),
                            classify_type(&expr.ty)
                        ),
                        pos,
                    });
                }
                expr = insert_cast_if_needed(expr, expected);
            }
            Some(expr)
        } else {
            if ret_ty.is_some() && !contains_return_stmt(body) {
                return Err(SemanticError {
                    msg: format!(
                        "missing return value, expected {:?}",
                        classify_type(&semantic_type_from_decl(ret_ty.clone().unwrap()))
                    ),
                    pos,
                });
            }
            None
        };

        self.current_ret_ty = prev_ret_ty;
        self.in_function = prev_in_function;
        self.pop_scope();

        Ok(SemanticStmt::FuncDef(SemanticFunction {
            id: func_id,
            name: name.to_string(),
            type_params: type_params.to_vec(),
            params: resolved_params,
            return_ty: ret_ty.clone().map(semantic_type_from_decl),
            body: semantic_body,
            ret_expr: semantic_ret_expr,
            pos,
        }))
    }

    fn analyze_return(
        &mut self,
        expr: &Option<Expr>,
        pos: usize,
    ) -> Result<SemanticStmt, SemanticError> {
        if !self.in_function {
            return Err(SemanticError {
                msg: "return used outside function body".to_string(),
                pos,
            });
        }

        let expr = match (expr, self.current_ret_ty.clone()) {
            (Some(expr), Some(expected)) => {
                let expr = self.analyze_expr(expr)?;
                if expr.ty == SemanticType::StrRef {
                    return Err(SemanticError {
                        msg: "cannot return a StrRef — it does not outlive its origin scope"
                            .to_string(),
                        pos,
                    });
                }
                if !types_compatible(&expected, &expr.ty) {
                    return Err(SemanticError {
                        msg: format!(
                            "return type mismatch: expected {:?}, got {:?}",
                            classify_type(&expected),
                            classify_type(&expr.ty)
                        ),
                        pos,
                    });
                }
                Some(insert_cast_if_needed(expr, &expected))
            }
            (None, None) => None,
            (None, Some(expected)) => {
                return Err(SemanticError {
                    msg: format!(
                        "missing return value, expected {:?}",
                        classify_type(&expected)
                    ),
                    pos,
                });
            }
            (Some(expr), None) => {
                self.analyze_expr(expr)?;
                return Err(SemanticError {
                    msg: "unexpected return value in void function".to_string(),
                    pos,
                });
            }
        };

        Ok(SemanticStmt::Return { expr, pos })
    }

    fn analyze_for(
        &mut self,
        var: &str,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        body: &[Stmt],
        pos: usize,
    ) -> Result<SemanticStmt, SemanticError> {
        self.push_scope();
        let binding = self.declare(var, Some(Type::T64), Some(SemanticType::I64), true, pos)?;

        let mut semantic_body = Vec::with_capacity(body.len());
        for stmt in body {
            match stmt {
                Stmt::Assign {
                    target: Expr::Ident(name, _),
                    ..
                } if name == var => {
                    self.pop_scope();
                    return Err(SemanticError {
                        msg: format!("loop variable '{}' is read-only", var),
                        pos,
                    });
                }
                Stmt::CompoundAssign { name, .. } if name == var => {
                    self.pop_scope();
                    return Err(SemanticError {
                        msg: format!("loop variable '{}' is read-only", var),
                        pos,
                    });
                }
                _ => semantic_body.push(self.analyze_stmt(stmt)?),
            }
        }
        self.pop_scope();

        Ok(SemanticStmt::For {
            binding,
            var: var.to_string(),
            start: self.analyze_expr(start)?,
            end: self.analyze_expr(end)?,
            inclusive,
            body: semantic_body,
            pos,
        })
    }

    fn analyze_when_pattern(&self, pattern: &WhenPattern) -> SemanticWhenPattern {
        match pattern {
            WhenPattern::Literal(value) => {
                SemanticWhenPattern::Literal(semantic_value_from_ast(value, &self.enums))
            }
            WhenPattern::Range(start, end, inclusive) => SemanticWhenPattern::Range(
                semantic_value_from_ast(start, &self.enums),
                semantic_value_from_ast(end, &self.enums),
                *inclusive,
            ),
            WhenPattern::EnumVariant(enum_name, variant_name) => {
                let enum_info = self.enums.get(enum_name);
                let variant_id =
                    enum_info.and_then(|info| info.variants.get(variant_name).copied());
                SemanticWhenPattern::EnumVariant {
                    enum_name: enum_name.clone(),
                    variant_name: variant_name.clone(),
                    enum_id: enum_info.map(|info| info.id),
                    variant_id,
                }
            }
            WhenPattern::Group(_, _) => SemanticWhenPattern::Catchall,
            WhenPattern::Catchall => SemanticWhenPattern::Catchall,
            WhenPattern::Placeholder => SemanticWhenPattern::Catchall,
        }
    }

    fn analyze_expr(&mut self, expr: &Expr) -> Result<SemanticExpr, SemanticError> {
        match expr {
            Expr::Val(value) => Ok(SemanticExpr {
                ty: semantic_type_from_value(value),
                kind: SemanticExprKind::Value(semantic_value_from_ast(value, &self.enums)),
            }),
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
                Ok(SemanticExpr {
                    ty: binding_type(info),
                    kind: SemanticExprKind::VarRef {
                        binding: info.binding,
                        name: name.clone(),
                    },
                })
            }
            Expr::DotAccess(container, field) => {
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
                Ok(SemanticExpr {
                    ty: SemanticType::I128,
                    kind: SemanticExprKind::DotAccess {
                        binding: Some(info.binding),
                        container: container.clone(),
                        field: field.clone(),
                    },
                })
            }
            Expr::HandleNew(inner, pos) => {
                let inner_analyzed = self.analyze_expr(inner)?;
                if inner_analyzed.ty == SemanticType::StrRef {
                    return Err(SemanticError {
                        msg: "cannot store a StrRef in a Handle — use an owned str instead"
                            .to_string(),
                        pos: *pos,
                    });
                }
                Ok(SemanticExpr {
                    ty: SemanticType::Handle(Box::new(SemanticType::I128)),
                    kind: SemanticExprKind::HandleNew {
                        value: Box::new(inner_analyzed),
                        pos: *pos,
                    },
                })
            }
            Expr::HandleVal(name, pos) => {
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
                Ok(SemanticExpr {
                    ty: SemanticType::I128,
                    kind: SemanticExprKind::HandleVal {
                        binding: info.binding,
                        name: name.clone(),
                        pos: *pos,
                    },
                })
            }
            Expr::HandleDrop(name, pos) => {
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
                Ok(SemanticExpr {
                    ty: SemanticType::Handle(Box::new(SemanticType::I128)),
                    kind: SemanticExprKind::HandleDrop {
                        binding: info.binding,
                        name: name.clone(),
                        pos: *pos,
                    },
                })
            }
            Expr::Call(name, args, pos) => self.analyze_call(name, args, *pos),
            Expr::Range(start, end, inclusive) => Ok(SemanticExpr {
                ty: SemanticType::I128,
                kind: SemanticExprKind::Range {
                    start: Box::new(self.analyze_expr(start)?),
                    end: Box::new(self.analyze_expr(end)?),
                    inclusive: *inclusive,
                },
            }),
            Expr::Unary(op, inner, pos) => {
                let expr = self.analyze_expr(inner)?;
                Ok(SemanticExpr {
                    ty: expr.ty.clone(),
                    kind: SemanticExprKind::Unary {
                        op: *op,
                        expr: Box::new(expr),
                        pos: *pos,
                    },
                })
            }
            Expr::Bin(lhs, op, op_pos, rhs) => self.analyze_binary(lhs, *op, *op_pos, rhs),
            Expr::ArrayLit(elems) => {
                for e in elems {
                    self.analyze_expr(e)?;
                }
                Ok(SemanticExpr {
                    ty: SemanticType::Unknown,
                    kind: SemanticExprKind::Value(SemanticValue::Unknown),
                })
            }
            Expr::Index(base, idx, pos) => {
                self.analyze_expr(base)?;
                self.analyze_expr(idx)?;
                Ok(SemanticExpr {
                    ty: SemanticType::Unknown,
                    kind: SemanticExprKind::VarRef {
                        binding: BindingId(0),
                        name: format!("index@{}", pos),
                    },
                })
            }
        }
    }

    fn analyze_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        pos: usize,
    ) -> Result<SemanticExpr, SemanticError> {
        if name == "is_known" {
            let expr = match args.first() {
                Some(CallArg::Expr(expr)) => self.analyze_expr(expr)?,
                _ => {
                    return Err(SemanticError {
                        msg: format!("call to undefined function '{}'", name),
                        pos,
                    });
                }
            };
            return Ok(SemanticExpr {
                ty: SemanticType::Bool,
                kind: SemanticExprKind::Call {
                    callee: name.to_string(),
                    function: FunctionId(u32::MAX),
                    args: vec![SemanticCallArg::Expr(expr)],
                },
            });
        }

        let function = self.funcs.get(name).cloned().ok_or_else(|| SemanticError {
            msg: format!("call to undefined function '{}'", name),
            pos,
        })?;

        if args.len() != function.params.len() {
            return Err(SemanticError {
                msg: format!(
                    "function '{}' expects {} argument(s), got {}",
                    name,
                    function.params.len(),
                    args.len()
                ),
                pos,
            });
        }

        // Resolve type parameters from typed arguments
        let mut type_param_map: std::collections::HashMap<String, SemanticType> = std::collections::HashMap::new();
        if !function.type_params.is_empty() {
            for (index, arg) in args.iter().enumerate() {
                if let Some(param) = function.params.get(index) {
                    if let Some(SemanticType::TypeParam(tname)) = &param.ty {
                        if let CallArg::Expr(expr) = arg {
                            let analyzed = self.analyze_expr(expr)?;
                            if let Some(existing) = type_param_map.get(tname) {
                                if !types_compatible(existing, &analyzed.ty) {
                                    return Err(SemanticError {
                                        msg: format!(
                                            "type parameter '{}' is bound to {:?} but argument {} has {:?}",
                                            tname,
                                            classify_type(existing),
                                            index + 1,
                                            classify_type(&analyzed.ty)
                                        ),
                                        pos,
                                    });
                                }
                            } else {
                                type_param_map.insert(tname.clone(), analyzed.ty.clone());
                            }
                        }
                    }
                }
            }
        }

        let mut semantic_args = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let expected = function
                .params
                .get(index)
                .and_then(|param| param.ty.clone())
                .map(|ty| match ty {
                    SemanticType::TypeParam(ref tname) => {
                        type_param_map.get(tname).cloned().unwrap_or(ty)
                    }
                    other => other,
                });
            match arg {
                CallArg::Expr(expr) => {
                    let expr = self.analyze_expr(expr)?;
                    let expr = if let Some(expected) = expected {
                        if !types_compatible(&expected, &expr.ty) {
                            return Err(SemanticError {
                                msg: format!(
                                    "argument {} to '{}': expected {:?}, got {:?}",
                                    index + 1,
                                    name,
                                    classify_type(&expected),
                                    classify_type(&expr.ty)
                                ),
                                pos,
                            });
                        }
                        insert_cast_if_needed(expr, &expected)
                    } else {
                        expr
                    };
                    semantic_args.push(SemanticCallArg::Expr(expr));
                }
                CallArg::Copy(outer_name) => {
                    let info = self.lookup_var(outer_name).ok_or_else(|| SemanticError {
                        msg: format!("'.copy' argument '{}' has not been declared", outer_name),
                        pos,
                    })?;
                    if !info.initialized {
                        return Err(SemanticError {
                            msg: format!("'.copy' argument '{}' is not initialized", outer_name),
                            pos,
                        });
                    }
                    semantic_args.push(SemanticCallArg::Copy {
                        binding: info.binding,
                        name: outer_name.clone(),
                    });
                }
                CallArg::CopyFree(outer_name) => {
                    let info = self.lookup_var(outer_name).ok_or_else(|| SemanticError {
                        msg: format!("'.copy' argument '{}' has not been declared", outer_name),
                        pos,
                    })?;
                    if !info.initialized {
                        return Err(SemanticError {
                            msg: format!("'.copy' argument '{}' is not initialized", outer_name),
                            pos,
                        });
                    }
                    semantic_args.push(SemanticCallArg::CopyFree {
                        binding: info.binding,
                        name: outer_name.clone(),
                    });
                }
                CallArg::CopyInto(outer_names) => {
                    let mut resolved = Vec::with_capacity(outer_names.len());
                    for outer_name in outer_names {
                        let info = self.lookup_var(outer_name).ok_or_else(|| SemanticError {
                            msg: format!(
                                "copy_into variable '{}' has not been declared",
                                outer_name
                            ),
                            pos,
                        })?;
                        if !info.initialized {
                            return Err(SemanticError {
                                msg: format!(
                                    "copy_into variable '{}' is not initialized",
                                    outer_name
                                ),
                                pos,
                            });
                        }
                        resolved.push(ResolvedBinding {
                            binding: info.binding,
                            name: outer_name.clone(),
                        });
                    }
                    semantic_args.push(SemanticCallArg::CopyInto(resolved));
                }
            }
        }

        Ok(SemanticExpr {
            ty: function.ret_ty.unwrap_or(SemanticType::I128),
            kind: SemanticExprKind::Call {
                callee: name.to_string(),
                function: function.id,
                args: semantic_args,
            },
        })
    }

    fn analyze_binary(
        &mut self,
        lhs: &Expr,
        op: Op,
        op_pos: usize,
        rhs: &Expr,
    ) -> Result<SemanticExpr, SemanticError> {
        let mut lhs = self.analyze_expr(lhs)?;
        let mut rhs = self.analyze_expr(rhs)?;

        match op {
            Op::Plus | Op::Minus | Op::Mul | Op::Div | Op::Mod => {
                if lhs.ty == SemanticType::Unknown || rhs.ty == SemanticType::Unknown {
                    return Ok(SemanticExpr {
                        ty: SemanticType::Unknown,
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    });
                }
                if !is_numeric(&lhs.ty) || !is_numeric(&rhs.ty) {
                    return Err(SemanticError {
                        msg: format!(
                            "arithmetic requires numeric operands, got {:?} and {:?}",
                            classify_type(&lhs.ty),
                            classify_type(&rhs.ty)
                        ),
                        pos: op_pos,
                    });
                }

                let result_ty = common_numeric_type(&lhs.ty, &rhs.ty);
                lhs = insert_cast_if_needed(lhs, &result_ty);
                rhs = insert_cast_if_needed(rhs, &result_ty);
                Ok(SemanticExpr {
                    ty: result_ty,
                    kind: SemanticExprKind::Binary {
                        lhs: Box::new(lhs),
                        op,
                        pos: op_pos,
                        rhs: Box::new(rhs),
                    },
                })
            }
            Op::EqEq => {
                if lhs.ty == SemanticType::Unknown || rhs.ty == SemanticType::Unknown {
                    return Ok(SemanticExpr {
                        ty: SemanticType::Unknown,
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    });
                }

                if is_numeric(&lhs.ty) && is_numeric(&rhs.ty) {
                    let compare_ty = common_numeric_type(&lhs.ty, &rhs.ty);
                    lhs = insert_cast_if_needed(lhs, &compare_ty);
                    rhs = insert_cast_if_needed(rhs, &compare_ty);
                    return Ok(SemanticExpr {
                        ty: SemanticType::Bool,
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    });
                }

                if lhs.ty == rhs.ty
                    && matches!(
                        lhs.ty,
                        SemanticType::Bool
                            | SemanticType::Char
                            | SemanticType::Str
                            | SemanticType::StrRef
                            | SemanticType::Enum(_)
                    )
                {
                    return Ok(SemanticExpr {
                        ty: SemanticType::Bool,
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    });
                }

                Err(SemanticError {
                    msg: format!(
                        "cannot compare {:?} == {:?}",
                        classify_type(&lhs.ty),
                        classify_type(&rhs.ty)
                    ),
                    pos: op_pos,
                })
            }
            Op::Lt | Op::Gt | Op::LtEq | Op::GtEq => {
                if lhs.ty == SemanticType::Unknown || rhs.ty == SemanticType::Unknown {
                    Ok(SemanticExpr {
                        ty: SemanticType::Unknown,
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    })
                } else {
                    Ok(SemanticExpr {
                        ty: SemanticType::Bool,
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    })
                }
            }
            Op::And | Op::Or => {
                if matches!(lhs.ty, SemanticType::Bool | SemanticType::Unknown)
                    && matches!(rhs.ty, SemanticType::Bool | SemanticType::Unknown)
                {
                    Ok(SemanticExpr {
                        ty: if lhs.ty == SemanticType::Unknown || rhs.ty == SemanticType::Unknown {
                            SemanticType::Unknown
                        } else {
                            SemanticType::Bool
                        },
                        kind: SemanticExprKind::Binary {
                            lhs: Box::new(lhs),
                            op,
                            pos: op_pos,
                            rhs: Box::new(rhs),
                        },
                    })
                } else {
                    Err(SemanticError {
                        msg: format!(
                            "logical operation requires bool operands, got {:?} and {:?}",
                            classify_type(&lhs.ty),
                            classify_type(&rhs.ty)
                        ),
                        pos: op_pos,
                    })
                }
            }
        }
    }
}

fn type_mismatch_error(expected: &SemanticType, got: &SemanticType, pos: usize) -> SemanticError {
    SemanticError {
        msg: format!(
            "type mismatch: expected {:?}, got {:?}",
            classify_type(expected),
            classify_type(got)
        ),
        pos,
    }
}

fn semantic_param_placeholder(param: &ParamKind) -> SemanticParam {
    match param {
        ParamKind::Typed(name, ty) => SemanticParam {
            binding: BindingId(u32::MAX),
            name: name.clone(),
            kind: SemanticParamKind::Typed,
            ty: Some(semantic_type_from_decl(ty.clone())),
        },
        ParamKind::Copy(name) => SemanticParam {
            binding: BindingId(u32::MAX),
            name: name.clone(),
            kind: SemanticParamKind::Copy,
            ty: None,
        },
        ParamKind::CopyFree(name) => SemanticParam {
            binding: BindingId(u32::MAX),
            name: name.clone(),
            kind: SemanticParamKind::CopyFree,
            ty: None,
        },
        ParamKind::CopyInto(name, _) => SemanticParam {
            binding: BindingId(u32::MAX),
            name: name.clone(),
            kind: SemanticParamKind::CopyInto,
            ty: None,
        },
    }
}

fn check_num_range(ty: Type, n: i128, pos: usize) -> Result<(), SemanticError> {
    let bounds: Option<(i128, i128)> = match ty {
        Type::T8 => Some((i8::MIN as i128, i8::MAX as i128)),
        Type::T16 => Some((i16::MIN as i128, i16::MAX as i128)),
        Type::T32 => Some((i32::MIN as i128, i32::MAX as i128)),
        Type::T64 => Some((i64::MIN as i128, i64::MAX as i128)),
        Type::T128 => None,
        _ => None,
    };

    if let Some((min, max)) = bounds {
        if n < min || n > max {
            return Err(SemanticError {
                msg: format!("value {} overflows type {:?} (range {}..{})", n, ty, min, max),
                pos,
            });
        }
    }
    Ok(())
}

fn semantic_type_from_decl(ty: Type) -> SemanticType {
    match ty {
        Type::T8 => SemanticType::I8,
        Type::T16 => SemanticType::I16,
        Type::T32 => SemanticType::I32,
        Type::T64 => SemanticType::I64,
        Type::T128 => SemanticType::I128,
        Type::Bool => SemanticType::Bool,
        Type::Str => SemanticType::Str,
        Type::StrRef => SemanticType::StrRef,
        Type::Container => SemanticType::Container,
        Type::Char => SemanticType::Char,
        Type::Enum(name) => SemanticType::Enum(name),
        Type::Unknown => SemanticType::Unknown,
        Type::Handle(inner) => SemanticType::Handle(Box::new(semantic_type_from_decl(*inner))),
        Type::Array(_, _) => SemanticType::Unknown,
        Type::TypeParam(s) => SemanticType::TypeParam(s),
    }
}

fn semantic_type_from_value(value: &AstValue) -> SemanticType {
    match value {
        AstValue::Num(_) => SemanticType::I128,
        AstValue::Float(_) => SemanticType::F64,
        AstValue::Str(_) => SemanticType::Str,
        AstValue::Bool(_) => SemanticType::Bool,
        AstValue::Char(_) => SemanticType::Char,
        AstValue::EnumVariant(enum_name, _) => SemanticType::Enum(enum_name.clone()),
        AstValue::Unknown => SemanticType::Unknown,
    }
}

fn semantic_value_from_ast(value: &AstValue, enums: &HashMap<String, EnumInfo>) -> SemanticValue {
    match value {
        AstValue::Num(n) => SemanticValue::Num(*n),
        AstValue::Float(f) => SemanticValue::Float(*f),
        AstValue::Str(s) => SemanticValue::Str(s.clone()),
        AstValue::Bool(b) => SemanticValue::Bool(*b),
        AstValue::Char(c) => SemanticValue::Char(*c),
        AstValue::EnumVariant(enum_name, variant_name) => {
            let enum_info = enums.get(enum_name);
            let variant_id = enum_info.and_then(|info| info.variants.get(variant_name).copied());
            SemanticValue::EnumVariant {
                enum_name: enum_name.clone(),
                variant_name: variant_name.clone(),
                enum_id: enum_info.map(|info| info.id),
                variant_id,
            }
        }
        AstValue::Unknown => SemanticValue::Unknown,
    }
}

fn classify_type(ty: &SemanticType) -> SemanticType {
    match ty {
        SemanticType::I8
        | SemanticType::I16
        | SemanticType::I32
        | SemanticType::I64
        | SemanticType::I128
        | SemanticType::F64
        | SemanticType::Numeric => SemanticType::Numeric,
        other => other.clone(),
    }
}

fn binding_type(info: &VarInfo) -> SemanticType {
    info.declared
        .clone()
        .map(semantic_type_from_decl)
        .or_else(|| info.inferred.clone())
        .unwrap_or(SemanticType::Numeric)
}

fn is_numeric(ty: &SemanticType) -> bool {
    matches!(
        ty,
        SemanticType::I8
            | SemanticType::I16
            | SemanticType::I32
            | SemanticType::I64
            | SemanticType::I128
            | SemanticType::F64
            | SemanticType::Numeric
    )
}

fn types_compatible(expected: &SemanticType, got: &SemanticType) -> bool {
    if expected == got || *got == SemanticType::Unknown {
        return true;
    }
    if matches!(expected, SemanticType::TypeParam(_)) || matches!(got, SemanticType::TypeParam(_)) {
        return true;
    }
    match (expected, got) {
        (SemanticType::Numeric, other) | (other, SemanticType::Numeric) => is_numeric(other),
        _ => is_numeric(expected) && is_numeric(got),
    }
}

fn common_numeric_type(lhs: &SemanticType, rhs: &SemanticType) -> SemanticType {
    if matches!(lhs, SemanticType::Numeric) || matches!(rhs, SemanticType::Numeric) {
        SemanticType::Numeric
    } else if matches!(lhs, SemanticType::F64) || matches!(rhs, SemanticType::F64) {
        SemanticType::F64
    } else {
        SemanticType::I128
    }
}

fn insert_cast_if_needed(expr: SemanticExpr, target: &SemanticType) -> SemanticExpr {
    if &expr.ty == target {
        return expr;
    }

    if !is_numeric(&expr.ty) || !is_numeric(target) {
        return expr;
    }

    SemanticExpr {
        ty: target.clone(),
        kind: SemanticExprKind::Cast {
            from: expr.ty.clone(),
            to: target.clone(),
            expr: Box::new(expr),
        },
    }
}

fn contains_return_stmt(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_return)
}

fn stmt_contains_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::Block { stmts, .. } => contains_return_stmt(stmts),
        Stmt::FuncDef { .. } => false,
        _ => false,
    }
}

pub fn analyze_program(program: &Program) -> Result<SemanticProgram, Vec<SemanticError>> {
    let mut analyzer = Analyzer::new();
    let mut errors = Vec::new();
    let mut semantic_stmts = Vec::with_capacity(program.stmts.len());

    for stmt in &program.stmts {
        if let Stmt::FuncDef {
            name,
            type_params,
            params,
            ret_ty,
            ..
        } = stmt
        {
            let placeholders = params
                .iter()
                .map(semantic_param_placeholder)
                .collect::<Vec<_>>();
            let func_id = analyzer.fresh_function();
            analyzer.funcs.insert(
                name.clone(),
                FunctionInfo {
                    id: func_id,
                    params: placeholders,
                    ret_ty: ret_ty.clone().map(semantic_type_from_decl),
                    type_params: type_params.clone(),
                },
            );
        }
    }
    for stmt in &program.stmts {
        match analyzer.analyze_stmt(stmt) {
            Ok(stmt) => semantic_stmts.push(stmt),
            Err(err) => errors.push(err),
        }
    }

    if errors.is_empty() {
        Ok(SemanticProgram {
            stmts: semantic_stmts,
            enums: analyzer.enum_defs,
        })
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str) -> Expr {
        Expr::Ident(name.to_string(), 0)
    }

    fn num(n: i128) -> Expr {
        Expr::Val(AstValue::Num(n))
    }

    fn float(n: f64) -> Expr {
        Expr::Val(AstValue::Float(n))
    }

    #[test]
    fn returns_semantic_program_on_success() {
        let program = Program {
            stmts: vec![
                Stmt::Decl {
                    name: "x".to_string(),
                    ty: None,
                    pos: 0,
                },
                Stmt::Assign {
                    target: ident("x"),
                    expr: num(1),
                    pos_eq: 0,
                },
            ],
        };

        let semantic = analyze_program(&program).expect("semantic analysis should succeed");
        assert_eq!(semantic.stmts.len(), 2);
    }

    #[test]
    fn resolves_variable_references_to_declarations() {
        let program = Program {
            stmts: vec![
                Stmt::Decl {
                    name: "x".to_string(),
                    ty: None,
                    pos: 0,
                },
                Stmt::Assign {
                    target: ident("x"),
                    expr: num(1),
                    pos_eq: 0,
                },
                Stmt::ExprStmt {
                    expr: ident("x"),
                    _pos: 0,
                },
            ],
        };

        let semantic = analyze_program(&program).unwrap();
        let decl_binding = match &semantic.stmts[0] {
            SemanticStmt::Decl { binding, .. } => *binding,
            other => panic!("unexpected stmt: {:?}", other),
        };
        match &semantic.stmts[2] {
            SemanticStmt::ExprStmt {
                expr:
                    SemanticExpr {
                        kind: SemanticExprKind::VarRef { binding, .. },
                        ..
                    },
                ..
            } => assert_eq!(*binding, decl_binding),
            other => panic!("unexpected stmt: {:?}", other),
        }
    }

    #[test]
    fn resolves_function_call_targets() {
        let program = Program {
            stmts: vec![
                Stmt::FuncDef {
                    name: "foo".to_string(),
                    type_params: vec![],
                    params: vec![ParamKind::Typed("a".to_string(), Type::T64)],
                    ret_ty: Some(Type::T64),
                    body: vec![],
                    ret_expr: Some(ident("a")),
                    pos: 0,
                },
                Stmt::ExprStmt {
                    expr: Expr::Call("foo".to_string(), vec![CallArg::Expr(num(1))], 0),
                    _pos: 0,
                },
            ],
        };

        let semantic = analyze_program(&program).unwrap();
        let function_id = match &semantic.stmts[0] {
            SemanticStmt::FuncDef(func) => func.id,
            other => panic!("unexpected stmt: {:?}", other),
        };
        match &semantic.stmts[1] {
            SemanticStmt::ExprStmt {
                expr:
                    SemanticExpr {
                        kind: SemanticExprKind::Call { function, .. },
                        ..
                    },
                ..
            } => assert_eq!(*function, function_id),
            other => panic!("unexpected stmt: {:?}", other),
        }
    }

    #[test]
    fn expressions_carry_resolved_types() {
        let program = Program {
            stmts: vec![Stmt::ExprStmt {
                expr: Expr::Bin(Box::new(num(1)), Op::Plus, 0, Box::new(float(2.5))),
                _pos: 0,
            }],
        };

        let semantic = analyze_program(&program).unwrap();
        match &semantic.stmts[0] {
            SemanticStmt::ExprStmt { expr, .. } => assert_eq!(expr.ty, SemanticType::F64),
            other => panic!("unexpected stmt: {:?}", other),
        }
    }

    #[test]
    fn inserts_explicit_casts_for_typed_numeric_assignment() {
        let program = Program {
            stmts: vec![Stmt::TypedAssign {
                name: "x".to_string(),
                ty: Type::T64,
                expr: num(1),
                pos_type: 0,
            }],
        };

        let semantic = analyze_program(&program).unwrap();
        match &semantic.stmts[0] {
            SemanticStmt::TypedAssign { expr, .. } => match &expr.kind {
                SemanticExprKind::Cast { to, .. } => assert_eq!(*to, SemanticType::I64),
                other => panic!("expected cast, got {:?}", other),
            },
            other => panic!("unexpected stmt: {:?}", other),
        }
    }

    #[test]
    fn populates_enum_registry_for_declared_enums() {
        let program = Program {
            stmts: vec![Stmt::EnumDef {
                name: "Color".to_string(),
                variants: vec!["Red".to_string(), "Blue".to_string()],
                groups: vec![],
                super_groups: vec![],
                pos: 0,
            }],
        };

        let semantic = analyze_program(&program).unwrap();
        assert_eq!(semantic.enums.len(), 1);
        assert_eq!(semantic.enums[0].name, "Color");
        assert_eq!(semantic.enums[0].variants.len(), 2);
        assert!(semantic.enums[0].groups.is_empty());
    }

    #[test]
    fn accumulates_semantic_errors() {
        let program = Program {
            stmts: vec![
                Stmt::ExprStmt {
                    expr: ident("missing"),
                    _pos: 3,
                },
                Stmt::Decl {
                    name: "x".to_string(),
                    ty: None,
                    pos: 10,
                },
                Stmt::Decl {
                    name: "x".to_string(),
                    ty: None,
                    pos: 11,
                },
            ],
        };

        let errors = analyze_program(&program).expect_err("analysis should fail");
        assert_eq!(errors.len(), 2);
    }
}
