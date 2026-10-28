//! Resolve the value shapes of implicit copy parameters before ABI planning.
//! This pass is native-only: the interpreter retains its dynamic containers.
use crate::frontend::semantic_types::*;
use std::collections::{HashMap, HashSet, VecDeque};

type Env = HashMap<BindingId, SemanticType>;
type Shape = Vec<SemanticType>;

struct Pass {
    functions: HashMap<String, SemanticFunction>,
    templates: HashSet<String>,
    cache: HashMap<(String, Shape), String>,
    pending: VecDeque<SemanticFunction>,
    structs: HashMap<String, Vec<(String, SemanticType)>>,
    struct_params: HashMap<String, Vec<String>>,
    bundles: HashMap<Vec<(String, SemanticType)>, String>,
    declarations: Vec<SemanticStmt>,
    globals: Env,
    fixed: HashSet<BindingId>,
    return_type: Option<SemanticType>,
}

fn concrete(t: &SemanticType) -> bool {
    match t {
        SemanticType::Unknown
        | SemanticType::TypeParam(_)
        | SemanticType::Container
        | SemanticType::Void => false,
        SemanticType::Handle(t) | SemanticType::Array(_, t) | SemanticType::Result(t) => {
            concrete(t)
        }
        _ => true,
    }
}

fn canonical(t: &SemanticType) -> SemanticType {
    match t {
        SemanticType::Numeric => SemanticType::I64,
        SemanticType::Array(n, t) => SemanticType::Array(*n, Box::new(canonical(t))),
        SemanticType::Result(t) => SemanticType::Result(Box::new(canonical(t))),
        _ => t.clone(),
    }
}

fn deferred(t: &SemanticType) -> bool {
    matches!(
        t,
        SemanticType::Unknown
            | SemanticType::Numeric
            | SemanticType::Container
            | SemanticType::TypeParam(_)
    )
}

fn check_shape(expected: &SemanticType, expr: &SemanticExpr) -> Result<(), String> {
    crate::ir::lower::ensure_enum_expr("copy/storage shape", expected, expr)
        .map_err(|e| e.to_string())?;
    if matches!(
        (expected, &expr.kind),
        (SemanticType::Result(_), SemanticExprKind::ResultErr { .. })
    ) {
        return Ok(());
    }
    if canonical(expected) != canonical(&expr.ty) {
        return Err(format!(
            "copy/storage shape mismatch: expected {}, got {}",
            crate::frontend::semantic::type_name(expected),
            crate::frontend::semantic::type_name(&expr.ty)
        ));
    }
    Ok(())
}

fn number(t: &SemanticType) -> bool {
    t.int_width().is_some() || matches!(t, SemanticType::Numeric | SemanticType::F64)
}

fn check_index(ty: &SemanticType) -> Result<(), String> {
    if concrete(ty) && ty.int_width().is_none() && *ty != SemanticType::Numeric {
        return Err("refined array index must be an integer".into());
    }
    Ok(())
}

fn check_binary(
    op: &crate::frontend::ast::Op,
    lhs: &SemanticType,
    rhs: &SemanticType,
) -> Result<(), String> {
    use crate::frontend::ast::Op;
    let valid = match op {
        Op::And | Op::Or => *lhs == SemanticType::Bool && *rhs == SemanticType::Bool,
        Op::EqEq | Op::NotEq => {
            (number(lhs) && number(rhs))
                || (lhs == rhs
                    && matches!(
                        lhs,
                        SemanticType::Bool
                            | SemanticType::Str
                            | SemanticType::Char
                            | SemanticType::Enum(_)
                    ))
        }
        Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => number(lhs) && number(rhs),
        Op::Plus => {
            (number(lhs) && number(rhs)) || (*lhs == SemanticType::Str && *rhs == SemanticType::Str)
        }
        Op::Minus | Op::Mul | Op::Div | Op::Mod => number(lhs) && number(rhs),
        Op::Not => false,
    };
    if !valid && concrete(lhs) && concrete(rhs) {
        return Err(format!(
            "unsupported refined operands for {op:?}: {} and {}",
            crate::frontend::semantic::type_name(lhs),
            crate::frontend::semantic::type_name(rhs)
        ));
    }
    Ok(())
}

impl Pass {
    fn register(&mut self, f: &SemanticFunction) {
        if f.type_params.is_empty() {
            if f.params
                .iter()
                .any(|p| p.ty.is_none() || matches!(p.kind, SemanticParamKind::CopyInto(_)))
            {
                self.templates.insert(f.name.clone());
            }
            self.functions.insert(f.name.clone(), f.clone());
        }
    }

    // Lift declarations only; an outer local read still needs explicit capture
    // conversion, and lowering refuses it rather than inventing a global.
    fn collect(&mut self, body: &mut [SemanticStmt], nested: bool) {
        for s in body {
            match s {
                SemanticStmt::FuncDef(f) => {
                    self.collect(&mut f.body, true);
                    if let Some(e) = &mut f.ret_expr {
                        self.collect_expr(e);
                    }
                    self.register(f);
                    if nested {
                        *s = SemanticStmt::Noop;
                    }
                }
                SemanticStmt::StructDef {
                    name,
                    fields,
                    type_params,
                    ..
                } => {
                    self.structs.insert(name.clone(), fields.clone());
                    self.struct_params.insert(name.clone(), type_params.clone());
                }
                SemanticStmt::Block { stmts, .. } => self.collect(stmts, true),
                SemanticStmt::While { cond, body, .. } => {
                    self.collect_expr(cond);
                    self.collect(body, true);
                }
                SemanticStmt::For {
                    start, end, body, ..
                } => {
                    self.collect_expr(start);
                    self.collect_expr(end);
                    self.collect(body, true);
                }
                SemanticStmt::Loop { body, .. } => self.collect(body, true),
                SemanticStmt::IfElse {
                    condition,
                    then_body,
                    else_ifs,
                    else_body,
                    ..
                } => {
                    self.collect_expr(condition);
                    self.collect(then_body, true);
                    for (c, b) in else_ifs {
                        self.collect_expr(c);
                        self.collect(b, true);
                    }
                    if let Some(b) = else_body {
                        self.collect(b, true);
                    }
                }
                SemanticStmt::When { expr, arms, .. } => {
                    self.collect_expr(expr);
                    self.collect_arms(arms);
                }
                SemanticStmt::WhileIn {
                    range_start,
                    range_end,
                    body,
                    then_chains,
                    result,
                    ..
                } => {
                    self.collect_expr(range_start);
                    self.collect_expr(range_end);
                    self.collect(body, true);
                    for c in then_chains {
                        self.collect_expr(&mut c.range_start);
                        self.collect_expr(&mut c.range_end);
                        self.collect(&mut c.body, true);
                    }
                    if let Some(e) = result {
                        self.collect_expr(e);
                    }
                }
                SemanticStmt::ImplBlock { methods, .. } | SemanticStmt::PhenDef { methods, .. } => {
                    for f in methods {
                        self.collect(&mut f.body, true);
                        if let Some(e) = &mut f.ret_expr {
                            self.collect_expr(e);
                        }
                    }
                }
                SemanticStmt::Assign { target, expr, .. } => {
                    self.collect_lvalue(target);
                    self.collect_expr(expr);
                }
                SemanticStmt::CompoundAssign {
                    target, operand, ..
                } => {
                    self.collect_lvalue(target);
                    self.collect_expr(operand);
                }
                SemanticStmt::TypedAssign { expr, .. }
                | SemanticStmt::ExprStmt { expr, .. }
                | SemanticStmt::ConstDecl { value: expr, .. } => self.collect_expr(expr),
                SemanticStmt::Return { expr, .. } => {
                    if let Some(e) = expr {
                        self.collect_expr(e);
                    }
                }
                SemanticStmt::Noop
                | SemanticStmt::ModuleNames { .. }
                | SemanticStmt::EnumDef { .. }
                | SemanticStmt::GeneDef { .. }
                | SemanticStmt::Decl { .. }
                | SemanticStmt::Break { .. }
                | SemanticStmt::Continue { .. } => {}
            }
        }
    }

    fn collect_lvalue(&mut self, lv: &mut SemanticLValue) {
        if let SemanticLValue::Index { target, index, .. } = lv {
            self.collect_expr(target);
            self.collect_expr(index);
        }
    }

    fn collect_arms(&mut self, arms: &mut [SemanticWhenArm]) {
        for a in arms {
            if let Some(g) = &mut a.guard {
                self.collect_expr(g);
            }
            self.collect(&mut a.body, true);
        }
    }

    fn collect_expr(&mut self, e: &mut SemanticExpr) {
        match &mut e.kind {
            SemanticExprKind::Value(_)
            | SemanticExprKind::VarRef { .. }
            | SemanticExprKind::DotAccess { .. }
            | SemanticExprKind::HandleVal { .. }
            | SemanticExprKind::HandleDrop { .. } => {}
            SemanticExprKind::HandleNew { value: expr, .. }
            | SemanticExprKind::Unary { expr, .. }
            | SemanticExprKind::Cast { expr, .. }
            | SemanticExprKind::ResultOk { expr }
            | SemanticExprKind::ResultErr { expr }
            | SemanticExprKind::Try { expr, .. } => self.collect_expr(expr),
            SemanticExprKind::Binary { lhs, rhs, .. }
            | SemanticExprKind::Range {
                start: lhs,
                end: rhs,
                ..
            }
            | SemanticExprKind::Index {
                target: lhs,
                index: rhs,
                ..
            } => {
                self.collect_expr(lhs);
                self.collect_expr(rhs);
            }
            SemanticExprKind::Call { args, .. } => {
                for a in args {
                    if let SemanticCallArg::Expr(e) = a {
                        self.collect_expr(e);
                    }
                }
            }
            SemanticExprKind::MethodCall {
                args,
                receiver_expr,
                ..
            } => {
                if let Some(e) = receiver_expr {
                    self.collect_expr(e);
                }
                for a in args {
                    if let SemanticCallArg::Expr(e) = a {
                        self.collect_expr(e);
                    }
                }
            }
            SemanticExprKind::ArrayLit { elements } => {
                for e in elements {
                    self.collect_expr(e);
                }
            }
            SemanticExprKind::StructInstance { fields, .. } => {
                for (_, e) in fields {
                    self.collect_expr(e);
                }
            }
            SemanticExprKind::When { expr, arms, .. } => {
                self.collect_expr(expr);
                self.collect_arms(arms);
            }
            SemanticExprKind::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.collect_expr(condition);
                self.collect(then_body, true);
                self.collect(else_body, true);
            }
        }
    }

    fn binding(&self, env: &Env, b: BindingId) -> SemanticType {
        env.get(&b)
            .or_else(|| self.globals.get(&b))
            .cloned()
            .unwrap_or(SemanticType::Unknown)
    }

    fn arg(&mut self, arg: &mut SemanticCallArg, env: &mut Env) -> Result<SemanticType, String> {
        match arg {
            SemanticCallArg::Expr(e) => {
                self.expr(e, env)?;
                Ok(canonical(&e.ty))
            }
            SemanticCallArg::Copy { binding, .. } | SemanticCallArg::CopyFree { binding, .. } => {
                Ok(canonical(&self.binding(env, *binding)))
            }
            SemanticCallArg::CopyInto(bindings) => {
                let fields: Vec<_> = bindings
                    .iter()
                    .map(|b| (b.name.clone(), canonical(&self.binding(env, b.binding))))
                    .collect();
                if fields.iter().any(|(_, t)| !concrete(t)) {
                    return Err("copy_into contains a value without a concrete layout".into());
                }
                let name = if let Some(name) = self.bundles.get(&fields) {
                    name.clone()
                } else {
                    let name = format!("$copy_into${}", self.bundles.len());
                    self.bundles.insert(fields.clone(), name.clone());
                    self.structs.insert(name.clone(), fields.clone());
                    self.declarations.push(SemanticStmt::StructDef {
                        name: name.clone(),
                        type_params: vec![],
                        fields: fields.clone(),
                        pos: 0,
                    });
                    name
                };
                let values = bindings
                    .iter()
                    .zip(fields)
                    .map(|(b, (field, ty))| {
                        (
                            field,
                            SemanticExpr {
                                ty,
                                kind: SemanticExprKind::VarRef {
                                    binding: b.binding,
                                    name: b.name.clone(),
                                },
                            },
                        )
                    })
                    .collect();
                let ty = SemanticType::Struct(name.clone());
                *arg = SemanticCallArg::Expr(SemanticExpr {
                    ty: ty.clone(),
                    kind: SemanticExprKind::StructInstance {
                        type_name: name,
                        type_args: vec![],
                        fields: values,
                    },
                });
                Ok(ty)
            }
        }
    }

    fn specialize(&mut self, name: &str, shape: Shape) -> Result<String, String> {
        let key = (name.to_string(), shape.clone());
        if let Some(n) = self.cache.get(&key) {
            return Ok(n.clone());
        }
        if shape.iter().any(|t| !concrete(t)) {
            return Err(format!(
                "call to '{name}' has an unresolved copy parameter shape"
            ));
        }
        if self.cache.keys().filter(|(n, _)| n == name).count() >= 64 {
            return Err(format!(
                "copy function '{name}' exceeded 64 argument shapes"
            ));
        }
        let mut f = self.functions[name].clone();
        let new_name = format!("{name}$copy${}", self.cache.len());
        if self.functions.contains_key(&new_name) {
            return Err(format!("copy specialization name collision: '{new_name}'"));
        }
        f.name = new_name.clone();
        for (p, t) in f.params.iter_mut().zip(shape) {
            if p.ty.is_none() || matches!(p.kind, SemanticParamKind::CopyInto(_)) {
                p.ty = Some(t);
            }
            if matches!(
                p.kind,
                SemanticParamKind::CopyFree | SemanticParamKind::CopyInto(_)
            ) {
                p.kind = SemanticParamKind::Typed;
            }
        }
        self.cache.insert(key, new_name.clone());
        self.pending.push_back(f);
        Ok(new_name)
    }

    fn field(
        &self,
        env: &Env,
        binding: Option<BindingId>,
        name: &mut String,
        field: &str,
    ) -> Option<SemanticType> {
        if let Some(b) = binding {
            if let SemanticType::Struct(n) = self.binding(env, b) {
                *name = n;
            }
        }
        self.structs
            .get(name)?
            .iter()
            .find(|(n, _)| n == field)
            .map(|(_, t)| t.clone())
    }

    fn expr(&mut self, e: &mut SemanticExpr, env: &mut Env) -> Result<(), String> {
        match &mut e.kind {
            SemanticExprKind::Value(_) => {}
            SemanticExprKind::VarRef { binding, .. } => {
                let t = self.binding(env, *binding);
                if deferred(&e.ty) && concrete(&t) {
                    e.ty = t;
                }
            }
            SemanticExprKind::DotAccess {
                binding,
                struct_name,
                field,
                ..
            } => {
                if let Some(t) = self.field(env, *binding, struct_name, field) {
                    if concrete(&t) && deferred(&e.ty) {
                        e.ty = t;
                    }
                }
            }
            SemanticExprKind::HandleNew { value, .. } => {
                self.expr(value, env)?;
                // The frontend owns constructor coercion. Only fill a deferred
                // payload when the parameter it depended on is now concrete.
                if let SemanticType::Handle(t) = &e.ty {
                    if !concrete(t) {
                        e.ty = SemanticType::Handle(Box::new(canonical(&value.ty)));
                    }
                }
            }
            SemanticExprKind::HandleVal { binding, .. } => {
                if let SemanticType::Handle(t) = self.binding(env, *binding) {
                    e.ty = *t;
                }
            }
            SemanticExprKind::HandleDrop { .. } => {}
            SemanticExprKind::Call { callee, args, .. } => {
                let mut shape: Shape = args
                    .iter_mut()
                    .map(|a| self.arg(a, env))
                    .collect::<Result<_, _>>()?;
                if self.templates.contains(callee) {
                    for (actual, param) in shape.iter_mut().zip(&self.functions[callee].params) {
                        if !matches!(param.kind, SemanticParamKind::CopyInto(_)) {
                            if let Some(declared) = &param.ty {
                                *actual = declared.clone();
                            }
                        }
                    }
                    *callee = self.specialize(callee, shape)?;
                }
            }
            SemanticExprKind::MethodCall {
                args,
                receiver_expr,
                ..
            } => {
                if let Some(e) = receiver_expr {
                    self.expr(e, env)?;
                }
                for a in args {
                    self.arg(a, env)?;
                }
            }
            SemanticExprKind::Range { start, end, .. } => {
                self.expr(start, env)?;
                self.expr(end, env)?;
            }
            SemanticExprKind::Unary { expr, op, .. } => {
                let before = expr.ty.clone();
                self.expr(expr, env)?;
                if before != expr.ty && concrete(&expr.ty) {
                    use crate::frontend::ast::Op;
                    if !((matches!(op, Op::Not) && expr.ty == SemanticType::Bool)
                        || (matches!(op, Op::Minus) && number(&expr.ty)))
                    {
                        return Err("unsupported refined unary operand".into());
                    }
                }
                if !concrete(&e.ty) {
                    e.ty = expr.ty.clone();
                }
            }
            SemanticExprKind::Binary { lhs, rhs, op, .. } => {
                let before = (lhs.ty.clone(), rhs.ty.clone());
                self.expr(lhs, env)?;
                self.expr(rhs, env)?;
                if before != (lhs.ty.clone(), rhs.ty.clone()) {
                    check_binary(op, &lhs.ty, &rhs.ty)?;
                }
                if !concrete(&e.ty) {
                    use crate::frontend::ast::Op;
                    e.ty = if matches!(
                        op,
                        Op::EqEq
                            | Op::NotEq
                            | Op::Lt
                            | Op::LtEq
                            | Op::Gt
                            | Op::GtEq
                            | Op::And
                            | Op::Or
                    ) {
                        SemanticType::Bool
                    } else if canonical(&lhs.ty) == canonical(&rhs.ty) {
                        canonical(&lhs.ty)
                    } else {
                        SemanticType::Unknown
                    };
                }
            }
            SemanticExprKind::Cast { expr, from, to } => {
                let before = from.clone();
                self.expr(expr, env)?;
                *from = expr.ty.clone();
                if before != *from {
                    if from == to {
                        *e = (**expr).clone();
                    } else if !number(from) || !number(to) {
                        return Err("refined cast requires compatible numeric types".into());
                    }
                }
            }
            SemanticExprKind::ArrayLit { elements } => {
                let elem_ty = match &e.ty {
                    SemanticType::Array(_, t) => *t.clone(),
                    _ => SemanticType::Unknown,
                };
                let mut changed = false;
                for e in elements.iter_mut() {
                    let before = e.ty.clone();
                    self.expr(e, env)?;
                    if before != e.ty {
                        changed = true;
                        if !deferred(&elem_ty) {
                            check_shape(&elem_ty, e)?;
                        }
                    }
                }
                if !concrete(&e.ty) || (changed && deferred(&elem_ty)) {
                    if let Some(first) = elements.first() {
                        if elements
                            .iter()
                            .all(|v| canonical(&v.ty) == canonical(&first.ty))
                        {
                            e.ty =
                                SemanticType::Array(elements.len(), Box::new(canonical(&first.ty)));
                        } else {
                            return Err(
                                "refined array elements require a uniform value shape".into()
                            );
                        }
                    }
                }
            }
            SemanticExprKind::Index { target, index, .. } => {
                self.expr(target, env)?;
                self.expr(index, env)?;
                check_index(&index.ty)?;
                if let SemanticType::Array(_, t) = &target.ty {
                    e.ty = *t.clone();
                }
            }
            SemanticExprKind::StructInstance {
                type_name,
                type_args,
                fields,
            } => {
                let subst: HashMap<_, _> = self
                    .struct_params
                    .get(type_name)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .zip(type_args.iter().cloned())
                    .collect();
                let declared = self.structs.get(type_name).cloned().unwrap_or_default();
                for (field, e) in fields {
                    let before = e.ty.clone();
                    self.expr(e, env)?;
                    if before != e.ty {
                        // The literal's instantiated declaration, not pointer
                        // erasure, defines each field's storage identity.
                        if let Some((_, ty)) = declared.iter().find(|(name, _)| name == field) {
                            let ty = crate::frontend::semantic::substitute_type_params(
                                ty.clone(),
                                &subst,
                            );
                            if concrete(&ty) {
                                check_shape(&ty, e)?;
                            }
                        }
                    }
                }
            }
            SemanticExprKind::When { expr, arms, .. } => {
                self.expr(expr, env)?;
                self.arms(arms, &expr.ty, env)?;
            }
            SemanticExprKind::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.expr(condition, env)?;
                self.stmts(then_body, &mut env.clone())?;
                self.stmts(else_body, &mut env.clone())?;
            }
            SemanticExprKind::ResultOk { expr } => {
                let before = expr.ty.clone();
                self.expr(expr, env)?;
                if before != expr.ty || !concrete(&e.ty) {
                    if matches!(
                        expr.ty,
                        SemanticType::Struct(_)
                            | SemanticType::Array(_, _)
                            | SemanticType::Result(_)
                    ) {
                        return Err(
                            "refined aggregate Result payload requires owned storage".into()
                        );
                    }
                    match &e.ty {
                        SemanticType::Result(t) if !deferred(t) => check_shape(t, expr)?,
                        _ => e.ty = SemanticType::Result(Box::new(expr.ty.clone())),
                    }
                }
            }
            SemanticExprKind::ResultErr { expr } => self.expr(expr, env)?,
            SemanticExprKind::Try { expr, .. } => {
                self.expr(expr, env)?;
                if let SemanticType::Result(t) = &expr.ty {
                    if deferred(&e.ty) {
                        e.ty = *t.clone();
                    }
                }
            }
        }
        Ok(())
    }

    fn lvalue(&mut self, lv: &mut SemanticLValue, env: &mut Env) -> Result<(), String> {
        match lv {
            SemanticLValue::Binding { binding, ty, .. } => {
                let t = self.binding(env, *binding);
                if deferred(ty) && concrete(&t) {
                    *ty = t;
                }
            }
            SemanticLValue::DotAccess {
                binding,
                struct_name,
                field,
                ty,
                ..
            } => {
                if let Some(t) = self.field(env, *binding, struct_name, field) {
                    *ty = t;
                }
            }
            SemanticLValue::Index {
                target,
                index,
                elem_ty,
            } => {
                self.expr(target, env)?;
                self.expr(index, env)?;
                check_index(&index.ty)?;
                if let SemanticType::Array(_, t) = &target.ty {
                    *elem_ty = *t.clone();
                }
            }
        }
        Ok(())
    }

    fn arms(
        &mut self,
        arms: &mut [SemanticWhenArm],
        ty: &SemanticType,
        env: &Env,
    ) -> Result<(), String> {
        for a in arms {
            let mut local = env.clone();
            if let SemanticWhenPattern::EnumVariant {
                binding: Some((b, _)),
                ..
            } = &a.pattern
            {
                local.insert(*b, ty.clone());
            }
            if let Some(g) = &mut a.guard {
                self.expr(g, &mut local)?;
            }
            self.stmts(&mut a.body, &mut local)?;
        }
        Ok(())
    }

    fn function(&mut self, f: &mut SemanticFunction) -> Result<(), String> {
        let mut env = self.globals.clone();
        let fixed = f.params.iter().filter(|p| matches!(p.kind, SemanticParamKind::Copy) || matches!(&p.ty, Some(SemanticType::Struct(n)) if n.starts_with("$copy_into$"))).map(|p| p.binding).collect();
        let outer_fixed = std::mem::replace(&mut self.fixed, fixed);
        let return_type = if f.name.contains("$copy$") {
            f.return_ty.clone()
        } else {
            None
        };
        let outer_return = std::mem::replace(&mut self.return_type, return_type);
        for p in &f.params {
            if let Some(t) = &p.ty {
                env.insert(p.binding, t.clone());
            }
        }
        self.stmts(&mut f.body, &mut env)?;
        if let Some(e) = &mut f.ret_expr {
            self.expr(e, &mut env)?;
            if f.name.contains("$copy$") {
                if let Some(t) = &f.return_ty {
                    check_shape(t, e)?;
                }
            }
        }
        self.fixed = outer_fixed;
        self.return_type = outer_return;
        Ok(())
    }

    fn stmts(&mut self, stmts: &mut [SemanticStmt], env: &mut Env) -> Result<(), String> {
        for s in stmts {
            match s {
                SemanticStmt::Noop
                | SemanticStmt::ModuleNames { .. }
                | SemanticStmt::EnumDef { .. }
                | SemanticStmt::GeneDef { .. }
                | SemanticStmt::StructDef { .. }
                | SemanticStmt::Break { .. }
                | SemanticStmt::Continue { .. } => {}
                SemanticStmt::Decl { binding, ty, .. } => {
                    env.insert(*binding, ty.clone().unwrap_or(SemanticType::Unknown));
                }
                SemanticStmt::TypedAssign {
                    binding, ty, expr, ..
                }
                | SemanticStmt::ConstDecl {
                    binding,
                    ty,
                    value: expr,
                    ..
                } => {
                    self.expr(expr, env)?;
                    env.insert(*binding, ty.clone());
                }
                SemanticStmt::Assign { target, expr, .. } => {
                    self.expr(expr, env)?;
                    if let SemanticLValue::Binding { binding, ty, .. } = target {
                        if self.fixed.contains(binding) {
                            check_shape(&self.binding(env, *binding), expr)?;
                        }
                        // Analysis may refine an untyped numeric variable on
                        // every assignment. Its validated target wins over an
                        // earlier environment entry.
                        if deferred(ty) {
                            *ty = canonical(&expr.ty);
                        }
                        env.insert(*binding, ty.clone());
                    } else {
                        self.lvalue(target, env)?;
                        let ty = match target {
                            SemanticLValue::DotAccess { ty, .. } => ty,
                            SemanticLValue::Index { elem_ty, .. } => elem_ty,
                            _ => unreachable!(),
                        };
                        check_shape(ty, expr)?;
                    }
                }
                SemanticStmt::CompoundAssign {
                    target,
                    operand,
                    op,
                    ..
                } => {
                    self.lvalue(target, env)?;
                    self.expr(operand, env)?;
                    let ty = match target {
                        SemanticLValue::Binding { ty, .. }
                        | SemanticLValue::DotAccess { ty, .. } => ty,
                        SemanticLValue::Index { elem_ty, .. } => elem_ty,
                    };
                    check_binary(op, ty, &operand.ty)?;
                }
                SemanticStmt::ExprStmt { expr, .. } => self.expr(expr, env)?,
                SemanticStmt::Return { expr, .. } => {
                    if let Some(e) = expr {
                        self.expr(e, env)?;
                        if let Some(t) = &self.return_type {
                            check_shape(t, e)?;
                        }
                    }
                }
                SemanticStmt::FuncDef(f) => {
                    if f.type_params.is_empty() && !self.templates.contains(&f.name) {
                        self.function(f)?;
                    }
                }
                SemanticStmt::Block { stmts, .. } => self.stmts(stmts, &mut env.clone())?,
                SemanticStmt::While { cond, body, .. } => {
                    self.expr(cond, env)?;
                    self.stmts(body, &mut env.clone())?;
                }
                SemanticStmt::For {
                    binding,
                    start,
                    end,
                    body,
                    ..
                } => {
                    self.expr(start, env)?;
                    self.expr(end, env)?;
                    let mut local = env.clone();
                    local.insert(*binding, SemanticType::I64);
                    self.stmts(body, &mut local)?;
                }
                SemanticStmt::Loop { body, .. } => self.stmts(body, &mut env.clone())?,
                SemanticStmt::IfElse {
                    condition,
                    then_body,
                    else_ifs,
                    else_body,
                    ..
                } => {
                    self.expr(condition, env)?;
                    self.stmts(then_body, &mut env.clone())?;
                    for (c, b) in else_ifs {
                        self.expr(c, env)?;
                        self.stmts(b, &mut env.clone())?;
                    }
                    if let Some(b) = else_body {
                        self.stmts(b, &mut env.clone())?;
                    }
                }
                SemanticStmt::When { expr, arms, .. } => {
                    self.expr(expr, env)?;
                    self.arms(arms, &expr.ty, env)?;
                }
                SemanticStmt::WhileIn {
                    range_start,
                    range_end,
                    body,
                    then_chains,
                    result,
                    ..
                } => {
                    self.expr(range_start, env)?;
                    self.expr(range_end, env)?;
                    self.stmts(body, &mut env.clone())?;
                    for c in then_chains {
                        self.expr(&mut c.range_start, env)?;
                        self.expr(&mut c.range_end, env)?;
                        self.stmts(&mut c.body, &mut env.clone())?;
                    }
                    if let Some(e) = result {
                        self.expr(e, env)?;
                    }
                }
                SemanticStmt::ImplBlock { methods, .. } | SemanticStmt::PhenDef { methods, .. } => {
                    for f in methods {
                        self.function(f)?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn resolve(program: &SemanticProgram) -> Result<SemanticProgram, String> {
    let mut out = program.clone();
    let mut p = Pass {
        functions: HashMap::new(),
        templates: HashSet::new(),
        cache: HashMap::new(),
        pending: VecDeque::new(),
        structs: HashMap::new(),
        struct_params: HashMap::new(),
        bundles: HashMap::new(),
        declarations: vec![],
        globals: Env::new(),
        fixed: HashSet::new(),
        return_type: None,
    };
    p.collect(&mut out.stmts, false);
    let original: HashSet<_> = out
        .stmts
        .iter()
        .filter_map(|s| {
            if let SemanticStmt::FuncDef(f) = s {
                Some(f.name.clone())
            } else {
                None
            }
        })
        .collect();
    let mut lifted: Vec<_> = p
        .functions
        .values()
        .filter(|f| !original.contains(&f.name) && !p.templates.contains(&f.name))
        .cloned()
        .collect();
    lifted.sort_by(|a, b| a.name.cmp(&b.name));
    out.stmts
        .extend(lifted.into_iter().map(SemanticStmt::FuncDef));
    // Seed initializer binding shapes before walking any function bodies.
    for s in &out.stmts {
        match s {
            SemanticStmt::TypedAssign { binding, ty, .. }
            | SemanticStmt::ConstDecl { binding, ty, .. } => {
                p.globals.insert(*binding, ty.clone());
            }
            SemanticStmt::Decl {
                binding,
                ty: Some(ty),
                ..
            } => {
                p.globals.insert(*binding, ty.clone());
            }
            SemanticStmt::Assign {
                target: SemanticLValue::Binding { binding, .. },
                expr,
                ..
            } => {
                p.globals
                    .entry(*binding)
                    .or_insert_with(|| canonical(&expr.ty));
            }
            _ => {}
        }
    }
    p.stmts(&mut out.stmts, &mut p.globals.clone())?;
    out.stmts
        .retain(|s| !matches!(s, SemanticStmt::FuncDef(f) if p.templates.contains(&f.name)));
    while let Some(mut f) = p.pending.pop_front() {
        p.function(&mut f)?;
        out.stmts.push(SemanticStmt::FuncDef(f));
    }
    out.stmts.extend(p.declarations);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{resolver, semantic};

    fn analyzed(source: &str) -> SemanticProgram {
        let source = source.replace("};", "}\n");
        let id = resolver::ModuleId(0);
        let file = resolver::ResolvedFile {
            id,
            path: "copy_shapes.cx".into(),
            program: resolver::parse_source(&source).expect("parses"),
            imports: vec![],
        };
        semantic::analyze_resolved_program(&resolver::ResolvedProgram {
            entry: id,
            files: HashMap::from([(id, file)]),
            edges: vec![],
            topo_order: vec![id],
        })
        .expect("analyzes")
    }

    #[test]
    fn preserves_sequential_numeric_refinement() {
        let p = analyzed("fnc:t64 run(){ a:t8=1; b:t64=300; let x; x=a; x=b; x }; print(run())");
        let ir = crate::ir::lower::lower_program(&p)
            .expect("validated later type remains authoritative");
        crate::ir::validate::validate_module(&ir).unwrap();
    }

    #[test]
    fn typed_non_copy_arguments_do_not_require_inferred_error_payloads() {
        let p = analyzed("fnc:t64 take(x.copy, unused:Result<t64>){x}; n:t64=1; print(take(n.copy, Err(\"unused\")))");
        crate::ir::lower::lower_program(&p).expect("declared Result layout is sufficient");
    }

    #[test]
    fn refined_copy_types_cannot_use_stale_numeric_casts() {
        for (ty, val) in [("bool", "true"), ("str", "\"value\"")] {
            let p = analyzed(&format!("fnc:t64 as_num(value.copy.free){{value}}; flag:{ty}={val}; print(as_num(flag.copy.free))"));
            assert!(
                crate::ir::lower::lower_program(&p).is_err(),
                "{ty} cannot become a numeric payload or address"
            );
        }
    }

    #[test]
    fn shape_changing_copy_assignment_cannot_outgrow_storage() {
        let p = analyzed("fnc:t64 replace(p.copy){p=[1]; 0}; a:[2:t64]=[10,20]; print(replace(a.copy)); print(a:[1])");
        let err = resolve(&p).expect_err("fixed copy-out storage must match every write");
        assert!(err.contains("copy/storage shape mismatch"), "{err}");
        let p = analyzed("fnc:t64 replace(p.copy){p=Ok(1);0} r:Result<str> = Ok(\"safe\"); print(replace(r.copy)); print(r)");
        let err =
            resolve(&p).expect_err("packed Result payload identity cannot change during copy-out");
        assert!(err.contains("copy/storage shape mismatch"), "{err}");
    }

    #[test]
    fn discovers_nested_functions_inside_value_branches() {
        let p = analyzed("fnc:t64 run(){ if true { fnc:t64 inner(n.copy){n}; x:t64=1; inner(x.copy) } else {0} }; print(run())");
        let ir =
            crate::ir::lower::lower_program(&p).expect("expression branch declarations are lifted");
        crate::ir::validate::validate_module(&ir).unwrap();
    }

    #[test]
    fn refined_fields_cannot_launder_unchecked_operators() {
        for source in [
            "enum A{First} enum B{First} a:A=A::First; b:B=B::First; fnc:bool same(t:copy_into(a,b)){t.a==t.b} print(same(copy_into(a,b)))",
            "struct B{a:t64,b:t64} s:str=\"xx\"; box:B=B{a:1,b:2}; fnc:bool same(t:copy_into(s,box)){t.s==t.box} print(same(copy_into(s,box)))",
            "enum A{First} a:A=A::First; fnc:A neg(t:copy_into(a)){-t.a} print(neg(copy_into(a)))",
            "enum C{Red,Green} a:C=C::Red; b:C=C::Green; fnc:t64 add(t:copy_into(a,b)){t.a+=t.b;0} print(add(copy_into(a,b)))",
        ] {
            let p = analyzed(source);
            let err = resolve(&p).expect_err("refined types require semantic operator checks");
            assert!(err.contains("unsupported refined"), "{err}");
        }
    }

    #[test]
    fn unknown_bool_bundle_cannot_silently_become_true() {
        let p = analyzed(
            "fnc: run(){u:bool=?; fnc: show(t:copy_into(u)){print(t.u)} show(copy_into(u))} run()",
        );
        let err = crate::ir::lower::lower_program(&p)
            .expect_err("unknown state needs storage support before bundling");
        assert!(
            err.to_string()
                .contains("three-state bool aggregate storage"),
            "{err}"
        );
    }

    #[test]
    fn fixed_copy_slots_and_explicit_returns_require_matching_shapes() {
        let p =
            analyzed("fnc:t64 replace(p.copy){p=300;0} a:t8=1; print(replace(a.copy)); print(a)");
        let err = resolve(&p).expect_err("fixed scalar writes must fit their slot");
        assert!(err.contains("copy/storage shape mismatch"), "{err}");

        // Analysis currently rejects an unresolved scalar as an array return.
        // Exercise the pass's own explicit-return contract with semantic input
        // so a future frontend inference improvement cannot bypass the check.
        let mut p = analyzed(
            "fnc:t64 wrong(p.copy.free){return p} a:[1:t64]=[7]; print(wrong(a.copy.free))",
        );
        for s in &mut p.stmts {
            if let SemanticStmt::FuncDef(f) = s {
                f.return_ty = Some(SemanticType::Array(2, Box::new(SemanticType::I64)));
                let param = &f.params[0];
                f.body = vec![SemanticStmt::Return {
                    expr: Some(SemanticExpr {
                        ty: SemanticType::Numeric,
                        kind: SemanticExprKind::VarRef {
                            binding: param.binding,
                            name: param.name.clone(),
                        },
                    }),
                    pos: 0,
                }];
            }
        }
        let err = resolve(&p).expect_err("explicit return must fit its return slot");
        assert!(err.contains("copy/storage shape mismatch"), "{err}");
    }

    #[test]
    fn refined_wrappers_preserve_payload_and_index_contracts() {
        for source in [
            "fnc:Result<t64> wrap(x.copy.free){Ok(x)} a:[1:t64]=[7]; print(wrap(a.copy.free))",
            "fnc:[1:str] wrap(t:copy_into(v)){[t.v]} v:t64=1; a:[1:str]=wrap(copy_into(v)); print(a:[0])",
            "enum C{Zero} a:[1:t64]=[5]; i:C=C::Zero; fnc:t64 f(t:copy_into(a,i)){items:[1:t64]=t.a; items:[t.i]=7;0} print(f(copy_into(a,i)))",
            "fnc:t64 run(x.copy.free){let values; values=[x,1]; print(values:[0]);0} s:str=\"text\"; print(run(s.copy.free))",
        ] {
            let p = analyzed(source);
            assert!(resolve(&p).is_err(), "refinement must reject an incompatible wrapper: {source}");
        }
    }
}
