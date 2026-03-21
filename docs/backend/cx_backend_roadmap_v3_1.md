# Cx Compiler Backend Roadmap
v3.1 — 2026-03-17

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

## Active 🔄

**Phase 6 — Function Call Lowering** *(next up)*

Goal: let already-lowered functions call each other.

See Up Next section for details.

---

## Up Next — Core Compiler Work 🔲

**Phase 6 — Function Call Lowering**

Goal: let already-lowered functions call each other.

- Direct call lowering
- Argument lowering with arity validation
- Type-checked call signatures at IR boundary
- Call result lowering
- Direct vs intrinsic call distinction established
- Call validation in validator
- Tests for call-containing IR

Done when:
- Simple direct calls lower successfully
- Call results flow into assignments, returns, and expressions
- Validator accepts call-containing IR
- Arity and type mismatches produce structured errors

---

**Phase 7 — IR Pretty Printer and Diagnostics Foundation**

Goal: human-readable IR output and the foundation for all backend observability. Must exist before Cranelift is touched. When Cranelift fails on something the first question is always what did the IR actually look like — without this you are debugging blind.

- Text format for IrModule, IrFunction, IrBlock
- Each instruction prints with its ValueId, type, and operands
- Each block prints with its params and terminator
- Optional source span comments where spans are available
- Stable textual format — same IR always prints the same way
- Readable block param printing
- IR dump triggered automatically on lowering test failures
- IR dump triggered automatically on validator failures
- Validate-only backend mode — run validation without codegen
- Optional verbose trace flag for instruction-by-instruction output

Done when:
- Any IrModule can be printed to a stable, readable string
- Lowering and validator test failures include IR dump automatically
- A developer can read the output and understand what lowered
- Validate-only mode works as a standalone diagnostic path

---

**Phase 8 — ABI and Data Layout**

Goal: freeze backend-visible representation of all core runtime types. For a game engine language where predictable memory layout is a core selling point, correct machine output is not fully defined until layout rules are documented, implemented, and tested.

Without this phase, parity testing is incomplete — the backend could produce output that matches the interpreter by accident rather than by design.

- Scalar layout rules — t8, t16, t32, t64, t128, f64, bool
- bool representation — 0 for false, 1 for true, backend-visible
- Enum layout — tag representation, variant layout
- Struct field layout — ordering, alignment, padding rules
- Array element layout — stride, alignment
- str and strref layout at backend boundary
- Handle<T> runtime representation
- TBool representation — 0/1/2 wire format preserved through backend
- Function calling convention — how arguments are passed and returned
- Parameter passing rules — by value, by reference, for each type
- Return value rules — small values, large values, void
- Synthetic main vs real main conventions documented
- Layout tests — struct sizes, field offsets, array strides match documented rules
- Target platform matrix explicit — Windows x64 and Linux x64 for 0.1

Done when:
- Every core Cx type has a documented backend representation
- Layout tests validate that representation
- Calling convention is documented for supported targets
- No layout rule is implicit or assumed

---

**Phase 9 — Runtime Intrinsics Boundary**

Goal: define exactly what the backend lowers as pure IR versus what becomes a runtime call. Without this the backend has ad hoc hooks scattered through the lowering code instead of a clean, testable boundary.

- Define backend-visible runtime entry points
- Categorize every builtin — pure IR, runtime call, or intrinsic
- print — classified and lowered correctly now that it is promoted to a function
- Allocation operations — arena, handle registry interactions
- Handle registry operations — insert, get, remove, stale detection
- String boundary operations — str copy-on-boundary, strref validity
- Error and panic paths — how they surface through the backend
- Define calling signatures for all runtime entry points
- Document ownership and lifetime expectations at each boundary
- Lower all builtins through structured intrinsics, not ad hoc hooks
- Tests confirm each intrinsic lowers and executes correctly

Done when:
- Every builtin has a documented classification
- No ad hoc runtime hooks exist in the lowering code
- All intrinsics have tests
- The boundary between IR math and runtime calls is explicit and stable

---

**Phase 10 — Loop Lowering**

Goal: lower loop constructs after branch lowering is stable.

- while loop lowering — header, body, exit blocks
- for loop lowering
- Loop-carried values through block params
- Backedge handling
- break lowering — exits to loop exit block
- continue lowering — jumps to loop header
- Returns inside loop body handled correctly
- Loop-aware SSA — values defined inside loop not visible outside
- Validator support for loop CFG — backedges, dominance-like invariants

Done when:
- while and for loops lower into valid CFG
- break and continue lower correctly
- Returns inside loops are handled
- Validator accepts loop-containing IR

---

**Phase 11 — Surface Area Reduction for Supported 0.1 Subset**

Goal: shrink the unsupported surface area intentionally. Every construct in this phase either gets supported or gets a documented, structured rejection. Nothing is silently unsupported.

- CompoundAssign — += style forms
- Unary expressions
- Range expressions
- Dot access and field indexing forms
- Array indexing forms
- Non-typed param kinds
- Method call lowering semantics
- Assignment semantics with field and index writes
- Side-effect ordering — evaluation order documented and stable
- Temporary evaluation order — consistent with semantic layer

Done when:
- Every construct either lowers or produces a named, structured error
- The supported and unsupported lists in this document are accurate
- No construct silently produces wrong output

---

**Phase 12 — Differential Backend Harness**

Goal: make parity a real tracked system, not a vague aspiration. The frontend has a 46+ test matrix. This phase builds the infrastructure to run that same matrix through the backend and compare results automatically.

This phase should be treated as a mini-system in its own right — not just a phase.

- Run every frontend matrix test through the interpreter, capture stdout, exit code, and errors
- Run the same test through the Cranelift backend
- Compare stdout — must match exactly
- Compare exit code — must match exactly
- Compare structured error family for expected-failure tests
- Report divergences automatically with IR dump on mismatch
- Fixture-based test format — each test has known-good interpreter output as golden reference
- Negative tests — unsupported constructs must return structured error, not crash
- Per-feature parity checklist — track which language features have backend coverage
- Determinism check — same IR plus same target always produces same output

Done when:
- Harness runs automatically in CI
- All supported frontend matrix tests pass through backend with matching output
- All unsupported constructs produce structured errors
- Divergences between interpreter and backend are surfaced immediately
- The harness is the definition of parity — not a vague description

---

**Phase 13 — Cranelift Lowering Skeleton**

Goal: teach Cranelift to consume IR shape safely before any execution is attempted.

- IrType to Cranelift type mapping — complete for all supported types
- Module traversal
- Function lowering skeleton
- Block lowering skeleton
- Instruction dispatch skeleton
- Structured error type and error code family for backend failures
- Structured not-implemented errors for every unsupported construct
- Explicit separation between valid-but-unsupported IR and invalid IR
- No AST or semantic leakage into the backend — IR is the only input
- Backend error messages include phase and context — validate, lower, codegen, runtime boundary

Done when:
- Cranelift backend can walk any valid IR safely
- Every unsupported instruction produces a structured named error
- No panics on valid IR under any condition

---


---

**JIT Runtime Host Boundary**

Before any JIT execution, these must be explicitly defined and documented.

- Who owns process startup and shutdown in JIT mode
- How the main function result becomes an exit code — what values map to what codes
- How stdout and stderr are surfaced during JIT execution — where they go, how the harness captures them
- How runtime failures surface — arena violations, handle stale access, boundary errors, panics
- How the differential harness hooks into JIT execution — what it captures and compares
- How unsupported construct errors reach the test harness

Done when:
- Every execution boundary is documented
- The differential harness can reliably capture and compare program output
- Runtime failures produce readable structured output, not silent corruption

---

**Phase 14 — First Executable Cranelift Slice**

Goal: first real backend execution. The simplest possible program runs through the full JIT pipeline and produces correct output.

First supported subset:
- Constants
- Arithmetic
- Returns
- Synthetic main
- One direct function call

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
Phase 9  — intrinsics        → required before builtins and runtime behavior land
Phase 10 — loops             → required before full control flow surface is covered
Phase 12 — harness           → defines what parity means — must exist before parity claims are made
Phase 13 — skeleton          → required before any JIT execution is attempted
Phase 14 — host boundary     → required before harness can capture JIT output reliably
Phase 15 — JIT 0.1 target    → closes only after all 0.1 hard blockers are satisfied
```

Nothing in the post-0.1 compiler targets should start until Phase 15 closes.

---

## Progress Board

**Done**
- Semantic boundary
- IR data model
- Straight-line lowering
- IR validator
- Function lowering
- if / else lowering

**Active**
- Function call lowering

**Next — 0.1 Path**
- IR pretty printer and diagnostics foundation
- ABI and data layout
- Runtime intrinsics boundary
- Loop lowering
- Surface area reduction
- Differential backend harness
- Cranelift skeleton
- First executable backend slice
- Cranelift JIT — 0.1 target

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
