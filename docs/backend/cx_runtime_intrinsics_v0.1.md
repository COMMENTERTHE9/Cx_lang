# Cx Runtime Intrinsics Boundary — v0.1
Phase 9 specification  
Status: sub-packets 1–3 complete; sub-packet 4 blocked on Phase 8 str layout

---

## Purpose

This document defines the boundary between code that the Cx backend lowers as
pure IR (arithmetic, control flow, memory ops) and code that must cross a
runtime call boundary (I/O, assertions, allocation, error paths).

Without this boundary the lowering code accumulates ad-hoc holes — callee
names that silently miss the signature table, producing generic errors that
reveal nothing about the missing piece or when it will be filled.

Phase 9 replaces those holes with:

1. An explicit classification of every builtin (this document).
2. Structured `UnsupportedSemanticConstruct` errors that name the builtin and
   reference Phase 9 as the tracking phase.
3. Concrete `IrInst::Call` lowering or inline IR for each builtin where the
   design is resolved (sub-packets 2 and 3).

---

## Audit — Current Ad-hoc Hooks

### How builtins are represented in the semantic layer

`src/frontend/semantic.rs` recognises these names in `analyze_call()` (around
line 1446) and assigns `FunctionId(u32::MAX)` to mark them as non-user-defined:

```text
print    println    printn
read     input
assert   assert_eq
```

The semantic node produced is:

```rust
SemanticExprKind::Call {
    callee: "<name>",
    function: FunctionId(u32::MAX),   // sentinel — not a real function
    args:   <analyzed args>,
}
```

Return type is `SemanticType::Str` for `read`/`input`; `SemanticType::Void`
for all others.

### What happened during lowering before Phase 9 sub-packet 1

These names are **absent from the `signature_table`** (which only holds
user-defined functions built by `build_signature_table()`).  When a builtin
reached lowering:

- As an `ExprStmt`: the `sig_info` lookup returned `None`, the code fell
  through to `lower_expr`, and the inner lookup failed.
- In `lower_expr` as `SemanticExprKind::Call`: `ctx.signature_table.get(callee)`
  returned `None`, producing:
  ```rust
  LoweringError::UnresolvedSemanticArtifact { artifact: "function '<name>'" }
  ```

This error is **misleading** — it implies a bug in the resolver, not a known
pending feature.

### Fix applied in Phase 9 sub-packet 1 (`src/ir/lower.rs`)

`is_cx_builtin(name: &str) -> bool` guards both call paths.  Any builtin hit
during lowering now returns:

```rust
LoweringError::UnsupportedSemanticConstruct {
    construct: "builtin '<name>' is not yet lowerable to IR — codegen pending (Phase 9)"
}
```

Seven tests verify this — one per builtin — ensuring the error family is
correct and contains the builtin name.

---

## Builtin Classification Table

| Builtin      | Category            | Return  | Backend mechanism                         | Status |
|--------------|---------------------|---------|-------------------------------------------|--------|
| `print`      | I/O — stdout        | void    | `IrInst::Call` to `cx_printn` / `cx_printf` / `cx_printb` / `cx_printn_i128` / `cx_print_tbool` | DONE (I8/I16/I32/I64/I128/F64/Bool/TBool) |
| `println`    | I/O — stdout        | void    | same type-dispatch as `print`             | DONE (I8/I16/I32/I64/I128/F64/Bool/TBool) |
| `printn`     | I/O — stdout        | void    | same type-dispatch as `print`             | DONE (I8/I16/I32/I64/I128/F64/Bool/TBool) |
| `read`       | I/O — stdin         | str     | runtime call to `cx_read`                 | BLOCKED — str/strref layout (Phase 8) |
| `input`      | I/O — stdin         | str     | runtime call to `cx_input`                | BLOCKED — str/strref layout (Phase 8) |
| `assert`     | Debug / assertion   | void    | inline `Branch` + `IrTerminator::Trap`    | DONE — abort semantics |
| `assert_eq`  | Debug / assertion   | void    | inline `Compare(Eq)` + `Branch` + `Trap`  | DONE — abort semantics |

### I/O builtins — print family

`print`, `println`, `printn` are stdout I/O.  They do not return a value.

**Implementation (sub-packet 2 — COMPLETE; CX-225 train integrates all type dispatch):**

All three builtins route through `lower_print_stmt` in `src/ir/lower.rs`, which
dispatches by argument `IrType`:

| Argument type       | Intrinsic         | Notes                                 |
|---------------------|-------------------|---------------------------------------|
| `I64`               | `cx_printn`       | direct                                |
| `I8`, `I16`, `I32`  | `cx_printn`       | widened to I64 via Cast (CX-153)      |
| `F64`               | `cx_printf`       | CX-146                                |
| `Bool`              | `cx_printb`       | CX-155                                |
| `I128`              | `cx_printn_i128`  | CX-172; JIT backend issues isplit     |
| `TBool`             | `cx_print_tbool`  | CX-178                                |

Neither `cx_print` nor `cx_println` exist as distinct symbols — all three print
builtins route through the same per-type intrinsic.  The `builtin_name` parameter
is preserved for error messages only.

The `cx_printn` symbol is:
- Implemented in `src/backend/cranelift/host_boundary.rs` as `extern "C" fn cx_printn(n: i64)`.
- Registered in every JIT module via `jit_builder.symbol("cx_printn", cx_printn as *const u8)`.
- Pre-declared as an imported C-ABI function in the Cranelift module before any
  user function is compiled.

The `cx_printf` symbol is:
- Implemented as `extern "C" fn cx_printf(x: f64)`.
- Prints the f64 value with Rust's `{}` format followed by a newline.

The `cx_printb` symbol is:
- Implemented as `extern "C" fn cx_printb(b: i8)`.
- Prints `"false"` for b=0, `"true"` for any other value.

The `cx_printn_i128` symbol is:
- Implemented as `extern "C" fn cx_printn_i128(lo: i64, hi: i64)`.
- Cranelift 0.115 x64 ABI does not support I128 call params; the JIT backend
  issues `isplit` to split the I128 into `(lo, hi)` before the call.
- Reconstructs the i128 as `((hi as i128) << 64) | (lo as u64 as i128)`.

The `cx_print_tbool` symbol is:
- Implemented as `extern "C" fn cx_print_tbool(n: i8)`.
- Prints `"false"` for n=0, `"true"` for n=1, `"?"` for any other value.
- Matches the interpreter's `value_to_string()` output for `Value::TBool(b)`.

### I/O builtins — read / input

`read` and `input` return a string.  They block until stdin delivers a line.

Blocker: the `str` / `strref` layout question from Phase 8 is unresolved
(arena ownership in JIT mode vs. interpreter mode).  The return type and
ownership model for these calls cannot be finalised until that decision is made.
These builtins remain in `is_cx_builtin()` and produce a structured error.

### Debug builtins — assert / assert_eq

`assert(cond)` and `assert_eq(lhs, rhs)` are diagnostic assertions.

**Implementation (sub-packet 3 — COMPLETE):**

Both builtins use abort semantics (abort the process, no unwinding).  They are
lowered to a two-branch CFG pattern:

```text
[current block]
  ... condition computation ...
  Branch { cond, then: pass_block, else: trap_block }

[pass_block]     ← execution continues here after a passing assertion
  (empty — caller receives this as the new current block)

[trap_block]
  Trap            ← IrTerminator::Trap; maps to Cranelift `trap` in the JIT
```

Condition type handling:
- `Bool` — used directly as the branch condition.
- `I8`, `I16`, `I32`, `I64`, `I128` — compared `!= 0` via `Compare(Ne)` to
  produce a `Bool` (truthy-integer assert, via `coerce_to_bool`).
- For `assert_eq`: both operands must have the same type and that type must be
  `Bool` or an integer (`I8`–`I128`).  A `Compare(Eq)` is emitted first, then
  the same Branch + Trap pattern follows.
- All other types produce a structured `UnsupportedSemanticConstruct` error.

---

## Implementation Path (Phase 9 sub-packets)

**Sub-packet 1 — audit + structured errors — COMPLETE**

`is_cx_builtin()` guard added; all seven builtins produce structured
`UnsupportedSemanticConstruct` errors instead of misleading artifact-resolution
failures.  Seven tests verify one per builtin.

**Sub-packet 2 — print family — COMPLETE**

`print`, `println`, `printn` lower via `lower_print_stmt` / `lower_printn_stmt`
to `IrInst::Call` targeting the `cx_printn` runtime symbol.  I64 arguments
supported; non-I64 returns a structured error.  JIT parity tests cover all
three builtins.

**Sub-packet 3 — assert / assert_eq — COMPLETE**

Abort semantics confirmed.  `lower_assert_stmt` and `lower_assert_eq_stmt`
emit the Branch + Trap CFG pattern.  `IrTerminator::Trap` added to the IR.
Six unit tests cover Bool, integer-truthy, and I128 variants for both builtins.

**Sub-packet 4 — read / input — BLOCKED on Phase 8 str layout**

Deferred until `str` and `strref` layout is locked in Phase 8.

---

## Runtime Entry Point Registry

Stable C-ABI symbols that JIT-compiled Cx code may call:

| Symbol            | Signature (C)                                | Provided by                       | Status  |
|-------------------|----------------------------------------------|-----------------------------------|---------|
| `cx_printn`       | `void cx_printn(int64_t n)`                  | `host_boundary.rs` (`extern "C"`) | LIVE    |
| `cx_printf`       | `void cx_printf(double x)`                   | `host_boundary.rs` (`extern "C"`) | LIVE    |
| `cx_printb`       | `void cx_printb(int8_t b)`                   | `host_boundary.rs` (`extern "C"`) | LIVE    |
| `cx_printn_i128`  | `void cx_printn_i128(int64_t lo, int64_t hi)` | `host_boundary.rs` (`extern "C"`) | LIVE    |
| `cx_print_tbool`  | `void cx_print_tbool(int8_t n)`              | `host_boundary.rs` (`extern "C"`) | LIVE    |
| `cx_read`         | TBD — blocked on str layout                  | —                                 | BLOCKED |
| `cx_input`        | TBD — blocked on str layout                  | —                                 | BLOCKED |

Notes:
- `cx_print` and `cx_println` do not exist as separate symbols.  All three print
  builtins route through the same per-type intrinsic as `printn`.
- All print intrinsics always append a newline (`writeln!`).
- `cx_printn_i128` takes (lo, hi) because Cranelift 0.115 x64 ABI does not support
  I128 call parameters; the JIT backend issues `isplit` before the call.
- All string pointers passed to future I/O shims will be read-only; the shim
  will not take ownership and will not free.

---

## Non-Goals for Phase 9

- Handle<T> registry intrinsics — post-0.1
- Arena allocation intrinsics — post-0.1
- Error and panic propagation through the backend — post-0.1
- TBool Unknown propagation — open design question tracked in Phase 8
- String copy-on-boundary rules — blocked on str layout decision

---

## References

- `src/frontend/semantic.rs` — builtin recognition in `analyze_call()` (~line 1446)
- `src/ir/lower.rs` — `is_cx_builtin()` guard, `lower_print_stmt`, `lower_printn_stmt`, `lower_assert_stmt`, `lower_assert_eq_stmt`
- `src/ir/instr.rs` — `IrTerminator::Trap` definition
- `src/ir/validate.rs` — pre-seeded `cx_printn` intrinsic signature
- `src/backend/cranelift/host_boundary.rs` — `cx_printn` extern "C" implementation and JIT symbol registration
- `docs/backend/cx_abi_v0.1.md` — scalar layout and calling convention
- `docs/backend/cx_backend_roadmap_v3_1.md` — Phase 9 and its blockers
