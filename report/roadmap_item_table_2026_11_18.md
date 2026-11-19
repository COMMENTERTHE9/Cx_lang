# Cx historical roadmaps vs. current state — full item table

Produced 2026-11-18, read-only. The checked-out branch was `backend/m0-followup-correctness`, at `5498b41`
(the same commit as `backend/m0-lowering-divergences`) when I started. Four fix commits landed on it while I
worked (`645ad51`, `5487113`, `93bfbfd`, `187f4f9`; known-issues §58–§61, closing §56a–d). The binary I ran,
`cx_m0b`, predates them, so every **ran:** result describes the M0 build at `5498b41`. I did not
re-run against a newer build (none was provided); the four commits concern `.copy` aggregates, early `?`
returns, `Result` with aggregate payloads and struct print order, none of which a row below depends on.

## Conventions

- **FR** = `docs/frontend/ROADMAP.md` (v5.0, 2026-04-25). **V3** = `docs/ROADMAP_v3.1.md` (2026-03-16).
  **BR** = `docs/backend/cx_backend_roadmap_v3_1.md` (v4.4, 2026-05-12). **GP** = `docs/post_0_1/gene_phen_design.md`.
- **LR** = `docment/ROADMAP.md` (living). **AUD** = `report/tsg_llvm_readiness_2026_11_18.md`.
  **M0** = `report/m0_verification_2026_11_18.md`. **KI §n** = `docs/known_issues.md` section n.
- Evidence prefixed **ran:** was executed by me with the JIT-enabled binary `cx_m0b`, interpreter and
  `--backend=cranelift`, 20 s timeout. Probe sources are in `report/roadmap_probes_2026_11_18/`; the
  probe name is given in brackets. Everything else is **read** (document or source), not executed.
  `cargo` was not run, so test counts are quoted from M0, not re-measured.
- "rc" is the process exit code. JIT exit 127 = clean refusal ("unsupported … during lowering").
- Status values: Done on both backends / Done on interpreter only / Partial / Absent / Superseded by X / Contradicted.

---

## A. FR "0.1 Release Candidate Status" and "Known 0.1 limitations" (L8–26)

| # | Item | Status now | Evidence |
|---|---|---|---|
| A1 | Matrix 182 fixtures; JIT parity 120/62/0 | Superseded by later counts | read: M0 §5 — 658 fixtures, parity 610 PASS / 48 SKIP / 0 FAIL |
| A2 | Examples 8/8 pass | Done on interpreter only (as a set) | ran: all 11 runnable files in `examples/` exit 0 on the interpreter; the JIT refuses 2 — `arrays_and_loops.cx` (WhileIn) and `tbool_uncertainty.cx` (`is_known`) |
| A3 | Limitation: string arena grows monotonically, no GC/compaction | Unchanged (not verified by running) | read: README "string arenas … still planned"; `docs/memory_safety.md` |
| A4 | Limitation: Handle double-drop is a silent no-op ("could warn in future") | Unchanged; behaviour Done on both backends | ran [d10]: `h.drop()` twice, exit 0, no warning, both backends |
| A5 | Limitation: no strref constructor syntax | Unchanged — Absent | ran [e03, d14, a07]: a literal or a `str` passed to a `strref` parameter, or assigned to a `strref` variable, is "expected strref, got str" |
| A6 | Limitation: interpreter stack via 64 MB thread; per-frame reduction post-0.1 | Superseded by the call-depth guard | ran [d11]: depth 300 → "call depth limit reached … limit is 256" on both; KI §24, §25 |
| A7 | Limitation: expression statements still require semicolons | Contradicted by current behaviour (now optional) | ran [a09, i02]: bare `f()` and `x + 1` statements, newline-terminated, at top level and inside a function body, run on both. `docs/cx_syntax.md` §14 still says they are required |
| A8 | Limitation: untyped assignment needs a prior declaration (by design) | Unchanged | ran [d12]: `g = 5` → "use of undeclared variable 'g'" |

## B. FR "What Cx Is" / "0.1 Release Definition" (L30–56)

| # | Item | Status now | Evidence |
|---|---|---|---|
| B1 | 0.1 = parser, semantic layer and interpreter agree on all supported constructs | Done (0.1 tagged) | read: LR "Shipped" — 0.1 tagged `9fc0d24`, 2026-05-22 |
| B2 | "The frontend and backend ship together" | Done | read: LR — 0.1 shipped with the Cranelift JIT subset |
| B3 | "Core syntax and semantics are frozen — no breaking changes after 0.1" | Contradicted | See report §3 (several post-0.1 behaviour changes) |
| B4 | 0.1 excludes: complete stdlib, filesystem/windowing, GPU, operator overloading, gene/phen, production backend, networking/audio/platform | Operator overloading and gene/phen have since shipped (0.3.2); all the others are still Absent | ran: see M, N, O, P below |

## C. FR "Syntax Decisions — Locked" (L60–150)

| # | Item | Status now | Evidence |
|---|---|---|---|
| C1 | Integer types `t8`–`t128`, signed, wrapping at declared width | Done on both for `t8`–`t64`; `t128` arithmetic works, printing is interpreter only | ran [a01]: 127+1 → -128 etc. on both. ran [a02]: `t128` print → JIT 127 |
| C2 | `f64` only at 0.1; `f32` post-0.1 | `f64` Done on both; `f32` Absent | ran [a03]: `x: f32 = 1.5` → "expected f32, got f64" |
| C3 | `bool` two-state; `tbool` three-state | Contradicted | ran [a05, i04]: `b: bool = ?` is accepted and matches the `unknown` arm (both backends for the initialised form). ran [g02, i03]: `x: tbool = true` and `x = true` → "expected tbool, got bool". README: "`bool` is three-state" |
| C4 | `str` owned; `strref` arena view that cannot escape | Partial | `str`: static strings on both; ran [d15] runtime `+` is interpreter only. `strref`: escape checks exist (fixtures t32, t115) but no value can be made (A5) |
| C5 | `char` | Done on interpreter only | ran [a06]: JIT 127 "unsupported semantic type: Char" |
| C6 | `Handle<T>` | Partial | ran [d10]: scalar payload, both backends. ran [g04, e14]: `str` and `f64` payloads interpreter only. ran [c05, c06]: `h: Handle<t64> = Handle.new(99)` → "expected Handle<t64>, got Handle<t128>" (payload type is hard-coded; LR "Future Design Work") |
| C7 | `NullPoint<T>` | Absent | ran [a08]: parse error. LR: deferred, "genuinely undesigned" |
| C8 | Semicolons always optional | Done on both | ran [c35, a09, i01, i02] |
| C9 | Compound assign `+= -= *= /= %=`, no postfix | Done on both | ran [c35]; [a19] `x++` is a parse error |
| C10 | `*` is multiplication only; `*arr` shorthand removed | Contradicted | ran [b01, g13]: `print(*arr)` inside `while in` prints the cursor element (interpreter). Fixtures `t34_while_in.cx`, `t35_while_in_then.cx` use it. ran [a10]: outside `while in`, `y: t64 = *arr` parses and fails only on type ("expected t64, got [3: t64]") |
| C11 | Array type `[N: Type]`, index `arr:[0]` | Done on both | ran [e12] |
| C12 | Function syntax `fnc: RetType <T>? name(params)` — frozen | Done on both for functions and phen bodies; Contradicted inside `gene` | ran [c01, c02]: a gene signature must be written `fnc name(params) -> Ret`; the frozen form is a parse error there. `src/prelude.cx` uses the arrow form |
| C13 | `print` / `printn` are functions; `print!` gone | Done on both (`println` also exists) | ran [a12, a11] |
| C14 | `Result<T>`, `Ok`, `Err("msg")`, `?` | Done on both for scalar payloads | ran [b08]. read: AUD §4.2 — float, aggregate and nested payloads interpreter only |
| C15 | Unknown does not merge with Result | Done on interpreter only | ran [g15]: `Ok(?)` prints `Ok(?)`; JIT 127 |
| C16 | `copy_into` survives as a distinct feature | Done on both | ran [d09]: 129 on both. LR still lists it as an open lowering blocker (stale; AUD §3.3) |
| C17 | `when` arms: single expression and block | Done on both | ran [a18] |
| C18 | Named binding `Variant as v` and guards — "designed, not implemented at 0.1" | Done on both since 0.3.1; guard form differs from the roadmap's example | ran [c28]: `_ if n > 5` works. ran [g03]: the roadmap's own example `n if n > 5` → "'n' is not a known group or super-group name". Fixtures `t_pattern_as_binding_*`, `t_pattern_guard_*` |
| C19 | No struct destructuring at 0.1 | Absent | ran [c26]: parse error |
| C20 | `:=` type inference | Absent | ran [a13]: parse error |
| C21 | Block comments `/# … #/` | Done on both (`//` line comments also work) | ran [a14] |
| C22 | UTF-8 strict; invalid bytes a runtime error; "binary data uses byte buffers" | Partial | ran [d13]: a source file with invalid UTF-8 is rejected at read. ran [a20]: `len("héllo ✓")` = 10 bytes. No byte-buffer type exists (read: AUD §4.2) |
| C23 | Import block `#![imports]`, `alias: use "path"` | Done on both | ran [d01] |
| C24 | "Lazy loading implicit — only referenced symbols loaded" | Contradicted | ran [d06]: an unreferenced private function in an imported file, with an undefined name in its body, fails the importing program |
| C25 | `pub` required for exports | Done on both for functions | ran [d02]. ran [e02]: `pub const` is a parse error |
| C26 | `math: use "std/math"` | Absent | ran [c14]: "stdlib not yet bundled" |
| C27 | Multi-alias impl blocks | Done on both | ran [b19] |
| C28 | `const` — "literal-only initializers", reassignment rejected | Done on both; "literal-only" Contradicted | ran [a17, e06]. ran [a15, e05]: `const A: t64 = 2 + 3` and `const A: t64 = f()` are accepted. ran [a16]: `const` in a function body is a parse error |

## D. FR "Hard Blockers" (L158–178) — all ticked in the document

| # | Item | Status now | Evidence |
|---|---|---|---|
| D1 | `f64` keyword | Done on both | ran [a01, h06] |
| D2 | Generic structs `Struct<T>`; stated gaps: type args in variable declarations, field type checking | Partial | ran [f07]: `Pair<f64> { … }` on both. ran [b05]: field type now enforced. ran [b03]: `p: Pair<t32> = …` is still a parse error. ran [b04]: with `p: Pair`, `p.first + p.second` → "got Pair::T and Pair::T" |
| D3 | `read(var)`, `input("prompt", var)` | Done on interpreter only; `read` into a number fails | ran [c08]: `read` into `str` works. ran [b06]: `read(n)` with `n: t64` and stdin `41` → runtime "expected 't64' but got 'str'". JIT 127 for both builtins |
| D4 | `const` declarations | Done on both | C28 |
| D5 | Value-producing `when` | Done on both | ran [a18] |
| D6 | `when` block-body arms | Done on both | ran [a18] |
| D7 | Multi-file imports | Done on both (relative paths only) | ran [d01–d05] |
| D8 | Test runner: `assert`, `assert_eq`, `--test`, `#[test]` | Done on both | ran [b07 --test]: "PASS: t_one / FAIL: t_two … 1 passed, 1 failed", rc 1 |
| D9 | Minimal error model | Done on both (scalar payloads) | C14 |
| D10 | `print` promoted to function | Done on both | C13 |
| D11 | UTF-8 decision locked | Done | C22 |
| D12 | String interpolation `{varname}` | Done on both, bare identifiers only | ran [b09]. ran [h04]: `{x + 1}` is a runtime error on the interpreter and JIT 127 |
| D13 | Integer overflow wraps at declared width | Done on both | ran [a01] |
| D14 | Semicolon rule enforced consistently | Done on both | C8 |
| D15 | Parser / semantic / interpreter agreement audit | Done as a dated event | read: `examples/audit/AUDIT_REPORT.md`. Later disagreements are logged in KI §14–§57 |
| D16 | "No known soundness holes in memory boundary model" | Superseded by later findings | read: KI §26 / LR — array returns "returned a dangling frame pointer and silently produced wrong values on both shipped paths"; KI §10 |
| D17 | All examples pass | Done on interpreter | A2 |
| D18 | Diagnostics readable for common mistakes | Partial | ran [d26]: type mismatch names Cx types. ran [d25]: an unknown-field error is reported at line 1 (the struct), not the access. KI §17 |
| D19 | "Roadmap and spec match actual language behavior" | Contradicted now | read: AUD §3.3; this table |

## E. FR "Quality Gates" (L180–190)

| # | Item | Status now | Evidence |
|---|---|---|---|
| E1 | `:=` inference | Absent | C20 |
| E2 | `when` as value | Done on both | D5 |
| E3 | Named binding and guards | Done on both | C18 |
| E4 | `NullPoint<T>` | Absent (deferred) | C7 |
| E5 | Generics v3 bounds `T: Numeric`, `T: Known` | Superseded by gene bounds `T: GeneName`; the two named bounds are Absent | ran [c01]: `T: Damageable` bound with a method call works on both. ran [b26]: `T: Known` → "no phen of gene 'Known'". ran [b25, b27, b24]: under `T: Numeric` or `T: Add`, `a + a` in the generic body → "arithmetic requires numeric operands, got T and T". ran [j01]: `a.add(b)` under `T: Add` works for a user struct on both. ran [j02]: a primitive does not satisfy it — "no phen of gene 'Add' for 't64'" (GP "known bootstrap gap"). So no generic function can be written over the built-in numeric types |
| E6 | Minimal stdlib — dynamic array, hashmap, string utilities | Absent | ran [c17, c18, c23, c24] |
| E7 | Diagnostics — span reporting, actionable help | Partial | D18 |
| E8 | Struct field type checking | Done on both | ran [d25, b05] |
| E9 | Method call return type resolution | Done on both | ran [f04] |

## F. FR "Done" (L194–272)

| # | Item | Status now | Evidence |
|---|---|---|---|
| F1 | Phase 1 functions; forward declarations; nested-function name leakage fixed | Done on both | ran [h01, h03] |
| F2 | Phase 2 — free checker | Superseded (removed) | read: LR `.copy` rulings 4–5 — `free_variable` "removed on 2026-03-06". ran [g14]: `free(x)` is an undefined function |
| F3 | Phase 3 — `.copy`, `.copy.free`, `copy_into` | Done on both; `.copy.free` deprecated | ran [b18]: `.copy.free` prints a deprecation warning. ran [e09]: `.copy` on a method parameter is rejected. ran [d09] |
| F4 | Phase 4 bump allocator; 4b arena string storage | Interpreter only (read, not run) | read: `docs/memory_safety.md`; compiled code has static strings only |
| F5 | Phase 5 — `Handle<T>` registry and surface | Partial | C6 |
| F6 | Phase 6a `when`; 6b ranges; 6c enums; 6d loops, compound assign, comparisons | Done on both | ran [a18, d21, e13, c35] |
| F7 | Phase 6e flat grouped enums; 6f super-group enums and `{_}` | Done on both | ran [b15, d17] |
| F8 | `Str` / `StrRef` split and boundary checker ("Memory Boundary Rules v0.1") | Partial | C4. The rules document is not in the repository |
| F9 | TBool, `is_known(x)`, unknown runtime | Three-state bool Done on both; `is_known` and untyped unknown interpreter only | ran [a05]; [b13] JIT 127 |
| F10 | Arrays — declaration, init, partial init, index read/write, pass/return, copy | Done on both, except partial init | ran [e12]. ran [b14]: `[1, 2]` for `[5: t64]` is a type error. ran [c07]: `[1, ?, 3]` interpreter only |
| F11 | `while in` / `then` chaining | Done on interpreter only | ran [b01, g13]: JIT 127 "WhileIn" |
| F12 | `if` / `else if` / `else` | Done on both | ran [b22] |
| F13 | Generics v1 and v2 | Done on both (monomorphised) | ran [e11, f07] |
| F14 | Structs phase 1+2; multi-struct impl | Done on both | ran [f04, b19] |
| F15 | `!=`, `!`, exit codes, `.expected_fail`, `run_matrix.sh` | Done | ran [h02]. read: `src/tests/run_matrix.sh` |
| F16 | GitHub Actions CI; CONTRIBUTING.md; matrix in CI | Done (Linux only) | read: `.github/workflows/ci.yml` |
| F17 | Semantic/interpreter parity; raw AST path deleted; cleanup and code-quality sprints | Done (not re-verified) | read only |
| F18 | Macros `#[test]`, `#[inline]`, `#[deprecated]` accepted | Done on both | ran [b10] |
| F19 | `#[reactive]`, `#[cfg]` reserved with post-v0.1 errors | Still reserved — Absent | ran [b11, b12] |
| F20 | Import validation: duplicate alias, registry path rejection | Done | ran [c15]: "registry imports not supported in v0.1" |

## G. FR "Active" (L276–283) and H. "Known Gaps" (L287–299)

| # | Item | Status now | Evidence |
|---|---|---|---|
| G1 | Backend IR phases 6, 10, 11 | Done on both | ran: calls, loops, structs, arrays in many probes |
| G2 | ABI phase 8 — open: string layout, copy-parameter convention | Partial | read: KI §30 (`.copy` by address landed). AUD §3.3: ABI doc's string layout differs from the code |
| G3 | Generic structs follow-up | Partial | D2 |
| H1 | t33 — array index assign | Done on both | ran [e12] |
| H2 | t32 — strref escape reject "rejects some valid patterns" | Unchanged | A5 |
| H3 | Expression-statement semicolons; "newline-aware parser redesign, post-0.1" | Done in practice | A7 |
| H4 | `*arr` deref removed | Contradicted | C10 |

## I. FR "Must Ship for 0.1" (L303–348)

| # | Item | Status now | Evidence |
|---|---|---|---|
| I1 | Module resolution, relative paths | Done on both | ran [d01] |
| I2 | `pub` enforcement | Done on both (functions) | ran [d02]: "function 'hidden' not found in module 'm'" |
| I3 | Dead symbol elimination — only referenced symbols loaded | Absent | C24 |
| I4 | Stdlib path resolution `std/math`, `std/string` | Absent | C26 |
| I5 | Circular import detection | Done | ran [d05]: "circular import detected: d_cyc_a.cx -> d_cyc_b.cx -> d_cyc_a.cx" |
| I6 | Project layout defined | Absent | read: no document; AUD §6 "relative imports only; whole program is one unit" |
| I7 | `assert`, `assert_eq` | Done on both | D8. Open defect on `Result` and enum operands: KI §55 |
| I8 | Test blocks "skipped in release builds" | Partial | ran [e08]: a `#[test]` function is an ordinary callable function outside `--test`. No release build exists |
| I9 | `cx test` runner | Superseded by the `--test` flag | ran: `--help` |
| I10 | Result direction, propagation model | Done on both | C14 |
| I11 | Panic vs recoverable boundary decided | Partial | No written decision found. ran [d22, d19]: runtime errors exit 1 with a diagnostic on the interpreter, 126 with a generic trap line on the JIT |
| I12 | Integration with Unknown — "does an error produce Unknown or halt?" | Settled as "separate" in the same document (L96) | C15 |
| I13 | Parser error spans — line, column, expected | Partial | ran: line, caret and expected-token list; no column number |
| I14 | Unknown-state diagnostics — "which value is unknown and where it entered" | Partial | ran [d19]: message names the `if`, not the value or its origin |
| I15 | Import resolution errors | Done | ran [d03, d04, d05]. ran [d06]: an error inside an imported file shows the wrong line and no file name |
| I16 | Struct/method resolution errors; boundary violation errors; actionable help | Partial | ran [d25, e03, g10] |
| I17 | `str` copy-on-boundary tested | Done on interpreter (lightly checked) | ran [h05] |
| I18 | "strref arena view confirmed working" | Contradicted | A5 |
| I19 | Substring without copy | Absent | ran [c25]: "cannot index 'str'" |

## J. FR "Strongly Desired for 0.1" (L352–377) and K. "Examples and Conformance Programs" (L381–396)

| # | Item | Status now | Evidence |
|---|---|---|---|
| J1 | Stdlib core: dynamic array (push/pop/len/capacity), hashmap, hashset, split/join/contains/trim | Absent | ran [c17, c18, c23, c24] |
| J2 | Struct field destructuring in `when` (post-0.1) | Absent | C19 |
| K1 | Examples: hello, fixed arrays, basic enums, `when`, structs+methods, generics, multi-file imports | Present | read: `examples/`; ran A2 |
| K2 | Examples: dynamic arrays; grouped and super-group enums; `Handle<T>` usage; str vs strref boundary; test blocks; failure examples; engine-facing starter (math/transform structs, entities) | Absent as examples | read: `examples/README.md` ("None are intentional failure cases"). Handle and boundary programs exist only under `examples/audit_memory/` |

## L. FR "0.1 Syntax and Semantics Freeze" (L400–410)

| # | Item | Status now | Evidence |
|---|---|---|---|
| L1 | Import syntax written here as `#![import]` | Contradicted (spelling) | ran [d07]: only `#![imports]` parses |
| L2 | Unknown-state behaviour frozen | Contradicted | read: KI §54 — an unknown `while` condition changed from "skip the loop" to a run-time error on the interpreter (2026-11-18, awaiting owner confirmation) |
| L3 | Struct and method surface | Done on both | ran [f04]. ran [g07]: `impl Player { … self … }` shown in `docs/cx_syntax.md` §16 is a parse error (also AUD §3.3) |

## M. FR "Post-0.1 — Language Core" (L414–424)

| # | Item | Status now | Evidence |
|---|---|---|---|
| M1 | gene + phen trait system | Done on both | ran [c01]. LR: shipped 0.3.2 |
| M2 | Operator overloading ("Vector3 + Vector3") | Done on both, including chained | ran [c04]: `a + b` and `(a + b) + a` on a `Vec3` of `f64` |
| M3 | Full pattern matching — array patterns, nested patterns | Absent | ran [e04]: an array-literal arm `[9, 9] => …` is accepted and always matches (against `[1, 2]` and against the integer 5), on both backends |
| M4 | Labeled breaks | Done on both | ran [b21] |
| M5 | Ternary / value-producing `if` | Done on both as an `if` expression; no `?:` | ran [b22, b23] |
| M6 | Closures and lambdas | Absent | ran [c11, c12] |
| M7 | Async and continuations | Absent | ran [c22] |
| M8 | Reflection / type introspection | Absent | ran [c31] |
| M9 | C interop — "nearly free if Cx emits C as a compilation target" | Absent | ran [c13]. read: no C-emitting backend in `src/backend/` |

## N. FR "Post-0.1 — Runtime and Stdlib" (L428–471)

| # | Item | Status now | Evidence |
|---|---|---|---|
| N1 | hashset, hashmap | Absent | ran [c18] |
| N2 | hashweb (graph collection, `<=>`, `=>`, `::name`) | Absent | ran [c19]: parse error |
| N3 | Vec<T>, ring buffer, queue, stack, LinkedList<T>, TreeMap<K,V> | Absent | ran [c17, c32] |
| N4 | Binary search, quicksort, merge sort | Absent | no stdlib bundled (C26) |
| N5 | String utilities | Absent | ran [c23, c24] |
| N6 | Phase 5b — region_id bulk handle invalidation on arena reset | Absent | read: `src/runtime/handle.rs:13` — `region_id: u32, // Phase 5b - bulk arena invalidation, unused for now` |
| N7 | Handle-backed containers | Absent | read: none in source |
| N8 | `rc<T>` | Absent | ran [c20]: parse error |
| N9 | `shared<T>` | Absent | ran [c21]: parse error |
| N10 | Reference cycle handling (design pass needed) | Absent, no design | read |

## O. FR "Post-0.1 — Filesystem I/O" (L475–483)

| # | Item | Status now | Evidence |
|---|---|---|---|
| O1 | open/close/read_line/read_all/write/write_line/append; exists/delete/create/mkdir/list_dir/is_dir | Absent | ran [c16]: "call to undefined function 'open'" |
| O2 | txt/csv/json generation, binary buffers; csv and json parsing | Absent | read: none |

## P. FR "Post-0.1 — Engine Systems" (L487–512)

| # | Item | Status now | Evidence |
|---|---|---|---|
| P1 | Window and screen system (images, `Handle<Window>`, blit/present, events, headless, Win32/Cocoa/X11/Wayland) | Absent | ran [c29]. read: no windowing code in `src/` |
| P2 | GPU system (VRAM registry, GS types, `.drop(fence)`, GPU memory lifetime model, GPU rendering path) | Absent | ran [g01]: `h.drop(fence)` → "method calls are only supported on struct values". read: no match for vram/fence/gpu in `src/` |
| P3 | Audio — "deferred until window system lands" | Absent | read |
| P4 | Networking — TCP/UDP, "deferred until filesystem I/O is proven" | Absent | read |

## Q. FR "Tooling" (L516–523) and R. "Design Backlog" (L527–538)

| # | Item | Status now | Evidence |
|---|---|---|---|
| Q1 | CLI — `cx build`, `cx run`, `cx test`, `cx check` | Absent | ran: `--help` shows one form, `cx [OPTIONS] <file.cx>` |
| Q2 | CLI visualizer | Absent | read: only `--debug-*` dumps (`docs/cx_flags.md`) |
| Q3 | Ricey registry server | Absent | ran [c15]. read: resolver `RegistryNotSupported`; fixture t70 |
| Q4 | Cranelift JIT backend ("Phase E") | Done (subset of the language) | ran throughout |
| Q5 | LLVM AOT backend | Absent | ran: `--backend=llvm` → "LLVM backend not implemented yet". read: 6-line stub, no LLVM dependency in `Cargo.toml` |
| Q6 | LSP | Absent | read |
| R1 | gene + phen full design | Done | read: GP v1.1 |
| R2 | 2D/3D/4D arrays native syntax | Done on both, ranks 2–4 | ran [c09]: a rank-4 literal, read and write. KI §28 |
| R3 | Designs for async/continuations/lambdas, closures, reflection, C FFI surface, Ricey package integration, hashweb traversal | None exists | read: no design document for any of them |

## S. Items that appear only in V3

| # | Item | Status now | Evidence |
|---|---|---|---|
| S1 | Unknown condition rejection on control-critical paths | Done on both (`if`, and `while` since KI §54) | ran [d19] |
| S2 | Generics v1 written `fnc identity<T>(x: T) -> T` | Superseded by `fnc: T <T> identity(x: T)` | ran [g05]: parse error |
| S3 | `[N]T` array return types | Superseded by `[N: T]` | ran [g06]: parse error |
| S4 | Stdlib early — growable array, hash table, ring buffer, binary search + quicksort | Absent | N1–N4 |
| S5 | Strings — "String interop with handles" | Done on interpreter only | ran [g04] |
| S6 | `read(var)` — "Type inferred from declaration"; "no new lexer tokens" | Partial | D3 |
| S7 | "2D/3D/4D arrays — use flat arrays + manual indexing" (deferred) | Contradicted (reversed) | R2 |
| S8 | Traits / interfaces / bounded polymorphism full design | Done | R1 |
| S9 | "C interop — emitting C as compilation target makes this nearly free" | Absent | M9 |
| S10 | CONTRIBUTING.md — four branch rules | Done | read: `CONTRIBUTING.md` Rules 1–4 |
| S11 | GPU System (VRAM registry, GS types, `.drop(fence)`, GPU memory lifetime model) | Absent | P2 |

## T. Items that appear only in BR

| # | Item | Where | Status now | Evidence |
|---|---|---|---|---|
| T1 | Gate: JIT output matches interpreter "stdout, exit code, behavior" | BR "0.1 Backend Release Gates" | Partial | ran [d22, d19]: run-time errors exit 1 on the interpreter and 126 on the JIT with different text. Silent differences: KI §57b and §57c open; §56a marked fixed in the tree by §58 (not in the binary I ran) |
| T2 | Gate: structured errors, never panics | same | Partial | ran: every refusal I hit was a clean exit 127. read: KI §56c — printing a `Result` holding a struct segfaulted compiled code; marked closed by refusal in the tree (§60), not in the binary I ran |
| T3 | Gate: IR validator rejects malformed IR before codegen | same | Contradicted | read: AUD §5.2 item 6 — the JIT path never calls the validator. ran [f02]: `--backend=validate` fails a string-equality program ("Call to undefined function 'cx_str_eq'") that the JIT runs correctly |
| T4 | Gate: minimal determinism; "stable IR printer output" | same; "Post-0.1 — Backend Quality" | Contradicted for any program with a string | ran [f01]: two `--backend=validate` dumps of one program differ in `const i64 <address>`; [f03] an arithmetic-only program is identical. `docs/backend/static_data_design.md` §1 |
| T5 | Gate: ABI/layout documented and tested; intrinsics boundary defined | same | Partial | read: AUD §3.3 — ABI doc stale on strings, return slots, Result/Handle packing; intrinsics doc lists 1 of 17 |
| T6 | Gate: evaluation order documented and stable | same | Partial | read: `docs/backend/cx_eval_order.md` exists; KI §52 (fixed), §56a (marked fixed by §58), §57b (open) |
| T7 | Quality gate: platform matrix Windows x64 and Linux x64 | same | Partial | read: CI is Linux only; `cx_abi_v0.1.md` platform note |
| T8 | "Explicitly unsupported in backend 0.1": `when`, full generics | BR "Backend Support Matrix" | Both now Done on both | ran [a18, f05, f07] |
| T9 | Dynamic dispatch | same | Absent by decision | read: GP item 1 — "Static monomorphization only" |
| T10 | Phase 8 open: str/strref layout and arena ownership question | BR Phase 8 | Partial | read: static strings only; design proposed in `static_data_design.md`, not implemented |
| T11 | Phase 8 open: Handle<T> representation; unknown propagation; large return values | BR Phase 8 | Partial | ran [d10] scalar handles on JIT; untyped unknown refused; [e12] array returns on both (KI §26) |
| T12 | Phase 9 sub-packet 4 — `read`/`input` lowering | BR Phase 9 | Absent on JIT | ran [b06, d23]: JIT 127 |
| T13 | Phase 9 remaining: allocation ops, string boundary ops, error and panic paths | BR Phase 9 | Partial | ran [d15]: runtime concatenation not lowered |
| T14 | Phase 11 — method call lowering | BR Phase 11 | Done on both | ran [f04] |
| T15 | Loop-variable read-only invariant | BR Phase 10/11 | Done on both (at analysis) | ran [f06] |
| T16 | Phase 12 — harness in CI on every PR | BR Phase 12 | Partial | read: CI runs `cargo test --features jit` on Linux; M0 §6 notes the parity test passes when its binary is missing |
| T17 | Phase 16 — Cranelift AOT (objects, ELF/COFF, symbols, linker, `cx build --release`) | BR "Post-0.1 — Compiler Targets" | Absent | read: `src/backend/cranelift/aot.rs` is a stub returning an error |
| T18 | Phase 17 — LLVM AOT; "Do not start this until Cranelift AOT is stable" | same | Absent | Q5 |
| T19 | Phase 18 — FFI and C boundary | same | Absent | M9 |
| T20 | Full reproducible builds; golden outputs | BR "Post-0.1 — Backend Quality" | Absent | no compiled artefacts exist |
| T21 | Extended layout suite — macOS, ARM64 | same | Absent | read: AUD §5.2 item 4 — layout is host-only, pointer size hard-coded |
| T22 | Source maps, DWARF, debugger integration, JIT breakpoints, variable inspection | BR "Post-0.1 — Debuggability and Tooling" | Absent | read: AUD §5.6 — no source locations in the IR |
| T23 | CFG visualization (Graphviz) | same | Absent | read: no such flag |
| T24 | Extended trace — per-instruction, JIT disassembly, SSA tracking | same | Partial | ran [f03]: `--debug-trace` prints each IR instruction; no disassembly |
