# Cx Compiler Backend Roadmap
v4.2 — 2026-05-06

---

## What This Roadmap Covers

This document covers the Cx compiler backend — the pipeline that takes a verified semantic program and produces correct machine output.

The GPU layer, windowing system, and platform API design are tracked separately in the Cx Platform and GPU Roadmap. They are not the same class of work as IR lowering and code generation, and mixing them here makes both look bigger and more confusing than they are.

**This roadmap covers:**
- Semantic program → IR lowering
- IR validation
- Control flow, call, and loop lowering
- ABI and data layout
- Runtime intrinsics boundary
- Backend diagnostics and observability
- Differential testing and parity harness
- Cranelift JIT — 0.1 backend target
- Cranelift AOT — post-0.1
- LLVM AOT — post-0.1

**This roadmap does not cover:**
- GPU layer — see Cx Platform and GPU Roadmap
- Window and screen system — see Cx Platform and GPU Roadmap
- Filesystem and I/O — see Cx Language Roadmap
- Language semantics — the backend preserves them, it does not define them

---

## Backend Philosophy

The backend exists to turn correct Cx programs into correct machine output. It does not invent behavior. The semantic layer and interpreter define what Cx does. The backend must match that exactly.

**The backend is responsible for:**
- Preserving semantic meaning exactly
- Preserving control flow exactly
- Preserving data layout according to Cx type layout rules
- Producing structured errors for unsupported constructs
- Never panicking on valid IR
- Rejecting invalid IR before codegen reaches it

**The backend is not responsible for:**
- Deciding language semantics
- Reinterpreting Unknown state
- Inventing implicit runtime behavior
- Silently widening unsupported features into partial behavior
- Optimizing code — correctness first, performance later
- Optimization is never allowed to change observable Cx behavior

For 0.1 the gate is correctness. Performance is a post-0.1 story.

---

## 0.1 Backend Release Definition

**Cx backend 0.1 means:**
- A non-trivial multi-function Cx program executes correctly through Cranelift JIT
- Backend output matches interpreter output on all supported frontend matrix tests
- Structured errors are produced for unsupported constructs — no panics, no silent failures
- ABI and data layout rules are documented and tested
- The differential harness runs automatically
- IR validation catches bad IR before codegen sees it

**Cx backend 0.1 does not mean:**
- Optimized release builds
- AOT compiled artifacts
- Full language surface supported in codegen
- GPU or platform API work

---

## 0.1 Backend Release Gates

These are conditions, not features. All must be true before 0.1 ships.

**Hard blockers:**
- All supported frontend matrix tests pass through the Cranelift JIT path
- Backend output matches interpreter output on every supported test — stdout, exit code, behavior
- Backend produces structured errors, not panics, for every unsupported construct
- IR validator rejects malformed IR before codegen is reached
- One non-trivial multi-function program runs correctly end to end through JIT
- ABI and layout rules are documented and tested for all core types
- Runtime intrinsics boundary is defined and implemented
- Backend must not panic on any valid IR, even when construct support is incomplete
- Minimal determinism guaranteed — same IR, same target, same input produces same observable output on every run
- Core layout confidence tests pass — struct size, field offsets, array strides, bool/enum/TBool representation
- Evaluation order for supported expressions is documented and stable — assignment side effects match semantic layer behavior exactly

**Quality gates — must be true or have a tracked plan:**
- Backend error messages refer back to source constructs where possible
- IR dump on failure is automatic
- Supported and unsupported construct lists are documented and accurate
- Target platform matrix is explicit — at minimum Windows x64 and Linux x64

---

## Backend Support Matrix — 0.1

**Supported in backend 0.1:**
- Straight-line arithmetic
- Variable declarations and assignments
- Functions — parameters, return types, return values
- if / else if / else
- Direct function calls
- while loops
- Basic array forms after frontend array semantics are frozen for 0.1
- Basic struct forms after frontend struct semantics and layout rules are frozen for 0.1

**Explicitly unsupported in backend 0.1:**
- GPU operations
- Filesystem operations
- Window and rendering operations
- Full generics surface
- Dynamic dispatch
- Closures and lambdas
- Async and continuations
- when blocks — produces structured UnsupportedSemanticConstruct error; when lowering will require design work for TBool three-way branching (true/false/unknown requires two nested Branch instructions since IR only has two-way Branch)

This list is intentional. Unsupported constructs must produce structured errors, not silently misbehave.

---

## Done ✅

**Phase 0 — Foundation Setup**
- SemanticProgram exists
- Semantic analysis returns Result<SemanticProgram, Vec<SemanticError>>
- Lowering consumes &SemanticProgram
- Backend consumes &IrModule
- Main prepares IR before backend dispatch
- Unsupported semantic-only artifacts reject cleanly

**Phase 1 — Real IR Data Model**
- IrType, IrModule, IrFunction, IrBlock
- ValueId, BlockId
- IrInst, IrTerminator
- Block params for SSA merges — not phi nodes, correct decision
- Builder helpers
- IR structure has unit test coverage

**Phase 2 — Straight-Line Lowering**
- Constants, variable refs, declaration-only handling
- Assignment, typed assignment
- Arithmetic, comparisons, explicit casts
- Synthetic main
- Unsupported constructs fail structurally
- Lowering tests exist

**Phase 3 — IR Validation**
- Duplicate block id checks
- Undefined value checks
- Invalid block target checks
- Duplicate value definition checks
- Basic type and invariant checks
- Synthetic main validation
- Lowering tests now validate produced IR

**Phase 4 — Function Lowering**
- Real SemanticStmt::FuncDef
- Typed parameters and return types
- Entry block param SSA setup
- Function body lowering for supported straight-line subset
- Return and trailing ret_expr
- Real functions plus synthetic main coexist
- Name collision handling for real main vs synthetic main
- Function-local SSA maps work
- Validator accepts normal functions

**Phase 5 — if / else Lowering**
- Conditional branch lowering with explicit then/else/merge blocks
- Chained else-if lowering
- SSA environment splitting and merge at branch points
- Join block params instead of phi nodes
- Dead-branch return behavior handled correctly
- Branch-local temporary handling
- Validator updates for multi-block functions and synthetic main
- Top-level and function-body if/else lower correctly
- 2559 insertions across lower.rs, mod.rs, validate.rs

---

**Phase 0.5 — Backend Trait Interface Change** *(DONE — 2026-03-25)*

Backend trait signature changed to take `&IrModule`. `main.rs` passes lowered IR into backend dispatch. All backend stubs compile against the new signature.

---

**Phase 11 — Surface Area Reduction** *(DONE — 2026-05-06)*

All 0.1-scope constructs either lower correctly or produce a named, structured rejection. See Phase 11 entry in Up Next for the full landed list. No construct is silently unsupported.

---

**Phase 13 — Cranelift Lowering Skeleton** *(DONE — 2026-05-06)*

IrType → Cranelift type mapping, module/function/block traversal, instruction and terminator dispatch with named `CraneliftLoweringError` variants. Every unsupported construct produces a specific named error. `src/backend/cranelift/mod.rs`.

---

**JIT Runtime Host Boundary** *(DONE — 2026-05-06)*

`HostBoundary`, `JitExitCode`, `JitOutcome`, `JitExecutionError` types. Process ownership, exit-code extraction, output scaffolding. `src/backend/cranelift/host_boundary.rs`.

---

**Phase 12 sub-packet 1 — Differential harness shell** *(DONE — 2026-05-06)*

`TestFixture`, `collect_matrix_tests()`, `run_interpreter()`, `cx_binary_path()` in `src/diff_harness.rs`. Interpreter baseline capture working. 4 harness tests.

---

**Phase 14 sub-packets 1–2 — First JIT execution** *(DONE — 2026-05-06)*

Sub-packet 1: ConstInt (I8/I16/I32/I64), Binary (Add/Sub/Mul/Div/Rem), Return — 8 tests.
Sub-packet 2: Alloca, Load, Store — 4 tests.
First real Cranelift execution confirmed working.

---

**Phase 9 sub-packet 1 — Runtime intrinsics audit + structured errors** *(DONE — 2026-05-06)*

`is_cx_builtin()` guard in `lower.rs`. All 7 builtins produce `UnsupportedSemanticConstruct` with Phase 9 note. `docs/backend/cx_runtime_intrinsics_v0.1.md` created. 7 tests.

---

**Evaluation order specification** *(DONE — 2026-05-06)*

Left-to-right expression evaluation order documented in `docs/backend/cx_eval_order.md` (CX-37). Tests verify that binary expression operands, call arguments, and assignment RHS all evaluate left-to-right in `lower.rs`. This closes the 0.1 hard blocker for evaluation order stability.

---

**IR validator loop variable read-only enforcement** *(DONE — 2026-05-06)*

`ReadOnlyLoopVar` enforcement added to the IR validator (CX-40). Loop variable bindings are now rejected by the validator if assigned inside the loop body, matching the runtime enforcement.

---

## Active 🔄

**Phase 8 Round 2 — ABI and Data Layout (open items)** *(in progress)*

str/strref layout at the backend boundary, Handle<T> runtime representation, TBool calling convention, and unknown propagation strategy are all still open. See Phase 8 entry in Up Next for details.

---

**Phase 9 sub-packets 2–4** *(in progress / blocked)*

Sub-packet 2 (print family lowering) is in Human Review (CX-38). Sub-packets 3–4 are blocked on design/frontend decisions. See Phase 9 entry in Up Next for details.

---

**Phase 14 sub-packets 3–4 + Phase 15 sub-packets 1–3** *(in Human Review)*

CX-27/CX-41 (Compare + Jump + Branch), CX-30 (direct calls), CX-32 (PtrOffset + PtrAdd), CX-33 (SsaBind + ConstFloat + Cast), CX-36 (float arithmetic) — all in Human Review.

---

## Up Next — Core Compiler Work 🔲

**Phase 6 — Function Call Lowering** *(DONE — 2026-03-22)*

Stage 2b: direct call lowering — `IrInst::Call` emitted; arity and arg-type validated against `signature_table` at lowering time. Call result flows into assignments, returns, and expressions.

Stage 3: cross-function call validation in IR validator — callee resolution, arity check, arg type check against the module function list.

Tests cover: unresolved callee, arity mismatch, type mismatch, call-with-assignment, and call-in-expr-stmt cases.

Void-return calls resolved: `lower_void_call()` added in Phase 11 work; void calls now emit `IrInst::Call { dst: None, return_ty: None }` and lower correctly as standalone statements. `IrType::Void` remains absent from the IR type system by design — void is a calling-convention property, not a stored type.

---

**Phase 7 — IR Pretty Printer and Diagnostics Foundation** *(DONE — 2026-03-25)*

IR pretty printer, `--backend=validate` mode, and `--debug-trace` verbose flag all implemented. IR dump triggered automatically on validation failure in test helper.

---

**Phase 8 — ABI and Data Layout** *(Round 1 DONE 2026-03-27 — open items remain)*

Goal: freeze backend-visible representation of all core runtime types. For a game engine language where predictable memory layout is a core selling point, correct machine output is not fully defined until layout rules are documented, implemented, and tested.

Without this phase, parity testing is incomplete — the backend could produce output that matches the interpreter by accident rather than by design.

**Landed in Round 1 (2026-03-27):**
- Scalar layout locked — t8/t16/t32/t64/t128/f64/bool: `size_bytes()` and `align_bytes()` on `IrType`, 7 confidence tests; spec in `docs/backend/cx_abi_v0.1.md` ✅
- bool representation locked — 0 for false, 1 for true ✅
- Struct field layout locked — declaration order, natural alignment, padding, total size rounded to largest field alignment; `compute_struct_layout()`, 7 confidence tests ✅
- Array element layout locked — fixed-size, contiguous, stride-based; `compute_array_layout()`, 5 confidence tests ✅
- Enum layout locked — tag-only u8, declaration order, 0–255 ✅
- `IrType::TBool` added — 1-byte three-state type (0/1/2), layout locked; awaiting frontend `SemanticType::TBool` wiring into the lowering path ✅
- Calling convention locked — C ABI, single return register; copy param bleed-back deferred to post-0.1 ✅
- Target platform matrix explicit — Windows x64 and Linux x64 ✅

**Still open:**
- str and strref layout at backend boundary
  - Arena ownership question: the tree-walk interpreter's arena is a Vec<u8> in RunTime. In JIT mode, does the JIT call into the same RunTime arena via intrinsic calls, maintain its own separate arena, or treat strings as heap-allocated (arena as interpreter-only optimization)? This decision affects strref escape rules since strref is an arena view that cannot outlive the arena. Must be answered before any string layout is defined.
- Handle<T> runtime representation
- TBool calling convention — a TBool param is not a bool param; function parameter passing convention for TBool needs explicit decision
- Unknown propagation strategy — does unknown checking happen in IR instructions or as runtime intrinsic calls? Arithmetic on unknown-infected values: propagation cost and mechanism must be defined
- Return value rules for large values and void — IrType::Void still pending

Done when:
- Every core Cx type has a documented backend representation
- Layout tests validate that representation
- Calling convention is documented for supported targets
- No layout rule is implicit or assumed

---

**Phase 9 — Runtime Intrinsics Boundary**

Goal: define exactly what the backend lowers as pure IR versus what becomes a runtime call. Without this the backend has ad hoc hooks scattered through the lowering code instead of a clean, testable boundary.

**Sub-packet 1 — Audit + structured errors** *(DONE — 2026-05-06)*

- Audited all ad-hoc builtin hooks: `print`, `println`, `printn`, `read`, `input`, `assert`, `assert_eq`
- All were previously reaching the lowering layer and failing with the generic `UnresolvedSemanticArtifact` error (a signature_table miss), which was misleading
- Added `is_cx_builtin()` guard in `src/ir/lower.rs` — builtins now produce `UnsupportedSemanticConstruct` with an explicit "codegen pending (Phase 9)" message
- Classified every builtin: I/O stdout, I/O stdin, debug assertion
- Created `docs/backend/cx_runtime_intrinsics_v0.1.md` — full boundary specification including classification table, planned implementation path, draft runtime entry point registry, and non-goals
- 7 tests added — one per builtin — verifying correct error family and that the builtin name appears in the message

**Sub-packet 2 — runtime intrinsics dispatch mechanism** *(in Human Review — CX-38)*

- Implements the runtime dispatch layer that sub-packets 2–4 will route through
- Note: `print`/`println`/`printn` require the frontend to promote them to real functions with a fixed call signature — cross-roadmap dependency that must resolve before print intrinsic lowering can proceed

**Sub-packet 3 — assert / assert_eq lowering** *(DESIGN NEEDED)*

- Abort-vs-panic semantics decision needed before implementation
- For 0.1 the expected answer is abort — confirmed before sub-packet 3 starts

**Sub-packet 4 — read / input lowering** *(BLOCKED — pending str layout)*

- Blocked on str/strref layout decision from Phase 8

**Remaining Phase 9 items (not yet sub-packet assigned):**

- Allocation operations — arena, handle registry interactions (post-0.1)
- Handle registry operations — insert, get, remove, stale detection (post-0.1)
- String boundary operations — str copy-on-boundary, strref validity (blocked on Phase 8)
- Error and panic paths — how they surface through the backend (post-0.1)

Done when:
- Every builtin has a documented classification
- No ad hoc runtime hooks exist in the lowering code
- All intrinsics have tests
- The boundary between IR math and runtime calls is explicit and stable

---

**Phase 10 — Loop Lowering** *(DONE — 2026-03-22)*

while loop: header/body/exit CFG, loop-carried SSA via block params, backedge, 3 tests.

for loop: inline range (explicit `start`/`end`/`inclusive` from semantic layer, not `SemanticExprKind::Range`), increment block, ascending only, inclusive/exclusive bounds, break/continue support, 4 tests.

infinite loop (`loop` keyword): header/body CFG with break as exit.

break: unconditional branch to loop exit block.

continue: unconditional branch to loop header.

Returns inside loop body handled correctly — conditional return inside while verified.

Implementation note: the for-loop range dependency on Phase 11 was resolved by implementing for-loop lowering using explicit `start`/`end`/`inclusive` params that the semantic layer extracts. `SemanticExprKind::Range` as a standalone expression remains unsupported in `lower_expr`.

Loop variable read-only invariant resolved: `ReadOnlyLoopVar` enforcement added to the IR validator (CX-40, 2026-05-06). The validator now rejects assignments to loop variable bindings inside loop bodies.

---

**Phase 11 — Surface Area Reduction for Supported 0.1 Subset** *(DONE — 2026-05-06)*

Goal: shrink the unsupported surface area intentionally. Every construct in this phase either gets supported or gets a documented, structured rejection. Nothing is silently unsupported.

**All landed:**
- CompoundAssign — `+=`, `-=`, `*=`, `/=`, `%=` on binding targets — 3 tests ✅
- Unary expressions — negate (int/float) and boolean not — 4 tests ✅; strategy documented in `lower.rs` comments (CX-6) ✅
- `IrType::Ptr`; `IrInst::Alloca`/`Load`/`Store` with validator and printer support ✅
- Struct registry threaded into `LoweringCtx`; `lower_type` maps `Struct` → `IrType::Ptr` ✅
- `IrInst::PtrOffset { dst, base, offset }` — compile-time byte offset on a Ptr, for field addressing ✅
- `IrInst::PtrAdd { dst, base, offset }` — runtime pointer arithmetic with a ValueId offset ✅
- `SemanticExprKind::StructInstance` lowering — Alloca(total_size, align) + PtrOffset + Store per field, returns base Ptr; 4 tests ✅
- `SemanticStmt::StructDef` in `lower_stmt` is a no-op (registry pre-built) ✅
- `when` statement and `when` expression — both produce structured `UnsupportedSemanticConstruct` errors ✅
- `SemanticExprKind::DotAccess` — struct field reads via `lower_dot_access()`: Load from PtrOffset ✅
- `SemanticLValue::DotAccess` in `Assign` — struct field writes via `resolve_field_ptr()` + Store ✅
- `SemanticLValue::DotAccess` in `CompoundAssign` — read-modify-write through field pointer ✅
- Void-return function calls — `lower_void_call()` emits `Call { dst: None, return_ty: None }` ✅
- `SemanticType::Array` in `lower_type` — maps to `IrType::Ptr` ✅
- `SemanticExprKind::ArrayLit` — Alloca + Store per element via `lower_array_lit()` ✅
- `SemanticExprKind::Index` — array element access via `lower_index()`, load from element pointer ✅
- `SemanticLValue::Index` in `Assign` and `CompoundAssign` — array element writes via `resolve_array_element_ptr()` + Store ✅
- Array-of-structs lowering tests added (CX-18) ✅
- `SemanticExprKind::Range` — named `UnsupportedSemanticConstruct` with clear message (standalone range not supported) ✅
- `SemanticExprKind::MethodCall` — named `UnsupportedSemanticConstruct("MethodCall '...'")` ✅

**Remaining unsupported (0.1 non-goals, all produce named structured errors):**
- `EnumDef`, `Enum` types
- `WhileIn`
- `ImplBlock`
- `ConstDecl`
- `Handle` types (`HandleNew`, `HandleVal`, `HandleDrop`)
- `WhenExpr`
- `ResultOk`, `ResultErr`, `Try`
- `StrRef`, `Str`, `Char` types

---

**Phase 12 — Differential Backend Harness**

Goal: make parity a real tracked system, not a vague aspiration. The frontend has a 117-test matrix. This phase builds the infrastructure to run that same matrix through the backend and compare results automatically.

This phase should be treated as a mini-system in its own right — not just a phase.

**Sub-packet 1 — Harness shell + interpreter baseline** *(DONE — 2026-05-06)*

- `TestFixture` format defined — expected pass/fail, expected output sidecar
- `collect_matrix_tests()` enumerates all matrix tests from `src/tests/verification_matrix/`
- `run_interpreter()` runs a fixture through the `cx` binary and captures stdout + exit code
- `cx_binary_path()` resolves the correct binary for the current build
- `InterpOutcome` captures stdout, exit code, and success/failure flag
- 4 harness-level tests: non-empty collection, pass/fail split, output expectations coverage, interpreter baseline sweep
- `src/diff_harness.rs` module wired into the test suite under `--features jit`

**Sub-packet 2 — JIT execution and arithmetic comparison** *(in Human Review — CX-31)*

- Runs a fixture through the JIT path alongside the interpreter
- Compares stdout and exit code
- Arithmetic subset (t01–t05) confirmed matching

**Sub-packet 3 — Full supported construct comparison** *(in Human Review — CX-34)*

- Expanded fixture coverage for all 0.1-supported constructs
- Divergences surfaced automatically with IR dump on mismatch

Done when:
- Harness runs automatically in CI
- All supported frontend matrix tests pass through backend with matching output
- All unsupported constructs produce structured errors
- Divergences between interpreter and backend are surfaced immediately
- The harness is the definition of parity — not a vague description

---

**Phase 13 — Cranelift Lowering Skeleton** *(DONE — 2026-05-06)*

Goal: teach Cranelift to consume IR shape safely before any execution is attempted.

- `ir_type_to_cranelift()` — complete mapping for all 9 `IrType` variants to Cranelift types ✅
- `lower_module()` / `lower_function()` / `lower_block()` — module traversal skeleton ✅
- `lower_instruction()` — instruction dispatch; every `IrInst` variant returns a named `UnsupportedInstruction` error ✅
- `lower_terminator()` — terminator dispatch; every `IrTerminator` variant returns a named `UnsupportedTerminator` error ✅
- `CraneliftLoweringError` structured error type: `InvalidIrType`, `UnsupportedInstruction`, `UnsupportedTerminator`, `FunctionLoweringFailed` ✅
- Explicit separation between valid-but-unsupported IR and invalid IR ✅
- No AST or semantic leakage into the backend — IR is the only input ✅
- `src/backend/cranelift/mod.rs` — all skeleton code in place, actual emit in `host_boundary.rs` ✅

---


---

**JIT Runtime Host Boundary** *(DONE — 2026-05-06)*

Before any JIT execution, these must be explicitly defined and documented.

- `HostBoundary::execute()` owns the JIT lifecycle — create module, compile, execute ✅
- `JitExitCode` type — `SUCCESS` (0), sentinel for runtime failure, sentinel for unsupported construct, arbitrary i32 from `main` ✅
- `JitOutcome` — wraps exit code; `success()` and `from_main_return()` constructors ✅
- `JitExecutionError` — `UnsupportedConstruct`, `CodegenFailure`, `MainNotFound`, `RuntimePanic` variants with Display ✅
- `run_jit()` thin public wrapper in `src/backend/cranelift/jit.rs` ✅
- `HostBoundary` struct in `src/backend/cranelift/host_boundary.rs` — full execution contract documented ✅
- Tests cover all error variant Display strings, exit code sentinels, and the stub placeholder behavior ✅

---

**Phase 14 — First Executable Cranelift Slice**

Goal: first real backend execution. The simplest possible program runs through the full JIT pipeline and produces correct output.

**Sub-packet 1 — ConstInt + arithmetic + Return** *(DONE — 2026-05-06)*

- I8/I16/I32/I64 `ConstInt` emit via `builder.ins().iconst()` ✅
- `Binary` (Add, Sub, Mul, Div, Rem) for signed integers ✅
- `Return` terminator: `builder.ins().return_()` ✅
- Synthetic `__cx_main` function compiles and executes ✅
- 8 tests: const zero/42/1, arithmetic for all 5 ops, no-main error case, unsupported-inst error case ✅

**Sub-packet 2 — Alloca + Load + Store** *(DONE — 2026-05-06)*

- `Alloca` emits a Cranelift `stack_slot` + `stack_addr` instruction; `dst` receives an I64 pointer ✅
- `Load` emits typed `load()` from a stack pointer ✅
- `Store` emits `store()` to a stack pointer ✅
- 4 tests: load/store I32, load/store I8, overwrite, two independent slots ✅

**Sub-packet 3 — Compare + Jump + Branch** *(in Human Review — CX-27, CX-41)*

- `Compare` instruction; `Jump` and `Branch` terminators
- Multi-block functions with conditional control flow

**Sub-packet 4 — Direct function calls** *(in Human Review — CX-30)*

- `IrInst::Call` with a callee name, args, and return value
- Multi-function programs execute correctly through JIT

Done when:
- A pure-computation .cx program executes through the backend path
- Output matches interpreter output exactly — stdout and exit code
- At least one multi-function program works
- Test harness automates execution and comparison
- Performance is not the gate — correctness is

---

**Phase 15 — Cranelift JIT — 0.1 Target**

Goal: full JIT execution for all constructs in the supported 0.1 subset. This is the compiled output deliverable for 0.1.

JIT is enough for 0.1. Nobody evaluating Cx at 0.1 is benchmarking release build performance. They are checking if the language works, if the semantics are correct, and if the developer experience is good. JIT answers all of those questions without the complexity of object emission, linker flow, and platform handling.

**Sub-packet 1 — PtrOffset + PtrAdd** *(in Human Review — CX-32)*

- `IrInst::PtrOffset` and `IrInst::PtrAdd` emit in the JIT path
- Enables struct field access and array element access through JIT

**Sub-packet 2 — SsaBind + ConstFloat + Cast** *(in Human Review — CX-33)*

- `IrInst::SsaBind` — SSA register binding
- `IrInst::ConstFloat` — f64 constants
- `IrInst::Cast` — numeric type widening/narrowing

**Sub-packet 3 — Float arithmetic dispatch** *(in Human Review — CX-36)*

- `Binary` for F64 operands: `fadd`, `fsub`, `fmul`, `fdiv`

**Remaining work:**
- Cranelift JIT pipeline wired end to end for all supported constructs
- All supported frontend matrix tests pass through JIT
- Backend output matches interpreter on every supported test
- Structured errors for all unsupported constructs
- Differential harness runs automatically on every PR
- Deterministic output — same program always produces same output

Done when:
- Every hard blocker in the 0.1 release gates is satisfied
- This is the line. When this phase closes, 0.1 backend ships.

---

## Post-0.1 — Compiler Targets 🔲

**Phase 16 — Cranelift AOT**

Goal: real compiled artifacts via Cranelift. Same dependency as JIT, extended to produce object files and executables. This is the natural next step after JIT is proven — no new dependency, just extending what is already there.

Note: this phase will split into sub-phases when you get close. Linker integration alone is significant work. Do not try to land object emission, executable emission, and linker flow all at once.

- Object file emission via Cranelift
- Target triple support — Windows x64, Linux x64 minimum
- Object format support — ELF on Linux, COFF on Windows
- Symbol handling and export rules
- Runtime linkage expectations documented
- Executable emission
- Linker flow
- Platform handling
- Basic release build workflow — cx build --release

Done when:
- Cx produces a real compiled executable via Cranelift
- Output is correct and matches interpreter behavior
- Basic release build workflow exists for supported targets

---

**Phase 17 — LLVM AOT**

Goal: maximum optimized ahead-of-time compilation via LLVM for production game engine builds.

Do not start this until Cranelift AOT is stable and the IR is proven correct across the full matrix. LLVM is downstream of backend correctness — it is not a substitute for it. The integration cost is a multi-week project on its own.

Why LLVM eventually: Cranelift produces working code fast. LLVM produces fast code correctly. For a game engine language where every cycle matters at production time, LLVM AOT is the right long-term target.

- LLVM IR lowering from Cx IR
- LLVM optimization pipeline integration
- Object and executable emission via LLVM
- Platform handling matching Cranelift AOT coverage
- Performance comparison — LLVM vs Cranelift AOT on representative game engine workloads

Done when:
- Cx can produce LLVM-optimized executables
- Output matches Cranelift output on all supported tests
- Performance is measurably better on representative workloads

---

**Phase 18 — FFI and C Boundary** *(post-0.1, design pass needed)*

Goal: external function calls and engine library interop.

- External function call lowering
- ABI-safe struct passing across the C boundary
- Engine library interop path — link against existing C/C++ engine libraries
- C-compatible function export — Cx functions callable from C

Design pass needed before implementation. C interop is nearly free if Cx emits C as a compilation target — revisit this decision when LLVM AOT is proven.

---

## Post-0.1 — Backend Quality 🔲

**Determinism — Minimal (0.1 required)**

Minimal determinism is a hard blocker for 0.1. Without it you cannot trust your debugging output.

- Same IR, same target, same input always produces the same observable output
- Stable IR printer output — same IR always prints the same string
- No random backend behavior — no unseeded randomness anywhere in the codegen path

**Determinism — Full Reproducibility (post-0.1)**

Full reproducible builds can wait. These are the extended guarantees:

- Reproducible binaries — byte-identical output for identical input on the same platform
- No timestamp or build-system leakage into output
- Golden reference outputs that never change without an explicit decision

**Data Layout Confidence Tests — Core (0.1 required)**

These land as part of Phase 8 and are required before 0.1 ships:
- Struct size assertions — test that structs have the expected byte size
- Field offset assertions — test that fields are at the expected offsets
- Array element stride assertions
- bool, enum, and TBool representation assertions
- These must pass on Windows x64 and Linux x64 before 0.1

**Data Layout Confidence Tests — Extended (post-0.1)**

- Cross-platform confidence suite — macOS, ARM64
- Larger matrix covering edge cases
- Exotic alignment and packing scenarios
- Platform divergence detection

---

---

## Post-0.1 — Debuggability and Tooling 🔲

The diagnostics foundation lands in Phase 7. These are the deeper tooling improvements that follow after 0.1 ships.

**Source Maps and Span Mapping**
- Richer source span attachment — spans preserved through lowering into codegen
- Backend error messages reference original source lines where possible
- Source map output format for external debugger integration

**Debugger Integration**
- DWARF debug info emission — line numbers, variable names, type info
- Integration with platform debuggers — gdb, lldb, Windows debugger
- Breakpoint support in JIT mode
- Variable inspection at runtime

**CFG Visualization**
- Optional CFG dump flag — visualize the control flow graph for a function
- Graphviz-compatible output format
- Useful for understanding complex lowering and branch merges

**Extended Backend Trace Tooling**
- Per-instruction trace mode showing IR instruction and generated machine code side by side
- JIT disassembly output for debugging codegen correctness
- Optional SSA value tracking through lowering

---

---

## Phase Dependencies

The ordering is not arbitrary. These are the hard dependency chains.

```
Phase 5  — branches          → required before Phase 10 loops
Phase 6  — calls             → required before meaningful Cranelift execution
Phase 7  — diagnostics       → required before Cranelift debugging is possible
Phase 8  — ABI and layout    → required before parity results are trustworthy
Phase 9  — intrinsics        → required before builtins and runtime behavior land; depends on frontend promoting print to a function
Phase 10 — loops             → required before full control flow surface is covered; for-loop lowering depends on Phase 11 range expressions
Phase 11 — surface area      → range expression lowering must complete before Phase 10 for-loops work
Phase 12 — harness           → defines what parity means — must exist before parity claims are made
Phase 13 — skeleton          → required before any JIT execution is attempted
Phase 14 — host boundary     → required before harness can capture JIT output reliably
Phase 15 — JIT 0.1 target    → closes only after all 0.1 hard blockers are satisfied

Known cross-roadmap dependencies:
- Frontend: print promoted to function → Phase 9 cannot close without it
- Frontend: compound assign syntax — frozen as `i += 1` (standard infix)
- Frontend: float type keyword — frozen as `f64`, landed 2026-03-22
```

Nothing in the post-0.1 compiler targets should start until Phase 15 closes.

---

## Progress Board

**Done**
- Semantic boundary (Phase 0)
- IR data model (Phase 1)
- Straight-line lowering (Phase 2)
- IR validator (Phase 3) + loop-var read-only enforcement (CX-40)
- Function lowering (Phase 4)
- if / else lowering (Phase 5)
- Backend trait interface change (Phase 0.5) — backend takes &IrModule
- IR pretty printer and diagnostics foundation (Phase 7) — --backend=validate, --debug-trace
- Function call lowering (Phase 6) — direct calls, arity/type validation, void calls resolved
- Loop lowering (Phase 10) — while, for, loop, break, continue, returns inside loops
- ABI and data layout Round 1 (Phase 8) — scalars, structs, arrays, enums, calling convention, IrType::TBool locked
- Surface area reduction (Phase 11) — all 0.1-scope constructs lower or produce named errors
- Phase 9 sub-packet 1 — is_cx_builtin guard, 7 tests, cx_runtime_intrinsics_v0.1.md
- Phase 13 — Cranelift lowering skeleton — IrType mapping, dispatch skeleton, CraneliftLoweringError
- JIT runtime host boundary — HostBoundary, JitExitCode, JitOutcome, JitExecutionError
- Phase 12 sub-packet 1 — differential harness shell, interpreter baseline, 4 tests
- Phase 14 sub-packets 1–2 — first JIT execution: arithmetic + memory ops, 12 tests
- Evaluation order specification — cx_eval_order.md, left-to-right verified in tests (CX-37)

**Active**
- ABI and data layout Round 2 (Phase 8) — str/strref layout, Handle<T>, TBool calling convention, unknown propagation still open
- Phase 9 sub-packet 2 — runtime intrinsics dispatch (CX-38, in Human Review)
- Phase 14 sub-packets 3–4 — Compare/Jump/Branch (CX-27/CX-41), direct calls (CX-30), in Human Review
- Phase 15 sub-packets 1–3 — PtrOffset/PtrAdd (CX-32), SsaBind/ConstFloat/Cast (CX-33), float arith (CX-36), in Human Review
- Phase 12 sub-packets 2–3 — JIT comparison (CX-31), full harness (CX-34), in Human Review

**Next — 0.1 Path**
- Runtime intrinsics boundary (Phase 9) — sub-packets 3–4 blocked on design/frontend
- Cranelift JIT — 0.1 target (Phase 15) — depends on Phase 14 sub-packets 3–4 and Phase 15 sub-packets 1–3 landing

**Post-0.1**
- Cranelift AOT
- LLVM AOT
- FFI and C boundary
- Full reproducible builds
- Extended data layout confidence suite
- Source maps and debugger integration
- CFG visualization
- Extended backend trace tooling

**Separate Roadmap**
- GPU layer — Cx Platform and GPU Roadmap
- Window and screen system — Cx Platform and GPU Roadmap

---

## Key Changes — v4.2 (2026-05-06)

Roadmap reconciled with all shipped work since v4.0. This update covers the substantial execution pipeline progress that landed between v4.0 and now.

- **Phase 11 complete** — all 0.1-scope constructs in the IR lowering layer either lower correctly or produce a named structured error; DotAccess (field reads/writes), array indexing, array literals, array element writes, void function calls, Range named error, MethodCall named error, PtrAdd instruction all resolved
- **Phase 13 done** — Cranelift lowering skeleton: IrType→Cranelift type mapping, module/function/block traversal, named CraneliftLoweringError for every unsupported construct (`src/backend/cranelift/mod.rs`)
- **JIT runtime host boundary done** — HostBoundary, JitExitCode, JitOutcome, JitExecutionError types; execution contract documented (`src/backend/cranelift/host_boundary.rs`)
- **Phase 12 sub-packet 1 done** — differential harness shell: TestFixture format, collect_matrix_tests, run_interpreter, interpreter baseline sweep (`src/diff_harness.rs`)
- **Phase 14 sub-packets 1–2 done** — first real Cranelift JIT execution: arithmetic (add/sub/mul/div/rem), ConstInt, Return, Alloca, Load, Store; 12 tests passing
- **Evaluation order spec done** — `docs/backend/cx_eval_order.md` created; left-to-right evaluation order tested in `lower.rs` (CX-37); closes the 0.1 hard blocker for evaluation order stability
- **IR validator loop-var read-only done** — ReadOnlyLoopVar enforcement added to validator (CX-40); closes the known gap noted in Phase 10
- **Phase 6 void-call limitation resolved** — `lower_void_call()` in Phase 11; Phase 6 entry updated
- **Phase 10 loop-var gap resolved** — noted in Phase 10 entry; validator now enforces it
- **Human Review pipeline** — Phase 14 sub-packets 3–4, Phase 15 sub-packets 1–3, Phase 12 sub-packets 2–3, Phase 9 sub-packet 2 all in Human Review; Active section and Progress Board updated
- Active section updated — Phase 11 removed (done); Phase 8 Round 2 + in-flight Human Review work remains active
- Progress Board updated to reflect current state

---

## Key Changes — v4.1 (2026-05-06)

- Phase 9 sub-packet 1 done — audit complete, `is_cx_builtin()` guard added in `lower.rs`, 7 tests, `docs/backend/cx_runtime_intrinsics_v0.1.md` created
- Phase 9 sub-packets 2–4 defined with explicit blocking conditions
- Progress board updated to reflect Phase 9 partial progress

---

## Key Changes — v4.0 (2026-05-03)

Roadmap reconciled with actually-shipped work. No design changes; this update records what landed since v3.1.

- Phase 6 (function call lowering) marked Done — Stage 2b (lowering) and Stage 3 (validator) landed 2026-03-22; void calls produce a structured error, `IrType::Void` tracked in Phase 11
- Phase 8 Round 1 marked Done — scalar, struct, array, and enum layouts locked; calling convention locked (C ABI, copy params post-0.1); `IrType::TBool` added; str/strref layout and unknown propagation remain open
- Phase 10 (loop lowering) marked Done — while, for, loop, break, continue, returns inside loops working; for-loop range dependency resolved inline; loop-var read-only validator enforcement noted as a gap
- Phase 11 (surface area reduction) marked Active — compound assign, unary, `IrType::Ptr`, `IrInst::Alloca`/`Load`/`Store`/`PtrOffset`, struct registry, and struct literal lowering all landed; range, DotAccess, array indexing, method calls, and void calls still open
- Active phase updated from Phase 6 to Phase 11
- Progress board updated to reflect current state

---

## Key Changes from v3.0

- Minimal determinism promoted to 0.1 hard blocker — same IR, same target, same input, same output
- Core layout confidence tests promoted to 0.1 required — struct sizes, field offsets, array strides, bool/enum/TBool
- Evaluation order added to 0.1 hard blockers — assignment side effects must match semantic layer exactly
- No-panic guarantee added to 0.1 hard blockers — backend must not panic on any valid IR
- Philosophy sharpened — optimization is never allowed to change observable Cx behavior
- JIT runtime host boundary added as explicit section — process startup, exit code, stdout capture, runtime failures
- Phase dependency map added — explicit dependency chain from Phase 5 through Phase 15
- Post-0.1 debuggability section added — source maps, debugger integration, CFG visualization, trace tooling
- Data layout confidence tests split — core tests in 0.1, cross-platform matrix post-0.1
- Determinism split — minimal guarantee in 0.1, full reproducible builds post-0.1
- Support matrix wording tightened — "after frontend semantics are frozen" not "once stable"
- Cranelift skeleton upgraded with error context — validate, lower, codegen, runtime boundary in messages
