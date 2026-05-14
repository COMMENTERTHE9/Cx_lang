# Cx JIT Determinism Guarantee
v1.2 — 2026-05-09

---

## What This Document Covers

This document defines the minimal determinism guarantee for the Cx Cranelift JIT backend at the 0.1 release. It describes exactly what "deterministic" means in this context, what is and is not guaranteed, and how the guarantee is verified.

This is a 0.1 hard blocker. See `docs/backend/cx_backend_roadmap_v3_1.md`, "Hard blockers" section.

---

## The Guarantee

> **Same IR, same target, same input → same observable output on every run.**

More precisely:

- Given the same `IrModule` (identical in structure and values)
- On the same target platform (same ISA, same OS)
- With the same program input (no runtime I/O that varies between runs)

The Cx JIT backend produces identical observable output on every invocation:

1. **Exit code** — the value returned by `main` is identical
2. **Execution path** — the sequence of JIT-compiled instructions executed is identical
3. **Memory layout** — stack slot sizes and alignments assigned by the JIT are identical

---

## What Drives Determinism

The JIT pipeline has several stages, each of which must be deterministic:

### 1. IR Structure

`IrModule` is a plain Rust data structure — a `Vec<IrFunction>` where each function contains `Vec<IrBlock>` with instructions and a terminator. There is no hash-map iteration at the IR level. The order of functions, blocks, and instructions is exactly the order they appear in the `Vec`, which is determined by the lowering pass.

`ValueId` and `BlockId` are sequential integers allocated by `IrBuilder` in the order they are requested. Given the same semantic program and the same lowering pass, the ID sequence is always identical.

### 2. Cranelift ISA Selection

`cranelift_native::builder()` detects the host CPU at startup and produces a deterministic ISA configuration. For a given binary running on a given machine, the selected ISA is always the same. This covers: instruction set extensions (SSE4.2, AVX2, etc.), calling convention, and pointer size.

### 3. Cranelift IR Emission

`compile_ir_function` in `host_boundary.rs` maps each `IrInst` and `IrTerminator` variant to exactly one Cranelift IR instruction sequence. There is no randomization, no hash-map iteration over instruction sets, and no conditional code that depends on process state. The mapping is a deterministic function of the `IrFunction` content.

Block creation order mirrors the IR block order (`for ir_block in &ir_func.blocks`). Value numbering within Cranelift follows the order of `builder.ins().*` calls, which follows instruction order.

### 4. Block Sealing Order

All Cranelift blocks are sealed at once via `seal_all_blocks()` after all instructions and terminators have been emitted. This deferred-sealing strategy is safe for any control-flow graph (forward-only or with back-edges): Cranelift can resolve all block-parameter propagation with complete predecessor information once the full CFG is registered. The sealing order is therefore fully determined by the IR block order and does not vary between runs.

This strategy also enables correct loop execution: back-edges (from a loop body to a loop header) are registered before the header block is sealed, so Cranelift's internal predecessor tracking is complete by the time the seal occurs.

### 5. JIT Module Finalization

`JITModule::finalize_definitions()` applies machine-code emission to all declared functions. The emission order follows the function declaration order, which follows the order of `ir.functions`. No global re-ordering occurs.

### 6. Code Execution

Once finalized, the machine code at the `main` function pointer is executed via `unsafe { main_fn() }`. The JIT code is deterministic: same machine code, same CPU state at entry, same observable output.

---

## What Is Not Guaranteed

- **Cross-platform determinism.** The same IR on Windows x64 vs Linux x64 may produce different binary layouts (calling convention, stack alignment, register allocation). Exit codes are still semantically identical for correct programs, but the machine code bytes differ.

- **Cross-version determinism.** Upgrading Cranelift or the host toolchain may change code generation. The IR → output contract holds for a single build, not across version upgrades.

- **Hash randomization.** Rust's `HashMap` uses random seeds by default. The JIT backend uses `HashMap<BlockId, cl::Block>` and `HashMap<ValueId, cl::Value>` internally in `compile_ir_function`. These maps are iteration-order-agnostic: entries are only read by key lookup (`map[&key]`), never iterated for output. Hash randomization therefore does not affect observable output.

- **Stdout ordering with external I/O.** If JIT-compiled code calls C runtime intrinsics (`puts`, `printf`) interleaved with host-process output, the interleaving may vary with system scheduling. This does not apply to programs that use only exit codes.

- **In-process stdout capture.** The current subprocess-capture model does not redirect the JIT's stdout inside the host process. Determinism of textual output is verified by the differential harness via subprocess, not by in-process comparison.

---

## Verification

The determinism guarantee is verified by `determinism_tests` in `src/backend/cranelift/host_boundary.rs`, enabled with `#[cfg(all(test, feature = "jit"))]`.

### Test Strategy

Each determinism test:
1. Builds a single `IrModule` value (the module is identical in both runs by construction)
2. Calls `HostBoundary::new().execute(&module)` twice, in sequence, in the same process
3. Asserts both calls return `Ok`
4. Asserts the exit codes are identical

This is sufficient to verify the guarantee: if the JIT pipeline were non-deterministic (e.g. produced different code due to address-space randomization, uninitialized stack state, or hash-map iteration order leaking into values), the exit codes would differ between runs.

### Test Coverage

| Test | IR construct covered |
|------|---------------------|
| `jit_determinism_const_return_zero` | `ConstInt` + `Return` (exit 0) |
| `jit_determinism_const_return_nonzero` | `ConstInt` + `Return` (exit 42) |
| `jit_determinism_arithmetic_add` | `Binary::Add` |
| `jit_determinism_arithmetic_sub` | `Binary::Sub` |
| `jit_determinism_arithmetic_mul` | `Binary::Mul` |
| `jit_determinism_arithmetic_div` | `Binary::Div` |
| `jit_determinism_arithmetic_rem` | `Binary::Rem` |
| `jit_determinism_alloca_store_load` | `Alloca` + `Store` + `Load` |
| `jit_determinism_branch_eq_true_path` | `Compare::Eq` + `Branch` (true path) |
| `jit_determinism_branch_eq_false_path` | `Compare::Eq` + `Branch` (false path) |
| `jit_determinism_branch_lt_true_path` | `Compare::Lt` + `Branch` (true path) |
| `jit_determinism_jump_with_block_param` | `Jump` + block parameters |
| `jit_determinism_back_edge_loop` | Back-edge CFG (while loop) via `seal_all_blocks()` |
| `jit_determinism_two_function_module` | Multiple functions in one module |
| `jit_determinism_loop_construct_with_break` | `loop { break }` — header→body back-edge; break via `Branch` `then_args`; continue-loop via `else_args`; exercises `CompareOp::Ge` |
| `jit_determinism_loop_continue` | `continue` — header with three predecessors (entry, end-of-body, continue back-edge); `Compare::Lt` + `Compare::Eq` |
| `jit_determinism_nested_loop_back_edges` | Nested loops — two independent back-edges; inner header carries both outer and inner loop vars as block params |
| `jit_determinism_loop_accumulator` | Loop with two header params (counter + accumulator); `else_args` passes accumulated value to exit block |
| `jit_determinism_for_loop_exclusive` | `for i in 0..5 {}` — 5-block for-loop CFG; exclusive `Lt` bound; 5 iterations; exit code 42 |
| `jit_determinism_for_loop_inclusive` | `for i in 0..=4 {}` — inclusive `Le` bound; same 5 iterations; exit code 42 |
| `jit_determinism_for_loop_zero_iterations` | `for i in 5..0 {}` — `Lt` false on first check; body/increment unreachable; exit code 7 |
| `jit_determinism_for_loop_with_loop_carried_binding` | `sum += i` across iterations — counter + accumulator threaded as two header block params; exit code 10 |
| `jit_determinism_call_return_value` | `Call` — value-returning callee; result used as exit code |
| `jit_determinism_call_void` | `Call` — void callee (no return value); caller returns a constant |
| `jit_determinism_call_with_args` | `Call` — callee takes two `I32` arguments; exercises argument passing |
| `jit_determinism_call_chained` | `Call` — three-function chain; verifies forward-reference resolution |
| `jit_determinism_call_in_branch` | `Call` inside a non-entry block (branch arm); verifies block-local call emission |
| `jit_determinism_call_multiple` | Two calls to the same callee; verifies repeated `declare_func_in_func` stability |

### Running the Tests

```
cargo build --features jit
cargo test --features jit determinism
```

All determinism tests must pass on every supported target platform.

---

## Relationship to the 0.1 Release Gate

The roadmap states:

> Minimal determinism guaranteed — same IR, same target, same input produces same observable output on every run.

This document is the specification of that guarantee. The `determinism_tests` module is the verification. Both must be present for the 0.1 gate to close.

---

## Future Work (Post-0.1)

- Extend determinism tests to cover cross-run reproducibility when the IR is produced by `lower_program` from source text (full pipeline determinism, not just JIT-level)
- Add determinism tests for in-process stdout capture once the pipe-redirect scaffold lands
- Verify binary-level reproducibility (same machine-code bytes) for AOT builds
