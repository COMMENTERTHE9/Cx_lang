# Cx Language Roadmap
v4.8 — 2026-03-29

---

## What Cx Is

Cx is a systems language for game engine developers. The goal is explicit memory behavior, predictable data layout, and a type system that makes costs visible — without requiring a borrow checker or a garbage collector. The language is built around the idea that uncertainty is a first-class value, and that a programmer should always be able to see where memory lives, how long it lives, and what happens when it doesn't.

0.1 is a language release. The frontend and backend ship together. The backend roadmap is tracked separately — this document covers the language surface, type system, runtime, stdlib, and platform systems.

---

## 0.1 Release Definition

**Cx 0.1 means:**
- The parser, semantic layer, and interpreter agree on behavior for all supported constructs
- You can write programs across multiple files using the import system
- Structs, methods, generics, enums, arrays, control flow, and memory boundaries all work together
- The language tells you clearly when something is wrong and why
- You can write tests in Cx and run them
- There are working examples that show what the language can do
- The core syntax and semantics are frozen — no breaking changes after 0.1

**Cx 0.1 does not mean:**
- A complete stdlib
- Filesystem or windowing APIs
- GPU system
- Operator overloading
- Full trait system (gene/phen)
- A production backend stack
- Networking, audio, or platform APIs

---

## Syntax Decisions — Locked

These decisions are frozen as of 2026-03-22. No breaking changes after 0.1.

**Types:**
- Integer types: `t8`, `t16`, `t32`, `t64`, `t128` — signed, wrapping arithmetic at declared width
- Float type: `f64` — single float width at 0.1, `f32` post-0.1
- Boolean: `bool` — two-state. `tbool` — three-state (true/false/unknown)
- String: `str` — owned. `strref` — arena view, cannot escape scope
- Other: `char`, `Handle<T>`, `NullPoint<T>`

**Integer overflow behavior — wrapping:**
All integer arithmetic wraps at the declared width. A `t8` variable wraps at 255. This is explicit and documented. Game engines expect wrapping — silent saturation or trapping would be surprising.

**Semicolons — always optional:**
Newlines terminate statements. Semicolons are ignored if present. One consistent rule, no exceptions, no context-dependent behavior.

**Compound assign — standard infix:**
`+=`, `-=`, `*=`, `/=`, `%=` — frozen. No postfix form.

**`*` operator — multiplication only:**
`*` means multiplication in all positions. `arr:[0]` is the cursor/index access syntax. The `*arr` shorthand for `arr:[0]` is removed — it was confusing because `*` already means multiplication. Use `arr:[0]` consistently.

**Array type syntax — `[N: Type]`:**
`arr: [5: t64]` — size colon type inside brackets. Frozen.

**Array index syntax — `arr:[0]`:**
Colon before bracket distinguishes index access from type annotation. Frozen.

**Function syntax — `fnc: RetType <T>? name(params)`:**
Return type after colon. Generic params before name. Void functions omit return type. Frozen.

**print functions:**
`print(x)` — adds newline. `printn(x)` — no newline. Both are regular functions, not statements. `print!` syntax is gone.

**Error model — Result\<T\> with ? propagation:**
`Result<t64>` syntax. `Ok(val)` and `Err("msg")` variants. `?` operator propagates errors up the call chain. Unknown state is a separate concept — it does not merge with Result.

**`copy_into` — survives as a distinct feature:**
Structs and `copy_into` solve different problems. `copy_into` is about passing multiple named values with bleed-back mutation. It is not deprecated in favor of structs.

**`when` arm bodies — both single expressions and blocks supported:**
```cx
when x {
    1 => print("one"),
    2 => {
        print("two")
        do_something()
    },
    _ => print("other"),
}
```

**Pattern matching — named binding and guards targeted post-0.1:**
Named binding (`SomeVariant as v`) and guard clauses (`n if n > 5`) are designed but not implemented at 0.1. No struct destructuring at 0.1.

**`:=` type inference — strongly desired, not a hard blocker:**
`score := 10` infers type from right-hand side. Targeted for 0.1 if time allows, not a gate.

**Block comments — `/# and #/`:**
Multiline comments use `/# ` to open and `#/` to close. Frozen.

**Import syntax — `#![imports]` block:**
```cx
#![imports]
math: use "std/math"
player: use "./player"
```
Lazy loading implicit — only referenced symbols loaded. `pub` required for exports.

**Multi-alias impl blocks:**
```cx
world_sync: impl (p: Player, w: World) {
    fnc: sync() { p.position = w.origin }
}
p.sync(w)  /# w is second alias, passed as leading arg #/
```

**const declarations:**
```cx
const MAX_HP: t32 = 100
const GRAVITY: f64 = 9.8
```
Literal-only initializers. Semantic pass rejects reassignment.

---

## 0.1 Release Gates

These are not features. These are conditions. A long gate list that never closes is a project killer — so the gates are split into two honest tiers.

### Hard Blockers (must ship, 0.1 does not exist without these)

- [x] `f64` type keyword — full pipeline landed 2026-03-22, t55 passing
- [x] Generic structs `Struct<T>` — Phase 1+2 landed 2026-03-23, t61/t62/t63 passing. Known gaps: type args in variable declarations, generic field type checking not yet enforced
- [x] `read(var)` stdin input — landed 2026-03-23, `input("prompt", var)` also implemented, t60 passing
- [x] `const` declarations — landed 2026-03-22, t56/t57 passing
- [x] Value-producing `when` — full pipeline landed 2026-03-22, t59 passing
- [x] `when` block-body arms — verified 2026-03-22, t58 passing
- [x] Multi-file imports working — resolver implemented 2026-03-25, t74 passing
- [ ] Basic test runner — `assert(cond)`, `assert_eq(a, b)`, test blocks
- [ ] Minimal error model — `Result<T>`, `Ok`, `Err`, `?` operator syntax locked and implemented
- [x] print promoted to function — landed 2026-03-23, print/printn are real function calls, keywords removed from lexer
- [ ] UTF-8 decision locked — blocks stdlib and filesystem
- [x] String interpolation — landed 2026-03-23, `{varname}` expanded at print time
- [ ] Integer overflow behavior enforced — wrapping at declared width, not just at assignment
- [ ] Semicolon rule enforced consistently — optional everywhere, no context-dependent exceptions
- [ ] Parser, semantic layer, and interpreter agree on all supported constructs
- [ ] No known soundness holes in memory boundary model
- [ ] All examples in `examples/` pass
- [ ] Diagnostics readable for common mistakes
- [ ] Roadmap and spec match actual language behavior

### Quality Gates (strongly desired, delays release if missing)

- [ ] `:=` type inference for literals and simple expressions
- [x] `when` as value-producing expression — landed 2026-03-22
- [ ] Pattern matching — named binding `as v` and guard clauses `if n > 5`
- [ ] `NullPoint<T>` — nullable pointer mapping into unknown/known model
- [ ] Generics v3 — type bounds `T: Numeric`, `T: Known`
- [ ] Minimal stdlib — dynamic array, hashmap, basic string utilities
- [ ] Diagnostic improvements — better span reporting, actionable help text
- [x] Struct field type checking in semantic layer — fixed 2026-03-25, DotAccess resolves actual field types
- [x] Method call return type resolution — fixed 2026-03-25, method_registry resolves return types

---

## Done ✅

- Phase 1 — Functions
- Phase 2 — Free checker
- Phase 3 — Copy system (.copy, .copy.free, copy_into)
- Phase 4 — Bump allocator
- Phase 4b — True arena string storage
- Phase 5 — Handle<T> registry + language surface
- Phase 6a — when blocks
- Phase 6b — Ranges .. and ..=
- Phase 6c — Basic enums + variant matching
- Phase 6d — Loops + compound assigns + comparison operators
- Phase 6e — Flat grouped enums
- Phase 6f — Super-group enums + {_} placeholder
- Nested function name leakage bug fixed
- Forward function declarations
- Type::Str vs Type::StrRef split + boundary checker (Memory Boundary Rules v0.1)
- TBool + is_known(x) + Unknown state runtime
- Block comments /# ... #/
- Arrays — declaration, init, partial init, index read/write, function pass/return, copy semantics
- while in / then chaining — cursor iteration over arrays, full pipeline, t34/t35 passing
- if / else if / else statements — full pipeline lexer through runtime, t44/t45/t46
- Generics v1 — single type parameter, full pipeline parser to semantic to runtime
- Generics v2 — multiple type parameters on functions, t52/t53 passing
- Structs Phase 1+2 — definition, instantiation, field read/write, impl blocks, method dispatch, compound assign dot-access
- Multi-struct impl blocks — impl (p: Player, w: World), t43 passing, multi-alias writeback working
- Easy wins sprint — != operator, unary ! operator, process exit codes, .expected_fail marker system, run_matrix.sh test runner
- GitHub Actions CI — frontend matrix + backend tests + stale base gate
- CONTRIBUTING.md
- run_matrix.sh wired into CI — full matrix runs on every PR
- Semantic/interpreter parity complete — raw AST path (eval_expr + run_stmt) deleted, ~790 lines removed
- Copy semantics native in semantic path — bleed_back mechanism, no fallback to old AST path
- contains_return_stmt recursion fix — now detects returns inside if/else/while/for/loop branches
- f64 runtime support — Value::Float, SemanticType::F64, AstValue::Float all work. Surface keyword missing.

**Cleanup Sprint — Complete**
- u128 to i128 — negative numbers now work
- For-loop range — direct iteration, no Vec allocation
- Debug formatting gated behind debug_scope
- run_stmt takes &Stmt — eliminates loop body cloning
- seen and order on RunTime — cleared correctly, no accumulation
- run_stmt free function vs eval_expr method — structural inconsistency resolved

**Language Features Sprint — Complete (2026-03-22)**
- f64 type keyword — full pipeline lexer to runtime, t55 passing
- const declarations + pub keyword — literal-only, semantic rejects reassignment, t56/t57
- Value-producing when — full pipeline, t59 passing
- when block-body arms — verified with t58
- SemanticType::Void — void function call typing fixed

**IO + Generic Structs Sprint — Complete (2026-03-23)**
- String interpolation — `{varname}` expansion at print time in string literals
- `read(var)` and `input("prompt", var)` built-ins — stdin input
- Generic structs Phase 1 — `struct Foo<T> { field: T }` definition, type param resolution in fields
- Generic structs Phase 2 — `Pair<t32> { ... }` instantiation with explicit type args
- print/printn promoted to functions — keywords removed from lexer, parse as Expr::Call
- t42 TypeParam vs Struct ambiguity resolved — expected_fail removed
- Dead enum group infrastructure deleted — EnumRuntimeInfo, enums field, super_group_handler_index all removed

**Macro + Import Syntax Sprint — Complete (2026-03-24)**
- `#![imports]` block parsing — `alias: use "path"` syntax, `ImportDecl` AST node, `Stmt::ImportBlock`
- Import semantic validation — duplicate alias rejection, registry path rejection (only `./` and `std/` in v0.1)
- Outer macro system — `CxMacro` enum: Test, Inline, Reactive, Deprecated, Cfg, Unknown
- `#[test]`, `#[inline]`, `#[deprecated]` accepted on functions
- `#[reactive]` and `#[cfg]` reserved with post-v0.1 errors
- Unknown macro names rejected with locked diagnostics
- `#[test]` with return type rejected
- `macros: Vec<CxMacro>` on `Stmt::FuncDef`
- 9 new matrix tests (t65–t73) — 4 passing, 5 expected-fail
- Matrix at 72/72 green

**Code Quality Sprint — Complete (2026-03-22)**
- Arc<SemanticFunction> — function bodies stored as Arc, no clone on every call
- sem_err! macro — 51 SemanticError constructions collapsed to 1-line macro calls
- unsupported! + unsupported_type! macros in ir/lower.rs — 35 arms collapsed
- print_value + print_value_inline unified via value_to_string
- semantic_type_to_ast duplicate deleted — replaced with From<SemanticType> for Type
- Test matrix renumbered — 10 duplicate pairs fixed, 54 tests total
- Old AST interpreter deleted — eval_expr (~314 lines) + run_stmt (~462 lines) removed

---

## Active 🔄

- **Backend IR Phase 6** — function call lowering and validation. Stage 2b (direct call lowering with arity/type validation) and Stage 3 (cross-function call validation in IR validator) landed 2026-03-22. Loops, structs not yet lowered.
- **Backend ABI / Data Layout** — Phase 8 complete on submain: scalar layout (2026-03-27), struct layout with natural alignment and padding (7 tests), array layout fixed-size contiguous stride-based (5 tests), enum layout tag-only u8 (2026-03-28), calling convention locked (single return, C ABI). All compound type layouts locked for 0.1 in `cx_abi_v0.1.md`. Pending merge to main.
- **Backend IR Phase 10 — Loop Lowering** — While loop lowering landed on submain 2026-03-28: header/body/exit CFG, loop-carried SSA via block params, backedge, 3 Rust-level tests. First backend IR loop support. If/else lowering not yet started.
- **Generic structs follow-up** — Phase 1+2 landed. Remaining: type args in variable declarations (`p: Pair<t32>`), generic field type checking enforcement.
- **Multi-file imports** — `#![imports]` block parsing, semantic validation, full resolution pipeline (resolver, semantic merge, runtime dispatch) landed. PR #27 merged submain to main 2026-03-28. t74/t64 passing on main.

---

## Known Gaps — Tracked ⚠️

These are known issues with expected_fail markers. They do not block CI but need resolution before 0.1.

- ~~**t42 — TypeParam vs Struct ambiguity**~~ — resolved 2026-03-23, expected_fail removed. Print-as-function refactor eliminated the ambiguity.
- **t33 — Array index assign** — array index write (`arr:[i] = val`) not fully wired through semantic path. Arrays work for read, pass, and return but mutable index assign has gaps.
- **t32 — StrRef escape reject** — strref boundary checker rejects some valid patterns. Expected_fail while boundary rules are refined.
- **Struct field type checking** — `DotAccess` in semantic layer always returns `SemanticType::I128` regardless of actual field type. Non-existent fields not caught. *(Fixed on `submain` 2026-03-25 — DotAccess resolves actual field types.)*
- **Method call return type** — `MethodCall` in semantic layer returns `SemanticType::Unknown`. Type information lost at method call boundaries. *(Fixed on `submain` 2026-03-25 — method_registry resolves return types.)*
- ~~**`when` block-body arms**~~ — resolved 2026-03-22, t58 passing.
- **Integer overflow not fully enforced in arithmetic** — wrapping is the locked decision. Partial fix on submain (`d93582b` 2026-03-28): `saturating_add`/`sub` → `wrapping_add`/`sub`, guards for `i128::MIN / -1` and `i128::MIN % -1`. Arithmetic now wraps consistently at i128 range. Per-type-width wrapping at arithmetic time (e.g., t8 wrapping at 255 during addition) is still not implemented.
- **Semicolons** — rule locked as optional but parser behavior not yet fully consistent across all constructs.
- **`*arr` deref removed** — `apply_unary Op::Mul` on arrays returns `arr[0]`. This behavior is being removed in favor of explicit `arr:[0]`. Any code using `*arr` should migrate.

---

## Must Ship for 0.1 🔲

**Multi-File Imports**
- ~~#![import] block parsing~~ — landed 2026-03-24 (parser + semantic validation)
- Module resolution — actual file loading not yet implemented
- pub keyword enforcement — only marked declarations cross file boundaries
- Dead symbol elimination — only referenced symbols loaded
- Relative path resolution — ./player imports from player.cx
- Stdlib path resolution — std/math, std/string
- Circular import detection — compile error
- Project layout defined — where files live, how modules resolve

**Testing Infrastructure**
- assert(cond) — runtime error if condition is false
- assert_eq(a, b) — equality check with diagnostic output
- Test blocks — functions marked as test-only, skipped in release builds
- Test runner — cx test runs all test blocks
- Pass/fail output with error context

**Minimal Error Model**
- Result<T> direction locked
- Panic vs recoverable error boundary decided
- Integration with Unknown state — does an error produce Unknown or halt?
- Error propagation model — how errors bubble through call chains
- Basic diagnostic policy for type errors, boundary errors, unknown-state errors

**Diagnostics and Developer Experience**
- Clear parser error spans — line, column, what was expected
- Type mismatch diagnostics — what type was found, what was expected
- Unknown-state diagnostics — which value is unknown and where it entered
- Import resolution errors — file not found, symbol not found, circular import
- Struct/method resolution errors — field not found, method not found
- Boundary violation errors — strref escape, container boundary crossing
- Actionable help text where possible

~~**print Promoted to Function**~~
Done — landed 2026-03-23, checked off in Hard Blockers.

**UTF-8 Decision Locked**
Blocks stdlib. Blocks filesystem. Must be decided before either lands.

**String Model Finalized**
- str copy-on-boundary fully tested
- strref arena view confirmed working
- String interpolation — {varname} inline syntax in print()
- Substring without copy

---

## Strongly Desired for 0.1 🔲

**Generic Structs — Phase 1+2 landed**
Struct<T> definition and instantiation with explicit type args work. Remaining: type args in variable declarations, generic field type enforcement.

**NullPoint<T>**
Nullable pointer mapping into the unknown/known model. Game engines need nullable handles constantly.

**Generics v3 — Type Bounds**
T: Numeric, T: Known — aliases into the existing type hierarchy, not a new constraint system.
Design pass needed before implementation.

**Pattern Matching Completeness**
- Named binding in match arms (`SomeVariant as v`)
- Guard clauses (`n if n > 5`)
- Struct field destructuring in when arms — post-0.1

**Minimal Stdlib Core**
- Dynamic array — push, pop, len, capacity
- hashmap — key-value lookup
- hashset — existence checks
- Basic string utilities — split, join, contains, trim
- Result<T> once error model lands

**:= Type Inference**
After generics. Reduces declaration verbosity.

---

## Examples and Conformance Programs 🔲

A language release without examples is barely a release.

- hello world
- arrays — fixed and dynamic
- enums — basic, grouped, super-group
- when blocks — tbool, unknown, enum matching
- structs + methods
- generics — single and multiple type params
- multi-file program using imports
- Handle<T> usage
- memory boundary — str vs strref, what escapes and what doesn't
- test blocks — showing how to write tests in Cx
- failure examples — what errors look like and what they mean
- engine-facing starter — math/transform structs, entity-like structs, fixed array usage, Handle<T>

---

## 0.1 Syntax and Semantics Freeze

Before the release candidate is cut these are frozen. No breaking changes after this point.

- Core syntax — all existing keywords, operators, and constructs
- Memory boundary rules — Memory Boundary Rules v0.1
- Generic function syntax
- Import syntax — #![import] block, pub, use
- Struct and method surface
- Enum surface — basic, grouped, super-group, when matching
- Unknown state behavior — propagation rules, TBool, is_known

---

## Post-0.1 — Language Core 🔲

- gene + phen trait system — language identity feature, not optional flavor. Design pass needed now even though implementation is later. Defines how operator overloading, bounded polymorphism, and the stdlib are structured.
- Operator overloading — blocked on gene/phen. Vector3 + Vector3 is not a nice-to-have in a game engine language.
- Full pattern matching — array patterns, nested patterns
- Labeled breaks for nested loops
- Ternary / value-producing if
- Closures and lambdas — design pass needed
- Async and continuations — design pass needed
- Reflection / type introspection
- C interop — nearly free if Cx emits C as a compilation target

---

## Post-0.1 — Runtime and Stdlib 🔲

After imports, structs, generics, and the string model are all locked.

**Collections — Three Core Types**

Three collection types covering every relationship between data: existence, connection, and full system interconnection.

- hashset — unique values, no keys, no duplicates, fast existence checks
- hashmap — key-value pairs, hashed lookup
- hashweb — first-class graph collection. Nodes, bidirectional edges, one-way edges, node aliases, queryable paths. The most distinctive collection in Cx — models how entire systems interconnect.

```cx
world = hashweb [
    "player"  <=> "inventory" ::inv,
    "items"   =>  player.inv,
    "player"  <=> "faction"  ::fac,
    "quest"   =>  player.fac,
]
```

- `<=>` bidirectional edge
- `=>` one-way edge
- `::name` alias a node for referencing elsewhere in the web
- Design pass needed — traversal API, path queries, cycle detection

**More Collections**
- Dynamic array / Vec<T> — runtime-sized, push/pop
- Ring buffer — fixed capacity, wrap-around
- Queue — push to back, pop from front
- Stack — push, pop, peek
- LinkedList<T> — O(1) insert/remove at cursor
- TreeMap<K, V> — ordered, sorted iteration

**Algorithms**
- Binary search, quicksort, merge sort
- String utilities — split, join, contains, trim, starts_with, ends_with

**Memory System Completion**
- Phase 5b — region_id bulk handle invalidation on arena reset
- Handle-backed containers — unlocks container boundary crossing
- rc<T> — single-threaded shared ownership
- shared<T> — multi-threaded shared ownership
- Reference cycle handling — design pass needed

---

## Post-0.1 — Filesystem I/O 🔲

File handles use Handle<T> internally — arena-managed, explicit open/close, stale access is a runtime error.

- open, close, read_line, read_all, write, write_line, append
- exists, delete, create, mkdir, list_dir, is_dir
- Primitive file generation — txt, csv, json via string formatting, binary buffers
- Parse csv into arrays
- Parse json into struct trees — design pass needed

---

## Post-0.1 — Engine Systems 🔲

These are what Cx is ultimately for. They are not 0.1 scope. They are why 0.1 needs to be solid.

**Window and Screen System**
- load_image, save_image — PNG, JPG, BMP
- Image struct — width, height, pixel data, Color type
- open_window, close_window — native OS window via Handle<Window>
- blit, clear, present, draw_rect, draw_text
- Event loop — poll_events, wait_event
- Event enum — KeyDown, KeyUp, MouseMove, MouseClick, WindowClose
- Headless mode — render to image buffer without display
- Backend targets — Win32, Cocoa, X11/Wayland

**GPU System**
- VRAM registry
- GS types
- .drop(fence)
- GPU memory lifetime model
- GPU-accelerated rendering path — connects into window system

**Audio System**
Deferred until window system lands.

**Networking**
TCP/UDP sockets. Deferred until filesystem I/O is proven.

---

## Tooling 🔲

- CLI — cx build, cx run, cx test, cx check
- CLI visualizer
- Ricey registry server
- Cranelift JIT backend (Phase E)
- LLVM AOT backend
- LSP — post-0.1

---

## Design Backlog 🔲

These need active design work before any implementation can begin.

- gene + phen full design — keep this active, not passive. It defines too much of the language to leave sitting.
- 2D/3D/4D arrays — flat + manual indexing is the game engine pattern, but native syntax is worth designing
- Async / continuations / lambdas
- Closures
- Reflection / type introspection
- C interop FFI surface design
- Package manager integration with Ricey
- hashweb traversal API and query language design

---

## Key Changes from v4.0

- Release gates split into two honest tiers — hard blockers and quality gates
- Hard blockers are the real finish line — seven conditions that must all be true
- Quality gates are tracked plans, not veto conditions
- Cleanup sprint remaining items moved to Done — seen/order and run_stmt both resolved this session
- CI matrix gate added — run_matrix.sh wired into GitHub Actions is a hard blocker
- Generics v2 status flagged for confirmation before doc goes out
- Version bumped to v4.1

## Key Changes from v4.1

- Semantic/interpreter parity marked COMPLETE — raw AST path deleted (~790 lines)
- Generics v2 marked COMPLETE — multiple type params confirmed working
- Multi-struct impl blocks moved from Post-0.1 to Done — already implemented
- Code quality sprint added to Done — Arc, macros, dead code removal
- Known Gaps section added — t42, t33, t32 tracked with expected_fail
- 4 of 7 hard blockers now resolved (parity, generics, structs, CI)
- 3 hard blockers remain: multi-file imports, print-as-function, UTF-8
- Test matrix at 78 tests, 78/78 green
- Version bumped to v4.2

## Key Changes from v4.7

- Active section updated: Phase 8 ABI/data layout marked complete on submain (struct, array, enum, calling convention all locked)
- Phase 10 (while loop lowering) added to Active — first backend IR loop support on submain
- Multi-file imports updated — PR #27 merged submain to main 2026-03-28
- Integer overflow Known Gap updated — partial fix (wrapping consistent at i128, per-width still open)
- Wrapping arithmetic fix on submain: saturating → wrapping, i128::MIN edge cases guarded
- Matrix holds at 78/78 green on main
- Version bumped to v4.8

## Key Changes from v4.6

- Backend IR Phase 7 debugging tools + Phase 0.5 backend trait refactor merged to main — IR is now the sole backend interface
- Module resolver (`resolver.rs`) landed on submain — full dependency graph, topo-sort, circular import detection, `ExportTable` foundation
- Site syntax docs updated to match frozen spec (site branch)
- Active section updated for both backend IR and multi-file imports progress
- Matrix holds at 72/72 green, no regressions
- Version bumped to v4.7

## Key Changes from v4.5

- Macro + Import Syntax Sprint added to Done
- `#![imports]` block parsing and semantic validation landed — import syntax is real, file resolution is next
- Outer macro system landed — `#[test]`, `#[inline]`, `#[deprecated]`, `#[reactive]` (reserved), `#[cfg]` (reserved)
- Multi-file imports in Must Ship updated — block parsing checked off, module resolution still open
- Multi-file imports added to Active section — syntax done, resolution remaining
- print Promoted to Function in Must Ship marked done (was already in Hard Blockers)
- 9 new matrix tests (t65–t73), matrix at 72/72 green
- Version bumped to v4.6

## Key Changes from v4.4

- Generic structs Phase 1+2 checked off as hard blocker (with noted gaps)
- read/input checked off as hard blocker
- print promoted to function checked off as hard blocker
- String interpolation checked off as hard blocker
- t42 TypeParam vs Struct ambiguity resolved — removed from Known Gaps
- IO + Generic Structs Sprint added to Done
- Generic Structs in "Strongly Desired" updated to reflect partial completion
- Test matrix at 63 tests, 63/63 green
- Version bumped to v4.5

## Key Changes from v4.3

- f64, const, value-producing when, when block-body arms all checked off as hard blockers
- Language Features Sprint added to Done
- Backend IR Phase 6 updated — Stages 2b/3 (function call lowering + validation) landed
- f64 surface keyword removed from Active (complete)
- when block-body arms removed from Known Gaps (resolved)
- when as value-producing expression checked off in Quality Gates
- Test matrix at 78 tests, 78/78 green
- Version bumped to v4.4

## Key Changes from v4.2

- Syntax Decisions — Locked section added — all frozen decisions in one place
- Release gates completely rewritten — honest hard blockers and quality gates
- Active section updated — only genuinely active work remains
- Known Gaps expanded — struct field types, method return types, overflow, semicolons tracked
- Multi-struct impl blocks removed from Post-0.1 — already in Done
- `*arr` deref marked for removal — `arr:[0]` is the canonical syntax
- f64 runtime support added to Done — surface keyword is the remaining work
- Version bumped to v4.3

## Key Changes from v4.4

- Struct field type checking marked complete — DotAccess resolves real field types
- Method call return type marked complete — method_registry resolves return types
- Test matrix at 78 tests, 78/78 green
- Multi-file imports fully wired — resolver, semantic merge, runtime dispatch
- Five correctness bugs fixed from audit
- Dead code cleanup — Print/PrintInline, Range, Placeholder, wait_for_step removed
- Output verification added to matrix runner via .expected_output sidecars
- Version bumped to v4.5
