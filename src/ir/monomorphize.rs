//! Generic-function monomorphization (known-issues §19).
//!
//! One specialized `SemanticFunction` per (template, concrete type arguments)
//! combination, emitted through the ordinary function-lowering path and called
//! directly by a mangled name — the same shape gene/phen slice 5 already ships
//! for phens. The difference, and the whole reason this module exists, is that
//! a phen names its concrete type at the DECLARATION (`phen Add (v: Vec2)`)
//! while a generic function learns its types only at call sites, possibly
//! several, so something has to go and find them.
//!
//! # Why this runs before `build_signature_table`
//!
//! `ret_struct_of` decides whether a function gets the hidden caller-allocated
//! `$ret_slot` parameter, and it reads `function.return_ty` directly. A
//! specialization still carrying `TypeParam("T")` there would answer `None`,
//! get no slot parameter, and silently corrupt a struct return — the exact
//! shape the caller-allocated-slot fix closed. Specializations must therefore
//! already be substituted `SemanticFunction`s by the time the signature table is
//! built. That is a placement guarantee, not a check.
//!
//! # Exhaustiveness
//!
//! The walker matches every `SemanticStmt` and `SemanticExprKind` variant with
//! **no catch-all arm**, so adding a variant to either enum is a compile error
//! here rather than a silently unvisited subtree. A missed variant would be a
//! hole in every guarantee above it: an unspecialized inner call keeps a
//! `TypeParam` and the whole program falls back to a SKIP, or worse, a name
//! that resolves to the wrong specialization.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::frontend::semantic::{substitute_type_params, type_name};
use crate::frontend::semantic_types::{
    FunctionId, SemanticCallArg, SemanticExpr, SemanticExprKind, SemanticFunction, SemanticLValue,
    SemanticProgram, SemanticStmt, SemanticType, SemanticWhenArm,
};

/// Maximum distinct instantiations of any one template.
///
/// Type-growing recursion — `fnc: T <T> grow(x: T) { grow(Box { v: x }) }` —
/// generates `T`, `Box<T>`, `Box<Box<T>>`, … without bound, so the worklist
/// would not terminate. A per-template count is the natural guard because the
/// dedup map already tracks exactly that; a depth cap has no meaning in a
/// worklist, which has no stack. The limit is set far above real code: the
/// largest template in the corpus has **two** distinct instantiations.
const MAX_INSTANTIATIONS_PER_TEMPLATE: usize = 64;

/// A concrete substitution for one template's type parameters.
type Subst = HashMap<String, SemanticType>;

#[derive(Debug)]
pub enum MonomorphizeError {
    /// A template exceeded [`MAX_INSTANTIATIONS_PER_TEMPLATE`].
    InstantiationExplosion {
        template: String,
        cap: usize,
        seen: Vec<String>,
    },
    /// A specialization's mangled name collides with something already defined.
    NameCollision { mangled: String, template: String },
}

impl std::fmt::Display for MonomorphizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstantiationExplosion { template, cap, seen } => write!(
                f,
                "generic function '{}' exceeded {} distinct instantiations — \
                 this usually means a recursive call instantiates it at an ever-larger type. \
                 First instantiations were: {}",
                template,
                cap,
                seen.join(", ")
            ),
            Self::NameCollision { mangled, template } => write!(
                f,
                "specialization of '{}' would be named '{}', which is already defined",
                template, mangled
            ),
        }
    }
}

/// Mangled name for one specialization: `identity$t64`, `pair_print$t64$bool`.
///
/// Same `$` convention as `mangle_method`, collision-free against user
/// identifiers for the same reason (`$` is not a legal identifier character).
/// Against phen specializations (`Type$method`) it is distinguishable in
/// principle — first segment a function name, later segments type names — but
/// "in principle" is not a guarantee, so `emit` checks the registry rather than
/// relying on the argument.
fn mangle_specialization(base: &str, args: &[SemanticType]) -> String {
    let mut out = base.to_string();
    for a in args {
        out.push('$');
        out.push_str(&type_name(a));
    }
    out
}

/// One pending specialization.
#[derive(Clone, Debug)]
struct Demand {
    function: FunctionId,
    template: String,
    args: Vec<SemanticType>,
}

/// Rewriting context: the substitution in force, plus the demands discovered.
struct Rewriter<'a> {
    subst: Subst,
    templates: &'a HashMap<FunctionId, SemanticFunction>,
    demands: Vec<Demand>,
}

impl Rewriter<'_> {
    fn ty(&self, t: &SemanticType) -> SemanticType {
        substitute_type_params(t.clone(), &self.subst)
    }

    /// Handle a `Call`: if the callee is a generic template, compose this
    /// call's recorded (possibly symbolic) arguments with the substitution in
    /// force, demand that specialization, and rewrite the callee to its name.
    ///
    /// Composition is the whole trick. A call inside a generic body recorded
    /// `[TypeParam("T")]`; specializing the enclosing function under
    /// `{T -> I64}` turns that into `[I64]` by ordinary substitution, with no
    /// re-derivation of inference outside the analyser.
    fn call(&mut self, callee: &mut String, function: FunctionId, type_args: &mut [SemanticType]) {
        for a in type_args.iter_mut() {
            *a = self.ty(a);
        }
        let Some(template) = self.templates.get(&function) else {
            return;
        };
        // A short vector means analysis could not bind every parameter; leave
        // the call alone so lowering SKIPs rather than emitting a wrong name.
        if type_args.len() != template.type_params.len() || type_args.is_empty() {
            return;
        }
        // Still symbolic after substitution — cannot specialize from here.
        if type_args.iter().any(contains_type_param) {
            return;
        }
        self.demands.push(Demand {
            function,
            template: template.name.clone(),
            args: type_args.to_vec(),
        });
        *callee = mangle_specialization(&template.name, type_args);
    }
}

fn contains_type_param(t: &SemanticType) -> bool {
    match t {
        SemanticType::TypeParam(_) => true,
        SemanticType::Handle(inner) | SemanticType::Result(inner) => contains_type_param(inner),
        SemanticType::Array(_, elem) => contains_type_param(elem),
        SemanticType::I8
        | SemanticType::I16
        | SemanticType::I32
        | SemanticType::I64
        | SemanticType::I128
        | SemanticType::F64
        | SemanticType::Bool
        | SemanticType::Str
        | SemanticType::StrRef
        | SemanticType::Container
        | SemanticType::Char
        | SemanticType::Enum(_)
        | SemanticType::Struct(_)
        | SemanticType::Unknown
        | SemanticType::Numeric
        | SemanticType::Void => false,
    }
}

// ── The walker ───────────────────────────────────────────────────────────────
//
// Every arm below is explicit. Do not add a `_ =>` arm: the compile error you
// get from a new variant is the only thing keeping this complete.

fn rw_expr(e: &mut SemanticExpr, r: &mut Rewriter) {
    e.ty = r.ty(&e.ty);
    match &mut e.kind {
        SemanticExprKind::Value(_) => {}
        SemanticExprKind::VarRef { .. } => {}
        SemanticExprKind::DotAccess { struct_name, .. } => {
            rw_type_name(struct_name, r);
        }
        SemanticExprKind::HandleNew { value, .. } => rw_expr(value, r),
        SemanticExprKind::HandleVal { .. } => {}
        SemanticExprKind::HandleDrop { .. } => {}
        SemanticExprKind::Call { callee, function, args, type_args } => {
            let f = *function;
            r.call(callee, f, type_args);
            for a in args {
                rw_call_arg(a, r);
            }
        }
        SemanticExprKind::Range { start, end, .. } => {
            rw_expr(start, r);
            rw_expr(end, r);
        }
        SemanticExprKind::Unary { expr, .. } => rw_expr(expr, r),
        SemanticExprKind::Binary { lhs, rhs, .. } => {
            rw_expr(lhs, r);
            rw_expr(rhs, r);
        }
        SemanticExprKind::Cast { expr, from, to } => {
            rw_expr(expr, r);
            *from = r.ty(from);
            *to = r.ty(to);
        }
        SemanticExprKind::ArrayLit { elements } => {
            for el in elements {
                rw_expr(el, r);
            }
        }
        SemanticExprKind::Index { target, index, .. } => {
            rw_expr(target, r);
            rw_expr(index, r);
        }
        SemanticExprKind::MethodCall { args, struct_name, receiver_expr, .. } => {
            // An expression receiver is a subexpression like any other and may
            // itself contain a generic call, so it must be walked.
            if let Some(rx) = receiver_expr {
                rw_expr(rx, r);
            }
            // Slice 2 territory: substituting the receiver's type-parameter name
            // to a concrete struct is what makes `mangle_method` produce a name
            // the signature table already holds. Doing the substitution here is
            // harmless now (a non-parameter name is unchanged) and is the hook
            // slice 2 needs.
            rw_type_name(struct_name, r);
            for a in args {
                rw_call_arg(a, r);
            }
        }
        SemanticExprKind::StructInstance { type_name: tn, type_args, fields } => {
            rw_type_name(tn, r);
            for a in type_args.iter_mut() {
                *a = r.ty(a);
            }
            for (_, fe) in fields {
                rw_expr(fe, r);
            }
        }
        SemanticExprKind::When { expr, arms, .. } => {
            rw_expr(expr, r);
            for arm in arms {
                rw_when_arm(arm, r);
            }
        }
        SemanticExprKind::If { condition, then_body, else_body, .. } => {
            rw_expr(condition, r);
            rw_stmts(then_body, r);
            rw_stmts(else_body, r);
        }
        SemanticExprKind::ResultOk { expr } => rw_expr(expr, r),
        SemanticExprKind::ResultErr { expr } => rw_expr(expr, r),
        SemanticExprKind::Try { expr, .. } => rw_expr(expr, r),
    }
}

/// A bare type NAME carried as a `String` (a struct name, or a type-parameter
/// name standing in for one). Substituting it keeps `DotAccess`/`MethodCall`/
/// `StructInstance` consistent with the types around them.
fn rw_type_name(name: &mut String, r: &mut Rewriter) {
    if let Some(SemanticType::Struct(s)) = r.subst.get(name.as_str()) {
        *name = s.clone();
    }
}

fn rw_call_arg(a: &mut SemanticCallArg, r: &mut Rewriter) {
    match a {
        SemanticCallArg::Expr(e) => rw_expr(e, r),
        SemanticCallArg::Copy { .. } => {}
        SemanticCallArg::CopyFree { .. } => {}
        SemanticCallArg::CopyInto(_) => {}
    }
}

fn rw_when_arm(arm: &mut SemanticWhenArm, r: &mut Rewriter) {
    if let Some(g) = &mut arm.guard {
        rw_expr(g, r);
    }
    rw_stmts(&mut arm.body, r);
}

fn rw_lvalue(lv: &mut SemanticLValue, r: &mut Rewriter) {
    match lv {
        SemanticLValue::Binding { ty, .. } => *ty = r.ty(ty),
        SemanticLValue::DotAccess { ty, struct_name, .. } => {
            *ty = r.ty(ty);
            rw_type_name(struct_name, r);
        }
        SemanticLValue::Index { target, index, elem_ty } => {
            rw_expr(target, r);
            rw_expr(index, r);
            *elem_ty = r.ty(elem_ty);
        }
    }
}

fn rw_stmts(stmts: &mut [SemanticStmt], r: &mut Rewriter) {
    for s in stmts {
        rw_stmt(s, r);
    }
}

fn rw_function(f: &mut SemanticFunction, r: &mut Rewriter) {
    for p in &mut f.params {
        if let Some(t) = &mut p.ty {
            *t = r.ty(t);
        }
    }
    if let Some(rt) = &mut f.return_ty {
        *rt = r.ty(rt);
    }
    rw_stmts(&mut f.body, r);
    if let Some(re) = &mut f.ret_expr {
        rw_expr(re, r);
    }
}

fn rw_stmt(s: &mut SemanticStmt, r: &mut Rewriter) {
    match s {
        SemanticStmt::Noop => {}
        SemanticStmt::EnumDef { .. } => {}
        SemanticStmt::GeneDef { .. } => {}
        SemanticStmt::PhenDef { receiver_type, methods, method_receiver_params, .. } => {
            *receiver_type = r.ty(receiver_type);
            for m in methods {
                rw_function(m, r);
            }
            for params in method_receiver_params {
                for p in params {
                    if let Some(t) = &mut p.ty {
                        *t = r.ty(t);
                    }
                }
            }
        }
        SemanticStmt::Decl { ty, .. } => {
            if let Some(t) = ty {
                *t = r.ty(t);
            }
        }
        SemanticStmt::Assign { target, expr, .. } => {
            rw_lvalue(target, r);
            rw_expr(expr, r);
        }
        SemanticStmt::TypedAssign { ty, expr, .. } => {
            *ty = r.ty(ty);
            rw_expr(expr, r);
        }
        SemanticStmt::CompoundAssign { target, operand, .. } => {
            rw_lvalue(target, r);
            rw_expr(operand, r);
        }
        SemanticStmt::ExprStmt { expr, .. } => rw_expr(expr, r),
        SemanticStmt::Return { expr, .. } => {
            if let Some(e) = expr {
                rw_expr(e, r);
            }
        }
        SemanticStmt::FuncDef(f) => rw_function(f, r),
        SemanticStmt::Block { stmts, .. } => rw_stmts(stmts, r),
        SemanticStmt::While { cond, body, .. } => {
            rw_expr(cond, r);
            rw_stmts(body, r);
        }
        SemanticStmt::For { start, end, body, .. } => {
            rw_expr(start, r);
            rw_expr(end, r);
            rw_stmts(body, r);
        }
        SemanticStmt::Loop { body, .. } => rw_stmts(body, r),
        SemanticStmt::Break { .. } => {}
        SemanticStmt::Continue { .. } => {}
        SemanticStmt::IfElse { condition, then_body, else_ifs, else_body, .. } => {
            rw_expr(condition, r);
            rw_stmts(then_body, r);
            for (c, b) in else_ifs {
                rw_expr(c, r);
                rw_stmts(b, r);
            }
            if let Some(b) = else_body {
                rw_stmts(b, r);
            }
        }
        SemanticStmt::WhileIn { range_start, range_end, body, then_chains, result, .. } => {
            rw_expr(range_start, r);
            rw_expr(range_end, r);
            rw_stmts(body, r);
            for chain in then_chains {
                rw_expr(&mut chain.range_start, r);
                rw_expr(&mut chain.range_end, r);
                rw_stmts(&mut chain.body, r);
            }
            if let Some(e) = result {
                rw_expr(e, r);
            }
        }
        SemanticStmt::When { expr, arms, .. } => {
            rw_expr(expr, r);
            for arm in arms {
                rw_when_arm(arm, r);
            }
        }
        SemanticStmt::StructDef { fields, .. } => {
            for (_, t) in fields {
                *t = r.ty(t);
            }
        }
        SemanticStmt::ImplBlock { aliases, methods, method_alias_params, .. } => {
            for (_, t) in aliases {
                *t = r.ty(t);
            }
            for m in methods {
                rw_function(m, r);
            }
            for params in method_alias_params {
                for p in params {
                    if let Some(t) = &mut p.ty {
                        *t = r.ty(t);
                    }
                }
            }
        }
        SemanticStmt::ConstDecl { ty, value, .. } => {
            *ty = r.ty(ty);
            rw_expr(value, r);
        }
    }
}

// ── The pass ─────────────────────────────────────────────────────────────────

/// Replace every generic call with a call to a concrete specialization, and
/// append those specializations to the program.
///
/// Returns the program unchanged when it declares no generic functions, so the
/// overwhelmingly common case pays one scan and nothing else.
pub fn monomorphize(program: &SemanticProgram) -> Result<SemanticProgram, MonomorphizeError> {
    let mut templates: HashMap<FunctionId, SemanticFunction> = HashMap::new();
    let mut defined: HashSet<String> = HashSet::new();
    for stmt in &program.stmts {
        match stmt {
            SemanticStmt::FuncDef(f) => {
                defined.insert(f.name.clone());
                if !f.type_params.is_empty() {
                    templates.insert(f.id, f.clone());
                }
            }
            SemanticStmt::PhenDef { receiver_type: SemanticType::Struct(tn), methods, .. } => {
                for m in methods {
                    defined.insert(format!("{}${}", tn, m.name));
                }
            }
            SemanticStmt::ImplBlock { aliases, methods, .. } => {
                if let Some((_, SemanticType::Struct(tn))) = aliases.first() {
                    for m in methods {
                        defined.insert(format!("{}${}", tn, m.name));
                    }
                }
            }
            _ => {}
        }
    }
    if templates.is_empty() {
        return Ok(program.clone());
    }

    let mut stmts = program.stmts.clone();

    // Seeding is the same rewrite with an empty substitution: top-level calls
    // already carry concrete arguments, and their callees need rewriting to the
    // specialization names just as inner calls do.
    let mut r = Rewriter { subst: Subst::new(), templates: &templates, demands: Vec::new() };
    for s in &mut stmts {
        // A template's own body is not rewritten in place — it is cloned per
        // instantiation below, and the template itself is dropped at emission.
        if matches!(s, SemanticStmt::FuncDef(f) if !f.type_params.is_empty()) {
            continue;
        }
        rw_stmt(s, &mut r);
    }

    let mut queue: VecDeque<Demand> = r.demands.drain(..).collect();
    let mut done: HashSet<(FunctionId, Vec<SemanticType>)> = HashSet::new();
    let mut per_template: HashMap<FunctionId, Vec<String>> = HashMap::new();
    let mut specialized: Vec<SemanticStmt> = Vec::new();

    while let Some(d) = queue.pop_front() {
        let key = (d.function, d.args.clone());
        if !done.insert(key) {
            continue;
        }
        let seen = per_template.entry(d.function).or_default();
        seen.push(mangle_specialization(&d.template, &d.args));
        if seen.len() > MAX_INSTANTIATIONS_PER_TEMPLATE {
            return Err(MonomorphizeError::InstantiationExplosion {
                template: d.template,
                cap: MAX_INSTANTIATIONS_PER_TEMPLATE,
                seen: seen.iter().take(4).cloned().collect(),
            });
        }

        let template = &templates[&d.function];
        let mangled = mangle_specialization(&template.name, &d.args);
        if defined.contains(&mangled) {
            return Err(MonomorphizeError::NameCollision {
                mangled,
                template: template.name.clone(),
            });
        }

        let subst: Subst = template
            .type_params
            .iter()
            .cloned()
            .zip(d.args.iter().cloned())
            .collect();

        let mut spec = template.clone();
        spec.name = mangled.clone();
        spec.type_params.clear();
        let mut sr = Rewriter { subst, templates: &templates, demands: Vec::new() };
        rw_function(&mut spec, &mut sr);
        queue.extend(sr.demands);

        defined.insert(mangled);
        specialized.push(SemanticStmt::FuncDef(spec));
    }

    // Specializations go before the statements that call them; ordering within
    // the statement list does not matter to lowering (functions are collected
    // in one pass), but keeping them first makes an IR dump readable.
    let mut out = specialized;
    out.extend(stmts);
    Ok(SemanticProgram { stmts: out, enums: program.enums.clone() })
}
