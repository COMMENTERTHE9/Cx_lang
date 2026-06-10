use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::builtins::{self, BuiltinKind, BuiltinRet};
use crate::frontend::semantic_types::*;

#[derive(Debug, Clone)]
pub struct SemanticError {
    pub msg: String,
    pub pos: usize,
}

macro_rules! sem_err {
    ($pos:expr, $msg:expr) => {
        SemanticError { msg: $msg.to_string(), pos: $pos }
    };
    ($pos:expr, $fmt:literal, $($arg:expr),+) => {
        SemanticError { msg: format!($fmt, $($arg),+), pos: $pos }
    };
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
    current_type_params: Vec<String>,
    in_function: bool,
    funcs: HashMap<String, FunctionInfo>,
    enums: HashMap<String, EnumInfo>,
    structs: HashMap<String, Vec<(String, Type)>>,
    pub method_registry: HashMap<(String, String), SemanticFunction>,
    pub method_alias_counts: HashMap<(String, String), usize>,
    pub struct_type_params: HashMap<String, Vec<String>>,
    enum_defs: Vec<SemanticEnum>,
    pub module_aliases: HashMap<String, ExportTable>,
    next_binding_id: u32,
    next_function_id: u32,
    next_enum_id: u32,
    next_enum_variant_id: u32,
    /// Declared array element type for the array literal currently being
    /// analyzed, when one is known from context (e.g. `let a: [N: T] = [...]`).
    /// Set by the typed-assignment arm before analyzing the initializer and
    /// consumed (taken) by the ArrayLit arm so element-mismatch errors report
    /// the DECLARED element type rather than element[0]'s inferred type (#031a).
    expected_array_elem: Option<SemanticType>,
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            current_ret_ty: None,
            current_type_params: vec![],
            in_function: false,
            funcs: HashMap::new(),
            enums: HashMap::new(),
            structs: HashMap::new(),
            method_registry: HashMap::new(),
            method_alias_counts: HashMap::new(),
            struct_type_params: HashMap::new(),
            enum_defs: Vec::new(),
            module_aliases: HashMap::new(),
            next_binding_id: 0,
            next_function_id: 0,
            next_enum_id: 0,
            next_enum_variant_id: 0,
            expected_array_elem: None,
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
            return Err(sem_err!(pos, "variable already declared in this scope: {}", name));
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
            Stmt::ConstDecl { name, ty, value, is_pub, pos } => {
                let semantic_value = self.analyze_expr(value)?;
                let sem_ty = semantic_type_from_decl(ty.clone(), &self.current_type_params);
                let binding = self.declare(name, Some(ty.clone()), Some(sem_ty.clone()), true, *pos)?;
                Ok(SemanticStmt::ConstDecl {
                    binding,
                    name: name.clone(),
                    ty: sem_ty,
                    value: semantic_value,
                    is_pub: *is_pub,
                    pos: *pos,
                })
            }
            Stmt::StructDef { name, type_params, fields, pos, .. } => {
                self.structs.insert(name.clone(), fields.clone());
                self.struct_type_params.insert(name.clone(), type_params.clone());
                let prev_type_params = std::mem::replace(&mut self.current_type_params, type_params.clone());
                let semantic_fields = fields.iter()
                    .map(|(fname, ftype)| (fname.clone(), semantic_type_from_decl(ftype.clone(), &self.current_type_params)))
                    .collect();
                self.current_type_params = prev_type_params;
                Ok(SemanticStmt::StructDef {
                    name: name.clone(),
                    type_params: type_params.clone(),
                    fields: semantic_fields,
                    pos: *pos,
                })
            }
            Stmt::ImplBlock { name, aliases, methods, pos, .. } => {
                let semantic_aliases: Vec<(String, SemanticType)> = aliases.iter()
                    .map(|(aname, aty)| (aname.clone(), semantic_type_from_decl(aty.clone(), &[])))
                    .collect();

                let mut semantic_methods: Vec<SemanticFunction> = Vec::new();
                let mut method_alias_params: Vec<Vec<SemanticParam>> = Vec::new();

                for (method_name, params, ret_ty, body, ret_expr) in methods {
                    // Prepend aliases as typed params so analyze_function declares them in scope
                    let mut full_params = aliases.iter()
                        .map(|(aname, aty)| ParamKind::Typed(aname.clone(), aty.clone()))
                        .collect::<Vec<_>>();
                    full_params.extend(params.iter().cloned());

                    match self.analyze_function(method_name, &[], &full_params, ret_ty, body, ret_expr, *pos, false) {
                        Ok(SemanticStmt::FuncDef(mut sem_func)) => {
                            // Capture the alias params (carrying this method's
                            // BindingIds) BEFORE the strip. The body's
                            // VarRef/DotAccess nodes reference these BindingIds;
                            // the IR layer re-prepends these exact params so the
                            // receiver resolves. The strip itself is unchanged,
                            // so the interpreter's call_semantic_method arity is
                            // preserved.
                            let captured: Vec<SemanticParam> = sem_func
                                .params
                                .iter()
                                .take(aliases.len())
                                .cloned()
                                .collect();
                            sem_func.params = sem_func.params.into_iter()
                                .skip(aliases.len())
                                .collect();
                            semantic_methods.push(sem_func);
                            method_alias_params.push(captured);
                        }
                        Ok(_) => {}
                        Err(e) => return Err(e),
                    }
                }

                // Register methods in method_registry for return type resolution
                for sem_func in &semantic_methods {
                    for (_, alias_type) in &semantic_aliases {
                        if let SemanticType::Struct(type_name) = alias_type {
                            self.method_registry.insert(
                                (type_name.clone(), sem_func.name.clone()),
                                sem_func.clone(),
                            );
                            self.method_alias_counts.insert(
                                (type_name.clone(), sem_func.name.clone()),
                                semantic_aliases.len(),
                            );
                        }
                    }
                }

                Ok(SemanticStmt::ImplBlock {
                    name: name.clone(),
                    aliases: semantic_aliases,
                    methods: semantic_methods,
                    method_alias_params,
                    pos: *pos,
                })
            }
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
                    ty: ty.clone().map(|t| semantic_type_from_decl(t, &[])),
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
                    return Err(sem_err!(*pos_eq, "cannot assign a StrRef to a variable — use an owned str instead"));
                }
                let tp = self.current_type_params.clone();
                let target = match target {
                    Expr::Ident(name, _) => {
                        let info = self.lookup_var_mut(name).ok_or_else(|| sem_err!(*pos_eq, "use of undeclared variable '{}'", name))?;

                        if let Some(declared) = &info.declared {
                            let expected = semantic_type_from_decl(declared.clone(), &tp);
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
                            ty: binding_type(info, &tp),
                        }
                    }
                    Expr::DotAccess(container, field) => {
                        let info = self.lookup_var(container).ok_or_else(|| sem_err!(*pos_eq, "use of undeclared variable '{}'", container))?;
                        if !info.initialized {
                            return Err(sem_err!(*pos_eq, "use of uninitialized variable '{}'", container));
                        }
                        let instance_ty = info.inferred.clone()
                            .or_else(|| info.declared.as_ref().map(|t| semantic_type_from_decl(t.clone(), &[])))
                            .unwrap_or(SemanticType::Unknown);
                        let struct_name = if let SemanticType::Struct(sn) = &instance_ty {
                            sn.clone()
                        } else {
                            String::new()
                        };
                        let field_ty = if !struct_name.is_empty() {
                            self.structs.get(&struct_name)
                                .and_then(|fields| fields.iter().find(|(fname, _)| fname == field))
                                .map(|(_, ftype)| semantic_type_from_decl(ftype.clone(), &self.current_type_params))
                                .unwrap_or(SemanticType::Unknown)
                        } else {
                            SemanticType::Unknown
                        };
                        if semantic_expr.ty == SemanticType::StrRef {
                            return Err(sem_err!(*pos_eq, "cannot assign a StrRef to a struct field — use an owned str instead"));
                        }
                        if field_ty != SemanticType::Unknown {
                            check_semantic_num_fits(&semantic_expr, &field_ty, *pos_eq)?;
                            if !types_compatible(&field_ty, &semantic_expr.ty) {
                                return Err(type_mismatch_error(&field_ty, &semantic_expr.ty, *pos_eq));
                            }
                            semantic_expr = insert_cast_if_needed(semantic_expr, &field_ty);
                        }
                        SemanticLValue::DotAccess {
                            binding: Some(info.binding),
                            container: container.clone(),
                            field: field.clone(),
                            ty: field_ty,
                            struct_name,
                        }
                    }
                    Expr::Index(target_expr, index_expr, _) => {
                        let sem_target = self.analyze_expr(target_expr)?;
                        let elem_ty = match &sem_target.ty {
                            SemanticType::Array(_, elem_ty) => *elem_ty.clone(),
                            SemanticType::Unknown => SemanticType::Unknown,
                            _ => return Err(sem_err!(*pos_eq, "index assignment target must be an array")),
                        };
                        if elem_ty != SemanticType::Unknown {
                            if !types_compatible(&elem_ty, &semantic_expr.ty) {
                                return Err(type_mismatch_error(&elem_ty, &semantic_expr.ty, *pos_eq));
                            }
                            semantic_expr = insert_cast_if_needed(semantic_expr, &elem_ty);
                        }
                        let sem_index = self.analyze_expr(index_expr)?;
                        SemanticLValue::Index {
                            target: Box::new(sem_target),
                            index: Box::new(sem_index),
                            elem_ty,
                        }
                    }
                    _ => {
                        return Err(sem_err!(*pos_eq, "bad assignment target"));
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
                let declared_ty = semantic_type_from_decl(ty.clone(), &self.current_type_params);
                // Analyze with the declared element-type hint (#031a element naming,
                // #028 element range-check) via the shared helper (CR#2).
                let mut semantic_expr = self.analyze_expr_with_elem_hint(expr, &declared_ty)?;
                if semantic_expr.ty == SemanticType::StrRef {
                    return Err(sem_err!(*pos_type, "cannot assign a StrRef to a variable — use an owned str instead"));
                }

                check_semantic_num_fits(&semantic_expr, &declared_ty, *pos_type)?;

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
                macros,
                pos,
                ..
            } => {
                // Validate outer macros on this function
                for macro_ in macros {
                    match macro_ {
                        CxMacro::Test => {
                            if ret_ty.is_some() {
                                return Err(sem_err!(*pos,
                                    "#[test] functions must not return a value — remove the return type from {}",
                                    name
                                ));
                            }
                        }
                        CxMacro::Inline => {}
                        CxMacro::Reactive => {
                            return Err(sem_err!(*pos,
                                "#[reactive] is reserved and not yet implemented — reactive edges are post-v0.1"
                            ));
                        }
                        CxMacro::Deprecated(_) => {}
                        CxMacro::Cfg(_) => {
                            return Err(sem_err!(*pos,
                                "#[cfg] is reserved and not yet implemented — conditional compilation is post-v0.1"
                            ));
                        }
                        CxMacro::Unknown(macro_name) => {
                            return Err(sem_err!(*pos,
                                "unknown macro \"#[{}]\" — this macro is not defined in this version of Cx",
                                macro_name
                            ));
                        }
                    }
                }
                let is_test = macros.contains(&CxMacro::Test);
                self.analyze_function(name, type_params, params, ret_ty, body, ret_expr, *pos, is_test)
            }
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
                                return Err(sem_err!(*pos, "'{}' is not a known group or super-group name", group_name));
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
                                    return Err(sem_err!(*pos, "super-group handler list is only valid on a group pattern arm"));
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
                                    .ok_or_else(|| sem_err!(*pos, "'{}' is not a known super-group name", super_name))?;

                                if handlers.len() != sub_groups.len() {
                                    return Err(sem_err!(*pos, "super-group '{}' has {} sub-groups but {} handlers were provided", super_name, sub_groups.len(), handlers.len()));
                                }

                                let mut semantic_stmts = Vec::new();
                                for (i, handler) in handlers.iter().enumerate() {
                                    match handler {
                                        SuperGroupHandler::Placeholder => {
                                            let sub_name = &sub_groups[i];
                                            if !explicit_groups.contains(sub_name) {
                                                return Err(sem_err!(*pos, "{{_}} at position {} covers sub-group '{}' but no explicit arm for '{}' exists in this when block", i + 1, sub_name, sub_name));
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
                if !when_is_exhaustive(&semantic_arms) {
                    return Err(sem_err!(*pos, "non-exhaustive `when`: add a `_` catch-all arm to handle the remaining cases"));
                }
                Ok(SemanticStmt::When {
                    expr: semantic_expr,
                    arms: semantic_arms,
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
                    return Err(sem_err!(*pos, "Unknown value cannot be used as an if condition — control-critical context"));
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
                    return Err(sem_err!(*pos, "Unknown value cannot be used as a loop condition -- control-critical context"));
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
            Stmt::ImportBlock { imports, pos: _ } => {
                // Rule 1: position enforced by parser

                // Rule 2: no duplicate aliases
                let mut seen_aliases = std::collections::HashSet::new();
                for import in imports {
                    if !seen_aliases.insert(import.alias.clone()) {
                        return Err(sem_err!(import.pos, "duplicate import alias '{}'", import.alias));
                    }
                }

                // Rule 3: no registry paths in v0.1
                for import in imports {
                    if !import.path.starts_with("./") && !import.path.starts_with("std/") {
                        return Err(sem_err!(import.pos, "registry imports are not yet supported — use std/ for stdlib or ./ for local files"));
                    }
                }

                Ok(SemanticStmt::Noop)
            }
            Stmt::Break { pos } => Ok(SemanticStmt::Break { pos: *pos }),
            Stmt::Continue { pos } => Ok(SemanticStmt::Continue { pos: *pos }),
            Stmt::CompoundAssign {
                target,
                op,
                operand,
                pos,
            } => {
                let tp = self.current_type_params.clone();
                let sem_target = match target {
                    AssignTarget::Var(name) => {
                        let info = self.lookup_var(name)
                            .ok_or_else(|| sem_err!(*pos, "use of undeclared variable '{}'", name))?;
                        if !info.initialized {
                            return Err(sem_err!(*pos, "use of uninitialized variable '{}'", name));
                        }
                        SemanticLValue::Binding {
                            binding: info.binding,
                            name: name.clone(),
                            ty: binding_type(info, &tp),
                        }
                    }
                    AssignTarget::Field(container, field) => {
                        let binding = self.lookup_var(container).map(|info| info.binding);
                        let instance_ty = self.lookup_var(container)
                            .and_then(|info| info.inferred.clone()
                                .or_else(|| info.declared.as_ref().map(|t| semantic_type_from_decl(t.clone(), &[]))))
                            .unwrap_or(SemanticType::Unknown);
                        let struct_name = if let SemanticType::Struct(sn) = &instance_ty {
                            sn.clone()
                        } else {
                            String::new()
                        };
                        let field_ty = if !struct_name.is_empty() {
                            self.structs.get(&struct_name)
                                .and_then(|fields| fields.iter().find(|(fname, _)| fname == field))
                                .map(|(_, ftype)| semantic_type_from_decl(ftype.clone(), &self.current_type_params))
                                .unwrap_or(SemanticType::Unknown)
                        } else {
                            SemanticType::Unknown
                        };
                        SemanticLValue::DotAccess { binding, container: container.clone(), field: field.clone(), ty: field_ty, struct_name }
                    }
                    AssignTarget::Index(arr_name, idx_expr) => {
                        let arr_info = self.lookup_var(arr_name)
                            .ok_or_else(|| sem_err!(*pos, "use of undeclared variable '{}'", arr_name))?;
                        let arr_ty = binding_type(arr_info, &tp);
                        let elem_ty = match &arr_ty {
                            SemanticType::Array(_, inner) => *inner.clone(),
                            SemanticType::Unknown => SemanticType::Unknown,
                            _ => {
                                return Err(sem_err!(
                                    *pos,
                                    "compound assignment index target must be an array"
                                ))
                            }
                        };
                        let arr_pos = *pos;
                        let sem_target = self.analyze_expr(&Expr::Ident(arr_name.clone(), arr_pos))?;
                        let sem_index = self.analyze_expr(idx_expr)?;
                        SemanticLValue::Index {
                            target: Box::new(sem_target),
                            index: Box::new(sem_index),
                            elem_ty,
                        }
                    }
                };
                // Narrow the operand against the target's type when both are
                // numeric — mirrors the regular assign-to-field narrowing at
                // ~semantic.rs:444 (`insert_cast_if_needed(semantic_expr, &field_ty)`)
                // and the Stage-3 narrowing closures (MethodCall args,
                // assert_eq peer, Lt/Gt/LtEq/GtEq). Without this the operand
                // keeps its bare Numeric type, lowers as the target-native
                // width (I64 on 64-bit), and produces Binary instructions
                // whose rhs type mismatches the declared ty — Verifier errors
                // at the IR-validation layer. Covers Binding, DotAccess, and
                // Index target variants uniformly.
                let sem_operand = self.analyze_expr(operand)?;
                let target_ty = match &sem_target {
                    SemanticLValue::Binding { ty, .. } => ty.clone(),
                    SemanticLValue::DotAccess { ty, .. } => ty.clone(),
                    SemanticLValue::Index { elem_ty, .. } => elem_ty.clone(),
                };
                let narrowed_operand = if is_numeric(&target_ty) && is_numeric(&sem_operand.ty) {
                    insert_cast_if_needed(sem_operand, &target_ty)
                } else {
                    sem_operand
                };
                Ok(SemanticStmt::CompoundAssign {
                    target: sem_target,
                    op: *op,
                    operand: narrowed_operand,
                    pos: *pos,
                })
            },
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
        is_test: bool,
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
                ret_ty: ret_ty.clone().map(|t| semantic_type_from_decl(t, type_params)),
                type_params: type_params.to_vec(),
            },
        );

        self.push_scope();
        let prev_ret_ty = self.current_ret_ty.clone();
        let prev_type_params = self.current_type_params.clone();
        let prev_in_function = self.in_function;
        self.in_function = true;
        self.current_type_params = type_params.to_vec();
        self.current_ret_ty = ret_ty.clone().map(|t| semantic_type_from_decl(t, type_params));

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
                        ty: Some(semantic_type_from_decl(param_ty.clone(), type_params)),
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
            match self.current_ret_ty.clone() {
                // CR#3: the trailing-return chokepoint — same checks as a typed
                // declaration (incl. the #028 width check the trailing path
                // previously skipped, silently wrapping `fnc: t8 { 300 }` to 44).
                Some(expected) => Some(self.analyze_returned_expr(expr, &expected, pos)?),
                // Void function with a trailing expression: no declared width to
                // check against, value discarded — analyze for errors only.
                None => {
                    let sem = self.analyze_expr(expr)?;
                    if sem.ty == SemanticType::StrRef {
                        return Err(sem_err!(pos, "cannot return a StrRef — it does not outlive its origin scope"));
                    }
                    Some(sem)
                }
            }
        } else {
            if ret_ty.is_some() && !contains_return_stmt(body) {
                return Err(sem_err!(pos, "missing return value, expected {}", type_name(&semantic_type_from_decl(ret_ty.clone().unwrap(), type_params))));
            }
            None
        };

        self.current_ret_ty = prev_ret_ty;
        self.current_type_params = prev_type_params;
        self.in_function = prev_in_function;
        self.pop_scope();

        Ok(SemanticStmt::FuncDef(SemanticFunction {
            id: func_id,
            name: name.to_string(),
            type_params: type_params.to_vec(),
            params: resolved_params,
            return_ty: ret_ty.clone().map(|t| semantic_type_from_decl(t, type_params)),
            body: semantic_body,
            ret_expr: semantic_ret_expr,
            is_test,
            pos,
        }))
    }

    fn analyze_return(
        &mut self,
        expr: &Option<Expr>,
        pos: usize,
    ) -> Result<SemanticStmt, SemanticError> {
        if !self.in_function {
            return Err(sem_err!(pos, "return used outside function body"));
        }

        let expr = match (expr, self.current_ret_ty.clone()) {
            // CR#3: explicit `return expr` routes through the same chokepoint as
            // the trailing path. (It already range-checked a bare literal via the
            // shared width check; the helper preserves that and ADDS the
            // array-element check it lacked — `return [1, 2, 300]` now rejects.)
            (Some(expr), Some(expected)) => Some(self.analyze_returned_expr(expr, &expected, pos)?),
            (None, None) => None,
            (None, Some(expected)) => {
                return Err(sem_err!(pos, "missing return value, expected {}", type_name(&expected)));
            }
            (Some(expr), None) => {
                self.analyze_expr(expr)?;
                return Err(sem_err!(pos, "unexpected return value in void function"));
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
                    return Err(sem_err!(pos, "loop variable '{}' is read-only", var));
                }
                Stmt::CompoundAssign { target, .. } if matches!(target, AssignTarget::Var(n) if n == var) => {
                    self.pop_scope();
                    return Err(sem_err!(pos, "loop variable '{}' is read-only", var));
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
}
    }

    fn analyze_expr(&mut self, expr: &Expr) -> Result<SemanticExpr, SemanticError> {
        match expr {
            Expr::Val(AstValue::StructInstance(type_name, type_args, field_exprs, pos)) => {
                let mut semantic_fields: Vec<(String, SemanticExpr)> = Vec::new();
                if let Some(struct_fields) = self.structs.get(type_name).cloned() {
                    // The struct's own generic parameters must be in scope when
                    // resolving its field types, so e.g. `first: T` resolves to a
                    // type parameter (which unifies with anything) rather than a
                    // concrete struct named "T".
                    let declared_params = self.struct_type_params.get(type_name).cloned().unwrap_or_default();
                    // CR#1: explicit type args (`Pair<t8> { … }`) instantiate the
                    // struct's generic params so each field literal range-checks at
                    // the supplied width (#028/#037) and type-checks against the
                    // supplied type. Arity must match in BOTH directions — extra
                    // args would be silently dropped by the zip, too few would leave
                    // a param unsubstituted — so a mismatch errors before any
                    // substitution. Empty `type_args` is the inferred path: no
                    // instantiation, fields stay generic (`T` unconstrained),
                    // behavior unchanged (a separate generics-inference concern).
                    let instantiation: std::collections::HashMap<String, SemanticType> = if type_args.is_empty() {
                        std::collections::HashMap::new()
                    } else {
                        if type_args.len() != declared_params.len() {
                            return Err(sem_err!(*pos, "struct '{}' expects {} type argument(s), got {}", type_name, declared_params.len(), type_args.len()));
                        }
                        declared_params.iter().cloned()
                            .zip(type_args.iter().map(|t| semantic_type_from_decl(t.clone(), &self.current_type_params)))
                            .collect()
                    };
                    let mut field_type_params = declared_params;
                    field_type_params.extend(self.current_type_params.iter().cloned());
                    // strref fields cannot be stored in a struct — reject at instantiation
                    for (fname, ftype) in &struct_fields {
                        let sem_ty = semantic_type_from_decl(ftype.clone(), &field_type_params);
                        if sem_ty == SemanticType::StrRef {
                            return Err(sem_err!(*pos, "struct '{}' has a strref field '{}' — StrRef cannot be stored in struct fields because it does not outlive the struct", type_name, fname));
                        }
                    }
                    // Each provided field must exist on the struct, and its value's
                    // type must unify with the declared field type.
                    for (fname, fexpr) in field_exprs {
                        // Resolve the declared field type FIRST (before analyzing the
                        // value) so an array field's element type is known and can be
                        // hinted to the value analysis (CR#2).
                        let sem_expr = match struct_fields.iter().find(|(decl_name, _)| decl_name == fname) {
                            None => return Err(sem_err!(*pos, "unknown field '{}' in struct literal of type '{}'", fname, type_name)),
                            Some((_, decl_ty)) => {
                                // Resolve the field's declared type to its generic
                                // params, then substitute the explicit instantiation
                                // (CR#1) so `b: T` becomes `b: t8` before the range
                                // and type checks. Empty map → unchanged (inferred).
                                let decl_sem = substitute_type_params(
                                    semantic_type_from_decl(decl_ty.clone(), &field_type_params),
                                    &instantiation,
                                );
                                // CR#2: an array-typed field (incl. `[N: T]` made
                                // concrete by CR#1's substitution above) hints its
                                // element type so the array literal's elements
                                // range-check at the declared width, mirroring
                                // `x: [N: t8] = [...]`.
                                let sem_expr = self.analyze_expr_with_elem_hint(fexpr, &decl_sem)?;
                                check_semantic_num_fits(&sem_expr, &decl_sem, *pos)?;
                                if !types_compatible(&decl_sem, &sem_expr.ty) {
                                    return Err(sem_err!(*pos, "field '{}' expects type '{}' but got '{}'", fname, self::type_name(&decl_sem), self::type_name(&sem_expr.ty)));
                                }
                                sem_expr
                            }
                        };
                        semantic_fields.push((fname.clone(), sem_expr));
                    }
                    // Every declared field must be supplied.
                    for (decl_name, _) in &struct_fields {
                        if !field_exprs.iter().any(|(fname, _)| fname == decl_name) {
                            return Err(sem_err!(*pos, "missing field '{}' in struct literal of type '{}'", decl_name, type_name));
                        }
                    }
                } else {
                    // Unknown struct type — analyze field values without struct-aware checks.
                    for (fname, fexpr) in field_exprs {
                        let sem_expr = self.analyze_expr(fexpr)?;
                        semantic_fields.push((fname.clone(), sem_expr));
                    }
                }
                Ok(SemanticExpr {
                    ty: SemanticType::Struct(type_name.clone()),
                    kind: SemanticExprKind::StructInstance {
                        type_name: type_name.clone(),
                        fields: semantic_fields,
                    },
                })
            }
            Expr::Val(value) => Ok(SemanticExpr {
                ty: semantic_type_from_value(value),
                kind: SemanticExprKind::Value(semantic_value_from_ast(value, &self.enums)),
            }),
            Expr::Ident(name, pos) => {
                let info = self.lookup_var(name).ok_or_else(|| sem_err!(*pos, "use of undeclared variable '{}'", name))?;
                if !info.initialized {
                    return Err(sem_err!(*pos, "use of uninitialized variable '{}'", name));
                }
                Ok(SemanticExpr {
                    ty: binding_type(info, &self.current_type_params),
                    kind: SemanticExprKind::VarRef {
                        binding: info.binding,
                        name: name.clone(),
                    },
                })
            }
            Expr::DotAccess(container, field) => {
                let info = self.lookup_var(container).ok_or_else(|| sem_err!(0, "use of undeclared variable '{}'", container))?;
                if !info.initialized {
                    return Err(sem_err!(0, "use of uninitialized variable '{}'", container));
                }
                let instance_ty = info.inferred.clone()
                    .or_else(|| info.declared.as_ref().map(|t| semantic_type_from_decl(t.clone(), &[])))
                    .unwrap_or(SemanticType::Unknown);
                let resolved_struct_name = if let SemanticType::Struct(sn) = &instance_ty {
                    sn.clone()
                } else {
                    String::new()
                };
                let field_ty = if resolved_struct_name.is_empty() {
                    SemanticType::Unknown
                } else {
                    self.structs.get(&resolved_struct_name)
                        .and_then(|fields| fields.iter().find(|(fname, _)| fname == field))
                        .map(|(_, ftype)| semantic_type_from_decl(ftype.clone(), &self.current_type_params))
                        .unwrap_or(SemanticType::Unknown)
                };
                Ok(SemanticExpr {
                    ty: field_ty,
                    kind: SemanticExprKind::DotAccess {
                        binding: Some(info.binding),
                        container: container.clone(),
                        field: field.clone(),
                        struct_name: resolved_struct_name,
                    },
                })
            }
            Expr::HandleNew(inner, pos) => {
                let inner_analyzed = self.analyze_expr(inner)?;
                if inner_analyzed.ty == SemanticType::StrRef {
                    return Err(sem_err!(*pos, "cannot store a StrRef in a Handle — use an owned str instead"));
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
                let info = self.lookup_var(name).ok_or_else(|| sem_err!(*pos, "use of undeclared variable '{}'", name))?;
                if !info.initialized {
                    return Err(sem_err!(*pos, "use of uninitialized variable '{}'", name));
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
                let info = self.lookup_var(name).ok_or_else(|| sem_err!(*pos, "use of undeclared variable '{}'", name))?;
                if !info.initialized {
                    return Err(sem_err!(*pos, "use of uninitialized variable '{}'", name));
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
Expr::Unary(op, inner, pos) => {
                let expr = self.analyze_expr(inner)?;
                let result_ty = if *op == Op::Not {
                    SemanticType::Bool
                } else {
                    expr.ty.clone()
                };
                Ok(SemanticExpr {
                    ty: result_ty,
                    kind: SemanticExprKind::Unary {
                        op: *op,
                        expr: Box::new(expr),
                        pos: *pos,
                    },
                })
            }
            Expr::Bin(lhs, op, op_pos, rhs) => self.analyze_binary(lhs, *op, *op_pos, rhs),
            Expr::ArrayLit(elems, pos) => {
                // Consume any declared-element-type hint up front (#031a) so the
                // element sub-analyses below don't inherit it.
                let declared_elem = self.expected_array_elem.take();
                let mut semantic_elems = Vec::new();
                for e in elems {
                    semantic_elems.push(self.analyze_expr(e)?);
                }
                // With a declared element type, every element (including index 0)
                // must unify with the DECLARED type, and mismatches name it. Without
                // one (untyped array), element[0] fixes the type and later elements
                // must unify with it.
                let (elem_ty, check_from) = match &declared_elem {
                    Some(t) => (t.clone(), 0),
                    None => (
                        semantic_elems.first().map(|e| e.ty.clone()).unwrap_or(SemanticType::Unknown),
                        1,
                    ),
                };
                for (i, e) in semantic_elems.iter().enumerate().skip(check_from) {
                    if declared_elem.is_some() {
                        // Range-check integer literals against the declared element
                        // type (#028).
                        check_semantic_num_fits(e, &elem_ty, *pos)?;
                    }
                    if !types_compatible(&elem_ty, &e.ty) {
                        return Err(sem_err!(*pos, "array element at index {} expects type '{}' but got '{}'", i, type_name(&elem_ty), type_name(&e.ty)));
                    }
                }
                Ok(SemanticExpr {
                    ty: SemanticType::Array(semantic_elems.len(), Box::new(elem_ty)),
                    kind: SemanticExprKind::ArrayLit {
                        elements: semantic_elems,
                    },
                })
            }
            Expr::Index(base, idx, pos) => {
                let sem_base = self.analyze_expr(base)?;
                let sem_idx = self.analyze_expr(idx)?;
                // Carry the array's element type so an indexed read behaves like
                // any other typed expression (tracker D1.1b). Mirrors the store-
                // lvalue path (the `Array(_, elem_ty)` arm above) and struct
                // `DotAccess`. Without it the leaked `Unknown` short-circuits
                // `analyze_binary` past `common_numeric_type`/`insert_cast_if_needed`
                // and defeats typed-assignment narrowing — so `a:[k] + a:[k]` at t8
                // failed to wrap and out-of-range values slipped into typed bindings.
                // A non-array base (genuinely unknown element type) keeps `Unknown`.
                let elem_ty = match &sem_base.ty {
                    SemanticType::Array(_, elem_ty) => *elem_ty.clone(),
                    _ => SemanticType::Unknown,
                };
                Ok(SemanticExpr {
                    ty: elem_ty,
                    kind: SemanticExprKind::Index {
                        target: Box::new(sem_base),
                        index: Box::new(sem_idx),
                        pos: *pos,
                    },
                })
            }
            Expr::When(match_expr, arms, pos) => {
                let semantic_match = self.analyze_expr(match_expr)?;
                let mut semantic_arms = Vec::new();
                let mut result_ty = SemanticType::Unknown;
                for (i, arm) in arms.iter().enumerate() {
                    let pattern = self.analyze_when_pattern(&arm.pattern);
                    let body: Vec<SemanticStmt> = match &arm.body {
                        WhenBody::Stmts(stmts) => stmts.iter()
                            .map(|s| self.analyze_stmt(s))
                            .collect::<Result<Vec<_>, _>>()?,
                        WhenBody::SuperGroup(_) => Vec::new(),
                    };
                    if i == 0 {
                        if let Some(last) = body.last() {
                            if let SemanticStmt::ExprStmt { expr, .. } = last {
                                result_ty = expr.ty.clone();
                            }
                        }
                    }
                    semantic_arms.push(SemanticWhenArm { pattern, body, pos: arm.pos });
                }
                if !when_is_exhaustive(&semantic_arms) {
                    return Err(sem_err!(*pos, "non-exhaustive `when`: add a `_` catch-all arm to handle the remaining cases"));
                }
                Ok(SemanticExpr {
                    ty: result_ty,
                    kind: SemanticExprKind::When {
                        expr: Box::new(semantic_match),
                        arms: semantic_arms,
                        pos: *pos,
                    },
                })
            }
            Expr::If(cond, then_body, else_body, pos) => {
                let semantic_cond = self.analyze_expr(cond)?;
                let then_semantic: Vec<SemanticStmt> = then_body.iter()
                    .map(|s| self.analyze_stmt(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let else_semantic: Vec<SemanticStmt> = else_body.iter()
                    .map(|s| self.analyze_stmt(s))
                    .collect::<Result<Vec<_>, _>>()?;
                // #046: result type mirrors `when` — take the then-branch's
                // trailing expression type. Branch type-unification is a 0.3
                // hardening tracked for both `if` and `when` together (W1); doing
                // it here would make `if` stricter than its sibling.
                let mut result_ty = SemanticType::Unknown;
                if let Some(SemanticStmt::ExprStmt { expr, .. }) = then_semantic.last() {
                    result_ty = expr.ty.clone();
                }
                Ok(SemanticExpr {
                    ty: result_ty,
                    kind: SemanticExprKind::If {
                        condition: Box::new(semantic_cond),
                        then_body: then_semantic,
                        else_body: else_semantic,
                        pos: *pos,
                    },
                })
            }
            Expr::MethodCall(instance, method, args, pos) => {
                // Check if instance is a module alias — if so resolve from ExportTable
                if let Some(export_table) = self.module_aliases.get(instance.as_str()) {
                    if let Some(func) = export_table.functions.get(method.as_str()) {
                        let ret_ty = func.return_ty.clone().unwrap_or(SemanticType::Void);
                        let mut semantic_args: Vec<SemanticCallArg> = Vec::new();
                        for arg in args {
                            match arg {
                                CallArg::Expr(expr) => {
                                    let sem_expr = self.analyze_expr(expr)?;
                                    semantic_args.push(SemanticCallArg::Expr(sem_expr));
                                }
                                CallArg::Copy(name) => {
                                    let binding = self.lookup_var(name)
                                        .map(|i| i.binding)
                                        .unwrap_or(BindingId(u32::MAX));
                                    semantic_args.push(SemanticCallArg::Copy { binding, name: name.clone() });
                                }
                                CallArg::CopyFree(name) => {
                                    let binding = self.lookup_var(name)
                                        .map(|i| i.binding)
                                        .unwrap_or(BindingId(u32::MAX));
                                    semantic_args.push(SemanticCallArg::CopyFree { binding, name: name.clone() });
                                }
                                CallArg::CopyInto(names) => {
                                    let resolved = names.iter().map(|n| {
                                        let binding = self.lookup_var(n)
                                            .map(|i| i.binding)
                                            .unwrap_or(BindingId(u32::MAX));
                                        ResolvedBinding { binding, name: n.clone() }
                                    }).collect();
                                    semantic_args.push(SemanticCallArg::CopyInto(resolved));
                                }
                            }
                        }
                        let mangled = format!("{}${}", instance, method);
                        return Ok(SemanticExpr {
                            ty: ret_ty,
                            kind: SemanticExprKind::Call {
                                callee: mangled,
                                function: FunctionId(u32::MAX),
                                args: semantic_args,
                            },
                        });
                    } else {
                        return Err(sem_err!(*pos, "function '{}' not found in module '{}'", method, instance));
                    }
                }

                // Look up instance binding + type. Reject unresolved receivers
                // at analysis time rather than deferring to the IR layer's
                // mangled-callee miss (which surfaces as UnresolvedSemanticArtifact
                // and is less actionable for the user).
                let instance_info = match self.lookup_var(instance) {
                    Some(info) => info,
                    None => return Err(sem_err!(*pos, "method call receiver '{}' is not in scope", instance)),
                };
                let instance_binding = instance_info.binding;
                let instance_ty = instance_info.inferred.clone()
                    .or_else(|| instance_info.declared.as_ref().map(|t| semantic_type_from_decl(t.clone(), &[])))
                    .unwrap_or(SemanticType::Unknown);

                let struct_name = match &instance_ty {
                    SemanticType::Struct(tn) => tn.clone(),
                    other => return Err(sem_err!(
                        *pos,
                        "method call on '{}': receiver has non-struct type {}, method calls are only supported on struct values",
                        instance, type_name(other)
                    )),
                };

                // Look up the method on the registry. A missing entry means the
                // method isn't defined on this struct (or no impl block has been
                // analyzed for the receiver's struct).
                let method_fn = match self.method_registry.get(&(struct_name.clone(), method.clone())) {
                    Some(f) => f.clone(),
                    None => return Err(sem_err!(
                        *pos,
                        "method '{}.{}' is not defined on struct '{}'",
                        instance, method, struct_name
                    )),
                };
                let ret_ty = method_fn.return_ty.clone().unwrap_or(SemanticType::Void);

                let alias_count = self.method_alias_counts
                    .get(&(struct_name.clone(), method.clone()))
                    .copied()
                    .unwrap_or(1);
                let extra_alias_count = alias_count.saturating_sub(1);

                // Enforce arity. Mirrors the free-fn precedent at semantic.rs:1624.
                // method_fn.params already excludes the alias params (stripped at
                // semantic.rs:309) so it represents only user-declared params.
                let expected_user_arg_count = method_fn.params.len();
                let expected_total = extra_alias_count + expected_user_arg_count;
                if args.len() != expected_total {
                    return Err(sem_err!(
                        *pos,
                        "method '{}.{}' expects {} argument{} ({} extra-alias + {} user), got {}",
                        instance, method,
                        expected_total, if expected_total == 1 { "" } else { "s" },
                        extra_alias_count, expected_user_arg_count,
                        args.len()
                    ));
                }

                // analyze args
                let mut semantic_args: Vec<SemanticCallArg> = Vec::new();
                for (index, arg) in args.iter().enumerate() {
                    match arg {
                        CallArg::Expr(expr) => {
                            let sem_expr = self.analyze_expr(expr)?;
                            let sem_expr = if index >= extra_alias_count {
                                // user-typed arg: narrow against the method's
                                // declared param type, mirroring the free-fn
                                // call path (~semantic.rs:1624).
                                let user_idx = index - extra_alias_count;
                                let expected = method_fn
                                    .params
                                    .get(user_idx)
                                    .and_then(|p| p.ty.clone());
                                if let Some(expected) = expected {
                                    if !types_compatible(&expected, &sem_expr.ty) {
                                        return Err(sem_err!(
                                            *pos,
                                            "argument {} to method '{}.{}': expected {}, got {}",
                                            user_idx + 1, instance, method,
                                            type_name(&expected), type_name(&sem_expr.ty)
                                        ));
                                    }
                                    insert_cast_if_needed(sem_expr, &expected)
                                } else {
                                    sem_expr
                                }
                            } else {
                                // extra-alias (struct receiver-alias) arg:
                                // pass through, never narrow.
                                sem_expr
                            };
                            semantic_args.push(SemanticCallArg::Expr(sem_expr));
                        }
                        CallArg::Copy(name) => {
                            let binding = self.lookup_var(name)
                                .map(|i| i.binding)
                                .unwrap_or(BindingId(u32::MAX));
                            semantic_args.push(SemanticCallArg::Copy { binding, name: name.clone() });
                        }
                        CallArg::CopyFree(name) => {
                            let binding = self.lookup_var(name)
                                .map(|i| i.binding)
                                .unwrap_or(BindingId(u32::MAX));
                            semantic_args.push(SemanticCallArg::CopyFree { binding, name: name.clone() });
                        }
                        CallArg::CopyInto(names) => {
                            let resolved = names.iter().map(|n| {
                                let binding = self.lookup_var(n)
                                    .map(|i| i.binding)
                                    .unwrap_or(BindingId(u32::MAX));
                                ResolvedBinding { binding, name: n.clone() }
                            }).collect();
                            semantic_args.push(SemanticCallArg::CopyInto(resolved));
                        }
                    }
                }

                Ok(SemanticExpr {
                    ty: ret_ty,
                    kind: SemanticExprKind::MethodCall {
                        instance: instance.clone(),
                        method: method.clone(),
                        args: semantic_args,
                        instance_binding,
                        struct_name,
                        pos: *pos,
                    },
                })
            }
            Expr::ResultOk(inner, _pos) => {
                let inner_analyzed = self.analyze_expr(inner)?;
                let result_ty = SemanticType::Result(Box::new(inner_analyzed.ty.clone()));
                Ok(SemanticExpr {
                    ty: result_ty,
                    kind: SemanticExprKind::ResultOk {
                        expr: Box::new(inner_analyzed),
                    },
                })
            }
            Expr::ResultErr(inner, pos) => {
                let inner_analyzed = self.analyze_expr(inner)?;
                // Err() must wrap a string
                if inner_analyzed.ty != SemanticType::Str {
                    return Err(sem_err!(*pos, "Err() argument must be a string, got {}", type_name(&inner_analyzed.ty)));
                }
                // We don't know the T in Result<T> from the Err site alone,
                // so use a generic Result<Unknown> that the return-type check will unify.
                let result_ty = SemanticType::Result(Box::new(SemanticType::Unknown));
                Ok(SemanticExpr {
                    ty: result_ty,
                    kind: SemanticExprKind::ResultErr {
                        expr: Box::new(inner_analyzed),
                    },
                })
            }
            Expr::Try(inner, pos) => {
                let inner_analyzed = self.analyze_expr(inner)?;
                // The inner expression must be Result<T>
                let unwrapped_ty = match &inner_analyzed.ty {
                    SemanticType::Result(inner_ty) => (**inner_ty).clone(),
                    other => {
                        return Err(sem_err!(*pos, "? operator requires a Result<T> value, got {}", type_name(other)));
                    }
                };
                // The enclosing function must return Result<U>
                if !self.in_function {
                    return Err(sem_err!(*pos, "? operator can only be used inside a function"));
                }
                match &self.current_ret_ty {
                    Some(SemanticType::Result(_)) => {}
                    _ => {
                        return Err(sem_err!(*pos, "? operator can only be used in a function that returns Result<T>"));
                    }
                }
                Ok(SemanticExpr {
                    ty: unwrapped_ty,
                    kind: SemanticExprKind::Try {
                        expr: Box::new(inner_analyzed),
                        pos: *pos,
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
        // Built-in dispatch — names and per-builtin metadata come from the
        // single-source-of-truth registry (crate::frontend::builtins, #008).
        // Each kind keeps its existing recognition behavior verbatim.
        if let Some(def) = builtins::lookup(name) {
            match def.kind {
                BuiltinKind::IsKnown => {
                    let expr = match args.first() {
                        Some(CallArg::Expr(expr)) => self.analyze_expr(expr)?,
                        _ => {
                            return Err(sem_err!(pos, "call to undefined function '{}'", name));
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

                // Built-in: exit(code) / exit() — terminate with a process exit
                // code. Arity (0 or 1) enforced here via the registry so the
                // runtime sees only valid shapes. Return type Void (no usable
                // value); divergence is a runtime concern, not a type-system one
                // for 0.2 (no Never/Diverges).
                BuiltinKind::Exit => {
                    if !def.arity.accepts(args.len()) {
                        return Err(sem_err!(
                            pos,
                            "'exit' expects 0 or 1 argument(s), got {}",
                            args.len()
                        ));
                    }
                    let mut semantic_args = Vec::new();
                    if let Some(CallArg::Expr(expr)) = args.first() {
                        semantic_args.push(SemanticCallArg::Expr(self.analyze_expr(expr)?));
                    }
                    return Ok(SemanticExpr {
                        ty: SemanticType::Void,
                        kind: SemanticExprKind::Call {
                            callee: name.to_string(),
                            function: FunctionId(u32::MAX),
                            args: semantic_args,
                        },
                    });
                }

                // read(var), input("prompt", var), print/println/printn, assert,
                // assert_eq — collect Expr args; return type comes from the
                // registry (Str for read/input, Void otherwise).
                BuiltinKind::Read
                | BuiltinKind::Input
                | BuiltinKind::Print
                | BuiltinKind::Println
                | BuiltinKind::Printn
                | BuiltinKind::Assert
                | BuiltinKind::AssertEq
                | BuiltinKind::Len => {
                    let mut semantic_args = Vec::new();
                    for arg in args {
                        if let CallArg::Expr(expr) = arg {
                            let sem_expr = self.analyze_expr(expr)?;
                            semantic_args.push(SemanticCallArg::Expr(sem_expr));
                        }
                    }
                    if def.kind == BuiltinKind::AssertEq && semantic_args.len() == 2 {
                        let lhs_ty = if let SemanticCallArg::Expr(e) = &semantic_args[0] {
                            Some(e.ty.clone())
                        } else { None };
                        let rhs_ty = if let SemanticCallArg::Expr(e) = &semantic_args[1] {
                            Some(e.ty.clone())
                        } else { None };
                        if let (Some(lhs_ty), Some(rhs_ty)) = (lhs_ty, rhs_ty) {
                            if lhs_ty == SemanticType::Numeric
                                && rhs_ty != SemanticType::Numeric
                                && is_numeric(&rhs_ty)
                            {
                                if let SemanticCallArg::Expr(lhs) = semantic_args[0].clone() {
                                    semantic_args[0] =
                                        SemanticCallArg::Expr(insert_cast_if_needed(lhs, &rhs_ty));
                                }
                            } else if rhs_ty == SemanticType::Numeric
                                && lhs_ty != SemanticType::Numeric
                                && is_numeric(&lhs_ty)
                            {
                                if let SemanticCallArg::Expr(rhs) = semantic_args[1].clone() {
                                    semantic_args[1] =
                                        SemanticCallArg::Expr(insert_cast_if_needed(rhs, &lhs_ty));
                                }
                            }
                        }
                    }
                    // Tracker #021: `len` accepts a string or array only. The
                    // registry doesn't type-check builtin args in general (a
                    // general arg-typing system is a 0.3 candidate); this is a
                    // targeted one-builtin check.
                    if def.kind == BuiltinKind::Len {
                        let arg_ty = match semantic_args.first() {
                            Some(SemanticCallArg::Expr(e)) => Some(&e.ty),
                            _ => None,
                        };
                        let ok = matches!(
                            arg_ty,
                            Some(SemanticType::Str)
                                | Some(SemanticType::StrRef)
                                | Some(SemanticType::Array(_, _))
                        );
                        if !ok {
                            let got = arg_ty.map(type_name).unwrap_or_else(|| "no argument".to_string());
                            return Err(sem_err!(pos, "`len` expects a string or array, got {}", got));
                        }
                    }
                    let ret_ty = match def.ret {
                        BuiltinRet::Str => SemanticType::Str,
                        BuiltinRet::Bool => SemanticType::Bool,
                        BuiltinRet::Void => SemanticType::Void,
                        BuiltinRet::Int => SemanticType::I64,
                    };
                    return Ok(SemanticExpr {
                        ty: ret_ty,
                        kind: SemanticExprKind::Call {
                            callee: name.to_string(),
                            function: FunctionId(u32::MAX),
                            args: semantic_args,
                        },
                    });
                }
            }
        }

        let function = self.funcs.get(name).cloned().ok_or_else(|| sem_err!(pos, "call to undefined function '{}'", name))?;

        if args.len() != function.params.len() {
            return Err(sem_err!(pos, "function '{}' expects {} argument(s), got {}", name, function.params.len(), args.len()));
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
                                    return Err(sem_err!(pos, "type parameter '{}' is bound to {} but argument {} has {}", tname, type_name(existing), index + 1, type_name(&analyzed.ty)));
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
                    // CR#2: if the parameter is an array type, hint its element type
                    // so an array-literal argument range-checks its elements (#028),
                    // mirroring the typed-declaration and struct-field paths.
                    let expr = match &expected {
                        Some(e) => self.analyze_expr_with_elem_hint(expr, e)?,
                        None => self.analyze_expr(expr)?,
                    };
                    let expr = if let Some(expected) = expected {
                        check_semantic_num_fits(&expr, &expected, pos)?;
                        if !types_compatible(&expected, &expr.ty) {
                            return Err(sem_err!(pos, "argument {} to '{}': expected {}, got {}", index + 1, name, type_name(&expected), type_name(&expr.ty)));
                        }
                        insert_cast_if_needed(expr, &expected)
                    } else {
                        expr
                    };
                    semantic_args.push(SemanticCallArg::Expr(expr));
                }
                CallArg::Copy(outer_name) => {
                    let info = self.lookup_var(outer_name).ok_or_else(|| sem_err!(pos, "'.copy' argument '{}' has not been declared", outer_name))?;
                    if !info.initialized {
                        return Err(sem_err!(pos, "'.copy' argument '{}' is not initialized", outer_name));
                    }
                    semantic_args.push(SemanticCallArg::Copy {
                        binding: info.binding,
                        name: outer_name.clone(),
                    });
                }
                CallArg::CopyFree(outer_name) => {
                    let info = self.lookup_var(outer_name).ok_or_else(|| sem_err!(pos, "'.copy' argument '{}' has not been declared", outer_name))?;
                    if !info.initialized {
                        return Err(sem_err!(pos, "'.copy' argument '{}' is not initialized", outer_name));
                    }
                    semantic_args.push(SemanticCallArg::CopyFree {
                        binding: info.binding,
                        name: outer_name.clone(),
                    });
                }
                CallArg::CopyInto(outer_names) => {
                    let mut resolved = Vec::with_capacity(outer_names.len());
                    for outer_name in outer_names {
                        let info = self.lookup_var(outer_name).ok_or_else(|| sem_err!(pos, "copy_into variable '{}' has not been declared", outer_name))?;
                        if !info.initialized {
                            return Err(sem_err!(pos, "copy_into variable '{}' is not initialized", outer_name));
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

        let ret_ty = substitute_type_params(
            function.ret_ty.unwrap_or(SemanticType::Void),
            &type_param_map,
        );

        Ok(SemanticExpr {
            ty: ret_ty,
            kind: SemanticExprKind::Call {
                callee: name.to_string(),
                function: function.id,
                args: semantic_args,
            },
        })
    }

    // Analyze `expr` while hinting the declared element type of an expected array
    // type, so an array literal's elements range-check at that width (#028/#037)
    // and name the declared type on a mismatch (#031a). The `ArrayLit` arm
    // consumes the hint (`expected_array_elem.take()`); the prev/set/restore here
    // keeps it from leaking to siblings or surviving when `expr` isn't an array
    // literal. For a non-array `expected`, this is a plain `analyze_expr`. Tracker
    // CR#2: the single delivery point for the element hint, shared by the typed-
    // declaration, struct-field, and call-argument sites.
    fn analyze_expr_with_elem_hint(&mut self, expr: &Expr, expected: &SemanticType) -> Result<SemanticExpr, SemanticError> {
        if let SemanticType::Array(_, elem) = expected {
            let prev = self.expected_array_elem.take();
            self.expected_array_elem = Some((**elem).clone());
            let r = self.analyze_expr(expr);
            self.expected_array_elem = prev;
            r
        } else {
            self.analyze_expr(expr)
        }
    }

    // Analyze a returned expression against the declared return type, running the
    // SAME sequence a typed declaration runs (tracker CR#3): the array-element
    // hint (CR#2), the bare-literal width check (#028/#037, both signs), the
    // type-compatibility gate, then the (widening / in-range) cast. This is the
    // single chokepoint both return forms route through — explicit `return expr`
    // and the trailing-expression path (bare trailing expr, the #046 if-promotion,
    // and `when`-as-return) — so a return is no longer a place where an
    // out-of-range literal dodges the width check and silently wraps.
    //
    // CR#4 update: literals nested inside `if`/`when` BRANCHES (e.g. `if c { 300 }`)
    // ARE now caught — `check_semantic_num_fits` descends into each branch tail, so
    // the return path inherits branch-tail range-checking through this chokepoint,
    // the same as a declaration or struct field.
    fn analyze_returned_expr(&mut self, expr: &Expr, expected: &SemanticType, pos: usize) -> Result<SemanticExpr, SemanticError> {
        let sem = self.analyze_expr_with_elem_hint(expr, expected)?;
        if sem.ty == SemanticType::StrRef {
            return Err(sem_err!(pos, "cannot return a StrRef — it does not outlive its origin scope"));
        }
        check_semantic_num_fits(&sem, expected, pos)?;
        if !types_compatible(expected, &sem.ty) {
            return Err(sem_err!(pos, "return type mismatch: expected {}, got {}", type_name(expected), type_name(&sem.ty)));
        }
        Ok(insert_cast_if_needed(sem, expected))
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
                // Tracker #020: `+` on strings is concatenation, producing a new
                // `str`. Only `str + str` is allowed — a mixed operand (either
                // order) is rejected with a pointer at string interpolation,
                // never an implicit value->string coercion (the silent-coercion
                // class the soundness arc closed). Only `+` concatenates; the
                // other arithmetic ops fall through to the numeric requirement.
                if op == Op::Plus {
                    let lhs_str = matches!(lhs.ty, SemanticType::Str | SemanticType::StrRef);
                    let rhs_str = matches!(rhs.ty, SemanticType::Str | SemanticType::StrRef);
                    if lhs_str && rhs_str {
                        return Ok(SemanticExpr {
                            ty: SemanticType::Str,
                            kind: SemanticExprKind::Binary {
                                lhs: Box::new(lhs),
                                op,
                                pos: op_pos,
                                rhs: Box::new(rhs),
                            },
                        });
                    }
                    if lhs_str || rhs_str {
                        let other = if lhs_str { &rhs.ty } else { &lhs.ty };
                        return Err(sem_err!(op_pos, "cannot concatenate `str` and `{}` with `+` — Cx does not implicitly convert values to strings; use string interpolation instead, e.g. \"total {{x}}\"", type_name(other)));
                    }
                }
                if !is_numeric(&lhs.ty) || !is_numeric(&rhs.ty) {
                    return Err(sem_err!(op_pos, "arithmetic requires numeric operands, got {} and {}", type_name(&lhs.ty), type_name(&rhs.ty)));
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
            Op::EqEq | Op::NotEq => {
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

                Err(sem_err!(op_pos, "cannot compare {} {:?} {}", type_name(&lhs.ty), op, type_name(&rhs.ty)))
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
                    if is_numeric(&lhs.ty) && is_numeric(&rhs.ty) {
                        let compare_ty = common_numeric_type(&lhs.ty, &rhs.ty);
                        lhs = insert_cast_if_needed(lhs, &compare_ty);
                        rhs = insert_cast_if_needed(rhs, &compare_ty);
                    }
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
            Op::Not => unreachable!("Op::Not is unary only"),
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
                    Err(sem_err!(op_pos, "logical operation requires bool operands, got {} and {}", type_name(&lhs.ty), type_name(&rhs.ty)))
                }
            }
        }
    }
}

fn type_mismatch_error(expected: &SemanticType, got: &SemanticType, pos: usize) -> SemanticError {
    sem_err!(pos, "type mismatch: expected {}, got {}", type_name(expected), type_name(got))
}

fn semantic_param_placeholder(param: &ParamKind) -> SemanticParam {
    match param {
        ParamKind::Typed(name, ty) => SemanticParam {
            binding: BindingId(u32::MAX),
            name: name.clone(),
            kind: SemanticParamKind::Typed,
            ty: Some(semantic_type_from_decl(ty.clone(), &[])),
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

/// Range-check an integer literal against a declared integer width (#028).
/// Cx integers are SIGNED (the runtime stores them as i8/i16/i32/i64/i128), so
/// the valid ranges are the signed ones. A literal that statically cannot fit is
/// a semantic error in both backends, rather than wrapping in the interpreter and
/// being rejected at JIT lowering. Non-integer types pass through.
fn check_num_range(ty: &SemanticType, n: i128, pos: usize) -> Result<(), SemanticError> {
    let bounds: Option<(i128, i128, &str)> = match ty {
        SemanticType::I8 => Some((i8::MIN as i128, i8::MAX as i128, "t8")),
        SemanticType::I16 => Some((i16::MIN as i128, i16::MAX as i128, "t16")),
        SemanticType::I32 => Some((i32::MIN as i128, i32::MAX as i128, "t32")),
        SemanticType::I64 => Some((i64::MIN as i128, i64::MAX as i128, "t64")),
        SemanticType::I128 => Some((i128::MIN, i128::MAX, "t128")),
        _ => None,
    };

    if let Some((min, max, name)) = bounds {
        if n < min || n > max {
            return Err(sem_err!(
                pos,
                "integer literal {} out of range for {} (valid range: {}..{})",
                n, name, min, max
            ));
        }
    }
    Ok(())
}

/// Range-check an already-analyzed expression's integer-literal value against the
/// declared type `expected` (#028 + #037), descending into branch tails (CR#4).
/// This is the single width-check entry point for every site that has a
/// `SemanticExpr` in scope: declaration, struct field, field assignment, array
/// element, call argument, and return.
///
/// Direct forms: a bare integer literal analyzes to `Value(Num(_))`; a negated one
/// to `Unary { Minus, Value(Num(_)) }` (the analyzer does not constant-fold). The
/// sign is folded first and the signed result checked, so the check is value-aware:
/// `Num(128)` is out of range for t8 but `-Num(128)` = -128 is the valid minimum.
/// `n` is a positive `Num` (≤ i128::MAX — the lexer rejects larger), so `-n` never
/// overflows.
///
/// Branch-producing forms (CR#4): an `if`/`when` used as an expression carries its
/// value in the TAIL of each branch (its last `ExprStmt`, per the interpreter's
/// "block value is the trailing expression" rule in `eval.rs`). Every tail is
/// range-checked through this same function, so `x: t8 = if c { 300 } else { 5 }`
/// rejects the 300 exactly as a bare `x: t8 = 300` would — uniformly, at all six
/// sites. Non-literal tails (`a + b`, a call) are not folded and so not checked
/// here, matching the bare-literal-only policy; non-literal narrowing is tracked
/// separately in the design queue.
fn check_semantic_num_fits(sem: &SemanticExpr, expected: &SemanticType, pos: usize) -> Result<(), SemanticError> {
    match &sem.kind {
        SemanticExprKind::Value(SemanticValue::Num(n)) => check_num_range(expected, *n, pos)?,
        SemanticExprKind::Unary { op: Op::Minus, expr, .. } => {
            if let SemanticExprKind::Value(SemanticValue::Num(n)) = &expr.kind {
                check_num_range(expected, -*n, pos)?;
            }
        }
        SemanticExprKind::If { then_body, else_body, .. } => {
            check_branch_tail_fits(then_body, expected, pos)?;
            check_branch_tail_fits(else_body, expected, pos)?;
        }
        SemanticExprKind::When { arms, .. } => {
            for arm in arms {
                check_branch_tail_fits(&arm.body, expected, pos)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Range-check the value a branch body produces: its trailing `ExprStmt` (the
/// interpreter takes the last statement's expression as the block value — see the
/// `If`/`when` arms in `eval.rs`). Recurses through [`check_semantic_num_fits`] so a
/// nested tail — `if c { if d { 300 } else { 5 } } else { 7 }` — is covered too. A
/// branch whose last statement is not a value-producing expression (or an empty
/// body) contributes no literal to check.
fn check_branch_tail_fits(body: &[SemanticStmt], expected: &SemanticType, pos: usize) -> Result<(), SemanticError> {
    if let Some(SemanticStmt::ExprStmt { expr, .. }) = body.last() {
        check_semantic_num_fits(expr, expected, pos)?;
    }
    Ok(())
}

/// Whether a `when`'s arms make it exhaustive (#027, lenient / Option 2).
/// Exhaustive iff it has a catch-all `_` arm — also produced by enum group /
/// super-group patterns, which `analyze_when_pattern` lowers to `Catchall` — OR
/// it is a TBool match covering all three states (true, false, unknown). Open
/// domains (numeric / range / string) and bare enum-variant matches must supply
/// a `_`. Full static enum exhaustiveness is deferred to 0.3.
fn when_is_exhaustive(arms: &[SemanticWhenArm]) -> bool {
    if arms.iter().any(|a| matches!(a.pattern, SemanticWhenPattern::Catchall)) {
        return true;
    }
    let (mut has_true, mut has_false, mut has_unknown) = (false, false, false);
    for a in arms {
        if let SemanticWhenPattern::Literal(sv) = &a.pattern {
            match sv {
                SemanticValue::Bool(true) => has_true = true,
                SemanticValue::Bool(false) => has_false = true,
                SemanticValue::Unknown => has_unknown = true,
                _ => {}
            }
        }
    }
    has_true && has_false && has_unknown
}

fn semantic_type_from_decl(ty: Type, type_params: &[String]) -> SemanticType {
    match ty {
        Type::T8 => SemanticType::I8,
        Type::T16 => SemanticType::I16,
        Type::T32 => SemanticType::I32,
        Type::T64 => SemanticType::I64,
        Type::T128 => SemanticType::I128,
        Type::F64 => SemanticType::F64,
        Type::Bool => SemanticType::Bool,
        Type::Str => SemanticType::Str,
        Type::StrRef => SemanticType::StrRef,
        Type::Container => SemanticType::Container,
        Type::Char => SemanticType::Char,
        Type::Void => SemanticType::Void,
        Type::Enum(name) => SemanticType::Enum(name),
        Type::Unknown => SemanticType::Unknown,
        Type::Handle(inner) => SemanticType::Handle(Box::new(semantic_type_from_decl(*inner, type_params))),
        Type::Array(size, elem_ty) => SemanticType::Array(size, Box::new(semantic_type_from_decl(*elem_ty, type_params))),
        Type::TypeParam(s) => SemanticType::TypeParam(s),
        Type::Result(inner) => SemanticType::Result(Box::new(semantic_type_from_decl(*inner, type_params))),
        Type::Struct(name) => {
            if type_params.contains(&name) {
                SemanticType::TypeParam(name)
            } else {
                SemanticType::Struct(name)
            }
        }
    }
}

// ── Tracker #019: enum-annotation normalization ──────────────────────────────
// The parser cannot distinguish an enum name from a struct name in type-
// annotation position, so every bare name parses to `Type::Struct(name)`
// (parser.rs `named_type`). This pass runs before semantic analysis and rewrites
// `Type::Struct(name) -> Type::Enum(name)` for every `name` that is a declared
// enum. With that done, enum annotations in parameter types, return types, typed
// bindings, and struct fields flow through the existing `semantic_type_from_decl`
// arm (`Type::Enum => SemanticType::Enum`) and unify with enum *values* (which
// already infer to `SemanticType::Enum`). The parser is left untouched and none
// of the ~25 `semantic_type_from_decl` call sites change.
fn normalize_enum_type(ty: Type, enums: &std::collections::HashSet<String>) -> Type {
    match ty {
        Type::Struct(name) if enums.contains(&name) => Type::Enum(name),
        Type::Array(size, elem) => Type::Array(size, Box::new(normalize_enum_type(*elem, enums))),
        Type::Handle(inner) => Type::Handle(Box::new(normalize_enum_type(*inner, enums))),
        Type::Result(inner) => Type::Result(Box::new(normalize_enum_type(*inner, enums))),
        other => other,
    }
}

fn normalize_enum_param(p: ParamKind, enums: &std::collections::HashSet<String>) -> ParamKind {
    match p {
        ParamKind::Typed(name, ty) => ParamKind::Typed(name, normalize_enum_type(ty, enums)),
        other => other,
    }
}

fn normalize_enum_stmts(stmts: Vec<Stmt>, enums: &std::collections::HashSet<String>) -> Vec<Stmt> {
    stmts.into_iter().map(|s| normalize_enum_stmt(s, enums)).collect()
}

fn normalize_enum_when_arm(arm: WhenArm, enums: &std::collections::HashSet<String>) -> WhenArm {
    WhenArm {
        pattern: arm.pattern,
        pos: arm.pos,
        body: match arm.body {
            WhenBody::Stmts(stmts) => WhenBody::Stmts(normalize_enum_stmts(stmts, enums)),
            WhenBody::SuperGroup(handlers) => WhenBody::SuperGroup(
                handlers.into_iter().map(|h| match h {
                    SuperGroupHandler::Stmts(stmts) => SuperGroupHandler::Stmts(normalize_enum_stmts(stmts, enums)),
                    SuperGroupHandler::Placeholder => SuperGroupHandler::Placeholder,
                }).collect(),
            ),
        },
    }
}

// Rewrites enum annotations everywhere a `Type` can appear in a statement,
// recursing through every nested statement body (function/impl bodies, blocks,
// loop/if/when arms) so a typed binding like `cur: Light = ...` is normalized no
// matter how deeply nested. Type annotations never appear in expression position
// (only struct-literal generic args, which enums don't use), so expressions are
// left untouched.
fn normalize_enum_stmt(stmt: Stmt, enums: &std::collections::HashSet<String>) -> Stmt {
    match stmt {
        Stmt::StructDef { name, type_params, fields, is_pub, pos } => Stmt::StructDef {
            name, type_params, is_pub, pos,
            fields: fields.into_iter().map(|(f, t)| (f, normalize_enum_type(t, enums))).collect(),
        },
        Stmt::ImplBlock { name, aliases, methods, is_pub, pos } => Stmt::ImplBlock {
            name, is_pub, pos,
            aliases: aliases.into_iter().map(|(a, t)| (a, normalize_enum_type(t, enums))).collect(),
            methods: methods.into_iter().map(|(mname, params, ret, body, ret_expr)| (
                mname,
                params.into_iter().map(|p| normalize_enum_param(p, enums)).collect(),
                ret.map(|t| normalize_enum_type(t, enums)),
                normalize_enum_stmts(body, enums),
                ret_expr,
            )).collect(),
        },
        Stmt::ConstDecl { name, ty, value, is_pub, pos } => Stmt::ConstDecl {
            name, value, is_pub, pos, ty: normalize_enum_type(ty, enums),
        },
        Stmt::Decl { name, ty, pos } => Stmt::Decl {
            name, pos, ty: ty.map(|t| normalize_enum_type(t, enums)),
        },
        Stmt::TypedAssign { name, ty, expr, pos_type } => Stmt::TypedAssign {
            name, expr, pos_type, ty: normalize_enum_type(ty, enums),
        },
        Stmt::FuncDef { name, type_params, params, ret_ty, body, ret_expr, is_pub, macros, pos } => Stmt::FuncDef {
            name, type_params, is_pub, macros, pos,
            params: params.into_iter().map(|p| normalize_enum_param(p, enums)).collect(),
            ret_ty: ret_ty.map(|t| normalize_enum_type(t, enums)),
            body: normalize_enum_stmts(body, enums),
            ret_expr,
        },
        Stmt::Block { stmts, _pos } => Stmt::Block { stmts: normalize_enum_stmts(stmts, enums), _pos },
        Stmt::While { cond, body, pos } => Stmt::While { cond, body: normalize_enum_stmts(body, enums), pos },
        Stmt::For { var, start, end, inclusive, body, pos } => Stmt::For {
            var, start, end, inclusive, pos, body: normalize_enum_stmts(body, enums),
        },
        Stmt::Loop { body, pos } => Stmt::Loop { body: normalize_enum_stmts(body, enums), pos },
        Stmt::IfElse { condition, then_body, else_ifs, else_body, pos } => Stmt::IfElse {
            condition, pos,
            then_body: normalize_enum_stmts(then_body, enums),
            else_ifs: else_ifs.into_iter().map(|(c, b)| (c, normalize_enum_stmts(b, enums))).collect(),
            else_body: else_body.map(|b| normalize_enum_stmts(b, enums)),
        },
        Stmt::WhileIn { arr, start_slot, range_start, range_end, inclusive, body, then_chains, result, pos } => Stmt::WhileIn {
            arr, start_slot, range_start, range_end, inclusive, result, pos,
            body: normalize_enum_stmts(body, enums),
            then_chains: then_chains.into_iter().map(|c| WhileInChain {
                body: normalize_enum_stmts(c.body, enums),
                ..c
            }).collect(),
        },
        Stmt::When { expr, arms, pos } => Stmt::When {
            expr, pos,
            arms: arms.into_iter().map(|a| normalize_enum_when_arm(a, enums)).collect(),
        },
        // No type annotation and no nested statement bodies: ImportBlock, EnumDef,
        // Assign, CompoundAssign, ExprStmt, Return, Break, Continue.
        other => other,
    }
}

fn substitute_type_params(ty: SemanticType, map: &std::collections::HashMap<String, SemanticType>) -> SemanticType {
    match ty {
        SemanticType::TypeParam(name) => {
            map.get(&name).cloned().unwrap_or(SemanticType::TypeParam(name))
        }
        SemanticType::Array(size, elem) => {
            SemanticType::Array(size, Box::new(substitute_type_params(*elem, map)))
        }
        SemanticType::Handle(inner) => {
            SemanticType::Handle(Box::new(substitute_type_params(*inner, map)))
        }
        SemanticType::Result(inner) => {
            SemanticType::Result(Box::new(substitute_type_params(*inner, map)))
        }
        other => other,
    }
}

fn semantic_type_from_value(value: &AstValue) -> SemanticType {
    match value {
        AstValue::Num(_) => SemanticType::Numeric,
        AstValue::Float(_) => SemanticType::F64,
        AstValue::Str(_) => SemanticType::Str,
        AstValue::Bool(_) => SemanticType::Bool,
        AstValue::Char(_) => SemanticType::Char,
        AstValue::EnumVariant(enum_name, _) => SemanticType::Enum(enum_name.clone()),
        AstValue::StructInstance(name, _, _, _) => SemanticType::Struct(name.clone()),
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
        AstValue::StructInstance(_, _, _, _) => SemanticValue::Unknown,
        AstValue::Unknown => SemanticValue::Unknown,
    }
}

pub(crate) fn type_name(ty: &SemanticType) -> String {
    match ty {
        SemanticType::I8 => "t8".to_string(),
        SemanticType::I16 => "t16".to_string(),
        SemanticType::I32 => "t32".to_string(),
        SemanticType::I64 => "t64".to_string(),
        SemanticType::I128 => "t128".to_string(),
        SemanticType::F64 => "f64".to_string(),
        SemanticType::Bool => "bool".to_string(),
        SemanticType::Str => "str".to_string(),
        SemanticType::StrRef => "strref".to_string(),
        SemanticType::Char => "char".to_string(),
        SemanticType::Container => "container".to_string(),
        SemanticType::Numeric => "numeric literal".to_string(),
        SemanticType::Unknown => "unknown".to_string(),
        SemanticType::Void => "void".to_string(),
        // Tracker #019: render an enum type by its bare name (`Light`), not
        // `enum Light`. The `enum ` prefix was a leaking internal tag — nothing
        // branches on the rendered string — and it produced the baffling
        // "expected Light, got enum Light" mismatch (audit F4) where the two
        // names looked identical. The `SemanticType::Enum` *variant* stays
        // distinct (it is load-bearing for variant matching); only the
        // user-facing text is unified with how struct/typeparam names render.
        SemanticType::Enum(name) => name.clone(),
        SemanticType::Struct(name) => name.clone(),
        SemanticType::TypeParam(name) => name.clone(),
        SemanticType::Handle(inner) => format!("Handle<{}>", type_name(inner)),
        SemanticType::Array(size, elem) => format!("[{}: {}]", size, type_name(elem)),
        SemanticType::Result(inner) => format!("Result<{}>", type_name(inner)),
    }
}

fn binding_type(info: &VarInfo, type_params: &[String]) -> SemanticType {
    info.declared
        .clone()
        .map(|t| semantic_type_from_decl(t, type_params))
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
        (SemanticType::Array(size1, elem1), SemanticType::Array(size2, elem2)) if size1 == size2 => {
            types_compatible(elem1, elem2)
        }
        (SemanticType::Result(a), SemanticType::Result(b)) => types_compatible(a, b),
        (SemanticType::Numeric, other) | (other, SemanticType::Numeric) => is_numeric(other),
        _ => is_numeric(expected) && is_numeric(got),
    }
}

fn common_numeric_type(lhs: &SemanticType, rhs: &SemanticType) -> SemanticType {
    // Float wins
    if matches!(lhs, SemanticType::F64) || matches!(rhs, SemanticType::F64) {
        return SemanticType::F64;
    }
    // Both literals — default to I64 rather than I128.
    // I64 is the widest signed integer that Cranelift can represent as a single
    // iconst (I128 requires two i64s and is not yet JIT-supported).  Practical
    // Cx programs with integer literals that require more than 64 bits declare
    // their variables with explicit `t128` types, so the widening to I128 is
    // not needed at the literal level.
    if matches!(lhs, SemanticType::Numeric) && matches!(rhs, SemanticType::Numeric) {
        return SemanticType::I64;
    }
    // Numeric (literal) marker — adopt the other side's declared type
    if matches!(lhs, SemanticType::Numeric) {
        return rhs.clone();
    }
    if matches!(rhs, SemanticType::Numeric) {
        return lhs.clone();
    }
    // Both declared integers — pick the wider
    let rank = |t: &SemanticType| match t {
        SemanticType::I8 => 1,
        SemanticType::I16 => 2,
        SemanticType::I32 => 3,
        SemanticType::I64 => 4,
        SemanticType::I128 => 5,
        _ => 5,
    };
    if rank(lhs) >= rank(rhs) { lhs.clone() } else { rhs.clone() }
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
        Stmt::IfElse { then_body, else_ifs, else_body, .. } => {
            contains_return_stmt(then_body)
                || else_ifs.iter().any(|(_, body)| contains_return_stmt(body))
                || else_body.as_ref().map_or(false, |b| contains_return_stmt(b))
        }
        Stmt::While { body, .. } => contains_return_stmt(body),
        Stmt::For { body, .. } => contains_return_stmt(body),
        Stmt::Loop { body, .. } => contains_return_stmt(body),
        Stmt::When { .. } => false,
        Stmt::FuncDef { .. } => false,
        _ => false,
    }
}

pub fn analyze_resolved_program(
    resolved: &crate::frontend::resolver::ResolvedProgram,
) -> Result<SemanticProgram, Vec<SemanticError>> {
    let mut alias_exports: HashMap<String, ExportTable> = HashMap::new();
    let mut merged_stmts: Vec<SemanticStmt> = Vec::new();
    let mut merged_enums: Vec<SemanticEnum> = Vec::new();

    for &module_id in &resolved.topo_order {
        let file = match resolved.files.get(&module_id) {
            Some(f) => f,
            None => continue,
        };

        let is_entry = module_id == resolved.entry;

        // Find alias for this file if it's an imported module
        let alias = resolved.edges.iter()
            .find(|e| e.importee == module_id)
            .map(|e| e.alias.clone());

        // Build analyzer with module aliases from already-processed dependencies
        let mut analyzer = Analyzer::new();
        analyzer.module_aliases = alias_exports.clone();

        // Enum pre-pass (tracker #019): collect declared enum names, then rewrite
        // every `Type::Struct(name)` annotation that names an enum to
        // `Type::Enum(name)` before any analysis. The parser emits all bare-name
        // annotations as `Type::Struct`, so without this an enum used in a
        // parameter/return/typed-binding position would not unify with enum
        // values. Cross-module (imported) enum annotations stay `Struct` for now;
        // resolving aliased enum types is a 0.3 follow-up.
        let enum_names: std::collections::HashSet<String> = file.program.stmts.iter()
            .filter_map(|s| match s {
                Stmt::EnumDef { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let normalized_stmts: Vec<Stmt> = file.program.stmts.iter()
            .map(|s| normalize_enum_stmt(s.clone(), &enum_names))
            .collect();

        // Struct pre-pass
        for stmt in &normalized_stmts {
            if let Stmt::StructDef { name, fields, type_params, .. } = stmt {
                analyzer.structs.insert(name.clone(), fields.clone());
                analyzer.struct_type_params.insert(name.clone(), type_params.clone());
            }
        }

        // Function pre-pass
        for stmt in &normalized_stmts {
            if let Stmt::FuncDef { name, params, ret_ty, type_params, .. } = stmt {
                let placeholders = params.iter()
                    .map(semantic_param_placeholder)
                    .collect::<Vec<_>>();
                let func_id = analyzer.fresh_function();
                analyzer.funcs.insert(name.clone(), FunctionInfo {
                    id: func_id,
                    params: placeholders,
                    ret_ty: ret_ty.clone().map(|t| semantic_type_from_decl(t, type_params)),
                    type_params: type_params.clone(),
                });
            }
        }

        // Main analysis pass — skip ImportBlock statements
        let mut file_stmts = Vec::new();
        let mut errors = Vec::new();
        for stmt in &normalized_stmts {
            if matches!(stmt, Stmt::ImportBlock { .. }) { continue; }
            match analyzer.analyze_stmt(stmt) {
                Ok(s) => file_stmts.push(s),
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        // Build ExportTable for imported modules
        if let Some(ref alias_name) = alias {
            let mut export_table = ExportTable::new();
            // Extract pub functions from analyzed stmts
            for stmt in &file_stmts {
                if let SemanticStmt::FuncDef(sem_func) = stmt {
                    // Check if the original AST had is_pub
                    let is_pub = file.program.stmts.iter().any(|s| {
                        matches!(s, Stmt::FuncDef { name, is_pub: true, .. } if name == &sem_func.name)
                    });
                    if is_pub {
                        export_table.functions.insert(sem_func.name.clone(), sem_func.clone());
                    }
                }
            }
            // Extract pub structs
            for stmt in &file.program.stmts {
                if let Stmt::StructDef { is_pub: true, name, .. } = stmt {
                    if let Some(fields) = analyzer.structs.get(name) {
                        let sem_fields = fields.iter()
                            .map(|(fname, ftype)| (fname.clone(), semantic_type_from_decl(ftype.clone(), &[])))
                            .collect();
                        export_table.structs.insert(name.clone(), sem_fields);
                    }
                }
            }
            alias_exports.insert(alias_name.clone(), export_table);
        }

        // Accumulate results
        if is_entry {
            merged_stmts.extend(file_stmts);
            merged_enums.extend(analyzer.enum_defs);
        } else {
            // Prefix non-entry declarations with alias$ for runtime lookup
            if let Some(ref a) = alias {
                for stmt in file_stmts {
                    merged_stmts.push(prefix_stmt_name(stmt, a));
                }
            }
            merged_enums.extend(analyzer.enum_defs);
        }
    }

    Ok(SemanticProgram {
        stmts: merged_stmts,
        enums: merged_enums,
    })
}

fn prefix_stmt_name(stmt: SemanticStmt, prefix: &str) -> SemanticStmt {
    match stmt {
        SemanticStmt::FuncDef(mut func) => {
            func.name = format!("{}${}", prefix, func.name);
            SemanticStmt::FuncDef(func)
        }
        SemanticStmt::StructDef { name, fields, type_params, pos } => {
            SemanticStmt::StructDef {
                name: format!("{}${}", prefix, name),
                fields,
                type_params,
                pos,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-only wrapper: wraps a Program into a single-file ResolvedProgram
    // and calls analyze_resolved_program (the real entry point since the old
    // analyze_program was deleted in the warning cleanup sprint).
    fn analyze_program(program: &Program) -> Result<SemanticProgram, Vec<SemanticError>> {
        use crate::frontend::resolver::{ResolvedProgram, ResolvedFile, ModuleId};
        use std::collections::HashMap;
        use std::path::PathBuf;

        let entry_id = ModuleId(0);
        let mut files = HashMap::new();
        files.insert(entry_id, ResolvedFile {
            id: entry_id,
            path: PathBuf::from("test.cx"),
            program: program.clone(),
            imports: vec![],
        });

        let resolved = ResolvedProgram {
            entry: entry_id,
            files,
            edges: vec![],
            topo_order: vec![entry_id],
        };

        analyze_resolved_program(&resolved)
    }

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
                    is_pub: false,
                    macros: vec![],
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

    #[test]
    fn void_returning_function_allows_trailing_void_call() {
        // Guards `fnc: void main() { print("hi") }`. The lexer emits
        // Token::TypeVoid, the parser maps it to Type::Void, and the
        // semantic mapper lowers that to SemanticType::Void — so the
        // trailing print() (typed Void) matches the declared return type.
        let program = Program {
            stmts: vec![Stmt::FuncDef {
                name: "main".to_string(),
                type_params: vec![],
                params: vec![],
                ret_ty: Some(Type::Void),
                body: vec![],
                ret_expr: Some(Expr::Call(
                    "print".to_string(),
                    vec![CallArg::Expr(Expr::Val(AstValue::Str("hi".to_string())))],
                    0,
                )),
                pos: 0,
                is_pub: false,
                macros: vec![],
            }],
        };

        let semantic = analyze_program(&program).expect("void main with trailing print should analyse");
        match &semantic.stmts[0] {
            SemanticStmt::FuncDef(func) => {
                assert_eq!(func.return_ty, Some(SemanticType::Void));
                let ret_expr = func.ret_expr.as_ref().expect("trailing expr preserved");
                assert_eq!(ret_expr.ty, SemanticType::Void);
            }
            other => panic!("unexpected stmt: {:?}", other),
        }
    }

    #[test]
    fn parser_emits_type_void_for_void_keyword() {
        use crate::frontend::lexer::Token;
        use crate::frontend::parser;
        use chumsky::input::Stream;
        use chumsky::prelude::*;
        use logos::Logos;

        let source = "fnc: void main() {}";
        let mut tokens = Vec::new();
        let mut lex = Token::lexer(source);
        while let Some(tok_result) = lex.next() {
            let span = lex.span();
            if let Ok(token) = tok_result {
                tokens.push((token, (span.start..span.end).into()));
            }
        }
        let eoi: SimpleSpan = (source.len()..source.len()).into();
        let input = Stream::from_iter(tokens).map(eoi, |(token, span): (_, _)| (token, span));
        let program = parser::program_parser()
            .parse(input)
            .into_result()
            .expect("source should parse");

        match &program.stmts[0] {
            Stmt::FuncDef { name, ret_ty, .. } => {
                assert_eq!(name, "main");
                assert_eq!(ret_ty, &Some(Type::Void));
            }
            other => panic!("unexpected stmt: {:?}", other),
        }
    }
}
