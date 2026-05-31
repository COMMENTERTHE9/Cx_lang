//! Single source of truth for Cx language builtins (tracker #008).
//!
//! Before this module, the set of builtin names (`print`, `println`, `printn`,
//! `read`, `input`, `assert`, `assert_eq`, `is_known`, `exit`) was hardcoded as
//! string literals in four places that had to agree by hand:
//!
//!   - `src/frontend/semantic.rs`  (signature recognition)
//!   - `src/runtime/runtime.rs`    (interpreter dispatch)
//!   - `src/ir/validate.rs`        (reserved-name gate)
//!   - `src/ir/lower.rs`           (JIT lowering dispatch)
//!
//! Drift between those four lists was the structural cause of several backend
//! disagreements. Each site now consults [`BUILTINS`] / [`lookup`] instead.
//!
//! Scope note: this registry covers *user-callable Cx builtins* only. The
//! C-ABI runtime intrinsic `cx_printn` (the lowered form of `print`/`printn`)
//! is a different concept and stays single-sourced by
//! `crate::ir::validate::runtime_intrinsic_names`.
//!
//! This is a pure-data leaf module: it depends on no `SemanticType` or IR type,
//! so all four consumers (frontend, runtime, ir) can import it without any
//! circular-dependency risk. Each call site maps the registry's small enums to
//! its own types.

/// Discriminator identifying each builtin. Call sites match on this instead of
/// comparing name strings, so the compiler catches a builtin that a site forgot
/// to handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Print,
    Println,
    Printn,
    Read,
    Input,
    Assert,
    AssertEq,
    IsKnown,
    Exit,
}

/// Argument-count contract for a builtin. Descriptive: only `exit` currently
/// enforces its arity at the semantic layer (see [`Arity::accepts`]); the other
/// builtins record their intended arity for documentation but do not enforce it
/// — preserving pre-#008 behavior exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    Exact(usize),
    Range(usize, usize),
    Variadic,
}

impl Arity {
    /// Whether `n` arguments satisfy this arity contract.
    pub fn accepts(&self, n: usize) -> bool {
        match *self {
            Arity::Exact(k) => n == k,
            Arity::Range(lo, hi) => n >= lo && n <= hi,
            Arity::Variadic => true,
        }
    }
}

/// Return type of a builtin, in registry-local terms. Mapped to `SemanticType`
/// at the semantic.rs call site so this module stays decoupled from the type
/// system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinRet {
    Void,
    Bool,
    Str,
}

/// How the Cranelift JIT lowering pass treats a builtin today.
///
/// - `Lowered` — `lower_stmt` routes it to a dedicated lowering function.
/// - `GatedUnsupported` — lowering is not implemented; the lowerer emits a
///   structured `UnsupportedSemanticConstruct` error (the `is_cx_builtin` gate).
/// - `Unhandled` — the lowerer does not special-case it; it falls through to the
///   ordinary call path (today this means it does not lower / is skipped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitStatus {
    Lowered,
    GatedUnsupported,
    Unhandled,
}

/// One builtin's canonical definition.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinDef {
    /// Cx source identifier.
    pub name: &'static str,
    /// Dispatch discriminator.
    pub kind: BuiltinKind,
    /// Intended argument count (enforced only for `exit`; see [`Arity`]).
    pub arity: Arity,
    /// Return type in registry terms.
    pub ret: BuiltinRet,
    /// Whether the IR validator's reserved-name gate rejects a user-defined
    /// function of this name. `true` for all user builtins — `exit` was the lone
    /// `false` holdout (#008 preserved it) until #033 closed the asymmetry.
    pub validator_reserved: bool,
    /// JIT lowering status (read by the lowerer to derive its `is_cx_builtin`
    /// gate).
    pub jit: JitStatus,
}

/// The canonical builtin set. Adding a builtin is a one-line change here.
///
/// `print` and `println` are exact aliases today — `println` does NOT add a
/// trailing newline. This is a preserved pre-#008 surprise, tracked as #034.
pub const BUILTINS: &[BuiltinDef] = &[
    BuiltinDef { name: "print",     kind: BuiltinKind::Print,    arity: Arity::Variadic,    ret: BuiltinRet::Void, validator_reserved: true,  jit: JitStatus::Lowered },
    BuiltinDef { name: "println",   kind: BuiltinKind::Println,  arity: Arity::Variadic,    ret: BuiltinRet::Void, validator_reserved: true,  jit: JitStatus::Lowered },
    BuiltinDef { name: "printn",    kind: BuiltinKind::Printn,   arity: Arity::Variadic,    ret: BuiltinRet::Void, validator_reserved: true,  jit: JitStatus::Lowered },
    BuiltinDef { name: "read",      kind: BuiltinKind::Read,     arity: Arity::Exact(1),    ret: BuiltinRet::Str,  validator_reserved: true,  jit: JitStatus::GatedUnsupported },
    BuiltinDef { name: "input",     kind: BuiltinKind::Input,    arity: Arity::Exact(2),    ret: BuiltinRet::Str,  validator_reserved: true,  jit: JitStatus::GatedUnsupported },
    BuiltinDef { name: "assert",    kind: BuiltinKind::Assert,   arity: Arity::Exact(1),    ret: BuiltinRet::Void, validator_reserved: true,  jit: JitStatus::Lowered },
    BuiltinDef { name: "assert_eq", kind: BuiltinKind::AssertEq, arity: Arity::Exact(2),    ret: BuiltinRet::Void, validator_reserved: true,  jit: JitStatus::Lowered },
    BuiltinDef { name: "is_known",  kind: BuiltinKind::IsKnown,  arity: Arity::Exact(1),    ret: BuiltinRet::Bool, validator_reserved: true,  jit: JitStatus::Unhandled },
    BuiltinDef { name: "exit",      kind: BuiltinKind::Exit,     arity: Arity::Range(0, 1), ret: BuiltinRet::Void, validator_reserved: true,  jit: JitStatus::Unhandled },
];

/// Look up a builtin by its Cx source name. Linear scan over ~9 entries.
pub fn lookup(name: &str) -> Option<&'static BuiltinDef> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// Whether the IR validator's reserved-name gate should reject a user-defined
/// function named `name`. Composed (in validate.rs) with the C-ABI intrinsic
/// names from `runtime_intrinsic_names()`.
pub fn is_validator_reserved(name: &str) -> bool {
    lookup(name).is_some_and(|b| b.validator_reserved)
}
