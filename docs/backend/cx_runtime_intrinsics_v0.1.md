# Cx Runtime Intrinsics Boundary — v0.1
Phase 9 specification  
Status: sub-packets 1–3 complete, sub-packet 4 blocked on Phase 8 str layout

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
3. Eventually, concrete `IrIntrinsic` opcodes or ABI-stable runtime-call
   signatures for each builtin.

---

## Audit — Current Ad-hoc Hooks

### How builtins are represented in the semantic layer

`src/frontend/semantic.rs` recognises these names in `analyze_call()` (around
line 1446) and assigns `FunctionId(u32::MAX)` to mark them as non-user-defined:

```
print    println    printn
read     input
assert   assert_eq
```

The semantic node produced is:

```
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
  ```
  LoweringError::UnresolvedSemanticArtifact { artifact: "function '<name>'" }
  ```

This error is **misleading** — it implies a bug in the resolver, not a known
pending feature.

### Fix applied in Phase 9 sub-packet 1 (`src/ir/lower.rs`)

`is_cx_builtin(name: &str) -> bool` guards both call paths.  Any builtin hit
during lowering now returns:

```
LoweringError::UnsupportedSemanticConstruct {
    construct: "builtin '<name>' is not yet lowerable to IR — codegen pending (Phase 9)"
}
```

Seven tests verify this — one per builtin — ensuring the error family is
correct and contains the builtin name.

---

## Builtin Classification Table

| Builtin      | Category            | Return  | Backend mechanism                                  | Status |
|--------------|---------------------|---------|-----------------------------------------------------|--------|
| `print`      | I/O — stdout        | void    | `IrInst::Call` → `cx_printn` (I64 only)            | **DONE** — CX-136 |
| `println`    | I/O — stdout        | void    | `IrInst::Call` → `cx_printn` (I64 only)            | **DONE** — CX-136 |
| `printn`     | I/O — stdout        | void    | `IrInst::Call` → `cx_printn`                       | **DONE** |
| `read`       | I/O — stdin         | str     | runtime call to `cx_read`                          | BLOCKED — str/strref layout (Phase 8) |
| `input`      | I/O — stdin         | str     | runtime call to `cx_input`                         | BLOCKED — same as `read` |
| `assert`     | Debug / assertion   | void    | `IrInst::Branch` + `IrTerminator::Trap` (abort)    | **DONE** — Phase 9 sub-packet 3 |
| `assert_eq`  | Debug / assertion   | void    | `IrInst::Compare(Eq)` + Branch + Trap (abort)      | **DONE** — Phase 9 sub-packet 3 |

### I/O builtins — print family

`print`, `println`, `printn` are stdout I/O.  They do not return a value.
All three are **COMPLETE** as of Phase 9 sub-packet 2 (CX-136, 2026-05-10).

**Implementation** (`src/ir/lower.rs`):

- `printn(n)` → `lower_printn_stmt()`: emits `IrInst::Call { callee: "cx_printn", args: [n] }` directly.
- `print(n)` and `println(n)` → `lower_print_stmt()`: both route to the same `cx_printn` call.
  In Cx both print and println already produce a newline (matching `cx_printn`'s `writeln!` behaviour),
  so a single runtime symbol covers all three.

**Type restriction**: Only `I64` arguments are supported.  Non-I64 arguments produce a structured
`UnsupportedSemanticConstruct` error naming the actual builtin (e.g. `"print argument must be I64"`).
String printing awaits the Phase 8 str/strref layout decision.

**Runtime symbol**: `cx_printn` is declared in the JIT module as an imported C-ABI symbol
(`src/backend/cranelift/host_boundary.rs`) and registered in `RESERVED_RUNTIME_INTRINSICS`.

**Diagnostic fix** (CX-141): `lower_print_stmt` threads `builtin_name: &str` through all error
paths so diagnostics correctly name `print` vs `println` rather than a generic label.

### I/O builtins — read / input

`read` and `input` return a string.  They block until stdin delivers a line.

Additional blocker: the `str` / `strref` layout question from Phase 8 is
unresolved (arena ownership in JIT mode vs. interpreter mode).  The return
type and ownership model for these calls cannot be finalised until that
decision is made.

### Debug builtins — assert / assert_eq

`assert(cond)` and `assert_eq(lhs, rhs)` are diagnostic assertions.
Both are **COMPLETE** as of Phase 9 sub-packet 3.

**Semantics locked**: abort (like C `assert`).  No unwinding, no Cx panic path.
The design decision was resolved in favour of the simpler model for 0.1.

**Implementation** (`src/ir/lower.rs` — `lower_assert_stmt` / `lower_assert_eq_stmt`):

Both builtins lower to a two-block CFG pattern:

```
[current block]
  ... condition computation ...
  Branch { cond, then: pass_block, else: trap_block }

[pass_block]          ← execution continues here if assertion passes
  (empty — caller receives this as the new current block)

[trap_block]
  Trap                ← abort; Cranelift emits a hardware trap instruction
```

- `assert(cond)`: condition must be Bool or a truthy integer (I8/I16/I32/I64 ≠ 0).
- `assert_eq(lhs, rhs)`: both operands must have the same type; a `Compare(Eq)` instruction
  produces the Bool passed to the branch.  Supported types: Bool, I8, I16, I32, I64.

Unsupported types (Ptr, F64, StrRef, etc.) produce a structured `UnsupportedSemanticConstruct`
error so the caller gets a clear diagnostic rather than a crash.

---

## Implementation Path (Phase 9 sub-packets)

**Sub-packet 1 — Audit + structured errors** ✓ COMPLETE

Added `is_cx_builtin()` guard producing `UnsupportedSemanticConstruct` errors for all seven
builtins.  Seven tests verify the error family (one per builtin).

**Sub-packet 2 — print family** ✓ COMPLETE (CX-136, 2026-05-10; CX-141 diagnostic fix)

1. Added `cx_printn` to `RESERVED_RUNTIME_INTRINSICS` and declared it in the JIT module
   as an imported C-ABI symbol.
2. Removed `print`, `println`, `printn` from `is_cx_builtin()`; they are now intercepted
   at the tier-1 `ExprStmt` gate in `lower_stmt()`.
3. `lower_print_stmt()` and `lower_printn_stmt()` emit `IrInst::Call { callee: "cx_printn" }`.
4. Tests added: `print_i64_lowers_to_cx_printn_call`, `println_i64_lowers_to_cx_printn_call`,
   `print_non_i64_arg_returns_unsupported_construct`.

**Sub-packet 3 — assert / assert_eq** ✓ COMPLETE

1. Abort semantics confirmed.  `IrTerminator::Trap` was already present in the IR.
2. Removed `assert`, `assert_eq` from `is_cx_builtin()`; intercepted at tier-1 gate.
3. `lower_assert_stmt()` and `lower_assert_eq_stmt()` emit Branch + Trap pattern.
4. Tests verify correct Branch IR generation for passing and failing assertion cases.

**Sub-packet 4 — read / input** BLOCKED on Phase 8 str layout

Deferred until `str` and `strref` layout is locked in Phase 8.

---

## Runtime Entry Point Registry

C-ABI symbols that JIT-compiled Cx code may call.  Sub-packets 2–3 are live;
sub-packet 4 symbols are TBD pending Phase 8 str layout.

| Symbol         | Signature (C)                     | Provided by                              | Status |
|----------------|-----------------------------------|------------------------------------------|--------|
| `cx_printn`    | `void cx_printn(int64_t n)`       | `src/backend/cranelift/host_boundary.rs` | **LIVE** |
| `cx_read`      | TBD — blocked on str layout       | —                                        | pending Phase 8 |
| `cx_input`     | TBD — blocked on str layout       | —                                        | pending Phase 8 |

Notes:
- `cx_print` and `cx_println` are **not** separate runtime symbols.  Both
  `print` and `println` builtins lower to `cx_printn` (see sub-packet 2).
- `assert` and `assert_eq` lower to inline IR (`Branch` + `Trap`) — no
  runtime symbol needed.

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
- `src/ir/lower.rs` — `is_cx_builtin()` guard (~line 94); tier-1 ExprStmt intercept (~line 666);
  `lower_printn_stmt` (~line 2678); `lower_print_stmt` (~line 2720);
  `lower_assert_stmt` (~line 2763); `lower_assert_eq_stmt` (~line 2796)
- `src/backend/cranelift/host_boundary.rs` — `cx_printn` extern C implementation (~line 267);
  JIT symbol registration (~line 342)
- `docs/backend/cx_abi_v0.1.md` — scalar layout, calling convention, and expression evaluation order
- `docs/backend/cx_backend_roadmap_v3_1.md` — Phase 9 and its sub-packets
