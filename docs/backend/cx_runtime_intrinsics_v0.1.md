# Cx Runtime Intrinsics Boundary — v0.1
Phase 9 specification  
Status: sub-packet 1 complete (audit + structured errors), codegen pending

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

| Builtin      | Category            | Return  | Planned backend mechanism        | Blocking condition |
|--------------|---------------------|---------|----------------------------------|--------------------|
| `print`      | I/O — stdout        | void    | runtime call to `cx_print`       | frontend must promote to function first |
| `println`    | I/O — stdout        | void    | runtime call to `cx_println`     | same as `print` |
| `printn`     | I/O — stdout        | void    | runtime call to `cx_printn`      | same as `print` |
| `read`       | I/O — stdin         | str     | runtime call to `cx_read`        | str/strref layout must be locked (Phase 8 open item) |
| `input`      | I/O — stdin         | str     | runtime call to `cx_input`       | same as `read` |
| `assert`     | Debug / assertion   | void    | inline Branch + trap, or runtime | semantics TBD — does assert abort or panic? |
| `assert_eq`  | Debug / assertion   | void    | inline cmp + Branch + trap       | same as `assert` |

### I/O builtins — print family

`print`, `println`, `printn` are stdout I/O.  They do not return a value.

Cross-roadmap dependency: **the frontend has not promoted these to real
functions yet**.  Until they have a proper call signature (parameter types,
arity rules) Phase 9 cannot define their ABI.  Phase 9 tracks this as its
primary blocker.

Planned mechanism: a thin runtime shim (`cx_runtime.c` or equivalent)
exported as a C-ABI symbol.  JIT-compiled code calls it via `IrInst::Call`
with the shim's name as callee.  The shim writes to stdout using the platform
C runtime (`puts` / `printf`).  No in-process pipe redirection.

### I/O builtins — read / input

`read` and `input` return a string.  They block until stdin delivers a line.

Additional blocker: the `str` / `strref` layout question from Phase 8 is
unresolved (arena ownership in JIT mode vs. interpreter mode).  The return
type and ownership model for these calls cannot be finalised until that
decision is made.

### Debug builtins — assert / assert_eq

`assert(cond)` and `assert_eq(lhs, rhs)` are diagnostic assertions.

Design decision needed before these can be lowered:

- Do they abort the process (like C `assert`)? If so, the backend emits a
  conditional Branch to a trap block.
- Do they raise a Cx panic that can be caught? If so, a runtime-call mechanism
  is needed.

For 0.1 the expected answer is "abort" (simple, no unwinding machinery).
That decision must be confirmed before implementation.

---

## Planned Implementation Path (Phase 9 remaining sub-packets)

**Sub-packet 2 — print family (blocked on frontend)**

When the frontend promotes `print`, `println`, `printn` to real functions:

1. Add shim symbols to `src/backend/cranelift/jit.rs` — declare them as
   external C-ABI functions in the Cranelift module.
2. Remove them from `is_cx_builtin()`.
3. They then flow through the normal `IrInst::Call` path.
4. Tests: a `.cx` program that calls `print` executes through the JIT and
   produces matching stdout in the differential harness.

**Sub-packet 3 — assert / assert_eq**

After the abort-vs-panic decision is locked:

1. If abort: lower to `IrInst::Branch` + `IrTerminator::Trap` (or equivalent).
   Requires adding `IrTerminator::Trap` to the IR.
2. Remove them from `is_cx_builtin()`.
3. Tests: a program that hits a failing assert produces exit code 126 (JIT
   runtime failure).

**Sub-packet 4 — read / input (blocked on Phase 8 str layout)**

Deferred until `str` and `strref` layout is locked in Phase 8.

---

## Runtime Entry Point Registry (draft)

This section will list the stable C-ABI symbols that JIT-compiled Cx code
may call.  It is a stub until sub-packets 2–4 land.

| Symbol         | Signature (C)                     | Provided by       |
|----------------|-----------------------------------|-------------------|
| `cx_print`     | `void cx_print(const char* s)`    | cx_runtime shim   |
| `cx_println`   | `void cx_println(const char* s)`  | cx_runtime shim   |
| `cx_printn`    | `void cx_printn(int64_t n)`       | cx_runtime shim   |
| `cx_read`      | TBD — blocked on str layout       | —                 |
| `cx_input`     | TBD — blocked on str layout       | —                 |

Ownership rules:
- All string pointers passed to I/O shims are read-only; the shim does not
  take ownership and does not free.
- The caller (JIT code) is responsible for keeping the backing memory alive
  for the duration of the call.

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
- `src/ir/lower.rs` — `is_cx_builtin()` guard and structured error
- `docs/backend/cx_abi_v0.1.md` — scalar layout and calling convention
- `docs/backend/cx_backend_roadmap_v3_1.md` — Phase 9 and its blockers
- `src/backend/cranelift/host_boundary.rs` — JIT host boundary documentation
