//! While a method runs, its receiver's variable is reached only through the
//! receiver — checked here, over the whole analyzed program.
//!
//! A struct or array receiver given a variable by name *is* that variable
//! (docs/backend/cx_value_and_alias_semantics.md, V2). If the method, or
//! anything it calls, then names the variable itself, there are two ways to
//! the same storage and the program has no defined result: the interpreter
//! runs a method on a copy of its receiver and stores it back at the end, so
//! such a read sees the value from before the call and such a write is
//! overwritten; compiled code cannot reach a top-level struct from a function
//! at all. The interpreter stops at run time when it happens and compiled
//! code refuses the program (rule V5).
//!
//! By the owner's decision of 2026-11-20 the conflict is found here, before
//! the program runs on either backend (docs/backend/cx_decisions_2026_11_20.md,
//! D6). The rule is static and deliberately conservative:
//!
//! 1. For every function and method, the set of variables declared **outside
//!    every function** that its body names — or that anything it can call
//!    names. Only such a variable can be reached by a method called on it: a
//!    function's local is invisible to every method.
//! 2. A method call whose receiver variable — the one it is made on, or a
//!    struct or array variable given by name for a further receiver — is in
//!    the method's set is an error.
//!
//! "Can call" follows names, not values: a call to a function, a method call
//! on a value of a known struct type, an operator on one. A path that is
//! never taken still counts, and a read counts as much as a write.
//!
//! A method call on a value whose type is a type parameter (`t.touch()` in
//! `fnc <T: G> call(t: T)`) is the method of whatever struct a call gives
//! that parameter, so a generic function is followed once for each such
//! struct it is reached with. Where a call does not say — the parameter is
//! left unbound, or bound to a generic struct's field, whose type is not
//! tracked — it is taken to be every method of that name: conservative, but
//! the two checks behind this rule (the interpreter's at run time, compiled
//! code's refusal) do not cover every case, so they are not relied on.

use std::collections::{HashMap, HashSet};

use crate::frontend::semantic::SemanticError;
use crate::frontend::semantic_types::*;

/// One function or method body.
#[derive(Default)]
struct Unit {
    /// Outer variables this body names itself.
    names: HashSet<BindingId>,
    /// What this body calls.
    calls: Vec<Callee>,
    /// How many leading parameters of a method call on this unit are
    /// receivers after the first (the one the call is made on).
    further_receivers: usize,
    /// The body's own type parameters. A method call on a value of one is
    /// resolved by what a call gives it.
    type_params: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Callee {
    /// A function, and what the call gives each of its type parameters, in
    /// their declared order.
    Function(FunctionId, Vec<TypeArg>),
    FunctionNamed(String),
    /// `(struct name, method name)`. In a generic function the first may be
    /// the name of one of its type parameters instead.
    Method(String, String),
}

/// What a call gives a type parameter, as far as method calls on a value of
/// that type go: methods belong to structs.
#[derive(Clone, PartialEq, Eq, Hash)]
enum TypeArg {
    Struct(String),
    /// The calling function's own type parameter of this name.
    Param(String),
    /// A type no method of the program is called on, or one that is not
    /// known.
    Other,
}

/// A body, and the struct each of its type parameters is — `None` for one
/// that is not a struct, or not known.
type Instance = (usize, Vec<Option<String>>);

/// How many instances of bodies the rule follows with their type parameters
/// bound before it follows each further body once with them unbound.
const INSTANCE_LIMIT: usize = 2048;

/// A written method call, wherever it stands.
struct MethodCallSite {
    instance: String,
    instance_binding: BindingId,
    struct_name: String,
    method: String,
    /// The leading arguments that are a struct or array variable given by name.
    named_aggregate_args: Vec<(usize, BindingId, String)>,
    pos: usize,
}

struct Walk<'a> {
    outer: &'a HashSet<BindingId>,
    /// Names the body being walked declares itself; a `{name}` in a string
    /// that is one of these is the local, not an outer variable.
    own_names: HashSet<String>,
    lexical_names: Option<&'a HashMap<String, BindingId>>,
    names: HashSet<BindingId>,
    calls: Vec<Callee>,
    sites: Vec<MethodCallSite>,
    /// Function bodies met on the way, to be walked as units of their own.
    nested: Vec<Nested<'a>>,
}

/// A function or method declared inside the code being walked.
struct Nested<'a> {
    function: &'a SemanticFunction,
    /// The struct types this is a method of; empty for a function.
    method_of: Vec<String>,
    is_method: bool,
    further_receivers: usize,
    /// The names of a method's receivers, which are locals of its body. They
    /// are not in the method's own parameter list.
    receiver_names: Vec<String>,
}

impl<'a> Walk<'a> {
    fn new(outer: &'a HashSet<BindingId>, lexical_names: Option<&'a HashMap<String, BindingId>>) -> Self {
        Walk {
            outer,
            own_names: HashSet::new(),
            lexical_names,
            names: HashSet::new(),
            calls: Vec::new(),
            sites: Vec::new(),
            nested: Vec::new(),
        }
    }

    fn name(&mut self, binding: BindingId) {
        if self.outer.contains(&binding) {
            self.names.insert(binding);
        }
    }

    /// `"… {top} …"` names `top` when the string is expanded.
    fn string(&mut self, text: &str) {
        let Some(lexical) = self.lexical_names else { return };
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else { break };
            let inside = after[..close].trim();
            let name = inside.strip_suffix(":?").unwrap_or(inside).trim();
            if !self.own_names.contains(name) {
                if let Some(binding) = lexical.get(name) {
                    self.name(*binding);
                }
            }
            rest = &after[close + 1..];
        }
    }

    fn value(&mut self, value: &SemanticValue) {
        if let SemanticValue::Str(text) = value {
            self.string(text);
        }
    }

    fn stmts(&mut self, stmts: &'a [SemanticStmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    fn lvalue(&mut self, target: &'a SemanticLValue) {
        match target {
            SemanticLValue::Binding { binding, .. } => self.name(*binding),
            SemanticLValue::DotAccess { binding, .. } => {
                if let Some(binding) = binding {
                    self.name(*binding);
                }
            }
            SemanticLValue::Index { target, index, .. } => {
                self.expr(target);
                self.expr(index);
            }
        }
    }

    fn stmt(&mut self, stmt: &'a SemanticStmt) {
        match stmt {
            SemanticStmt::Noop
            | SemanticStmt::ModuleNames { .. }
            | SemanticStmt::EnumDef { .. }
            | SemanticStmt::GeneDef { .. }
            | SemanticStmt::StructDef { .. }
            | SemanticStmt::Break { .. }
            | SemanticStmt::Continue { .. } => {}
            SemanticStmt::Decl { name, .. } => {
                self.own_names.insert(name.clone());
            }
            SemanticStmt::TypedAssign { name, expr, .. } => {
                self.expr(expr);
                self.own_names.insert(name.clone());
            }
            SemanticStmt::ConstDecl { name, value, .. } => {
                self.expr(value);
                self.own_names.insert(name.clone());
            }
            SemanticStmt::Assign { target, expr, .. } => {
                self.lvalue(target);
                self.expr(expr);
            }
            SemanticStmt::CompoundAssign { target, operand, .. } => {
                self.lvalue(target);
                self.expr(operand);
            }
            SemanticStmt::ExprStmt { expr, .. } => self.expr(expr),
            SemanticStmt::Return { expr, .. } => {
                if let Some(expr) = expr {
                    self.expr(expr);
                }
            }
            SemanticStmt::FuncDef(function) => self.nested.push(Nested {
                function,
                method_of: Vec::new(),
                is_method: false,
                further_receivers: 0,
                receiver_names: Vec::new(),
            }),
            SemanticStmt::ImplBlock { aliases, methods, method_alias_params, .. } => {
                // A method is registered for every struct among the block's
                // receivers, as analysis registers it.
                let method_of: Vec<String> = aliases
                    .iter()
                    .filter_map(|(_, ty)| match ty {
                        SemanticType::Struct(name) => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                for (index, function) in methods.iter().enumerate() {
                    self.nested.push(Nested {
                        function,
                        method_of: method_of.clone(),
                        is_method: true,
                        further_receivers: aliases.len().saturating_sub(1),
                        receiver_names: method_alias_params
                            .get(index)
                            .map(|params| params.iter().map(|p| p.name.clone()).collect())
                            .unwrap_or_default(),
                    });
                }
            }
            SemanticStmt::PhenDef { receiver_type, methods, method_receiver_params, .. } => {
                let method_of: Vec<String> = match receiver_type {
                    SemanticType::Struct(name) => vec![name.clone()],
                    _ => Vec::new(),
                };
                for (index, function) in methods.iter().enumerate() {
                    self.nested.push(Nested {
                        function,
                        method_of: method_of.clone(),
                        is_method: true,
                        further_receivers: 0,
                        receiver_names: method_receiver_params
                            .get(index)
                            .map(|params| params.iter().map(|p| p.name.clone()).collect())
                            .unwrap_or_default(),
                    });
                }
            }
            SemanticStmt::Block { stmts, .. } | SemanticStmt::Loop { body: stmts, .. } => self.scoped(stmts, None),
            SemanticStmt::While { cond, body, .. } => {
                self.expr(cond);
                self.scoped(body, None);
            }
            SemanticStmt::For { var, start, end, body, .. } => {
                self.expr(start);
                self.expr(end);
                self.scoped(body, Some(var));
            }
            SemanticStmt::IfElse { condition, then_body, else_ifs, else_body, .. } => {
                self.expr(condition);
                self.scoped(then_body, None);
                for (condition, body) in else_ifs {
                    self.expr(condition);
                    self.scoped(body, None);
                }
                if let Some(body) = else_body {
                    self.scoped(body, None);
                }
            }
            SemanticStmt::WhileIn { arr, range_start, range_end, body, then_chains, result, .. } => {
                self.named(arr);
                self.expr(range_start);
                self.expr(range_end);
                self.scoped(body, None);
                for chain in then_chains {
                    self.named(&chain.arr);
                    self.expr(&chain.range_start);
                    self.expr(&chain.range_end);
                    self.scoped(&chain.body, None);
                }
                if let Some(result) = result {
                    self.expr(result);
                }
            }
            SemanticStmt::When { expr, arms, .. } => {
                self.expr(expr);
                self.arms(arms);
            }
        }
    }

    /// A nested body: the names it declares — and `local`, a loop variable —
    /// are its own, and end with it. Kept for the rest of the function, a
    /// block's `vv` hid the module's `vv` named in a `"{vv}"` after the block.
    fn scoped(&mut self, stmts: &'a [SemanticStmt], local: Option<&String>) {
        let outside = self.own_names.clone();
        if let Some(name) = local {
            self.own_names.insert(name.clone());
        }
        self.stmts(stmts);
        self.own_names = outside;
    }

    /// A variable referred to by its name alone, as `while … in arr` does.
    fn named(&mut self, name: &str) {
        if self.own_names.contains(name) {
            return;
        }
        if let Some(binding) = self.lexical_names.and_then(|names| names.get(name)) {
            self.name(*binding);
        }
    }

    fn arms(&mut self, arms: &'a [SemanticWhenArm]) {
        for arm in arms {
            let outside = self.own_names.clone();
            match &arm.pattern {
                SemanticWhenPattern::Literal(value) => self.value(value),
                SemanticWhenPattern::Range(low, high, _) => {
                    self.value(low);
                    self.value(high);
                }
                SemanticWhenPattern::EnumVariant { binding, .. } => {
                    if let Some((_, name)) = binding {
                        self.own_names.insert(name.clone());
                    }
                }
                SemanticWhenPattern::Catchall => {}
            }
            if let Some(guard) = &arm.guard {
                self.expr(guard);
            }
            self.stmts(&arm.body);
            self.own_names = outside;
        }
    }

    fn args(&mut self, args: &'a [SemanticCallArg]) {
        for arg in args {
            match arg {
                SemanticCallArg::Expr(expr) => self.expr(expr),
                SemanticCallArg::Copy { binding, .. } | SemanticCallArg::CopyFree { binding, .. } => self.name(*binding),
                SemanticCallArg::CopyInto(bindings) => {
                    for resolved in bindings {
                        self.name(resolved.binding);
                    }
                }
            }
        }
    }

    fn expr(&mut self, expr: &'a SemanticExpr) {
        match &expr.kind {
            SemanticExprKind::Value(value) => self.value(value),
            SemanticExprKind::VarRef { binding, .. }
            | SemanticExprKind::HandleVal { binding, .. }
            | SemanticExprKind::HandleDrop { binding, .. } => self.name(*binding),
            SemanticExprKind::DotAccess { binding, .. } => {
                if let Some(binding) = binding {
                    self.name(*binding);
                }
            }
            SemanticExprKind::HandleNew { value: inner, .. }
            | SemanticExprKind::Unary { expr: inner, .. }
            | SemanticExprKind::Cast { expr: inner, .. }
            | SemanticExprKind::ResultOk { expr: inner }
            | SemanticExprKind::ResultErr { expr: inner }
            | SemanticExprKind::Try { expr: inner, .. } => self.expr(inner),
            SemanticExprKind::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
            }
            SemanticExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            SemanticExprKind::Index { target, index, .. } => {
                self.expr(target);
                self.expr(index);
            }
            SemanticExprKind::ArrayLit { elements } => {
                for element in elements {
                    self.expr(element);
                }
            }
            SemanticExprKind::StructInstance { fields, .. } => {
                for (_, field) in fields {
                    self.expr(field);
                }
            }
            SemanticExprKind::Call { callee, function, args, type_args, .. } => {
                self.args(args);
                self.calls.push(if *function == FunctionId(u32::MAX) {
                    Callee::FunctionNamed(callee.clone())
                } else {
                    let type_args = type_args
                        .iter()
                        .map(|ty| match ty {
                            SemanticType::Struct(name) => TypeArg::Struct(name.clone()),
                            SemanticType::TypeParam(name) => TypeArg::Param(name.clone()),
                            _ => TypeArg::Other,
                        })
                        .collect();
                    Callee::Function(*function, type_args)
                });
            }
            SemanticExprKind::MethodCall { instance, method, args, instance_binding, struct_name, receiver_expr, pos, .. } => {
                match receiver_expr {
                    // An operator: its operand is evaluated here, as a value.
                    Some(receiver) => self.expr(receiver),
                    // A written call names the variable it is made on, and
                    // lends it to the method.
                    None => {
                        self.name(*instance_binding);
                        let named_aggregate_args = args
                            .iter()
                            .enumerate()
                            .filter_map(|(index, arg)| match arg {
                                SemanticCallArg::Expr(SemanticExpr { ty, kind: SemanticExprKind::VarRef { binding, name } })
                                    if matches!(ty, SemanticType::Struct(_) | SemanticType::Array(..)) =>
                                {
                                    Some((index, *binding, name.clone()))
                                }
                                _ => None,
                            })
                            .collect();
                        self.sites.push(MethodCallSite {
                            instance: instance.clone(),
                            instance_binding: *instance_binding,
                            struct_name: struct_name.clone(),
                            method: method.clone(),
                            named_aggregate_args,
                            pos: *pos,
                        });
                    }
                }
                self.args(args);
                self.calls.push(Callee::Method(struct_name.clone(), method.clone()));
            }
            SemanticExprKind::When { expr, arms, .. } => {
                self.expr(expr);
                self.arms(arms);
            }
            SemanticExprKind::If { condition, then_body, else_body, .. } => {
                self.expr(condition);
                self.scoped(then_body, None);
                self.scoped(else_body, None);
            }
        }
    }
}

/// The variables declared outside every function: at the top level of a
/// module, or in a block, loop or branch there.
fn outer_bindings(stmts: &[SemanticStmt], into: &mut HashSet<BindingId>) {
    for stmt in stmts {
        match stmt {
            SemanticStmt::Decl { binding, .. }
            | SemanticStmt::TypedAssign { binding, .. }
            | SemanticStmt::ConstDecl { binding, .. } => {
                into.insert(*binding);
            }
            SemanticStmt::Block { stmts, .. } | SemanticStmt::Loop { body: stmts, .. } | SemanticStmt::While { body: stmts, .. } => {
                outer_bindings(stmts, into);
            }
            SemanticStmt::For { binding, body, .. } => {
                into.insert(*binding);
                outer_bindings(body, into);
            }
            SemanticStmt::IfElse { then_body, else_ifs, else_body, .. } => {
                outer_bindings(then_body, into);
                for (_, body) in else_ifs {
                    outer_bindings(body, into);
                }
                if let Some(body) = else_body {
                    outer_bindings(body, into);
                }
            }
            SemanticStmt::WhileIn { body, then_chains, .. } => {
                outer_bindings(body, into);
                for chain in then_chains {
                    outer_bindings(&chain.body, into);
                }
            }
            SemanticStmt::When { arms, .. } => {
                for arm in arms {
                    outer_bindings(&arm.body, into);
                }
            }
            // A function's own declarations are not outside every function,
            // and nothing else declares a variable.
            _ => {}
        }
    }
}

/// Where each function and method body is among the units.
#[derive(Default)]
struct UnitIndex {
    by_function: HashMap<FunctionId, usize>,
    by_function_name: HashMap<String, usize>,
    by_method: HashMap<(String, String), Vec<usize>>,
    by_method_name: HashMap<String, Vec<usize>>,
}

impl UnitIndex {
    /// Give each newly found body a unit, and queue it to be walked.
    fn register<'a>(&mut self, found: Vec<Nested<'a>>, units: &mut Vec<Unit>, queue: &mut Vec<(Nested<'a>, usize)>) {
        for nested in found {
            if self.by_function.contains_key(&nested.function.id) {
                continue;
            }
            let unit = units.len();
            units.push(Unit {
                further_receivers: nested.further_receivers,
                type_params: nested.function.type_params.clone(),
                ..Unit::default()
            });
            self.by_function.insert(nested.function.id, unit);
            if nested.is_method {
                self.by_method_name.entry(nested.function.name.clone()).or_default().push(unit);
                for struct_name in &nested.method_of {
                    self.by_method.entry((struct_name.clone(), nested.function.name.clone())).or_default().push(unit);
                }
            } else {
                self.by_function_name.entry(nested.function.name.clone()).or_insert(unit);
            }
            queue.push((nested, unit));
        }
    }
}

/// Every analysis error of rule D6 in the program, in source order.
pub(crate) fn check(stmts: &[SemanticStmt]) -> Vec<SemanticError> {
    let mut outer = HashSet::new();
    outer_bindings(stmts, &mut outer);
    if outer.is_empty() {
        return Vec::new();
    }

    let mut units: Vec<Unit> = Vec::new();
    let mut index = UnitIndex::default();
    let mut sites: Vec<MethodCallSite> = Vec::new();

    // Walk the top level, then every function and method body, wherever it
    // is declared.
    let mut top = Walk::new(&outer, None);
    top.stmts(stmts);
    sites.append(&mut top.sites);
    let mut queue: Vec<(Nested, usize)> = Vec::new();
    index.register(top.nested, &mut units, &mut queue);
    while let Some((nested, unit)) = queue.pop() {
        let function = nested.function;
        let mut walk = Walk::new(&outer, Some(&function.lexical_names));
        walk.own_names.extend(function.params.iter().map(|p| p.name.clone()));
        walk.own_names.extend(nested.receiver_names.iter().cloned());
        walk.stmts(&function.body);
        if let Some(ret) = &function.ret_expr {
            walk.expr(ret);
        }
        units[unit].names = walk.names;
        units[unit].calls = walk.calls;
        sites.append(&mut walk.sites);
        index.register(walk.nested, &mut units, &mut queue);
    }
    let UnitIndex { by_function, by_function_name, by_method, by_method_name } = index;

    // What a body calls, given the struct each of its type parameters is.
    let unbound = |unit: usize| -> Instance { (unit, vec![None; units[unit].type_params.len()]) };
    let methods = |struct_name: &str, method: &str| -> Vec<usize> {
        by_method.get(&(struct_name.to_string(), method.to_string())).cloned().unwrap_or_default()
    };
    let callees = |(unit, bound): &Instance| -> Vec<Instance> {
        let params = &units[*unit].type_params;
        let bound_to = |name: &str| params.iter().position(|p| p == name).and_then(|i| bound.get(i).cloned().flatten());
        let mut out = Vec::new();
        for callee in &units[*unit].calls {
            match callee {
                Callee::Function(id, type_args) => {
                    let Some(&target) = by_function.get(id) else { continue };
                    let wanted = units[target].type_params.len();
                    // A call that leaves a parameter unbound records fewer
                    // types than there are parameters, and which is which is
                    // then not known.
                    let known = type_args.len() == wanted;
                    let target_bound = (0..wanted)
                        .map(|i| match type_args.get(i).filter(|_| known) {
                            Some(TypeArg::Struct(name)) => Some(name.clone()),
                            Some(TypeArg::Param(name)) => bound_to(name),
                            _ => None,
                        })
                        .collect();
                    out.push((target, target_bound));
                }
                Callee::FunctionNamed(name) => out.extend(by_function_name.get(name).map(|&target| unbound(target))),
                Callee::Method(type_name, method) => {
                    // On a value of one of this body's type parameters, the
                    // method of the struct that parameter is here. Where that
                    // is not known — the call left it unbound, or bound it to
                    // something that is not a struct — every method of the
                    // name: the run-time check and compiled code's refusal
                    // behind this rule do not cover every case.
                    let targets = if params.iter().any(|p| p == type_name) {
                        match bound_to(type_name) {
                            Some(struct_name) => methods(&struct_name, method),
                            None => by_method_name.get(method).cloned().unwrap_or_default(),
                        }
                    } else {
                        methods(type_name, method)
                    };
                    out.extend(targets.into_iter().map(unbound));
                }
            }
        }
        out
    };

    // Every body a written method call can reach, with the types it is
    // reached with, and what each of those names: its own names and those of
    // everything it calls.
    let mut ids: HashMap<Instance, usize> = HashMap::new();
    let mut instances: Vec<Instance> = Vec::new();
    for site in &sites {
        for target in methods(&site.struct_name, &site.method) {
            let root = unbound(target);
            if !ids.contains_key(&root) {
                ids.insert(root.clone(), instances.len());
                instances.push(root);
            }
        }
    }
    let mut edges: Vec<Vec<usize>> = Vec::new();
    let mut next = 0;
    while next < instances.len() {
        let mut out = Vec::new();
        for mut callee in callees(&instances[next]) {
            // Past the limit a body is followed once, with every type
            // parameter taken as not known — which reaches at least what any
            // of its instances reaches. A generic function that calls itself
            // with its type arguments permuted made the instances grow
            // factorially (10 parameters: 53 s and 4.7 GB).
            if instances.len() >= INSTANCE_LIMIT && !ids.contains_key(&callee) {
                callee = unbound(callee.0);
            }
            let id = match ids.get(&callee) {
                Some(&id) => id,
                None => {
                    ids.insert(callee.clone(), instances.len());
                    instances.push(callee);
                    instances.len() - 1
                }
            };
            out.push(id);
        }
        edges.push(out);
        next += 1;
    }
    let mut reach: Vec<HashSet<BindingId>> = instances.iter().map(|(unit, _)| units[*unit].names.clone()).collect();
    loop {
        let mut changed = false;
        for index in 0..instances.len() {
            for &target in &edges[index] {
                if target == index {
                    continue;
                }
                let add: Vec<BindingId> = reach[target].difference(&reach[index]).copied().collect();
                if !add.is_empty() {
                    reach[index].extend(add);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut errors = Vec::new();
    sites.sort_by_key(|site| site.pos);
    for site in &sites {
        let targets = methods(&site.struct_name, &site.method);
        if targets.is_empty() {
            continue;
        }
        let reached = |binding: BindingId| {
            outer.contains(&binding) && targets.iter().any(|&t| reach[ids[&unbound(t)]].contains(&binding))
        };
        if reached(site.instance_binding) {
            errors.push(SemanticError {
                pos: site.pos,
                msg: format!(
                    "method '{0}.{1}', or something it calls, names '{0}' — the variable it is called on. While a method runs, its receiver's variable can be reached only through the receiver",
                    site.instance, site.method
                ),
            });
            continue;
        }
        let further = targets.iter().map(|&t| units[t].further_receivers).max().unwrap_or(0);
        for (index, binding, name) in &site.named_aggregate_args {
            if *index < further && reached(*binding) {
                errors.push(SemanticError {
                    pos: site.pos,
                    msg: format!(
                        "method '{}.{}', or something it calls, names '{}' — the variable given for one of its receivers. While a method runs, a receiver's variable can be reached only through the receiver",
                        site.instance, site.method, name
                    ),
                });
                break;
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use crate::frontend::{resolver, semantic};
    use std::collections::HashMap;

    /// The semantic errors of a one-file program, joined; empty if it analyzes.
    fn errors(src: &str) -> String {
        let id = resolver::ModuleId(0);
        let file = resolver::ResolvedFile {
            id,
            path: "receiver_access_test.cx".into(),
            program: resolver::parse_source(src).unwrap_or_else(|error| panic!("{error}: {src}")),
            imports: vec![],
        };
        let resolved = resolver::ResolvedProgram { entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id] };
        match semantic::analyze_resolved_program(&resolved) {
            Ok(_) => String::new(),
            Err(errors) => errors.iter().map(|e| e.msg.clone()).collect::<Vec<_>>().join(" | "),
        }
    }

    const HEAD: &str = "struct V { x: t64 }\ntop: V = V { x: 1 }\nother: V = V { x: 2 }\nfnc: t64 bump(v.copy: V) {\n    v.x += 5\n    return v.x\n}\nim: impl (s: V) {\n    fnc: t64 inc() {\n        s.x += 1\n        return s.x\n    }\n}\n";

    const GENERIC: &str = "struct V { x: t64 }\nstruct W { y: t64 }\ngene G { fnc touch() -> t64 }\ntop: V = V { x: 1 }\n\
        fnc: t64 <T: G> call(t: T) { t.touch() }\nfnc: t64 <T: G> outer(t: T) { return call(t) }\n";

    /// A name declared inside a block is that block's own: after it, the
    /// same name in a string is the module's variable again.
    #[test]
    fn a_name_declared_in_a_block_ends_with_the_block() {
        let src = "struct V { x: t64 }\nvv: V = V { x: 1 }\nvm: impl (s: V) {\n    fnc: t64 run() {\n        {\n            vv: t64 = 5\n            s.x += vv\n        }\n        print(\"v={vv}\")\n        return s.x\n    }\n}\nr: t64 = vv.run()\n";
        assert!(errors(src).contains("method 'vv.run', or something it calls, names 'vv'"), "{}", errors(src));
    }

    /// A method call on a type parameter that a call does not determine —
    /// here the generic function's other parameter is unbound, so which is
    /// which is not known — is taken to reach every method of that name.
    #[test]
    fn an_undetermined_method_call_reaches_every_method_of_its_name() {
        let src = format!("{GENERIC}fnc: t64 <T: G, U> call2(t: T) {{ t.touch() }}\nphen G (v: V) {{ fnc: t64 touch() {{ return top.x }} }}\nvm: impl (s: V) {{\n    fnc: t64 run() {{\n        s.x += 10\n        return call2(s)\n    }}\n}}\nr: t64 = top.run()\n");
        assert!(errors(&src).contains("method 'top.run', or something it calls, names 'top'"), "{}", errors(&src));
    }

    /// The bodies followed are bounded: a generic function that calls itself
    /// with its type arguments permuted used to make them grow factorially
    /// (ten type parameters: 53 s and 4.7 GB).
    #[test]
    fn instances_are_bounded() {
        let n = 10;
        let mut src = String::new();
        for i in 0..n {
            src.push_str(&format!("struct S{i} {{ v: t64 }}\n"));
        }
        src.push_str("struct V { x: t64 }\n");
        let params: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
        let args: Vec<String> = (0..n).map(|i| format!("a{i}: T{i}")).collect();
        let swapped: Vec<String> = [1, 0].iter().copied().chain(2..n).map(|i| format!("a{i}")).collect();
        let rotated: Vec<String> = (1..n).chain([0]).map(|i| format!("a{i}")).collect();
        src.push_str(&format!(
            "fnc: t64 <{}> perm(k: t64, {}) {{\n    if k <= 0 {{ return 0 }}\n    one: t64 = perm(k - 1, {})\n    return one + perm(k - 1, {})\n}}\n",
            params.join(", "), args.join(", "), swapped.join(", "), rotated.join(", ")
        ));
        src.push_str("top: V = V { x: 1 }\nvm: impl (s: V) {\n    fnc: t64 run() {\n");
        for i in 0..n {
            src.push_str(&format!("        l{i}: S{i} = S{i} {{ v: {i} }}\n"));
        }
        let locals: Vec<String> = (0..n).map(|i| format!("l{i}")).collect();
        src.push_str(&format!("        return perm(2, {})\n    }}\n}}\nr: t64 = top.run()\n", locals.join(", ")));
        let started = std::time::Instant::now();
        assert_eq!(errors(&src), "");
        assert!(started.elapsed().as_secs() < 20, "took {:?}", started.elapsed());
    }

    /// A call is followed to a function defined further down the file.
    #[test]
    fn a_call_to_a_later_function_is_followed() {
        let src = "struct V { x: t64 }\ntop: V = V { x: 1 }\nvm: impl (s: V) {\n    fnc: t64 run(c: bool) {\n        s.x += 10\n        if c { return peek() }\n        return s.x\n    }\n}\nfnc: t64 peek() { return top.x }\nr: t64 = top.run(false)\n";
        assert!(errors(src).contains("method 'top.run', or something it calls, names 'top'"), "{}", errors(src));
    }

    /// A method call on a value of a type parameter is the method of the
    /// struct each call gives that parameter — not every method of its name.
    #[test]
    fn a_method_on_a_type_parameter_is_resolved_by_what_the_call_gives_it() {
        // `W`'s method names `top`; the method running on `top` reaches `V`'s.
        let other_struct_names_it = "phen G (v: V) { fnc: t64 touch() { return v.x } }\nphen G (w: W) { fnc: t64 touch() { return top.x + w.y } }\n";
        for run in ["return call(s)", "return outer(s)"] {
            let src = format!("{GENERIC}{other_struct_names_it}vm: impl (s: V) {{\n    fnc: t64 run() {{\n        s.x += 10\n        {run}\n    }}\n}}\nr: t64 = top.run()\n");
            assert_eq!(errors(&src), "", "{src}");
        }
        // The method running on `top` reaches `W`'s, which names `top`.
        for run in ["return call(w)", "return outer(w)"] {
            let src = format!("{GENERIC}{other_struct_names_it}vm: impl (s: V) {{\n    fnc: t64 run() {{\n        w: W = W {{ y: 5 }}\n        {run}\n    }}\n}}\nr: t64 = top.run()\n");
            assert!(errors(&src).contains("method 'top.run', or something it calls, names 'top'"), "{src}: {}", errors(&src));
        }
        // `V`'s own method names `top`, and is the one reached.
        let own_names_it = "phen G (v: V) { fnc: t64 touch() { return top.x } }\nphen G (w: W) { fnc: t64 touch() { return w.y } }\n";
        for run in ["return call(s)", "return outer(s)"] {
            let src = format!("{GENERIC}{own_names_it}vm: impl (s: V) {{\n    fnc: t64 run() {{\n        s.x += 10\n        {run}\n    }}\n}}\nr: t64 = top.run()\n");
            assert!(errors(&src).contains("method 'top.run', or something it calls, names 'top'"), "{src}: {}", errors(&src));
        }
    }

    /// A method that can reach the variable it is called on — itself or
    /// through what it calls, on a path taken or not, to read or to write.
    #[test]
    fn a_method_that_can_reach_its_receivers_variable_is_rejected() {
        let called_on = "names 'top' — the variable it is called on";
        for body in [
            "        return top.x",
            "        top.x = 5\n        return 1",
            "        t: V = top\n        return t.x",
            "        top = V { x: 5 }\n        return 1",
            "        return bump(top.copy)",
            "        return top.inc()",
            "        print(\"inside {top}\")\n        return 1",
            "        if s.x > 100 { return top.x }\n        return 1",
        ] {
            let src = format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m() {{\n{body}\n    }}\n}}\nr: t64 = top.m()\n");
            assert!(errors(&src).contains(called_on), "{src}\ngave: {}", errors(&src));
        }
        // Through a function, through two, through another method, and
        // through a nested function.
        for (helpers, body) in [
            ("fnc: t64 poke() {\n    top.x += 1000\n    return 1\n}\n", "        return poke()"),
            ("fnc: t64 poke() {\n    return top.x\n}\nfnc: t64 mid() {\n    return poke()\n}\n", "        return mid()"),
            ("hm: impl (h: V) {\n    fnc: t64 peek() {\n        return top.x\n    }\n}\n", "        return other.peek()"),
            ("", "        fnc: t64 inner() {\n            return top.x\n        }\n        return inner()"),
        ] {
            let src = format!("{HEAD}{helpers}vm: impl (s: V) {{\n    fnc: t64 m() {{\n{body}\n    }}\n}}\nr: t64 = top.m()\n");
            assert!(errors(&src).contains(called_on), "{src}\ngave: {}", errors(&src));
        }
        // A struct given by name for a further receiver.
        let further = format!("{HEAD}vm: impl (a: V, b: V) {{\n    fnc: t64 m() {{\n        return other.x\n    }}\n}}\nr: t64 = top.m(other)\n");
        assert!(errors(&further).contains("names 'other' — the variable given for one of its receivers"), "{}", errors(&further));
        // Inside a function as well as at the top level.
        let in_function = format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m() {{\n        return top.x\n    }}\n}}\nfnc: t64 run() {{\n    return top.m()\n}}\nr: t64 = run()\n");
        assert!(errors(&in_function).contains(called_on), "{}", errors(&in_function));
    }

    /// What is not a conflict: the receiver under any name, another variable,
    /// the caller's own use of the variable, a function's local, an operator.
    #[test]
    fn a_method_may_reach_everything_but_the_variable_it_runs_on() {
        for src in [
            // The receiver has the name of its variable.
            format!("{HEAD}vm: impl (top: V) {{\n    fnc: t64 m() {{\n        top.x += 10\n        return top.inc() + bump(top.copy)\n    }}\n}}\nr: t64 = top.m()\n"),
            // Another variable.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m() {{\n        return s.x + other.x\n    }}\n}}\nr: t64 = top.m()\n"),
            // The method names `top`, but is called on `other`.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m() {{\n        return s.x + top.x\n    }}\n}}\nr: t64 = other.m()\n"),
            // The caller reads the variable for an argument; a copy is passed.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m(k: t64, o: V) {{\n        return s.x + k + o.x\n    }}\n}}\nr: t64 = top.m(top.x, top)\n"),
            // A function's local cannot be named by a method.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m() {{\n        return s.x + other.x\n    }}\n}}\nfnc: t64 run() {{\n    top: V = V {{ x: 9 }}\n    return top.m()\n}}\nr: t64 = run()\n"),
            // A struct given for an ordinary parameter is a copy.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m(o: V) {{\n        return s.x + other.x\n    }}\n}}\nr: t64 = top.m(other)\n"),
        ] {
            assert_eq!(errors(&src), "", "{src}");
        }
    }
}
