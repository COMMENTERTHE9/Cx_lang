//! Type parameters inside a generic function, and what a call must satisfy.
//!
//! The owner's decision of 2026-11-20 on generic type safety
//! (docs/backend/cx_decisions_2026_11_20.md, decision 2; known-issues §81).
//! Analysis checks a generic body once, with `T` unknown, and used to let a
//! type parameter match anything — so `return "not a T"` was accepted from a
//! function declared to return `T`, the interpreter returned the string to a
//! caller that asked for a number, and compiled code refused the program
//! (known-issues §74).
//!
//! Inside the body of a generic function each of its own type parameters is
//! a type of its own:
//!
//! - where `T` is declared, only a `T` fits — checked in the body, whatever
//!   the function is called with;
//! - a value whose type is, or holds, `T` may be used where another type is
//!   declared — `n: t64 = x`, `s: str = x`, a `Result<T>` returned as a
//!   `Result<t64>` — and that is right only for some `T`. So is a number
//!   literal written beside `T` elements in an array literal (`[x, 1]` is a
//!   `[2: T]`, §42).
//!
//! Analysis records each use of the second kind as an [`Obligation`] of the
//! function, and [`check_instantiations`] checks the obligations for every
//! set of concrete types the function is reached with, directly or through
//! other generic functions.
//!
//! The scope itself is thread-local state rather than an argument: the rules
//! that consult it (`types_compatible` and its helpers) are free functions
//! called from several dozen places, and outside a generic function the scope
//! is empty and they behave exactly as before.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::frontend::semantic::{
    mentions_type_param, source_function_name, substitute_type_params, type_name, types_compatible, SemanticError,
};
use crate::frontend::semantic_types::*;

/// Something a generic function's body does that is right only for some types.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Obligation {
    /// A value of type `got` is used where `expected` is declared, and one of
    /// them mentions a type parameter of the function. With the parameters
    /// replaced by what they are at a call, the value must fit by the rule
    /// for concrete types: the same type; any number where a number is
    /// declared (it is converted, §43); a `Result` by the payload rule (§75).
    Fits { expected: SemanticType, got: SemanticType },
    /// A number literal is an element of an array of `param`.
    Literal { param: String, literal: Literal },
}

/// A number literal, as far as fitting a type goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Literal {
    Whole(i128),
    Float,
}

struct Frame {
    params: Vec<String>,
    obligations: Vec<Obligation>,
}

thread_local! {
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    static UNCHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Analyze with type parameters matching anything, as they did before this
/// rule — for the tests of the checks that stand behind it: the
/// interpreter's at run time and compiled code's refusals, which a program
/// can no longer reach through a generic body.
#[cfg(test)]
pub(crate) fn unchecked<R>(analyze: impl FnOnce() -> R) -> R {
    let before = UNCHECKED.with(|flag| flag.replace(true));
    let result = analyze();
    UNCHECKED.with(|flag| flag.set(before));
    result
}

/// Begin analyzing a function with these type parameters (none for a function
/// that is not generic). Paired with [`leave`].
pub(crate) fn enter(params: &[String]) {
    FRAMES.with(|frames| frames.borrow_mut().push(Frame { params: params.to_vec(), obligations: Vec::new() }));
}

/// End the innermost function, returning what its body obliges a call to.
pub(crate) fn leave() -> Vec<Obligation> {
    FRAMES.with(|frames| frames.borrow_mut().pop().map(|frame| frame.obligations).unwrap_or_default())
}

/// Whether `name` is a type parameter of the function being analyzed.
pub(crate) fn is_own(name: &str) -> bool {
    #[cfg(test)]
    if UNCHECKED.with(|flag| flag.get()) {
        return false;
    }
    FRAMES.with(|frames| frames.borrow().last().is_some_and(|frame| frame.params.iter().any(|p| p == name)))
}

/// Record an obligation of the function being analyzed.
pub(crate) fn record(obligation: Obligation) {
    FRAMES.with(|frames| {
        if let Some(frame) = frames.borrow_mut().last_mut() {
            if !frame.obligations.contains(&obligation) {
                frame.obligations.push(obligation);
            }
        }
    });
}

/// A generic function, as far as its calls are concerned.
#[derive(Debug, Clone)]
pub(crate) struct GenericFunction {
    pub name: String,
    pub obligations: Vec<Obligation>,
}

/// A call to a generic function: what each of its type parameters is bound
/// to there. The types may mention the type parameters of `caller`.
#[derive(Debug, Clone)]
pub(crate) struct GenericCall {
    pub caller: Option<FunctionId>,
    pub callee: FunctionId,
    pub bindings: Vec<(String, SemanticType)>,
    pub pos: usize,
}

/// A function may instantiate itself with ever larger types — `g([x])`
/// inside `g` — which has no end. Compiled code refuses such a program at 64
/// instantiations of one function; here, an instantiation with a type nested
/// deeper than this is not followed, and neither is anything past this many
/// instantiations reached from one call.
const DEPTH_LIMIT: usize = 24;
const INSTANTIATION_LIMIT: usize = 4096;

fn depth(ty: &SemanticType) -> usize {
    match ty {
        SemanticType::Array(_, inner) | SemanticType::Result(inner) | SemanticType::Handle(inner) => 1 + depth(inner),
        _ => 0,
    }
}

/// Check every generic function's obligations for every set of concrete types
/// it is reached with — by a call outside every generic function, or through
/// the generic functions such a call reaches. An error is reported at that
/// outermost call, and says which function and types; one obligation is
/// reported once per call, for the first types that break it.
pub(crate) fn check_instantiations(
    functions: &HashMap<FunctionId, GenericFunction>,
    calls: &[GenericCall],
) -> Vec<SemanticError> {
    let mut by_caller: HashMap<FunctionId, Vec<&GenericCall>> = HashMap::new();
    for call in calls {
        if let Some(caller) = call.caller {
            by_caller.entry(caller).or_default().push(call);
        }
    }

    let concrete = |bindings: &[(String, SemanticType)]| {
        bindings.iter().all(|(_, ty)| !mentions_type_param(ty) && depth(ty) <= DEPTH_LIMIT)
    };
    // Earliest call first, and the instantiations a call reaches nearest
    // first, so that what is reported is the first place and the simplest
    // types.
    let mut roots: Vec<&GenericCall> = calls.iter().filter(|call| concrete(&call.bindings)).collect();
    roots.sort_by_key(|call| call.pos);

    // What a call with given types breaks is the same wherever it is written.
    let mut found: HashMap<String, Vec<String>> = HashMap::new();
    let mut errors: Vec<SemanticError> = Vec::new();
    for root in roots {
        let key = format!("{:?} {:?}", root.callee, root.bindings);
        let messages = found.entry(key).or_insert_with(|| {
            let mut messages = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let mut reported: HashSet<(FunctionId, usize)> = HashSet::new();
            // The last field: whether this is the call itself, or something
            // it reaches.
            let mut work: VecDeque<(FunctionId, Vec<(String, SemanticType)>, bool)> =
                VecDeque::from([(root.callee, root.bindings.clone(), true)]);
            while let Some((callee, bindings, direct)) = work.pop_front() {
                if seen.len() >= INSTANTIATION_LIMIT || !seen.insert(format!("{callee:?} {bindings:?}")) {
                    continue;
                }
                let map: HashMap<String, SemanticType> = bindings.iter().cloned().collect();
                if let Some(function) = functions.get(&callee) {
                    for (index, obligation) in function.obligations.iter().enumerate() {
                        if let Some(message) = violation(obligation, &map, &function.name, direct) {
                            if reported.insert((callee, index)) {
                                messages.push(message);
                            }
                        }
                    }
                }
                // The generic calls this function makes, with its own type
                // parameters replaced by what they are here.
                for inner in by_caller.get(&callee).into_iter().flatten() {
                    let composed: Vec<(String, SemanticType)> = inner
                        .bindings
                        .iter()
                        .map(|(name, ty)| (name.clone(), substitute_type_params(ty.clone(), &map)))
                        .collect();
                    if concrete(&composed) {
                        work.push_back((inner.callee, composed, false));
                    }
                }
            }
            messages
        });
        errors.extend(messages.iter().map(|msg| SemanticError { pos: root.pos, msg: msg.clone() }));
    }
    errors
}

/// A type with the type parameters replaced by what they are at a call. A
/// number of no declared width is a t64 there, which is how it is stored.
fn at_call(ty: &SemanticType, bindings: &HashMap<String, SemanticType>) -> SemanticType {
    fn pinned(ty: SemanticType) -> SemanticType {
        match ty {
            SemanticType::Numeric => SemanticType::I64,
            SemanticType::Array(n, inner) => SemanticType::Array(n, Box::new(pinned(*inner))),
            SemanticType::Result(inner) => SemanticType::Result(Box::new(pinned(*inner))),
            SemanticType::Handle(inner) => SemanticType::Handle(Box::new(pinned(*inner))),
            other => other,
        }
    }
    let pinned_bindings: HashMap<String, SemanticType> =
        bindings.iter().map(|(name, ty)| (name.clone(), pinned(ty.clone()))).collect();
    substitute_type_params(ty.clone(), &pinned_bindings)
}

/// The bindings a message names: those of the parameters `types` mention.
fn named_bindings(types: &[&SemanticType], bindings: &HashMap<String, SemanticType>) -> String {
    fn collect(ty: &SemanticType, out: &mut Vec<String>) {
        match ty {
            SemanticType::TypeParam(name) if !out.contains(name) => out.push(name.clone()),
            SemanticType::Array(_, inner) | SemanticType::Result(inner) | SemanticType::Handle(inner) => collect(inner, out),
            _ => {}
        }
    }
    let mut params = Vec::new();
    for ty in types {
        collect(ty, &mut params);
    }
    params
        .iter()
        .filter(|param| bindings.contains_key(*param))
        .map(|param| format!("{param} = {}", type_name(&at_call(&SemanticType::TypeParam(param.clone()), bindings))))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why `obligation` does not hold with these types, if it does not. An
/// obligation on a parameter the call left unbound, or bound to an unknown
/// type, is not checked: nothing is known to check it against. `direct`: the
/// types are those of the call the error is reported at, not of a call that
/// one leads to.
fn violation(
    obligation: &Obligation,
    bindings: &HashMap<String, SemanticType>,
    function: &str,
    direct: bool,
) -> Option<String> {
    let function = source_function_name(function);
    let reached = |with: String| {
        if direct {
            format!("'{function}' is called with {with}")
        } else {
            format!("this call reaches '{function}' with {with}")
        }
    };
    match obligation {
        Obligation::Fits { expected, got } => {
            let (declared, given) = (at_call(expected, bindings), at_call(got, bindings));
            let unsettled = |ty: &SemanticType| mentions_type_param(ty) || *ty == SemanticType::Unknown;
            if unsettled(&declared) || unsettled(&given) || types_compatible(&declared, &given) {
                return None;
            }
            Some(format!(
                "{}, but its body uses a {} value where {} is declared: {} is not {}",
                reached(named_bindings(&[got, expected], bindings)),
                type_name(got),
                type_name(expected),
                type_name(&given),
                type_name(&declared)
            ))
        }
        Obligation::Literal { param, literal } => {
            let bound = at_call(&SemanticType::TypeParam(param.clone()), bindings);
            let problem = match (literal, &bound) {
                (_, SemanticType::Unknown | SemanticType::TypeParam(_)) => return None,
                (Literal::Float, SemanticType::F64) => return None,
                (Literal::Whole(value), ty) if ty.int_width().is_some() => {
                    let facts = ty.int_width().expect("checked by the guard").facts();
                    if *value >= facts.min && *value <= facts.max {
                        return None;
                    }
                    format!("{value} does not fit {} (valid range: {}..{})", facts.name, facts.min, facts.max)
                }
                (Literal::Whole(value), SemanticType::F64) => {
                    format!("{value} is a whole number and {param} is f64 here")
                }
                (Literal::Float, ty) => format!("a float literal is not {}", type_name(ty)),
                (Literal::Whole(value), ty) => format!("{value} is not {}", type_name(ty)),
            };
            Some(format!(
                "{}, but its body writes a number literal as an element of a {param} array: {problem}",
                reached(format!("{param} = {}", type_name(&bound)))
            ))
        }
    }
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
            path: "generic_scope_test.cx".into(),
            program: resolver::parse_source(src).unwrap_or_else(|error| panic!("{error}: {src}")),
            imports: vec![],
        };
        let resolved = resolver::ResolvedProgram { entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id] };
        match semantic::analyze_resolved_program(&resolved) {
            Ok(_) => String::new(),
            Err(errors) => errors.iter().map(|e| e.msg.clone()).collect::<Vec<_>>().join(" | "),
        }
    }

    /// Where `T` is declared only a `T` fits, whatever the function is called
    /// with — a call that gives `T` the very type of the value included.
    #[test]
    fn where_a_type_parameter_is_declared_only_its_own_values_fit() {
        for (body, call, reason) in [
            ("fnc: T <T> f(x: T) { return \"s\" }", "f(\"a\")", "return type mismatch: expected T, got str"),
            ("fnc: T <T> f(x: T) { return 5 }", "f(5)", "return type mismatch: expected T, got numeric literal"),
            ("fnc: T <T> f(x: T) {\n y: T = 5\n return y\n}", "f(5)", "type mismatch: expected T, got numeric literal"),
            ("fnc: T <T> f(x: T) {\n y: T = x\n y = true\n return y\n}", "f(true)", "type mismatch: expected T, got bool"),
            ("fnc: T <T> f(x: T) {\n n: t64 = 1\n return n\n}", "f(5)", "return type mismatch: expected T, got t64"),
            ("fnc: U <T, U> f(x: T, y: U) {\n z: U = x\n return z\n}", "f(1, 2)", "type mismatch: expected U, got T"),
            ("fnc: U <T, U> f(x: T, y: U) { return x }", "f(1, 2)", "return type mismatch: expected U, got T"),
            ("fnc: T <T> p(a: T, b: T) { return b }\nfnc: T <T> f(x: T) { return p(x, 5) }", "f(5)", "type parameter 'T' is bound to T but argument 2 has numeric literal"),
            ("fnc: Result<T> <T> f(x: T) { return Ok(5) }", "f(5)", "return type mismatch: expected Result<T>, got Result<integer>"),
            ("fnc: T <T> f(x: T) { return Ok(x) }", "f(5)", "return type mismatch: expected T, got Result<T>"),
            ("fnc: [2: T] <T> f(x: T) {\n n: t64 = 1\n return [n, n]\n}", "f(5)", "return type mismatch: expected [2: T], got [2: t64]"),
            ("struct P { v: t64 }\nfnc: T <T> f(x: T) {\n p: P = P { v: 1 }\n return p\n}", "f(P { v: 2 })", "return type mismatch: expected T, got P"),
            ("fnc: T <T> f(c: bool, x: T) { return if c { x } else { 300 } }", "f(true, 1)", "a branch of this if/when yields numeric literal, where T is declared"),
            ("fnc: T <T> f(k: t64, x: T) { return when k { 0 => x, _ => \"s\" } }", "f(0, \"a\")", "a branch of this if/when yields str, where T is declared"),
            ("fnc: five() { 300 }\nfnc: T <T> f(x: T) {\n y: T = five()\n return y\n}", "f(1)", "type mismatch: expected T, got void"),
            ("fnc: T <T> f(x: T, n: t64) {\n if n == 0 { return x }\n return f(300, n - 1)\n}", "f(1, 0)", "return type mismatch: expected T, got numeric literal"),
        ] {
            let got = errors(&format!("{body}\nprint({call})\n"));
            assert!(got.contains(reason), "{body}: {got}");
        }
    }

    /// What needs nothing of `T` is right for every `T`.
    #[test]
    fn what_a_body_does_with_its_own_values_is_accepted() {
        assert_eq!(
            errors(
                "struct P { v: t64 }\n\
                 fnc: T <T> id(x: T) { return x }\n\
                 fnc: T <T> twice(x: T) { return id(id(x)) }\n\
                 fnc: T <T> keep(x: T) {\n y: T = x\n y = x\n return y\n}\n\
                 fnc: T <T> pick(c: bool, a: T, b: T) { return if c { a } else { b } }\n\
                 fnc: [2: T] <T> pair(x: T) { return [x, x] }\n\
                 fnc: Result<T> <T> wrap(x: T) { return Ok(x) }\n\
                 fnc: Result<T> <T> fail(x: T) { return Err(\"no\") }\n\
                 fnc: Result<T> <T> pass(r: Result<T>) {\n v: T = r?\n return Ok(v)\n}\n\
                 fnc: T <T> swap(a: T, b: T) { a = b\n return a }\n\
                 a: t8 = 5\n\
                 print(twice(a))\nprint(keep(\"s\"))\nprint(pick(true, a, 9))\n\
                 xs: [2: t8] = pair(a)\nprint(wrap(P { v: 1 }))\nprint(fail(a))\n\
                 r: Result<t64> = Ok(7)\nprint(pass(r))\nprint(swap(Err(\"x\"), Err(\"y\")))\nprint(id(?))\n"
            ),
            ""
        );
    }

    /// A `T` value where another type is declared is checked for each call:
    /// the same type, any number where a number is declared, a `Result` by
    /// the payload rule.
    #[test]
    fn a_value_used_at_another_type_is_checked_for_each_call() {
        let head = "enum Phase { Idle, Active }\nenum Other { First, Second }\nstruct P { v: t64 }\n\
                    fnc: t64 <T> num(x: T) {\n n: t64 = x\n return n\n}\n\
                    fnc: f64 <T> real(x: T) { return x }\n\
                    fnc: str <T> text(x: T) {\n s: str = x\n return s\n}\n\
                    fnc: Phase <T> phase(x: T) { x }\n\
                    fnc: t64 <T> field(x: T) {\n p: P = P { v: x }\n return p.v\n}\n\
                    fnc: Result<t8> <T> conv(r: Result<T>) { return r }\n\
                    fnc: Result<t8> <T> made(x: T) { return Ok(x) }\n\
                    fnc: t64 <T> either(c: bool, x: T) {\n n: t64 = if c { 1 } else { x }\n return n\n}\n\
                    a: t8 = 5\nr8: Result<t8> = Ok(1)\nr64: Result<t64> = Ok(300)\nw: t64 = 300\n";
        for fine in [
            "num(a)", "num(300)", "num(2.5)", "real(a)", "real(2.5)", "text(\"s\")", "phase(Phase::Active)", "field(a)",
            "conv(r8)", "conv(Err(\"x\"))", "made(a)", "either(false, a)", "num(?)",
        ] {
            assert_eq!(errors(&format!("{head}print({fine})\n")), "", "{fine}");
        }
        for (call, reason) in [
            ("num(\"s\")", "'num' is called with T = str, but its body uses a T value where t64 is declared: str is not t64"),
            ("num(true)", "bool is not t64"),
            ("num(Phase::Idle)", "Phase is not t64"),
            ("real(\"s\")", "str is not f64"),
            ("text(5)", "'text' is called with T = t64, but its body uses a T value where str is declared: t64 is not str"),
            ("text(a)", "t8 is not str"),
            ("phase(Other::First)", "Other is not Phase"),
            ("phase(a)", "t8 is not Phase"),
            ("field(true)", "bool is not t64"),
            ("conv(r64)", "'conv' is called with T = t64, but its body uses a Result<T> value where Result<t8> is declared: Result<t64> is not Result<t8>"),
            ("made(w)", "Result<t64> is not Result<t8>"),
            ("either(false, \"s\")", "str is not t64"),
        ] {
            let got = errors(&format!("{head}print({call})\n"));
            assert!(got.contains(reason), "{call}: {got}");
        }
    }

    /// A number literal beside `T` elements is checked for each call.
    #[test]
    fn a_literal_element_is_checked_for_each_call() {
        let head = "fnc: T <T> second(x: T) {\n c: [2: T] = [x, 34]\n return c:[1]\n}\n\
                    fnc: T <T> big(x: T) {\n c: [2: T] = [x, 349]\n return c:[1]\n}\n\
                    fnc: T <T> low(x: T) {\n c: [2: T] = [-129, x]\n return c:[0]\n}\n\
                    fnc: T <T> half(x: T) {\n c: [2: T] = [x, 2.5]\n return c:[1]\n}\n\
                    fnc: T <T> rows(x: T) {\n g: [2: [2: T]] = [[x, 900], [8, x]]\n return g:[1]:[0]\n}\n\
                    a: t8 = 5\nw: t64 = 5\nf: f64 = 1.5\n";
        for fine in ["second(a)", "second(w)", "second(7)", "big(w)", "low(w)", "half(f)", "rows(w)"] {
            assert_eq!(errors(&format!("{head}print({fine})\n")), "", "{fine}");
        }
        for (call, reason) in [
            ("big(a)", "'big' is called with T = t8, but its body writes a number literal as an element of a T array: 349 does not fit t8 (valid range: -128..127)"),
            ("low(a)", "-129 does not fit t8"),
            ("rows(a)", "900 does not fit t8"),
            ("second(f)", "34 is a whole number and T is f64 here"),
            ("half(w)", "a float literal is not t64"),
            ("second(\"s\")", "34 is not str"),
            ("second(true)", "34 is not bool"),
        ] {
            let got = errors(&format!("{head}print({call})\n"));
            assert!(got.contains(reason), "{call}: {got}");
        }
    }

    /// The check follows a call through the generic functions it reaches, is
    /// reported at the call outside them, once, and ends for a function that
    /// instantiates itself with ever larger types.
    #[test]
    fn a_call_is_followed_through_the_generic_functions_it_reaches() {
        let chain = "fnc: t64 <T> g(x: T) {\n n: t64 = x\n return n\n}\n\
                     fnc: t64 <T> h(x: T) { return g(x) }\n\
                     fnc: t64 <T, U> k(x: T, y: U) { return h(y) }\n";
        assert_eq!(errors(&format!("{chain}print(k(\"s\", 5))\n")), "");
        assert_eq!(
            errors(&format!("{chain}print(k(5, \"s\"))\nprint(k(5, \"t\"))\n")),
            "this call reaches 'g' with T = str, but its body uses a T value where t64 is declared: str is not t64 | \
             this call reaches 'g' with T = str, but its body uses a T value where t64 is declared: str is not t64"
        );
        // Never called with concrete types: nothing to check it against.
        assert_eq!(errors(&format!("{chain}fnc: t64 <T> unused(x: T) {{ return k(x, x) }}\n")), "");
        // `g` reaches itself with `[1: t64]`, `[1: [1: t64]]`, …: reported
        // once, for the first.
        let grows = "fnc: t64 <T> g(x: T, n: t64) {\n if n == 0 {\n m: t64 = x\n return m\n }\n return g([x], n - 1)\n}\nprint(g(5, 0))\n";
        assert_eq!(
            errors(grows),
            "this call reaches 'g' with T = [1: t64], but its body uses a T value where t64 is declared: [1: t64] is not t64"
        );
        assert_eq!(errors("fnc: t64 <T> g(x: T) { g([x]) }\nprint(g(1))\n"), "");
    }

    /// A generic struct's type parameter is not the function's: struct type
    /// arguments are not tracked, and a field's value is still unchecked.
    #[test]
    fn a_generic_struct_field_is_still_unchecked() {
        assert_eq!(
            errors(
                "struct Box<T> { v: T }\n\
                 fnc: T <T> open(x: T, b: Box) {\n y: T = b.v\n return y\n}\n\
                 fnc: t64 <T> count(x: T, b: Box) {\n n: t64 = b.v\n return n\n}\n\
                 b: Box = Box { v: 5 }\nprint(open(1, b))\nprint(count(1, b))\n"
            ),
            ""
        );
    }
}
