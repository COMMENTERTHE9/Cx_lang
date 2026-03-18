# Cx Language Roadmap
v4.1 — 2026-03-17

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

## 0.1 Release Gates

These are not features. These are conditions. A long gate list that never closes is a project killer — so the gates are split into two honest tiers.

**Hard blockers — 0.1 cannot ship without these:**
- Semantic/interpreter parity complete — interpreter runs off SemanticProgram, raw AST path removed
- Multi-file imports working — programs can span multiple .cx files
- Generics v2 complete — multiple type parameters on functions (confirm current status before publishing)
- Structs, methods, impl blocks working end to end
- print promoted to function — must happen before backend locks calling convention
- UTF-8 decision locked
- CI runs the full matrix on every PR and must be green — run_matrix.sh wired into GitHub Actions

**Quality gates — must be true or have a tracked plan before 0.1:**
- Parser, semantic layer, and interpreter agree on all supported constructs — no silent behavioral divergence
- No known soundness holes in the memory boundary model
- Minimal error model in place — Result<T> direction locked, panic vs recoverable decided
- Basic test runner exists — assert, assert_eq, test blocks
- Diagnostics are readable for common mistakes — parser errors, type mismatches, boundary violations
- All examples in examples/ pass
- Roadmap and spec match actual language behavior

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
- while in / then chaining — cursor iteration over arrays
- if / else if / else statements
- Generics v1 — single type parameter, full pipeline parser to semantic to runtime
- Structs Phase 1+2 — definition, instantiation, field read/write, impl blocks, method dispatch, compound assign dot-access (on submain)
- Easy wins sprint — != operator, unary ! operator, process exit codes, .expected_fail marker system, run_matrix.sh test runner (on submain)
- GitHub Actions CI — frontend matrix + backend tests + stale base gate
- CONTRIBUTING.md
- run_matrix.sh wired into CI — full matrix runs on every PR

**Cleanup Sprint — Complete**
- u128 to i128 — negative numbers now work
- For-loop range — direct iteration, no Vec allocation
- Debug formatting gated behind debug_scope
- run_stmt takes &Stmt — eliminates loop body cloning
- seen and order on RunTime — cleared correctly, no accumulation
- run_stmt free function vs eval_expr method — structural inconsistency resolved

---

## Active — 0.1 Work 🔄

**1 — Semantic/Interpreter Parity** — most important technical blocker
- [x] SemanticStmt at full parity with raw AST interpreter — landed on submain 2026-03-17
- [x] StructDef, ImplBlock, MethodCall wired in semantic layer — landed on submain 2026-03-17
- Interpreter runs off SemanticProgram
- Raw AST interpretation path removed
- Until this is done, everything else is built on unstable ground

**2 — Generics v2**
- Multiple type parameters: fnc swap<A, B>(a: A, b: B)
- Confirm current status with owning dev before this doc goes out — may already be landed

**3 — Backend IR**
- Function lowering in progress
- if/else lowering next
- Cranelift stubs, not yet functional
- Backend does not block 0.1 frontend release — but must stay in sync

---

## Must Ship for 0.1 🔲

**Multi-File Imports**
- #![import] block parsing and module resolution
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

**print Promoted to Function**
Must happen before the backend locks calling convention.

**UTF-8 Decision Locked**
Blocks stdlib. Blocks filesystem. Must be decided before either lands.

**String Model Finalized**
- str copy-on-boundary fully tested
- strref arena view confirmed working
- String interpolation — {varname} inline syntax in print()
- Substring without copy

---

## Strongly Desired for 0.1 🔲

**Generic Structs**
Struct<T> unlocks a large amount of useful code. Lands after structs parity and generics v2.

**NullPoint<T>**
Nullable pointer mapping into the unknown/known model. Game engines need nullable handles constantly.

**Generics v3 — Type Bounds**
T: Numeric, T: Known — aliases into the existing type hierarchy, not a new constraint system.
Design pass needed before implementation.

**Pattern Matching Completeness**
- Struct field destructuring in when arms
- Binding in match arms
- Guard clauses

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
- Multi-struct impl blocks — impl (p: Player, w: World)
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
