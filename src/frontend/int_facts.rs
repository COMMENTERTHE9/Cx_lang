//! Single source of truth for signed integer width facts (tracker D1.1).
//!
//! Before this module, "what fits a `tN`" was decided in three places that had
//! to agree by hand — the same drift-by-hand hazard that motivated the builtin
//! registry (#008), here for integer widths:
//!
//!   - `src/frontend/semantic.rs`  `check_num_range` — the #028 range-check bounds
//!   - `src/runtime/runtime.rs`    `apply_numeric_cast` — the `as iN` narrowing
//!   - `src/ir/lower.rs`           `ir_int_range` — the IR range-validation bounds
//!
//! Cx integers are SIGNED (stored as i8/i16/i32/i64/i128), so the facts are the
//! signed ranges. Each site now maps its own type ([`crate::frontend::semantic_types::SemanticType`]
//! / [`crate::ir::types::IrType`]) into [`IntWidth`] and reads the one table.
//!
//! Pure-data leaf, exactly like `builtins.rs`: it depends on no `SemanticType`
//! or IR type, so frontend, runtime, and ir can all consult it without any
//! circular-dependency or layering violation — the dependency points *into*
//! this leaf, never out. [`IntWidth`] is the neutral discriminator (the role
//! `BuiltinKind` plays for builtins) that lets both the frontend type and the
//! backend type key the same table.
//!
//! Scope note: `Bool` is deliberately NOT a row here. It is not a `tN` integer
//! width — it has no Cx type name, is not a narrowing target (neither
//! `check_num_range` nor `apply_numeric_cast` handle it), and only the IR
//! range-validator needs its `(0, 1)` bound. That stays a one-line local arm in
//! `ir_int_range` rather than a table row two of the three consumers must skip.

/// Signed integer width identity — the neutral key both the frontend
/// (`SemanticType`) and the backend (`IrType`) map into, so this leaf depends on
/// neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    W8,
    W16,
    W32,
    W64,
    W128,
}

/// The signed-range facts for one integer width. `name` is the Cx type name used
/// verbatim in #028 diagnostics (`t8`..`t128`).
#[derive(Debug, Clone, Copy)]
pub struct IntFacts {
    pub min: i128,
    pub max: i128,
    pub name: &'static str,
}

impl IntWidth {
    /// The inclusive signed `[min, max]` range and Cx name for this width.
    pub const fn facts(self) -> IntFacts {
        match self {
            IntWidth::W8 => IntFacts { min: i8::MIN as i128, max: i8::MAX as i128, name: "t8" },
            IntWidth::W16 => IntFacts { min: i16::MIN as i128, max: i16::MAX as i128, name: "t16" },
            IntWidth::W32 => IntFacts { min: i32::MIN as i128, max: i32::MAX as i128, name: "t32" },
            IntWidth::W64 => IntFacts { min: i64::MIN as i128, max: i64::MAX as i128, name: "t64" },
            IntWidth::W128 => IntFacts { min: i128::MIN, max: i128::MAX, name: "t128" },
        }
    }

    /// Truncate an `i128` to this width, preserving the EXACT `as iN` wrapping
    /// the runtime cast relied on. This is the same `n as i8 as i128` etc. the
    /// cast wrote inline, relocated unchanged — bit-identical by construction,
    /// not re-derived from a bit count.
    pub const fn truncate(self, n: i128) -> i128 {
        match self {
            IntWidth::W8 => n as i8 as i128,
            IntWidth::W16 => n as i16 as i128,
            IntWidth::W32 => n as i32 as i128,
            IntWidth::W64 => n as i64 as i128,
            IntWidth::W128 => n,
        }
    }
}
