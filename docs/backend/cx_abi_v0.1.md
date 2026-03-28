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

### Notes

- **i128 on Cranelift:** Not a native register type. Cranelift emulates it as two i64 values. LLVM handles i128 as a first-class type. This difference may affect performance but not correctness for 0.1.
- **bool:** Stored as a single byte. Only values 0 and 1 are valid. Any other value is undefined behavior at the backend level. Cranelift represents bool as I8 with a 0/1 convention.
- **Calling convention:** C ABI (SystemV on Linux, Windows fastcall on Windows) for all function calls at 0.1. Scalars passed in registers following platform convention.

---

## Open Design Questions

### TBool Representation — PARTIALLY LOCKED
Three-state value: true (1), false (0), unknown (2).
- Wire format and storage size: LOCKED. 1 byte, values 0/1/2, stored as I8 at Cranelift level.
- IrType::TBool exists in the IR type system. Not yet produced by lower_type (awaiting SemanticType::TBool in frontend).
- Valid operations: comparison (0/1/2), three-way branching. Invalid: arithmetic, bitwise.
- Wire format 0/1/2 is locked from the language spec.
- Runtime representation: u8? enum? tagged union?
- Does IrType need a TBool variant or is it lowered as I8 with 0/1/2 convention?
- Unknown propagation: IR-level checks or runtime intrinsic calls?
- TBool function parameters: calling convention implications.
- Arithmetic on unknown-infected values: propagation cost and mechanism.

### String Layout — OPEN
- `str` at C boundary is `(*const u8, u32)` — pointer + length, no null termination. LOCKED per frontend dev.
- Arena ownership in JIT mode: does the JIT call into the interpreter's RunTime arena, maintain its own arena, or heap-allocate?
- `strref` escape rules depend on arena ownership decision.

### Copy Parameter Convention — OPEN
- `(x.copy)` requires bleed-back: modified parameter written back to caller on return.
- Options: pass by pointer, return struct with modified params, secondary return slot.
- Decision affects every function call involving .copy parameters.

### Struct Layout — OPEN
- Field ordering: declaration order or optimized packing?
- Alignment: per-field natural alignment? Packed option?
- Padding rules.

### Array Layout — OPEN
- Element stride: size rounded up to alignment.
- Contiguous in memory — no indirection.

### Enum Layout — OPEN
- Tag representation: u8? u32?
- Variant layout for data-carrying enums (post-0.1).
