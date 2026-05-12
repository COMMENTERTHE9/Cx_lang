# Cx ABI and Data Layout — v0.1
Target: x86-64 (Windows, Linux)

---

## Status

This document tracks layout decisions for backend 0.1. Decisions marked **LOCKED** are frozen and have layout tests. Decisions marked **OPEN** need design work before implementation.

---

## Scalar Layout — LOCKED

All integers are signed two's complement. No unsigned types at 0.1. Cx type names (`t8`, `t16`, etc.) map 1:1 to IR types and Cranelift types.

| Cx Type | IR Type | Size (bytes) | Alignment (bytes) | Representation | Cranelift Type |
|---------|---------|--------------|-------------------|----------------|----------------|
| t8      | I8      | 1            | 1                 | signed i8      | types::I8      |
| t16     | I16     | 2            | 2                 | signed i16     | types::I16     |
| t32     | I32     | 4            | 4                 | signed i32     | types::I32     |
| t64     | I64     | 8            | 8                 | signed i64     | types::I64     |
| t128    | I128    | 16           | 16                | signed i128    | emulated (2x i64) |
| f64     | F64     | 8            | 8                 | IEEE 754 double | types::F64    |
| bool    | Bool    | 1            | 1                 | 0=false, 1=true | types::I8 (0/1) |
| tbool   | TBool   | 1            | 1                 | 0=false, 1=true, 2=unknown | types::I8 (0/1/2) |
| ptr     | Ptr     | 8            | 8                 | platform-width pointer | types::I64 (Cranelift) |

### Notes

- **i128 on Cranelift:** Not a native register type. Cranelift emulates it as two i64 values. LLVM handles i128 as a first-class type. This difference may affect performance but not correctness for 0.1.
- **bool:** Stored as a single byte. Only values 0 and 1 are valid. Any other value is undefined behavior at the backend level. Cranelift represents bool as I8 with a 0/1 convention.
- **Calling convention:** C ABI (SystemV on Linux, Windows fastcall on Windows) for all function calls at 0.1. Scalars passed in registers following platform convention.

---

## Calling Convention — LOCKED (0.1)

Single return value or void. No multi-return at 0.1.

| Return Type | Register | Notes |
|-------------|----------|-------|
| I8–I64      | RAX      | sign-extended as needed |
| I128        | RAX:RDX  | low 64 in RAX, high 64 in RDX |
| F64         | XMM0     | IEEE 754 double |
| Bool        | RAX      | 0 or 1, zero-extended in RAX |
| TBool       | RAX      | 0, 1, or 2, zero-extended in RAX |
| void        | —        | no return register used |

Parameter passing follows platform C ABI:
- Linux x64: SystemV — first 6 integer args in RDI, RSI, RDX, RCX, R8, R9. First 8 float args in XMM0–XMM7.
- Windows x64: fastcall — first 4 args in RCX, RDX, R8, R9 (integer) or XMM0–XMM3 (float).

### Copy Param Bleed-Back — POST-0.1

Copy params (`.copy`, `.copy.free`, `copy_into`) are post-0.1 for the compiled backend. The interpreter handles them correctly via `bleed_back` HashMap in `ScopeFrame`.

When copy param support lands in the compiled backend:
- Use hidden out-pointer pattern — callee receives a pointer to caller's variable, writes modified value back through it on return.
- Observable behavior must match interpreter exactly.
- Tests t10–t13 cover copy param semantics and must pass identically through both interpreter and compiled paths.

---

## Open Design Questions

### TBool Representation — LOCKED (0.1)
Three-state value: true (1), false (0), unknown (2).
- Wire format and storage size: LOCKED. 1 byte, values 0/1/2, stored as I8 at Cranelift level.
- IrType::TBool exists in the IR type system. Maps to Cranelift `types::I8`.
- Valid operations: comparison (0/1/2), three-way branching. Invalid: arithmetic, bitwise.
- Runtime representation: u8 stored in a 1-byte slot. No enum wrapping. Values 0/1/2 are the only valid states.
- IrType::TBool is a first-class IR type — not lowered as I8 with a comment. The type carries semantic intent.
- TBool function parameters: LOCKED. Follows C ABI treating TBool as I8. Passed in the same integer registers as Bool (RDI/RSI/RDX/RCX/R8/R9 on Linux x64; RCX/RDX/R8/R9 on Windows x64). Values 0/1/2 are preserved across the call boundary without padding or encoding. Zero-extended when widened to a larger integer type (Cast TBool → I32 uses `uextend`, not `sextend`).
- JIT validation: all three TBool wire values (0 = false, 1 = true, 2 = unknown) round-trip correctly through JIT-compiled function calls (confirmed by CX-127 tests in `host_boundary.rs`).

**Post-0.1 deferred items:**
- Unknown propagation strategy — does unknown checking happen in IR instructions or as runtime intrinsic calls? This decision gates when-block lowering and unknown-infected arithmetic.
- Arithmetic on unknown-infected values: propagation cost and mechanism. Blocked on unknown propagation strategy.
- When-block lowering: TBool three-way branching requires two nested Branch instructions since IR Branch is two-way only. Design work needed before implementation.

### String Layout — OPEN (str and strref not supported in 0.1)
- `str` at C boundary is `(*const u8, u32)` — pointer + length, no null termination. LOCKED per frontend dev.
- Arena ownership in JIT mode: does the JIT call into the interpreter's RunTime arena, maintain its own arena, or heap-allocate?
- `strref` escape rules depend on arena ownership decision.
- Both `str` and `strref` are explicitly unsupported in backend 0.1 pending the arena decision. See "Explicitly Unsupported in Backend 0.1" section below.

### Copy Parameter Convention — LOCKED (post-0.1)
Deferred to post-0.1. See Calling Convention section above for the locked decision and implementation plan.

### Struct Layout — LOCKED

- Field ordering: declaration order. No reordering. C-compatible for FFI.
- Alignment: natural alignment per field. Each field aligned to its own `align_bytes()`.
- Padding: implicit, inserted between fields to satisfy alignment.
- Struct total size: rounded up to largest field alignment (so arrays of structs stay aligned).
- No `#[packed]` option at 0.1. Can be added post-0.1 without ABI break.

Example:
```
struct { a: I8, b: I64 }
→ offset 0: a (1 byte) + 7 padding → offset 8: b (8 bytes) → total 16, align 8
```

Layout computation implemented in `src/ir/types.rs` as `compute_struct_layout`. Confidence tests cover single-field, padding, mixed fields, empty struct, and worst-case alignment scenarios.

### Array Layout — LOCKED (fixed-size only)

Fixed-size arrays only at 0.1. Length is compile-time, baked into `SemanticType::Array(usize, Box<SemanticType>)`. No runtime length field, no header, no fat pointer.

- Element stride: element size rounded up to element alignment.
- Total size: `count * stride`.
- Alignment: element alignment.
- Contiguous in memory — no indirection.

Example: `[5: t64]` → element 8 bytes, stride 8, total 40 bytes, alignment 8.

Dynamic arrays (push/pop, runtime-sized) are post-0.1 stdlib work. When those land they will need a different layout (fat pointer or struct with ptr + len + cap). This section covers fixed-size arrays only.

Layout computation implemented in `src/ir/types.rs` as `compute_array_layout`. Confidence tests cover i64, i8, bool, i128, and zero-length arrays.

### Expression Evaluation Order — LOCKED (0.1)

All Cx expressions are evaluated strictly left-to-right. Authoritative specification: `docs/backend/cx_eval_order.md`.

ABI impact: the IR instruction stream produced by `lower_binary` and the call argument-lowering loop encodes evaluation order structurally. Instructions for the left operand are emitted before instructions for the right operand within the same basic block. All backends must preserve this instruction order — no reordering of side-effecting instructions across operand boundaries is permitted.

Covered expression forms: binary arithmetic (`+`, `-`, `*`, `/`, `%`), comparison operators (`==`, `!=`, `<`, `<=`, `>`, `>=`), and function call argument lists. Short-circuit operators (`&&`, `||`) and `when` expressions are post-0.1.

JIT conformance tests in `src/backend/cranelift/host_boundary.rs` (`jit_eval_order_*`) verify that `IrInst::Binary { lhs, rhs }` and `IrInst::Compare { lhs, rhs }` map `lhs` as the left operand and `rhs` as the right operand in emitted Cranelift instructions.

---

### Enum Layout — LOCKED (tag-only)

Tag-only enums at 0.1. No data-carrying variants.

- Tag type: `u8`, 1 byte, align 1.
- Variant numbering: declaration order, starting at 0.
- Max 256 variants per enum.
- No payload, no discriminant + union layout.
- Data-carrying enums (tag + payload) are post-0.1.

Group and super-group membership is a semantic concept only — no impact on wire format. The tag is just a `u8` regardless of group structure.

Note: the interpreter matches enum variants by string name, not numeric tag. The compiled backend will use numeric tags. These are different internal representations for the same value. Enum values cannot be passed between interpreted and compiled code without a translation layer. Not a 0.1 problem — documented here so it doesn't surprise anyone later.

---

## Explicitly Unsupported in Backend 0.1

These types exist in the Cx frontend and semantic layer but have no backend representation in 0.1. Any program that triggers these types at the lowering boundary receives a structured `UnsupportedSemanticType` error — no panics, no silent misbehavior. This section documents the blocking reason and post-0.1 path for each type.

This section was added in Phase 8 Round 4 (CX-145).

### str — NOT SUPPORTED IN 0.1

`str` is an owned string type. The C-boundary wire format is settled (`(*const u8, u32)` — pointer + length, no null termination, per frontend developer decision), but backend support requires resolving arena ownership first.

**Blocking decision — arena ownership in JIT mode.** The tree-walk interpreter allocates strings into a `Vec<u8>` arena in `RunTime`. Three options are open:
1. JIT calls into the interpreter's `RunTime` arena via intrinsic calls.
2. JIT maintains its own separate arena.
3. Strings are heap-allocated in JIT mode (arena is an interpreter-only optimization).

This decision affects `strref` escape rules and gates the Phase 9 read/input intrinsic implementation.

**Current enforcement.** `SemanticType::Str` at the lowering boundary produces `LoweringError::UnsupportedSemanticType { ty: "Str" }`. `SemanticValue::Str(_)` in expression lowering also produces this error. Both are structured and named.

**Post-0.1 path.** Once the arena ownership decision is made, the String Layout section above will be finalized and this entry will move to the type layout sections as a LOCKED entry.

---

### strref — NOT SUPPORTED IN 0.1

`strref` is a borrowed view into a `str`'s backing arena. It cannot outlive the arena that owns the underlying bytes.

**Blocking decision — same as `str`.** `strref` escape rules are entirely dependent on the arena ownership decision that blocks `str`. The two types are co-blocked: `strref` cannot be defined at the backend level until `str`'s arena model is decided.

**Current enforcement.** The semantic layer proactively rejects `strref` in several positions before lowering is reached:
- Cannot assign a `strref` to a variable (it does not outlive its origin scope).
- Cannot use `strref` as a struct field type.
- Cannot return `strref` from a function.

Any `strref` that does reach the lowering pass produces `LoweringError::UnsupportedSemanticType { ty: "StrRef" }`. The semantic rejections live in `src/frontend/semantic.rs`.

**Post-0.1 path.** Blocked on the `str` arena ownership decision. Resolves together with `str`.

---

### Handle\<T\> — NOT SUPPORTED IN 0.1

`Handle<T>` is a typed generational handle into a `HandleRegistry<T>`. The handle itself is two `u32` fields — slot index and generation counter. The registry detects stale handles by bumping the generation counter on removal.

**Reason not supported.** Handle registry operations — insert, get, remove, stale detection — are classified as post-0.1 work. No ABI-level intrinsic protocol for handle registry interaction has been designed. The handle's machine footprint (8 bytes, two u32 fields at natural alignment) is derivable from the struct layout rules, but the runtime intrinsic calls needed to operate on a live registry are not yet defined.

**Current enforcement.** `SemanticType::Handle(_)` at the lowering boundary produces `LoweringError::UnsupportedSemanticType { ty: "Handle" }`. Handle operations in expressions — `HandleNew`, `HandleVal`, `HandleDrop` — produce `UnsupportedSemanticConstruct` errors before the type check is reached. The runtime implementation lives in `src/runtime/handle.rs` but is not wired into the backend codegen path.

**Post-0.1 path.** Tracked as "Handle registry operations — post-0.1" in Phase 9 of `docs/backend/cx_backend_roadmap_v3_1.md`. When the Phase 9 handle sub-packet lands, this section will be updated with the ABI for handle registry intrinsic calls and the handle wire format will be promoted to a LOCKED entry.
