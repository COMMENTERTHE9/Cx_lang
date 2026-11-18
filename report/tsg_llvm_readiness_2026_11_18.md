# Cx — TSG engine readiness and LLVM AOT plan

Investigation and planning only. No compiler source was changed; nothing was merged.

- **Date:** 2026-11-18
- **Audited head:** `submain` @ `d4b2c06` (2026-10-28), which is the real development head. `main` @ `24b6820` was audited first and is used only for comparison.
- **Environment:** Linux x86-64, rustc/cargo 1.97.0, LLVM/clang 18.1.3, debug builds.
- **File paths** below refer to the `submain` tree unless marked otherwise.

Every statement is labelled by how it was established:

| Label | Meaning |
|---|---|
| **Ran** | I executed it on this machine and observed the result |
| **Read** | Established by reading the code or document at the cited location; not executed |
| **Inferred** | A conclusion drawn from the evidence; could be wrong |
| **Recommend** | My proposal; needs the owner's decision |

---

## 1. Summary

1. **The LLVM backend has never existed.** `src/backend/llvm/aot.rs` is a 6-line stub that returns an error, there is no LLVM dependency in `Cargo.toml`, and that is true at every commit reachable in this clone (back to 2026-03-21; the clone is shallow). `src/backend/cranelift/aot.rs` is the same kind of stub. This is a first implementation, not a recovery. (Read, Ran)
2. **`main` is 86 commits behind `submain`** and still contains silent miscompiles that `submain` fixed in August (struct and array assignment aliasing on the JIT). Anything started from `main`, including this session, sees a stale compiler. (Ran)
3. **`submain` is healthy on its own gates, on Linux as well as Windows:** 314 default tests, 493 JIT tests, parity 602 PASS / 48 SKIP / 0 PARITY_FAIL over 650 fixtures. (Ran)
4. **Those gates do not mean the two engines agree.** I found three silent interpreter/JIT divergences on `submain` (same exit code, different output) that no fixture covers and `docs/known_issues.md` does not list. All three are in IR lowering, so an LLVM backend would inherit them. (Ran — §4.3)
5. **One hard blocker stands between the current IR and any AOT backend:** string literals are lowered as addresses inside the compiler process. Everything else an AOT backend needs is small and well-bounded: 17 runtime functions, a 14-instruction IR, and a Cranelift emitter of about 1,800 lines. (Read, Ran — §5)
6. **The Cranelift JIT generates code at about clang `-O0` speed.** On three kernels, clang `-O2` was 2.2–4.5× faster. That is the measured case for LLVM. (Ran — §5.5)
7. **Cx cannot yet express an engine.** Missing on both backends: nested field access (`a.b.c`), `f32`, unsigned integers, bitwise operators, any math function, heap allocation and dynamic collections, references, function values, FFI, and file I/O. These are language-design items, not lowering gaps. (Ran — §6)
8. **TSG's requirements are not written down anywhere in the repository.** The only mention is the roadmap line "2029+ — TSG playable". The engine gap list in §6 is therefore built from general engine needs and has to be checked against the real design. (Read — §3)

**Recommended first task:** fix the three silent lowering divergences, with fixtures. **Recommended first milestone:** an AOT-ready IR and runtime (§8, M1), which unblocks Cranelift AOT and LLVM equally.

---

## 2. What was examined

| Area | How |
|---|---|
| Repository | Both `main` and `submain` built with and without `--features jit`; all gates run on each |
| Behaviour | About 110 small probe programs run on the interpreter and the Cranelift JIT, on `submain` |
| IR | Dumped with `--backend=validate` for representative programs |
| Code | `src/ir/*`, `src/backend/*`, `src/main.rs`, lexer, semantic types, builtins, prelude. Three parallel code/history audits; I re-ran every surprising claim they made before using it |
| Documents | `docment/ROADMAP.md` (both branches), `report/restart_2026_10_28.md`, `docs/AGENT_OPERATING_DOCTRINE.md`, `docs/backend/*`, `docs/known_issues.md` (section list and selected entries), `docs/cx_syntax.md`, `CONTRIBUTING.md`, CI workflow |
| History | 142 daily logs on `main`, 82 unmerged `daily-log-*` branches (sampled; all diff-stat'ed), the 86 `submain` commits, `deep-dive-pillar-3/4` reports, open PRs and issues |
| Feasibility | One program's IR hand-translated to LLVM IR and built with clang; three kernels benchmarked against C |

**Not verified:** Windows behaviour; the Linear tracker (CX-### tickets); branches and evidence that exist only on the owner's machine; the correctness of open PR #476; interpreter performance.

---

## 3. Recovered context

### 3.1 Standing decisions this plan keeps

| Decision | Source |
|---|---|
| The interpreter is the reference for defined behaviour, but is not assumed infallible | Doctrine v1.1; restart report |
| Sequence: library/runtime maturity → Cranelift object emission → linking → target/ABI conformance and C boundary design → LLVM optimization → bootstrapping | `report/restart_2026_10_28.md`, "Recommended next milestone" |
| "Do not start [LLVM] until Cranelift AOT is stable"; do not land object emission, executable emission and linker flow at once | `docs/backend/cx_backend_roadmap_v3_1.md`, Phases 16–17 |
| "Whole programs compile and agree should precede AOT and wider feature work" | Restart report |
| The 0.4 bucket is not one implementation packet | `docment/ROADMAP.md` |
| Refuse rather than miscompile; never weaken expectations; file a known-issue before fixing | Doctrine; daily logs |
| Preserve established syntax, semantics, copy/memory behaviour and ABI; ask on genuine language-design decisions | Doctrine v1.1 |
| Aggregates are values (Model B); nested arrays are contiguous and row-major (Model A) | `known_issues.md` §27, §28 |
| C calling convention; struct fields in declaration order; all integers signed; enums are a 1-byte tag | `docs/backend/cx_abi_v0.1.md` |
| Static monomorphization only, capped at 64 instantiations per template | `src/ir/monomorphize.rs`; daily log 07-13 |
| Graphics (Vulkan/DX12) not before 1.0; the 0.4 math layer is graphics preparation | `docment/ROADMAP.md` |
| Human approval for `main` and releases; no auto-merge; fresh backend branches from `submain` | `CONTRIBUTING.md`; doctrine |

### 3.2 The work queue that is already authorized

From the restart report, in order: shared module storage for bool, string, Handle and composite values; owned Handle payloads; outer-local capture for nested functions; concrete generic-field inference; general interpolation; then runtime/I/O, executable output, platform checks and measured performance. This plan folds that queue into milestone E1 rather than restating it.

### 3.3 Where the documents disagree with the code

- `docment/ROADMAP.md` still lists `copy_into` and nested functions as open. PR #474 landed both (capture-free nested functions only) on the same day. (Ran)
- The roadmap's "design gate, no code" section for multidimensional arrays is out of date: Model A is implemented and rank 3 works. (Ran)
- The roadmap quotes parity 528/57; the head is 602/48. (Ran)
- `docs/cx_syntax.md` §16 shows `impl Player { ... self.health ... }`. That form is a parse error; the working form is `name: impl (p: Player) { ... }`. (Ran)
- `docs/backend/cx_abi_v0.1.md` says a string is `(*const u8, u32)`; the code passes a pointer to a `{i64, i64}` descriptor. It also omits return slots, inline nesting, Result and Handle packing, and module globals. (Read)
- `docs/backend/cx_runtime_intrinsics_v0.1.md` lists one intrinsic; the code has 17. (Read)
- `Cargo.toml` reports version `0.1.0`; the latest tag is `v0.3.3`. (Ran)

### 3.4 Context that is missing, and how to bring it in

I did not have access to any previous AI conversation, and the repository contains no transcripts. No daily log cites a ChatGPT or Gemini conversation. The following are referenced but absent:

| Missing | Why it matters | Where it is referenced |
|---|---|---|
| What TSG is: genre, scale, platforms, graphics API, threading model, performance targets | Decides which engine gaps in §6 are real and in what order | Roadmap line only |
| "Cx Platform and GPU Roadmap" (VRAM registry, GS types, `.drop(fence)`, window system) | Defines the GPU-facing language features | `cx_backend_roadmap_v3_1.md` |
| `cx_arrays.md`, `cx_4d_arrays.md` | Design intent for arrays | `docment/ROADMAP.md` |
| Memory-model design: `rc<T>`, `shared<T>`, `NullPoint<T>`, region invalidation, Handle-backed containers, the `hashweb` collection | The largest open design area for an engine | `docs/ROADMAP_v3.1.md`, `docs/frontend/ROADMAP.md` |
| Stdlib v1 contents, math-layer contents, Ricey (the package registry), NOD Protocol, what "bootstrapping begins/completes" means | All named in the 0.4/1.x lines with no backing detail | `docment/ROADMAP.md` |
| Linear tickets (CX-###), tracker items #001–#042, R6, D1/D2, "open-decision #2" | Possible duplicate or conflicting tracked work | Commits and logs; the restart report could not reach Linear either |
| The owner's restart/autonomy instructions | Basis of doctrine v1.1 | Doctrine changelog |
| Local-only branches `backend/nested-functions`, `backend/jit-printing`, the Handle and storage prototypes, `target/restart-2026-10-28/` | Unpushed work that could overlap with E1 | Restart report |

**How to incorporate it.** Export the relevant conversations and documents as files (any format) and put them in a directory in the repository or hand them to a session. I will extract each decision into a single register with its source, date and status (locked, open, or superseded), compare it against §3.1 and the roadmap in §8, and bring every conflict to the owner as a question with a recommendation. Nothing in an export overrides a recorded decision silently.

The four `deep-dive-pillar-*` branches (2026-05-24) are on origin but were never merged. Their architecture findings are still relevant and are used in §7.

---

## 4. Current state of Cx on `submain`

### 4.1 Gates (Ran, Linux)

| Check | `main` @ 24b6820 | `submain` @ d4b2c06 |
|---|---|---|
| `cargo test` | 250 / 0 | 314 / 0 |
| `cargo test --features jit` | 426 / 0 | 493 / 0 |
| Fixtures | 414 | 650 |
| JIT parity | (374 / 40 / 0 recorded) | 602 PASS / 48 SKIP / 0 PARITY_FAIL |
| `run_matrix.sh` | 414 / 414 | not re-run |

The `submain` numbers match the Windows figures in the restart report, so the two platforms agree at this commit.

### 4.2 Capability matrix (Ran)

**Works on both the interpreter and the JIT**

- Signed integers `t8`–`t64` with wrapping arithmetic; `f64`; implicit int/float conversion on typed assignment; `bool` and three-state bool.
- Structs, methods with a named receiver, struct returns, value semantics for every bind, parameter and return.
- Fixed-size arrays including nested (`[2: [3: t64]]`, `a:[i]:[j]`), arrays in structs, arrays of structs, array parameters and returns.
- `.copy` parameters (by address, written back on return).
- Tag-only enums and `when`; `Result<T>` with `?`; generic functions and structs; gene/phen with operator overloading.
- `if`/`else`, `while`, `for` over ranges, `loop`, labelled `break`/`continue`; recursion and mutual recursion (256-frame limit on both).
- Multi-file imports; module-level numeric and enum state shared across functions; top-level `const`; capture-free nested functions.
- Static strings: literals, equality, identifier interpolation, strings in structs and arrays; `len`.
- Bounds checks, division guards, `assert`, `exit`.

**Interpreter only — the JIT refuses with exit 127**

- Runtime string concatenation; `char`; `while-in`; `read`/`input`.
- `t128` printing and division.
- Module-level `bool`, struct or array used inside a function.
- `Handle<f64>` and `Handle<struct>`; `Result` with a float, aggregate or nested payload.
- A generic struct passed to a function.

The 47 refusals across the fixture corpus group as: enum-identity refusals of invalid programs (12), untyped values (7), branch-type mismatches (5), `t128` printing, division and float conversion (5), interpolation (4), `while-in` (3), and 11 singletons.

**Absent on both — parse or semantic error**

| Feature | Probe result |
|---|---|
| Nested field access `b.pos.x`; element field `ps:[1].x`; `h.val.x` | Parse error |
| Bitwise `&`, `\|`, `^`; shifts | Parse error (no tokens) |
| `f32`; unsigned integers | Type mismatch |
| Hex literals | `0xFF` reads as `0` then identifier `xFF` |
| Math functions (`sqrt`, …); `std/math` import | Undefined function; "stdlib not yet bundled" |
| Explicit cast (`x as t8`) | Parse error |
| Array fill or uninitialized array | No syntax; a `[1000: t64]` needs 1,000 literal elements |
| Generic type in a parameter annotation (`p: Pair<f64>`) | Parse error |
| `const` inside a function body | Parse error |
| Data-carrying enums | Parse error |
| Binding a `Result` payload in `when` (`Ok(v) =>`) | Undeclared variable |
| Exhaustive `when` over an enum without `_` | Rejected as non-exhaustive |
| Function values; closures | Semantic / parse error |
| `extern` declarations (FFI) | Parse error |
| File I/O; string indexing | Undefined function / "cannot index 'str'" |
| Heap allocation, dynamic arrays, maps, references, threads | No syntax exists |

### 4.3 Interpreter/JIT divergences on `submain` (Ran)

Three are silent: both backends exit 0 and print different output. None is in `docs/known_issues.md` and no fixture covers them.

**1. A field assignment evaluates its right-hand side twice on the JIT.**

```cx
struct P { x: t64, y: t64 }
calls: t64 = 0
fnc: t64 next() {
    calls += 1
    calls * 10
}
p: P = P { x: 0, y: 0 }
p.x = next()
print(p.x)      // interpreter 10, JIT 20
print(calls)    // interpreter 1,  JIT 2
```

**2. `when` on a string compares descriptor addresses on the JIT and never matches a literal arm.**

```cx
fnc: t64 code(s: str) {
    when s {
        "alpha" => 1,
        "beta" => 2,
        _ => 0,
    }
}
print(code("alpha"))   // interpreter 1, JIT 0
print(code("beta"))    // interpreter 2, JIT 0
```

**3. `while` on an unknown bool: the interpreter skips the loop, the JIT enters it.**

```cx
flag: bool = ?
n: t64 = 0
while (flag) {
    n += 1
    if n >= 3 { break }
}
print(n)               // interpreter 0, JIT 3
```

Other differences, none silent:

- A user function named `cx_trap` replaces the runtime's trap handler. A failing `assert` then runs the user's body and the process dies with SIGILL (exit 132). Only some runtime names are reserved.
- Float division by zero is rejected by both, but with different exit codes and messages (interpreter 1 with a diagnostic; JIT 126 with a generic trap line).
- A user function named `main` is ignored by the interpreter (never called, exit 0) and refused by the JIT.
- `--backend=<anything unknown>` silently runs the interpreter.

On `main` only, there are four further silent divergences (struct and array assignment and parameter aliasing). `submain` fixed them in `f291116` and `4fe3fd2`.

---

## 5. Backend state and what an LLVM backend can reuse

### 5.1 The IR (Read, Ran)

- 14 instructions: `GlobalAddr`, `ConstInt`, `ConstFloat`, `SsaBind`, `Binary` (add, sub, mul, div, rem), `Compare`, `Call` (direct, by name), `Cast`, `Alloca`, `ArrayAlloca`, `PtrOffset`, `PtrAdd`, `Load`, `Store`.
- 4 terminators: `Jump`, `Branch`, `Return`, `Trap`.
- 10 types: `I8`–`I128`, `F64`, `Bool`, `TBool`, `Ptr`, `Void`.
- SSA with block parameters. Aggregates do not exist in the IR: a struct, array or string is a `Ptr` plus byte offsets, and layout lives in lowering's side tables.
- Safety checks are expressed **in the IR** as branches to `Trap`: array bounds, division by zero, `INT_MIN / -1`, handle validity, module-state lifecycle. A second backend gets them for free.
- Three behaviours are added by the Cranelift emitter and are **not** in the IR: the recursion-depth guard (calls to `cx_depth_enter`/`cx_depth_exit` in functions that can recurse), the `Trap` → `cx_trap` call, and `f64` remainder → `fmod`. A second backend must reproduce them.

### 5.2 Blockers for any AOT backend

| # | Blocker | Evidence |
|---|---|---|
| 1 | String literals, enum names and Result text are leaked in the compiler process and emitted as `const i64 <address>` + `cast → ptr` | Read `src/ir/lower.rs:3789-3810`; Ran: two dumps of one program differ in that constant |
| 2 | The runtime (17 `cx_*` functions, the handle registry, the depth counter) exists only as Rust functions inside the compiler binary | Read `host_boundary.rs:272-728, 783-799` |
| 3 | The entry point is a synthetic IR function named `main`; user symbols are unmangled and can collide with runtime or libc names; import mangling uses `@` (`math@2$add`), which ELF reserves for symbol versions | Ran (`cx_trap` hijack; a user `malloc` is accepted) |
| 4 | Layout is computed for the compiler's host only; `Ptr` is hard-coded to 8 bytes; Result packing assumes little-endian | Read `src/ir/types.rs`, `lower.rs:7089-7147` |
| 5 | The `Backend` trait is `execute(&IrModule)`; the CLI has no output path, no build mode | Read `src/backend/mod.rs`, `src/main.rs` |
| 6 | The IR validator cannot serve as a gate: it rejects IR the JIT runs correctly (`cx_str_eq`, `cx_handle_*` are unknown to it) and the JIT path never calls it | Ran |

### 5.3 What carries over

- **To Cranelift AOT:** most of the emitter. `compile_ir_function` takes a concrete `JITModule` but calls only methods of Cranelift's `Module` trait, and `build_cl_signature` is already generic over it. About 1,000 of the 1,830 non-test lines carry over nearly unchanged; about 110 are JIT-only. (Read; not compiled against `cranelift-object`, which is not yet a dependency.)
- **To LLVM:** none of the emitter, since the code-generation APIs differ. What carries over is everything upstream of it — all of `src/ir` (lowering, monomorphization, layouts, `call_shapes.rs`), the recursion analysis, the runtime contract, the fixtures and the differential harness. The Cranelift emitter serves as a 600-line specification of what each instruction means.

### 5.4 Feasibility check (Ran)

I hand-translated the IR dump of a small program (a module global, a struct, a string literal, a function call) into LLVM IR, wrote a 12-line C runtime for the four functions it calls, and built it with `clang -O2`. The result was a 16.6 KB executable depending only on libc, with output identical to the interpreter's. The mapping from Cx IR to LLVM IR was one-to-one. This was a scratch experiment and is not in the repository.

### 5.5 Measured performance (Ran)

Wall time, same machine, includes process start and JIT compilation.

| Kernel | Cranelift JIT | clang `-O2` (C) | clang `-O0` (C) | JIT vs `-O2` |
|---|---|---|---|---|
| Integer loop, 400 M iterations | 1.07 s | 0.24 s | 1.05 s | 4.5× slower |
| Array loop, 256 M element updates | 0.69 s | 0.20 s | 0.69 s | 3.5× slower |
| `f64` struct fields, 200 M iterations | 0.52 s | 0.24 s | 0.48 s | 2.2× slower |

The interpreter is about 75–240× slower than the JIT on a recursive and an iterative loop (debug build of the compiler).

### 5.6 What the IR lacks for *good* LLVM output (Read, Inferred)

- No block-copy instruction. Aggregate copies are unrolled into one load and one store per scalar. Passing one `[64: t64]` by value produces 321 load/store instructions, and every aggregate argument is copied twice (caller, then callee).
- Pointers are untyped byte offsets: no aliasing, bounds, or read-only information.
- `Alloca` appears mid-function and inside loops. Cranelift treats these as fixed frame slots; LLVM would grow the stack per iteration unless the emitter hoists them to the entry block.
- No shifts or bitwise instructions, so Result packing goes through memory.
- `Trap` has no reason code and is not marked cold or non-returning.
- No source locations in the IR, so no debug info.
- Module globals carry a lifecycle byte that is checked on every access.

None of these block a correct LLVM backend. They determine how much `-O2` can recover.

---

## 6. What an engine needs that Cx does not have

TSG's requirements are not documented (§3.4), so this list is drawn from what any real-time engine needs. Status is from §4.2.

| Need | Status | Kind of work |
|---|---|---|
| Standalone executables and libraries | No AOT backend | Backend (M1–M3) |
| Optimized code | JIT at `-O0` level | Backend (M4) |
| Calling the OS, GPU and C libraries | No FFI of any kind | Design, then implementation (E5) |
| Access to nested data (`world.player.pos.x`, `bodies:[i].vel`) | Parse error | Language (E2) |
| `f32`, unsigned integers, bitwise operations, hex literals | Absent | Design, then implementation (E3) |
| Math functions | Absent; no stdlib is bundled | E3, stdlib |
| Large or runtime-sized buffers | Arrays are fixed-size and must be written out element by element | E2 (fill), E4 (dynamic) |
| Heap allocation, dynamic arrays, maps, growable strings | Only `Handle<T>` for scalars | Design-heavy (E4) |
| Mutating or reading large data without copying it | Aggregates are values; `.copy` copies in and out. No reference or slice type | Design-heavy (E4) |
| Tagged unions | Enums are tag-only | E4 |
| Callbacks and event dispatch | No function values or closures | E6 |
| File I/O, time, process arguments | Absent | Stdlib on top of E5 |
| Threads, atomics, a job system | Absent | E7 |
| Multi-file builds, packages | Relative imports only; whole program is one unit | Tooling |
| Debugger support, source positions | Some diagnostics still report position 0; no debug info | Tooling |

Limits that will bite at engine scale: the 256-frame call limit, the 64-instantiation generic cap, enums of at most 256 variants, and `Result` payloads no larger than a machine word. (Read)

---

## 7. Testing and architecture risks

### 7.1 Testing gaps

- **Parity is measured against stored expectations, not against the interpreter.** A program with no fixture is unchecked, which is how the three divergences in §4.3 exist under a "0 PARITY_FAIL" gate. The restart report says this in its own words. (Read, Ran)
- **Fixture categories are coarse.** 534 of 650 fixtures fall into "Other". (Ran)
- **CI runs on Linux only; development is on Windows.** No single run covers both. (Read)
- **`run_matrix.sh` accepts any non-zero exit for an expected failure**, and builds without the JIT feature. (Read)
- **The IR validator is out of the loop** (§5.2, item 6).
- **No fixtures for compiled artefacts**, because none exist yet. There is also no test that IR output is reproducible, and today it is not.
- **Benchmarks** exist (`bench/`) but the runner is PowerShell-only and the recorded baseline predates most of the current lowering.

### 7.2 Architecture risks

1. **Two engines implement every semantic rule independently.** The May deep dive named this the top risk ("R1") and left the decision open; it is still open. LLVM does not add a third copy, because it consumes the same IR as Cranelift — provided that rules stay in lowering and out of the emitters. The three emitter-injected behaviours in §5.1 are exceptions to that and should be made backend-neutral before a second backend exists. (Read, Inferred)
2. **Three numeric type enums** (`ast::Type`, `SemanticType`, `IrType`) plus the Cranelift mapping, and an interpreter that stores every integer as `i128`. Adding `f32` and unsigned types touches all of them. The deep dive's "R4" recommended unifying before adding numeric types; that has not been done. (Read)
3. **Three separate lists of runtime intrinsics** (lowering, the validator, the emitter) that already disagree. (Ran)
4. **Large single files.** `src/ir/lower.rs` has about 7,700 lines of non-test code; `src/frontend/semantic.rs` about 5,600. (Ran)
5. **Integration drift.** `submain` and `main` have diverged by 37 days and now by 86 commits. 82 automated daily-log PRs are open against `main`; 25 of them edit `docment/ROADMAP.md` and will conflict with each other. The daily-log routine runs from a `main` checkout, so its reported gate numbers describe `main`, not the compiler under development. (Read)
6. **Open PR #476** (`codex/runtime-string-identity`, three weeks old) changes string handling in lowering and adds host-side string state. It overlaps directly with the first AOT task. (Read)

---

## 8. Roadmap

Sizes: **S** about a day, **M** several days, **L** weeks, **XL** months or design-led. Priorities: **P0** must precede everything after it, **P1** next, **P2** after the first native executables exist, **P3** later.

Every language milestone (E-track) must land in the semantic layer, the interpreter, lowering, and every native backend that exists at the time, with fixtures that pass on all of them.

### 8.1 Dependency overview

```
M0 baseline ──► M1 AOT-ready IR + runtime ──► M2 Cranelift AOT ──► M3 LLVM AOT v0 ──► M4 LLVM release builds
                     │                              │
                     │                              └──► capstone C0 as a standalone executable
E1 current-surface lowering (authorized queue) ─────────────────────────────────────────┐
E2 place expressions ──► E3 numeric/bit foundation ──► E4 memory + collections ──► E5 FFI ──► E6 ──► E7
        C1                        C2                          C3                     C4
```

The M-track and the E-track touch different parts of the compiler and can proceed side by side, except that both pass through `src/ir/lower.rs`.

### 8.2 Backend track

**M0 — Trustworthy baseline (P0, S–M)**

- Fix the three silent divergences in §4.3, each with a fixture and a `known_issues.md` entry.
- Reserve or mangle runtime symbol names so user code cannot replace `cx_trap` and similar.
- Make an unknown `--backend` value an error.
- Decide PR #476: review and merge, or close.
- Correct the documents listed in §3.3.
- *Owner actions:* promote `submain` to `main` (or state that `submain` is the base for all sessions); decide the daily-log PR backlog.
- **Validation:** gates unchanged or higher; the three repro programs print identical output on both backends; a fixture named after each.

**M1 — AOT-ready IR and runtime (P0, M–L). Depends on M0.**

| Packet | Content | Validation |
|---|---|---|
| M1.1 | Static data in the IR: a module-level data table and a data-address instruction, replacing leaked process addresses for strings, enum names and Result text. The JIT consumes it through Cranelift data objects, as it already does for module globals | No host address appears in the IR of any fixture; two dumps of one program are byte-identical; parity stays 602/48/0 or better |
| M1.2 | One intrinsic table shared by lowering, the validator and the backends. The validator runs on the JIT path under test and gains checks for return types, legal casts and load/store types | Every JIT-PASS fixture passes `--backend=validate` |
| M1.3 | The runtime as a linkable library: the 17 functions, the handle registry, the depth counter, and an entry shim that calls the program and maps exit codes. The JIT registers the same functions | JIT behaviour unchanged; the library builds on its own; `f64` output is byte-identical because formatting stays in Rust |
| M1.4 | Depth guard, trap and `fmod` made backend-neutral, either as IR or as a shared pass | A unit test asserts the guard is present from IR alone |
| M1.5 | Layout takes its pointer size and alignment from a target description; symbol mangling scheme fixed | Layout tests pass for the host target through the new path |

**M2 — Cranelift AOT (P1, M–L). Depends on M1.**

- Object emission through `cranelift-object`, with the emitter made generic over Cranelift's `Module` trait.
- A link step using the system toolchain, and the minimum CLI needed to request an executable.
- A harness mode that builds and runs each fixture as an executable.
- **Validation:** every JIT-PASS fixture passes as a standalone executable on Linux and Windows; the executable runs with the `cx` binary absent; `examples/tooling_metrics.cx` (capstone C0) matches the interpreter.

**M3 — LLVM AOT v0, correct first (P1, M–L). Depends on M1; accepted after M2.**

- An emitter from Cx IR to LLVM IR covering all 14 instructions and 4 terminators: block parameters become phi nodes, allocas are hoisted to the entry block, casts use saturating float-to-int to match the JIT.
- A driver that invokes the toolchain at `-O0`, then `-O2`, and links the runtime library.
- A harness mode for the LLVM backend.
- **Validation:** every JIT-PASS fixture passes at `-O0` and at `-O2` on Linux and Windows; LLVM's verifier accepts every emitted module; no fixture passes on one native backend and fails on the other.

**M4 — LLVM release builds (P2, L). Depends on M3.**

- A benchmark suite that runs on both platforms, with C reference kernels.
- IR additions that let the optimizer work: a block-copy instruction, single-copy aggregate arguments, aliasing and read-only information on return slots and by-value aggregates, cold non-returning traps, global initializers.
- A release mode (optimization level, target CPU, stripping). Debug info needs source positions carried into the IR.
- **Validation:** a performance target agreed in advance against the C kernels and measured, not assumed; output identical to the interpreter on the whole corpus at every optimization level.

### 8.3 Engine-capability track

Each milestone starts with a **design gate**: a short written decision by the owner on syntax and semantics. The doctrine requires this for language-design questions, and most items below have no recorded design.

| ID | Milestone | Size | Priority | Depends on | Design gate |
|---|---|---|---|---|---|
| E1 | Finish lowering the current surface (the authorized queue in §3.2, plus `while-in`, `char`, `t128`, interpolation, `read`/`input`, wider Result payloads) | L | P1 | M0 | None; behaviour is defined by the interpreter |
| E2 | Place expressions and declarations: `a.b.c`, `arr:[i].f`, generic types in annotations, function-body `const`, array fill, explicit casts, enum exhaustiveness, Result payload binding | M | P1 | — | Fill and cast syntax |
| E3 | Numeric and bit foundation (the "math layer" prerequisite): `f32`, unsigned integers, bitwise and shift operators, hex/binary literals, math intrinsics | L | P1 | Numeric type unification | Type names, operator spellings, conversion rules |
| E4 | Memory and data model: non-copying access to aggregates, heap allocation and ownership, dynamic arrays, growable strings, maps, data-carrying enums | XL | P2 | E2 | The whole model; needs the missing design material in §3.4 |
| E5 | FFI and the C boundary: `extern` declarations, C-ABI struct passing, exported functions, linking libraries | L | P2 | M2, E3 | Syntax; pointer or opaque types |
| E6 | Function values and closures; a decision on dynamic dispatch | L | P3 | E4 | Yes |
| E7 | Concurrency: threads, atomics, `shared<T>` | XL | P3 | E4, E5 | Yes |

Stdlib v1 grows alongside: math with E3, I/O and time with E5, collections with E4. Bundling needs the resolver's `std/` path to resolve, which it does not today. Ricey and a build driver follow M2.

### 8.4 Capstone programs

Each must produce identical output on the interpreter and on every native backend that exists. They are the measure of "increasingly complex real programs".

| ID | Program | Gates |
|---|---|---|
| C0 | `examples/tooling_metrics.cx` as a standalone executable (the program exists) | M2, M3 |
| C1 | Fixed-timestep particle or n-body simulation over arrays of structs, printing a checksum | E2; first M4 benchmark |
| C2 | Software rasterizer writing an image file (`f32`, byte packing, file output) | E3 and the first I/O |
| C3 | Entity store with dynamic storage and hashed lookup at 100k entities | E4 |
| C4 | A program calling a C library through FFI; after 1.0, a window and a cleared frame | E5 |
| C5 | A TSG vertical slice | Needs TSG's requirements |

### 8.5 Where this proposes changing a recorded decision

1. **Start the LLVM emitter once M2 produces its first passing executable, not once Cranelift AOT is stable across the full matrix.** *Reason:* the recorded concern was LLVM's integration cost and unproven IR. The IR is now exercised by 602 fixtures, and at that point the shared contract (data, runtime, entry, harness) has been proven with an emitter that is already trusted. What remains in M2 after that is platform linking work, which does not change the IR. The acceptance gate for M3 still comes after M2. If the owner prefers the original wording, M3 simply starts later.
2. **Move "bootstrapping begins/completes", Ricey, and the bulk of stdlib v1 out of the first native-executable release.** *Reason:* a compiler written in Cx needs growable strings, dynamic collections and file I/O — the same E4 and E5 work an engine needs — and none of it exists. Keeping them in the same bucket as AOT makes that bucket unshippable. The roadmap already warns that 0.4 is not one packet; I have not invented version numbers for the pieces.
3. **Treat unsigned integers and `f32` as extensions of the 0.1 ABI document**, which says "all integers are signed … no unsigned types at 0.1". That statement is scoped to 0.1, so this is an addition rather than a reversal, but it changes a locked document and needs the owner's sign-off.

Nothing else in §3.1 is altered.

---

## 9. LLVM AOT — the concrete plan

1. **Do M1 first.** Without relocatable string data and a linkable runtime, no AOT backend can produce a working program.
2. **Emit LLVM IR as text and drive `clang`, rather than binding to LLVM's libraries.** *(Recommend.)*
   - No new build-time dependency: `cargo build` stays as it is, and the LLVM path needs only a `clang` on `PATH` at the time a program is built.
   - Binding crates need LLVM's development libraries when the *compiler* is built. This machine has `clang` and `libLLVM` but not the LLVM-C headers (Ran). On Windows, where primary development happens, the standard LLVM installer has historically not included what those crates need, so a separate full archive or a source build is required (Inferred from general knowledge; check on the Windows machine before deciding).
   - Emitted text is diffable, so emitter tests can be golden files.
   - `clang` also acts as the link driver, which removes a separate problem.
   - Cost: slower builds (a process spawn and a parse) and a minimum supported LLVM version to pin, because the text format changes between major versions.
   - The decision is reversible: a text emitter and a library emitter share all their logic except the final printing.
3. **Write the runtime library in Rust and share it with the JIT.** One source for both, so the two cannot drift, and `f64` printing stays byte-identical to the interpreter.
4. **Correctness before optimization.** M3 targets `-O0` parity first, then `-O2` on the unchanged IR. M4 improves the IR only where a benchmark shows it matters.
5. **Three-way differential testing from the first day:** interpreter, Cranelift, LLVM, on every fixture and example, comparing against each other and not only against stored expectations.

**Expected emitter size (Inferred):** comparable to the Cranelift emitter's 1,800 non-test lines.

---

## 10. Recommended first task

**Fix the three silent lowering divergences in §4.3.**

- **Why first.** They are wrong output on the development head, in the layer every backend shares. The project's own rule has put a silent miscompile ahead of feature work each time one was found. An optimizer on top of them would make them harder to find.
- **Scope.** `src/ir/lower.rs` and fixtures only: the field-assignment path that lowers its right-hand side twice; string `when` arms, which must compare content through `cx_str_eq`; and a guard for an unknown `while` condition matching the one `if` already has. One commit each.
- **Out of scope.** No language, ABI or interpreter change.
- **Done when.** Each repro prints the same on both backends; three new fixtures pass; `known_issues.md` has three new entries; `cargo test` 314+ and `--features jit` 493+ pass; parity is 605 / 48 / 0 or better with no fixture moved to SKIP.

**Then M1.1 — static data in the IR.** It is the first task that moves LLVM forward, and it is bounded to `src/ir` and `src/backend`. PR #476 touches the same string code, so it should be decided first.

---

## 11. Decisions needed from the owner

1. **Branch base.** Promote `submain` to `main`, or confirm `submain` as the base for all work. This session's branch was cut from `main`; it needs to be moved onto `submain` before any PR, and I did not do that without authorization.
2. **PR #476:** review and merge, or close.
3. **Backend order:** keep Cranelift AOT before LLVM as recorded, with or without the relaxation in §8.5 item 1.
4. **LLVM integration:** text IR and `clang` (recommended), or library bindings.
5. **Command-line surface** for building executables.
6. **TSG's requirements** and the missing design material in §3.4.
7. **Design gates** for E2 and E3: array fill, cast syntax, unsigned and `f32` type names, bitwise operators.
8. **0.4 scope:** whether bootstrapping, Ricey and full stdlib v1 leave the first native-executable release (§8.5 item 2).
