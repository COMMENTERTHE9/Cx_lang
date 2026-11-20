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
//! other generic functions — up to a limit per function, past which the
//! program is marked as not fully checked and compiled code refuses it.
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
    /// Number literals are elements of an array of `param`: the smallest and
    /// largest whole number among them, and whether one is a float. (One
    /// obligation per type parameter, however many literals: a function with
    /// hundreds of them was checked hundreds of times per instantiation.)
    Literals { param: String, low: Option<i128>, high: Option<i128>, float: bool },
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

/// Record that a number literal is an element of an array of `param`, in the
/// function being analyzed.
pub(crate) fn record_literal(param: &str, literal: Literal) {
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let Some(frame) = frames.last_mut() else { return };
        let existing = frame.obligations.iter_mut().find_map(|o| match o {
            Obligation::Literals { param: p, low, high, float } if p == param => Some((low, high, float)),
            _ => None,
        });
        let (low, high, float) = match existing {
            Some(found) => found,
            None => {
                frame.obligations.push(Obligation::Literals { param: param.to_string(), low: None, high: None, float: false });
                match frame.obligations.last_mut() {
                    Some(Obligation::Literals { low, high, float, .. }) => (low, high, float),
                    _ => unreachable!("just pushed"),
                }
            }
        };
        match literal {
            Literal::Float => *float = true,
            Literal::Whole(value) => {
                *low = Some(low.map_or(value, |l| l.min(value)));
                *high = Some(high.map_or(value, |h| h.max(value)));
            }
        }
    });
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
    pub type_params: Vec<String>,
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

/// One generic function is checked for at most this many sets of types. A
/// self-instantiating function — `g([x])` inside `g` — has no end of them.

/// Past the limit an instantiation is not checked, and the program says so
/// (`SemanticProgram::unchecked_generics`): compiled code refuses it, and the
/// interpreter runs it with its run-time checks. The limit is above compiled
/// code's own (64 instantiations of one function), which refuses such a
/// program in any case when the instantiations are real.
const PER_FUNCTION_LIMIT: usize = 128;

/// A rejected program is reported for at most this many calls that reach
/// something broken.
const REPORTED_CALLS: usize = 32;

/// Nor is a set of types nested deeper than this: a self-instantiating
/// function nests its types one level more each time, and every step of the
/// check walks them. Compiled code takes time exponential in the nesting of an
/// array literal (known-issues §82 f) long before this depth.
const DEPTH_LIMIT: usize = 32;

fn depth(ty: &SemanticType) -> usize {
    match ty {
        SemanticType::Array(_, inner) | SemanticType::Result(inner) | SemanticType::Handle(inner) => 1 + depth(inner),
        _ => 0,
    }
}

/// A type as a key: a number of no declared width is a t64.
fn pinned(ty: SemanticType) -> SemanticType {
    match ty {
        SemanticType::Numeric => SemanticType::I64,
        SemanticType::Array(n, inner) => SemanticType::Array(n, Box::new(pinned(*inner))),
        SemanticType::Result(inner) => SemanticType::Result(Box::new(pinned(*inner))),
        SemanticType::Handle(inner) => SemanticType::Handle(Box::new(pinned(*inner))),
        other => other,
    }
}

/// One generic function with its type parameters bound, and what it breaks.
struct Instance {
    callee: FunctionId,
    bindings: HashMap<String, SemanticType>,
    /// The instances it calls; worked out when first needed.
    calls: Option<Vec<usize>>,
    /// The obligations it breaks; the messages are made when reported.
    broken: Vec<usize>,
}

/// Every instance met, shared by all the calls checked: what a function
/// breaks with given types, and what it calls with them, is worked out once.
struct Instances<'a> {
    functions: &'a HashMap<FunctionId, GenericFunction>,
    by_caller: HashMap<FunctionId, Vec<&'a GenericCall>>,
    ids: HashMap<String, usize>,
    all: Vec<Instance>,
    per_function: HashMap<FunctionId, usize>,
    /// The first function an instantiation of which was not checked.
    unchecked: Option<String>,
    /// Whether what is not checked now is recorded: while following calls
    /// that compiled code may compile. A call written in a generic function's
    /// body is followed again through that function's instances whenever
    /// compiled code compiles it; followed on its own, it is checked as far
    /// as the limits allow, and what is left costs nothing.
    recording: bool,
}

impl<'a> Instances<'a> {
    fn concrete(bindings: &[(String, SemanticType)]) -> bool {
        bindings.iter().all(|(_, ty)| !mentions_type_param(ty))
    }

    fn record(&mut self, what: String) {
        if self.recording && self.unchecked.is_none() {
            self.unchecked = Some(what);
        }
    }

    fn name(&self, callee: FunctionId) -> String {
        self.functions.get(&callee).map(|f| source_function_name(&f.name).to_string()).unwrap_or_default()
    }

    /// Whether these bindings leave a type parameter of `callee` unbound.
    fn partial(&self, callee: FunctionId, bindings: &[(String, SemanticType)]) -> bool {
        self.functions.get(&callee).is_some_and(|f| f.type_params.iter().any(|p| !bindings.iter().any(|(name, _)| name == p)))
    }

    /// The instance of `callee` with these types, made if it is new and the
    /// limit allows; if it does not, that is recorded.
    fn get(&mut self, callee: FunctionId, bindings: &[(String, SemanticType)]) -> Option<usize> {
        let bindings: Vec<(String, SemanticType)> =
            bindings.iter().map(|(name, ty)| (name.clone(), pinned(ty.clone()))).collect();
        let key = format!("{callee:?} {bindings:?}");
        if let Some(&id) = self.ids.get(&key) {
            return Some(id);
        }
        let count = self.per_function.get(&callee).copied().unwrap_or(0);
        let too_deep = bindings.iter().any(|(_, ty)| depth(ty) > DEPTH_LIMIT);
        if count >= PER_FUNCTION_LIMIT || too_deep {
            let name = self.name(callee);
            self.record(if too_deep {
                format!("generic function '{name}' is reached with types nested more than {DEPTH_LIMIT} deep")
            } else {
                format!("generic function '{name}' is reached with more than {PER_FUNCTION_LIMIT} sets of types")
            });
            return None;
        }
        // A call that leaves a type parameter unbound is checked for what its
        // bound ones need, and not recorded: compiled code cannot compile one
        // at all. It emits only the specialisations of a generic function,
        // never the function itself, and does not specialise such a call
        // (`monomorphize`), so the call names nothing it emits.
        let map: HashMap<String, SemanticType> = bindings.into_iter().collect();
        let mut broken = Vec::new();
        if let Some(function) = self.functions.get(&callee) {
            for (index, obligation) in function.obligations.iter().enumerate() {
                if violation(obligation, &map, &function.name, true).is_some() {
                    broken.push(index);
                }
            }
        }
        let id = self.all.len();
        self.all.push(Instance { callee, bindings: map, calls: None, broken });
        self.ids.insert(key, id);
        self.per_function.insert(callee, count + 1);
        Some(id)
    }

    /// A call whose types, once the caller's are settled, still name a type
    /// parameter analysis does not track — a generic struct's, or one a callee
    /// leaves unbound: it cannot be checked.
    fn untracked(&mut self, callee: FunctionId) {
        let name = self.name(callee);
        self.record(format!("generic function '{name}' is called with a type analysis does not track (a generic struct's field)"));
    }

    /// The generic calls an instance makes, with its own type parameters
    /// replaced by what they are there.
    fn calls(&mut self, id: usize) -> Vec<usize> {
        if let Some(calls) = &self.all[id].calls {
            return calls.clone();
        }
        let callee = self.all[id].callee;
        let inner_calls: Vec<&'a GenericCall> = self.by_caller.get(&callee).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for inner in inner_calls {
            let composed: Vec<(String, SemanticType)> = inner
                .bindings
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type_params(ty.clone(), &self.all[id].bindings)))
                .collect();
            if Self::concrete(&composed) {
                if let Some(target) = self.get(inner.callee, &composed) {
                    if !out.contains(&target) {
                        out.push(target);
                    }
                }
            } else if !self.partial(self.all[id].callee, &self.all[id].bindings.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()) {
                // (In an instance that leaves one of its own type parameters
                // unbound — one compiled code cannot compile — a call naming
                // it is not a type analysis fails to track.)
                self.untracked(inner.callee);
            }
        }
        self.all[id].calls = Some(out.clone());
        out
    }

    /// Make every instance reachable from `start`.
    fn expand(&mut self, start: usize) {
        let mut work = vec![start];
        let mut visited: HashSet<usize> = HashSet::from([start]);
        while let Some(id) = work.pop() {
            for next in self.calls(id) {
                if visited.insert(next) {
                    work.push(next);
                }
            }
        }
    }
}

/// The strongly connected components of the instance graph, each a list of
/// instances, in an order in which a component comes after every component it
/// calls into (Tarjan's algorithm, without recursion).
fn components(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut next_index = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, position in its edge list)
        let mut frames: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (node, ref mut at)) = frames.last_mut() {
            if *at < edges[node].len() {
                let next = edges[node][*at];
                *at += 1;
                if index[next] == usize::MAX {
                    index[next] = next_index;
                    low[next] = next_index;
                    next_index += 1;
                    stack.push(next);
                    on_stack[next] = true;
                    frames.push((next, 0));
                } else if on_stack[next] {
                    low[node] = low[node].min(index[next]);
                }
            } else {
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == index[node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    out.push(component);
                }
            }
        }
    }
    out
}

/// Check every generic function's obligations for every set of concrete types
/// it is reached with — by a call written with concrete types, wherever it is
/// written, and through the generic functions such a call reaches. An error
/// is reported at that call, and says which function and types; one
/// obligation is reported once per call. Also returns what was left
/// unchecked, if the limit was met.
pub(crate) fn check_instantiations(
    functions: &HashMap<FunctionId, GenericFunction>,
    calls: &[GenericCall],
) -> (Vec<SemanticError>, Option<String>) {
    let mut by_caller: HashMap<FunctionId, Vec<&GenericCall>> = HashMap::new();
    for call in calls {
        if let Some(caller) = call.caller {
            by_caller.entry(caller).or_default().push(call);
        }
    }
    let mut instances = Instances {
        functions,
        by_caller,
        ids: HashMap::new(),
        all: Vec::new(),
        per_function: HashMap::new(),
        unchecked: None,
        recording: true,
    };

    // A call written outside every generic function's body — at top level,
    // in a plain function or method, in a plain function nested in a generic
    // one — is one compiled code may compile directly: what is not checked
    // from it is recorded. Those first, then calls written in generic bodies;
    // each in source order.
    let direct = |call: &GenericCall| call.caller.is_none_or(|caller| !functions.contains_key(&caller));
    let mut roots: Vec<&GenericCall> = calls.iter().collect();
    roots.sort_by_key(|call| (!direct(call), call.pos));
    let mut starts: Vec<(usize, usize)> = Vec::new();
    for root in &roots {
        instances.recording = direct(root);
        if !Instances::concrete(&root.bindings) {
            // Only a type parameter analysis does not track can remain in a
            // call written outside a generic body.
            if instances.recording {
                instances.untracked(root.callee);
            }
            continue;
        }
        if let Some(start) = instances.get(root.callee, &root.bindings) {
            instances.expand(start);
            starts.push((root.pos, start));
        }
    }

    // Which broken obligations each instance reaches: one bit per (function,
    // obligation) that some instance breaks, worked out once per strongly
    // connected component, callees first.
    let n = instances.all.len();
    let edges: Vec<Vec<usize>> = (0..n).map(|id| instances.all[id].calls.clone().unwrap_or_default()).collect();
    let mut bit_of: HashMap<(FunctionId, usize), usize> = HashMap::new();
    for inst in &instances.all {
        for index in &inst.broken {
            let next = bit_of.len();
            bit_of.entry((inst.callee, *index)).or_insert(next);
        }
    }
    let words = bit_of.len().div_ceil(64);
    let mut errors: Vec<SemanticError> = Vec::new();
    if words > 0 {
        let comps = components(&edges);
        let mut component_of = vec![0; n];
        for (c, members) in comps.iter().enumerate() {
            for &m in members {
                component_of[m] = c;
            }
        }
        let mut reach: Vec<Vec<u64>> = vec![vec![0; words]; comps.len()];
        for (c, members) in comps.iter().enumerate() {
            let mut bits = vec![0u64; words];
            for &m in members {
                for index in &instances.all[m].broken {
                    let bit = bit_of[&(instances.all[m].callee, *index)];
                    bits[bit / 64] |= 1 << (bit % 64);
                }
                for &next in &edges[m] {
                    let other = component_of[next];
                    if other != c {
                        for (word, theirs) in bits.iter_mut().zip(&reach[other]) {
                            *word |= theirs;
                        }
                    }
                }
            }
            reach[c] = bits;
        }

        // For a call that reaches something broken, the messages: its own,
        // then the nearest instance that breaks each other obligation. A
        // rejected program is reported for its first few such calls.
        let mut reported_calls = 0;
        let mut found: HashMap<usize, Vec<String>> = HashMap::new();
        for (pos, start) in starts {
            if reach[component_of[start]].iter().all(|w| *w == 0) {
                continue;
            }
            if !found.contains_key(&start) {
                if reported_calls >= REPORTED_CALLS {
                    break;
                }
                reported_calls += 1;
                let wanted = &reach[component_of[start]];
                let mut messages = Vec::new();
                let mut reported: HashSet<usize> = HashSet::new();
                let mut visited: HashSet<usize> = HashSet::from([start]);
                let mut work: VecDeque<usize> = VecDeque::from([start]);
                while let Some(id) = work.pop_front() {
                    let callee = instances.all[id].callee;
                    let function = &functions[&callee];
                    for index in &instances.all[id].broken {
                        let bit = bit_of[&(callee, *index)];
                        if wanted[bit / 64] & (1 << (bit % 64)) != 0 && reported.insert(bit) {
                            if let Some(message) =
                                violation(&function.obligations[*index], &instances.all[id].bindings, &function.name, id == start)
                            {
                                messages.push(message);
                            }
                        }
                    }
                    for &next in &edges[id] {
                        if visited.insert(next) {
                            work.push_back(next);
                        }
                    }
                }
                found.insert(start, messages);
            }
            errors.extend(found[&start].iter().map(|msg| SemanticError { pos, msg: msg.clone() }));
        }
    }
    errors.sort_by_key(|e| e.pos);
    (errors, instances.unchecked)
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
        Obligation::Literals { param, low, high, float } => {
            let bound = at_call(&SemanticType::TypeParam(param.clone()), bindings);
            let whole = low.or(*high);
            let problem = match &bound {
                SemanticType::Unknown | SemanticType::TypeParam(_) => return None,
                SemanticType::F64 => match whole {
                    None => return None,
                    Some(value) => format!("{value} is a whole number and {param} is f64 here"),
                },
                ty if ty.int_width().is_some() => {
                    let facts = ty.int_width().expect("checked by the guard").facts();
                    match (low, high) {
                        (Some(value), _) if *value < facts.min => {
                            format!("{value} does not fit {} (valid range: {}..{})", facts.name, facts.min, facts.max)
                        }
                        (_, Some(value)) if *value > facts.max => {
                            format!("{value} does not fit {} (valid range: {}..{})", facts.name, facts.min, facts.max)
                        }
                        _ if *float => format!("a float literal is not {}", type_name(ty)),
                        _ => return None,
                    }
                }
                ty => match whole {
                    Some(value) => format!("{value} is not {}", type_name(ty)),
                    None => format!("a float literal is not {}", type_name(ty)),
                },
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

    /// Holes the review of the implementation found in rule 1, and the
    /// messages it found misleading.
    #[test]
    fn the_branches_inference_and_names_of_a_generic_body() {
        for (body, call, reason) in [
            // An if/when value whose branches differ, wherever it is used.
            ("fnc: [2: T] <T> f(c: bool, x: T) { return [x, if c { x } else { 300 }] }", "f(false, 1)", "a branch of this if/when yields numeric literal, and another yields T"),
            ("fnc: T <T> f(c: bool, x: T) {\n let y\n y = if c { x } else { 300 }\n return y\n}", "f(false, 1)", "a branch of this if/when yields numeric literal, and another yields T"),
            ("fnc: T <T> f(c: bool, x: T) {\n let y\n y = if c { 300 } else { x }\n return y\n}", "f(false, 1)", "and another yields T"),
            ("fnc: [2: T] <T> f(x: T) {\n xs: [2: T] = [x, when 1 { 0 => x, _ => 300 }]\n return xs\n}", "f(1)", "and another yields T"),
            // A callee's type parameter bound to a number, then given a `T`.
            ("fnc: T <T> pick(a: T, b: T) { return b }\nfnc: f64 <T> f(x: T) { return pick(5, x) }", "f(2.5)", "type parameter 'T' of 'pick' is bound to a number literal by an earlier argument, but argument 2 is a value of type parameter T"),
            // A variable known only by inference is as good as declared: the
            // branches are checked against what it holds.
            ("fnc: T <T> f(c: bool, x: T) {\n let y\n y = x\n y = if c { x } else { 300 }\n return y\n}", "f(false, 5)", "a branch of this if/when yields numeric literal, where T is declared"),
            // With nothing declared for the value, its branches must agree.
            ("fnc: t64 <T> f(c: bool, x: T) {\n print(if c { x } else { 300 })\n return 1\n}", "f(true, 1)", "and another yields T"),
            // `T { .. }` and a nested function using the enclosing `T`.
            ("fnc: T <T> f(x: T) { return T { v: 1 } }", "f(5)", "'T' is a type parameter of this function, not a struct"),
            ("fnc: T <T> f(x: T) {\n fnc: T inner(y: T) { return y }\n return inner(x)\n}", "f(5)", "nested function 'inner' uses 'T', a type parameter of the function it is declared in"),
        ] {
            let got = errors(&format!("{body}\nprint({call})\n"));
            assert!(got.contains(reason), "{body}: {got}");
        }
        for fine in [
            "fnc: T <T> f(c: bool, x: T) { return if c { x } else { x } }\nprint(f(true, 1))\n",
            "fnc: Result<T> <T> f(c: bool, x: T) { return if c { Ok(x) } else { Err(\"no\") } }\nprint(f(true, 1))\n",
            "fnc: T <T> f(k: t64, a: T, b: T) { return when k { 0 => a, _ => b } }\nprint(f(0, 1, 2))\n",
            "fnc: T <T> pick(a: T, b: T) { return b }\nfnc: T <T> f(x: T) { return pick(x, x) }\nprint(f(2))\n",
            "fnc: Result<T> <T> pick(a: Result<T>, b: Result<T>) { return b }\nfnc: Result<T> <T> f(x: T) { return pick(Err(\"e\"), Ok(x)) }\nprint(f(2))\n",
            "fnc: T <T> outer(x: T) {\n fnc: U <U> inner(y: U) { return y }\n return inner(x)\n}\nprint(outer(5))\n",
            // A declared type for the value: each branch is checked against
            // it; a concrete binding then a `T`, and a `?` then a `T`, are
            // checked for each call.
            "fnc: t8 <T> f(c: bool, x: T) { return if c { x } else { 7 } }\nw: t64 = 300\nprint(f(true, w))\n",
            "fnc: str <T> f(c: bool, x: T) {\n s: str = if c { x } else { \"lit\" }\n return s\n}\nprint(f(true, \"a\"))\n",
            "fnc: T <T> pick(a: T, b: T) { return b }\nfnc: f64 <T> f(x: T) {\n n: f64 = 0.0\n return pick(n, x)\n}\nprint(f(2.5))\n",
            "fnc: T <T> pick(a: T, b: T) { return b }\nfnc: T <T> f(x: T) { return pick(?, x) }\nprint(f(2))\n",
            // An `Err` first, then the value that settles it.
            "fnc: T <T> pick(a: T, b: T) { return b }\nfnc: Result<T> <T> f(x: T) { return pick(Err(\"e\"), Ok(x)) }\nprint(f(2))\n",
            // A callee's unbound type parameter is not the caller's `T`,
            // however it is spelled.
            "fnc: Result<T> <T> fail(n: t64) { return Err(\"no\") }\nfnc: t64 <T> g(x: T) {\n r: Result<t64> = fail(1)\n print(r)\n return 1\n}\nprint(g(\"s\"))\n",
        ] {
            assert_eq!(errors(fine), "", "{fine}");
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
                    a: t8 = 5\nr8: Result<t8> = Ok(1)\nr64: Result<t64> = Ok(300)\nw: t64 = 300\n";
        for fine in [
            "num(a)", "num(300)", "num(2.5)", "real(a)", "real(2.5)", "text(\"s\")", "phase(Phase::Active)", "field(a)",
            "conv(r8)", "conv(Err(\"x\"))", "made(a)", "num(?)",
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
        ] {
            let got = errors(&format!("{head}print({call})\n"));
            assert!(got.contains(reason), "{call}: {got}");
        }
        // Where a type is declared for an if/when value, each branch is
        // checked against it — a `T` branch for each call, as `n: t64 = x` is.
        let either = "fnc: t64 <T> either(c: bool, x: T) {\n n: t64 = if c { 1 } else { x }\n return n\n}\n";
        assert_eq!(errors(&format!("{either}a: t8 = 5\nprint(either(false, a))\n")), "");
        assert!(errors(&format!("{either}print(either(false, \"s\"))\n")).contains("str is not t64"));
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
        // A call written before the function it calls is defined is followed
        // too, from a plain function and from a generic one.
        let later = "fnc: T <T> second(x: T) {\n c: [2: T] = [x, 349]\n return c:[1]\n}\n";
        for caller in ["fnc: t8 big(a: t8) { return second(a) }\n", "fnc: T <T> outer(x: T) { return second(x) }\n"] {
            let call = if caller.contains("big") { "v: t8 = 5\nprint(big(v))\n" } else { "v: t8 = 5\nprint(outer(v))\n" };
            let got = errors(&format!("{caller}{later}{call}"));
            assert!(got.contains("349 does not fit t8"), "{caller}: {got}");
        }
    }

    /// The work is bounded, and shared between calls: one function with
    /// hundreds of calls to itself at ever larger types, and hundreds of calls
    /// with distinct types to a function that instantiates itself, took
    /// 11 s and 25 s.
    #[test]
    fn the_work_is_bounded() {
        let mut grows = String::from("fnc: t64 <T> g(k: t64, x: T) {\n    if k <= 0 { return 0 }\n    s: t64 = 0\n");
        for _ in 0..300 {
            grows.push_str("    s = s + g(k - 1, [x])\n    s = s + g(k - 1, Ok(x))\n");
        }
        grows.push_str("    return s\n}\nprint(g(1, 5))\n");
        let mut many = String::from("fnc: t64 <T> h(k: t64, x: T) {\n    if k <= 0 { return 0 }\n    return h(k - 1, [x]) + h(k - 1, Ok(x)) + h(k - 1, [[x]])\n}\n");
        for i in 0..300 {
            many.push_str(&format!("struct S{i} {{ v: t64 }}\nprint(h(1, S{i} {{ v: {i} }}))\n"));
        }
        // Hundreds of literals beside `T` elements, in ten functions that
        // instantiate one another: one obligation per function, and the
        // messages made only for what is reported (was 12 s).
        let literals: Vec<String> = (1000..1200).map(|n| n.to_string()).collect();
        let mut chain = String::new();
        for i in 0..10 {
            chain.push_str(&format!("fnc: t64 <T> g{i}(x: T) {{\n c: [201: T] = [x, {}]\n", literals.join(", ")));
            for j in 0..10 {
                chain.push_str(&format!(" print(g{j}([x]))\n print(g{j}(Ok(x)))\n"));
            }
            chain.push_str(" return 1\n}\n");
        }
        chain.push_str("print(g0(1))\n");
        let started = std::time::Instant::now();
        assert!(errors(&chain).contains("is not [1: t64]") || errors(&chain).contains("does not fit"), "{}", errors(&chain));
        assert!(started.elapsed().as_secs() < 20, "took {:?}", started.elapsed());
        for src in [grows, many] {
            let started = std::time::Instant::now();
            assert_eq!(errors(&src), "");
            assert!(started.elapsed().as_secs() < 20, "took {:?}", started.elapsed());
        }
    }

    /// Where the check stops short, compiled code must refuse: its limit per
    /// function has to stay below this one. And the calls compiled code
    /// instantiates are checked first, however many calls sit in generic
    /// functions nothing calls — they used to use up the budget, and the
    /// instance compiled code did make went unchecked.
    #[test]
    fn the_limit_stays_above_compiled_codes() {
        assert!(super::PER_FUNCTION_LIMIT > crate::ir::monomorphize::MAX_INSTANTIATIONS_PER_TEMPLATE);
        let mut src = String::from("fnc: T <T, U> f(x: T, u: U) {\n c: [2: T] = [x, 34]\n return c:[1]\n}\nfnc: T <T> g(x: T) { return f(x, 0) }\nfnc: t64 <Q> unused(q: Q) {\n");
        // 200 distinct types for `U`: arrays of 1 to 200 elements.
        for n in 1..=200 {
            src.push_str(&format!(" print(f(1, [{}]))\n", vec!["0"; n].join(", ")));
        }
        src.push_str(" return 1\n}\nh: f64 = 1.5\ny: f64 = g(h)\nprint(y)\n");
        assert!(errors(&src).contains("34 is a whole number and T is f64 here"), "{}", errors(&src));
    }

    /// Where the check stops short of an instantiation compiled code may
    /// compile, the program says so, and compiled code refuses it. Here 130
    /// calls with distinct types use up `f`'s budget before the last call;
    /// a version that recorded nothing let compiled code compile such a
    /// program unchecked (the interpreter held 34 where compiled code held
    /// 34.0, when the budget went on instances compiled code never makes).
    #[test]
    fn a_program_not_fully_checked_is_refused_by_compiled_code() {
        // 130 calls with distinct types, after a `return`: never lowered.
        let mut src = String::from("fnc: T <T, U> f(x: T, u: U) {\n c: [2: T] = [x, 34]\n return c:[1]\n}\nfnc: t64 dead() {\n return 1\n");
        for n in 1..=130 {
            src.push_str(&format!(" print(f(1, [{}]))\n", vec!["0"; n].join(", ")));
        }
        src.push_str("}\nh: f64 = 1.5\nwhen f(h, 0) { 34 => print(\"int\"), _ => print(\"other\") }\n");
        let id = resolver::ModuleId(0);
        let file = resolver::ResolvedFile { id, path: "unchecked.cx".into(), program: resolver::parse_source(&src).expect("parses"), imports: vec![] };
        let resolved = resolver::ResolvedProgram { entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id] };
        let program = semantic::analyze_resolved_program(&resolved).expect("analysis cannot reach the last call's instance");
        let unchecked = program.unchecked_generics.clone().expect("the program says what was not checked");
        assert!(unchecked.contains("'f'"), "{unchecked}");
        let refused = format!("{:?}", crate::ir::lower::lower_program(&program).expect_err("compiled code refuses it"));
        assert!(refused.contains("compiled code refuses what was not checked"), "{refused}");
        // A program within the limit says nothing.
        let fine = errors("fnc: T <T> id(x: T) { return x }\nprint(id(5))\n");
        assert_eq!(fine, "");
        // A call with a generic struct's field, whose type is not tracked,
        // cannot be checked either.
        let field = "struct Box<T> { v: T }\nfnc: t64 <T> widen(x: T) {\n n: t64 = x\n return n\n}\nb: Box = Box<str> { v: \"s\" }\nprint(widen(b.v))\n";
        let id = resolver::ModuleId(0);
        let file = resolver::ResolvedFile { id, path: "field.cx".into(), program: resolver::parse_source(field).expect("parses"), imports: vec![] };
        let resolved = resolver::ResolvedProgram { entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id] };
        let program = semantic::analyze_resolved_program(&resolved).expect("a struct field's type is not checked");
        assert!(program.unchecked_generics.as_deref().is_some_and(|u| u.contains("does not track")), "{:?}", program.unchecked_generics);
    }

    /// What is not checked is recorded only where compiled code may compile
    /// it. Calls in a generic function that nothing calls, however many, and
    /// a call that leaves a type parameter unbound, do not make compiled code
    /// refuse a program it compiles correctly.
    #[test]
    fn only_what_compiled_code_may_compile_is_recorded() {
        let analyzed = |src: &str| {
            let id = resolver::ModuleId(0);
            let file = resolver::ResolvedFile { id, path: "record.cx".into(), program: resolver::parse_source(src).expect("parses"), imports: vec![] };
            let resolved = resolver::ResolvedProgram { entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id] };
            semantic::analyze_resolved_program(&resolved).expect("analyzes")
        };
        let mut uncalled = String::from("fnc: T <T, U> f(x: T, u: U) {\n c: [2: T] = [x, x]\n return c:[1]\n}\nfnc: t64 <Q> unused(q: Q) {\n");
        for n in 1..=130 {
            uncalled.push_str(&format!(" print(f(1, [{}]))\n", vec!["0"; n].join(", ")));
        }
        uncalled.push_str(" return 1\n}\nh: f64 = 1.5\nprint(f(h, 0))\n");
        let nest = "fnc: t64 <T> nest(x: T, n: t64) {\n if n <= 0 { return 0 }\n return nest([x], n - 1)\n}\nfnc: t64 <T> demo(x: T) { return nest(5, 40) }\nprint(42)\n";
        let partial = "fnc: T <T> id(x: T) { return x }\nfnc: Result<T> <T> fail(n: t64) {\n r: Result<T> = Err(\"no\")\n return id(r)\n}\nfnc: t64 later() {\n return 1\n print(fail(1))\n}\nprint(later())\n";
        for src in [uncalled.as_str(), nest, partial] {
            assert_eq!(analyzed(src).unchecked_generics, None, "{src}");
        }
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
